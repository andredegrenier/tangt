//! Chart layout: scheduling already fixed every task's days, so layout is a
//! cheap pure function — day→x via the time scale, row→y via constants, plus
//! tick generation and dependency-arrow routing. Recomputed per frame at the
//! camera's zoom; the exporter runs it at zoom 1.

use crate::ast::AxisKind;
use crate::geometry::{
    Rect, TextMeasurer, BAR_INSET, BASE_FONT_SIZE, GUTTER_PAD, MAX_GUTTER_W, MIN_GUTTER_W,
    PX_PER_DAY, ROW_H,
};
use crate::model::{weekday, GanttModel, TaskId};
use crate::resolve::{day_of_month, month_label, month_start};
use crate::timescale::TimeScale;
use std::collections::HashMap;

/// Days of padding on each side of the data range.
const PAD_DAYS: i64 = 2;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TickKind {
    Day,
    Week,
    Month,
}

#[derive(Debug, Clone)]
pub struct Tick {
    pub day: i64,
    pub kind: TickKind,
    /// Label drawn in the header's lower band (day-of-month for day/week
    /// ticks).
    pub label: Option<String>,
}

#[derive(Debug, Clone)]
pub struct Arrow {
    pub from: TaskId,
    pub to: TaskId,
    /// Chart-space polyline ending at the target's left entry point.
    pub points: Vec<[f32; 2]>,
}

#[derive(Debug, Clone)]
pub struct GanttLayout {
    pub scale: TimeScale,
    pub gutter_w: f32,
    /// Bar (or milestone diamond bounding box) rects in chart space.
    pub bar_rects: HashMap<TaskId, Rect>,
    pub arrows: Vec<Arrow>,
    pub ticks: Vec<Tick>,
    /// Month header bands: (first shown day of month, label).
    pub months: Vec<(i64, String)>,
    pub row_count: usize,
    pub chart_h: f32,
}

impl GanttLayout {
    pub fn row_top(&self, row: usize) -> f32 {
        row as f32 * ROW_H
    }
    pub fn row_at_y(&self, y: f32) -> Option<usize> {
        if y < 0.0 {
            return None;
        }
        let row = (y / ROW_H) as usize;
        (row < self.row_count).then_some(row)
    }
}

pub fn compute(model: &GanttModel, measurer: &dyn TextMeasurer, px_per_day: f32) -> GanttLayout {
    let scale = TimeScale {
        min_day: model.min_day - PAD_DAYS,
        max_day: model.max_day + PAD_DAYS + 1,
        px_per_day,
    };

    // ---- gutter width from labels --------------------------------------
    let mut gutter: f32 = MIN_GUTTER_W;
    for t in model.tasks.values() {
        let w = measurer.measure(&t.label, BASE_FONT_SIZE)[0] + 2.0 * GUTTER_PAD;
        gutter = gutter.max(w);
    }
    for s in &model.sections {
        let w = measurer.measure(&s.label, BASE_FONT_SIZE)[0] + 2.0 * GUTTER_PAD;
        gutter = gutter.max(w);
    }
    let gutter_w = gutter.min(MAX_GUTTER_W);

    // ---- bar rects ------------------------------------------------------
    let mut bar_rects = HashMap::with_capacity(model.tasks.len());
    for t in model.tasks.values() {
        let y = t.row as f32 * ROW_H;
        let rect = if t.milestone {
            let side = ROW_H - 2.0 * BAR_INSET;
            Rect::from_center_size(
                [scale.day_center_x(t.start_day), y + ROW_H / 2.0],
                [side, side],
            )
        } else {
            Rect::new(
                [scale.day_to_x(t.start_day), y + BAR_INSET],
                [scale.day_to_x(t.end_day + 1), y + ROW_H - BAR_INSET],
            )
        };
        bar_rects.insert(t.id.clone(), rect);
    }

    // ---- dependency arrows ----------------------------------------------
    let mut arrows = Vec::new();
    for t in model.tasks.values() {
        for dep in &t.deps {
            let (Some(from_rect), Some(to_rect)) =
                (bar_rects.get(&dep.on), bar_rects.get(&t.id))
            else {
                continue;
            };
            let start = [from_rect.max[0], from_rect.center()[1]];
            let entry = [to_rect.min[0], to_rect.center()[1]];
            let stub = 8.0;
            let below = entry[1] > start[1];
            let points = if entry[0] >= start[0] + 2.0 * stub {
                // Straight elbow: right, down/up, right.
                vec![
                    start,
                    [start[0] + stub, start[1]],
                    [start[0] + stub, entry[1]],
                    entry,
                ]
            } else if start[0] + stub >= to_rect.min[0] && start[0] + stub <= to_rect.max[0] {
                // Adjacent finish-to-start: drop into the successor's edge.
                let edge_y = if below { to_rect.min[1] } else { to_rect.max[1] };
                vec![start, [start[0] + stub, start[1]], [start[0] + stub, edge_y]]
            } else {
                // Down-around: right, into the row gap, back left, down, in.
                let gap_y = if below {
                    to_rect.min[1] - BAR_INSET
                } else {
                    to_rect.max[1] + BAR_INSET
                };
                vec![
                    start,
                    [start[0] + stub, start[1]],
                    [start[0] + stub, gap_y],
                    [entry[0] - stub, gap_y],
                    [entry[0] - stub, entry[1]],
                    entry,
                ]
            };
            arrows.push(Arrow { from: dep.on.clone(), to: t.id.clone(), points });
        }
    }

    // ---- ticks ----------------------------------------------------------
    let kind = match model.axis {
        AxisKind::Day => TickKind::Day,
        AxisKind::Week => TickKind::Week,
        AxisKind::Month => TickKind::Month,
        AxisKind::Auto => {
            if px_per_day >= 16.0 {
                TickKind::Day
            } else if px_per_day >= 3.5 {
                TickKind::Week
            } else {
                TickKind::Month
            }
        }
    };
    let mut ticks = Vec::new();
    for day in scale.min_day..=scale.max_day {
        let is_week_start = weekday(day) == 0;
        let is_month_start = day_of_month(day) == 1;
        match kind {
            TickKind::Day => ticks.push(Tick {
                day,
                kind: if is_week_start { TickKind::Week } else { TickKind::Day },
                label: Some(day_of_month(day).to_string()),
            }),
            TickKind::Week if is_week_start => ticks.push(Tick {
                day,
                kind: TickKind::Week,
                label: Some(day_of_month(day).to_string()),
            }),
            TickKind::Month if is_month_start => {
                ticks.push(Tick { day, kind: TickKind::Month, label: None })
            }
            _ => {}
        }
    }

    // ---- month bands -----------------------------------------------------
    let mut months = Vec::new();
    let mut m = month_start(scale.min_day).max(scale.min_day);
    // First (possibly partial) month starts at the range start.
    months.push((scale.min_day, month_label(scale.min_day)));
    loop {
        m = month_start(m) + 45; // hop into the next month
        let start = month_start(m);
        if start > scale.max_day {
            break;
        }
        months.push((start, month_label(start)));
        m = start;
    }

    let row_count = model.rows.len();
    GanttLayout {
        scale,
        gutter_w,
        bar_rects,
        arrows,
        ticks,
        months,
        row_count,
        chart_h: row_count as f32 * ROW_H,
    }
}

/// Layout at the default zoom (used by exporters and tests).
pub fn compute_default(model: &GanttModel, measurer: &dyn TextMeasurer) -> GanttLayout {
    compute(model, measurer, PX_PER_DAY)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::geometry::MonoMeasurer;
    use crate::parser::parse;
    use crate::resolve::{parse_date, resolve};

    const TODAY: i64 = 20675;

    fn layout(src: &str) -> (GanttModel, GanttLayout) {
        let model = resolve(&parse(src), TODAY);
        let l = compute_default(&model, &MonoMeasurer);
        (model, l)
    }

    #[test]
    fn bars_map_days_to_x() {
        let (model, l) = layout("task \"A\" : a, 2026-08-10, 5d\n");
        let t = &model.tasks["a"];
        let r = l.bar_rects["a"];
        assert_eq!(r.min[0], l.scale.day_to_x(t.start_day));
        assert_eq!(r.max[0], l.scale.day_to_x(t.end_day + 1));
        assert_eq!(r.width(), 5.0 * PX_PER_DAY);
    }

    #[test]
    fn rows_stack_with_sections() {
        let (model, l) = layout("section S\ntask \"A\" : a, 2d\ntask \"B\" : b, 2d\n");
        assert_eq!(l.row_count, 3);
        let a = &model.tasks["a"];
        assert_eq!(l.bar_rects["a"].min[1], a.row as f32 * ROW_H + BAR_INSET);
        assert_eq!(l.row_at_y(l.bar_rects["b"].center()[1]), Some(2));
    }

    #[test]
    fn forward_arrow_with_gap_is_elbow() {
        let (_, l) =
            layout("task \"A\" : a, 2026-08-10, 5d\ntask \"B\" : b, after a + 3d, 3d\n");
        assert_eq!(l.arrows.len(), 1);
        let arrow = &l.arrows[0];
        assert_eq!(arrow.points.len(), 4);
        // Ends at B's left edge.
        assert_eq!(arrow.points.last().unwrap()[0], l.bar_rects["b"].min[0]);
    }

    #[test]
    fn adjacent_arrow_drops_into_top_edge() {
        let (_, l) = layout("task \"A\" : a, 2026-08-10, 5d\ntask \"B\" : b, after a, 3d\n");
        let arrow = &l.arrows[0];
        assert_eq!(arrow.points.len(), 3);
        assert_eq!(arrow.points.last().unwrap()[1], l.bar_rects["b"].min[1]);
    }

    #[test]
    fn backward_arrow_routes_around() {
        let (_, l) = layout(
            "task \"A\" : a, 2026-08-10, 5d\ntask \"B\" : b, after a - 6d, 3d\n",
        );
        assert_eq!(l.arrows[0].points.len(), 6);
    }

    #[test]
    fn month_bands_cover_range() {
        let (_, l) = layout("task \"A\" : a, 2026-08-10, until 2026-10-15\n");
        let labels: Vec<&str> = l.months.iter().map(|(_, s)| s.as_str()).collect();
        assert_eq!(labels, vec!["Aug 2026", "Sep 2026", "Oct 2026"]);
        assert_eq!(l.months[1].0, parse_date("2026-09-01").unwrap());
    }

    #[test]
    fn auto_ticks_by_zoom() {
        let src = "task \"A\" : a, 2026-08-10, 5d\n";
        let model = resolve(&parse(src), TODAY);
        let day_l = compute(&model, &MonoMeasurer, 26.0);
        assert!(day_l.ticks.iter().all(|t| t.label.is_some()));
        assert_eq!(
            day_l.ticks.len() as i64,
            day_l.scale.max_day - day_l.scale.min_day + 1
        );
        let week_l = compute(&model, &MonoMeasurer, 5.0);
        assert!(week_l.ticks.iter().all(|t| t.kind == TickKind::Week));
    }
}
