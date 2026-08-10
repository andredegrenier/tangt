//! End-to-end export smoke test: example file → SVG + PNG + PDF on disk.
//! Set TANGT_EXPORT_DIR to keep the artifacts for inspection.

use tangt_core::geometry::MonoMeasurer;
use tangt_core::parser::parse;
use tangt_core::resolve::{parse_date, resolve};

// Included directly rather than through the binary crate; the file-dialog
// helper has no caller in this context.
#[path = "../src/export.rs"]
#[allow(dead_code)]
mod export;

const EXAMPLE: &str = include_str!("../../../examples/website-redesign.tangt");

#[test]
fn example_exports_all_formats() {
    let today = parse_date("2026-08-10").unwrap();
    let model = resolve(&parse(EXAMPLE), today);
    assert!(!model.has_errors());

    let dir = std::env::var("TANGT_EXPORT_DIR")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|_| std::env::temp_dir().join("tangt-export-smoke"));
    std::fs::create_dir_all(&dir).unwrap();

    export::export_svg(&model, &MonoMeasurer, &dir.join("example.svg")).unwrap();
    export::export_png(&model, &MonoMeasurer, &dir.join("example.png"), 2.0).unwrap();
    export::export_pdf(&model, &MonoMeasurer, &dir.join("example.pdf")).unwrap();

    let pdf = std::fs::read(dir.join("example.pdf")).unwrap();
    assert!(pdf.starts_with(b"%PDF"));
    let png = std::fs::read(dir.join("example.png")).unwrap();
    assert!(png.len() > 5000, "png suspiciously small: {}", png.len());
}
