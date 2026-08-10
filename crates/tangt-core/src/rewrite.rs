//! Surgical text rewriting for canvas→text sync. Only ever splices spans the
//! parser recorded, so comments and formatting elsewhere survive by
//! construction.
//!
//! Every function returns the `TextEdit`s for one gesture; the app wraps them
//! in an `Op` and hands them to `Document::apply`.

use crate::model::{Calendar, DurSrc, GanttModel, Row, StartSrc, Task, TaskId};
use crate::ops::TextEdit;
use crate::parser::quote;
use crate::resolve::format_day;
use smol_str::SmolStr;

/// Signed working-day distance from `from` to `to` (0 when equal).
fn signed_workdays(cal: &Calendar, from: i64, to: i64) -> i64 {
    if !cal.has_exclusions() {
        return to - from;
    }
    let (lo, hi, sign) = if to >= from { (from, to, 1) } else { (to, from, -1) };
    let mut n = 0;
    for d in (lo + 1)..=hi {
        if cal.is_workday(d) {
            n += 1;
        }
    }
    n * sign
}

fn fmt_lag(days: i64) -> String {
    if days >= 0 {
        format!("+ {days}d")
    } else {
        format!("- {}d", -days)
    }
}

/// Byte range of the whole line containing `span`, including the trailing
/// newline (if any) — what delete/reorder operate on.
pub fn line_bounds(src: &str, start: usize, end: usize) -> (usize, usize) {
    let line_start = src[..start].rfind('\n').map_or(0, |p| p + 1);
    let line_end = src[end..].find('\n').map_or(src.len(), |p| end + p + 1);
    (line_start, line_end)
}

// ---- move / resize -----------------------------------------------------

/// Drag a bar (or milestone) so it starts on `new_start`.
///
/// - Explicit date: the date span is replaced.
/// - `after …`: the dependency is preserved and the lag is spliced/inserted/
///   removed to make up the difference (in working days).
/// - Implicit: a date field is inserted at the end of the statement.
pub fn move_task(model: &GanttModel, task: &Task, new_start: i64) -> Vec<TextEdit> {
    let mut edits = move_start_only(model, task, new_start);
    // Moving the whole bar of an `until` task shifts its end date too,
    // preserving the working-day duration.
    if let DurSrc::Until { span } = &task.dur_src {
        if !edits.is_empty() {
            let n = model.calendar.working_days_between(task.start_day, task.end_day);
            let new_end = model.calendar.span_end(new_start, n);
            edits.push(TextEdit::new(span.start, span.end, format_day(new_end)));
        }
    }
    edits
}

fn move_start_only(model: &GanttModel, task: &Task, new_start: i64) -> Vec<TextEdit> {
    match &task.start_src {
        StartSrc::Date { span } => {
            vec![TextEdit::new(span.start, span.end, format_day(new_start))]
        }
        StartSrc::After { ids_end, lag_span, .. } => {
            let delta = signed_workdays(&model.calendar, task.start_day, new_start);
            if delta == 0 {
                return Vec::new();
            }
            let old_lag = task.deps.first().map_or(0, |d| d.lag_days);
            let new_lag = old_lag + delta;
            match (lag_span, new_lag) {
                (Some(span), 0) => vec![TextEdit::new(*ids_end, span.end, "")],
                (Some(span), lag) => {
                    vec![TextEdit::new(span.start, span.end, fmt_lag(lag))]
                }
                (None, 0) => Vec::new(),
                (None, lag) => {
                    vec![TextEdit::new(*ids_end, *ids_end, format!(" {}", fmt_lag(lag)))]
                }
            }
        }
        StartSrc::Implicit => vec![TextEdit::new(
            task.insert_field_at,
            task.insert_field_at,
            format!(", {}", format_day(new_start)),
        )],
    }
}

/// Alt-drag: force the task onto an explicit date, replacing an `after`
/// clause (its dependencies) if present.
pub fn move_task_to_date(task: &Task, new_start: i64) -> Vec<TextEdit> {
    match &task.start_src {
        StartSrc::Date { span } | StartSrc::After { span, .. } => {
            vec![TextEdit::new(span.start, span.end, format_day(new_start))]
        }
        StartSrc::Implicit => vec![TextEdit::new(
            task.insert_field_at,
            task.insert_field_at,
            format!(", {}", format_day(new_start)),
        )],
    }
}

/// Drag the right edge to `new_end` (clamped to the start day).
pub fn resize_end(model: &GanttModel, task: &Task, new_end: i64) -> Vec<TextEdit> {
    let new_end = new_end.max(task.start_day);
    match &task.dur_src {
        DurSrc::Until { span } => {
            vec![TextEdit::new(span.start, span.end, format_day(new_end))]
        }
        DurSrc::Duration { span } => {
            let n = model.calendar.working_days_between(task.start_day, new_end);
            vec![TextEdit::new(span.start, span.end, format!("{n}d"))]
        }
        DurSrc::Implicit => {
            let n = model.calendar.working_days_between(task.start_day, new_end);
            vec![TextEdit::new(
                task.insert_field_at,
                task.insert_field_at,
                format!(", {n}d"),
            )]
        }
    }
}

/// Drag the left edge to `new_start`, keeping the end day fixed.
pub fn resize_start(model: &GanttModel, task: &Task, new_start: i64) -> Vec<TextEdit> {
    let new_start = new_start.min(task.end_day);
    let start_edits = move_start_only(model, task, new_start);
    // With an `until` end the end day is already pinned by the text.
    if matches!(task.dur_src, DurSrc::Until { .. }) {
        return start_edits;
    }
    let n = model.calendar.working_days_between(new_start, task.end_day);
    match &task.dur_src {
        DurSrc::Duration { span } => {
            let mut edits = start_edits;
            edits.push(TextEdit::new(span.start, span.end, format!("{n}d")));
            edits
        }
        DurSrc::Implicit => {
            // Both fields may need inserting at the same offset — merge into
            // one edit so field order comes out right.
            let dur = format!(", {n}d");
            let mut edits = start_edits;
            match edits.iter_mut().find(|e| e.start == task.insert_field_at && e.end == e.start)
            {
                Some(insert) => insert.insert.push_str(&dur),
                None => edits.push(TextEdit::new(
                    task.insert_field_at,
                    task.insert_field_at,
                    dur,
                )),
            }
            edits
        }
        DurSrc::Until { .. } => unreachable!(),
    }
}

// ---- simple field edits ------------------------------------------------

pub fn rename(task: &Task, new_label: &str) -> Vec<TextEdit> {
    vec![TextEdit::new(
        task.label_span.start,
        task.label_span.end,
        quote(new_label),
    )]
}

pub fn set_progress(task: &Task, percent: u8) -> Vec<TextEdit> {
    let percent = percent.min(100);
    match &task.progress {
        Some((_, span)) => vec![TextEdit::new(span.start, span.end, format!("{percent}%"))],
        None => vec![TextEdit::new(
            task.insert_field_at,
            task.insert_field_at,
            format!(", {percent}%"),
        )],
    }
}

// ---- dependencies ------------------------------------------------------

/// Would making `task` depend on `on` create a cycle?
pub fn creates_cycle(model: &GanttModel, task: &TaskId, on: &TaskId) -> bool {
    // Cycle iff `on` (transitively) depends on `task`.
    let mut stack = vec![on];
    let mut seen = std::collections::HashSet::new();
    while let Some(id) = stack.pop() {
        if id == task {
            return true;
        }
        if !seen.insert(id.clone()) {
            continue;
        }
        if let Some(t) = model.tasks.get(id) {
            stack.extend(t.deps.iter().map(|d| &d.on));
        }
    }
    false
}

/// Make `task` start after `on`. Returns None if the dependency already
/// exists or would create a cycle.
pub fn add_dependency(model: &GanttModel, task: &Task, on: &TaskId) -> Option<Vec<TextEdit>> {
    if task.id == *on
        || task.deps.iter().any(|d| d.on == *on)
        || creates_cycle(model, &task.id, on)
    {
        return None;
    }
    Some(match &task.start_src {
        StartSrc::After { ids_end, .. } => {
            vec![TextEdit::new(*ids_end, *ids_end, format!(" {on}"))]
        }
        // The gesture's meaning: the fixed start becomes a dependency.
        StartSrc::Date { span } => {
            vec![TextEdit::new(span.start, span.end, format!("after {on}"))]
        }
        StartSrc::Implicit => vec![TextEdit::new(
            task.insert_field_at,
            task.insert_field_at,
            format!(", after {on}"),
        )],
    })
}

// ---- add / delete / reorder --------------------------------------------

/// Smallest unused `{prefix}{n}` id.
pub fn gen_id(model: &GanttModel, prefix: &str) -> SmolStr {
    let mut n = 1;
    loop {
        let candidate = SmolStr::new(format!("{prefix}{n}"));
        if !model.tasks.contains_key(&candidate) {
            return candidate;
        }
        n += 1;
    }
}

/// Insert a new task (or milestone) line after `anchor`'s line, or at end of
/// file. Returns the edits and the generated id.
pub fn add_task(
    model: &GanttModel,
    src: &str,
    anchor: Option<&Task>,
    start_day: i64,
    milestone: bool,
) -> (Vec<TextEdit>, TaskId) {
    let id = gen_id(model, if milestone { "m" } else { "t" });
    let line = if milestone {
        format!("milestone \"New milestone\" : {id}, {}\n", format_day(start_day))
    } else {
        format!("task \"New task\" : {id}, {}, 5d\n", format_day(start_day))
    };
    let (offset, line) = match anchor {
        Some(a) => {
            let (_, line_end) = line_bounds(src, a.stmt_span.start, a.stmt_span.end);
            // A last line without trailing newline needs one before ours.
            if line_end == src.len() && !src.ends_with('\n') && !src.is_empty() {
                (line_end, format!("\n{line}"))
            } else {
                (line_end, line)
            }
        }
        None => {
            if src.is_empty() || src.ends_with('\n') {
                (src.len(), line)
            } else {
                (src.len(), format!("\n{line}"))
            }
        }
    };
    (vec![TextEdit::new(offset, offset, line)], id)
}

/// Insert `section <label>` at the end of the file (or after `anchor`).
pub fn add_section(src: &str, anchor: Option<&Task>, label: &str) -> Vec<TextEdit> {
    let line = format!("section {label}\n");
    let (offset, line) = match anchor {
        Some(a) => {
            let (_, line_end) = line_bounds(src, a.stmt_span.start, a.stmt_span.end);
            if line_end == src.len() && !src.ends_with('\n') && !src.is_empty() {
                (line_end, format!("\n{line}"))
            } else {
                (line_end, line)
            }
        }
        None => {
            if src.is_empty() || src.ends_with('\n') {
                (src.len(), line)
            } else {
                (src.len(), format!("\n{line}"))
            }
        }
    };
    vec![TextEdit::new(offset, offset, line)]
}

/// Delete a task's line and scrub it from every other task's `after` list.
/// A task whose only dependency was the deleted one gets its current start
/// date written instead, so it doesn't jump.
pub fn delete_task(model: &GanttModel, src: &str, task: &Task) -> Vec<TextEdit> {
    let (line_start, line_end) = line_bounds(src, task.stmt_span.start, task.stmt_span.end);
    let mut edits = vec![TextEdit::new(line_start, line_end, "")];

    for other in model.tasks.values() {
        if other.id == task.id {
            continue;
        }
        let Some(dep) = other.deps.iter().find(|d| d.on == task.id) else {
            continue;
        };
        if other.deps.len() == 1 {
            // Only dependency: pin the dependent where it is today.
            if let StartSrc::After { span, .. } = &other.start_src {
                edits.push(TextEdit::new(
                    span.start,
                    span.end,
                    format_day(other.start_day),
                ));
            }
        } else if let StartSrc::After { ids_end, .. } = &other.start_src {
            // Remove this id from the list, eating one adjacent gap.
            let (mut s, mut e) = (dep.span.start, dep.span.end);
            if e < *ids_end {
                // Not the last id: eat the following whitespace.
                e += src[e..*ids_end].len() - src[e..*ids_end].trim_start().len();
            } else {
                // Last id: eat the preceding whitespace.
                s -= src[..s].len() - src[..s].trim_end().len();
            }
            edits.push(TextEdit::new(s, e, ""));
        }
    }
    edits
}

/// Delete a section header line. Its tasks join the previous section.
pub fn delete_section(model: &GanttModel, src: &str, section: usize) -> Vec<TextEdit> {
    let Some(sec) = model.sections.get(section) else {
        return Vec::new();
    };
    let (line_start, line_end) = line_bounds(src, sec.stmt_span.start, sec.stmt_span.end);
    vec![TextEdit::new(line_start, line_end, "")]
}

/// Move a task's whole line so it sits immediately before the row currently
/// at `before_row` (or at the end of the task list if None). The moved line's
/// bytes are preserved exactly.
pub fn reorder_task(
    model: &GanttModel,
    src: &str,
    task: &Task,
    before_row: Option<usize>,
) -> Vec<TextEdit> {
    let (line_start, line_end) = line_bounds(src, task.stmt_span.start, task.stmt_span.end);
    let mut line = src[line_start..line_end].to_string();
    if !line.ends_with('\n') {
        line.push('\n');
    }

    let dest = match before_row.and_then(|r| model.rows.get(r)) {
        Some(Row::Task(id)) => {
            let t = &model.tasks[id];
            line_bounds(src, t.stmt_span.start, t.stmt_span.end).0
        }
        Some(Row::Section(s)) => {
            let sec = &model.sections[*s];
            line_bounds(src, sec.stmt_span.start, sec.stmt_span.end).0
        }
        None => src.len(),
    };
    if dest >= line_start && dest <= line_end {
        return Vec::new(); // dropped onto itself
    }

    let mut edits = vec![TextEdit::new(line_start, line_end, "")];
    if dest == src.len() && !src.ends_with('\n') {
        edits.push(TextEdit::new(dest, dest, format!("\n{}", line.trim_end_matches('\n'))));
    } else {
        edits.push(TextEdit::new(dest, dest, line));
    }
    edits
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::document::Document;
    use crate::ops::{Intent, Op};
    use crate::resolve::parse_date;

    const TODAY: i64 = 20675; // 2026-08-10, a Monday

    fn doc(src: &str) -> Document {
        Document::new(src.to_string(), TODAY)
    }

    fn day(s: &str) -> i64 {
        parse_date(s).unwrap()
    }

    fn apply(d: &mut Document, intent: Intent, edits: Vec<TextEdit>) {
        assert_eq!(
            d.apply(Op { edits, intent, actor: 0, seq: 0 }),
            crate::document::ApplyResult::Applied
        );
    }

    fn move_to(d: &mut Document, id: &str, date: &str) {
        let task = d.model().tasks[id].clone();
        let edits = move_task(d.model(), &task, day(date));
        apply(d, Intent::MoveBar(id.into()), edits);
    }

    #[test]
    fn move_explicit_date_replaces_in_place() {
        let mut d = doc("task \"A\" : a, 2026-08-10, 5d // keep me\n");
        move_to(&mut d, "a", "2026-08-12");
        assert_eq!(d.text(), "task \"A\" : a, 2026-08-12, 5d // keep me\n");
    }

    #[test]
    fn move_after_task_splices_lag_keeping_dependency() {
        let mut d = doc("task \"A\" : a, 2026-08-10, 5d\ntask \"B\" : b, after a, 3d\n");
        // B starts 08-15; drag two days later.
        move_to(&mut d, "b", "2026-08-17");
        assert_eq!(
            d.text(),
            "task \"A\" : a, 2026-08-10, 5d\ntask \"B\" : b, after a + 2d, 3d\n"
        );
        assert_eq!(d.model().tasks["b"].start_day, day("2026-08-17"));
    }

    #[test]
    fn move_after_task_back_to_zero_lag_removes_clause() {
        let mut d =
            doc("task \"A\" : a, 2026-08-10, 5d\ntask \"B\" : b, after a + 2d, 3d\n");
        move_to(&mut d, "b", "2026-08-15");
        assert_eq!(
            d.text(),
            "task \"A\" : a, 2026-08-10, 5d\ntask \"B\" : b, after a, 3d\n"
        );
    }

    #[test]
    fn move_implicit_inserts_date_before_comment() {
        let mut d = doc("task \"A\" : a, 5d // note\n");
        move_to(&mut d, "a", "2026-08-12");
        assert_eq!(d.text(), "task \"A\" : a, 5d, 2026-08-12 // note\n");
    }

    #[test]
    fn alt_move_converts_dependency_to_date() {
        let mut d = doc("task \"A\" : a, 2026-08-10, 5d\ntask \"B\" : b, after a + 1d, 3d\n");
        let task = d.model().tasks["b"].clone();
        let edits = move_task_to_date(&task, day("2026-08-20"));
        apply(&mut d, Intent::MoveBar("b".into()), edits);
        assert_eq!(
            d.text(),
            "task \"A\" : a, 2026-08-10, 5d\ntask \"B\" : b, 2026-08-20, 3d\n"
        );
    }

    #[test]
    fn resize_end_updates_duration() {
        let mut d = doc("task \"A\" : a, 2026-08-10, 5d\n");
        let task = d.model().tasks["a"].clone();
        let edits = resize_end(d.model(), &task, day("2026-08-12"));
        apply(&mut d, Intent::ResizeEnd("a".into()), edits);
        assert_eq!(d.text(), "task \"A\" : a, 2026-08-10, 3d\n");
    }

    #[test]
    fn resize_end_respects_weekends() {
        let mut d = doc("exclude weekends\ntask \"A\" : a, 2026-08-10, 5d\n");
        let task = d.model().tasks["a"].clone();
        // Drag to the following Tuesday: Mon-Fri + Mon + Tue = 7 workdays.
        let edits = resize_end(d.model(), &task, day("2026-08-18"));
        apply(&mut d, Intent::ResizeEnd("a".into()), edits);
        assert_eq!(d.text(), "exclude weekends\ntask \"A\" : a, 2026-08-10, 7d\n");
    }

    #[test]
    fn resize_end_with_until_replaces_date() {
        let mut d = doc("task \"A\" : a, 2026-08-10, until 2026-08-20\n");
        let task = d.model().tasks["a"].clone();
        let edits = resize_end(d.model(), &task, day("2026-08-25"));
        apply(&mut d, Intent::ResizeEnd("a".into()), edits);
        assert_eq!(d.text(), "task \"A\" : a, 2026-08-10, until 2026-08-25\n");
    }

    #[test]
    fn resize_start_keeps_end_fixed() {
        let mut d = doc("task \"A\" : a, 2026-08-10, 5d\n");
        let task = d.model().tasks["a"].clone();
        let edits = resize_start(d.model(), &task, day("2026-08-12"));
        apply(&mut d, Intent::ResizeStart("a".into()), edits);
        assert_eq!(d.text(), "task \"A\" : a, 2026-08-12, 3d\n");
        assert_eq!(d.model().tasks["a"].end_day, day("2026-08-14"));
    }

    #[test]
    fn resize_start_implicit_both_inserts_merge() {
        let mut d = doc("task \"A\" : a\n");
        let task = d.model().tasks["a"].clone();
        assert_eq!(task.start_day, TODAY);
        let edits = resize_start(d.model(), &task, TODAY - 2);
        apply(&mut d, Intent::ResizeStart("a".into()), edits);
        assert_eq!(d.text(), "task \"A\" : a, 2026-08-08, 3d\n");
    }

    #[test]
    fn rename_escapes_quotes() {
        let mut d = doc("task \"A\" : a, 5d\n");
        let task = d.model().tasks["a"].clone();
        let edits = rename(&task, "Say \"hi\"");
        apply(&mut d, Intent::Rename("a".into()), edits);
        assert_eq!(d.text(), "task \"Say \\\"hi\\\"\" : a, 5d\n");
        assert_eq!(d.model().tasks["a"].label, "Say \"hi\"");
    }

    #[test]
    fn set_progress_inserts_or_replaces() {
        let mut d = doc("task \"A\" : a, 5d\n");
        let task = d.model().tasks["a"].clone();
        apply(&mut d, Intent::SetProgress("a".into()), set_progress(&task, 40));
        assert_eq!(d.text(), "task \"A\" : a, 5d, 40%\n");
        let task = d.model().tasks["a"].clone();
        apply(&mut d, Intent::SetProgress("a".into()), set_progress(&task, 80));
        assert_eq!(d.text(), "task \"A\" : a, 5d, 80%\n");
    }

    #[test]
    fn add_dependency_appends_to_list() {
        let mut d = doc(
            "task \"A\" : a, 2026-08-10, 2d\ntask \"B\" : b, 2026-08-10, 5d\ntask \"C\" : c, after a, 1d\n",
        );
        let task = d.model().tasks["c"].clone();
        let edits = add_dependency(d.model(), &task, &"b".into()).unwrap();
        apply(&mut d, Intent::AddDependency, edits);
        assert!(d.text().contains("after a b, 1d"));
    }

    #[test]
    fn add_dependency_converts_date() {
        let mut d = doc("task \"A\" : a, 2026-08-10, 2d\ntask \"B\" : b, 2026-08-20, 5d\n");
        let task = d.model().tasks["b"].clone();
        let edits = add_dependency(d.model(), &task, &"a".into()).unwrap();
        apply(&mut d, Intent::AddDependency, edits);
        assert!(d.text().contains("task \"B\" : b, after a, 5d"));
    }

    #[test]
    fn add_dependency_refuses_cycle() {
        let d = doc("task \"A\" : a, 2026-08-10, 2d\ntask \"B\" : b, after a, 5d\n");
        let task_a = d.model().tasks["a"].clone();
        assert!(add_dependency(d.model(), &task_a, &"b".into()).is_none());
        let task_b = d.model().tasks["b"].clone();
        assert!(add_dependency(d.model(), &task_b, &"a".into()).is_none()); // duplicate
    }

    #[test]
    fn add_task_after_anchor() {
        let mut d = doc("task \"A\" : a, 2026-08-10, 5d // c\ntask \"B\" : b, after a, 3d\n");
        let anchor = d.model().tasks["a"].clone();
        let (edits, id) = add_task(d.model(), d.text(), Some(&anchor), day("2026-08-11"), false);
        apply(&mut d, Intent::AddTask, edits);
        assert_eq!(id, "t1");
        assert_eq!(
            d.text(),
            "task \"A\" : a, 2026-08-10, 5d // c\ntask \"New task\" : t1, 2026-08-11, 5d\ntask \"B\" : b, after a, 3d\n"
        );
    }

    #[test]
    fn delete_task_scrubs_multi_dep_list() {
        let mut d = doc(
            "task \"A\" : a, 2026-08-10, 2d\ntask \"B\" : b, 2026-08-10, 5d\ntask \"C\" : c, after a b + 1d, 1d\n",
        );
        let task = d.model().tasks["a"].clone();
        let edits = delete_task(d.model(), d.text(), &task);
        apply(&mut d, Intent::DeleteTask, edits);
        assert_eq!(
            d.text(),
            "task \"B\" : b, 2026-08-10, 5d\ntask \"C\" : c, after b + 1d, 1d\n"
        );
        assert!(!d.model().has_errors());
    }

    #[test]
    fn delete_task_pins_sole_dependent() {
        let mut d = doc("task \"A\" : a, 2026-08-10, 5d\ntask \"B\" : b, after a, 3d\n");
        let b_start = d.model().tasks["b"].start_day;
        let task = d.model().tasks["a"].clone();
        let edits = delete_task(d.model(), d.text(), &task);
        apply(&mut d, Intent::DeleteTask, edits);
        assert_eq!(d.text(), format!("task \"B\" : b, {}, 3d\n", format_day(b_start)));
        assert!(!d.model().has_errors());
    }

    #[test]
    fn reorder_moves_line_bytes_exactly() {
        let src = "task \"A\" : a, 2026-08-10, 5d // c\ntask \"B\" : b, 2026-08-11, 3d\ntask \"C\" : c, 2026-08-12, 1d\n";
        let mut d = doc(src);
        // Move C before B (B is row 1).
        let task = d.model().tasks["c"].clone();
        let b_row = d.model().tasks["b"].row;
        let edits = reorder_task(d.model(), d.text(), &task, Some(b_row));
        apply(&mut d, Intent::Reorder("c".into()), edits);
        assert_eq!(
            d.text(),
            "task \"A\" : a, 2026-08-10, 5d // c\ntask \"C\" : c, 2026-08-12, 1d\ntask \"B\" : b, 2026-08-11, 3d\n"
        );
    }

    #[test]
    fn undo_restores_exact_bytes_after_gesture_storm() {
        let src = "title \"Plan\"\n// header comment\ntask \"A\" : a, 2026-08-10, 5d // keep\ntask \"B\" : b, after a, 3d\n";
        let mut d = doc(src);
        move_to(&mut d, "b", "2026-08-19");
        let task = d.model().tasks["a"].clone();
        let edits = resize_end(d.model(), &task, day("2026-08-11"));
        apply(&mut d, Intent::ResizeEnd("a".into()), edits);
        let task = d.model().tasks["b"].clone();
        apply(&mut d, Intent::Rename("b".into()), rename(&task, "Renamed"));
        while d.can_undo() {
            d.undo();
        }
        assert_eq!(d.text(), src);
    }
}
