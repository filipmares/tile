//! The set of window actions Tile can perform, and the pure geometry that
//! turns an action into a target rectangle.

use serde::{Deserialize, Serialize};
use std::fmt;
use std::str::FromStr;

use crate::geometry::Rect;

/// Every window action supported by Tile.
///
/// The MVP deliberately ships halves, maximize and restore only; new variants
/// are additive and only need a `target_rect` arm plus a default hotkey.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum WindowAction {
    LeftHalf,
    RightHalf,
    TopHalf,
    BottomHalf,
    Maximize,
    Center,
    Restore,
}

impl WindowAction {
    /// All actions, in the order they should appear in the UI.
    pub const ALL: [WindowAction; 7] = [
        WindowAction::LeftHalf,
        WindowAction::RightHalf,
        WindowAction::TopHalf,
        WindowAction::BottomHalf,
        WindowAction::Maximize,
        WindowAction::Center,
        WindowAction::Restore,
    ];

    /// Stable machine-readable identifier, also used as the JSON config key.
    pub const fn id(self) -> &'static str {
        match self {
            WindowAction::LeftHalf => "left-half",
            WindowAction::RightHalf => "right-half",
            WindowAction::TopHalf => "top-half",
            WindowAction::BottomHalf => "bottom-half",
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
            WindowAction::Maximize => "Maximize",
            WindowAction::Center => "Center",
            WindowAction::Restore => "Restore",
        }
    }

    /// `Restore` is handled by the window-history layer rather than by pure
    /// geometry, so it has no computable target rectangle.
    pub const fn uses_history(self) -> bool {
        matches!(self, WindowAction::Restore)
    }

    /// Computes the destination rectangle for this action within `work_area`.
    ///
    /// `gap` is the padding applied between the window and the screen edges.
    /// Returns `None` for actions that are not expressible as pure geometry
    /// (currently only [`WindowAction::Restore`]).
    pub fn target_rect(self, work_area: Rect, gap: f64, current: Rect) -> Option<Rect> {
        let a = work_area;
        let half_w = a.width / 2.0;
        let half_h = a.height / 2.0;

        let rect = match self {
            WindowAction::LeftHalf => Rect::new(a.x, a.y, half_w, a.height),
            WindowAction::RightHalf => Rect::new(a.x + half_w, a.y, half_w, a.height),
            WindowAction::TopHalf => Rect::new(a.x, a.y, a.width, half_h),
            WindowAction::BottomHalf => Rect::new(a.x, a.y + half_h, a.width, half_h),
            WindowAction::Maximize => a,
            WindowAction::Center => {
                // Centering preserves the window's current size, clamped to the
                // work area so it can never end up larger than the screen.
                let w = current.width.min(a.width);
                let h = current.height.min(a.height);
                return Some(
                    Rect::new(a.x + (a.width - w) / 2.0, a.y + (a.height - h) / 2.0, w, h)
                        .rounded(),
                );
            }
            WindowAction::Restore => return None,
        };

        // Maximize intentionally honours the gap too, matching Rectangle's
        // behaviour where a non-zero gap insets every action uniformly.
        Some(rect.inset(gap).rounded())
    }
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

    #[test]
    fn halves_tile_the_work_area_exactly() {
        let l = WindowAction::LeftHalf
            .target_rect(AREA, 0.0, CURRENT)
            .unwrap();
        let r = WindowAction::RightHalf
            .target_rect(AREA, 0.0, CURRENT)
            .unwrap();
        assert_eq!(l, Rect::new(0.0, 0.0, 960.0, 1040.0));
        assert_eq!(r, Rect::new(960.0, 0.0, 960.0, 1040.0));
        assert_eq!(l.max_x(), r.x);
        assert_eq!(l.width + r.width, AREA.width);
    }

    #[test]
    fn odd_width_halves_leave_no_seam_and_no_overflow() {
        let area = Rect::new(0.0, 0.0, 1367.0, 768.0);
        let l = WindowAction::LeftHalf
            .target_rect(area, 0.0, CURRENT)
            .unwrap();
        let r = WindowAction::RightHalf
            .target_rect(area, 0.0, CURRENT)
            .unwrap();
        assert_eq!(l.max_x(), r.x);
        assert_eq!(r.max_x(), area.max_x());
    }

    #[test]
    fn work_area_offset_is_respected() {
        // Simulates a taskbar on the left and a second monitor origin.
        let area = Rect::new(1920.0 + 60.0, 30.0, 1860.0, 1010.0);
        let l = WindowAction::LeftHalf
            .target_rect(area, 0.0, CURRENT)
            .unwrap();
        assert_eq!(l.x, 1980.0);
        assert_eq!(l.y, 30.0);
        assert_eq!(l.width, 930.0);
    }

    #[test]
    fn maximize_fills_work_area() {
        let m = WindowAction::Maximize
            .target_rect(AREA, 0.0, CURRENT)
            .unwrap();
        assert_eq!(m, AREA);
    }

    #[test]
    fn gap_insets_every_edge() {
        let m = WindowAction::Maximize
            .target_rect(AREA, 10.0, CURRENT)
            .unwrap();
        assert_eq!(m, Rect::new(10.0, 10.0, 1900.0, 1020.0));
    }

    #[test]
    fn center_preserves_size_and_centers() {
        let c = WindowAction::Center
            .target_rect(AREA, 0.0, CURRENT)
            .unwrap();
        assert_eq!(c.width, 800.0);
        assert_eq!(c.height, 600.0);
        assert_eq!(c.center(), AREA.center());
    }

    #[test]
    fn center_clamps_oversized_windows() {
        let huge = Rect::new(0.0, 0.0, 5000.0, 5000.0);
        let c = WindowAction::Center.target_rect(AREA, 0.0, huge).unwrap();
        assert_eq!(c, AREA);
    }

    #[test]
    fn restore_has_no_geometry() {
        assert!(WindowAction::Restore
            .target_rect(AREA, 0.0, CURRENT)
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
}
