//! Structural golden test: the bundled example renders to an SVG with the
//! expected shape — right bar count, arrows, sections, headers. Uses the
//! MonoMeasurer so it is font-independent.

use tangt_core::export::{build_scene, SceneItem, SceneStyle};
use tangt_core::export::svg::to_svg;
use tangt_core::geometry::MonoMeasurer;
use tangt_core::layout::compute_default;
use tangt_core::parser::parse;
use tangt_core::resolve::{parse_date, resolve};

const EXAMPLE: &str = include_str!("../../../examples/website-redesign.tangt");

#[test]
fn example_renders_structurally() {
    let today = parse_date("2026-08-10").unwrap();
    let model = resolve(&parse(EXAMPLE), today);
    assert!(!model.has_errors(), "diags: {:?}", model.diagnostics);
    assert_eq!(model.tasks.len(), 11);
    assert_eq!(model.sections.len(), 3);
    assert!(model.calendar.exclude_weekends);

    let layout = compute_default(&model, &MonoMeasurer);
    assert_eq!(layout.bar_rects.len(), 11);
    // vis, rev, ds, fe, be, int(×2: fe+be), qa, cl, gl, hc — one arrow per dep.
    assert_eq!(layout.arrows.len(), 11);

    let scene = build_scene(&model, &layout, SceneStyle::default(), &MonoMeasurer);
    let polygons = scene
        .items
        .iter()
        .filter(|i| matches!(i, SceneItem::Polygon { .. }))
        .count();
    // 2 milestone diamonds + 11 arrowheads.
    assert_eq!(polygons, 13);

    let svg = to_svg(&scene);
    assert!(svg.starts_with("<svg "));
    assert!(svg.contains("Website Redesign"));
    assert!(svg.contains("Wireframes"));
    assert!(svg.contains("Aug 2026"));
    assert!(svg.contains("<polygon"));
    assert!(svg.contains("stroke-dasharray")); // today marker
    // Bars are rects with rounding 4.
    assert!(svg.contains(r#"rx="4""#));
}

#[test]
fn empty_file_still_renders() {
    let model = resolve(&parse(""), 20675);
    let layout = compute_default(&model, &MonoMeasurer);
    let scene = build_scene(&model, &layout, SceneStyle::default(), &MonoMeasurer);
    let svg = to_svg(&scene);
    assert!(svg.starts_with("<svg "));
    assert!(scene.bounds.is_finite());
}
