//! The single place that talks to the OS login item.
//!
//! Both the startup reconcile and the settings commands go through this
//! module, so the "a development build must not touch the login item" rule is
//! enforced once rather than at every call site.

use tauri::{AppHandle, Runtime};
use tauri_plugin_autostart::ManagerExt;

use crate::build_kind::BuildKind;

/// Applies `enabled` to the OS login item unconditionally — what the settings
/// commands do, since the user just asked for exactly this state.
pub fn apply<R: Runtime>(app: &AppHandle<R>, kind: BuildKind, enabled: bool) {
    if skip_for_development(kind, enabled) {
        return;
    }
    set(app, enabled);
}

/// Aligns the OS login item with the persisted preference at startup, doing
/// nothing when it already agrees.
pub fn reconcile_on_launch<R: Runtime>(app: &AppHandle<R>, kind: BuildKind, desired: bool) {
    if skip_for_development(kind, desired) {
        return;
    }
    if app.autolaunch().is_enabled().unwrap_or(false) == desired {
        return;
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
    if let Err(err) = result {
        log::error!("failed to update launch-on-login to {enabled}: {err}");
    }
}
