//! Property tests for the riskiest core logic: visual-edit writeback must
//! change only the target task's fields while leaving every other byte of the
//! file intact, drags must land exactly where they said they would, and undo
//! must restore exact bytes after arbitrary edit storms.

use proptest::prelude::*;
use tangt_core::document::{ApplyResult, Document};
use tangt_core::ops::{Intent, Op, TextEdit};
use tangt_core::rewrite::{delete_task, move_task, resize_end};

/// 2026-08-10, a Monday.
const TODAY: i64 = 20675;

fn doc(src: &str) -> Document {
    Document::new(src.to_string(), TODAY)
}

const SRC: &str = "// header comment\n\
title \"Plan\"\n\
exclude weekends\n\
section Design\n\
task \"Alpha\" : a, 2026-08-10, 5d // trail\n\
task \"Beta\" : b, after a + 1d, 3d\n\
milestone \"Gate\" : g, after b\n\
section Build\n\
task \"Gamma\" : c, after a b, until 2026-09-18 // keep\n\
task \"Delta\" : d, 4d, 40%, #4fa3ff\n";

proptest! {
    #[test]
    fn drag_rewrite_touches_only_the_target_fields(
        offsets in proptest::collection::vec((0usize..4, -20i64..40), 1..12)
    ) {
        let mut d = doc(SRC);
        let ids = ["a", "b", "c", "d"];
        for (which, delta) in offsets {
            let id = ids[which];
            let task = d.model().tasks[id].clone();
            let target = d.model().calendar.snap_forward(task.start_day + delta);
            let edits = move_task(d.model(), &task, target);
            if edits.is_empty() { continue; }
            let applied = d.apply(Op {
                edits,
                intent: Intent::MoveBar(id.into()),
                actor: 0,
                seq: 0,
            });
            prop_assert_eq!(applied, ApplyResult::Applied);
            // The bar landed exactly where the drag said (workday targets).
            prop_assert_eq!(d.model().tasks[id].start_day, target,
                "drag of {} by {} landed wrong\ntext: {}", id, delta, d.text());
        }
        // Comments and unrelated statements survived every rewrite.
        prop_assert!(d.text().contains("// header comment"));
        prop_assert!(d.text().contains("// trail"));
        prop_assert!(d.text().contains("// keep"));
        prop_assert!(d.text().contains("title \"Plan\""));
        prop_assert!(d.text().contains("section Design"));
        prop_assert!(d.text().contains("40%, #4fa3ff"));
        prop_assert!(!d.model().has_errors(), "diags: {:?}", d.model().diagnostics);
    }

    #[test]
    fn drag_to_same_day_is_text_idempotent(delta in -15i64..30) {
        let mut d = doc(SRC);
        let task = d.model().tasks["b"].clone();
        let target = d.model().calendar.snap_forward(task.start_day + delta);
        for _ in 0..3 {
            let task = d.model().tasks["b"].clone();
            let edits = move_task(d.model(), &task, target);
            if edits.is_empty() { break; }
            d.apply(Op { edits, intent: Intent::MoveBar("b".into()), actor: 0, seq: 0 });
        }
        let once = d.text().to_string();
        let task = d.model().tasks["b"].clone();
        let edits = move_task(d.model(), &task, target);
        prop_assert!(edits.is_empty(), "re-drag to same day produced edits: {:?}", edits);
        prop_assert_eq!(d.text(), once);
    }

    #[test]
    fn resize_roundtrips_duration(new_len in 1i64..30) {
        let mut d = doc(SRC);
        let task = d.model().tasks["a"].clone();
        let target_end = d.model().calendar.span_end(task.start_day, new_len);
        let edits = resize_end(d.model(), &task, target_end);
        d.apply(Op { edits, intent: Intent::ResizeEnd("a".into()), actor: 0, seq: 0 });
        prop_assert_eq!(d.model().tasks["a"].end_day, target_end);
        let expected = format!("{new_len}d");
        prop_assert!(d.text().contains(&expected));
    }

    #[test]
    fn undo_restores_exact_bytes(
        edits in proptest::collection::vec((0usize..300, 0usize..10, "[a-z0-9,: \\n\"]{0,8}"), 1..10)
    ) {
        let mut d = doc(SRC);
        let mut applied = 0;
        for (start, len, insert) in edits {
            let s = start.min(d.text().len());
            let e = (s + len).min(d.text().len());
            if !d.text().is_char_boundary(s) || !d.text().is_char_boundary(e) { continue; }
            let op = Op {
                edits: vec![TextEdit::new(s, e, insert)],
                intent: Intent::Typing,
                actor: 0,
                seq: 0,
            };
            if d.apply(op) == ApplyResult::Applied {
                applied += 1;
            }
        }
        for _ in 0..applied {
            d.undo();
        }
        prop_assert_eq!(d.text(), SRC);
    }

    #[test]
    fn delete_leaves_no_dangling_references(which in 0usize..4) {
        let mut d = doc(SRC);
        let ids = ["a", "b", "c", "d"];
        let id = ids[which];
        let task = d.model().tasks[id].clone();
        let edits = delete_task(d.model(), d.text(), &task);
        prop_assert_eq!(
            d.apply(Op { edits, intent: Intent::DeleteTask, actor: 0, seq: 0 }),
            ApplyResult::Applied
        );
        prop_assert!(!d.model().tasks.contains_key(id));
        prop_assert!(!d.model().has_errors(), "diags after deleting {}: {:?}\ntext: {}",
            id, d.model().diagnostics, d.text());
        // And undo brings the exact file back.
        d.undo();
        prop_assert_eq!(d.text(), SRC);
    }
}
