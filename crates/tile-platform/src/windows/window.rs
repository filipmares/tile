//! Window enumeration and manipulation on Windows.

use std::ffi::c_void;

use tile_core::{Rect, Screen, WindowId, WindowSnapshot};

use windows::core::{Error as WinError, HRESULT};
use windows::Win32::Foundation::{BOOL, ERROR_ACCESS_DENIED, HWND, LPARAM, RECT};
use windows::Win32::Graphics::Dwm::{
    DwmGetWindowAttribute, DWMWA_CLOAKED, DWMWA_EXTENDED_FRAME_BOUNDS,
};
use windows::Win32::Graphics::Gdi::{
    EnumDisplayMonitors, GetMonitorInfoW, HDC, HMONITOR, MONITORINFO, MONITORINFOEXW,
};
use windows::Win32::UI::HiDpi::{GetDpiForMonitor, MDT_EFFECTIVE_DPI};
use windows::Win32::UI::WindowsAndMessaging::{
    GetClassNameW, GetForegroundWindow, GetShellWindow, GetWindowLongW, GetWindowRect, IsIconic,
    IsWindowVisible, IsZoomed, SetWindowPos, ShowWindow, GWL_EXSTYLE, GWL_STYLE,
    MONITORINFOF_PRIMARY, SWP_NOACTIVATE, SWP_NOZORDER, SW_RESTORE, WS_CAPTION, WS_EX_TOOLWINDOW,
};

use crate::{PermissionStatus, PlatformError, Result, WindowBackend};

use super::{ensure_dpi_awareness, hwnd_from_id, id_from_hwnd, rect_from_win};

/// The per-side difference, in pixels, between a window's *visible* frame
/// (`DWMWA_EXTENDED_FRAME_BOUNDS`) and its *outer* frame (`GetWindowRect`).
///
/// On Windows 10/11 the outer frame includes an invisible resize border of a
/// few pixels on the left, right and bottom. We read positions from the visible
/// frame but must drive `SetWindowPos`, which operates on the outer frame, so we
/// carry this delta to translate between the two spaces.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct FrameDelta {
    pub left: f64,
    pub top: f64,
    pub right: f64,
    pub bottom: f64,
}

impl FrameDelta {
    /// `visible - outer` on each edge. With the typical invisible border this
    /// yields roughly `{ left: +7, top: 0, right: -7, bottom: -7 }`.
    fn between(visible: Rect, outer: Rect) -> Self {
        FrameDelta {
            left: visible.x - outer.x,
            top: visible.y - outer.y,
            right: visible.max_x() - outer.max_x(),
            bottom: visible.max_y() - outer.max_y(),
        }
    }

    #[cfg(test)]
    fn zero() -> Self {
        FrameDelta {
            left: 0.0,
            top: 0.0,
            right: 0.0,
            bottom: 0.0,
        }
    }
}

/// Given the frame we *want the visible window to occupy* and the invisible
/// border `delta` for that specific window, returns the outer rectangle to feed
/// to `SetWindowPos` so the visible frame lands exactly on `target`.
///
/// Pure and HWND-free so it can be unit tested. Inverts [`FrameDelta::between`]:
/// `outer = visible - delta` on every edge.
pub(crate) fn apply_frame_delta(target: Rect, delta: FrameDelta) -> Rect {
    Rect::new(
        target.x - delta.left,
        target.y - delta.top,
        target.width + delta.left - delta.right,
        target.height + delta.top - delta.bottom,
    )
}

pub struct WindowsWindowBackend;

impl WindowsWindowBackend {
    pub fn new() -> Result<Self> {
        // Establish physical-pixel coordinates before any window/monitor query;
        // see the module-level comment for the full rationale.
        ensure_dpi_awareness();
        Ok(Self)
    }
}

impl WindowBackend for WindowsWindowBackend {
    fn focused_window(&self) -> Result<Option<WindowSnapshot>> {
        // SAFETY: `GetForegroundWindow` takes no arguments and returns either a
        // valid window handle or null; we validate everything before use.
        let hwnd = unsafe { GetForegroundWindow() };
        if hwnd.0.is_null() {
            return Ok(None);
        }
        if !is_manageable(hwnd) {
            // Not an error: a non-movable foreground window (desktop, shell,
            // tool window, a full-screen game) simply means "nothing to do".
            return Ok(None);
        }
        let frame = window_frame(hwnd)?;
        Ok(Some(WindowSnapshot {
            id: id_from_hwnd(hwnd),
            frame,
        }))
    }

    fn screens(&self) -> Result<Vec<Screen>> {
        let mut screens: Vec<Screen> = Vec::new();
        // SAFETY: we pass a valid callback and hand it a pointer to our local
        // `screens` vec as the user data. The callback only runs synchronously
        // for the duration of this call, so the borrow is live throughout.
        unsafe {
            let _ = EnumDisplayMonitors(
                HDC(std::ptr::null_mut()),
                None,
                Some(monitor_enum_proc),
                LPARAM(&mut screens as *mut Vec<Screen> as isize),
            );
        }
        if screens.is_empty() {
            return Err(PlatformError::os(
                "screens",
                "EnumDisplayMonitors reported no usable displays",
            ));
        }
        Ok(screens)
    }

    fn set_window_frame(&self, id: WindowId, target: Rect) -> Result<Rect> {
        let hwnd = hwnd_from_id(id);

        // SAFETY: `hwnd` came from `id_from_hwnd`; every call below is a plain
        // Win32 query/command on that handle. Individual reasoning inline.
        unsafe {
            // `SetWindowPos` is a silent no-op on a maximized window and cannot
            // move a minimized one, so restore to a normal state first. A
            // minimized window may take a beat to finish restoring, but we read
            // the true resulting frame back at the end, so we do not depend on
            // the restore having completed synchronously.
            if IsZoomed(hwnd).as_bool() || IsIconic(hwnd).as_bool() {
                let _ = ShowWindow(hwnd, SW_RESTORE);
            }

            // Compute this window's invisible-border delta fresh: it varies with
            // window style, and reusing another window's delta lands ~7px off.
            let outer = window_rect(hwnd)?;
            let visible = extended_frame(hwnd).unwrap_or(outer);
            let delta = FrameDelta::between(visible, outer);

            let outer_target = apply_frame_delta(target, delta).rounded();

            // Synchronous (no `SWP_ASYNCWINDOWPOS`) on purpose: we must read the
            // resulting frame back immediately below, and the async flag would
            // let `SetWindowPos` return before the move is applied, so the
            // read-back could observe the old frame. `SWP_NOZORDER` keeps the
            // stacking order; `SWP_NOACTIVATE` avoids stealing focus.
            SetWindowPos(
                hwnd,
                HWND(std::ptr::null_mut()),
                outer_target.x as i32,
                outer_target.y as i32,
                outer_target.width as i32,
                outer_target.height as i32,
                SWP_NOZORDER | SWP_NOACTIVATE,
            )
            .map_err(|e| classify_setpos_error(&e))?;

            // Return the ACTUAL visible frame: apps that enforce a minimum size
            // or size increments (terminals, for instance) will not honour the
            // request exactly, and callers need the truth for history/no-op
            // detection.
            let actual = extended_frame(hwnd).or_else(|| window_rect(hwnd).ok());
            Ok(actual.unwrap_or(target))
        }
    }

    fn permission_status(&self, _prompt: bool) -> Result<PermissionStatus> {
        // Windows needs no up-front grant to move ordinary windows. Elevation is
        // handled per-window: moving an admin-owned window fails with
        // `PermissionDenied` from `set_window_frame`.
        Ok(PermissionStatus::NotRequired)
    }
}

/// Reads the visible frame, falling back to the outer frame if DWM is
/// unavailable (e.g. classic theme / remote session).
fn window_frame(hwnd: HWND) -> Result<Rect> {
    // SAFETY: `hwnd` is a validated foreground window handle.
    unsafe {
        if let Some(r) = extended_frame(hwnd) {
            return Ok(r);
        }
        window_rect(hwnd)
    }
}

/// `GetWindowRect` wrapper (outer frame, includes the invisible border).
///
/// # Safety
/// `hwnd` must be a valid window handle.
unsafe fn window_rect(hwnd: HWND) -> Result<Rect> {
    let mut r = RECT::default();
    GetWindowRect(hwnd, &mut r).map_err(|e| PlatformError::os("GetWindowRect", e.message()))?;
    Ok(rect_from_win(r))
}

/// `DWMWA_EXTENDED_FRAME_BOUNDS` wrapper (visible frame). Returns `None` on any
/// failure so the caller can fall back to `GetWindowRect`.
///
/// # Safety
/// `hwnd` must be a valid window handle.
unsafe fn extended_frame(hwnd: HWND) -> Option<Rect> {
    let mut r = RECT::default();
    let ok = DwmGetWindowAttribute(
        hwnd,
        DWMWA_EXTENDED_FRAME_BOUNDS,
        &mut r as *mut RECT as *mut c_void,
        std::mem::size_of::<RECT>() as u32,
    );
    ok.ok().map(|_| rect_from_win(r))
}

/// Decides whether the foreground window is something Tile should ever move.
fn is_manageable(hwnd: HWND) -> bool {
    // SAFETY: every call is a read-only query on `hwnd`, which the caller has
    // already checked is non-null.
    unsafe {
        if hwnd == GetShellWindow() {
            return false;
        }
        if !IsWindowVisible(hwnd).as_bool() {
            return false;
        }

        let style = GetWindowLongW(hwnd, GWL_STYLE) as u32;
        if style & WS_CAPTION.0 != WS_CAPTION.0 {
            // No title bar: not a normal, user-movable top-level window.
            return false;
        }

        let ex_style = GetWindowLongW(hwnd, GWL_EXSTYLE) as u32;
        if ex_style & WS_EX_TOOLWINDOW.0 != 0 {
            return false;
        }

        if is_cloaked(hwnd) {
            // Cloaked windows are invisible UWP/store phantoms living on other
            // virtual desktops; picking one up would move something the user
            // cannot even see.
            return false;
        }

        if is_shell_class(hwnd) {
            return false;
        }

        true
    }
}

/// # Safety
/// `hwnd` must be a valid window handle.
unsafe fn is_cloaked(hwnd: HWND) -> bool {
    let mut cloaked: u32 = 0;
    let ok = DwmGetWindowAttribute(
        hwnd,
        DWMWA_CLOAKED,
        &mut cloaked as *mut u32 as *mut c_void,
        std::mem::size_of::<u32>() as u32,
    );
    ok.is_ok() && cloaked != 0
}

/// # Safety
/// `hwnd` must be a valid window handle.
unsafe fn is_shell_class(hwnd: HWND) -> bool {
    let mut buf = [0u16; 256];
    let len = GetClassNameW(hwnd, &mut buf);
    if len <= 0 {
        return false;
    }
    let name = String::from_utf16_lossy(&buf[..len as usize]);
    matches!(name.as_str(), "Progman" | "WorkerW")
}

/// Maps a `SetWindowPos` failure to a `PlatformError`, recognising the elevated
/// window case (`ERROR_ACCESS_DENIED`) so the app can prompt the user to run as
/// administrator instead of showing an opaque OS error.
fn classify_setpos_error(e: &WinError) -> PlatformError {
    if e.code() == HRESULT::from_win32(ERROR_ACCESS_DENIED.0) {
        PlatformError::PermissionDenied(
            "the focused window belongs to a process running as administrator; \
             run Tile as administrator to manage it"
                .to_string(),
        )
    } else {
        PlatformError::os("SetWindowPos", e.message())
    }
}

/// Callback invoked once per monitor by `EnumDisplayMonitors`.
///
/// # Safety
/// `lparam` must carry a `*mut Vec<Screen>` that outlives the enumeration, as
/// guaranteed by [`WindowsWindowBackend::screens`].
unsafe extern "system" fn monitor_enum_proc(
    hmon: HMONITOR,
    _hdc: HDC,
    _clip: *mut RECT,
    lparam: LPARAM,
) -> BOOL {
    let screens = &mut *(lparam.0 as *mut Vec<Screen>);

    let mut mi: MONITORINFOEXW = std::mem::zeroed();
    mi.monitorInfo.cbSize = std::mem::size_of::<MONITORINFOEXW>() as u32;

    if GetMonitorInfoW(hmon, &mut mi.monitorInfo as *mut MONITORINFO).as_bool() {
        // DPI is informational only (see module comment); default to 96 (100%)
        // if the query fails so we still report a sane scale factor.
        let mut dpi_x: u32 = 96;
        let mut dpi_y: u32 = 96;
        let _ = GetDpiForMonitor(hmon, MDT_EFFECTIVE_DPI, &mut dpi_x, &mut dpi_y);

        screens.push(Screen {
            id: device_name(&mi.szDevice),
            frame: rect_from_win(mi.monitorInfo.rcMonitor),
            work_area: rect_from_win(mi.monitorInfo.rcWork),
            scale_factor: dpi_x as f64 / 96.0,
            is_primary: mi.monitorInfo.dwFlags & MONITORINFOF_PRIMARY != 0,
        });
    }

    // Keep enumerating remaining monitors.
    BOOL(1)
}

/// Extracts the NUL-terminated device name (e.g. `\\.\DISPLAY1`) from a
/// `MONITORINFOEXW::szDevice` buffer.
fn device_name(raw: &[u16; 32]) -> String {
    let end = raw.iter().position(|&c| c == 0).unwrap_or(raw.len());
    String::from_utf16_lossy(&raw[..end])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn frame_delta_round_trips_through_apply() {
        // A window whose outer frame has a 7px invisible border on the sides and
        // bottom, no border on top (the Windows 10/11 default).
        let outer = Rect::new(93.0, 0.0, 814.0, 407.0);
        let visible = Rect::new(100.0, 0.0, 800.0, 400.0);
        let delta = FrameDelta::between(visible, outer);

        assert_eq!(delta.left, 7.0);
        assert_eq!(delta.top, 0.0);
        assert_eq!(delta.right, -7.0);
        assert_eq!(delta.bottom, -7.0);

        // Asking for the visible frame to sit exactly on `visible` must yield
        // back the original outer rect we would pass to SetWindowPos.
        assert_eq!(apply_frame_delta(visible, delta), outer);
    }

    #[test]
    fn apply_frame_delta_expands_to_cover_invisible_border() {
        // Target a clean left-half: 0,0 960x1040 in visible space.
        let target = Rect::new(0.0, 0.0, 960.0, 1040.0);
        let delta = FrameDelta {
            left: 7.0,
            top: 0.0,
            right: -7.0,
            bottom: -7.0,
        };
        let outer = apply_frame_delta(target, delta);

        // Outer must start 7px left and be 14px wider so the *visible* edges are
        // flush with the screen split, not inset by the border.
        assert_eq!(outer, Rect::new(-7.0, 0.0, 974.0, 1047.0));
    }

    #[test]
    fn zero_delta_is_identity() {
        let target = Rect::new(10.0, 20.0, 300.0, 400.0);
        assert_eq!(apply_frame_delta(target, FrameDelta::zero()), target);
    }

    #[test]
    fn device_name_stops_at_nul() {
        let mut raw = [0u16; 32];
        for (i, c) in "\\\\.\\DISPLAY1".encode_utf16().enumerate() {
            raw[i] = c;
        }
        assert_eq!(device_name(&raw), "\\\\.\\DISPLAY1");
    }
}
