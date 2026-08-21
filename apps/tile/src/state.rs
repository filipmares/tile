//! Shared application state and the action pipeline that is the heart of Tile.
//!
//! Threading model:
//! * The **main thread** runs Tauri's event loop and owns tray/menu handling.
//!   On macOS the hotkey backend must be constructed here (Carbon
//!   `RegisterEventHotKey` needs the main thread's run loop), which is why the
//!   backend is built inside Tauri's `setup` closure.
//! * A single **worker thread** owns the [`std::sync::mpsc::Receiver`] end of
//!   the hotkey channel and drains it, calling [`AppState::perform_action`].
//! * The window backend, engine and hotkey backend are each behind a [`Mutex`]
//!   inside [`AppState`], which Tauri manages, so both the worker thread and
//!   the command handlers (tray menu, settings window) drive the same pipeline.
//!
//! # Why an animated move still holds both locks
//!
//! With animation enabled, a single action occupies the backend and engine
//! locks for the length of the animation (~140 ms) rather than for one
//! `SetWindowPos`. That is deliberate.
//!
//! [`tile_core::Engine::plan`] plans against the window's *current* frame, and
//! [`tile_core::Engine::commit`] has to run before the next `plan` for size
//! cycling and Restore to work. Animating on a separate thread would break
//! both: a second hotkey would plan against a meaningless mid-flight rectangle,
//! and the first move would commit after the second was already planned. Keeping
//! the pipeline strictly sequential means animation changes *nothing* about the
//! engine's view of the world.
//!
//! The cost is bounded and invisible: the only other users of these locks are
//! the settings commands, which are user-driven and infrequent, and the
//! permission poll, which runs every two seconds. Rapid hotkeys are handled by
//! preemption instead of by queueing — see
//! [`AppState::perform_action_preemptible`].

use std::path::PathBuf;
use std::sync::{Mutex, MutexGuard};
use std::time::Duration;

use tile_core::{
    AnimationParams, Animator, Config, Engine, Plan, WindowAction, WindowId, WindowSnapshot,
};
use tile_platform::{
    AnimationSession, HotkeyBackend, HotkeyFailure, PermissionStatus, PlatformError, WindowBackend,
};

use crate::animate::{self, Interruption, Pacer, SleepPacer};
use crate::config_store;
use crate::ratelimit::RateLimiter;

/// How long a `PermissionDenied` dialog is suppressed after being shown once.
const PERMISSION_DIALOG_COOLDOWN: Duration = Duration::from_secs(20);

/// Locks a mutex, recovering the guard even if a previous holder panicked, so a
/// poisoned lock can never crash the tray app.
fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// Everything the app needs to service hotkeys, tray clicks and commands.
pub struct AppState {
    backend: Mutex<Box<dyn WindowBackend>>,
    hotkeys: Mutex<Box<dyn HotkeyBackend>>,
    engine: Mutex<Engine>,
    config_dir: Option<PathBuf>,
    hotkey_failures: Mutex<Vec<HotkeyFailure>>,
    permission_dialog_limiter: Mutex<RateLimiter>,
}

impl AppState {
    pub fn new(
        backend: Box<dyn WindowBackend>,
        hotkeys: Box<dyn HotkeyBackend>,
        config: Config,
        config_dir: Option<PathBuf>,
    ) -> Self {
        Self {
            backend: Mutex::new(backend),
            hotkeys: Mutex::new(hotkeys),
            engine: Mutex::new(Engine::new(config)),
            config_dir,
            hotkey_failures: Mutex::new(Vec::new()),
            permission_dialog_limiter: Mutex::new(RateLimiter::new(PERMISSION_DIALOG_COOLDOWN)),
        }
    }

    /// A snapshot of the current configuration.
    pub fn config(&self) -> Config {
        lock(&self.engine).config.clone()
    }

    /// The hotkey registrations the OS most recently refused.
    pub fn hotkey_failures(&self) -> Vec<HotkeyFailure> {
        lock(&self.hotkey_failures).clone()
    }

    /// Reports the OS permission status, optionally prompting the user. The
    /// prompt must only ever be requested from the main thread.
    pub fn permission_status(&self, prompt: bool) -> tile_platform::Result<PermissionStatus> {
        lock(&self.backend).permission_status(prompt)
    }

    /// Runs the full pipeline for `action`: read the focused window and
    /// screens, ask the engine for a [`Plan`], apply it, and commit history
    /// using the frame the backend actually produced.
    ///
    /// Callers with no source of further actions (the tray menu, the settings
    /// window) use this; the hotkey worker uses
    /// [`AppState::perform_action_preemptible`] so a second press can steer an
    /// animation that is still in flight.
    pub fn perform_action(&self, action: WindowAction) -> tile_platform::Result<()> {
        self.perform_action_preemptible(action, &mut || None)
    }

    /// As [`AppState::perform_action`], but able to pick up further actions
    /// while a window is still animating.
    ///
    /// `next` is polled once per animation frame and should yield an action
    /// that has already arrived without blocking — the hotkey worker passes a
    /// non-blocking receive on its channel. With animation switched off it is
    /// never called, and the pipeline is exactly what it always was.
    pub fn perform_action_preemptible(
        &self,
        action: WindowAction,
        next: &mut dyn FnMut() -> Option<WindowAction>,
    ) -> tile_platform::Result<()> {
        let backend = lock(&self.backend);
        let mut engine = lock(&self.engine);

        let animation = engine.config.animation;
        if !animation.enabled {
            return apply_once(backend.as_ref(), &mut engine, action);
        }

        animated_pipeline(
            backend.as_ref(),
            &mut engine,
            action,
            animation.params(),
            &mut SleepPacer::new(),
            next,
        )
    }

    /// Registers the currently bound hotkeys, recording any the OS refused.
    /// Returns the failures for convenience.
    pub fn apply_hotkeys(&self) -> Vec<HotkeyFailure> {
        let bindings = lock(&self.engine).config.active_bindings();
        let result = lock(&self.hotkeys).apply(&bindings);
        let failures = match result {
            Ok(failures) => failures,
            Err(err) => {
                log::error!("failed to apply hotkeys: {err}");
                Vec::new()
            }
        };
        *lock(&self.hotkey_failures) = failures.clone();
        failures
    }

    /// Persists the current config atomically, logging (never panicking) on
    /// failure.
    pub fn save_config(&self) {
        let Some(dir) = self.config_dir.as_deref() else {
            log::warn!("no config directory resolved; not persisting settings");
            return;
        };
        let config = self.config();
        if let Err(err) = config_store::save_to_dir(dir, &config) {
            log::error!("failed to save config: {err}");
        }
    }

    /// Mutates the config under lock, then persists and re-applies hotkeys.
    /// Returns the updated config so callers (commands) can hand truth back to
    /// the UI.
    pub fn update_config(&self, mutate: impl FnOnce(&mut Config)) -> Config {
        {
            let mut engine = lock(&self.engine);
            mutate(&mut engine.config);
            engine.config.normalize();
        }
        self.save_config();
        self.apply_hotkeys();
        self.config()
    }

    /// Decides whether a `PermissionDenied` dialog should be shown now, given
    /// the rate limit. Returns `true` at most once per cooldown window.
    pub fn should_show_permission_dialog(&self) -> bool {
        lock(&self.permission_dialog_limiter).allow()
    }

    /// Releases OS hotkeys. Called on shutdown.
    pub fn shutdown_hotkeys(&self) {
        lock(&self.hotkeys).shutdown();
    }
}

/// Classifies an error from the pipeline for the caller's reaction.
pub fn is_permission_denied(err: &PlatformError) -> bool {
    matches!(err, PlatformError::PermissionDenied(_))
}

/// The unanimated pipeline: plan, apply in one jump, commit.
fn apply_once(
    backend: &dyn WindowBackend,
    engine: &mut Engine,
    action: WindowAction,
) -> tile_platform::Result<()> {
    let Some(window) = backend.focused_window()? else {
        log::debug!("ignoring {action}: no movable focused window");
        return Ok(());
    };
    let screens = backend.screens()?;

    match engine.plan(action, &window, &screens) {
        Plan::Move { id, target } => {
            let actual = backend.set_window_frame(id, target)?;
            engine.commit(action, &window, actual);
            log::debug!("performed {action} on window {id}");
        }
        Plan::NoOp(reason) => {
            log::debug!("no-op for {action}: {reason:?}");
        }
    }
    Ok(())
}

/// A window currently travelling towards a target.
struct Flight {
    id: WindowId,
    /// The action that put it in motion, held so it can still be committed if
    /// something supersedes it.
    action: WindowAction,
    /// Where the window was when this action was planned. This is the "before"
    /// frame Restore will return to.
    window: WindowSnapshot,
    animator: Animator,
    /// The backend's fast path for intermediate frames, opened on the first
    /// frame and kept across retargets of the same window.
    session: Option<Box<dyn AnimationSession>>,
}

/// The animated pipeline.
///
/// Plans an action, animates the window towards the result, and keeps going if
/// another action arrives mid-flight — retargeting rather than queueing, so a
/// burst of hotkeys reads as one continuous movement instead of the window
/// visibly stepping through every intermediate layout.
fn animated_pipeline(
    backend: &dyn WindowBackend,
    engine: &mut Engine,
    action: WindowAction,
    params: AnimationParams,
    pacer: &mut dyn Pacer,
    next: &mut dyn FnMut() -> Option<WindowAction>,
) -> tile_platform::Result<()> {
    let mut pending = Some(action);
    let mut flight: Option<Flight> = None;

    loop {
        if let Some(action) = pending.take() {
            let Some(mut window) = backend.focused_window()? else {
                log::debug!("ignoring {action}: no movable focused window");
                break;
            };

            // A window that is mid-flight reports an interpolated frame that
            // means nothing to the engine — it is neither where the window was
            // nor where it is going. Plan against the destination instead, so
            // a second press sees exactly the world it would have seen if the
            // first move had already landed. This is what keeps size cycling
            // (½ → ⅔ → ⅓) working on a fast double-press.
            if let Some(in_flight) = &flight {
                if in_flight.id == window.id {
                    window.frame = in_flight.animator.target();
                }
            }

            let screens = backend.screens()?;
            match engine.plan(action, &window, &screens) {
                Plan::Move { id, target } => {
                    // An action aimed at a *different* window must not leave
                    // the current one stranded halfway. Land it on its exact
                    // target first, and commit it, before moving on.
                    if let Some(previous) = flight.take() {
                        if previous.id == id {
                            flight = Some(previous);
                        } else {
                            land(backend, engine, previous)?;
                        }
                    }

                    match &mut flight {
                        Some(in_flight) => {
                            // Same window: commit the superseded action at the
                            // target it was heading for. It never physically
                            // arrived, but the engine's model has to match what
                            // the next plan was computed against, and
                            // `WindowHistory::record` only replaces the stored
                            // original when the window is somewhere Tile did
                            // not put it — so Restore still returns to the
                            // true pre-Tile frame.
                            engine.commit(
                                in_flight.action,
                                &in_flight.window,
                                in_flight.animator.target(),
                            );

                            // Retarget without resetting velocity: the window
                            // bends towards the new frame instead of stopping
                            // dead and starting again.
                            in_flight.animator.retarget(target);
                            in_flight.action = action;
                            in_flight.window = window;
                        }
                        None => {
                            flight = Some(Flight {
                                id,
                                action,
                                animator: Animator::new(window.frame, target, params),
                                window,
                                session: None,
                            });
                        }
                    }
                }
                Plan::NoOp(reason) => {
                    // Nothing to do for this action, but a window already in
                    // flight must still finish its journey.
                    log::debug!("no-op for {action}: {reason:?}");
                }
            }
        }

        let Some(in_flight) = flight.as_mut() else {
            // Nothing pending and nothing moving.
            break;
        };

        match animate::pump(
            backend,
            in_flight.id,
            &mut in_flight.session,
            &mut in_flight.animator,
            params,
            pacer,
            next,
        )? {
            Interruption::Settled(actual) => {
                // Commit with the frame the window truly ended up with, not
                // the one that was planned: an app enforcing a minimum size or
                // size increments will not have honoured the request exactly,
                // and history and no-op detection need the truth.
                if let Some(landed) = flight.take() {
                    engine.commit(landed.action, &landed.window, actual);
                    log::debug!("performed {} on window {}", landed.action, landed.id);
                }
                break;
            }
            Interruption::Preempted(action) => {
                log::debug!("{action} arrived mid-flight; retargeting");
                pending = Some(action);
            }
        }
    }

    Ok(())
}

/// Jumps a superseded animation straight to its target and commits it, so no
/// window is ever left halfway when attention moves elsewhere.
fn land(
    backend: &dyn WindowBackend,
    engine: &mut Engine,
    flight: Flight,
) -> tile_platform::Result<()> {
    // Drop the session before the final move: the fast path deliberately skips
    // the app's own clamping, which the frame that has to stick must honour.
    drop(flight.session);

    let actual = backend.set_window_frame(flight.id, flight.animator.target())?;
    engine.commit(flight.action, &flight.window, actual);
    log::debug!("landed {} on window {}", flight.action, flight.id);
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::cell::RefCell;
    use std::collections::VecDeque;
    use std::time::Duration;

    use tile_core::{AnimationConfig, Rect, Screen};

    use super::*;

    /// A window backend that records every frame it is asked to apply.
    ///
    /// Substitutes for a real window server so the preemption pipeline — the
    /// part of this file with real decisions in it — can be exercised without
    /// one. It deliberately does not implement `begin_animation`, so it takes
    /// the `Ok(None)` fallback and every frame, intermediate or final, lands
    /// in `frames`. Frames are applied verbatim, which is the honest model for
    /// a well-behaved app; `min_size` reproduces one that clamps, so the
    /// "commit the truth, not the request" rule can be checked.
    struct FakeBackend {
        frames: RefCell<Vec<Rect>>,
        min_size: Option<(f64, f64)>,
    }

    impl FakeBackend {
        fn new() -> Self {
            Self {
                frames: RefCell::new(Vec::new()),
                min_size: None,
            }
        }

        fn with_min_size(width: f64, height: f64) -> Self {
            Self {
                min_size: Some((width, height)),
                ..Self::new()
            }
        }

        fn clamp(&self, target: Rect) -> Rect {
            match self.min_size {
                Some((w, h)) => Rect::new(
                    target.x,
                    target.y,
                    target.width.max(w),
                    target.height.max(h),
                ),
                None => target,
            }
        }

        fn last_frame(&self) -> Rect {
            *self.frames.borrow().last().expect("no frame was applied")
        }
    }

    // The pipeline is driven from one thread at a time under the state locks,
    // so interior mutability is enough; `Send` is only needed to satisfy the
    // trait bound.
    // SAFETY: test-only. Every use below is single-threaded, so the `RefCell`s
    // are never shared across threads despite this promise.
    unsafe impl Send for FakeBackend {}

    impl WindowBackend for FakeBackend {
        fn focused_window(&self) -> tile_platform::Result<Option<WindowSnapshot>> {
            Ok(Some(WindowSnapshot {
                id: 1,
                frame: self
                    .frames
                    .borrow()
                    .last()
                    .copied()
                    .unwrap_or(Rect::new(100.0, 100.0, 400.0, 300.0)),
            }))
        }

        fn screens(&self) -> tile_platform::Result<Vec<Screen>> {
            Ok(vec![Screen {
                id: "fake".into(),
                frame: Rect::new(0.0, 0.0, 1920.0, 1080.0),
                work_area: Rect::new(0.0, 0.0, 1920.0, 1080.0),
                scale_factor: 1.0,
                is_primary: true,
            }])
        }

        fn set_window_frame(&self, _id: WindowId, target: Rect) -> tile_platform::Result<Rect> {
            let actual = self.clamp(target);
            self.frames.borrow_mut().push(actual);
            Ok(actual)
        }

        fn permission_status(&self, _prompt: bool) -> tile_platform::Result<PermissionStatus> {
            Ok(PermissionStatus::NotRequired)
        }
    }

    fn engine_with_animation() -> Engine {
        let config = Config {
            animation: AnimationConfig {
                enabled: true,
                duration_ms: 140,
                fps: 90,
            },
            ..Default::default()
        };
        Engine::new(config)
    }

    fn params() -> AnimationParams {
        AnimationParams {
            duration_ms: 340,
            fps: 90,
        }
    }

    /// A pacer that never sleeps and always reports the nominal interval.
    ///
    /// This is what makes these tests deterministic. The real [`SleepPacer`]
    /// reports the wall-clock time each frame actually took, so on a loaded
    /// machine a frame can overrun badly, the animator advances further per
    /// step, and the animation settles in a handful of frames instead of
    /// dozens. That is correct behaviour in production — a late frame should
    /// catch up rather than play in slow motion — but it makes frame counts
    /// unassertable, and an earlier version of these tests failed on a busy
    /// macOS CI runner for exactly that reason. It also keeps the suite fast:
    /// with real pacing each of these tests would sleep for a whole animation.
    struct FixedPacer;

    impl Pacer for FixedPacer {
        fn wait(&mut self, interval: Duration) -> Duration {
            interval
        }
    }

    /// A preemption source that yields each queued action on a later frame, so
    /// the animation is genuinely interrupted mid-flight rather than before it
    /// starts.
    fn after_frames(
        delay: usize,
        actions: Vec<WindowAction>,
    ) -> impl FnMut() -> Option<WindowAction> {
        let mut queued: VecDeque<WindowAction> = actions.into();
        let mut frame = 0usize;
        move || {
            frame += 1;
            if frame % delay == 0 {
                queued.pop_front()
            } else {
                None
            }
        }
    }

    #[test]
    fn an_animated_move_ends_on_the_planned_frame() {
        let backend = FakeBackend::new();
        let mut engine = engine_with_animation();

        animated_pipeline(
            &backend,
            &mut engine,
            WindowAction::LeftHalf,
            params(),
            &mut FixedPacer,
            &mut || None,
        )
        .unwrap();

        // Many frames, and the last of them is the exact left half.
        assert!(backend.frames.borrow().len() > 3);
        assert_eq!(backend.last_frame(), Rect::new(0.0, 0.0, 960.0, 1080.0));
        // The intermediate frames really were intermediate.
        assert!(backend
            .frames
            .borrow()
            .iter()
            .any(|f| *f != Rect::new(0.0, 0.0, 960.0, 1080.0)));
    }

    #[test]
    fn history_records_the_frame_before_the_animation_started() {
        let backend = FakeBackend::new();
        let mut engine = engine_with_animation();
        let original = backend.focused_window().unwrap().unwrap().frame;

        animated_pipeline(
            &backend,
            &mut engine,
            WindowAction::LeftHalf,
            params(),
            &mut FixedPacer,
            &mut || None,
        )
        .unwrap();
        animated_pipeline(
            &backend,
            &mut engine,
            WindowAction::Restore,
            params(),
            &mut FixedPacer,
            &mut || None,
        )
        .unwrap();

        // Restore must return the window to where it was before Tile touched
        // it, not to some frame sampled mid-animation.
        assert_eq!(backend.last_frame(), original);
    }

    #[test]
    fn the_committed_frame_is_the_one_the_app_allowed() {
        // An app with a minimum size does not honour the planned rectangle.
        // History has to record what actually happened, or Restore and no-op
        // detection drift out of step with reality.
        let backend = FakeBackend::with_min_size(1200.0, 200.0);
        let mut engine = engine_with_animation();

        animated_pipeline(
            &backend,
            &mut engine,
            WindowAction::LeftHalf,
            params(),
            &mut FixedPacer,
            &mut || None,
        )
        .unwrap();

        assert_eq!(backend.last_frame().width, 1200.0);
    }

    #[test]
    fn a_preempting_action_lands_on_its_own_target() {
        let backend = FakeBackend::new();
        let mut engine = engine_with_animation();

        animated_pipeline(
            &backend,
            &mut engine,
            WindowAction::LeftHalf,
            params(),
            &mut FixedPacer,
            &mut after_frames(2, vec![WindowAction::TopHalf]),
        )
        .unwrap();

        // The left half was abandoned in flight; the top half is where the
        // window actually comes to rest.
        assert_eq!(backend.last_frame(), Rect::new(0.0, 0.0, 1920.0, 540.0));
    }

    #[test]
    fn a_repeat_arriving_mid_flight_still_advances_the_size_cycle() {
        // The regression this pipeline exists to avoid: a second press has to
        // see the world as though the first move had landed, or a fast
        // double-press sits on the half instead of cycling to two thirds.
        let backend = FakeBackend::new();
        let mut engine = engine_with_animation();

        animated_pipeline(
            &backend,
            &mut engine,
            WindowAction::LeftHalf,
            params(),
            &mut FixedPacer,
            &mut after_frames(2, vec![WindowAction::LeftHalf]),
        )
        .unwrap();

        let cycled = backend.last_frame();
        assert!(
            cycled.width > 960.0,
            "expected the cycle to grow past a half, got {cycled:?}"
        );
    }

    #[test]
    fn several_presses_in_flight_resolve_to_the_last_one() {
        let backend = FakeBackend::new();
        let mut engine = engine_with_animation();

        animated_pipeline(
            &backend,
            &mut engine,
            WindowAction::LeftHalf,
            params(),
            &mut FixedPacer,
            &mut after_frames(2, vec![WindowAction::TopHalf, WindowAction::RightHalf]),
        )
        .unwrap();

        assert_eq!(backend.last_frame(), Rect::new(960.0, 0.0, 960.0, 1080.0));
    }

    #[test]
    fn switching_the_animation_off_applies_the_move_in_one_frame() {
        let backend = FakeBackend::new();
        let config = Config {
            animation: AnimationConfig {
                enabled: false,
                ..AnimationConfig::default()
            },
            ..Default::default()
        };
        let mut engine = Engine::new(config);

        apply_once(&backend, &mut engine, WindowAction::LeftHalf).unwrap();

        assert_eq!(backend.frames.borrow().len(), 1);
        assert_eq!(backend.last_frame(), Rect::new(0.0, 0.0, 960.0, 1080.0));
    }
}
