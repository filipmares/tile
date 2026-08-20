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

    /// Moves and shrinks the rectangle as little as necessary to fit inside
    /// `bounds`: first capped to the bounds' size, then slid back inside them.
    ///
    /// This is what stops an incremental move or resize from pushing a window
    /// off the edge of its display, and what keeps a proportional cross-display
    /// map inside its destination when the source frame overhung its own screen.
    pub fn clamped_within(&self, bounds: Rect) -> Rect {
        let width = self.width.min(bounds.width).max(0.0);
        let height = self.height.min(bounds.height).max(0.0);
        let x = self.x.min(bounds.max_x() - width).max(bounds.x);
        let y = self.y.min(bounds.max_y() - height).max(bounds.y);
        Rect {
            x,
            y,
            width,
            height,
        }
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

    /// Screens ordered left-to-right, then top-to-bottom. Display throws walk
    /// this order rather than the OS enumeration, which is not stable across
    /// reconnects.
    pub fn geometrically_ordered(screens: &[Screen]) -> Vec<&Screen> {
        let mut ordered: Vec<&Screen> = screens.iter().collect();
        ordered.sort_by(|a, b| {
            a.frame
                .x
                .partial_cmp(&b.frame.x)
                .unwrap_or(std::cmp::Ordering::Equal)
                .then(
                    a.frame
                        .y
                        .partial_cmp(&b.frame.y)
                        .unwrap_or(std::cmp::Ordering::Equal),
                )
                .then(a.id.cmp(&b.id))
        });
        ordered
    }

    /// The screen `step` places away from `current` in geometric order,
    /// wrapping at the ends. `step` of `-1` is previous, `+1` is next.
    pub fn adjacent<'a>(screens: &'a [Screen], current: &Screen, step: i32) -> Option<&'a Screen> {
        if screens.is_empty() || step == 0 {
            return None;
        }
        let ordered = Screen::geometrically_ordered(screens);
        let index = ordered.iter().position(|s| s.id == current.id)?;
        let len = ordered.len() as i32;
        let next = (index as i32 + step).rem_euclid(len) as usize;
        Some(ordered[next])
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

    fn screen(id: &str, x: f64, y: f64) -> Screen {
        Screen {
            id: id.into(),
            frame: Rect::new(x, y, 100.0, 100.0),
            work_area: Rect::new(x, y, 100.0, 100.0),
            scale_factor: 1.0,
            is_primary: id == "a",
        }
    }

    /// A display of a given size and backing scale, for the mixed-DPI cases
    /// where the 100x100 squares [`screen`] produces are not enough.
    fn sized_screen(id: &str, frame: Rect, scale: f64) -> Screen {
        Screen {
            id: id.into(),
            frame,
            work_area: frame,
            scale_factor: scale,
            is_primary: false,
        }
    }

    #[test]
    fn adjacent_walks_left_to_right_and_wraps() {
        let screens = vec![
            screen("c", 200.0, 0.0),
            screen("a", 0.0, 0.0),
            screen("b", 100.0, 50.0),
        ];
        let a = screens.iter().find(|s| s.id == "a").unwrap();
        assert_eq!(Screen::adjacent(&screens, a, 1).unwrap().id, "b");
        assert_eq!(Screen::adjacent(&screens, a, -1).unwrap().id, "c");
        let c = screens.iter().find(|s| s.id == "c").unwrap();
        assert_eq!(Screen::adjacent(&screens, c, 1).unwrap().id, "a");
    }

    #[test]
    fn adjacent_is_identity_on_a_single_screen() {
        let screens = vec![screen("only", 0.0, 0.0)];
        assert_eq!(
            Screen::adjacent(&screens, &screens[0], 1).unwrap().id,
            "only"
        );
    }

    #[test]
    fn clamped_within_slides_a_window_back_onto_the_screen() {
        let bounds = Rect::new(0.0, 0.0, 1000.0, 800.0);
        let off_right = Rect::new(950.0, 100.0, 400.0, 200.0).clamped_within(bounds);
        assert_eq!(off_right, Rect::new(600.0, 100.0, 400.0, 200.0));
        let off_top_left = Rect::new(-50.0, -50.0, 100.0, 100.0).clamped_within(bounds);
        assert_eq!(off_top_left, Rect::new(0.0, 0.0, 100.0, 100.0));
    }

    #[test]
    fn clamped_within_caps_an_oversized_window_to_the_bounds() {
        let bounds = Rect::new(10.0, 20.0, 100.0, 100.0);
        let huge = Rect::new(-500.0, -500.0, 5000.0, 5000.0).clamped_within(bounds);
        assert_eq!(huge, bounds);
    }

    /// The three-monitor layout the issue asks for by name, including a
    /// vertically stacked display.
    #[test]
    fn ordering_sorts_left_to_right_then_top_to_bottom() {
        let screens = vec![
            sized_screen(
                "stacked-lower",
                Rect::new(1920.0, 1080.0, 1920.0, 1080.0),
                1.0,
            ),
            sized_screen("stacked-upper", Rect::new(1920.0, 0.0, 1920.0, 1080.0), 1.0),
            sized_screen("laptop", Rect::new(0.0, 0.0, 1920.0, 1080.0), 1.0),
        ];
        let ordered = Screen::geometrically_ordered(&screens);
        let ids: Vec<&str> = ordered.iter().map(|s| s.id.as_str()).collect();
        assert_eq!(ids, ["laptop", "stacked-upper", "stacked-lower"]);
    }

    /// The reason the order is geometric rather than the OS enumeration: the
    /// same desk must produce the same order however the backend lists it.
    #[test]
    fn ordering_is_stable_regardless_of_enumeration_order() {
        let a = sized_screen("a", Rect::new(0.0, 0.0, 100.0, 100.0), 1.0);
        let b = sized_screen("b", Rect::new(100.0, 0.0, 100.0, 100.0), 1.0);
        let ascending = [a.clone(), b.clone()];
        let descending = [b, a];
        let forwards = Screen::geometrically_ordered(&ascending);
        let backwards = Screen::geometrically_ordered(&descending);
        assert_eq!(
            forwards.iter().map(|s| &s.id).collect::<Vec<_>>(),
            backwards.iter().map(|s| &s.id).collect::<Vec<_>>()
        );
    }

    #[test]
    fn ordering_breaks_origin_ties_by_id() {
        // Mirrored displays share an origin; the id keeps the order total,
        // which is what makes next and previous exact inverses.
        let screens = vec![
            sized_screen("z", Rect::new(0.0, 0.0, 100.0, 100.0), 1.0),
            sized_screen("a", Rect::new(0.0, 0.0, 100.0, 100.0), 1.0),
        ];
        let ordered = Screen::geometrically_ordered(&screens);
        assert_eq!(ordered[0].id, "a");
        assert_eq!(ordered[1].id, "z");
    }
}
