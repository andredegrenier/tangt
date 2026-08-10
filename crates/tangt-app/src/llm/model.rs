//! Talking to a model. One trait, two HTTP shapes (Ollama and
//! OpenAI-compatible), and an optional in-process llama.cpp backend behind the
//! `builtin-llm` feature.
//!
//! Blocking `ureq` on a worker thread rather than async: the LLM path then
//! needs no runtime of its own (tokio exists here only for MCP).

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::io::{BufRead, BufReader};
use std::ops::ControlFlow;
use std::time::Duration;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ModelError {
    /// The endpoint could not be reached or refused the request.
    Unreachable(String),
    /// The model replied, but not in a shape we could use.
    Read(String),
    /// The chosen backend cannot run: not downloaded, or not in this build.
    Unavailable(String),
}

impl std::fmt::Display for ModelError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ModelError::Unreachable(m) | ModelError::Read(m) | ModelError::Unavailable(m) => {
                f.write_str(m)
            }
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ToolCall {
    pub name: String,
    pub arguments: Value,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolDef {
    pub name: String,
    pub description: String,
    /// JSON Schema for the arguments.
    pub schema: Value,
}

impl ToolDef {
    /// The shape both Ollama and OpenAI expect on the wire.
    pub fn wire(&self) -> Value {
        json!({
            "type": "function",
            "function": {
                "name": self.name,
                "description": self.description,
                "parameters": self.schema,
            }
        })
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChatMessage {
    pub role: String,
    pub content: String,
    #[serde(skip_serializing_if = "Vec::is_empty", default)]
    pub tool_calls: Vec<ToolCall>,
}

impl ChatMessage {
    pub fn user(content: impl Into<String>) -> Self {
        ChatMessage { role: "user".into(), content: content.into(), tool_calls: Vec::new() }
    }
    pub fn calling(content: impl Into<String>, calls: Vec<ToolCall>) -> Self {
        ChatMessage { role: "assistant".into(), content: content.into(), tool_calls: calls }
    }
    pub fn tool_result(call: &ToolCall, result: &str) -> Self {
        ChatMessage {
            role: "tool".into(),
            content: format!("{}: {result}", call.name),
            tool_calls: Vec::new(),
        }
    }
}

#[derive(Debug, Clone, Default)]
pub struct GenerateRequest {
    pub model: String,
    pub system: Option<String>,
    pub history: Vec<ChatMessage>,
    pub tools: Vec<ToolDef>,
    pub temperature: Option<f32>,
    pub max_tokens: Option<u32>,
}

#[derive(Debug, Clone, Default)]
pub struct GenerateOutcome {
    pub text: String,
    pub tool_calls: Vec<ToolCall>,
}

pub trait ModelBackend: Send {
    fn generate(
        &self,
        req: &GenerateRequest,
        on_token: &mut dyn FnMut(&str) -> ControlFlow<()>,
    ) -> Result<GenerateOutcome, ModelError>;

    fn list_models(&self) -> Result<Vec<String>, ModelError>;

    fn describe(&self) -> String;
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Api {
    /// A model tangt downloaded and runs itself — no server to install.
    Builtin,
    Ollama,
    OpenAiCompatible,
}

impl Api {
    pub const ALL: [Api; 3] = [Api::Builtin, Api::Ollama, Api::OpenAiCompatible];

    pub fn default_url(self) -> &'static str {
        match self {
            Api::Builtin => "",
            Api::Ollama => "http://localhost:11434",
            Api::OpenAiCompatible => "http://localhost:8080",
        }
    }
    pub fn label(self) -> &'static str {
        match self {
            Api::Builtin => "Built in (runs on this machine)",
            Api::Ollama => "Ollama",
            Api::OpenAiCompatible => "OpenAI-compatible",
        }
    }
    pub fn is_builtin(self) -> bool {
        matches!(self, Api::Builtin)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModelConfig {
    pub api: Api,
    pub base_url: String,
    pub model: String,
    pub timeout_secs: u64,
}

impl Default for ModelConfig {
    fn default() -> Self {
        // Built in by default: a fresh install can use the assistant after one
        // download, with no server to set up.
        ModelConfig {
            api: Api::Builtin,
            base_url: String::new(),
            model: super::catalog::DEFAULT_MODEL.to_string(),
            timeout_secs: 120,
        }
    }
}

impl ModelConfig {
    pub fn build(&self) -> Box<dyn ModelBackend> {
        match self.api {
            Api::Builtin => builtin(&self.model),
            _ => Box::new(HttpBackend { config: self.clone() }),
        }
    }
}

#[cfg(feature = "builtin-llm")]
fn builtin(model_id: &str) -> Box<dyn ModelBackend> {
    Box::new(super::builtin::BuiltinBackend::new(model_id))
}

/// The twin for builds without the engine: the setting still exists, and
/// choosing it fails with a sentence rather than something obscure.
#[cfg(not(feature = "builtin-llm"))]
fn builtin(_model_id: &str) -> Box<dyn ModelBackend> {
    struct Unavailable;
    impl ModelBackend for Unavailable {
        fn generate(
            &self,
            _req: &GenerateRequest,
            _on_token: &mut dyn FnMut(&str) -> ControlFlow<()>,
        ) -> Result<GenerateOutcome, ModelError> {
            Err(ModelError::Unavailable(
                "this build of tangt has no built-in model engine; point the assistant at \
                 Ollama or an OpenAI-compatible endpoint instead"
                    .to_owned(),
            ))
        }
        fn list_models(&self) -> Result<Vec<String>, ModelError> {
            Ok(Vec::new())
        }
        fn describe(&self) -> String {
            "built-in engine not compiled in".to_owned()
        }
    }
    Box::new(Unavailable)
}

pub struct HttpBackend {
    pub config: ModelConfig,
}

impl HttpBackend {
    fn agent(&self) -> ureq::Agent {
        ureq::Agent::config_builder()
            // Localhost either answers at once or is not there.
            .timeout_connect(Some(Duration::from_secs(5)))
            .timeout_global(Some(Duration::from_secs(self.config.timeout_secs)))
            .build()
            .into()
    }

    fn url(&self, path: &str) -> String {
        format!("{}/{path}", self.config.base_url.trim_end_matches('/'))
    }

    fn body(&self, req: &GenerateRequest) -> Value {
        let mut messages: Vec<Value> = Vec::new();
        if let Some(system) = &req.system {
            messages.push(json!({ "role": "system", "content": system }));
        }
        for m in &req.history {
            if m.tool_calls.is_empty() {
                messages.push(json!({ "role": m.role, "content": m.content }));
            } else {
                messages.push(json!({
                    "role": m.role,
                    "content": m.content,
                    "tool_calls": m.tool_calls.iter().map(|c| json!({
                        "type": "function",
                        "function": { "name": c.name, "arguments": c.arguments }
                    })).collect::<Vec<_>>(),
                }));
            }
        }
        let mut body = json!({
            "model": req.model,
            "messages": messages,
            "stream": true,
        });
        if !req.tools.is_empty() {
            body["tools"] = Value::Array(req.tools.iter().map(|t| t.wire()).collect());
        }
        match self.config.api {
            Api::Ollama => {
                let mut options = json!({});
                if let Some(t) = req.temperature {
                    options["temperature"] = json!(t);
                }
                if let Some(n) = req.max_tokens {
                    options["num_predict"] = json!(n);
                }
                body["options"] = options;
            }
            // Builtin never reaches HttpBackend; grouped so the match stays
            // exhaustive without a catch-all that could hide a real variant.
            Api::OpenAiCompatible | Api::Builtin => {
                if let Some(t) = req.temperature {
                    body["temperature"] = json!(t);
                }
                if let Some(n) = req.max_tokens {
                    body["max_tokens"] = json!(n);
                }
            }
        }
        body
    }
}

/// One decoded streaming chunk, whichever API produced it.
#[derive(Debug, Default)]
struct Chunk {
    text: String,
    tool_calls: Vec<ToolCall>,
    done: bool,
}

fn parse_chunk(api: Api, line: &str) -> Option<Chunk> {
    let raw = match api {
        Api::Ollama => line.trim(),
        Api::OpenAiCompatible | Api::Builtin => {
            let data = line.strip_prefix("data:")?.trim();
            if data == "[DONE]" {
                return Some(Chunk { done: true, ..Default::default() });
            }
            data
        }
    };
    if raw.is_empty() {
        return None;
    }
    let value: Value = serde_json::from_str(raw).ok()?;
    let mut chunk = Chunk::default();
    let message = match api {
        Api::Ollama => {
            chunk.done = value.get("done").and_then(Value::as_bool).unwrap_or(false);
            value.get("message").cloned()
        }
        Api::OpenAiCompatible | Api::Builtin => value
            .get("choices")
            .and_then(|c| c.get(0))
            .and_then(|c| c.get("delta").or_else(|| c.get("message")))
            .cloned(),
    };
    if let Some(message) = message {
        if let Some(text) = message.get("content").and_then(Value::as_str) {
            chunk.text.push_str(text);
        }
        if let Some(calls) = message.get("tool_calls").and_then(Value::as_array) {
            for call in calls {
                let f = call.get("function").unwrap_or(call);
                let Some(name) = f.get("name").and_then(Value::as_str) else { continue };
                // Ollama sends an object; OpenAI sends a JSON string.
                let arguments = match f.get("arguments") {
                    Some(Value::String(s)) => {
                        serde_json::from_str(s).unwrap_or_else(|_| json!({}))
                    }
                    Some(other) => other.clone(),
                    None => json!({}),
                };
                chunk.tool_calls.push(ToolCall { name: name.to_string(), arguments });
            }
        }
    }
    Some(chunk)
}

impl ModelBackend for HttpBackend {
    fn generate(
        &self,
        req: &GenerateRequest,
        on_token: &mut dyn FnMut(&str) -> ControlFlow<()>,
    ) -> Result<GenerateOutcome, ModelError> {
        let path = match self.config.api {
            Api::Ollama => "api/chat",
            Api::OpenAiCompatible | Api::Builtin => "v1/chat/completions",
        };
        let response = self
            .agent()
            .post(self.url(path))
            .send_json(self.body(req))
            .map_err(|e| {
                ModelError::Unreachable(format!(
                    "could not reach {} at {} ({e}). Is it running?",
                    self.config.api.label(),
                    self.config.base_url
                ))
            })?;

        let mut out = GenerateOutcome::default();
        let reader = BufReader::new(response.into_body().into_reader());
        for line in reader.lines() {
            let line = line.map_err(|e| ModelError::Read(format!("stream ended badly: {e}")))?;
            let Some(chunk) = parse_chunk(self.config.api, &line) else { continue };
            if !chunk.text.is_empty() {
                out.text.push_str(&chunk.text);
                // Dropping the reader closes the connection, which stops the
                // server generating.
                if on_token(&chunk.text).is_break() {
                    return Ok(out);
                }
            }
            out.tool_calls.extend(chunk.tool_calls);
            if chunk.done {
                break;
            }
        }
        Ok(out)
    }

    fn list_models(&self) -> Result<Vec<String>, ModelError> {
        let path = match self.config.api {
            Api::Ollama => "api/tags",
            Api::OpenAiCompatible | Api::Builtin => "v1/models",
        };
        let mut response = self.agent().get(self.url(path)).call().map_err(|e| {
            ModelError::Unreachable(format!(
                "could not reach {} at {} ({e})",
                self.config.api.label(),
                self.config.base_url
            ))
        })?;
        let value: Value = response
            .body_mut()
            .read_json()
            .map_err(|e| ModelError::Read(format!("unexpected model list: {e}")))?;
        let names = match self.config.api {
            Api::Ollama => value
                .get("models")
                .and_then(Value::as_array)
                .map(|a| {
                    a.iter()
                        .filter_map(|m| m.get("name").and_then(Value::as_str))
                        .map(str::to_string)
                        .collect()
                })
                .unwrap_or_default(),
            Api::OpenAiCompatible | Api::Builtin => value
                .get("data")
                .and_then(Value::as_array)
                .map(|a| {
                    a.iter()
                        .filter_map(|m| m.get("id").and_then(Value::as_str))
                        .map(str::to_string)
                        .collect()
                })
                .unwrap_or_default(),
        };
        Ok(names)
    }

    fn describe(&self) -> String {
        format!(
            "{} · {} · {}",
            self.config.api.label(),
            self.config.base_url,
            self.config.model
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ollama_chunks() {
        let line = r#"{"message":{"role":"assistant","content":"Hi"},"done":false}"#;
        let chunk = parse_chunk(Api::Ollama, line).unwrap();
        assert_eq!(chunk.text, "Hi");
        assert!(!chunk.done);

        let line = r#"{"message":{"tool_calls":[{"function":{"name":"list_tasks","arguments":{}}}]},"done":true}"#;
        let chunk = parse_chunk(Api::Ollama, line).unwrap();
        assert_eq!(chunk.tool_calls[0].name, "list_tasks");
        assert!(chunk.done);
    }

    #[test]
    fn openai_chunks_and_string_arguments() {
        let line = r#"data: {"choices":[{"delta":{"content":"Hello"}}]}"#;
        assert_eq!(parse_chunk(Api::OpenAiCompatible, line).unwrap().text, "Hello");

        let line = r#"data: {"choices":[{"delta":{"tool_calls":[{"function":{"name":"set_progress","arguments":"{\"id\":\"a\",\"percent\":50}"}}]}}]}"#;
        let chunk = parse_chunk(Api::OpenAiCompatible, line).unwrap();
        assert_eq!(chunk.tool_calls[0].name, "set_progress");
        assert_eq!(chunk.tool_calls[0].arguments["percent"], 50);

        assert!(parse_chunk(Api::OpenAiCompatible, "data: [DONE]").unwrap().done);
        assert!(parse_chunk(Api::OpenAiCompatible, "garbage").is_none());
    }

    #[test]
    fn request_body_shapes_per_api() {
        let req = GenerateRequest {
            model: "m".into(),
            system: Some("sys".into()),
            history: vec![ChatMessage::user("hi")],
            temperature: Some(0.2),
            ..Default::default()
        };
        let close = |v: &Value| (v.as_f64().unwrap() - 0.2).abs() < 1e-6;
        let ollama = HttpBackend { config: ModelConfig { api: Api::Ollama, ..Default::default() } };
        let body = ollama.body(&req);
        assert_eq!(body["messages"][0]["role"], "system");
        assert_eq!(body["messages"][1]["content"], "hi");
        assert!(close(&body["options"]["temperature"]));

        let openai = HttpBackend {
            config: ModelConfig { api: Api::OpenAiCompatible, ..Default::default() },
        };
        assert!(close(&openai.body(&req)["temperature"]));
    }

    #[test]
    fn urls_tolerate_trailing_slashes() {
        let b = HttpBackend {
            config: ModelConfig { base_url: "http://x:1/".into(), ..Default::default() },
        };
        assert_eq!(b.url("api/chat"), "http://x:1/api/chat");
    }
}
