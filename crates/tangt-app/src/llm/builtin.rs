//! Running a downloaded GGUF model in process, through llama.cpp.
//!
//! The second implementation of [`ModelBackend`]: same trait, same streaming
//! callback and same cancellation as the HTTP one, so the agent loop above it
//! cannot tell which is answering.
//!
//! Loading weights costs seconds and a couple of gigabytes, so the model is
//! kept alive between requests in a process-wide cache of exactly one. The
//! *context* (the KV cache) is not: a fresh one per request is cheap next to
//! the load, and means no conversation can leak into the next.
//!
//! All FFI stays inside `llama-cpp-2`; there is no `unsafe` here.

use super::model::{GenerateOutcome, GenerateRequest, ModelBackend, ModelError};
#[cfg(test)]
use super::model::ToolCall;
use super::toolfmt;
use llama_cpp_2::context::params::LlamaContextParams;
use llama_cpp_2::llama_backend::LlamaBackend;
use llama_cpp_2::llama_batch::LlamaBatch;
use llama_cpp_2::model::params::LlamaModelParams;
use llama_cpp_2::model::{AddBos, LlamaChatMessage, LlamaModel};
use llama_cpp_2::sampling::LlamaSampler;
use std::num::NonZeroU32;
use std::ops::ControlFlow;
use std::path::PathBuf;
use std::sync::{Arc, Mutex, OnceLock};

/// How much conversation the model is given. Larger costs memory: the KV
/// cache is allocated for the whole window whether it is used or not.
const N_CTX: u32 = 8192;
const DEFAULT_MAX_TOKENS: u32 = 1024;
/// Batch size for feeding the prompt in; also the context's `n_batch`.
const N_BATCH: usize = 512;
const TOP_P: f32 = 0.9;
const DEFAULT_TEMPERATURE: f32 = 0.7;
/// llama.cpp's "pick a seed for me" sentinel.
const DEFAULT_SEED: u32 = u32::MAX;

/// llama.cpp's global init: a process-wide resource, created once and never
/// dropped. A refusal to start is remembered rather than retried.
fn backend() -> Result<&'static LlamaBackend, ModelError> {
    static BACKEND: OnceLock<Result<LlamaBackend, String>> = OnceLock::new();
    BACKEND
        .get_or_init(|| {
            let mut backend = LlamaBackend::init().map_err(|e| e.to_string())?;
            // llama.cpp writes a great deal to stderr; a windowed application
            // has nowhere to show it, and every failure returns a Result anyway.
            backend.void_logs();
            Ok(backend)
        })
        .as_ref()
        .map_err(|e| ModelError::Unavailable(format!("could not start llama.cpp: {e}")))
}

/// The last model loaded. Weights are gigabytes, so a second request for the
/// same file reuses them; a different one replaces the entry.
static LOADED: Mutex<Option<(PathBuf, Arc<LlamaModel>)>> = Mutex::new(None);

fn load(path: &PathBuf) -> Result<Arc<LlamaModel>, ModelError> {
    let backend = backend()?;
    let mut slot = LOADED.lock().expect("model cache lock");
    if let Some((loaded, model)) = &*slot {
        if loaded == path {
            return Ok(model.clone());
        }
    }
    // Drop the previous model first, so two sets of weights are never
    // resident at once.
    *slot = None;
    let model = LlamaModel::load_from_file(backend, path, &LlamaModelParams::default())
        .map_err(|e| ModelError::Unavailable(format!("could not load {}: {e}", path.display())))?;
    let model = Arc::new(model);
    *slot = Some((path.clone(), model.clone()));
    Ok(model)
}

/// Let the cached weights go.
///
/// This has to happen before the process exits. llama.cpp's Metal backend
/// tears itself down from a C++ static destructor and asserts that nothing
/// still holds its buffers; weights parked in a `static` for the life of the
/// program are exactly that, and the assert aborts on the way out.
pub fn unload() {
    if let Ok(mut slot) = LOADED.lock() {
        *slot = None;
    }
}

pub struct BuiltinBackend {
    /// Catalogue id, resolved to a path at generation time so a model
    /// downloaded after the backend was built is still picked up.
    model_id: String,
}

impl BuiltinBackend {
    pub fn new(model_id: impl Into<String>) -> Self {
        BuiltinBackend { model_id: model_id.into() }
    }

    fn model_path(&self) -> Result<PathBuf, ModelError> {
        let Some(entry) = super::catalog::entry(&self.model_id) else {
            return Err(ModelError::Unavailable(format!(
                "\"{}\" is not one of the built-in models",
                self.model_id
            )));
        };
        let dir = super::catalog::models_dir().ok_or_else(|| {
            ModelError::Unavailable("could not find tangt's data directory".to_owned())
        })?;
        entry.installed_in(&dir).ok_or_else(|| {
            ModelError::Unavailable(format!(
                "{} has not been downloaded yet — View ▸ Preferences ▸ Assistant",
                entry.label
            ))
        })
    }
}

impl ModelBackend for BuiltinBackend {
    fn generate(
        &self,
        req: &GenerateRequest,
        on_token: &mut dyn FnMut(&str) -> ControlFlow<()>,
    ) -> Result<GenerateOutcome, ModelError> {
        let path = self.model_path()?;
        let model = load(&path)?;

        let max_tokens = req.max_tokens.unwrap_or(DEFAULT_MAX_TOKENS).max(1);
        // Whatever is not reserved for the answer is the prompt's, less slack
        // for the template's own tokens.
        let budget = N_CTX.saturating_sub(max_tokens + 64).max(256) as usize;
        let tokens = fit_prompt(&model, req, budget)?;

        let threads = threads();
        let ctx_params = LlamaContextParams::default()
            .with_n_ctx(NonZeroU32::new(N_CTX))
            .with_n_batch(N_BATCH as u32)
            .with_n_threads(threads)
            .with_n_threads_batch(threads);
        let mut ctx = model
            .new_context(backend()?, ctx_params)
            .map_err(|e| ModelError::Unavailable(format!("could not start the model: {e}")))?;

        let mut batch = LlamaBatch::new(N_BATCH, 1);
        let mut pos = 0i32;
        let last = tokens.len() - 1;
        for chunk in tokens.chunks(N_BATCH) {
            batch.clear();
            for token in chunk {
                let index = pos as usize;
                batch
                    .add(*token, pos, &[0], index == last)
                    .map_err(|e| ModelError::Read(e.to_string()))?;
                pos += 1;
            }
            ctx.decode(&mut batch).map_err(|e| {
                ModelError::Read(format!("the model could not read the prompt: {e}"))
            })?;
        }

        let mut sampler = sampler(req.temperature.unwrap_or(DEFAULT_TEMPERATURE));
        let mut text = String::new();
        // Tokens are pieces of UTF-8, not characters: a piece can end
        // mid-sequence and only complete with the next one.
        let mut pending: Vec<u8> = Vec::new();
        let mut produced = 0u32;
        // With tools on offer the answer may contain <tool_call> blocks, which
        // are addressed to tangt and not to the reader.
        let tools_offered = !req.tools.is_empty();
        let mut streamed = 0usize;

        while produced < max_tokens && (pos as u32) < N_CTX {
            let token = sampler.sample(&ctx, batch.n_tokens() - 1);
            sampler.accept(token);
            if model.is_eog_token(token) {
                break;
            }
            produced += 1;

            pending.extend_from_slice(&piece(&model, token)?);
            if let Some(chunk) = take_utf8(&mut pending) {
                text.push_str(&chunk);
                if stream(&text, &mut streamed, tools_offered, on_token).is_break() {
                    return Ok(finish(text, tools_offered));
                }
            }

            batch.clear();
            batch
                .add(token, pos, &[0], true)
                .map_err(|e| ModelError::Read(e.to_string()))?;
            pos += 1;
            ctx.decode(&mut batch)
                .map_err(|e| ModelError::Read(format!("the model stopped generating: {e}")))?;
        }

        // Anything left is an incomplete sequence; the replacement character
        // beats dropping the bytes silently.
        if !pending.is_empty() {
            text.push_str(&String::from_utf8_lossy(&pending));
            let _ = stream(&text, &mut streamed, tools_offered, on_token);
        }
        Ok(finish(text, tools_offered))
    }

    fn list_models(&self) -> Result<Vec<String>, ModelError> {
        let Some(dir) = super::catalog::models_dir() else {
            return Ok(Vec::new());
        };
        Ok(super::catalog::CATALOG
            .iter()
            .filter(|e| e.installed_in(&dir).is_some())
            .map(|e| e.id.to_owned())
            .collect())
    }

    fn describe(&self) -> String {
        let label = super::catalog::entry(&self.model_id)
            .map_or(self.model_id.as_str(), |e| e.label);
        format!("{label} (built in)")
    }
}

fn finish(text: String, tools_offered: bool) -> GenerateOutcome {
    if !tools_offered {
        return GenerateOutcome { text, tool_calls: Vec::new() };
    }
    let (text, tool_calls) = toolfmt::parse_tool_calls(&text);
    GenerateOutcome { text, tool_calls }
}

/// Threads to generate with. All of them would make the interface stutter on
/// the very machines that need the small model, and llama.cpp sees little
/// from the last few cores anyway.
fn threads() -> i32 {
    let cores = std::thread::available_parallelism().map_or(4, std::num::NonZeroUsize::get);
    cores.saturating_sub(1).clamp(1, 8) as i32
}

fn sampler(temperature: f32) -> LlamaSampler {
    if temperature <= 0.0 {
        LlamaSampler::chain_simple([LlamaSampler::greedy()])
    } else {
        LlamaSampler::chain_simple([
            LlamaSampler::top_p(TOP_P, 1),
            LlamaSampler::temp(temperature),
            LlamaSampler::dist(DEFAULT_SEED),
        ])
    }
}

/// Hand the reader everything of `text` that is theirs to see and has not
/// been handed over yet.
fn stream(
    text: &str,
    streamed: &mut usize,
    tools_offered: bool,
    on_token: &mut dyn FnMut(&str) -> ControlFlow<()>,
) -> ControlFlow<()> {
    let visible = if tools_offered {
        std::borrow::Cow::Owned(toolfmt::visible_text(text))
    } else {
        std::borrow::Cow::Borrowed(text)
    };
    if visible.len() <= *streamed {
        return ControlFlow::Continue(());
    }
    let chunk = &visible[*streamed..];
    *streamed = visible.len();
    on_token(chunk)
}

/// The conversation as roles and text, in the order the model should see it.
///
/// There is no tool role in a chat template we can rely on, so a tool round
/// trip is written the way the model itself writes it: the assistant's
/// `<tool_call>` block, and the result as a `<tool_response>` from the user.
fn turns(req: &GenerateRequest) -> Vec<(String, String)> {
    let mut turns = Vec::with_capacity(req.history.len() + 1);
    let system = if req.tools.is_empty() {
        req.system.clone()
    } else {
        Some(toolfmt::system_with_tools(req.system.as_deref(), &req.tools))
    };
    if let Some(system) = system {
        turns.push(("system".to_string(), system));
    }
    for m in &req.history {
        match m.role.as_str() {
            "tool" => turns.push((
                "user".to_string(),
                toolfmt::render_tool_result(None, &m.content),
            )),
            "assistant" if !m.tool_calls.is_empty() => turns.push((
                "assistant".to_string(),
                toolfmt::render_tool_calls(&m.content, &m.tool_calls),
            )),
            _ => turns.push((m.role.clone(), m.content.clone())),
        }
    }
    turns
}

/// Render through the model's own chat template, falling back to ChatML when
/// the GGUF carries none.
fn render(model: &LlamaModel, turns: &[(String, String)]) -> String {
    let templated = model.chat_template(None).ok().and_then(|tmpl| {
        let chat: Option<Vec<LlamaChatMessage>> = turns
            .iter()
            .map(|(role, content)| LlamaChatMessage::new(role.clone(), content.clone()).ok())
            .collect();
        model.apply_chat_template(&tmpl, &chat?, true).ok()
    });
    templated.unwrap_or_else(|| chatml(turns))
}

fn chatml(turns: &[(String, String)]) -> String {
    let mut out = String::new();
    for (role, content) in turns {
        out.push_str(&format!("<|im_start|>{role}\n{content}<|im_end|>\n"));
    }
    out.push_str("<|im_start|>assistant\n");
    out
}

/// Tokenize the conversation, trimming the *oldest* turn after the system
/// prompt until it fits: the recent turns are the ones that matter.
fn fit_prompt(
    model: &LlamaModel,
    req: &GenerateRequest,
    budget: usize,
) -> Result<Vec<llama_cpp_2::token::LlamaToken>, ModelError> {
    let mut turns = turns(req);
    if turns.is_empty() {
        return Err(ModelError::Read("there was nothing to ask".to_owned()));
    }
    for _ in 0..8 {
        let rendered = render(model, &turns);
        let tokens = model
            .str_to_token(&rendered, AddBos::Always)
            .map_err(|e| ModelError::Read(format!("could not read the prompt: {e}")))?;
        if tokens.is_empty() {
            return Err(ModelError::Read("the prompt was empty".to_owned()));
        }
        if tokens.len() <= budget {
            return Ok(tokens);
        }
        // Drop the oldest non-system turn; if only the system prompt and one
        // turn remain, there is nothing safe left to cut.
        let first_droppable = usize::from(turns.first().is_some_and(|(r, _)| r == "system"));
        if turns.len() <= first_droppable + 1 {
            return Err(ModelError::Read(
                "the request is too long for the model's context window".to_owned(),
            ));
        }
        turns.remove(first_droppable);
    }
    Err(ModelError::Read(
        "the conversation could not be trimmed to fit the model's context window".to_owned(),
    ))
}

fn piece(
    model: &LlamaModel,
    token: llama_cpp_2::token::LlamaToken,
) -> Result<Vec<u8>, ModelError> {
    // Special tokens are the template's scaffolding; end-of-turn is handled
    // above and the rest are not the answer.
    match model.token_to_piece_bytes(token, 32, false, None) {
        Ok(bytes) => Ok(bytes),
        Err(llama_cpp_2::TokenToStringError::InsufficientBufferSpace(needed)) => model
            .token_to_piece_bytes(token, needed.unsigned_abs() as usize, false, None)
            .map_err(|e| ModelError::Read(e.to_string())),
        Err(e) => Err(ModelError::Read(e.to_string())),
    }
}

/// Take whatever complete UTF-8 is at the front of `pending`, leaving any
/// partial sequence for the next token.
fn take_utf8(pending: &mut Vec<u8>) -> Option<String> {
    if pending.is_empty() {
        return None;
    }
    match std::str::from_utf8(pending) {
        Ok(s) => {
            let out = s.to_owned();
            pending.clear();
            Some(out)
        }
        Err(e) => {
            let valid = e.valid_up_to();
            if valid == 0 {
                return None;
            }
            let out = String::from_utf8_lossy(&pending[..valid]).into_owned();
            pending.drain(..valid);
            Some(out)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::llm::model::ChatMessage;

    #[test]
    fn utf8_is_reassembled_across_token_boundaries() {
        // "é" is two bytes; arriving one at a time it must not be mangled.
        let mut pending = vec![0xC3];
        assert_eq!(take_utf8(&mut pending), None, "a half character is not text yet");
        pending.push(0xA9);
        assert_eq!(take_utf8(&mut pending).as_deref(), Some("é"));
        assert!(pending.is_empty());

        // Complete text followed by a partial sequence: emit the first, keep
        // the rest.
        let mut pending = b"hi".to_vec();
        pending.push(0xE2);
        assert_eq!(take_utf8(&mut pending).as_deref(), Some("hi"));
        assert_eq!(pending, vec![0xE2]);
    }

    #[test]
    fn a_tool_round_trip_is_written_the_way_the_model_writes_it() {
        let req = GenerateRequest {
            system: Some("edit charts".into()),
            history: vec![
                ChatMessage::user("mark qa done"),
                ChatMessage::calling(
                    String::new(),
                    vec![ToolCall {
                        name: "set_progress".into(),
                        arguments: serde_json::json!({"id": "qa"}),
                    }],
                ),
                ChatMessage {
                    role: "tool".into(),
                    content: "{\"updated\":\"qa\"}".into(),
                    tool_calls: vec![],
                },
            ],
            tools: vec![crate::llm::model::ToolDef {
                name: "set_progress".into(),
                description: "set it".into(),
                schema: serde_json::json!({}),
            }],
            ..Default::default()
        };
        let turns = turns(&req);
        assert_eq!(turns[0].0, "system");
        assert!(turns[0].1.contains("<tools>"), "tools must be described in the system turn");
        assert!(turns[2].1.contains("<tool_call>"), "the call replays as the model wrote it");
        // The result comes back as a user turn, since there is no tool role.
        assert_eq!(turns[3].0, "user");
        assert!(turns[3].1.contains("<tool_response>"));
    }

    #[test]
    fn chatml_fallback_closes_every_turn_and_invites_the_assistant() {
        let rendered = chatml(&[
            ("system".into(), "be brief".into()),
            ("user".into(), "hello".into()),
        ]);
        assert_eq!(rendered.matches("<|im_start|>").count(), 3);
        assert_eq!(rendered.matches("<|im_end|>").count(), 2);
        assert!(rendered.ends_with("<|im_start|>assistant\n"));
    }

    #[test]
    fn an_unknown_model_id_says_so() {
        let backend = BuiltinBackend::new("not-a-model");
        let err = backend.model_path().unwrap_err();
        assert!(
            matches!(&err, ModelError::Unavailable(m) if m.contains("not one of the built-in")),
            "{err}"
        );
    }
}
