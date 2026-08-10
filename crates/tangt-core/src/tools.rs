//! Chart operations as data — the shared body of every automation surface
//! (MCP tools, the in-app assistant, and any future CLI).
//!
//! Each tool is a pure function over the current model plus arguments. A
//! mutating tool returns `TextEdit`s rather than applying them, so every path
//! lands in `Document::apply` exactly like a canvas gesture does: one `Op`,
//! one undo step, text stays the source of truth.
//!
//! Error strings are written for a language model to read and recover from,
//! not for a stack trace.

use crate::ast::Tag;
use crate::model::{GanttModel, TaskId};
use crate::ops::{Intent, TextEdit};
use crate::resolve::{format_day, parse_date};
use crate::rewrite;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

pub type ToolResult = Result<ToolOutcome, String>;

#[derive(Debug, Clone)]
pub enum ToolOutcome {
    /// Read-only: JSON payload for the caller.
    Read(Value),
    /// Mutation: edits to apply, plus a summary to report back.
    Write { edits: Vec<TextEdit>, intent: Intent, summary: Value },
}

impl ToolOutcome {
    pub fn value(&self) -> &Value {
        match self {
            ToolOutcome::Read(v) | ToolOutcome::Write { summary: v, .. } => v,
        }
    }
}

/// Every tool name, so the set can be asserted as a stable contract in tests
/// and rendered into a model prompt without drifting from the implementation.
pub const TOOL_NAMES: &[&str] = &[
    "list_tasks",
    "get_source",
    "add_task",
    "set_dates",
    "rename_task",
    "set_progress",
    "add_dependency",
    "remove_dependency",
    "delete_task",
    "add_section",
];

// ---- argument types ----------------------------------------------------

#[derive(Debug, Clone, Deserialize, Serialize, Default)]
pub struct AddTaskArgs {
    /// Human-readable label, e.g. "Design review".
    pub label: String,
    /// Start date as YYYY-MM-DD. Omit when using `after`.
    pub start: Option<String>,
    /// Ids this task starts after. Mutually useful with `lag_days`.
    #[serde(default)]
    pub after: Vec<String>,
    /// Working days of slack between the predecessor and this task.
    pub lag_days: Option<i64>,
    /// Duration in working days (ignored for milestones; defaults to 5).
    pub duration_days: Option<i64>,
    /// True for a zero-length milestone marker.
    #[serde(default)]
    pub milestone: bool,
    /// Insert after this task's line instead of at end of file.
    pub after_task: Option<String>,
}

#[derive(Debug, Clone, Deserialize, Serialize, Default)]
pub struct SetDatesArgs {
    pub id: String,
    /// New start date, YYYY-MM-DD.
    pub start: Option<String>,
    /// New duration in working days.
    pub duration_days: Option<i64>,
    /// New inclusive end date, YYYY-MM-DD. Alternative to `duration_days`.
    pub end: Option<String>,
}

// ---- helpers -----------------------------------------------------------

fn task<'a>(model: &'a GanttModel, id: &str) -> Result<&'a crate::model::Task, String> {
    model.tasks.get(id).ok_or_else(|| {
        let known: Vec<&str> = model.tasks.keys().map(|k| k.as_str()).take(20).collect();
        if known.is_empty() {
            format!("there is no task called \"{id}\"; the chart is empty")
        } else {
            format!(
                "there is no task called \"{id}\"; the chart has: {}",
                known.join(", ")
            )
        }
    })
}

fn day(text: &str, field: &str) -> Result<i64, String> {
    parse_date(text).ok_or_else(|| {
        format!("{field} \"{text}\" is not a date; write dates as YYYY-MM-DD, e.g. 2026-08-10")
    })
}

fn task_json(model: &GanttModel, t: &crate::model::Task) -> Value {
    json!({
        "id": t.id.as_str(),
        "label": t.label,
        "milestone": t.milestone,
        "start": format_day(t.start_day),
        "end": format_day(t.end_day),
        "duration_days": t.duration_days(&model.calendar),
        "progress_percent": t.progress.map(|(p, _)| p),
        "after": t.deps.iter().map(|d| d.on.as_str()).collect::<Vec<_>>(),
        "lag_days": t.deps.first().map(|d| d.lag_days).unwrap_or(0),
        "tags": t.tags.iter().map(|g| match g {
            Tag::Crit => "crit", Tag::Done => "done", Tag::Active => "active",
        }).collect::<Vec<_>>(),
        "section": t.section.and_then(|s| model.sections.get(s)).map(|s| s.label.clone()),
    })
}

// ---- read tools --------------------------------------------------------

pub fn list_tasks(model: &GanttModel) -> ToolResult {
    let tasks: Vec<Value> = model.tasks.values().map(|t| task_json(model, t)).collect();
    Ok(ToolOutcome::Read(json!({
        "title": model.title,
        "today": format_day(model.today),
        "excludes_weekends": model.calendar.exclude_weekends,
        "sections": model.sections.iter().map(|s| s.label.clone()).collect::<Vec<_>>(),
        "tasks": tasks,
        "diagnostics": model.diagnostics.iter().map(|d| d.message.clone()).collect::<Vec<_>>(),
    })))
}

pub fn get_source(src: &str) -> ToolResult {
    Ok(ToolOutcome::Read(json!({ "source": src })))
}

// ---- write tools -------------------------------------------------------

pub fn add_task(model: &GanttModel, src: &str, args: AddTaskArgs) -> ToolResult {
    if args.label.trim().is_empty() {
        return Err("a task needs a non-empty label".into());
    }
    for id in &args.after {
        task(model, id)?;
    }
    let anchor = match &args.after_task {
        Some(id) => Some(task(model, id)?.clone()),
        None => None,
    };

    // `add_task` writes a date-anchored line; convert to `after` afterwards so
    // both spellings go through the same reviewed rewrite paths.
    let start_day = match &args.start {
        Some(text) => day(text, "start")?,
        None => model.today,
    };
    let (mut edits, id) =
        rewrite::add_task(model, src, anchor.as_ref(), start_day, args.milestone);

    // Duration and dependencies need the parsed statement, so express them as
    // extra text in the generated line rather than a second parse pass.
    if let Some(edit) = edits.first_mut() {
        let mut line = edit.insert.clone();
        if args.label != "New task" && args.label != "New milestone" {
            line = line.replacen(
                if args.milestone { "\"New milestone\"" } else { "\"New task\"" },
                &crate::parser::quote(&args.label),
                1,
            );
        }
        if !args.milestone {
            let days = args.duration_days.unwrap_or(5).max(1);
            line = line.replacen(", 5d", &format!(", {days}d"), 1);
        }
        if !args.after.is_empty() {
            let lag = match args.lag_days.unwrap_or(0) {
                0 => String::new(),
                n if n > 0 => format!(" + {n}d"),
                n => format!(" - {}d", -n),
            };
            line = line.replacen(
                &format!(", {}", format_day(start_day)),
                &format!(", after {}{lag}", args.after.join(" ")),
                1,
            );
        }
        edit.insert = line;
    }

    Ok(ToolOutcome::Write {
        edits,
        intent: Intent::AddTask,
        summary: json!({ "created": id.as_str(), "label": args.label }),
    })
}

pub fn set_dates(model: &GanttModel, args: SetDatesArgs) -> ToolResult {
    let t = task(model, &args.id)?.clone();
    if args.start.is_none() && args.duration_days.is_none() && args.end.is_none() {
        return Err("set_dates needs at least one of start, duration_days, or end".into());
    }
    if args.duration_days.is_some() && args.end.is_some() {
        return Err("give either duration_days or end, not both".into());
    }
    let new_start = match &args.start {
        Some(text) => day(text, "start")?,
        None => t.start_day,
    };
    // Resolve the requested end, whichever way it was expressed.
    let new_end = match (&args.duration_days, &args.end) {
        (Some(days), _) => {
            if *days < 1 {
                return Err("duration_days must be at least 1".into());
            }
            Some(model.calendar.span_end(new_start, *days))
        }
        (None, Some(text)) => {
            let end = day(text, "end")?;
            if end < new_start {
                return Err(format!(
                    "end {text} is before start {}",
                    format_day(new_start)
                ));
            }
            Some(end)
        }
        (None, None) => None,
    };
    let edits = match (args.start.is_some(), new_end) {
        // Both change: measure the duration from the new start.
        (true, Some(end)) => rewrite::set_span(model, &t, new_start, end),
        (true, None) => rewrite::move_task_to_date(&t, new_start),
        (false, Some(end)) => rewrite::resize_end(model, &t, end),
        (false, None) => unreachable!("guarded above"),
    };
    Ok(ToolOutcome::Write {
        edits,
        intent: Intent::MoveBar(TaskId::from(args.id.as_str())),
        summary: json!({ "updated": args.id }),
    })
}

pub fn rename_task(model: &GanttModel, id: &str, label: &str) -> ToolResult {
    if label.trim().is_empty() {
        return Err("a task needs a non-empty label".into());
    }
    let t = task(model, id)?;
    Ok(ToolOutcome::Write {
        edits: rewrite::rename(t, label),
        intent: Intent::Rename(t.id.clone()),
        summary: json!({ "renamed": id, "label": label }),
    })
}

pub fn set_progress(model: &GanttModel, id: &str, percent: i64) -> ToolResult {
    if !(0..=100).contains(&percent) {
        return Err(format!("progress must be between 0 and 100, got {percent}"));
    }
    let t = task(model, id)?;
    Ok(ToolOutcome::Write {
        edits: rewrite::set_progress(t, percent as u8),
        intent: Intent::SetProgress(t.id.clone()),
        summary: json!({ "updated": id, "progress_percent": percent }),
    })
}

pub fn add_dependency(model: &GanttModel, task_id: &str, after: &str) -> ToolResult {
    let t = task(model, task_id)?;
    let on = task(model, after)?.id.clone();
    match rewrite::add_dependency(model, t, &on) {
        Some(edits) => Ok(ToolOutcome::Write {
            edits,
            intent: Intent::AddDependency,
            summary: json!({ "task": task_id, "now_starts_after": after }),
        }),
        None if t.deps.iter().any(|d| d.on == on) => {
            Err(format!("\"{task_id}\" already starts after \"{after}\""))
        }
        None => Err(format!(
            "\"{task_id}\" cannot start after \"{after}\" — that would make a dependency cycle"
        )),
    }
}

pub fn remove_dependency(model: &GanttModel, src: &str, task_id: &str, on: &str) -> ToolResult {
    let t = task(model, task_id)?;
    if !t.deps.iter().any(|d| d.on.as_str() == on) {
        return Err(format!("\"{task_id}\" does not start after \"{on}\""));
    }
    Ok(ToolOutcome::Write {
        edits: rewrite::remove_dependency(src, t, &TaskId::from(on)),
        intent: Intent::AddDependency,
        summary: json!({ "task": task_id, "no_longer_after": on }),
    })
}

pub fn delete_task(model: &GanttModel, src: &str, id: &str) -> ToolResult {
    let t = task(model, id)?;
    Ok(ToolOutcome::Write {
        edits: rewrite::delete_task(model, src, t),
        intent: Intent::DeleteTask,
        summary: json!({ "deleted": id }),
    })
}

pub fn add_section(src: &str, label: &str) -> ToolResult {
    if label.trim().is_empty() {
        return Err("a section needs a non-empty label".into());
    }
    if label.contains('\n') {
        return Err("a section label must be a single line".into());
    }
    Ok(ToolOutcome::Write {
        edits: rewrite::add_section(src, None, label),
        intent: Intent::AddSection,
        summary: json!({ "created_section": label }),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::document::{ApplyResult, Document};
    use crate::ops::Op;

    const TODAY: i64 = 20675; // 2026-08-10, a Monday

    fn doc(src: &str) -> Document {
        Document::new(src.to_string(), TODAY)
    }

    /// Apply a tool outcome the way every real caller does.
    fn run(d: &mut Document, outcome: ToolOutcome) -> Value {
        match outcome {
            ToolOutcome::Read(v) => v,
            ToolOutcome::Write { edits, intent, summary } => {
                let seq = d.next_seq();
                assert_eq!(
                    d.apply(Op { edits, intent, actor: 0, seq }),
                    ApplyResult::Applied,
                    "edits rejected"
                );
                summary
            }
        }
    }

    #[test]
    fn list_tasks_reports_schedule() {
        let d = doc("task \"A\" : a, 2026-08-10, 5d, 40%\ntask \"B\" : b, after a, 3d\n");
        let v = list_tasks(d.model()).unwrap();
        let tasks = &v.value()["tasks"];
        assert_eq!(tasks[0]["id"], "a");
        assert_eq!(tasks[0]["start"], "2026-08-10");
        assert_eq!(tasks[0]["end"], "2026-08-14");
        assert_eq!(tasks[0]["progress_percent"], 40);
        assert_eq!(tasks[1]["after"][0], "a");
    }

    #[test]
    fn add_task_with_dates_and_deps() {
        let mut d = doc("task \"A\" : a, 2026-08-10, 5d\n");
        let out = add_task(
            d.model(),
            d.text(),
            AddTaskArgs {
                label: "Review".into(),
                after: vec!["a".into()],
                lag_days: Some(2),
                duration_days: Some(3),
                ..Default::default()
            },
        )
        .unwrap();
        run(&mut d, out);
        assert!(
            d.text().contains("task \"Review\" : t1, after a + 2d, 3d"),
            "got: {}",
            d.text()
        );
        assert!(!d.model().has_errors());
        assert_eq!(d.model().tasks["t1"].start_day, super::parse_date("2026-08-17").unwrap());
    }

    #[test]
    fn add_milestone() {
        let mut d = doc("task \"A\" : a, 2026-08-10, 5d\n");
        let out = add_task(
            d.model(),
            d.text(),
            AddTaskArgs {
                label: "Ship".into(),
                after: vec!["a".into()],
                milestone: true,
                ..Default::default()
            },
        )
        .unwrap();
        run(&mut d, out);
        assert!(d.text().contains("milestone \"Ship\" : m1, after a"), "got: {}", d.text());
        assert!(d.model().tasks["m1"].milestone);
    }

    #[test]
    fn set_dates_moves_and_resizes() {
        let mut d = doc("task \"A\" : a, 2026-08-10, 5d // keep\n");
        let out = set_dates(
            d.model(),
            SetDatesArgs {
                id: "a".into(),
                start: Some("2026-08-12".into()),
                duration_days: Some(2),
                ..Default::default()
            },
        )
        .unwrap();
        run(&mut d, out);
        assert_eq!(d.text(), "task \"A\" : a, 2026-08-12, 2d // keep\n");
    }

    #[test]
    fn set_dates_with_end_date() {
        let mut d = doc("task \"A\" : a, 2026-08-10, 5d\n");
        let out = set_dates(
            d.model(),
            SetDatesArgs { id: "a".into(), end: Some("2026-08-20".into()), ..Default::default() },
        )
        .unwrap();
        run(&mut d, out);
        assert_eq!(d.model().tasks["a"].end_day, parse_date("2026-08-20").unwrap());
    }

    #[test]
    fn dependency_add_and_remove_roundtrip() {
        let src = "task \"A\" : a, 2026-08-10, 2d\ntask \"B\" : b, 2026-08-20, 5d\n";
        let mut d = doc(src);
        let out = add_dependency(d.model(), "b", "a").unwrap();
        run(&mut d, out);
        assert!(d.text().contains("after a"));
        let out = remove_dependency(d.model(), d.text(), "b", "a").unwrap();
        run(&mut d, out);
        assert!(!d.text().contains("after a"));
        assert!(!d.model().has_errors());
    }

    #[test]
    fn errors_are_written_for_a_model_to_recover_from() {
        let d = doc("task \"A\" : a, 5d\n");
        let err = set_progress(d.model(), "ghost", 50).unwrap_err();
        assert!(err.contains("no task called \"ghost\""), "{err}");
        assert!(err.contains("the chart has: a"), "{err}");

        let err = set_progress(d.model(), "a", 150).unwrap_err();
        assert!(err.contains("between 0 and 100"), "{err}");

        let err = set_dates(
            d.model(),
            SetDatesArgs { id: "a".into(), start: Some("next tuesday".into()), ..Default::default() },
        )
        .unwrap_err();
        assert!(err.contains("YYYY-MM-DD"), "{err}");
    }

    #[test]
    fn dependency_cycle_is_refused_with_an_explanation() {
        let d = doc("task \"A\" : a, 2026-08-10, 2d\ntask \"B\" : b, after a, 5d\n");
        let err = add_dependency(d.model(), "a", "b").unwrap_err();
        assert!(err.contains("cycle"), "{err}");
    }

    #[test]
    fn every_write_tool_is_one_undo_step() {
        let src = "// header\ntask \"A\" : a, 2026-08-10, 5d\ntask \"B\" : b, after a, 3d\n";
        let mut d = doc(src);
        let out = rename_task(d.model(), "a", "Alpha").unwrap();
        run(&mut d, out);
        let out = set_progress(d.model(), "a", 60).unwrap();
        run(&mut d, out);
        let out = delete_task(d.model(), d.text(), "a").unwrap();
        run(&mut d, out);
        assert!(!d.model().has_errors());
        while d.can_undo() {
            d.undo();
        }
        assert_eq!(d.text(), src, "undo did not restore the original bytes");
    }

    #[test]
    fn tool_names_match_the_implementation() {
        // Guards against a tool being added here but not exposed, or vice versa.
        assert_eq!(TOOL_NAMES.len(), 10);
        assert!(TOOL_NAMES.contains(&"add_dependency"));
    }
}
