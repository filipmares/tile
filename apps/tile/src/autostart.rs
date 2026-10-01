//! The single place that talks to the OS login item.
//!
//! Both the startup reconcile and the settings commands go through this
//! module, so the "a development build must not touch the login item" rule is
//! enforced once rather than at every call site.

use tauri::{AppHandle, Runtime};
use tauri_plugin_autostart::ManagerExt;

use crate::build_kind::BuildKind;

/// Appended to the command the OS login item runs, so a launch at sign-in can
/// be told apart from a manual one in the logs.
pub const AUTOSTART_ARG: &str = "--autostart";

/// Whether this process was started by the OS login item.
pub fn launched_by_login_item<I, S>(args: I) -> bool
where
    I: IntoIterator<Item = S>,
    S: AsRef<str>,
{
    args.into_iter().any(|arg| arg.as_ref() == AUTOSTART_ARG)
}

/// Applies `enabled` to the OS login item unconditionally — what the settings
/// commands do, since the user just asked for exactly this state.
pub fn apply<R: Runtime>(app: &AppHandle<R>, kind: BuildKind, enabled: bool) {
    if skip_for_development(kind, enabled) {
        return;
    }
    log::info!("launch-on-login changed in settings; setting the OS login item to {enabled}");
    set(app, enabled);
}

/// Aligns the OS login item with the persisted preference at startup.
///
/// The login item can disappear without Tile being involved — a manual
/// reinstall runs the previous uninstaller, which deletes it — so its state is
/// logged on every launch and repaired whenever it disagrees. An item that is
/// already enabled is rewritten too: that keeps its command pointing at this
/// executable (with [`AUTOSTART_ARG`]) if Tile moved or the command predates
/// the argument. Rewriting is idempotent and cheap.
pub fn reconcile_on_launch<R: Runtime>(app: &AppHandle<R>, kind: BuildKind, desired: bool) {
    if skip_for_development(kind, desired) {
        return;
    }
    let current = app.autolaunch().is_enabled();
    match (&current, desired) {
        (Ok(true), true) => log::info!("OS login item is enabled; refreshing its command"),
        (Ok(false), false) => {
            log::info!("OS login item is disabled, matching the preference");
            return;
        }
        (Ok(false), true) => log::warn!(
            "OS login item is missing or disabled but launch-on-login is on; re-enabling it \
             (a reinstall or Task Manager can remove it)"
        ),
        (Ok(true), false) => {
            log::warn!("OS login item is enabled but launch-on-login is off; disabling it")
        }
        (Err(err), _) => log::warn!(
            "could not read the OS login item ({err}); setting it to {desired} regardless"
        ),
    }
    set(app, desired);
}

/// A development build persists the preference but never rewrites the login
/// item of the copy the user actually installed.
fn skip_for_development(kind: BuildKind, desired: bool) -> bool {
    if kind.manages_autostart() {
        return false;
    }
    log::debug!("development build: leaving the OS login item alone (preference: {desired})");
    true
}

fn set<R: Runtime>(app: &AppHandle<R>, enabled: bool) {
    let manager = app.autolaunch();
    let result = if enabled {
        manager.enable()
    } else {
        manager.disable()
    };
    match result {
        Ok(()) => match manager.is_enabled() {
            Ok(state) if state == enabled => {
                log::debug!("OS login item is now {}", on_off(enabled))
            }
            Ok(state) => log::error!(
                "set the OS login item {} but it still reads {}",
                on_off(enabled),
                on_off(state)
            ),
            Err(err) => log::warn!("could not verify the OS login item: {err}"),
        },
        Err(err) => log::error!("failed to update launch-on-login to {enabled}: {err}"),
    }
}

fn on_off(enabled: bool) -> &'static str {
    if enabled {
        "on"
    } else {
        "off"
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_login_item_argument_marks_an_autostart_launch() {
        assert!(launched_by_login_item(["tile.exe", AUTOSTART_ARG]));
        assert!(!launched_by_login_item(["tile.exe"]));
        assert!(!launched_by_login_item(["tile.exe", "--autostarted"]));
    }
}
