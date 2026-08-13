//! User configuration: key bindings and behaviour, persisted as JSON.

use serde::de::{self, MapAccess, Visitor};
use serde::{Deserialize, Deserializer, Serialize};
use std::collections::BTreeMap;
use std::fmt;

use crate::action::WindowAction;
use crate::geometry::Rect;
use crate::hotkey::{Hotkey, KeyCode, Modifiers};

/// Name of the config file inside the platform config directory.
pub const CONFIG_FILE_NAME: &str = "config.json";

/// Errors produced while loading or validating configuration.
#[derive(Debug, thiserror::Error)]
pub enum ConfigError {
    #[error("failed to parse configuration: {0}")]
    Parse(#[from] serde_json::Error),
    #[error("{0}")]
    Invalid(String),
}

/// A conflict between two actions bound to the same hotkey.
#[derive(Debug, Clone, PartialEq)]
pub struct Conflict {
    pub hotkey: Hotkey,
    pub actions: Vec<WindowAction>,
}

/// Which edges of a target rectangle are shared with a neighbouring window
/// rather than lying against the screen edge.
///
/// A shared edge receives *half* the window gap, so two adjacent windows end
/// up separated by exactly one window gap. A non-shared (screen) edge receives
/// the appropriate per-side screen-edge gap instead.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct SharedEdges {
    pub left: bool,
    pub right: bool,
    pub top: bool,
    pub bottom: bool,
}

/// The gap model: a single window gap between adjacent tiled windows plus
/// independently configurable per-side screen-edge gaps.
///
/// This generalises the original scalar `gap`, which insets every side
/// uniformly and therefore cannot express "one gap between two windows but a
/// different gap against the screen edge". A legacy scalar config still loads —
/// see the custom [`Deserialize`] impl below.
#[derive(Debug, Clone, Copy, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Gaps {
    /// Space between two adjacent tiled windows. Each shares half of it.
    pub window: f64,
    pub edge_top: f64,
    pub edge_bottom: f64,
    pub edge_left: f64,
    pub edge_right: f64,
    /// Drop the top screen-edge gap entirely (Rectangle's `skipGapTopEdge`).
    pub skip_top_edge: bool,
    /// Apply screen-edge gaps only on the primary display (Rectangle's
    /// `screenEdgeGapsOnMainScreenOnly`). The window gap always applies.
    pub main_screen_only: bool,
}

impl Default for Gaps {
    fn default() -> Self {
        Self {
            window: 0.0,
            edge_top: 0.0,
            edge_bottom: 0.0,
            edge_left: 0.0,
            edge_right: 0.0,
            skip_top_edge: false,
            main_screen_only: false,
        }
    }
}

impl Gaps {
    /// A uniform gap on every side and between windows, matching the legacy
    /// scalar `gap` semantics for migration.
    pub fn uniform(value: f64) -> Self {
        Self {
            window: value,
            edge_top: value,
            edge_bottom: value,
            edge_left: value,
            edge_right: value,
            skip_top_edge: false,
            main_screen_only: false,
        }
    }

    /// Clamps every gap into `[0, MAX_GAP]`, mapping negative or non-finite
    /// values to zero so a hand-edited or older config can never shrink a
    /// window away.
    pub fn normalize(&mut self) {
        for value in [
            &mut self.window,
            &mut self.edge_top,
            &mut self.edge_bottom,
            &mut self.edge_left,
            &mut self.edge_right,
        ] {
            if !value.is_finite() || *value < 0.0 {
                *value = 0.0;
            }
            *value = value.min(MAX_GAP);
        }
    }

    fn screen_edge(&self, value: f64, main_screen: bool) -> f64 {
        if self.main_screen_only && !main_screen {
            0.0
        } else {
            value
        }
    }

    fn edge_top_effective(&self, main_screen: bool) -> f64 {
        if self.skip_top_edge {
            0.0
        } else {
            self.screen_edge(self.edge_top, main_screen)
        }
    }

    /// Insets `rect` per edge: shared edges get half the window gap, screen
    /// edges get the relevant per-side screen-edge gap. Never grows the window.
    pub fn apply(&self, rect: Rect, shared: SharedEdges, main_screen: bool) -> Rect {
        let half = (self.window / 2.0).max(0.0);
        let left = if shared.left {
            half
        } else {
            self.screen_edge(self.edge_left, main_screen)
        };
        let right = if shared.right {
            half
        } else {
            self.screen_edge(self.edge_right, main_screen)
        };
        let top = if shared.top {
            half
        } else {
            self.edge_top_effective(main_screen)
        };
        let bottom = if shared.bottom {
            half
        } else {
            self.screen_edge(self.edge_bottom, main_screen)
        };
        rect.inset_edges(left, top, right, bottom)
    }
}

/// Accepts either a legacy scalar (`"gap": 24`) or the new object form
/// (`"gap": { "window": 8, "edgeTop": 0, ... }`), so no saved config breaks.
/// A number and an object are unambiguous in JSON, which makes this safe.
impl<'de> Deserialize<'de> for Gaps {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        struct GapsVisitor;

        impl<'de> Visitor<'de> for GapsVisitor {
            type Value = Gaps;

            fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str("a gap size (number) or a gaps object")
            }

            fn visit_f64<E>(self, value: f64) -> Result<Gaps, E> {
                Ok(Gaps::uniform(value))
            }

            fn visit_u64<E>(self, value: u64) -> Result<Gaps, E> {
                Ok(Gaps::uniform(value as f64))
            }

            fn visit_i64<E>(self, value: i64) -> Result<Gaps, E> {
                Ok(Gaps::uniform(value as f64))
            }

            fn visit_map<M>(self, mut map: M) -> Result<Gaps, M::Error>
            where
                M: MapAccess<'de>,
            {
                let mut gaps = Gaps::default();
                while let Some(key) = map.next_key::<String>()? {
                    match key.as_str() {
                        "window" => gaps.window = map.next_value()?,
                        "edgeTop" => gaps.edge_top = map.next_value()?,
                        "edgeBottom" => gaps.edge_bottom = map.next_value()?,
                        "edgeLeft" => gaps.edge_left = map.next_value()?,
                        "edgeRight" => gaps.edge_right = map.next_value()?,
                        "skipTopEdge" => gaps.skip_top_edge = map.next_value()?,
                        "mainScreenOnly" => gaps.main_screen_only = map.next_value()?,
                        // Ignore unknown keys so a newer config never fails to
                        // load on an older build.
                        _ => {
                            let _: de::IgnoredAny = map.next_value()?;
                        }
                    }
                }
                Ok(gaps)
            }
        }

        deserializer.deserialize_any(GapsVisitor)
    }
}

fn default_true() -> bool {
    true
}

/// Persisted user settings.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct Config {
    /// Hotkey per action. An action absent from the map, or mapped to `None`,
    /// is unbound.
    pub bindings: BTreeMap<WindowAction, Option<Hotkey>>,
    /// Gap model between windows and against the screen edges. Persisted under
    /// the `gap` key; a legacy scalar value still loads.
    #[serde(rename = "gap")]
    pub gaps: Gaps,
    /// Start Tile when the user logs in.
    pub launch_on_login: bool,
    /// Show the tray / menu-bar icon.
    pub show_tray_icon: bool,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            bindings: default_bindings(),
            gaps: Gaps::default(),
            launch_on_login: false,
            show_tray_icon: default_true(),
        }
    }
}

impl Config {
    /// Parses configuration from JSON, filling in defaults for missing fields.
    pub fn from_json(json: &str) -> Result<Self, ConfigError> {
        let mut config: Config = serde_json::from_str(json)?;
        config.normalize();
        Ok(config)
    }

    pub fn to_json(&self) -> Result<String, ConfigError> {
        Ok(serde_json::to_string_pretty(self)?)
    }

    /// Drops invalid bindings and clamps out-of-range values so a hand-edited
    /// or older config can never put the app into a broken state.
    pub fn normalize(&mut self) {
        self.bindings.retain(|_, hk| match hk {
            Some(h) => h.is_valid(),
            None => true,
        });
        self.gaps.normalize();
    }

    pub fn binding(&self, action: WindowAction) -> Option<Hotkey> {
        self.bindings.get(&action).copied().flatten()
    }

    /// Binds `hotkey` to `action`, unbinding any other action that already
    /// used it so the configuration can never contain a duplicate.
    pub fn set_binding(&mut self, action: WindowAction, hotkey: Option<Hotkey>) {
        if let Some(hk) = hotkey {
            let clashing: Vec<_> = self
                .bindings
                .iter()
                .filter(|(a, h)| **a != action && **h == Some(hk))
                .map(|(a, _)| *a)
                .collect();
            for a in clashing {
                self.bindings.insert(a, None);
            }
        }
        self.bindings.insert(action, hotkey);
    }

    /// Returns every hotkey bound to more than one action.
    pub fn conflicts(&self) -> Vec<Conflict> {
        let mut by_hotkey: BTreeMap<String, (Hotkey, Vec<WindowAction>)> = BTreeMap::new();
        for (action, hotkey) in &self.bindings {
            if let Some(hk) = hotkey {
                by_hotkey
                    .entry(hk.to_string())
                    .or_insert_with(|| (*hk, Vec::new()))
                    .1
                    .push(*action);
            }
        }
        by_hotkey
            .into_values()
            .filter(|(_, actions)| actions.len() > 1)
            .map(|(hotkey, actions)| Conflict { hotkey, actions })
            .collect()
    }

    /// Every currently bound (hotkey, action) pair, for backend registration.
    pub fn active_bindings(&self) -> Vec<(Hotkey, WindowAction)> {
        self.bindings
            .iter()
            .filter_map(|(a, h)| h.map(|h| (h, *a)))
            .collect()
    }
}

/// Upper bound for the screen-edge gap, to stop a typo shrinking windows away.
pub const MAX_GAP: f64 = 200.0;

/// Platform-appropriate default key bindings.
///
/// macOS keeps Rectangle's well-known `Control+Option` defaults so existing
/// muscle memory carries over. Windows uses the `Win` key combinations users
/// already associate with snapping; `Win+Arrow` is claimed from the shell by
/// the low-level keyboard hook in the Windows backend.
pub fn default_bindings() -> BTreeMap<WindowAction, Option<Hotkey>> {
    let mut map = BTreeMap::new();

    #[cfg(target_os = "macos")]
    {
        let base = Modifiers::CONTROL | Modifiers::ALT;
        map.insert(
            WindowAction::LeftHalf,
            Some(Hotkey::new(base, KeyCode::Left)),
        );
        map.insert(
            WindowAction::RightHalf,
            Some(Hotkey::new(base, KeyCode::Right)),
        );
        map.insert(WindowAction::TopHalf, Some(Hotkey::new(base, KeyCode::Up)));
        map.insert(
            WindowAction::BottomHalf,
            Some(Hotkey::new(base, KeyCode::Down)),
        );
        map.insert(
            WindowAction::Maximize,
            Some(Hotkey::new(base, KeyCode::Enter)),
        );
        map.insert(WindowAction::Center, Some(Hotkey::new(base, KeyCode::C)));
        map.insert(
            WindowAction::Restore,
            Some(Hotkey::new(base, KeyCode::Backspace)),
        );
    }

    #[cfg(not(target_os = "macos"))]
    {
        let win = Modifiers::META;
        let win_alt = Modifiers::META | Modifiers::ALT;
        map.insert(
            WindowAction::LeftHalf,
            Some(Hotkey::new(win, KeyCode::Left)),
        );
        map.insert(
            WindowAction::RightHalf,
            Some(Hotkey::new(win, KeyCode::Right)),
        );
        map.insert(WindowAction::Maximize, Some(Hotkey::new(win, KeyCode::Up)));
        map.insert(WindowAction::Restore, Some(Hotkey::new(win, KeyCode::Down)));
        map.insert(
            WindowAction::TopHalf,
            Some(Hotkey::new(win_alt, KeyCode::Up)),
        );
        map.insert(
            WindowAction::BottomHalf,
            Some(Hotkey::new(win_alt, KeyCode::Down)),
        );
        map.insert(WindowAction::Center, Some(Hotkey::new(win_alt, KeyCode::C)));
    }

    map
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The actions that ship with a default binding. The wider catalogue
    /// (corners, thirds, fourths, sixths, ninths, size variants) is deliberately
    /// unbound so the defaults stay conflict-free and users can pick their own.
    const CORE_BOUND: [WindowAction; 7] = [
        WindowAction::LeftHalf,
        WindowAction::RightHalf,
        WindowAction::TopHalf,
        WindowAction::BottomHalf,
        WindowAction::Maximize,
        WindowAction::Center,
        WindowAction::Restore,
    ];

    #[test]
    fn defaults_bind_the_core_actions_without_conflicts() {
        let config = Config::default();
        for action in CORE_BOUND {
            assert!(
                config.binding(action).is_some(),
                "{action} has no default binding"
            );
        }
        // Every other action ships unbound.
        for action in WindowAction::ALL {
            if !CORE_BOUND.contains(&action) {
                assert!(
                    config.binding(action).is_none(),
                    "{action} unexpectedly has a default binding"
                );
            }
        }
        assert_eq!(
            config.conflicts(),
            vec![],
            "default bindings must not clash"
        );
    }

    #[test]
    fn defaults_round_trip_through_json() {
        let config = Config::default();
        let parsed = Config::from_json(&config.to_json().unwrap()).unwrap();
        assert_eq!(config, parsed);
    }

    #[test]
    fn missing_fields_fall_back_to_defaults() {
        let config = Config::from_json("{}").unwrap();
        assert_eq!(config, Config::default());
    }

    #[test]
    fn setting_a_binding_unbinds_the_previous_owner() {
        let mut config = Config::default();
        let hk = config.binding(WindowAction::LeftHalf).unwrap();
        config.set_binding(WindowAction::Center, Some(hk));
        assert_eq!(config.binding(WindowAction::LeftHalf), None);
        assert_eq!(config.binding(WindowAction::Center), Some(hk));
        assert_eq!(config.conflicts(), vec![]);
    }

    #[test]
    fn conflicts_are_detected() {
        let mut config = Config::default();
        let hk = config.binding(WindowAction::LeftHalf).unwrap();
        // Bypass set_binding to construct a config like a hand-edited file.
        config.bindings.insert(WindowAction::Center, Some(hk));
        let conflicts = config.conflicts();
        assert_eq!(conflicts.len(), 1);
        assert_eq!(conflicts[0].actions.len(), 2);
    }

    #[test]
    fn normalize_clamps_bad_gaps() {
        let mut config = Config {
            gaps: Gaps::uniform(-5.0),
            ..Default::default()
        };
        config.normalize();
        assert_eq!(config.gaps, Gaps::default());

        let mut config = Config {
            gaps: Gaps::uniform(10_000.0),
            ..Default::default()
        };
        config.normalize();
        assert_eq!(config.gaps, Gaps::uniform(MAX_GAP));

        let config = Config::from_json(r#"{"gap": -1}"#).unwrap();
        assert_eq!(config.gaps, Gaps::default());
    }

    #[test]
    fn legacy_scalar_gap_migrates_to_uniform_gaps() {
        // A config saved by an older build stores a plain number.
        let config = Config::from_json(r#"{"gap": 24}"#).unwrap();
        assert_eq!(config.gaps, Gaps::uniform(24.0));
        assert_eq!(config.gaps.window, 24.0);
        assert_eq!(config.gaps.edge_left, 24.0);
        assert!(!config.gaps.skip_top_edge);

        // A fractional legacy scalar still loads.
        let config = Config::from_json(r#"{"gap": 12.5}"#).unwrap();
        assert_eq!(config.gaps, Gaps::uniform(12.5));
    }

    #[test]
    fn new_object_gaps_load_and_partial_objects_fall_back_to_defaults() {
        let json = r#"{"gap": {
            "window": 8,
            "edgeTop": 0,
            "edgeBottom": 10,
            "edgeLeft": 12,
            "edgeRight": 12,
            "skipTopEdge": true,
            "mainScreenOnly": true
        }}"#;
        let config = Config::from_json(json).unwrap();
        assert_eq!(config.gaps.window, 8.0);
        assert_eq!(config.gaps.edge_top, 0.0);
        assert_eq!(config.gaps.edge_bottom, 10.0);
        assert_eq!(config.gaps.edge_left, 12.0);
        assert!(config.gaps.skip_top_edge);
        assert!(config.gaps.main_screen_only);

        // Missing object fields fall back to defaults (zero / false).
        let config = Config::from_json(r#"{"gap": {"window": 6}}"#).unwrap();
        assert_eq!(config.gaps.window, 6.0);
        assert_eq!(config.gaps.edge_left, 0.0);
        assert!(!config.gaps.main_screen_only);
    }

    #[test]
    fn gaps_object_round_trips_through_json() {
        let config = Config {
            gaps: Gaps {
                window: 8.0,
                edge_top: 0.0,
                edge_bottom: 10.0,
                edge_left: 12.0,
                edge_right: 12.0,
                skip_top_edge: true,
                main_screen_only: false,
            },
            ..Default::default()
        };
        let parsed = Config::from_json(&config.to_json().unwrap()).unwrap();
        assert_eq!(config, parsed);
    }

    #[test]
    fn active_bindings_skips_unbound_actions() {
        let mut config = Config::default();
        let before = config.active_bindings().len();
        config.set_binding(WindowAction::Center, None);
        let active = config.active_bindings();
        assert_eq!(active.len(), before - 1);
        assert!(!active.iter().any(|(_, a)| *a == WindowAction::Center));
    }
}
