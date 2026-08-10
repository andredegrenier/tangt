//! Colors for canvas and editor, resolved against egui's light/dark visuals.

use eframe::egui::Color32;
use tangt_core::geometry::Rgba;

pub fn to_color32(Rgba([r, g, b, a]): Rgba) -> Color32 {
    Color32::from_rgba_unmultiplied(r, g, b, a)
}

pub struct Theme {
    pub canvas_bg: Color32,
    pub header_bg: Color32,
    pub gutter_bg: Color32,
    pub text: Color32,
    pub muted: Color32,
    pub grid: Color32,
    pub grid_major: Color32,
    pub bar: Color32,
    pub bar_stroke: Color32,
    pub bar_done: Color32,
    pub crit: Color32,
    pub milestone: Color32,
    pub arrow: Color32,
    pub weekend: Color32,
    pub section_band: Color32,
    pub today: Color32,
    pub progress: Color32,
    pub selection: Color32,
    // editor token colors
    pub tok_keyword: Color32,
    pub tok_string: Color32,
    pub tok_number: Color32,
    pub tok_date: Color32,
    pub tok_color: Color32,
    pub tok_comment: Color32,
    pub tok_ident: Color32,
    pub tok_punct: Color32,
    pub diagnostic: Color32,
}

impl Theme {
    pub fn new(dark: bool) -> Self {
        if dark {
            Theme {
                canvas_bg: Color32::from_rgb(0x14, 0x17, 0x1c),
                header_bg: Color32::from_rgb(0x1a, 0x1e, 0x25),
                gutter_bg: Color32::from_rgb(0x18, 0x1c, 0x22),
                text: Color32::from_rgb(0xe8, 0xec, 0xf2),
                muted: Color32::from_rgb(0x8a, 0x96, 0xa8),
                grid: Color32::from_rgba_unmultiplied(0xff, 0xff, 0xff, 0x10),
                grid_major: Color32::from_rgba_unmultiplied(0xff, 0xff, 0xff, 0x2c),
                bar: Color32::from_rgb(0x4f, 0xa3, 0xff),
                bar_stroke: Color32::from_rgb(0x77, 0xb8, 0xff),
                bar_done: Color32::from_rgb(0x5a, 0x64, 0x70),
                crit: Color32::from_rgb(0xe0, 0x55, 0x55),
                milestone: Color32::from_rgb(0xb0, 0x8a, 0xe8),
                arrow: Color32::from_rgb(0x8a, 0x96, 0xa8),
                weekend: Color32::from_rgba_unmultiplied(0xff, 0xff, 0xff, 0x0a),
                section_band: Color32::from_rgba_unmultiplied(0x9a, 0xa6, 0xb8, 0x22),
                today: Color32::from_rgb(0xe8, 0xa3, 0x3d),
                progress: Color32::from_rgba_unmultiplied(0x00, 0x00, 0x00, 0x60),
                selection: Color32::from_rgb(0x4f, 0xa3, 0xff),
                tok_keyword: Color32::from_rgb(0xc7, 0x92, 0xea),
                tok_string: Color32::from_rgb(0xa5, 0xd6, 0x8a),
                tok_number: Color32::from_rgb(0xf0, 0xc6, 0x74),
                tok_date: Color32::from_rgb(0x6c, 0xc7, 0xd8),
                tok_color: Color32::from_rgb(0x6c, 0xc7, 0xd8),
                tok_comment: Color32::from_rgb(0x6a, 0x73, 0x7d),
                tok_ident: Color32::from_rgb(0xe8, 0xec, 0xf2),
                tok_punct: Color32::from_rgb(0x9a, 0xa4, 0xb0),
                diagnostic: Color32::from_rgb(0xff, 0x6b, 0x6b),
            }
        } else {
            Theme {
                canvas_bg: Color32::from_rgb(0xfd, 0xfd, 0xfe),
                header_bg: Color32::from_rgb(0xf5, 0xf7, 0xfa),
                gutter_bg: Color32::from_rgb(0xf8, 0xf9, 0xfb),
                text: Color32::from_rgb(0x1a, 0x20, 0x28),
                muted: Color32::from_rgb(0x6a, 0x74, 0x80),
                grid: Color32::from_rgba_unmultiplied(0x00, 0x00, 0x00, 0x14),
                grid_major: Color32::from_rgba_unmultiplied(0x00, 0x00, 0x00, 0x30),
                bar: Color32::from_rgb(0x4f, 0xa3, 0xff),
                bar_stroke: Color32::from_rgb(0x2d, 0x74, 0xc4),
                bar_done: Color32::from_rgb(0x9a, 0xa6, 0xb2),
                crit: Color32::from_rgb(0xe0, 0x55, 0x55),
                milestone: Color32::from_rgb(0x8a, 0x5c, 0xd6),
                arrow: Color32::from_rgb(0x55, 0x60, 0x70),
                weekend: Color32::from_rgba_unmultiplied(0x00, 0x00, 0x00, 0x0d),
                section_band: Color32::from_rgba_unmultiplied(0x5a, 0x64, 0x70, 0x18),
                today: Color32::from_rgb(0xc7, 0x7d, 0x00),
                progress: Color32::from_rgba_unmultiplied(0x00, 0x00, 0x00, 0x38),
                selection: Color32::from_rgb(0x1f, 0x6f, 0xeb),
                tok_keyword: Color32::from_rgb(0x8f, 0x3f, 0xc8),
                tok_string: Color32::from_rgb(0x2e, 0x7d, 0x32),
                tok_number: Color32::from_rgb(0xb2, 0x60, 0x00),
                tok_date: Color32::from_rgb(0x00, 0x77, 0x8b),
                tok_color: Color32::from_rgb(0x00, 0x77, 0x8b),
                tok_comment: Color32::from_rgb(0x8a, 0x91, 0x99),
                tok_ident: Color32::from_rgb(0x1a, 0x20, 0x28),
                tok_punct: Color32::from_rgb(0x5a, 0x64, 0x70),
                diagnostic: Color32::from_rgb(0xd3, 0x2f, 0x2f),
            }
        }
    }
}
