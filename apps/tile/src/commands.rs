//! The Tauri command surface exposed to the settings UI.
//!
//! Commands are intentionally thin: every real decision lives in
//! `tile_core::Engine` or on [`AppState`]. Mutating commands return the updated
//! [`Config`] so the UI always re-renders from the persisted truth rather than
//! guessing (e.g. [`set_binding`] may unbind a conflicting action). When a
//! change cannot be saved or applied they return a [`SettingsError`] instead,
//! having left the running app on its previous settings.

use std::sync::Arc;

use tauri::{AppHandle, Runtime, State};
use tile_core::{Config, CycleSize, Gaps, Hotkey, SubsequentExecutionMode, WindowAction};

use crate::autostart;
use crate::dto::{
    BuildInfoDto, ConfigRecoveryDto, HotkeyBindingStatusDto, HotkeyStatusDto, PermissionStatusDto,
    UpdateStatusDto, WelcomeStatusDto,
};
use crate::settings_error::SettingsError;
use crate::state::AppState;
use crate::update::UpdateManager;

type Shared = Arc<AppState>;

/// Sets the OS login item. A development build succeeds without touching it —
/// see [`crate::autostart`].
fn sync_autostart<R: Runtime>(
    app: &AppHandle<R>,
    state: &AppState,
    enabled: bool,
) -> Result<(), SettingsError> {
    autostart::apply(app, state.build_kind(), enabled).map_err(SettingsError::login_item)
}

/// Puts the login item back after the preference that asked for the change
/// could not be saved, and returns the error to report. The save failure
/// stands only if the revert worked; otherwise the OS and the preference now
/// disagree, which is a login-item problem the user has to hear about.
fn revert_autostart<R: Runtime>(
    app: &AppHandle<R>,
    state: &AppState,
    enabled: bool,
    save_error: SettingsError,
) -> SettingsError {
    match sync_autostart(app, state, enabled) {
        Ok(()) => save_error,
        Err(revert_error) => SettingsError::login_item(format!(
            "{save_error}; putting the login item back also failed: {revert_error}"
        )),
    }
}

#[tauri::command]
pub fn get_config(state: State<'_, Shared>) -> Config {
    state.config()
}

/// Which kind of build this is, and where it keeps its config. Read once by
/// the UI at boot: it is fixed for the lifetime of the process.
#[tauri::command]
pub fn get_build_info(state: State<'_, Shared>) -> BuildInfoDto {
    BuildInfoDto {
        kind: state.build_kind().into(),
        config_dir: state.config_dir().map(|dir| dir.display().to_string()),
    }
}

/// The notice owed to the user when this launch could not read their saved
/// settings, or `None` once dismissed (or when nothing went wrong).
#[tauri::command]
pub fn get_config_recovery(state: State<'_, Shared>) -> Option<ConfigRecoveryDto> {
    state
        .config_recovery()
        .as_ref()
        .map(ConfigRecoveryDto::from)
}

#[tauri::command]
pub fn dismiss_config_recovery(state: State<'_, Shared>) {
    state.dismiss_config_recovery();
}

/// Shows the kept copy of an unreadable config in the file manager, or the
/// settings folder when it could not be kept. The path comes from app state,
/// never from the UI.
#[tauri::command]
pub fn reveal_config_backup<R: Runtime>(
    app: AppHandle<R>,
    state: State<'_, Shared>,
) -> Result<(), String> {
    use tauri_plugin_opener::OpenerExt;

    let backup = state
        .config_recovery()
        .and_then(|recovery| recovery.backup_path);
    let result = match (backup, state.config_dir()) {
        (Some(path), _) => app.opener().reveal_item_in_dir(path),
        (None, Some(dir)) => app
            .opener()
            .open_path(dir.display().to_string(), None::<&str>),
        (None, None) => return Err("Tile has no settings folder on this machine.".into()),
    };
    result.map_err(|err| err.to_string())
}

#[tauri::command]
pub fn set_binding<R: Runtime>(
    app: AppHandle<R>,
    state: State<'_, Shared>,
    action: WindowAction,
    hotkey: Option<Hotkey>,
) -> Result<Config, SettingsError> {
    let config = state.update_config(|config| config.set_binding(action, hotkey))?;
    crate::tray::sync_bindings(&app);
    Ok(config)
}

#[tauri::command]
pub fn set_gaps(state: State<'_, Shared>, gaps: Gaps) -> Result<Config, SettingsError> {
    state.update_config(|config| config.gaps = gaps)
}

/// Sets what a repeated press of an already-satisfied shortcut does, and which
/// sizes it cycles through. The two travel together because a mode of
/// "cycle sizes" with no sizes selected is indistinguishable from "do nothing".
#[tauri::command]
pub fn set_cycling(
    state: State<'_, Shared>,
    mode: SubsequentExecutionMode,
    sizes: Vec<CycleSize>,
) -> Result<Config, SettingsError> {
    state.update_config(|config| {
        config.subsequent_execution_mode = mode;
        config.cycle_sizes = sizes;
    })
}

/// Turns the animated snap on or off.
///
/// The on/off choice matters most to someone who finds the motion distracting
/// or is working over a remote-desktop session. Duration has its own control
/// alongside it; only the frame-rate pacing knob stays in `config.json`.
#[tauri::command]
pub fn set_animation(state: State<'_, Shared>, enabled: bool) -> Result<Config, SettingsError> {
    state.update_config(|config| config.animation.enabled = enabled)
}

/// Sets how long a snap takes. `update_config` normalizes afterwards, so an
/// out-of-range value clamps exactly as a hand-edited one does.
#[tauri::command]
pub fn set_animation_duration(
    state: State<'_, Shared>,
    duration_ms: u32,
) -> Result<Config, SettingsError> {
    state.update_config(|config| config.animation.duration_ms = duration_ms)
}

/// Changes the OS login item first and only then records the preference, so
/// the checkbox never claims a login item the OS refused to create.
#[tauri::command]
pub fn set_launch_on_login<R: Runtime>(
    app: AppHandle<R>,
    state: State<'_, Shared>,
    enabled: bool,
) -> Result<Config, SettingsError> {
    let previous = state.config().launch_on_login;
    sync_autostart(&app, &state, enabled)?;
    state
        .update_config(|config| config.launch_on_login = enabled)
        .map_err(|err| revert_autostart(&app, &state, previous, err))
}

/// Claims the one-time first-run orientation. Returns `true` at most once per
/// installation, and records that fact before returning, so reopening the
/// welcome screen or relaunching never re-triggers a first run.
#[tauri::command]
pub fn take_orientation(state: State<'_, Shared>) -> bool {
    state.take_orientation()
}

/// Opens the settings window. Used by the welcome screen, which is a window of
/// its own and so cannot simply scroll the user to the controls.
#[tauri::command]
pub fn open_settings<R: Runtime>(
    app: AppHandle<R>,
    state: State<'_, Shared>,
) -> Result<(), String> {
    crate::window::open_settings(&app, state.build_kind()).map_err(|err| err.to_string())
}

/// Reopens the welcome screen on demand, from the settings footer.
///
/// This command must be asynchronous even though the window operation itself
/// is synchronous. On Windows, a synchronous IPC command runs inside WebView2's
/// message callback; building another WebView there waits on the event loop
/// that is still servicing that callback and deadlocks. Returning a future lets
/// the callback finish before the queued main-thread operation runs.
#[tauri::command]
pub async fn open_welcome<R: Runtime>(app: AppHandle<R>) -> Result<(), String> {
    let (sender, mut receiver) = tauri::async_runtime::channel(1);
    let handle = app.clone();

    app.run_on_main_thread(move || {
        let result = crate::window::open_welcome(&handle).map_err(|err| err.to_string());
        if result.is_ok() {
            if let Err(err) = crate::window::close_settings(&handle) {
                log::warn!("could not close the settings window: {err}");
            }
        }
        if sender.try_send(result).is_err() {
            log::error!("could not return the welcome window result to the settings window");
        }
    })
    .map_err(|err| err.to_string())?;

    receiver
        .recv()
        .await
        .ok_or_else(|| "the welcome window operation did not complete".to_string())?
}

/// Lets the welcome window take the keyboard for its closing slide.
///
/// The walkthrough is keyboard-driven from the first slide to the last, but the
/// first four slides are driven by *global* shortcuts, which do not need this
/// window focused — and must not have it, or Tile would be moving the very
/// window the user is being taught with. The closing slide is the one step
/// whose key is an ordinary keystroke, so it is the one step that needs the
/// window to actually be listening.
#[tauri::command]
pub fn focus_welcome<R: Runtime>(app: AppHandle<R>) -> Result<(), String> {
    crate::window::focus_welcome(&app).map_err(|err| err.to_string())
}

/// Closes the welcome window, returning Tile to the menu bar first.
///
/// The closing slide promotes Tile to a regular app so the window can hold the
/// keyboard; this hands that back. It is a command rather than a window event
/// handler because registering `on_window_event` for this window — on the
/// handle or on the builder — stops it from ever appearing, so the close has to
/// be the thing that announces itself.
#[tauri::command]
pub fn close_welcome<R: Runtime>(app: AppHandle<R>) -> Result<(), String> {
    crate::window::close_welcome(&app).map_err(|err| err.to_string())
}

/// Restores every default. If the login item cannot be changed, everything
/// else is still restored and launch-at-login keeps its current value, so the
/// preference keeps matching the OS; the error says so.
#[tauri::command]
pub fn reset_to_defaults<R: Runtime>(
    app: AppHandle<R>,
    state: State<'_, Shared>,
) -> Result<Config, SettingsError> {
    let previous_login = state.config().launch_on_login;
    let login = sync_autostart(&app, &state, Config::default().launch_on_login);
    let kept_login = login.is_err().then_some(previous_login);
    state
        .update_config(|config| {
            *config = Config::default();
            if let Some(launch_on_login) = kept_login {
                config.launch_on_login = launch_on_login;
            }
        })
        .map_err(|err| {
            if login.is_ok() {
                revert_autostart(&app, &state, previous_login, err)
            } else {
                err
            }
        })?;
    crate::tray::sync_bindings(&app);
    login.map(|()| state.config())
}

#[tauri::command]
pub fn perform_action(state: State<'_, Shared>, action: WindowAction) -> Result<(), String> {
    state
        .perform_action(action)
        .map(|_| ())
        .map_err(|err| err.to_string())
}

/// Reports what the welcome walkthrough can honestly ask for on this machine.
#[tauri::command]
pub fn get_welcome_status(state: State<'_, Shared>) -> Result<WelcomeStatusDto, String> {
    let screen_count = state.screen_count().map_err(|err| err.to_string())?;
    let has_movable_window = state.has_movable_window().map_err(|err| err.to_string())?;
    let current_screen = state
        .current_screen_index()
        .map_err(|err| err.to_string())?;
    Ok(WelcomeStatusDto {
        screen_count,
        has_movable_window,
        current_screen,
        display_neighbors: state.display_neighbors().map_err(|err| err.to_string())?,
    })
}

#[tauri::command]
pub fn get_permission_status(
    state: State<'_, Shared>,
    prompt: bool,
) -> Result<PermissionStatusDto, String> {
    state
        .permission_status(prompt)
        .map(PermissionStatusDto::from)
        .map_err(|err| err.to_string())
}

#[tauri::command]
pub fn get_hotkey_status(state: State<'_, Shared>) -> HotkeyStatusDto {
    let status = state.hotkey_status();
    HotkeyStatusDto {
        bindings: status
            .report
            .as_ref()
            .map(|report| {
                report
                    .bindings
                    .iter()
                    .map(HotkeyBindingStatusDto::from)
                    .collect()
            })
            .unwrap_or_default(),
        hook_installed: status
            .report
            .as_ref()
            .is_some_and(|report| report.hook_installed),
        apply_error: status.apply_error,
    }
}

#[tauri::command]
pub fn get_update_status(manager: State<'_, Arc<UpdateManager>>) -> UpdateStatusDto {
    manager.status().into()
}

/// Opens the dedicated update window, optionally starting a check on arrival.
/// Used by the About window, whose only update affordance is to hand the user
/// over to the screen that can actually install one.
#[tauri::command]
pub fn open_update_window<R: Runtime>(
    app: AppHandle<R>,
    check_for_updates: bool,
) -> Result<(), String> {
    crate::window::open_updates(&app, check_for_updates).map_err(|err| err.to_string())
}

#[tauri::command]
pub async fn check_for_updates<R: Runtime>(
    app: AppHandle<R>,
    manager: State<'_, Arc<UpdateManager>>,
) -> Result<UpdateStatusDto, String> {
    let manager = manager.inner().clone();
    manager.check(&app).await.map(UpdateStatusDto::from)
}

#[tauri::command]
pub async fn install_update<R: Runtime>(
    app: AppHandle<R>,
    manager: State<'_, Arc<UpdateManager>>,
    relaunch_after_install: bool,
) -> Result<UpdateStatusDto, String> {
    let manager = manager.inner().clone();
    manager
        .install(&app, relaunch_after_install)
        .await
        .map(UpdateStatusDto::from)
}
