//! DSL text editor pane: egui TextEdit with a lexer-driven highlighting
//! layouter, diagnostic underlines, single-diff capture per frame (text ->
//! Op), and two-way cursor/task jumping. Adapted from gridflow's editor.

use crate::theme::Theme;
use eframe::egui::text::{CCursor, CCursorRange, LayoutJob, TextFormat};
use eframe::egui::{self, FontId, Stroke, TextEdit, Ui};
use tangt_core::ast::{Diagnostic, Severity, Span};
use tangt_core::lexer::{is_keyword, lex, Token};
use tangt_core::ops::TextEdit as CoreEdit;

pub struct EditorPane {
    /// Scratch copy the TextEdit mutates; re-synced whenever the document
    /// changes underneath us (drag writeback, undo, file load).
    pub scratch: String,
    pub synced_version: u64,
    /// Byte offset to move the cursor to next frame (task -> text jump).
    pub pending_cursor: Option<usize>,
    /// Last known cursor byte offset (for text -> task highlight/jump).
    pub cursor_byte: Option<usize>,
}

impl EditorPane {
    pub fn new() -> Self {
        EditorPane {
            scratch: String::new(),
            synced_version: u64::MAX,
            pending_cursor: None,
            cursor_byte: None,
        }
    }

    pub fn sync(&mut self, text: &str, version: u64) {
        if self.synced_version != version {
            self.scratch = text.to_string();
            self.synced_version = version;
        }
    }

    /// Returns the single contiguous text edit made this frame, if any.
    pub fn show(
        &mut self,
        ui: &mut Ui,
        diagnostics: &[Diagnostic],
        theme: &Theme,
    ) -> Option<CoreEdit> {
        let before = self.scratch.clone();

        let diags: Vec<(Span, Severity)> =
            diagnostics.iter().map(|d| (d.span, d.severity)).collect();
        let colors = TokenColors::from(theme);
        let mut layouter = move |ui: &Ui, buf: &dyn egui::TextBuffer, wrap_width: f32| {
            let mut job = highlight(buf.as_str(), &diags, &colors);
            job.wrap.max_width = wrap_width;
            ui.painter().layout_job(job)
        };

        let id = egui::Id::new("tangt-editor");
        // Apply a pending task->text jump before showing, so the state change
        // is picked up this frame.
        if let Some(byte) = self.pending_cursor.take() {
            let char_idx = byte_to_char(&self.scratch, byte);
            let mut state = TextEdit::load_state(ui.ctx(), id).unwrap_or_default();
            state
                .cursor
                .set_char_range(Some(CCursorRange::one(CCursor::new(char_idx))));
            TextEdit::store_state(ui.ctx(), id, state);
            ui.memory_mut(|m| m.request_focus(id));
        }

        let output = TextEdit::multiline(&mut self.scratch)
            .id(id)
            .font(FontId::monospace(13.0))
            .code_editor()
            .desired_width(f32::INFINITY)
            .desired_rows(40)
            .layouter(&mut layouter)
            .show(ui);

        if let Some(range) = output.cursor_range {
            self.cursor_byte = Some(char_to_byte(&self.scratch, range.primary.index.0));
        }

        (self.scratch != before).then(|| diff_single(&before, &self.scratch))
    }
}

struct TokenColors {
    keyword: egui::Color32,
    string: egui::Color32,
    number: egui::Color32,
    date: egui::Color32,
    color: egui::Color32,
    comment: egui::Color32,
    ident: egui::Color32,
    punct: egui::Color32,
    diagnostic: egui::Color32,
    warning: egui::Color32,
}

impl From<&Theme> for TokenColors {
    fn from(t: &Theme) -> Self {
        TokenColors {
            keyword: t.tok_keyword,
            string: t.tok_string,
            number: t.tok_number,
            date: t.tok_date,
            color: t.tok_color,
            comment: t.tok_comment,
            ident: t.tok_ident,
            punct: t.tok_punct,
            diagnostic: t.diagnostic,
            warning: t.tok_number,
        }
    }
}

fn highlight(src: &str, diags: &[(Span, Severity)], colors: &TokenColors) -> LayoutJob {
    let font = FontId::monospace(13.0);
    let mut job = LayoutJob::default();
    let mut push = |text: &str, color: egui::Color32, range: (usize, usize)| {
        if text.is_empty() {
            return;
        }
        let underline = diags
            .iter()
            .find(|(s, _)| s.start < range.1 && range.0 < s.end.max(s.start + 1))
            .map(|(_, sev)| match sev {
                Severity::Error => Stroke::new(1.5, colors.diagnostic),
                Severity::Warning => Stroke::new(1.0, colors.warning),
            })
            .unwrap_or(Stroke::NONE);
        job.append(
            text,
            0.0,
            TextFormat { font_id: font.clone(), color, underline, ..Default::default() },
        );
    };

    let mut last = 0usize;
    for lexeme in lex(src) {
        if lexeme.start > last {
            push(&src[last..lexeme.start], colors.ident, (last, lexeme.start));
        }
        let text = &src[lexeme.start..lexeme.end];
        let color = match lexeme.token {
            Token::Comment => colors.comment,
            Token::Str => colors.string,
            Token::Date => colors.date,
            Token::Duration | Token::Percent | Token::Int => colors.number,
            Token::Color => colors.color,
            Token::Ident if is_keyword(text) => colors.keyword,
            Token::Ident => colors.ident,
            _ => colors.punct,
        };
        push(text, color, (lexeme.start, lexeme.end));
        last = lexeme.end;
    }
    if last < src.len() {
        push(&src[last..], colors.ident, (last, src.len()));
    }
    job
}

/// One frame of edits in a TextEdit is always a single contiguous change:
/// common-prefix/common-suffix scan yields the minimal splice.
fn diff_single(before: &str, after: &str) -> CoreEdit {
    let b = before.as_bytes();
    let a = after.as_bytes();
    let mut start = 0;
    while start < b.len() && start < a.len() && b[start] == a[start] {
        start += 1;
    }
    let mut end_b = b.len();
    let mut end_a = a.len();
    while end_b > start && end_a > start && b[end_b - 1] == a[end_a - 1] {
        end_b -= 1;
        end_a -= 1;
    }
    // Snap to char boundaries (multi-byte chars can split mid-sequence).
    while start > 0 && (!before.is_char_boundary(start) || !after.is_char_boundary(start)) {
        start -= 1;
    }
    while end_b < before.len() && !before.is_char_boundary(end_b) {
        end_b += 1;
    }
    while end_a < after.len() && !after.is_char_boundary(end_a) {
        end_a += 1;
    }
    CoreEdit::new(start, end_b, &after[start..end_a])
}

pub fn char_to_byte(s: &str, char_idx: usize) -> usize {
    s.char_indices()
        .nth(char_idx)
        .map(|(b, _)| b)
        .unwrap_or(s.len())
}

pub fn byte_to_char(s: &str, byte: usize) -> usize {
    s[..byte.min(s.len())].chars().count()
}
