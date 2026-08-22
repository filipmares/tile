//! Serializable mirrors of the `tile-platform` types that cross the Tauri
//! bridge.
//!
//! `PermissionStatus` and `HotkeyFailure` live in `tile-platform` and do not
//! derive serde, and that crate is owned by other agents, so we mirror them
//! here rather than editing it.

use serde::{Deserialize, Serialize};
use tile_core::{Hotkey, WindowAction};
use tile_platform::{HotkeyFailure, PermissionStatus};

use crate::build_kind::BuildKind;

/// Serializable form of [`BuildKind`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum BuildKindDto {
    Development,
    Installed,
}

impl From<BuildKind> for BuildKindDto {
    fn from(kind: BuildKind) -> Self {
        match kind {
            BuildKind::Development => BuildKindDto::Development,
            BuildKind::Installed => BuildKindDto::Installed,
        }
    }
}

/// What the settings UI needs to know about the binary it is talking to: which
/// kind of build it is, and where that build keeps its config.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BuildInfoDto {
    pub kind: BuildKindDto,
    /// `None` when no config directory could be resolved, in which case
    /// settings live in memory only for this run.
    pub config_dir: Option<String>,
}

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

#[cfg(test)]
mod tests {
    use super::*;

    /// The settings UI branches on these exact strings; they are the wire
    /// contract with `ui/src/types.ts`.
    #[test]
    fn build_info_serializes_the_shape_the_ui_expects() {
        let json = serde_json::to_string(&BuildInfoDto {
            kind: BuildKind::Development.into(),
            config_dir: Some("/tmp/tile".to_string()),
        })
        .unwrap();
        assert_eq!(json, r#"{"kind":"development","configDir":"/tmp/tile"}"#);
    }

    #[test]
    fn an_installed_build_serializes_as_installed() {
        let json = serde_json::to_string(&BuildInfoDto {
            kind: BuildKind::Installed.into(),
            config_dir: None,
        })
        .unwrap();
        assert_eq!(json, r#"{"kind":"installed","configDir":null}"#);
    }
}
