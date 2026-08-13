//! Screen-space geometry primitives.
//!
//! All rectangles use a top-left origin with y growing downwards. Platform
//! backends are responsible for converting to and from their native coordinate
//! space (macOS' Accessibility API already uses a top-left origin, whereas
//! `NSScreen` does not).

use serde::{Deserialize, Serialize};

/// A rectangle in logical (device-independent) pixels.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Rect {
    pub x: f64,
    pub y: f64,
    pub width: f64,
    pub height: f64,
}

impl Rect {
    pub const fn new(x: f64, y: f64, width: f64, height: f64) -> Self {
        Self {
            x,
            y,
            width,
            height,
        }
    }

    pub fn max_x(&self) -> f64 {
        self.x + self.width
    }

    pub fn max_y(&self) -> f64 {
        self.y + self.height
    }

    pub fn center(&self) -> (f64, f64) {
        (self.x + self.width / 2.0, self.y + self.height / 2.0)
    }

    pub fn is_empty(&self) -> bool {
        self.width <= 0.0 || self.height <= 0.0
    }

    /// Rounds all edges to whole pixels, preserving the right and bottom edges
    /// so that complementary halves stay flush against each other.
    pub fn rounded(&self) -> Rect {
        let x = self.x.round();
        let y = self.y.round();
        Rect {
            x,
            y,
            width: self.max_x().round() - x,
            height: self.max_y().round() - y,
        }
    }

    /// True when the two rectangles are equal within `tolerance` on every edge.
    pub fn approx_eq(&self, other: &Rect, tolerance: f64) -> bool {
        (self.x - other.x).abs() <= tolerance
            && (self.y - other.y).abs() <= tolerance
            && (self.width - other.width).abs() <= tolerance
            && (self.height - other.height).abs() <= tolerance
    }

    /// Shrinks the rectangle by `gap` on every side, never below zero size.
    pub fn inset(&self, gap: f64) -> Rect {
        if gap <= 0.0 {
            return *self;
        }
        Rect {
            x: self.x + gap,
            y: self.y + gap,
            width: (self.width - gap * 2.0).max(0.0),
            height: (self.height - gap * 2.0).max(0.0),
        }
    }

    /// Shrinks each edge independently by the given amount, never below zero
    /// size. Negative insets are treated as zero so a gap can only ever shrink
    /// a window, never grow it past its allotted cell.
    ///
    /// This is the primitive behind the [`crate::config::Gaps`] model, where a
    /// screen-edge inset and a (halved) window-gap inset differ per edge.
    pub fn inset_edges(&self, left: f64, top: f64, right: f64, bottom: f64) -> Rect {
        let left = left.max(0.0);
        let top = top.max(0.0);
        let right = right.max(0.0);
        let bottom = bottom.max(0.0);
        Rect {
            x: self.x + left,
            y: self.y + top,
            width: (self.width - left - right).max(0.0),
            height: (self.height - top - bottom).max(0.0),
        }
    }

    /// Area of intersection with `other`. Used to pick the screen a window
    /// belongs to.
    pub fn intersection_area(&self, other: &Rect) -> f64 {
        let w = (self.max_x().min(other.max_x()) - self.x.max(other.x)).max(0.0);
        let h = (self.max_y().min(other.max_y()) - self.y.max(other.y)).max(0.0);
        w * h
    }
}

/// A display, as reported by the platform backend.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Screen {
    /// Stable, backend-specific identifier for the display.
    pub id: String,
    /// Full display bounds.
    pub frame: Rect,
    /// Bounds excluding OS chrome (taskbar, menu bar, dock).
    pub work_area: Rect,
    /// Backing scale factor, e.g. 2.0 on a Retina or 200% display.
    pub scale_factor: f64,
    pub is_primary: bool,
}

impl Screen {
    /// Picks the screen that contains the largest portion of `rect`, falling
    /// back to the primary screen and then to the first screen in the list.
    pub fn best_match<'a>(screens: &'a [Screen], rect: &Rect) -> Option<&'a Screen> {
        screens
            .iter()
            .filter(|s| s.frame.intersection_area(rect) > 0.0)
            .max_by(|a, b| {
                let aa = a.frame.intersection_area(rect);
                let ba = b.frame.intersection_area(rect);
                aa.partial_cmp(&ba).unwrap_or(std::cmp::Ordering::Equal)
            })
            .or_else(|| screens.iter().find(|s| s.is_primary))
            .or_else(|| screens.first())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rounded_keeps_edges_flush() {
        let left = Rect::new(0.0, 0.0, 683.5, 768.0).rounded();
        let right = Rect::new(683.5, 0.0, 683.5, 768.0).rounded();
        assert_eq!(left.max_x(), right.x, "halves must not leave a seam");
        assert_eq!(right.max_x(), 1367.0);
    }

    #[test]
    fn inset_never_goes_negative() {
        let r = Rect::new(0.0, 0.0, 10.0, 10.0).inset(50.0);
        assert_eq!(r.width, 0.0);
        assert_eq!(r.height, 0.0);
    }

    #[test]
    fn inset_edges_shrinks_each_side_independently() {
        let r = Rect::new(0.0, 0.0, 100.0, 100.0).inset_edges(10.0, 20.0, 5.0, 15.0);
        assert_eq!(r, Rect::new(10.0, 20.0, 85.0, 65.0));
    }

    #[test]
    fn inset_edges_never_goes_negative() {
        let r = Rect::new(0.0, 0.0, 10.0, 10.0).inset_edges(30.0, 30.0, 30.0, 30.0);
        assert_eq!(r.width, 0.0);
        assert_eq!(r.height, 0.0);
        // Negative insets are clamped to zero rather than growing the window.
        let r = Rect::new(0.0, 0.0, 10.0, 10.0).inset_edges(-5.0, -5.0, 0.0, 0.0);
        assert_eq!(r, Rect::new(0.0, 0.0, 10.0, 10.0));
    }

    #[test]
    fn best_match_picks_largest_overlap() {
        let screens = vec![
            Screen {
                id: "a".into(),
                frame: Rect::new(0.0, 0.0, 1000.0, 1000.0),
                work_area: Rect::new(0.0, 0.0, 1000.0, 1000.0),
                scale_factor: 1.0,
                is_primary: true,
            },
            Screen {
                id: "b".into(),
                frame: Rect::new(1000.0, 0.0, 1000.0, 1000.0),
                work_area: Rect::new(1000.0, 0.0, 1000.0, 1000.0),
                scale_factor: 1.0,
                is_primary: false,
            },
        ];
        // Window straddles both screens but is mostly on "b".
        let win = Rect::new(900.0, 0.0, 400.0, 100.0);
        assert_eq!(Screen::best_match(&screens, &win).unwrap().id, "b");
    }

    #[test]
    fn best_match_falls_back_to_primary_when_offscreen() {
        let screens = vec![Screen {
            id: "a".into(),
            frame: Rect::new(0.0, 0.0, 100.0, 100.0),
            work_area: Rect::new(0.0, 0.0, 100.0, 100.0),
            scale_factor: 1.0,
            is_primary: true,
        }];
        let win = Rect::new(-5000.0, -5000.0, 10.0, 10.0);
        assert_eq!(Screen::best_match(&screens, &win).unwrap().id, "a");
    }
}
