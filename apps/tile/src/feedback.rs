//! Running an action with user-facing feedback.
//!
//! The action worker thread (hotkeys and tray menu clicks) funnels through
//! [`run_action_preemptible`], so its feedback policy lives in exactly one
//! place. A denied permission is always explained. Otherwise hotkeys stay
//! silent — a shortcut pressed with nothing to move should just do nothing —
//! but a menu click that achieves nothing would look like an ignored click, so
//! it gets a short, rate-limited explanation. The settings window's
//! `perform_action` command calls the pipeline directly and returns errors to
//! its caller instead.

use std::path::Path;
use std::sync::Arc;

use tauri::{AppHandle, Manager, Runtime};
use tauri_plugin_dialog::{DialogExt, MessageDialogButtons, MessageDialogKind};
use tile_core::WindowAction;
use tile_platform::PlatformError;

use crate::logging;
use crate::state::{is_permission_denied, ActionOutcome, ActionRequest, AppState};
use crate::window;

/// Where an action came from, which decides how loudly it may fail.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ActionOrigin {
    Hotkey,
    TrayMenu,
}

impl ActionOrigin {
    /// Only tray menu items send exact requests: a hotkey is never exact,
    /// because repeating one is how a window cycles through sizes.
    pub fn of(request: &ActionRequest) -> Self {
        if request.exact {
            Self::TrayMenu
        } else {
            Self::Hotkey
        }
    }
}

/// What the user is told once an action has run.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Notice {
    /// Log only.
    Silent,
    /// The permission dialog, whatever the origin.
    PermissionDenied,
    /// Nothing was focused that Tile could move.
    NoWindow,
    /// The move itself failed.
    Failed,
}

fn notice_for(origin: ActionOrigin, result: &Result<ActionOutcome, PlatformError>) -> Notice {
    match (origin, result) {
        (_, Err(err)) if is_permission_denied(err) => Notice::PermissionDenied,
        (ActionOrigin::Hotkey, _) => Notice::Silent,
        (ActionOrigin::TrayMenu, Ok(ActionOutcome::NoWindow)) => Notice::NoWindow,
        // Already where it was asked to go: the click did what it said.
        (ActionOrigin::TrayMenu, Ok(ActionOutcome::Moved | ActionOutcome::NoOp)) => Notice::Silent,
        (ActionOrigin::TrayMenu, Err(_)) => Notice::Failed,
    }
}

/// Performs `action`, then tells the user only what the policy in
/// [`notice_for`] says is worth telling. Everything is logged.
///
/// `next` is polled once per animation frame and must not block; the hotkey
/// worker passes a non-blocking receive on its channel, which is what lets a
/// second press steer a movement already under way instead of queueing behind
/// it. Callers with no source of further actions pass a closure returning
/// `None`.
///
/// The worker's only entry point, so hotkeys and tray menu clicks share one
/// animation pipeline.
pub fn run_action_preemptible<R: Runtime>(
    app: &AppHandle<R>,
    request: ActionRequest,
    next: &mut dyn FnMut() -> Option<WindowAction>,
) {
    let state = app.state::<Arc<AppState>>();
    let action = request.action;

    // Reported from inside the pipeline rather than from its return value.
    // The return value arrives only once every window has finished
    // travelling, which would leave the walkthrough silent for the whole
    // length of the animation it is supposed to be narrating — the window
    // would come to rest before the screen acknowledged the key. It also
    // collapses a burst of presses into a single verdict, because each press
    // after the first is swallowed to retarget the flight, so a walkthrough
    // counting presses would undercount exactly when the user is fluent.
    let result = state.perform_action_preemptible(request, next, &mut |report| {
        window::notify_action_performed(app, report);
    });

    if let Err(err) = &result {
        log::error!("action {action} failed: {err}");
    }
    match notice_for(ActionOrigin::of(&request), &result) {
        Notice::Silent => {}
        Notice::PermissionDenied => {
            if let Err(err) = &result {
                on_permission_denied(app, err);
            }
        }
        Notice::NoWindow => show_menu_notice(app, NO_WINDOW_MESSAGE),
        Notice::Failed => show_menu_notice(app, &failed_message(logging::active_log_dir())),
    }
}

const NO_WINDOW_MESSAGE: &str = "There is no window for Tile to move.\n\nClick the window you \
                                 want to place, then choose the action from the menu again.";

/// Told when a tray menu action could not be handed to the action worker.
pub const WORKER_GONE_MESSAGE: &str = "Tile could not move the window because part of it has \
                                       stopped working.\n\nQuit Tile from the menu and open it \
                                       again.";

fn failed_message(log_dir: Option<&Path>) -> String {
    let mut message = String::from(
        "Tile could not move that window.\n\nSome windows, such as full-screen apps and system \
         windows, cannot be moved or resized.",
    );
    if let Some(dir) = log_dir {
        message.push_str(&format!(
            " If this keeps happening, the details are in the log folder:\n{}",
            dir.display()
        ));
    }
    message
}

/// Shows a small, rate-limited notice for a tray menu action that achieved
/// nothing, so a click never looks ignored and a stuck menu cannot stack up
/// dialogs.
pub fn show_menu_notice<R: Runtime>(app: &AppHandle<R>, message: &str) {
    let state = app.state::<Arc<AppState>>();
    if !state.should_show_menu_notice() {
        log::debug!("suppressing a menu notice inside its cooldown");
        return;
    }
    app.dialog()
        .message(message)
        .title("Tile")
        .kind(MessageDialogKind::Info)
        .buttons(MessageDialogButtons::Ok)
        .show(|_| {});
}

fn on_permission_denied<R: Runtime>(app: &AppHandle<R>, err: &PlatformError) {
    // One failed call proves nothing on its own, so ask macOS again. A real
    // revocation flips the shared state, which refreshes the tray and the
    // settings window exactly as the monitor would have.
    if let Err(check_err) = crate::permission::refresh(app, false) {
        log::warn!("could not re-check permission after a denied action: {check_err}");
    }

    let state = app.state::<Arc<AppState>>();
    if !state.should_show_permission_dialog() {
        return;
    }

    // Bring the settings window (which hosts the permission panel) forward.
    if let Err(open_err) = window::open_settings(app, state.build_kind()) {
        log::error!("could not open settings window: {open_err}");
    }

    app.dialog()
        .message(permission_message(err))
        .title("Permission needed")
        .kind(MessageDialogKind::Warning)
        .buttons(MessageDialogButtons::Ok)
        .show(|_| {});
}

fn permission_message(err: &PlatformError) -> String {
    if cfg!(target_os = "windows") {
        format!(
            "Windows denied Tile permission to move this window.\n\n{err}\n\nThe target may be \
             running as administrator. Run Tile at the same privilege level, then try again."
        )
    } else if cfg!(target_os = "macos") {
        format!(
            "Tile needs Accessibility permission to move windows.\n\n{err}\n\nIn System \
             Settings, open Privacy & Security ▸ Accessibility and switch on Tile. Tile's \
             settings window explains each step."
        )
    } else {
        format!("Tile does not have permission to move this window.\n\n{err}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn denied() -> PlatformError {
        PlatformError::PermissionDenied("denied".into())
    }

    fn failed() -> PlatformError {
        PlatformError::os("set frame", "simulated failure")
    }

    #[test]
    fn exact_requests_come_from_the_tray_menu() {
        let menu = ActionRequest::exact(WindowAction::LeftHalf);
        let hotkey = ActionRequest::from(WindowAction::LeftHalf);
        assert_eq!(ActionOrigin::of(&menu), ActionOrigin::TrayMenu);
        assert_eq!(ActionOrigin::of(&hotkey), ActionOrigin::Hotkey);
    }

    #[test]
    fn hotkeys_stay_silent_unless_permission_is_denied() {
        let hotkey = ActionOrigin::Hotkey;
        for result in [
            Ok(ActionOutcome::Moved),
            Ok(ActionOutcome::NoOp),
            Ok(ActionOutcome::NoWindow),
            Err(failed()),
        ] {
            assert_eq!(notice_for(hotkey, &result), Notice::Silent, "{result:?}");
        }
        assert_eq!(notice_for(hotkey, &Err(denied())), Notice::PermissionDenied);
    }

    #[test]
    fn a_menu_click_that_achieves_nothing_is_explained() {
        let menu = ActionOrigin::TrayMenu;
        assert_eq!(notice_for(menu, &Ok(ActionOutcome::Moved)), Notice::Silent);
        assert_eq!(notice_for(menu, &Ok(ActionOutcome::NoOp)), Notice::Silent);
        assert_eq!(
            notice_for(menu, &Ok(ActionOutcome::NoWindow)),
            Notice::NoWindow
        );
        assert_eq!(notice_for(menu, &Err(failed())), Notice::Failed);
        assert_eq!(notice_for(menu, &Err(denied())), Notice::PermissionDenied);
    }

    #[test]
    fn the_failure_notice_names_the_log_folder_when_there_is_one() {
        let dir = Path::new("logs-here");
        assert!(failed_message(Some(dir)).contains("logs-here"));
        assert!(!failed_message(None).contains("log folder"));
    }

    #[test]
    fn permission_message_matches_the_platform_boundary() {
        let error = PlatformError::PermissionDenied("denied".into());
        let message = permission_message(&error);
        #[cfg(target_os = "windows")]
        assert!(message.contains("administrator"));
        #[cfg(target_os = "macos")]
        assert!(message.contains("Accessibility"));
        #[cfg(not(any(target_os = "windows", target_os = "macos")))]
        assert!(message.contains("does not have permission"));
    }
}
