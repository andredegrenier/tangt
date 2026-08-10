//! The document: source text plus its resolved model, with a single mutation
//! path (`apply`) and unified undo/redo spanning text edits and canvas edits.

use crate::model::GanttModel;
use crate::ops::{Intent, Op, TextEdit};
use crate::parser::parse;
use crate::resolve::resolve;

pub struct Document {
    text: String,
    model: GanttModel,
    /// Fallback day number for the today-marker (and first-task default) when
    /// the file has no `today` directive.
    today_fallback: i64,
    undo: Vec<UndoEntry>,
    redo: Vec<UndoEntry>,
    version: u64,
    saved_version: u64,
    next_seq: u64,
}

/// Undo entries hold *sequences* of edit batches so coalesced ops (drag,
/// drag, drag) compose correctly. The document text alternates between
/// exactly two states across undo/redo, so both sequences stay valid forever:
/// `forward` applies against the pre-op text, `inverse` against the post-op.
struct UndoEntry {
    /// Batches that revert the op, applied in order against the post-op text.
    inverse: Vec<Vec<TextEdit>>,
    /// Batches that re-apply the op, applied in order against the pre-op text.
    forward: Vec<Vec<TextEdit>>,
    intent: Intent,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ApplyResult {
    Applied,
    /// One or more edit ranges were out of bounds; nothing was changed.
    Rejected,
}

impl Document {
    pub fn new(text: String, today_fallback: i64) -> Self {
        let model = resolve(&parse(&text), today_fallback);
        Document {
            text,
            model,
            today_fallback,
            undo: Vec::new(),
            redo: Vec::new(),
            version: 0,
            saved_version: 0,
            next_seq: 0,
        }
    }

    pub fn text(&self) -> &str {
        &self.text
    }

    pub fn model(&self) -> &GanttModel {
        &self.model
    }

    pub fn version(&self) -> u64 {
        self.version
    }

    pub fn is_dirty(&self) -> bool {
        self.version != self.saved_version
    }

    pub fn mark_saved(&mut self) {
        self.saved_version = self.version;
    }

    pub fn next_seq(&mut self) -> u64 {
        self.next_seq += 1;
        self.next_seq
    }

    pub fn task_at_byte(&self, offset: usize) -> Option<&crate::model::TaskId> {
        self.model.task_at_byte(offset)
    }

    /// THE mutation path. Applies edits back-to-front, records the inverse for
    /// undo, reparses, bumps the version.
    pub fn apply(&mut self, op: Op) -> ApplyResult {
        let Some((forward, inverse)) = self.apply_batch(&op.edits) else {
            return ApplyResult::Rejected;
        };
        self.commit();

        // Coalesce consecutive drags of the same task into one undo step:
        // sequences compose, so undo restores the state before the first drag.
        let coalesce = match (op.intent.coalesce_key(), self.undo.last()) {
            (Some(key), Some(prev)) => prev.intent.coalesce_key() == Some(key),
            _ => false,
        };
        if coalesce {
            let prev = self.undo.last_mut().unwrap();
            prev.forward.push(forward);
            prev.inverse.insert(0, inverse);
        } else {
            self.undo.push(UndoEntry {
                inverse: vec![inverse],
                forward: vec![forward],
                intent: op.intent,
            });
        }
        self.redo.clear();
        ApplyResult::Applied
    }

    pub fn can_undo(&self) -> bool {
        !self.undo.is_empty()
    }

    pub fn can_redo(&self) -> bool {
        !self.redo.is_empty()
    }

    pub fn undo(&mut self) {
        if let Some(entry) = self.undo.pop() {
            for batch in &entry.inverse {
                self.apply_batch(batch);
            }
            self.commit();
            self.redo.push(entry);
        }
    }

    pub fn redo(&mut self) {
        if let Some(entry) = self.redo.pop() {
            for batch in &entry.forward {
                self.apply_batch(batch);
            }
            self.commit();
            self.undo.push(entry);
        }
    }

    fn commit(&mut self) {
        self.model = resolve(&parse(&self.text), self.today_fallback);
        self.version += 1;
    }

    /// Validates and splices one batch of edits atomically (descending by
    /// start so earlier ranges stay valid). Returns (forward, inverse) batches,
    /// or None if any range is invalid — in which case the text is untouched.
    fn apply_batch(&mut self, edits: &[TextEdit]) -> Option<(Vec<TextEdit>, Vec<TextEdit>)> {
        let mut sorted: Vec<&TextEdit> = edits.iter().collect();
        sorted.sort_by_key(|e| std::cmp::Reverse(e.start));

        for e in &sorted {
            if e.start > e.end
                || e.end > self.text.len()
                || !self.text.is_char_boundary(e.start)
                || !self.text.is_char_boundary(e.end)
            {
                return None;
            }
        }
        // Reject overlapping edits within one batch.
        for pair in sorted.windows(2) {
            if pair[1].end > pair[0].start {
                return None;
            }
        }

        let mut inverse = Vec::new();
        for e in &sorted {
            let old = self.text[e.start..e.end].to_string();
            self.text.replace_range(e.start..e.end, &e.insert);
            inverse.push(TextEdit::new(e.start, e.start + e.insert.len(), old));
        }
        // Inverse ranges must be in post-batch coordinates: edits applied
        // later (at lower offsets) shift everything after them.
        let mut shift = 0i64;
        for (i, e) in sorted.iter().enumerate().rev() {
            let inv = &mut inverse[i];
            inv.start = (inv.start as i64 + shift) as usize;
            inv.end = (inv.end as i64 + shift) as usize;
            shift += e.insert.len() as i64 - (e.end - e.start) as i64;
        }
        Some((sorted.into_iter().cloned().collect(), inverse))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const TODAY: i64 = 20675;

    fn doc(src: &str) -> Document {
        Document::new(src.to_string(), TODAY)
    }

    fn typing(edits: Vec<TextEdit>) -> Op {
        Op { edits, intent: Intent::Typing, actor: 0, seq: 0 }
    }

    #[test]
    fn apply_and_undo_roundtrip() {
        let mut d = doc("task \"A\" : a, 5d\n");
        let pos = d.text().find(": a").unwrap() + 2;
        d.apply(typing(vec![TextEdit::new(pos, pos + 1, "b")]));
        assert_eq!(d.text(), "task \"A\" : b, 5d\n");
        assert!(d.model().tasks.contains_key("b"));
        d.undo();
        assert_eq!(d.text(), "task \"A\" : a, 5d\n");
        assert!(d.model().tasks.contains_key("a"));
        d.redo();
        assert_eq!(d.text(), "task \"A\" : b, 5d\n");
    }

    #[test]
    fn rejects_out_of_bounds() {
        let mut d = doc("abc");
        assert_eq!(
            d.apply(typing(vec![TextEdit::new(2, 10, "x")])),
            ApplyResult::Rejected
        );
        assert_eq!(d.text(), "abc");
    }

    #[test]
    fn coalesced_drags_undo_as_one() {
        let mut d = doc("task \"A\" : a, 2026-08-10, 5d\n");
        let span = match &d.model().tasks["a"].start_src {
            crate::model::StartSrc::Date { span } => *span,
            _ => panic!(),
        };
        let drag = |seq: u64, date: &str| Op {
            edits: vec![TextEdit::new(span.start, span.end, date)],
            intent: Intent::MoveBar("a".into()),
            actor: 0,
            seq,
        };
        d.apply(drag(1, "2026-08-11"));
        d.apply(drag(2, "2026-08-12"));
        d.undo();
        assert!(d.text().contains("2026-08-10"));
        assert!(!d.can_undo());
    }
}
