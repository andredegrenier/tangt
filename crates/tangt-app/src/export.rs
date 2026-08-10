//! SVG, PNG, and PDF export. PNG rasterizes and PDF converts our own SVG, so
//! every format always agrees with the canvas. Exports are theme-independent
//! (light `SceneStyle`).

use std::path::Path;
use tangt_core::export::{build_scene, svg::to_svg, SceneStyle};
use tangt_core::geometry::TextMeasurer;
use tangt_core::layout::compute_default;
use tangt_core::model::GanttModel;

fn chart_svg(model: &GanttModel, measurer: &dyn TextMeasurer) -> String {
    let layout = compute_default(model, measurer);
    let scene = build_scene(model, &layout, SceneStyle::default(), measurer);
    to_svg(&scene)
}

pub fn export_svg(
    model: &GanttModel,
    measurer: &dyn TextMeasurer,
    path: &Path,
) -> Result<(), String> {
    std::fs::write(path, chart_svg(model, measurer)).map_err(|e| e.to_string())
}

pub fn export_png(
    model: &GanttModel,
    measurer: &dyn TextMeasurer,
    path: &Path,
    scale: f32,
) -> Result<(), String> {
    let svg = chart_svg(model, measurer);
    let mut opts = resvg::usvg::Options::default();
    opts.fontdb_mut().load_system_fonts();
    let tree = resvg::usvg::Tree::from_str(&svg, &opts).map_err(|e| e.to_string())?;
    let size = tree.size();
    let (w, h) = (
        (size.width() * scale).ceil() as u32,
        (size.height() * scale).ceil() as u32,
    );
    let mut pixmap = resvg::tiny_skia::Pixmap::new(w.max(1), h.max(1))
        .ok_or_else(|| "pixmap allocation failed".to_string())?;
    resvg::render(
        &tree,
        resvg::tiny_skia::Transform::from_scale(scale, scale),
        &mut pixmap.as_mut(),
    );
    pixmap.save_png(path).map_err(|e| e.to_string())
}

/// Vector PDF: one page auto-sized to the chart (at 72 dpi, 1 px = 1 pt).
pub fn export_pdf(
    model: &GanttModel,
    measurer: &dyn TextMeasurer,
    path: &Path,
) -> Result<(), String> {
    let svg = chart_svg(model, measurer);
    let mut opts = svg2pdf::usvg::Options::default();
    opts.fontdb_mut().load_system_fonts();
    let tree = svg2pdf::usvg::Tree::from_str(&svg, &opts).map_err(|e| e.to_string())?;
    let pdf = svg2pdf::to_pdf(
        &tree,
        svg2pdf::ConversionOptions::default(),
        svg2pdf::PageOptions::default(),
    )
    .map_err(|e| e.to_string())?;
    std::fs::write(path, pdf).map_err(|e| e.to_string())
}

pub fn pick_export(extension: &str) -> Option<std::path::PathBuf> {
    rfd::FileDialog::new()
        .add_filter(extension, &[extension])
        .set_file_name(format!("chart.{extension}"))
        .save_file()
}
