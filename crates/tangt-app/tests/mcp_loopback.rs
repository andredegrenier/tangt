//! Drives the HTTP MCP server the way a real client would: a full JSON-RPC
//! handshake over loopback, with a stand-in for the UI thread answering the
//! bridge. Proves the transport, the token gate, and the tool plumbing all
//! agree before anything ships.

#[path = "../src/mcp/mod.rs"]
#[allow(dead_code)]
mod mcp;

use mcp::bridge::{AppBridge, AppCommand};
use std::sync::Arc;
use tangt_core::document::Document;
use tangt_core::ops::Op;
use tangt_core::tools::{self, ToolOutcome};

const CHART: &str = "task \"Design\" : d, 2026-08-10, 5d // keep\n";
const TODAY: i64 = 20675;

/// Stands in for the egui frame loop: owns the document, answers commands.
/// Publishes the document text after every command so the test can assert on
/// it without joining (the server keeps a channel clone alive).
fn fake_ui_thread(
    rx: std::sync::mpsc::Receiver<AppCommand>,
    published: Arc<std::sync::Mutex<String>>,
) {
    std::thread::spawn(move || {
        let mut doc = Document::new(CHART.to_string(), TODAY);
        while let Ok(command) = rx.recv() {
            let (reply, result) = match command {
                AppCommand::ListTasks { reply } => (reply, tools::list_tasks(doc.model())),
                AppCommand::SetProgress { id, percent, reply } => {
                    (reply, tools::set_progress(doc.model(), &id, percent))
                }
                AppCommand::AddTask { args, reply } => {
                    (reply, tools::add_task(doc.model(), doc.text(), *args))
                }
                other => {
                    other_fail(other);
                    continue;
                }
            };
            let answer = match result {
                Ok(ToolOutcome::Read(v)) => Ok(v),
                Ok(ToolOutcome::Write { edits, intent, summary }) => {
                    let seq = doc.next_seq();
                    doc.apply(Op { edits, intent, actor: 0, seq });
                    Ok(summary)
                }
                Err(e) => Err(e),
            };
            *published.lock().expect("publish lock") = doc.text().to_string();
            let _ = reply.send(answer);
        }
    });
}

fn other_fail(_command: AppCommand) {}

/// Returns (status, body) so the test can assert on both.
fn post(
    agent: &ureq::Agent,
    url: &str,
    token: Option<&str>,
    session: Option<&str>,
    body: serde_json::Value,
) -> (u16, String) {
    let mut req = agent
        .post(url)
        .header("Accept", "application/json, text/event-stream");
    if let Some(token) = token {
        req = req.header("Authorization", &format!("Bearer {token}"));
    }
    if let Some(session) = session {
        req = req.header("Mcp-Session-Id", session);
    }
    let response = req.send_json(body).expect("request completed");
    let status = response.status().as_u16();
    let text = response.into_body().read_to_string().expect("body read");
    (status, text)
}

/// The rmcp handshake answers over SSE; pull the JSON payload out.
fn json_payload(body: &str) -> String {
    body.lines()
        .filter_map(|l| l.strip_prefix("data:"))
        .map(str::trim)
        .collect::<Vec<_>>()
        .join("")
}

#[test]
fn a_client_can_read_and_edit_the_chart_over_loopback() {
    let (tx, rx) = std::sync::mpsc::channel();
    let published = Arc::new(std::sync::Mutex::new(CHART.to_string()));
    fake_ui_thread(rx, published.clone());
    let ctx = eframe::egui::Context::default();
    let bridge = Arc::new(AppBridge::new(tx, ctx));
    let token = mcp::new_token();

    let runtime = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()
        .unwrap();
    let (addr_tx, addr_rx) = std::sync::mpsc::channel();
    let shutdown = tokio_util::sync::CancellationToken::new();
    {
        let (token, shutdown) = (token.clone(), shutdown.clone());
        runtime.spawn(async move {
            // Port 0: let the OS pick, so the test never fights a real server.
            let listener = tokio::net::TcpListener::bind(("127.0.0.1", 0)).await.unwrap();
            addr_tx.send(listener.local_addr().unwrap()).unwrap();
            let app = mcp::runtime::router(bridge, token, Default::default());
            let _ = axum::serve(listener, app)
                .with_graceful_shutdown(async move { shutdown.cancelled_owned().await })
                .await;
        });
    }
    let addr = addr_rx.recv().unwrap();
    let url = format!("http://{addr}/mcp");
    let agent: ureq::Agent = ureq::Agent::config_builder()
        .timeout_global(Some(std::time::Duration::from_secs(10)))
        // Report status codes rather than turning them into errors, so the
        // test can assert on the gate as well as the happy path.
        .http_status_as_error(false)
        .build()
        .into();

    // A request without the token is refused before it reaches any tool.
    let (status, _) = post(
        &agent,
        &url,
        None,
        None,
        serde_json::json!({"jsonrpc":"2.0","id":0,"method":"initialize"}),
    );
    assert_eq!(status, 401, "an untokened request must not be served");

    // Handshake.
    let init_body = serde_json::json!({
        "jsonrpc": "2.0", "id": 1, "method": "initialize",
        "params": {
            "protocolVersion": "2025-06-18",
            "capabilities": {},
            "clientInfo": {"name": "tangt-test", "version": "0"}
        }
    });
    let mut init_req = agent
        .post(&url)
        .header("Accept", "application/json, text/event-stream")
        .header("Authorization", &format!("Bearer {token}"));
    init_req = init_req.header("Content-Type", "application/json");
    let response = init_req.send_json(init_body).expect("initialize completed");
    let status = response.status().as_u16();
    let session = response
        .headers()
        .get("mcp-session-id")
        .and_then(|v| v.to_str().ok())
        .map(str::to_string);
    let init = response.into_body().read_to_string().expect("body");
    assert_eq!(status, 200, "initialize failed: {init}");
    assert!(
        json_payload(&init).contains("tangt") || init.contains("tangt"),
        "server did not identify itself: {init}"
    );
    let session = session.as_deref();

    // Clients must say they are initialized before calling tools.
    post(
        &agent,
        &url,
        Some(&token),
        session,
        serde_json::json!({"jsonrpc":"2.0","method":"notifications/initialized"}),
    );

    // The tool list is what we advertise.
    let (_, listed) = post(
        &agent,
        &url,
        Some(&token),
        session,
        serde_json::json!({"jsonrpc":"2.0","id":2,"method":"tools/list"}),
    );
    for name in tangt_core::tools::TOOL_NAMES {
        assert!(listed.contains(name), "tools/list omitted {name}: {listed}");
    }

    // A read tool round-trips through the bridge to the fake UI thread.
    let (_, tasks) = post(
        &agent,
        &url,
        Some(&token),
        session,
        serde_json::json!({
            "jsonrpc":"2.0","id":3,"method":"tools/call",
            "params":{"name":"list_tasks","arguments":{}}
        }),
    );
    assert!(tasks.contains("Design"), "list_tasks did not reach the document: {tasks}");

    // A write tool actually edits the document.
    let (_, progress) = post(
        &agent,
        &url,
        Some(&token),
        session,
        serde_json::json!({
            "jsonrpc":"2.0","id":4,"method":"tools/call",
            "params":{"name":"set_progress","arguments":{"id":"d","percent":60}}
        }),
    );
    assert!(progress.contains("60"), "set_progress reported oddly: {progress}");

    // And a bad call reports a readable reason instead of a protocol error.
    let (_, missing) = post(
        &agent,
        &url,
        Some(&token),
        session,
        serde_json::json!({
            "jsonrpc":"2.0","id":5,"method":"tools/call",
            "params":{"name":"set_progress","arguments":{"id":"ghost","percent":10}}
        }),
    );
    assert!(missing.contains("ghost"), "expected a readable error: {missing}");

    let final_text = published.lock().expect("publish lock").clone();
    assert_eq!(
        final_text, "task \"Design\" : d, 2026-08-10, 5d, 60% // keep\n",
        "the edit did not land in the document text (comment must survive)"
    );

    shutdown.cancel();
    runtime.shutdown_background();
}
