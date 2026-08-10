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
}

impl Default for Persisted {
    fn default() -> Self {
        Persisted {
            recents: Vec::new(),
            last_file: None,
            show_editor: true,
            editor_on_left: true,
            camera: None,
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
    fit_requested: bool,
    status: String,
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
            camera: persisted.camera.unwrap_or_default(),
            editor: EditorPane::new(),
            help: crate::help::HelpWindow::new(),
            canvas_state: CanvasState::default(),
            selection: None,
            recents: persisted.recents,
            show_editor: persisted.show_editor,
            editor_on_left: persisted.editor_on_left,
            confirm_close: false,
            fit_requested,
            status: String::new(),
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
        if consume(cmd.plus(Modifiers::SHIFT), Key::Z) {
            self.doc.redo();
        } else if consume(cmd, Key::Z) {
            self.doc.undo();
        }
        if consume(cmd, Key::S) {
            self.save();
        }
        if consume(cmd, Key::O) {
            if let Some(p) = files::pick_open() {
                self.load(p);
            }
        }
        if consume(cmd, Key::Num1) {
            self.show_editor = !self.show_editor;
        }
        if consume(cmd.plus(Modifiers::SHIFT), Key::L) {
            self.editor_on_left = !self.editor_on_left;
        }
        if consume(cmd, Key::Slash) {
            self.help.open = !self.help.open;
        }
        if consume(cmd, Key::J) {
            if let Some(byte) = self.editor.cursor_byte {
                if let Some(id) = self.doc.task_at_byte(byte).cloned() {
                    // Text cursor -> canvas: select and reveal.
                    self.selection = Some(id.clone());
                    if let Some(t) = self.doc.model().tasks.get(&id) {
                        self.camera.left_day = t.start_day as f64 - 3.0;
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
                    if ui.button("New").clicked() {
                        self.doc = Document::new(String::new(), today_local());
                        self.path = None;
                        self.selection = None;
                        ui.close();
                    }
                    if ui.button("Open…").clicked() {
                        if let Some(p) = files::pick_open() {
                            self.load(p);
                        }
                        ui.close();
                    }
                    ui.menu_button("Open Recent", |ui| {
                        let recents = self.recents.clone();
                        for p in recents {
                            let name =
                                p.file_name().map(|f| f.to_string_lossy().into_owned());
                            if ui.button(name.unwrap_or_default()).clicked() {
                                self.load(p);
                                ui.close();
                            }
                        }
                    });
                    ui.separator();
                    if ui.button("Save").clicked() {
                        self.save();
                        ui.close();
                    }
                    if ui.button("Save As…").clicked() {
                        if let Some(p) = files::pick_save() {
                            self.path = Some(p);
                            self.save();
                        }
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
                    ui.checkbox(&mut self.show_editor, "Editor pane   ⌘1");
                    if ui
                        .button(if self.editor_on_left {
                            "Move editor right   ⌘⇧L"
                        } else {
                            "Move editor left   ⌘⇧L"
                        })
                        .clicked()
                    {
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
                    if ui.button("Reference   ⌘/").clicked() {
                        self.help.open = true;
                        ui.close();
                    }
                });
            });
        });
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
                let diags = &self.doc.model().diagnostics;
                let errors = diags
                    .iter()
                    .filter(|d| d.severity == tangt_core::ast::Severity::Error)
                    .count();
                if errors > 0 {
                    ui.colored_label(
                        Color32::from_rgb(0xd3, 0x2f, 0x2f),
                        format!("{errors} problem(s)"),
                    );
                    if let Some(first) = diags.first() {
                        ui.weak(&first.message);
                    }
                } else if let Some(warn) = diags.first() {
                    ui.weak(&warn.message);
                } else {
                    ui.weak("no problems");
                }
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    ui.label(format!("{:.0}%", self.camera.zoom * 100.0));
                    ui.separator();
                    ui.weak(&self.status);
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

        self.shortcuts(ctx);
        self.menu_bar(root);
        self.status_bar(root);

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
            if ui.input_mut(|i| i.consume_key(Modifiers::NONE, Key::F))
                && !ctx.egui_wants_keyboard_input()
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
