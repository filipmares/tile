//! Loading and atomically persisting the user [`Config`].
//!
//! The store never panics: a missing or corrupt file falls back to
//! [`Config::default`] so the tray app always starts. Saves are atomic — the
//! JSON is written to a sibling temp file and then renamed over the real file,
//! so a crash mid-write cannot leave an unparseable config behind.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use directories::ProjectDirs;
use tile_core::{Config, CONFIG_FILE_NAME};

use crate::build_kind::BuildKind;

/// Resolves the platform config directory for Tile, e.g.
/// `%APPDATA%\Tile\Tile\config` on Windows and
/// `~/Library/Application Support/dev.Tile.Tile` on macOS.
///
/// A development build resolves to a *sibling* directory (`Tile-Development`)
/// instead, so running from a checkout can never rewrite — or be confused
/// with — the config of the copy the user installed. It is a separate
/// top-level directory rather than a subdirectory so that removing one leaves
/// the other untouched.
pub fn resolve_config_dir(kind: BuildKind) -> Option<PathBuf> {
    ProjectDirs::from("dev", "Tile", kind.project_app_name())
        .map(|dirs| dirs.config_dir().to_path_buf())
}

/// Full path to the config file inside `dir`.
pub fn config_file_path(dir: &Path) -> PathBuf {
    dir.join(CONFIG_FILE_NAME)
}

/// How a config load turned out, alongside the resulting [`Config`].
///
/// The distinction matters for first-run detection. Every outcome yields a
/// usable config, but only [`ConfigOrigin::Missing`] means Tile has genuinely
/// never run here. A corrupt or unreadable config belongs to someone who has
/// used Tile before, and re-onboarding them would be wrong.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConfigOrigin {
    /// No config file exists. This is a first run.
    Missing,
    /// A config file was read and parsed.
    Loaded,
    /// A config file exists but could not be read or parsed.
    Corrupt,
}

/// A loaded [`Config`] and the [`ConfigOrigin`] it came from.
#[derive(Debug, Clone)]
pub struct LoadedConfig {
    pub config: Config,
    pub origin: ConfigOrigin,
}

impl LoadedConfig {
    /// Whether this load represents a genuine first run.
    pub fn is_first_run(&self) -> bool {
        self.origin == ConfigOrigin::Missing
    }
}

/// Loads the config from `dir`, returning [`Config::default`] when the file is
/// absent or cannot be parsed, along with which of those happened. Never fails.
pub fn load_from_dir(dir: &Path) -> LoadedConfig {
    let path = config_file_path(dir);
    let (config, origin) = match fs::read_to_string(&path) {
        Ok(contents) => match Config::from_json(&contents) {
            Ok(config) => (config, ConfigOrigin::Loaded),
            Err(err) => {
                log::warn!(
                    "config at {} is corrupt ({err}); falling back to defaults",
                    path.display()
                );
                (Config::default(), ConfigOrigin::Corrupt)
            }
        },
        Err(err) if err.kind() == io::ErrorKind::NotFound => {
            log::info!("no config at {}; using defaults", path.display());
            (Config::default(), ConfigOrigin::Missing)
        }
        Err(err) => {
            log::warn!(
                "could not read config at {} ({err}); using defaults",
                path.display()
            );
            (Config::default(), ConfigOrigin::Corrupt)
        }
    };
    LoadedConfig { config, origin }
}

/// Serializes `config` and writes it atomically into `dir`, creating the
/// directory if necessary.
pub fn save_to_dir(dir: &Path, config: &Config) -> io::Result<()> {
    fs::create_dir_all(dir)?;
    let json = config
        .to_json()
        .map_err(|err| io::Error::new(io::ErrorKind::InvalidData, err.to_string()))?;

    let final_path = config_file_path(dir);
    let tmp_path = dir.join(format!("{CONFIG_FILE_NAME}.tmp"));

    fs::write(&tmp_path, json.as_bytes())?;
    // `rename` replaces the destination atomically on both Windows and Unix.
    match fs::rename(&tmp_path, &final_path) {
        Ok(()) => Ok(()),
        Err(err) => {
            let _ = fs::remove_file(&tmp_path);
            Err(err)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::env;
    use std::sync::atomic::{AtomicU32, Ordering};

    /// A throwaway directory under the OS temp dir, cleaned up on drop.
    struct TempDir(PathBuf);

    impl TempDir {
        fn new() -> Self {
            static COUNTER: AtomicU32 = AtomicU32::new(0);
            let n = COUNTER.fetch_add(1, Ordering::Relaxed);
            let pid = std::process::id();
            let dir = env::temp_dir().join(format!("tile-cfg-test-{pid}-{n}"));
            fs::create_dir_all(&dir).expect("create temp dir");
            TempDir(dir)
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn missing_file_yields_defaults() {
        let dir = TempDir::new();
        assert_eq!(load_from_dir(&dir.0).config, Config::default());
    }

    #[test]
    fn corrupt_file_yields_defaults() {
        let dir = TempDir::new();
        fs::write(config_file_path(&dir.0), b"{ not json ]").unwrap();
        assert_eq!(load_from_dir(&dir.0).config, Config::default());
    }

    /// Only a genuinely absent config means Tile has never run here.
    #[test]
    fn a_missing_config_is_a_first_run() {
        let dir = TempDir::new();
        let loaded = load_from_dir(&dir.0);
        assert_eq!(loaded.origin, ConfigOrigin::Missing);
        assert!(loaded.is_first_run());
    }

    #[test]
    fn an_existing_config_is_not_a_first_run() {
        let dir = TempDir::new();
        save_to_dir(&dir.0, &Config::default()).unwrap();
        let loaded = load_from_dir(&dir.0);
        assert_eq!(loaded.origin, ConfigOrigin::Loaded);
        assert!(!loaded.is_first_run());
    }

    /// A user whose config broke has still used Tile before. Re-onboarding
    /// them would be worse than showing nothing.
    #[test]
    fn a_corrupt_config_is_not_a_first_run() {
        let dir = TempDir::new();
        fs::write(config_file_path(&dir.0), b"{ not json ]").unwrap();
        let loaded = load_from_dir(&dir.0);
        assert_eq!(loaded.origin, ConfigOrigin::Corrupt);
        assert!(!loaded.is_first_run());
    }

    /// An older config written before orientation existed must not trigger it.
    #[test]
    fn a_legacy_config_without_the_marker_is_not_a_first_run() {
        let dir = TempDir::new();
        fs::write(
            config_file_path(&dir.0),
            br#"{"bindings":{},"gap":8,"launchOnLogin":true}"#,
        )
        .unwrap();
        let loaded = load_from_dir(&dir.0);
        assert_eq!(loaded.origin, ConfigOrigin::Loaded);
        assert!(!loaded.is_first_run());
        assert!(!loaded.config.orientation_shown);
    }

    #[test]
    fn save_then_load_round_trips() {
        let dir = TempDir::new();
        let config = Config {
            gaps: tile_core::Gaps::uniform(42.0),
            launch_on_login: true,
            ..Default::default()
        };
        save_to_dir(&dir.0, &config).unwrap();
        assert_eq!(load_from_dir(&dir.0).config, config);
    }

    #[test]
    fn save_creates_missing_directory() {
        let dir = TempDir::new();
        let nested = dir.0.join("a").join("b");
        save_to_dir(&nested, &Config::default()).unwrap();
        assert!(config_file_path(&nested).exists());
    }

    #[test]
    fn save_leaves_no_temp_file_behind() {
        let dir = TempDir::new();
        save_to_dir(&dir.0, &Config::default()).unwrap();
        let tmp = dir.0.join(format!("{CONFIG_FILE_NAME}.tmp"));
        assert!(!tmp.exists(), "temp file should have been renamed away");
    }

    #[test]
    fn save_overwrites_existing_config() {
        let dir = TempDir::new();
        save_to_dir(&dir.0, &Config::default()).unwrap();
        let config = Config {
            gaps: tile_core::Gaps::uniform(99.0),
            ..Default::default()
        };
        save_to_dir(&dir.0, &config).unwrap();
        assert_eq!(
            load_from_dir(&dir.0).config.gaps,
            tile_core::Gaps::uniform(99.0)
        );
    }

    #[test]
    fn a_development_build_never_shares_the_installed_config_directory() {
        let installed = resolve_config_dir(BuildKind::Installed);
        let development = resolve_config_dir(BuildKind::Development);
        // Both resolve on every supported host; if one ever does not, the app
        // falls back to in-memory defaults rather than crossing the streams.
        assert!(installed.is_some(), "installed config dir should resolve");
        assert!(development.is_some(), "dev config dir should resolve");
        assert_ne!(installed, development);
        // Sibling directories, not one nested inside the other.
        let (installed, development) = (installed.unwrap(), development.unwrap());
        assert!(!development.starts_with(&installed));
        assert!(!installed.starts_with(&development));
    }
}
