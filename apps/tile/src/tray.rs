//! Tray icon and its menu.
//!
//! The menu is deliberately minimal: "About Tile", "Settings…" and "Quit".
//! Window actions are driven by hotkeys and configured in the settings window
//! rather than being duplicated as a large tray catalogue.

use std::sync::Arc;

use tauri::menu::{IsMenuItem, Menu, MenuItem, PredefinedMenuItem};
use tauri::tray::TrayIconBuilder;
use tauri::{AppHandle, Manager, Runtime};

use crate::build_kind::BuildKind;
use crate::state::AppState;
use crate::update::{UpdateManager, UpdateStatus};
use crate::window;

const TRAY_ID: &str = "tile-tray";

/// Monochrome menu bar glyph. macOS tints template images to match the menu
/// bar appearance, so the icon stays black on light and white on dark instead
/// of showing the blue app icon.
#[cfg(target_os = "macos")]
const MENU_BAR_TEMPLATE: &[u8] = include_bytes!("../icons/menubar-template.png");

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

    #[cfg(target_os = "macos")]
    {
        builder = builder.icon_as_template(true);
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
    let separator = PredefinedMenuItem::separator(app)?;
    let quit = MenuItem::with_id(app, ID_QUIT, "Quit Tile", true, None::<&str>)?;

    let mut items: Vec<&dyn IsMenuItem<R>> = Vec::new();
    if let Some((header, dev_separator)) = &dev_header {
        items.push(header);
        items.push(dev_separator);
    }
    items.push(&about);
    items.push(&settings);
    items.push(&update);
    items.push(&separator);
    items.push(&quit);

    Menu::with_items(app, &items)
}

/// Whether the tray icon should carry a status badge.
fn needs_badge(kind: BuildKind, status: &UpdateStatus) -> bool {
    kind.is_development()
        || matches!(status, UpdateStatus::Available { .. })
        || ready_version(status).is_some()
}

/// Adds a status badge to the normal icon without requiring another asset.
#[cfg(not(target_os = "macos"))]
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

/// Adds a status badge to the template glyph. Template images are tinted by
/// macOS, so the badge is carved out with alpha rather than colour: a solid dot
/// separated from the glyph by a transparent ring.
#[cfg(target_os = "macos")]
fn template_badged_icon() -> tauri::Result<tauri::image::Image<'static>> {
    let icon = tauri::image::Image::from_bytes(MENU_BAR_TEMPLATE)?;
    let width = icon.width();
    let height = icon.height();
    let mut pixels = icon.rgba().to_vec();

    let badge_radius = f64::from(width.min(height)) / 8.0;
    let gap = 1.5;
    let center_x = f64::from(width) - badge_radius - 1.0;
    let center_y = f64::from(height) - badge_radius - 1.0;
    let outer_radius = badge_radius + gap;

    // Antialiased coverage: fully covered a half pixel inside the radius,
    // fully clear a half pixel outside it.
    let coverage = |distance: f64, radius: f64| (radius + 0.5 - distance).clamp(0.0, 1.0);

    for y in 0..height {
        for x in 0..width {
            let dx = f64::from(x) - center_x;
            let dy = f64::from(y) - center_y;
            let distance = dx.hypot(dy);
            if distance > outer_radius + 1.0 {
                continue;
            }
            let offset = ((y * width + x) * 4) as usize;
            let previous = f64::from(pixels[offset + 3]);
            let alpha = (previous * (1.0 - coverage(distance, outer_radius)))
                .max(coverage(distance, badge_radius) * 255.0);
            pixels[offset..offset + 4].copy_from_slice(&[0, 0, 0, alpha.round() as u8]);
        }
    }

    Ok(tauri::image::Image::new_owned(pixels, width, height))
}

#[cfg(target_os = "macos")]
fn tray_icon<R: Runtime>(
    _app: &AppHandle<R>,
    kind: BuildKind,
    status: &UpdateStatus,
) -> Option<tauri::image::Image<'static>> {
    if needs_badge(kind, status) {
        template_badged_icon()
            .map_err(|err| log::warn!("could not create badged tray icon: {err}"))
            .ok()
    } else {
        tauri::image::Image::from_bytes(MENU_BAR_TEMPLATE)
            .map_err(|err| log::warn!("could not load tray icon: {err}"))
            .ok()
    }
}

#[cfg(not(target_os = "macos"))]
fn tray_icon<R: Runtime>(
    _app: &AppHandle<R>,
    kind: BuildKind,
    status: &UpdateStatus,
) -> Option<tauri::image::Image<'static>> {
    let badge = if !needs_badge(kind, status) {
        None
    } else if kind.is_development() {
        Some([245, 145, 35, 255])
    } else {
        Some([37, 99, 235, 255])
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
    if let UpdateStatus::Available { version, .. } = status {
        format!("Tile — {version} available")
    } else if let Some(version) = ready_version(status) {
        format!("Tile — relaunch to finish {version}")
    } else {
        kind.tray_tooltip().to_string()
    }
}

fn ready_version(status: &UpdateStatus) -> Option<&str> {
    #[cfg(target_os = "macos")]
    if let UpdateStatus::ReadyToRelaunch { version } = status {
        return Some(version);
    }
    let _ = status;
    None
}

fn update_menu_state(status: &UpdateStatus) -> (String, bool) {
    if let Some(version) = ready_version(status) {
        return (format!("Relaunch Tile {version}"), true);
    }
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
        #[cfg(target_os = "macos")]
        UpdateStatus::ReadyToRelaunch { .. } => unreachable!("handled before match"),
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
        #[cfg(target_os = "macos")]
        if let Err(err) = tray.set_icon_as_template(true) {
            log::warn!("could not keep tray icon as a template: {err}");
        }
    }
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
                #[cfg(target_os = "macos")]
                UpdateStatus::ReadyToRelaunch { .. } => app.request_restart(),
                _ => {
                    let check_for_updates = !matches!(status, UpdateStatus::Available { .. });
                    if let Err(err) = window::open_update_settings(app, kind, check_for_updates) {
                        log::error!("failed to open update settings: {err}");
                    }
                }
            }
        }
        ID_QUIT => {
            app.state::<Arc<AppState>>().shutdown_hotkeys();
            app.exit(0);
        }
        other => log::warn!("unknown tray menu id: {other}"),
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

    #[cfg(target_os = "macos")]
    #[test]
    fn menu_bar_glyph_is_monochrome_and_badges_stay_inside_it() {
        let icon = tauri::image::Image::from_bytes(MENU_BAR_TEMPLATE).expect("template loads");
        let (width, height) = (icon.width(), icon.height());
        assert!(
            icon.rgba()
                .chunks_exact(4)
                .all(|pixel| pixel[0] == 0 && pixel[1] == 0 && pixel[2] == 0),
            "template images must carry shape in alpha only"
        );

        let badged = template_badged_icon().expect("badged template renders");
        assert_eq!((badged.width(), badged.height()), (width, height));
        assert_ne!(badged.rgba(), icon.rgba());
    }

    #[test]
    fn badges_only_appear_for_development_or_pending_updates() {
        assert!(needs_badge(BuildKind::Development, &UpdateStatus::Current));
        assert!(needs_badge(
            BuildKind::Installed,
            &UpdateStatus::Available {
                version: "1.2.3".into(),
                notes: None,
                date: None,
            }
        ));
        assert!(!needs_badge(BuildKind::Installed, &UpdateStatus::Current));
    }
}
