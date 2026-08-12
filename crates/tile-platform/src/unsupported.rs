//! Fallback backends for platforms Tile does not support.
//!
//! These exist so the workspace still builds (and `cargo test` still runs) on
//! Linux CI runners, rather than failing to compile.

#![allow(dead_code)]

use std::sync::mpsc::Sender;

use tile_core::{Hotkey, Rect, Screen, WindowAction, WindowId, WindowSnapshot};

use crate::{HotkeyBackend, HotkeyFailure, PermissionStatus, PlatformError, Result, WindowBackend};

pub struct UnsupportedWindowBackend;

impl WindowBackend for UnsupportedWindowBackend {
    fn focused_window(&self) -> Result<Option<WindowSnapshot>> {
        Err(PlatformError::Unsupported("window management"))
    }

    fn screens(&self) -> Result<Vec<Screen>> {
        Err(PlatformError::Unsupported("display enumeration"))
    }

    fn set_window_frame(&self, _id: WindowId, _target: Rect) -> Result<Rect> {
        Err(PlatformError::Unsupported("window management"))
    }

    fn permission_status(&self, _prompt: bool) -> Result<PermissionStatus> {
        Ok(PermissionStatus::NotRequired)
    }
}

pub struct UnsupportedHotkeyBackend;

impl HotkeyBackend for UnsupportedHotkeyBackend {
    fn apply(&mut self, _bindings: &[(Hotkey, WindowAction)]) -> Result<Vec<HotkeyFailure>> {
        Err(PlatformError::Unsupported("global hotkeys"))
    }

    fn shutdown(&mut self) {}
}

/// Silences the unused-import warning on supported platforms.
fn _assert_sender_is_used(_: Option<Sender<WindowAction>>) {}
