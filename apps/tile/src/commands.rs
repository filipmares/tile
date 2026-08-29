//! The Tauri command surface exposed to the settings UI.
//!
//! Commands are intentionally thin: every real decision lives in
//! `tile_core::Engine` or on [`AppState`]. Mutating commands return the updated
//! [`Config`] so the UI always re-renders from the persisted truth rather than
//! guessing (e.g. [`set_binding`] may unbind a conflicting action).

use std::sync::Arc;

use tauri::{AppHandle, Runtime, State};
use tile_core::{Config, CycleSize, Gaps, Hotkey, SubsequentExecutionMode, WindowAction};

use crate::autostart;
use crate::dto::{
    BuildInfoDto, HotkeyFailureDto, PermissionStatusDto, UpdateStatusDto, WelcomeStatusDto,
};
use crate::state::AppState;
use crate::update::UpdateManager;

type Shared = Arc<AppState>;

/// Keeps the OS login-item in sync with the desired state, logging on failure
/// rather than surfacing an error that would block saving the preference. A
/// development build persists the preference without touching the login item —
/// see [`crate::autostart`].
fn sync_autostart<R: Runtime>(app: &AppHandle<R>, state: &AppState, enabled: bool) {
    autostart::apply(app, state.build_kind(), enabled);
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

#[tauri::command]
pub fn set_binding(
    state: State<'_, Shared>,
    action: WindowAction,
    hotkey: Option<Hotkey>,
) -> Config {
    state.update_config(|config| config.set_binding(action, hotkey))
}

#[tauri::command]
pub fn set_gaps(state: State<'_, Shared>, gaps: Gaps) -> Config {
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
) -> Config {
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
pub fn set_animation(state: State<'_, Shared>, enabled: bool) -> Config {
    state.update_config(|config| config.animation.enabled = enabled)
}

/// Sets how long a snap takes. `update_config` normalizes afterwards, so an
/// out-of-range value clamps exactly as a hand-edited one does.
#[tauri::command]
pub fn set_animation_duration(state: State<'_, Shared>, duration_ms: u32) -> Config {
    state.update_config(|config| config.animation.duration_ms = duration_ms)
}

#[tauri::command]
pub fn set_launch_on_login<R: Runtime>(
    app: AppHandle<R>,
    state: State<'_, Shared>,
    enabled: bool,
) -> Config {
    let config = state.update_config(|config| config.launch_on_login = enabled);
    sync_autostart(&app, &state, config.launch_on_login);
    config
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
#[tauri::command]
pub fn open_welcome<R: Runtime>(app: AppHandle<R>) -> Result<(), String> {
    crate::window::open_welcome(&app).map_err(|err| err.to_string())
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

#[tauri::command]
pub fn reset_to_defaults<R: Runtime>(app: AppHandle<R>, state: State<'_, Shared>) -> Config {
    let config = state.update_config(|config| *config = Config::default());
    sync_autostart(&app, &state, config.launch_on_login);
    config
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
    Ok(WelcomeStatusDto {
        screen_count,
        has_movable_window,
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
pub fn get_hotkey_failures(state: State<'_, Shared>) -> Vec<HotkeyFailureDto> {
    state
        .hotkey_failures()
        .iter()
        .map(HotkeyFailureDto::from)
        .collect()
}

#[tauri::command]
pub fn get_update_status(manager: State<'_, Arc<UpdateManager>>) -> UpdateStatusDto {
    manager.status().into()
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
