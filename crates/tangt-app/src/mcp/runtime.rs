//! The loopback HTTP transport: a tokio runtime on its own thread, an axum
//! router wrapping rmcp's streamable-HTTP service, and a bearer-token gate.
//! Dropping the handle cancels and joins, so the port is free before a
//! restart rebinds it.

use super::bridge::AppBridge;
use super::server::TangtMcp;
use super::token_matches;
use axum::body::Body;
use axum::extract::Request;
use axum::http::{header, StatusCode};
use axum::middleware::Next;
use axum::response::Response;
use eframe::egui;
use rmcp::transport::streamable_http_server::session::local::LocalSessionManager;
use rmcp::transport::streamable_http_server::{StreamableHttpServerConfig, StreamableHttpService};
use std::sync::{Arc, Mutex};
use tokio_util::sync::CancellationToken;

/// What the Preferences pane shows about the server.
#[derive(Clone, Default, PartialEq, Eq, Debug)]
pub struct McpStatus {
    pub listening: Option<u16>,
    pub error: Option<String>,
}

pub struct McpServer {
    cancel: CancellationToken,
    thread: Option<std::thread::JoinHandle<()>>,
    pub status: Arc<Mutex<McpStatus>>,
    pub port: u16,
    pub token: String,
}

impl McpServer {
    pub fn spawn(
        bridge: Arc<AppBridge>,
        port: u16,
        token: String,
        ctx: egui::Context,
    ) -> McpServer {
        let cancel = CancellationToken::new();
        let status = Arc::new(Mutex::new(McpStatus::default()));
        let thread = {
            let (cancel, status, token) = (cancel.clone(), status.clone(), token.clone());
            std::thread::Builder::new()
                .name("tangt-mcp".into())
                .spawn(move || {
                    let runtime = match tokio::runtime::Builder::new_multi_thread()
                        .worker_threads(2)
                        .enable_all()
                        .build()
                    {
                        Ok(rt) => rt,
                        Err(e) => {
                            set_error(&status, format!("could not start the MCP runtime: {e}"));
                            ctx.request_repaint();
                            return;
                        }
                    };
                    runtime.block_on(serve(bridge, port, token, cancel, &status, &ctx));
                })
                .expect("spawning a thread")
        };
        McpServer { cancel, thread: Some(thread), status, port, token }
    }

    pub fn status(&self) -> McpStatus {
        self.status.lock().expect("status lock").clone()
    }
}

impl Drop for McpServer {
    fn drop(&mut self) {
        self.cancel.cancel();
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

fn set_error(status: &Arc<Mutex<McpStatus>>, message: String) {
    *status.lock().expect("status lock") =
        McpStatus { listening: None, error: Some(message) };
}

async fn serve(
    bridge: Arc<AppBridge>,
    port: u16,
    token: String,
    cancel: CancellationToken,
    status: &Arc<Mutex<McpStatus>>,
    ctx: &egui::Context,
) {
    // Loopback only, and only IPv4: binding 0.0.0.0 would put the chart on
    // every network this machine is on, which is not what "let my assistant
    // reach tangt" asks for.
    let listener = match tokio::net::TcpListener::bind(("127.0.0.1", port)).await {
        Ok(listener) => listener,
        Err(e) => {
            set_error(
                status,
                format!(
                    "could not listen on 127.0.0.1:{port}: {e}. Another program may already \
                     be using that port; try a different one."
                ),
            );
            ctx.request_repaint();
            return;
        }
    };
    let bound = listener.local_addr().map(|a| a.port()).unwrap_or(port);
    *status.lock().expect("status lock") =
        McpStatus { listening: Some(bound), error: None };
    ctx.request_repaint();

    let app = router(bridge, token, StreamableHttpServerConfig::default());
    let served = axum::serve(listener, app)
        .with_graceful_shutdown(async move { cancel.cancelled_owned().await })
        .await;
    if let Err(e) = served {
        set_error(status, format!("the MCP server stopped: {e}"));
    } else {
        *status.lock().expect("status lock") = McpStatus::default();
    }
    ctx.request_repaint();
}

pub fn router(
    bridge: Arc<AppBridge>,
    token: String,
    config: StreamableHttpServerConfig,
) -> axum::Router {
    let service: StreamableHttpService<TangtMcp, LocalSessionManager> =
        StreamableHttpService::new(
            move || Ok(TangtMcp::new(bridge.clone())),
            Arc::new(LocalSessionManager::default()),
            config,
        );
    let token = Arc::<str>::from(token);
    axum::Router::new()
        .nest_service("/mcp", service)
        .layer(axum::middleware::from_fn(move |req, next| {
            authorize(token.clone(), req, next)
        }))
}

/// Everything on the port needs the token: a loopback port is reachable by
/// every program on the machine, so "it is only local" is not a policy.
async fn authorize(expected: Arc<str>, req: Request, next: Next) -> Response {
    let presented = req
        .headers()
        .get(header::AUTHORIZATION)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.strip_prefix("Bearer "))
        .map(str::trim);
    match presented {
        Some(token) if token_matches(&expected, token) => next.run(req).await,
        _ => (
            StatusCode::UNAUTHORIZED,
            [(header::WWW_AUTHENTICATE, "Bearer")],
            Body::from(
                "tangt's MCP server needs the token from Preferences ▸ MCP, sent as \
                 `Authorization: Bearer <token>`.",
            ),
        )
            .into_response(),
    }
}

use axum::response::IntoResponse;
