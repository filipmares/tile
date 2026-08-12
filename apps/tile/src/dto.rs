//! Serializable mirrors of the `tile-platform` types that cross the Tauri
//! bridge.
//!
//! `PermissionStatus` and `HotkeyFailure` live in `tile-platform` and do not
//! derive serde, and that crate is owned by other agents, so we mirror them
//! here rather than editing it.

use serde::{Deserialize, Serialize};
use tile_core::{Hotkey, WindowAction};
use tile_platform::{HotkeyFailure, PermissionStatus};

/// Serializable form of [`PermissionStatus`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum PermissionStatusDto {
    Granted,
    Denied,
    NotRequired,
}

impl From<PermissionStatus> for PermissionStatusDto {
    fn from(status: PermissionStatus) -> Self {
        match status {
            PermissionStatus::Granted => PermissionStatusDto::Granted,
            PermissionStatus::Denied => PermissionStatusDto::Denied,
            PermissionStatus::NotRequired => PermissionStatusDto::NotRequired,
        }
    }
}

/// Serializable form of [`HotkeyFailure`].
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct HotkeyFailureDto {
    pub hotkey: Hotkey,
    pub action: WindowAction,
    pub reason: String,
}

impl From<&HotkeyFailure> for HotkeyFailureDto {
    fn from(failure: &HotkeyFailure) -> Self {
        Self {
            hotkey: failure.hotkey,
            action: failure.action,
            reason: failure.reason.clone(),
        }
    }
}
