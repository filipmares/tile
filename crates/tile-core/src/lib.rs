//! `tile-core` — the platform-independent heart of Tile.
//!
//! This crate deliberately contains **no** platform code and no I/O beyond
//! JSON (de)serialization, so all of the interesting logic can be unit tested
//! on any host. Platform backends live in `tile-platform` and are driven
//! through the [`Engine`] in this crate.

pub mod action;
pub mod config;
pub mod geometry;
pub mod history;
pub mod hotkey;

pub use action::{ParseActionError, WindowAction};
pub use config::{Config, ConfigError, Conflict, CONFIG_FILE_NAME, MAX_GAP};
pub use geometry::{Rect, Screen};
pub use history::{WindowHistory, WindowId};
pub use hotkey::{Hotkey, KeyCode, Modifiers, ParseHotkeyError};

/// A snapshot of the window an action is about to be applied to.
#[derive(Debug, Clone, PartialEq)]
pub struct WindowSnapshot {
    pub id: WindowId,
    /// Current window frame, in the same top-left-origin space as [`Screen`].
    pub frame: Rect,
}

/// The outcome of asking the engine what to do about an action.
#[derive(Debug, Clone, PartialEq)]
pub enum Plan {
    /// Move the window to this rectangle.
    Move { id: WindowId, target: Rect },
    /// Nothing to do — the window is already where it should be, or there is
    /// no history to restore.
    NoOp(NoOpReason),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NoOpReason {
    AlreadyInPosition,
    NoHistory,
    NoScreen,
}

/// Pure decision layer: given the focused window, the available screens and
/// the user's configuration, decides where the window should go.
///
/// Keeping this separate from the platform backends means the entire
/// behaviour of the app is testable without a window server.
#[derive(Debug, Default)]
pub struct Engine {
    pub config: Config,
    pub history: WindowHistory,
}

impl Engine {
    pub fn new(config: Config) -> Self {
        Self {
            config,
            history: WindowHistory::new(),
        }
    }

    /// Computes the [`Plan`] for applying `action` to `window`.
    ///
    /// This does not mutate history; call [`Engine::commit`] once the backend
    /// has actually moved the window, so a failed move does not corrupt the
    /// restore point.
    pub fn plan(&self, action: WindowAction, window: &WindowSnapshot, screens: &[Screen]) -> Plan {
        if action.uses_history() {
            return match self.history.peek(window.id) {
                Some(target) => Plan::Move {
                    id: window.id,
                    target,
                },
                None => Plan::NoOp(NoOpReason::NoHistory),
            };
        }

        let Some(screen) = Screen::best_match(screens, &window.frame) else {
            return Plan::NoOp(NoOpReason::NoScreen);
        };

        let Some(target) = action.target_rect(screen.work_area, self.config.gap, window.frame)
        else {
            return Plan::NoOp(NoOpReason::NoHistory);
        };

        if target.approx_eq(&window.frame, 1.0) {
            return Plan::NoOp(NoOpReason::AlreadyInPosition);
        }

        Plan::Move {
            id: window.id,
            target,
        }
    }

    /// Records a successful move so that `Restore` can undo it.
    pub fn commit(&mut self, action: WindowAction, window: &WindowSnapshot, target: Rect) {
        if action.uses_history() {
            self.history.forget(window.id);
        } else {
            self.history.record(window.id, window.frame, target);
        }
    }

    /// Looks up the action bound to `hotkey`, if any.
    pub fn action_for(&self, hotkey: Hotkey) -> Option<WindowAction> {
        self.config
            .active_bindings()
            .into_iter()
            .find(|(hk, _)| *hk == hotkey)
            .map(|(_, action)| action)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn screen() -> Screen {
        Screen {
            id: "primary".into(),
            frame: Rect::new(0.0, 0.0, 1920.0, 1080.0),
            work_area: Rect::new(0.0, 0.0, 1920.0, 1040.0),
            scale_factor: 1.0,
            is_primary: true,
        }
    }

    fn window() -> WindowSnapshot {
        WindowSnapshot {
            id: 1,
            frame: Rect::new(300.0, 200.0, 800.0, 600.0),
        }
    }

    #[test]
    fn plans_a_left_half_move_within_the_work_area() {
        let engine = Engine::default();
        let plan = engine.plan(WindowAction::LeftHalf, &window(), &[screen()]);
        assert_eq!(
            plan,
            Plan::Move {
                id: 1,
                target: Rect::new(0.0, 0.0, 960.0, 1040.0)
            }
        );
    }

    #[test]
    fn repeating_an_action_is_a_no_op() {
        let engine = Engine::default();
        let win = WindowSnapshot {
            id: 1,
            frame: Rect::new(0.0, 0.0, 960.0, 1040.0),
        };
        assert_eq!(
            engine.plan(WindowAction::LeftHalf, &win, &[screen()]),
            Plan::NoOp(NoOpReason::AlreadyInPosition)
        );
    }

    #[test]
    fn restore_returns_the_window_to_where_it_started() {
        let mut engine = Engine::default();
        let win = window();

        let Plan::Move { target, .. } = engine.plan(WindowAction::LeftHalf, &win, &[screen()])
        else {
            panic!("expected a move");
        };
        engine.commit(WindowAction::LeftHalf, &win, target);

        let moved = WindowSnapshot {
            id: 1,
            frame: target,
        };
        assert_eq!(
            engine.plan(WindowAction::Restore, &moved, &[screen()]),
            Plan::Move {
                id: 1,
                target: win.frame
            }
        );
    }

    #[test]
    fn restore_without_history_does_nothing() {
        let engine = Engine::default();
        assert_eq!(
            engine.plan(WindowAction::Restore, &window(), &[screen()]),
            Plan::NoOp(NoOpReason::NoHistory)
        );
    }

    #[test]
    fn committing_a_restore_clears_the_history_entry() {
        let mut engine = Engine::default();
        let win = window();
        engine.commit(
            WindowAction::LeftHalf,
            &win,
            Rect::new(0.0, 0.0, 960.0, 1040.0),
        );
        assert!(!engine.history.is_empty());
        engine.commit(WindowAction::Restore, &win, win.frame);
        assert!(engine.history.is_empty());
    }

    #[test]
    fn actions_target_the_screen_the_window_is_on() {
        let secondary = Screen {
            id: "secondary".into(),
            frame: Rect::new(1920.0, 0.0, 1920.0, 1080.0),
            work_area: Rect::new(1920.0, 0.0, 1920.0, 1080.0),
            scale_factor: 1.0,
            is_primary: false,
        };
        let engine = Engine::default();
        let win = WindowSnapshot {
            id: 7,
            frame: Rect::new(2200.0, 100.0, 400.0, 400.0),
        };
        let plan = engine.plan(WindowAction::LeftHalf, &win, &[screen(), secondary]);
        assert_eq!(
            plan,
            Plan::Move {
                id: 7,
                target: Rect::new(1920.0, 0.0, 960.0, 1080.0)
            }
        );
    }

    #[test]
    fn no_screens_is_handled_gracefully() {
        let engine = Engine::default();
        assert_eq!(
            engine.plan(WindowAction::LeftHalf, &window(), &[]),
            Plan::NoOp(NoOpReason::NoScreen)
        );
    }

    #[test]
    fn hotkeys_resolve_to_their_bound_action() {
        let engine = Engine::default();
        let hk = engine.config.binding(WindowAction::Maximize).unwrap();
        assert_eq!(engine.action_for(hk), Some(WindowAction::Maximize));
        assert_eq!(
            engine.action_for(Hotkey::new(Modifiers::SHIFT, KeyCode::Escape)),
            None
        );
    }
}
