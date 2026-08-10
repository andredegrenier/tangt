//! Axis-locked Gantt camera. Time (x) pans and zooms; rows (y) only scroll —
//! row height and fonts stay constant on screen, the standard Gantt UX.

use eframe::egui::Rect;
use tangt_core::geometry::PX_PER_DAY;

pub const MIN_ZOOM: f32 = 0.08;
pub const MAX_ZOOM: f32 = 8.0;

#[derive(Clone, Copy, serde::Serialize, serde::Deserialize)]
pub struct GanttCamera {
    /// Fractional day number at the left edge of the chart region.
    pub left_day: f64,
    /// Horizontal zoom; effective px/day = `PX_PER_DAY * zoom`.
    pub zoom: f32,
    /// Vertical scroll offset in pixels.
    pub y_scroll: f32,
}

impl Default for GanttCamera {
    fn default() -> Self {
        GanttCamera { left_day: 0.0, zoom: 1.0, y_scroll: 0.0 }
    }
}

impl GanttCamera {
    pub fn px_per_day(&self) -> f32 {
        PX_PER_DAY * self.zoom
    }

    /// Screen x of (the left edge of) a fractional day, given the chart
    /// region's left edge.
    pub fn day_to_screen_x(&self, day: f64, chart_left: f32) -> f32 {
        chart_left + ((day - self.left_day) as f32) * self.px_per_day()
    }

    /// Fractional day at a screen x.
    pub fn screen_x_to_day(&self, x: f32, chart_left: f32) -> f64 {
        self.left_day + ((x - chart_left) / self.px_per_day()) as f64
    }

    /// Whole day whose column contains screen x.
    pub fn day_at(&self, x: f32, chart_left: f32) -> i64 {
        self.screen_x_to_day(x, chart_left).floor() as i64
    }

    pub fn pan_days(&mut self, screen_dx: f32) {
        self.left_day -= (screen_dx / self.px_per_day()) as f64;
    }

    pub fn scroll(&mut self, screen_dy: f32, content_h: f32, viewport_h: f32) {
        self.y_scroll =
            (self.y_scroll - screen_dy).clamp(0.0, (content_h - viewport_h).max(0.0));
    }

    /// Multiply zoom by `factor`, keeping the day under `anchor_x` fixed.
    pub fn zoom_at(&mut self, factor: f32, anchor_x: f32, chart_left: f32) {
        let day = self.screen_x_to_day(anchor_x, chart_left);
        self.zoom = (self.zoom * factor).clamp(MIN_ZOOM, MAX_ZOOM);
        self.left_day = day - ((anchor_x - chart_left) / self.px_per_day()) as f64;
    }

    /// Fit a day range into the chart region's width.
    pub fn fit(&mut self, min_day: i64, max_day: i64, viewport: Rect) {
        let days = ((max_day - min_day + 1) as f32).max(1.0);
        self.zoom = (viewport.width() / days / PX_PER_DAY * 0.94).clamp(MIN_ZOOM, MAX_ZOOM);
        self.left_day = min_day as f64 - (days * 0.03) as f64;
        self.y_scroll = 0.0;
    }
}
