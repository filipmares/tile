//! Shared application state and the action pipeline that is the heart of Tile.
//!
//! Threading model:
//! * The **main thread** runs Tauri's event loop and owns tray/menu handling.
//!   On macOS the hotkey backend must be constructed here (Carbon
//!   `RegisterEventHotKey` needs the main thread's run loop), which is why the
//!   backend is built inside Tauri's `setup` closure.
//! * A single **worker thread** owns the [`std::sync::mpsc::Receiver`] end of
//!   the action channel, which carries both hotkey presses and tray menu
//!   clicks as [`ActionRequest`]s, and drains it through
//!   [`AppState::perform_action_preemptible`].
//! * The window backend, engine and hotkey backend are each behind a [`Mutex`]
//!   inside [`AppState`], which Tauri manages, so both the worker thread and
//!   the command handlers (settings window) drive the same pipeline.
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
//! permission monitor, which checks every few seconds. Rapid hotkeys are handled by
//! preemption instead of by queueing — see
//! [`AppState::perform_action_preemptible`].

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Mutex, MutexGuard};
use std::time::Duration;

use tile_core::{
    AnimationParams, Animator, Config, Engine, Hotkey, Plan, Rect, Screen, WindowAction, WindowId,
    WindowSnapshot,
};
use tile_platform::{
    AnimationSession, HotkeyApplyReport, HotkeyBackend, HotkeyBinding, PermissionStatus,
    PlatformError, WindowBackend,
};

use crate::animate::{self, Interruption, Pacer, SleepPacer};
use crate::build_kind::BuildKind;
use crate::config_store;
use crate::permission::{GrantStep, PermissionTracker, Transition};
use crate::ratelimit::RateLimiter;
use crate::settings_error::SettingsError;

/// How long a `PermissionDenied` dialog is suppressed after being shown once.
const PERMISSION_DIALOG_COOLDOWN: Duration = Duration::from_secs(20);

/// How long a failed tray-menu action stays quiet after telling the user once.
const MENU_NOTICE_COOLDOWN: Duration = Duration::from_secs(10);

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct HotkeyStatus {
    pub report: Option<HotkeyApplyReport>,
    pub apply_error: Option<String>,
}

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
    hotkey_status: Mutex<HotkeyStatus>,
    /// Told after every apply so the app can push the new status out.
    hotkey_status_notifier: Mutex<Option<Box<dyn Fn() + Send>>>,
    permission_dialog_limiter: Mutex<RateLimiter>,
    menu_notice_limiter: Mutex<RateLimiter>,
    /// The shared Accessibility permission state. See [`crate::permission`].
    permission: Mutex<PermissionTracker>,
    /// Whether the one-time first-run orientation is still owed to the user.
    /// Set once at startup and cleared as soon as the settings UI claims it,
    /// so a reopened settings window never shows it twice in one session.
    orientation_pending: AtomicBool,
    /// An unreadable config this launch replaced with defaults, until the user
    /// dismisses the notice about it.
    config_recovery: Mutex<Option<config_store::ConfigRecovery>>,
    /// Set when an unreadable config could not be moved aside. Saving would
    /// overwrite the user's only copy, so nothing is written this session.
    saves_blocked: bool,
    /// The config a restore-defaults replaced, until any other settings
    /// change makes undoing it unsafe.
    reset_undo: Mutex<Option<Config>>,
    /// Serializes settings writes across every window; see
    /// [`SettingsTransaction`].
    settings: Mutex<()>,
    /// Bumped by every committed settings write, so a caller can tell whether
    /// a transaction changed anything and other windows need telling.
    revision: AtomicU64,
}

/// Changes the OS login item, as the settings commands do. Injected so the
/// transaction can be exercised without a running app.
pub type LoginItem<'a> = dyn FnMut(bool) -> Result<(), SettingsError> + 'a;

/// Exclusive access to the settings for one whole read, login item, save and
/// apply sequence.
///
/// The settings and welcome windows can both be open and both write, so a
/// change like launch at login — which reads the old value, moves the OS
/// login item, saves, and may have to move the login item back — must not
/// interleave with another window's write. Every settings write holds one of
/// these from its first read to its last side effect.
///
/// Nothing done under it may wait on the main thread: Tauri runs synchronous
/// commands there, so a holder that blocked on it would deadlock against the
/// next command. Tray menu updates happen after it is dropped; emitting an
/// event only queues work and is safe under it. The lock is not reentrant:
/// while holding one, use its methods, never the [`AppState`] conveniences
/// that take their own.
pub struct SettingsTransaction<'a> {
    state: &'a AppState,
    _guard: MutexGuard<'a, ()>,
}

impl AppState {
    pub fn new(
        backend: Box<dyn WindowBackend>,
        hotkeys: Box<dyn HotkeyBackend>,
        config: Config,
        build_kind: BuildKind,
        config_dir: Option<PathBuf>,
        orientation_pending: bool,
    ) -> Self {
        Self {
            backend: Mutex::new(backend),
            hotkeys: Mutex::new(hotkeys),
            engine: Mutex::new(Engine::new(config)),
            build_kind,
            config_dir,
            hotkey_status: Mutex::new(HotkeyStatus::default()),
            hotkey_status_notifier: Mutex::new(None),
            permission_dialog_limiter: Mutex::new(RateLimiter::new(PERMISSION_DIALOG_COOLDOWN)),
            menu_notice_limiter: Mutex::new(RateLimiter::new(MENU_NOTICE_COOLDOWN)),
            permission: Mutex::new(PermissionTracker::new()),
            orientation_pending: AtomicBool::new(orientation_pending),
            config_recovery: Mutex::new(None),
            saves_blocked: false,
            reset_undo: Mutex::new(None),
            settings: Mutex::new(()),
            revision: AtomicU64::new(0),
        }
    }

    /// Starts a settings write, waiting for any other window's to finish.
    pub fn settings_transaction(&self) -> SettingsTransaction<'_> {
        SettingsTransaction {
            state: self,
            _guard: lock(&self.settings),
        }
    }

    /// Records that this launch started from defaults because the config could
    /// not be read. If the old file could not be kept, saving is switched off
    /// for the session so it is never overwritten.
    pub fn with_config_recovery(mut self, recovery: Option<config_store::ConfigRecovery>) -> Self {
        self.saves_blocked = recovery
            .as_ref()
            .is_some_and(|recovery| recovery.backup_path.is_none());
        self.config_recovery = Mutex::new(recovery);
        self
    }

    /// The pending notice about an unreadable config, if it has not been
    /// dismissed yet.
    pub fn config_recovery(&self) -> Option<config_store::ConfigRecovery> {
        lock(&self.config_recovery).clone()
    }

    /// Dismisses the notice for the rest of this session. The backup itself is
    /// left exactly where it is.
    pub fn dismiss_config_recovery(&self) {
        lock(&self.config_recovery).take();
    }

    /// Whether the first-run orientation still needs showing, without
    /// consuming it.
    pub fn orientation_pending(&self) -> bool {
        self.orientation_pending.load(Ordering::Relaxed)
    }

    /// Claims the pending orientation in a transaction of its own; see
    /// [`SettingsTransaction::take_orientation`].
    pub fn take_orientation(&self) -> bool {
        self.settings_transaction().take_orientation()
    }

    /// Whether this binary is a local development build or an installed one.
    pub fn build_kind(&self) -> BuildKind {
        self.build_kind
    }

    /// Where the config is being read from and written to, if anywhere.
    pub fn config_dir(&self) -> Option<&Path> {
        self.config_dir.as_deref()
    }

    /// A snapshot of the current configuration.
    pub fn config(&self) -> Config {
        lock(&self.engine).config.clone()
    }

    /// The last confirmed native routes and the latest apply error, if any.
    pub fn hotkey_status(&self) -> HotkeyStatus {
        lock(&self.hotkey_status).clone()
    }

    /// Reports the OS permission status, optionally prompting the user. The
    /// prompt must only ever be requested from the main thread.
    pub fn permission_status(&self, prompt: bool) -> tile_platform::Result<PermissionStatus> {
        lock(&self.backend).permission_status(prompt)
    }

    /// Checks trust and records it in the shared tracker, returning the
    /// transition it caused. Callers should go through
    /// [`crate::permission::refresh`], which acts on that transition.
    ///
    /// The tracker stays locked across the check (tracker, then backend — the
    /// only order these two are ever taken in), so two concurrent checks can
    /// never record their answers out of order and invent a transition.
    pub fn refresh_permission(
        &self,
        prompt: bool,
    ) -> tile_platform::Result<(PermissionStatus, Option<Transition>)> {
        let mut tracker = lock(&self.permission);
        if prompt {
            tracker.mark_prompted();
        }
        let status = self.permission_status(prompt)?;
        Ok((status, tracker.observe(status)))
    }

    /// Whether Tile is known to lack the permission it needs right now.
    pub fn permission_blocked(&self) -> bool {
        lock(&self.permission).is_blocked()
    }

    /// What the settings window's grant button should do next.
    pub fn grant_step(&self) -> GrantStep {
        lock(&self.permission).grant_step()
    }

    /// How long the permission monitor should wait before checking again.
    pub fn permission_poll_interval(&self) -> Duration {
        lock(&self.permission).poll_interval()
    }

    /// How many displays are connected right now.
    ///
    /// The welcome window asks so it can leave out the "send it to your other
    /// display" step on a laptop with nothing plugged in, rather than teaching
    /// a shortcut that would do nothing.
    pub fn screen_count(&self) -> tile_platform::Result<usize> {
        Ok(lock(&self.backend).screens()?.len())
    }

    /// Directional destinations in the same geometric order as the welcome
    /// stage. Resolve them with the engine's geometry rather than guessing in JS.
    pub fn display_neighbors(
        &self,
    ) -> tile_platform::Result<Vec<std::collections::BTreeMap<WindowAction, usize>>> {
        let screens = lock(&self.backend).screens()?;
        let ordered = Screen::geometrically_ordered(&screens);
        Ok(ordered
            .iter()
            .map(|screen| {
                WindowAction::ALL
                    .iter()
                    .filter_map(|action| {
                        let direction = action.display_direction()?;
                        let destination = Screen::in_direction(&screens, screen, direction)?;
                        let index = ordered.iter().position(|s| s.id == destination.id)?;
                        Some((*action, index))
                    })
                    .collect()
            })
            .collect())
    }

    /// Whether anything Tile could move is focused right now.
    ///
    /// Tile skips its own windows, so this stays true while the welcome window
    /// itself is in front — it reports the window that would actually move.
    pub fn has_movable_window(&self) -> tile_platform::Result<bool> {
        Ok(lock(&self.backend).focused_window()?.is_some())
    }

    /// Where the window a shortcut would move is sitting, as an index into
    /// [`Screen::geometrically_ordered`].
    ///
    /// The welcome stage draws one miniature per display, left to right. It has
    /// to start its pane on the display the real window is actually on, or the
    /// first shortcut moves a window on one screen while the mirror of it moves
    /// on another. Geometric order rather than the backend's enumeration for
    /// the same reason display throws use it: the OS lists displays in an order
    /// that says nothing about where they sit, and the main display is often
    /// not the leftmost one.
    ///
    /// Falls back to the primary display when nothing movable is focused, which
    /// is where a window would most likely open.
    pub fn current_screen_index(&self) -> tile_platform::Result<usize> {
        // One lock for both reads: a display unplugged between them would
        // otherwise yield an index into a screen list that no longer exists.
        let backend = lock(&self.backend);
        let screens = backend.screens()?;
        let focused = backend.focused_window()?;

        if let Some(window) = &focused {
            if let Some(index) = screen_index(&screens, window.frame) {
                return Ok(index);
            }
        }

        let fallback = screens
            .iter()
            .find(|screen| screen.is_primary)
            .or_else(|| screens.first());

        Ok(fallback
            .and_then(|screen| {
                Screen::geometrically_ordered(&screens)
                    .iter()
                    .position(|ordered| ordered.id == screen.id)
            })
            .unwrap_or(0))
    }

    /// Runs the full pipeline for `action`: read the focused window and
    /// screens, ask the engine for a [`Plan`], apply it, and commit history
    /// using the frame the backend actually produced.
    ///
    /// Callers with no source of further actions (the settings
    /// window) use this; the hotkey worker uses
    /// [`AppState::perform_action_preemptible`] so a second press can steer an
    /// animation that is still in flight.
    pub fn perform_action(&self, action: WindowAction) -> tile_platform::Result<ActionOutcome> {
        self.perform_action_preemptible(action, &mut || None, &mut |_| {})
    }

    /// As [`AppState::perform_action`], but able to pick up further actions
    /// while a window is still animating.
    ///
    /// `next` is polled once per animation frame and should yield an action
    /// that has already arrived without blocking — the hotkey worker passes a
    /// non-blocking receive on its channel. With animation switched off it is
    /// never called, and the pipeline is exactly what it always was.
    ///
    /// `observe` is told about each action the moment its fate is decided,
    /// which is *before* the window has begun travelling. Two things make
    /// that the right moment rather than an optimistic one. The verdict is
    /// already final — planning is what decides it, and by then the window
    /// has been found and successfully opened for animation, so a report of
    /// `Moved` has survived three real round-trips with the OS rather than
    /// being assumed. And a burst of presses produces one return value but
    /// several decisions: everything after the first is absorbed by `next` to
    /// retarget the flight, so a caller watching only the return value would
    /// never hear about them at all.
    pub fn perform_action_preemptible(
        &self,
        request: impl Into<ActionRequest>,
        next: &mut dyn FnMut() -> Option<WindowAction>,
        observe: &mut dyn FnMut(ActionReport),
    ) -> tile_platform::Result<ActionOutcome> {
        let request = request.into();
        let backend = lock(&self.backend);
        let mut engine = lock(&self.engine);

        let animation = engine.config.animation;
        if !animation.enabled {
            return apply_once(backend.as_ref(), &mut engine, request, observe);
        }

        animated_pipeline(
            backend.as_ref(),
            &mut engine,
            request,
            animation.params(),
            &mut SleepPacer::new(),
            next,
            observe,
        )
    }

    /// Applies the currently bound hotkeys and records their last confirmed
    /// routes plus any apply error.
    pub fn apply_hotkeys(&self) -> HotkeyStatus {
        let config = lock(&self.engine).config.clone();
        let bindings: Vec<_> = config
            .active_bindings()
            .into_iter()
            .map(|(hotkey, action)| HotkeyBinding {
                hotkey,
                action,
                repeat: action.repeats_while_held(),
            })
            .collect();
        let result = lock(&self.hotkeys).apply(&bindings);
        let snapshot = {
            let mut status = lock(&self.hotkey_status);
            match result {
                Ok(report) => {
                    if let Some(warning) = &report.warning {
                        log::warn!("hotkeys applied with a cleanup warning: {warning}");
                    }
                    status.apply_error = None;
                    // Recovery may already have published something newer
                    // between the backend replying and this line.
                    if status
                        .report
                        .as_ref()
                        .map_or(true, |current| report.revision >= current.revision)
                    {
                        status.report = Some(report);
                    }
                }
                Err(err) => {
                    log::error!("failed to apply hotkeys: {err}");
                    if matches!(err, PlatformError::HotkeyStateUnknown(_)) {
                        status.report = None;
                    }
                    status.apply_error = Some(err.to_string());
                }
            }
            status.clone()
        };
        self.notify_hotkey_status();
        snapshot
    }

    /// Adopts a report the hotkey backend published on its own, after a
    /// recovery or a change in the keyboard hook's health. Returns whether it
    /// was newer than what is held; an older one lost a race with an apply.
    pub fn record_published_hotkeys(&self, report: HotkeyApplyReport) -> bool {
        let mut status = lock(&self.hotkey_status);
        let newer = status
            .report
            .as_ref()
            .map_or(true, |current| report.revision > current.revision);
        if newer {
            status.report = Some(report);
        }
        newer
    }

    /// Installs the callback told after every apply. It must not block.
    pub fn set_hotkey_status_notifier(&self, notifier: Box<dyn Fn() + Send>) {
        *lock(&self.hotkey_status_notifier) = Some(notifier);
    }

    fn notify_hotkey_status(&self) {
        if let Some(notifier) = lock(&self.hotkey_status_notifier).as_ref() {
            notifier();
        }
    }

    /// Writes `config` to the config directory. Without one, settings live in
    /// memory only — already logged at startup — and that is not an error.
    fn persist(&self, config: &Config) -> std::io::Result<()> {
        let Some(dir) = self.config_dir.as_deref() else {
            log::warn!("no config directory resolved; not persisting settings");
            return Ok(());
        };
        // Deliberate, and already explained by the config-recovery notice.
        if self.saves_blocked {
            log::warn!("not saving settings: the unreadable config could not be backed up first");
            return Ok(());
        }
        config_store::save_to_dir(dir, config)
    }

    /// Changes the config in a transaction of its own; see
    /// [`SettingsTransaction::update_config`].
    #[cfg(test)]
    pub fn update_config(&self, mutate: impl FnOnce(&mut Config)) -> Result<Config, SettingsError> {
        self.settings_transaction().update_config(mutate)
    }

    #[cfg(test)]
    pub fn set_binding(
        &self,
        action: WindowAction,
        hotkey: Option<Hotkey>,
        replace: &[WindowAction],
    ) -> Result<Config, SettingsError> {
        self.settings_transaction()
            .set_binding(action, hotkey, replace)
    }

    #[cfg(test)]
    pub fn reset_to_defaults(&self, launch_on_login: bool) -> Result<Config, SettingsError> {
        self.settings_transaction()
            .reset_to_defaults(launch_on_login)
    }

    #[cfg(test)]
    pub fn reset_undo_launch_on_login(&self) -> Option<bool> {
        self.settings_transaction().reset_undo_launch_on_login()
    }

    #[cfg(test)]
    pub fn undo_reset_to_defaults(
        &self,
        keep_login: Option<bool>,
    ) -> Result<Option<Config>, SettingsError> {
        self.settings_transaction()
            .undo_reset_to_defaults(keep_login)
    }

    /// The one path every settings write takes: mutate the config and the
    /// reset snapshot together under the engine lock, then normalize, persist,
    /// and (unless `reapply_hotkeys` is false) re-apply hotkeys. A failed save
    /// rolls both back, as does a `mutate` that refuses the change. Only a
    /// [`SettingsTransaction`] calls this.
    fn commit_config(
        &self,
        reapply_hotkeys: bool,
        mutate: impl FnOnce(&mut Config, &mut Option<Config>) -> Result<(), SettingsError>,
    ) -> Result<Config, SettingsError> {
        {
            let mut engine = lock(&self.engine);
            let mut undo = lock(&self.reset_undo);
            let previous = engine.config.clone();
            let previous_undo = undo.clone();
            if let Err(err) = mutate(&mut engine.config, &mut undo) {
                engine.config = previous;
                *undo = previous_undo;
                return Err(err);
            }
            engine.config.normalize();
            if let Err(err) = self.persist(&engine.config) {
                engine.config = previous;
                *undo = previous_undo;
                return Err(SettingsError::not_saved(err));
            }
            self.revision.fetch_add(1, Ordering::Relaxed);
        }
        if reapply_hotkeys {
            self.apply_hotkeys();
        }
        Ok(self.config())
    }

    /// Decides whether a `PermissionDenied` dialog should be shown now, given
    /// the rate limit. Returns `true` at most once per cooldown window.
    pub fn should_show_permission_dialog(&self) -> bool {
        lock(&self.permission_dialog_limiter).allow()
    }

    /// As [`AppState::should_show_permission_dialog`], for the notice a failed
    /// tray-menu action raises.
    pub fn should_show_menu_notice(&self) -> bool {
        lock(&self.menu_notice_limiter).allow()
    }

    /// Releases OS hotkeys. Called on shutdown.
    pub fn shutdown_hotkeys(&self) {
        lock(&self.hotkeys).shutdown();
    }
}

impl SettingsTransaction<'_> {
    /// The config as this transaction sees it.
    pub fn config(&self) -> Config {
        self.state.config()
    }

    /// How many settings writes have been committed so far. Compare before
    /// and after to learn whether this transaction changed anything.
    pub fn revision(&self) -> u64 {
        self.state.revision.load(Ordering::Relaxed)
    }

    /// Mutates the config, persists it, then re-applies hotkeys. Returns the
    /// updated config so callers (commands) can hand truth back to the UI.
    ///
    /// A change that cannot be saved is rolled back before anything else sees
    /// it, so the running app never disagrees with what is on disk and the UI
    /// can simply re-read the config to show the truth.
    ///
    /// Any change ends the chance to undo a restore-defaults: undoing after it
    /// would silently throw that change away.
    pub fn update_config(&self, mutate: impl FnOnce(&mut Config)) -> Result<Config, SettingsError> {
        self.state.commit_config(true, |config, undo| {
            *undo = None;
            mutate(config);
            Ok(())
        })
    }

    /// Binds `hotkey` to `action` as one config change. Other actions holding
    /// the hotkey are unbound in the same write only if every one of them is
    /// in `replace`, the holders the user agreed to replace. Otherwise nothing
    /// changes and the error names the holders, so a chord is never taken from
    /// an action the user was not asked about, even one that picked it up
    /// while they were being asked.
    pub fn set_binding(
        &self,
        action: WindowAction,
        hotkey: Option<Hotkey>,
        replace: &[WindowAction],
    ) -> Result<Config, SettingsError> {
        self.state.commit_config(true, |config, undo| {
            if let Some(hk) = hotkey {
                let holders = config.actions_using(hk, action);
                if holders.iter().any(|holder| !replace.contains(holder)) {
                    let names: Vec<_> = holders.iter().map(ToString::to_string).collect();
                    return Err(SettingsError::shortcut_taken(format!(
                        "{hk} is already used by {}",
                        names.join(", ")
                    )));
                }
            }
            *undo = None;
            config.set_binding(action, hotkey);
            Ok(())
        })
    }

    /// Claims the pending orientation, returning whether the caller won it.
    /// Only the first caller gets `true`.
    ///
    /// Winning the claim persists `orientation_shown` immediately rather than
    /// waiting for the user to dismiss the panel. Nothing else writes the
    /// config at startup, so deferring would mean a user who quits without
    /// touching a setting is shown the orientation again on the next launch.
    ///
    /// It commits like any other write, so it cannot interleave with one, but
    /// recording that a welcome panel appeared is not a settings change: it
    /// keeps the restore-defaults undo and does not re-register the OS
    /// hotkeys or disturb the recorded hotkey failures. A failed save is
    /// logged and rolled back like any other; the claim itself still stands,
    /// so the orientation is not shown twice in one session.
    pub fn take_orientation(&self) -> bool {
        let won = self
            .state
            .orientation_pending
            .swap(false, Ordering::Relaxed);
        if won {
            if let Err(err) = self.state.commit_config(false, |config, _| {
                config.orientation_shown = true;
                Ok(())
            }) {
                log::error!("could not record the first-run orientation: {err}");
            }
        }
        won
    }

    /// Changes the OS login item first and only then records the preference,
    /// so the checkbox never claims a login item the OS refused to create. A
    /// failed save puts the login item back.
    pub fn set_launch_on_login(
        &self,
        enabled: bool,
        login_item: &mut LoginItem<'_>,
    ) -> Result<Config, SettingsError> {
        let previous = self.config().launch_on_login;
        login_item(enabled)?;
        self.update_config(|config| config.launch_on_login = enabled)
            .map_err(|err| revert_login_item(login_item, previous, err))
    }

    /// Restores every default, login item included. If the login item cannot
    /// be changed, everything else is still restored and launch-at-login keeps
    /// its current value, so the preference keeps matching the OS; the error
    /// says so.
    pub fn restore_defaults(
        &self,
        login_item: &mut LoginItem<'_>,
    ) -> Result<Config, SettingsError> {
        let previous_login = self.config().launch_on_login;
        let default_login = Config::default().launch_on_login;
        let login = login_item(default_login);
        let launch_on_login = if login.is_ok() {
            default_login
        } else {
            previous_login
        };
        let config = self.reset_to_defaults(launch_on_login).map_err(|err| {
            if login.is_ok() {
                revert_login_item(login_item, previous_login, err)
            } else {
                err
            }
        })?;
        login.map(|()| config)
    }

    /// Puts back the settings the last restore replaced, login item included,
    /// or returns `None` once any other change has been made since, from any
    /// window. The login item is handled as in [`Self::restore_defaults`].
    pub fn undo_restore_defaults(
        &self,
        login_item: &mut LoginItem<'_>,
    ) -> Result<Option<Config>, SettingsError> {
        let Some(target_login) = self.reset_undo_launch_on_login() else {
            return Ok(None);
        };
        let previous_login = self.config().launch_on_login;
        let login = login_item(target_login);
        let kept_login = login.is_err().then_some(previous_login);
        let restored = self.undo_reset_to_defaults(kept_login).map_err(|err| {
            if login.is_ok() {
                revert_login_item(login_item, previous_login, err)
            } else {
                err
            }
        })?;
        let Some(restored) = restored else {
            // Unreachable while the transaction is held, since nothing else
            // can take the snapshot; if it ever happens, the login item
            // follows the config that stands.
            if login.is_ok() {
                login_item(self.config().launch_on_login)?;
            }
            return Ok(None);
        };
        login.map(|()| Some(restored))
    }

    /// Puts every setting back to its default, keeping what it replaced so
    /// [`Self::undo_reset_to_defaults`] can bring it back. Resetting again
    /// before anything else changed keeps the first snapshot: the settings
    /// worth getting back are the ones from before the first reset, not the
    /// defaults.
    ///
    /// `launch_on_login` is passed in because the login item is the caller's
    /// to change: it stays as it was when the OS refused. Whether the
    /// orientation was shown is a fact about this installation rather than a
    /// setting, so a reset never rewinds it.
    fn reset_to_defaults(&self, launch_on_login: bool) -> Result<Config, SettingsError> {
        self.state.commit_config(true, |config, undo| {
            let previous = std::mem::take(config);
            config.launch_on_login = launch_on_login;
            config.orientation_shown = previous.orientation_shown;
            undo.get_or_insert(previous);
            Ok(())
        })
    }

    /// What launch at login would become if the last reset were undone, or
    /// `None` when there is nothing to undo. Lets the caller move the login
    /// item before the undo is committed, as every other change does.
    fn reset_undo_launch_on_login(&self) -> Option<bool> {
        lock(&self.state.reset_undo)
            .as_ref()
            .map(|config| config.launch_on_login)
    }

    /// Restores the config a restore-defaults replaced, if no other change
    /// has been made since — from this window or any other. Returns `None`
    /// when there is nothing left to undo. `keep_login` overrides the restored
    /// launch-at-login value when the OS refused to change the login item.
    fn undo_reset_to_defaults(
        &self,
        keep_login: Option<bool>,
    ) -> Result<Option<Config>, SettingsError> {
        if lock(&self.state.reset_undo).is_none() {
            return Ok(None);
        }
        let mut restored = false;
        let config = self.state.commit_config(true, |config, undo| {
            if let Some(mut previous) = undo.take() {
                previous.orientation_shown |= config.orientation_shown;
                if let Some(launch_on_login) = keep_login {
                    previous.launch_on_login = launch_on_login;
                }
                *config = previous;
                restored = true;
            }
            Ok(())
        })?;
        Ok(restored.then_some(config))
    }
}

/// Puts the login item back after the preference that asked for the change
/// could not be saved, and returns the error to report. The save failure
/// stands only if the revert worked; otherwise the OS and the preference now
/// disagree, and the user has to hear that instead.
fn revert_login_item(
    login_item: &mut LoginItem<'_>,
    enabled: bool,
    save_error: SettingsError,
) -> SettingsError {
    match login_item(enabled) {
        Ok(()) => save_error,
        Err(revert_error) => SettingsError::out_of_sync(format!(
            "{save_error}; putting the login item back also failed: {revert_error}"
        )),
    }
}

/// Classifies an error from the pipeline for the caller's reaction.
pub fn is_permission_denied(err: &PlatformError) -> bool {
    matches!(err, PlatformError::PermissionDenied(_))
}

/// The unanimated pipeline: plan, apply in one jump, commit.
/// What one run of the action pipeline actually did.
///
/// Callers that only care about errors can ignore this, but the welcome
/// window's walkthrough cannot: it ticks a step off when Tile really moves
/// something, and tells the user to open a window when there was nothing to
/// move. Both of those are `Ok` today, so they have to be distinguishable.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ActionOutcome {
    /// A window was moved.
    Moved,
    /// Nothing movable was focused, so there was nothing to act on.
    NoWindow,
    /// There was a window, but the action would not change anything.
    NoOp,
}

/// One decided action, reported the moment its verdict is final.
///
/// Carries the display as well as the verdict because the only listener — the
/// welcome walkthrough — cannot ask for it afterwards: the pipeline holds the
/// backend lock while reporting, so a listener that called back in to find out
/// where the window went would deadlock against the very action it was told
/// about.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ActionReport {
    pub action: WindowAction,
    pub outcome: ActionOutcome,
    /// The display the window is on, or heading for, counted left to right.
    /// `None` when there was no window to act on.
    pub screen: Option<usize>,
}

/// Which display `frame` belongs to, as an index into
/// [`Screen::geometrically_ordered`] — the left-to-right order the welcome
/// stage draws its miniatures in.
fn screen_index(screens: &[Screen], frame: Rect) -> Option<usize> {
    let screen = Screen::best_match(screens, &frame)?;
    Screen::geometrically_ordered(screens)
        .iter()
        .position(|ordered| ordered.id == screen.id)
}

pub use tile_core::ActionRequest;

fn plan(
    engine: &Engine,
    action: WindowAction,
    exact: bool,
    window: &WindowSnapshot,
    screens: &[Screen],
) -> Plan {
    if exact {
        engine.plan_exact(action, window, screens)
    } else {
        engine.plan(action, window, screens)
    }
}

fn apply_once(
    backend: &dyn WindowBackend,
    engine: &mut Engine,
    request: impl Into<ActionRequest>,
    observe: &mut dyn FnMut(ActionReport),
) -> tile_platform::Result<ActionOutcome> {
    let ActionRequest { action, exact } = request.into();
    let Some(window) = backend.focused_window()? else {
        log::debug!("ignoring {action}: no movable focused window");
        observe(ActionReport {
            action,
            outcome: ActionOutcome::NoWindow,
            screen: None,
        });
        return Ok(ActionOutcome::NoWindow);
    };
    let screens = backend.screens()?;

    match plan(engine, action, exact, &window, &screens) {
        Plan::Move { id, target } => {
            let actual = backend.set_window_frame(id, target)?;
            engine.commit(action, &window, actual);
            log::debug!("performed {action} on window {id}");
            observe(ActionReport {
                action,
                outcome: ActionOutcome::Moved,
                screen: screen_index(&screens, actual),
            });
            Ok(ActionOutcome::Moved)
        }
        Plan::NoOp(reason) => {
            log::debug!("no-op for {action}: {reason:?}");
            observe(ActionReport {
                action,
                outcome: ActionOutcome::NoOp,
                screen: screen_index(&screens, window.frame),
            });
            Ok(ActionOutcome::NoOp)
        }
    }
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
    request: impl Into<ActionRequest>,
    params: AnimationParams,
    pacer: &mut dyn Pacer,
    next: &mut dyn FnMut() -> Option<WindowAction>,
    observe: &mut dyn FnMut(ActionReport),
) -> tile_platform::Result<ActionOutcome> {
    let mut flight: Option<Flight> = None;
    let mut outcome = ActionOutcome::NoOp;
    let result = run_animated_pipeline(
        backend,
        engine,
        request.into(),
        params,
        pacer,
        next,
        observe,
        &mut flight,
        &mut outcome,
    );

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

    result.map(|()| outcome)
}

/// The pipeline proper. Hands its in-flight window back through `flight` so
/// [`animated_pipeline`] can reconcile it if any step fails, and its verdict
/// back through `outcome` so a caller can tell a real move from the two ways
/// an action legitimately does nothing.
#[allow(clippy::too_many_arguments)]
fn run_animated_pipeline(
    backend: &dyn WindowBackend,
    engine: &mut Engine,
    request: ActionRequest,
    params: AnimationParams,
    pacer: &mut dyn Pacer,
    next: &mut dyn FnMut() -> Option<WindowAction>,
    observe: &mut dyn FnMut(ActionReport),
    flight: &mut Option<Flight>,
    outcome: &mut ActionOutcome,
) -> tile_platform::Result<()> {
    // Only the first action can be exact. Anything that preempts it mid-flight
    // came from a hotkey, so it keeps the ordinary repeat-to-cycle behaviour.
    let mut exact = request.exact;
    let mut pending = Some(request.action);

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
                if flight.is_none() {
                    *outcome = ActionOutcome::NoWindow;
                }
                observe(ActionReport {
                    action,
                    outcome: ActionOutcome::NoWindow,
                    screen: None,
                });
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

            match plan(
                engine,
                action,
                std::mem::take(&mut exact),
                &window,
                &screens,
            ) {
                Plan::Move { id, target } => {
                    *outcome = ActionOutcome::Moved;
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

                    // Only now is the move a fact rather than a plan: the
                    // window was found, planned against, and successfully
                    // opened for animation — three real round-trips with the
                    // OS. No pixel has moved yet, which is the point. The
                    // walkthrough's own little pane sets off at the same
                    // instant as the window it is describing, rather than
                    // waiting for it to arrive and then repeating the journey.
                    //
                    // The display reported is the one the window is heading
                    // *for*, not the one it is leaving, so a throw across
                    // screens moves the miniature with the window.
                    observe(ActionReport {
                        action,
                        outcome: ActionOutcome::Moved,
                        screen: screen_index(&screens, target),
                    });
                }
                Plan::NoOp(reason) => {
                    // Nothing to do for this action, but a window already in
                    // flight must still finish its journey.
                    log::debug!("no-op for {action}: {reason:?}");
                    observe(ActionReport {
                        action,
                        outcome: ActionOutcome::NoOp,
                        screen: screen_index(&screens, window.frame),
                    });
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
    use std::sync::atomic::AtomicUsize;
    use std::sync::Arc;
    use std::time::Duration;

    use tile_core::{AnimationConfig, Hotkey, KeyCode, Modifiers, Screen};

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
            &mut |_| {},
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
            &mut |_| {},
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
            &mut |_| {},
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
            &mut |_| {},
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
                &mut |_| {},
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
            &mut |_| {},
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
            &mut |_| {},
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
            &mut |_| {},
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
            &mut |_| {},
        )
        .unwrap();

        animated_pipeline(
            &backend,
            &mut engine,
            WindowAction::Restore,
            params(),
            &mut FixedPacer,
            &mut || None,
            &mut |_| {},
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
            &mut |_| {},
        )
        .unwrap();
        animated_pipeline(
            &backend,
            &mut engine,
            WindowAction::Restore,
            params(),
            &mut FixedPacer,
            &mut || None,
            &mut |_| {},
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
            &mut |_| {},
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
            &mut |_| {},
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
            &mut |_| {},
        )
        .unwrap();

        let cycled = backend.last_frame();
        assert!(
            cycled.width > 960.0,
            "expected the cycle to grow past a half, got {cycled:?}"
        );
    }

    #[test]
    fn every_press_is_reported_even_when_absorbed_mid_flight() {
        // The walkthrough counts presses. A burst returns a single verdict —
        // everything after the first press is swallowed to retarget the
        // flight — so the observer, not the return value, is what has to see
        // all three.
        let backend = FakeBackend::new();
        let mut engine = engine_with_animation();
        let mut seen: Vec<(WindowAction, ActionOutcome)> = Vec::new();

        animated_pipeline(
            &backend,
            &mut engine,
            WindowAction::LeftHalf,
            params(),
            &mut FixedPacer,
            &mut after_frames(2, vec![WindowAction::TopHalf, WindowAction::RightHalf]),
            &mut |report| seen.push((report.action, report.outcome)),
        )
        .unwrap();

        assert_eq!(
            seen,
            vec![
                (WindowAction::LeftHalf, ActionOutcome::Moved),
                (WindowAction::TopHalf, ActionOutcome::Moved),
                (WindowAction::RightHalf, ActionOutcome::Moved),
            ]
        );
    }

    #[test]
    fn a_move_is_reported_before_the_window_starts_travelling() {
        // Reporting on arrival would leave the walkthrough silent for the
        // whole animation, then ask it to replay a journey the user has
        // already watched. The report lands before the first frame, so both
        // windows move together.
        //
        // It is not a guess: by then the window has been found, planned
        // against, and opened for animation, and a failure later in the pump
        // still lands the window on this exact target.
        let backend = FakeBackend::new();
        let mut engine = engine_with_animation();
        let mut frames_at_report = None;

        animated_pipeline(
            &backend,
            &mut engine,
            WindowAction::LeftHalf,
            params(),
            &mut FixedPacer,
            &mut || None,
            &mut |_| frames_at_report = Some(backend.frames.borrow().len()),
        )
        .unwrap();

        let total = backend.frames.borrow().len();
        assert!(total > 3, "expected a real animation, got {total} frames");
        assert_eq!(
            frames_at_report,
            Some(0),
            "the report must precede the motion, not trail it"
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
            &mut |_| {},
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

        apply_once(&backend, &mut engine, WindowAction::LeftHalf, &mut |_| {}).unwrap();

        assert_eq!(backend.frames.borrow().len(), 1);
        assert_eq!(backend.last_frame(), Rect::new(0.0, 0.0, 960.0, 1080.0));
    }

    /// A tray item labelled "½" must not cycle to ⅔ when chosen twice, on
    /// either pipeline.
    #[test]
    fn exact_requests_do_not_cycle() {
        let half = Rect::new(0.0, 0.0, 960.0, 1080.0);
        let exact = ActionRequest::exact(WindowAction::LeftHalf);

        let backend = FakeBackend::new();
        let mut engine = Engine::new(Config::default());
        apply_once(&backend, &mut engine, exact, &mut |_| {}).unwrap();
        assert_eq!(
            apply_once(&backend, &mut engine, exact, &mut |_| {}).unwrap(),
            ActionOutcome::NoOp
        );
        assert_eq!(backend.last_frame(), half);

        let backend = FakeBackend::new();
        let mut engine = engine_with_animation();
        for _ in 0..2 {
            animated_pipeline(
                &backend,
                &mut engine,
                exact,
                params(),
                &mut FixedPacer,
                &mut || None,
                &mut |_| {},
            )
            .unwrap();
        }
        assert_eq!(backend.last_frame(), half);
    }

    /// The welcome walkthrough ticks a step off on `Moved` and asks the user
    /// to open a window on `NoWindow`, so a pipeline that reported both as
    /// plain success would have it congratulating people for nothing.
    #[test]
    fn the_outcome_tells_a_move_apart_from_having_nothing_to_move() {
        let backend = FakeBackend::new();
        let mut engine = Engine::new(Config::default());

        assert_eq!(
            apply_once(&backend, &mut engine, WindowAction::LeftHalf, &mut |_| {}).unwrap(),
            ActionOutcome::Moved
        );

        let empty = InertWindowBackend;
        assert_eq!(
            apply_once(&empty, &mut engine, WindowAction::LeftHalf, &mut |_| {}).unwrap(),
            ActionOutcome::NoWindow
        );
    }

    /// Minimal backends so `AppState` can be built in a test. Neither is
    /// exercised here: the orientation claim never touches a window or a
    /// hotkey, it only decides whether to persist a flag.
    struct InertWindowBackend;

    impl WindowBackend for InertWindowBackend {
        fn focused_window(&self) -> tile_platform::Result<Option<WindowSnapshot>> {
            Ok(None)
        }

        fn screens(&self) -> tile_platform::Result<Vec<Screen>> {
            Ok(Vec::new())
        }

        fn set_window_frame(&self, _id: WindowId, target: Rect) -> tile_platform::Result<Rect> {
            Ok(target)
        }

        fn permission_status(&self, _prompt: bool) -> tile_platform::Result<PermissionStatus> {
            Ok(PermissionStatus::NotRequired)
        }
    }

    /// Two displays arranged the way a laptop usually sits beside a larger
    /// main display: the primary is the *second* screen from the left.
    ///
    /// `screens` deliberately lists the primary first, which is how the OS
    /// tends to enumerate them, so a test using this backend fails if the
    /// index is taken from enumeration order rather than geometry.
    struct SideBySideBackend {
        focused: Option<Rect>,
    }

    impl SideBySideBackend {
        const LEFT: Rect = Rect {
            x: -1920.0,
            y: 243.0,
            width: 1440.0,
            height: 900.0,
        };
        const PRIMARY: Rect = Rect {
            x: 0.0,
            y: 0.0,
            width: 2560.0,
            height: 1440.0,
        };
    }

    impl WindowBackend for SideBySideBackend {
        fn focused_window(&self) -> tile_platform::Result<Option<WindowSnapshot>> {
            Ok(self.focused.map(|frame| WindowSnapshot { id: 1, frame }))
        }

        fn screens(&self) -> tile_platform::Result<Vec<Screen>> {
            Ok(vec![
                Screen {
                    id: "primary".into(),
                    frame: Self::PRIMARY,
                    work_area: Self::PRIMARY,
                    scale_factor: 1.0,
                    is_primary: true,
                },
                Screen {
                    id: "left".into(),
                    frame: Self::LEFT,
                    work_area: Self::LEFT,
                    scale_factor: 1.0,
                    is_primary: false,
                },
            ])
        }

        fn set_window_frame(&self, _id: WindowId, target: Rect) -> tile_platform::Result<Rect> {
            Ok(target)
        }

        fn permission_status(&self, _prompt: bool) -> tile_platform::Result<PermissionStatus> {
            Ok(PermissionStatus::NotRequired)
        }
    }

    fn state_looking_at(focused: Option<Rect>) -> AppState {
        AppState::new(
            Box::new(SideBySideBackend { focused }),
            Box::new(CountingHotkeyBackend::default()),
            Config::default(),
            BuildKind::Development,
            None,
            false,
        )
    }

    /// The welcome stage counts its miniatures left to right, and the main
    /// display is often not the leftmost one. A window on the primary of this
    /// desk belongs on the *second* miniature, not the first.
    #[test]
    fn the_current_screen_is_counted_left_to_right_not_by_enumeration() {
        let state = state_looking_at(Some(Rect::new(200.0, 200.0, 800.0, 600.0)));
        assert_eq!(state.current_screen_index().unwrap(), 1);
    }

    #[test]
    fn welcome_display_neighbors_match_directional_moves_and_bridge_keys() {
        let state = state_looking_at(None);
        let neighbors = state.display_neighbors().unwrap();
        assert_eq!(neighbors.len(), 2);
        assert_eq!(neighbors[0].len(), 1);
        assert_eq!(neighbors[0].get(&WindowAction::DisplayRight), Some(&1));
        assert_eq!(neighbors[1].len(), 1);
        assert_eq!(neighbors[1].get(&WindowAction::DisplayLeft), Some(&0));
        assert_eq!(
            serde_json::to_value(&neighbors).unwrap(),
            serde_json::json!([{"display-right": 1}, {"display-left": 0}])
        );
    }

    #[test]
    fn a_window_on_the_leftmost_display_is_the_first_screen() {
        let state = state_looking_at(Some(Rect::new(-1800.0, 300.0, 800.0, 600.0)));
        assert_eq!(state.current_screen_index().unwrap(), 0);
    }

    /// With nothing movable focused, the pane belongs where a window would
    /// most likely open rather than on whichever screen sits furthest left.
    #[test]
    fn without_a_focused_window_the_primary_display_is_used() {
        let state = state_looking_at(None);
        assert_eq!(state.current_screen_index().unwrap(), 1);
    }

    /// The stage mirrors the window it is describing, so a throw across
    /// displays has to report where the window is *going*. Reporting where it
    /// started would leave the miniature behind on the old screen.
    #[test]
    fn a_display_throw_reports_the_destination_screen() {
        let backend = SideBySideBackend {
            focused: Some(Rect::new(200.0, 200.0, 800.0, 600.0)),
        };
        let mut engine = Engine::new(Config::default());
        let mut seen: Vec<Option<usize>> = Vec::new();

        apply_once(
            &backend,
            &mut engine,
            WindowAction::DisplayLeft,
            &mut |report| seen.push(report.screen),
        )
        .unwrap();

        assert_eq!(
            seen,
            vec![Some(0)],
            "thrown off the primary, the window lands on the leftmost display"
        );
    }

    /// A press with nothing to act on still reaches the walkthrough, and must
    /// not claim a display it never touched.
    #[test]
    fn an_action_with_nothing_to_move_reports_no_display() {
        let backend = SideBySideBackend { focused: None };
        let mut engine = Engine::new(Config::default());
        let mut seen: Vec<Option<usize>> = Vec::new();

        apply_once(
            &backend,
            &mut engine,
            WindowAction::LeftHalf,
            &mut |report| seen.push(report.screen),
        )
        .unwrap();

        assert_eq!(seen, vec![None]);
    }

    /// Counts `apply` calls through a shared handle, so a test can prove that
    /// recording a UI flag did not re-register the OS hotkeys.
    #[derive(Default)]
    struct CountingHotkeyBackend {
        applies: Arc<AtomicUsize>,
    }

    impl HotkeyBackend for CountingHotkeyBackend {
        fn apply(
            &mut self,
            _bindings: &[HotkeyBinding],
        ) -> tile_platform::Result<HotkeyApplyReport> {
            self.applies.fetch_add(1, Ordering::Relaxed);
            Ok(HotkeyApplyReport::default())
        }

        fn shutdown(&mut self) {}
    }

    fn state_with_orientation(dir: &std::path::Path, pending: bool) -> AppState {
        state_counting_applies(dir, pending).0
    }

    fn state_counting_applies(
        dir: &std::path::Path,
        pending: bool,
    ) -> (AppState, Arc<AtomicUsize>) {
        let applies = Arc::new(AtomicUsize::new(0));
        let state = AppState::new(
            Box::new(InertWindowBackend),
            Box::new(CountingHotkeyBackend {
                applies: Arc::clone(&applies),
            }),
            Config::default(),
            BuildKind::Development,
            Some(dir.to_path_buf()),
            pending,
        );
        (state, applies)
    }

    /// An unreadable config that could not be moved aside is the user's only
    /// copy, so no setting change may be written over it.
    #[test]
    fn settings_are_not_saved_over_a_config_that_could_not_be_backed_up() {
        let dir =
            std::env::temp_dir().join(format!("tile-recovery-{}-{}", std::process::id(), line!()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = crate::config_store::config_file_path(&dir);
        std::fs::write(&path, b"precious but broken").unwrap();

        let state = state_with_orientation(&dir, false).with_config_recovery(Some(
            crate::config_store::ConfigRecovery {
                kind: crate::config_store::RecoveryKind::Corrupt,
                some_fields_reset: true,
                backup_path: None,
            },
        ));
        state
            .update_config(|config| config.launch_on_login = !config.launch_on_login)
            .unwrap();

        assert_eq!(std::fs::read(&path).unwrap(), b"precious but broken");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_recovery_notice_is_shown_until_dismissed() {
        let dir =
            std::env::temp_dir().join(format!("tile-recovery-{}-{}", std::process::id(), line!()));
        let backup = dir.join("config.corrupt-1.json");
        let state = state_with_orientation(&dir, false).with_config_recovery(Some(
            crate::config_store::ConfigRecovery {
                kind: crate::config_store::RecoveryKind::PartialReset,
                some_fields_reset: true,
                backup_path: Some(backup.clone()),
            },
        ));
        assert_eq!(
            state.config_recovery().and_then(|r| r.backup_path),
            Some(backup)
        );
        state.dismiss_config_recovery();
        assert!(state.config_recovery().is_none());
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// The orientation must record itself the moment it is claimed. Nothing
    /// else writes the config at startup, so a user who quits without touching
    /// a setting would otherwise be shown it again on the next launch.
    #[test]
    fn claiming_the_orientation_persists_it_immediately() {
        let dir = std::env::temp_dir().join(format!(
            "tile-orientation-{}-{}",
            std::process::id(),
            line!()
        ));
        std::fs::create_dir_all(&dir).unwrap();

        let state = state_with_orientation(&dir, true);
        assert!(state.take_orientation(), "the first claim wins");
        assert!(
            state.config().orientation_shown,
            "claiming must set the flag"
        );

        // The point of the test: it survives a restart, without any dismissal.
        let reloaded = crate::config_store::load_from_dir(&dir);
        assert!(
            reloaded.config.orientation_shown,
            "the flag must be on disk, not only in memory"
        );

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_orientation_can_only_be_claimed_once() {
        let dir = std::env::temp_dir().join(format!(
            "tile-orientation-{}-{}",
            std::process::id(),
            line!()
        ));
        std::fs::create_dir_all(&dir).unwrap();

        let state = state_with_orientation(&dir, true);
        assert!(state.take_orientation());
        assert!(!state.take_orientation(), "a second claim must lose");
        assert!(!state.orientation_pending());

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// An existing user is never owed an orientation, and claiming must not
    /// write a config on their behalf.
    #[test]
    fn a_returning_user_is_never_owed_an_orientation() {
        let dir = std::env::temp_dir().join(format!(
            "tile-orientation-{}-{}",
            std::process::id(),
            line!()
        ));
        std::fs::create_dir_all(&dir).unwrap();

        let state = state_with_orientation(&dir, false);
        assert!(!state.take_orientation());
        assert!(
            !crate::config_store::config_file_path(&dir).exists(),
            "nothing should have been written"
        );

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Recording that a welcome panel appeared is not a settings change, so it
    /// must not re-register the OS hotkeys or disturb the recorded failures.
    #[test]
    fn claiming_the_orientation_does_not_reapply_hotkeys() {
        let dir = std::env::temp_dir().join(format!(
            "tile-orientation-{}-{}",
            std::process::id(),
            line!()
        ));
        std::fs::create_dir_all(&dir).unwrap();

        let (state, applies) = state_counting_applies(&dir, true);
        assert_eq!(applies.load(Ordering::Relaxed), 0);

        assert!(state.take_orientation());

        assert_eq!(
            applies.load(Ordering::Relaxed),
            0,
            "claiming the orientation must not touch the hotkey backend"
        );
        // The flag still reached disk through the save-only path.
        assert!(
            crate::config_store::load_from_dir(&dir)
                .config
                .orientation_shown
        );

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A change that cannot reach disk must not linger in memory: the UI
    /// re-reads the config after an error and must see what is actually in
    /// force, and the OS hotkeys must not be re-registered for it.
    #[test]
    fn an_unsaved_change_is_rolled_back() {
        let dir =
            std::env::temp_dir().join(format!("tile-unsaved-{}-{}", std::process::id(), line!()));
        std::fs::create_dir_all(&dir).unwrap();
        // A file where the config directory should be makes every save fail.
        let blocked = dir.join("not-a-directory");
        std::fs::write(&blocked, b"").unwrap();

        let (state, applies) = state_counting_applies(&blocked, false);
        let before = state.config();

        let err = state
            .update_config(|config| config.animation.enabled = !config.animation.enabled)
            .unwrap_err();

        assert_eq!(err.kind, crate::settings_error::SettingsErrorKind::NotSaved);
        assert_eq!(state.config(), before, "the change must be rolled back");
        assert_eq!(applies.load(Ordering::Relaxed), 0);

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_saved_change_is_kept_and_applied() {
        let dir =
            std::env::temp_dir().join(format!("tile-saved-{}-{}", std::process::id(), line!()));
        std::fs::create_dir_all(&dir).unwrap();

        let (state, applies) = state_counting_applies(&dir, false);
        let config = state
            .update_config(|config| config.animation.enabled = false)
            .unwrap();

        assert!(!config.animation.enabled);
        assert!(
            !crate::config_store::load_from_dir(&dir)
                .config
                .animation
                .enabled
        );
        assert_eq!(applies.load(Ordering::Relaxed), 1);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A chord held by another action is only moved when the user chose
    /// Replace; otherwise nothing is written, applied, or taken.
    #[test]
    fn binding_a_taken_shortcut_needs_replace() {
        let dir =
            std::env::temp_dir().join(format!("tile-taken-{}-{}", std::process::id(), line!()));
        std::fs::create_dir_all(&dir).unwrap();

        let (state, applies) = state_counting_applies(&dir, false);
        let before = state.config();
        let hk = before.binding(WindowAction::LeftHalf).unwrap();

        let err = state
            .set_binding(WindowAction::Center, Some(hk), &[])
            .unwrap_err();
        assert_eq!(
            err.kind,
            crate::settings_error::SettingsErrorKind::ShortcutTaken
        );
        assert_eq!(state.config(), before);
        assert_eq!(applies.load(Ordering::Relaxed), 0);

        // Approval to replace one action does not cover a different holder,
        // such as one that took the chord while the user was being asked.
        let err = state
            .set_binding(WindowAction::Center, Some(hk), &[WindowAction::RightHalf])
            .unwrap_err();
        assert_eq!(
            err.kind,
            crate::settings_error::SettingsErrorKind::ShortcutTaken
        );
        assert_eq!(state.config(), before);

        let config = state
            .set_binding(WindowAction::Center, Some(hk), &[WindowAction::LeftHalf])
            .unwrap();
        assert_eq!(config.binding(WindowAction::Center), Some(hk));
        assert_eq!(config.binding(WindowAction::LeftHalf), None);
        assert_eq!(applies.load(Ordering::Relaxed), 1);

        // Clearing, or rebinding a chord to its own action, is never a conflict.
        state
            .set_binding(WindowAction::Center, Some(hk), &[])
            .unwrap();
        state.set_binding(WindowAction::Center, None, &[]).unwrap();
        let _ = std::fs::remove_dir_all(&dir);
    }

    fn reset_test_dir(tag: u32) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("tile-reset-{}-{tag}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    /// The user's settings as they stood before a reset in these tests.
    fn customised() -> Config {
        let mut config = Config::default();
        config.set_binding(WindowAction::LeftHalf, None);
        config.gaps.window = 12.0;
        config.launch_on_login = false;
        config.animation.enabled = false;
        config
    }

    fn default_login() -> bool {
        Config::default().launch_on_login
    }

    /// Undo after a reset brings the previous settings back — on disk and
    /// with their hotkeys re-applied — and neither step rewinds the
    /// orientation, which is a fact about the installation, not a setting.
    #[test]
    fn undoing_a_reset_restores_the_previous_settings() {
        let dir = reset_test_dir(line!());
        let (state, applies) = state_counting_applies(&dir, true);
        state
            .update_config(|config| *config = customised())
            .unwrap();
        assert!(state.take_orientation());

        let reset = state.reset_to_defaults(default_login()).unwrap();
        assert_eq!(reset.gaps.window, Config::default().gaps.window);
        assert!(
            reset.orientation_shown,
            "a reset must not owe the orientation again"
        );
        assert_eq!(state.reset_undo_launch_on_login(), Some(false));

        let applied_before = applies.load(Ordering::Relaxed);
        let restored = state
            .undo_reset_to_defaults(None)
            .unwrap()
            .expect("nothing changed since the reset");

        assert_eq!(restored.binding(WindowAction::LeftHalf), None);
        assert_eq!(restored.gaps.window, 12.0);
        assert!(!restored.launch_on_login);
        assert!(!restored.animation.enabled);
        assert!(
            restored.orientation_shown,
            "undo must not owe the orientation again"
        );
        assert_eq!(state.config(), restored);
        assert_eq!(
            applies.load(Ordering::Relaxed),
            applied_before + 1,
            "the restored shortcuts must be re-applied"
        );
        assert_eq!(crate::config_store::load_from_dir(&dir).config, restored);
        assert!(
            state.undo_reset_to_defaults(None).unwrap().is_none(),
            "an undo is used up once taken"
        );

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Settings ▸ Advanced saves through the same path as every other
    /// control, so its values persist, reset to defaults, and come back on
    /// undo.
    #[test]
    fn advanced_settings_persist_reset_and_undo() {
        use tile_core::AdvancedSetting;

        let dir = reset_test_dir(line!());
        let state = state_with_orientation(&dir, false);
        state
            .update_config(|config| config.set_advanced(AdvancedSetting::MoveStep(32.0)))
            .unwrap();
        let saved = state
            .update_config(|config| {
                config.set_advanced(AdvancedSetting::AlmostMaximizeWidth(1.5));
                config.set_advanced(AdvancedSetting::AnimationFps(60));
            })
            .unwrap();
        assert_eq!(saved.move_step, 32.0);
        assert_eq!(saved.almost_maximize_width, 1.0, "clamped, not defaulted");
        assert_eq!(saved.animation.fps, 60);
        assert_eq!(crate::config_store::load_from_dir(&dir).config, saved);

        let defaults = Config::default();
        let reset = state.reset_to_defaults(default_login()).unwrap();
        assert_eq!(reset.move_step, defaults.move_step);
        assert_eq!(reset.almost_maximize_width, defaults.almost_maximize_width);
        assert_eq!(reset.animation.fps, defaults.animation.fps);

        let restored = state
            .undo_reset_to_defaults(None)
            .unwrap()
            .expect("nothing changed since the reset");
        assert_eq!(restored.move_step, 32.0);
        assert_eq!(restored.almost_maximize_width, 1.0);
        assert_eq!(restored.animation.fps, 60);

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Any change after a reset — from the settings window or the welcome
    /// window — ends the undo, so it can never overwrite a newer choice.
    #[test]
    fn a_change_after_a_reset_ends_the_undo() {
        let dir = reset_test_dir(line!());
        let state = state_with_orientation(&dir, false);
        state
            .update_config(|config| *config = customised())
            .unwrap();

        state.reset_to_defaults(default_login()).unwrap();
        let newer = state
            .update_config(|config| config.launch_on_login = false)
            .unwrap();

        assert_eq!(state.reset_undo_launch_on_login(), None);
        assert!(state.undo_reset_to_defaults(None).unwrap().is_none());
        assert_eq!(state.config(), newer, "the newer change stands");

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Resetting twice in a row keeps the settings from before the first
    /// reset, rather than offering to "undo" back to the defaults.
    #[test]
    fn a_repeated_reset_keeps_the_first_snapshot() {
        let dir = reset_test_dir(line!());
        let state = state_with_orientation(&dir, false);
        state
            .update_config(|config| *config = customised())
            .unwrap();

        state.reset_to_defaults(default_login()).unwrap();
        state.reset_to_defaults(default_login()).unwrap();
        let restored = state
            .undo_reset_to_defaults(None)
            .unwrap()
            .expect("undo is available");

        assert_eq!(restored.gaps.window, 12.0);
        assert!(!restored.launch_on_login);

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// When the OS refused to move the login item, the caller keeps the
    /// current value rather than the one being restored.
    #[test]
    fn an_undo_can_keep_the_current_login_item() {
        let dir = reset_test_dir(line!());
        let state = state_with_orientation(&dir, false);
        state
            .update_config(|config| *config = customised())
            .unwrap();
        state.reset_to_defaults(true).unwrap();

        let restored = state.undo_reset_to_defaults(Some(true)).unwrap().unwrap();

        assert!(restored.launch_on_login);
        assert_eq!(restored.gaps.window, 12.0);

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A change that fails to save is rolled back entirely, so it does not
    /// cost the user their undo either.
    #[test]
    fn a_failed_save_keeps_the_undo() {
        let dir = reset_test_dir(line!());
        let blocked = dir.join("not-a-directory");
        std::fs::write(&blocked, b"").unwrap();
        let state = state_with_orientation(&blocked, false);
        // Seed a snapshot directly: with saves failing, no reset can commit.
        *lock(&state.reset_undo) = Some(customised());

        assert!(state
            .update_config(|config| config.gaps.window = 3.0)
            .is_err());
        assert_eq!(state.reset_undo_launch_on_login(), Some(false));

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A fake OS login item: records every value it is asked to set, and
    /// refuses the calls whose (zero-based) index is in `refuse`.
    fn fake_login_item<'a>(
        calls: &'a RefCell<Vec<bool>>,
        refuse: &'a [usize],
    ) -> impl FnMut(bool) -> Result<(), SettingsError> + 'a {
        move |enabled| {
            let index = calls.borrow().len();
            calls.borrow_mut().push(enabled);
            if refuse.contains(&index) {
                Err(SettingsError::login_item("refused"))
            } else {
                Ok(())
            }
        }
    }

    /// The login item moves before the preference is saved, and both land.
    #[test]
    fn launch_on_login_moves_the_login_item_then_saves() {
        let dir = reset_test_dir(line!());
        let state = state_with_orientation(&dir, false);
        let target = !state.config().launch_on_login;
        let calls = RefCell::new(Vec::new());

        let txn = state.settings_transaction();
        let before = txn.revision();
        let config = txn
            .set_launch_on_login(target, &mut fake_login_item(&calls, &[]))
            .unwrap();

        assert_eq!(*calls.borrow(), vec![target]);
        assert_eq!(config.launch_on_login, target);
        assert_eq!(txn.revision(), before + 1);
        drop(txn);
        assert_eq!(
            crate::config_store::load_from_dir(&dir)
                .config
                .launch_on_login,
            target
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A login item the OS refused is never recorded as the preference.
    #[test]
    fn a_refused_login_item_saves_nothing() {
        let dir = reset_test_dir(line!());
        let state = state_with_orientation(&dir, false);
        let before_config = state.config();
        let calls = RefCell::new(Vec::new());

        let txn = state.settings_transaction();
        let before = txn.revision();
        let err = txn
            .set_launch_on_login(
                !before_config.launch_on_login,
                &mut fake_login_item(&calls, &[0]),
            )
            .unwrap_err();

        assert_eq!(
            err.kind,
            crate::settings_error::SettingsErrorKind::LoginItem
        );
        assert_eq!(txn.revision(), before, "nothing was committed");
        assert_eq!(txn.config(), before_config);
        drop(txn);
        assert!(!crate::config_store::config_file_path(&dir).exists());
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A save that fails after the login item moved puts the login item back,
    /// and reports the save failure.
    #[test]
    fn a_failed_save_puts_the_login_item_back() {
        let dir = reset_test_dir(line!());
        let blocked = dir.join("not-a-directory");
        std::fs::write(&blocked, b"").unwrap();
        let state = state_with_orientation(&blocked, false);
        let previous = state.config().launch_on_login;
        let calls = RefCell::new(Vec::new());

        let txn = state.settings_transaction();
        let err = txn
            .set_launch_on_login(!previous, &mut fake_login_item(&calls, &[]))
            .unwrap_err();

        assert_eq!(err.kind, crate::settings_error::SettingsErrorKind::NotSaved);
        assert_eq!(*calls.borrow(), vec![!previous, previous]);
        assert_eq!(txn.config().launch_on_login, previous);
        drop(txn);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// When the login item cannot be put back either, the user must hear that
    /// the OS and the preference now disagree.
    #[test]
    fn a_failed_revert_reports_the_login_item_out_of_sync() {
        let dir = reset_test_dir(line!());
        let blocked = dir.join("not-a-directory");
        std::fs::write(&blocked, b"").unwrap();
        let state = state_with_orientation(&blocked, false);
        let previous = state.config().launch_on_login;
        let calls = RefCell::new(Vec::new());

        let err = state
            .settings_transaction()
            .set_launch_on_login(!previous, &mut fake_login_item(&calls, &[1]))
            .unwrap_err();

        assert_eq!(
            err.kind,
            crate::settings_error::SettingsErrorKind::OutOfSync
        );
        assert_eq!(*calls.borrow(), vec![!previous, previous]);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A restore whose login item the OS refused still restores everything
    /// else, keeps launch at login as it was, and counts as committed so the
    /// other windows are told.
    #[test]
    fn a_restore_with_a_refused_login_item_restores_the_rest() {
        let dir = reset_test_dir(line!());
        let state = state_with_orientation(&dir, false);
        state
            .update_config(|config| {
                *config = customised();
                config.launch_on_login = !default_login();
            })
            .unwrap();
        let calls = RefCell::new(Vec::new());

        let txn = state.settings_transaction();
        let before = txn.revision();
        let err = txn
            .restore_defaults(&mut fake_login_item(&calls, &[0]))
            .unwrap_err();

        assert_eq!(
            err.kind,
            crate::settings_error::SettingsErrorKind::LoginItem
        );
        assert_eq!(txn.revision(), before + 1, "the rest was committed");
        let config = txn.config();
        assert_eq!(config.gaps.window, Config::default().gaps.window);
        assert_eq!(config.launch_on_login, !default_login());
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Undo moves the login item back to the value it is restoring, inside the
    /// same transaction as the restore it undoes.
    #[test]
    fn undoing_a_restore_moves_the_login_item_back() {
        let dir = reset_test_dir(line!());
        let state = state_with_orientation(&dir, false);
        state
            .update_config(|config| {
                *config = customised();
                config.launch_on_login = !default_login();
            })
            .unwrap();
        let calls = RefCell::new(Vec::new());

        let txn = state.settings_transaction();
        txn.restore_defaults(&mut fake_login_item(&calls, &[]))
            .unwrap();
        let restored = txn
            .undo_restore_defaults(&mut fake_login_item(&calls, &[]))
            .unwrap()
            .expect("nothing changed since the restore");

        assert_eq!(*calls.borrow(), vec![default_login(), !default_login()]);
        assert_eq!(restored.launch_on_login, !default_login());
        assert_eq!(restored.gaps.window, 12.0);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A second window's write waits for the first window's whole
    /// transaction, so it can neither slip in between a read and the save
    /// built on it nor be overwritten by it.
    #[test]
    fn a_write_from_another_window_waits_for_the_transaction() {
        let dir = reset_test_dir(line!());
        let state = Arc::new(state_with_orientation(&dir, false));
        let finished = Arc::new(AtomicBool::new(false));

        let txn = state.settings_transaction();
        let other = {
            let state = Arc::clone(&state);
            let finished = Arc::clone(&finished);
            std::thread::spawn(move || {
                state
                    .update_config(|config| config.gaps.window = 7.0)
                    .unwrap();
                finished.store(true, Ordering::SeqCst);
            })
        };
        std::thread::sleep(Duration::from_millis(100));
        assert!(
            !finished.load(Ordering::SeqCst),
            "the other write must wait for the open transaction"
        );
        txn.update_config(|config| config.gaps.window = 3.0)
            .unwrap();
        assert_eq!(txn.config().gaps.window, 3.0);
        drop(txn);

        other.join().unwrap();
        assert!(finished.load(Ordering::SeqCst));
        assert_eq!(state.config().gaps.window, 7.0, "the later write wins");
        assert_eq!(
            crate::config_store::load_from_dir(&dir).config.gaps.window,
            7.0
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Recording the orientation commits like any other write, but is not a
    /// settings change, so it must not cost the user a pending undo.
    #[test]
    fn claiming_the_orientation_keeps_the_undo() {
        let dir = reset_test_dir(line!());
        let state = state_with_orientation(&dir, true);
        state
            .update_config(|config| *config = customised())
            .unwrap();
        state.reset_to_defaults(default_login()).unwrap();

        let txn = state.settings_transaction();
        let before = txn.revision();
        assert!(txn.take_orientation());
        assert_eq!(txn.revision(), before + 1);
        drop(txn);

        assert_eq!(state.reset_undo_launch_on_login(), Some(false));
        let _ = std::fs::remove_dir_all(&dir);
    }

    struct SequencedHotkeyBackend {
        results: VecDeque<tile_platform::Result<HotkeyApplyReport>>,
    }

    impl HotkeyBackend for SequencedHotkeyBackend {
        fn apply(
            &mut self,
            _bindings: &[HotkeyBinding],
        ) -> tile_platform::Result<HotkeyApplyReport> {
            self.results
                .pop_front()
                .expect("unexpected extra hotkey apply")
        }

        fn shutdown(&mut self) {}
    }

    #[test]
    fn failed_reapply_preserves_the_last_confirmed_hotkey_report() {
        let binding = HotkeyBinding {
            hotkey: Hotkey::new(Modifiers::META, KeyCode::Left),
            action: WindowAction::LeftHalf,
            repeat: false,
        };
        let report = HotkeyApplyReport {
            bindings: vec![tile_platform::HotkeyBindingStatus {
                binding,
                route: tile_platform::HotkeyRoute::Registered,
                reason: None,
            }],
            hook_installed: false,
            hook_unavailable: false,
            warning: None,
            revision: 1,
        };
        let state = AppState::new(
            Box::new(InertWindowBackend),
            Box::new(SequencedHotkeyBackend {
                results: VecDeque::from([
                    Ok(report.clone()),
                    Err(PlatformError::os("hotkey apply", "simulated failure")),
                    Err(PlatformError::HotkeyStateUnknown(
                        "simulated uncertain ownership".into(),
                    )),
                ]),
            }),
            Config::default(),
            BuildKind::Development,
            None,
            false,
        );

        let first = state.apply_hotkeys();
        assert_eq!(first.report, Some(report.clone()));
        assert_eq!(first.apply_error, None);

        let second = state.apply_hotkeys();
        assert_eq!(second.report, Some(report));
        assert!(second
            .apply_error
            .as_deref()
            .is_some_and(|error| error.contains("simulated failure")));

        let permission_error = PlatformError::PermissionDenied("elevated target".into());
        assert!(is_permission_denied(&permission_error));
        assert_eq!(state.hotkey_status(), second);

        let unknown = state.apply_hotkeys();
        assert_eq!(unknown.report, None);
        assert!(unknown
            .apply_error
            .as_deref()
            .is_some_and(|error| error.contains("uncertain ownership")));
    }

    fn report(revision: u64, hook_unavailable: bool) -> HotkeyApplyReport {
        HotkeyApplyReport {
            bindings: Vec::new(),
            hook_installed: true,
            hook_unavailable,
            warning: None,
            revision,
        }
    }

    fn sequenced_state(results: Vec<tile_platform::Result<HotkeyApplyReport>>) -> AppState {
        AppState::new(
            Box::new(InertWindowBackend),
            Box::new(SequencedHotkeyBackend {
                results: results.into(),
            }),
            Config::default(),
            BuildKind::Development,
            None,
            false,
        )
    }

    #[test]
    fn published_hotkey_status_replaces_an_older_report_and_notifies() {
        let state = sequenced_state(vec![Ok(report(1, false))]);
        let notified = std::sync::Arc::new(AtomicUsize::new(0));
        let counter = std::sync::Arc::clone(&notified);
        state.set_hotkey_status_notifier(Box::new(move || {
            counter.fetch_add(1, Ordering::SeqCst);
        }));

        state.apply_hotkeys();
        assert_eq!(notified.load(Ordering::SeqCst), 1);

        assert!(state.record_published_hotkeys(report(2, true)));
        assert_eq!(state.hotkey_status().report, Some(report(2, true)));

        assert!(state.record_published_hotkeys(report(3, false)));
        assert_eq!(state.hotkey_status().report, Some(report(3, false)));
    }

    #[test]
    fn a_stale_report_never_replaces_a_newer_one() {
        // Recovery published revision 3 before the reply to the apply that
        // produced revision 2 was recorded.
        let state = sequenced_state(vec![Ok(report(2, false))]);
        assert!(state.record_published_hotkeys(report(3, true)));

        let applied = state.apply_hotkeys();
        assert_eq!(applied.report, Some(report(3, true)));
        assert_eq!(applied.apply_error, None);

        assert!(!state.record_published_hotkeys(report(1, false)));
        assert_eq!(state.hotkey_status().report, Some(report(3, true)));
    }
}
