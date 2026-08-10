//! The tool loop: generate, run whatever tools the model asked for, feed the
//! results back, repeat until it answers in prose or runs out of turns.
//!
//! Backend-agnostic — both HTTP shapes report calls through `GenerateOutcome`.

use super::model::{
    ChatMessage, GenerateOutcome, GenerateRequest, ModelBackend, ModelError, ToolCall,
};
use std::ops::ControlFlow;
use std::sync::atomic::{AtomicBool, Ordering};

/// How many tool rounds before we stop. Five is enough for "look at the chart,
/// change three things, confirm" and short enough to bound a runaway loop.
pub const MAX_ITERATIONS: usize = 5;

pub fn run_agent(
    backend: &dyn ModelBackend,
    mut request: GenerateRequest,
    execute: &mut dyn FnMut(&ToolCall) -> Result<String, String>,
    max_iterations: usize,
    on_token: &mut dyn FnMut(&str) -> ControlFlow<()>,
    cancel: &AtomicBool,
) -> Result<GenerateOutcome, ModelError> {
    for _ in 0..max_iterations {
        if cancel.load(Ordering::Relaxed) {
            return Ok(GenerateOutcome::default());
        }
        let outcome = backend.generate(&request, on_token)?;
        if outcome.tool_calls.is_empty() {
            return Ok(outcome);
        }
        request
            .history
            .push(ChatMessage::calling(outcome.text.clone(), outcome.tool_calls.clone()));
        for call in &outcome.tool_calls {
            if cancel.load(Ordering::Relaxed) {
                return Ok(outcome);
            }
            // A failed tool becomes a message in the conversation, not an
            // error to the user: the model can read it and try something else.
            let result = match execute(call) {
                Ok(text) => text,
                Err(e) => format!("The tool failed: {e}"),
            };
            request.history.push(ChatMessage::tool_result(call, &result));
        }
    }
    Err(ModelError::Read(format!(
        "the assistant kept calling tools after {max_iterations} rounds without answering; \
         try a smaller request"
    )))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::llm::model::GenerateOutcome;
    use serde_json::json;
    use std::cell::RefCell;

    /// A backend that replays a fixed script of outcomes.
    struct Scripted {
        replies: RefCell<Vec<GenerateOutcome>>,
        seen_history: RefCell<Vec<usize>>,
    }

    impl ModelBackend for Scripted {
        fn generate(
            &self,
            req: &GenerateRequest,
            _on_token: &mut dyn FnMut(&str) -> ControlFlow<()>,
        ) -> Result<GenerateOutcome, ModelError> {
            self.seen_history.borrow_mut().push(req.history.len());
            Ok(self.replies.borrow_mut().remove(0))
        }
        fn list_models(&self) -> Result<Vec<String>, ModelError> {
            Ok(vec![])
        }
        fn describe(&self) -> String {
            "scripted".into()
        }
    }
    // The loop is single-threaded; Send is only needed by the trait.
    unsafe impl Send for Scripted {}

    fn call(name: &str) -> ToolCall {
        ToolCall { name: name.into(), arguments: json!({}) }
    }

    #[test]
    fn runs_tools_then_returns_the_prose_answer() {
        let backend = Scripted {
            replies: RefCell::new(vec![
                GenerateOutcome { text: String::new(), tool_calls: vec![call("list_tasks")] },
                GenerateOutcome { text: "There are 3 tasks.".into(), tool_calls: vec![] },
            ]),
            seen_history: RefCell::new(vec![]),
        };
        let mut ran = Vec::new();
        let mut execute = |c: &ToolCall| {
            ran.push(c.name.clone());
            Ok("{\"tasks\":[]}".to_string())
        };
        let out = run_agent(
            &backend,
            GenerateRequest::default(),
            &mut execute,
            MAX_ITERATIONS,
            &mut |_| ControlFlow::Continue(()),
            &AtomicBool::new(false),
        )
        .unwrap();
        assert_eq!(out.text, "There are 3 tasks.");
        assert_eq!(ran, vec!["list_tasks"]);
        // Second turn saw the call plus its result.
        assert_eq!(*backend.seen_history.borrow(), vec![0, 2]);
    }

    #[test]
    fn a_failing_tool_is_reported_to_the_model_not_the_user() {
        let backend = Scripted {
            replies: RefCell::new(vec![
                GenerateOutcome { text: String::new(), tool_calls: vec![call("set_progress")] },
                GenerateOutcome { text: "I could not do that.".into(), tool_calls: vec![] },
            ]),
            seen_history: RefCell::new(vec![]),
        };
        let mut execute = |_: &ToolCall| Err("there is no task called \"ghost\"".to_string());
        let mut request = GenerateRequest::default();
        request.history.push(ChatMessage::user("set ghost to 50%"));
        let out = run_agent(
            &backend,
            request,
            &mut execute,
            MAX_ITERATIONS,
            &mut |_| ControlFlow::Continue(()),
            &AtomicBool::new(false),
        )
        .unwrap();
        assert_eq!(out.text, "I could not do that.");
    }

    #[test]
    fn a_tool_loop_is_bounded() {
        let backend = Scripted {
            replies: RefCell::new(vec![
                GenerateOutcome { text: String::new(), tool_calls: vec![call("list_tasks")] },
                GenerateOutcome { text: String::new(), tool_calls: vec![call("list_tasks")] },
            ]),
            seen_history: RefCell::new(vec![]),
        };
        let mut execute = |_: &ToolCall| Ok("{}".to_string());
        let err = run_agent(
            &backend,
            GenerateRequest::default(),
            &mut execute,
            2,
            &mut |_| ControlFlow::Continue(()),
            &AtomicBool::new(false),
        )
        .unwrap_err();
        assert!(matches!(err, ModelError::Read(m) if m.contains("2 rounds")));
    }

    #[test]
    fn cancellation_stops_before_the_next_call() {
        let backend = Scripted {
            replies: RefCell::new(vec![GenerateOutcome::default()]),
            seen_history: RefCell::new(vec![]),
        };
        let mut execute = |_: &ToolCall| Ok(String::new());
        let cancel = AtomicBool::new(true);
        let out = run_agent(
            &backend,
            GenerateRequest::default(),
            &mut execute,
            MAX_ITERATIONS,
            &mut |_| ControlFlow::Continue(()),
            &cancel,
        )
        .unwrap();
        assert!(out.text.is_empty());
        // Never even asked the backend.
        assert!(backend.seen_history.borrow().is_empty());
    }
}
