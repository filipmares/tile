//! Corner composition: the side-arrow-then-vertical-arrow gesture.
//!
//! While the base modifier stays held, pressing a side arrow (base+`Left` or
//! base+`Right`) and then base+`Up` or base+`Down` turns the side placement
//! into a half-height corner on that side, keeping the width the side had.
//! Releasing the base modifier ends the composition, so a later base+`Up`
//! maximizes and base+`Down` restores as usual.
//!
//! Composition is specific to the standard bindings: the side arrows must be
//! bound to [`WindowAction::LeftHalf`]/[`WindowAction::RightHalf`] and the
//! vertical arrows to [`WindowAction::Maximize`]/[`WindowAction::Restore`],
//! all on the base modifier. Rebound shortcuts keep their plain meaning.

use crate::config::BASE_MODIFIERS;
use crate::{ActionRequest, Hotkey, KeyCode, WindowAction};

/// Turns hotkey presses into [`ActionRequest`]s, composing corners.
///
/// Backends feed every press through [`CornerComposer::press`] together with
/// a *hold* identifier: any value that changes whenever a modifier key is
/// released. Two presses with the same hold were made without letting go of
/// the modifiers in between.
#[derive(Debug, Default)]
pub struct CornerComposer {
    /// The side last pressed, and the hold it was pressed in.
    side: Option<(u64, WindowAction)>,
}

impl CornerComposer {
    pub fn new() -> Self {
        Self::default()
    }

    /// The request a press of `hotkey`, bound to `action`, should send.
    pub fn press(&mut self, hotkey: Hotkey, action: WindowAction, hold: u64) -> ActionRequest {
        let standard = hotkey.modifiers == BASE_MODIFIERS;
        match (standard, hotkey.key, action) {
            (true, KeyCode::Left, WindowAction::LeftHalf)
            | (true, KeyCode::Right, WindowAction::RightHalf) => {
                self.side = Some((hold, action));
                action.into()
            }
            (true, KeyCode::Up, WindowAction::Maximize)
            | (true, KeyCode::Down, WindowAction::Restore) => match self.side {
                Some((side_hold, side)) if side_hold == hold => {
                    let up = hotkey.key == KeyCode::Up;
                    let corner = match (side, up) {
                        (WindowAction::LeftHalf, true) => WindowAction::TopLeft,
                        (WindowAction::LeftHalf, false) => WindowAction::BottomLeft,
                        (_, true) => WindowAction::TopRight,
                        (_, false) => WindowAction::BottomRight,
                    };
                    ActionRequest::composed(corner)
                }
                _ => {
                    self.side = None;
                    action.into()
                }
            },
            _ => {
                self.side = None;
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
    fn side_then_vertical_in_one_hold_composes_a_corner() {
        let mut c = CornerComposer::new();
        assert_eq!(
            c.press(base(KeyCode::Left), WindowAction::LeftHalf, 1),
            WindowAction::LeftHalf.into()
        );
        assert_eq!(
            c.press(base(KeyCode::Up), WindowAction::Maximize, 1),
            ActionRequest::composed(WindowAction::TopLeft)
        );
        // Still held: the other vertical arrow flips to the other corner.
        assert_eq!(
            c.press(base(KeyCode::Down), WindowAction::Restore, 1),
            ActionRequest::composed(WindowAction::BottomLeft)
        );
        c.press(base(KeyCode::Right), WindowAction::RightHalf, 1);
        assert_eq!(
            c.press(base(KeyCode::Down), WindowAction::Restore, 1),
            ActionRequest::composed(WindowAction::BottomRight)
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
