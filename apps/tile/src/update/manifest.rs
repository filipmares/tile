//! Parsing for Tauri's update manifest contract.
//!
//! A release may intentionally omit a platform, notably when a macOS build is
//! unsigned. Keeping platform lookup optional makes that a normal "no update"
//! result rather than turning a safe publishing decision into an error.

use std::collections::HashMap;

use semver::Version;
use serde::Deserialize;
use thiserror::Error;

/// A parsed Tauri `latest.json` manifest.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Manifest {
    pub version: Version,
    pub notes: Option<String>,
    pub pub_date: Option<String>,
    pub platforms: HashMap<String, Platform>,
}

/// The signed artifact Tauri publishes for one runtime target.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct Platform {
    pub signature: String,
    pub url: String,
}

#[derive(Debug, Deserialize)]
struct RawManifest {
    version: String,
    #[serde(default)]
    notes: Option<String>,
    #[serde(default)]
    pub_date: Option<String>,
    platforms: HashMap<String, Platform>,
}

/// Why a manifest could not be interpreted.
#[derive(Debug, Error)]
pub enum ParseError {
    #[error("invalid update manifest JSON: {0}")]
    Json(#[from] serde_json::Error),
    #[error("invalid update version: {0}")]
    Version(#[from] semver::Error),
}

/// Parses Tauri's `latest.json` response without selecting a platform.
pub fn parse(input: &str) -> Result<Manifest, ParseError> {
    let raw: RawManifest = serde_json::from_str(input)?;
    Ok(Manifest {
        version: Version::parse(&raw.version)?,
        notes: raw.notes,
        pub_date: raw.pub_date,
        platforms: raw.platforms,
    })
}

/// The key Tauri uses for this binary's runtime target.
pub fn current_platform_key() -> String {
    let os = match std::env::consts::OS {
        "macos" => "darwin",
        os => os,
    };
    format!("{os}-{}", std::env::consts::ARCH)
}

impl Manifest {
    /// Returns `None` when the publisher intentionally did not ship this target.
    pub fn platform(&self, target: &str) -> Option<&Platform> {
        self.platforms.get(target)
    }
}

/// Provides manifest text to the coordinator while keeping network I/O
/// replaceable by an in-memory test stub.
pub trait ManifestFetcher {
    type Error;

    fn fetch_manifest(&self) -> Result<String, Self::Error>;
}

#[cfg(test)]
mod tests {
    use super::*;

    const WELL_FORMED: &str = r#"{
        "version": "0.2.0",
        "notes": "A safer updater",
        "pub_date": "2026-08-21T12:00:00Z",
        "platforms": {
            "darwin-aarch64": {
                "signature": "mac-signature",
                "url": "https://example.com/Tile.app.tar.gz"
            },
            "windows-x86_64": {
                "signature": "windows-signature",
                "url": "https://example.com/Tile_0.2.0_x64-setup.exe"
            }
        }
    }"#;

    #[test]
    fn parses_a_well_formed_tauri_manifest() {
        let manifest = parse(WELL_FORMED).expect("manifest should parse");

        assert_eq!(manifest.version, Version::new(0, 2, 0));
        assert_eq!(manifest.notes.as_deref(), Some("A safer updater"));
        assert_eq!(manifest.pub_date.as_deref(), Some("2026-08-21T12:00:00Z"));
        assert_eq!(
            manifest.platform("darwin-aarch64"),
            Some(&Platform {
                signature: "mac-signature".into(),
                url: "https://example.com/Tile.app.tar.gz".into(),
            })
        );
    }

    #[test]
    fn a_missing_current_platform_is_not_an_error() {
        let manifest = parse(
            r#"{
                "version": "0.2.0",
                "notes": "No artifact for this target",
                "pub_date": "2026-08-21T12:00:00Z",
                "platforms": {}
            }"#,
        )
        .expect("omitting a target is valid");

        assert_eq!(manifest.platform(&current_platform_key()), None);
    }

    #[test]
    fn malformed_json_is_an_error() {
        assert!(matches!(parse("{"), Err(ParseError::Json(_))));
    }

    #[test]
    fn unknown_fields_are_ignored() {
        let manifest = parse(
            r#"{
                "version": "0.2.0",
                "notes": null,
                "pub_date": null,
                "channel": "stable",
                "platforms": {
                    "darwin-aarch64": {
                        "signature": "signature",
                        "url": "https://example.com/Tile.app.tar.gz",
                        "size": 1234
                    }
                }
            }"#,
        )
        .expect("additive schema changes should be tolerated");

        assert_eq!(manifest.version, Version::new(0, 2, 0));
        assert!(manifest.platform("darwin-aarch64").is_some());
    }

    #[test]
    fn malformed_manifest_versions_are_errors() {
        let input = WELL_FORMED.replace("\"0.2.0\"", "\"not-a-version\"");
        assert!(matches!(parse(&input), Err(ParseError::Version(_))));
    }
}
