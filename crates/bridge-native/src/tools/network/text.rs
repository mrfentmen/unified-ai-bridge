//! Bounded streaming tokenization, not a DOM renderer or a readability claim.
use super::invalid;
use crate::failure::ToolFailure;
use html5ever::{
    buffer_queue::BufferQueue,
    tokenizer::{states::RawKind, StartTag, Token, TokenSink, TokenSinkResult, Tokenizer},
};
use std::cell::RefCell;

struct State {
    output: String,
    limit: usize,
    truncated: bool,
    skip: Option<String>,
}
impl State {
    fn push(&mut self, text: &str) {
        if self.skip.is_some() || self.truncated {
            return;
        }
        for ch in text.chars() {
            let ch = if ch.is_whitespace() { ' ' } else { ch };
            if ch.is_control()
                || (ch == ' ' && (self.output.is_empty() || self.output.ends_with(' ')))
            {
                continue;
            }
            if self.output.len() + ch.len_utf8() > self.limit {
                self.truncated = true;
                return;
            }
            self.output.push(ch);
        }
    }
}
struct Sink(RefCell<State>);
impl TokenSink for Sink {
    type Handle = ();
    fn process_token(&self, token: Token, _line: u64) -> TokenSinkResult<()> {
        let mut s = self.0.borrow_mut();
        match token {
            Token::CharacterTokens(text) => s.push(&text),
            Token::TagToken(tag) => {
                let name = tag.name.as_ref();
                if tag.kind == StartTag {
                    if matches!(name, "script" | "style" | "noscript" | "iframe") {
                        s.skip = Some(name.into());
                        return TokenSinkResult::RawData(if name == "script" {
                            RawKind::ScriptData
                        } else {
                            RawKind::Rawtext
                        });
                    }
                    if matches!(name, "title" | "textarea") {
                        return TokenSinkResult::RawData(RawKind::Rcdata);
                    }
                    if name == "plaintext" {
                        return TokenSinkResult::Plaintext;
                    }
                } else if s.skip.as_deref() == Some(name) {
                    s.skip = None;
                }
                if matches!(
                    name,
                    "p" | "div"
                        | "br"
                        | "li"
                        | "h1"
                        | "h2"
                        | "h3"
                        | "tr"
                        | "td"
                        | "section"
                        | "article"
                        | "title"
                        | "hr"
                ) {
                    s.push(" ");
                }
            }
            _ => {}
        }
        TokenSinkResult::Continue
    }
}

pub(super) fn extract(
    bytes: &[u8],
    content_type: &str,
    limit: usize,
) -> Result<(String, bool), ToolFailure> {
    let parts: Vec<_> = content_type.split(';').map(str::trim).collect();
    let mime = parts[0].to_ascii_lowercase();
    if !(mime.starts_with("text/")
        || matches!(mime.as_str(), "application/json" | "application/xml"))
    {
        return Err(invalid("server did not return a supported text Content-Type; use download_file for binary data"));
    }
    for part in &parts[1..] {
        if let Some((name, value)) = part.split_once('=') {
            if name.trim().eq_ignore_ascii_case("charset")
                && !matches!(
                    value.trim().trim_matches('"').to_ascii_lowercase().as_str(),
                    "utf-8" | "utf8" | "us-ascii"
                )
            {
                return Err(invalid("only UTF-8/ASCII response text is supported"));
            }
        }
    }
    let raw =
        std::str::from_utf8(bytes).map_err(|_| invalid("response is not valid UTF-8 text"))?;
    if mime == "text/html" {
        let sink = Sink(RefCell::new(State {
            output: String::new(),
            limit,
            truncated: false,
            skip: None,
        }));
        let tokenizer = Tokenizer::new(sink, Default::default());
        let input = BufferQueue::default();
        input.push_back(raw.into());
        let _ = tokenizer.feed(&input);
        tokenizer.end();
        let s = tokenizer.sink.0.into_inner();
        let mut output = s.output.trim().to_string();
        if s.truncated {
            output.push_str("\n[bridge: text truncated at the requested byte limit]");
        }
        Ok((output, s.truncated))
    } else {
        let mut end = raw.len().min(limit);
        while !raw.is_char_boundary(end) {
            end -= 1;
        }
        let mut output = raw[..end].to_string();
        let truncated = end < raw.len();
        if truncated {
            output.push_str("\n[bridge: text truncated at the requested byte limit]");
        }
        Ok((output, truncated))
    }
}
