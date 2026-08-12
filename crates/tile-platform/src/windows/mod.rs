//! Windows backend for Tile.
//!
//! # Coordinate space: physical pixels, end to end
//!
//! This backend makes the process **per-monitor DPI aware v2** (see
//! [`ensure_dpi_awareness`]). Under PMv2 awareness every Win32 API that takes or
//! returns screen coordinates works in *physical* pixels — the OS performs no
//! hidden scaling. `tile-core` nominally speaks "logical" pixels, but it never
//! actually performs DPI conversion itself: it only ever compares and subdivides
//! rectangles that this backend hands it (window frames and screen work areas).
//!
//! We therefore deliberately treat **everything as physical pixels** and never
//! divide by the scale factor. Because window frames, monitor bounds and target
//! rectangles all live in the same physical space, every comparison and every
//! bit of tiling arithmetic stays internally consistent — including on
//! multi-monitor setups that mix 100% and 150% displays, which is exactly the
//! case that breaks if you convert to logical pixels with a single global
//! scale. `Screen::scale_factor` is reported for information only (e.g. so a UI
//! could show "150%"); it is never used to transform coordinates.
//!
//! The alternative (convert to logical pixels per monitor) is strictly more
//! complex and buys nothing here, since no code downstream needs true logical
//! units. Keeping one space removes an entire class of rounding/offset bugs.

use std::ffi::c_void;

use tile_core::Rect;

use windows::Win32::Foundation::{HWND, RECT};

mod hotkey;
mod window;

pub use hotkey::WindowsHotkeyBackend;
pub use window::{topmost_manageable_window, WindowsWindowBackend};

/// Converts a raw `HWND` into the [`tile_core::WindowId`] we expose to the rest
/// of the app. The pointer value uniquely identifies the window for its
/// lifetime, which is all history tracking needs.
pub(crate) fn id_from_hwnd(hwnd: HWND) -> u64 {
    hwnd.0 as u64
}

/// Reconstructs an `HWND` from a [`tile_core::WindowId`].
///
/// The id is only ever produced by [`id_from_hwnd`] from a real window handle,
/// so the round-trip is exact.
pub(crate) fn hwnd_from_id(id: u64) -> HWND {
    HWND(id as *mut c_void)
}

/// Converts a Win32 `RECT` (left/top/right/bottom) into a top-left-origin
/// [`Rect`] (x/y/width/height), in physical pixels.
pub(crate) fn rect_from_win(r: RECT) -> Rect {
    Rect::new(
        r.left as f64,
        r.top as f64,
        (r.right - r.left) as f64,
        (r.bottom - r.top) as f64,
    )
}

/// Makes the process per-monitor DPI aware v2 so Win32 coordinates are reported
/// in physical pixels and never silently scaled.
///
/// Idempotent: calling it more than once (or when the awareness was already set
/// via a manifest) simply fails harmlessly and is ignored, which is the
/// documented behaviour of a second `SetProcessDpiAwarenessContext` call.
pub fn ensure_dpi_awareness() {
    use windows::Win32::UI::HiDpi::{
        SetProcessDpiAwarenessContext, DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2,
    };

    // SAFETY: `SetProcessDpiAwarenessContext` takes a well-known context handle
    // constant and has no memory-safety preconditions. A failure here only
    // means awareness was already established (e.g. by an app manifest or a
    // prior call), which is exactly the idempotent case we intend to ignore.
    unsafe {
        let _ = SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);
    }
}
