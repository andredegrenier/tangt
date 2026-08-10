//! In-app syntax reference (⌘/).

use eframe::egui;

pub struct HelpWindow {
    pub open: bool,
}

impl HelpWindow {
    pub fn new() -> Self {
        HelpWindow { open: false }
    }

    pub fn show(&mut self, ctx: &egui::Context) {
        let mut open = self.open;
        egui::Window::new("tangt reference")
            .open(&mut open)
            .default_width(520.0)
            .show(ctx, |ui| {
                egui::ScrollArea::vertical().show(ui, |ui| {
                    ui.heading("Syntax");
                    ui.monospace(SYNTAX);
                    ui.separator();
                    ui.heading("Task fields (any order, comma-separated)");
                    ui.monospace(FIELDS);
                    ui.separator();
                    ui.heading("Mouse");
                    ui.label("Drag bar: move · drag edges: resize · drag the dot at a bar's right edge onto another bar: dependency");
                    ui.label("Double-click bar: rename · double-click gutter: jump to source · drag gutter row: reorder");
                    ui.label("Right-click: add/delete tasks and sections, set progress");
                    ui.label("Scroll: rows · Shift+scroll: pan time · pinch / ⌘scroll: zoom time");
                    ui.separator();
                    ui.heading("Keyboard");
                    ui.label("←/→: move selected task ±1 day (⇧: ±1 week) · ⌥←/→: resize");
                    ui.label("F: fit chart · ⌘J: jump canvas↔text · ⌘Z/⌘⇧Z: undo/redo");
                    ui.label("⌘1: editor pane · ⌘⇧L: swap editor side · ⌘/: this window");
                });
            });
        self.open = open;
    }
}

const SYNTAX: &str = r#"title "Website Redesign"
axis auto                   // auto | day | week | month
today 2026-08-10            // today-marker override
exclude weekends            // durations count working days
exclude 2026-12-24..2027-01-02

section Design
task "Wireframes"    : wf, 2026-08-03, 5d
task "Visual design" : vis, after wf, 10d, #4fa3ff, 60%
milestone "Sign-off" : ds, after vis

section Build
task "Frontend" : fe, after ds + 2d, until 2026-10-01
task "QA"       : qa, after fe be, 5d, crit"#;

const FIELDS: &str = r#"2026-08-10          start date
after id [id…]      start after those tasks end
after id + 2d       …with lag (or - 2d for lead)
10d / 2w            duration (working days if excluding)
until 2026-09-01    explicit end date (inclusive)
60%                 progress
#4fa3ff             bar color
crit | done | active  tags"#;
