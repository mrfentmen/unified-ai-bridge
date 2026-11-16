use crate::catalog::Category;
use crate::protocol::{BLOCK_TAG, RESULT_CLOSE, RESULT_OPEN};
use crate::provider::Agent;
use crate::workspace::{PermissionMode, Profile};

/// Instructions injected into the conversation as a normal user message when
/// the user connects a tab. They are the only place the model learns the tool
/// protocol; there are no hidden endpoints.
pub fn build_instructions(agent: Agent, workspace: &str, provider: &str) -> String {
    build_profile_instructions(
        agent,
        workspace,
        provider,
        Profile::Research,
        PermissionMode::Standard,
        false,
    )
}

pub fn build_profile_instructions(
    agent: Agent,
    workspace: &str,
    provider: &str,
    profile: Profile,
    mode: PermissionMode,
    read_only: bool,
) -> String {
    let mut out = String::new();
    out.push_str("# Unified AI Bridge — local tools\n\n");
    out.push_str(&format!(
        "You are **{agent}**, connected through the visible {provider} website to a local Rust bridge on the user's computer. \
Your active workspace root is `{workspace}`. The bridge gives you real, permission-checked local tools. Nothing here is simulated: \
when a tool reports success, a real file operation happened; when it fails, the error is real.\n\n"
    ));
    out.push_str("## How to call tools\n\n");
    if agent == Agent::Qwen {
        out.push_str("Qwen display requirement: emit each tool request as ONE plain-text line using the exact sentinels [[BRIDGE_TOOL]] and [[/BRIDGE_TOOL]] around strict compact JSON. Do not use Markdown fences, backticks, lists or code editors for tool calls: this website can hide code lines. Example: [[BRIDGE_TOOL]]{\"tool\":\"list_files\",\"arguments\":{\"path\":\".\"}}[[/BRIDGE_TOOL]]. The following fenced format describes the shared protocol; on Qwen prefer the plain-text sentinel form.\n\n");
    }
    out.push_str(
        "Use one fenced code block tagged `bridge-tool` per call. Each block contains exactly one JSON object with `tool` and `arguments`. \
The JSON must be strict: no comments, no trailing commas, and no `agent` or `provider` field (identity is fixed by the registered browser tab). \
If your interface strips the code fence, a line containing exactly `bridge-tool` immediately followed by the JSON object is also accepted.\n\n",
    );
    let example_path = if profile == Profile::Research {
        "PROJECT"
    } else {
        "."
    };
    out.push_str(&format!("```{BLOCK_TAG}\n{{\"tool\":\"list_files\",\"arguments\":{{\"path\":\"{example_path}\"}}}}\n```\n\n"));
    out.push_str(
        "Some chat interfaces hide a code block's language label. If `bridge-tool` disappears from the rendered page, a code block containing only the JSON object is also accepted; the bridge validates the tool name and arguments before doing anything.\n\n",
    );
    out.push_str("The bridge executes the calls and returns a result in this form:\n\n");
    out.push_str(&format!(
        "{RESULT_OPEN}\n{{\"tool\":\"list_files\",\"status\":\"ok\",\"summary\":\"...\",\"output\":\"...\"}}\n{RESULT_CLOSE}\n\n"
    ));
    out.push_str(
        "Do not invent result blocks, do not claim a tool ran before you receive its result, and never place secrets in arguments. \
Wait for the result before drawing conclusions. If a call is denied, explain the denial and ask the user to approve or adjust the request. \
A permission denial is not evidence that a file does not exist.\n\n",
    );
    out.push_str("## Workspace rules\n\n");
    out.push_str(&format!(
        "Profile: {}. Permission mode: {mode:?}. Read-only: {read_only}.\n\n",
        profile.name()
    ));
    if profile == Profile::Research {
        out.push_str(&format!(
        "- Your writable workspace is `AGENTS/{}/` (notes, findings, experiments, reports, logs).\n\
- You may read `PROJECT/`, approved `SHARED/` files, `COLLABORATION/`, `VERIFICATION/`, your own workspace, and other agents' published `findings/`.\n\
- You cannot write another agent's workspace. Never overwrite another agent's research.\n\
- `SHARED/datasets/` is immutable. Do not attempt to modify original measurement data.\n\
- Writes to shared documentation, project files, and deletions require the user's approval. Shell execution is prohibited; only user-approved named scripts can run.\n\
- Use relative paths (`AGENTS/{}/findings/example.md`), not absolute paths.\n\
- Record evidence, uncertainty, and negative results honestly. Agreement between models is not proof.\n\n",
        agent.dir(), agent.dir()
    ));
        out.push_str("## Collaboration\n\n");
        out.push_str(
        "Remain independent. You may read other agents' findings, submit proposals, peer reviews, and disagreements. \
Keep your own attribution and timestamps. Do not merge all agents into one conclusion; compare evidence and preserve competing explanations.\n\n",
    );
    } else {
        out.push_str("- Paths are relative to the active root; absolute paths are accepted only inside that root. Traversal and symlink escapes are denied.\n- You may inspect and edit ordinary UTF-8 files throughout this workspace. There are no per-agent silos.\n- `.bridge/` metadata and `.git/` internals are protected. Binary formats require an appropriate application tool; do not overwrite them with text.\n- Strict mode requires approval for every write. Standard mode permits writes inside the granted root. Deletes always require approval. A locked workspace rejects all writes.\n- Commands must come from the workspace's explicit allowlist; shell strings, pipes and redirection are unavailable.\n- Research and collaboration tools are unavailable in this profile.\n\n");
    }
    out.push_str("Read before editing: supply the returned sha256 as expected_sha256 for replacements, edits, appends and deletes. A file_changed error means read and preview again, never force overwrite. New files omit expected_sha256. Use preview_document to inspect changes, list_changes to find recovery IDs, and restore_change to request an approved restore.\n\n");
    out.push_str("## Available tools\n\n");
    for category in Category::ALL {
        if !profile.tools().any(|tool| tool.category == category) {
            continue;
        }
        out.push_str(&format!("### {}\n", category.label()));
        for tool in profile.tools().filter(|tool| tool.category == category) {
            let write = if tool.writes { " [writes]" } else { "" };
            out.push_str(&format!(
                "- `{}`{} — {}\n  Arguments: {}\n",
                tool.name, write, tool.description, tool.arguments
            ));
        }
        out.push('\n');
    }
    out.push_str(
        "Connection setup is not a task. Acknowledge the connection and wait for an explicit user task; do not call tools for orientation. \
Use tools only to fulfill that task. A tool result is not a new task. Stop when the task is complete, denied, or a call budget is exceeded; \
give the user a concise summary with paths to files you created. Do not start follow-up work unless asked.\n",
    );
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn instructions_contain_protocol_and_every_tool() {
        let text = build_instructions(Agent::ChatGpt, "/tmp/workspace", "ChatGPT");
        assert!(text.contains(BLOCK_TAG));
        assert!(text.contains(RESULT_CLOSE));
        for tool in crate::catalog::TOOLS {
            assert!(text.contains(tool.name), "missing {}", tool.name);
        }
        assert!(text.contains("AGENTS/CHATGPT"));
        assert!(!text.contains("agent\":"));
    }
}
