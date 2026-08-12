//! User configuration: key bindings and behaviour, persisted as JSON.

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

use crate::action::WindowAction;
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

fn default_gap() -> f64 {
    0.0
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
    /// Padding in logical pixels between the window and the screen edges.
    pub gap: f64,
    /// Start Tile when the user logs in.
    pub launch_on_login: bool,
    /// Show the tray / menu-bar icon.
    pub show_tray_icon: bool,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            bindings: default_bindings(),
            gap: default_gap(),
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
        if !self.gap.is_finite() || self.gap < 0.0 {
            self.gap = 0.0;
        }
        self.gap = self.gap.min(MAX_GAP);
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

    #[test]
    fn defaults_cover_every_action_without_conflicts() {
        let config = Config::default();
        for action in WindowAction::ALL {
            assert!(
                config.binding(action).is_some(),
                "{action} has no default binding"
            );
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
            gap: -5.0,
            ..Default::default()
        };
        config.normalize();
        assert_eq!(config.gap, 0.0);

        let mut config = Config {
            gap: 10_000.0,
            ..Default::default()
        };
        config.normalize();
        assert_eq!(config.gap, MAX_GAP);

        let config = Config::from_json(r#"{"gap": -1}"#).unwrap();
        assert_eq!(config.gap, 0.0);
    }

    #[test]
    fn active_bindings_skips_unbound_actions() {
        let mut config = Config::default();
        config.set_binding(WindowAction::Center, None);
        let active = config.active_bindings();
        assert_eq!(active.len(), WindowAction::ALL.len() - 1);
        assert!(!active.iter().any(|(_, a)| *a == WindowAction::Center));
    }
}
