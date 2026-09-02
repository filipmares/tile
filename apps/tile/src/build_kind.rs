//! Build provenance: is this binary a local development build or the one users
//! install from a release?
//!
//! `debug_assertions` cannot answer that on its own — `cargo tauri build` run
//! on a developer's machine produces a *release* binary that is still a
//! development build. So the answer is baked in at compile time from the
//! `TILE_BUILD_KIND` environment variable, which only the release workflow
//! sets (`TILE_BUILD_KIND=installed`). Anything else — absent, empty,
//! misspelled — classifies as [`BuildKind::Development`], so the failure mode
//! of a broken CI variable is a release that behaves like a dev build rather
//! than a dev build that quietly writes to the installed app's config and
//! rewrites the OS login item.
//!
//! `build.rs` re-emits the variable with `cargo:rerun-if-env-changed`, which
//! makes it a recorded input of the crate so flipping it always recompiles.

/// Where this binary came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BuildKind {
    /// Built locally (`cargo run`, `cargo tauri dev`, `cargo tauri build`).
    Development,
    /// Built by the release workflow and installed by a user.
    Installed,
}

/// The value the release workflow sets.
const INSTALLED: &str = "installed";

impl BuildKind {
    /// The kind compiled into this binary.
    pub fn detect() -> Self {
        Self::from_env_value(option_env!("TILE_BUILD_KIND"))
    }

    /// Pure classification of a `TILE_BUILD_KIND` value.
    ///
    /// Forgiving about how the value is spelled (CI plumbing adds whitespace
    /// and case), strict about what it means: only `installed` opts in.
    pub fn from_env_value(raw: Option<&str>) -> Self {
        let Some(value) = raw.map(str::trim) else {
            return BuildKind::Development;
        };
        if value.eq_ignore_ascii_case(INSTALLED) {
            BuildKind::Installed
        } else {
            if !value.is_empty() && !value.eq_ignore_ascii_case("development") {
                log::warn!(
                    "unrecognized TILE_BUILD_KIND={value:?}; treating as a development build"
                );
            }
            BuildKind::Development
        }
    }

    pub fn is_development(self) -> bool {
        matches!(self, BuildKind::Development)
    }

    /// Whether this build owns the OS login item. Development builds must not
    /// touch it: running a dev build would otherwise repoint (or remove) the
    /// login item of the copy the user actually installed.
    pub fn manages_autostart(self) -> bool {
        matches!(self, BuildKind::Installed)
    }

    /// Whether this build refuses to run a second time.
    ///
    /// Two Tile processes cannot share a machine. Each installs its own global
    /// keyboard hook, and the OS offers a keystroke to the most recently
    /// installed hook first, which swallows it so the shortcut fires once —
    /// but in whichever process happens to be newest. Windows still move, so
    /// nothing looks broken until a screen that is *waiting* on a shortcut
    /// (the first-run walkthrough) never hears the key it asked for. Two tray
    /// icons and two writers of `config.json` come with it.
    ///
    /// Development builds are exempt, and deliberately so. The single-instance
    /// lock is keyed on the bundle identifier, which a checkout shares with the
    /// installed copy, so enforcing it here would stop a developer running
    /// their build while their own Tile sits in the tray — the very separation
    /// [`Self::project_app_name`] and [`Self::manages_autostart`] exist to
    /// preserve. It would also race `tauri dev`, which restarts the app by
    /// replacing the process.
    pub fn enforces_single_instance(self) -> bool {
        matches!(self, BuildKind::Installed)
    }

    /// Application name handed to `ProjectDirs`, which is what keeps
    /// development config in a sibling directory of the installed one.
    pub fn project_app_name(self) -> &'static str {
        match self {
            BuildKind::Installed => "Tile",
            BuildKind::Development => "Tile-Development",
        }
    }

    /// Tray icon tooltip.
    pub fn tray_tooltip(self) -> &'static str {
        match self {
            BuildKind::Installed => "Tile",
            BuildKind::Development => "Tile (Development)",
        }
    }

    /// Settings window title.
    pub fn window_title(self) -> &'static str {
        match self {
            BuildKind::Installed => "Tile Settings",
            BuildKind::Development => "Tile Settings (Development)",
        }
    }

    /// Disabled header row at the top of the tray menu, if any.
    pub fn tray_header(self) -> Option<&'static str> {
        match self {
            BuildKind::Installed => None,
            BuildKind::Development => Some("Tile — Development build"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_missing_variable_is_a_development_build() {
        assert_eq!(BuildKind::from_env_value(None), BuildKind::Development);
    }

    #[test]
    fn an_empty_variable_is_a_development_build() {
        assert_eq!(BuildKind::from_env_value(Some("")), BuildKind::Development);
        assert_eq!(
            BuildKind::from_env_value(Some("   ")),
            BuildKind::Development
        );
    }

    #[test]
    fn only_installed_opts_in() {
        assert_eq!(
            BuildKind::from_env_value(Some("installed")),
            BuildKind::Installed
        );
    }

    #[test]
    fn the_value_is_case_insensitive_and_trimmed() {
        for raw in ["INSTALLED", "Installed", "  installed  ", "\tinstalled\n"] {
            assert_eq!(
                BuildKind::from_env_value(Some(raw)),
                BuildKind::Installed,
                "{raw:?} should classify as installed"
            );
        }
    }

    #[test]
    fn development_is_spelled_out_without_a_warning_path() {
        assert_eq!(
            BuildKind::from_env_value(Some("development")),
            BuildKind::Development
        );
    }

    #[test]
    fn an_unrecognized_value_falls_back_to_development() {
        for raw in ["instaled", "release", "prod", "1", "true"] {
            assert_eq!(
                BuildKind::from_env_value(Some(raw)),
                BuildKind::Development,
                "{raw:?} must not be mistaken for an installed build"
            );
        }
    }

    #[test]
    fn the_two_kinds_never_share_a_config_directory_name() {
        assert_eq!(BuildKind::Installed.project_app_name(), "Tile");
        assert_ne!(
            BuildKind::Development.project_app_name(),
            BuildKind::Installed.project_app_name()
        );
    }

    #[test]
    fn only_an_installed_build_manages_the_login_item() {
        assert!(BuildKind::Installed.manages_autostart());
        assert!(!BuildKind::Development.manages_autostart());
        assert!(BuildKind::Development.is_development());
        assert!(!BuildKind::Installed.is_development());
    }

    /// A second copy of an installed Tile is never harmless: the newest global
    /// keyboard hook swallows the keystroke, so the shortcut fires in whichever
    /// process happens to be newest rather than the one the user is looking at.
    #[test]
    fn only_an_installed_build_refuses_a_second_instance() {
        assert!(BuildKind::Installed.enforces_single_instance());
        assert!(!BuildKind::Development.enforces_single_instance());
    }

    #[test]
    fn an_installed_build_carries_no_development_labelling() {
        assert_eq!(BuildKind::Installed.tray_tooltip(), "Tile");
        assert_eq!(BuildKind::Installed.window_title(), "Tile Settings");
        assert_eq!(BuildKind::Installed.tray_header(), None);
    }

    #[test]
    fn a_development_build_is_labelled_everywhere() {
        assert_eq!(BuildKind::Development.tray_tooltip(), "Tile (Development)");
        assert_eq!(
            BuildKind::Development.window_title(),
            "Tile Settings (Development)"
        );
        assert_eq!(
            BuildKind::Development.tray_header(),
            Some("Tile — Development build")
        );
    }
}
