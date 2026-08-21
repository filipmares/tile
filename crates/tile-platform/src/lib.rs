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

/// An open run of intermediate animation frames for one window.
///
/// Created by [`WindowBackend::begin_animation`], which performs the
/// once-per-animation preparation, so pushing a frame is as close to a single
/// system call as the platform allows. Dropped when the animation ends.
///
/// Frames pushed through here are *intermediate*: the window is expected to
/// keep moving, so nothing is read back and the result is not reported. The
/// final frame goes through [`AnimationSession::finish`] instead.
pub trait AnimationSession {
    /// Places the window's visible frame at `target`, as cheaply as possible.
    ///
    /// An error aborts the animation: the caller stops immediately and
    /// propagates, so a window that becomes unmovable mid-flight (an elevated
    /// process, revoked Accessibility permission) surfaces exactly as it would
    /// have from a plain [`WindowBackend::set_window_frame`].
    fn set_intermediate_frame(&mut self, target: Rect) -> Result<()>;

    /// Applies the **final** frame through the same retained handle and reports
    /// the frame the window actually ended up with.
    ///
    /// This is the final-frame path for an *opened session*, and the one
    /// [`WindowBackend::begin_animation`]'s caller uses whenever a session
    /// exists. [`WindowBackend::set_window_frame`] is the fallback for the
    /// `Ok(None)` case only.
    ///
    /// Going through the session matters because `set_window_frame` identifies
    /// the window by *focus* on macOS and refuses when focus has moved, so an
    /// ordinary click on another window mid-animation would fail the last
    /// frame — stranding the window on its final intermediate rectangle and
    /// skipping the history commit. The session already holds the right
    /// window, so it does not need to ask.
    ///
    /// Unlike [`AnimationSession::set_intermediate_frame`], this must let the
    /// application clamp the request (minimum sizes, size increments) and must
    /// verify the result by reading it back, because this is the frame that has
    /// to stick and the value the engine records. An implementation that cannot
    /// confirm where the window ended up must return an error rather than
    /// echoing `target`, or the engine will record a frame that was never
    /// applied.
    fn finish(&mut self, target: Rect) -> Result<Rect>;

    /// The window's current frame, read through the retained handle.
    ///
    /// Used to discover where opening the session left the window: doing so
    /// restores it out of any maximized, minimized or full-screen state, which
    /// moves it. Reading through the session rather than re-querying the
    /// focused window keeps this correct even if focus changed during the
    /// setup, which on macOS can take up to its setup timeout.
    fn current_frame(&self) -> Result<Rect>;
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

    /// Opens a fast path for a run of *intermediate* animation frames.
    ///
    /// Animating a snap means calling into the window server 10–20 times where
    /// a plain move calls once, so the per-call work [`set_window_frame`] has
    /// to do — restoring a maximized window, re-measuring the window's
    /// invisible border, reading the resulting frame back — becomes the
    /// dominant cost. A session hoists all of that out of the loop: it is
    /// created once, does the preparation once, and then each frame is the
    /// smallest possible call.
    ///
    /// Returning `Ok(None)` means "this backend has no fast path", and the
    /// caller falls back to [`set_window_frame`] for every frame, discarding
    /// the read-back on the intermediate ones. That is the default, which
    /// keeps this addition free for the `unsupported` fallback and for any
    /// future backend.
    ///
    /// When a session *is* opened, [`AnimationSession::finish`] is the path
    /// for the final frame, not [`set_window_frame`] — see that method for
    /// why. `set_window_frame` remains the final-frame path only for the
    /// `Ok(None)` case.
    ///
    /// [`set_window_frame`]: WindowBackend::set_window_frame
    fn begin_animation(&self, _id: WindowId) -> Result<Option<Box<dyn AnimationSession>>> {
        Ok(None)
    }

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
