//! The date↔x mapping. Chart-space x = 0 at the left edge of the padded time
//! range; y = 0 at the top of the first row. The camera and exporter place
//! this space on screen/page.

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TimeScale {
    /// First (padded) day shown.
    pub min_day: i64,
    /// Last (padded) day shown, inclusive.
    pub max_day: i64,
    pub px_per_day: f32,
}

impl TimeScale {
    /// Left edge of `day`.
    pub fn day_to_x(&self, day: i64) -> f32 {
        (day - self.min_day) as f32 * self.px_per_day
    }

    /// Center of `day`.
    pub fn day_center_x(&self, day: i64) -> f32 {
        self.day_to_x(day) + self.px_per_day / 2.0
    }

    /// Day whose column contains `x` (inverse of `day_to_x`).
    pub fn x_to_day(&self, x: f32) -> i64 {
        self.min_day + (x / self.px_per_day).floor() as i64
    }

    /// Total chart width.
    pub fn width(&self) -> f32 {
        (self.max_day - self.min_day + 1) as f32 * self.px_per_day
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roundtrip() {
        let s = TimeScale { min_day: 100, max_day: 130, px_per_day: 26.0 };
        assert_eq!(s.day_to_x(100), 0.0);
        assert_eq!(s.day_to_x(101), 26.0);
        assert_eq!(s.x_to_day(0.0), 100);
        assert_eq!(s.x_to_day(25.9), 100);
        assert_eq!(s.x_to_day(26.0), 101);
        for d in 100..=130 {
            assert_eq!(s.x_to_day(s.day_to_x(d) + 0.1), d);
            assert_eq!(s.x_to_day(s.day_center_x(d)), d);
        }
    }
}
