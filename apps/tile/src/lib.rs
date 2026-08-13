//! Tile application shell: tray-only Tauri app that turns global hotkeys and
//! tray clicks into window moves via `tile_core::Engine` and the
//! `tile_platform` backends.
//!
//! See [`state`] for the threading model.

mod commands;
mod config_store;
mod dto;
mod feedback;
mod ratelimit;
mod state;
mod tray;
mod window;

use std::sync::mpsc;
use std::sync::Arc;
use std::thread;
use std::time::Duration;

use tauri::{AppHandle, Manager, RunEvent, Runtime};
use tauri_plugin_autostart::{MacosLauncher, ManagerExt};
use tile_core::{Config, WindowAction};
use tile_platform::PermissionStatus;

use state::AppState;

/// How often the startup permission poll re-checks while access is denied.
const PERMISSION_POLL_INTERVAL: Duration = Duration::from_secs(2);

/// Runs the Tile application. Blocks until the app exits.
pub fn run() {
    // Logging must never take down the app; ignore a double-init.
    let _ = env_logger::try_init();

    // The only channel actions leave the hotkey backend on. The worker thread
    // owns the receiver; the sender is handed to the backend.
    let (tx, rx) = mpsc::channel::<WindowAction>();
    let mut rx = Some(rx);

    let context = tauri::generate_context!();
    let build = tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_autostart::init(
            MacosLauncher::LaunchAgent,
            None,
        ))
        .invoke_handler(tauri::generate_handler![
            commands::get_config,
            commands::set_binding,
            commands::set_gaps,
            commands::set_launch_on_login,
            commands::reset_to_defaults,
            commands::perform_action,
            commands::get_permission_status,
            commands::get_hotkey_failures,
        ])
        .setup(move |app| {
            let handle = app.handle().clone();
            let rx = match rx.take() {
                Some(rx) => rx,
                None => return Err("setup invoked more than once".into()),
            };
            setup_app(&handle, tx.clone(), rx)?;
            Ok(())
        })
        .build(context);

    match build {
        Ok(app) => app.run(|app, event| match event {
            // Closing the settings window must not quit the app — Tile lives in
            // the tray. Tauri reports that case with `code: None`, whereas an
            // explicit `app.exit(code)` (the tray's Quit item) arrives with
            // `Some(code)`. Preventing *every* exit request, rather than only
            // the window-driven one, is what previously made Quit a no-op.
            RunEvent::ExitRequested { code, api, .. } => {
                if code.is_none() {
                    api.prevent_exit();
                }
            }
            // Release the keyboard hook however the app is being torn down, not
            // just via the tray, so the hook never outlives the process.
            RunEvent::Exit => {
                if let Some(state) = app.try_state::<Arc<AppState>>() {
                    state.shutdown_hotkeys();
                }
            }
            _ => {}
        }),
        Err(err) => log::error!("failed to start Tile: {err}"),
    }
}

/// Constructs the backends, loads config, wires the tray and the worker thread,
/// and kicks off permission handling. Runs on the main thread inside Tauri's
/// `setup`, which is where the macOS hotkey backend must be created.
fn setup_app<R: Runtime>(
    app: &AppHandle<R>,
    tx: mpsc::Sender<WindowAction>,
    rx: mpsc::Receiver<WindowAction>,
) -> Result<(), Box<dyn std::error::Error>> {
    let window_backend = tile_platform::window_backend()?;
    let hotkey_backend = tile_platform::hotkey_backend(tx)?;

    let config_dir = config_store::resolve_config_dir();
    let config = match &config_dir {
        Some(dir) => config_store::load_from_dir(dir),
        None => {
            log::warn!("could not resolve a config directory; using defaults in memory only");
            Config::default()
        }
    };
    let launch_on_login = config.launch_on_login;

    let state = Arc::new(AppState::new(
        window_backend,
        hotkey_backend,
        config,
        config_dir,
    ));
    app.manage(state.clone());

    // macOS: run as an accessory (no Dock icon), matching the tray-only design.
    #[cfg(target_os = "macos")]
    if let Err(err) = app.set_activation_policy(tauri::ActivationPolicy::Accessory) {
        log::error!("failed to set macOS accessory activation policy: {err}");
    }

    tray::build_tray(app)?;

    // Worker thread: drains hotkey presses and performs them. It only touches
    // the window backend (safe off the main thread); hotkey registration stays
    // with the backend's own loop.
    let worker_handle = app.clone();
    thread::Builder::new()
        .name("tile-action-worker".into())
        .spawn(move || {
            while let Ok(action) = rx.recv() {
                feedback::run_action(&worker_handle, action);
            }
            log::debug!("action worker thread exiting");
        })?;

    sync_autostart_on_launch(app, launch_on_login);
    begin_permission_flow(app, state);

    Ok(())
}

/// Aligns the OS login item with the persisted preference at startup.
fn sync_autostart_on_launch<R: Runtime>(app: &AppHandle<R>, desired: bool) {
    let manager = app.autolaunch();
    let is_enabled = manager.is_enabled().unwrap_or(false);
    if is_enabled == desired {
        return;
    }
    let result = if desired {
        manager.enable()
    } else {
        manager.disable()
    };
    if let Err(err) = result {
        log::error!("failed to reconcile launch-on-login at startup: {err}");
    }
}

/// Checks permission and applies hotkeys, or waits for the user to grant
/// Accessibility on macOS before applying.
fn begin_permission_flow<R: Runtime>(app: &AppHandle<R>, state: Arc<AppState>) {
    match state.permission_status(false) {
        Ok(PermissionStatus::Granted) | Ok(PermissionStatus::NotRequired) => {
            state.apply_hotkeys();
        }
        Ok(PermissionStatus::Denied) => {
            log::info!("accessibility permission denied; opening settings and polling");
            if let Err(err) = window::open_settings(app) {
                log::error!("failed to open settings window: {err}");
            }
            poll_until_granted(state);
        }
        Err(err) => {
            log::error!("could not read permission status: {err}; applying hotkeys anyway");
            state.apply_hotkeys();
        }
    }
}

/// Background poll: applies hotkeys as soon as permission is granted. Only
/// calls the non-prompting `permission_status(false)`, so it is safe off the
/// main thread.
fn poll_until_granted(state: Arc<AppState>) {
    thread::Builder::new()
        .name("tile-permission-poll".into())
        .spawn(move || loop {
            thread::sleep(PERMISSION_POLL_INTERVAL);
            match state.permission_status(false) {
                Ok(PermissionStatus::Granted) | Ok(PermissionStatus::NotRequired) => {
                    log::info!("accessibility permission granted; applying hotkeys");
                    state.apply_hotkeys();
                    break;
                }
                Ok(PermissionStatus::Denied) => continue,
                Err(err) => {
                    log::error!("permission poll failed: {err}");
                    break;
                }
            }
        })
        .map(|_| ())
        .unwrap_or_else(|err| log::error!("failed to spawn permission poll thread: {err}"));
}
