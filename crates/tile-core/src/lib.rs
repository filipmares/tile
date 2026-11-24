//! `tile-core` — the platform-independent heart of Tile.
//!
//! This crate deliberately contains **no** platform code and no I/O beyond
//! JSON (de)serialization, so all of the interesting logic can be unit tested
//! on any host. Platform backends live in `tile-platform` and are driven
//! through the [`Engine`] in this crate.

pub mod action;
pub mod animation;
pub mod compose;
pub mod config;
pub mod geometry;
pub mod history;
pub mod hotkey;

pub use action::{
    ActionRequest, ActionTarget, Corner, ParseActionError, ShortcutGroup, WindowAction,
};
pub use animation::{AnimationParams, Animator};
pub use compose::CornerComposer;
pub use config::{
    AnimationConfig, Config, ConfigError, CycleSize, Gaps, LenientConfig, SharedEdges,
    SubsequentExecutionMode, BASE_MODIFIERS, CONFIG_FILE_NAME, CONFIG_SCHEMA_VERSION,
    DEFAULT_ANIMATION_FPS, DISPLAY_MODIFIERS, MAX_GAP,
};
pub use geometry::{Direction, Rect, Screen};
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
#[derive(Debug, Default)]
pub struct Engine {
    pub config: Config,
    pub history: WindowHistory,
    cycle: Option<CycleState>,
}

/// The last cycle step Tile applied. Cycling continues only while the window
/// is still where this says Tile left it.
#[derive(Debug, Clone, PartialEq)]
struct CycleState {
    window: WindowId,
    target: ActionTarget,
    applied: Rect,
}

/// How far a window may sit from a computed rectangle and still count as being
/// in that position.
const POSITION_TOLERANCE: f64 = 1.0;

/// How far a window may drift from the rectangle Tile applied and still count
/// as untouched by the user.
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
    pub fn plan(
        &self,
        action: impl Into<ActionTarget>,
        window: &WindowSnapshot,
        screens: &[Screen],
    ) -> Plan {
        self.plan_target(action.into(), window, screens)
    }

    /// Plans a request as it arrived from a hotkey or the tray menu.
    pub fn plan_request(
        &self,
        request: ActionRequest,
        window: &WindowSnapshot,
        screens: &[Screen],
    ) -> Plan {
        if request.composed {
            if let Some(plan) = self.plan_composed(request.target, window, screens) {
                return plan;
            }
        }
        self.plan_target(request.target, window, screens)
    }

    /// A vertical step composed on a side (`Top ↔ Full ↔ Bottom`), keeping the
    /// width the window already has on that side.
    fn plan_composed(
        &self,
        target: ActionTarget,
        window: &WindowSnapshot,
        screens: &[Screen],
    ) -> Option<Plan> {
        let side = target.composition_side()?;
        let Some(screen) = Screen::best_match(screens, &window.frame) else {
            return Some(Plan::NoOp(NoOpReason::NoScreen));
        };
        let fraction = self.side_width(side, window.frame, screen).unwrap_or(0.5);
        let rect = target.cycle_rect(
            screen.work_area,
            &self.config.gaps,
            screen.is_primary,
            fraction,
        )?;
        Some(if rect.approx_eq(&window.frame, POSITION_TOLERANCE) {
            Plan::NoOp(NoOpReason::AlreadyInPosition)
        } else {
            Plan::Move {
                id: window.id,
                target: rect,
            }
        })
    }

    /// The width fraction `frame` occupies on `side`, recognised as the
    /// full-height side or either of its corners at a half or any configured
    /// cycle size.
    fn side_width(&self, side: WindowAction, frame: Rect, screen: &Screen) -> Option<f64> {
        let (top, bottom) = match side {
            WindowAction::LeftHalf => (Corner::TopLeft, Corner::BottomLeft),
            WindowAction::RightHalf => (Corner::TopRight, Corner::BottomRight),
            _ => return None,
        };
        let candidates = [
            ActionTarget::Action(side),
            ActionTarget::Corner(top),
            ActionTarget::Corner(bottom),
        ];
        std::iter::once(0.5)
            .chain(self.config.cycle_sizes().iter().map(|size| size.fraction()))
            .find(|&fraction| {
                candidates.into_iter().any(|candidate| {
                    candidate
                        .cycle_rect(
                            screen.work_area,
                            &self.config.gaps,
                            screen.is_primary,
                            fraction,
                        )
                        .is_some_and(|r| r.approx_eq(&frame, APPLIED_TOLERANCE))
                })
            })
    }

    fn plan_target(
        &self,
        target: ActionTarget,
        window: &WindowSnapshot,
        screens: &[Screen],
    ) -> Plan {
        if target.moves_display() {
            return self.plan_display_move(target, window, screens);
        }

        if target.uses_history() {
            return match self.history.peek(window.id) {
                Some(rect) => Plan::Move {
                    id: window.id,
                    target: rect,
                },
                None => Plan::NoOp(NoOpReason::NoHistory),
            };
        }

        let Some(screen) = Screen::best_match(screens, &window.frame) else {
            return Plan::NoOp(NoOpReason::NoScreen);
        };

        let Some(rect) = target.target_rect(screen.work_area, &self.config.gaps, screen.is_primary)
        else {
            return Plan::NoOp(NoOpReason::NoHistory);
        };

        let settled = || {
            if rect.approx_eq(&window.frame, POSITION_TOLERANCE) {
                Plan::NoOp(NoOpReason::AlreadyInPosition)
            } else {
                Plan::Move {
                    id: window.id,
                    target: rect,
                }
            }
        };

        let continues = self.continues_cycle(target, window)
            || rect.approx_eq(&window.frame, POSITION_TOLERANCE);

        if !target.cycles() || !self.config.cycles_sizes() {
            return settled();
        }

        if !continues {
            return Plan::Move {
                id: window.id,
                target: rect,
            };
        }

        match self.next_cycle_rect(target, window, screen) {
            Some(next) => Plan::Move {
                id: window.id,
                target: next,
            },
            None => Plan::NoOp(NoOpReason::AlreadyInPosition),
        }
    }

    /// Throws `window` to the display in the requested direction without
    /// wrapping, keeping its current tile slot when recognised or mapping
    /// proportionally otherwise.
    fn plan_display_move(
        &self,
        target: ActionTarget,
        window: &WindowSnapshot,
        screens: &[Screen],
    ) -> Plan {
        let Some(from) = Screen::best_match(screens, &window.frame) else {
            return Plan::NoOp(NoOpReason::NoScreen);
        };
        let Some(direction) = target.display_direction() else {
            return Plan::NoOp(NoOpReason::NoScreen);
        };
        let Some(dest) = Screen::in_direction(screens, from, direction) else {
            return Plan::NoOp(NoOpReason::NoScreen);
        };
        if dest.id == from.id {
            return Plan::NoOp(NoOpReason::AlreadyInPosition);
        }
        let rect = remap_slot(&self.config, window.frame, from, dest);
        if rect.approx_eq(&window.frame, POSITION_TOLERANCE) {
            Plan::NoOp(NoOpReason::AlreadyInPosition)
        } else {
            Plan::Move {
                id: window.id,
                target: rect,
            }
        }
    }

    fn continues_cycle(&self, target: ActionTarget, window: &WindowSnapshot) -> bool {
        self.cycle.as_ref().is_some_and(|state| {
            state.window == window.id
                && state.target == target
                && state.applied.approx_eq(&window.frame, APPLIED_TOLERANCE)
        })
    }

    fn next_cycle_rect(
        &self,
        target: ActionTarget,
        window: &WindowSnapshot,
        screen: &Screen,
    ) -> Option<Rect> {
        let rects: Vec<Rect> = self
            .config
            .cycle_sizes()
            .iter()
            .filter_map(|size| {
                target.cycle_rect(
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

        let len = rects.len();
        let current = rects
            .iter()
            .position(|r| r.approx_eq(&window.frame, APPLIED_TOLERANCE));
        let start = current.map_or(0, |i| i + 1);
        (0..len)
            .map(|offset| rects[(start + offset) % len])
            .find(|r| !r.approx_eq(&window.frame, APPLIED_TOLERANCE))
    }

    /// Records a successful move so that `Restore` can undo it.
    pub fn commit(
        &mut self,
        target: impl Into<ActionTarget>,
        window: &WindowSnapshot,
        applied: Rect,
    ) {
        let target = target.into();
        if target.uses_history() {
            self.history.forget(window.id);
            self.cycle = None;
        } else if target.moves_display() {
            self.history.record(window.id, window.frame, applied);
            self.cycle = None;
        } else {
            self.history.record(window.id, window.frame, applied);
            self.cycle = if target.cycles() {
                Some(CycleState {
                    window: window.id,
                    target,
                    applied,
                })
            } else {
                None
            };
        }
    }
}

/// All placement targets whose cycled slots should be recognised when throwing
/// a window across displays.
const SLOT_TARGETS: [ActionTarget; 7] = [
    ActionTarget::Action(WindowAction::LeftHalf),
    ActionTarget::Action(WindowAction::RightHalf),
    ActionTarget::Action(WindowAction::CenterHalf),
    ActionTarget::Corner(Corner::TopLeft),
    ActionTarget::Corner(Corner::TopRight),
    ActionTarget::Corner(Corner::BottomLeft),
    ActionTarget::Corner(Corner::BottomRight),
];

/// Rebuilds `frame` on `dest`, preferring a recognised tile slot so a left
/// third or corner stays in that slot when the two work areas differ.
fn remap_slot(config: &Config, frame: Rect, from: &Screen, dest: &Screen) -> Rect {
    for target in SLOT_TARGETS {
        for size in CycleSize::ALL {
            let Some(src) = target.cycle_rect(
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
            if let Some(dst) = target.cycle_rect(
                dest.work_area,
                &config.gaps,
                dest.is_primary,
                size.fraction(),
            ) {
                return dst;
            }
        }
    }
    if let Some(src) =
        WindowAction::Maximize.target_rect(from.work_area, &config.gaps, from.is_primary)
    {
        if src.approx_eq(&frame, POSITION_TOLERANCE) {
            if let Some(dst) =
                WindowAction::Maximize.target_rect(dest.work_area, &config.gaps, dest.is_primary)
            {
                return dst;
            }
        }
    }
    map_proportionally(frame, from.work_area, dest.work_area)
}

/// Re-expresses `frame` as fractions of `from` and applies them to `to`.
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
            refresh_rate_hz: Some(60),
        }
    }

    fn second_screen() -> Screen {
        Screen {
            id: "right".into(),
            frame: Rect::new(1920.0, 0.0, 1200.0, 900.0),
            work_area: Rect::new(1920.0, 0.0, 1200.0, 860.0),
            scale_factor: 1.0,
            is_primary: false,
            refresh_rate_hz: Some(120),
        }
    }

    fn window() -> WindowSnapshot {
        WindowSnapshot {
            id: 1,
            frame: Rect::new(300.0, 200.0, 800.0, 600.0),
        }
    }

    fn apply(
        engine: &mut Engine,
        req: impl Into<ActionRequest>,
        win: &mut WindowSnapshot,
        screens: &[Screen],
    ) -> Rect {
        let req = req.into();
        match engine.plan_request(req, win, screens) {
            Plan::Move { target, .. } => {
                engine.commit(req.target, win, target);
                win.frame = target;
                target
            }
            other => panic!("expected Move, got {other:?}"),
        }
    }

    #[test]
    fn side_and_center_column_cycle_through_configured_sizes() {
        let mut engine = Engine::default();
        let screens = [screen()];
        let mut win = window();

        let half = apply(&mut engine, WindowAction::LeftHalf, &mut win, &screens);
        assert_eq!(half, Rect::new(0.0, 0.0, 960.0, 1040.0));
        let two_thirds = apply(&mut engine, WindowAction::LeftHalf, &mut win, &screens);
        assert_eq!(two_thirds, Rect::new(0.0, 0.0, 1280.0, 1040.0));
        let third = apply(&mut engine, WindowAction::LeftHalf, &mut win, &screens);
        assert_eq!(third, Rect::new(0.0, 0.0, 640.0, 1040.0));
        let back_to_half = apply(&mut engine, WindowAction::LeftHalf, &mut win, &screens);
        assert_eq!(back_to_half, half);

        let center_half = apply(&mut engine, WindowAction::CenterHalf, &mut win, &screens);
        assert_eq!(center_half, Rect::new(480.0, 0.0, 960.0, 1040.0));
        let center_two_thirds = apply(&mut engine, WindowAction::CenterHalf, &mut win, &screens);
        assert_eq!(center_two_thirds, Rect::new(320.0, 0.0, 1280.0, 1040.0));
    }

    #[test]
    fn tray_menu_requests_cycle_on_repeat_clicks() {
        let mut engine = Engine::default();
        let screens = [screen()];
        let mut win = window();

        let first = apply(
            &mut engine,
            ActionRequest::from_menu(WindowAction::LeftHalf),
            &mut win,
            &screens,
        );
        assert_eq!(first, Rect::new(0.0, 0.0, 960.0, 1040.0));

        let second = apply(
            &mut engine,
            ActionRequest::from_menu(WindowAction::LeftHalf),
            &mut win,
            &screens,
        );
        assert_eq!(second, Rect::new(0.0, 0.0, 1280.0, 1040.0));
    }

    #[test]
    fn corner_composition_preserves_width_and_cycles_corner() {
        let mut engine = Engine::default();
        let mut composer = CornerComposer::with_base_modifiers(engine.config.compose_modifiers());
        let screens = [screen()];
        let mut win = window();
        let hk = |key| Hotkey::new(BASE_MODIFIERS, key);

        // Left -> Left (⅔) -> Up (TopLeft at ⅔) -> Left (cycles TopLeft to ⅓) -> Down (Left at ⅓).
        let r1 = composer.press(hk(KeyCode::Left), WindowAction::LeftHalf, 1);
        apply(&mut engine, r1, &mut win, &screens);
        let r2 = composer.press(hk(KeyCode::Left), WindowAction::LeftHalf, 1);
        let left_two_thirds = apply(&mut engine, r2, &mut win, &screens);
        assert_eq!(left_two_thirds, Rect::new(0.0, 0.0, 1280.0, 1040.0));

        let r3 = composer.press(hk(KeyCode::Up), WindowAction::Maximize, 1);
        let top_left_two_thirds = apply(&mut engine, r3, &mut win, &screens);
        assert_eq!(top_left_two_thirds, Rect::new(0.0, 0.0, 1280.0, 520.0));

        let r4 = composer.press(hk(KeyCode::Left), WindowAction::LeftHalf, 1);
        let top_left_third = apply(&mut engine, r4, &mut win, &screens);
        assert_eq!(top_left_third, Rect::new(0.0, 0.0, 640.0, 520.0));

        let r5 = composer.press(hk(KeyCode::Down), WindowAction::Restore, 1);
        let left_third = apply(&mut engine, r5, &mut win, &screens);
        assert_eq!(left_third, Rect::new(0.0, 0.0, 640.0, 1040.0));
    }

    #[test]
    fn display_throw_preserves_cycled_side_and_corner_slots() {
        let mut engine = Engine::default();
        let screens = [screen(), second_screen()];
        let mut win = window();

        // Place in TopLeft at ⅓ on screen 0.
        apply(&mut engine, WindowAction::LeftHalf, &mut win, &screens);
        apply(&mut engine, WindowAction::LeftHalf, &mut win, &screens);
        apply(&mut engine, WindowAction::LeftHalf, &mut win, &screens);
        apply(
            &mut engine,
            ActionRequest::composed(Corner::TopLeft),
            &mut win,
            &screens,
        );
        assert_eq!(win.frame, Rect::new(0.0, 0.0, 640.0, 520.0));

        // Throw right: lands in TopLeft at ⅓ on the 1200x860 display.
        let thrown = apply(&mut engine, WindowAction::DisplayRight, &mut win, &screens);
        assert_eq!(thrown, Rect::new(1920.0, 0.0, 400.0, 430.0));
    }

    #[test]
    fn restore_returns_to_pre_tile_frame_after_cycling() {
        let mut engine = Engine::default();
        let screens = [screen()];
        let original = window().frame;
        let mut win = window();

        apply(&mut engine, WindowAction::LeftHalf, &mut win, &screens);
        apply(&mut engine, WindowAction::LeftHalf, &mut win, &screens);
        apply(
            &mut engine,
            ActionRequest::composed(Corner::TopLeft),
            &mut win,
            &screens,
        );

        let restored = apply(&mut engine, WindowAction::Restore, &mut win, &screens);
        assert_eq!(restored, original);
        assert_eq!(
            engine.plan(WindowAction::Restore, &win, &screens),
            Plan::NoOp(NoOpReason::NoHistory)
        );
    }

    #[test]
    fn do_nothing_mode_disables_repeat_cycling() {
        let mut engine = Engine::default();
        engine.config.subsequent_execution_mode = SubsequentExecutionMode::DoNothing;
        let screens = [screen()];
        let mut win = window();

        apply(&mut engine, WindowAction::LeftHalf, &mut win, &screens);
        assert_eq!(
            engine.plan(WindowAction::LeftHalf, &win, &screens),
            Plan::NoOp(NoOpReason::AlreadyInPosition)
        );
    }
}
