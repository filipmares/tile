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
use tauri_plugin_dialog::{DialogExt, MessageDialogButtons, MessageDialogKind};
use tile_core::{WindowAction, WindowFamily};

use crate::build_kind::BuildKind;
use crate::state::AppState;
use crate::update::{UpdateManager, UpdateStatus};
use crate::window;

const TRAY_ID: &str = "tile-tray";

/// Menu item id for the "Settings…" entry.
const ID_SETTINGS: &str = "settings";
/// Menu item id for the "About Tile" entry.
const ID_ABOUT: &str = "about";
/// Menu item id for the "Quit" entry.
const ID_QUIT: &str = "quit";
/// Menu item id for checking for or installing an update.
const ID_UPDATE: &str = "update";
/// Menu item id for the disabled development-build header. It is never
/// clickable, so it deliberately matches nothing in the event handler.
const ID_DEV_HEADER: &str = "development-header";

/// Builds the tray icon and installs its menu handler.
pub fn build_tray<R: Runtime>(app: &AppHandle<R>, kind: BuildKind) -> tauri::Result<()> {
    let status = app.state::<Arc<UpdateManager>>().status();
    let menu = build_menu(app, kind, &status)?;

    let mut builder = TrayIconBuilder::with_id(TRAY_ID)
        .tooltip(tray_tooltip(kind, &status))
        .menu(&menu)
        .show_menu_on_left_click(true)
        .on_menu_event(move |app, event| handle_menu_event(app, event.id.as_ref(), kind));

    if let Some(icon) = tray_icon(app, kind, &status) {
        builder = builder.icon(icon);
    }

    builder.build(app)?;
    Ok(())
}

fn build_menu<R: Runtime>(
    app: &AppHandle<R>,
    kind: BuildKind,
    update_status: &UpdateStatus,
) -> tauri::Result<Menu<R>> {
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
    let (update_label, update_enabled) = update_menu_state(update_status);
    let update = MenuItem::with_id(app, ID_UPDATE, update_label, update_enabled, None::<&str>)?;
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
    items.push(&update);
    items.push(&sep_top);
    for submenu in &submenus {
        items.push(submenu);
    }
    items.push(&sep_bottom);
    items.push(&quit);

    Menu::with_items(app, &items)
}

/// Adds a status badge to the normal icon without requiring another asset.
fn badged_icon(color: [u8; 4]) -> tauri::Result<tauri::image::Image<'static>> {
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
            let pixel = if distance > badge_radius {
                [20, 35, 55, 255]
            } else {
                color
            };
            pixels[offset..offset + 4].copy_from_slice(&pixel);
        }
    }

    Ok(tauri::image::Image::new_owned(pixels, width, height))
}

fn tray_icon<R: Runtime>(
    _app: &AppHandle<R>,
    kind: BuildKind,
    status: &UpdateStatus,
) -> Option<tauri::image::Image<'static>> {
    let badge = if kind.is_development() {
        Some([245, 145, 35, 255])
    } else if matches!(
        status,
        UpdateStatus::Available { .. } | UpdateStatus::ReadyToRelaunch { .. }
    ) {
        Some([37, 99, 235, 255])
    } else {
        None
    };
    match badge {
        Some(color) => badged_icon(color)
            .map_err(|err| log::warn!("could not create badged tray icon: {err}"))
            .ok(),
        None => tauri::image::Image::from_bytes(include_bytes!("../icons/icon.png"))
            .map_err(|err| log::warn!("could not load tray icon: {err}"))
            .ok(),
    }
}

fn tray_tooltip(kind: BuildKind, status: &UpdateStatus) -> String {
    match status {
        UpdateStatus::Available { version, .. } => format!("Tile — {version} available"),
        UpdateStatus::ReadyToRelaunch { version } => {
            format!("Tile — relaunch to finish {version}")
        }
        _ => kind.tray_tooltip().to_string(),
    }
}

fn update_menu_state(status: &UpdateStatus) -> (String, bool) {
    match status {
        UpdateStatus::Unavailable => (
            "Check for Updates (Unavailable in Development)".into(),
            false,
        ),
        UpdateStatus::Idle | UpdateStatus::Current => ("Check for Updates…".into(), true),
        UpdateStatus::Checking => ("Checking for Updates…".into(), false),
        UpdateStatus::Available { version, .. } => (format!("Update Tile to {version}…"), true),
        UpdateStatus::Downloading { version, .. } => {
            (format!("Downloading Tile {version}…"), false)
        }
        UpdateStatus::ReadyToRelaunch { version } => (format!("Relaunch Tile {version}"), true),
        UpdateStatus::Error { .. } => ("Retry Update Check…".into(), true),
    }
}

pub fn sync_update_state<R: Runtime>(app: &AppHandle<R>, status: &UpdateStatus) {
    let Some(tray) = app.tray_by_id(TRAY_ID) else {
        return;
    };
    let kind = app.state::<Arc<AppState>>().build_kind();
    match build_menu(app, kind, status) {
        Ok(menu) => {
            if let Err(err) = tray.set_menu(Some(menu)) {
                log::warn!("could not update tray menu: {err}");
            }
        }
        Err(err) => log::warn!("could not rebuild tray menu: {err}"),
    }
    if let Err(err) = tray.set_tooltip(Some(tray_tooltip(kind, status))) {
        log::warn!("could not update tray tooltip: {err}");
    }
    if let Some(icon) = tray_icon(app, kind, status) {
        if let Err(err) = tray.set_icon(Some(icon)) {
            log::warn!("could not update tray icon: {err}");
        }
    }
}

fn spawn_update_check<R: Runtime>(app: AppHandle<R>) {
    let manager = app.state::<Arc<UpdateManager>>().inner().clone();
    tauri::async_runtime::spawn(async move {
        if let Err(err) = manager.check(&app).await {
            log::warn!("manual update check failed: {err}");
        }
    });
}

fn request_install<R: Runtime>(app: &AppHandle<R>, version: String) {
    let message = if cfg!(target_os = "windows") {
        format!(
            "Tile {version} is ready to download.\n\nTile will close, install the update, and \
             reopen automatically. Continue?"
        )
    } else {
        format!(
            "Tile {version} is ready to download.\n\nTile will install the update and relaunch. \
             Continue?"
        )
    };
    let app_handle = app.clone();
    app.dialog()
        .message(message)
        .title("Update Tile")
        .kind(MessageDialogKind::Info)
        .buttons(MessageDialogButtons::OkCancel)
        .show(move |confirmed| {
            if !confirmed {
                return;
            }
            let manager = app_handle.state::<Arc<UpdateManager>>().inner().clone();
            let install_handle = app_handle.clone();
            tauri::async_runtime::spawn(async move {
                if let Err(err) = manager.install(&install_handle, true).await {
                    log::error!("could not install Tile update: {err}");
                }
            });
        });
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
        ID_UPDATE => {
            let status = app.state::<Arc<UpdateManager>>().status();
            match status {
                UpdateStatus::Available { version, .. } => request_install(app, version),
                UpdateStatus::ReadyToRelaunch { .. } => app.request_restart(),
                _ => spawn_update_check(app.clone()),
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn update_menu_labels_follow_status() {
        assert_eq!(
            update_menu_state(&UpdateStatus::Checking),
            ("Checking for Updates…".into(), false)
        );
        assert_eq!(
            update_menu_state(&UpdateStatus::Available {
                version: "1.2.3".into(),
                notes: None,
                date: None,
            }),
            ("Update Tile to 1.2.3…".into(), true)
        );
        assert!(!update_menu_state(&UpdateStatus::Unavailable).1);
    }

    #[test]
    fn available_updates_change_the_tooltip() {
        assert_eq!(
            tray_tooltip(
                BuildKind::Installed,
                &UpdateStatus::Available {
                    version: "1.2.3".into(),
                    notes: None,
                    date: None,
                }
            ),
            "Tile — 1.2.3 available"
        );
    }
}
