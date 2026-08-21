//! Tray icon and its menu.
//!
//! The menu exposes "Settings…", every window action (so they are usable
//! without hotkeys), and "Quit". Actions are grouped into one submenu per
//! [`WindowFamily`] so the ~45-entry catalogue stays navigable. Clicking an
//! action performs it immediately.

use std::str::FromStr;
use std::sync::Arc;

use tauri::menu::{IsMenuItem, Menu, MenuItem, PredefinedMenuItem, Submenu};
use tauri::tray::TrayIconBuilder;
use tauri::{AppHandle, Manager, Runtime};
use tile_core::{WindowAction, WindowFamily};

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

    // One submenu per family, each holding its actions. Keeping every action
    // reachable but grouped stops the menu from becoming an unusable flat list.
    let mut submenus: Vec<Submenu<R>> = Vec::new();
    for family in WindowFamily::ALL {
        let actions: Vec<WindowAction> = family.actions().collect();
        if actions.is_empty() {
            continue;
        }
        let items: Vec<MenuItem<R>> = actions
            .iter()
            .map(|action| MenuItem::with_id(app, action.id(), action.label(), true, None::<&str>))
            .collect::<tauri::Result<_>>()?;
        let refs: Vec<&dyn IsMenuItem<R>> = items.iter().map(|i| i as &dyn IsMenuItem<R>).collect();
        submenus.push(Submenu::with_items(app, family.label(), true, &refs)?);
    }

    let sep_bottom = PredefinedMenuItem::separator(app)?;
    let quit = MenuItem::with_id(app, ID_QUIT, "Quit Tile", true, None::<&str>)?;

    let mut items: Vec<&dyn IsMenuItem<R>> = Vec::new();
    items.push(&settings);
    items.push(&sep_top);
    for submenu in &submenus {
        items.push(submenu);
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
            // Queue rather than run: this callback is on Tauri's main event
            // loop, and an animated action would hold it for the whole
            // animation, freezing the tray and the settings window.
            Ok(action) => app.state::<Arc<AppState>>().enqueue_action(action),
            Err(_) => log::warn!("unknown tray menu id: {other}"),
        },
    }
}
