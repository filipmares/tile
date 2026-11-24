//! Corner composition: the side-key-then-vertical-key gesture.
//!
//! While the configured base modifier stays held, pressing the side placement
//! key (`LeftHalf` or `RightHalf`) starts on the middle stop of a three-stop
//! vertical rail for that side (`Top ↔ Full ↔ Bottom`):
//!
//! - From `Full`, pressing `Maximize` / `Restore` moves to the top or bottom
//!   corner keeping the side's current width.
//! - From `Top`, pressing `Restore` returns to `Full` keeping the corner's
//!   width, and pressing `Maximize` maximizes and ends composition.
//! - From `Bottom`, pressing `Maximize` returns to `Full` keeping the corner's
//!   width, and pressing `Restore` restores and ends composition.
//! - While in a corner, pressing that side's key cycles the corner's width;
//!   pressing the opposite side key resets to a fresh full-height placement on
//!   the opposite side.
//!
//! Releasing the base modifier (or pressing any other action) ends composition,
//! so a later `Maximize` maximizes, `Restore` restores, and `LeftHalf` /
//! `RightHalf` starts a fresh full-height placement.
//!
//! Composition follows whichever base modifier and keys the user configured for
//! `LeftHalf`, `RightHalf`, `Maximize`, and `Restore`.

use crate::config::BASE_MODIFIERS;
use crate::{ActionRequest, ActionTarget, Corner, Hotkey, Modifiers, WindowAction};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Side {
    Left,
    Right,
}

impl Side {
    fn target(self, vertical: Vertical) -> ActionTarget {
        match (self, vertical) {
            (Side::Left, Vertical::Top) => ActionTarget::Corner(Corner::TopLeft),
            (Side::Left, Vertical::Full) => ActionTarget::Action(WindowAction::LeftHalf),
            (Side::Left, Vertical::Bottom) => ActionTarget::Corner(Corner::BottomLeft),
            (Side::Right, Vertical::Top) => ActionTarget::Corner(Corner::TopRight),
            (Side::Right, Vertical::Full) => ActionTarget::Action(WindowAction::RightHalf),
            (Side::Right, Vertical::Bottom) => ActionTarget::Corner(Corner::BottomRight),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Vertical {
    Top,
    Full,
    Bottom,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Active {
    hold: u64,
    side: Side,
    vertical: Vertical,
}

/// Turns hotkey presses into [`ActionRequest`]s, composing corners.
///
/// Backends feed every press through [`CornerComposer::press`] together with
/// a *hold* identifier: any value that changes whenever a modifier key is
/// released. Two presses with the same hold were made without letting go of
/// the modifiers in between.
#[derive(Debug)]
pub struct CornerComposer {
    /// The configured base modifier that enables single-hold corner
    /// composition, or `None` when composition is inactive.
    base_modifiers: Option<Modifiers>,
    /// The side and vertical stop active in the current hold.
    active: Option<Active>,
}

impl Default for CornerComposer {
    fn default() -> Self {
        Self::with_base_modifiers(Some(BASE_MODIFIERS))
    }
}

impl CornerComposer {
    pub fn new() -> Self {
        Self::default()
    }

    pub const fn with_base_modifiers(base_modifiers: Option<Modifiers>) -> Self {
        Self {
            base_modifiers,
            active: None,
        }
    }

    /// Updates the configured base modifier and clears any in-progress hold.
    pub fn set_base_modifiers(&mut self, base_modifiers: Option<Modifiers>) {
        self.base_modifiers = base_modifiers;
        self.active = None;
    }

    /// The request a press of `hotkey`, bound to `action`, should send.
    pub fn press(&mut self, hotkey: Hotkey, action: WindowAction, hold: u64) -> ActionRequest {
        let Some(base_modifiers) = self.base_modifiers else {
            self.active = None;
            return action.into();
        };
        if hotkey.modifiers != base_modifiers {
            self.active = None;
            return action.into();
        }

        let current = self.active.filter(|state| state.hold == hold);

        match action {
            WindowAction::LeftHalf | WindowAction::RightHalf => {
                let side = if action == WindowAction::LeftHalf {
                    Side::Left
                } else {
                    Side::Right
                };
                match current {
                    Some(state) if state.side == side && state.vertical != Vertical::Full => {
                        state.side.target(state.vertical).into()
                    }
                    _ => {
                        self.active = Some(Active {
                            hold,
                            side,
                            vertical: Vertical::Full,
                        });
                        action.into()
                    }
                }
            }
            WindowAction::Maximize | WindowAction::Restore => {
                let next = current.and_then(|state| {
                    let vertical = match (state.vertical, action) {
                        (Vertical::Full, WindowAction::Maximize) => Vertical::Top,
                        (Vertical::Bottom, WindowAction::Maximize)
                        | (Vertical::Top, WindowAction::Restore) => Vertical::Full,
                        (Vertical::Full, WindowAction::Restore) => Vertical::Bottom,
                        _ => return None,
                    };
                    Some((state.side, vertical))
                });
                match next {
                    Some((side, vertical)) => {
                        self.active = Some(Active {
                            hold,
                            side,
                            vertical,
                        });
                        ActionRequest::composed(side.target(vertical))
                    }
                    None => {
                        self.active = None;
                        action.into()
                    }
                }
            }
            _ => {
                self.active = None;
                action.into()
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::KeyCode;

    fn base(key: KeyCode) -> Hotkey {
        Hotkey::new(BASE_MODIFIERS, key)
    }

    #[test]
    fn side_then_vertical_walks_the_three_stop_rail() {
        let mut c = CornerComposer::new();
        assert_eq!(
            c.press(base(KeyCode::Left), WindowAction::LeftHalf, 1),
            WindowAction::LeftHalf.into()
        );
        // Full -> Top -> Full -> Bottom -> Full.
        assert_eq!(
            c.press(base(KeyCode::Up), WindowAction::Maximize, 1),
            ActionRequest::composed(Corner::TopLeft)
        );
        assert_eq!(
            c.press(base(KeyCode::Down), WindowAction::Restore, 1),
            ActionRequest::composed(WindowAction::LeftHalf)
        );
        assert_eq!(
            c.press(base(KeyCode::Down), WindowAction::Restore, 1),
            ActionRequest::composed(Corner::BottomLeft)
        );
        assert_eq!(
            c.press(base(KeyCode::Up), WindowAction::Maximize, 1),
            ActionRequest::composed(WindowAction::LeftHalf)
        );
    }

    #[test]
    fn same_side_key_in_a_corner_cycles_that_corner() {
        let mut c = CornerComposer::new();
        c.press(base(KeyCode::Left), WindowAction::LeftHalf, 1);
        assert_eq!(
            c.press(base(KeyCode::Up), WindowAction::Maximize, 1),
            ActionRequest::composed(Corner::TopLeft)
        );
        // Repeating Left while in TopLeft emits plain TopLeft so Engine cycles its width.
        assert_eq!(
            c.press(base(KeyCode::Left), WindowAction::LeftHalf, 1),
            Corner::TopLeft.into()
        );
        assert_eq!(
            c.press(base(KeyCode::Left), WindowAction::LeftHalf, 1),
            Corner::TopLeft.into()
        );
        // Restore still steps back to Full Left.
        assert_eq!(
            c.press(base(KeyCode::Down), WindowAction::Restore, 1),
            ActionRequest::composed(WindowAction::LeftHalf)
        );
        // Now Left is a plain LeftHalf again.
        assert_eq!(
            c.press(base(KeyCode::Left), WindowAction::LeftHalf, 1),
            WindowAction::LeftHalf.into()
        );
    }

    #[test]
    fn opposite_side_key_in_a_corner_resets_to_opposite_full_side() {
        let mut c = CornerComposer::new();
        c.press(base(KeyCode::Left), WindowAction::LeftHalf, 1);
        c.press(base(KeyCode::Up), WindowAction::Maximize, 1);
        assert_eq!(
            c.press(base(KeyCode::Right), WindowAction::RightHalf, 1),
            WindowAction::RightHalf.into()
        );
        assert_eq!(
            c.press(base(KeyCode::Down), WindowAction::Restore, 1),
            ActionRequest::composed(Corner::BottomRight)
        );
        assert_eq!(
            c.press(base(KeyCode::Right), WindowAction::RightHalf, 1),
            Corner::BottomRight.into()
        );
        assert_eq!(
            c.press(base(KeyCode::Left), WindowAction::LeftHalf, 1),
            WindowAction::LeftHalf.into()
        );
    }

    #[test]
    fn pushing_past_top_or_bottom_corner_maximizes_or_restores_and_ends_composition() {
        let mut c = CornerComposer::new();
        c.press(base(KeyCode::Left), WindowAction::LeftHalf, 1);
        assert_eq!(
            c.press(base(KeyCode::Up), WindowAction::Maximize, 1),
            ActionRequest::composed(Corner::TopLeft)
        );
        // Maximize from TopLeft maximizes and ends composition.
        assert_eq!(
            c.press(base(KeyCode::Up), WindowAction::Maximize, 1),
            WindowAction::Maximize.into()
        );
        // Composition ended, so Restore is now plain Restore even in the same hold.
        assert_eq!(
            c.press(base(KeyCode::Down), WindowAction::Restore, 1),
            WindowAction::Restore.into()
        );

        // Symmetric path at the bottom.
        c.press(base(KeyCode::Right), WindowAction::RightHalf, 1);
        assert_eq!(
            c.press(base(KeyCode::Down), WindowAction::Restore, 1),
            ActionRequest::composed(Corner::BottomRight)
        );
        assert_eq!(
            c.press(base(KeyCode::Down), WindowAction::Restore, 1),
            WindowAction::Restore.into()
        );
        assert_eq!(
            c.press(base(KeyCode::Up), WindowAction::Maximize, 1),
            WindowAction::Maximize.into()
        );
    }

    #[test]
    fn releasing_the_modifier_ends_composition() {
        let mut c = CornerComposer::new();
        c.press(base(KeyCode::Left), WindowAction::LeftHalf, 1);
        assert_eq!(
            c.press(base(KeyCode::Up), WindowAction::Maximize, 2),
            WindowAction::Maximize.into()
        );

        // Releasing while in a corner also ends composition: Left is full LeftHalf again.
        c.press(base(KeyCode::Left), WindowAction::LeftHalf, 3);
        c.press(base(KeyCode::Up), WindowAction::Maximize, 3);
        assert_eq!(
            c.press(base(KeyCode::Left), WindowAction::LeftHalf, 4),
            WindowAction::LeftHalf.into()
        );
    }

    #[test]
    fn vertical_first_or_after_another_action_is_plain() {
        let mut c = CornerComposer::new();
        assert_eq!(
            c.press(base(KeyCode::Up), WindowAction::Maximize, 1),
            WindowAction::Maximize.into()
        );
        c.press(base(KeyCode::Left), WindowAction::LeftHalf, 1);
        c.press(base(KeyCode::Enter), WindowAction::CenterHalf, 1);
        assert_eq!(
            c.press(base(KeyCode::Up), WindowAction::Maximize, 1),
            WindowAction::Maximize.into()
        );
    }

    #[test]
    fn composition_follows_custom_base_modifier_and_custom_keys() {
        let custom = Modifiers::CONTROL | Modifiers::META;
        let mut c = CornerComposer::with_base_modifiers(Some(custom));
        let hk = |key| Hotkey::new(custom, key);

        assert_eq!(
            c.press(hk(KeyCode::H), WindowAction::LeftHalf, 1),
            WindowAction::LeftHalf.into()
        );
        assert_eq!(
            c.press(hk(KeyCode::K), WindowAction::Maximize, 1),
            ActionRequest::composed(Corner::TopLeft)
        );
        assert_eq!(
            c.press(hk(KeyCode::H), WindowAction::LeftHalf, 1),
            Corner::TopLeft.into()
        );
        assert_eq!(
            c.press(hk(KeyCode::J), WindowAction::Restore, 1),
            ActionRequest::composed(WindowAction::LeftHalf)
        );

        // A display throw on the same modifier ends composition rather than composing.
        assert_eq!(
            c.press(hk(KeyCode::D), WindowAction::DisplayRight, 1),
            WindowAction::DisplayRight.into()
        );
        assert_eq!(
            c.press(hk(KeyCode::K), WindowAction::Maximize, 1),
            WindowAction::Maximize.into()
        );
    }

    #[test]
    fn composition_is_disabled_when_base_modifiers_is_none() {
        let mut c = CornerComposer::with_base_modifiers(None);
        c.press(base(KeyCode::Left), WindowAction::LeftHalf, 1);
        assert_eq!(
            c.press(base(KeyCode::Up), WindowAction::Maximize, 1),
            WindowAction::Maximize.into()
        );
    }
}
