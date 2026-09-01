//! Application-owned update state and Tauri updater coordination.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use std::thread;
use std::time::Duration;

#[cfg(target_os = "macos")]
use std::path::Path;
#[cfg(target_os = "macos")]
use std::process::{Command, Stdio};

use tauri::{AppHandle, Runtime};
use tauri_plugin_updater::{Update, UpdaterExt};

use crate::build_kind::BuildKind;

const STARTUP_CHECK_DELAY: Duration = Duration::from_secs(5);
const RECHECK_INTERVAL: Duration = Duration::from_secs(24 * 60 * 60);
#[cfg(target_os = "macos")]
const RELAUNCH_DELAY_SECONDS: &str = "1";

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum UpdateStatus {
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
    #[cfg(target_os = "macos")]
    ReadyToRelaunch {
        version: String,
    },
    Error {
        message: String,
    },
}

struct UpdateInner {
    status: UpdateStatus,
    available: Option<Update>,
}

pub struct UpdateManager {
    build_kind: BuildKind,
    checking: AtomicBool,
    installing: AtomicBool,
    inner: Mutex<UpdateInner>,
}

fn suppresses_check(status: &UpdateStatus) -> bool {
    if matches!(status, UpdateStatus::Downloading { .. }) {
        return true;
    }
    #[cfg(target_os = "macos")]
    {
        matches!(status, UpdateStatus::ReadyToRelaunch { .. })
    }
    #[cfg(not(target_os = "macos"))]
    {
        false
    }
}

#[cfg(target_os = "macos")]
fn app_bundle_for_executable(executable: &Path) -> Option<&Path> {
    let macos = executable.parent()?;
    if macos.file_name()? != "MacOS" {
        return None;
    }
    let contents = macos.parent()?;
    if contents.file_name()? != "Contents" {
        return None;
    }
    let bundle = contents.parent()?;
    (bundle.extension()? == "app").then_some(bundle)
}

/// Relaunches the installed macOS bundle after an update has settled on disk.
///
/// Tauri restarts by executing the replaced binary directly. macOS can reject
/// that immediate exec while Gatekeeper is still evaluating the new bundle, so
/// defer until this process has exited and ask Launch Services to open the app.
#[cfg(target_os = "macos")]
pub(crate) fn relaunch<R: Runtime>(app: &AppHandle<R>) -> Result<(), String> {
    let executable =
        std::env::current_exe().map_err(|err| format!("could not locate Tile: {err}"))?;
    let bundle = app_bundle_for_executable(&executable).ok_or_else(|| {
        format!(
            "Tile is not running from an app bundle: {}",
            executable.display()
        )
    })?;

    Command::new("/bin/sh")
        .args([
            "-c",
            "sleep \"$1\"; exec /usr/bin/open -n \"$2\"",
            "tile-relaunch",
            RELAUNCH_DELAY_SECONDS,
        ])
        .arg(bundle)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|err| format!("could not schedule Tile to relaunch: {err}"))?;

    app.exit(0);
    Ok(())
}

impl UpdateManager {
    pub fn new(build_kind: BuildKind) -> Self {
        let status = if build_kind.is_development() {
            UpdateStatus::Unavailable
        } else {
            UpdateStatus::Idle
        };
        Self {
            build_kind,
            checking: AtomicBool::new(false),
            installing: AtomicBool::new(false),
            inner: Mutex::new(UpdateInner {
                status,
                available: None,
            }),
        }
    }

    pub fn status(&self) -> UpdateStatus {
        lock(&self.inner).status.clone()
    }

    pub async fn check<R: Runtime>(&self, app: &AppHandle<R>) -> Result<UpdateStatus, String> {
        if self.build_kind.is_development() {
            return Ok(UpdateStatus::Unavailable);
        }
        if suppresses_check(&self.status()) {
            return Ok(self.status());
        }
        if self
            .checking
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .is_err()
        {
            return Ok(self.status());
        }

        self.publish_status(app, UpdateStatus::Checking);
        let result = async {
            let updater = app.updater().map_err(|err| err.to_string())?;
            updater.check().await.map_err(|err| err.to_string())
        }
        .await;

        let status = match result {
            Ok(Some(update)) => {
                let status = UpdateStatus::Available {
                    version: update.version.clone(),
                    notes: update.body.clone(),
                    date: update.date.map(|date| date.to_string()),
                };
                let mut inner = lock(&self.inner);
                inner.available = Some(update);
                inner.status = status.clone();
                drop(inner);
                crate::tray::sync_update_state(app, &status);
                status
            }
            Ok(None) => {
                let mut inner = lock(&self.inner);
                inner.available = None;
                inner.status = UpdateStatus::Current;
                drop(inner);
                crate::tray::sync_update_state(app, &UpdateStatus::Current);
                UpdateStatus::Current
            }
            Err(message) => {
                let status = UpdateStatus::Error { message };
                let mut inner = lock(&self.inner);
                inner.available = None;
                inner.status = status.clone();
                drop(inner);
                crate::tray::sync_update_state(app, &status);
                status
            }
        };
        self.checking.store(false, Ordering::Release);
        Ok(status)
    }

    pub async fn install<R: Runtime>(
        &self,
        app: &AppHandle<R>,
        relaunch_after_install: bool,
    ) -> Result<UpdateStatus, String> {
        if self.build_kind.is_development() {
            return Err("updates are unavailable in development builds".into());
        }
        #[cfg(not(target_os = "macos"))]
        let _ = relaunch_after_install;

        #[cfg(target_os = "macos")]
        if relaunch_after_install && matches!(self.status(), UpdateStatus::ReadyToRelaunch { .. }) {
            relaunch(app)?;
            return Ok(self.status());
        }
        if self
            .installing
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .is_err()
        {
            return Err("an update installation is already in progress".into());
        }

        let Some(update) = lock(&self.inner).available.clone() else {
            self.installing.store(false, Ordering::Release);
            return Err("no update is available".into());
        };
        let version = update.version.clone();
        self.publish_status(
            app,
            UpdateStatus::Downloading {
                version: version.clone(),
                downloaded_bytes: 0,
                total_bytes: None,
            },
        );

        let downloaded = Mutex::new(0_u64);
        let result = update
            .download_and_install(
                |chunk_length, content_length| {
                    let mut downloaded = lock(&downloaded);
                    *downloaded += chunk_length as u64;
                    self.set_status(UpdateStatus::Downloading {
                        version: version.clone(),
                        downloaded_bytes: *downloaded,
                        total_bytes: content_length,
                    });
                },
                || {},
            )
            .await;

        if let Err(err) = result {
            let message = err.to_string();
            self.publish_status(
                app,
                UpdateStatus::Error {
                    message: message.clone(),
                },
            );
            self.installing.store(false, Ordering::Release);
            return Err(message);
        }

        lock(&self.inner).available = None;

        #[cfg(target_os = "macos")]
        {
            self.publish_status(
                app,
                UpdateStatus::ReadyToRelaunch {
                    version: version.clone(),
                },
            );
            if relaunch_after_install {
                self.installing.store(false, Ordering::Release);
                relaunch(app)?;
            }
        }

        #[cfg(not(target_os = "macos"))]
        self.publish_status(app, UpdateStatus::Current);

        self.installing.store(false, Ordering::Release);
        Ok(self.status())
    }

    fn set_status(&self, status: UpdateStatus) {
        lock(&self.inner).status = status;
    }

    fn publish_status<R: Runtime>(&self, app: &AppHandle<R>, status: UpdateStatus) {
        self.set_status(status.clone());
        crate::tray::sync_update_state(app, &status);
    }
}

pub fn begin_update_checks<R: Runtime>(app: AppHandle<R>, manager: Arc<UpdateManager>) {
    if manager.build_kind.is_development() {
        return;
    }
    thread::Builder::new()
        .name("tile-update-check".into())
        .spawn(move || {
            thread::sleep(STARTUP_CHECK_DELAY);
            loop {
                if let Err(err) = tauri::async_runtime::block_on(manager.check(&app)) {
                    log::warn!("update check failed: {err}");
                }
                thread::sleep(RECHECK_INTERVAL);
            }
        })
        .map(|_| ())
        .unwrap_or_else(|err| log::error!("failed to spawn update-check thread: {err}"));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn development_builds_are_permanently_unavailable() {
        let manager = UpdateManager::new(BuildKind::Development);
        assert_eq!(manager.status(), UpdateStatus::Unavailable);
    }

    #[test]
    fn installed_builds_start_idle() {
        let manager = UpdateManager::new(BuildKind::Installed);
        assert_eq!(manager.status(), UpdateStatus::Idle);
    }

    #[test]
    fn status_transitions_preserve_progress_and_errors() {
        let manager = UpdateManager::new(BuildKind::Installed);
        manager.set_status(UpdateStatus::Checking);
        assert_eq!(manager.status(), UpdateStatus::Checking);

        manager.set_status(UpdateStatus::Downloading {
            version: "1.2.3".into(),
            downloaded_bytes: 512,
            total_bytes: Some(1024),
        });
        assert_eq!(
            manager.status(),
            UpdateStatus::Downloading {
                version: "1.2.3".into(),
                downloaded_bytes: 512,
                total_bytes: Some(1024),
            }
        );

        manager.set_status(UpdateStatus::Error {
            message: "offline".into(),
        });
        assert_eq!(
            manager.status(),
            UpdateStatus::Error {
                message: "offline".into()
            }
        );
    }

    #[test]
    fn concurrent_check_guard_collapses_in_flight_checks() {
        let manager = UpdateManager::new(BuildKind::Installed);
        assert!(manager
            .checking
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .is_ok());
        assert!(manager
            .checking
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .is_err());
    }

    #[test]
    fn concurrent_install_guard_collapses_in_flight_installs() {
        let manager = UpdateManager::new(BuildKind::Installed);
        assert!(manager
            .installing
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .is_ok());
        assert!(manager
            .installing
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .is_err());
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn installed_updates_are_not_regressed_by_another_check() {
        assert!(suppresses_check(&UpdateStatus::ReadyToRelaunch {
            version: "1.2.3".into()
        }));
        assert!(suppresses_check(&UpdateStatus::Downloading {
            version: "1.2.3".into(),
            downloaded_bytes: 1,
            total_bytes: None,
        }));
        assert!(!suppresses_check(&UpdateStatus::Current));
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn finds_app_bundle_from_macos_executable() {
        let executable = Path::new("/Applications/Tile.app/Contents/MacOS/tile");
        assert_eq!(
            app_bundle_for_executable(executable),
            Some(Path::new("/Applications/Tile.app"))
        );
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn rejects_executables_outside_an_app_bundle() {
        assert_eq!(
            app_bundle_for_executable(Path::new("/usr/local/bin/tile")),
            None
        );
        assert_eq!(
            app_bundle_for_executable(Path::new("/Applications/Tile/Contents/MacOS/tile")),
            None
        );
    }
}
