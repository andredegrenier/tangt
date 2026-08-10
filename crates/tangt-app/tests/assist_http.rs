//! Drives the assistant's HTTP path against a stub server that speaks the
//! Ollama and OpenAI wire formats. Proves the streaming parser, tool-call
//! extraction, and the agent loop work over a real socket — the parts tangt
//! owns — without needing a model installed.

#[path = "../src/llm/mod.rs"]
#[allow(dead_code)]
mod llm;

use axum::response::IntoResponse;
use llm::agent::{run_agent, MAX_ITERATIONS};
use llm::model::{Api, ChatMessage, GenerateRequest, ModelConfig, ToolCall, ToolDef};
use serde_json::json;
use std::ops::ControlFlow;
use std::sync::atomic::AtomicBool;
use std::sync::{Arc, Mutex};

/// Bodies the stub returns, in order, plus the requests it received.
#[derive(Default)]
struct Stub {
    replies: Mutex<Vec<String>>,
    seen: Mutex<Vec<serde_json::Value>>,
}

fn spawn_stub(replies: Vec<String>) -> (String, Arc<Stub>, tokio::runtime::Runtime) {
    let stub = Arc::new(Stub {
        replies: Mutex::new(replies),
        seen: Mutex::new(Vec::new()),
    });
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(1)
        .enable_all()
        .build()
        .unwrap();
    let (addr_tx, addr_rx) = std::sync::mpsc::channel();
    {
        let stub = stub.clone();
        runtime.spawn(async move {
            let app = axum::Router::new().fallback(axum::routing::any(
                move |body: axum::extract::Json<serde_json::Value>| {
                    let stub = stub.clone();
                    async move {
                        stub.seen.lock().unwrap().push(body.0);
                        let mut replies = stub.replies.lock().unwrap();
                        let reply =
                            if replies.is_empty() { String::new() } else { replies.remove(0) };
                        reply.into_response()
                    }
                },
            ));
            let listener = tokio::net::TcpListener::bind(("127.0.0.1", 0)).await.unwrap();
            addr_tx.send(listener.local_addr().unwrap()).unwrap();
            let _ = axum::serve(listener, app).await;
        });
    }
    let addr = addr_rx.recv().unwrap();
    (format!("http://{addr}"), stub, runtime)
}

fn tools() -> Vec<ToolDef> {
    vec![ToolDef {
        name: "set_progress".into(),
        description: "Set completion".into(),
        schema: json!({"type": "object", "properties": {}}),
    }]
}

#[test]
fn ollama_stream_with_a_tool_call_drives_the_agent_loop() {
    // Turn 1: the model asks for a tool. Turn 2: it answers in prose.
    let call = json!({
        "message": {
            "role": "assistant",
            "tool_calls": [{"function": {"name": "set_progress",
                                          "arguments": {"id": "d", "percent": 60}}}]
        },
        "done": true
    })
    .to_string();
    let answer = [
        json!({"message": {"role":"assistant","content":"Marked "}, "done": false}).to_string(),
        json!({"message": {"role":"assistant","content":"Design 60% complete."}, "done": true})
            .to_string(),
    ]
    .join("\n");
    let (base_url, stub, runtime) = spawn_stub(vec![call, answer]);

    let config = ModelConfig {
        api: Api::Ollama,
        base_url,
        model: "stub".into(),
        timeout_secs: 10,
    };
    let backend = config.build();

    let mut executed: Vec<ToolCall> = Vec::new();
    let mut execute = |c: &ToolCall| {
        executed.push(c.clone());
        Ok(json!({"updated": "d", "progress_percent": 60}).to_string())
    };
    let mut streamed = String::new();
    let mut on_token = |t: &str| {
        streamed.push_str(t);
        ControlFlow::Continue(())
    };

    let out = run_agent(
        backend.as_ref(),
        GenerateRequest {
            model: "stub".into(),
            system: Some("you edit charts".into()),
            history: vec![ChatMessage::user("mark design 60% done")],
            tools: tools(),
            temperature: Some(0.2),
            max_tokens: None,
        },
        &mut execute,
        MAX_ITERATIONS,
        &mut on_token,
        &AtomicBool::new(false),
    )
    .expect("agent completed");

    assert_eq!(out.text, "Marked Design 60% complete.");
    assert_eq!(streamed, "Marked Design 60% complete.", "tokens should stream as they arrive");
    assert_eq!(executed.len(), 1);
    assert_eq!(executed[0].name, "set_progress");
    assert_eq!(executed[0].arguments["percent"], 60);

    // The second request carried the tool call and its result back to the model.
    let seen = stub.seen.lock().unwrap();
    assert_eq!(seen.len(), 2);
    let messages = seen[1]["messages"].as_array().unwrap();
    assert!(
        messages.iter().any(|m| m["role"] == "tool"),
        "the tool result was not fed back: {messages:?}"
    );
    // Tools are advertised in the Ollama shape.
    assert_eq!(seen[0]["tools"][0]["function"]["name"], "set_progress");
    runtime.shutdown_background();
}

#[test]
fn openai_sse_stream_is_parsed() {
    let sse = [
        "data: {\"choices\":[{\"delta\":{\"content\":\"All \"}}]}",
        "data: {\"choices\":[{\"delta\":{\"content\":\"set.\"}}]}",
        "data: [DONE]",
    ]
    .join("\n");
    let (base_url, _stub, runtime) = spawn_stub(vec![sse]);

    let config = ModelConfig {
        api: Api::OpenAiCompatible,
        base_url,
        model: "stub".into(),
        timeout_secs: 10,
    };
    let out = config
        .build()
        .generate(
            &GenerateRequest {
                model: "stub".into(),
                history: vec![ChatMessage::user("hi")],
                ..Default::default()
            },
            &mut |_| ControlFlow::Continue(()),
        )
        .expect("generate completed");
    assert_eq!(out.text, "All set.");
    runtime.shutdown_background();
}

#[test]
fn an_unreachable_endpoint_explains_itself() {
    let config = ModelConfig {
        api: Api::Ollama,
        // Port 1 is never a model server.
        base_url: "http://127.0.0.1:1".into(),
        model: "stub".into(),
        timeout_secs: 3,
    };
    let err = config
        .build()
        .generate(&GenerateRequest::default(), &mut |_| ControlFlow::Continue(()))
        .unwrap_err();
    let message = err.to_string();
    assert!(message.contains("Ollama"), "{message}");
    assert!(message.contains("Is it running?"), "{message}");
}
