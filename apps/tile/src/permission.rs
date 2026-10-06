//! The one place Tile's Accessibility permission state lives and changes.
//!
//! macOS gates every window move on Accessibility permission, and the user can
//! grant or revoke it at any time in System Settings. The startup flow, the
//! background monitor, the settings window, runtime action failures and the
//! tray menu all read and update the same [`PermissionTracker`] (held by
//! [`AppState`]), so they can never disagree about whether Tile is blocked.
//!
//! The tracker only ever records what `AXIsProcessTrusted` reported. One failed
//! AX call is not treated as proof of anything: a runtime `PermissionDenied`
//! triggers a fresh trust check through [`refresh`] instead of flipping the
//! state on its own. Whether the one-time system prompt was requested is
//! session-local UI history, not TCC truth.
//!
//! Windows and other platforms report `NotRequired`, which the tracker treats
//! as granted; nothing here ever reports them as blocked and the monitor is
//! never started for them.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::thread;
use std::time::Duration;

use tauri::{AppHandle, Emitter, Manager, Runtime};
use tile_platform::PermissionStatus;

use crate::dto::PermissionStatusDto;
use crate::state::AppState;

/// How often the monitor re-checks while access is denied, so a grant applies
/// hotkeys within moments of the user flipping the switch.
const DENIED_POLL_INTERVAL: Duration = Duration::from_secs(2);

/// How often the monitor re-checks while access is granted, to notice a
/// revocation. `AXIsProcessTrusted` is a cheap local query.
const GRANTED_POLL_INTERVAL: Duration = Duration::from_secs(5);

/// Emitted to every window when the permission state changes.
pub const PERMISSION_CHANGED_EVENT: &str = "permission-changed";

/// What Tile currently believes about its Accessibility permission.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PermissionState {
    /// Not checked yet.
    Unknown,
    /// macOS reported Tile as untrusted: window moves will fail.
    Denied,
    /// Trusted, or the platform needs no permission at all.
    Granted,
}

/// A change worth acting on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Transition {
    /// The first check found permission missing.
    Blocked,
    /// Permission arrived after being missing: apply hotkeys now.
    Granted,
    /// Permission went away while Tile was running.
    Revoked,
}

/// What the primary "grant" button does next. Both steps end with the
/// Privacy & Security pane open while access is still missing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GrantStep {
    /// Also ask macOS for its one-time prompt, which lists Tile in the pane.
    /// macOS may already have used it up in an earlier launch.
    Prompt,
    /// The prompt was already requested this session, so only open the pane.
    OpenSettings,
}

/// Pure permission state machine. See the module docs.
#[derive(Debug)]
pub struct PermissionTracker {
    state: PermissionState,
    prompted: bool,
    /// Bumped on every transition, so effects dispatched for an older one can
    /// tell they have been superseded.
    generation: u64,
}

impl Default for PermissionTracker {
    fn default() -> Self {
        Self::new()
    }
}

impl PermissionTracker {
    pub fn new() -> Self {
        Self {
            state: PermissionState::Unknown,
            prompted: false,
            generation: 0,
        }
    }

    #[cfg(test)]
    fn state(&self) -> PermissionState {
        self.state
    }

    /// Whether Tile is known to be blocked right now.
    pub fn is_blocked(&self) -> bool {
        self.state == PermissionState::Denied
    }

    /// Records a fresh trust check and reports the transition it caused.
    ///
    /// `Unknown → Granted` is not a transition: the startup flow applies
    /// hotkeys itself on a granted first check.
    pub fn observe(&mut self, status: PermissionStatus) -> Option<Transition> {
        let next = match status {
            PermissionStatus::Denied => PermissionState::Denied,
            PermissionStatus::Granted | PermissionStatus::NotRequired => PermissionState::Granted,
        };
        let previous = std::mem::replace(&mut self.state, next);
        let transition = match (previous, next) {
            (PermissionState::Unknown, PermissionState::Denied) => Some(Transition::Blocked),
            (PermissionState::Granted, PermissionState::Denied) => Some(Transition::Revoked),
            (PermissionState::Denied, PermissionState::Granted) => Some(Transition::Granted),
            _ => None,
        };
        if transition.is_some() {
            self.generation += 1;
        }
        transition
    }

    /// Identifies the latest transition. See [`refresh`].
    pub fn generation(&self) -> u64 {
        self.generation
    }

    /// Notes that the system prompt has been requested this session.
    pub fn mark_prompted(&mut self) {
        self.prompted = true;
    }

    pub fn grant_step(&self) -> GrantStep {
        if self.prompted {
            GrantStep::OpenSettings
        } else {
            GrantStep::Prompt
        }
    }

    /// How long the monitor waits before its next check.
    pub fn poll_interval(&self) -> Duration {
        match self.state {
            PermissionState::Denied => DENIED_POLL_INTERVAL,
            PermissionState::Unknown | PermissionState::Granted => GRANTED_POLL_INTERVAL,
        }
    }
}

/// The `.app` bundle an executable runs from, if it runs from one: the
/// executable must sit at `<Name>.app/Contents/MacOS/<exe>`. A `cargo run`
/// binary in `target/` has no bundle, and macOS lists whatever launched it
/// (Terminal, an IDE) in the Accessibility pane instead.
pub fn app_bundle(executable: &Path) -> Option<PathBuf> {
    let macos_dir = executable.parent()?;
    if macos_dir.file_name()? != "MacOS" {
        return None;
    }
    let contents = macos_dir.parent()?;
    if contents.file_name()? != "Contents" {
        return None;
    }
    let bundle = contents.parent()?;
    let is_app = bundle
        .extension()
        .is_some_and(|ext| ext.eq_ignore_ascii_case("app"));
    is_app.then(|| bundle.to_path_buf())
}

/// The running app's bundle, on macOS only.
pub fn running_app_bundle() -> Option<PathBuf> {
    if !cfg!(target_os = "macos") {
        return None;
    }
    std::env::current_exe().ok().as_deref().and_then(app_bundle)
}

/// Re-checks trust (optionally requesting the system prompt) and carries out
/// whatever the resulting transition requires. Every path that reads the
/// permission goes through here, so a grant noticed by the settings window's
/// poll applies hotkeys exactly as one noticed by the monitor would.
///
/// `prompt` must only be `true` on the main thread.
pub fn refresh<R: Runtime>(
    app: &AppHandle<R>,
    prompt: bool,
) -> tile_platform::Result<PermissionStatus> {
    let state = app.state::<Arc<AppState>>().inner().clone();
    let (status, transition) = state.refresh_permission(prompt)?;
    if let Some((transition, generation)) = transition {
        on_transition(app, &state, transition, generation, status);
    }
    Ok(status)
}

/// Carries out a transition's effects.
///
/// Observations are recorded in order under the tracker lock, but the effects
/// run after it is released (holding it here could deadlock against the main
/// thread, which tray updates wait on). Two refreshes racing a grant and a
/// revocation could therefore act out of order, so every one-shot effect first
/// checks that its transition is still the latest and is dropped otherwise.
/// The tray and the settings window always re-read the current state, so they
/// end up right whichever order their updates land in.
fn on_transition<R: Runtime>(
    app: &AppHandle<R>,
    state: &Arc<AppState>,
    transition: Transition,
    generation: u64,
    status: PermissionStatus,
) {
    let is_current = move |state: &AppState| state.permission_generation() == generation;
    match transition {
        Transition::Blocked => {
            log::info!("accessibility permission denied");
        }
        Transition::Granted => {
            log::info!("accessibility permission granted; applying hotkeys");
            // The apply runs on the main thread, which owns the macOS Carbon
            // event target and its wake/session-switch observers. The
            // shortcuts the welcome describes only start working once applied,
            // so that is the first honest moment to show it.
            let handle = app.clone();
            let state = state.clone();
            if let Err(err) = app.run_on_main_thread(move || {
                if !is_current(&state) {
                    log::info!("permission changed again before hotkeys were applied");
                    return;
                }
                state.apply_hotkeys();
                crate::open_welcome_for_first_run(&handle, &state);
            }) {
                log::error!("could not apply hotkeys on the main thread: {err}");
            }
        }
        Transition::Revoked => {
            log::warn!("accessibility permission was revoked while Tile was running");
            let handle = app.clone();
            let state = state.clone();
            if let Err(err) = app.run_on_main_thread(move || {
                if !is_current(&state) {
                    log::info!("permission was granted again before settings opened");
                    return;
                }
                if let Err(err) = crate::window::open_settings(&handle, state.build_kind()) {
                    log::error!("failed to open settings window: {err}");
                }
            }) {
                log::error!("could not open the settings window: {err}");
            }
        }
    }
    crate::tray::sync_bindings(app);
    if let Err(err) = app.emit(PERMISSION_CHANGED_EVENT, PermissionStatusDto::from(status)) {
        log::warn!("could not announce the permission change: {err}");
    }
}

/// Low-frequency background check that keeps the tracker current for as long
/// as Tile runs: it picks up a grant within [`DENIED_POLL_INTERVAL`] and a
/// revocation within [`GRANTED_POLL_INTERVAL`]. Only uses the non-prompting
/// check, so it is safe off the main thread.
pub fn spawn_monitor<R: Runtime>(app: AppHandle<R>) {
    let state = app.state::<Arc<AppState>>().inner().clone();
    thread::Builder::new()
        .name("tile-permission-monitor".into())
        .spawn(move || {
            let mut failing = false;
            loop {
                thread::sleep(state.permission_poll_interval());
                match refresh(&app, false) {
                    Ok(_) => failing = false,
                    Err(err) => {
                        // Logged once per streak so a persistent failure
                        // cannot flood the log every few seconds.
                        if !failing {
                            log::error!("permission check failed: {err}");
                        }
                        failing = true;
                    }
                }
            }
        })
        .map(|_| ())
        .unwrap_or_else(|err| log::error!("failed to spawn permission monitor thread: {err}"));
}

/// Opens System Settings ▸ Privacy & Security ▸ Accessibility (System
/// Preferences ▸ Security & Privacy on older macOS). Tries the deep links
/// first, then falls back to opening the Settings app itself, so the button is
/// never a silent no-op even where URL routing differs.
#[cfg(target_os = "macos")]
pub fn open_accessibility_settings() -> Result<(), String> {
    use std::process::Command;

    let opened = |args: &[&str]| {
        Command::new("/usr/bin/open")
            .args(args)
            .status()
            .is_ok_and(|status| status.success())
    };
    for url in ACCESSIBILITY_DEEP_LINKS {
        if opened(&[url]) {
            return Ok(());
        }
        log::warn!("could not open {url}; trying the next way into System Settings");
    }
    // System Settings and System Preferences share this bundle identifier.
    if opened(&["-b", "com.apple.systempreferences"]) {
        return Ok(());
    }
    Err(
        "Tile could not open System Settings. From the Apple menu, open System Settings ▸ \
         Privacy & Security ▸ Accessibility (on macOS 12 and earlier, System Preferences ▸ \
         Security & Privacy ▸ Privacy ▸ Accessibility)."
            .into(),
    )
}

#[cfg(not(target_os = "macos"))]
pub fn open_accessibility_settings() -> Result<(), String> {
    Err("Accessibility permission is only needed on macOS.".into())
}

/// Deep links into the Accessibility list. The legacy scheme comes first
/// because every supported macOS version still routes it; the newer System
/// Settings scheme is the second try.
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
const ACCESSIBILITY_DEEP_LINKS: [&str; 2] = [
    "x-apple.systempreferences:com.apple.preference.security?Privacy_Accessibility",
    "x-apple.systempreferences:com.apple.settings.PrivacySecurity.extension?Privacy_Accessibility",
];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_fresh_tracker_knows_nothing() {
        let tracker = PermissionTracker::new();
        assert_eq!(tracker.state(), PermissionState::Unknown);
        assert!(!tracker.is_blocked());
        assert_eq!(tracker.grant_step(), GrantStep::Prompt);
    }

    #[test]
    fn a_denied_first_run_is_blocked() {
        let mut tracker = PermissionTracker::new();
        assert_eq!(
            tracker.observe(PermissionStatus::Denied),
            Some(Transition::Blocked)
        );
        assert!(tracker.is_blocked());
        // Staying denied is not news.
        assert_eq!(tracker.observe(PermissionStatus::Denied), None);
        assert!(tracker.is_blocked());
    }

    #[test]
    fn a_granted_first_check_is_not_a_transition() {
        let mut tracker = PermissionTracker::new();
        assert_eq!(tracker.observe(PermissionStatus::Granted), None);
        assert_eq!(tracker.state(), PermissionState::Granted);
        assert_eq!(tracker.observe(PermissionStatus::Granted), None);
    }

    #[test]
    fn granting_while_running_is_picked_up_without_a_restart() {
        let mut tracker = PermissionTracker::new();
        tracker.observe(PermissionStatus::Denied);
        assert_eq!(
            tracker.observe(PermissionStatus::Granted),
            Some(Transition::Granted)
        );
        assert!(!tracker.is_blocked());
    }

    #[test]
    fn revocation_while_running_blocks_again_and_can_recover() {
        let mut tracker = PermissionTracker::new();
        tracker.observe(PermissionStatus::Granted);
        assert_eq!(
            tracker.observe(PermissionStatus::Denied),
            Some(Transition::Revoked)
        );
        assert!(tracker.is_blocked());
        assert_eq!(
            tracker.observe(PermissionStatus::Granted),
            Some(Transition::Granted)
        );
    }

    #[test]
    fn platforms_without_a_permission_are_never_blocked() {
        let mut tracker = PermissionTracker::new();
        for _ in 0..3 {
            assert_eq!(tracker.observe(PermissionStatus::NotRequired), None);
            assert!(!tracker.is_blocked());
        }
    }

    #[test]
    fn the_first_grant_prompts_and_later_ones_open_settings() {
        let mut tracker = PermissionTracker::new();
        tracker.observe(PermissionStatus::Denied);
        assert_eq!(tracker.grant_step(), GrantStep::Prompt);
        tracker.mark_prompted();
        assert_eq!(tracker.grant_step(), GrantStep::OpenSettings);
        // A consumed prompt stays consumed for the session, whatever happens.
        tracker.observe(PermissionStatus::Granted);
        tracker.observe(PermissionStatus::Denied);
        assert_eq!(tracker.grant_step(), GrantStep::OpenSettings);
    }

    #[test]
    fn every_transition_supersedes_the_one_before() {
        let mut tracker = PermissionTracker::new();
        assert_eq!(tracker.generation(), 0);
        tracker.observe(PermissionStatus::Denied);
        let blocked = tracker.generation();
        tracker.observe(PermissionStatus::Denied);
        assert_eq!(
            tracker.generation(),
            blocked,
            "no transition, no new generation"
        );
        tracker.observe(PermissionStatus::Granted);
        let granted = tracker.generation();
        assert!(granted > blocked);
        tracker.observe(PermissionStatus::Denied);
        assert!(tracker.generation() > granted);
    }

    #[test]
    fn the_monitor_polls_faster_while_blocked() {
        let mut tracker = PermissionTracker::new();
        tracker.observe(PermissionStatus::Denied);
        assert_eq!(tracker.poll_interval(), DENIED_POLL_INTERVAL);
        tracker.observe(PermissionStatus::Granted);
        assert_eq!(tracker.poll_interval(), GRANTED_POLL_INTERVAL);
        assert!(GRANTED_POLL_INTERVAL >= Duration::from_secs(5));
    }

    #[test]
    fn an_installed_app_finds_its_bundle() {
        let exe = Path::new("/Applications/Tile.app/Contents/MacOS/tile");
        assert_eq!(
            app_bundle(exe),
            Some(PathBuf::from("/Applications/Tile.app"))
        );
        let relocated = Path::new("/Users/me/Downloads/Tile 2.APP/Contents/MacOS/tile");
        assert_eq!(
            app_bundle(relocated),
            Some(PathBuf::from("/Users/me/Downloads/Tile 2.APP"))
        );
    }

    #[test]
    fn an_unbundled_development_binary_has_no_bundle() {
        for exe in [
            "/Users/me/tile/target/debug/tile",
            "/Users/me/tile/target/release/bundle/macos/MacOS/tile",
            "/Users/me/Contents/MacOS/tile",
            "tile",
        ] {
            assert_eq!(app_bundle(Path::new(exe)), None, "{exe}");
        }
    }

    #[test]
    fn deep_links_point_at_the_accessibility_privacy_list() {
        for url in ACCESSIBILITY_DEEP_LINKS {
            assert!(url.starts_with("x-apple.systempreferences:"));
            assert!(url.ends_with("?Privacy_Accessibility"));
        }
    }
}
