//! Application shell: split editor/chart layout with swappable sides, menu
//! bar, global shortcuts, file lifecycle, and the text<->canvas sync loop.

use crate::camera::GanttCamera;
use crate::canvas::{self, CanvasEvent, CanvasState};
use crate::editor::EditorPane;
use crate::files;
use crate::theme::Theme;
use eframe::egui::{self, Color32, FontId, Key, KeyboardShortcut, Modifiers};
use std::path::PathBuf;
use tangt_core::document::Document;
use tangt_core::geometry::TextMeasurer;
use tangt_core::layout::{self, GanttLayout};
use tangt_core::model::TaskId;
use tangt_core::ops::{Intent, Op};
use tangt_core::resolve::today_local;
use tangt_core::rewrite;

const EXAMPLE: &str = include_str!("../../../examples/website-redesign.tangt");

// Shortcuts defined once; labels come from `ctx.format_shortcut` so they read
// "⌘⇧L" on macOS and "Ctrl+Shift+L" on Windows/Linux.
const SC_NEW: KeyboardShortcut = KeyboardShortcut::new(Modifiers::COMMAND, Key::N);
const SC_OPEN: KeyboardShortcut = KeyboardShortcut::new(Modifiers::COMMAND, Key::O);
const SC_SAVE: KeyboardShortcut = KeyboardShortcut::new(Modifiers::COMMAND, Key::S);
const SC_SAVE_AS: KeyboardShortcut =
    KeyboardShortcut::new(Modifiers::COMMAND.plus(Modifiers::SHIFT), Key::S);
const SC_EDITOR: KeyboardShortcut = KeyboardShortcut::new(Modifiers::COMMAND, Key::Num1);
const SC_SWAP: KeyboardShortcut =
    KeyboardShortcut::new(Modifiers::COMMAND.plus(Modifiers::SHIFT), Key::L);
const SC_HELP: KeyboardShortcut = KeyboardShortcut::new(Modifiers::COMMAND, Key::Slash);
const SC_ASSIST: KeyboardShortcut = KeyboardShortcut::new(Modifiers::COMMAND, Key::Num2);

/// Action deferred behind the unsaved-changes confirmation.
#[derive(Clone)]
enum PendingAction {
    New,
    Open(Option<PathBuf>),
}

pub struct EguiMeasurer<'a>(pub &'a egui::Context);

impl TextMeasurer for EguiMeasurer<'_> {
    fn measure(&self, text: &str, size: f32) -> [f32; 2] {
        let galley = self.0.fonts_mut(|f| {
            f.layout(
                text.to_string(),
                FontId::proportional(size),
                Color32::WHITE,
                f32::INFINITY,
            )
        });
        [galley.size().x, galley.size().y]
    }
}

#[derive(serde::Serialize, serde::Deserialize)]
struct Persisted {
    recents: Vec<PathBuf>,
    last_file: Option<PathBuf>,
    show_editor: bool,
    editor_on_left: bool,
    camera: Option<GanttCamera>,
    #[serde(default)]
    mcp: crate::mcp::McpPrefs,
    #[serde(default)]
    assist: crate::llm::AssistPrefs,
}

impl Default for Persisted {
    fn default() -> Self {
        Persisted {
            recents: Vec::new(),
            last_file: None,
            show_editor: true,
            editor_on_left: true,
            camera: None,
            mcp: crate::mcp::McpPrefs::default(),
            assist: crate::llm::AssistPrefs::default(),
        }
    }
}

pub struct TangtApp {
    doc: Document,
    path: Option<PathBuf>,
    layout: GanttLayout,
    camera: GanttCamera,
    editor: EditorPane,
    help: crate::help::HelpWindow,
    canvas_state: CanvasState,
    selection: Option<TaskId>,
    recents: Vec<PathBuf>,
    show_editor: bool,
    editor_on_left: bool,
    confirm_close: bool,
    pending_discard: Option<PendingAction>,
    fit_requested: bool,
    status: String,
    last_title: String,
    // --- MCP ---
    mcp_prefs: crate::mcp::McpPrefs,
    mcp_server: Option<crate::mcp::runtime::McpServer>,
    mcp_rx: Option<std::sync::mpsc::Receiver<crate::mcp::bridge::AppCommand>>,
    show_prefs: bool,
    assist: crate::llm::AssistEngine,
}

impl TangtApp {
    pub fn new(cc: &eframe::CreationContext<'_>) -> Self {
        let persisted: Persisted = cc
            .storage
            .and_then(|s| eframe::get_value(s, "tangt"))
            .unwrap_or_default();

        let doc = Document::new(EXAMPLE.to_string(), today_local());
        let layout = layout::compute_default(doc.model(), &tangt_core::geometry::MonoMeasurer);
        let fit_requested = persisted.camera.is_none();
        let mut app = TangtApp {
            doc,
            path: None,
            layout,
            camera: persisted.camera.unwrap_or_default().sanitized(),
            editor: EditorPane::new(),
            help: crate::help::HelpWindow::new(),
            canvas_state: CanvasState::default(),
            selection: None,
            recents: persisted.recents,
            show_editor: persisted.show_editor,
            editor_on_left: persisted.editor_on_left,
            confirm_close: false,
            pending_discard: None,
            fit_requested,
            status: String::new(),
            last_title: String::new(),
            mcp_prefs: persisted.mcp,
            mcp_server: None,
            mcp_rx: None,
            show_prefs: false,
            assist: crate::llm::AssistEngine::spawn(&cc.egui_ctx, persisted.assist),
        };
        if let Some(path) = persisted.last_file.clone() {
            app.load(path);
        }
        app
    }

    fn load(&mut self, path: PathBuf) {
        match files::open_document(&path) {
            Ok(doc) => {
                self.doc = doc;
                files::push_recent(&mut self.recents, &path);
                self.path = Some(path);
                self.selection = None;
                self.fit_requested = true;
                self.status = "Opened".into();
            }
            Err(e) => self.status = format!("Open failed: {e}"),
        }
    }

    fn save(&mut self) {
        let path = match &self.path {
            Some(p) => p.clone(),
            None => match files::pick_save() {
                Some(p) => p,
                None => return,
            },
        };
        match files::save_document(&mut self.doc, &path) {
            Ok(()) => {
                files::push_recent(&mut self.recents, &path);
                self.path = Some(path);
                self.status = "Saved".into();
            }
            Err(e) => self.status = format!("Save failed: {e}"),
        }
    }

    /// Start, stop, or restart the MCP server so it matches the prefs.
    fn reconcile_mcp(&mut self, ctx: &egui::Context) {
        let running = self
            .mcp_server
            .as_ref()
            .map(|s| (s.port, s.token.clone()));
        let wanted = self
            .mcp_prefs
            .server_enabled
            .then(|| (self.mcp_prefs.port, self.mcp_prefs.token.clone()));
        if running == wanted {
            return;
        }
        // Drop first: `Drop` cancels and joins, so the port is free before we
        // rebind it.
        self.mcp_server = None;
        if let Some((port, token)) = wanted {
            let (tx, rx) = std::sync::mpsc::channel();
            let bridge =
                std::sync::Arc::new(crate::mcp::bridge::AppBridge::new(tx, ctx.clone()));
            self.mcp_rx = Some(rx);
            self.mcp_server = Some(crate::mcp::runtime::McpServer::spawn(
                bridge,
                port,
                token,
                ctx.clone(),
            ));
        } else {
            self.mcp_rx = None;
        }
    }

    /// Tool definitions handed to the model — same set, same descriptions as
    /// MCP, generated from one place so the two can't drift.
    fn assist_tools(&self) -> Vec<crate::llm::model::ToolDef> {
        use serde_json::json;
        let s = |props: serde_json::Value, required: &[&str]| {
            json!({ "type": "object", "properties": props, "required": required })
        };
        vec![
            crate::llm::model::ToolDef {
                name: "list_tasks".into(),
                description: "List every task with id, label, start, end, duration in working \
                    days, progress, dependencies and tags. Call this before editing."
                    .into(),
                schema: s(json!({}), &[]),
            },
            crate::llm::model::ToolDef {
                name: "add_task".into(),
                description: "Add a task or milestone to the end of the chart.".into(),
                schema: s(
                    json!({
                        "label": {"type": "string", "description": "Label shown on the bar"},
                        "start": {"type": "string", "description": "Start date YYYY-MM-DD; omit when using after"},
                        "after": {"type": "array", "items": {"type": "string"}, "description": "Ids this task starts after"},
                        "lag_days": {"type": "integer", "description": "Working days of slack after the predecessor"},
                        "duration_days": {"type": "integer", "description": "Working days; defaults to 5"},
                        "milestone": {"type": "boolean", "description": "Zero-length marker instead of a bar"}
                    }),
                    &["label"],
                ),
            },
            crate::llm::model::ToolDef {
                name: "set_dates".into(),
                description: "Reschedule a task: change its start, duration, or end date.".into(),
                schema: s(
                    json!({
                        "id": {"type": "string"},
                        "start": {"type": "string", "description": "YYYY-MM-DD"},
                        "duration_days": {"type": "integer"},
                        "end": {"type": "string", "description": "Inclusive end date YYYY-MM-DD"}
                    }),
                    &["id"],
                ),
            },
            crate::llm::model::ToolDef {
                name: "rename_task".into(),
                description: "Change a task's label.".into(),
                schema: s(
                    json!({"id": {"type": "string"}, "label": {"type": "string"}}),
                    &["id", "label"],
                ),
            },
            crate::llm::model::ToolDef {
                name: "set_progress".into(),
                description: "Set a task's completion percentage, 0-100.".into(),
                schema: s(
                    json!({"id": {"type": "string"}, "percent": {"type": "integer"}}),
                    &["id", "percent"],
                ),
            },
            crate::llm::model::ToolDef {
                name: "add_dependency".into(),
                description: "Make one task start after another finishes. Refused if it would \
                    create a cycle."
                    .into(),
                schema: s(
                    json!({"task": {"type": "string"}, "after": {"type": "string"}}),
                    &["task", "after"],
                ),
            },
            crate::llm::model::ToolDef {
                name: "remove_dependency".into(),
                description: "Detach a dependency, keeping the task's current dates.".into(),
                schema: s(
                    json!({"task": {"type": "string"}, "on": {"type": "string"}}),
                    &["task", "on"],
                ),
            },
            crate::llm::model::ToolDef {
                name: "delete_task".into(),
                description: "Delete a task.".into(),
                schema: s(json!({"id": {"type": "string"}}), &["id"]),
            },
            crate::llm::model::ToolDef {
                name: "add_section".into(),
                description: "Append a section heading that groups the tasks after it.".into(),
                schema: s(json!({"label": {"type": "string"}}), &["label"]),
            },
        ]
    }

    /// Run one assistant tool call against the document.
    fn run_assist_tool(&mut self, call: &crate::llm::model::ToolCall) -> Result<String, String> {
        use tangt_core::tools;
        let a = &call.arguments;
        let str_arg = |key: &str| -> Result<String, String> {
            a.get(key)
                .and_then(|v| v.as_str())
                .map(str::to_string)
                .ok_or_else(|| format!("{} needs a \"{key}\" string", call.name))
        };
        let result = match call.name.as_str() {
            "list_tasks" => tools::list_tasks(self.doc.model()),
            "get_source" => tools::get_source(self.doc.text()),
            "add_task" => {
                let args: tools::AddTaskArgs = serde_json::from_value(a.clone())
                    .map_err(|e| format!("add_task arguments were not usable: {e}"))?;
                tools::add_task(self.doc.model(), self.doc.text(), args)
            }
            "set_dates" => {
                let args: tools::SetDatesArgs = serde_json::from_value(a.clone())
                    .map_err(|e| format!("set_dates arguments were not usable: {e}"))?;
                tools::set_dates(self.doc.model(), args)
            }
            "rename_task" => {
                tools::rename_task(self.doc.model(), &str_arg("id")?, &str_arg("label")?)
            }
            "set_progress" => {
                let pct = a
                    .get("percent")
                    .and_then(|v| v.as_i64())
                    .ok_or("set_progress needs an integer \"percent\"")?;
                tools::set_progress(self.doc.model(), &str_arg("id")?, pct)
            }
            "add_dependency" => {
                tools::add_dependency(self.doc.model(), &str_arg("task")?, &str_arg("after")?)
            }
            "remove_dependency" => tools::remove_dependency(
                self.doc.model(),
                self.doc.text(),
                &str_arg("task")?,
                &str_arg("on")?,
            ),
            "delete_task" => {
                tools::delete_task(self.doc.model(), self.doc.text(), &str_arg("id")?)
            }
            "add_section" => tools::add_section(self.doc.text(), &str_arg("label")?),
            other => Err(format!(
                "there is no tool called \"{other}\"; the chart tools are: {}",
                tools::TOOL_NAMES.join(", ")
            )),
        };
        match result? {
            tools::ToolOutcome::Read(value) => Ok(value.to_string()),
            tools::ToolOutcome::Write { edits, intent, summary } => {
                self.apply_op(edits, intent);
                Ok(summary.to_string())
            }
        }
    }

    fn drain_assist(&mut self) {
        let mut pending = Vec::new();
        while let Ok(request) = self.assist.tool_rx.try_recv() {
            pending.push(request);
        }
        for request in pending {
            let answer = self.run_assist_tool(&request.call);
            let _ = request.reply.send(answer);
        }
        if let Some(outcome) = self.assist.take_outcome() {
            self.status = match outcome {
                Ok(text) => format!("Assistant: {}", text.trim()),
                Err(e) => format!("Assistant failed: {e}"),
            };
        }
    }

    /// Answer any MCP tool calls waiting on the UI thread.
    fn drain_mcp(&mut self) {
        let Some(rx) = &self.mcp_rx else { return };
        // Collect first: handling needs `&mut self`.
        let mut pending = Vec::new();
        while let Ok(command) = rx.try_recv() {
            pending.push(command);
        }
        for command in pending {
            self.handle_mcp_command(command);
        }
    }

    fn handle_mcp_command(&mut self, command: crate::mcp::bridge::AppCommand) {
        use crate::mcp::bridge::AppCommand as C;
        use tangt_core::tools;

        // Each arm produces a ToolResult; applying is shared below.
        let (reply, result) = match command {
            C::ListTasks { reply } => (reply, tools::list_tasks(self.doc.model())),
            C::GetSource { reply } => (reply, tools::get_source(self.doc.text())),
            C::AddTask { args, reply } => (
                reply,
                tools::add_task(self.doc.model(), self.doc.text(), *args),
            ),
            C::SetDates { args, reply } => (reply, tools::set_dates(self.doc.model(), *args)),
            C::RenameTask { id, label, reply } => {
                (reply, tools::rename_task(self.doc.model(), &id, &label))
            }
            C::SetProgress { id, percent, reply } => {
                (reply, tools::set_progress(self.doc.model(), &id, percent))
            }
            C::AddDependency { task, after, reply } => {
                (reply, tools::add_dependency(self.doc.model(), &task, &after))
            }
            C::RemoveDependency { task, on, reply } => (
                reply,
                tools::remove_dependency(self.doc.model(), self.doc.text(), &task, &on),
            ),
            C::DeleteTask { id, reply } => (
                reply,
                tools::delete_task(self.doc.model(), self.doc.text(), &id),
            ),
            C::AddSection { label, reply } => {
                (reply, tools::add_section(self.doc.text(), &label))
            }
        };

        let answer = match result {
            Ok(tools::ToolOutcome::Read(value)) => Ok(value),
            Ok(tools::ToolOutcome::Write { edits, intent, summary }) => {
                self.apply_op(edits, intent);
                self.status = format!("MCP: {summary}");
                Ok(summary)
            }
            Err(message) => Err(message),
        };
        let _ = reply.send(answer);
    }

    /// New/Open go through here so a dirty document always gets a
    /// save/discard/cancel prompt instead of silently losing work.
    fn request(&mut self, action: PendingAction) {
        if self.doc.is_dirty() {
            self.pending_discard = Some(action);
        } else {
            self.perform(action);
        }
    }

    fn perform(&mut self, action: PendingAction) {
        match action {
            PendingAction::New => {
                self.doc = Document::new(String::new(), today_local());
                self.path = None;
                self.selection = None;
            }
            PendingAction::Open(path) => {
                if let Some(p) = path.or_else(files::pick_open) {
                    self.load(p);
                }
            }
        }
    }

    fn save_as(&mut self) {
        if let Some(p) = files::pick_save() {
            self.path = Some(p);
            self.save();
        }
    }

    fn apply_op(&mut self, edits: Vec<tangt_core::ops::TextEdit>, intent: Intent) {
        if edits.is_empty() {
            return;
        }
        let seq = self.doc.next_seq();
        self.doc.apply(Op { edits, intent, actor: 0, seq });
    }

    fn handle_canvas_event(&mut self, event: CanvasEvent) {
        let model = self.doc.model();
        match event {
            CanvasEvent::Select(id) => self.selection = id,
            CanvasEvent::MoveBar { id, new_start, to_date } => {
                if let Some(t) = model.tasks.get(&id).cloned() {
                    let edits = if to_date {
                        rewrite::move_task_to_date(&t, new_start)
                    } else {
                        rewrite::move_task(model, &t, new_start)
                    };
                    self.apply_op(edits, Intent::MoveBar(id.clone()));
                    self.selection = Some(id);
                }
            }
            CanvasEvent::ResizeEnd { id, day } => {
                if let Some(t) = model.tasks.get(&id).cloned() {
                    let edits = rewrite::resize_end(model, &t, day);
                    self.apply_op(edits, Intent::ResizeEnd(id.clone()));
                    self.selection = Some(id);
                }
            }
            CanvasEvent::ResizeStart { id, day } => {
                if let Some(t) = model.tasks.get(&id).cloned() {
                    let edits = rewrite::resize_start(model, &t, day);
                    self.apply_op(edits, Intent::ResizeStart(id.clone()));
                    self.selection = Some(id);
                }
            }
            CanvasEvent::CreateDep { from, to } => {
                if let Some(t) = model.tasks.get(&to).cloned() {
                    match rewrite::add_dependency(model, &t, &from) {
                        Some(edits) => {
                            self.apply_op(edits, Intent::AddDependency);
                            self.status = format!("{to} now starts after {from}");
                        }
                        None => {
                            self.status =
                                "Dependency not added (already exists or would create a cycle)"
                                    .into();
                        }
                    }
                }
            }
            CanvasEvent::RemoveDep { task, on } => {
                if let Some(t) = model.tasks.get(&task).cloned() {
                    let edits = rewrite::remove_dependency(self.doc.text(), &t, &on);
                    self.apply_op(edits, Intent::AddDependency);
                }
            }
            CanvasEvent::Rename { id, label } => {
                if let Some(t) = model.tasks.get(&id).cloned() {
                    if t.label != label && !label.trim().is_empty() {
                        let edits = rewrite::rename(&t, label.trim());
                        self.apply_op(edits, Intent::Rename(id));
                    }
                }
            }
            CanvasEvent::SetProgress { id, pct } => {
                if let Some(t) = model.tasks.get(&id).cloned() {
                    let edits = rewrite::set_progress(&t, pct);
                    self.apply_op(edits, Intent::SetProgress(id));
                }
            }
            CanvasEvent::AddTask { anchor, day, milestone } => {
                let anchor_task = anchor.and_then(|id| model.tasks.get(&id).cloned());
                let (edits, new_id) =
                    rewrite::add_task(model, self.doc.text(), anchor_task.as_ref(), day, milestone);
                self.apply_op(edits, Intent::AddTask);
                self.selection = Some(new_id.clone());
                // Rename immediately — a fresh task wants a real name.
                self.canvas_state.rename = Some((
                    new_id,
                    if milestone { "New milestone".into() } else { "New task".into() },
                ));
            }
            CanvasEvent::AddSection { anchor } => {
                let anchor_task = anchor.and_then(|id| model.tasks.get(&id).cloned());
                let edits =
                    rewrite::add_section(self.doc.text(), anchor_task.as_ref(), "New section");
                self.apply_op(edits, Intent::AddSection);
            }
            CanvasEvent::DeleteTask { id } => {
                if let Some(t) = model.tasks.get(&id).cloned() {
                    let edits = rewrite::delete_task(model, self.doc.text(), &t);
                    self.apply_op(edits, Intent::DeleteTask);
                    if self.selection.as_ref() == Some(&id) {
                        self.selection = None;
                    }
                }
            }
            CanvasEvent::DeleteSection { idx } => {
                let edits = rewrite::delete_section(model, self.doc.text(), idx);
                self.apply_op(edits, Intent::DeleteSection);
            }
            CanvasEvent::Reorder { id, before_row } => {
                if let Some(t) = model.tasks.get(&id).cloned() {
                    let edits = rewrite::reorder_task(model, self.doc.text(), &t, before_row);
                    self.apply_op(edits, Intent::Reorder(id));
                }
            }
            CanvasEvent::JumpToText(byte) => {
                self.show_editor = true;
                self.editor.pending_cursor = Some(byte);
            }
        }
    }

    fn nudge_selection(&mut self, ctx: &egui::Context) {
        // Arrow-key nudges only when the text editor isn't capturing keys.
        if ctx.egui_wants_keyboard_input() {
            return;
        }
        // Up/Down walk the selection through task rows (keyboard-only nav).
        let vertical = ctx.input_mut(|i| {
            i.consume_key(Modifiers::NONE, Key::ArrowDown) as i32
                - i.consume_key(Modifiers::NONE, Key::ArrowUp) as i32
        });
        if vertical != 0 {
            let model = self.doc.model();
            let task_rows: Vec<&TaskId> = model
                .rows
                .iter()
                .filter_map(|r| match r {
                    tangt_core::model::Row::Task(id) => Some(id),
                    _ => None,
                })
                .collect();
            if !task_rows.is_empty() {
                let current = self
                    .selection
                    .as_ref()
                    .and_then(|id| task_rows.iter().position(|t| *t == id));
                let next = match current {
                    Some(i) => (i as i32 + vertical).clamp(0, task_rows.len() as i32 - 1),
                    None if vertical > 0 => 0,
                    None => task_rows.len() as i32 - 1,
                };
                self.selection = Some(task_rows[next as usize].clone());
            }
            return;
        }
        let Some(id) = self.selection.clone() else { return };
        let Some(t) = self.doc.model().tasks.get(&id).cloned() else { return };
        let cal = self.doc.model().calendar.clone();

        let consume = |m: Modifiers, k: Key| {
            ctx.input_mut(|i| i.consume_key(m, k))
        };
        let step = |shift: bool| if shift { 5 } else { 1 };

        let (mut move_by, mut resize_by) = (0i64, 0i64);
        if consume(Modifiers::NONE, Key::ArrowRight) {
            move_by = step(false);
        } else if consume(Modifiers::NONE, Key::ArrowLeft) {
            move_by = -step(false);
        } else if consume(Modifiers::SHIFT, Key::ArrowRight) {
            move_by = step(true);
        } else if consume(Modifiers::SHIFT, Key::ArrowLeft) {
            move_by = -step(true);
        } else if consume(Modifiers::ALT, Key::ArrowRight) {
            resize_by = 1;
        } else if consume(Modifiers::ALT, Key::ArrowLeft) {
            resize_by = -1;
        } else if consume(Modifiers::NONE, Key::Delete)
            || consume(Modifiers::NONE, Key::Backspace)
        {
            self.handle_canvas_event(CanvasEvent::DeleteTask { id });
            return;
        } else if consume(Modifiers::NONE, Key::Enter) {
            self.canvas_state.rename = Some((id, t.label.clone()));
            return;
        }

        if move_by != 0 {
            let new_start = cal.add_working_days(t.start_day, move_by);
            let edits = rewrite::move_task(self.doc.model(), &t, new_start);
            self.apply_op(edits, Intent::MoveBar(id));
        } else if resize_by != 0 && !t.milestone {
            let new_end = cal.add_working_days(t.end_day, resize_by);
            let edits = rewrite::resize_end(self.doc.model(), &t, new_end);
            self.apply_op(edits, Intent::ResizeEnd(id));
        }
    }

    fn shortcuts(&mut self, ctx: &egui::Context) {
        let cmd = Modifiers::COMMAND;
        let consume = |m: Modifiers, k: Key| {
            ctx.input_mut(|i| i.consume_shortcut(&KeyboardShortcut::new(m, k)))
        };
        // Undo/redo are intercepted here so TextEdit's built-in undoer never
        // sees them — the document's unified undo covers both panes.
        // Ctrl+Y is the Windows redo convention.
        if consume(cmd.plus(Modifiers::SHIFT), Key::Z) || consume(cmd, Key::Y) {
            self.doc.redo();
        } else if consume(cmd, Key::Z) {
            self.doc.undo();
        }
        if consume(Modifiers::COMMAND.plus(Modifiers::SHIFT), Key::S) {
            self.save_as();
        } else if consume(cmd, Key::S) {
            self.save();
        }
        if consume(cmd, Key::N) {
            self.request(PendingAction::New);
        }
        if consume(cmd, Key::O) {
            self.request(PendingAction::Open(None));
        }
        if consume(cmd, Key::Num1) {
            self.show_editor = !self.show_editor;
        }
        if consume(cmd, Key::Num2) {
            self.assist.prefs.show_panel = !self.assist.prefs.show_panel;
        }
        if consume(cmd.plus(Modifiers::SHIFT), Key::L) {
            self.editor_on_left = !self.editor_on_left;
        }
        // F1 is the layout-independent fallback (⌘/ fails on e.g. German
        // keyboards where "/" needs Shift).
        if consume(cmd, Key::Slash) || consume(Modifiers::NONE, Key::F1) {
            self.help.open = !self.help.open;
        }
        if consume(cmd, Key::J) {
            if let Some(byte) = self.editor.cursor_byte {
                if let Some(id) = self.doc.task_at_byte(byte).cloned() {
                    // Text cursor -> canvas: select and reveal (both axes).
                    self.selection = Some(id.clone());
                    if let Some(t) = self.doc.model().tasks.get(&id) {
                        self.camera.left_day = t.start_day as f64 - 3.0;
                        self.camera.y_scroll = (t.row as f32
                            * tangt_core::geometry::ROW_H
                            - 160.0)
                            .max(0.0);
                    }
                }
            }
        }
        if ctx.input_mut(|i| i.consume_key(Modifiers::NONE, Key::Escape)) {
            self.selection = None;
            self.canvas_state.rename = None;
        }
        self.nudge_selection(ctx);
    }

    fn menu_bar(&mut self, root: &mut egui::Ui) {
        let ctx = root.ctx().clone();
        let ctx = &ctx;
        egui::Panel::top("menu").show(root, |ui| {
            egui::MenuBar::new().ui(ui, |ui| {
                ui.menu_button("File", |ui| {
                    let item = |label: &str, sc: &KeyboardShortcut| {
                        egui::Button::new(label).shortcut_text(ctx.format_shortcut(sc))
                    };
                    if ui.add(item("New", &SC_NEW)).clicked() {
                        self.request(PendingAction::New);
                        ui.close();
                    }
                    if ui.add(item("Open…", &SC_OPEN)).clicked() {
                        self.request(PendingAction::Open(None));
                        ui.close();
                    }
                    ui.menu_button("Open Recent", |ui| {
                        let recents = self.recents.clone();
                        for p in recents {
                            let name =
                                p.file_name().map(|f| f.to_string_lossy().into_owned());
                            if ui.button(name.unwrap_or_default()).clicked() {
                                self.request(PendingAction::Open(Some(p)));
                                ui.close();
                            }
                        }
                    });
                    ui.separator();
                    if ui.add(item("Save", &SC_SAVE)).clicked() {
                        self.save();
                        ui.close();
                    }
                    if ui.add(item("Save As…", &SC_SAVE_AS)).clicked() {
                        self.save_as();
                        ui.close();
                    }
                    ui.separator();
                    if ui.button("Export PDF…").clicked() {
                        if let Some(p) = crate::export::pick_export("pdf") {
                            let m = EguiMeasurer(ctx);
                            match crate::export::export_pdf(self.doc.model(), &m, &p) {
                                Ok(()) => self.status = "Exported PDF".into(),
                                Err(e) => self.status = format!("Export failed: {e}"),
                            }
                        }
                        ui.close();
                    }
                    if ui.button("Export SVG…").clicked() {
                        if let Some(p) = crate::export::pick_export("svg") {
                            let m = EguiMeasurer(ctx);
                            match crate::export::export_svg(self.doc.model(), &m, &p) {
                                Ok(()) => self.status = "Exported SVG".into(),
                                Err(e) => self.status = format!("Export failed: {e}"),
                            }
                        }
                        ui.close();
                    }
                    if ui.button("Export PNG (2x)…").clicked() {
                        if let Some(p) = crate::export::pick_export("png") {
                            let m = EguiMeasurer(ctx);
                            match crate::export::export_png(self.doc.model(), &m, &p, 2.0) {
                                Ok(()) => self.status = "Exported PNG".into(),
                                Err(e) => self.status = format!("Export failed: {e}"),
                            }
                        }
                        ui.close();
                    }
                });
                ui.menu_button("View", |ui| {
                    ui.checkbox(
                        &mut self.show_editor,
                        format!("Editor pane   {}", ctx.format_shortcut(&SC_EDITOR)),
                    );
                    ui.checkbox(
                        &mut self.assist.prefs.show_panel,
                        format!("Assistant   {}", ctx.format_shortcut(&SC_ASSIST)),
                    );
                    if ui.button("Preferences…").clicked() {
                        self.show_prefs = true;
                        ui.close();
                    }
                    let swap_label = format!(
                        "{}   {}",
                        if self.editor_on_left { "Move editor right" } else { "Move editor left" },
                        ctx.format_shortcut(&SC_SWAP)
                    );
                    if ui.button(swap_label).clicked() {
                        self.editor_on_left = !self.editor_on_left;
                        ui.close();
                    }
                    ui.separator();
                    if ui.button("Fit chart   F").clicked() {
                        self.fit_requested = true;
                        ui.close();
                    }
                    if ui.button("Zoom 100%").clicked() {
                        self.camera.zoom = 1.0;
                        ui.close();
                    }
                });
                ui.menu_button("Help", |ui| {
                    let label =
                        format!("Reference   {} / F1", ctx.format_shortcut(&SC_HELP));
                    if ui.button(label).clicked() {
                        self.help.open = true;
                        ui.close();
                    }
                });
            });
        });
    }

    /// Compact description of the chart, so the model starts oriented instead
    /// of spending its first turn on list_tasks.
    fn chart_summary(&self) -> String {
        let m = self.doc.model();
        let mut out = String::new();
        if let Some(title) = &m.title {
            out.push_str(&format!("title: {title}\n"));
        }
        out.push_str(&format!(
            "today: {}\nworking days exclude weekends: {}\n{} task(s):\n",
            tangt_core::resolve::format_day(m.today),
            m.calendar.exclude_weekends,
            m.tasks.len()
        ));
        for t in m.tasks.values().take(60) {
            out.push_str(&format!(
                "  {} \"{}\" {} → {}{}{}\n",
                t.id,
                t.label,
                tangt_core::resolve::format_day(t.start_day),
                tangt_core::resolve::format_day(t.end_day),
                if t.deps.is_empty() {
                    String::new()
                } else {
                    format!(
                        " after {}",
                        t.deps.iter().map(|d| d.on.as_str()).collect::<Vec<_>>().join(" ")
                    )
                },
                if t.milestone { " (milestone)" } else { "" },
            ));
        }
        if m.tasks.len() > 60 {
            out.push_str(&format!("  … and {} more\n", m.tasks.len() - 60));
        }
        out
    }

    fn assist_panel(&mut self, root: &mut egui::Ui) {
        egui::Panel::bottom("assist")
            .resizable(true)
            .default_size(180.0)
            .show(root, |ui| {
                let running = self.assist.with_status(|s| s.running);
                ui.horizontal(|ui| {
                    ui.strong("Assistant");
                    ui.weak(format!("· {}", self.assist.prefs.config.build().describe()));
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if ui.button("Settings…").clicked() {
                            self.show_prefs = true;
                        }
                        if running && ui.button("Stop").clicked() {
                            self.assist.cancel();
                        }
                    });
                });

                let (streaming, activity) = self
                    .assist
                    .with_status(|s| (s.streaming.clone(), s.activity.join(" · ")));
                egui::ScrollArea::vertical()
                    .max_height(90.0)
                    .stick_to_bottom(true)
                    .show(ui, |ui| {
                        if !activity.is_empty() {
                            ui.weak(format!("tools: {activity}"));
                        }
                        if !streaming.is_empty() {
                            ui.label(&streaming);
                        } else if !running {
                            ui.weak(
                                "e.g. \"add a two-week QA phase after the backend work\" or \
                                 \"push everything after the design sign-off out by a week\"",
                            );
                        }
                    });

                ui.horizontal(|ui| {
                    let send = ui
                        .add_enabled(
                            !running,
                            egui::TextEdit::singleline(&mut self.assist.draft)
                                .hint_text("Describe a change to the plan…")
                                .desired_width(ui.available_width() - 70.0),
                        )
                        .lost_focus()
                        && ui.input(|i| i.key_pressed(egui::Key::Enter));
                    let clicked = ui.add_enabled(!running, egui::Button::new("Send")).clicked();
                    if (send || clicked) && !self.assist.draft.trim().is_empty() {
                        let job = crate::llm::AssistJob {
                            prompt: std::mem::take(&mut self.assist.draft),
                            chart_summary: self.chart_summary(),
                            config: self.assist.prefs.config.clone(),
                            tools: self.assist_tools(),
                        };
                        self.assist.ask(job);
                    }
                });
            });
    }

    fn preferences(&mut self, ctx: &egui::Context) {
        let mut open = self.show_prefs;
        egui::Window::new("Preferences")
            .open(&mut open)
            .default_width(520.0)
            .show(ctx, |ui| {
                ui.heading("MCP server");
                ui.label(
                    "Let an assistant read and edit this chart while it is open. Its edits go \
                     through the same path as canvas edits, so ⌘Z undoes them.",
                );
                ui.add_space(6.0);
                ui.checkbox(&mut self.mcp_prefs.server_enabled, "Serve on 127.0.0.1 (loopback only)");
                ui.horizontal(|ui| {
                    ui.label("Port");
                    ui.add(egui::DragValue::new(&mut self.mcp_prefs.port).range(1024..=65535));
                    if ui.button("Regenerate token").clicked() {
                        self.mcp_prefs.token = crate::mcp::new_token();
                    }
                });
                match self.mcp_server.as_ref().map(|s| s.status()) {
                    Some(status) if status.listening.is_some() => {
                        ui.colored_label(
                            Color32::from_rgb(0x2e, 0x7d, 0x32),
                            format!("listening on 127.0.0.1:{}", status.listening.unwrap()),
                        );
                    }
                    Some(status) if status.error.is_some() => {
                        ui.colored_label(
                            Color32::from_rgb(0xd3, 0x2f, 0x2f),
                            status.error.unwrap_or_default(),
                        );
                    }
                    _ => {
                        ui.weak("not running");
                    }
                }
                ui.add_space(6.0);
                ui.label("Client configuration:");
                let mut config = self.mcp_prefs.client_config();
                ui.add(
                    egui::TextEdit::multiline(&mut config)
                        .font(FontId::monospace(11.0))
                        .desired_width(f32::INFINITY),
                );
                if ui.button("Copy config").clicked() {
                    ctx.copy_text(self.mcp_prefs.client_config());
                    self.status = "Copied MCP client config".into();
                }
                ui.add_space(4.0);
                ui.weak(
                    "Headless alternative, no window needed:  tangt mcp-serve path/to/chart.tangt",
                );

                ui.separator();
                ui.heading("Assistant");
                self.assist.settings_ui(ui);
            });
        self.show_prefs = open;
    }

    fn status_bar(&mut self, root: &mut egui::Ui) {
        egui::Panel::bottom("status").show(root, |ui| {
            ui.horizontal(|ui| {
                let name = self
                    .path
                    .as_ref()
                    .and_then(|p| p.file_name())
                    .map(|f| f.to_string_lossy().into_owned())
                    .unwrap_or_else(|| "untitled.tangt".into());
                ui.label(format!("{name}{}", if self.doc.is_dirty() { " •" } else { "" }));
                ui.separator();
                let diags = self.doc.model().diagnostics.clone();
                let first_error = diags
                    .iter()
                    .find(|d| d.severity == tangt_core::ast::Severity::Error);
                let errors =
                    diags.iter().filter(|d| d.severity == tangt_core::ast::Severity::Error).count();
                let mut jump: Option<usize> = None;
                if let Some(err) = first_error {
                    // The problem label jumps the editor to the offending span.
                    let label = egui::RichText::new(format!("{errors} problem(s)"))
                        .color(Color32::from_rgb(0xd3, 0x2f, 0x2f));
                    if ui.link(label).on_hover_text("Jump to the first error").clicked() {
                        jump = Some(err.span.start);
                    }
                    ui.weak(&err.message);
                } else if let Some(warn) = diags.first() {
                    ui.weak(&warn.message);
                } else {
                    ui.weak("no problems");
                }
                if let Some(byte) = jump {
                    self.show_editor = true;
                    self.editor.pending_cursor = Some(byte);
                }
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    ui.label(format!("{:.0}%", self.camera.zoom * 100.0));
                    ui.separator();
                    // Live drag feedback wins over the last action message.
                    match &self.canvas_state.hint {
                        Some(hint) => ui.strong(hint),
                        None => ui.weak(&self.status),
                    };
                });
            });
        });
    }
}

impl eframe::App for TangtApp {
    fn save(&mut self, storage: &mut dyn eframe::Storage) {
        eframe::set_value(
            storage,
            "tangt",
            &Persisted {
                recents: self.recents.clone(),
                last_file: self.path.clone(),
                show_editor: self.show_editor,
                editor_on_left: self.editor_on_left,
                camera: Some(self.camera),
                mcp: self.mcp_prefs.clone(),
                assist: self.assist.prefs.clone(),
            },
        );
    }

    fn ui(&mut self, root: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let ctx = root.ctx().clone();
        let ctx = &ctx;
        // Confirm-close when dirty.
        if ctx.input(|i| i.viewport().close_requested())
            && self.doc.is_dirty()
            && !self.confirm_close
        {
            ctx.send_viewport_cmd(egui::ViewportCommand::CancelClose);
            self.confirm_close = true;
        }
        if self.confirm_close {
            egui::Window::new("Unsaved changes")
                .collapsible(false)
                .resizable(false)
                .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
                .show(ctx, |ui| {
                    ui.label("The chart has unsaved changes.");
                    ui.horizontal(|ui| {
                        if ui.button("Save and quit").clicked() {
                            self.save();
                            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
                        }
                        if ui.button("Quit without saving").clicked() {
                            self.doc.mark_saved();
                            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
                        }
                        if ui.button("Cancel").clicked() {
                            self.confirm_close = false;
                        }
                    });
                });
        }

        // Unsaved-changes prompt for New/Open (window close has its own).
        if let Some(action) = self.pending_discard.clone() {
            egui::Window::new("Unsaved changes ")
                .collapsible(false)
                .resizable(false)
                .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
                .show(ctx, |ui| {
                    ui.label("The chart has unsaved changes.");
                    ui.horizontal(|ui| {
                        if ui.button("Save and continue").clicked() {
                            self.save();
                            if !self.doc.is_dirty() {
                                self.perform(action.clone());
                            }
                            self.pending_discard = None;
                        }
                        if ui.button("Discard changes").clicked() {
                            self.doc.mark_saved();
                            self.perform(action.clone());
                            self.pending_discard = None;
                        }
                        if ui.button("Cancel").clicked() {
                            self.pending_discard = None;
                        }
                    });
                });
        }

        // Keep the OS window title in sync: "name • — tangt".
        let name = self
            .path
            .as_ref()
            .and_then(|p| p.file_name())
            .map(|f| f.to_string_lossy().into_owned())
            .unwrap_or_else(|| "untitled".into());
        let title =
            format!("{name}{} — tangt", if self.doc.is_dirty() { " •" } else { "" });
        if title != self.last_title {
            ctx.send_viewport_cmd(egui::ViewportCommand::Title(title.clone()));
            self.last_title = title;
        }

        self.reconcile_mcp(ctx);
        self.drain_mcp();
        self.drain_assist();
        self.preferences(ctx);
        self.shortcuts(ctx);
        self.menu_bar(root);
        self.status_bar(root);
        if self.assist.prefs.show_panel {
            self.assist_panel(root);
        }

        let theme = Theme::new(ctx.theme() == egui::Theme::Dark);

        // Layout is cheap: recompute every frame at the camera's zoom so
        // ticks and bar rects always match.
        {
            let measurer = EguiMeasurer(ctx);
            self.layout =
                layout::compute(self.doc.model(), &measurer, self.camera.px_per_day());
        }

        let show_editor_pane = |app: &mut TangtApp, ui: &mut egui::Ui, theme: &Theme| {
            app.editor.sync(app.doc.text(), app.doc.version());
            let diags = app.doc.model().diagnostics.clone();
            egui::ScrollArea::vertical().show(ui, |ui| {
                if let Some(edit) = app.editor.show(ui, &diags, theme) {
                    app.apply_op(vec![edit], Intent::Typing);
                    app.editor.synced_version = app.doc.version();
                    app.editor.scratch = app.doc.text().to_string();
                }
            });
        };

        if self.show_editor {
            if self.editor_on_left {
                egui::Panel::left("editor-pane")
                    .resizable(true)
                    .default_size(420.0)
                    .min_size(240.0)
                    .show(root, |ui| show_editor_pane(self, ui, &theme));
            } else {
                egui::Panel::right("editor-pane")
                    .resizable(true)
                    .default_size(420.0)
                    .min_size(240.0)
                    .show(root, |ui| show_editor_pane(self, ui, &theme));
            }
        }

        egui::CentralPanel::default().show(root, |ui| {
            let viewport = ui.max_rect();
            if !ctx.egui_wants_keyboard_input()
                && ui.input_mut(|i| i.consume_key(Modifiers::NONE, Key::F))
            {
                self.fit_requested = true;
            }
            if self.fit_requested {
                let chart = egui::Rect::from_min_max(
                    viewport.min + egui::Vec2::new(self.layout.gutter_w, 0.0),
                    viewport.max,
                );
                let model = self.doc.model();
                self.camera.fit(model.min_day, model.max_day, chart);
                self.fit_requested = false;
                // Zoom changed: rebuild layout before drawing.
                let measurer = EguiMeasurer(ctx);
                self.layout =
                    layout::compute(self.doc.model(), &measurer, self.camera.px_per_day());
            }

            let events = canvas::show(
                ui,
                self.doc.model(),
                &self.layout,
                &mut self.camera,
                &mut self.canvas_state,
                self.selection.as_ref(),
                &theme,
            );
            for event in events {
                self.handle_canvas_event(event);
            }
        });

        self.help.show(ctx);
    }
}
