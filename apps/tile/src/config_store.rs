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

/// Resolves the platform config directory for Tile, e.g.
/// `%APPDATA%\Tile\Tile\config` on Windows and
/// `~/Library/Application Support/dev.Tile.Tile` on macOS.
pub fn resolve_config_dir() -> Option<PathBuf> {
    ProjectDirs::from("dev", "Tile", "Tile").map(|dirs| dirs.config_dir().to_path_buf())
}

/// Full path to the config file inside `dir`.
pub fn config_file_path(dir: &Path) -> PathBuf {
    dir.join(CONFIG_FILE_NAME)
}

/// Loads the config from `dir`, returning [`Config::default`] when the file is
/// absent or cannot be parsed. Never fails.
pub fn load_from_dir(dir: &Path) -> Config {
    let path = config_file_path(dir);
    match fs::read_to_string(&path) {
        Ok(contents) => match Config::from_json(&contents) {
            Ok(config) => config,
            Err(err) => {
                log::warn!(
                    "config at {} is corrupt ({err}); falling back to defaults",
                    path.display()
                );
                Config::default()
            }
        },
        Err(err) if err.kind() == io::ErrorKind::NotFound => {
            log::info!("no config at {}; using defaults", path.display());
            Config::default()
        }
        Err(err) => {
            log::warn!(
                "could not read config at {} ({err}); using defaults",
                path.display()
            );
            Config::default()
        }
    }
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
        assert_eq!(load_from_dir(&dir.0), Config::default());
    }

    #[test]
    fn corrupt_file_yields_defaults() {
        let dir = TempDir::new();
        fs::write(config_file_path(&dir.0), b"{ not json ]").unwrap();
        assert_eq!(load_from_dir(&dir.0), Config::default());
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
        assert_eq!(load_from_dir(&dir.0), config);
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
        assert_eq!(load_from_dir(&dir.0).gaps, tile_core::Gaps::uniform(99.0));
    }
}
