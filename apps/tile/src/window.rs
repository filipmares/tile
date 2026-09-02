//! Window lifecycle.
//!
//! The app has no window at startup; each window is created on demand and is a
//! single instance — opening one again just re-focuses the existing one.
//! Closing it destroys it (the tray keeps the app alive), and the next open
//! recreates it. Settings, About, Update, and Welcome are four separate
//! windows: the update flow deliberately has a screen to itself, and so does
//! the first-run walkthrough.

#[cfg(target_os = "macos")]
use std::collections::HashSet;
#[cfg(target_os = "macos")]
use std::sync::{Mutex, OnceLock};

use tauri::{
    AppHandle, Emitter, Manager, Runtime, WebviewUrl, WebviewWindow, WebviewWindowBuilder,
};

use crate::build_kind::BuildKind;
use crate::dto::ActionPerformedDto;
use crate::state::{ActionOutcome, ActionReport};

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

#[cfg(target_os = "macos")]
static PROMOTED_WINDOWS: OnceLock<Mutex<HashSet<&'static str>>> = OnceLock::new();

/// Shows a user-requested window and gives it the keyboard.
///
/// On macOS, Tile normally runs as an accessory app. AppKit will order an
/// accessory window onscreen but will not activate it, so Tauri's `set_focus`
/// can succeed while leaving the window behind the user's current app. Promote,
/// activate, and focus together on the main thread to make the request reliable.
fn present_focusable_window<R: Runtime>(
    app: &AppHandle<R>,
    window: WebviewWindow<R>,
    label: &'static str,
) -> tauri::Result<()> {
    // Preserve the public contract of the open helpers: failures to show or
    // focus are returned to their caller. On macOS, set_focus can still be an
    // ineffective success while Tile is inactive, so activation below repeats
    // that final step after promoting the app.
    window.show()?;
    window.unminimize().ok();
    window.set_focus()?;

    #[cfg(not(target_os = "macos"))]
    {
        let _ = (app, label);
        Ok(())
    }

    #[cfg(target_os = "macos")]
    {
        let handle = app.clone();
        app.run_on_main_thread(move || {
            promoted_windows()
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .insert(label);
            if let Err(err) = handle.set_activation_policy(tauri::ActivationPolicy::Regular) {
                log::warn!("could not promote Tile to show the {label} window: {err}");
            }
            tile_platform::macos::activate_app();

            if let Err(err) = window.set_focus() {
                log::warn!("could not focus the {label} window: {err}");
            }
        })
    }
}

/// Returns Tile to accessory mode after the last activated window closes.
fn demote_when_destroyed<R: Runtime>(
    app: &AppHandle<R>,
    window: &WebviewWindow<R>,
    label: &'static str,
) {
    #[cfg(not(target_os = "macos"))]
    let _ = (app, window, label);

    #[cfg(target_os = "macos")]
    {
        let handle = app.clone();
        window.on_window_event(move |event| {
            if matches!(event, tauri::WindowEvent::Destroyed) {
                release_promotion(&handle, label);
            }
        });
    }
}

#[cfg(target_os = "macos")]
fn promoted_windows() -> &'static Mutex<HashSet<&'static str>> {
    PROMOTED_WINDOWS.get_or_init(|| Mutex::new(HashSet::new()))
}

#[cfg(target_os = "macos")]
fn release_promotion<R: Runtime>(app: &AppHandle<R>, label: &'static str) {
    let should_demote = {
        let mut windows = promoted_windows()
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        windows.remove(label);
        windows.is_empty()
    };
    if should_demote {
        demote_to_accessory(app);
    }
}

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
pub fn notify_action_performed<R: Runtime>(app: &AppHandle<R>, report: ActionReport) {
    if app.get_webview_window(WELCOME_LABEL).is_none() {
        return;
    }
    let payload = ActionPerformedDto {
        action: report.action,
        moved: report.outcome == ActionOutcome::Moved,
        had_window: report.outcome != ActionOutcome::NoWindow,
        screen: report.screen,
    };
    if let Err(err) = app.emit_to(WELCOME_LABEL, ACTION_PERFORMED_EVENT, payload) {
        log::debug!(
            "could not tell the welcome window about {}: {err}",
            report.action
        );
    }
}

/// Opens the settings window, focusing it if it already exists.
pub fn open_settings<R: Runtime>(app: &AppHandle<R>, kind: BuildKind) -> tauri::Result<()> {
    if let Some(window) = app.get_webview_window(SETTINGS_LABEL) {
        return present_focusable_window(app, window, SETTINGS_LABEL);
    }

    let window = WebviewWindowBuilder::new(app, SETTINGS_LABEL, WebviewUrl::default())
        .title(kind.window_title())
        .inner_size(560.0, 640.0)
        .min_inner_size(460.0, 480.0)
        .resizable(true)
        .visible(false)
        .build()?;
    demote_when_destroyed(app, &window, SETTINGS_LABEL);
    present_focusable_window(app, window, SETTINGS_LABEL)
}

/// Closes the settings window when another app-owned window takes over.
pub fn close_settings<R: Runtime>(app: &AppHandle<R>) -> tauri::Result<()> {
    if let Some(window) = app.get_webview_window(SETTINGS_LABEL) {
        window.close()?;
    }
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
        window.emit(event, ())?;
        return present_focusable_window(app, window, UPDATES_LABEL);
    }

    // A window that does not exist yet cannot receive an event, so the intent
    // rides along in its own session storage and is claimed once at boot.
    let update_intent = if check_for_updates { "check" } else { "show" };
    let initialization_script =
        format!("window.sessionStorage.setItem('tile-update-intent', '{update_intent}');");
    let window = WebviewWindowBuilder::new(
        app,
        UPDATES_LABEL,
        WebviewUrl::App("index.html?updates".into()),
    )
    .initialization_script(initialization_script)
    .title("Tile Update")
    .inner_size(420.0, 460.0)
    .min_inner_size(380.0, 380.0)
    .resizable(false)
    .visible(false)
    .build()?;
    demote_when_destroyed(app, &window, UPDATES_LABEL);
    present_focusable_window(app, window, UPDATES_LABEL)
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
    let builder = WebviewWindowBuilder::new(
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
    .always_on_top(true);

    // `focused(false)` only controls creation-time activation. On Windows the
    // user can still focus the WebView by clicking the walkthrough, after which
    // Win+Arrow belongs to the focused Tile window and Aero Snap moves the card
    // instead of the window it is demonstrating. Make that impossible for the
    // teaching slides; `focus_welcome` opts back in for the ordinary Enter/Esc
    // interaction on the closing slide.
    #[cfg(target_os = "windows")]
    let builder = builder.focusable(false);

    let window = builder.build()?;

    // The safety net for the promotion `focus_welcome` performs. The
    // walkthrough's own exit demotes before it closes, but Cmd+W, the red
    // traffic light, and anything the OS does to this window all bypass that
    // path -- and a tray-only app left with a Dock icon for the rest of the
    // session is a wart the user cannot explain or get rid of. Destroyed fires
    // on every one of those paths, including the tidy one, where demoting
    // again is a no-op.
    demote_when_destroyed(app, &window, WELCOME_LABEL);

    Ok(())
}

/// Hands the Dock icon back after the last activated Tile window closes.
///
/// Safe to call when Tile is already an accessory app: setting the policy it
/// is already in does nothing, which is what lets both the tidy exit and the
/// window-event net call it without coordinating.
#[cfg(target_os = "macos")]
fn demote_to_accessory<R: Runtime>(app: &AppHandle<R>) {
    let handle = app.clone();
    let dispatched = app.run_on_main_thread(move || set_accessory_policy(&handle));
    if let Err(err) = dispatched {
        log::warn!("could not reach the main thread to hide the Dock icon: {err}");
    }
}

/// The demotion itself, for callers already on the main thread.
///
/// Split out so the tidy exit can demote and close on one turn of the run loop
/// while the window-event net dispatches to reach the same code. Activation
/// policy is `NSApplication` state, so calling this off the main thread is a
/// silent no-op rather than an error.
#[cfg(target_os = "macos")]
fn set_accessory_policy<R: Runtime>(app: &AppHandle<R>) {
    if let Err(err) = app.set_activation_policy(tauri::ActivationPolicy::Accessory) {
        log::warn!("could not return Tile to the menu bar: {err}");
    }
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

    #[cfg(target_os = "windows")]
    window.set_focusable(true)?;

    present_focusable_window(app, window, WELCOME_LABEL)
}

/// Closes the welcome window and releases its claim on app activation.
///
/// The promotion in `focus_welcome` is what lets this window be typed into; a
/// tray-only app that kept a Dock icon afterwards would be a wart nobody could
/// explain. When no other Tile window needs focus, demoting before the close
/// means the icon and the window leave together instead of the icon outliving
/// the window by a frame. Releasing an unpromoted welcome window is a no-op.
///
/// This is the tidy exit, not the only one. Every other way the window can go
/// -- Cmd+W, the red traffic light, the OS -- is caught by the `Destroyed`
/// handler installed in `open_welcome`, which demotes as well.
pub fn close_welcome<R: Runtime>(app: &AppHandle<R>) -> tauri::Result<()> {
    let Some(window) = app.get_webview_window(WELCOME_LABEL) else {
        return Ok(());
    };

    #[cfg(target_os = "macos")]
    let handle = app.clone();
    app.run_on_main_thread(move || {
        #[cfg(target_os = "macos")]
        release_promotion(&handle, WELCOME_LABEL);

        if let Err(err) = window.close() {
            log::warn!("could not close the welcome window: {err}");
        }
    })
}

/// Opens the about window, focusing it if it already exists.
pub fn open_about<R: Runtime>(app: &AppHandle<R>) -> tauri::Result<()> {
    if let Some(window) = app.get_webview_window(ABOUT_LABEL) {
        return present_focusable_window(app, window, ABOUT_LABEL);
    }

    let window =
        WebviewWindowBuilder::new(app, ABOUT_LABEL, WebviewUrl::App("index.html?about".into()))
            .title("About Tile")
            .inner_size(420.0, 420.0)
            .resizable(false)
            .visible(false)
            .build()?;
    demote_when_destroyed(app, &window, ABOUT_LABEL);
    present_focusable_window(app, window, ABOUT_LABEL)
}
