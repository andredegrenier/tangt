//! The seam between the async MCP server and the synchronous egui app.
//!
//! The server runs on its own tokio runtime; the document lives on the UI
//! thread and is only touched there. A tool call sends an `AppCommand` with a
//! oneshot reply channel, nudges egui awake, and waits. The UI drains the
//! queue each frame and answers.

use eframe::egui;
use std::sync::mpsc::Sender;
use std::sync::Mutex;
use std::time::Duration;
use tangt_core::tools::{AddTaskArgs, SetDatesArgs};

pub type Reply = tokio::sync::oneshot::Sender<Result<serde_json::Value, String>>;

/// How long a tool call waits for the UI thread. Long enough for a busy
/// frame, short enough that a modal dialog reports rather than hangs.
pub const TIMEOUT: Duration = Duration::from_secs(10);

const CLOSED: &str = "tangt is no longer accepting commands (the window may have closed)";

pub enum AppCommand {
    ListTasks { reply: Reply },
    GetSource { reply: Reply },
    AddTask { args: Box<AddTaskArgs>, reply: Reply },
    SetDates { args: Box<SetDatesArgs>, reply: Reply },
    RenameTask { id: String, label: String, reply: Reply },
    SetProgress { id: String, percent: i64, reply: Reply },
    AddDependency { task: String, after: String, reply: Reply },
    RemoveDependency { task: String, on: String, reply: Reply },
    DeleteTask { id: String, reply: Reply },
    AddSection { label: String, reply: Reply },
}

pub struct AppBridge {
    // `Sender` is Send but not Sync, and the rmcp router shares one handler
    // across worker tasks.
    tx: Mutex<Sender<AppCommand>>,
    egui: egui::Context,
}

impl AppBridge {
    pub fn new(tx: Sender<AppCommand>, egui: egui::Context) -> Self {
        AppBridge { tx: Mutex::new(tx), egui }
    }

    pub async fn submit(
        &self,
        build: impl FnOnce(Reply) -> AppCommand,
    ) -> Result<serde_json::Value, String> {
        let (reply, rx) = tokio::sync::oneshot::channel();
        {
            let tx = self.tx.lock().map_err(|_| CLOSED.to_owned())?;
            tx.send(build(reply)).map_err(|_| CLOSED.to_owned())?;
        }
        // An idle window never runs another frame, so it would never drain
        // the queue without this.
        self.egui.request_repaint();
        match tokio::time::timeout(TIMEOUT, rx).await {
            Ok(Ok(result)) => result,
            Ok(Err(_)) => Err(CLOSED.to_owned()),
            Err(_) => Err(
                "tangt did not respond within 10s — a dialog may be open, or the window busy"
                    .to_owned(),
            ),
        }
    }
}
