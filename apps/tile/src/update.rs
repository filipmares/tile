//! Application-owned update state and Tauri updater coordination.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use std::thread;
use std::time::Duration;

#[cfg(target_os = "macos")]
use std::path::Path;
#[cfg(target_os = "macos")]
use std::process::{Command, Stdio};

use serde::{Deserialize, Serialize};
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
        kind: UpdateErrorKind,
    },
}

/// Why an update check or install failed, in terms the UI and tray can explain
/// without showing raw updater text. The camelCase names are the IPC contract
/// (see `updateErrorMessage` in `ui/src/errors.ts`); the raw detail is logged.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum UpdateErrorKind {
    /// GitHub could not be reached: no connection, DNS, proxy or timeout.
    Offline,
    /// GitHub answered, but not with a usable release manifest.
    Server,
    /// The update's signature did not verify, so it was refused.
    Signature,
    /// The download stopped part way, or arrived unusable.
    Interrupted,
    /// The update could not be written: no space, or no permission.
    Disk,
    /// The installer failed to start or was cancelled (e.g. a declined UAC prompt).
    Installer,
    Unknown,
}

impl UpdateErrorKind {
    /// A few words for the tray, which has no room for a sentence.
    pub fn short_cause(self) -> &'static str {
        match self {
            Self::Offline => "offline",
            Self::Server => "server problem",
            Self::Signature => "signature check failed",
            Self::Interrupted => "download interrupted",
            Self::Disk => "could not save",
            Self::Installer => "installer did not finish",
            Self::Unknown => "unknown error",
        }
    }
}

/// A failed update command, as the UI receives it. `detail` is raw updater
/// text for the log and developer console only and is never displayed.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UpdateError {
    pub kind: UpdateErrorKind,
    pub detail: String,
}

impl UpdateError {
    fn new(kind: UpdateErrorKind, detail: impl std::fmt::Display) -> Self {
        Self {
            kind,
            detail: detail.to_string(),
        }
    }

    fn unknown(detail: impl std::fmt::Display) -> Self {
        Self::new(UpdateErrorKind::Unknown, detail)
    }
}

impl std::fmt::Display for UpdateError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{:?}: {}", self.kind, self.detail)
    }
}

/// Whether the updater failed while looking for an update or while
/// downloading and installing one; the same error can mean different things.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum UpdatePhase {
    Check,
    Install,
}

#[cfg(windows)]
const OS_DISK_FULL: &[i32] = &[39, 112]; // ERROR_HANDLE_DISK_FULL, ERROR_DISK_FULL
#[cfg(not(windows))]
const OS_DISK_FULL: &[i32] = &[28, 69]; // ENOSPC, EDQUOT (macOS)
#[cfg(windows)]
const OS_READ_ONLY: &[i32] = &[19]; // ERROR_WRITE_PROTECT
#[cfg(not(windows))]
const OS_READ_ONLY: &[i32] = &[30]; // EROFS
#[cfg(windows)]
const OS_CANCELLED: &[i32] = &[1223]; // ERROR_CANCELLED, e.g. a declined UAC prompt
#[cfg(not(windows))]
const OS_CANCELLED: &[i32] = &[];

fn classify_io(err: &std::io::Error, phase: UpdatePhase) -> UpdateErrorKind {
    use std::io::ErrorKind;
    let code = err.raw_os_error();
    let has = |codes: &[i32]| code.is_some_and(|code| codes.contains(&code));
    if has(OS_CANCELLED) {
        UpdateErrorKind::Installer
    } else if err.kind() == ErrorKind::PermissionDenied || has(OS_DISK_FULL) || has(OS_READ_ONLY) {
        UpdateErrorKind::Disk
    } else if matches!(
        err.kind(),
        ErrorKind::TimedOut
            | ErrorKind::ConnectionRefused
            | ErrorKind::ConnectionReset
            | ErrorKind::ConnectionAborted
            | ErrorKind::NotConnected
    ) {
        match phase {
            UpdatePhase::Check => UpdateErrorKind::Offline,
            UpdatePhase::Install => UpdateErrorKind::Interrupted,
        }
    } else if matches!(
        err.kind(),
        ErrorKind::InvalidData | ErrorKind::UnexpectedEof
    ) {
        // A corrupt or truncated archive, as the macOS tar/gzip extraction
        // reports it.
        match phase {
            UpdatePhase::Check => UpdateErrorKind::Server,
            UpdatePhase::Install => UpdateErrorKind::Interrupted,
        }
    } else {
        match phase {
            UpdatePhase::Check => UpdateErrorKind::Unknown,
            UpdatePhase::Install => UpdateErrorKind::Installer,
        }
    }
}

/// What kind of HTTP failure `reqwest` reported, so the decision can be
/// tested without a network (a `reqwest::Error` cannot be built by hand).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum HttpFailure {
    /// The server answered with an error status.
    Status,
    /// The response body broke off or could not be decoded.
    Body,
    /// The request could not be built at all.
    Builder,
    /// No connection: DNS, refused, proxy or TLS.
    Connect,
    /// Anything else, such as a timeout.
    Other,
}

fn classify_http(failure: HttpFailure, phase: UpdatePhase) -> UpdateErrorKind {
    match (failure, phase) {
        (HttpFailure::Status, _) | (HttpFailure::Body, UpdatePhase::Check) => {
            UpdateErrorKind::Server
        }
        (HttpFailure::Builder, _) => UpdateErrorKind::Unknown,
        (HttpFailure::Connect, _) | (HttpFailure::Other, UpdatePhase::Check) => {
            UpdateErrorKind::Offline
        }
        // A timeout or reset once bytes were flowing.
        (HttpFailure::Body | HttpFailure::Other, UpdatePhase::Install) => {
            UpdateErrorKind::Interrupted
        }
    }
}

fn classify(err: &tauri_plugin_updater::Error, phase: UpdatePhase) -> UpdateErrorKind {
    use tauri_plugin_updater::Error as E;
    match err {
        E::Reqwest(err) => {
            let failure = if err.is_status() {
                HttpFailure::Status
            } else if err.is_body() || err.is_decode() {
                HttpFailure::Body
            } else if err.is_builder() {
                HttpFailure::Builder
            } else if err.is_connect() {
                HttpFailure::Connect
            } else {
                HttpFailure::Other
            };
            classify_http(failure, phase)
        }
        E::Io(err) => classify_io(err, phase),
        // ZIP extraction on Windows: pass nested I/O through, and treat a
        // malformed archive as a download that did not arrive intact.
        #[cfg(windows)]
        E::Extract(err) => match std::error::Error::source(err)
            .and_then(|source| source.downcast_ref::<std::io::Error>())
        {
            Some(err) => classify_io(err, phase),
            None => UpdateErrorKind::Interrupted,
        },
        // A non-success download status is reported as `Network`.
        E::Network(_)
        | E::ReleaseNotFound
        | E::Serialization(_)
        | E::Semver(_)
        | E::TargetNotFound(_)
        | E::TargetsNotFound(_)
        | E::FormatDate => UpdateErrorKind::Server,
        E::Minisign(_)
        | E::Base64(_)
        | E::SignatureUtf8(_)
        | E::SignedVersionMismatch { .. }
        | E::MissingSignedVersion => UpdateErrorKind::Signature,
        E::BinaryNotFoundInArchive | E::InvalidUpdaterFormat => UpdateErrorKind::Interrupted,
        E::TempDirNotFound | E::FailedToDetermineExtractPath | E::TempDirNotOnSameMountPoint => {
            UpdateErrorKind::Disk
        }
        E::AuthenticationFailed | E::DebInstallFailed | E::PackageInstallFailed => {
            UpdateErrorKind::Installer
        }
        _ => UpdateErrorKind::Unknown,
    }
}

fn update_error(err: tauri_plugin_updater::Error, phase: UpdatePhase) -> UpdateError {
    UpdateError::new(classify(&err, phase), err)
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

    pub async fn check<R: Runtime>(&self, app: &AppHandle<R>) -> Result<UpdateStatus, UpdateError> {
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
            // On Windows the updater ends this process with
            // `std::process::exit(0)` once the installer is launched, which
            // skips `RunEvent::Exit`; record the handoff before that happens.
            let updater = app
                .updater_builder()
                .on_before_exit(|| {
                    crate::logging::end_session("handing off to the update installer")
                })
                .build()
                .map_err(|err| update_error(err, UpdatePhase::Check))?;
            updater
                .check()
                .await
                .map_err(|err| update_error(err, UpdatePhase::Check))
        }
        .await;

        let status = match result {
            Ok(Some(update)) => {
                log::info!("update available: {}", update.version);
                let status = UpdateStatus::Available {
                    version: update.version.clone(),
                    notes: update.body.clone(),
                    date: update.date.map(|date| date.to_string()),
                };
                let mut inner = lock(&self.inner);
                inner.available = Some(update);
                inner.status = status.clone();
                drop(inner);
                crate::tray::sync_update_state(app);
                status
            }
            Ok(None) => {
                let mut inner = lock(&self.inner);
                inner.available = None;
                inner.status = UpdateStatus::Current;
                drop(inner);
                crate::tray::sync_update_state(app);
                UpdateStatus::Current
            }
            Err(err) => {
                log::warn!("update check failed ({:?}): {}", err.kind, err.detail);
                let status = UpdateStatus::Error { kind: err.kind };
                let mut inner = lock(&self.inner);
                inner.available = None;
                inner.status = status.clone();
                drop(inner);
                crate::tray::sync_update_state(app);
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
    ) -> Result<UpdateStatus, UpdateError> {
        if self.build_kind.is_development() {
            return Err(UpdateError::unknown(
                "updates are unavailable in development builds",
            ));
        }
        #[cfg(not(target_os = "macos"))]
        let _ = relaunch_after_install;

        #[cfg(target_os = "macos")]
        if relaunch_after_install && matches!(self.status(), UpdateStatus::ReadyToRelaunch { .. }) {
            relaunch(app).map_err(UpdateError::unknown)?;
            return Ok(self.status());
        }
        if self
            .installing
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .is_err()
        {
            return Err(UpdateError::unknown(
                "an update installation is already in progress",
            ));
        }

        let Some(update) = lock(&self.inner).available.clone() else {
            self.installing.store(false, Ordering::Release);
            return Err(UpdateError::unknown("no update is available"));
        };
        let version = update.version.clone();
        log::info!("downloading and installing update {version}");
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
            let err = update_error(err, UpdatePhase::Install);
            log::error!(
                "installing update {version} failed ({:?}): {}",
                err.kind,
                err.detail
            );
            self.publish_status(app, UpdateStatus::Error { kind: err.kind });
            self.installing.store(false, Ordering::Release);
            return Err(err);
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
                relaunch(app).map_err(UpdateError::unknown)?;
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
        crate::tray::sync_update_state(app);
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
            kind: UpdateErrorKind::Offline,
        });
        assert_eq!(
            manager.status(),
            UpdateStatus::Error {
                kind: UpdateErrorKind::Offline
            }
        );
    }

    fn io(code: i32) -> tauri_plugin_updater::Error {
        std::io::Error::from_raw_os_error(code).into()
    }

    #[test]
    fn classifies_server_and_metadata_errors() {
        use tauri_plugin_updater::Error as E;
        for err in [
            E::ReleaseNotFound,
            E::Network("Download request failed with status: 404".into()),
            E::TargetNotFound("windows-x86_64".into()),
            E::TargetsNotFound(vec!["windows-x86_64".into()]),
            serde_json::from_str::<u8>("nope").unwrap_err().into(),
        ] {
            assert_eq!(classify(&err, UpdatePhase::Check), UpdateErrorKind::Server);
        }
    }

    #[test]
    fn classifies_signature_errors() {
        use tauri_plugin_updater::Error as E;
        for err in [
            E::SignatureUtf8("bad".into()),
            E::MissingSignedVersion,
            E::SignedVersionMismatch {
                signed: "1.0.0".into(),
                announced: "2.0.0".into(),
            },
        ] {
            assert_eq!(
                classify(&err, UpdatePhase::Install),
                UpdateErrorKind::Signature
            );
        }
    }

    #[test]
    fn classifies_unusable_downloads_as_interrupted() {
        use tauri_plugin_updater::Error as E;
        for err in [E::BinaryNotFoundInArchive, E::InvalidUpdaterFormat] {
            assert_eq!(
                classify(&err, UpdatePhase::Install),
                UpdateErrorKind::Interrupted
            );
        }
    }

    #[test]
    fn classifies_disk_and_permission_errors() {
        use tauri_plugin_updater::Error as E;
        let denied: tauri_plugin_updater::Error =
            std::io::Error::from(std::io::ErrorKind::PermissionDenied).into();
        for err in [
            denied,
            io(OS_DISK_FULL[0]),
            io(OS_READ_ONLY[0]),
            E::TempDirNotFound,
            E::FailedToDetermineExtractPath,
        ] {
            assert_eq!(classify(&err, UpdatePhase::Install), UpdateErrorKind::Disk);
        }
    }

    #[cfg(windows)]
    #[test]
    fn a_declined_uac_prompt_is_an_installer_error() {
        assert_eq!(
            classify(&io(1223), UpdatePhase::Install),
            UpdateErrorKind::Installer
        );
    }

    #[test]
    fn corrupt_archives_are_interrupted_downloads() {
        for kind in [
            std::io::ErrorKind::InvalidData,
            std::io::ErrorKind::UnexpectedEof,
        ] {
            let err: tauri_plugin_updater::Error = std::io::Error::from(kind).into();
            assert_eq!(
                classify(&err, UpdatePhase::Install),
                UpdateErrorKind::Interrupted
            );
        }
    }

    #[cfg(windows)]
    #[test]
    fn zip_extraction_errors_are_classified() {
        use tauri_plugin_updater::Error as E;
        // `ZipError` converts from these, so it need not be named here.
        let malformed = E::Extract(String::from_utf8(vec![0xff]).unwrap_err().into());
        assert_eq!(
            classify(&malformed, UpdatePhase::Install),
            UpdateErrorKind::Interrupted
        );
        let denied = E::Extract(std::io::Error::from(std::io::ErrorKind::PermissionDenied).into());
        assert_eq!(
            classify(&denied, UpdatePhase::Install),
            UpdateErrorKind::Disk
        );
    }

    #[test]
    fn classifies_installer_failures() {
        use tauri_plugin_updater::Error as E;
        for err in [E::AuthenticationFailed, E::PackageInstallFailed] {
            assert_eq!(
                classify(&err, UpdatePhase::Install),
                UpdateErrorKind::Installer
            );
        }
        let other: tauri_plugin_updater::Error = std::io::Error::other("launch failed").into();
        assert_eq!(
            classify(&other, UpdatePhase::Install),
            UpdateErrorKind::Installer
        );
        assert_eq!(
            classify(&other, UpdatePhase::Check),
            UpdateErrorKind::Unknown
        );
    }

    #[test]
    fn dropped_connections_depend_on_the_phase() {
        let timeout = || -> tauri_plugin_updater::Error {
            std::io::Error::from(std::io::ErrorKind::TimedOut).into()
        };
        assert_eq!(
            classify(&timeout(), UpdatePhase::Check),
            UpdateErrorKind::Offline
        );
        assert_eq!(
            classify(&timeout(), UpdatePhase::Install),
            UpdateErrorKind::Interrupted
        );
    }

    #[test]
    fn http_failures_depend_on_the_phase() {
        use HttpFailure::*;
        use UpdateErrorKind as K;
        use UpdatePhase::{Check, Install};
        for (failure, check, install) in [
            (Status, K::Server, K::Server),
            (Body, K::Server, K::Interrupted),
            (Connect, K::Offline, K::Offline),
            (Other, K::Offline, K::Interrupted),
            (Builder, K::Unknown, K::Unknown),
        ] {
            assert_eq!(classify_http(failure, Check), check, "{failure:?} check");
            assert_eq!(
                classify_http(failure, Install),
                install,
                "{failure:?} install"
            );
        }
    }

    #[test]
    fn configuration_errors_are_unknown() {
        use tauri_plugin_updater::Error as E;
        for err in [E::EmptyEndpoints, E::InsecureTransportProtocol] {
            assert_eq!(classify(&err, UpdatePhase::Check), UpdateErrorKind::Unknown);
        }
    }

    /// The UI switches on these exact strings (see `updateErrorMessage` in
    /// `ui/src/errors.ts`), so renaming a variant must be a deliberate change.
    #[test]
    fn errors_serialize_to_the_strings_the_ui_maps() {
        let err = UpdateError::new(UpdateErrorKind::Offline, "dns error");
        assert_eq!(
            serde_json::to_value(err).unwrap(),
            serde_json::json!({ "kind": "offline", "detail": "dns error" })
        );
        for (kind, name) in [
            (UpdateErrorKind::Server, "server"),
            (UpdateErrorKind::Signature, "signature"),
            (UpdateErrorKind::Interrupted, "interrupted"),
            (UpdateErrorKind::Disk, "disk"),
            (UpdateErrorKind::Installer, "installer"),
            (UpdateErrorKind::Unknown, "unknown"),
        ] {
            assert_eq!(serde_json::to_value(kind).unwrap(), name);
        }
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
