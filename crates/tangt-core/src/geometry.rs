//! Shared primitives: rectangles, colors, text measurement. Text measurement
//! is abstracted so core stays independent of egui; the app implements
//! `TextMeasurer` with real font metrics, tests use a stub.

pub const BASE_FONT_SIZE: f32 = 13.0;
/// World pixels per day at zoom 1.0.
pub const PX_PER_DAY: f32 = 26.0;
pub const ROW_H: f32 = 28.0;
pub const HEADER_H: f32 = 44.0;
/// Vertical inset of a bar within its row.
pub const BAR_INSET: f32 = 5.0;
pub const GUTTER_PAD: f32 = 12.0;
pub const MIN_GUTTER_W: f32 = 120.0;
pub const MAX_GUTTER_W: f32 = 320.0;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Rgba(pub [u8; 4]);

impl Rgba {
    pub const fn rgb(r: u8, g: u8, b: u8) -> Self {
        Rgba([r, g, b, 255])
    }
    #[allow(clippy::self_named_constructors)]
    pub const fn rgba(r: u8, g: u8, b: u8, a: u8) -> Self {
        Rgba([r, g, b, a])
    }
    pub const fn from_rgb3(rgb: [u8; 3]) -> Self {
        Rgba([rgb[0], rgb[1], rgb[2], 255])
    }
    /// Same color with a different alpha.
    pub const fn with_alpha(self, a: u8) -> Self {
        Rgba([self.0[0], self.0[1], self.0[2], a])
    }
}

/// Black or white, whichever is readable on `fill` (relative-luminance test).
/// Used for bar labels on screen and in every export.
pub fn contrast_text(fill: Rgba) -> Rgba {
    let [r, g, b, _] = fill.0;
    let luma = 0.299 * r as f32 + 0.587 * g as f32 + 0.114 * b as f32;
    if luma > 150.0 {
        Rgba::rgb(0x1a, 0x20, 0x28)
    } else {
        Rgba::rgb(0xff, 0xff, 0xff)
    }
}

pub trait TextMeasurer {
    /// Width/height of a single line of text at the given font size.
    fn measure(&self, text: &str, size: f32) -> [f32; 2];
}

/// Fixed-metric measurer for tests and headless use.
pub struct MonoMeasurer;

impl TextMeasurer for MonoMeasurer {
    fn measure(&self, text: &str, size: f32) -> [f32; 2] {
        [text.chars().count() as f32 * size * 0.6, size * 1.3]
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Rect {
    pub min: [f32; 2],
    pub max: [f32; 2],
}

impl Rect {
    pub const NOTHING: Rect = Rect { min: [f32::INFINITY; 2], max: [f32::NEG_INFINITY; 2] };

    pub fn new(min: [f32; 2], max: [f32; 2]) -> Self {
        Rect { min, max }
    }
    pub fn from_min_size(min: [f32; 2], size: [f32; 2]) -> Self {
        Rect { min, max: [min[0] + size[0], min[1] + size[1]] }
    }
    pub fn from_center_size(center: [f32; 2], size: [f32; 2]) -> Self {
        Rect {
            min: [center[0] - size[0] / 2.0, center[1] - size[1] / 2.0],
            max: [center[0] + size[0] / 2.0, center[1] + size[1] / 2.0],
        }
    }
    pub fn union(self, other: Rect) -> Rect {
        Rect {
            min: [self.min[0].min(other.min[0]), self.min[1].min(other.min[1])],
            max: [self.max[0].max(other.max[0]), self.max[1].max(other.max[1])],
        }
    }
    pub fn expand(self, by: f32) -> Rect {
        Rect {
            min: [self.min[0] - by, self.min[1] - by],
            max: [self.max[0] + by, self.max[1] + by],
        }
    }
    pub fn center(&self) -> [f32; 2] {
        [(self.min[0] + self.max[0]) / 2.0, (self.min[1] + self.max[1]) / 2.0]
    }
    pub fn size(&self) -> [f32; 2] {
        [self.max[0] - self.min[0], self.max[1] - self.min[1]]
    }
    pub fn width(&self) -> f32 {
        self.max[0] - self.min[0]
    }
    pub fn height(&self) -> f32 {
        self.max[1] - self.min[1]
    }
    pub fn contains(&self, p: [f32; 2]) -> bool {
        p[0] >= self.min[0] && p[0] <= self.max[0] && p[1] >= self.min[1] && p[1] <= self.max[1]
    }
    pub fn is_finite(&self) -> bool {
        self.min[0].is_finite() && self.max[0].is_finite()
    }
}
