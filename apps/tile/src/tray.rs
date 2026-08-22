//! Tray icon and its menu.
//!
//! The menu exposes "About Tile", "Settings…", every window action (so they are usable
//! without hotkeys), and "Quit". Actions are grouped into one submenu per
//! [`WindowFamily`] so the ~45-entry catalogue stays navigable. Clicking an
//! action performs it immediately.

use std::str::FromStr;
use std::sync::Arc;

use tauri::menu::{IsMenuItem, Menu, MenuItem, PredefinedMenuItem, Submenu};
use tauri::tray::TrayIconBuilder;
use tauri::{AppHandle, Manager, Runtime};
use tile_core::{WindowAction, WindowFamily};

use crate::build_kind::BuildKind;
use crate::state::AppState;
use crate::window;

/// Menu item id for the "Settings…" entry.
const ID_SETTINGS: &str = "settings";
/// Menu item id for the "About Tile" entry.
const ID_ABOUT: &str = "about";
/// Menu item id for the "Quit" entry.
const ID_QUIT: &str = "quit";
/// Menu item id for the disabled development-build header. It is never
/// clickable, so it deliberately matches nothing in the event handler.
const ID_DEV_HEADER: &str = "development-header";

/// Builds the tray icon and installs its menu handler.
pub fn build_tray<R: Runtime>(app: &AppHandle<R>, kind: BuildKind) -> tauri::Result<()> {
    // A development build says so at the top of its menu, so two running
    // copies are never confused for one another.
    let dev_header = match kind.tray_header() {
        Some(label) => Some((
            MenuItem::with_id(app, ID_DEV_HEADER, label, false, None::<&str>)?,
            PredefinedMenuItem::separator(app)?,
        )),
        None => None,
    };

    let settings = MenuItem::with_id(app, ID_SETTINGS, "Settings…", true, None::<&str>)?;
    let about = MenuItem::with_id(app, ID_ABOUT, "About Tile", true, None::<&str>)?;
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
    items.push(&about);
    if let Some((header, separator)) = &dev_header {
        items.push(header);
        items.push(separator);
    }
    items.push(&settings);
    items.push(&sep_top);
    for submenu in &submenus {
        items.push(submenu);
    }
    items.push(&sep_bottom);
    items.push(&quit);

    let menu = Menu::with_items(app, &items)?;

    let mut builder = TrayIconBuilder::new()
        .tooltip(kind.tray_tooltip())
        .menu(&menu)
        .show_menu_on_left_click(true)
        .on_menu_event(move |app, event| handle_menu_event(app, event.id.as_ref(), kind));

    if kind.is_development() {
        match development_icon() {
            Ok(icon) => builder = builder.icon(icon),
            Err(err) => log::warn!("could not create development tray icon: {err}"),
        }
    } else if let Some(icon) = app.default_window_icon().cloned() {
        builder = builder.icon(icon);
    }

    builder.build(app)?;
    Ok(())
}

/// Adds an orange status badge to the normal icon without requiring a second
/// platform-specific asset. The badge remains visible at menu-bar/tray sizes.
fn development_icon() -> tauri::Result<tauri::image::Image<'static>> {
    let icon = tauri::image::Image::from_bytes(include_bytes!("../icons/icon.png"))?;
    let width = icon.width();
    let height = icon.height();
    let mut pixels = icon.rgba().to_vec();

    let badge_radius = width.min(height) / 6;
    let center_x = width.saturating_sub(badge_radius + 4);
    let center_y = height.saturating_sub(badge_radius + 4);
    let outer_radius = badge_radius + 2;

    for y in center_y.saturating_sub(outer_radius)..=(center_y + outer_radius).min(height - 1) {
        for x in center_x.saturating_sub(outer_radius)..=(center_x + outer_radius).min(width - 1) {
            let dx = x as i64 - center_x as i64;
            let dy = y as i64 - center_y as i64;
            let distance = ((dx * dx + dy * dy) as f64).sqrt() as u32;
            if distance > outer_radius {
                continue;
            }
            let offset = ((y * width + x) * 4) as usize;
            let color = if distance > badge_radius {
                [20, 35, 55, 255]
            } else {
                [245, 145, 35, 255]
            };
            pixels[offset..offset + 4].copy_from_slice(&color);
        }
    }

    Ok(tauri::image::Image::new_owned(pixels, width, height))
}

fn handle_menu_event<R: Runtime>(app: &AppHandle<R>, id: &str, kind: BuildKind) {
    match id {
        ID_ABOUT => {
            if let Err(err) = window::open_about(app) {
                log::error!("failed to open about window: {err}");
            }
        }
        ID_SETTINGS => {
            if let Err(err) = window::open_settings(app, kind) {
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
