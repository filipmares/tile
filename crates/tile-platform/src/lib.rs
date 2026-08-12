//! `tile-platform` — the OS-specific half of Tile.
//!
//! Everything platform-dependent lives behind the two traits in this module.
//! The application only ever talks to [`WindowBackend`] and [`HotkeyBackend`],
//! which keeps `tile-core`'s logic testable and makes adding a third platform
//! a matter of implementing these traits.

use std::sync::mpsc::Sender;

use tile_core::{Hotkey, Rect, Screen, WindowAction, WindowId, WindowSnapshot};

#[cfg(target_os = "macos")]
pub mod macos;
#[cfg(windows)]
pub mod windows;

// The macOS backend's pure helpers (coordinate flipping, key-code tables) are
// `include!`d into `macos` when building for macOS. Compiling them here on
// every other host means their unit tests run on all CI runners, not just the
// macOS one — which is the only place the rest of that backend can build.
// Nothing outside the tests consumes them here, hence `dead_code`.
#[cfg(not(target_os = "macos"))]
#[path = "macos_pure.rs"]
#[allow(dead_code)]
mod macos_pure;

mod unsupported;

/// Errors surfaced by a platform backend.
#[derive(Debug, thiserror::Error)]
pub enum PlatformError {
    /// The OS denied access to other applications' windows. On macOS this
    /// means Accessibility permission has not been granted; on Windows it
    /// usually means the target window belongs to an elevated process.
    #[error("permission denied: {0}")]
    PermissionDenied(String),

    /// There is no focused window, or it is not one Tile can move (a desktop,
    /// a shell window, or a window that refuses to be resized).
    #[error("no movable focused window")]
    NoFocusedWindow,

    /// The hotkey could not be claimed from the OS.
    #[error("failed to register hotkey {hotkey}: {reason}")]
    HotkeyRegistration { hotkey: String, reason: String },

    /// A native API returned an unexpected failure.
    #[error("{context}: {source_message}")]
    Os {
        context: String,
        source_message: String,
    },

    #[error("{0} is not supported on this platform")]
    Unsupported(&'static str),
}

impl PlatformError {
    pub fn os(context: impl Into<String>, source_message: impl Into<String>) -> Self {
        PlatformError::Os {
            context: context.into(),
            source_message: source_message.into(),
        }
    }
}

pub type Result<T> = std::result::Result<T, PlatformError>;

/// Whether the app is allowed to manipulate other applications' windows.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PermissionStatus {
    /// Tile can move windows.
    Granted,
    /// The user must grant permission (macOS Accessibility). The app should
    /// show guidance rather than silently failing.
    Denied,
    /// The platform requires no explicit permission.
    NotRequired,
}

/// Reads and manipulates the windows and displays of the host OS.
///
/// Implementations must present coordinates in a single, unified space: a
/// top-left origin, y growing downwards, spanning the whole virtual desktop.
///
/// The *unit* of that space is whatever the platform natively uses, and the
/// backend must be internally consistent about it:
/// - **Windows** reports physical pixels (the process is per-monitor DPI aware
///   v2, so every Win32 coordinate is already physical).
/// - **macOS** reports points, which the window server treats as logical.
///
/// This is deliberate. `tile-core` only ever subdivides and compares
/// rectangles that came from the same backend, so it never needs to convert
/// between the two — and forcing a conversion would introduce rounding errors
/// on mixed-DPI multi-monitor setups for no benefit. [`Screen::scale_factor`]
/// is therefore informational only.
pub trait WindowBackend: Send {
    /// The currently focused window, or `None` when nothing movable is focused.
    fn focused_window(&self) -> Result<Option<WindowSnapshot>>;

    /// All connected displays. Must never return an empty vector on a working
    /// system; callers treat that as "no screen" and do nothing.
    fn screens(&self) -> Result<Vec<Screen>>;

    /// Moves and resizes a window.
    ///
    /// Returns the frame the window actually ended up with, which may differ
    /// from `target` when an app enforces a minimum size or size increments.
    /// A window in a native full-screen/maximized state must be restored to a
    /// normal state first, otherwise the move silently does nothing.
    fn set_window_frame(&self, id: WindowId, target: Rect) -> Result<Rect>;

    /// Current permission status, optionally prompting the user.
    ///
    /// `prompt` must only be honoured when called from the main thread of a
    /// running app, since it may present system UI.
    fn permission_status(&self, prompt: bool) -> Result<PermissionStatus>;
}

/// Claims global hotkeys from the OS and reports presses.
///
/// Implementations own whatever thread and message loop the platform requires;
/// [`HotkeyBackend::apply`] may be called from any thread.
pub trait HotkeyBackend: Send {
    /// Replaces the full set of registered hotkeys.
    ///
    /// This is intentionally all-or-nothing per call: the backend unregisters
    /// everything it previously held and then registers `bindings`, so the
    /// settings UI can simply re-apply the whole config after any edit.
    ///
    /// Individual bindings that the OS refuses are reported in the returned
    /// vector rather than failing the whole call, so one bad binding cannot
    /// leave the app with no working hotkeys.
    fn apply(&mut self, bindings: &[(Hotkey, WindowAction)]) -> Result<Vec<HotkeyFailure>>;

    /// Releases every hotkey and stops any background thread.
    fn shutdown(&mut self);
}

/// A binding the OS refused to hand over.
#[derive(Debug, Clone, PartialEq)]
pub struct HotkeyFailure {
    pub hotkey: Hotkey,
    pub action: WindowAction,
    pub reason: String,
}

/// Creates the window backend for the current platform.
pub fn window_backend() -> Result<Box<dyn WindowBackend>> {
    #[cfg(windows)]
    {
        Ok(Box::new(windows::WindowsWindowBackend::new()?))
    }
    #[cfg(target_os = "macos")]
    {
        Ok(Box::new(macos::MacWindowBackend::new()?))
    }
    #[cfg(not(any(windows, target_os = "macos")))]
    {
        Ok(Box::new(unsupported::UnsupportedWindowBackend))
    }
}

/// Creates the hotkey backend for the current platform.
///
/// Every recognised hotkey press sends the bound [`WindowAction`] on `events`.
/// The channel is the only way actions leave the backend's thread.
pub fn hotkey_backend(events: Sender<WindowAction>) -> Result<Box<dyn HotkeyBackend>> {
    #[cfg(windows)]
    {
        Ok(Box::new(windows::WindowsHotkeyBackend::new(events)?))
    }
    #[cfg(target_os = "macos")]
    {
        Ok(Box::new(macos::MacHotkeyBackend::new(events)?))
    }
    #[cfg(not(any(windows, target_os = "macos")))]
    {
        let _ = events;
        Ok(Box::new(unsupported::UnsupportedHotkeyBackend))
    }
}
