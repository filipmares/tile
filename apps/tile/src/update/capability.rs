//! Compile-time update eligibility.
//!
//! Installing over a development checkout can replace a user's installed copy,
//! and updating an unsigned macOS bundle invalidates its Accessibility grant.
//! Consequently only the release workflow's explicit `TILE_UPDATER=enabled`
//! opt-in permits installation. Missing or malformed CI input fails toward
//! notification-only for installed builds and no update activity for
//! development builds.
//!
//! `build.rs` records `TILE_UPDATER` as a crate input so changing it cannot
//! silently reuse an artifact compiled with the other capability.

use crate::build_kind::BuildKind;

/// What this binary is permitted to do with an available update.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UpdateCapability {
    /// Release build eligible to install: Windows, or signed macOS.
    Install,
    /// Discovers and reports, links out, and never touches the bundle.
    ///
    /// This is the safe behavior for unsigned macOS release builds.
    NotifyOnly,
    /// Development build: no network and no update UI.
    Disabled,
}

const ENABLED: &str = "enabled";

impl UpdateCapability {
    /// Returns the capability compiled into this binary.
    pub fn detect(build_kind: BuildKind) -> Self {
        Self::from_env_value(build_kind, option_env!("TILE_UPDATER"))
    }

    /// Pure classification of a `TILE_UPDATER` value.
    ///
    /// Only the release workflow's explicit opt-in permits installation.
    pub fn from_env_value(build_kind: BuildKind, raw: Option<&str>) -> Self {
        if build_kind.is_development() {
            return Self::Disabled;
        }

        let Some(value) = raw.map(str::trim) else {
            return Self::NotifyOnly;
        };
        if value.eq_ignore_ascii_case(ENABLED) {
            Self::Install
        } else {
            if !value.is_empty() && !value.eq_ignore_ascii_case("disabled") {
                log::warn!(
                    "unrecognized TILE_UPDATER={value:?}; updates will be notification-only"
                );
            }
            Self::NotifyOnly
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_missing_variable_is_notification_only_for_an_installed_build() {
        assert_eq!(
            UpdateCapability::from_env_value(BuildKind::Installed, None),
            UpdateCapability::NotifyOnly
        );
    }

    #[test]
    fn an_empty_variable_is_notification_only_for_an_installed_build() {
        for raw in ["", "   "] {
            assert_eq!(
                UpdateCapability::from_env_value(BuildKind::Installed, Some(raw)),
                UpdateCapability::NotifyOnly
            );
        }
    }

    #[test]
    fn only_enabled_opts_into_installation() {
        assert_eq!(
            UpdateCapability::from_env_value(BuildKind::Installed, Some("enabled")),
            UpdateCapability::Install
        );
    }

    #[test]
    fn the_value_is_case_insensitive_and_trimmed() {
        for raw in ["ENABLED", "Enabled", "  enabled  ", "\tenabled\n"] {
            assert_eq!(
                UpdateCapability::from_env_value(BuildKind::Installed, Some(raw)),
                UpdateCapability::Install,
                "{raw:?} should enable installation"
            );
        }
    }

    #[test]
    fn disabled_is_spelled_out_without_an_install_path() {
        assert_eq!(
            UpdateCapability::from_env_value(BuildKind::Installed, Some("disabled")),
            UpdateCapability::NotifyOnly
        );
    }

    #[test]
    fn an_unrecognized_value_falls_back_to_notification_only() {
        for raw in ["enable", "install", "release", "1", "true"] {
            assert_eq!(
                UpdateCapability::from_env_value(BuildKind::Installed, Some(raw)),
                UpdateCapability::NotifyOnly,
                "{raw:?} must not enable installation"
            );
        }
    }

    #[test]
    fn a_development_build_is_disabled_when_the_variable_is_missing() {
        assert_eq!(
            UpdateCapability::from_env_value(BuildKind::Development, None),
            UpdateCapability::Disabled
        );
    }

    #[test]
    fn a_development_build_cannot_opt_into_installation() {
        assert_eq!(
            UpdateCapability::from_env_value(BuildKind::Development, Some("enabled")),
            UpdateCapability::Disabled
        );
    }

    #[test]
    fn a_development_build_ignores_unrecognized_values() {
        assert_eq!(
            UpdateCapability::from_env_value(BuildKind::Development, Some("surprise")),
            UpdateCapability::Disabled
        );
    }
}
