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

/// Declares [`KeyCode`] once and derives `label()`, `ALL` and `COUNT` from that
/// single list.
///
/// A hand-written `ALL` array would be the obvious alternative, but nothing
/// would force it to stay in sync with the enum: a variant missing from `ALL`
/// is invisible to `FromStr` (the key becomes unparseable) *and* to every test
/// that iterates `ALL` looking for duplicate platform key codes. Generating
/// both from one list makes that class of bug unrepresentable.
///
/// The backends are unaffected: they still `match` on `KeyCode` exhaustively
/// with no wildcard arm, so adding a line here is a compile error until both
/// platform tables map it.
macro_rules! key_codes {
    (@unit $variant:ident) => { () };
    (@count $($variant:ident)*) => { <[()]>::len(&[$(key_codes!(@unit $variant)),*]) };
    ($($variant:ident => $label:literal,)+) => {
        /// The non-modifier key of a hotkey.
        ///
        /// A closed enum rather than a raw platform scan code, so a config file
        /// stays portable between Windows and macOS and so every backend is
        /// forced to map every key.
        ///
        /// Serialised kebab-case into the user's config, therefore **variants
        /// must never be renamed** — that would silently invalidate saved
        /// bindings. Adding variants is backward compatible.
        #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
        #[serde(rename_all = "kebab-case")]
        pub enum KeyCode {
            $($variant,)+
        }

        impl KeyCode {
            /// Human-readable name, and the token [`Hotkey`]'s `FromStr`
            /// accepts (case-insensitively).
            ///
            /// Labels must be unique and must never contain `+`, because
            /// `Hotkey` renders as `Ctrl+Alt+Left` and parses by splitting on
            /// `+`. Both invariants are covered by tests.
            pub const fn label(self) -> &'static str {
                match self {
                    $(KeyCode::$variant => $label,)+
                }
            }

            /// Number of keys in [`KeyCode::ALL`].
            pub const COUNT: usize = key_codes!(@count $($variant)+);

            /// Every key, in a stable, roughly keyboard-ordered sequence.
            pub const ALL: [KeyCode; Self::COUNT] = [$(KeyCode::$variant,)+];
        }
    };
}

key_codes! {
    // --- navigation and editing ---
    Left => "Left",
    Right => "Right",
    Up => "Up",
    Down => "Down",
    Enter => "Enter",
    Space => "Space",
    Backspace => "Backspace",
    Delete => "Delete",
    Escape => "Esc",
    Tab => "Tab",
    Insert => "Insert",
    Home => "Home",
    End => "End",
    PageUp => "PageUp",
    PageDown => "PageDown",

    // --- letters ---
    A => "A",
    B => "B",
    C => "C",
    D => "D",
    E => "E",
    F => "F",
    G => "G",
    H => "H",
    I => "I",
    J => "J",
    K => "K",
    L => "L",
    M => "M",
    N => "N",
    O => "O",
    P => "P",
    Q => "Q",
    R => "R",
    S => "S",
    T => "T",
    U => "U",
    V => "V",
    W => "W",
    X => "X",
    Y => "Y",
    Z => "Z",

    // --- top-row digits (distinct from the numpad digits below) ---
    Digit0 => "0",
    Digit1 => "1",
    Digit2 => "2",
    Digit3 => "3",
    Digit4 => "4",
    Digit5 => "5",
    Digit6 => "6",
    Digit7 => "7",
    Digit8 => "8",
    Digit9 => "9",

    // --- punctuation, named after the physical US-layout key ---
    // Word labels, not the symbols themselves: `Equals` cannot be confused
    // with the `+` separator, and every label stays a single safe token.
    Backtick => "Backtick",
    Minus => "Minus",
    Equals => "Equals",
    LeftBracket => "LeftBracket",
    RightBracket => "RightBracket",
    Backslash => "Backslash",
    Semicolon => "Semicolon",
    Quote => "Quote",
    Comma => "Comma",
    Period => "Period",
    Slash => "Slash",

    // --- function keys ---
    F1 => "F1",
    F2 => "F2",
    F3 => "F3",
    F4 => "F4",
    F5 => "F5",
    F6 => "F6",
    F7 => "F7",
    F8 => "F8",
    F9 => "F9",
    F10 => "F10",
    F11 => "F11",
    F12 => "F12",
    F13 => "F13",
    F14 => "F14",
    F15 => "F15",
    F16 => "F16",
    F17 => "F17",
    F18 => "F18",
    F19 => "F19",
    F20 => "F20",
    F21 => "F21",
    F22 => "F22",
    F23 => "F23",
    F24 => "F24",

    // --- numeric keypad ---
    Numpad0 => "Num0",
    Numpad1 => "Num1",
    Numpad2 => "Num2",
    Numpad3 => "Num3",
    Numpad4 => "Num4",
    Numpad5 => "Num5",
    Numpad6 => "Num6",
    Numpad7 => "Num7",
    Numpad8 => "Num8",
    Numpad9 => "Num9",
    NumpadAdd => "NumAdd",
    NumpadSubtract => "NumSubtract",
    NumpadMultiply => "NumMultiply",
    NumpadDivide => "NumDivide",
    NumpadDecimal => "NumDecimal",
    NumpadEnter => "NumEnter",
}

/// Modifier tokens [`Hotkey`]'s `FromStr` recognises, in lowercase.
///
/// Only used by the test that proves no key label collides with one: modifiers
/// are matched first, so a colliding key would be permanently unbindable.
#[cfg(test)]
const MODIFIER_TOKENS: [&str; 11] = [
    "ctrl", "control", "alt", "option", "opt", "shift", "win", "cmd", "command", "meta", "super",
];

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

    #[test]
    fn key_labels_are_unique_case_insensitively() {
        // `FromStr` compares with `eq_ignore_ascii_case`, so labels that differ
        // only in case would still be ambiguous.
        let mut labels: Vec<String> = KeyCode::ALL
            .iter()
            .map(|k| k.label().to_ascii_lowercase())
            .collect();
        labels.sort();
        let before = labels.len();
        labels.dedup();
        assert_eq!(before, labels.len(), "labels must differ by more than case");
    }

    #[test]
    fn no_key_label_contains_the_hotkey_separator() {
        // `Display` joins with `+` and `FromStr` splits on it, so a label
        // containing `+` (e.g. naming the numpad plus key "+") would make its
        // own hotkeys unparseable.
        for key in KeyCode::ALL {
            assert!(
                !key.label().contains('+'),
                "{key:?} has a label containing '+': {}",
                key.label()
            );
        }
    }

    #[test]
    fn key_labels_are_non_empty_and_free_of_whitespace() {
        // `FromStr` trims tokens and drops empty ones, so a label made of (or
        // containing) whitespace could never be parsed back.
        for key in KeyCode::ALL {
            let label = key.label();
            assert!(!label.is_empty(), "{key:?} has an empty label");
            assert!(
                !label.chars().any(char::is_whitespace),
                "{key:?} has whitespace in its label: {label:?}"
            );
        }
    }

    #[test]
    fn no_key_label_collides_with_a_modifier_token() {
        // Modifiers are matched before keys, so a key labelled e.g. "Alt"
        // would be impossible to bind.
        for key in KeyCode::ALL {
            let lower = key.label().to_ascii_lowercase();
            assert!(
                !MODIFIER_TOKENS.contains(&lower.as_str()),
                "{key:?}'s label shadows the modifier token {lower:?}"
            );
        }
    }

    #[test]
    fn every_key_round_trips_through_display_and_parse() {
        for key in KeyCode::ALL {
            let hotkey = Hotkey::new(Modifiers::CONTROL | Modifiers::ALT, key);
            let rendered = hotkey.to_string();
            let parsed: Hotkey = rendered
                .parse()
                .unwrap_or_else(|e| panic!("{rendered:?} ({key:?}) failed to parse: {e}"));
            assert_eq!(parsed, hotkey, "{rendered:?} did not round-trip");
        }
    }

    #[test]
    fn every_key_round_trips_through_serde() {
        // The kebab-case names are written into the user's config file, so a
        // variant that does not survive a JSON round-trip would drop bindings.
        for key in KeyCode::ALL {
            let json = serde_json::to_string(&key).unwrap();
            let back: KeyCode = serde_json::from_str(&json).unwrap();
            assert_eq!(back, key, "{key:?} did not survive serde as {json}");
        }
    }

    #[test]
    fn existing_key_names_keep_their_serialised_form() {
        // Renaming a variant silently invalidates saved bindings, so the names
        // that shipped are pinned here.
        for (key, name) in [
            (KeyCode::Left, "\"left\""),
            (KeyCode::Escape, "\"escape\""),
            (KeyCode::Backspace, "\"backspace\""),
            (KeyCode::Numpad0, "\"numpad0\""),
            (KeyCode::C, "\"c\""),
            (KeyCode::F, "\"f\""),
            (KeyCode::M, "\"m\""),
        ] {
            assert_eq!(serde_json::to_string(&key).unwrap(), name);
        }
    }

    #[test]
    fn all_contains_every_key_exactly_once() {
        let mut seen = std::collections::HashSet::new();
        for key in KeyCode::ALL {
            assert!(seen.insert(key), "{key:?} appears twice in ALL");
        }
        assert_eq!(seen.len(), KeyCode::COUNT);
    }

    // A full keyboard, not the 22-key MVP set.
    const _: () = assert!(KeyCode::COUNT >= 100, "unexpectedly small key set");

    #[test]
    fn newly_added_keys_are_parseable() {
        assert_eq!("Ctrl+Alt+1".parse::<Hotkey>().unwrap().key, KeyCode::Digit1);
        assert_eq!("ctrl+alt+f13".parse::<Hotkey>().unwrap().key, KeyCode::F13);
        assert_eq!(
            "Ctrl+Alt+NumAdd".parse::<Hotkey>().unwrap().key,
            KeyCode::NumpadAdd
        );
        assert_eq!(
            "Ctrl+Alt+Equals".parse::<Hotkey>().unwrap().key,
            KeyCode::Equals
        );
        // Top-row 1 and numpad 1 are different physical keys.
        assert_ne!(
            "Ctrl+Alt+1".parse::<Hotkey>().unwrap().key,
            "Ctrl+Alt+Num1".parse::<Hotkey>().unwrap().key
        );
    }
}
