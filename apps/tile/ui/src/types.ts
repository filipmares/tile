// TypeScript mirrors of the Rust types that cross the Tauri bridge. These MUST
// match the serde representations in `tile-core` and the app's DTOs exactly.

/** `WindowAction` — serde `rename_all = "kebab-case"`. */
export type WindowAction =
  | "left-half"
  | "right-half"
  | "top-half"
  | "bottom-half"
  | "maximize"
  | "center"
  | "restore";

/** Order and labels mirror `WindowAction::ALL` / `WindowAction::label`. */
export const ACTIONS: { id: WindowAction; label: string }[] = [
  { id: "left-half", label: "Left Half" },
  { id: "right-half", label: "Right Half" },
  { id: "top-half", label: "Top Half" },
  { id: "bottom-half", label: "Bottom Half" },
  { id: "maximize", label: "Maximize" },
  { id: "center", label: "Center" },
  { id: "restore", label: "Restore" },
];

/** `KeyCode` — serde `rename_all = "kebab-case"`. Mirrors `KeyCode::ALL`. */
export type KeyCode =
  // navigation and editing
  | "left"
  | "right"
  | "up"
  | "down"
  | "enter"
  | "space"
  | "backspace"
  | "delete"
  | "escape"
  | "tab"
  | "insert"
  | "home"
  | "end"
  | "page-up"
  | "page-down"
  // letters
  | "a"
  | "b"
  | "c"
  | "d"
  | "e"
  | "f"
  | "g"
  | "h"
  | "i"
  | "j"
  | "k"
  | "l"
  | "m"
  | "n"
  | "o"
  | "p"
  | "q"
  | "r"
  | "s"
  | "t"
  | "u"
  | "v"
  | "w"
  | "x"
  | "y"
  | "z"
  // top-row digits
  | "digit0"
  | "digit1"
  | "digit2"
  | "digit3"
  | "digit4"
  | "digit5"
  | "digit6"
  | "digit7"
  | "digit8"
  | "digit9"
  // punctuation
  | "backtick"
  | "minus"
  | "equals"
  | "left-bracket"
  | "right-bracket"
  | "backslash"
  | "semicolon"
  | "quote"
  | "comma"
  | "period"
  | "slash"
  // function keys
  | "f1"
  | "f2"
  | "f3"
  | "f4"
  | "f5"
  | "f6"
  | "f7"
  | "f8"
  | "f9"
  | "f10"
  | "f11"
  | "f12"
  | "f13"
  | "f14"
  | "f15"
  | "f16"
  | "f17"
  | "f18"
  | "f19"
  | "f20"
  | "f21"
  | "f22"
  | "f23"
  | "f24"
  // numeric keypad
  | "numpad0"
  | "numpad1"
  | "numpad2"
  | "numpad3"
  | "numpad4"
  | "numpad5"
  | "numpad6"
  | "numpad7"
  | "numpad8"
  | "numpad9"
  | "numpad-add"
  | "numpad-subtract"
  | "numpad-multiply"
  | "numpad-divide"
  | "numpad-decimal"
  | "numpad-enter";

/** `Modifiers` is a transparent `u8` bitmask. */
export const MOD = {
  SHIFT: 1 << 0,
  CONTROL: 1 << 1,
  ALT: 1 << 2,
  META: 1 << 3,
} as const;

/** `Hotkey { modifiers, key }`; `modifiers` is the raw bitmask number. */
export interface Hotkey {
  modifiers: number;
  key: KeyCode;
}

/** `Config` — serde `rename_all = "camelCase"`. */
export interface Config {
  bindings: Partial<Record<WindowAction, Hotkey | null>>;
  gap: number;
  launchOnLogin: boolean;
  showTrayIcon: boolean;
}

export type PermissionStatus = "granted" | "denied" | "not-required";

export interface HotkeyFailure {
  hotkey: Hotkey;
  action: WindowAction;
  reason: string;
}
