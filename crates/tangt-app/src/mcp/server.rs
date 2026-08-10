//! The tool router served over HTTP to a client talking to the *running* app.
//! Every tool is a thin shim: it forwards to the UI thread through the bridge
//! and reports whatever `tangt_core::tools` decided.

use super::bridge::{AppBridge, AppCommand};
use rmcp::handler::server::router::tool::ToolRouter;
use rmcp::handler::server::wrapper::Parameters;
use rmcp::model::{CallToolResult, ContentBlock, Implementation, ServerCapabilities, ServerInfo};
use rmcp::{tool, tool_handler, tool_router, ErrorData, ServerHandler};
use schemars::JsonSchema;
use serde::Deserialize;
use std::sync::Arc;
use tangt_core::tools::{AddTaskArgs, SetDatesArgs};

pub const INSTRUCTIONS: &str = "tangt holds a Gantt chart written in a plain-text DSL. \
    Read it with list_tasks, then change it with the editing tools — each edit rewrites \
    the underlying text and is undoable in the app. Dates are always YYYY-MM-DD. \
    Durations count working days when the chart excludes weekends. Prefer expressing \
    sequencing with `after` (add_dependency) over hard-coded dates, so the schedule \
    reflows when something slips.";

/// A tool that cannot run reports why *inside* the result, so the model can
/// read the reason and try something else, rather than seeing a protocol error.
pub(super) fn outcome(result: Result<serde_json::Value, String>) -> Result<CallToolResult, ErrorData> {
    Ok(match result {
        Ok(value) => CallToolResult::success(vec![ContentBlock::text(value.to_string())]),
        Err(message) => CallToolResult::error(vec![ContentBlock::text(message)]),
    })
}

// ---- argument schemas (doc comments become the model-facing descriptions) --

#[derive(Debug, Deserialize, JsonSchema)]
pub struct AddTaskParams {
    /// Label shown on the bar, e.g. "Design review".
    pub label: String,
    /// Start date as YYYY-MM-DD. Omit when using `after`.
    pub start: Option<String>,
    /// Ids of tasks this one starts after. Draws dependency arrows.
    #[serde(default)]
    pub after: Vec<String>,
    /// Working days of slack after the predecessor ends (negative to overlap).
    pub lag_days: Option<i64>,
    /// Duration in working days. Defaults to 5. Ignored for milestones.
    pub duration_days: Option<i64>,
    /// True for a zero-length milestone diamond instead of a bar.
    #[serde(default)]
    pub milestone: bool,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct SetDatesParams {
    /// Id of the task to reschedule.
    pub id: String,
    /// New start date, YYYY-MM-DD.
    pub start: Option<String>,
    /// New duration in working days.
    pub duration_days: Option<i64>,
    /// New inclusive end date, YYYY-MM-DD. Use instead of duration_days.
    pub end: Option<String>,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct IdParams {
    /// Id of the task.
    pub id: String,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct RenameParams {
    /// Id of the task to rename.
    pub id: String,
    /// The new label.
    pub label: String,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct ProgressParams {
    /// Id of the task.
    pub id: String,
    /// Completion, 0-100.
    pub percent: i64,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct DependencyParams {
    /// The dependent task, which will start after the other one ends.
    pub task: String,
    /// The task that must finish first.
    pub after: String,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct RemoveDependencyParams {
    /// The dependent task.
    pub task: String,
    /// The predecessor to detach from.
    pub on: String,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct SectionParams {
    /// Heading text for the new section.
    pub label: String,
}

#[derive(Clone)]
pub struct TangtMcp {
    bridge: Arc<AppBridge>,
    tool_router: ToolRouter<Self>,
}

impl TangtMcp {
    pub fn new(bridge: Arc<AppBridge>) -> Self {
        TangtMcp { bridge, tool_router: Self::tool_router() }
    }
}

#[tool_router]
impl TangtMcp {
    /// List every task in the chart with its resolved schedule: id, label,
    /// start, end, duration in working days, progress, dependencies and tags.
    /// Call this first — the ids it returns are what the editing tools take.
    #[tool]
    async fn list_tasks(&self) -> Result<CallToolResult, ErrorData> {
        outcome(self.bridge.submit(|reply| AppCommand::ListTasks { reply }).await)
    }

    /// Read the chart's raw DSL source text.
    #[tool]
    async fn get_source(&self) -> Result<CallToolResult, ErrorData> {
        outcome(self.bridge.submit(|reply| AppCommand::GetSource { reply }).await)
    }

    /// Add a task or milestone to the end of the chart.
    #[tool]
    async fn add_task(
        &self,
        Parameters(p): Parameters<AddTaskParams>,
    ) -> Result<CallToolResult, ErrorData> {
        let args = AddTaskArgs {
            label: p.label,
            start: p.start,
            after: p.after,
            lag_days: p.lag_days,
            duration_days: p.duration_days,
            milestone: p.milestone,
            after_task: None,
        };
        outcome(
            self.bridge
                .submit(|reply| AppCommand::AddTask { args: Box::new(args), reply })
                .await,
        )
    }

    /// Reschedule a task: change its start, its duration, or its end date.
    #[tool]
    async fn set_dates(
        &self,
        Parameters(p): Parameters<SetDatesParams>,
    ) -> Result<CallToolResult, ErrorData> {
        let args = SetDatesArgs {
            id: p.id,
            start: p.start,
            duration_days: p.duration_days,
            end: p.end,
        };
        outcome(
            self.bridge
                .submit(|reply| AppCommand::SetDates { args: Box::new(args), reply })
                .await,
        )
    }

    /// Change a task's label.
    #[tool]
    async fn rename_task(
        &self,
        Parameters(p): Parameters<RenameParams>,
    ) -> Result<CallToolResult, ErrorData> {
        outcome(
            self.bridge
                .submit(|reply| AppCommand::RenameTask { id: p.id, label: p.label, reply })
                .await,
        )
    }

    /// Set a task's completion percentage (0-100).
    #[tool]
    async fn set_progress(
        &self,
        Parameters(p): Parameters<ProgressParams>,
    ) -> Result<CallToolResult, ErrorData> {
        outcome(
            self.bridge
                .submit(|reply| AppCommand::SetProgress { id: p.id, percent: p.percent, reply })
                .await,
        )
    }

    /// Make one task start after another finishes, drawing a dependency arrow.
    /// Refused if it would create a cycle.
    #[tool]
    async fn add_dependency(
        &self,
        Parameters(p): Parameters<DependencyParams>,
    ) -> Result<CallToolResult, ErrorData> {
        outcome(
            self.bridge
                .submit(|reply| AppCommand::AddDependency {
                    task: p.task,
                    after: p.after,
                    reply,
                })
                .await,
        )
    }

    /// Detach a dependency. If it was the task's only one, the task keeps its
    /// current start date so the chart does not jump.
    #[tool]
    async fn remove_dependency(
        &self,
        Parameters(p): Parameters<RemoveDependencyParams>,
    ) -> Result<CallToolResult, ErrorData> {
        outcome(
            self.bridge
                .submit(|reply| AppCommand::RemoveDependency { task: p.task, on: p.on, reply })
                .await,
        )
    }

    /// Delete a task. Any other task that depended on it keeps its dates.
    #[tool]
    async fn delete_task(
        &self,
        Parameters(p): Parameters<IdParams>,
    ) -> Result<CallToolResult, ErrorData> {
        outcome(
            self.bridge
                .submit(|reply| AppCommand::DeleteTask { id: p.id, reply })
                .await,
        )
    }

    /// Append a section heading, which groups the tasks that follow it.
    #[tool]
    async fn add_section(
        &self,
        Parameters(p): Parameters<SectionParams>,
    ) -> Result<CallToolResult, ErrorData> {
        outcome(
            self.bridge
                .submit(|reply| AppCommand::AddSection { label: p.label, reply })
                .await,
        )
    }
}

#[tool_handler(router = self.tool_router)]
impl ServerHandler for TangtMcp {
    fn get_info(&self) -> ServerInfo {
        ServerInfo::new(ServerCapabilities::builder().enable_tools().build())
            .with_server_info(Implementation::new("tangt", env!("CARGO_PKG_VERSION")))
            .with_instructions(INSTRUCTIONS)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_tool_is_offered_with_a_description_and_schema() {
        let (tx, _rx) = std::sync::mpsc::channel();
        let bridge = Arc::new(AppBridge::new(tx, eframe::egui::Context::default()));
        let mcp = TangtMcp::new(bridge);
        let tools = mcp.tool_router.list_all();

        let mut names: Vec<&str> = tools.iter().map(|t| t.name.as_ref()).collect();
        names.sort_unstable();
        // A frozen contract: renaming a tool breaks every client config.
        assert_eq!(
            names,
            vec![
                "add_dependency",
                "add_section",
                "add_task",
                "delete_task",
                "get_source",
                "list_tasks",
                "remove_dependency",
                "rename_task",
                "set_dates",
                "set_progress",
            ]
        );
        for tool in &tools {
            assert!(
                tool.description.as_ref().is_some_and(|d| d.len() > 20),
                "{} has no usable description",
                tool.name
            );
            assert!(
                tool.input_schema.contains_key("properties")
                    || tool.name == "list_tasks"
                    || tool.name == "get_source",
                "{} has no input schema",
                tool.name
            );
        }
    }

    #[test]
    fn tool_set_matches_core() {
        let (tx, _rx) = std::sync::mpsc::channel();
        let bridge = Arc::new(AppBridge::new(tx, eframe::egui::Context::default()));
        let mcp = TangtMcp::new(bridge);
        assert_eq!(mcp.tool_router.list_all().len(), tangt_core::tools::TOOL_NAMES.len());
    }
}
