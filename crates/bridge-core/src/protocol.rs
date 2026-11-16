use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::catalog;
use crate::error::CoreError;

pub const TOOL_OPEN: &str = "[[BRIDGE_TOOL]]";
pub const TOOL_CLOSE: &str = "[[/BRIDGE_TOOL]]";
pub const RESULT_OPEN: &str = "[[BRIDGE_RESULT]]";
pub const RESULT_CLOSE: &str = "[[/BRIDGE_RESULT]]";
/// Structured, fence-independent tool block tag. ChatGPT's current web UI
/// strips Markdown fences from transcript text, so the tag must also work as a
/// bare line followed by one strict JSON object.
pub const BLOCK_TAG: &str = "bridge-tool";
pub const MAX_BLOCK_BYTES: usize = 256 * 1024;

/// One tool request. There is deliberately no agent/provider field: identity is
/// bound to the registered browser session by the native host.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ToolCall {
    pub tool: String,
    #[serde(default)]
    pub arguments: Value,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ParsedCall {
    pub index: usize,
    pub call: ToolCall,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ParseProblem {
    pub block: usize,
    pub message: String,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize)]
pub struct ParsedMessage {
    pub calls: Vec<ParsedCall>,
    pub problems: Vec<ParseProblem>,
}

impl ParsedMessage {
    pub fn is_empty(&self) -> bool {
        self.calls.is_empty() && self.problems.is_empty()
    }

    pub fn has_tool_request(&self) -> bool {
        !self.calls.is_empty()
    }
}

/// Parse only the explicit bridge sentinels. Natural-language phrases never
/// trigger execution on their own.
pub fn parse_tool_requests(text: &str) -> ParsedMessage {
    let mut parsed = ParsedMessage::default();
    let mut cursor = 0usize;
    let mut block = 0usize;
    while let Some(relative_start) = text[cursor..].find(TOOL_OPEN) {
        let start = cursor + relative_start;
        let body_start = start + TOOL_OPEN.len();
        let Some(relative_end) = text[body_start..].find(TOOL_CLOSE) else {
            // Models sometimes emit only the opening sentinel after receiving a
            // result. Treat a bare marker as an incomplete/no-op block rather
            // than a malformed request; a non-empty body is still an error.
            if text[body_start..].trim().is_empty() {
                break;
            }
            parsed.problems.push(ParseProblem {
                block,
                message: "tool request has an opening marker but no closing marker".into(),
            });
            break;
        };
        let end = body_start + relative_end;
        let body = text[body_start..end].trim();
        if body.len() > MAX_BLOCK_BYTES {
            parsed.problems.push(ParseProblem {
                block,
                message: "tool request is larger than the 256 KiB limit".into(),
            });
        } else if body.is_empty() {
            parsed.problems.push(ParseProblem {
                block,
                message: "tool request block is empty".into(),
            });
        } else {
            match serde_json::from_str::<ToolCall>(body) {
                Ok(call) => match validate_call(&call) {
                    Ok(()) => parsed.calls.push(ParsedCall { index: block, call }),
                    Err(err) => parsed.problems.push(ParseProblem {
                        block,
                        message: err.to_string(),
                    }),
                },
                Err(err) => parsed.problems.push(ParseProblem {
                    block,
                    message: format!("tool request is not a valid JSON object: {err}"),
                }),
            }
        }
        block += 1;
        cursor = end + TOOL_CLOSE.len();
        if cursor >= text.len() {
            break;
        }
    }
    parse_tagged_blocks(text, &mut parsed, &mut block);
    // Gemini renders a fenced code block's language tag as a "Code snippet"
    // header, so the literal `bridge-tool` label can disappear while the JSON
    // survives. Accept a strict ToolCall-shaped JSON object as a structured
    // fallback; arbitrary JSON and unknown tools are still rejected.
    if parsed.calls.is_empty() && parsed.problems.is_empty() {
        parse_bare_json_objects(text, &mut parsed);
    }
    parsed
}

fn parse_bare_json_objects(text: &str, parsed: &mut ParsedMessage) {
    let mut cursor = 0usize;
    while let Some(relative) = text[cursor..].find('{') {
        let start = cursor + relative;
        let Some(json) = extract_json_object(text, start) else {
            break;
        };
        cursor = start + json.len();
        if json.len() > MAX_BLOCK_BYTES {
            continue;
        }
        let Ok(call) = serde_json::from_str::<ToolCall>(json) else {
            continue;
        };
        if validate_call(&call).is_err() {
            continue;
        }
        parsed.calls.push(ParsedCall {
            index: parsed.calls.len(),
            call,
        });
    }
}

/// Parse the fence-independent `bridge-tool` form produced by current model
/// interfaces, including when Markdown fences are stripped from `innerText`.
fn parse_tagged_blocks(text: &str, parsed: &mut ParsedMessage, block: &mut usize) {
    let mut cursor = 0usize;
    while let Some(relative) = text[cursor..].find(BLOCK_TAG) {
        let start = cursor + relative;
        let after_tag = start + BLOCK_TAG.len();
        cursor = after_tag;
        // ChatGPT and DeepSeek glue their accessibility/code-block labels
        // directly to the tag, e.g. `ChatGPTbridge-tool{...}` or
        // `bridge-toolCopyDownload{...}`. Do not require a boundary before or
        // after the tag; the JSON/chrome check below is what validates it.
        let rest = &text[after_tag..];
        let Some(brace_relative) = rest.find('{') else {
            // A prose mention such as "the bridge-tool protocol" is not a
            // malformed request; ignore it.
            continue;
        };
        let between = rest[..brace_relative].trim();
        if !between.is_empty() && !between.starts_with("```") && !is_block_chrome(between) {
            // Prose that merely mentions the tag is not a tool request.
            continue;
        }
        let brace = after_tag + brace_relative;
        let Some(json) = extract_json_object(text, brace) else {
            parsed.problems.push(ParseProblem {
                block: *block,
                message: "bridge-tool block has an unterminated JSON object".into(),
            });
            *block += 1;
            break;
        };
        cursor = brace + json.len();
        if json.len() > MAX_BLOCK_BYTES {
            parsed.problems.push(ParseProblem {
                block: *block,
                message: "bridge-tool request is larger than the 256 KiB limit".into(),
            });
            *block += 1;
            continue;
        }
        match serde_json::from_str::<ToolCall>(json) {
            Ok(call) => match validate_call(&call) {
                Ok(()) => parsed.calls.push(ParsedCall {
                    index: *block,
                    call,
                }),
                Err(err) => parsed.problems.push(ParseProblem {
                    block: *block,
                    message: err.to_string(),
                }),
            },
            Err(err) => parsed.problems.push(ParseProblem {
                block: *block,
                message: format!("bridge-tool JSON is invalid: {err}"),
            }),
        }
        *block += 1;
        if cursor >= text.len() {
            break;
        }
    }
}

/// Code blocks in DeepSeek/Claude/Gemini render action labels inside the
/// assistant turn. Their text content appears between the tag and the JSON, so
/// a short, label-only gap must not make an otherwise valid request invisible.
fn is_block_chrome(between: &str) -> bool {
    const LABELS: &[&str] = &[
        "copy code",
        "download code",
        "open code",
        "copy",
        "download",
        "edit",
        "run",
        "open",
        "share",
        "more",
        "json",
        "code",
    ];
    let mut remaining = between.to_ascii_lowercase();
    for label in LABELS {
        remaining = remaining.replace(label, "");
    }
    remaining.trim().trim_matches('`').trim().is_empty()
}

fn extract_json_object(text: &str, start: usize) -> Option<&str> {
    let bytes = text.as_bytes();
    if bytes.get(start) != Some(&b'{') {
        return None;
    }
    let mut depth = 0usize;
    let mut in_string = false;
    let mut escaped = false;
    for (offset, byte) in bytes[start..].iter().enumerate() {
        if in_string {
            if escaped {
                escaped = false;
            } else if *byte == b'\\' {
                escaped = true;
            } else if *byte == b'"' {
                in_string = false;
            }
            continue;
        }
        match *byte {
            b'"' => in_string = true,
            b'{' => depth += 1,
            b'}' => {
                depth = depth.saturating_sub(1);
                if depth == 0 {
                    let end = start + offset + 1;
                    return Some(&text[start..end]);
                }
            }
            _ => {}
        }
    }
    None
}

fn validate_call(call: &ToolCall) -> Result<(), CoreError> {
    let name = call.tool.trim();
    if name.is_empty() || name.len() > 80 {
        return Err(CoreError::InvalidToolRequest(
            "tool name is empty or too long".into(),
        ));
    }
    if name != call.tool {
        return Err(CoreError::InvalidToolRequest(
            "tool name has surrounding whitespace".into(),
        ));
    }
    if catalog::find(name).is_none() {
        return Err(CoreError::UnknownTool(name.to_string()));
    }
    if !call.arguments.is_null() && !call.arguments.is_object() {
        return Err(CoreError::InvalidToolRequest(
            "arguments must be a JSON object (or omitted)".into(),
        ));
    }
    if call.note.as_ref().is_some_and(|note| note.len() > 4_000) {
        return Err(CoreError::InvalidToolRequest("note is too long".into()));
    }
    Ok(())
}

/// Format a single tool result as a sentinel block. `payload` is the host's
/// JSON result object and is embedded as a string so Markdown rendering cannot
/// change its meaning.
pub fn format_tool_result(payload: &Value) -> String {
    let rendered =
        serde_json::to_string(payload).unwrap_or_else(|_| "{\"status\":\"error\"}".into());
    format!("{RESULT_OPEN}\n{rendered}\n{RESULT_CLOSE}")
}

/// Format several results as one reply message, in call order.
pub fn format_tool_results(payloads: &[Value]) -> String {
    let mut out = String::from(crate::RESULT_BANNER);
    out.push_str("\n\n");
    for payload in payloads {
        out.push_str(&format_tool_result(payload));
        out.push('\n');
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_a_valid_call() {
        let text = "Working on it.\n[[BRIDGE_TOOL]]\n{\"tool\":\"read_file\",\"arguments\":{\"path\":\"PROJECT/PROJECT_CONTEXT.md\"}}\n[[/BRIDGE_TOOL]]";
        let parsed = parse_tool_requests(text);
        assert!(parsed.problems.is_empty());
        assert_eq!(parsed.calls.len(), 1);
        assert_eq!(parsed.calls[0].call.tool, "read_file");
    }

    #[test]
    fn parses_tagged_bridge_tool_without_fences() {
        let text =
            "Working.\nbridge-tool\n{\"tool\":\"list_files\",\"arguments\":{\"path\":\"PROJECT\"}}";
        let parsed = parse_tool_requests(text);
        assert!(parsed.problems.is_empty(), "{:?}", parsed.problems);
        assert_eq!(parsed.calls.len(), 1);
        assert_eq!(parsed.calls[0].call.tool, "list_files");
        assert_eq!(parsed.calls[0].call.arguments["path"], "PROJECT");
    }

    #[test]
    fn parses_tagged_bridge_tool_glued_to_chatgpt_label() {
        let text = "ChatGPT said:ChatGPTbridge-tool{\"tool\":\"list_files\",\"arguments\":{\"path\":\"PROJECT\"}}";
        let parsed = parse_tool_requests(text);
        assert!(parsed.problems.is_empty(), "{:?}", parsed.problems);
        assert_eq!(parsed.calls.len(), 1);
        assert_eq!(parsed.calls[0].call.tool, "list_files");
    }

    #[test]
    fn parses_fenced_tagged_bridge_tool() {
        let text = "```bridge-tool\n{\"tool\":\"read_file\",\"arguments\":{\"path\":\"PROJECT/PROJECT_CONTEXT.md\"}}\n```";
        let parsed = parse_tool_requests(text);
        assert!(parsed.problems.is_empty(), "{:?}", parsed.problems);
        assert_eq!(parsed.calls.len(), 1);
        assert_eq!(parsed.calls[0].call.tool, "read_file");
    }

    #[test]
    fn ignores_prose_mentioning_bridge_tool() {
        let parsed = parse_tool_requests("The bridge-tool protocol is documented here.");
        assert!(parsed.is_empty());
    }

    #[test]
    fn ignores_plain_natural_language() {
        let parsed = parse_tool_requests("Please read PROJECT_CONTEXT.md using read_file.");
        assert!(parsed.is_empty());
    }

    #[test]
    fn rejects_unknown_tools_extra_fields_and_missing_close() {
        let unknown = "[[BRIDGE_TOOL]]{\"tool\":\"shell\",\"arguments\":{\"cmd\":\"rm -rf /\"}}[[/BRIDGE_TOOL]]";
        assert_eq!(parse_tool_requests(unknown).calls.len(), 0);
        assert_eq!(parse_tool_requests(unknown).problems.len(), 1);

        let spoof = "[[BRIDGE_TOOL]]{\"tool\":\"read_file\",\"agent\":\"CLAUDE\",\"arguments\":{}}[[/BRIDGE_TOOL]]";
        assert_eq!(parse_tool_requests(spoof).calls.len(), 0);

        let dangling = "[[BRIDGE_TOOL]]{\"tool\":\"read_file\"}";
        assert_eq!(parse_tool_requests(dangling).problems.len(), 1);
    }

    #[test]
    fn parses_bare_json_tool_object_when_the_site_strips_the_tag() {
        let text = "Code snippet\n{\"tool\":\"list_files\",\"arguments\":{\"path\":\"PROJECT\"}}";
        let parsed = parse_tool_requests(text);
        assert!(parsed.problems.is_empty(), "{:?}", parsed.problems);
        assert_eq!(parsed.calls.len(), 1);
        assert_eq!(parsed.calls[0].call.tool, "list_files");
    }

    #[test]
    fn ignores_bare_json_that_is_not_a_tool_call() {
        let parsed = parse_tool_requests("An example result: {\"status\":\"ok\"}");
        assert!(parsed.is_empty(), "{:?}", parsed);
    }

    #[test]
    fn parses_tagged_block_with_concatenated_code_chrome() {
        let text =
            "bridge-toolCopyDownload{\"tool\":\"list_files\",\"arguments\":{\"path\":\"PROJECT\"}}";
        let parsed = parse_tool_requests(text);
        assert!(parsed.problems.is_empty(), "{:?}", parsed.problems);
        assert_eq!(parsed.calls.len(), 1);
        assert_eq!(parsed.calls[0].call.tool, "list_files");
    }

    #[test]
    fn parses_tagged_block_with_rendered_code_chrome() {
        let text = "bridge-tool\nCopy\nDownload\n{\"tool\":\"list_files\",\"arguments\":{\"path\":\"PROJECT\"}}";
        let parsed = parse_tool_requests(text);
        assert!(parsed.problems.is_empty(), "{:?}", parsed.problems);
        assert_eq!(parsed.calls.len(), 1);
        assert_eq!(parsed.calls[0].call.tool, "list_files");
    }

    #[test]
    fn ignores_a_bare_opening_marker() {
        let parsed = parse_tool_requests("ChatGPT said:\n\n[[BRIDGE_TOOL]]");
        assert!(parsed.is_empty(), "{:?}", parsed);
    }

    #[test]
    fn result_is_machine_readable() {
        let body = format_tool_result(&serde_json::json!({"status":"ok","summary":"one file"}));
        assert!(body.starts_with(RESULT_OPEN));
        assert!(body.contains("\"status\":\"ok\""));
        assert!(body.ends_with(RESULT_CLOSE));
    }

    #[test]
    fn parsing_does_not_hide_calls_over_the_execution_budget() {
        let block = "{\"tool\":\"list_files\",\"arguments\":{\"path\":\"PROJECT\"}}";
        let parsed = parse_tool_requests(&vec![block; 10].join("\n"));
        assert_eq!(
            parsed.calls.len(),
            10,
            "the host must see overflow and return a budget error"
        );
    }
}
