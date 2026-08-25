//! macOS backend for Tile.
//!
//! This module is only compiled on macOS (`lib.rs` gates it behind
//! `#[cfg(target_os = "macos")]`). It provides:
//!
//!   * [`MacWindowBackend`] — reads and moves windows through the
//!     **Accessibility (AX) API** from `ApplicationServices`, and enumerates
//!     displays through `NSScreen`.
//!   * [`MacHotkeyBackend`] — claims global hotkeys through **Carbon's**
//!     `RegisterEventHotKey`, which (despite the "deprecated" label) is still
//!     the correct way to get global hotkeys without any extra permission.
//!
//! ## FFI strategy (important, because this cannot be compiled on the author's
//! Windows host)
//!
//! The AX and Carbon C APIs are extremely stable, so almost everything here is
//! hand-written `extern "C"` against `ApplicationServices`, `Carbon` and
//! `CoreFoundation`. That deliberately avoids depending on the fast-moving
//! `objc2-*` wrapper crates for the parts that matter most, so a version bump
//! in those crates cannot silently break window moving or hotkeys.
//!
//! The **one** place Objective-C messaging is unavoidable is display
//! enumeration: `NSScreen.visibleFrame` (the menu-bar/Dock-excluding work area)
//! has no CoreGraphics equivalent. That code uses `objc2`'s low-level
//! `msg_send!` with the runtime class lookup `class!(NSScreen)` (so no
//! `objc2-app-kit` typed API, and no `MainThreadMarker` gating), and only the
//! geometry type `NSRect` comes from `objc2-foundation` (for its `Encode`
//! impl, required for correct struct-return dispatch).
//!
//! ## Coordinate spaces
//!
//! `tile-core` uses a single top-left-origin, y-down space in logical points.
//!   * The **AX API already uses that space**, so window positions/sizes from
//!     AX are used directly with no conversion.
//!   * **`NSScreen` uses AppKit's bottom-left-origin, y-up space**, so every
//!     screen frame/visibleFrame is run through [`flip_rect`] (see
//!     `macos_pure.rs`). The flip pivots around `NSScreen.screens[0]` — the
//!     menu-bar display — *not* `NSScreen.main`.

#![allow(clippy::missing_safety_doc)]

use std::collections::HashMap;
use std::os::raw::c_void;
use std::sync::mpsc::Sender;
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

use core_foundation::base::TCFType;
use core_foundation::boolean::CFBoolean;
use core_foundation::dictionary::CFDictionary;
use core_foundation::string::CFString;

use objc2::runtime::AnyObject;
use objc2::{class, msg_send};
use objc2_foundation::NSRect;

use tile_core::{Hotkey, Rect, Screen, WindowAction, WindowId, WindowSnapshot};

use crate::{
    AnimationSession, HotkeyBackend, HotkeyFailure, PermissionStatus, PlatformError, Result,
    WindowBackend,
};

// The pure, host-testable helpers (`flip_rect`, `carbon_key_code`,
// `carbon_modifiers`) live in their own file so they can also be compiled and
// tested on non-macOS hosts. See the header of `macos_pure.rs`.
include!("macos_pure.rs");

// ---------------------------------------------------------------------------
// Raw FFI declarations
// ---------------------------------------------------------------------------

#[allow(non_snake_case, non_upper_case_globals, dead_code)]
mod ffi {
    use std::os::raw::c_void;

    pub type CFTypeRef = *const c_void;
    pub type CFStringRef = *const c_void;
    pub type CFDictionaryRef = *const c_void;
    pub type CFArrayRef = *const c_void;
    pub type CFNumberRef = *const c_void;
    pub type AXUIElementRef = *const c_void;
    pub type AXValueRef = *const c_void;
    pub type AXError = i32;
    /// `DarwinBoolean` / `Boolean`: a single unsigned byte, non-zero == true.
    pub type Boolean = u8;
    pub type CFHashCode = usize;
    /// `CFIndex`: a signed word used for CoreFoundation counts/indices.
    pub type CFIndex = isize;
    /// `CFNumberType`: selects how `CFNumberGetValue` interprets the buffer.
    pub type CFNumberType = isize;
    /// `pid_t`: a BSD process identifier (a signed 32-bit int on Darwin).
    pub type Pid = i32;

    pub type OSStatus = i32;
    pub type OSType = u32;
    pub type EventTargetRef = *mut c_void;
    pub type EventHandlerRef = *mut c_void;
    pub type EventHandlerCallRef = *mut c_void;
    pub type EventRef = *mut c_void;
    pub type EventHotKeyRef = *mut c_void;
    pub type EventHandlerUPP =
        extern "C" fn(EventHandlerCallRef, EventRef, *mut c_void) -> OSStatus;

    #[repr(C)]
    pub struct EventTypeSpec {
        pub eventClass: OSType,
        pub eventKind: u32,
    }

    #[repr(C)]
    #[derive(Clone, Copy)]
    pub struct EventHotKeyID {
        pub signature: OSType,
        pub id: u32,
    }

    // AXValueType values (AXValue.h). Older headers spell these
    // `kAXValueCGPointType` / `kAXValueCGSizeType`.
    pub const K_AXVALUE_CGPOINT: u32 = 1;
    pub const K_AXVALUE_CGSIZE: u32 = 2;

    // AXError values (AXError.h).
    pub const kAXErrorSuccess: AXError = 0;
    pub const kAXErrorFailure: AXError = -25200;
    pub const kAXErrorCannotComplete: AXError = -25204;
    pub const kAXErrorNotImplemented: AXError = -25208;
    pub const kAXErrorAPIDisabled: AXError = -25211;

    // CFNumberType values (CFNumber.h). We only ever request a 64-bit signed
    // read, which safely widens the 32-bit ints CoreGraphics actually stores.
    pub const kCFNumberSInt64Type: CFNumberType = 4;

    // CGWindowListOption bits (CGWindow.h) and the "no relative window" id.
    pub const kCGWindowListOptionOnScreenOnly: u32 = 1 << 0;
    pub const kCGWindowListExcludeDesktopElements: u32 = 1 << 4;
    pub const kCGNullWindowID: u32 = 0;

    // Carbon event constants.
    pub const kEventClassKeyboard: u32 = 0x6B65_7962; // 'keyb'
    pub const kEventHotKeyPressed: u32 = 5;
    pub const kEventParamDirectObject: u32 = 0x2D2D_2D2D; // '----'
    pub const typeEventHotKeyID: u32 = 0x686B_6964; // 'hkid'
    pub const eventHotKeyExistsErr: OSStatus = -9878;
    pub const noErr: OSStatus = 0;

    #[link(name = "ApplicationServices", kind = "framework")]
    extern "C" {
        pub fn AXUIElementCreateSystemWide() -> AXUIElementRef;
        /// Builds an application-level AX element from a process id. Used to
        /// walk a specific app's `AXWindows` when mapping a `CGWindowID` (from
        /// the CoreGraphics window list) back to a movable AX element.
        pub fn AXUIElementCreateApplication(pid: Pid) -> AXUIElementRef;
        /// Reads the owning process id of an AX element. Used to detect that
        /// the AX "focused application" is Tile itself (its menu-bar item is
        /// frontmost), so we can fall back to the CoreGraphics Z-order scan.
        pub fn AXUIElementGetPid(element: AXUIElementRef, pid: *mut Pid) -> AXError;
        pub fn AXUIElementCopyAttributeValue(
            element: AXUIElementRef,
            attribute: CFStringRef,
            value: *mut CFTypeRef,
        ) -> AXError;
        pub fn AXUIElementSetAttributeValue(
            element: AXUIElementRef,
            attribute: CFStringRef,
            value: CFTypeRef,
        ) -> AXError;
        pub fn AXValueGetValue(value: AXValueRef, theType: u32, valuePtr: *mut c_void) -> Boolean;
        pub fn AXValueCreate(theType: u32, valuePtr: *const c_void) -> AXValueRef;
        pub fn AXIsProcessTrusted() -> Boolean;
        pub fn AXIsProcessTrustedWithOptions(options: CFDictionaryRef) -> Boolean;

        /// Bounds how long an AX call on `element` waits for the target
        /// application to answer, in seconds.
        ///
        /// AX calls are synchronous IPC into another process, so an app that
        /// is busy (or wedged) blocks the caller for the *system* default of
        /// six seconds. That is survivable for a one-shot move, but an
        /// animation makes the call ten times over, so a short per-element
        /// timeout is what stops one unresponsive app from freezing the frame
        /// loop. Passing `0` restores the global default.
        pub fn AXUIElementSetMessagingTimeout(
            element: AXUIElementRef,
            timeoutInSeconds: f32,
        ) -> AXError;

        /// Private-but-universally-used SPI that yields the `CGWindowID` for an
        /// AX element. Rectangle and essentially every window manager relies on
        /// it. See `AXExtension.swift` in the reference implementation.
        ///
        /// Risk: being an underscore-prefixed private symbol, Apple could
        /// remove it. We fall back to a hash-derived id when it fails
        /// (`kAXErrorSuccess` is not returned), so window bookkeeping keeps
        /// working, only losing cross-fetch id stability.
        pub fn _AXUIElementGetWindow(element: AXUIElementRef, out: *mut u32) -> AXError;

        /// CFString key for the `AXIsProcessTrustedWithOptions` prompt option.
        pub static kAXTrustedCheckOptionPrompt: CFStringRef;
    }

    #[link(name = "CoreFoundation", kind = "framework")]
    extern "C" {
        pub fn CFRelease(cf: CFTypeRef);
        pub fn CFRetain(cf: CFTypeRef) -> CFTypeRef;
        pub fn CFHash(cf: CFTypeRef) -> CFHashCode;
        pub fn CFBooleanGetValue(boolean: CFTypeRef) -> Boolean;
        pub fn CFArrayGetCount(array: CFArrayRef) -> CFIndex;
        pub fn CFArrayGetValueAtIndex(array: CFArrayRef, idx: CFIndex) -> *const c_void;
        pub fn CFDictionaryGetValue(dict: CFDictionaryRef, key: *const c_void) -> *const c_void;
        pub fn CFNumberGetValue(
            number: CFNumberRef,
            theType: CFNumberType,
            valuePtr: *mut c_void,
        ) -> Boolean;
        pub static kCFBooleanTrue: CFTypeRef;
        pub static kCFBooleanFalse: CFTypeRef;
    }

    #[link(name = "CoreGraphics", kind = "framework")]
    extern "C" {
        /// Returns the on-screen window list in **front-to-back Z-order** as a
        /// CFArray of CFDictionaries. Follows the CoreFoundation create rule
        /// (the caller owns a `+1` reference).
        pub fn CGWindowListCopyWindowInfo(option: u32, relativeToWindow: u32) -> CFArrayRef;
        /// CFString keys into each window-info dictionary. These are `const`
        /// CFStrings exported by CoreGraphics.
        pub static kCGWindowNumber: CFStringRef;
        pub static kCGWindowOwnerPID: CFStringRef;
        pub static kCGWindowLayer: CFStringRef;
    }

    #[link(name = "Carbon", kind = "framework")]
    extern "C" {
        pub fn GetApplicationEventTarget() -> EventTargetRef;
        pub fn InstallEventHandler(
            target: EventTargetRef,
            handler: EventHandlerUPP,
            numTypes: usize,
            list: *const EventTypeSpec,
            userData: *mut c_void,
            outRef: *mut EventHandlerRef,
        ) -> OSStatus;
        pub fn RemoveEventHandler(handlerRef: EventHandlerRef) -> OSStatus;
        pub fn RegisterEventHotKey(
            hotKeyCode: u32,
            hotKeyModifiers: u32,
            hotKeyID: EventHotKeyID,
            target: EventTargetRef,
            options: u32,
            outRef: *mut EventHotKeyRef,
        ) -> OSStatus;
        pub fn UnregisterEventHotKey(hotKey: EventHotKeyRef) -> OSStatus;
        pub fn GetEventParameter(
            event: EventRef,
            name: u32,
            desiredType: u32,
            actualType: *mut u32,
            bufferSize: usize,
            actualSize: *mut usize,
            data: *mut c_void,
        ) -> OSStatus;
    }
}

/// FourCharCode signature for our hotkeys: `'TILE'`.
const HOTKEY_SIGNATURE: ffi::OSType = 0x5449_4C45;

/// Plain C geometry structs used purely as read/write buffers for `AXValue`.
/// (Kept separate from `objc2-foundation`'s `NSRect`, which is only used for
/// Objective-C struct-return dispatch in screen enumeration.)
#[repr(C)]
#[derive(Clone, Copy)]
struct CGPoint {
    x: f64,
    y: f64,
}

#[repr(C)]
#[derive(Clone, Copy)]
struct CGSize {
    width: f64,
    height: f64,
}

// ---------------------------------------------------------------------------
// CoreFoundation ownership helper
// ---------------------------------------------------------------------------

/// Owns a `+1`-retained CoreFoundation object (e.g. the result of an AX
/// `Copy...`/`Create...` call) and `CFRelease`s it on drop.
struct CfOwned(ffi::CFTypeRef);

impl Drop for CfOwned {
    fn drop(&mut self) {
        // SAFETY: `CfOwned` is only ever constructed from a pointer we own a
        // `+1` reference to (AX `Copy`/`Create` follow the CoreFoundation
        // create rule). Releasing exactly once here balances that reference.
        // The null guard covers the "attribute missing" case.
        if !self.0.is_null() {
            unsafe { ffi::CFRelease(self.0) };
        }
    }
}

// ---------------------------------------------------------------------------
// AX attribute helpers (free functions over a raw element ref)
// ---------------------------------------------------------------------------

/// Copies an attribute value, returning an owned CF reference or `None` when
/// the attribute is absent or the call fails.
fn copy_attribute(element: ffi::AXUIElementRef, name: &str) -> Option<CfOwned> {
    let attr = CFString::new(name);
    let mut value: ffi::CFTypeRef = std::ptr::null();
    // SAFETY: `element` is a valid AX element; `attr` outlives the call and its
    // pointer is a valid CFStringRef; `value` is a valid out-pointer.
    let err = unsafe {
        ffi::AXUIElementCopyAttributeValue(
            element,
            attr.as_concrete_TypeRef() as ffi::CFStringRef,
            &mut value,
        )
    };
    if err == ffi::kAXErrorSuccess && !value.is_null() {
        Some(CfOwned(value))
    } else {
        None
    }
}

/// Reads a `CGPoint`-typed `AXValue` attribute (e.g. `AXPosition`).
fn copy_point(element: ffi::AXUIElementRef, name: &str) -> Option<CGPoint> {
    let value = copy_attribute(element, name)?;
    let mut point = CGPoint { x: 0.0, y: 0.0 };
    // SAFETY: `value.0` is a live AXValue; the destination buffer matches the
    // requested CGPoint type. `AXValueGetValue` returns false if the type is
    // wrong, which we surface as `None`.
    let ok = unsafe {
        ffi::AXValueGetValue(
            value.0,
            ffi::K_AXVALUE_CGPOINT,
            &mut point as *mut CGPoint as *mut c_void,
        )
    };
    (ok != 0).then_some(point)
}

/// Reads a `CGSize`-typed `AXValue` attribute (e.g. `AXSize`).
fn copy_size(element: ffi::AXUIElementRef, name: &str) -> Option<CGSize> {
    let value = copy_attribute(element, name)?;
    let mut size = CGSize {
        width: 0.0,
        height: 0.0,
    };
    // SAFETY: as in `copy_point`, with a CGSize-typed destination buffer.
    let ok = unsafe {
        ffi::AXValueGetValue(
            value.0,
            ffi::K_AXVALUE_CGSIZE,
            &mut size as *mut CGSize as *mut c_void,
        )
    };
    (ok != 0).then_some(size)
}

/// Reads a boolean attribute (e.g. `AXFullScreen`, `AXMinimized`).
fn copy_bool(element: ffi::AXUIElementRef, name: &str) -> Option<bool> {
    let value = copy_attribute(element, name)?;
    // SAFETY: `value.0` is a live CFBoolean; `CFBooleanGetValue` reads it.
    Some(unsafe { ffi::CFBooleanGetValue(value.0) } != 0)
}

/// Reads a string attribute (e.g. `AXSubrole`) as an owned Rust `String`.
fn copy_string(element: ffi::AXUIElementRef, name: &str) -> Option<String> {
    let attr = CFString::new(name);
    let mut value: ffi::CFTypeRef = std::ptr::null();
    // SAFETY: see `copy_attribute`.
    let err = unsafe {
        ffi::AXUIElementCopyAttributeValue(
            element,
            attr.as_concrete_TypeRef() as ffi::CFStringRef,
            &mut value,
        )
    };
    if err != ffi::kAXErrorSuccess || value.is_null() {
        return None;
    }
    // SAFETY: the value is a CFString we own a `+1` reference to (create rule);
    // `wrap_under_create_rule` takes ownership and releases it on drop.
    let string =
        unsafe { CFString::wrap_under_create_rule(value as core_foundation::string::CFStringRef) };
    Some(string.to_string())
}

/// Writes a `CGPoint`/`CGSize` `AXValue` attribute, returning the raw AXError.
fn set_ax_value(
    element: ffi::AXUIElementRef,
    name: &str,
    value_type: u32,
    ptr: *const c_void,
) -> ffi::AXError {
    // SAFETY: `ptr` points to a live CGPoint/CGSize matching `value_type`.
    let created = unsafe { ffi::AXValueCreate(value_type, ptr) };
    if created.is_null() {
        return ffi::kAXErrorFailure;
    }
    let owned = CfOwned(created);
    let attr = CFString::new(name);
    // SAFETY: `element` is valid; `attr`/`owned` outlive the call.
    unsafe {
        ffi::AXUIElementSetAttributeValue(
            element,
            attr.as_concrete_TypeRef() as ffi::CFStringRef,
            owned.0,
        )
    }
}

fn set_position(element: ffi::AXUIElementRef, point: CGPoint) -> ffi::AXError {
    set_ax_value(
        element,
        "AXPosition",
        ffi::K_AXVALUE_CGPOINT,
        &point as *const CGPoint as *const c_void,
    )
}

fn set_size(element: ffi::AXUIElementRef, size: CGSize) -> ffi::AXError {
    set_ax_value(
        element,
        "AXSize",
        ffi::K_AXVALUE_CGSIZE,
        &size as *const CGSize as *const c_void,
    )
}

/// Writes a boolean attribute (used to leave full-screen / minimized state).
fn set_bool(element: ffi::AXUIElementRef, name: &str, value: bool) -> ffi::AXError {
    // SAFETY: `kCFBoolean{True,False}` are immortal CoreFoundation constants.
    let cf_bool = unsafe {
        if value {
            ffi::kCFBooleanTrue
        } else {
            ffi::kCFBooleanFalse
        }
    };
    let attr = CFString::new(name);
    // SAFETY: `element` is valid; `attr` outlives the call; `cf_bool` is a
    // valid CFBoolean.
    unsafe {
        ffi::AXUIElementSetAttributeValue(
            element,
            attr.as_concrete_TypeRef() as ffi::CFStringRef,
            cf_bool,
        )
    }
}

/// Derives a stable-ish [`WindowId`] for an AX window element.
///
/// Prefers the real `CGWindowID` via the private `_AXUIElementGetWindow`. When
/// that is unavailable it falls back to the element's `CFHash`, tagged with the
/// high bit so derived ids can never collide with real window-server ids (which
/// are small incrementing integers). This mirrors Rectangle's approach.
fn window_id(element: ffi::AXUIElementRef) -> WindowId {
    let mut cg_window_id: u32 = 0;
    // SAFETY: `element` is a valid AX window element; `cg_window_id` is a valid
    // out-pointer. The function is a private SPI (see the extern declaration).
    let err = unsafe { ffi::_AXUIElementGetWindow(element, &mut cg_window_id) };
    if err == ffi::kAXErrorSuccess && cg_window_id != 0 {
        return cg_window_id as WindowId;
    }
    // SAFETY: `element` is a valid CF object; `CFHash` reads its hash.
    let hash = unsafe { ffi::CFHash(element) } as u64;
    0x8000_0000_0000_0000u64 | (hash & 0x7FFF_FFFF_FFFF_FFFF)
}

/// Maps an AXError to a [`PlatformError`]. `kAXErrorAPIDisabled` specifically
/// means Accessibility permission is missing.
fn map_ax_error(context: &str, err: ffi::AXError) -> PlatformError {
    match err {
        ffi::kAXErrorAPIDisabled => {
            PlatformError::PermissionDenied("Accessibility permission not granted".to_string())
        }
        ffi::kAXErrorNotImplemented => PlatformError::os(
            context.to_string(),
            "window does not implement this attribute",
        ),
        ffi::kAXErrorCannotComplete => {
            PlatformError::os(context.to_string(), "the application did not respond")
        }
        other => PlatformError::os(context.to_string(), format!("AXError {other}")),
    }
}

/// The frontmost, movable window and the values we read from it in one pass.
struct FrontWindow {
    /// Kept alive so the element ref stays valid while we act on it.
    element: CfOwned,
    id: WindowId,
    frame: Rect,
}

/// True when `element`'s `AXSubrole` marks it as a standard, movable window.
///
/// A window that reports no subrole at all is treated as standard, matching
/// Rectangle, which only *excludes* known non-standard subroles (sheets,
/// popovers, dialogs, ...).
fn is_standard_window(element: ffi::AXUIElementRef) -> bool {
    match copy_string(element, "AXSubrole") {
        Some(subrole) => subrole == "AXStandardWindow",
        None => true,
    }
}

/// Reads the owning process id of an AX element, or `None` if the query fails.
fn ax_element_pid(element: ffi::AXUIElementRef) -> Option<ffi::Pid> {
    let mut pid: ffi::Pid = 0;
    // SAFETY: `element` is a valid AX element and `pid` is a valid out-pointer;
    // the call only reads the element's owning pid.
    let err = unsafe { ffi::AXUIElementGetPid(element, &mut pid) };
    (err == ffi::kAXErrorSuccess).then_some(pid)
}

trait EnhancedUiAccess {
    fn current(&self) -> Option<bool>;
    /// Returns whether the attribute was changed.
    fn set(&self, value: bool) -> Result<bool>;
}

struct AxEnhancedUiAccess {
    app: CfOwned,
}

impl EnhancedUiAccess for AxEnhancedUiAccess {
    fn current(&self) -> Option<bool> {
        copy_bool(self.app.0, "AXEnhancedUserInterface")
    }

    fn set(&self, value: bool) -> Result<bool> {
        let err = set_bool(self.app.0, "AXEnhancedUserInterface", value);
        match err {
            ffi::kAXErrorSuccess => Ok(true),
            ffi::kAXErrorAPIDisabled => Err(map_ax_error("set AXEnhancedUserInterface", err)),
            other => {
                log::debug!(
                    "macOS AXEnhancedUserInterface write returned {other}; continuing without it"
                );
                Ok(false)
            }
        }
    }
}

/// Temporarily suppresses the native frame animations some applications enable
/// through `AXEnhancedUserInterface`.
struct EnhancedUiGuard<A: EnhancedUiAccess> {
    access: A,
    restore_enabled: bool,
}

impl<A: EnhancedUiAccess> EnhancedUiGuard<A> {
    fn new(access: A) -> Result<Self> {
        let mut guard = Self {
            access,
            restore_enabled: false,
        };
        guard.ensure_disabled()?;
        Ok(guard)
    }

    /// Re-suppresses Enhanced UI if another accessibility client enabled it
    /// while Tile was animating the window.
    fn ensure_disabled(&mut self) -> Result<()> {
        if self.access.current() == Some(true) && self.access.set(false)? {
            // Restore every true value Tile actually changed, including one an
            // external client re-enabled during a running animation.
            self.restore_enabled = true;
        }
        Ok(())
    }
}

impl EnhancedUiGuard<AxEnhancedUiAccess> {
    fn for_window(window: ffi::AXUIElementRef) -> Result<Option<Self>> {
        let Some(pid) = ax_element_pid(window) else {
            log::debug!("macOS AX could not resolve the window owner for Enhanced UI suppression");
            return Ok(None);
        };
        // SAFETY: `pid` owns `window`; the returned application element follows
        // the create rule and is released by `CfOwned`.
        let app = CfOwned(unsafe { ffi::AXUIElementCreateApplication(pid) });
        if app.0.is_null() {
            log::debug!("macOS AX could not create the application element for pid {pid}");
            return Ok(None);
        }
        Self::new(AxEnhancedUiAccess { app }).map(Some)
    }
}

impl<A: EnhancedUiAccess> Drop for EnhancedUiGuard<A> {
    fn drop(&mut self) {
        if self.restore_enabled {
            match self.access.set(true) {
                Ok(true) => {}
                Ok(false) => {
                    log::warn!("failed to restore macOS AXEnhancedUserInterface");
                }
                Err(err) => {
                    log::warn!("failed to restore macOS AXEnhancedUserInterface: {err}");
                }
            }
        }
    }
}

/// Builds a [`FrontWindow`] snapshot from a movable AX window element, reading
/// its position and size. Returns `None` if either attribute is unavailable.
fn snapshot_window(element: CfOwned) -> Option<FrontWindow> {
    let position = copy_point(element.0, "AXPosition")?;
    let size = copy_size(element.0, "AXSize")?;
    let id = window_id(element.0);
    // AX is already top-left-origin, so no coordinate flip for the window.
    let frame = Rect::new(position.x, position.y, size.width, size.height);
    Some(FrontWindow { element, id, frame })
}

/// Fetches the frontmost focused window through the system-wide AX element.
///
/// Returns `Ok(None)` (never an error) when nothing suitable is focused or the
/// focused window is not a standard window (a sheet, popover, dialog, ...).
/// Returns `Err(PermissionDenied)` when Accessibility permission is missing.
///
/// When Tile itself is the focused application — which happens when the user
/// clicks Tile's menu-bar item to pick an action — this falls back to the
/// front-most *other* window in CoreGraphics Z-order, so menu-driven actions
/// apply to the user's window rather than to Tile. The global-hotkey path does
/// not hit this fallback: pressing a hotkey never changes which app is
/// frontmost, so the AX focused application is still the user's app.
fn front_window() -> Result<Option<FrontWindow>> {
    // SAFETY: no arguments; returns a bool byte.
    if unsafe { ffi::AXIsProcessTrusted() } == 0 {
        return Err(PlatformError::PermissionDenied(
            "Accessibility permission not granted".to_string(),
        ));
    }

    let own_pid = std::process::id() as ffi::Pid;

    if let Some(window) = focused_ax_window(own_pid) {
        return Ok(Some(window));
    }

    // The AX focused application is Tile itself (or there was no usable focused
    // window). Fall back to the CoreGraphics window list, which mirrors the
    // Windows `EnumWindows` Z-order scan.
    Ok(topmost_foreign_window(own_pid))
}

/// The window focused according to the system-wide AX element, or `None` when
/// the focused application is Tile itself or nothing standard is focused.
fn focused_ax_window(own_pid: ffi::Pid) -> Option<FrontWindow> {
    // SAFETY: creates a `+1` system-wide element; wrapped for release.
    let system_wide = CfOwned(unsafe { ffi::AXUIElementCreateSystemWide() });
    if system_wide.0.is_null() {
        return None;
    }

    let app = copy_attribute(system_wide.0, "AXFocusedApplication")?;

    // If Tile is the focused application, its own status-bar window is what AX
    // would hand back. Bail so the caller falls back to the Z-order scan; this
    // is the whole reason menu-driven actions used to be silent no-ops.
    if ax_element_pid(app.0) == Some(own_pid) {
        return None;
    }

    let window = copy_attribute(app.0, "AXFocusedWindow")?;
    if !is_standard_window(window.0) {
        return None;
    }
    snapshot_window(window)
}

/// Walks the CoreGraphics on-screen window list (front-to-back Z-order) and
/// returns the front-most standard, movable window that is not one of ours.
///
/// This is the macOS analogue of the Windows `topmost_manageable_window`: the
/// CG list is stateless (no run-loop observer, no notification tracking), and
/// front-to-back order means the first match is the window the user worked with
/// immediately before the menu opened. Each candidate `CGWindowID` is mapped
/// back to an `AXUIElement` so it can actually be moved.
fn topmost_foreign_window(own_pid: ffi::Pid) -> Option<FrontWindow> {
    for info in foreign_normal_windows(&copy_cg_window_infos(), own_pid as i64) {
        let Some(element) = ax_window_for_id(info.pid as ffi::Pid, info.window_id) else {
            continue;
        };
        if !is_standard_window(element.0) {
            continue;
        }
        if let Some(window) = snapshot_window(element) {
            return Some(window);
        }
    }
    None
}

/// Reads the on-screen window list into a plain, order-preserving vector so the
/// pure [`foreign_normal_windows`] filter can be unit tested off-device.
fn copy_cg_window_infos() -> Vec<CgWindowInfo> {
    // SAFETY: the option bits are valid `CGWindowListOption` flags and
    // `kCGNullWindowID` requests the whole list; the result follows the create
    // rule and is wrapped in `CfOwned` for release.
    let list = CfOwned(unsafe {
        ffi::CGWindowListCopyWindowInfo(
            ffi::kCGWindowListOptionOnScreenOnly | ffi::kCGWindowListExcludeDesktopElements,
            ffi::kCGNullWindowID,
        )
    });
    if list.0.is_null() {
        return Vec::new();
    }
    let array = list.0 as ffi::CFArrayRef;
    // SAFETY: `array` is a valid CFArray (create rule, checked non-null).
    let count = unsafe { ffi::CFArrayGetCount(array) };

    let mut out = Vec::with_capacity(count.max(0) as usize);
    for index in 0..count {
        // SAFETY: `index` is in `0..count`; the returned dictionary is owned by
        // the array (a `+0` borrow), which outlives this loop.
        let dict = unsafe { ffi::CFArrayGetValueAtIndex(array, index) } as ffi::CFDictionaryRef;
        if dict.is_null() {
            continue;
        }
        // SAFETY: the `kCGWindow*` keys are immortal CoreGraphics CFString
        // constants; every value read is a CFNumber (or absent, handled below).
        let (Some(pid), Some(layer), Some(window_id)) = (unsafe {
            (
                dict_i64(dict, ffi::kCGWindowOwnerPID),
                dict_i64(dict, ffi::kCGWindowLayer),
                dict_i64(dict, ffi::kCGWindowNumber),
            )
        }) else {
            continue;
        };
        out.push(CgWindowInfo {
            pid,
            layer,
            window_id: window_id as u32,
        });
    }
    out
}

/// Reads an integer value out of a CoreGraphics window-info dictionary.
///
/// # Safety
/// `dict` must be a live CFDictionary and `key` a valid CFString key.
unsafe fn dict_i64(dict: ffi::CFDictionaryRef, key: ffi::CFStringRef) -> Option<i64> {
    let value = ffi::CFDictionaryGetValue(dict, key as *const c_void);
    if value.is_null() {
        return None;
    }
    let mut out: i64 = 0;
    // A wider (64-bit) read of the 32-bit ints CG stores is a safe conversion.
    let ok = ffi::CFNumberGetValue(
        value as ffi::CFNumberRef,
        ffi::kCFNumberSInt64Type,
        &mut out as *mut i64 as *mut c_void,
    );
    (ok != 0).then_some(out)
}

/// Finds the AX window element on application `pid` whose `CGWindowID` matches
/// `target_id`, returning an owned (`+1`-retained) reference.
///
/// The mapping walks the app's `AXWindows` and matches each element's id via
/// the same private `_AXUIElementGetWindow` used by [`window_id`]. This is how
/// a CoreGraphics window (which is not an AX element) becomes something Tile
/// can move.
fn ax_window_for_id(pid: ffi::Pid, target_id: u32) -> Option<CfOwned> {
    // SAFETY: `pid` is a real process id; the returned element follows the
    // create rule and is wrapped in `CfOwned` for release.
    let app = CfOwned(unsafe { ffi::AXUIElementCreateApplication(pid) });
    if app.0.is_null() {
        return None;
    }
    let windows = copy_attribute(app.0, "AXWindows")?;
    let array = windows.0 as ffi::CFArrayRef;
    // SAFETY: `array` is the AX `AXWindows` CFArray (create rule, non-null).
    let count = unsafe { ffi::CFArrayGetCount(array) };

    for index in 0..count {
        // SAFETY: `index` is in `0..count`; the element is a `+0` borrow owned
        // by `windows`, valid for the duration of this loop.
        let element = unsafe { ffi::CFArrayGetValueAtIndex(array, index) } as ffi::AXUIElementRef;
        if element.is_null() {
            continue;
        }
        let mut wid: u32 = 0;
        // SAFETY: `element` is a valid AX window element; `wid` is a valid
        // out-pointer. `_AXUIElementGetWindow` is the private SPI declared in
        // `ffi` and already relied upon by `window_id`.
        let err = unsafe { ffi::_AXUIElementGetWindow(element, &mut wid) };
        if err == ffi::kAXErrorSuccess && wid == target_id {
            // The array only lends us the element (`+0`); retain it so it
            // outlives `windows` when we hand it back inside `CfOwned`.
            // SAFETY: `element` is a live CF object; `CFRetain` adds the `+1`
            // that `CfOwned` will balance on drop.
            let retained = unsafe { ffi::CFRetain(element) };
            return Some(CfOwned(retained));
        }
    }
    None
}

// ---------------------------------------------------------------------------
// Window backend
// ---------------------------------------------------------------------------

/// Per-element AX timeout used while animating, in seconds.
///
/// Comfortably longer than the interval between animation frames, so a merely
/// busy app is not cut off mid-move, but far below the six-second system
/// default — which would otherwise let one wedged application stall the whole
/// frame loop, and with it the hotkey worker thread.
const ANIMATION_MESSAGING_TIMEOUT: f32 = 0.2;

/// Per-element AX timeout used while *preparing* an animation.
///
/// Deliberately much longer than [`ANIMATION_MESSAGING_TIMEOUT`]. Setup has to
/// take a window out of full screen, and macOS animates that transition over
/// the better part of a second, so the frame-loop timeout would abort a
/// perfectly healthy exit. Still far below the six-second system default, so a
/// genuinely wedged app cannot hold the worker thread for that long.
const SETUP_MESSAGING_TIMEOUT: f32 = 2.0;

/// Chrome and other applications can transiently accept or partially apply an
/// AX frame update, so final frames receive a few quick verified attempts.
const FRAME_SETTLE_ATTEMPTS: usize = 3;
const FRAME_SETTLE_DELAY: Duration = Duration::from_millis(20);

/// Leaves the native full-screen and minimized states, which silently swallow
/// position and size changes.
fn leave_fullscreen_and_minimized(element: ffi::AXUIElementRef) -> Result<()> {
    if copy_bool(element, "AXFullScreen") == Some(true) {
        let err = set_bool(element, "AXFullScreen", false);
        if err != ffi::kAXErrorSuccess {
            return Err(map_ax_error("exit full screen", err));
        }
    }
    if copy_bool(element, "AXMinimized") == Some(true) {
        let err = set_bool(element, "AXMinimized", false);
        if err != ffi::kAXErrorSuccess {
            return Err(map_ax_error("restore minimized window", err));
        }
    }
    Ok(())
}

/// Applies a frame and verifies the frame Chrome (and other AX clients) report.
///
/// macOS may transiently accept or partially apply an AX position/size update
/// while an app is moving between displays. The leading size write is still
/// required to avoid display clamping; retries cover the separate case where
/// the target app has not settled its native frame yet.
fn apply_frame_and_readback(
    element: ffi::AXUIElementRef,
    target: Rect,
    operation: &str,
) -> Result<Rect> {
    let size = CGSize {
        width: target.width,
        height: target.height,
    };
    let position = CGPoint {
        x: target.x,
        y: target.y,
    };

    for attempt in 0..FRAME_SETTLE_ATTEMPTS {
        let mut retry = false;
        for (context, err) in [
            ("set size", set_size(element, size)),
            ("set position", set_position(element, position)),
            ("set size", set_size(element, size)),
        ] {
            if err == ffi::kAXErrorAPIDisabled {
                return Err(map_ax_error(context, err));
            }
            if err != ffi::kAXErrorSuccess {
                retry = true;
                log::debug!(
                    "macOS AX {context} returned {err} on frame attempt {}",
                    attempt + 1
                );
            }
        }

        let actual = match (
            copy_point(element, "AXPosition"),
            copy_size(element, "AXSize"),
        ) {
            (Some(p), Some(s)) => Some(Rect::new(p.x, p.y, s.width, s.height)),
            _ => {
                retry = true;
                None
            }
        };

        if let Some(actual) = actual {
            if (!retry && actual.approx_eq(&target, 2.0)) || attempt + 1 == FRAME_SETTLE_ATTEMPTS {
                return Ok(actual);
            }
        } else if attempt + 1 == FRAME_SETTLE_ATTEMPTS {
            return Err(PlatformError::os(
                operation,
                "the application did not report where the window ended up",
            ));
        }
        thread::sleep(FRAME_SETTLE_DELAY);
    }

    unreachable!("frame attempts always return")
}

pub struct MacWindowBackend;

impl MacWindowBackend {
    pub fn new() -> Result<Self> {
        Ok(Self)
    }
}

impl WindowBackend for MacWindowBackend {
    fn focused_window(&self) -> Result<Option<WindowSnapshot>> {
        Ok(front_window()?.map(|w| WindowSnapshot {
            id: w.id,
            frame: w.frame,
        }))
    }

    fn screens(&self) -> Result<Vec<Screen>> {
        let screens = enumerate_screens();
        if screens.is_empty() {
            return Err(PlatformError::os(
                "screens",
                "no displays reported by NSScreen",
            ));
        }
        Ok(screens)
    }

    fn set_window_frame(&self, id: WindowId, target: Rect) -> Result<Rect> {
        let Some(front) = front_window()? else {
            return Err(PlatformError::NoFocusedWindow);
        };
        // The engine plans against the focused window and applies immediately.
        // If focus moved to a different window in between, refuse rather than
        // resize the wrong one.
        if front.id != id {
            return Err(PlatformError::os(
                "set_window_frame",
                "the focused window changed before the move could be applied",
            ));
        }
        let element = front.element.0;
        let _enhanced_ui = EnhancedUiGuard::for_window(element)?;

        leave_fullscreen_and_minimized(element)?;

        // The AX size/position/size dance (see AccessibilityElement.swift):
        // macOS clamps a window's size to whatever display it currently
        // overlaps. Setting size first shrinks it to fit the *old* display,
        // then setting position moves it to the target display, then setting
        // size again grows it to the intended size now that it fits there.
        // Transient per-call errors and a stale read-back are retried by the
        // helper; persistent failures are reflected in the final read-back.
        apply_frame_and_readback(element, target, "set_window_frame")
    }

    fn begin_animation(&self, id: WindowId) -> Result<Option<Box<dyn AnimationSession>>> {
        // Resolve the window once. This is the expensive part of an AX move —
        // it walks the system-wide element or the CoreGraphics window list —
        // and doing it per frame would dominate the cost of the animation.
        // Holding the element for the run of the animation is also what makes
        // every frame apply to this one window even if focus wanders
        // mid-flight: `finish` drives the final frame through the same
        // retained element, so nothing in the animation re-checks focus once
        // it has started.
        let Some(front) = front_window()? else {
            return Err(PlatformError::NoFocusedWindow);
        };
        if front.id != id {
            return Err(PlatformError::os(
                "begin_animation",
                "the focused window changed before the move could be applied",
            ));
        }
        let enhanced_ui = EnhancedUiGuard::for_window(front.element.0)?;

        // Bound the setup calls before making any of them. Leaving full screen
        // or un-minimizing is a synchronous round trip into the target app, so
        // on the system default this could block the worker thread for six
        // seconds before the animation had drawn a single frame.
        //
        // SAFETY: `front.element.0` is a live AX element owned by `front`,
        // which the session below takes ownership of, so it outlives every use
        // of this timeout. The call only sets a per-element property.
        unsafe {
            ffi::AXUIElementSetMessagingTimeout(front.element.0, SETUP_MESSAGING_TIMEOUT);
        }

        if let Err(err) = leave_fullscreen_and_minimized(front.element.0) {
            // Put the element back on the system default before bailing. The
            // session that would otherwise do this on drop is never
            // constructed on this path, so without it the element would be
            // released still carrying an animation-tuned timeout.
            //
            // SAFETY: as above — the element is still live and owned by
            // `front`, which has not been dropped yet.
            unsafe {
                ffi::AXUIElementSetMessagingTimeout(front.element.0, 0.0);
            }
            return Err(err);
        }

        // Tighten to the frame-loop budget now the slow part is done.
        // SAFETY: as above — the element is still live and owned by `front`.
        unsafe {
            ffi::AXUIElementSetMessagingTimeout(front.element.0, ANIMATION_MESSAGING_TIMEOUT);
        }

        Ok(Some(Box::new(MacAnimationSession {
            element: front.element,
            enhanced_ui,
        })))
    }

    fn permission_status(&self, prompt: bool) -> Result<PermissionStatus> {
        // NOTE: the system permission prompt shown by
        // `AXIsProcessTrustedWithOptions` is only presented for a running,
        // bundled app and should be triggered from the main thread.
        let trusted = if prompt {
            // Build `{ kAXTrustedCheckOptionPrompt: true }`.
            // SAFETY: `kAXTrustedCheckOptionPrompt` is an immortal CFString
            // constant; `wrap_under_get_rule` adds a balanced retain.
            let key = unsafe {
                CFString::wrap_under_get_rule(
                    ffi::kAXTrustedCheckOptionPrompt as core_foundation::string::CFStringRef,
                )
            };
            let options = CFDictionary::from_CFType_pairs(&[(
                key.as_CFType(),
                CFBoolean::true_value().as_CFType(),
            )]);
            // SAFETY: the dictionary outlives the call; its pointer is a valid
            // CFDictionaryRef.
            unsafe {
                ffi::AXIsProcessTrustedWithOptions(
                    options.as_concrete_TypeRef() as ffi::CFDictionaryRef
                )
            }
        } else {
            // SAFETY: no arguments.
            unsafe { ffi::AXIsProcessTrusted() }
        };

        Ok(if trusted != 0 {
            PermissionStatus::Granted
        } else {
            PermissionStatus::Denied
        })
    }
}

/// An in-progress animation on one macOS window.
///
/// Owns the retained `AXUIElement`, so a frame costs two AX calls instead of
/// the element lookup plus three that a full `set_window_frame` would.
struct MacAnimationSession {
    element: CfOwned,
    enhanced_ui: Option<EnhancedUiGuard<AxEnhancedUiAccess>>,
}

impl AnimationSession for MacAnimationSession {
    fn set_intermediate_frame(&mut self, target: Rect) -> Result<()> {
        let element = self.element.0;

        // Position then size, dropping the leading `set_size` of the
        // size/position/size dance. That extra call exists only to beat the
        // window server's clamp-to-current-display when a window moves to
        // another screen: it shrinks the window so it fits the destination
        // before the position change. Two things make it unnecessary here.
        // The animation walks the window across the gap in small steps rather
        // than teleporting it, and the *final* frame goes through `finish`,
        // which still does the whole dance — so even if an
        // intermediate frame is clamped short, the window is corrected before
        // the animation ends. Halving the AX round-trips per frame matters
        // because each one is synchronous IPC into the target app.
        //
        // As in `set_window_frame`, only `kAXErrorAPIDisabled` (Accessibility
        // permission revoked mid-flight) is fatal. Everything else — including
        // the timeout of a briefly busy app — is tolerated, so a single slow
        // frame drops rather than aborting the whole animation.
        for (context, err) in [
            (
                "set position",
                set_position(
                    element,
                    CGPoint {
                        x: target.x,
                        y: target.y,
                    },
                ),
            ),
            (
                "set size",
                set_size(
                    element,
                    CGSize {
                        width: target.width,
                        height: target.height,
                    },
                ),
            ),
        ] {
            if err == ffi::kAXErrorAPIDisabled {
                return Err(map_ax_error(context, err));
            }
        }
        Ok(())
    }

    fn finish(&mut self, target: Rect) -> Result<Rect> {
        let element = self.element.0;

        // Put the element back on the system default timeout first. The short
        // animation timeout exists so one busy app cannot stall the frame
        // loop, and dropping a frame there is harmless. This frame is not
        // droppable: timing it out would leave the window on its last
        // intermediate rectangle while the read-back quietly reported the
        // target as achieved.
        //
        // SAFETY: `element` is the live, retained AX element this session owns;
        // `0.0` is the documented "use the global default" value.
        unsafe {
            ffi::AXUIElementSetMessagingTimeout(element, 0.0);
        }

        // Re-check the native state, mirroring the Windows session. The
        // restore ran when the session was opened, but a window can enter full
        // screen or be minimized *during* the animation — by the app itself,
        // or by a system shortcut Tile does not swallow — and both states
        // silently swallow position and size changes. Without this the final
        // move would do nothing and the read-back would report the unchanged
        // frame as the result. Done after the timeout reset above, since
        // leaving full screen is slow.
        leave_fullscreen_and_minimized(element)?;
        if let Some(guard) = &mut self.enhanced_ui {
            guard.ensure_disabled()?;
        }

        // The full size/position/size dance, exactly as `set_window_frame`
        // does it: macOS clamps a window's size to whatever display it
        // currently overlaps, so the leading `set_size` shrinks it to fit the
        // old display, the position change moves it to the target display, and
        // the second `set_size` grows it once it fits there. Intermediate
        // frames can skip that; the frame that has to stick cannot.
        //
        // Unlike `set_window_frame` this never consults the focused window: the
        // session already holds the element it has been driving, so a click on
        // another window mid-animation cannot make the final frame fail.
        let actual = apply_frame_and_readback(element, target, "finish");
        drop(self.enhanced_ui.take());
        actual
    }

    fn current_frame(&self) -> Result<Rect> {
        match (
            copy_point(self.element.0, "AXPosition"),
            copy_size(self.element.0, "AXSize"),
        ) {
            (Some(p), Some(s)) => Ok(Rect::new(p.x, p.y, s.width, s.height)),
            _ => Err(PlatformError::os(
                "current_frame",
                "the application did not report the window's frame",
            )),
        }
    }
}

impl Drop for MacAnimationSession {
    fn drop(&mut self) {
        // Put the element back on the system default timeout before releasing
        // it. The element itself is about to go away, but the same window can
        // be resolved again by the next action, and an AX timeout tuned for a
        // 20 ms animation frame is far too tight for a one-shot move into a
        // busy app.
        //
        // SAFETY: `self.element.0` is still live — `CfOwned`'s own `Drop` runs
        // after this one — and `0.0` is the documented "use the global
        // default" value.
        unsafe {
            ffi::AXUIElementSetMessagingTimeout(self.element.0, 0.0);
        }
        drop(self.enhanced_ui.take());
    }
}

#[cfg(test)]
mod enhanced_ui_tests {
    use super::{EnhancedUiAccess, EnhancedUiGuard, PlatformError, Result};
    use std::cell::{Cell, RefCell};
    use std::rc::Rc;

    #[derive(Clone)]
    struct FakeAccess {
        state: Rc<Cell<Option<bool>>>,
        writes: Rc<RefCell<Vec<bool>>>,
    }

    impl FakeAccess {
        fn new(state: Option<bool>) -> Self {
            Self {
                state: Rc::new(Cell::new(state)),
                writes: Rc::new(RefCell::new(Vec::new())),
            }
        }
    }

    impl EnhancedUiAccess for FakeAccess {
        fn current(&self) -> Option<bool> {
            self.state.get()
        }

        fn set(&self, value: bool) -> Result<bool> {
            self.state.set(Some(value));
            self.writes.borrow_mut().push(value);
            Ok(true)
        }
    }

    #[test]
    fn unavailable_and_disabled_enhanced_ui_are_initially_untouched() {
        for initial in [None, Some(false)] {
            let access = FakeAccess::new(initial);
            let writes = access.writes.clone();
            drop(EnhancedUiGuard::new(access).unwrap());
            assert!(writes.borrow().is_empty());
        }
    }

    #[test]
    fn enabled_enhanced_ui_is_suppressed_and_restored() {
        let access = FakeAccess::new(Some(true));
        let state = access.state.clone();
        let writes = access.writes.clone();
        let guard = EnhancedUiGuard::new(access).unwrap();
        assert_eq!(state.get(), Some(false));
        drop(guard);
        assert_eq!(state.get(), Some(true));
        assert_eq!(*writes.borrow(), [false, true]);
    }

    #[test]
    fn enhanced_ui_is_suppressed_again_before_the_final_frame() {
        let access = FakeAccess::new(Some(false));
        let state = access.state.clone();
        let writes = access.writes.clone();
        let mut guard = EnhancedUiGuard::new(access).unwrap();
        state.set(Some(true));
        guard.ensure_disabled().unwrap();
        drop(guard);
        assert_eq!(state.get(), Some(true));
        assert_eq!(*writes.borrow(), [false, true]);
    }

    #[test]
    fn early_operation_error_still_restores_enhanced_ui_on_scope_exit() {
        let access = FakeAccess::new(Some(true));
        let state = access.state.clone();
        let writes = access.writes.clone();
        let operation: Result<()> = {
            let _guard = EnhancedUiGuard::new(access).unwrap();
            Err(PlatformError::os("test operation", "injected failure"))
        };
        assert!(operation.is_err());
        assert_eq!(state.get(), Some(true));
        assert_eq!(*writes.borrow(), [false, true]);
    }
}

/// Enumerates displays via `NSScreen`, converting each frame from AppKit's
/// bottom-left space into Tile's top-left space.
fn enumerate_screens() -> Vec<Screen> {
    // The `deviceDescription` key identifying a display's `CGDirectDisplayID`.
    // A CFString is toll-free bridged to `NSString`, so we can pass it straight
    // to `-objectForKey:`.
    let screen_number_key = CFString::new("NSScreenNumber");

    // SAFETY: everything below is Objective-C messaging through `objc2`. All
    // receivers are null-checked before use; `frame`/`visibleFrame` return
    // `NSRect` (which implements `Encode`, so struct-return dispatch is
    // correct); primitive returns are annotated with their C types. The whole
    // block runs inside an autorelease pool so the autoreleased `+[NSScreen
    // screens]` array and its members are cleaned up. `NSScreen` is a
    // main-thread class; callers should invoke `screens()` on the main thread.
    objc2::rc::autoreleasepool(|_pool| unsafe {
        let ns_screen = class!(NSScreen);
        let array: *mut AnyObject = msg_send![ns_screen, screens];
        if array.is_null() {
            return Vec::new();
        }
        let count: usize = msg_send![array, count];
        if count == 0 {
            return Vec::new();
        }

        // The flip pivots around screens[0] (the menu-bar display), whose
        // AppKit frame has origin (0, 0); its maxY equals its height.
        let primary: *mut AnyObject = msg_send![array, objectAtIndex: 0usize];
        if primary.is_null() {
            return Vec::new();
        }
        let primary_frame: NSRect = msg_send![primary, frame];
        let primary_height = primary_frame.origin.y + primary_frame.size.height;

        let mut result = Vec::with_capacity(count);
        for index in 0..count {
            let screen: *mut AnyObject = msg_send![array, objectAtIndex: index];
            if screen.is_null() {
                continue;
            }

            let frame: NSRect = msg_send![screen, frame];
            let visible: NSRect = msg_send![screen, visibleFrame];
            let scale_factor: f64 = msg_send![screen, backingScaleFactor];

            let frame_rect = flip_rect(
                Rect::new(
                    frame.origin.x,
                    frame.origin.y,
                    frame.size.width,
                    frame.size.height,
                ),
                primary_height,
            );
            let work_area = flip_rect(
                Rect::new(
                    visible.origin.x,
                    visible.origin.y,
                    visible.size.width,
                    visible.size.height,
                ),
                primary_height,
            );

            // Prefer the stable NSScreenNumber (survives display relayout);
            // fall back to the enumeration index.
            let id = display_number(screen, &screen_number_key)
                .map(|n| n.to_string())
                .unwrap_or_else(|| index.to_string());

            result.push(Screen {
                id,
                frame: frame_rect,
                work_area,
                scale_factor,
                is_primary: index == 0,
            });
        }
        result
    })
}

/// Reads `screen.deviceDescription[NSScreenNumber]` as a `CGDirectDisplayID`.
///
/// # Safety
/// `screen` must be a live `NSScreen` object.
unsafe fn display_number(screen: *mut AnyObject, key: &CFString) -> Option<u32> {
    let description: *mut AnyObject = msg_send![screen, deviceDescription];
    if description.is_null() {
        return None;
    }
    // A CFStringRef is a valid `id` for `-objectForKey:` (toll-free bridged).
    let key_obj = key.as_concrete_TypeRef() as *mut AnyObject;
    let number: *mut AnyObject = msg_send![description, objectForKey: key_obj];
    if number.is_null() {
        return None;
    }
    let value: u32 = msg_send![number, unsignedIntValue];
    Some(value)
}

// ---------------------------------------------------------------------------
// Hotkey backend
// ---------------------------------------------------------------------------

/// Shared state reachable from the Carbon event-handler callback.
///
/// Both fields are behind a `Mutex` so the struct is `Sync` (a raw `Sender` is
/// `!Sync`), which lets `Arc<HotkeyState>` — and therefore [`MacHotkeyBackend`]
/// — be `Send` as the trait requires. Contention is negligible: the callback
/// runs on the main thread and `apply` only briefly touches the map.
struct HotkeyState {
    sender: Mutex<Sender<WindowAction>>,
    actions: Mutex<HashMap<u32, WindowAction>>,
}

pub struct MacHotkeyBackend {
    state: Arc<HotkeyState>,
    /// Installed `EventHandlerRef`, stored as an address so the struct stays
    /// `Send`. `0` means "not installed".
    handler: usize,
    /// Live `EventHotKeyRef`s, as addresses, for later unregistration.
    registered: Vec<usize>,
    next_id: u32,
}

// SAFETY: the only non-`Send` conceptual data are the Carbon handles, stored as
// plain `usize` addresses; the shared `Arc<HotkeyState>` is `Send + Sync`. The
// Carbon APIs that consume these handles must be called on the main thread,
// which is a documented caller requirement, not a memory-safety property.
unsafe impl Send for MacHotkeyBackend {}

impl MacHotkeyBackend {
    pub fn new(events: Sender<WindowAction>) -> Result<Self> {
        Ok(Self {
            state: Arc::new(HotkeyState {
                sender: Mutex::new(events),
                actions: Mutex::new(HashMap::new()),
            }),
            handler: 0,
            registered: Vec::new(),
            next_id: 1,
        })
    }

    /// Installs the shared keyboard event handler once, lazily.
    ///
    /// Must run on the main thread (which owns the Carbon run loop).
    fn ensure_handler_installed(&mut self) -> Result<()> {
        if self.handler != 0 {
            return Ok(());
        }
        let spec = ffi::EventTypeSpec {
            eventClass: ffi::kEventClassKeyboard,
            eventKind: ffi::kEventHotKeyPressed,
        };
        let mut handler_ref: ffi::EventHandlerRef = std::ptr::null_mut();
        // The callback receives a pointer to our `HotkeyState`, which stays
        // alive for as long as `self` (and is removed before drop).
        let user_data = Arc::as_ptr(&self.state) as *mut c_void;
        // SAFETY: `GetApplicationEventTarget` returns the process-wide target;
        // `hotkey_handler` matches `EventHandlerUPP`; `spec`/`handler_ref` are
        // valid pointers; `user_data` outlives the handler.
        let status = unsafe {
            ffi::InstallEventHandler(
                ffi::GetApplicationEventTarget(),
                hotkey_handler,
                1,
                &spec,
                user_data,
                &mut handler_ref,
            )
        };
        if status != ffi::noErr {
            return Err(PlatformError::os(
                "InstallEventHandler",
                format!("OSStatus {status}"),
            ));
        }
        self.handler = handler_ref as usize;
        Ok(())
    }
}

impl HotkeyBackend for MacHotkeyBackend {
    fn apply(&mut self, bindings: &[(Hotkey, WindowAction)]) -> Result<Vec<HotkeyFailure>> {
        self.ensure_handler_installed()?;

        // Drop everything previously held.
        for hotkey_ref in self.registered.drain(..) {
            // SAFETY: each address was returned by `RegisterEventHotKey` and is
            // unregistered at most once (we drain the vec).
            unsafe { ffi::UnregisterEventHotKey(hotkey_ref as ffi::EventHotKeyRef) };
        }
        if let Ok(mut map) = self.state.actions.lock() {
            map.clear();
        }

        let mut failures = Vec::new();
        for (hotkey, action) in bindings {
            // A few keys (F21-F24) have no Carbon virtual key code at all.
            // Report them as a per-binding failure instead of registering some
            // other physical key.
            let Some(code) = carbon_key_code(hotkey.key) else {
                failures.push(HotkeyFailure {
                    hotkey: *hotkey,
                    action: *action,
                    reason: format!("macOS has no key code for {}", hotkey.key.label()),
                });
                continue;
            };
            let modifiers = carbon_modifiers(hotkey.modifiers);
            let id = self.next_id;
            self.next_id = self.next_id.wrapping_add(1);

            let hotkey_id = ffi::EventHotKeyID {
                signature: HOTKEY_SIGNATURE,
                id,
            };
            let mut hotkey_ref: ffi::EventHotKeyRef = std::ptr::null_mut();
            // SAFETY: all scalar arguments; `hotkey_ref` is a valid out-pointer;
            // the target is the process-wide event target.
            let status = unsafe {
                ffi::RegisterEventHotKey(
                    code,
                    modifiers,
                    hotkey_id,
                    ffi::GetApplicationEventTarget(),
                    0,
                    &mut hotkey_ref,
                )
            };

            if status == ffi::noErr && !hotkey_ref.is_null() {
                self.registered.push(hotkey_ref as usize);
                if let Ok(mut map) = self.state.actions.lock() {
                    map.insert(id, *action);
                }
            } else {
                let reason = if status == ffi::eventHotKeyExistsErr {
                    "hotkey is already registered by another application".to_string()
                } else {
                    format!("RegisterEventHotKey failed with OSStatus {status}")
                };
                // One bad binding must not abort the rest.
                failures.push(HotkeyFailure {
                    hotkey: *hotkey,
                    action: *action,
                    reason,
                });
            }
        }

        Ok(failures)
    }

    fn shutdown(&mut self) {
        for hotkey_ref in self.registered.drain(..) {
            // SAFETY: see `apply`; each handle is unregistered at most once.
            unsafe { ffi::UnregisterEventHotKey(hotkey_ref as ffi::EventHotKeyRef) };
        }
        if self.handler != 0 {
            // SAFETY: `self.handler` is a live handler ref we installed; removed
            // exactly once (guarded by the reset below).
            unsafe { ffi::RemoveEventHandler(self.handler as ffi::EventHandlerRef) };
            self.handler = 0;
        }
        if let Ok(mut map) = self.state.actions.lock() {
            map.clear();
        }
    }
}

impl Drop for MacHotkeyBackend {
    fn drop(&mut self) {
        // Remove the handler before the `Arc<HotkeyState>` it points at is
        // released, so no in-flight callback can observe freed state.
        self.shutdown();
    }
}

/// Carbon event-handler callback for `kEventHotKeyPressed`.
extern "C" fn hotkey_handler(
    _call: ffi::EventHandlerCallRef,
    event: ffi::EventRef,
    user_data: *mut c_void,
) -> ffi::OSStatus {
    if user_data.is_null() || event.is_null() {
        return ffi::noErr;
    }
    // SAFETY: `user_data` is the `Arc<HotkeyState>` pointer passed to
    // `InstallEventHandler`. The handler is removed in `shutdown`/`Drop` before
    // that `Arc` is released, so the reference is valid for every callback.
    let state = unsafe { &*(user_data as *const HotkeyState) };

    let mut hotkey_id = ffi::EventHotKeyID {
        signature: 0,
        id: 0,
    };
    // SAFETY: `event` is a live EventRef; we request the direct-object
    // parameter as an `EventHotKeyID` into a correctly sized buffer.
    let status = unsafe {
        ffi::GetEventParameter(
            event,
            ffi::kEventParamDirectObject,
            ffi::typeEventHotKeyID,
            std::ptr::null_mut(),
            std::mem::size_of::<ffi::EventHotKeyID>(),
            std::ptr::null_mut(),
            &mut hotkey_id as *mut ffi::EventHotKeyID as *mut c_void,
        )
    };
    if status != ffi::noErr {
        return ffi::noErr;
    }

    let action = state
        .actions
        .lock()
        .ok()
        .and_then(|map| map.get(&hotkey_id.id).copied());
    if let Some(action) = action {
        if let Ok(sender) = state.sender.lock() {
            let _ = sender.send(action);
        }
    }
    ffi::noErr
}
