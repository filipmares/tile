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
use crate::update::UpdateStatus;

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

/// Serializable update state consumed by the tray-adjacent settings UI.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(
    tag = "status",
    rename_all = "kebab-case",
    rename_all_fields = "camelCase"
)]
pub enum UpdateStatusDto {
    Unavailable,
    Idle,
    Checking,
    Current,
    Available {
        version: String,
        notes: Option<String>,
        date: Option<String>,
    },
    Downloading {
        version: String,
        downloaded_bytes: u64,
        total_bytes: Option<u64>,
    },
    ReadyToRelaunch {
        version: String,
    },
    Error {
        message: String,
    },
}

impl From<UpdateStatus> for UpdateStatusDto {
    fn from(status: UpdateStatus) -> Self {
        match status {
            UpdateStatus::Unavailable => Self::Unavailable,
            UpdateStatus::Idle => Self::Idle,
            UpdateStatus::Checking => Self::Checking,
            UpdateStatus::Current => Self::Current,
            UpdateStatus::Available {
                version,
                notes,
                date,
            } => Self::Available {
                version,
                notes,
                date,
            },
            UpdateStatus::Downloading {
                version,
                downloaded_bytes,
                total_bytes,
            } => Self::Downloading {
                version,
                downloaded_bytes,
                total_bytes,
            },
            UpdateStatus::ReadyToRelaunch { version } => Self::ReadyToRelaunch { version },
            UpdateStatus::Error { message } => Self::Error { message },
        }
    }
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

    #[test]
    fn update_status_serializes_the_ui_contract() {
        let available = UpdateStatusDto::from(UpdateStatus::Available {
            version: "1.2.3".into(),
            notes: Some("What changed".into()),
            date: None,
        });
        assert_eq!(
            serde_json::to_string(&available).unwrap(),
            r#"{"status":"available","version":"1.2.3","notes":"What changed","date":null}"#
        );

        let progress = UpdateStatusDto::from(UpdateStatus::Downloading {
            version: "1.2.3".into(),
            downloaded_bytes: 512,
            total_bytes: Some(1024),
        });
        assert_eq!(
            serde_json::to_string(&progress).unwrap(),
            r#"{"status":"downloading","version":"1.2.3","downloadedBytes":512,"totalBytes":1024}"#
        );
    }
}
