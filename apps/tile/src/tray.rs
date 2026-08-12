//! Tray icon and its menu.
//!
//! The menu exposes "Settings…", every window action (so they are usable
//! without hotkeys), and "Quit". Clicking an action performs it immediately.

use std::str::FromStr;
use std::sync::Arc;

use tauri::menu::{IsMenuItem, Menu, MenuItem, PredefinedMenuItem};
use tauri::tray::TrayIconBuilder;
use tauri::{AppHandle, Manager, Runtime};
use tile_core::WindowAction;

use crate::feedback;
use crate::state::AppState;
use crate::window;

/// Menu item id for the "Settings…" entry.
const ID_SETTINGS: &str = "settings";
/// Menu item id for the "Quit" entry.
const ID_QUIT: &str = "quit";

/// Builds the tray icon and installs its menu handler.
pub fn build_tray<R: Runtime>(app: &AppHandle<R>) -> tauri::Result<()> {
    let settings = MenuItem::with_id(app, ID_SETTINGS, "Settings…", true, None::<&str>)?;
    let sep_top = PredefinedMenuItem::separator(app)?;

    let mut action_items: Vec<MenuItem<R>> = Vec::with_capacity(WindowAction::ALL.len());
    for action in WindowAction::ALL {
        action_items.push(MenuItem::with_id(
            app,
            action.id(),
            action.label(),
            true,
            None::<&str>,
        )?);
    }

    let sep_bottom = PredefinedMenuItem::separator(app)?;
    let quit = MenuItem::with_id(app, ID_QUIT, "Quit Tile", true, None::<&str>)?;

    let mut items: Vec<&dyn IsMenuItem<R>> = Vec::new();
    items.push(&settings);
    items.push(&sep_top);
    for item in &action_items {
        items.push(item);
    }
    items.push(&sep_bottom);
    items.push(&quit);

    let menu = Menu::with_items(app, &items)?;

    let mut builder = TrayIconBuilder::new()
        .tooltip("Tile")
        .menu(&menu)
        .show_menu_on_left_click(true)
        .on_menu_event(|app, event| handle_menu_event(app, event.id.as_ref()));

    if let Some(icon) = app.default_window_icon().cloned() {
        builder = builder.icon(icon);
    }

    builder.build(app)?;
    Ok(())
}

fn handle_menu_event<R: Runtime>(app: &AppHandle<R>, id: &str) {
    match id {
        ID_SETTINGS => {
            if let Err(err) = window::open_settings(app) {
                log::error!("failed to open settings window: {err}");
            }
        }
        ID_QUIT => {
            app.state::<Arc<AppState>>().shutdown_hotkeys();
            app.exit(0);
        }
        other => match WindowAction::from_str(other) {
            Ok(action) => feedback::run_action(app, action),
            Err(_) => log::warn!("unknown tray menu id: {other}"),
        },
    }
}
