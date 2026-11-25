//! The set of window actions Tile can perform, composed corner placements, and
//! the pure geometry that turns them into target rectangles.

use serde::{Deserialize, Serialize};
use std::fmt;
use std::str::FromStr;

use crate::config::{Gaps, SharedEdges};
use crate::geometry::{Direction, Rect};

/// The shortcut group a [`WindowAction`] belongs to.
///
/// Each group carries its own configurable modifier combination — the **base
/// modifier** for [`ShortcutGroup::Placement`] and the **display modifier** for
/// [`ShortcutGroup::Displays`] — combined with each action's key.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ShortcutGroup {
    Placement,
    Displays,
}

impl ShortcutGroup {
    /// Groups in the order they appear in Settings.
    pub const ALL: [ShortcutGroup; 2] = [ShortcutGroup::Placement, ShortcutGroup::Displays];

    /// Human-readable heading for the settings window.
    pub const fn label(self) -> &'static str {
        match self {
            ShortcutGroup::Placement => "Window placement",
            ShortcutGroup::Displays => "Displays",
        }
    }

    /// The actions belonging to this group, in UI order.
    pub fn actions(self) -> impl Iterator<Item = WindowAction> {
        WindowAction::ALL
            .iter()
            .copied()
            .filter(move |a| a.group() == self)
    }
}

/// Every bindable window action supported by Tile.
///
/// Fractional widths are reached by repeating a placement shortcut through the
/// size cycle, and corners are reached through single-hold corner composition —
/// neither has its own bindable action.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum WindowAction {
    LeftHalf,
    RightHalf,
    Maximize,
    Restore,
    CenterHalf,
    DisplayLeft,
    DisplayRight,
    DisplayUp,
    DisplayDown,
}

impl WindowAction {
    /// All bindable actions, grouped by [`ShortcutGroup`] in UI order.
    pub const ALL: [WindowAction; 9] = [
        // Window placement
        WindowAction::LeftHalf,
        WindowAction::RightHalf,
        WindowAction::Maximize,
        WindowAction::Restore,
        WindowAction::CenterHalf,
        // Displays
        WindowAction::DisplayLeft,
        WindowAction::DisplayRight,
        WindowAction::DisplayUp,
        WindowAction::DisplayDown,
    ];

    /// Stable machine-readable identifier, also used as the JSON config key.
    ///
    /// Preserved from v1 configs so saved keys remain stable across versions.
    pub const fn id(self) -> &'static str {
        match self {
            WindowAction::LeftHalf => "left-half",
            WindowAction::RightHalf => "right-half",
            WindowAction::Maximize => "maximize",
            WindowAction::Restore => "restore",
            WindowAction::CenterHalf => "center-half",
            WindowAction::DisplayLeft => "display-left",
            WindowAction::DisplayRight => "display-right",
            WindowAction::DisplayUp => "display-up",
            WindowAction::DisplayDown => "display-down",
        }
    }

    /// Human-readable label for menus and the settings window.
    pub const fn label(self) -> &'static str {
        match self {
            WindowAction::LeftHalf => "Left",
            WindowAction::RightHalf => "Right",
            WindowAction::Maximize => "Maximize",
            WindowAction::Restore => "Restore",
            WindowAction::CenterHalf => "Center Column",
            WindowAction::DisplayLeft => "Display Left",
            WindowAction::DisplayRight => "Display Right",
            WindowAction::DisplayUp => "Display Above",
            WindowAction::DisplayDown => "Display Below",
        }
    }

    /// Which modifier group owns this action.
    pub const fn group(self) -> ShortcutGroup {
        match self {
            WindowAction::LeftHalf
            | WindowAction::RightHalf
            | WindowAction::Maximize
            | WindowAction::Restore
            | WindowAction::CenterHalf => ShortcutGroup::Placement,
            WindowAction::DisplayLeft
            | WindowAction::DisplayRight
            | WindowAction::DisplayUp
            | WindowAction::DisplayDown => ShortcutGroup::Displays,
        }
    }

    /// `Restore` is handled by the window-history layer rather than pure
    /// geometry, so it has no computable target rectangle.
    pub const fn uses_history(self) -> bool {
        matches!(self, WindowAction::Restore)
    }

    /// Directional display throws pick another display rather than a rectangle
    /// on the current one.
    pub const fn moves_display(self) -> bool {
        self.display_direction().is_some()
    }

    pub const fn display_direction(self) -> Option<Direction> {
        match self {
            WindowAction::DisplayLeft => Some(Direction::Left),
            WindowAction::DisplayRight => Some(Direction::Right),
            WindowAction::DisplayUp => Some(Direction::Up),
            WindowAction::DisplayDown => Some(Direction::Down),
            _ => None,
        }
    }

    /// Whether repeatedly invoking this action steps through [`crate::CycleSize`]s.
    pub const fn cycles(self) -> bool {
        matches!(
            self,
            WindowAction::LeftHalf | WindowAction::RightHalf | WindowAction::CenterHalf
        )
    }

    /// This action's rectangle resized to `fraction` of the work area along
    /// its cycle axis. Passing `0.5` reproduces [`WindowAction::target_rect`].
    pub fn cycle_rect(
        self,
        work_area: Rect,
        gaps: &Gaps,
        main_screen: bool,
        fraction: f64,
    ) -> Option<Rect> {
        if !(fraction.is_finite() && fraction > 0.0 && fraction <= 1.0) {
            return None;
        }
        let f = fraction;
        let rect = match self {
            WindowAction::LeftHalf => grid(work_area, gaps, main_screen, (0.0, f), (0.0, 1.0)),
            WindowAction::RightHalf => {
                grid(work_area, gaps, main_screen, (1.0 - f, 1.0), (0.0, 1.0))
            }
            WindowAction::CenterHalf => grid(
                work_area,
                gaps,
                main_screen,
                ((1.0 - f) / 2.0, (1.0 + f) / 2.0),
                (0.0, 1.0),
            ),
            _ => return None,
        };
        Some(rect.rounded())
    }

    /// Computes the default destination rectangle for this action within
    /// `work_area`. Returns `None` for history (`Restore`) and display throws.
    pub fn target_rect(self, work_area: Rect, gaps: &Gaps, main_screen: bool) -> Option<Rect> {
        let rect = match self {
            WindowAction::LeftHalf => grid(work_area, gaps, main_screen, (0.0, 0.5), (0.0, 1.0)),
            WindowAction::RightHalf => grid(work_area, gaps, main_screen, (0.5, 1.0), (0.0, 1.0)),
            WindowAction::CenterHalf => {
                grid(work_area, gaps, main_screen, (0.25, 0.75), (0.0, 1.0))
            }
            WindowAction::Maximize => grid(work_area, gaps, main_screen, (0.0, 1.0), (0.0, 1.0)),
            WindowAction::Restore
            | WindowAction::DisplayLeft
            | WindowAction::DisplayRight
            | WindowAction::DisplayUp
            | WindowAction::DisplayDown => return None,
        };
        Some(rect.rounded())
    }
}

impl fmt::Display for WindowAction {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.id())
    }
}

/// A half-height corner placement reached through [`crate::CornerComposer`].
///
/// Corners are not bindable actions; they are composed from a side placement
/// followed by the maximize or restore key while holding the base modifier, and
/// cycle horizontally through the configured size cycle.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Corner {
    TopLeft,
    TopRight,
    BottomLeft,
    BottomRight,
}

impl Corner {
    pub const ALL: [Corner; 4] = [
        Corner::TopLeft,
        Corner::TopRight,
        Corner::BottomLeft,
        Corner::BottomRight,
    ];

    pub const fn id(self) -> &'static str {
        match self {
            Corner::TopLeft => "top-left",
            Corner::TopRight => "top-right",
            Corner::BottomLeft => "bottom-left",
            Corner::BottomRight => "bottom-right",
        }
    }

    /// The full-height side this corner sits on.
    pub const fn side(self) -> WindowAction {
        match self {
            Corner::TopLeft | Corner::BottomLeft => WindowAction::LeftHalf,
            Corner::TopRight | Corner::BottomRight => WindowAction::RightHalf,
        }
    }

    /// This corner's rectangle at `fraction` of the work-area width, keeping
    /// its half height.
    pub fn cycle_rect(
        self,
        work_area: Rect,
        gaps: &Gaps,
        main_screen: bool,
        fraction: f64,
    ) -> Option<Rect> {
        if !(fraction.is_finite() && fraction > 0.0 && fraction <= 1.0) {
            return None;
        }
        let f = fraction;
        let rect = match self {
            Corner::TopLeft => grid(work_area, gaps, main_screen, (0.0, f), (0.0, 0.5)),
            Corner::TopRight => grid(work_area, gaps, main_screen, (1.0 - f, 1.0), (0.0, 0.5)),
            Corner::BottomLeft => grid(work_area, gaps, main_screen, (0.0, f), (0.5, 1.0)),
            Corner::BottomRight => grid(work_area, gaps, main_screen, (1.0 - f, 1.0), (0.5, 1.0)),
        };
        Some(rect.rounded())
    }

    pub fn target_rect(self, work_area: Rect, gaps: &Gaps, main_screen: bool) -> Rect {
        self.cycle_rect(work_area, gaps, main_screen, 0.5)
            .expect("0.5 is always a valid cycle fraction")
    }
}

impl fmt::Display for Corner {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.id())
    }
}

/// Either a bindable [`WindowAction`] or a composed [`Corner`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ActionTarget {
    Action(WindowAction),
    Corner(Corner),
}

impl ActionTarget {
    pub const fn id(self) -> &'static str {
        match self {
            ActionTarget::Action(action) => action.id(),
            ActionTarget::Corner(corner) => corner.id(),
        }
    }

    pub const fn as_action(self) -> Option<WindowAction> {
        match self {
            ActionTarget::Action(action) => Some(action),
            ActionTarget::Corner(_) => None,
        }
    }

    pub const fn uses_history(self) -> bool {
        matches!(self, ActionTarget::Action(action) if action.uses_history())
    }

    pub const fn moves_display(self) -> bool {
        matches!(self, ActionTarget::Action(action) if action.moves_display())
    }

    pub const fn display_direction(self) -> Option<Direction> {
        match self {
            ActionTarget::Action(action) => action.display_direction(),
            ActionTarget::Corner(_) => None,
        }
    }

    pub const fn cycles(self) -> bool {
        match self {
            ActionTarget::Action(action) => action.cycles(),
            ActionTarget::Corner(_) => true,
        }
    }

    /// The full-height side this target belongs to when participating in
    /// corner composition (`LeftHalf` / `RightHalf` or their corners).
    pub const fn composition_side(self) -> Option<WindowAction> {
        match self {
            ActionTarget::Action(action @ (WindowAction::LeftHalf | WindowAction::RightHalf)) => {
                Some(action)
            }
            ActionTarget::Corner(corner) => Some(corner.side()),
            _ => None,
        }
    }

    pub fn cycle_rect(
        self,
        work_area: Rect,
        gaps: &Gaps,
        main_screen: bool,
        fraction: f64,
    ) -> Option<Rect> {
        match self {
            ActionTarget::Action(action) => {
                action.cycle_rect(work_area, gaps, main_screen, fraction)
            }
            ActionTarget::Corner(corner) => {
                corner.cycle_rect(work_area, gaps, main_screen, fraction)
            }
        }
    }

    pub fn target_rect(self, work_area: Rect, gaps: &Gaps, main_screen: bool) -> Option<Rect> {
        match self {
            ActionTarget::Action(action) => action.target_rect(work_area, gaps, main_screen),
            ActionTarget::Corner(corner) => Some(corner.target_rect(work_area, gaps, main_screen)),
        }
    }
}

impl From<WindowAction> for ActionTarget {
    fn from(action: WindowAction) -> Self {
        ActionTarget::Action(action)
    }
}

impl From<Corner> for ActionTarget {
    fn from(corner: Corner) -> Self {
        ActionTarget::Corner(corner)
    }
}

impl fmt::Display for ActionTarget {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.id())
    }
}

impl Serialize for ActionTarget {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        serializer.serialize_str(self.id())
    }
}

/// A placement or display-move request delivered by a hotkey or the tray menu.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ActionRequest {
    pub target: ActionTarget,
    /// A vertical step composed on a side (`Top ↔ Full ↔ Bottom`) — see
    /// [`crate::CornerComposer`]. Keeps the width the window already has on
    /// that side instead of starting at a half.
    pub composed: bool,
    /// Originated from the tray/menu-bar menu, so failures surface a brief
    /// notice rather than staying silent like a hotkey. Menu clicks still cycle
    /// on repeats.
    pub from_menu: bool,
}

impl ActionRequest {
    pub fn composed(target: impl Into<ActionTarget>) -> Self {
        Self {
            target: target.into(),
            composed: true,
            from_menu: false,
        }
    }

    pub const fn from_menu(action: WindowAction) -> Self {
        Self {
            target: ActionTarget::Action(action),
            composed: false,
            from_menu: true,
        }
    }
}

impl From<WindowAction> for ActionRequest {
    fn from(action: WindowAction) -> Self {
        Self {
            target: ActionTarget::Action(action),
            composed: false,
            from_menu: false,
        }
    }
}

impl From<Corner> for ActionRequest {
    fn from(corner: Corner) -> Self {
        Self {
            target: ActionTarget::Corner(corner),
            composed: false,
            from_menu: false,
        }
    }
}

impl From<ActionTarget> for ActionRequest {
    fn from(target: ActionTarget) -> Self {
        Self {
            target,
            composed: false,
            from_menu: false,
        }
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

    #[test]
    fn halves_tile_the_work_area_without_seam_even_on_odd_width() {
        let odd = Rect::new(0.0, 0.0, 1367.0, 768.0);
        let left = WindowAction::LeftHalf
            .target_rect(odd, &Gaps::default(), true)
            .unwrap();
        let right = WindowAction::RightHalf
            .target_rect(odd, &Gaps::default(), true)
            .unwrap();
        assert_eq!(left.x, odd.x);
        assert_eq!(left.max_x(), right.x);
        assert_eq!(right.max_x(), odd.max_x());
        assert_eq!(left.height, odd.height);
    }

    #[test]
    fn corners_tile_with_no_seam_on_odd_dimensions() {
        let odd = Rect::new(100.0, 40.0, 1367.0, 769.0);
        let tl = Corner::TopLeft.target_rect(odd, &Gaps::default(), true);
        let tr = Corner::TopRight.target_rect(odd, &Gaps::default(), true);
        let bl = Corner::BottomLeft.target_rect(odd, &Gaps::default(), true);
        let br = Corner::BottomRight.target_rect(odd, &Gaps::default(), true);

        assert_eq!(tl.max_x(), tr.x);
        assert_eq!(bl.max_x(), br.x);
        assert_eq!(tl.max_y(), bl.y);
        assert_eq!(tr.max_y(), br.y);
        assert_eq!(tr.max_x(), odd.max_x());
        assert_eq!(br.max_y(), odd.max_y());
    }

    #[test]
    fn adjacent_halves_are_separated_by_exactly_one_window_gap() {
        let gaps = Gaps {
            window: 12.0,
            edge_left: 20.0,
            edge_right: 20.0,
            edge_top: 10.0,
            edge_bottom: 10.0,
            skip_top_edge: false,
            main_screen_only: false,
        };
        let left = WindowAction::LeftHalf
            .target_rect(AREA, &gaps, true)
            .unwrap();
        let right = WindowAction::RightHalf
            .target_rect(AREA, &gaps, true)
            .unwrap();
        assert_eq!(left.x, 20.0);
        assert_eq!(right.x - left.max_x(), 12.0);
        assert_eq!(AREA.max_x() - right.max_x(), 20.0);
    }

    #[test]
    fn skip_top_edge_and_main_screen_only_apply_per_edge() {
        let gaps = Gaps {
            window: 10.0,
            edge_top: 16.0,
            edge_bottom: 16.0,
            edge_left: 16.0,
            edge_right: 16.0,
            skip_top_edge: true,
            main_screen_only: true,
        };
        let main = WindowAction::LeftHalf
            .target_rect(AREA, &gaps, true)
            .unwrap();
        assert_eq!(main.y, 0.0);
        assert_eq!(main.x, 16.0);

        let secondary = WindowAction::LeftHalf
            .target_rect(AREA, &gaps, false)
            .unwrap();
        assert_eq!(secondary.x, 0.0);
        assert_eq!(secondary.y, 0.0);
        // Shared edge still keeps half the window gap on secondary screens.
        assert_eq!(secondary.width, 960.0 - 5.0);
    }

    #[test]
    fn cycle_rect_at_half_matches_target_rect_and_anchors_correctly() {
        let gaps = Gaps::default();
        for action in [
            WindowAction::LeftHalf,
            WindowAction::RightHalf,
            WindowAction::CenterHalf,
        ] {
            assert_eq!(
                action.cycle_rect(AREA, &gaps, true, 0.5),
                action.target_rect(AREA, &gaps, true)
            );
        }
        assert_eq!(
            WindowAction::CenterHalf.cycle_rect(AREA, &gaps, true, 1.0 / 3.0),
            Some(Rect::new(640.0, 0.0, 640.0, 1040.0))
        );
        assert_eq!(
            Corner::BottomRight.cycle_rect(AREA, &gaps, true, 2.0 / 3.0),
            Some(Rect::new(640.0, 520.0, 1280.0, 520.0))
        );
        assert_eq!(
            WindowAction::LeftHalf.cycle_rect(AREA, &gaps, true, 0.0),
            None
        );
        assert_eq!(
            WindowAction::LeftHalf.cycle_rect(AREA, &gaps, true, 1.5),
            None
        );
        assert_eq!(
            WindowAction::Maximize.cycle_rect(AREA, &gaps, true, 0.5),
            None
        );
    }

    #[test]
    fn ids_round_trip_and_serde_matches_id() {
        let mut seen = std::collections::HashSet::new();
        for action in WindowAction::ALL {
            assert!(seen.insert(action.id()));
            assert_eq!(WindowAction::from_str(action.id()).unwrap(), action);
            assert_eq!(
                serde_json::to_string(&action).unwrap(),
                format!("\"{}\"", action.id())
            );
        }
        assert!(WindowAction::from_str("first-third").is_err());
    }

    #[test]
    fn display_actions_and_restore_have_no_target_rect() {
        for action in [
            WindowAction::Restore,
            WindowAction::DisplayLeft,
            WindowAction::DisplayRight,
            WindowAction::DisplayUp,
            WindowAction::DisplayDown,
        ] {
            assert!(action.target_rect(AREA, &Gaps::default(), true).is_none());
        }
    }

    #[test]
    fn center_column_is_centered_across_all_cycle_sizes() {
        let areas = [
            AREA,
            Rect::new(0.0, 25.0, 1470.0, 931.0),
            Rect::new(0.0, 25.0, 1512.0, 957.0),
            Rect::new(-1441.0, 0.0, 1441.0, 900.0),
            Rect::new(71.0, 0.0, 1369.0, 800.0),
        ];
        let gap_sets = [Gaps::default(), Gaps::uniform(10.0), Gaps::uniform(11.0)];

        for area in areas {
            for gaps in &gap_sets {
                for size in crate::CycleSize::ALL {
                    let rect = WindowAction::CenterHalf
                        .cycle_rect(area, gaps, true, size.fraction())
                        .unwrap();
                    let left_margin = rect.x - area.x;
                    let right_margin = area.max_x() - rect.max_x();
                    assert!(
                        (left_margin - right_margin).abs() <= 1.0,
                        "off-center for size={size:?} area={area:?} gaps={gaps:?}: \
                         rect={rect:?} left={left_margin} right={right_margin}"
                    );
                    assert_eq!(
                        rect.recentered_horizontally(area),
                        None,
                        "unclamped {size:?} should not trigger recentering"
                    );
                }
            }
        }
    }
}
