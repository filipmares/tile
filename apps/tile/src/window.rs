//! Settings-window lifecycle.
//!
//! The app has no window at startup; the settings window is created on demand
//! and is a single instance — opening it again just re-focuses the existing
//! one. Closing it destroys it (the tray keeps the app alive), and the next
//! open recreates it.

use tauri::{AppHandle, Emitter, Manager, Runtime, WebviewUrl, WebviewWindowBuilder};

use crate::build_kind::BuildKind;

/// Stable label for the single settings window.
pub const SETTINGS_LABEL: &str = "settings";
/// Stable label for the about window.
pub const ABOUT_LABEL: &str = "about";
/// Requests that an open settings window start and surface an update check.
const CHECK_FOR_UPDATES_EVENT: &str = "tile://check-for-updates";
/// Requests that an open settings window surface the current update state.
const SHOW_UPDATES_EVENT: &str = "tile://show-updates";

/// Opens the settings window, focusing it if it already exists.
pub fn open_settings<R: Runtime>(app: &AppHandle<R>, kind: BuildKind) -> tauri::Result<()> {
    if let Some(window) = app.get_webview_window(SETTINGS_LABEL) {
        window.show()?;
        window.unminimize().ok();
        window.set_focus()?;
        return Ok(());
    }

    WebviewWindowBuilder::new(app, SETTINGS_LABEL, WebviewUrl::default())
        .title(kind.window_title())
        .inner_size(560.0, 640.0)
        .min_inner_size(460.0, 480.0)
        .resizable(true)
        .visible(true)
        .build()?;
    Ok(())
}

/// Opens Settings with the Updates panel active, optionally starting a check.
pub fn open_update_settings<R: Runtime>(
    app: &AppHandle<R>,
    kind: BuildKind,
    check_for_updates: bool,
) -> tauri::Result<()> {
    let event = if check_for_updates {
        CHECK_FOR_UPDATES_EVENT
    } else {
        SHOW_UPDATES_EVENT
    };
    if let Some(window) = app.get_webview_window(SETTINGS_LABEL) {
        window.show()?;
        window.unminimize().ok();
        window.set_focus()?;
        window.emit(event, ())?;
        return Ok(());
    }

    let update_intent = if check_for_updates { "check" } else { "show" };
    let initialization_script =
        format!("window.sessionStorage.setItem('tile-update-intent', '{update_intent}');");
    WebviewWindowBuilder::new(app, SETTINGS_LABEL, WebviewUrl::default())
        .initialization_script(initialization_script)
        .title(kind.window_title())
        .inner_size(560.0, 640.0)
        .min_inner_size(460.0, 480.0)
        .resizable(true)
        .visible(true)
        .build()?;
    Ok(())
}

/// Opens the about window, focusing it if it already exists.
pub fn open_about<R: Runtime>(app: &AppHandle<R>) -> tauri::Result<()> {
    if let Some(window) = app.get_webview_window(ABOUT_LABEL) {
        window.show()?;
        window.unminimize().ok();
        window.set_focus()?;
        return Ok(());
    }

    WebviewWindowBuilder::new(app, ABOUT_LABEL, WebviewUrl::App("index.html?about".into()))
        .title("About Tile")
        .inner_size(420.0, 360.0)
        .min_inner_size(380.0, 340.0)
        .resizable(false)
        .visible(true)
        .build()?;
    Ok(())
}
