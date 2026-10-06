//! The error a settings command hands back to the UI.
//!
//! The settings window must never show a value the app is not actually using,
//! and must never show raw Rust error text either. So a failure crosses the
//! IPC boundary as a small, stable `kind` the UI maps to a plain sentence; the
//! technical detail goes to the log and travels along only for diagnostics.

use serde::Serialize;

/// What went wrong, in terms the UI can explain without parsing text.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum SettingsErrorKind {
    /// The config could not be written to disk. The change was rolled back,
    /// so the app is still running on the previous settings.
    NotSaved,
    /// The OS login item could not be changed to match the request. The
    /// launch-at-login preference was left as it was.
    LoginItem,
    /// The config could not be saved and was rolled back, but the OS login
    /// item had already changed and could not be put back, so it may no
    /// longer match the launch-at-login preference.
    OutOfSync,
    /// The shortcut is already bound to another action, and the request did
    /// not ask to replace it. Nothing was changed.
    ShortcutTaken,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SettingsError {
    pub kind: SettingsErrorKind,
    /// The underlying error, for the log and the developer console only.
    pub detail: String,
}

impl SettingsError {
    pub fn not_saved(detail: impl std::fmt::Display) -> Self {
        Self::new(SettingsErrorKind::NotSaved, detail)
    }

    pub fn login_item(detail: impl std::fmt::Display) -> Self {
        Self::new(SettingsErrorKind::LoginItem, detail)
    }

    pub fn out_of_sync(detail: impl std::fmt::Display) -> Self {
        Self::new(SettingsErrorKind::OutOfSync, detail)
    }

    pub fn shortcut_taken(detail: impl std::fmt::Display) -> Self {
        Self::new(SettingsErrorKind::ShortcutTaken, detail)
    }

    fn new(kind: SettingsErrorKind, detail: impl std::fmt::Display) -> Self {
        let detail = detail.to_string();
        log::error!("settings change failed ({kind:?}): {detail}");
        Self { kind, detail }
    }
}

impl std::fmt::Display for SettingsError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{:?}: {}", self.kind, self.detail)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The UI switches on these exact strings (see `settingsErrorMessage` in
    /// `ui/src/errors.ts`), so renaming a variant must be a deliberate change.
    #[test]
    fn kinds_serialize_to_the_strings_the_ui_maps() {
        let saved = serde_json::to_value(SettingsError::not_saved("disk full")).unwrap();
        assert_eq!(
            saved,
            serde_json::json!({ "kind": "notSaved", "detail": "disk full" })
        );
        let login = serde_json::to_value(SettingsError::login_item("denied")).unwrap();
        assert_eq!(login["kind"], "loginItem");
        let out_of_sync = serde_json::to_value(SettingsError::out_of_sync("both")).unwrap();
        assert_eq!(out_of_sync["kind"], "outOfSync");
        let taken = serde_json::to_value(SettingsError::shortcut_taken("Left Half")).unwrap();
        assert_eq!(taken["kind"], "shortcutTaken");
    }
}
