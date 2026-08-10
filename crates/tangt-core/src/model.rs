//! Resolved chart model: what the canvas, exporter, and rewrite engine
//! consume. Dates are day numbers (days since 1970-01-01) so everything
//! downstream is integer arithmetic; jiff appears only in `resolve`.

use crate::ast::{AxisKind, Diagnostic, Span, Tag};
use indexmap::IndexMap;
use smol_str::SmolStr;
use std::collections::BTreeSet;

pub type TaskId = SmolStr;

/// Weekday of a day number: 0 = Monday … 6 = Sunday.
/// Day 0 = 1970-01-01 was a Thursday (index 3).
pub fn weekday(day: i64) -> i64 {
    (day + 3).rem_euclid(7)
}

pub fn is_weekend(day: i64) -> bool {
    weekday(day) >= 5
}

/// Working-day calendar: optional weekend exclusion plus holiday day-set.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Calendar {
    pub exclude_weekends: bool,
    pub holidays: BTreeSet<i64>,
}

impl Calendar {
    pub fn is_workday(&self, day: i64) -> bool {
        if self.exclude_weekends && is_weekend(day) {
            return false;
        }
        !self.holidays.contains(&day)
    }

    /// Whether any day is ever excluded (false → pure calendar arithmetic).
    pub fn has_exclusions(&self) -> bool {
        self.exclude_weekends || !self.holidays.is_empty()
    }

    /// First workday at or after `day`.
    pub fn snap_forward(&self, day: i64) -> i64 {
        let mut d = day;
        while !self.is_workday(d) {
            d += 1;
        }
        d
    }

    /// Move `n` workdays from `day` (`day` itself is not counted; `n` may be
    /// negative). With no exclusions this is plain `day + n`.
    pub fn add_working_days(&self, day: i64, n: i64) -> i64 {
        if !self.has_exclusions() {
            return day + n;
        }
        let step = if n >= 0 { 1 } else { -1 };
        let mut remaining = n.abs();
        let mut d = day;
        while remaining > 0 {
            d += step;
            if self.is_workday(d) {
                remaining -= 1;
            }
        }
        d
    }

    /// Inclusive end day of a bar starting at `start` and spanning
    /// `n` workdays (n >= 1). Counting starts at the first workday >= start.
    pub fn span_end(&self, start: i64, n: i64) -> i64 {
        let n = n.max(1);
        if !self.has_exclusions() {
            return start + n - 1;
        }
        let first = self.snap_forward(start);
        self.add_working_days(first, n - 1)
    }

    /// Number of workdays in `[start, end]` (inclusive); minimum 1.
    pub fn working_days_between(&self, start: i64, end: i64) -> i64 {
        if !self.has_exclusions() {
            return (end - start + 1).max(1);
        }
        let mut count = 0;
        for d in start..=end {
            if self.is_workday(d) {
                count += 1;
            }
        }
        count.max(1)
    }
}

/// Where a task's start came from in the source — determines what a drag
/// rewrites.
#[derive(Debug, Clone, PartialEq)]
pub enum StartSrc {
    /// Explicit `2026-08-10`; drag splices the date.
    Date { span: Span },
    /// `after a b [+ 2d]`; drag splices/inserts the lag, keeping the deps.
    /// `ids_end` is the byte just past the last id (lag/new ids insert here).
    After { span: Span, ids_end: usize, lag_span: Option<Span> },
    /// No start written; drag inserts a date at `insert_field_at`.
    Implicit,
}

#[derive(Debug, Clone, PartialEq)]
pub enum DurSrc {
    Duration { span: Span },
    Until { span: Span },
    Implicit,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Dep {
    pub on: TaskId,
    /// Lag in working days (negative = lead).
    pub lag_days: i64,
    /// Span of this id inside the `after` clause (for surgical removal).
    pub span: Span,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Task {
    pub id: TaskId,
    pub label: String,
    pub milestone: bool,
    /// Resolved schedule, inclusive day numbers. Milestones: start == end.
    pub start_day: i64,
    pub end_day: i64,
    pub deps: Vec<Dep>,
    pub start_src: StartSrc,
    pub dur_src: DurSrc,
    pub progress: Option<(u8, Span)>,
    pub color: Option<([u8; 3], Span)>,
    pub tags: Vec<Tag>,
    /// Index into `GanttModel::sections`, if any section precedes this task.
    pub section: Option<usize>,
    /// Index into `GanttModel::rows`.
    pub row: usize,
    pub label_span: Span,
    pub id_span: Span,
    pub stmt_span: Span,
    pub insert_field_at: usize,
}

impl Task {
    pub fn has_tag(&self, tag: Tag) -> bool {
        self.tags.contains(&tag)
    }
    /// Duration in working days under `cal` (what a resize splices).
    pub fn duration_days(&self, cal: &Calendar) -> i64 {
        if self.milestone {
            return 0;
        }
        cal.working_days_between(self.start_day, self.end_day)
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct Section {
    pub label: String,
    pub label_span: Span,
    pub stmt_span: Span,
    /// Index into `GanttModel::rows` of the section header row.
    pub row: usize,
}

/// One row of the chart, in file order.
#[derive(Debug, Clone, PartialEq)]
pub enum Row {
    Section(usize),
    Task(TaskId),
}

#[derive(Debug, Clone, Default)]
pub struct GanttModel {
    pub title: Option<String>,
    pub axis: AxisKind,
    /// Day number of the today-marker.
    pub today: i64,
    pub calendar: Calendar,
    pub tasks: IndexMap<TaskId, Task>,
    pub sections: Vec<Section>,
    pub rows: Vec<Row>,
    /// Chart time bounds (inclusive), padded by resolve.
    pub min_day: i64,
    pub max_day: i64,
    pub diagnostics: Vec<Diagnostic>,
}

impl GanttModel {
    /// Task whose statement contains this byte offset — used to sync the text
    /// cursor with the canvas selection.
    pub fn task_at_byte(&self, offset: usize) -> Option<&TaskId> {
        self.tasks
            .values()
            .find(|t| t.stmt_span.contains(offset))
            .map(|t| &t.id)
    }

    pub fn has_errors(&self) -> bool {
        use crate::ast::Severity;
        self.diagnostics.iter().any(|d| d.severity == Severity::Error)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn weekday_matches_known_dates() {
        // 1970-01-01 = Thursday
        assert_eq!(weekday(0), 3);
        // 2026-08-10 = Monday = day 20675
        assert_eq!(weekday(20675), 0);
        assert!(is_weekend(20675 + 5)); // Saturday
        assert!(is_weekend(20675 + 6)); // Sunday
    }

    #[test]
    fn working_day_arithmetic() {
        let cal = Calendar { exclude_weekends: true, holidays: BTreeSet::new() };
        let monday = 20675;
        // 5 workdays starting Monday end on Friday.
        assert_eq!(cal.span_end(monday, 5), monday + 4);
        // 6 workdays starting Monday end next Monday.
        assert_eq!(cal.span_end(monday, 6), monday + 7);
        // Moving one workday from Friday lands on Monday.
        assert_eq!(cal.add_working_days(monday + 4, 1), monday + 7);
        // Backward across a weekend.
        assert_eq!(cal.add_working_days(monday + 7, -1), monday + 4);
        assert_eq!(cal.working_days_between(monday, monday + 7), 6);
    }

    #[test]
    fn holidays_are_skipped() {
        let mut holidays = BTreeSet::new();
        let monday = 20675;
        holidays.insert(monday + 1); // Tuesday off
        let cal = Calendar { exclude_weekends: true, holidays };
        assert_eq!(cal.span_end(monday, 2), monday + 2); // Mon + Wed
    }

    #[test]
    fn no_exclusions_is_calendar_math() {
        let cal = Calendar::default();
        assert_eq!(cal.span_end(100, 5), 104);
        assert_eq!(cal.add_working_days(100, -3), 97);
        assert_eq!(cal.working_days_between(100, 104), 5);
    }
}
