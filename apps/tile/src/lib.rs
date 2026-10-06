//! Tile application shell: tray-only Tauri app that turns global hotkeys and
//! tray clicks into window moves via `tile_core::Engine` and the
//! `tile_platform` backends.
//!
//! See [`state`] for the threading model.

mod animate;
mod autostart;
mod build_kind;
mod commands;
mod config_store;
mod dto;
mod feedback;
mod logging;
mod ratelimit;
mod settings_error;
mod state;
mod tray;
mod update;
mod window;

use std::sync::mpsc;
use std::sync::Arc;
use std::thread;
use std::time::Duration;

use tauri::{AppHandle, Manager, RunEvent, Runtime};
use tauri_plugin_autostart::MacosLauncher;
use tile_core::Config;
use tile_platform::PermissionStatus;

use build_kind::BuildKind;
use state::{ActionRequest, AppState};
use update::UpdateManager;

/// How often the startup permission poll re-checks while access is denied.
const PERMISSION_POLL_INTERVAL: Duration = Duration::from_secs(2);

/// Runs the Tile application. Blocks until the app exits.
pub fn run() {
    let build_kind = BuildKind::detect();
    logging::init(build_kind);
    log_launch(build_kind);

    let context = tauri::generate_context!();
    let mut builder = tauri::Builder::default();

    // Before every other plugin, which is what this one requires: it has to
    // claim the lock and hand off to the running copy before anything else
    // starts building state this process is about to throw away.
    if build_kind.enforces_single_instance() {
        builder = builder.plugin(tauri_plugin_single_instance::init(answer_second_launch));
    }

    let build = builder
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_updater::Builder::new().build())
        .plugin(tauri_plugin_autostart::init(
            MacosLauncher::LaunchAgent,
            Some(vec![autostart::AUTOSTART_ARG]),
        ))
        .invoke_handler(tauri::generate_handler![
            commands::get_config,
            commands::get_build_info,
            commands::get_config_recovery,
            commands::dismiss_config_recovery,
            commands::reveal_config_backup,
            commands::set_binding,
            commands::set_gaps,
            commands::set_cycling,
            commands::set_animation,
            commands::set_animation_duration,
            commands::set_launch_on_login,
            commands::reset_to_defaults,
            commands::take_orientation,
            commands::open_settings,
            commands::open_welcome,
            commands::focus_welcome,
            commands::close_welcome,
            commands::get_welcome_status,
            commands::perform_action,
            commands::get_permission_status,
            commands::get_hotkey_status,
            commands::get_update_status,
            commands::open_update_window,
            commands::check_for_updates,
            commands::install_update,
        ])
        .setup(move |app| {
            let handle = app.handle().clone();
            if let Err(err) = setup_app(&handle) {
                // `setup_app` claimed the session, and this process now exits
                // without entering the event loop, so `RunEvent::Exit` will
                // never clear the marker. This is a handled failure, not a crash.
                logging::end_session(&format!("setup failed: {err}"));
                return Err(err);
            }
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
                } else {
                    log::info!("exit requested with code {code:?}");
                }
            }
            // Release the keyboard hook however the app is being torn down, not
            // just via the tray, so the hook never outlives the process. This
            // also fires when Windows ends the session (sign-out, shutdown).
            RunEvent::Exit => {
                if let Some(state) = app.try_state::<Arc<AppState>>() {
                    state.shutdown_hotkeys();
                }
                logging::end_session("event loop finished");
            }
            _ => {}
        }),
        Err(err) => {
            // Setup failed, so Tile is about to exit without ever showing a
            // tray icon. At sign-in that is indistinguishable from "did not
            // start" unless it is written down — and shown.
            log::error!("failed to start Tile: {err}");
            log::logger().flush();
            show_startup_failure(logging::active_log_dir());
        }
    }
}

/// What the user reads when Tile cannot start. Plain words, and the one place
/// that can explain why.
fn startup_failure_message(log_dir: Option<&std::path::Path>) -> String {
    let mut message = String::from("Tile could not start.");
    match log_dir {
        Some(dir) => message.push_str(&format!(
            "\n\nThe reason is written in the log folder:\n{}",
            dir.display()
        )),
        None => message.push_str("\n\nTile could not create its log folder either."),
    }
    message
}

/// A native, blocking dialog. Tauri never started, so its dialog plugin is not
/// available; this runs on the main thread, which is what macOS requires.
#[cfg(any(windows, target_os = "macos"))]
fn show_startup_failure(log_dir: Option<&std::path::Path>) {
    rfd::MessageDialog::new()
        .set_title("Tile")
        .set_description(startup_failure_message(log_dir))
        .set_level(rfd::MessageLevel::Error)
        .set_buttons(rfd::MessageButtons::Ok)
        .show();
}

#[cfg(not(any(windows, target_os = "macos")))]
fn show_startup_failure(log_dir: Option<&std::path::Path>) {
    eprintln!("{}", startup_failure_message(log_dir));
}

/// The first lines of every log session: enough to tell from the log alone
/// which build ran, from where, and whether the OS login item launched it.
fn log_launch(build_kind: BuildKind) {
    let args: Vec<String> = std::env::args().collect();
    let exe = std::env::current_exe()
        .map(|path| path.display().to_string())
        .unwrap_or_else(|err| format!("<unknown: {err}>"));
    let source = if autostart::launched_by_login_item(args.iter().skip(1)) {
        "the OS login item"
    } else {
        "a manual launch, an installer, or an updater"
    };
    log::info!(
        "Tile {} starting (pid {}, {build_kind:?} build, {} {}) via {source}",
        env!("CARGO_PKG_VERSION"),
        std::process::id(),
        std::env::consts::OS,
        std::env::consts::ARCH,
    );
    log::info!(
        "executable: {exe}; arguments: {:?}",
        &args[1.min(args.len())..]
    );
    match logging::active_log_dir() {
        Some(dir) => log::info!("logging to {}", dir.display()),
        None => log::warn!("no log directory; logging to stderr only"),
    }
}

/// Answers a second launch of an already-running Tile, in the copy that was
/// there first. The newcomer exits on its own.
///
/// Tile has no Dock icon or taskbar button, so launching it again is exactly
/// what someone does when they cannot tell whether it is already running.
/// Exiting in silence would look like a failure to start, so the running copy
/// opens settings: the same window the tray offers, and proof of which process
/// owns the shortcuts.
fn answer_second_launch<R: Runtime>(app: &AppHandle<R>, argv: Vec<String>, _cwd: String) {
    if autostart::launched_by_login_item(argv.iter().skip(1)) {
        // The login item fired while Tile was already up (for example after a
        // fast sign-out/sign-in). Nothing to surface; the copy that is running
        // is the one the login item wanted.
        log::info!("the OS login item launched Tile again; it is already running");
        return;
    }
    log::info!("a second Tile was launched ({argv:?}); surfacing the copy that is already running");
    let Some(state) = app.try_state::<Arc<AppState>>() else {
        // Setup has not run yet, so there is no settings window to open and
        // nothing the newcomer needed that this copy is not about to do anyway.
        return;
    };
    if let Err(err) = window::open_settings(app, state.build_kind()) {
        log::error!("could not surface the running Tile for a second launch: {err}");
    }
}

/// Constructs the backends, loads config, wires the tray and the worker thread,
/// and kicks off permission handling. Runs on the main thread inside Tauri's
/// `setup`, which is where the macOS hotkey backend must be created.
fn setup_app<R: Runtime>(app: &AppHandle<R>) -> Result<(), Box<dyn std::error::Error>> {
    // Only the instance that owns the app gets here (a second launch has
    // already handed off), so this is the place to claim the session.
    logging::begin_session();

    // One channel feeds the worker thread, in arrival order: the hotkey
    // backend sends cycling requests on it and tray menu clicks send exact
    // ones. A single ingress is what keeps a hotkey and a menu click from
    // being reordered.
    let (requests, rx) = mpsc::channel::<ActionRequest>();

    let window_backend = tile_platform::window_backend()?;
    let hotkey_backend = tile_platform::hotkey_backend(requests.clone())?;

    // Everything that must differ between a checkout and an installed copy
    // hangs off this one value: which config directory is used, whether the OS
    // login item is touched, and how the app labels itself.
    let build_kind = BuildKind::detect();
    if build_kind.is_development() {
        log::info!(
            "development build: settings are stored separately and the OS login item is left alone"
        );
    }

    let config_dir = config_store::resolve_config_dir(build_kind);
    let loaded = match &config_dir {
        Some(dir) => config_store::load_from_dir(dir),
        None => {
            log::warn!("could not resolve a config directory; using defaults in memory only");
            // Without a config directory there is nowhere to record that
            // orientation was shown, so showing it would repeat on every
            // launch. Treat this as already-onboarded rather than nag.
            config_store::LoadedConfig {
                config: Config::default(),
                origin: config_store::ConfigOrigin::Corrupt,
                recovery: None,
            }
        }
    };
    let show_orientation = loaded.is_first_run() && !loaded.config.orientation_shown;
    let recovery = loaded.recovery;
    let config = loaded.config;
    let launch_on_login = config.launch_on_login;
    log::info!(
        "config loaded from {} (launch on login: {launch_on_login})",
        config_dir
            .as_deref()
            .map(|dir| dir.display().to_string())
            .unwrap_or_else(|| "<memory only>".into())
    );

    let recovered = recovery.is_some();
    let state = Arc::new(
        AppState::new(
            window_backend,
            hotkey_backend,
            config,
            build_kind,
            config_dir,
            show_orientation,
        )
        .with_config_recovery(recovery),
    );
    app.manage(state.clone());
    let updates = Arc::new(UpdateManager::new(build_kind));
    app.manage(updates.clone());

    // macOS: run as an accessory (no Dock icon), matching the tray-only design.
    #[cfg(target_os = "macos")]
    if let Err(err) = app.set_activation_policy(tauri::ActivationPolicy::Accessory) {
        log::error!("failed to set macOS accessory activation policy: {err}");
    }

    app.manage(tray::MenuActions::new(requests));
    tray::build_tray(app, build_kind)?;

    // Worker thread: drains hotkey presses and menu clicks and performs them.
    // It only touches the window backend (safe off the main thread); hotkey
    // registration stays with the backend's own loop.
    //
    // The receiver is also handed to the pipeline as a non-blocking poll, so a
    // press that arrives while the previous one is still animating steers that
    // animation instead of waiting for it. Draining the channel from inside
    // the action is safe precisely because this thread is the only consumer.
    //
    // Only hotkeys preempt. An exact menu request met mid-flight is held back
    // until the window lands, and nothing behind it is drained in the
    // meantime, so order is kept and the request is never downgraded into a
    // cycling one.
    let worker_handle = app.clone();
    thread::Builder::new()
        .name("tile-action-worker".into())
        .spawn(move || {
            let mut deferred: Option<ActionRequest> = None;
            loop {
                let request = match deferred.take() {
                    Some(request) => request,
                    None => match rx.recv() {
                        Ok(request) => request,
                        Err(_) => break,
                    },
                };
                feedback::run_action_preemptible(&worker_handle, request, &mut || {
                    if deferred.is_some() {
                        return None;
                    }
                    match rx.try_recv().ok()? {
                        ActionRequest {
                            action,
                            exact: false,
                        } => Some(action),
                        exact => {
                            deferred = Some(exact);
                            None
                        }
                    }
                });
            }
            log::debug!("action worker thread exiting");
        })?;

    autostart::reconcile_on_launch(app, build_kind, launch_on_login);
    // Starting from defaults silently would look like Tile forgot everything;
    // settings carries the notice that says what happened and where the old
    // file went.
    if recovered {
        log::info!("opening settings to explain that saved settings could not be read");
        if let Err(err) = window::open_settings(app, build_kind) {
            log::error!("failed to open settings window: {err}");
        }
    }
    begin_permission_flow(app, state);
    update::begin_update_checks(app.clone(), updates);

    log::info!("Tile is running");
    Ok(())
}

/// Checks permission and applies hotkeys, or waits for the user to grant
/// Accessibility on macOS before applying.
///
/// This is also where the one-time first-run welcome surfaces. A genuine first
/// run opens the welcome window for the one moment it is owed; nothing else
/// would open a window at all on Windows, or on macOS once permission is
/// granted. `state.orientation_pending()` is false on every later launch, so
/// this cannot become a recurring interruption.
///
/// With permission denied the welcome waits: settings opens for its permission
/// panel, and "hold this modifier and press an arrow" would be a lie until the
/// user grants access. The permission poll opens the welcome the moment it
/// stops being one.
fn begin_permission_flow<R: Runtime>(app: &AppHandle<R>, state: Arc<AppState>) {
    match state.permission_status(false) {
        Ok(PermissionStatus::Granted) | Ok(PermissionStatus::NotRequired) => {
            state.apply_hotkeys();
            open_welcome_for_first_run(app, &state);
        }
        Ok(PermissionStatus::Denied) => {
            log::info!("accessibility permission denied; opening settings and polling");
            if let Err(err) = window::open_settings(app, state.build_kind()) {
                log::error!("failed to open settings window: {err}");
            }
            poll_until_granted(app.clone(), state);
        }
        Err(err) => {
            log::error!("could not read permission status: {err}; applying hotkeys anyway");
            state.apply_hotkeys();
            open_welcome_for_first_run(app, &state);
        }
    }
}

/// Opens the welcome screen only when a first run is still owed. Does not claim
/// the orientation: the welcome UI does that, so a failure to open the window
/// leaves it owed for next time rather than losing it.
fn open_welcome_for_first_run<R: Runtime>(app: &AppHandle<R>, state: &AppState) {
    if !state.orientation_pending() {
        return;
    }
    log::info!("first run; opening the welcome screen to introduce the default shortcuts");
    if let Err(err) = window::open_welcome(app) {
        log::error!("failed to open welcome window for first run: {err}");
    }
}

/// Background poll: applies hotkeys as soon as permission is granted. Only
/// calls the non-prompting `permission_status(false)`, so it is safe off the
/// main thread.
fn poll_until_granted<R: Runtime>(app: AppHandle<R>, state: Arc<AppState>) {
    thread::Builder::new()
        .name("tile-permission-poll".into())
        .spawn(move || loop {
            thread::sleep(PERMISSION_POLL_INTERVAL);
            match state.permission_status(false) {
                Ok(PermissionStatus::Granted) | Ok(PermissionStatus::NotRequired) => {
                    log::info!("accessibility permission granted; applying hotkeys");
                    state.apply_hotkeys();
                    // The shortcuts the welcome describes only started working
                    // just now, so this is the first honest moment to show it.
                    let handle = app.clone();
                    let state = state.clone();
                    if let Err(err) = app.run_on_main_thread(move || {
                        open_welcome_for_first_run(&handle, &state);
                    }) {
                        log::error!("could not open the welcome window: {err}");
                    }
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_startup_failure_names_the_log_folder() {
        let message = startup_failure_message(Some(std::path::Path::new("C:/Tile/logs")));
        assert!(message.starts_with("Tile could not start."));
        assert!(message.contains("C:/Tile/logs"));
        assert!(startup_failure_message(None).contains("could not create its log folder"));
    }
}
