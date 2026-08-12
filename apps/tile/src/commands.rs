//! The Tauri command surface exposed to the settings UI.
//!
//! Commands are intentionally thin: every real decision lives in
//! `tile_core::Engine` or on [`AppState`]. Mutating commands return the updated
//! [`Config`] so the UI always re-renders from the persisted truth rather than
//! guessing (e.g. [`set_binding`] may unbind a conflicting action).

use std::sync::Arc;

use tauri::{AppHandle, Runtime, State};
use tauri_plugin_autostart::ManagerExt;
use tile_core::{Config, Hotkey, WindowAction};

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
pub fn set_gap(state: State<'_, Shared>, gap: f64) -> Config {
    state.update_config(|config| config.gap = gap)
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
