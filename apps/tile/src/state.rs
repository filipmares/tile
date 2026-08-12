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

use std::path::PathBuf;
use std::sync::{Mutex, MutexGuard};
use std::time::Duration;

use tile_core::{Config, Engine, Plan, WindowAction};
use tile_platform::{HotkeyBackend, HotkeyFailure, PermissionStatus, PlatformError, WindowBackend};

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
    pub fn perform_action(&self, action: WindowAction) -> tile_platform::Result<()> {
        let backend = lock(&self.backend);
        let mut engine = lock(&self.engine);

        let window = match backend.focused_window()? {
            Some(window) => window,
            None => {
                log::debug!("ignoring {action}: no movable focused window");
                return Ok(());
            }
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
