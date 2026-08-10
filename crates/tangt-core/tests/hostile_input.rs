//! A hostile `.tangt` file must never panic or hang the resolve→layout→export
//! pipeline. These reproduce the pre-hardening security findings.

use std::time::{Duration, Instant};
use tangt_core::export::{build_scene, svg::to_svg, SceneStyle};
use tangt_core::geometry::MonoMeasurer;
use tangt_core::layout::compute_default;
use tangt_core::parser::parse;
use tangt_core::resolve::resolve;

const TODAY: i64 = 20675;

/// Full pipeline on `src`; returns the tick count and asserts it ran fast.
fn run(src: &str) -> usize {
    let start = Instant::now();
    let model = resolve(&parse(src), TODAY);
    let layout = compute_default(&model, &MonoMeasurer);
    let scene = build_scene(&model, &layout, SceneStyle::default(), &MonoMeasurer);
    let _ = to_svg(&scene);
    assert!(
        start.elapsed() < Duration::from_secs(2),
        "pipeline took too long ({:?}) — possible DoS regression",
        start.elapsed()
    );
    layout.ticks.len()
}

#[test]
fn huge_negative_lag_does_not_hang() {
    run("exclude weekends\ntask \"A\" : a, 2026-01-01, 5d\ntask \"B\" : b, after a - 999999999999999d\n");
}

#[test]
fn huge_duration_does_not_panic() {
    run("task \"A\" : a, 2026-01-01, 99999999d\n");
    run("task \"A\" : a, 2026-01-01, 1500000000000000000w\n");
}

#[test]
fn wide_date_span_is_bounded() {
    let ticks = run("task \"A\" : a, 1200-01-01, 1d\ntask \"B\" : b, 2900-01-01, 1d\n");
    assert!(ticks < 200_000, "tick vector unbounded: {ticks}");
}

#[test]
fn extreme_dates_do_not_panic() {
    run("task \"A\" : a, 0001-01-01, 1d\ntask \"B\" : b, 9999-12-01, 1d\n");
    run("exclude weekends\ntask \"A\" : a, 9999-12-31, 99999999w\n");
}

#[test]
fn giant_exclude_range_is_capped() {
    let model = resolve(
        &parse("exclude 0001-01-01..9999-12-31\ntask \"A\" : a, 2026-01-01, 5d\n"),
        TODAY,
    );
    assert!(model.calendar.holidays.is_empty());
    assert!(model.has_errors());
}
