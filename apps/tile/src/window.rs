//! Settings-window lifecycle.
//!
//! The app has no window at startup; the settings window is created on demand
//! and is a single instance — opening it again just re-focuses the existing
//! one. Closing it destroys it (the tray keeps the app alive), and the next
//! open recreates it.

use tauri::{AppHandle, Emitter, Manager, Runtime, WebviewUrl, WebviewWindowBuilder};
use tile_core::WindowAction;

use crate::build_kind::BuildKind;
use crate::dto::ActionPerformedDto;
use crate::state::ActionOutcome;

/// Stable label for the single settings window.
pub const SETTINGS_LABEL: &str = "settings";
/// Stable label for the about window.
pub const ABOUT_LABEL: &str = "about";
/// Stable label for the welcome window.
pub const WELCOME_LABEL: &str = "welcome";
/// Requests that an open settings window start and surface an update check.
const CHECK_FOR_UPDATES_EVENT: &str = "tile://check-for-updates";
/// Requests that an open settings window surface the current update state.
const SHOW_UPDATES_EVENT: &str = "tile://show-updates";
/// Tells the welcome window that an action just ran, so its walkthrough can
/// tick itself off.
const ACTION_PERFORMED_EVENT: &str = "tile://action-performed";

/// Reports a finished action to the welcome window, if one is open.
///
/// Deliberately addressed rather than broadcast: the walkthrough is the only
/// listener there will ever be, and the settings window has no business
/// hearing about every hotkey the user presses.
pub fn notify_action_performed<R: Runtime>(
    app: &AppHandle<R>,
    action: WindowAction,
    outcome: ActionOutcome,
) {
    if app.get_webview_window(WELCOME_LABEL).is_none() {
        return;
    }
    let payload = ActionPerformedDto {
        action,
        moved: outcome == ActionOutcome::Moved,
        had_window: outcome != ActionOutcome::NoWindow,
    };
    if let Err(err) = app.emit_to(WELCOME_LABEL, ACTION_PERFORMED_EVENT, payload) {
        log::debug!("could not tell the welcome window about {action}: {err}");
    }
}

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

/// Opens the welcome window, focusing it if it already exists.
///
/// This is the onboarding screen: it is what a first run opens, and the only
/// place the defaults are explained. Settings carries controls, not tuition.
pub fn open_welcome<R: Runtime>(app: &AppHandle<R>) -> tauri::Result<()> {
    if let Some(window) = app.get_webview_window(WELCOME_LABEL) {
        window.show()?;
        window.unminimize().ok();
        window.set_focus()?;
        return Ok(());
    }

    let window = WebviewWindowBuilder::new(
        app,
        WELCOME_LABEL,
        WebviewUrl::App("index.html?welcome".into()),
    )
    .title("Welcome to Tile")
    .inner_size(520.0, 560.0)
    .min_inner_size(460.0, 500.0)
    .resizable(true)
    .visible(true)
    .focused(true)
    .build()?;

    // Tile is an accessory app with no Dock icon, so a new window is not
    // brought forward for us the way it would be for an ordinary app. A
    // welcome screen that opens behind whatever the user was doing is a
    // welcome screen they never see.
    window.set_focus()?;
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
        .inner_size(420.0, 420.0)
        .resizable(false)
        .visible(true)
        .build()?;
    Ok(())
}
