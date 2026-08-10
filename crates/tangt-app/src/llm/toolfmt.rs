//! Tool calling for a model with no tool-calling API — the built-in one.
//!
//! A server implements tools by parsing the model's output against whatever
//! format its template uses. llama.cpp does this internally but does not
//! expose the parser through its C API, so tangt does the same thing in the
//! open: the tools are described in the system prompt, and the `<tool_call>`
//! blocks the model writes are read back out of the answer.
//!
//! The format is the Hermes one, which the catalogue's Qwen3 models are
//! trained on:
//!
//! ```text
//! <tool_call>
//! {"name": "set_progress", "arguments": {"id": "qa", "percent": 50}}
//! </tool_call>
//! ```
//!
//! Everything here is a pure function over strings, compiled whether or not
//! the engine is, so it is always under test.

use super::model::{ToolCall, ToolDef};

const OPEN: &str = "<tool_call>";
const CLOSE: &str = "</tool_call>";

/// The system prompt used when tools are on offer: what the caller wanted to
/// say, then the tools and how to ask for them.
pub fn system_with_tools(system: Option<&str>, tools: &[ToolDef]) -> String {
    let mut out = String::new();
    if let Some(system) = system {
        out.push_str(system.trim());
        out.push_str("\n\n");
    }
    out.push_str(
        "# Tools\n\nYou may call one or more functions to carry out the request. The functions \
         available to you are described inside <tools></tools> tags:\n<tools>\n",
    );
    for tool in tools {
        out.push_str(&tool.wire().to_string());
        out.push('\n');
    }
    out.push_str(
        "</tools>\n\nTo call one, write a JSON object with the function's name and its arguments \
         inside <tool_call></tool_call> tags, and nothing else:\n<tool_call>\n\
         {\"name\": \"function-name\", \"arguments\": {\"argument\": \"value\"}}\n</tool_call>\n\
         Call a function only when it is needed; otherwise answer directly.",
    );
    out
}

/// An assistant turn that asked for tools, written the way the model wrote it,
/// so replaying the conversation looks like its own output.
pub fn render_tool_calls(text: &str, calls: &[ToolCall]) -> String {
    let mut out = text.trim().to_owned();
    for call in calls {
        if !out.is_empty() {
            out.push('\n');
        }
        let body = serde_json::json!({"name": call.name, "arguments": call.arguments});
        out.push_str(&format!("{OPEN}\n{body}\n{CLOSE}"));
    }
    out
}

/// What a tool answered. There is no tool role in this format: the result
/// comes back as if the user pasted it.
pub fn render_tool_result(name: Option<&str>, content: &str) -> String {
    match name {
        Some(name) => format!("<tool_response>\n{name}: {content}\n</tool_response>"),
        None => format!("<tool_response>\n{content}\n</tool_response>"),
    }
}

/// Split an answer into the text meant for the reader and the calls the model
/// asked for. Malformed blocks are left in the text: showing the user
/// something odd beats silently swallowing what the model said.
pub fn parse_tool_calls(reply: &str) -> (String, Vec<ToolCall>) {
    let mut text = String::new();
    let mut calls = Vec::new();
    let mut rest = reply;

    while let Some(start) = rest.find(OPEN) {
        let Some(end_offset) = rest[start + OPEN.len()..].find(CLOSE) else {
            break;
        };
        let body_start = start + OPEN.len();
        let body = &rest[body_start..body_start + end_offset];
        match parse_call(body) {
            Some(call) => {
                text.push_str(&rest[..start]);
                calls.push(call);
            }
            // Not a call after all; keep it where it was.
            None => text.push_str(&rest[..body_start + end_offset + CLOSE.len()]),
        }
        rest = &rest[body_start + end_offset + CLOSE.len()..];
    }
    text.push_str(rest);
    (text.trim().to_owned(), calls)
}

/// What a reader should see of an answer that is still arriving: the text with
/// `<tool_call>` blocks removed, and an unfinished one held back so the raw
/// XML never flashes on screen. It only ever grows, so a caller can stream
/// whatever is new since last time.
pub fn visible_text(text: &str) -> String {
    let (visible, _) = parse_tool_calls(text);
    // A block that has opened but not closed is still arriving: hide from the
    // opening tag onward.
    if let Some(open) = visible.rfind(OPEN) {
        if !visible[open..].contains(CLOSE) {
            return visible[..open].trim_end().to_owned();
        }
    }
    // The opening tag itself arrives a token at a time, so a tail that is only
    // *part* of `<tool_call>` must be held back too — otherwise "<tool_ca"
    // flashes on screen before it becomes a tag.
    for cut in (1..OPEN.len()).rev() {
        if visible.len() >= cut && visible.is_char_boundary(visible.len() - cut) {
            let tail = &visible[visible.len() - cut..];
            if OPEN.starts_with(tail) {
                return visible[..visible.len() - cut].trim_end().to_owned();
            }
        }
    }
    visible
}

/// Accepts the Hermes shape and the OpenAI one, since models trained on both
/// turn up in the wild.
fn parse_call(body: &str) -> Option<ToolCall> {
    let value: serde_json::Value = serde_json::from_str(body.trim()).ok()?;
    let f = value.get("function").unwrap_or(&value);
    let name = f.get("name")?.as_str()?.to_owned();
    if name.is_empty() {
        return None;
    }
    let arguments = match f.get("arguments") {
        // OpenAI passes arguments as a JSON *string*.
        Some(serde_json::Value::String(s)) => {
            serde_json::from_str(s).unwrap_or_else(|_| serde_json::json!({}))
        }
        Some(other) => other.clone(),
        None => serde_json::json!({}),
    };
    Some(ToolCall { name, arguments })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn tool() -> ToolDef {
        ToolDef {
            name: "set_progress".into(),
            description: "Set completion".into(),
            schema: json!({"type": "object"}),
        }
    }

    #[test]
    fn the_system_prompt_lists_the_tools_and_the_format() {
        let prompt = system_with_tools(Some("You edit charts."), &[tool()]);
        assert!(prompt.starts_with("You edit charts."));
        assert!(prompt.contains("<tools>"));
        assert!(prompt.contains("set_progress"));
        assert!(prompt.contains("<tool_call>"));
    }

    #[test]
    fn a_call_is_lifted_out_of_the_answer() {
        let reply = "Sure.\n<tool_call>\n{\"name\": \"set_progress\", \
                     \"arguments\": {\"id\": \"qa\", \"percent\": 50}}\n</tool_call>";
        let (text, calls) = parse_tool_calls(reply);
        assert_eq!(text, "Sure.");
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].name, "set_progress");
        assert_eq!(calls[0].arguments["percent"], 50);
    }

    #[test]
    fn several_calls_in_one_turn() {
        let reply = "<tool_call>{\"name\":\"a\",\"arguments\":{}}</tool_call>\
                     <tool_call>{\"name\":\"b\",\"arguments\":{}}</tool_call>";
        let (text, calls) = parse_tool_calls(reply);
        assert!(text.is_empty());
        assert_eq!(calls.iter().map(|c| c.name.as_str()).collect::<Vec<_>>(), ["a", "b"]);
    }

    #[test]
    fn openai_shaped_calls_are_understood() {
        let reply = "<tool_call>{\"function\":{\"name\":\"set_progress\",\
                     \"arguments\":\"{\\\"percent\\\":25}\"}}</tool_call>";
        let (_, calls) = parse_tool_calls(reply);
        assert_eq!(calls[0].name, "set_progress");
        assert_eq!(calls[0].arguments["percent"], 25);
    }

    #[test]
    fn a_malformed_block_stays_in_the_text() {
        let reply = "Look: <tool_call>not json</tool_call> done";
        let (text, calls) = parse_tool_calls(reply);
        assert!(calls.is_empty());
        assert!(text.contains("not json"), "{text}");
    }

    #[test]
    fn an_unfinished_block_is_held_back_while_streaming() {
        // Simulates the answer arriving token by token.
        let mut seen = Vec::new();
        let full = "Working.\n<tool_call>\n{\"name\": \"set_progress\", \
                    \"arguments\": {}}\n</tool_call>";
        for end in 1..=full.len() {
            if !full.is_char_boundary(end) {
                continue;
            }
            seen.push(visible_text(&full[..end]));
        }
        // The raw tag never reaches the reader.
        assert!(
            seen.iter().all(|s| !s.contains("<tool_call") && !s.contains("\"name\"")),
            "raw tool-call syntax leaked while streaming"
        );
        // And what is visible only ever grows.
        for pair in seen.windows(2) {
            assert!(
                pair[1].len() >= pair[0].len() || pair[1].starts_with(pair[0].trim_end()),
                "visible text shrank: {:?} -> {:?}",
                pair[0],
                pair[1]
            );
        }
        assert_eq!(seen.last().unwrap(), "Working.");
    }

    #[test]
    fn a_round_trip_reads_back_as_the_model_wrote_it() {
        let calls = vec![ToolCall { name: "set_progress".into(), arguments: json!({"id": "a"}) }];
        let rendered = render_tool_calls("On it.", &calls);
        let (text, parsed) = parse_tool_calls(&rendered);
        assert_eq!(text, "On it.");
        assert_eq!(parsed, calls);
    }

    #[test]
    fn tool_results_are_labelled() {
        let out = render_tool_result(Some("list_tasks"), "{\"tasks\":[]}");
        assert!(out.starts_with("<tool_response>"));
        assert!(out.contains("list_tasks: {\"tasks\":[]}"));
    }
}
