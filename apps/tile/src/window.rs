//! Window lifecycle.
//!
//! The app has no window at startup; each window is created on demand and is a
//! single instance — opening one again just re-focuses the existing one.
//! Closing it destroys it (the tray keeps the app alive), and the next open
//! recreates it. Settings, About, Update, and Welcome are four separate
//! windows: the update flow deliberately has a screen to itself, and so does
//! the first-run walkthrough.

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
/// Stable label for the dedicated update window.
pub const UPDATES_LABEL: &str = "updates";
/// Requests that an open update window start and surface an update check.
const CHECK_FOR_UPDATES_EVENT: &str = "tile://check-for-updates";
/// Requests that an open update window surface the current update state.
const SHOW_UPDATES_EVENT: &str = "tile://show-updates";
/// Tells the welcome window that an action just ran, so its walkthrough can
/// tick itself off.
const ACTION_PERFORMED_EVENT: &str = "tile://action-performed";

/// Reports an action's verdict to the welcome window, if one is open.
///
/// Called the moment the verdict is decided, which for an animated move is
/// while the window is still travelling. That is deliberate: the walkthrough
/// mirrors the movement on its own little stage, and the two should set off
/// together rather than one waiting politely for the other to finish.
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

/// Opens the dedicated update window, optionally starting a check straight
/// away. Updating has no home in Settings: this window is the whole flow, from
/// the check through the download to the relaunch.
pub fn open_updates<R: Runtime>(app: &AppHandle<R>, check_for_updates: bool) -> tauri::Result<()> {
    let event = if check_for_updates {
        CHECK_FOR_UPDATES_EVENT
    } else {
        SHOW_UPDATES_EVENT
    };
    if let Some(window) = app.get_webview_window(UPDATES_LABEL) {
        window.show()?;
        window.unminimize().ok();
        window.set_focus()?;
        window.emit(event, ())?;
        return Ok(());
    }

    // A window that does not exist yet cannot receive an event, so the intent
    // rides along in its own session storage and is claimed once at boot.
    let update_intent = if check_for_updates { "check" } else { "show" };
    let initialization_script =
        format!("window.sessionStorage.setItem('tile-update-intent', '{update_intent}');");
    WebviewWindowBuilder::new(
        app,
        UPDATES_LABEL,
        WebviewUrl::App("index.html?updates".into()),
    )
    .initialization_script(initialization_script)
    .title("Tile Update")
    .inner_size(420.0, 460.0)
    .min_inner_size(380.0, 380.0)
    .resizable(false)
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
        return Ok(());
    }

    // Seen but not focused, which for this window are two different needs.
    //
    // Tile is an accessory app with no Dock icon, so a new window is not
    // brought forward for us the way it would be for an ordinary app: a
    // welcome screen that opens behind the user's work is one they never see.
    // Floating it above solves that without the usual price. Taking focus
    // would make *this* window the one the user was last in, so the first
    // shortcut they press would move something they are no longer looking at,
    // and the proof the walkthrough exists to give would land out of sight.
    // Left unfocused, the window they were already working in stays the
    // target, and it moves in plain view behind the card.
    WebviewWindowBuilder::new(
        app,
        WELCOME_LABEL,
        WebviewUrl::App("index.html?welcome".into()),
    )
    .title("Welcome to Tile")
    .inner_size(520.0, 560.0)
    .min_inner_size(460.0, 500.0)
    .resizable(true)
    .visible(true)
    .focused(false)
    .always_on_top(true)
    .build()?;

    Ok(())
}

/// Gives the welcome window the keyboard, activating Tile first.
///
/// Both halves are required and neither is sufficient. Activating without
/// `set_focus` brings the app forward but leaves whichever window was last key
/// still key; `set_focus` without activating is what Tauri gives us alone, and
/// for an accessory app it is a no-op the API cheerfully reports as success.
pub fn focus_welcome<R: Runtime>(app: &AppHandle<R>) -> tauri::Result<()> {
    let Some(window) = app.get_webview_window(WELCOME_LABEL) else {
        return Ok(());
    };

    // All three steps have to happen on the main thread, and a command handler
    // is not on it. `NSApplication` is main-thread-only, so activating from the
    // command's own worker thread is a silent no-op — the same shape of bug
    // this function exists to fix, one level down. They also have to happen in
    // this order and on one turn of the run loop.
    let handle = app.clone();
    app.run_on_main_thread(move || {
        // macOS 14 replaced free activation with *cooperative* activation: an
        // app in the background asks, and the window server decides. For an
        // accessory app the answer is no — measured, not assumed. `-activate`
        // returns cleanly and `-isActive` is still false afterwards, so the
        // window comes forward and the keyboard never follows.
        //
        // An app that owns a real, visible, focusable window is entitled to be
        // a regular app, so for as long as this one is up, Tile is one. The
        // Dock icon is the honest price of a window that can be typed into, and
        // it is handed back the moment the window closes.
        #[cfg(target_os = "macos")]
        if let Err(err) = handle.set_activation_policy(tauri::ActivationPolicy::Regular) {
            log::warn!("could not promote Tile for the closing slide: {err}");
        }

        #[cfg(target_os = "macos")]
        tile_platform::macos::activate_app();

        if let Err(err) = window.set_focus() {
            log::warn!("could not focus the welcome window: {err}");
        }
    })
}

/// Closes the welcome window and gives the Dock icon back.
///
/// The promotion in `focus_welcome` is what lets this window be typed into; a
/// tray-only app that kept a Dock icon afterwards would be a wart nobody could
/// explain. Demoting before the close rather than after it means the icon and
/// the window leave together, instead of the icon outliving the window by a
/// frame. Demoting when Tile is still an accessory app is a no-op, so this is
/// safe on every path out of the walkthrough, promoted or not.
pub fn close_welcome<R: Runtime>(app: &AppHandle<R>) -> tauri::Result<()> {
    let Some(window) = app.get_webview_window(WELCOME_LABEL) else {
        return Ok(());
    };

    let handle = app.clone();
    app.run_on_main_thread(move || {
        #[cfg(not(target_os = "macos"))]
        let _ = &handle;

        #[cfg(target_os = "macos")]
        if let Err(err) = handle.set_activation_policy(tauri::ActivationPolicy::Accessory) {
            log::warn!("could not return Tile to the menu bar: {err}");
        }

        if let Err(err) = window.close() {
            log::warn!("could not close the welcome window: {err}");
        }
    })
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
