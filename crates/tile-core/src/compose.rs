//! Corner composition: the side-arrow-then-vertical-arrow gesture.
//!
//! While the base modifier stays held, pressing a side arrow (base+`Left` or
//! base+`Right`) starts on the middle stop of a three-stop vertical rail for
//! that side (`Top ↔ Full ↔ Bottom`):
//!
//! - From `Full`, base+`Up` / base+`Down` moves to the top or bottom corner
//!   keeping the side's current width.
//! - From `Top`, base+`Down` returns to `Full` keeping the corner's width, and
//!   base+`Up` maximizes and ends composition.
//! - From `Bottom`, base+`Up` returns to `Full` keeping the corner's width, and
//!   base+`Down` restores and ends composition.
//! - While in a corner, pressing that side's arrow (`Left` in a left corner,
//!   `Right` in a right corner) cycles the corner's width; pressing the
//!   opposite side arrow resets to a fresh full-height placement on the
//!   opposite side.
//!
//! Releasing the base modifier (or pressing any other action) ends composition,
//! so a later base+`Up` maximizes, base+`Down` restores, and base+`Left`/`Right`
//! starts a fresh full-height half.
//!
//! Composition is specific to the standard bindings: the side arrows must be
//! bound to [`WindowAction::LeftHalf`]/[`WindowAction::RightHalf`] and the
//! vertical arrows to [`WindowAction::Maximize`]/[`WindowAction::Restore`],
//! all on the base modifier. Rebound shortcuts keep their plain meaning.

use crate::config::BASE_MODIFIERS;
use crate::{ActionRequest, Hotkey, KeyCode, WindowAction};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Side {
    Left,
    Right,
}

impl Side {
    fn action(self, vertical: Vertical) -> WindowAction {
        match (self, vertical) {
            (Side::Left, Vertical::Top) => WindowAction::TopLeft,
            (Side::Left, Vertical::Full) => WindowAction::LeftHalf,
            (Side::Left, Vertical::Bottom) => WindowAction::BottomLeft,
            (Side::Right, Vertical::Top) => WindowAction::TopRight,
            (Side::Right, Vertical::Full) => WindowAction::RightHalf,
            (Side::Right, Vertical::Bottom) => WindowAction::BottomRight,
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
#[derive(Debug, Default)]
pub struct CornerComposer {
    /// The side and vertical stop active in the current hold.
    active: Option<Active>,
}

impl CornerComposer {
    pub fn new() -> Self {
        Self::default()
    }

    /// The request a press of `hotkey`, bound to `action`, should send.
    pub fn press(&mut self, hotkey: Hotkey, action: WindowAction, hold: u64) -> ActionRequest {
        if hotkey.modifiers != BASE_MODIFIERS {
            self.active = None;
            return action.into();
        }

        let current = self.active.filter(|state| state.hold == hold);

        match (hotkey.key, action) {
            (KeyCode::Left, WindowAction::LeftHalf) | (KeyCode::Right, WindowAction::RightHalf) => {
                let side = if hotkey.key == KeyCode::Left {
                    Side::Left
                } else {
                    Side::Right
                };
                match current {
                    Some(state) if state.side == side && state.vertical != Vertical::Full => {
                        state.side.action(state.vertical).into()
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
            (KeyCode::Up, WindowAction::Maximize) | (KeyCode::Down, WindowAction::Restore) => {
                let next = current.and_then(|state| {
                    let vertical = match (state.vertical, hotkey.key) {
                        (Vertical::Full, KeyCode::Up) => Vertical::Top,
                        (Vertical::Bottom, KeyCode::Up) | (Vertical::Top, KeyCode::Down) => {
                            Vertical::Full
                        }
                        (Vertical::Full, KeyCode::Down) => Vertical::Bottom,
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
                        ActionRequest::composed(side.action(vertical))
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
            ActionRequest::composed(WindowAction::TopLeft)
        );
        assert_eq!(
            c.press(base(KeyCode::Down), WindowAction::Restore, 1),
            ActionRequest::composed(WindowAction::LeftHalf)
        );
        assert_eq!(
            c.press(base(KeyCode::Down), WindowAction::Restore, 1),
            ActionRequest::composed(WindowAction::BottomLeft)
        );
        assert_eq!(
            c.press(base(KeyCode::Up), WindowAction::Maximize, 1),
            ActionRequest::composed(WindowAction::LeftHalf)
        );
    }

    #[test]
    fn same_side_arrow_in_a_corner_cycles_that_corner() {
        let mut c = CornerComposer::new();
        c.press(base(KeyCode::Left), WindowAction::LeftHalf, 1);
        assert_eq!(
            c.press(base(KeyCode::Up), WindowAction::Maximize, 1),
            ActionRequest::composed(WindowAction::TopLeft)
        );
        // Repeating Left while in TopLeft emits plain TopLeft so Engine cycles its width.
        assert_eq!(
            c.press(base(KeyCode::Left), WindowAction::LeftHalf, 1),
            WindowAction::TopLeft.into()
        );
        assert_eq!(
            c.press(base(KeyCode::Left), WindowAction::LeftHalf, 1),
            WindowAction::TopLeft.into()
        );
        // Down still steps back to Full Left.
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
    fn opposite_side_arrow_in_a_corner_resets_to_opposite_full_side() {
        let mut c = CornerComposer::new();
        c.press(base(KeyCode::Left), WindowAction::LeftHalf, 1);
        c.press(base(KeyCode::Up), WindowAction::Maximize, 1);
        assert_eq!(
            c.press(base(KeyCode::Right), WindowAction::RightHalf, 1),
            WindowAction::RightHalf.into()
        );
        assert_eq!(
            c.press(base(KeyCode::Down), WindowAction::Restore, 1),
            ActionRequest::composed(WindowAction::BottomRight)
        );
        assert_eq!(
            c.press(base(KeyCode::Right), WindowAction::RightHalf, 1),
            WindowAction::BottomRight.into()
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
            ActionRequest::composed(WindowAction::TopLeft)
        );
        // Up from TopLeft maximizes and ends composition.
        assert_eq!(
            c.press(base(KeyCode::Up), WindowAction::Maximize, 1),
            WindowAction::Maximize.into()
        );
        // Composition ended, so Down is now plain Restore even in the same hold.
        assert_eq!(
            c.press(base(KeyCode::Down), WindowAction::Restore, 1),
            WindowAction::Restore.into()
        );

        // Symmetric path at the bottom.
        c.press(base(KeyCode::Right), WindowAction::RightHalf, 1);
        assert_eq!(
            c.press(base(KeyCode::Down), WindowAction::Restore, 1),
            ActionRequest::composed(WindowAction::BottomRight)
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
    fn rebound_shortcuts_do_not_compose() {
        let mut c = CornerComposer::new();
        c.press(base(KeyCode::H), WindowAction::LeftHalf, 1);
        assert_eq!(
            c.press(base(KeyCode::K), WindowAction::Maximize, 1),
            WindowAction::Maximize.into()
        );
        c.press(base(KeyCode::Left), WindowAction::LeftHalf, 1);
        assert_eq!(
            c.press(base(KeyCode::Up), WindowAction::TopHalf, 1),
            WindowAction::TopHalf.into()
        );
    }
}
