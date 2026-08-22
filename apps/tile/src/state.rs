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
//! locks for the configured animation duration — 250 ms by default on macOS and
//! 220 ms elsewhere, see [`tile_core::AnimationConfig`] — rather than for one
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

use std::path::{Path, PathBuf};
use std::sync::mpsc;
use std::sync::{Mutex, MutexGuard};
use std::time::Duration;

use tile_core::{
    AnimationParams, Animator, Config, Engine, Plan, Rect, WindowAction, WindowId, WindowSnapshot,
};
use tile_platform::{
    AnimationSession, HotkeyBackend, HotkeyFailure, PermissionStatus, PlatformError, WindowBackend,
};

use crate::animate::{self, Interruption, Pacer, SleepPacer};
use crate::build_kind::BuildKind;
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
    build_kind: BuildKind,
    config_dir: Option<PathBuf>,
    hotkey_failures: Mutex<Vec<HotkeyFailure>>,
    permission_dialog_limiter: Mutex<RateLimiter>,
    /// The same channel the hotkey backend posts to, so callers that must not
    /// block can hand an action to the worker thread instead of running it.
    actions: mpsc::Sender<WindowAction>,
}

impl AppState {
    pub fn new(
        backend: Box<dyn WindowBackend>,
        hotkeys: Box<dyn HotkeyBackend>,
        config: Config,
        build_kind: BuildKind,
        config_dir: Option<PathBuf>,
        actions: mpsc::Sender<WindowAction>,
    ) -> Self {
        Self {
            backend: Mutex::new(backend),
            hotkeys: Mutex::new(hotkeys),
            engine: Mutex::new(Engine::new(config)),
            build_kind,
            config_dir,
            hotkey_failures: Mutex::new(Vec::new()),
            permission_dialog_limiter: Mutex::new(RateLimiter::new(PERMISSION_DIALOG_COOLDOWN)),
            actions,
        }
    }

    /// Whether this binary is a local development build or an installed one.
    pub fn build_kind(&self) -> BuildKind {
        self.build_kind
    }

    /// Where the config is being read from and written to, if anywhere.
    pub fn config_dir(&self) -> Option<&Path> {
        self.config_dir.as_deref()
    }

    /// Hands `action` to the worker thread instead of performing it here.
    ///
    /// This is what the tray menu uses. Its callback runs on Tauri's main
    /// event loop, and an animated action occupies the pipeline for the whole
    /// animation — so running it inline would freeze the tray, the menu and
    /// the settings window for every tray-driven snap. Posting it to the same
    /// channel the hotkeys use also means a tray action and a hotkey press
    /// stay in one order and can preempt each other, rather than racing for
    /// the locks.
    pub fn enqueue_action(&self, action: WindowAction) {
        if let Err(err) = self.actions.send(action) {
            log::error!("could not queue {action}: the action worker is gone ({err})");
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
    /// The backend's fast path for intermediate frames, opened up front and
    /// kept across retargets of the same window.
    session: Option<Box<dyn AnimationSession>>,
    /// Whether [`Flight::action`] has already been recorded with the engine.
    /// Set when a newly pressed hotkey supersedes this flight, so the action is
    /// never committed twice.
    committed: bool,
}

impl Flight {
    /// Starts a flight, opening the backend's animation session up front.
    ///
    /// The session is opened *before* the animator is constructed because
    /// opening it is what restores a maximized, minimized or full-screen
    /// window to its normal state — which moves the window. Building the
    /// animator from the pre-restore frame would start the animation from a
    /// rectangle the window no longer occupies, so the first frame would jump.
    /// The frame is therefore re-read afterwards and used as the true origin.
    ///
    /// Only the animator's origin changes. `window` keeps the frame the action
    /// was planned against, so Restore still returns the window to where it
    /// was before Tile touched it, native state and all.
    fn begin(
        backend: &dyn WindowBackend,
        id: WindowId,
        action: WindowAction,
        window: WindowSnapshot,
        target: Rect,
        params: AnimationParams,
    ) -> tile_platform::Result<Self> {
        let session = backend.begin_animation(id)?;

        let start = match session.as_ref() {
            // Read through the session rather than re-querying the focused
            // window. Opening the session is what restored the window, and on
            // macOS that can take until the setup timeout — long enough for
            // focus to move, which would make a `focused_window` read return
            // the wrong window or nothing at all and silently fall back to the
            // stale pre-restore frame.
            Some(open) => open.current_frame()?,
            // No fast path: nothing has been restored yet, so the frame the
            // action was planned against is still current.
            None => window.frame,
        };

        Ok(Self {
            id,
            action,
            animator: Animator::new(start, target, params),
            window,
            session,
            committed: false,
        })
    }

    /// Records this flight's action with the engine, at the target it is
    /// heading for, unless that has already happened.
    ///
    /// The window may not have physically arrived, but the engine's model has
    /// to match what the next plan will be computed against.
    /// [`tile_core::history::WindowHistory::record`] only replaces the stored
    /// original when the window is somewhere Tile did not put it, so Restore
    /// still returns to the true pre-Tile frame.
    fn commit_to(&mut self, engine: &mut Engine) {
        if self.committed {
            return;
        }
        engine.commit(self.action, &self.window, self.animator.target());
        self.committed = true;
    }

    /// Records the frame the window truly ended up with.
    ///
    /// When this flight was already committed at its target — because a
    /// newly pressed hotkey superseded it and the plan that followed turned
    /// out to be a no-op — the reconciliation has to start from that recorded
    /// target, not from the original "before" frame.
    /// [`tile_core::history::WindowHistory::record`] keeps the stored original
    /// only when `before` matches the entry's `last_applied`; passing the
    /// original again would no longer match, so it would insert a fresh entry
    /// whose original is the mid-flight frame and send Restore back to a
    /// rectangle the window never really occupied.
    fn commit_final(&self, engine: &mut Engine, actual: Rect) {
        let window = if self.committed {
            WindowSnapshot {
                id: self.id,
                frame: self.animator.target(),
            }
        } else {
            self.window.clone()
        };
        engine.commit(self.action, &window, actual);
    }
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
    let mut flight: Option<Flight> = None;
    let result = run_animated_pipeline(backend, engine, action, params, pacer, next, &mut flight);

    // Any error anywhere above abandons the loop with the window possibly
    // part-way through its journey. Leaving it there is not neutral: Tile's
    // model has no record of the move, so the *next* action would treat that
    // arbitrary intermediate rectangle as the frame the user chose and make it
    // the Restore point. Land it on the target it was heading for and record
    // that instead.
    //
    // Best effort by definition — whatever failed may well fail again, and
    // `land` only commits once the move has actually succeeded, so a second
    // failure leaves history untouched rather than claiming something false.
    // The original error is what propagates either way.
    if result.is_err() {
        if let Some(stranded) = flight.take() {
            if let Err(err) = land(backend, engine, stranded) {
                log::debug!("could not land the in-flight window after a failure: {err}");
            }
        }
    }

    result
}

/// The pipeline proper. Hands its in-flight window back through `flight` so
/// [`animated_pipeline`] can reconcile it if any step fails.
#[allow(clippy::too_many_arguments)]
fn run_animated_pipeline(
    backend: &dyn WindowBackend,
    engine: &mut Engine,
    action: WindowAction,
    params: AnimationParams,
    pacer: &mut dyn Pacer,
    next: &mut dyn FnMut() -> Option<WindowAction>,
    flight: &mut Option<Flight>,
) -> tile_platform::Result<()> {
    let mut pending = Some(action);

    loop {
        if let Some(action) = pending.take() {
            // Read everything fallible *before* touching the engine. Committing
            // first and then failing on one of these would leave history and
            // the cycle claiming the old target was applied while the window is
            // still mid-flight.
            let focused = backend.focused_window()?;
            let screens = backend.screens()?;

            let Some(window) = focused else {
                // Nothing to plan against, but a window already in flight
                // must not be abandoned mid-air just because focus went
                // somewhere unmovable.
                log::debug!("ignoring {action}: no movable focused window");
                if let Some(previous) = flight.take() {
                    land(backend, engine, previous)?;
                }
                break;
            };
            let mut window = window;

            // A flight that is about to be superseded has to be committed
            // *before* the next plan, not after. `Engine::plan` reads the cycle
            // state that `commit` writes, so committing afterwards leaves the
            // plan one press behind: a third rapid LeftHalf would see the
            // window at two thirds but a cycle still recorded at a half,
            // decide the cycle had been broken, and jump back to a half
            // instead of advancing. Restore has the same problem, seeing no
            // history while the first move is still in the air.
            if let Some(in_flight) = flight.as_mut() {
                in_flight.commit_to(engine);
            }

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

            match engine.plan(action, &window, &screens) {
                Plan::Move { id, target } => {
                    // An action aimed at a *different* window must not leave
                    // the current one stranded halfway. Land it on its exact
                    // target first before moving on.
                    if let Some(previous) = flight.take() {
                        if previous.id == id {
                            *flight = Some(previous);
                        } else {
                            land(backend, engine, previous)?;
                        }
                    }

                    match flight.as_mut() {
                        Some(in_flight) => {
                            // Retarget without resetting velocity: the window
                            // bends towards the new frame instead of stopping
                            // dead and starting again. The superseded action
                            // was committed above, so this flight now belongs
                            // to the new one.
                            in_flight.animator.retarget(target);
                            in_flight.action = action;
                            in_flight.window = window;
                            in_flight.committed = false;
                        }
                        None => {
                            *flight =
                                Some(Flight::begin(backend, id, action, window, target, params)?);
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
                    landed.commit_final(engine, actual);
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
    mut flight: Flight,
) -> tile_platform::Result<()> {
    let target = flight.animator.target();

    // As in the pump, prefer the session: it addresses the window directly, so
    // this still works when focus has already moved on — which is precisely
    // the situation that gets a flight landed early.
    let actual = match flight.session.as_mut() {
        Some(open) => open.finish(target)?,
        None => backend.set_window_frame(flight.id, target)?,
    };

    // Commit with the true frame. `commit_final` reconciles correctly whether
    // or not this flight was already committed at its target when it was
    // superseded, so the stored original — and therefore Restore — survives.
    flight.commit_final(engine, actual);
    log::debug!("landed {} on window {}", flight.action, flight.id);
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::cell::RefCell;
    use std::collections::VecDeque;
    use std::rc::Rc;
    use std::time::Duration;

    use tile_core::{AnimationConfig, Screen};

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
        /// When set, `begin_animation` hands out a session and reports this
        /// frame afterwards, standing in for the platform restoring a window
        /// out of a maximized or full-screen state.
        restored_frame: Option<Rect>,
        /// When true, `focused_window` reports nothing, as if focus moved to
        /// something Tile cannot manage.
        focus_lost: RefCell<bool>,
        /// Set once `begin_animation` has run, after which `focused_window`
        /// reports `restored_frame`.
        restored: RefCell<bool>,
        /// The id `focused_window` reports. Changing it mid-run stands in for
        /// focus moving to a different window.
        focused_id: RefCell<WindowId>,
        /// When set, `set_intermediate_frame` fails after this many frames, as
        /// Accessibility revocation would.
        fail_after: Option<usize>,
        /// Frames that went through the session rather than `set_window_frame`.
        via_session: Rc<RefCell<Vec<Rect>>>,
        /// Whether the last frame was landed through `AnimationSession::finish`.
        finished_via_session: Rc<RefCell<bool>>,
    }

    impl FakeBackend {
        fn new() -> Self {
            Self {
                frames: RefCell::new(Vec::new()),
                min_size: None,
                restored_frame: None,
                focus_lost: RefCell::new(false),
                restored: RefCell::new(false),
                focused_id: RefCell::new(1),
                fail_after: None,
                via_session: Rc::new(RefCell::new(Vec::new())),
                finished_via_session: Rc::new(RefCell::new(false)),
            }
        }

        /// A backend whose intermediate frames start failing part-way through,
        /// the way revoked Accessibility permission would.
        fn failing_after(frames: usize) -> Self {
            Self {
                restored_frame: Some(Rect::new(100.0, 100.0, 400.0, 300.0)),
                fail_after: Some(frames),
                ..Self::new()
            }
        }

        fn with_min_size(width: f64, height: f64) -> Self {
            Self {
                min_size: Some((width, height)),
                ..Self::new()
            }
        }

        /// A backend whose `begin_animation` restores the window, changing its
        /// frame the way leaving a maximized state does.
        fn with_restore_to(frame: Rect) -> Self {
            Self {
                restored_frame: Some(frame),
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
            if *self.focus_lost.borrow() {
                return Ok(None);
            }
            // Once `begin_animation` has "restored" the window, that is where
            // it now is — which is the whole point of re-reading the frame.
            let frame = match self.restored_frame {
                Some(restored) if *self.restored.borrow() => restored,
                _ => self
                    .frames
                    .borrow()
                    .last()
                    .copied()
                    .unwrap_or(Rect::new(100.0, 100.0, 400.0, 300.0)),
            };
            Ok(Some(WindowSnapshot {
                id: *self.focused_id.borrow(),
                frame,
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

        fn begin_animation(
            &self,
            _id: WindowId,
        ) -> tile_platform::Result<Option<Box<dyn AnimationSession>>> {
            // Only the restore-aware backend offers a session; the others take
            // the `Ok(None)` fallback so every frame lands in `frames` and the
            // older tests keep observing the whole animation.
            if self.restored_frame.is_none() {
                return Ok(None);
            }
            *self.restored.borrow_mut() = true;
            Ok(Some(Box::new(FakeSession {
                frames: Rc::clone(&self.via_session),
                finished: Rc::clone(&self.finished_via_session),
                restored: self
                    .restored_frame
                    .unwrap_or(Rect::new(100.0, 100.0, 400.0, 300.0)),
                fail_after: self.fail_after,
            })))
        }
    }

    /// The fake backend's animation fast path, recording what it is asked to
    /// do so tests can tell an intermediate frame from the final one.
    struct FakeSession {
        frames: Rc<RefCell<Vec<Rect>>>,
        finished: Rc<RefCell<bool>>,
        /// Where opening the session left the window.
        restored: Rect,
        fail_after: Option<usize>,
    }

    impl AnimationSession for FakeSession {
        fn set_intermediate_frame(&mut self, target: Rect) -> tile_platform::Result<()> {
            if let Some(limit) = self.fail_after {
                if self.frames.borrow().len() >= limit {
                    return Err(tile_platform::PlatformError::PermissionDenied(
                        "accessibility permission revoked mid-animation".into(),
                    ));
                }
            }
            self.frames.borrow_mut().push(target);
            Ok(())
        }

        fn finish(&mut self, target: Rect) -> tile_platform::Result<Rect> {
            self.frames.borrow_mut().push(target);
            *self.finished.borrow_mut() = true;
            Ok(target)
        }

        fn current_frame(&self) -> tile_platform::Result<Rect> {
            Ok(self
                .frames
                .borrow()
                .last()
                .copied()
                .unwrap_or(self.restored))
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

    /// The frame rates a rate-sensitive test has to hold at.
    ///
    /// Every platform cap Tile ships, resolved against the configured rate,
    /// plus the configurable floor. Running all of them from one host is what
    /// stops a frame-rate-dependent assertion passing here and failing on
    /// another platform's CI runner — which it did, twice, before this existed.
    fn rates_to_cover() -> Vec<u32> {
        let configured = params().fps;
        let mut rates: Vec<u32> = animate::ALL_FPS_CAPS
            .iter()
            .map(|cap| cap.map_or(configured, |cap| configured.min(cap)))
            .collect();
        // The lowest rate `Config::normalize` will hand over, where a single
        // frame covers most of the journey.
        rates.push(tile_core::config::MIN_ANIMATION_FPS);
        rates.sort_unstable();
        rates.dedup();
        rates
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
        fn reset(&mut self) {}

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
    fn a_third_press_mid_flight_keeps_advancing_the_cycle() {
        // The commit has to happen *before* the next plan. When it lagged one
        // press behind, a third rapid LeftHalf saw the window at two thirds
        // but a cycle still recorded at a half, concluded the cycle had been
        // broken, and jumped back to a half instead of advancing.
        let backend = FakeBackend::new();
        let mut engine = engine_with_animation();

        animated_pipeline(
            &backend,
            &mut engine,
            WindowAction::LeftHalf,
            params(),
            &mut FixedPacer,
            &mut after_frames(2, vec![WindowAction::LeftHalf, WindowAction::LeftHalf]),
        )
        .unwrap();

        let landed = backend.last_frame();
        assert_ne!(
            landed.width, 960.0,
            "the cycle fell back to a half instead of advancing"
        );
        // Default cycle order is a half, two thirds, a third.
        assert_eq!(landed, Rect::new(0.0, 0.0, 640.0, 1080.0));
    }

    #[test]
    fn losing_focus_mid_flight_still_lands_the_window() {
        // A preempting action arrives, but by the time it is planned there is
        // no movable focused window. The flight must not simply be dropped:
        // that leaves the window on its last intermediate frame with its
        // action never committed.
        let backend = FakeBackend::new();
        let mut engine = engine_with_animation();

        let mut frame = 0usize;
        animated_pipeline(
            &backend,
            &mut engine,
            WindowAction::LeftHalf,
            params(),
            &mut FixedPacer,
            &mut || {
                frame += 1;
                if frame == 2 {
                    // Focus disappears at the same moment as the next press.
                    *backend.focus_lost.borrow_mut() = true;
                    Some(WindowAction::TopHalf)
                } else {
                    None
                }
            },
        )
        .unwrap();

        // The window finished on the target it was already heading for.
        assert_eq!(backend.last_frame(), Rect::new(0.0, 0.0, 960.0, 1080.0));

        // ...and the move was committed, so Restore still works.
        *backend.focus_lost.borrow_mut() = false;
        animated_pipeline(
            &backend,
            &mut engine,
            WindowAction::Restore,
            params(),
            &mut FixedPacer,
            &mut || None,
        )
        .unwrap();
        assert_eq!(backend.last_frame(), Rect::new(100.0, 100.0, 400.0, 300.0));
    }

    #[test]
    fn the_animation_starts_from_the_frame_left_by_the_restore() {
        // Opening the session is what leaves a maximized or full-screen
        // window, and that moves the window. Starting the animator from the
        // pre-restore rectangle would make the first frame jump.
        let restored = Rect::new(300.0, 300.0, 500.0, 400.0);
        let planned_against = Rect::new(100.0, 100.0, 400.0, 300.0);

        // Run at every rate a platform cap can produce, plus the configurable
        // floor. How far a single frame travels depends on the rate, so a test
        // that inspects the first frame has to hold at all of them.
        for fps in rates_to_cover() {
            let backend = FakeBackend::with_restore_to(restored);
            let mut engine = engine_with_animation();
            let params = AnimationParams {
                duration_ms: 340,
                fps,
            };

            animated_pipeline(
                &backend,
                &mut engine,
                WindowAction::LeftHalf,
                params,
                &mut FixedPacer,
                &mut || None,
            )
            .unwrap();

            let frames = backend.via_session.borrow();
            let first = *frames.first().expect("no frame was applied");

            // Predict the first frame from each candidate origin using the
            // animator directly, which is deterministic, and assert the
            // observed frame matches the restored one exactly.
            //
            // This is deliberately not a distance tolerance. How far a single
            // frame travels depends on the frame rate — at 15fps the first
            // frame is already most of the way there — so any threshold that
            // separates the two origins at one rate fails at another. An
            // earlier version of this test did exactly that and passed on
            // Windows while failing on macOS's 45fps cap.
            let interval = animate::effective_interval(params);
            let target = Rect::new(0.0, 0.0, 960.0, 1080.0);
            let from_restored = Animator::new(restored, target, params).step(interval);
            let from_planned = Animator::new(planned_against, target, params).step(interval);

            assert_eq!(
                first, from_restored,
                "at {fps}fps the animation did not start from the restored frame"
            );
            assert_ne!(
                from_restored, from_planned,
                "at {fps}fps the two origins are indistinguishable, so this \
                 test proves nothing"
            );
            assert_eq!(*frames.last().unwrap(), target);
        }
    }

    #[test]
    fn the_final_frame_lands_through_the_session() {
        // `set_window_frame` identifies the window by focus on macOS, so the
        // last frame has to go through the session's retained handle instead —
        // otherwise a click elsewhere mid-animation strands the window.
        let backend = FakeBackend::with_restore_to(Rect::new(300.0, 300.0, 500.0, 400.0));
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

        assert!(
            *backend.finished_via_session.borrow(),
            "the final frame did not go through the session"
        );
        // Nothing went through the focus-dependent path at all.
        assert!(backend.frames.borrow().is_empty());
    }

    #[test]
    fn an_action_on_another_window_lands_the_first_one() {
        // The different-window preemption path: the flight in progress must be
        // finalized and committed before the new window is planned, or it is
        // left on an intermediate frame with no history entry. Every other
        // test retargets a single window, so this is the only coverage of
        // `land` being reached through a focus change.
        let backend = FakeBackend::new();
        let mut engine = engine_with_animation();
        let first_original = backend.focused_window().unwrap().unwrap().frame;

        animated_pipeline(
            &backend,
            &mut engine,
            WindowAction::LeftHalf,
            params(),
            &mut FixedPacer,
            &mut || {
                // Focus moves to a second window at the same moment the next
                // action arrives.
                if *backend.focused_id.borrow() == 1 && !backend.frames.borrow().is_empty() {
                    *backend.focused_id.borrow_mut() = 2;
                    Some(WindowAction::RightHalf)
                } else {
                    None
                }
            },
        )
        .unwrap();

        // The second window ends on its own target.
        assert_eq!(backend.last_frame(), Rect::new(960.0, 0.0, 960.0, 1080.0));

        // The first window was landed on the left half, not abandoned...
        assert!(
            backend
                .frames
                .borrow()
                .iter()
                .any(|f| *f == Rect::new(0.0, 0.0, 960.0, 1080.0)),
            "the first window was never landed on its target"
        );
        // ...and committed, so its Restore point is the pre-Tile frame.
        assert_eq!(engine.history.peek(1), Some(first_original));
    }

    #[test]
    fn a_failure_mid_flight_does_not_strand_the_window() {
        // Accessibility revocation makes intermediate frames fail. The error
        // must still reach the caller — the permission dialog depends on it —
        // but the window must not be left on an arbitrary intermediate frame
        // with no record of the move, or the next action would treat that
        // rectangle as the user's own placement and make it the Restore point.
        let backend = FakeBackend::failing_after(3);
        let mut engine = engine_with_animation();
        let original = backend.focused_window().unwrap().unwrap().frame;

        let err = animated_pipeline(
            &backend,
            &mut engine,
            WindowAction::LeftHalf,
            params(),
            &mut FixedPacer,
            &mut || None,
        )
        .expect_err("the failing backend should surface its error");
        assert!(
            is_permission_denied(&err),
            "the original error must propagate unchanged, got {err}"
        );

        // The window was landed on the target it was heading for.
        assert!(
            *backend.finished_via_session.borrow(),
            "the stranded flight was never landed"
        );
        assert_eq!(
            *backend.via_session.borrow().last().unwrap(),
            Rect::new(0.0, 0.0, 960.0, 1080.0)
        );

        // ...and recorded, so Restore still points at the pre-Tile frame
        // rather than at wherever the animation happened to stop.
        assert_eq!(engine.history.peek(1), Some(original));
    }

    #[test]
    fn a_no_op_after_a_retarget_does_not_corrupt_restore() {
        // The sequence that used to poison history: an action is committed at
        // its target when a second press supersedes it, the second is
        // committed when a third arrives, and the third turns out to be a
        // no-op — so the flight settles while already marked committed.
        //
        // Reconciling that from the flight's original "before" frame no longer
        // matches the stored `last_applied`, so history inserted a fresh entry
        // whose original was the mid-flight frame, and Restore returned there
        // instead of to the pre-Tile position.
        let backend = FakeBackend::new();
        // `DoNothing` is what makes the third press a no-op rather than a
        // cycle step.
        let config = Config {
            animation: AnimationConfig {
                enabled: true,
                duration_ms: 340,
                fps: 90,
            },
            subsequent_execution_mode: tile_core::SubsequentExecutionMode::DoNothing,
            ..Default::default()
        };
        let mut engine = Engine::new(config);
        let original = backend.focused_window().unwrap().unwrap().frame;

        animated_pipeline(
            &backend,
            &mut engine,
            WindowAction::LeftHalf,
            params(),
            &mut FixedPacer,
            &mut after_frames(2, vec![WindowAction::TopHalf, WindowAction::TopHalf]),
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

        assert_eq!(
            backend.last_frame(),
            original,
            "Restore returned to a mid-flight frame instead of the pre-Tile one"
        );
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
