//! Settings-window lifecycle.
//!
//! The app has no window at startup; the settings window is created on demand
//! and is a single instance — opening it again just re-focuses the existing
//! one. Closing it destroys it (the tray keeps the app alive), and the next
//! open recreates it.

use tauri::{AppHandle, Manager, Runtime, WebviewUrl, WebviewWindowBuilder};

use crate::build_kind::BuildKind;

/// Stable label for the single settings window.
pub const SETTINGS_LABEL: &str = "settings";

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
