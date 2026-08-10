//! In-app syntax reference (⌘/ / Ctrl+/ / F1).

use eframe::egui;

pub struct HelpWindow {
    pub open: bool,
}

impl HelpWindow {
    pub fn new() -> Self {
        HelpWindow { open: false }
    }

    pub fn show(&mut self, ctx: &egui::Context) {
        let mac = ctx.os() == egui::os::OperatingSystem::Mac;
        let (cmd, shift, alt) = if mac { ("⌘", "⇧", "⌥") } else { ("Ctrl+", "Shift+", "Alt+") };
        let mut open = self.open;
        egui::Window::new("tangt reference")
            .open(&mut open)
            .default_width(560.0)
            .show(ctx, |ui| {
                egui::ScrollArea::vertical().show(ui, |ui| {
                    ui.heading("Syntax");
                    ui.monospace(SYNTAX);
                    ui.separator();
                    ui.heading("Task fields (any order, comma-separated)");
                    ui.monospace(FIELDS);
                    ui.separator();
                    ui.heading("Mouse");
                    ui.label("Drag bar: move (keeps `after` dependencies, adjusting lag)");
                    ui.label(format!("{alt}drag bar: detach dependencies and set a fixed date"));
                    ui.label("Drag bar edges: resize duration");
                    ui.label("Drag the dot at a bar's right edge onto another bar: create a dependency");
                    ui.label("Double-click a bar: rename · double-click gutter: jump to source");
                    ui.label("Drag a gutter row up/down: reorder tasks");
                    ui.label("Right-click bar/gutter/empty space: add, delete, progress, dependencies");
                    ui.label("Drag empty chart space: pan · scroll: rows · horizontal scroll: pan time");
                    ui.label(format!("Pinch / {cmd}scroll: zoom time around the pointer"));
                    ui.separator();
                    ui.heading("Keyboard");
                    ui.label("↑/↓: select previous/next task");
                    ui.label(format!("←/→: move selected task ±1 day ({shift}: ±1 week)"));
                    ui.label(format!("{alt}←/→: resize selected task"));
                    ui.label("Enter: rename selected · Delete: delete selected · Esc: deselect");
                    ui.label(format!("F: fit chart · {cmd}J: reveal task under the text cursor"));
                    ui.label(format!("{cmd}Z / {cmd}{shift}Z: undo/redo (canvas and text share one history)"));
                    ui.label(format!("{cmd}1: editor pane · {cmd}2: assistant · {cmd}{shift}L: swap editor side"));
                    ui.label(format!("{cmd}+ / {cmd}−: UI zoom (larger/smaller interface)"));
                    ui.label(format!("{cmd}/ or F1: this window"));
                    ui.separator();
                    ui.heading("Assistant and MCP");
                    ui.label(format!(
                        "{cmd}2 opens a prompt box — describe a change (\"add a two-week QA phase \
                         after the backend work\") and the assistant edits the chart. Its changes \
                         are ordinary edits: {cmd}Z undoes each one."
                    ));
                    ui.label(
                        "Configure the model in View ▸ Preferences (Ollama, or any \
                         OpenAI-compatible endpoint).",
                    );
                    ui.label(
                        "The same tools are available to MCP clients: switch the server on in \
                         Preferences, or run  tangt mcp-serve chart.tangt  for a headless one.",
                    );
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
