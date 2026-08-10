//! Confirms svg2pdf handles the SVG features tangt's exporter emits:
//! rects with rounded corners and opacity, dashed lines, polylines, text.

#[test]
fn svg_features_convert_to_pdf() {
    let svg = r##"<svg xmlns="http://www.w3.org/2000/svg" width="400" height="200" viewBox="0 0 400 200">
  <rect x="0" y="0" width="400" height="200" fill="#ffffff"/>
  <rect x="20" y="30" width="120" height="18" rx="4" fill="#4fa3ff"/>
  <rect x="20" y="30" width="72" height="18" rx="4" fill="#1d6fd1" fill-opacity="0.6"/>
  <rect x="150" y="0" width="26" height="200" fill="#000000" fill-opacity="0.05"/>
  <line x1="200" y1="0" x2="200" y2="200" stroke="#e05555" stroke-width="1.5" stroke-dasharray="4 3"/>
  <polyline points="140,39 152,39 152,70 164,70" fill="none" stroke="#888888" stroke-width="1.2"/>
  <path d="M164,70 l-6,-4 l0,8 z" fill="#888888"/>
  <text x="24" y="43" font-family="Helvetica" font-size="12" fill="#111111">Wireframes</text>
</svg>"##;

    let mut opts = svg2pdf::usvg::Options::default();
    opts.fontdb_mut().load_system_fonts();
    let tree = svg2pdf::usvg::Tree::from_str(svg, &opts).expect("usvg parses exporter SVG");
    let pdf = svg2pdf::to_pdf(
        &tree,
        svg2pdf::ConversionOptions::default(),
        svg2pdf::PageOptions::default(),
    )
    .expect("svg2pdf converts");

    assert!(pdf.starts_with(b"%PDF"));
    assert!(pdf.len() > 1000, "suspiciously small PDF: {} bytes", pdf.len());
}
