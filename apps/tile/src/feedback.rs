//! Running an action with user-facing feedback.
//!
//! Both the settings window and the hotkey worker thread funnel through
//! [`run_action`], so the "perform it, and only nag about denied permission"
//! policy lives in exactly one place.

use std::sync::Arc;

use tauri::{AppHandle, Manager, Runtime};
use tauri_plugin_dialog::{DialogExt, MessageDialogButtons, MessageDialogKind};
use tile_core::WindowAction;
use tile_platform::PlatformError;

use crate::state::{is_permission_denied, AppState};
use crate::window;

/// Performs `action`, showing a (rate-limited) dialog only when the OS denies
/// permission. NoOps and other errors are logged, never surfaced.
///
/// `next` is polled once per animation frame and must not block; the hotkey
/// worker passes a non-blocking receive on its channel, which is what lets a
/// second press steer a movement already under way instead of queueing behind
/// it. Callers with no source of further actions pass a closure returning
/// `None`.
///
/// This is the only entry point, so hotkeys and the settings window share one
/// animation pipeline.
pub fn run_action_preemptible<R: Runtime>(
    app: &AppHandle<R>,
    action: WindowAction,
    next: &mut dyn FnMut() -> Option<WindowAction>,
) {
    let state = app.state::<Arc<AppState>>();

    // Reported from inside the pipeline rather than from its return value.
    // The return value arrives only once every window has finished
    // travelling, which would leave the walkthrough silent for the whole
    // length of the animation it is supposed to be narrating — the window
    // would come to rest before the screen acknowledged the key. It also
    // collapses a burst of presses into a single verdict, because each press
    // after the first is swallowed to retarget the flight, so a walkthrough
    // counting presses would undercount exactly when the user is fluent.
    let result = state.perform_action_preemptible(action, next, &mut |action, outcome| {
        window::notify_action_performed(app, action, outcome);
    });

    if let Err(err) = result {
        log::error!("action {action} failed: {err}");
        if is_permission_denied(&err) {
            on_permission_denied(app, &err);
        }
    }
}

fn on_permission_denied<R: Runtime>(app: &AppHandle<R>, err: &PlatformError) {
    let state = app.state::<Arc<AppState>>();
    if !state.should_show_permission_dialog() {
        return;
    }

    // Bring the settings window (which hosts the permission panel) forward.
    if let Err(open_err) = window::open_settings(app, state.build_kind()) {
        log::error!("could not open settings window: {open_err}");
    }

    app.dialog()
        .message(format!(
            "Tile needs Accessibility permission to move windows.\n\n{err}\n\nGrant it in System \
             Settings ▸ Privacy & Security ▸ Accessibility, then try again."
        ))
        .title("Permission needed")
        .kind(MessageDialogKind::Warning)
        .buttons(MessageDialogButtons::Ok)
        .show(|_| {});
}
