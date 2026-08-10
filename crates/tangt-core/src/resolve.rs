//! The scheduler: turns a parsed `Ast` into a `GanttModel`.
//!
//! All date parsing happens here — jiff types never escape this module (day
//! numbers do). Tasks are scheduled in dependency order regardless of textual
//! order; cycles and unknown ids are diagnosed and given fallback dates so a
//! bad edit never blanks the chart.

use crate::ast::{
    Ast, AxisKind, DateLit, Diagnostic, DurField, ExcludeKind, StartField, Stmt, TaskStmt,
};
use crate::model::{Calendar, Dep, DurSrc, GanttModel, Row, Section, StartSrc, Task, TaskId};
use indexmap::IndexMap;
use jiff::civil::Date;
use std::collections::{HashMap, HashSet};

/// Longest holiday range we will expand, in days.
const MAX_EXCLUDE_RANGE: i64 = 3660;
/// Total excluded days we will hold. Bounds both memory and every day-by-day
/// loop in `Calendar` — without it, many chained ranges amplify a small file
/// into a multi-second reparse on every keystroke.
const MAX_HOLIDAYS: usize = 20_000;

/// jiff's representable civil-date window as day numbers (years -9999..9999).
/// Every resolved day is clamped into it so hostile files can neither panic
/// date formatting nor drive unbounded loops downstream.
pub const MIN_DAY: i64 = -4_371_587;
pub const MAX_DAY: i64 = 2_932_896;
/// Longest span a single task may cover (~200 years) and the cap between a
/// dependency end and its successor (lag clamp).
pub const MAX_TASK_SPAN: i64 = 73_000;
/// Longest total chart range; keeps per-frame tick generation bounded.
pub const MAX_CHART_SPAN: i64 = 73_000;

fn epoch() -> Date {
    Date::constant(1970, 1, 1)
}

pub fn day_of(date: Date) -> i64 {
    (date - epoch()).get_days() as i64
}

/// Total function: out-of-range day numbers saturate to jiff's bounds.
pub fn date_of(day: i64) -> Date {
    epoch()
        .checked_add(jiff::Span::new().days(day.clamp(MIN_DAY, MAX_DAY)))
        .expect("clamped day is always in range")
}

/// Format a day number as the DSL's `YYYY-MM-DD`.
pub fn format_day(day: i64) -> String {
    date_of(day).to_string()
}

/// `"Aug 2026"` — month header label.
pub fn month_label(day: i64) -> String {
    date_of(day).strftime("%b %Y").to_string()
}

/// Day of month, for day/week tick labels.
pub fn day_of_month(day: i64) -> i8 {
    date_of(day).day()
}

/// First day of the month containing `day`.
pub fn month_start(day: i64) -> i64 {
    day - (date_of(day).day() as i64 - 1)
}

pub fn parse_date(text: &str) -> Option<i64> {
    text.parse::<Date>().ok().map(day_of)
}

/// Today's local date as a day number — the app's `today_fallback`.
pub fn today_local() -> i64 {
    day_of(jiff::Zoned::now().date())
}

/// `today_fallback`: today's date as a day number, used when the file has no
/// `today` directive (the app passes the real date; tests pass a constant).
pub fn resolve(ast: &Ast, today_fallback: i64) -> GanttModel {
    let mut diags = ast.diagnostics.clone();
    let date = |lit: &DateLit, diags: &mut Vec<Diagnostic>| -> Option<i64> {
        match parse_date(&lit.text) {
            Some(d) => Some(d),
            None => {
                diags.push(Diagnostic::error(lit.span, format!("invalid date `{}`", lit.text)));
                None
            }
        }
    };

    // ---- directives ----------------------------------------------------
    let mut title = None;
    let mut axis = AxisKind::Auto;
    let mut today = None;
    let mut calendar = Calendar::default();

    for stmt in &ast.stmts {
        match stmt {
            Stmt::Title { value, .. } => title = Some(value.clone()),
            Stmt::Axis { value, .. } => axis = *value,
            Stmt::Today { date: lit, .. } => {
                today = date(lit, &mut diags);
            }
            Stmt::Exclude { kind, .. } => match kind {
                ExcludeKind::Weekends => calendar.exclude_weekends = true,
                ExcludeKind::Date(lit) => {
                    if let Some(d) = date(lit, &mut diags) {
                        if calendar.holidays.len() < MAX_HOLIDAYS {
                            calendar.holidays.insert(d);
                        } else {
                            diags.push(Diagnostic::error(
                                lit.span,
                                format!("too many excluded days in this file (limit {MAX_HOLIDAYS})"),
                            ));
                        }
                    }
                }
                ExcludeKind::Range(a, b) => {
                    if let (Some(da), Some(db)) = (date(a, &mut diags), date(b, &mut diags)) {
                        if db < da {
                            diags.push(Diagnostic::error(
                                a.span.merge(b.span),
                                "exclude range ends before it starts",
                            ));
                        } else if db - da > MAX_EXCLUDE_RANGE {
                            diags.push(Diagnostic::error(
                                a.span.merge(b.span),
                                "exclude range is longer than ten years",
                            ));
                        } else if calendar.holidays.len() + (db - da + 1) as usize > MAX_HOLIDAYS {
                            diags.push(Diagnostic::error(
                                a.span.merge(b.span),
                                format!("too many excluded days in this file (limit {MAX_HOLIDAYS})"),
                            ));
                        } else {
                            calendar.holidays.extend(da..=db);
                        }
                    }
                }
            },
            _ => {}
        }
    }
    let today = today.unwrap_or(today_fallback);

    // ---- rows, sections, duplicate ids ---------------------------------
    let mut sections = Vec::new();
    let mut rows = Vec::new();
    let mut stmts: Vec<&TaskStmt> = Vec::new(); // parallel to task rows
    let mut seen: HashSet<TaskId> = HashSet::new();

    for stmt in &ast.stmts {
        match stmt {
            Stmt::Section { label, label_span, stmt_span } => {
                sections.push(Section {
                    label: label.clone(),
                    label_span: *label_span,
                    stmt_span: *stmt_span,
                    row: rows.len(),
                });
                rows.push(Row::Section(sections.len() - 1));
            }
            Stmt::Task(t) => {
                if !seen.insert(t.id.clone()) {
                    diags.push(Diagnostic::error(
                        t.id_span,
                        format!("duplicate task id `{}` — this task is ignored", t.id),
                    ));
                    continue;
                }
                rows.push(Row::Task(t.id.clone()));
                stmts.push(t);
            }
            _ => {}
        }
    }

    // ---- dependency graph & scheduling order ---------------------------
    let index_of: HashMap<&TaskId, usize> =
        stmts.iter().enumerate().map(|(i, t)| (&t.id, i)).collect();

    // edges[i] = tasks that must be scheduled before i (deps + implicit prev).
    let mut before: Vec<Vec<usize>> = vec![Vec::new(); stmts.len()];
    for (i, t) in stmts.iter().enumerate() {
        match &t.start {
            Some(StartField::After(after)) => {
                for (id, span) in &after.ids {
                    match index_of.get(id) {
                        Some(&j) => before[i].push(j),
                        None => diags.push(Diagnostic::error(
                            *span,
                            format!("unknown task id `{id}` in `after`"),
                        )),
                    }
                }
            }
            // Implicit start: follows the previous task in file order.
            None if i > 0 => before[i].push(i - 1),
            _ => {}
        }
    }

    // Kahn's algorithm; leftovers are cycle members.
    let mut indegree: Vec<usize> = vec![0; stmts.len()];
    let mut after_edges: Vec<Vec<usize>> = vec![Vec::new(); stmts.len()];
    for (i, bs) in before.iter().enumerate() {
        indegree[i] = bs.len();
        for &j in bs {
            after_edges[j].push(i);
        }
    }
    let mut queue: Vec<usize> = (0..stmts.len()).filter(|&i| indegree[i] == 0).collect();
    let mut order = Vec::with_capacity(stmts.len());
    while let Some(i) = queue.pop() {
        order.push(i);
        for &j in &after_edges[i] {
            indegree[j] -= 1;
            if indegree[j] == 0 {
                queue.push(j);
            }
        }
    }
    let mut in_cycle: Vec<usize> = (0..stmts.len()).filter(|&i| indegree[i] > 0).collect();
    if !in_cycle.is_empty() {
        let names: Vec<&str> = in_cycle.iter().map(|&i| stmts[i].id.as_str()).collect();
        let span = stmts[in_cycle[0]].start.as_ref().map_or(stmts[in_cycle[0]].stmt_span, |s| s.span());
        diags.push(Diagnostic::error(
            span,
            format!("dependency cycle between: {}", names.join(", ")),
        ));
        // Schedule them anyway (at fallback) so the chart still draws.
        order.append(&mut in_cycle);
    }

    // ---- schedule ------------------------------------------------------
    let mut start_days: Vec<i64> = vec![today; stmts.len()];
    let mut end_days: Vec<i64> = vec![today; stmts.len()];

    for &i in &order {
        let t = stmts[i];
        let start = match &t.start {
            Some(StartField::Date(lit)) => date(lit, &mut diags).unwrap_or(today),
            Some(StartField::After(after)) => {
                let lag = after
                    .lag
                    .as_ref()
                    .map_or(0, |l| l.sign.saturating_mul(l.unit.days(l.n)))
                    .clamp(-MAX_TASK_SPAN, MAX_TASK_SPAN);
                let base = after
                    .ids
                    .iter()
                    .filter_map(|(id, _)| index_of.get(id))
                    .map(|&j| end_days[j])
                    .max();
                match base {
                    Some(dep_end) => calendar.add_working_days(dep_end, 1 + lag),
                    None => today, // all ids unknown; already diagnosed
                }
            }
            None => {
                if i == 0 {
                    today
                } else {
                    calendar.add_working_days(end_days[i - 1], 1)
                }
            }
        };
        let start = if matches!(t.start, Some(StartField::Date(_))) {
            start
        } else {
            calendar.snap_forward(start)
        }
        .clamp(MIN_DAY, MAX_DAY);

        let end = if t.milestone {
            start
        } else {
            match &t.dur {
                Some(DurField::Duration { n, unit, .. }) => calendar.span_end(start, unit.days(*n)),
                Some(DurField::Until(lit)) => {
                    let e = date(lit, &mut diags).unwrap_or(start);
                    if e < start {
                        diags.push(Diagnostic::warning(
                            lit.span,
                            "`until` date is before the task's start — bar clamped",
                        ));
                        start
                    } else {
                        e
                    }
                }
                None => calendar.span_end(start, 1),
            }
        };
        start_days[i] = start;
        // A task may span at most MAX_TASK_SPAN days and must stay in range.
        end_days[i] = end.clamp(start, start.saturating_add(MAX_TASK_SPAN).min(MAX_DAY));
    }

    // ---- assemble ------------------------------------------------------
    let mut tasks: IndexMap<TaskId, Task> = IndexMap::with_capacity(stmts.len());
    let mut section_idx: Option<usize> = None;
    let mut row_of_task: HashMap<&TaskId, usize> = HashMap::new();
    for (row, r) in rows.iter().enumerate() {
        match r {
            Row::Section(_) => {}
            Row::Task(id) => {
                row_of_task.insert(id, row);
            }
        }
    }
    // Recompute section membership by walking rows in order.
    let mut task_sections: HashMap<TaskId, usize> = HashMap::new();
    for r in &rows {
        match r {
            Row::Section(s) => section_idx = Some(*s),
            Row::Task(id) => {
                if let Some(s) = section_idx {
                    task_sections.insert(id.clone(), s);
                }
            }
        }
    }

    for (i, t) in stmts.iter().enumerate() {
        let start_src = match &t.start {
            Some(StartField::Date(lit)) => StartSrc::Date { span: lit.span },
            Some(StartField::After(a)) => StartSrc::After {
                span: a.span,
                ids_end: a.ids_end,
                lag_span: a.lag.as_ref().map(|l| l.span),
            },
            None => StartSrc::Implicit,
        };
        let dur_src = match &t.dur {
            Some(DurField::Duration { span, .. }) => DurSrc::Duration { span: *span },
            Some(DurField::Until(lit)) => DurSrc::Until { span: lit.span },
            None => DurSrc::Implicit,
        };
        let deps = match &t.start {
            Some(StartField::After(a)) => {
                let lag = a.lag.as_ref().map_or(0, |l| l.sign * l.unit.days(l.n));
                a.ids
                    .iter()
                    .map(|(id, span)| Dep { on: id.clone(), lag_days: lag, span: *span })
                    .collect()
            }
            _ => Vec::new(),
        };
        let color = t.color.as_ref().and_then(|(text, span)| {
            parse_hex(text).map(|rgb| (rgb, *span))
        });
        tasks.insert(
            t.id.clone(),
            Task {
                id: t.id.clone(),
                label: t.label.clone(),
                milestone: t.milestone,
                start_day: start_days[i],
                end_day: end_days[i],
                deps,
                start_src,
                dur_src,
                progress: t.progress,
                color,
                tags: t.tags.iter().map(|(tag, _)| *tag).collect(),
                section: task_sections.get(&t.id).copied(),
                row: row_of_task[&t.id],
                label_span: t.label_span,
                id_span: t.id_span,
                stmt_span: t.stmt_span,
                insert_field_at: t.insert_field_at,
            },
        );
    }

    let (min_day, max_day) = if tasks.is_empty() {
        (today, today + 13)
    } else {
        let min = tasks.values().map(|t| t.start_day).min().unwrap();
        let max = tasks.values().map(|t| t.end_day).max().unwrap();
        // Cap the total chart range so per-frame layout stays bounded no
        // matter what dates a file contains.
        (min, max.min(min.saturating_add(MAX_CHART_SPAN)))
    };

    GanttModel {
        title,
        axis,
        today,
        calendar,
        tasks,
        sections,
        rows,
        min_day,
        max_day,
        diagnostics: diags,
    }
}

fn parse_hex(text: &str) -> Option<[u8; 3]> {
    let hex = &text[1..];
    match hex.len() {
        6 => {
            let v = u32::from_str_radix(hex, 16).ok()?;
            Some([(v >> 16) as u8, (v >> 8) as u8, v as u8])
        }
        3 => {
            let v = u32::from_str_radix(hex, 16).ok()?;
            let (r, g, b) = ((v >> 8) & 0xf, (v >> 4) & 0xf, v & 0xf);
            Some([(r * 17) as u8, (g * 17) as u8, (b * 17) as u8])
        }
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::parser::parse;

    /// 2026-08-10, a Monday.
    const TODAY: i64 = 20675;

    fn model(src: &str) -> GanttModel {
        resolve(&parse(src), TODAY)
    }

    fn day(s: &str) -> i64 {
        parse_date(s).unwrap()
    }

    #[test]
    fn jiff_day_roundtrip() {
        assert_eq!(day_of(Date::constant(1970, 1, 1)), 0);
        assert_eq!(day("2026-08-10"), TODAY);
        assert_eq!(format_day(TODAY), "2026-08-10");
        // Cross-check integer weekday math against jiff.
        for offset in 0..14 {
            let d = TODAY + offset;
            let jiff_wd = date_of(d).weekday().to_monday_zero_offset() as i64;
            assert_eq!(crate::model::weekday(d), jiff_wd);
        }
    }

    #[test]
    fn explicit_dates_and_duration() {
        let m = model("task \"A\" : a, 2026-08-10, 5d\n");
        let t = &m.tasks["a"];
        assert_eq!(t.start_day, day("2026-08-10"));
        assert_eq!(t.end_day, day("2026-08-14"));
    }

    #[test]
    fn after_chain_with_lag() {
        let m = model(
            "task \"A\" : a, 2026-08-10, 5d\ntask \"B\" : b, after a + 2d, 3d\n",
        );
        let b = &m.tasks["b"];
        // A ends Fri 08-14; +1+2 calendar days (no excludes) = Mon 08-17.
        assert_eq!(b.start_day, day("2026-08-17"));
        assert_eq!(b.end_day, day("2026-08-19"));
    }

    #[test]
    fn after_respects_weekends() {
        let m = model(
            "exclude weekends\ntask \"A\" : a, 2026-08-10, 5d\ntask \"B\" : b, after a, 2d\n",
        );
        let b = &m.tasks["b"];
        // A ends Fri 08-14; next workday is Mon 08-17; 2 workdays end Tue.
        assert_eq!(b.start_day, day("2026-08-17"));
        assert_eq!(b.end_day, day("2026-08-18"));
    }

    #[test]
    fn multiple_deps_take_latest() {
        let m = model(
            "task \"A\" : a, 2026-08-10, 2d\ntask \"B\" : b, 2026-08-10, 5d\ntask \"C\" : c, after a b, 1d\n",
        );
        assert_eq!(m.tasks["c"].start_day, day("2026-08-15"));
    }

    #[test]
    fn forward_reference_resolves() {
        let m = model(
            "task \"B\" : b, after a, 3d\ntask \"A\" : a, 2026-08-10, 5d\n",
        );
        assert_eq!(m.tasks["b"].start_day, day("2026-08-15"));
        assert!(!m.has_errors());
    }

    #[test]
    fn implicit_start_follows_previous() {
        let m = model("task \"A\" : a, 2026-08-10, 5d\ntask \"B\" : b, 3d\n");
        assert_eq!(m.tasks["b"].start_day, day("2026-08-15"));
    }

    #[test]
    fn first_task_defaults_to_today() {
        let m = model("task \"A\" : a, 5d\n");
        assert_eq!(m.tasks["a"].start_day, TODAY);
    }

    #[test]
    fn until_sets_end() {
        let m = model("task \"A\" : a, 2026-08-10, until 2026-08-20\n");
        assert_eq!(m.tasks["a"].end_day, day("2026-08-20"));
    }

    #[test]
    fn milestone_is_a_point() {
        let m = model("task \"A\" : a, 2026-08-10, 5d\nmilestone \"M\" : m, after a\n");
        let ms = &m.tasks["m"];
        assert!(ms.milestone);
        assert_eq!(ms.start_day, ms.end_day);
        assert_eq!(ms.start_day, day("2026-08-15"));
    }

    #[test]
    fn cycle_is_diagnosed_not_fatal() {
        let m = model("task \"A\" : a, after b, 2d\ntask \"B\" : b, after a, 2d\n");
        assert!(m.has_errors());
        assert!(m.diagnostics.iter().any(|d| d.message.contains("cycle")));
        assert_eq!(m.tasks.len(), 2); // still drawn
    }

    #[test]
    fn unknown_dep_is_diagnosed() {
        let m = model("task \"A\" : a, after ghost, 2d\n");
        assert!(m.diagnostics.iter().any(|d| d.message.contains("ghost")));
        assert_eq!(m.tasks["a"].start_day, TODAY);
    }

    #[test]
    fn duplicate_id_is_diagnosed() {
        let m = model("task \"A\" : a, 2d\ntask \"B\" : a, 3d\n");
        assert!(m.diagnostics.iter().any(|d| d.message.contains("duplicate")));
        assert_eq!(m.tasks.len(), 1);
    }

    #[test]
    fn holiday_range_excluded() {
        let m = model(
            "exclude weekends\nexclude 2026-08-12..2026-08-13\ntask \"A\" : a, 2026-08-10, 4d\n",
        );
        // Mon, Tue, (Wed+Thu off), Fri, Mon.
        assert_eq!(m.tasks["a"].end_day, day("2026-08-17"));
    }

    #[test]
    fn sections_and_rows() {
        let m = model(
            "section One\ntask \"A\" : a, 2d\nsection Two\ntask \"B\" : b, 2d\n",
        );
        assert_eq!(m.rows.len(), 4);
        assert_eq!(m.sections.len(), 2);
        assert_eq!(m.tasks["a"].section, Some(0));
        assert_eq!(m.tasks["b"].section, Some(1));
        assert_eq!(m.tasks["b"].row, 3);
    }

    #[test]
    fn task_at_byte_finds_task() {
        let src = "task \"A\" : a, 2d\ntask \"B\" : b, 3d\n";
        let m = model(src);
        let second = src.find("\"B\"").unwrap();
        assert_eq!(m.task_at_byte(second).unwrap(), "b");
    }
}
