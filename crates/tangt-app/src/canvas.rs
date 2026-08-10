//! The Gantt canvas: pinned timeline header and task gutter around a
//! pan/zoomable chart region, bar painting, hit testing, and every drag
//! gesture (move, resize, create-dependency, reorder). Emits `CanvasEvent`s;
//! the app turns them into rewrite ops.

use crate::camera::GanttCamera;
use crate::theme::{to_color32, Theme};
use eframe::egui::{
    self, Align2, Color32, CursorIcon, FontId, Pos2, Rect, Sense, Shape as EShape, Stroke,
    StrokeKind, Ui, Vec2,
};
use std::collections::HashMap;
use tangt_core::ast::Tag;
use tangt_core::geometry::{BAR_INSET, BASE_FONT_SIZE, GUTTER_PAD, HEADER_H, ROW_H};
use tangt_core::layout::{GanttLayout, TickKind};
use tangt_core::model::{GanttModel, Row, TaskId};

const EDGE_GRIP: f32 = 7.0;
const DEP_DOT_R: f32 = 5.0;
const DEP_DOT_OFFSET: f32 = 11.0;

pub enum CanvasEvent {
    Select(Option<TaskId>),
    MoveBar { id: TaskId, new_start: i64, to_date: bool },
    ResizeStart { id: TaskId, day: i64 },
    ResizeEnd { id: TaskId, day: i64 },
    CreateDep { from: TaskId, to: TaskId },
    RemoveDep { task: TaskId, on: TaskId },
    Rename { id: TaskId, label: String },
    SetProgress { id: TaskId, pct: u8 },
    AddTask { anchor: Option<TaskId>, day: i64, milestone: bool },
    AddSection { anchor: Option<TaskId> },
    DeleteTask { id: TaskId },
    DeleteSection { idx: usize },
    Reorder { id: TaskId, before_row: Option<usize> },
    JumpToText(usize),
}

#[derive(Clone)]
pub enum DragKind {
    Move { grab_offset: i64, alt: bool },
    ResizeL,
    ResizeR,
    Dep,
    Reorder,
}

pub struct DragState {
    pub id: TaskId,
    pub kind: DragKind,
    pub pointer: Pos2,
    pub moved: bool,
}

#[derive(Clone)]
pub enum CtxTarget {
    Task(TaskId),
    Section(usize),
    Empty { day: i64, anchor: Option<TaskId> },
}

#[derive(Default)]
pub struct CanvasState {
    pub drag: Option<DragState>,
    /// Live (start, end) overrides during a drag.
    pub preview: HashMap<TaskId, (i64, i64)>,
    pub rename: Option<(TaskId, String)>,
    pub rename_focused: bool,
    pub ctx_target: Option<CtxTarget>,
    /// Live feedback line for the status bar ("Aug 17 → Aug 24 · 5d").
    pub hint: Option<String>,
}

struct Regions {
    corner: Rect,
    header: Rect,
    gutter: Rect,
    chart: Rect,
}

#[derive(Clone, Copy, PartialEq)]
enum Zone {
    Body,
    LeftEdge,
    RightEdge,
    DepDot,
}

pub fn show(
    ui: &mut Ui,
    model: &GanttModel,
    layout: &GanttLayout,
    camera: &mut GanttCamera,
    state: &mut CanvasState,
    selection: Option<&TaskId>,
    theme: &Theme,
) -> Vec<CanvasEvent> {
    let mut events = Vec::new();
    let (full, response) =
        ui.allocate_exact_size(ui.available_size(), Sense::click_and_drag());
    let gutter_w = layout.gutter_w.min(full.width() * 0.4);

    let regions = Regions {
        corner: Rect::from_min_max(full.min, full.min + Vec2::new(gutter_w, HEADER_H)),
        header: Rect::from_min_max(
            full.min + Vec2::new(gutter_w, 0.0),
            Pos2::new(full.max.x, full.min.y + HEADER_H),
        ),
        gutter: Rect::from_min_max(
            full.min + Vec2::new(0.0, HEADER_H),
            Pos2::new(full.min.x + gutter_w, full.max.y),
        ),
        chart: Rect::from_min_max(full.min + Vec2::new(gutter_w, HEADER_H), full.max),
    };

    // ---- input: zoom / pan / scroll ------------------------------------
    if let Some(hover) = response.hover_pos() {
        let in_time_area = regions.chart.contains(hover) || regions.header.contains(hover);
        let zoom_delta = ui.input(|i| i.zoom_delta());
        if zoom_delta != 1.0 && in_time_area {
            camera.zoom_at(zoom_delta, hover.x, regions.chart.left());
        }
        let scroll = ui.input(|i| i.smooth_scroll_delta);
        if zoom_delta == 1.0 && scroll != Vec2::ZERO && !regions.corner.contains(hover) {
            if scroll.x != 0.0 && in_time_area {
                camera.pan_days(scroll.x);
            }
            if scroll.y != 0.0 {
                camera.scroll(scroll.y, layout.chart_h, regions.chart.height());
            }
        }
    }

    // ---- screen mapping --------------------------------------------------
    // Snapshot for the read-only mapping closures; `camera` itself stays
    // mutable for the background-drag pan below (GanttCamera is Copy).
    let cam = *camera;
    let origin_x = cam.day_to_screen_x(layout.scale.min_day as f64, regions.chart.left());
    let top_y = regions.chart.top() - cam.y_scroll;
    let to_screen = move |p: [f32; 2]| Pos2::new(origin_x + p[0], top_y + p[1]);
    let row_top = |row: usize| top_y + row as f32 * ROW_H;
    let pointer_day = move |pos: Pos2| cam.day_at(pos.x, regions.chart.left());

    // Screen-space bar rects with drag preview applied.
    let mut screen_bars: HashMap<TaskId, Rect> = HashMap::with_capacity(model.tasks.len());
    for t in model.tasks.values() {
        let (start, end) = state
            .preview
            .get(&t.id)
            .copied()
            .unwrap_or((t.start_day, t.end_day));
        let y = row_top(t.row);
        let rect = if t.milestone {
            let side = ROW_H - 2.0 * BAR_INSET;
            let cx = camera.day_to_screen_x(start as f64 + 0.5, regions.chart.left());
            Rect::from_center_size(Pos2::new(cx, y + ROW_H / 2.0), Vec2::splat(side))
        } else {
            Rect::from_min_max(
                Pos2::new(
                    camera.day_to_screen_x(start as f64, regions.chart.left()),
                    y + BAR_INSET,
                ),
                Pos2::new(
                    camera.day_to_screen_x(end as f64 + 1.0, regions.chart.left()),
                    y + ROW_H - BAR_INSET,
                ),
            )
        };
        screen_bars.insert(t.id.clone(), rect);
    }

    // ---- hit testing -----------------------------------------------------
    let hover = response.hover_pos();
    let hovered_bar: Option<(TaskId, Zone)> = hover.and_then(|p| {
        if !regions.chart.contains(p) {
            return None;
        }
        model.tasks.values().find_map(|t| {
            let r = screen_bars.get(&t.id)?;
            // Reach extends past the right edge so the dep dot stays live
            // while the pointer travels from the bar to it.
            let reach = Rect::from_min_max(
                r.min - Vec2::splat(2.0),
                Pos2::new(r.right() + DEP_DOT_OFFSET + DEP_DOT_R + 3.0, r.bottom() + 2.0),
            );
            if !reach.contains(p) {
                return None;
            }
            if !t.milestone {
                let dot = Pos2::new(r.right() + DEP_DOT_OFFSET, r.center().y);
                if p.distance(dot) <= DEP_DOT_R + 3.0 {
                    return Some((t.id.clone(), Zone::DepDot));
                }
            }
            if !r.expand(2.0).contains(p) {
                return None;
            }
            if !t.milestone && (p.x - r.left()).abs() <= EDGE_GRIP {
                Some((t.id.clone(), Zone::LeftEdge))
            } else if !t.milestone && (r.right() - p.x).abs() <= EDGE_GRIP {
                Some((t.id.clone(), Zone::RightEdge))
            } else {
                Some((t.id.clone(), Zone::Body))
            }
        })
    });
    let hovered_gutter_row: Option<usize> = hover.and_then(|p| {
        regions
            .gutter
            .contains(p)
            .then(|| ((p.y - top_y) / ROW_H) as usize)
            .filter(|r| p.y >= top_y && *r < layout.row_count)
    });

    match (&hovered_bar, &state.drag) {
        (_, Some(d)) => {
            let icon = match d.kind {
                DragKind::Move { .. } => CursorIcon::Grabbing,
                DragKind::ResizeL | DragKind::ResizeR => CursorIcon::ResizeHorizontal,
                DragKind::Dep => CursorIcon::Crosshair,
                DragKind::Reorder => CursorIcon::ResizeVertical,
            };
            ui.output_mut(|o| o.cursor_icon = icon);
        }
        (Some((_, Zone::LeftEdge | Zone::RightEdge)), None) => {
            ui.output_mut(|o| o.cursor_icon = CursorIcon::ResizeHorizontal)
        }
        (Some((_, Zone::Body)), None) => {
            ui.output_mut(|o| o.cursor_icon = CursorIcon::Grab)
        }
        (Some((_, Zone::DepDot)), None) => {
            ui.output_mut(|o| o.cursor_icon = CursorIcon::Crosshair)
        }
        _ => {}
    }

    // ---- drag ------------------------------------------------------------
    if response.drag_started() {
        let alt = ui.input(|i| i.modifiers.alt);
        if let (Some((id, zone)), Some(p)) = (hovered_bar.clone(), hover) {
            let t = &model.tasks[&id];
            let kind = match zone {
                Zone::DepDot => DragKind::Dep,
                Zone::LeftEdge => DragKind::ResizeL,
                Zone::RightEdge => DragKind::ResizeR,
                Zone::Body => DragKind::Move {
                    grab_offset: pointer_day(p) - t.start_day,
                    alt,
                },
            };
            state.drag = Some(DragState { id, kind, pointer: p, moved: false });
        } else if let (Some(row), Some(p)) = (hovered_gutter_row, hover) {
            if let Some(Row::Task(id)) = model.rows.get(row) {
                state.drag = Some(DragState {
                    id: id.clone(),
                    kind: DragKind::Reorder,
                    pointer: p,
                    moved: false,
                });
            }
        }
    }
    if response.dragged() {
        if let Some(p) = response.interact_pointer_pos() {
            if let Some(drag) = &mut state.drag {
                drag.pointer = p;
                if response.drag_delta() != Vec2::ZERO {
                    drag.moved = true;
                }
                // Sample ⌥ live so pressing it mid-drag works.
                if let DragKind::Move { alt, .. } = &mut drag.kind {
                    *alt = ui.input(|i| i.modifiers.alt);
                }
                let t = model.tasks.get(&drag.id).cloned();
                if let Some(t) = t {
                    let fmt = tangt_core::resolve::format_day;
                    match drag.kind {
                        DragKind::Move { grab_offset, alt } => {
                            let dur = t.end_day - t.start_day;
                            let new_start = pointer_day(p) - grab_offset;
                            state.preview.insert(t.id.clone(), (new_start, new_start + dur));
                            let mode = if !t.deps.is_empty() {
                                if alt {
                                    " · detaching to a fixed date"
                                } else {
                                    " · adjusting lag (⌥ to detach)"
                                }
                            } else {
                                ""
                            };
                            state.hint = Some(format!(
                                "{} → {}{mode}",
                                fmt(new_start),
                                fmt(new_start + dur)
                            ));
                        }
                        DragKind::ResizeR => {
                            let end = pointer_day(p).max(t.start_day);
                            state.preview.insert(t.id.clone(), (t.start_day, end));
                            let n = model.calendar.working_days_between(t.start_day, end);
                            state.hint = Some(format!("until {} · {n}d", fmt(end)));
                        }
                        DragKind::ResizeL => {
                            let start = pointer_day(p).min(t.end_day);
                            state.preview.insert(t.id.clone(), (start, t.end_day));
                            let n = model.calendar.working_days_between(start, t.end_day);
                            state.hint = Some(format!("starts {} · {n}d", fmt(start)));
                        }
                        DragKind::Dep => {
                            state.hint =
                                Some("Drop on a bar to make it start after this task".into());
                        }
                        DragKind::Reorder => {}
                    }
                }
            } else if regions.chart.contains(p) {
                // Background drag pans time / scrolls rows.
                let d = response.drag_delta();
                camera.pan_days(d.x);
                camera.scroll(d.y, layout.chart_h, regions.chart.height());
            }
        }
    }
    if response.drag_stopped() {
        if let Some(drag) = state.drag.take() {
            let p = drag.pointer;
            if drag.moved {
                match drag.kind {
                    DragKind::Move { alt, .. } => {
                        if let Some(&(start, _)) = state.preview.get(&drag.id) {
                            let t = &model.tasks[&drag.id];
                            if start != t.start_day {
                                events.push(CanvasEvent::MoveBar {
                                    id: drag.id.clone(),
                                    new_start: start,
                                    to_date: alt,
                                });
                            }
                        }
                    }
                    DragKind::ResizeR => {
                        if let Some(&(_, end)) = state.preview.get(&drag.id) {
                            events.push(CanvasEvent::ResizeEnd { id: drag.id.clone(), day: end });
                        }
                    }
                    DragKind::ResizeL => {
                        if let Some(&(start, _)) = state.preview.get(&drag.id) {
                            events
                                .push(CanvasEvent::ResizeStart { id: drag.id.clone(), day: start });
                        }
                    }
                    DragKind::Dep => {
                        let target = model.tasks.values().find(|t| {
                            t.id != drag.id
                                && screen_bars
                                    .get(&t.id)
                                    .is_some_and(|r| r.expand(4.0).contains(p))
                        });
                        if let Some(t) = target {
                            events.push(CanvasEvent::CreateDep {
                                from: drag.id.clone(),
                                to: t.id.clone(),
                            });
                        }
                    }
                    DragKind::Reorder => {
                        let row = (((p.y - top_y) / ROW_H).round().max(0.0)) as usize;
                        let before = (row < layout.row_count).then_some(row);
                        events.push(CanvasEvent::Reorder { id: drag.id.clone(), before_row: before });
                    }
                }
            }
            state.preview.clear();
        }
    }

    // ---- clicks ----------------------------------------------------------
    if response.double_clicked() {
        if let Some((id, _)) = &hovered_bar {
            let label = model.tasks[id].label.clone();
            state.rename = Some((id.clone(), label));
        } else if let Some(row) = hovered_gutter_row {
            match model.rows.get(row) {
                Some(Row::Task(id)) => {
                    events.push(CanvasEvent::JumpToText(model.tasks[id].stmt_span.start));
                }
                Some(Row::Section(s)) => {
                    events.push(CanvasEvent::JumpToText(model.sections[*s].stmt_span.start));
                }
                None => {}
            }
        }
    } else if response.clicked() {
        let picked = hovered_bar.as_ref().map(|(id, _)| id.clone()).or_else(|| {
            hovered_gutter_row.and_then(|row| match model.rows.get(row) {
                Some(Row::Task(id)) => Some(id.clone()),
                _ => None,
            })
        });
        events.push(CanvasEvent::Select(picked));
    }

    // Capture the context-menu target at right-click time.
    if response.secondary_clicked() {
        state.ctx_target = if let Some((id, _)) = &hovered_bar {
            Some(CtxTarget::Task(id.clone()))
        } else if let Some(row) = hovered_gutter_row {
            match model.rows.get(row) {
                Some(Row::Task(id)) => Some(CtxTarget::Task(id.clone())),
                Some(Row::Section(s)) => Some(CtxTarget::Section(*s)),
                None => None,
            }
        } else if let Some(p) = hover {
            let day = pointer_day(p);
            // Anchor: the task on the row under (or last before) the pointer.
            let row = ((p.y - top_y) / ROW_H) as usize;
            let anchor = model.rows[..model.rows.len().min(row + 1)]
                .iter()
                .rev()
                .find_map(|r| match r {
                    Row::Task(id) => Some(id.clone()),
                    _ => None,
                });
            Some(CtxTarget::Empty { day, anchor })
        } else {
            None
        };
    }
    response.context_menu(|ui| {
        context_menu(ui, model, state, &mut events);
    });

    // ---- paint -----------------------------------------------------------
    let painter = ui.painter_at(full);
    painter.rect_filled(full, 0.0, theme.canvas_bg);

    // Chart region.
    {
        let painter = ui.painter_at(regions.chart);
        // Weekend / holiday shading.
        if model.calendar.has_exclusions() {
            let first = camera.day_at(regions.chart.left(), regions.chart.left());
            let last = camera.day_at(regions.chart.right(), regions.chart.left()) + 1;
            for day in first..=last {
                if !model.calendar.is_workday(day) {
                    let x0 = camera.day_to_screen_x(day as f64, regions.chart.left());
                    let x1 = camera.day_to_screen_x(day as f64 + 1.0, regions.chart.left());
                    painter.rect_filled(
                        Rect::from_min_max(
                            Pos2::new(x0, regions.chart.top()),
                            Pos2::new(x1, regions.chart.bottom()),
                        ),
                        0.0,
                        theme.weekend,
                    );
                }
            }
        }
        // Section bands.
        for (row, r) in model.rows.iter().enumerate() {
            if matches!(r, Row::Section(_)) {
                let y = row_top(row);
                painter.rect_filled(
                    Rect::from_min_max(
                        Pos2::new(regions.chart.left(), y),
                        Pos2::new(regions.chart.right(), y + ROW_H),
                    ),
                    0.0,
                    theme.section_band,
                );
            }
        }
        // Grid.
        for tick in &layout.ticks {
            let x = to_screen([layout.scale.day_to_x(tick.day), 0.0]).x;
            if x < regions.chart.left() - 1.0 || x > regions.chart.right() + 1.0 {
                continue;
            }
            let major = tick.kind != TickKind::Day;
            painter.line_segment(
                [
                    Pos2::new(x, regions.chart.top()),
                    Pos2::new(x, regions.chart.bottom()),
                ],
                Stroke::new(1.0, if major { theme.grid_major } else { theme.grid }),
            );
        }
        // Today marker.
        if model.today >= layout.scale.min_day && model.today <= layout.scale.max_day {
            let x = camera.day_to_screen_x(model.today as f64 + 0.5, regions.chart.left());
            let stroke = Stroke::new(1.5, theme.today);
            painter.extend(EShape::dashed_line(
                &[
                    Pos2::new(x, regions.chart.top()),
                    Pos2::new(x, regions.chart.bottom()),
                ],
                stroke,
                5.0,
                4.0,
            ));
        }
        // Dependency arrows, routed live from (preview-adjusted) screen rects.
        for arrow in &layout.arrows {
            let (Some(from), Some(to)) =
                (screen_bars.get(&arrow.from), screen_bars.get(&arrow.to))
            else {
                continue;
            };
            draw_arrow(&painter, *from, *to, theme.arrow);
        }
        // Rubber band during a dependency drag, highlighting the drop target.
        if let Some(drag) = &state.drag {
            if matches!(drag.kind, DragKind::Dep) {
                let target = model.tasks.values().find(|t| {
                    t.id != drag.id
                        && screen_bars
                            .get(&t.id)
                            .is_some_and(|r| r.expand(4.0).contains(drag.pointer))
                });
                if let Some(t) = &target {
                    if let Some(r) = screen_bars.get(&t.id) {
                        painter.rect_stroke(
                            r.expand(3.0),
                            5.0,
                            Stroke::new(2.0, theme.selection),
                            StrokeKind::Middle,
                        );
                    }
                }
                if let Some(r) = screen_bars.get(&drag.id) {
                    let from = Pos2::new(r.right() + DEP_DOT_OFFSET, r.center().y);
                    let stroke = if target.is_some() {
                        Stroke::new(2.0, theme.selection)
                    } else {
                        Stroke::new(1.5, theme.selection.gamma_multiply(0.5))
                    };
                    painter.line_segment([from, drag.pointer], stroke);
                }
            }
        }
        // Bars & milestones.
        let label_font = FontId::proportional(BASE_FONT_SIZE - 2.0);
        for t in model.tasks.values() {
            let Some(&rect) = screen_bars.get(&t.id) else { continue };
            if rect.max.y < regions.chart.top() - ROW_H
                || rect.min.y > regions.chart.bottom() + ROW_H
            {
                continue; // cull off-screen rows
            }
            if t.milestone {
                let c = rect.center();
                let (hw, hh) = (rect.width() / 2.0, rect.height() / 2.0);
                let pts = vec![
                    Pos2::new(c.x, c.y - hh),
                    Pos2::new(c.x + hw, c.y),
                    Pos2::new(c.x, c.y + hh),
                    Pos2::new(c.x - hw, c.y),
                ];
                let fill = t
                    .color
                    .map(|(c, _)| to_color32(tangt_core::geometry::Rgba::from_rgb3(c)))
                    .unwrap_or(theme.milestone);
                painter.add(EShape::convex_polygon(pts.clone(), fill, Stroke::NONE));
                if selection == Some(&t.id) {
                    painter.add(EShape::closed_line(pts, Stroke::new(2.0, theme.selection)));
                }
                painter.text(
                    Pos2::new(rect.right() + 6.0, c.y),
                    Align2::LEFT_CENTER,
                    &t.label,
                    label_font.clone(),
                    theme.text,
                );
            } else {
                let fill = if let Some((c, _)) = t.color {
                    to_color32(tangt_core::geometry::Rgba::from_rgb3(c))
                } else if t.has_tag(Tag::Done) {
                    theme.bar_done
                } else if t.has_tag(Tag::Crit) {
                    theme.crit
                } else {
                    theme.bar
                };
                painter.rect_filled(rect, 4.0, fill);
                if let Some((pct, _)) = t.progress {
                    if pct > 0 {
                        let w = rect.width() * pct as f32 / 100.0;
                        painter.rect_filled(
                            Rect::from_min_size(rect.min, Vec2::new(w, rect.height())),
                            4.0,
                            theme.progress,
                        );
                    }
                }
                let sw = if t.has_tag(Tag::Active) { 2.0 } else { 1.0 };
                painter.rect_stroke(rect, 4.0, Stroke::new(sw, theme.bar_stroke), StrokeKind::Middle);
                if selection == Some(&t.id) {
                    painter.rect_stroke(
                        rect.expand(2.5),
                        5.0,
                        Stroke::new(2.0, theme.selection),
                        StrokeKind::Middle,
                    );
                }
                // Label inside if it fits (black/white by fill luminance),
                // else to the right.
                let galley =
                    painter.layout_no_wrap(t.label.clone(), label_font.clone(), theme.text);
                if galley.size().x + 12.0 <= rect.width() {
                    let [r, g, b, _] = fill.to_array();
                    let inside_color = to_color32(tangt_core::geometry::contrast_text(
                        tangt_core::geometry::Rgba::rgb(r, g, b),
                    ));
                    painter.text(
                        Pos2::new(rect.left() + 6.0, rect.center().y),
                        Align2::LEFT_CENTER,
                        &t.label,
                        label_font.clone(),
                        inside_color,
                    );
                } else {
                    painter.text(
                        Pos2::new(rect.right() + 6.0, rect.center().y),
                        Align2::LEFT_CENTER,
                        &t.label,
                        label_font.clone(),
                        theme.text,
                    );
                }
                // Dependency dot for hovered/selected bars.
                let show_dot = selection == Some(&t.id)
                    || hovered_bar.as_ref().is_some_and(|(id, _)| id == &t.id);
                if show_dot {
                    painter.circle_filled(
                        Pos2::new(rect.right() + DEP_DOT_OFFSET, rect.center().y),
                        DEP_DOT_R,
                        theme.selection,
                    );
                }
            }
        }
        // Reorder insertion line.
        if let Some(drag) = &state.drag {
            if matches!(drag.kind, DragKind::Reorder) && drag.moved {
                let row = (((drag.pointer.y - top_y) / ROW_H).round().max(0.0) as usize)
                    .min(layout.row_count);
                let y = top_y + row as f32 * ROW_H;
                ui.painter_at(full).line_segment(
                    [
                        Pos2::new(full.left(), y),
                        Pos2::new(full.right(), y),
                    ],
                    Stroke::new(2.0, theme.selection),
                );
            }
        }
    }

    // Gutter.
    {
        let painter = ui.painter_at(regions.gutter);
        painter.rect_filled(regions.gutter, 0.0, theme.gutter_bg);
        let font = FontId::proportional(BASE_FONT_SIZE);
        for (row, r) in model.rows.iter().enumerate() {
            let y = row_top(row);
            if y + ROW_H < regions.gutter.top() || y > regions.gutter.bottom() {
                continue;
            }
            match r {
                Row::Section(s) => {
                    painter.rect_filled(
                        Rect::from_min_max(
                            Pos2::new(regions.gutter.left(), y),
                            Pos2::new(regions.gutter.right(), y + ROW_H),
                        ),
                        0.0,
                        theme.section_band,
                    );
                    painter.text(
                        Pos2::new(regions.gutter.left() + GUTTER_PAD / 2.0, y + ROW_H / 2.0),
                        Align2::LEFT_CENTER,
                        &model.sections[*s].label,
                        FontId::proportional(BASE_FONT_SIZE),
                        theme.text,
                    );
                }
                Row::Task(id) => {
                    let selected = selection == Some(id);
                    let hovered = hovered_gutter_row == Some(row);
                    if selected || hovered {
                        painter.rect_filled(
                            Rect::from_min_max(
                                Pos2::new(regions.gutter.left(), y),
                                Pos2::new(regions.gutter.right(), y + ROW_H),
                            ),
                            0.0,
                            theme.selection.gamma_multiply(if selected { 0.25 } else { 0.12 }),
                        );
                    }
                    let t = &model.tasks[id];
                    let indent = if t.section.is_some() { GUTTER_PAD } else { GUTTER_PAD / 2.0 };
                    painter.text(
                        Pos2::new(regions.gutter.left() + indent, y + ROW_H / 2.0),
                        Align2::LEFT_CENTER,
                        &t.label,
                        font.clone(),
                        if t.milestone { theme.muted } else { theme.text },
                    );
                }
            }
        }
    }

    // Header.
    {
        let painter = ui.painter_at(regions.header);
        painter.rect_filled(regions.header, 0.0, theme.header_bg);
        let tick_font = FontId::proportional(BASE_FONT_SIZE - 3.0);
        for tick in &layout.ticks {
            let x = to_screen([layout.scale.day_to_x(tick.day), 0.0]).x;
            if x < regions.header.left() - 40.0 || x > regions.header.right() {
                continue;
            }
            let major = tick.kind != TickKind::Day;
            painter.line_segment(
                [
                    Pos2::new(x, regions.header.bottom() - if major { 18.0 } else { 10.0 }),
                    Pos2::new(x, regions.header.bottom()),
                ],
                Stroke::new(1.0, if major { theme.grid_major } else { theme.grid }),
            );
            if let Some(label) = &tick.label {
                painter.text(
                    Pos2::new(x + 3.0, regions.header.bottom() - 10.0),
                    Align2::LEFT_CENTER,
                    label,
                    tick_font.clone(),
                    theme.muted,
                );
            }
        }
        let month_font = FontId::proportional(BASE_FONT_SIZE - 1.0);
        for (start, label) in &layout.months {
            let x = to_screen([layout.scale.day_to_x(*start), 0.0]).x;
            let x = x.max(regions.header.left() + 4.0);
            painter.text(
                Pos2::new(x + 2.0, regions.header.top() + HEADER_H * 0.3),
                Align2::LEFT_CENTER,
                label,
                month_font.clone(),
                theme.text,
            );
        }
    }

    // Corner: title.
    {
        let painter = ui.painter_at(regions.corner);
        painter.rect_filled(regions.corner, 0.0, theme.header_bg);
        if let Some(title) = &model.title {
            painter.text(
                Pos2::new(regions.corner.left() + GUTTER_PAD / 2.0, regions.corner.center().y),
                Align2::LEFT_CENTER,
                title,
                FontId::proportional(BASE_FONT_SIZE + 1.0),
                theme.text,
            );
        }
    }

    // Separators over everything.
    painter.line_segment(
        [
            Pos2::new(full.left(), full.top() + HEADER_H),
            Pos2::new(full.right(), full.top() + HEADER_H),
        ],
        Stroke::new(1.0, theme.grid_major),
    );
    painter.line_segment(
        [
            Pos2::new(full.left() + gutter_w, full.top()),
            Pos2::new(full.left() + gutter_w, full.bottom()),
        ],
        Stroke::new(1.0, theme.grid_major),
    );

    // ---- inline rename ---------------------------------------------------
    if let Some((id, buffer)) = &mut state.rename {
        // Anchor near the bar when visible, else at the pointer; clamped into
        // the chart region so it can't open under the pinned header/offscreen.
        let raw = screen_bars
            .get(id)
            .map(|r| r.left_top() + Vec2::new(0.0, -34.0))
            .filter(|p| regions.chart.expand(40.0).contains(*p))
            .or(hover)
            .unwrap_or(regions.chart.left_top());
        let anchor = Pos2::new(
            raw.x.clamp(regions.chart.left(), regions.chart.right() - 220.0),
            raw.y.clamp(regions.chart.top(), regions.chart.bottom() - 32.0),
        );
        let mut commit = false;
        let mut cancel = false;
        egui::Window::new("rename-task")
            .title_bar(false)
            .resizable(false)
            .fixed_pos(anchor)
            .show(ui.ctx(), |ui| {
                let resp = ui.text_edit_singleline(buffer);
                if !state.rename_focused {
                    resp.request_focus();
                    state.rename_focused = true;
                }
                if ui.input(|i| i.key_pressed(egui::Key::Enter)) || resp.lost_focus() {
                    commit = true;
                }
                if ui.input(|i| i.key_pressed(egui::Key::Escape)) {
                    cancel = true;
                }
            });
        if cancel {
            state.rename = None;
            state.rename_focused = false;
        } else if commit {
            let (id, label) = state.rename.take().unwrap();
            state.rename_focused = false;
            events.push(CanvasEvent::Rename { id, label });
        }
    } else {
        state.rename_focused = false;
    }

    // ---- empty-state hint --------------------------------------------------
    if model.tasks.is_empty() && state.drag.is_none() {
        let painter = ui.painter_at(regions.chart);
        let c = regions.chart.center();
        painter.text(
            c,
            Align2::CENTER_CENTER,
            "Right-click to add a task — or type in the editor:",
            FontId::proportional(BASE_FONT_SIZE + 1.0),
            theme.muted,
        );
        painter.text(
            c + Vec2::new(0.0, 24.0),
            Align2::CENTER_CENTER,
            "task \"Design\" : d1, 2026-08-10, 5d        (⌘/ opens the reference)",
            FontId::monospace(BASE_FONT_SIZE - 1.0),
            theme.muted,
        );
    }

    // ---- hover tooltip -----------------------------------------------------
    if state.drag.is_none() {
        if let Some((id, _)) = &hovered_bar {
            if let Some(t) = model.tasks.get(id) {
                let fmt = tangt_core::resolve::format_day;
                let mut line = format!("{} · {} → {}", t.label, fmt(t.start_day), fmt(t.end_day));
                if !t.milestone {
                    line.push_str(&format!(
                        " · {}d",
                        model.calendar.working_days_between(t.start_day, t.end_day)
                    ));
                }
                if let Some((pct, _)) = t.progress {
                    line.push_str(&format!(" · {pct}%"));
                }
                for tag in &t.tags {
                    line.push_str(&format!(" · {tag:?}").to_lowercase());
                }
                egui::Tooltip::always_open(
                    ui.ctx().clone(),
                    ui.layer_id(),
                    egui::Id::new("bar-tip"),
                    egui::PopupAnchor::Pointer,
                )
                .show(|ui| {
                    ui.label(line);
                });
            }
        }
    }

    if state.drag.is_none() {
        state.hint = None;
    }

    events
}

fn context_menu(
    ui: &mut Ui,
    model: &GanttModel,
    state: &mut CanvasState,
    events: &mut Vec<CanvasEvent>,
) {
    let Some(target) = state.ctx_target.clone() else {
        ui.close();
        return;
    };
    match target {
        CtxTarget::Task(id) => {
            let Some(t) = model.tasks.get(&id) else {
                ui.close();
                return;
            };
            ui.label(format!("{} ({})", t.label, t.id));
            ui.separator();
            if ui.button("Rename…").clicked() {
                state.rename = Some((id.clone(), t.label.clone()));
                ui.close();
            }
            if !t.milestone {
                ui.menu_button("Progress", |ui| {
                    for pct in [0u8, 25, 50, 75, 100] {
                        if ui.button(format!("{pct}%")).clicked() {
                            events.push(CanvasEvent::SetProgress { id: id.clone(), pct });
                            ui.close();
                        }
                    }
                });
            }
            if !t.deps.is_empty() {
                ui.menu_button("Dependencies", |ui| {
                    for dep in &t.deps {
                        if ui.button(format!("Remove “after {}”", dep.on)).clicked() {
                            events.push(CanvasEvent::RemoveDep {
                                task: id.clone(),
                                on: dep.on.clone(),
                            });
                            ui.close();
                        }
                    }
                });
            }
            ui.separator();
            if ui.button("Add task below").clicked() {
                events.push(CanvasEvent::AddTask {
                    anchor: Some(id.clone()),
                    day: t.start_day,
                    milestone: false,
                });
                ui.close();
            }
            if ui.button("Add milestone below").clicked() {
                events.push(CanvasEvent::AddTask {
                    anchor: Some(id.clone()),
                    day: t.end_day + 1,
                    milestone: true,
                });
                ui.close();
            }
            if ui.button("Add section below").clicked() {
                events.push(CanvasEvent::AddSection { anchor: Some(id.clone()) });
                ui.close();
            }
            ui.separator();
            if ui.button("Jump to source").clicked() {
                events.push(CanvasEvent::JumpToText(t.stmt_span.start));
                ui.close();
            }
            if ui.button("Delete task").clicked() {
                events.push(CanvasEvent::DeleteTask { id: id.clone() });
                ui.close();
            }
        }
        CtxTarget::Section(idx) => {
            ui.label(format!("section {}", model.sections[idx].label));
            ui.separator();
            if ui.button("Jump to source").clicked() {
                events.push(CanvasEvent::JumpToText(model.sections[idx].stmt_span.start));
                ui.close();
            }
            if ui
                .button("Delete section header")
                .on_hover_text("Tasks join the previous section")
                .clicked()
            {
                events.push(CanvasEvent::DeleteSection { idx });
                ui.close();
            }
        }
        CtxTarget::Empty { day, anchor } => {
            if ui.button("Add task here").clicked() {
                events.push(CanvasEvent::AddTask { anchor: anchor.clone(), day, milestone: false });
                ui.close();
            }
            if ui.button("Add milestone here").clicked() {
                events.push(CanvasEvent::AddTask { anchor: anchor.clone(), day, milestone: true });
                ui.close();
            }
            if ui.button("Add section").clicked() {
                events.push(CanvasEvent::AddSection { anchor });
                ui.close();
            }
        }
    }
}

/// Orthogonal arrow between two screen bar rects (mirrors core routing so the
/// canvas stays live during drags).
fn draw_arrow(painter: &egui::Painter, from: Rect, to: Rect, color: Color32) {
    let start = Pos2::new(from.right(), from.center().y);
    let entry = Pos2::new(to.left(), to.center().y);
    let stub = 8.0;
    let below = entry.y > start.y;
    let pts: Vec<Pos2> = if entry.x >= start.x + 2.0 * stub {
        vec![
            start,
            Pos2::new(start.x + stub, start.y),
            Pos2::new(start.x + stub, entry.y),
            entry,
        ]
    } else if start.x + stub >= to.left() && start.x + stub <= to.right() {
        let edge_y = if below { to.top() } else { to.bottom() };
        vec![start, Pos2::new(start.x + stub, start.y), Pos2::new(start.x + stub, edge_y)]
    } else {
        let gap_y = if below { to.top() - BAR_INSET } else { to.bottom() + BAR_INSET };
        vec![
            start,
            Pos2::new(start.x + stub, start.y),
            Pos2::new(start.x + stub, gap_y),
            Pos2::new(entry.x - stub, gap_y),
            Pos2::new(entry.x - stub, entry.y),
            entry,
        ]
    };
    let stroke = Stroke::new(1.4, color);
    for pair in pts.windows(2) {
        painter.line_segment([pair[0], pair[1]], stroke);
    }
    // Arrowhead along the final segment.
    let tip = pts[pts.len() - 1];
    let prev = pts[pts.len() - 2];
    let d = tip - prev;
    let len = d.length().max(1e-3);
    let u = d / len;
    let p = Vec2::new(-u.y, u.x);
    let base = tip - u * 7.0;
    painter.add(EShape::convex_polygon(
        vec![tip, base + p * 4.0, base - p * 4.0],
        color,
        Stroke::NONE,
    ));
}
