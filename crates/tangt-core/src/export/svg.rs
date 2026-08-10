//! Scene -> SVG string. No dependencies; the app rasterizes this same SVG for
//! PNG export and converts it for PDF, so all formats always agree.

use super::{Align, Scene, SceneItem};
use crate::geometry::Rgba;
use std::fmt::Write;

fn color(Rgba([r, g, b, a]): Rgba) -> String {
    if a == 255 {
        format!("#{r:02x}{g:02x}{b:02x}")
    } else {
        format!("rgba({r},{g},{b},{:.3})", a as f32 / 255.0)
    }
}

fn esc(s: &str) -> String {
    s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;")
}

pub fn to_svg(scene: &Scene) -> String {
    let b = scene.bounds;
    let (w, h) = (b.width(), b.height());
    let mut out = String::new();
    let _ = writeln!(
        out,
        r#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="{} {} {} {}" width="{w}" height="{h}" font-family="Helvetica, Arial, sans-serif">"#,
        b.min[0], b.min[1], w, h
    );
    let _ = writeln!(
        out,
        r#"<rect x="{}" y="{}" width="{w}" height="{h}" fill="{}"/>"#,
        b.min[0],
        b.min[1],
        color(scene.style.background)
    );

    for item in &scene.items {
        match item {
            SceneItem::Rect { rect, fill, stroke, rounding } => {
                let stroke_attr = match stroke {
                    Some((c, sw)) => format!(r#" stroke="{}" stroke-width="{sw}""#, color(*c)),
                    None => String::new(),
                };
                let _ = writeln!(
                    out,
                    r#"<rect x="{}" y="{}" width="{}" height="{}" rx="{rounding}" fill="{}"{stroke_attr}/>"#,
                    rect.min[0],
                    rect.min[1],
                    rect.width(),
                    rect.height(),
                    color(*fill)
                );
            }
            SceneItem::Line { points, stroke, width, dashed } => {
                let dash = if *dashed { r#" stroke-dasharray="5 4""# } else { "" };
                let pts: Vec<String> =
                    points.iter().map(|p| format!("{},{}", p[0], p[1])).collect();
                let _ = writeln!(
                    out,
                    r#"<polyline points="{}" fill="none" stroke="{}" stroke-width="{width}"{dash}/>"#,
                    pts.join(" "),
                    color(*stroke)
                );
            }
            SceneItem::Polygon { points, fill } => {
                let pts: Vec<String> =
                    points.iter().map(|p| format!("{},{}", p[0], p[1])).collect();
                let _ = writeln!(
                    out,
                    r#"<polygon points="{}" fill="{}"/>"#,
                    pts.join(" "),
                    color(*fill)
                );
            }
            SceneItem::Text { pos, text, size, color: c, bold, align } => {
                let weight = if *bold { r#" font-weight="bold""# } else { "" };
                let anchor = match align {
                    Align::Left => "start",
                    Align::Center => "middle",
                };
                let _ = writeln!(
                    out,
                    r#"<text x="{}" y="{}" font-size="{size}" text-anchor="{anchor}" dominant-baseline="central" fill="{}"{weight}>{}</text>"#,
                    pos[0],
                    pos[1],
                    color(*c),
                    esc(text)
                );
            }
        }
    }
    out.push_str("</svg>\n");
    out
}
