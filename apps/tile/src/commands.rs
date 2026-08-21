//! The Tauri command surface exposed to the settings UI.
//!
//! Commands are intentionally thin: every real decision lives in
//! `tile_core::Engine` or on [`AppState`]. Mutating commands return the updated
//! [`Config`] so the UI always re-renders from the persisted truth rather than
//! guessing (e.g. [`set_binding`] may unbind a conflicting action).

use std::sync::Arc;

use tauri::{AppHandle, Runtime, State};
use tauri_plugin_autostart::ManagerExt;
use tile_core::{Config, CycleSize, Gaps, Hotkey, SubsequentExecutionMode, WindowAction};

use crate::dto::{HotkeyFailureDto, PermissionStatusDto};
use crate::state::AppState;

type Shared = Arc<AppState>;

/// Keeps the OS login-item in sync with the desired state, logging on failure
/// rather than surfacing an error that would block saving the preference.
fn sync_autostart<R: Runtime>(app: &AppHandle<R>, enabled: bool) {
    let manager = app.autolaunch();
    let result = if enabled {
        manager.enable()
    } else {
        manager.disable()
    };
    if let Err(err) = result {
        log::error!("failed to update launch-on-login to {enabled}: {err}");
    }
}

#[tauri::command]
pub fn get_config(state: State<'_, Shared>) -> Config {
    state.config()
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
/// Only the on/off choice is exposed: it is the one that matters to someone
/// who finds the motion distracting or is working over a remote-desktop
/// session. The timing knobs stay in `config.json`.
#[tauri::command]
pub fn set_animation(state: State<'_, Shared>, enabled: bool) -> Config {
    state.update_config(|config| config.animation.enabled = enabled)
}

#[tauri::command]
pub fn set_launch_on_login<R: Runtime>(
    app: AppHandle<R>,
    state: State<'_, Shared>,
    enabled: bool,
) -> Config {
    let config = state.update_config(|config| config.launch_on_login = enabled);
    sync_autostart(&app, config.launch_on_login);
    config
}

#[tauri::command]
pub fn reset_to_defaults<R: Runtime>(app: AppHandle<R>, state: State<'_, Shared>) -> Config {
    let config = state.update_config(|config| *config = Config::default());
    sync_autostart(&app, config.launch_on_login);
    config
}

#[tauri::command]
pub fn perform_action(state: State<'_, Shared>, action: WindowAction) -> Result<(), String> {
    state.perform_action(action).map_err(|err| err.to_string())
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
