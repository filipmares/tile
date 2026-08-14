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

pub use action::{ParseActionError, WindowAction, WindowFamily};
pub use config::{
    Config, ConfigError, Conflict, CycleSize, Gaps, SharedEdges, SizeOptions,
    SubsequentExecutionMode, CONFIG_FILE_NAME, MAX_GAP,
};
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
///
/// # Cycling state
///
/// Repeating a shortcut cycles a window through sizes, which means the engine
/// has to know whether *Tile* is what put the window where it is. Two designs
/// were possible:
///
/// * **Stateless** — infer the step purely from the current rectangle ("it is
///   a half, so go to a third"). It survives a restart and needs no
///   bookkeeping, but it cannot tell a window Tile placed from one the user
///   dragged into the same shape, so a hand-sized window jumps unexpectedly.
/// * **Stored state** — remember the rectangle the last cycle step produced,
///   and only continue while the window is still exactly there.
///
/// Tile stores state, for the same reason [`WindowHistory`] does: "the user
/// moved it" and "we moved it" have to be told apart, and only a remembered
/// rectangle can do that. The stored value is deliberately *just* the last
/// applied rectangle, not a step counter — the step is recovered by matching
/// that rectangle against the configured sizes, which keeps [`Engine::commit`]
/// free of any dependency on the screen list and means a config change between
/// two presses can never leave a stale index pointing at a size that no longer
/// exists.
///
/// The one place the stateless reading is used is as a fallback: if there is
/// no stored state but the window already occupies the action's own rectangle
/// (after a restart, say), a repeat still starts cycling. That inherits the
/// stateless coincidence caveat, but only for the exact rectangle the shortcut
/// itself produces, where advancing is what the user asked for anyway.
#[derive(Debug, Default)]
pub struct Engine {
    pub config: Config,
    pub history: WindowHistory,
    cycle: Option<CycleState>,
}

/// The last cycle step Tile applied. Cycling continues only while the window
/// is still exactly where this says Tile left it.
#[derive(Debug, Clone, PartialEq)]
struct CycleState {
    window: WindowId,
    action: WindowAction,
    applied: Rect,
}

/// How far a window may sit from a computed rectangle and still count as being
/// in that position. Matches the tolerance [`Engine::plan`] has always used to
/// decide an action is already satisfied.
const POSITION_TOLERANCE: f64 = 1.0;

/// How far a window may drift from the rectangle Tile applied and still count
/// as untouched by the user. Backends round and clamp, so this is looser than
/// [`POSITION_TOLERANCE`]; it is the same value [`WindowHistory`] uses.
const APPLIED_TOLERANCE: f64 = 2.0;

impl Engine {
    pub fn new(config: Config) -> Self {
        Self {
            config,
            history: WindowHistory::new(),
            cycle: None,
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

        let Some(target) = action.target_rect(
            screen.work_area,
            &self.config.gaps,
            window.frame,
            screen.is_primary,
            self.config.size_options(),
        ) else {
            return Plan::NoOp(NoOpReason::NoHistory);
        };

        let settled = || {
            if target.approx_eq(&window.frame, POSITION_TOLERANCE) {
                Plan::NoOp(NoOpReason::AlreadyInPosition)
            } else {
                Plan::Move {
                    id: window.id,
                    target,
                }
            }
        };

        // Anything that does not cycle behaves exactly as it always has: move
        // to the target, or report that there is nothing to do.
        if !action.cycles() || !self.config.cycles_sizes() {
            return settled();
        }

        // A repeat is only a repeat if the window is still where the last
        // cycle step left it, or if it happens to sit on the action's own
        // rectangle. Otherwise the user moved it, moved to another window, or
        // ran a different action in between — all of which restart the cycle.
        let continues = self.continues_cycle(action, window)
            || target.approx_eq(&window.frame, POSITION_TOLERANCE);
        if !continues {
            return Plan::Move {
                id: window.id,
                target,
            };
        }

        match self.next_cycle_rect(action, window, screen) {
            Some(next) => Plan::Move {
                id: window.id,
                target: next,
            },
            None => Plan::NoOp(NoOpReason::AlreadyInPosition),
        }
    }

    /// Whether `window` is still exactly where the last cycle step for this
    /// same action and window put it.
    fn continues_cycle(&self, action: WindowAction, window: &WindowSnapshot) -> bool {
        self.cycle.as_ref().is_some_and(|state| {
            state.window == window.id
                && state.action == action
                && state.applied.approx_eq(&window.frame, APPLIED_TOLERANCE)
        })
    }

    /// The next rectangle in `action`'s cycle, or `None` when every configured
    /// size lands on the rectangle the window already occupies.
    fn next_cycle_rect(
        &self,
        action: WindowAction,
        window: &WindowSnapshot,
        screen: &Screen,
    ) -> Option<Rect> {
        let rects: Vec<Rect> = self
            .config
            .cycle_sizes()
            .iter()
            .filter_map(|size| {
                action.cycle_rect(
                    screen.work_area,
                    &self.config.gaps,
                    screen.is_primary,
                    size.fraction(),
                )
            })
            .collect();
        if rects.is_empty() {
            return None;
        }

        // Recover the current step from geometry rather than from a stored
        // counter. A window that matches no configured size — because the
        // sizes changed since the last press — restarts at the first one.
        let start = rects
            .iter()
            .position(|r| r.approx_eq(&window.frame, APPLIED_TOLERANCE))
            .map_or(0, |i| i + 1);

        // Skip any step that would leave the window exactly where it is, so a
        // duplicate-looking size (a half of a screen whose gaps make it equal
        // to another step) never turns a press into a no-op.
        (0..rects.len())
            .map(|offset| rects[(start + offset) % rects.len()])
            .find(|r| !r.approx_eq(&window.frame, APPLIED_TOLERANCE))
    }

    /// Records a successful move so that `Restore` can undo it.
    ///
    /// The cycle state is rewritten on every move, which is what makes a
    /// different action or a different window reset the cycle. Restore is
    /// unaffected by cycling: [`WindowHistory::record`] only takes a new
    /// original when the window is *not* where Tile last left it, so an
    /// arbitrarily long cycle keeps the pre-Tile frame.
    pub fn commit(&mut self, action: WindowAction, window: &WindowSnapshot, target: Rect) {
        if action.uses_history() {
            self.history.forget(window.id);
            self.cycle = None;
        } else {
            self.history.record(window.id, window.frame, target);
            self.cycle = if action.cycles() {
                Some(CycleState {
                    window: window.id,
                    action,
                    applied: target,
                })
            } else {
                None
            };
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
    fn repeating_an_action_cycles_to_the_next_size() {
        let engine = Engine::default();
        let win = WindowSnapshot {
            id: 1,
            frame: Rect::new(0.0, 0.0, 960.0, 1040.0),
        };
        assert_eq!(
            engine.plan(WindowAction::LeftHalf, &win, &[screen()]),
            Plan::Move {
                id: 1,
                // Two thirds of 1920, the second default cycle size.
                target: Rect::new(0.0, 0.0, 1280.0, 1040.0)
            }
        );
    }

    #[test]
    fn repeating_an_action_is_a_no_op_when_cycling_is_off() {
        let mut engine = Engine::default();
        engine.config.subsequent_execution_mode = SubsequentExecutionMode::DoNothing;
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
    fn repeating_an_action_is_a_no_op_when_no_sizes_are_selected() {
        let mut engine = Engine::default();
        engine.config.cycle_sizes.clear();
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
    fn a_full_cycle_visits_every_size_and_wraps_around() {
        let mut engine = Engine::default();
        let half = Rect::new(0.0, 0.0, 960.0, 1040.0);
        let two_thirds = Rect::new(0.0, 0.0, 1280.0, 1040.0);
        let third = Rect::new(0.0, 0.0, 640.0, 1040.0);

        // Two laps, to prove the wrap-around is not a one-off.
        assert_eq!(
            press_repeatedly(&mut engine, WindowAction::LeftHalf, window(), 7),
            vec![half, two_thirds, third, half, two_thirds, third, half]
        );
    }

    #[test]
    fn every_cycling_action_cycles_off_its_own_rectangle() {
        // The first press of each cycling action must land on the same
        // rectangle it produced before cycling existed, and the second must
        // move somewhere else.
        for action in WindowAction::ALL.iter().copied().filter(|a| a.cycles()) {
            let mut engine = Engine::default();
            let steps = press_repeatedly(&mut engine, action, window(), 2);
            let plain = action
                .target_rect(
                    screen().work_area,
                    &engine.config.gaps,
                    window().frame,
                    true,
                    engine.config.size_options(),
                )
                .unwrap();
            assert_eq!(steps[0], plain, "{action} moved on its first press");
            assert_ne!(steps[0], steps[1], "{action} did not cycle");
        }
    }

    #[test]
    fn corners_cycle_their_width_and_keep_their_height() {
        let mut engine = Engine::default();
        let steps = press_repeatedly(&mut engine, WindowAction::BottomRight, window(), 3);
        assert_eq!(steps[0], Rect::new(960.0, 520.0, 960.0, 520.0));
        assert_eq!(steps[1], Rect::new(640.0, 520.0, 1280.0, 520.0));
        assert_eq!(steps[2], Rect::new(1280.0, 520.0, 640.0, 520.0));
    }

    #[test]
    fn an_intervening_action_resets_the_cycle() {
        let mut engine = Engine::default();
        let mut win = window();

        win = apply(&mut engine, WindowAction::LeftHalf, win);
        win = apply(&mut engine, WindowAction::LeftHalf, win);
        assert_eq!(win.frame, Rect::new(0.0, 0.0, 1280.0, 1040.0));

        // A different action, then back: the cycle starts over at a half.
        win = apply(&mut engine, WindowAction::RightHalf, win);
        win = apply(&mut engine, WindowAction::LeftHalf, win);
        assert_eq!(win.frame, Rect::new(0.0, 0.0, 960.0, 1040.0));
    }

    #[test]
    fn a_different_window_does_not_inherit_the_cycle() {
        let mut engine = Engine::default();
        let first = apply(&mut engine, WindowAction::LeftHalf, window());
        assert_eq!(first.frame, Rect::new(0.0, 0.0, 960.0, 1040.0));

        let other = WindowSnapshot {
            id: 2,
            frame: Rect::new(400.0, 400.0, 500.0, 500.0),
        };
        let moved = apply(&mut engine, WindowAction::LeftHalf, other);
        assert_eq!(moved.frame, Rect::new(0.0, 0.0, 960.0, 1040.0));
    }

    #[test]
    fn a_user_initiated_move_resets_the_cycle() {
        let mut engine = Engine::default();
        let win = apply(&mut engine, WindowAction::LeftHalf, window());

        // The user drags the window somewhere Tile did not put it, then
        // presses the same shortcut again: a half, not the next cycle step.
        let dragged = WindowSnapshot {
            id: win.id,
            frame: Rect::new(500.0, 500.0, 400.0, 300.0),
        };
        let moved = apply(&mut engine, WindowAction::LeftHalf, dragged);
        assert_eq!(moved.frame, Rect::new(0.0, 0.0, 960.0, 1040.0));
    }

    #[test]
    fn restore_after_a_long_cycle_returns_the_original_frame() {
        let mut engine = Engine::default();
        let original = window();
        let mut win = original.clone();
        for _ in 0..17 {
            win = apply(&mut engine, WindowAction::LeftHalf, win);
        }
        assert_ne!(win.frame, original.frame);
        assert_eq!(
            engine.plan(WindowAction::Restore, &win, &[screen()]),
            Plan::Move {
                id: original.id,
                target: original.frame
            }
        );
    }

    #[test]
    fn a_restore_in_the_middle_of_a_cycle_starts_the_next_one_over() {
        let mut engine = Engine::default();
        let original = window();
        let mut win = apply(&mut engine, WindowAction::LeftHalf, original.clone());
        win = apply(&mut engine, WindowAction::LeftHalf, win);
        win = apply(&mut engine, WindowAction::Restore, win);
        assert_eq!(win.frame, original.frame);

        let moved = apply(&mut engine, WindowAction::LeftHalf, win);
        assert_eq!(moved.frame, Rect::new(0.0, 0.0, 960.0, 1040.0));
    }

    #[test]
    fn cycling_respects_gaps_and_an_offset_work_area() {
        let mut engine = Engine::default();
        engine.config.gaps = Gaps {
            window: 10.0,
            edge_top: 20.0,
            edge_bottom: 20.0,
            edge_left: 20.0,
            edge_right: 20.0,
            skip_top_edge: false,
            main_screen_only: false,
        };
        // A secondary-style work area with a non-zero origin.
        let offset = Screen {
            id: "offset".into(),
            frame: Rect::new(1980.0, 30.0, 1860.0, 1010.0),
            work_area: Rect::new(1980.0, 30.0, 1860.0, 1010.0),
            scale_factor: 1.0,
            is_primary: true,
        };
        let start = WindowSnapshot {
            id: 3,
            frame: Rect::new(2200.0, 200.0, 400.0, 400.0),
        };

        let steps = press_repeatedly_on(&mut engine, WindowAction::LeftHalf, start, &[offset], 4);
        // Left edge and vertical extent are the same at every step: only the
        // right edge, which is shared with a neighbour, moves.
        for step in &steps {
            assert_eq!(step.x, 2000.0, "left screen-edge gap lost in {step:?}");
            assert_eq!(step.y, 50.0);
            assert_eq!(step.height, 970.0);
        }
        assert_eq!(steps[0].width, 905.0); // half of 1860, less 20 edge + 5 shared
        assert_eq!(steps[1].width, 1215.0); // two thirds
        assert_eq!(steps[2].width, 595.0); // one third
        assert_eq!(steps[3].width, steps[0].width); // wrapped
    }

    #[test]
    fn a_non_cycling_action_still_reports_already_in_position() {
        let engine = Engine::default();
        let win = WindowSnapshot {
            id: 1,
            frame: Rect::new(0.0, 0.0, 640.0, 1040.0),
        };
        assert_eq!(
            engine.plan(WindowAction::FirstThird, &win, &[screen()]),
            Plan::NoOp(NoOpReason::AlreadyInPosition)
        );
    }

    #[test]
    fn a_single_configured_size_cycles_to_nothing() {
        let mut engine = Engine::default();
        engine.config.cycle_sizes = vec![CycleSize::OneHalf];
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
    fn a_cycle_without_the_first_size_still_starts_from_the_actions_rectangle() {
        let mut engine = Engine::default();
        engine.config.cycle_sizes = vec![CycleSize::OneThird, CycleSize::TwoThirds];
        engine.config.normalize();
        let steps = press_repeatedly(&mut engine, WindowAction::LeftHalf, window(), 4);
        assert_eq!(
            steps,
            vec![
                Rect::new(0.0, 0.0, 960.0, 1040.0),  // the half it always was
                Rect::new(0.0, 0.0, 1280.0, 1040.0), // two thirds
                Rect::new(0.0, 0.0, 640.0, 1040.0),  // one third
                Rect::new(0.0, 0.0, 1280.0, 1040.0), // wrapped
            ]
        );
    }

    #[test]
    fn a_window_already_in_position_after_a_restart_still_cycles() {
        // No stored state, but the window sits exactly on the action's own
        // rectangle: the stateless fallback picks the cycle up from there.
        let engine = Engine::default();
        let win = WindowSnapshot {
            id: 9,
            frame: Rect::new(0.0, 0.0, 960.0, 1040.0),
        };
        assert!(engine.cycle.is_none());
        assert_eq!(
            engine.plan(WindowAction::LeftHalf, &win, &[screen()]),
            Plan::Move {
                id: 9,
                target: Rect::new(0.0, 0.0, 1280.0, 1040.0)
            }
        );
    }

    /// Runs one action end to end — plan, "apply", commit — and returns the
    /// window as the backend would report it afterwards.
    fn apply(engine: &mut Engine, action: WindowAction, win: WindowSnapshot) -> WindowSnapshot {
        apply_on(engine, action, win, &[screen()])
    }

    fn apply_on(
        engine: &mut Engine,
        action: WindowAction,
        win: WindowSnapshot,
        screens: &[Screen],
    ) -> WindowSnapshot {
        match engine.plan(action, &win, screens) {
            Plan::Move { id, target } => {
                engine.commit(action, &win, target);
                WindowSnapshot { id, frame: target }
            }
            Plan::NoOp(reason) => panic!("expected a move for {action}, got {reason:?}"),
        }
    }

    fn press_repeatedly(
        engine: &mut Engine,
        action: WindowAction,
        start: WindowSnapshot,
        times: usize,
    ) -> Vec<Rect> {
        press_repeatedly_on(engine, action, start, &[screen()], times)
    }

    fn press_repeatedly_on(
        engine: &mut Engine,
        action: WindowAction,
        start: WindowSnapshot,
        screens: &[Screen],
        times: usize,
    ) -> Vec<Rect> {
        let mut win = start;
        let mut seen = Vec::new();
        for _ in 0..times {
            win = apply_on(engine, action, win, screens);
            seen.push(win.frame);
        }
        seen
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
