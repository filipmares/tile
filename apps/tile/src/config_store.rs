//! Loading and atomically persisting the user [`Config`].
//!
//! The store never panics: a missing or corrupt file falls back to
//! [`Config::default`] so the tray app always starts. A corrupt file is moved
//! aside first, so starting from defaults never costs the user their only
//! copy. Saves are atomic — the JSON is written to a uniquely named sibling
//! temp file, flushed to disk, and then renamed over the real file, so a crash
//! mid-write cannot leave an unparseable config behind.

use std::fs;
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

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

/// What happened to a config file that could not be read or parsed.
///
/// Tile starts from defaults in that case, and the next save would replace the
/// user's only copy, so the old file is moved aside first and the user is told
/// once where it went.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConfigRecovery {
    /// Where the unreadable file was moved. `None` when it could not be moved
    /// aside, in which case nothing may be saved over it this session.
    pub backup_path: Option<PathBuf>,
}

/// A loaded [`Config`] and the [`ConfigOrigin`] it came from.
#[derive(Debug, Clone)]
pub struct LoadedConfig {
    pub config: Config,
    pub origin: ConfigOrigin,
    /// Set only for [`ConfigOrigin::Corrupt`] loads of a file that exists.
    pub recovery: Option<ConfigRecovery>,
}

impl LoadedConfig {
    /// Whether this load represents a genuine first run.
    pub fn is_first_run(&self) -> bool {
        self.origin == ConfigOrigin::Missing
    }
}

/// Loads the config from `dir`, returning [`Config::default`] when the file is
/// absent or cannot be parsed, along with which of those happened. Never fails.
///
/// A file that exists but cannot be read or parsed is moved aside to
/// `config.corrupt-<unix-seconds>.json` before this returns, so no later save
/// can overwrite it. See [`ConfigRecovery`].
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
    let recovery = (origin == ConfigOrigin::Corrupt).then(|| back_up_corrupt(dir, &path));
    let mut config = config;
    if let Some(recovery) = &recovery {
        // Whoever owned the broken file has used Tile before; the welcome
        // walkthrough is not owed to them on this launch or the next.
        config.orientation_shown = true;
        // With the old file safely aside, put a readable one in its place so
        // the next launch neither repeats this notice nor mistakes the user
        // for a first run.
        if recovery.backup_path.is_some() {
            if let Err(err) = save_to_dir(dir, &config) {
                log::error!("could not write fresh defaults after backing up: {err}");
            }
        }
    }
    LoadedConfig {
        config,
        origin,
        recovery,
    }
}

/// Moves an unreadable config out of the way so it survives the next save.
fn back_up_corrupt(dir: &Path, path: &Path) -> ConfigRecovery {
    let backup = unused_backup_path(dir, unix_seconds());
    let backup_path = match fs::rename(path, &backup) {
        Ok(()) => {
            log::warn!(
                "moved the unreadable config to {}; starting from defaults",
                backup.display()
            );
            Some(backup)
        }
        Err(rename_err) => match fs::copy(path, &backup) {
            Ok(_) => {
                log::warn!(
                    "could not move the unreadable config ({rename_err}); copied it to {}",
                    backup.display()
                );
                Some(backup)
            }
            Err(copy_err) => {
                log::error!(
                    "could not back up the unreadable config at {} (move: {rename_err}; \
                     copy: {copy_err}); settings will not be saved this session",
                    path.display()
                );
                None
            }
        },
    };
    ConfigRecovery { backup_path }
}

/// The backup name for a corrupt config found at `timestamp`, e.g.
/// `config.corrupt-1767225600.json`.
pub fn backup_file_name(timestamp: u64) -> String {
    let path = Path::new(CONFIG_FILE_NAME);
    let stem = path
        .file_stem()
        .and_then(|stem| stem.to_str())
        .unwrap_or("config");
    match path.extension().and_then(|ext| ext.to_str()) {
        Some(ext) => format!("{stem}.corrupt-{timestamp}.{ext}"),
        None => format!("{stem}.corrupt-{timestamp}"),
    }
}

/// A backup path in `dir` that does not exist yet. Two corrupt loads in the
/// same second must not have the second one replace the first backup.
fn unused_backup_path(dir: &Path, timestamp: u64) -> PathBuf {
    let first = dir.join(backup_file_name(timestamp));
    if !first.exists() {
        return first;
    }
    let name = backup_file_name(timestamp);
    let (stem, ext) = name.rsplit_once('.').unwrap_or((name.as_str(), ""));
    (1u32..)
        .map(|n| {
            if ext.is_empty() {
                dir.join(format!("{stem}-{n}"))
            } else {
                dir.join(format!("{stem}-{n}.{ext}"))
            }
        })
        .find(|candidate| !candidate.exists())
        .unwrap_or(first)
}

fn unix_seconds() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|elapsed| elapsed.as_secs())
        .unwrap_or(0)
}

/// A temp file name no other save — in this process or another — is using.
fn unique_temp_path(dir: &Path) -> PathBuf {
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let n = COUNTER.fetch_add(1, Ordering::Relaxed);
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|elapsed| elapsed.as_nanos())
        .unwrap_or(0);
    dir.join(format!(
        "{CONFIG_FILE_NAME}.{}-{nanos}-{n}.tmp",
        std::process::id()
    ))
}

/// Serializes `config` and writes it atomically into `dir`, creating the
/// directory if necessary.
pub fn save_to_dir(dir: &Path, config: &Config) -> io::Result<()> {
    fs::create_dir_all(dir)?;
    let json = config
        .to_json()
        .map_err(|err| io::Error::new(io::ErrorKind::InvalidData, err.to_string()))?;

    let final_path = config_file_path(dir);
    let tmp_path = unique_temp_path(dir);

    let result = write_synced(&tmp_path, json.as_bytes())
        // `rename` replaces the destination atomically on both Windows and Unix.
        .and_then(|()| fs::rename(&tmp_path, &final_path));
    if result.is_err() {
        let _ = fs::remove_file(&tmp_path);
    }
    result
}

/// Writes `bytes` to a new file at `path` and flushes it to disk, so the
/// rename that follows can never publish a file whose contents are still only
/// in the OS cache.
fn write_synced(path: &Path, bytes: &[u8]) -> io::Result<()> {
    let mut file = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)?;
    file.write_all(bytes)?;
    file.sync_all()
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
        let config = load_from_dir(&dir.0).config;
        assert_eq!(
            config,
            Config {
                orientation_shown: true,
                ..Config::default()
            }
        );
    }

    fn backups_in(dir: &Path) -> Vec<PathBuf> {
        let mut found: Vec<PathBuf> = fs::read_dir(dir)
            .unwrap()
            .map(|entry| entry.unwrap().path())
            .filter(|path| {
                path.file_name()
                    .and_then(|name| name.to_str())
                    .is_some_and(|name| name.starts_with("config.corrupt-"))
            })
            .collect();
        found.sort();
        found
    }

    #[test]
    fn backup_names_carry_the_timestamp() {
        assert_eq!(
            backup_file_name(1_767_225_600),
            "config.corrupt-1767225600.json"
        );
    }

    #[test]
    fn a_corrupt_config_is_backed_up_byte_for_byte() {
        let dir = TempDir::new();
        let broken = b"{ \"gap\": 12, oops";
        fs::write(config_file_path(&dir.0), broken).unwrap();

        let loaded = load_from_dir(&dir.0);

        let backup = loaded
            .recovery
            .as_ref()
            .and_then(|recovery| recovery.backup_path.clone())
            .expect("the corrupt file should have been backed up");
        assert_eq!(backups_in(&dir.0), vec![backup.clone()]);
        assert_eq!(fs::read(&backup).unwrap(), broken);
    }

    #[test]
    fn saving_after_a_corrupt_load_leaves_the_backup_alone() {
        let dir = TempDir::new();
        let broken = b"not json at all";
        fs::write(config_file_path(&dir.0), broken).unwrap();
        let loaded = load_from_dir(&dir.0);
        let backup = loaded.recovery.unwrap().backup_path.unwrap();

        let config = Config {
            gaps: tile_core::Gaps::uniform(7.0),
            ..loaded.config
        };
        save_to_dir(&dir.0, &config).unwrap();

        assert_eq!(fs::read(&backup).unwrap(), broken);
        assert_eq!(load_from_dir(&dir.0).config, config);
    }

    /// The fresh defaults written in place of the broken file mean the next
    /// launch is an ordinary one: no second notice, no second backup, and no
    /// first-run welcome.
    #[test]
    fn the_launch_after_a_recovery_is_ordinary() {
        let dir = TempDir::new();
        fs::write(config_file_path(&dir.0), b"][").unwrap();
        load_from_dir(&dir.0);

        let next = load_from_dir(&dir.0);
        assert_eq!(next.origin, ConfigOrigin::Loaded);
        assert!(next.recovery.is_none());
        assert!(!next.is_first_run());
        assert!(next.config.orientation_shown);
        assert_eq!(backups_in(&dir.0).len(), 1);
    }

    #[test]
    fn a_second_backup_in_the_same_second_does_not_replace_the_first() {
        let dir = TempDir::new();
        let first = unused_backup_path(&dir.0, 42);
        fs::write(&first, b"first").unwrap();
        let second = unused_backup_path(&dir.0, 42);
        assert_ne!(first, second);
        assert_eq!(
            second.file_name().unwrap().to_str().unwrap(),
            "config.corrupt-42-1.json"
        );
    }

    #[test]
    fn a_missing_config_needs_no_recovery() {
        let dir = TempDir::new();
        let loaded = load_from_dir(&dir.0);
        assert!(loaded.recovery.is_none());
        assert!(backups_in(&dir.0).is_empty());
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
        save_to_dir(&dir.0, &Config::default()).unwrap();
        let leftovers: Vec<_> = fs::read_dir(&dir.0)
            .unwrap()
            .map(|entry| entry.unwrap().file_name())
            .filter(|name| name.to_string_lossy().ends_with(".tmp"))
            .collect();
        assert!(
            leftovers.is_empty(),
            "temp files left behind: {leftovers:?}"
        );
    }

    #[test]
    fn temp_file_names_are_unique() {
        let dir = Path::new("unused");
        assert_ne!(unique_temp_path(dir), unique_temp_path(dir));
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
