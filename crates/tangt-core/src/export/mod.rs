//! Render-target-agnostic scene: a flat list of styled primitives, built from
//! a model + layout. The canvas paints the same structure the SVG exporter
//! serializes, so exports always match the screen.
//!
//! Scene coordinates: (0,0) is the top-left of the whole chart image — gutter
//! and header included. Day x = `gutter_w + scale.day_to_x(day)`, row y =
//! `HEADER_H + row * ROW_H`.

pub mod svg;

use crate::ast::Tag;
use crate::geometry::{
    contrast_text, Rect, Rgba, TextMeasurer, BASE_FONT_SIZE, GUTTER_PAD, HEADER_H, ROW_H,
};
use crate::layout::{GanttLayout, TickKind};
use crate::model::{GanttModel, Row};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Align {
    Left,
    Center,
}

#[derive(Debug, Clone)]
pub enum SceneItem {
    Rect {
        rect: Rect,
        fill: Rgba,
        stroke: Option<(Rgba, f32)>,
        rounding: f32,
    },
    Line {
        points: Vec<[f32; 2]>,
        stroke: Rgba,
        width: f32,
        dashed: bool,
    },
    /// Filled polygon (arrowheads, milestone diamonds).
    Polygon {
        points: Vec<[f32; 2]>,
        fill: Rgba,
    },
    Text {
        /// Anchor point: left-center for `Align::Left`, center for `Center`.
        pos: [f32; 2],
        text: String,
        size: f32,
        color: Rgba,
        bold: bool,
        align: Align,
    },
}

#[derive(Debug, Clone, Copy)]
pub struct SceneStyle {
    pub background: Rgba,
    pub text: Rgba,
    pub muted: Rgba,
    pub grid: Rgba,
    pub grid_major: Rgba,
    pub bar: Rgba,
    pub bar_stroke: Rgba,
    pub bar_done: Rgba,
    pub crit: Rgba,
    pub milestone: Rgba,
    pub arrow: Rgba,
    pub weekend: Rgba,
    pub section_band: Rgba,
    pub today: Rgba,
    /// Overlay drawn over the completed fraction of a bar.
    pub progress: Rgba,
}

impl Default for SceneStyle {
    fn default() -> Self {
        SceneStyle {
            background: Rgba::rgb(0xff, 0xff, 0xff),
            text: Rgba::rgb(0x1a, 0x20, 0x28),
            muted: Rgba::rgb(0x6a, 0x74, 0x80),
            grid: Rgba::rgba(0x00, 0x00, 0x00, 0x14),
            grid_major: Rgba::rgba(0x00, 0x00, 0x00, 0x30),
            bar: Rgba::rgb(0x4f, 0xa3, 0xff),
            bar_stroke: Rgba::rgb(0x2d, 0x74, 0xc4),
            bar_done: Rgba::rgb(0x9a, 0xa6, 0xb2),
            crit: Rgba::rgb(0xe0, 0x55, 0x55),
            milestone: Rgba::rgb(0x8a, 0x5c, 0xd6),
            arrow: Rgba::rgb(0x55, 0x60, 0x70),
            weekend: Rgba::rgba(0x00, 0x00, 0x00, 0x0d),
            section_band: Rgba::rgba(0x5a, 0x64, 0x70, 0x18),
            today: Rgba::rgb(0xc7, 0x7d, 0x00),
            progress: Rgba::rgba(0x00, 0x00, 0x00, 0x38),
        }
    }
}

#[derive(Debug, Clone)]
pub struct Scene {
    pub items: Vec<SceneItem>,
    pub bounds: Rect,
    pub style: SceneStyle,
}

/// Build the complete chart as a scene (header + gutter + rows).
pub fn build_scene(
    model: &GanttModel,
    layout: &GanttLayout,
    style: SceneStyle,
    measurer: &dyn TextMeasurer,
) -> Scene {
    let mut items = Vec::new();
    let g = layout.gutter_w;
    let chart_w = layout.scale.width();
    let total_w = g + chart_w;
    let total_h = HEADER_H + layout.chart_h;
    let day_x = |day: i64| g + layout.scale.day_to_x(day);
    let row_y = |row: usize| HEADER_H + row as f32 * ROW_H;

    // Weekend / holiday shading columns.
    if model.calendar.has_exclusions() {
        for day in layout.scale.min_day..=layout.scale.max_day {
            if !model.calendar.is_workday(day) {
                items.push(SceneItem::Rect {
                    rect: Rect::new([day_x(day), HEADER_H], [day_x(day + 1), total_h]),
                    fill: style.weekend,
                    stroke: None,
                    rounding: 0.0,
                });
            }
        }
    }

    // Section bands.
    for (row, r) in model.rows.iter().enumerate() {
        if let Row::Section(_) = r {
            items.push(SceneItem::Rect {
                rect: Rect::new([0.0, row_y(row)], [total_w, row_y(row) + ROW_H]),
                fill: style.section_band,
                stroke: None,
                rounding: 0.0,
            });
        }
    }

    // Grid lines + header tick labels.
    for tick in &layout.ticks {
        let x = day_x(tick.day);
        let major = tick.kind != TickKind::Day;
        items.push(SceneItem::Line {
            points: vec![[x, HEADER_H], [x, total_h]],
            stroke: if major { style.grid_major } else { style.grid },
            width: 1.0,
            dashed: false,
        });
        if let Some(label) = &tick.label {
            items.push(SceneItem::Text {
                pos: [x + 3.0, HEADER_H - 10.0],
                text: label.clone(),
                size: BASE_FONT_SIZE - 3.0,
                color: style.muted,
                bold: false,
                align: Align::Left,
            });
        }
    }

    // Month bands in the header's upper half. The first label yields to the
    // title if the gutter is too narrow for it.
    let title_end = model
        .title
        .as_ref()
        .map_or(0.0, |t| GUTTER_PAD / 2.0 + measurer.measure(t, BASE_FONT_SIZE + 2.0)[0]);
    for (i, (start, label)) in layout.months.iter().enumerate() {
        let mut x = day_x(*start) + 4.0;
        if i == 0 {
            x = x.max(title_end + 16.0);
        }
        items.push(SceneItem::Text {
            pos: [x, HEADER_H * 0.3],
            text: label.clone(),
            size: BASE_FONT_SIZE - 1.0,
            color: style.text,
            bold: true,
            align: Align::Left,
        });
    }

    // Header/gutter separators.
    items.push(SceneItem::Line {
        points: vec![[0.0, HEADER_H], [total_w, HEADER_H]],
        stroke: style.grid_major,
        width: 1.0,
        dashed: false,
    });
    items.push(SceneItem::Line {
        points: vec![[g, 0.0], [g, total_h]],
        stroke: style.grid_major,
        width: 1.0,
        dashed: false,
    });

    // Today marker.
    if model.today >= layout.scale.min_day && model.today <= layout.scale.max_day {
        let x = g + layout.scale.day_center_x(model.today);
        items.push(SceneItem::Line {
            points: vec![[x, HEADER_H], [x, total_h]],
            stroke: style.today,
            width: 1.5,
            dashed: true,
        });
    }

    // Dependency arrows (under the bars' labels, over the grid).
    for arrow in &layout.arrows {
        let points: Vec<[f32; 2]> =
            arrow.points.iter().map(|p| [p[0] + g, p[1] + HEADER_H]).collect();
        let n = points.len();
        let tip = points[n - 1];
        let prev = points[n - 2];
        // Head follows the direction of the final segment.
        let (dx, dy) = (tip[0] - prev[0], tip[1] - prev[1]);
        let len = (dx * dx + dy * dy).sqrt().max(1e-3);
        let (ux, uy) = (dx / len, dy / len);
        let base = [tip[0] - ux * 7.0, tip[1] - uy * 7.0];
        items.push(SceneItem::Line {
            points,
            stroke: style.arrow,
            width: 1.4,
            dashed: false,
        });
        items.push(SceneItem::Polygon {
            points: vec![
                tip,
                [base[0] - uy * 4.0, base[1] + ux * 4.0],
                [base[0] + uy * 4.0, base[1] - ux * 4.0],
            ],
            fill: style.arrow,
        });
    }

    // Bars, milestones, gutter labels. Track how far outside-labels reach so
    // the bounds never clip them.
    let mut label_max_x: f32 = 0.0;
    let gutter_text_w = g - 1.5 * GUTTER_PAD;
    for t in model.tasks.values() {
        let Some(r) = layout.bar_rects.get(&t.id) else { continue };
        let rect = Rect::new([r.min[0] + g, r.min[1] + HEADER_H], [r.max[0] + g, r.max[1] + HEADER_H]);

        if t.milestone {
            let c = rect.center();
            let (hw, hh) = (rect.width() / 2.0, rect.height() / 2.0);
            items.push(SceneItem::Polygon {
                points: vec![
                    [c[0], c[1] - hh],
                    [c[0] + hw, c[1]],
                    [c[0], c[1] + hh],
                    [c[0] - hw, c[1]],
                ],
                fill: t.color.map(|(c, _)| Rgba::from_rgb3(c)).unwrap_or(style.milestone),
            });
            let tw = measurer.measure(&t.label, BASE_FONT_SIZE - 2.0)[0];
            label_max_x = label_max_x.max(rect.max[0] + 6.0 + tw);
        } else {
            let fill = if let Some((c, _)) = t.color {
                Rgba::from_rgb3(c)
            } else if t.has_tag(Tag::Done) {
                style.bar_done
            } else if t.has_tag(Tag::Crit) {
                style.crit
            } else {
                style.bar
            };
            items.push(SceneItem::Rect {
                rect,
                fill,
                stroke: Some((style.bar_stroke, if t.has_tag(Tag::Active) { 2.0 } else { 1.0 })),
                rounding: 4.0,
            });
            if let Some((pct, _)) = t.progress {
                if pct > 0 {
                    let w = rect.width() * pct as f32 / 100.0;
                    items.push(SceneItem::Rect {
                        rect: Rect::from_min_size(rect.min, [w, rect.height()]),
                        fill: style.progress,
                        stroke: None,
                        rounding: 4.0,
                    });
                }
            }
            // Label inside the bar if it fits, otherwise to the right.
            let tw = measurer.measure(&t.label, BASE_FONT_SIZE - 2.0)[0];
            let inside = tw + 12.0 <= rect.width();
            if !inside {
                label_max_x = label_max_x.max(rect.max[0] + 6.0 + tw);
            }
            items.push(SceneItem::Text {
                pos: if inside {
                    [rect.min[0] + 6.0, rect.center()[1]]
                } else {
                    [rect.max[0] + 6.0, rect.center()[1]]
                },
                text: t.label.clone(),
                size: BASE_FONT_SIZE - 2.0,
                color: if inside { contrast_text(fill) } else { style.text },
                bold: false,
                align: Align::Left,
            });
        }

        // Gutter row label, elided to the gutter width.
        items.push(SceneItem::Text {
            pos: [GUTTER_PAD, row_y(t.row) + ROW_H / 2.0],
            text: elide(&t.label, gutter_text_w, BASE_FONT_SIZE, measurer),
            size: BASE_FONT_SIZE,
            color: style.text,
            bold: false,
            align: Align::Left,
        });
    }

    // Section labels in the gutter (bold, over the band).
    for s in &model.sections {
        items.push(SceneItem::Text {
            pos: [GUTTER_PAD / 2.0, row_y(s.row) + ROW_H / 2.0],
            text: elide(&s.label, gutter_text_w, BASE_FONT_SIZE, measurer),
            size: BASE_FONT_SIZE,
            color: style.text,
            bold: true,
            align: Align::Left,
        });
    }

    // Title in the header, above the gutter.
    if let Some(title) = &model.title {
        items.push(SceneItem::Text {
            pos: [GUTTER_PAD / 2.0, HEADER_H * 0.3],
            text: title.clone(),
            size: BASE_FONT_SIZE + 2.0,
            color: style.text,
            bold: true,
            align: Align::Left,
        });
    }

    Scene {
        items,
        bounds: Rect::new([0.0, 0.0], [total_w.max(label_max_x), total_h]).expand(16.0),
        style,
    }
}

/// Truncate `text` with an ellipsis so it measures at most `max_w`.
fn elide(text: &str, max_w: f32, size: f32, measurer: &dyn TextMeasurer) -> String {
    if measurer.measure(text, size)[0] <= max_w {
        return text.to_string();
    }
    let mut out = String::new();
    for c in text.chars() {
        out.push(c);
        if measurer.measure(&out, size)[0] > max_w - measurer.measure("…", size)[0] {
            out.pop();
            out.push('…');
            return out;
        }
    }
    out
}
