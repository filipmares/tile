//! Version ordering kept separate from discovery so a newer development or
//! prerelease build can never be offered a downgrade.

use semver::Version;

/// The action implied by comparing the running and published versions.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Decision {
    UpToDate,
    UpdateAvailable {
        version: Version,
    },
    /// The running build is newer than the newest release.
    Ahead,
}

/// Compares the running version with the newest published version.
pub fn decide(current: &Version, latest: &Version) -> Decision {
    use std::cmp::Ordering;

    match current.cmp(latest) {
        Ordering::Less => Decision::UpdateAvailable {
            version: latest.clone(),
        },
        Ordering::Equal => Decision::UpToDate,
        Ordering::Greater => Decision::Ahead,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn version(value: &str) -> Version {
        Version::parse(value).expect("test version should be valid")
    }

    #[test]
    fn equal_versions_are_up_to_date() {
        assert_eq!(
            decide(&version("0.2.0"), &version("0.2.0")),
            Decision::UpToDate
        );
    }

    #[test]
    fn patch_minor_and_major_bumps_are_available() {
        for latest in ["0.2.1", "0.3.0", "1.0.0"] {
            let latest = version(latest);
            assert_eq!(
                decide(&version("0.2.0"), &latest),
                Decision::UpdateAvailable {
                    version: latest.clone()
                }
            );
        }
    }

    #[test]
    fn a_newer_running_version_is_ahead() {
        assert_eq!(
            decide(&version("0.3.0"), &version("0.2.0")),
            Decision::Ahead
        );
    }

    #[test]
    fn a_stable_release_succeeds_its_prerelease() {
        let latest = version("0.2.0");
        assert_eq!(
            decide(&version("0.2.0-rc.1"), &latest),
            Decision::UpdateAvailable { version: latest }
        );
    }

    #[test]
    fn malformed_versions_are_parse_errors() {
        for raw in ["", "not-a-version", "0.2", "v0.2.0"] {
            assert!(Version::parse(raw).is_err(), "{raw:?} must not parse");
        }
    }
}
