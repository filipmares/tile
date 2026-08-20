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
        if action.moves_display() {
            return self.plan_display_move(action, window, screens);
        }

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

        // A repeat is only a repeat if the window is still where the last
        // cycle step left it, or if it happens to sit on the action's own
        // rectangle. Otherwise the user moved it, moved to another window, or
        // ran a different action in between — all of which restart the cycle.
        let continues = self.continues_cycle(action, window)
            || target.approx_eq(&window.frame, POSITION_TOLERANCE);

        // Anything that does not cycle behaves exactly as it always has: move
        // to the target, or report that there is nothing to do.
        if !action.cycles() || !self.config.cycles_sizes() {
            return settled();
        }

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

    /// Throws `window` to another display, keeping its current tile slot when
    /// the frame matches one, otherwise mapping it proportionally.
    ///
    /// Handles both kinds of display action. The relative throws step through
    /// [`Screen::geometrically_ordered`] and wrap; the absolute ones name a
    /// position in that same order, so "second display" means the same screen
    /// however many times it is pressed.
    fn plan_display_move(
        &self,
        action: WindowAction,
        window: &WindowSnapshot,
        screens: &[Screen],
    ) -> Plan {
        let Some(from) = Screen::best_match(screens, &window.frame) else {
            return Plan::NoOp(NoOpReason::NoScreen);
        };
        let dest = match action.display_index() {
            // An index past the end means that display is not plugged in,
            // which is a no-op rather than a move to the nearest one — a
            // window silently landing on the wrong screen would be worse.
            Some(index) => match Screen::geometrically_ordered(screens).get(index) {
                Some(screen) => *screen,
                None => return Plan::NoOp(NoOpReason::NoScreen),
            },
            None => match Screen::adjacent(screens, from, action.display_step()) {
                Some(screen) => screen,
                None => return Plan::NoOp(NoOpReason::AlreadyInPosition),
            },
        };
        if dest.id == from.id {
            return Plan::NoOp(NoOpReason::AlreadyInPosition);
        }
        let target = remap_slot(&self.config, window.frame, from, dest);
        if target.approx_eq(&window.frame, POSITION_TOLERANCE) {
            Plan::NoOp(NoOpReason::AlreadyInPosition)
        } else {
            Plan::Move {
                id: window.id,
                target,
            }
        }
    }

    /// Whether `window` is still exactly where the last cycle step for this
    /// same cycle and window put it.
    ///
    /// Compared by [`WindowAction::cycle_anchor`] rather than by the action
    /// itself, so stepping forwards with `CenterHalf` and backwards with
    /// `CenterHalfBack` continues one cycle instead of restarting it.
    fn continues_cycle(&self, action: WindowAction, window: &WindowSnapshot) -> bool {
        self.cycle.as_ref().is_some_and(|state| {
            state.window == window.id
                && state.action.cycle_anchor() == action.cycle_anchor()
                && state.applied.approx_eq(&window.frame, APPLIED_TOLERANCE)
        })
    }

    /// The next rectangle in `action`'s cycle, or `None` when every configured
    /// size lands on the rectangle the window already occupies.
    ///
    /// Actions where [`WindowAction::cycles_backwards`] holds walk the same
    /// sequence in reverse.
    fn next_cycle_rect(
        &self,
        action: WindowAction,
        window: &WindowSnapshot,
        screen: &Screen,
    ) -> Option<Rect> {
        let anchor = action.cycle_anchor();
        let rects: Vec<Rect> = self
            .config
            .cycle_sizes()
            .iter()
            .filter_map(|size| {
                anchor.cycle_rect(
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
        // sizes changed since the last press — restarts at one end.
        let len = rects.len();
        let current = rects
            .iter()
            .position(|r| r.approx_eq(&window.frame, APPLIED_TOLERANCE));

        // Skip any step that would leave the window exactly where it is, so a
        // duplicate-looking size (a half of a screen whose gaps make it equal
        // to another step) never turns a press into a no-op.
        if action.cycles_backwards() {
            let start = current.map_or(len - 1, |i| (i + len - 1) % len);
            (0..len)
                .map(|offset| rects[(start + len - offset) % len])
                .find(|r| !r.approx_eq(&window.frame, APPLIED_TOLERANCE))
        } else {
            let start = current.map_or(0, |i| i + 1);
            (0..len)
                .map(|offset| rects[(start + offset) % len])
                .find(|r| !r.approx_eq(&window.frame, APPLIED_TOLERANCE))
        }
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
        } else if action.moves_display() {
            self.history.record(window.id, window.frame, target);
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

/// Rebuilds `frame` on `dest`, preferring a recognised tile slot so a left
/// third stays a left third when the two work areas differ.
fn remap_slot(config: &Config, frame: Rect, from: &Screen, dest: &Screen) -> Rect {
    let sizes = config.size_options();
    for action in WindowAction::ALL {
        if !action.cycles() {
            continue;
        }
        for size in config.cycle_sizes() {
            let Some(src) = action.cycle_rect(
                from.work_area,
                &config.gaps,
                from.is_primary,
                size.fraction(),
            ) else {
                continue;
            };
            if !src.approx_eq(&frame, POSITION_TOLERANCE) {
                continue;
            }
            if let Some(dst) = action.cycle_rect(
                dest.work_area,
                &config.gaps,
                dest.is_primary,
                size.fraction(),
            ) {
                return dst;
            }
        }
    }
    for action in WindowAction::ALL {
        // Only actions that name a fixed region of the work area describe a
        // "slot" that can be recognised and rebuilt elsewhere. An action whose
        // rectangle is derived from the window's own frame does not: it
        // reproduces that frame for *any* window, so it would match every time
        // and defeat the proportional fallback entirely.
        if action.uses_history() || action.moves_display() || action.depends_on_current_frame() {
            continue;
        }
        let Some(src) =
            action.target_rect(from.work_area, &config.gaps, frame, from.is_primary, sizes)
        else {
            continue;
        };
        if !src.approx_eq(&frame, POSITION_TOLERANCE) {
            continue;
        }
        if let Some(dst) =
            action.target_rect(dest.work_area, &config.gaps, frame, dest.is_primary, sizes)
        {
            return dst;
        }
    }
    map_proportionally(frame, from.work_area, dest.work_area)
}

/// Re-expresses `frame` as fractions of `from` and applies them to `to`.
///
/// Working in fractions rather than pixels is what makes this correct across a
/// mixed-DPI boundary: `frame` and `from` come from the same display, so the
/// unit they are expressed in — physical pixels on Windows, points on macOS —
/// cancels out. [`Screen::scale_factor`] is never consulted and no raw pixel
/// value is carried across a display edge.
///
/// The result is clamped into `to`, because a window that overhung its source
/// display would otherwise keep overhanging the destination by the same
/// proportion.
fn map_proportionally(frame: Rect, from: Rect, to: Rect) -> Rect {
    if from.width <= 0.0 || from.height <= 0.0 {
        return to;
    }
    let x = to.x + (frame.x - from.x) / from.width * to.width;
    let y = to.y + (frame.y - from.y) / from.height * to.height;
    let width = (frame.width / from.width * to.width).max(0.0);
    let height = (frame.height / from.height * to.height).max(0.0);
    Rect::new(x, y, width, height).clamped_within(to).rounded()
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

    /// Up and Down share one cycle, so walking forward twice and back once
    /// must land on the rectangle the first press produced. If they kept
    /// separate cycle state, Down would restart instead of stepping back.
    #[test]
    fn up_and_down_walk_one_shared_centered_cycle() {
        let mut engine = Engine::default();
        let mut win = window();

        // Forward to the centred half, then to the centred two thirds.
        let apply = |engine: &mut Engine, action, win: &mut WindowSnapshot| match engine.plan(
            action,
            win,
            &[screen()],
        ) {
            Plan::Move { target, .. } => {
                engine.commit(action, win, target);
                win.frame = target;
                target
            }
            other => panic!("expected a move, got {other:?}"),
        };

        let half = apply(&mut engine, WindowAction::CenterHalf, &mut win);
        assert_eq!(half, Rect::new(480.0, 0.0, 960.0, 1040.0));

        let two_thirds = apply(&mut engine, WindowAction::CenterHalf, &mut win);
        assert_eq!(two_thirds, Rect::new(320.0, 0.0, 1280.0, 1040.0));

        // Back one step returns to the half rather than starting over.
        let back = apply(&mut engine, WindowAction::CenterHalfBack, &mut win);
        assert_eq!(back, half, "Down must step back into Up's cycle");
    }

    /// Stepping back from the first size wraps to the last, so Down is usable
    /// as "make it smaller" straight from a fresh centred half.
    #[test]
    fn stepping_back_from_the_first_size_wraps_to_the_last() {
        let mut engine = Engine::default();
        let mut win = window();

        let Plan::Move { target: half, .. } =
            engine.plan(WindowAction::CenterHalf, &win, &[screen()])
        else {
            panic!("expected a move");
        };
        engine.commit(WindowAction::CenterHalf, &win, half);
        win.frame = half;

        let Plan::Move { target: back, .. } =
            engine.plan(WindowAction::CenterHalfBack, &win, &[screen()])
        else {
            panic!("expected a move");
        };
        // Defaults are ½, ⅔, ⅓; stepping back from ½ lands on ⅓.
        assert_eq!(back, Rect::new(640.0, 0.0, 640.0, 1040.0));
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

    fn dual_screens() -> [Screen; 2] {
        [
            screen(),
            Screen {
                id: "secondary".into(),
                frame: Rect::new(1920.0, 0.0, 1280.0, 800.0),
                work_area: Rect::new(1920.0, 0.0, 1280.0, 760.0),
                scale_factor: 1.0,
                is_primary: false,
            },
        ]
    }

    #[test]
    fn next_display_preserves_a_left_half_slot() {
        let engine = Engine::default();
        let win = WindowSnapshot {
            id: 1,
            frame: Rect::new(0.0, 0.0, 960.0, 1040.0),
        };
        assert_eq!(
            engine.plan(WindowAction::NextDisplay, &win, &dual_screens()),
            Plan::Move {
                id: 1,
                target: Rect::new(1920.0, 0.0, 640.0, 760.0)
            }
        );
    }

    #[test]
    fn next_display_preserves_a_cycled_left_third() {
        let engine = Engine::default();
        let win = WindowSnapshot {
            id: 1,
            frame: Rect::new(0.0, 0.0, 640.0, 1040.0),
        };
        assert_eq!(
            engine.plan(WindowAction::NextDisplay, &win, &dual_screens()),
            Plan::Move {
                id: 1,
                target: Rect::new(1920.0, 0.0, 427.0, 760.0)
            }
        );
    }

    #[test]
    fn previous_display_wraps_from_the_leftmost_screen() {
        let engine = Engine::default();
        let win = WindowSnapshot {
            id: 1,
            frame: Rect::new(0.0, 0.0, 960.0, 1040.0),
        };
        assert_eq!(
            engine.plan(WindowAction::PreviousDisplay, &win, &dual_screens()),
            Plan::Move {
                id: 1,
                target: Rect::new(1920.0, 0.0, 640.0, 760.0)
            }
        );
    }

    #[test]
    fn display_throw_is_a_no_op_on_a_single_screen() {
        let engine = Engine::default();
        assert_eq!(
            engine.plan(WindowAction::NextDisplay, &window(), &[screen()]),
            Plan::NoOp(NoOpReason::AlreadyInPosition)
        );
    }

    #[test]
    fn a_floating_window_maps_proportionally_across_displays() {
        let engine = Engine::default();
        let win = WindowSnapshot {
            id: 1,
            frame: Rect::new(192.0, 104.0, 384.0, 208.0),
        };
        let Plan::Move { target, .. } =
            engine.plan(WindowAction::NextDisplay, &win, &dual_screens())
        else {
            panic!("expected a move");
        };
        assert_eq!(target, Rect::new(2048.0, 76.0, 256.0, 152.0));
    }

    #[test]
    fn display_throw_resets_size_cycle() {
        let mut engine = Engine::default();
        let screens = dual_screens();
        let mut win = apply_on(&mut engine, WindowAction::LeftHalf, window(), &screens);
        win = apply_on(&mut engine, WindowAction::LeftHalf, win, &screens);
        assert_eq!(win.frame, Rect::new(0.0, 0.0, 1280.0, 1040.0));

        win = apply_on(&mut engine, WindowAction::NextDisplay, win, &screens);
        win = apply_on(&mut engine, WindowAction::LeftHalf, win, &screens);
        assert_eq!(
            win.frame,
            Rect::new(1920.0, 0.0, 640.0, 760.0),
            "Left after a throw must start a fresh cycle, not continue two-thirds"
        );
    }

    // ---------------------------------------------------------------------
    // Displays named outright
    // ---------------------------------------------------------------------

    fn display(id: &str, frame: Rect, scale: f64) -> Screen {
        Screen {
            id: id.into(),
            frame,
            work_area: frame,
            scale_factor: scale,
            is_primary: id == "left",
        }
    }

    /// Three 1080p displays in a row, deliberately listed out of geometric
    /// order so the tests prove the ordering rather than the vec literal.
    fn row_of_three() -> Vec<Screen> {
        vec![
            display("right", Rect::new(3840.0, 0.0, 1920.0, 1080.0), 1.0),
            display("left", Rect::new(0.0, 0.0, 1920.0, 1080.0), 1.0),
            display("middle", Rect::new(1920.0, 0.0, 1920.0, 1080.0), 1.0),
        ]
    }

    /// The mixed-DPI desk the issue warns about: a 2x Retina panel left of a
    /// 1x 1080p monitor. Their work areas differ in raw size by a factor that
    /// has nothing to do with either `scale_factor`, because each backend
    /// reports in its own unit.
    fn mixed_dpi() -> Vec<Screen> {
        vec![
            display("retina", Rect::new(0.0, 0.0, 2880.0, 1800.0), 2.0),
            display("hd", Rect::new(2880.0, 0.0, 1920.0, 1080.0), 1.0),
        ]
    }

    fn moved_to(plan: Plan) -> Rect {
        match plan {
            Plan::Move { target, .. } => target,
            other => panic!("expected a move, got {other:?}"),
        }
    }

    #[test]
    fn a_named_display_is_picked_by_geometric_order() {
        let screens = row_of_three();
        let engine = Engine::default();
        let win = WindowSnapshot {
            id: 1,
            frame: Rect::new(480.0, 270.0, 960.0, 540.0),
        };
        // "Third display" is the rightmost, whatever order the backend
        // enumerated the screens in.
        assert_eq!(
            moved_to(engine.plan(WindowAction::ThirdDisplay, &win, &screens)).x,
            4320.0
        );
        assert_eq!(
            moved_to(engine.plan(WindowAction::SecondDisplay, &win, &screens)).x,
            2400.0
        );
    }

    #[test]
    fn a_named_display_the_window_is_already_on_does_nothing() {
        let engine = Engine::default();
        let win = WindowSnapshot {
            id: 1,
            frame: Rect::new(480.0, 270.0, 960.0, 540.0),
        };
        assert_eq!(
            engine.plan(WindowAction::FirstDisplay, &win, &row_of_three()),
            Plan::NoOp(NoOpReason::AlreadyInPosition)
        );
    }

    /// A display that is not plugged in is a no-op rather than a move to the
    /// nearest one: a window silently landing on the wrong screen would be
    /// harder to understand than nothing happening.
    #[test]
    fn a_named_display_that_is_absent_does_nothing() {
        let engine = Engine::default();
        let win = WindowSnapshot {
            id: 1,
            frame: Rect::new(480.0, 270.0, 960.0, 540.0),
        };
        assert_eq!(
            engine.plan(WindowAction::FourthDisplay, &win, &row_of_three()),
            Plan::NoOp(NoOpReason::NoScreen)
        );
        for action in [
            WindowAction::SecondDisplay,
            WindowAction::ThirdDisplay,
            WindowAction::FourthDisplay,
        ] {
            assert_eq!(
                engine.plan(action, &window(), &[screen()]),
                Plan::NoOp(NoOpReason::NoScreen),
                "{action} on a single display"
            );
        }
    }

    #[test]
    fn a_named_display_keeps_a_recognised_slot() {
        let engine = Engine::default();
        // A left half of the primary display, thrown at the second one.
        let win = WindowSnapshot {
            id: 1,
            frame: Rect::new(0.0, 0.0, 960.0, 1040.0),
        };
        assert_eq!(
            engine.plan(WindowAction::SecondDisplay, &win, &dual_screens()),
            Plan::Move {
                id: 1,
                target: Rect::new(1920.0, 0.0, 640.0, 760.0)
            },
            "a named display must remap the slot exactly as a relative throw does"
        );
    }

    /// The acceptance criterion the issue calls the trap: an absolute pixel
    /// move would leave this window hanging off the smaller display.
    #[test]
    fn a_named_display_preserves_proportions_across_mixed_dpi() {
        let screens = mixed_dpi();
        let engine = Engine::default();
        // The right half of the 2880x1800 Retina panel.
        let win = WindowSnapshot {
            id: 1,
            frame: Rect::new(1440.0, 0.0, 1440.0, 1800.0),
        };
        let target = moved_to(engine.plan(WindowAction::SecondDisplay, &win, &screens));
        // Still a right half, now of the 1920x1080 display, and entirely on
        // it — an absolute move would have left it 1440 wide and overhanging.
        assert_eq!(target, Rect::new(3840.0, 0.0, 960.0, 1080.0));
        assert!(target.max_x() <= 4800.0);
    }

    /// Nothing may land off the destination, including a window that already
    /// overhung the display it came from.
    #[test]
    fn a_display_move_never_leaves_a_window_off_the_destination() {
        let screens = mixed_dpi();
        let engine = Engine::default();
        for frame in [
            Rect::new(0.0, 0.0, 2880.0, 1800.0),
            Rect::new(2600.0, 1600.0, 600.0, 400.0),
            Rect::new(-200.0, -100.0, 900.0, 700.0),
        ] {
            let win = WindowSnapshot { id: 1, frame };
            let target = moved_to(engine.plan(WindowAction::SecondDisplay, &win, &screens));
            assert!(
                target.x >= 2880.0
                    && target.y >= 0.0
                    && target.max_x() <= 4800.0
                    && target.max_y() <= 1080.0,
                "{frame:?} landed outside the destination display: {target:?}"
            );
        }
    }

    #[test]
    fn a_named_display_throw_resets_the_size_cycle() {
        let mut engine = Engine::default();
        let screens = dual_screens();
        let mut win = apply_on(&mut engine, WindowAction::LeftHalf, window(), &screens);
        win = apply_on(&mut engine, WindowAction::LeftHalf, win, &screens);
        assert_eq!(win.frame, Rect::new(0.0, 0.0, 1280.0, 1040.0));

        win = apply_on(&mut engine, WindowAction::SecondDisplay, win, &screens);
        win = apply_on(&mut engine, WindowAction::LeftHalf, win, &screens);
        assert_eq!(
            win.frame,
            Rect::new(1920.0, 0.0, 640.0, 760.0),
            "Left after a named throw must start a fresh cycle"
        );
    }

    /// Regression: the incremental actions reproduce the window's own frame
    /// when their step is refused, so if slot-matching considered them every
    /// window would look like it was already "in a slot" and the proportional
    /// fallback would never run. A free-floating window must still map by
    /// proportion.
    #[test]
    fn incremental_actions_are_not_mistaken_for_tile_slots() {
        let engine = Engine::default();
        for action in WindowAction::ALL
            .into_iter()
            .filter(|a| a.depends_on_current_frame())
        {
            assert!(
                !action.moves_display() && !action.uses_history(),
                "{action} is excluded from slot matching for the wrong reason"
            );
        }
        // A window at an awkward size that matches no tile slot at all.
        let win = WindowSnapshot {
            id: 1,
            frame: Rect::new(192.0, 104.0, 384.0, 208.0),
        };
        let target = moved_to(engine.plan(WindowAction::NextDisplay, &win, &dual_screens()));
        assert_eq!(
            target,
            Rect::new(2048.0, 76.0, 256.0, 152.0),
            "a free-floating window must map proportionally, not be treated as a slot"
        );
    }

    #[test]
    fn restore_undoes_a_named_display_move() {
        let screens = row_of_three();
        let mut engine = Engine::default();
        let original = Rect::new(480.0, 270.0, 960.0, 540.0);
        let win = WindowSnapshot {
            id: 1,
            frame: original,
        };
        let target = moved_to(engine.plan(WindowAction::ThirdDisplay, &win, &screens));
        engine.commit(WindowAction::ThirdDisplay, &win, target);

        let moved = WindowSnapshot {
            id: 1,
            frame: target,
        };
        assert_eq!(
            engine.plan(WindowAction::Restore, &moved, &screens),
            Plan::Move {
                id: 1,
                target: original
            }
        );
    }
}
