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

use core_foundation::base::TCFType;
use core_foundation::boolean::CFBoolean;
use core_foundation::dictionary::CFDictionary;
use core_foundation::string::CFString;

use objc2::runtime::AnyObject;
use objc2::{class, msg_send};
use objc2_foundation::NSRect;

use tile_core::{Hotkey, Rect, Screen, WindowAction, WindowId, WindowSnapshot};

use crate::{HotkeyBackend, HotkeyFailure, PermissionStatus, PlatformError, Result, WindowBackend};

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
    pub type AXUIElementRef = *const c_void;
    pub type AXValueRef = *const c_void;
    pub type AXError = i32;
    /// `DarwinBoolean` / `Boolean`: a single unsigned byte, non-zero == true.
    pub type Boolean = u8;
    pub type CFHashCode = usize;

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
        pub fn CFHash(cf: CFTypeRef) -> CFHashCode;
        pub fn CFBooleanGetValue(boolean: CFTypeRef) -> Boolean;
        pub static kCFBooleanTrue: CFTypeRef;
        pub static kCFBooleanFalse: CFTypeRef;
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

/// Fetches the frontmost focused window through the system-wide AX element.
///
/// Returns `Ok(None)` (never an error) when nothing suitable is focused or the
/// focused window is not a standard window (a sheet, popover, dialog, ...).
/// Returns `Err(PermissionDenied)` when Accessibility permission is missing.
fn front_window() -> Result<Option<FrontWindow>> {
    // SAFETY: no arguments; returns a bool byte.
    if unsafe { ffi::AXIsProcessTrusted() } == 0 {
        return Err(PlatformError::PermissionDenied(
            "Accessibility permission not granted".to_string(),
        ));
    }

    // SAFETY: creates a `+1` system-wide element; wrapped for release.
    let system_wide = CfOwned(unsafe { ffi::AXUIElementCreateSystemWide() });
    if system_wide.0.is_null() {
        return Ok(None);
    }

    let Some(app) = copy_attribute(system_wide.0, "AXFocusedApplication") else {
        return Ok(None);
    };
    let Some(window) = copy_attribute(app.0, "AXFocusedWindow") else {
        return Ok(None);
    };

    // Filter out sheets/popovers/dialogs: only standard windows are movable.
    // A window that does not report a subrole at all is treated as standard,
    // matching Rectangle which only *excludes* known non-standard subroles.
    if let Some(subrole) = copy_string(window.0, "AXSubrole") {
        if subrole != "AXStandardWindow" {
            return Ok(None);
        }
    }

    let Some(position) = copy_point(window.0, "AXPosition") else {
        return Ok(None);
    };
    let Some(size) = copy_size(window.0, "AXSize") else {
        return Ok(None);
    };

    let id = window_id(window.0);
    // AX is already top-left-origin, so no coordinate flip for the window.
    let frame = Rect::new(position.x, position.y, size.width, size.height);

    Ok(Some(FrontWindow {
        element: window,
        id,
        frame,
    }))
}

// ---------------------------------------------------------------------------
// Window backend
// ---------------------------------------------------------------------------

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

        // A window in native full-screen (or minimized) ignores position/size
        // changes, so leave those states first.
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

        let size = CGSize {
            width: target.width,
            height: target.height,
        };
        let position = CGPoint {
            x: target.x,
            y: target.y,
        };

        // The AX size/position/size dance (see AccessibilityElement.swift):
        // macOS clamps a window's size to whatever display it currently
        // overlaps. Setting size first shrinks it to fit the *old* display,
        // then setting position moves it to the target display, then setting
        // size again grows it to the intended size now that it fits there.
        // Only `kAXErrorAPIDisabled` (permission lost mid-flight) is fatal;
        // other per-call errors are tolerated and reflected in the read-back.
        for (context, err) in [
            ("set size", set_size(element, size)),
            ("set position", set_position(element, position)),
            ("set size", set_size(element, size)),
        ] {
            if err == ffi::kAXErrorAPIDisabled {
                return Err(map_ax_error(context, err));
            }
        }

        // Return the frame the window actually ended up with — apps such as
        // Terminal and iTerm snap to character-cell increments, so this can
        // differ from `target`.
        let actual = match (
            copy_point(element, "AXPosition"),
            copy_size(element, "AXSize"),
        ) {
            (Some(p), Some(s)) => Rect::new(p.x, p.y, s.width, s.height),
            _ => target,
        };
        Ok(actual)
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
            let code = carbon_key_code(hotkey.key);
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
