//! Platform-independent hotkey description.
//!
//! Backends translate [`Hotkey`] into native registrations: a `WH_KEYBOARD_LL`
//! hook on Windows and `RegisterEventHotKey` on macOS.

use serde::{Deserialize, Serialize};
use std::fmt;
use std::str::FromStr;

/// Modifier keys, stored as a bitmask so a hotkey is cheap to compare inside a
/// keyboard hook callback.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
#[serde(transparent)]
pub struct Modifiers(pub u8);

impl Modifiers {
    pub const NONE: Modifiers = Modifiers(0);
    pub const SHIFT: Modifiers = Modifiers(1 << 0);
    pub const CONTROL: Modifiers = Modifiers(1 << 1);
    /// `Alt` on Windows, `Option` on macOS.
    pub const ALT: Modifiers = Modifiers(1 << 2);
    /// `Win` on Windows, `Command` on macOS.
    pub const META: Modifiers = Modifiers(1 << 3);

    pub const fn contains(self, other: Modifiers) -> bool {
        self.0 & other.0 == other.0
    }

    pub const fn is_empty(self) -> bool {
        self.0 == 0
    }

    pub const fn union(self, other: Modifiers) -> Modifiers {
        Modifiers(self.0 | other.0)
    }
}

impl std::ops::BitOr for Modifiers {
    type Output = Modifiers;
    fn bitor(self, rhs: Modifiers) -> Modifiers {
        self.union(rhs)
    }
}

/// The non-modifier key of a hotkey. Deliberately a small, explicit set: these
/// are the only keys the MVP needs, and each backend maps them exhaustively so
/// an unmapped key is a compile error rather than a silent no-op.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum KeyCode {
    Left,
    Right,
    Up,
    Down,
    Enter,
    Space,
    Backspace,
    Delete,
    Escape,
    Numpad0,
    Numpad1,
    Numpad2,
    Numpad3,
    Numpad4,
    Numpad5,
    Numpad6,
    Numpad7,
    Numpad8,
    Numpad9,
    C,
    F,
    M,
}

impl KeyCode {
    pub const fn label(self) -> &'static str {
        match self {
            KeyCode::Left => "Left",
            KeyCode::Right => "Right",
            KeyCode::Up => "Up",
            KeyCode::Down => "Down",
            KeyCode::Enter => "Enter",
            KeyCode::Space => "Space",
            KeyCode::Backspace => "Backspace",
            KeyCode::Delete => "Delete",
            KeyCode::Escape => "Esc",
            KeyCode::Numpad0 => "Num0",
            KeyCode::Numpad1 => "Num1",
            KeyCode::Numpad2 => "Num2",
            KeyCode::Numpad3 => "Num3",
            KeyCode::Numpad4 => "Num4",
            KeyCode::Numpad5 => "Num5",
            KeyCode::Numpad6 => "Num6",
            KeyCode::Numpad7 => "Num7",
            KeyCode::Numpad8 => "Num8",
            KeyCode::Numpad9 => "Num9",
            KeyCode::C => "C",
            KeyCode::F => "F",
            KeyCode::M => "M",
        }
    }

    pub const ALL: [KeyCode; 22] = [
        KeyCode::Left,
        KeyCode::Right,
        KeyCode::Up,
        KeyCode::Down,
        KeyCode::Enter,
        KeyCode::Space,
        KeyCode::Backspace,
        KeyCode::Delete,
        KeyCode::Escape,
        KeyCode::Numpad0,
        KeyCode::Numpad1,
        KeyCode::Numpad2,
        KeyCode::Numpad3,
        KeyCode::Numpad4,
        KeyCode::Numpad5,
        KeyCode::Numpad6,
        KeyCode::Numpad7,
        KeyCode::Numpad8,
        KeyCode::Numpad9,
        KeyCode::C,
        KeyCode::F,
        KeyCode::M,
    ];
}

/// A modifier combination plus a key.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct Hotkey {
    pub modifiers: Modifiers,
    pub key: KeyCode,
}

impl Hotkey {
    pub const fn new(modifiers: Modifiers, key: KeyCode) -> Self {
        Self { modifiers, key }
    }

    /// A hotkey with no modifiers would swallow ordinary typing, so it is
    /// rejected at the configuration boundary.
    pub const fn is_valid(&self) -> bool {
        !self.modifiers.is_empty()
    }

    /// True when this hotkey uses the Windows key. Such hotkeys cannot be
    /// registered with `RegisterHotKey` when the shell already owns them, which
    /// is why the Windows backend uses a low-level keyboard hook instead.
    pub const fn uses_meta(&self) -> bool {
        self.modifiers.contains(Modifiers::META)
    }
}

impl fmt::Display for Hotkey {
    /// Renders in the platform's conventional order, e.g. `Ctrl+Alt+Left`.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let m = self.modifiers;
        if m.contains(Modifiers::CONTROL) {
            f.write_str("Ctrl+")?;
        }
        if m.contains(Modifiers::ALT) {
            f.write_str("Alt+")?;
        }
        if m.contains(Modifiers::SHIFT) {
            f.write_str("Shift+")?;
        }
        if m.contains(Modifiers::META) {
            f.write_str(if cfg!(target_os = "macos") {
                "Cmd+"
            } else {
                "Win+"
            })?;
        }
        f.write_str(self.key.label())
    }
}

/// Error returned when parsing a hotkey string such as `"Ctrl+Alt+Left"`.
#[derive(Debug, thiserror::Error)]
pub enum ParseHotkeyError {
    #[error("hotkey string is empty")]
    Empty,
    #[error("unknown key or modifier: {0}")]
    UnknownToken(String),
    #[error("hotkey must contain at least one modifier")]
    NoModifier,
}

impl FromStr for Hotkey {
    type Err = ParseHotkeyError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let mut modifiers = Modifiers::NONE;
        let mut key = None;

        for token in s.split('+').map(str::trim).filter(|t| !t.is_empty()) {
            let lower = token.to_ascii_lowercase();
            match lower.as_str() {
                "ctrl" | "control" => modifiers = modifiers | Modifiers::CONTROL,
                "alt" | "option" | "opt" => modifiers = modifiers | Modifiers::ALT,
                "shift" => modifiers = modifiers | Modifiers::SHIFT,
                "win" | "cmd" | "command" | "meta" | "super" => {
                    modifiers = modifiers | Modifiers::META
                }
                _ => {
                    let found = KeyCode::ALL
                        .iter()
                        .copied()
                        .find(|k| k.label().eq_ignore_ascii_case(token))
                        .ok_or_else(|| ParseHotkeyError::UnknownToken(token.to_string()))?;
                    key = Some(found);
                }
            }
        }

        let key = key.ok_or(ParseHotkeyError::Empty)?;
        let hotkey = Hotkey::new(modifiers, key);
        if !hotkey.is_valid() {
            return Err(ParseHotkeyError::NoModifier);
        }
        Ok(hotkey)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn modifiers_contains_is_a_subset_check() {
        let m = Modifiers::CONTROL | Modifiers::ALT;
        assert!(m.contains(Modifiers::CONTROL));
        assert!(m.contains(Modifiers::CONTROL | Modifiers::ALT));
        assert!(!m.contains(Modifiers::SHIFT));
        // An exact-equality bug would make this fail.
        assert!(m.contains(Modifiers::NONE));
    }

    #[test]
    fn hotkeys_round_trip_through_strings() {
        for s in ["Ctrl+Alt+Left", "Win+Alt+Up", "Ctrl+Alt+Shift+M"] {
            let parsed: Hotkey = s.parse().unwrap();
            assert_eq!(parsed.to_string().parse::<Hotkey>().unwrap(), parsed);
        }
    }

    #[test]
    fn parsing_is_case_insensitive_and_alias_aware() {
        let a: Hotkey = "ctrl+alt+left".parse().unwrap();
        let b: Hotkey = "CONTROL + OPTION + Left".parse().unwrap();
        assert_eq!(a, b);
        assert_eq!(
            "cmd+up".parse::<Hotkey>().unwrap().modifiers,
            Modifiers::META
        );
        assert_eq!(
            "win+up".parse::<Hotkey>().unwrap().modifiers,
            Modifiers::META
        );
    }

    #[test]
    fn modifierless_and_unknown_hotkeys_are_rejected() {
        assert!(matches!(
            "Left".parse::<Hotkey>(),
            Err(ParseHotkeyError::NoModifier)
        ));
        assert!(matches!(
            "Ctrl+Banana".parse::<Hotkey>(),
            Err(ParseHotkeyError::UnknownToken(_))
        ));
        assert!("Ctrl".parse::<Hotkey>().is_err());
    }

    #[test]
    fn key_labels_are_unique_so_parsing_is_unambiguous() {
        let mut labels: Vec<_> = KeyCode::ALL.iter().map(|k| k.label()).collect();
        labels.sort_unstable();
        let before = labels.len();
        labels.dedup();
        assert_eq!(before, labels.len());
    }
}
