//! Update lifecycle transitions.
//!
//! The phase, rather than callers or timers, owns concurrency. Requests that do
//! not apply to the current phase leave it unchanged, so manual and scheduled
//! checks cannot overlap and stale callbacks cannot advance a newer operation.

use semver::Version;

use super::version::Decision;

/// The single source of truth for update progress.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum UpdatePhase {
    Idle,
    Checking,
    UpToDate,
    Available { version: Version },
    Failed { reason: String },
    Downloading,
    ReadyToInstall,
    Quiescing,
    Installing,
}

/// Why a check was requested.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CheckSource {
    Manual,
    Scheduled,
}

/// Inputs accepted by the update state machine.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Event {
    CheckRequested(CheckSource),
    CheckCompleted(Decision),
    CheckFailed(String),
    Dismiss,
    Confirm,
    DownloadCompleted,
    BeginQuiescing,
    BeginInstalling,
    Cancel,
    Quit,
}

/// Applies one event. Invalid or duplicate events are intentionally no-ops.
pub fn reduce(phase: UpdatePhase, event: Event) -> UpdatePhase {
    match (phase, event) {
        (UpdatePhase::Idle, Event::CheckRequested(_)) => UpdatePhase::Checking,
        (UpdatePhase::Checking, Event::CheckCompleted(Decision::UpToDate | Decision::Ahead)) => {
            UpdatePhase::UpToDate
        }
        (UpdatePhase::Checking, Event::CheckCompleted(Decision::UpdateAvailable { version })) => {
            UpdatePhase::Available { version }
        }
        (UpdatePhase::Checking, Event::CheckFailed(reason)) => UpdatePhase::Failed { reason },
        (
            UpdatePhase::UpToDate | UpdatePhase::Available { .. } | UpdatePhase::Failed { .. },
            Event::Dismiss,
        ) => UpdatePhase::Idle,
        (UpdatePhase::Available { .. }, Event::Confirm) => UpdatePhase::Downloading,
        (UpdatePhase::Downloading, Event::DownloadCompleted) => UpdatePhase::ReadyToInstall,
        (UpdatePhase::ReadyToInstall, Event::BeginQuiescing) => UpdatePhase::Quiescing,
        (UpdatePhase::Quiescing, Event::BeginInstalling) => UpdatePhase::Installing,
        (UpdatePhase::Downloading, Event::Cancel | Event::Quit) => UpdatePhase::Idle,
        (phase, _) => phase,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn version(value: &str) -> Version {
        Version::parse(value).expect("test version should be valid")
    }

    #[test]
    fn valid_transitions_follow_the_update_lifecycle() {
        let cases = [
            (
                UpdatePhase::Idle,
                Event::CheckRequested(CheckSource::Manual),
                UpdatePhase::Checking,
            ),
            (
                UpdatePhase::Checking,
                Event::CheckCompleted(Decision::UpToDate),
                UpdatePhase::UpToDate,
            ),
            (UpdatePhase::UpToDate, Event::Dismiss, UpdatePhase::Idle),
            (
                UpdatePhase::Checking,
                Event::CheckCompleted(Decision::UpdateAvailable {
                    version: version("0.3.0"),
                }),
                UpdatePhase::Available {
                    version: version("0.3.0"),
                },
            ),
            (
                UpdatePhase::Available {
                    version: version("0.3.0"),
                },
                Event::Confirm,
                UpdatePhase::Downloading,
            ),
            (
                UpdatePhase::Downloading,
                Event::DownloadCompleted,
                UpdatePhase::ReadyToInstall,
            ),
            (
                UpdatePhase::ReadyToInstall,
                Event::BeginQuiescing,
                UpdatePhase::Quiescing,
            ),
            (
                UpdatePhase::Quiescing,
                Event::BeginInstalling,
                UpdatePhase::Installing,
            ),
        ];

        for (phase, event, expected) in cases {
            assert_eq!(reduce(phase, event), expected);
        }
    }

    #[test]
    fn duplicate_and_overlapping_checks_are_rejected() {
        let cases = [
            Event::CheckRequested(CheckSource::Manual),
            Event::CheckRequested(CheckSource::Scheduled),
        ];

        for event in cases {
            assert_eq!(reduce(UpdatePhase::Checking, event), UpdatePhase::Checking);
        }
    }

    #[test]
    fn a_manual_check_cannot_overlap_a_scheduled_check() {
        let phase = reduce(
            UpdatePhase::Idle,
            Event::CheckRequested(CheckSource::Scheduled),
        );
        assert_eq!(
            reduce(phase, Event::CheckRequested(CheckSource::Manual)),
            UpdatePhase::Checking
        );
    }

    #[test]
    fn failure_is_reported_and_can_return_to_idle() {
        let failed = reduce(
            UpdatePhase::Checking,
            Event::CheckFailed("manifest unavailable".into()),
        );
        assert_eq!(
            failed,
            UpdatePhase::Failed {
                reason: "manifest unavailable".into()
            }
        );
        assert_eq!(reduce(failed, Event::Dismiss), UpdatePhase::Idle);
    }

    #[test]
    fn cancellation_and_quit_during_download_return_to_idle() {
        for event in [Event::Cancel, Event::Quit] {
            assert_eq!(reduce(UpdatePhase::Downloading, event), UpdatePhase::Idle);
        }
    }

    #[test]
    fn stale_events_do_not_advance_the_wrong_phase() {
        let available = UpdatePhase::Available {
            version: version("0.3.0"),
        };
        assert_eq!(
            reduce(available.clone(), Event::DownloadCompleted),
            available
        );
        assert_eq!(
            reduce(UpdatePhase::Idle, Event::BeginInstalling),
            UpdatePhase::Idle
        );
    }

    #[test]
    fn being_ahead_is_presented_as_up_to_date() {
        assert_eq!(
            reduce(
                UpdatePhase::Checking,
                Event::CheckCompleted(Decision::Ahead)
            ),
            UpdatePhase::UpToDate
        );
    }
}
