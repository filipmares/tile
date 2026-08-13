//! The set of window actions Tile can perform, and the pure geometry that
//! turns an action into a target rectangle.

use serde::{Deserialize, Serialize};
use std::fmt;
use std::str::FromStr;

use crate::config::{Gaps, SharedEdges};
use crate::geometry::Rect;

/// The family a [`WindowAction`] belongs to.
///
/// Families exist purely for presentation: the tray menu renders one submenu
/// per family and the settings window renders one group per family, which
/// keeps ~45 actions navigable instead of dumping them into one flat list.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum WindowFamily {
    Halves,
    Corners,
    HorizontalThirds,
    VerticalThirds,
    Fourths,
    CornerThirds,
    Sixths,
    Ninths,
    Sizing,
}

impl WindowFamily {
    /// Families in the order they should appear in the UI.
    pub const ALL: [WindowFamily; 9] = [
        WindowFamily::Halves,
        WindowFamily::Corners,
        WindowFamily::HorizontalThirds,
        WindowFamily::VerticalThirds,
        WindowFamily::Fourths,
        WindowFamily::CornerThirds,
        WindowFamily::Sixths,
        WindowFamily::Ninths,
        WindowFamily::Sizing,
    ];

    /// Human-readable heading for menus and the settings window.
    pub const fn label(self) -> &'static str {
        match self {
            WindowFamily::Halves => "Halves",
            WindowFamily::Corners => "Corners",
            WindowFamily::HorizontalThirds => "Horizontal Thirds",
            WindowFamily::VerticalThirds => "Vertical Thirds",
            WindowFamily::Fourths => "Fourths",
            WindowFamily::CornerThirds => "Corner Thirds",
            WindowFamily::Sixths => "Sixths",
            WindowFamily::Ninths => "Ninths",
            WindowFamily::Sizing => "Size & Position",
        }
    }

    /// The actions belonging to this family, in UI order.
    pub fn actions(self) -> impl Iterator<Item = WindowAction> {
        WindowAction::ALL
            .iter()
            .copied()
            .filter(move |a| a.family() == self)
    }
}

/// Every window action supported by Tile.
///
/// New variants are additive: a variant only needs a `target_rect` arm, an
/// [`WindowAction::ALL`] entry, an [`WindowAction::id`], an
/// [`WindowAction::label`] and a [`WindowAction::family`]. Default hotkeys are
/// optional — most of the catalogue ships unbound.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum WindowAction {
    LeftHalf,
    RightHalf,
    TopHalf,
    BottomHalf,
    TopLeft,
    TopRight,
    BottomLeft,
    BottomRight,
    Maximize,
    Center,
    Restore,
}

impl WindowAction {
    /// All actions, grouped by [`WindowFamily`] in the order they appear in
    /// the UI.
    pub const ALL: [WindowAction; 11] = [
        // Halves
        WindowAction::LeftHalf,
        WindowAction::RightHalf,
        WindowAction::TopHalf,
        WindowAction::BottomHalf,
        // Corners
        WindowAction::TopLeft,
        WindowAction::TopRight,
        WindowAction::BottomLeft,
        WindowAction::BottomRight,
        // Sizing
        WindowAction::Maximize,
        WindowAction::Center,
        WindowAction::Restore,
    ];

    /// Stable machine-readable identifier, also used as the JSON config key.
    ///
    /// These strings are the config format's keys. Once published they are
    /// effectively permanent: never rename or renumber an existing one.
    pub const fn id(self) -> &'static str {
        match self {
            WindowAction::LeftHalf => "left-half",
            WindowAction::RightHalf => "right-half",
            WindowAction::TopHalf => "top-half",
            WindowAction::BottomHalf => "bottom-half",
            WindowAction::TopLeft => "top-left",
            WindowAction::TopRight => "top-right",
            WindowAction::BottomLeft => "bottom-left",
            WindowAction::BottomRight => "bottom-right",
            WindowAction::Maximize => "maximize",
            WindowAction::Center => "center",
            WindowAction::Restore => "restore",
        }
    }

    /// Human-readable label for menus and the settings window.
    pub const fn label(self) -> &'static str {
        match self {
            WindowAction::LeftHalf => "Left Half",
            WindowAction::RightHalf => "Right Half",
            WindowAction::TopHalf => "Top Half",
            WindowAction::BottomHalf => "Bottom Half",
            WindowAction::TopLeft => "Top Left",
            WindowAction::TopRight => "Top Right",
            WindowAction::BottomLeft => "Bottom Left",
            WindowAction::BottomRight => "Bottom Right",
            WindowAction::Maximize => "Maximize",
            WindowAction::Center => "Center",
            WindowAction::Restore => "Restore",
        }
    }

    /// The presentation family this action belongs to.
    pub const fn family(self) -> WindowFamily {
        match self {
            WindowAction::LeftHalf
            | WindowAction::RightHalf
            | WindowAction::TopHalf
            | WindowAction::BottomHalf => WindowFamily::Halves,
            WindowAction::TopLeft
            | WindowAction::TopRight
            | WindowAction::BottomLeft
            | WindowAction::BottomRight => WindowFamily::Corners,
            WindowAction::Maximize | WindowAction::Center | WindowAction::Restore => {
                WindowFamily::Sizing
            }
        }
    }

    /// `Restore` is handled by the window-history layer rather than by pure
    /// geometry, so it has no computable target rectangle.
    pub const fn uses_history(self) -> bool {
        matches!(self, WindowAction::Restore)
    }

    /// Computes the destination rectangle for this action within `work_area`.
    ///
    /// `gaps` describes the window gap and the per-side screen-edge gaps;
    /// `main_screen` is whether `work_area` belongs to the primary display,
    /// which matters when [`Gaps::main_screen_only`] is set. Returns `None` for
    /// actions that are not expressible as pure geometry (currently only
    /// [`WindowAction::Restore`]).
    pub fn target_rect(
        self,
        work_area: Rect,
        gaps: &Gaps,
        current: Rect,
        main_screen: bool,
    ) -> Option<Rect> {
        let a = work_area;

        let rect = match self {
            WindowAction::LeftHalf => grid(a, gaps, main_screen, (0.0, 0.5), (0.0, 1.0)),
            WindowAction::RightHalf => grid(a, gaps, main_screen, (0.5, 1.0), (0.0, 1.0)),
            WindowAction::TopHalf => grid(a, gaps, main_screen, (0.0, 1.0), (0.0, 0.5)),
            WindowAction::BottomHalf => grid(a, gaps, main_screen, (0.0, 1.0), (0.5, 1.0)),
            WindowAction::TopLeft => grid(a, gaps, main_screen, (0.0, 0.5), (0.0, 0.5)),
            WindowAction::TopRight => grid(a, gaps, main_screen, (0.5, 1.0), (0.0, 0.5)),
            WindowAction::BottomLeft => grid(a, gaps, main_screen, (0.0, 0.5), (0.5, 1.0)),
            WindowAction::BottomRight => grid(a, gaps, main_screen, (0.5, 1.0), (0.5, 1.0)),
            WindowAction::Maximize => grid(a, gaps, main_screen, (0.0, 1.0), (0.0, 1.0)),
            WindowAction::Center => {
                // Centering preserves the window's current size, clamped to the
                // work area so it can never end up larger than the screen. It
                // deliberately ignores gaps, matching Rectangle.
                let w = current.width.min(a.width);
                let h = current.height.min(a.height);
                return Some(
                    Rect::new(a.x + (a.width - w) / 2.0, a.y + (a.height - h) / 2.0, w, h)
                        .rounded(),
                );
            }
            WindowAction::Restore => return None,
        };

        Some(rect.rounded())
    }
}

/// Builds a grid cell spanning the given column and row fractions of
/// `work_area`, applying the gap model. An edge whose fraction is exactly 0 or
/// 1 lies against the screen and receives a screen-edge gap; any other edge is
/// shared with a neighbour and receives half the window gap, so two adjacent
/// cells are separated by exactly one window gap.
fn grid(area: Rect, gaps: &Gaps, main_screen: bool, cols: (f64, f64), rows: (f64, f64)) -> Rect {
    let (c0, c1) = cols;
    let (r0, r1) = rows;
    let raw = Rect::new(
        area.x + area.width * c0,
        area.y + area.height * r0,
        area.width * (c1 - c0),
        area.height * (r1 - r0),
    );
    let shared = SharedEdges {
        left: c0 != 0.0,
        right: c1 != 1.0,
        top: r0 != 0.0,
        bottom: r1 != 1.0,
    };
    gaps.apply(raw, shared, main_screen)
}

impl fmt::Display for WindowAction {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.id())
    }
}

/// Error returned when parsing an unknown action identifier.
#[derive(Debug, thiserror::Error)]
#[error("unknown window action: {0}")]
pub struct ParseActionError(pub String);

impl FromStr for WindowAction {
    type Err = ParseActionError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        WindowAction::ALL
            .iter()
            .copied()
            .find(|a| a.id() == s)
            .ok_or_else(|| ParseActionError(s.to_string()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const AREA: Rect = Rect::new(0.0, 0.0, 1920.0, 1040.0);
    const CURRENT: Rect = Rect::new(300.0, 300.0, 800.0, 600.0);
    const NO_GAPS: Gaps = Gaps {
        window: 0.0,
        edge_top: 0.0,
        edge_bottom: 0.0,
        edge_left: 0.0,
        edge_right: 0.0,
        skip_top_edge: false,
        main_screen_only: false,
    };

    /// A work area with a non-zero origin, simulating a taskbar on the left
    /// edge of a secondary monitor.
    const OFFSET_AREA: Rect = Rect::new(1980.0, 30.0, 1860.0, 1010.0);

    /// A 10px window gap with 20px screen-edge gaps on every side, to exercise
    /// the screen-edge vs shared-edge distinction.
    const GAPPY: Gaps = Gaps {
        window: 10.0,
        edge_top: 20.0,
        edge_bottom: 20.0,
        edge_left: 20.0,
        edge_right: 20.0,
        skip_top_edge: false,
        main_screen_only: false,
    };

    fn rect(action: WindowAction, area: Rect, gaps: &Gaps) -> Rect {
        action.target_rect(area, gaps, CURRENT, true).unwrap()
    }

    /// Asserts that `members` exactly cover `area`: no gaps between them, no
    /// overflow past the edges, and no overlap. Because [`Rect::rounded`] keeps
    /// complementary edges flush, this holds even on odd pixel dimensions.
    fn assert_tiles_exactly(members: &[Rect], area: Rect) {
        let mut sum = 0.0;
        for (i, m) in members.iter().enumerate() {
            assert!(
                m.x >= area.x && m.y >= area.y,
                "member {m:?} starts before area {area:?}"
            );
            assert!(
                m.max_x() <= area.max_x() && m.max_y() <= area.max_y(),
                "member {m:?} overflows area {area:?}"
            );
            sum += m.width * m.height;
            for n in &members[i + 1..] {
                assert_eq!(
                    m.intersection_area(n),
                    0.0,
                    "members overlap: {m:?} and {n:?}"
                );
            }
        }
        assert_eq!(
            sum,
            area.width * area.height,
            "members leave a seam or overflow {area:?}"
        );
    }

    #[test]
    fn halves_tile_the_work_area_exactly() {
        let l = rect(WindowAction::LeftHalf, AREA, &NO_GAPS);
        let r = rect(WindowAction::RightHalf, AREA, &NO_GAPS);
        assert_eq!(l, Rect::new(0.0, 0.0, 960.0, 1040.0));
        assert_eq!(r, Rect::new(960.0, 0.0, 960.0, 1040.0));
        assert_eq!(l.max_x(), r.x);
        assert_eq!(l.width + r.width, AREA.width);
    }

    #[test]
    fn odd_width_halves_leave_no_seam_and_no_overflow() {
        let area = Rect::new(0.0, 0.0, 1367.0, 768.0);
        let l = rect(WindowAction::LeftHalf, area, &NO_GAPS);
        let r = rect(WindowAction::RightHalf, area, &NO_GAPS);
        assert_eq!(l.max_x(), r.x);
        assert_eq!(r.max_x(), area.max_x());
    }

    #[test]
    fn work_area_offset_is_respected() {
        // Simulates a taskbar on the left and a second monitor origin.
        let area = Rect::new(1920.0 + 60.0, 30.0, 1860.0, 1010.0);
        let l = rect(WindowAction::LeftHalf, area, &NO_GAPS);
        assert_eq!(l.x, 1980.0);
        assert_eq!(l.y, 30.0);
        assert_eq!(l.width, 930.0);
    }

    #[test]
    fn maximize_fills_work_area() {
        let m = rect(WindowAction::Maximize, AREA, &NO_GAPS);
        assert_eq!(m, AREA);
    }

    #[test]
    fn corners_are_exact_quarters() {
        let tl = rect(WindowAction::TopLeft, AREA, &NO_GAPS);
        let tr = rect(WindowAction::TopRight, AREA, &NO_GAPS);
        let bl = rect(WindowAction::BottomLeft, AREA, &NO_GAPS);
        let br = rect(WindowAction::BottomRight, AREA, &NO_GAPS);
        assert_eq!(tl, Rect::new(0.0, 0.0, 960.0, 520.0));
        assert_eq!(tr, Rect::new(960.0, 0.0, 960.0, 520.0));
        assert_eq!(bl, Rect::new(0.0, 520.0, 960.0, 520.0));
        assert_eq!(br, Rect::new(960.0, 520.0, 960.0, 520.0));
        // Adjacent corners share an edge exactly.
        assert_eq!(tl.max_x(), tr.x);
        assert_eq!(tl.max_y(), bl.y);
        assert_eq!(br.x, bl.max_x());
        assert_eq!(br.y, tr.max_y());
    }

    #[test]
    fn corners_tile_with_no_seam_on_odd_dimensions() {
        let area = Rect::new(0.0, 0.0, 1367.0, 769.0);
        let members = [
            rect(WindowAction::TopLeft, area, &NO_GAPS),
            rect(WindowAction::TopRight, area, &NO_GAPS),
            rect(WindowAction::BottomLeft, area, &NO_GAPS),
            rect(WindowAction::BottomRight, area, &NO_GAPS),
        ];
        assert_tiles_exactly(&members, area);
    }

    #[test]
    fn corners_respect_work_area_offset() {
        let tl = rect(WindowAction::TopLeft, OFFSET_AREA, &NO_GAPS);
        assert_eq!(tl, Rect::new(1980.0, 30.0, 930.0, 505.0));
        let br = rect(WindowAction::BottomRight, OFFSET_AREA, &NO_GAPS);
        assert_eq!(br.max_x(), OFFSET_AREA.max_x());
        assert_eq!(br.max_y(), OFFSET_AREA.max_y());
    }

    #[test]
    fn corners_apply_screen_and_shared_edge_gaps() {
        let tl = rect(WindowAction::TopLeft, AREA, &GAPPY);
        let tr = rect(WindowAction::TopRight, AREA, &GAPPY);
        let bl = rect(WindowAction::BottomLeft, AREA, &GAPPY);
        // Outer edges get the full 20px screen gap.
        assert_eq!(tl.x, 20.0);
        assert_eq!(tl.y, 20.0);
        // Shared inner edges are separated by exactly one window gap.
        assert_eq!(tr.x - tl.max_x(), GAPPY.window);
        assert_eq!(bl.y - tl.max_y(), GAPPY.window);
        assert_eq!(tr.max_x(), AREA.width - 20.0);
    }

    #[test]
    fn uniform_gap_insets_every_screen_edge() {
        // A uniform gap (window == edges) behaves like the old scalar for
        // Maximize: every screen edge is inset by the same amount.
        let m = rect(WindowAction::Maximize, AREA, &Gaps::uniform(10.0));
        assert_eq!(m, Rect::new(10.0, 10.0, 1900.0, 1020.0));
    }

    #[test]
    fn adjacent_halves_are_separated_by_exactly_one_window_gap() {
        // Window gap 10, screen-edge gaps 20 on every side.
        let gaps = Gaps {
            window: 10.0,
            edge_top: 20.0,
            edge_bottom: 20.0,
            edge_left: 20.0,
            edge_right: 20.0,
            ..NO_GAPS
        };
        let l = rect(WindowAction::LeftHalf, AREA, &gaps);
        let r = rect(WindowAction::RightHalf, AREA, &gaps);

        // Full edge gap on the outer edges.
        assert_eq!(l.x, 20.0);
        assert_eq!(l.y, 20.0);
        assert_eq!(r.max_x(), AREA.width - 20.0);
        // Exactly one window gap between the two halves, not two.
        assert_eq!(r.x - l.max_x(), gaps.window);
    }

    #[test]
    fn skip_top_edge_drops_only_the_top_screen_gap() {
        let gaps = Gaps {
            window: 0.0,
            edge_top: 20.0,
            edge_bottom: 20.0,
            edge_left: 20.0,
            edge_right: 20.0,
            skip_top_edge: true,
            main_screen_only: false,
        };
        let m = rect(WindowAction::Maximize, AREA, &gaps);
        assert_eq!(m.y, 0.0, "top gap is skipped");
        assert_eq!(m.max_y(), AREA.height - 20.0, "bottom gap still applies");
        assert_eq!(m.x, 20.0);
    }

    #[test]
    fn main_screen_only_suppresses_edge_gaps_off_primary() {
        let gaps = Gaps {
            window: 10.0,
            edge_top: 20.0,
            edge_bottom: 20.0,
            edge_left: 20.0,
            edge_right: 20.0,
            skip_top_edge: false,
            main_screen_only: true,
        };
        // On a secondary display, screen-edge gaps vanish but the window gap
        // between two halves is preserved.
        let l = WindowAction::LeftHalf
            .target_rect(AREA, &gaps, CURRENT, false)
            .unwrap();
        let r = WindowAction::RightHalf
            .target_rect(AREA, &gaps, CURRENT, false)
            .unwrap();
        assert_eq!(l.x, 0.0);
        assert_eq!(l.y, 0.0);
        assert_eq!(r.max_x(), AREA.width);
        assert_eq!(r.x - l.max_x(), gaps.window);
    }

    #[test]
    fn gaps_never_shrink_a_window_below_zero() {
        let tiny = Rect::new(0.0, 0.0, 10.0, 10.0);
        let m = WindowAction::Maximize
            .target_rect(tiny, &Gaps::uniform(50.0), CURRENT, true)
            .unwrap();
        assert!(m.width >= 0.0);
        assert!(m.height >= 0.0);
    }

    #[test]
    fn center_preserves_size_and_centers() {
        let c = rect(WindowAction::Center, AREA, &NO_GAPS);
        assert_eq!(c.width, 800.0);
        assert_eq!(c.height, 600.0);
        assert_eq!(c.center(), AREA.center());
    }

    #[test]
    fn center_clamps_oversized_windows() {
        let huge = Rect::new(0.0, 0.0, 5000.0, 5000.0);
        let c = WindowAction::Center
            .target_rect(AREA, &NO_GAPS, huge, true)
            .unwrap();
        assert_eq!(c, AREA);
    }

    #[test]
    fn restore_has_no_geometry() {
        assert!(WindowAction::Restore
            .target_rect(AREA, &NO_GAPS, CURRENT, true)
            .is_none());
        assert!(WindowAction::Restore.uses_history());
    }

    #[test]
    fn ids_round_trip_and_are_unique() {
        let mut ids: Vec<_> = WindowAction::ALL.iter().map(|a| a.id()).collect();
        ids.sort_unstable();
        let before = ids.len();
        ids.dedup();
        assert_eq!(before, ids.len(), "action ids must be unique");

        for a in WindowAction::ALL {
            assert_eq!(a.id().parse::<WindowAction>().unwrap(), a);
        }
        assert!("nope".parse::<WindowAction>().is_err());
    }

    #[test]
    fn serde_repr_matches_id() {
        // The config format keys and the TS mirror rely on serde's kebab-case
        // matching id() exactly.
        for a in WindowAction::ALL {
            let json = serde_json::to_string(&a).unwrap();
            assert_eq!(json, format!("\"{}\"", a.id()));
        }
    }
}
