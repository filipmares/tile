//! Tray icon and its menu.
//!
//! The menu offers a curated set of layouts — the two sides with their
//! corners, the centred column, halves, maximize/restore and the display
//! throws — each showing its current
//! shortcut, so the menu teaches the bindings and reaches layouts that have
//! none. Positions that come in sizes offer ½ / ⅔ / ⅓. A menu item
//! is always exact: it lands where its label says rather than cycling, see
//! [`crate::state::ActionRequest`]. The full catalogue stays in the settings
//! window rather than becoming an unusable tray list.

use std::str::FromStr;
use std::sync::mpsc::Sender;
use std::sync::Arc;

use tauri::menu::{IsMenuItem, Menu, MenuItem, PredefinedMenuItem, Submenu};
use tauri::tray::TrayIconBuilder;
use tauri::{AppHandle, Manager, Runtime};
use tile_core::{Config, Hotkey, KeyCode, Modifiers, WindowAction};

use crate::build_kind::BuildKind;
use crate::state::{ActionRequest, AppState};
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
/// Prefix for menu item ids that perform a window action.
const ACTION_ID_PREFIX: &str = "action:";

/// The labels of a sized position's submenu, in the order of its actions.
const SIZE_LABELS: [&str; 3] = ["½", "⅔", "⅓"];

/// One row of the window-action part of the menu.
enum Entry {
    /// A position offered at ½, ⅔ and ⅓ — each an existing catalogue action,
    /// so the menu never invents a layout the settings window cannot bind.
    Sized(&'static str, [WindowAction; 3]),
    Single(WindowAction),
    /// An action under a shorter label than its catalogue name, for use
    /// inside a submenu whose title already supplies the context.
    Labelled(&'static str, WindowAction),
    /// A submenu of further entries.
    Group(&'static str, &'static [Entry]),
    Separator,
}

/// A side: its ½ / ⅔ / ⅓ columns, then its two corners. Corners stay
/// half-height and vary in width, so a ⅓ corner leaves room to stack a second
/// window beneath it.
const LEFT: &[Entry] = &[
    Entry::Labelled("½", WindowAction::LeftHalf),
    Entry::Labelled("⅔", WindowAction::FirstTwoThirds),
    Entry::Labelled("⅓", WindowAction::FirstThird),
    Entry::Separator,
    Entry::Sized(
        "Top",
        [
            WindowAction::TopLeft,
            WindowAction::TopLeftThird,
            WindowAction::TopLeftSixth,
        ],
    ),
    Entry::Sized(
        "Bottom",
        [
            WindowAction::BottomLeft,
            WindowAction::BottomLeftThird,
            WindowAction::BottomLeftSixth,
        ],
    ),
];

const RIGHT: &[Entry] = &[
    Entry::Labelled("½", WindowAction::RightHalf),
    Entry::Labelled("⅔", WindowAction::LastTwoThirds),
    Entry::Labelled("⅓", WindowAction::LastThird),
    Entry::Separator,
    Entry::Sized(
        "Top",
        [
            WindowAction::TopRight,
            WindowAction::TopRightThird,
            WindowAction::TopRightSixth,
        ],
    ),
    Entry::Sized(
        "Bottom",
        [
            WindowAction::BottomRight,
            WindowAction::BottomRightThird,
            WindowAction::BottomRightSixth,
        ],
    ),
];

const DISPLAYS: &[Entry] = &[
    Entry::Labelled("Left", WindowAction::DisplayLeft),
    Entry::Labelled("Right", WindowAction::DisplayRight),
    Entry::Labelled("Above", WindowAction::DisplayUp),
    Entry::Labelled("Below", WindowAction::DisplayDown),
];

const ACTION_ENTRIES: &[Entry] = &[
    Entry::Group("Left", LEFT),
    Entry::Group("Right", RIGHT),
    Entry::Sized(
        "Center Column",
        [
            WindowAction::CenterHalf,
            WindowAction::CenterTwoThirds,
            WindowAction::CenterThird,
        ],
    ),
    Entry::Single(WindowAction::TopHalf),
    Entry::Single(WindowAction::BottomHalf),
    Entry::Separator,
    Entry::Single(WindowAction::Maximize),
    Entry::Single(WindowAction::AlmostMaximize),
    Entry::Single(WindowAction::Center),
    Entry::Single(WindowAction::Restore),
    Entry::Separator,
    Entry::Group("Displays", DISPLAYS),
];

/// The worker thread's queue, as seen by the tray.
///
/// The menu callback runs on Tauri's main event loop, and an animated action
/// holds the pipeline for the whole animation, so running it inline would
/// freeze the menu and the settings window. Queueing behind hotkeys also keeps
/// the two in order rather than racing for the locks.
pub struct MenuActions(Sender<ActionRequest>);

impl MenuActions {
    pub fn new(sender: Sender<ActionRequest>) -> Self {
        Self(sender)
    }

    fn enqueue(&self, action: WindowAction) {
        if let Err(err) = self.0.send(ActionRequest::exact(action)) {
            log::error!("could not queue {action}: the action worker is gone ({err})");
        }
    }
}

/// The hotkey in the accelerator syntax the menu library parses.
///
/// Only macOS uses this (see [`native_accelerator`]). Accelerators on a tray
/// menu are display-only: neither platform registers
/// them as shortcuts, so this never competes with Tile's own hotkeys. A string
/// the library cannot parse is dropped silently, leaving the item unlabelled
/// rather than missing.
fn accelerator(hotkey: Hotkey) -> String {
    let m = hotkey.modifiers;
    let mut text = String::new();
    for (modifier, token) in [
        (Modifiers::CONTROL, "Ctrl+"),
        (Modifiers::ALT, "Alt+"),
        (Modifiers::SHIFT, "Shift+"),
        (Modifiers::META, "Super+"),
    ] {
        if m.contains(modifier) {
            text.push_str(token);
        }
    }
    text.push_str(match hotkey.key {
        KeyCode::Backtick => "Backquote",
        KeyCode::Equals => "Equal",
        KeyCode::LeftBracket => "BracketLeft",
        KeyCode::RightBracket => "BracketRight",
        other => other.label(),
    });
    text
}

fn action_item<R: Runtime>(
    app: &AppHandle<R>,
    config: &Config,
    action: WindowAction,
    label: &str,
) -> tauri::Result<MenuItem<R>> {
    let hotkey = config.binding(action);
    MenuItem::with_id(
        app,
        format!("{ACTION_ID_PREFIX}{}", action.id()),
        item_text(label, hotkey),
        true,
        native_accelerator(hotkey),
    )
}

/// The item's text. Windows draws whatever follows a tab right-aligned as the
/// shortcut column, so the hotkey is written there in Tile's own notation
/// (`Win+Left`, as in Settings) instead of the menu library's `Windows+Left`.
fn item_text(label: &str, hotkey: Option<Hotkey>) -> String {
    match hotkey {
        Some(hotkey) if cfg!(windows) => format!("{label}\t{hotkey}"),
        _ => label.to_owned(),
    }
}

/// macOS renders a real key equivalent with its native glyphs (`⌃⌥←`), which
/// is the convention there. Windows uses [`item_text`] instead.
fn native_accelerator(hotkey: Option<Hotkey>) -> Option<String> {
    hotkey.filter(|_| !cfg!(windows)).map(accelerator)
}

/// Builds the window-action rows. Each is boxed so singles and submenus can
/// share one list.
fn action_items<R: Runtime>(
    app: &AppHandle<R>,
    config: &Config,
) -> tauri::Result<Vec<Box<dyn IsMenuItem<R>>>> {
    entry_items(app, config, ACTION_ENTRIES)
}

fn submenu<R: Runtime>(
    app: &AppHandle<R>,
    label: &str,
    children: &[Box<dyn IsMenuItem<R>>],
) -> tauri::Result<Submenu<R>> {
    let refs: Vec<&dyn IsMenuItem<R>> = children.iter().map(|item| item.as_ref()).collect();
    Submenu::with_items(app, label, true, &refs)
}

fn entry_items<R: Runtime>(
    app: &AppHandle<R>,
    config: &Config,
    entries: &[Entry],
) -> tauri::Result<Vec<Box<dyn IsMenuItem<R>>>> {
    let mut items: Vec<Box<dyn IsMenuItem<R>>> = Vec::new();
    for entry in entries {
        match entry {
            Entry::Sized(label, actions) => {
                let sizes = actions
                    .iter()
                    .zip(SIZE_LABELS)
                    .map(|(action, size)| {
                        action_item(app, config, *action, size)
                            .map(|item| Box::new(item) as Box<dyn IsMenuItem<R>>)
                    })
                    .collect::<tauri::Result<Vec<_>>>()?;
                items.push(Box::new(submenu(app, label, &sizes)?));
            }
            Entry::Single(action) => {
                items.push(Box::new(action_item(app, config, *action, action.label())?));
            }
            Entry::Labelled(label, action) => {
                items.push(Box::new(action_item(app, config, *action, label)?));
            }
            Entry::Group(label, children) => {
                let children = entry_items(app, config, children)?;
                items.push(Box::new(submenu(app, label, &children)?));
            }
            Entry::Separator => items.push(Box::new(PredefinedMenuItem::separator(app)?)),
        }
    }
    Ok(items)
}

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
    let actions_separator = PredefinedMenuItem::separator(app)?;
    let separator = PredefinedMenuItem::separator(app)?;
    let quit = MenuItem::with_id(app, ID_QUIT, "Quit Tile", true, None::<&str>)?;
    let config = app.state::<Arc<AppState>>().config();
    let actions = action_items(app, &config)?;

    let mut items: Vec<&dyn IsMenuItem<R>> = Vec::new();
    if let Some((header, dev_separator)) = &dev_header {
        items.push(header);
        items.push(dev_separator);
    }
    items.extend(actions.iter().map(|item| item.as_ref()));
    items.push(&actions_separator);
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
                UpdateStatus::ReadyToRelaunch { .. } => {
                    if let Err(err) = crate::update::relaunch(app) {
                        log::error!("failed to relaunch Tile: {err}");
                    }
                }
                _ => {
                    let check_for_updates = !matches!(status, UpdateStatus::Available { .. });
                    if let Err(err) = window::open_updates(app, check_for_updates) {
                        log::error!("failed to open update window: {err}");
                    }
                }
            }
        }
        ID_QUIT => {
            app.state::<Arc<AppState>>().shutdown_hotkeys();
            app.exit(0);
        }
        other => match other
            .strip_prefix(ACTION_ID_PREFIX)
            .map(WindowAction::from_str)
        {
            Some(Ok(action)) => app.state::<MenuActions>().enqueue(action),
            _ => log::warn!("unknown tray menu id: {other}"),
        },
    }
}

/// Rebuilds the menu so its shortcut labels follow the current bindings.
pub fn sync_bindings<R: Runtime>(app: &AppHandle<R>) {
    let status = app.state::<Arc<UpdateManager>>().status();
    sync_update_state(app, &status);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accelerators_use_the_menu_library_tokens() {
        let base = Hotkey::new(Modifiers::META, KeyCode::Left);
        assert_eq!(accelerator(base), "Super+Left");
        assert_eq!(
            accelerator(Hotkey::new(
                Modifiers::CONTROL | Modifiers::ALT | Modifiers::SHIFT,
                KeyCode::Equals
            )),
            "Ctrl+Alt+Shift+Equal"
        );
    }

    #[test]
    fn windows_writes_the_shortcut_in_tiles_own_notation() {
        let hotkey = Hotkey::new(Modifiers::META | Modifiers::ALT, KeyCode::Left);
        if cfg!(windows) {
            assert_eq!(item_text("Left", Some(hotkey)), "Left\tAlt+Win+Left");
            assert_eq!(native_accelerator(Some(hotkey)), None);
        } else {
            assert_eq!(item_text("Left", Some(hotkey)), "Left");
            assert_eq!(
                native_accelerator(Some(hotkey)).as_deref(),
                Some("Alt+Super+Left")
            );
        }
        assert_eq!(item_text("Left", None), "Left");
    }

    #[test]
    fn every_menu_action_appears_once() {
        fn collect(entries: &[Entry], seen: &mut std::collections::HashSet<WindowAction>) {
            for entry in entries {
                let actions: &[WindowAction] = match entry {
                    Entry::Sized(_, actions) => actions,
                    Entry::Single(action) | Entry::Labelled(_, action) => {
                        std::slice::from_ref(action)
                    }
                    Entry::Group(_, children) => {
                        collect(children, seen);
                        &[]
                    }
                    Entry::Separator => &[],
                };
                for action in actions {
                    assert!(seen.insert(*action), "{action} appears twice in the menu");
                }
            }
        }
        let mut seen = std::collections::HashSet::new();
        collect(ACTION_ENTRIES, &mut seen);
        for corner in [
            WindowAction::TopLeft,
            WindowAction::TopRight,
            WindowAction::BottomLeft,
            WindowAction::BottomRight,
        ] {
            assert!(seen.contains(&corner), "{corner} missing from the menu");
        }
    }

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
    fn menu_bar_glyph_carries_its_shape_in_alpha_only() {
        let icon = tauri::image::Image::from_bytes(MENU_BAR_TEMPLATE).expect("template loads");
        assert!(
            icon.rgba()
                .chunks_exact(4)
                .all(|pixel| pixel[0] == 0 && pixel[1] == 0 && pixel[2] == 0),
            "template images must carry shape in alpha only"
        );
        assert!(
            icon.rgba().chunks_exact(4).any(|pixel| pixel[3] > 0),
            "template must not be fully transparent"
        );
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn badge_is_opaque_and_confined_to_the_lower_right_corner() {
        let icon = tauri::image::Image::from_bytes(MENU_BAR_TEMPLATE).expect("template loads");
        let (width, height) = (icon.width(), icon.height());
        let plain = icon.rgba().to_vec();

        let badged = template_badged_icon().expect("badged template renders");
        assert_eq!((badged.width(), badged.height()), (width, height));
        let badged = badged.rgba();

        // Same geometry the badge is drawn with, so the test pins the region
        // rather than restating the drawing code's arithmetic.
        let badge_radius = f64::from(width.min(height)) / 8.0;
        let center_x = f64::from(width) - badge_radius - 1.0;
        let center_y = f64::from(height) - badge_radius - 1.0;
        let touched_radius = badge_radius + 1.5 + 1.0;

        let mut badge_centre_alpha = None;
        for y in 0..height {
            for x in 0..width {
                let offset = ((y * width + x) * 4) as usize;
                let pixel = &badged[offset..offset + 4];
                assert_eq!(
                    (pixel[0], pixel[1], pixel[2]),
                    (0, 0, 0),
                    "badging must keep the image a template"
                );

                let distance = (f64::from(x) - center_x).hypot(f64::from(y) - center_y);
                if distance > touched_radius {
                    assert_eq!(
                        pixel[3],
                        plain[offset + 3],
                        "badge changed pixel ({x}, {y}) outside its own region"
                    );
                } else if distance < 1.0 {
                    badge_centre_alpha = Some(pixel[3]);
                }
            }
        }

        assert_eq!(
            badge_centre_alpha,
            Some(255),
            "the badge dot itself must be solid"
        );
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

    /// A downloaded update waiting on a relaunch still deserves a badge, so the
    /// tray keeps nudging until the user restarts.
    #[cfg(target_os = "macos")]
    #[test]
    fn updates_waiting_on_a_relaunch_keep_their_badge() {
        assert!(needs_badge(
            BuildKind::Installed,
            &UpdateStatus::ReadyToRelaunch {
                version: "1.2.3".into(),
            }
        ));
    }
}
