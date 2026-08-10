//! `tangt mcp-serve <file.tangt>`: the same tools over stdio, without a
//! window, for clients that would rather start their own process.
//!
//! This half owns the document itself and writes the file after every
//! mutation — there is no UI thread to answer, and nobody watching a canvas.

use rmcp::handler::server::router::tool::ToolRouter;
use rmcp::handler::server::wrapper::Parameters;
use rmcp::model::{CallToolResult, Implementation, ServerCapabilities, ServerInfo};
use rmcp::{tool, tool_handler, tool_router, ErrorData, ServerHandler, ServiceExt};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use tangt_core::document::Document;
use tangt_core::ops::Op;
use tangt_core::resolve::today_local;
use tangt_core::tools::{self, AddTaskArgs, SetDatesArgs, ToolOutcome};

use super::server::{
    outcome, AddTaskParams, DependencyParams, IdParams, ProgressParams, RemoveDependencyParams,
    RenameParams, SectionParams, SetDatesParams, INSTRUCTIONS,
};

/// Entry point for the `mcp-serve` argv personality. Never returns normally.
pub fn main(path: PathBuf) -> ! {
    let text = match std::fs::read_to_string(&path) {
        Ok(text) => text,
        // A missing file is the "start a new plan" case, not an error.
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => String::new(),
        Err(e) => {
            eprintln!("tangt: cannot read {}: {e}", path.display());
            std::process::exit(1);
        }
    };
    let server = TangtFileMcp::new(Document::new(text, today_local()), path);
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("building the stdio runtime");
    let code = runtime.block_on(async move {
        match server.serve(rmcp::transport::stdio()).await {
            Ok(service) => {
                let _ = service.waiting().await;
                0
            }
            Err(e) => {
                eprintln!("tangt: MCP server failed: {e}");
                1
            }
        }
    });
    std::process::exit(code)
}

#[derive(Clone)]
pub struct TangtFileMcp {
    doc: Arc<Mutex<Document>>,
    path: Arc<PathBuf>,
    tool_router: ToolRouter<Self>,
}

impl TangtFileMcp {
    pub fn new(doc: Document, path: PathBuf) -> Self {
        TangtFileMcp {
            doc: Arc::new(Mutex::new(doc)),
            path: Arc::new(path),
            tool_router: Self::tool_router(),
        }
    }

    /// Run a tool against the document, applying and saving any edits.
    fn run(
        &self,
        f: impl FnOnce(&Document) -> tools::ToolResult,
    ) -> Result<serde_json::Value, String> {
        let mut doc = self.doc.lock().map_err(|_| "document lock poisoned".to_string())?;
        match f(&doc)? {
            ToolOutcome::Read(value) => Ok(value),
            ToolOutcome::Write { edits, intent, summary } => {
                let seq = doc.next_seq();
                if doc.apply(Op { edits, intent, actor: 0, seq })
                    != tangt_core::document::ApplyResult::Applied
                {
                    return Err("the edit did not apply cleanly; the chart is unchanged".into());
                }
                std::fs::write(self.path.as_path(), doc.text())
                    .map_err(|e| format!("wrote the chart but could not save it: {e}"))?;
                doc.mark_saved();
                Ok(summary)
            }
        }
    }
}

#[tool_router]
impl TangtFileMcp {
    /// List every task in the chart with its resolved schedule: id, label,
    /// start, end, duration in working days, progress, dependencies and tags.
    /// Call this first — the ids it returns are what the editing tools take.
    #[tool]
    fn list_tasks(&self) -> Result<CallToolResult, ErrorData> {
        outcome(self.run(|d| tools::list_tasks(d.model())))
    }

    /// Read the chart's raw DSL source text.
    #[tool]
    fn get_source(&self) -> Result<CallToolResult, ErrorData> {
        outcome(self.run(|d| tools::get_source(d.text())))
    }

    /// Add a task or milestone to the end of the chart.
    #[tool]
    fn add_task(
        &self,
        Parameters(p): Parameters<AddTaskParams>,
    ) -> Result<CallToolResult, ErrorData> {
        outcome(self.run(|d| {
            tools::add_task(
                d.model(),
                d.text(),
                AddTaskArgs {
                    label: p.label,
                    start: p.start,
                    after: p.after,
                    lag_days: p.lag_days,
                    duration_days: p.duration_days,
                    milestone: p.milestone,
                    after_task: None,
                },
            )
        }))
    }

    /// Reschedule a task: change its start, its duration, or its end date.
    #[tool]
    fn set_dates(
        &self,
        Parameters(p): Parameters<SetDatesParams>,
    ) -> Result<CallToolResult, ErrorData> {
        outcome(self.run(|d| {
            tools::set_dates(
                d.model(),
                SetDatesArgs {
                    id: p.id,
                    start: p.start,
                    duration_days: p.duration_days,
                    end: p.end,
                },
            )
        }))
    }

    /// Change a task's label.
    #[tool]
    fn rename_task(
        &self,
        Parameters(p): Parameters<RenameParams>,
    ) -> Result<CallToolResult, ErrorData> {
        outcome(self.run(|d| tools::rename_task(d.model(), &p.id, &p.label)))
    }

    /// Set a task's completion percentage (0-100).
    #[tool]
    fn set_progress(
        &self,
        Parameters(p): Parameters<ProgressParams>,
    ) -> Result<CallToolResult, ErrorData> {
        outcome(self.run(|d| tools::set_progress(d.model(), &p.id, p.percent)))
    }

    /// Make one task start after another finishes, drawing a dependency arrow.
    /// Refused if it would create a cycle.
    #[tool]
    fn add_dependency(
        &self,
        Parameters(p): Parameters<DependencyParams>,
    ) -> Result<CallToolResult, ErrorData> {
        outcome(self.run(|d| tools::add_dependency(d.model(), &p.task, &p.after)))
    }

    /// Detach a dependency. If it was the task's only one, the task keeps its
    /// current start date so the chart does not jump.
    #[tool]
    fn remove_dependency(
        &self,
        Parameters(p): Parameters<RemoveDependencyParams>,
    ) -> Result<CallToolResult, ErrorData> {
        outcome(self.run(|d| tools::remove_dependency(d.model(), d.text(), &p.task, &p.on)))
    }

    /// Delete a task. Any other task that depended on it keeps its dates.
    #[tool]
    fn delete_task(
        &self,
        Parameters(p): Parameters<IdParams>,
    ) -> Result<CallToolResult, ErrorData> {
        outcome(self.run(|d| tools::delete_task(d.model(), d.text(), &p.id)))
    }

    /// Append a section heading, which groups the tasks that follow it.
    #[tool]
    fn add_section(
        &self,
        Parameters(p): Parameters<SectionParams>,
    ) -> Result<CallToolResult, ErrorData> {
        outcome(self.run(|d| tools::add_section(d.text(), &p.label)))
    }
}

#[tool_handler(router = self.tool_router)]
impl ServerHandler for TangtFileMcp {
    fn get_info(&self) -> ServerInfo {
        ServerInfo::new(ServerCapabilities::builder().enable_tools().build())
            .with_server_info(Implementation::new("tangt", env!("CARGO_PKG_VERSION")))
            .with_instructions(INSTRUCTIONS)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_chart(name: &str, contents: &str) -> PathBuf {
        let path = std::env::temp_dir().join(format!("tangt-mcp-{name}.tangt"));
        std::fs::write(&path, contents).unwrap();
        path
    }

    #[test]
    fn editing_tools_persist_to_the_file() {
        let path = temp_chart("persist", "task \"A\" : a, 2026-08-10, 5d // keep\n");
        let server = TangtFileMcp::new(
            Document::new(std::fs::read_to_string(&path).unwrap(), 20675),
            path.clone(),
        );

        let summary = server
            .run(|d| tools::set_progress(d.model(), "a", 75))
            .expect("tool ran");
        assert_eq!(summary["progress_percent"], 75);

        // The file on disk carries the edit, comment intact.
        let saved = std::fs::read_to_string(&path).unwrap();
        assert_eq!(saved, "task \"A\" : a, 2026-08-10, 5d, 75% // keep\n");

        // And a failing tool reports rather than corrupting anything.
        let err = server
            .run(|d| tools::set_progress(d.model(), "ghost", 10))
            .unwrap_err();
        assert!(err.contains("ghost"), "{err}");
        assert_eq!(std::fs::read_to_string(&path).unwrap(), saved);
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn a_missing_file_starts_an_empty_chart() {
        let path = std::env::temp_dir().join("tangt-mcp-absent.tangt");
        let _ = std::fs::remove_file(&path);
        let server = TangtFileMcp::new(Document::new(String::new(), 20675), path.clone());
        let summary = server
            .run(|d| {
                tools::add_task(
                    d.model(),
                    d.text(),
                    AddTaskArgs { label: "Kickoff".into(), ..Default::default() },
                )
            })
            .expect("tool ran");
        assert_eq!(summary["label"], "Kickoff");
        assert!(std::fs::read_to_string(&path).unwrap().contains("Kickoff"));
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn headless_offers_the_same_tools_as_the_app() {
        let server = TangtFileMcp::new(Document::new(String::new(), 20675), PathBuf::from("x"));
        let mut names: Vec<String> = server
            .tool_router
            .list_all()
            .iter()
            .map(|t| t.name.to_string())
            .collect();
        names.sort();
        let mut expected: Vec<String> =
            tangt_core::tools::TOOL_NAMES.iter().map(|s| s.to_string()).collect();
        expected.sort();
        assert_eq!(names, expected);
    }
}
