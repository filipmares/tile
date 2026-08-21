// TypeScript mirrors of the Rust types that cross the Tauri bridge. These MUST
// match the serde representations in `tile-core` and the app's DTOs exactly.

/** `WindowAction` — serde `rename_all = "kebab-case"`. */
export type WindowAction =
  | "left-half"
  | "right-half"
  | "top-half"
  | "bottom-half"
  | "previous-display"
  | "next-display"
  | "top-left"
  | "top-right"
  | "bottom-left"
  | "bottom-right"
  | "first-third"
  | "center-third"
  | "last-third"
  | "first-two-thirds"
  | "last-two-thirds"
  | "center-two-thirds"
  | "top-vertical-third"
  | "middle-vertical-third"
  | "bottom-vertical-third"
  | "top-vertical-two-thirds"
  | "bottom-vertical-two-thirds"
  | "first-fourth"
  | "second-fourth"
  | "third-fourth"
  | "last-fourth"
  | "first-three-fourths"
  | "last-three-fourths"
  | "center-three-fourths"
  | "top-left-third"
  | "top-right-third"
  | "bottom-left-third"
  | "bottom-right-third"
  | "top-left-sixth"
  | "top-center-sixth"
  | "top-right-sixth"
  | "bottom-left-sixth"
  | "bottom-center-sixth"
  | "bottom-right-sixth"
  | "top-left-ninth"
  | "top-center-ninth"
  | "top-right-ninth"
  | "middle-left-ninth"
  | "middle-center-ninth"
  | "middle-right-ninth"
  | "bottom-left-ninth"
  | "bottom-center-ninth"
  | "bottom-right-ninth"
  | "maximize"
  | "almost-maximize"
  | "maximize-height"
  | "center-half"
  | "center"
  | "restore"
  | "move-left"
  | "move-right"
  | "move-up"
  | "move-down"
  | "larger"
  | "smaller"
  | "larger-width"
  | "smaller-width"
  | "larger-height"
  | "smaller-height"
  | "double-width-left"
  | "double-width-right"
  | "double-height-up"
  | "double-height-down"
  | "halve-width-left"
  | "halve-width-right"
  | "halve-height-up"
  | "halve-height-down"
  | "first-display"
  | "second-display"
  | "third-display"
  | "fourth-display";

/** Presentation families, mirroring `WindowFamily::ALL` and its labels. */
export const FAMILIES: { id: string; label: string }[] = [
  { id: "halves", label: "Halves" },
  { id: "displays", label: "Displays" },
  { id: "corners", label: "Corners" },
  { id: "horizontal-thirds", label: "Horizontal Thirds" },
  { id: "vertical-thirds", label: "Vertical Thirds" },
  { id: "fourths", label: "Fourths" },
  { id: "corner-thirds", label: "Corner Thirds" },
  { id: "sixths", label: "Sixths" },
  { id: "ninths", label: "Ninths" },
  { id: "sizing", label: "Size & Position" },
  { id: "move", label: "Move" },
  { id: "resize", label: "Resize" },
  { id: "halve-double", label: "Halve & Double" },
];

/**
 * Order, labels and families mirror `WindowAction::ALL`, `WindowAction::label`
 * and `WindowAction::family`. `family` keys into {@link FAMILIES}.
 */
export const ACTIONS: { id: WindowAction; label: string; family: string }[] = [
  { id: "left-half", label: "Left Half", family: "halves" },
  { id: "right-half", label: "Right Half", family: "halves" },
  { id: "top-half", label: "Top Half", family: "halves" },
  { id: "bottom-half", label: "Bottom Half", family: "halves" },
  { id: "previous-display", label: "Previous Display", family: "displays" },
  { id: "next-display", label: "Next Display", family: "displays" },
  { id: "first-display", label: "First Display", family: "displays" },
  { id: "second-display", label: "Second Display", family: "displays" },
  { id: "third-display", label: "Third Display", family: "displays" },
  { id: "fourth-display", label: "Fourth Display", family: "displays" },
  { id: "top-left", label: "Top Left", family: "corners" },
  { id: "top-right", label: "Top Right", family: "corners" },
  { id: "bottom-left", label: "Bottom Left", family: "corners" },
  { id: "bottom-right", label: "Bottom Right", family: "corners" },
  { id: "first-third", label: "First Third", family: "horizontal-thirds" },
  { id: "center-third", label: "Center Third", family: "horizontal-thirds" },
  { id: "last-third", label: "Last Third", family: "horizontal-thirds" },
  {
    id: "first-two-thirds",
    label: "First Two Thirds",
    family: "horizontal-thirds",
  },
  {
    id: "last-two-thirds",
    label: "Last Two Thirds",
    family: "horizontal-thirds",
  },
  {
    id: "center-two-thirds",
    label: "Center Two Thirds",
    family: "horizontal-thirds",
  },
  {
    id: "top-vertical-third",
    label: "Top Vertical Third",
    family: "vertical-thirds",
  },
  {
    id: "middle-vertical-third",
    label: "Middle Vertical Third",
    family: "vertical-thirds",
  },
  {
    id: "bottom-vertical-third",
    label: "Bottom Vertical Third",
    family: "vertical-thirds",
  },
  {
    id: "top-vertical-two-thirds",
    label: "Top Vertical Two Thirds",
    family: "vertical-thirds",
  },
  {
    id: "bottom-vertical-two-thirds",
    label: "Bottom Vertical Two Thirds",
    family: "vertical-thirds",
  },
  { id: "first-fourth", label: "First Fourth", family: "fourths" },
  { id: "second-fourth", label: "Second Fourth", family: "fourths" },
  { id: "third-fourth", label: "Third Fourth", family: "fourths" },
  { id: "last-fourth", label: "Last Fourth", family: "fourths" },
  {
    id: "first-three-fourths",
    label: "First Three Fourths",
    family: "fourths",
  },
  {
    id: "last-three-fourths",
    label: "Last Three Fourths",
    family: "fourths",
  },
  {
    id: "center-three-fourths",
    label: "Center Three Fourths",
    family: "fourths",
  },
  { id: "top-left-third", label: "Top Left Third", family: "corner-thirds" },
  { id: "top-right-third", label: "Top Right Third", family: "corner-thirds" },
  {
    id: "bottom-left-third",
    label: "Bottom Left Third",
    family: "corner-thirds",
  },
  {
    id: "bottom-right-third",
    label: "Bottom Right Third",
    family: "corner-thirds",
  },
  { id: "top-left-sixth", label: "Top Left Sixth", family: "sixths" },
  { id: "top-center-sixth", label: "Top Center Sixth", family: "sixths" },
  { id: "top-right-sixth", label: "Top Right Sixth", family: "sixths" },
  { id: "bottom-left-sixth", label: "Bottom Left Sixth", family: "sixths" },
  {
    id: "bottom-center-sixth",
    label: "Bottom Center Sixth",
    family: "sixths",
  },
  {
    id: "bottom-right-sixth",
    label: "Bottom Right Sixth",
    family: "sixths",
  },
  { id: "top-left-ninth", label: "Top Left Ninth", family: "ninths" },
  { id: "top-center-ninth", label: "Top Center Ninth", family: "ninths" },
  { id: "top-right-ninth", label: "Top Right Ninth", family: "ninths" },
  { id: "middle-left-ninth", label: "Middle Left Ninth", family: "ninths" },
  {
    id: "middle-center-ninth",
    label: "Middle Center Ninth",
    family: "ninths",
  },
  {
    id: "middle-right-ninth",
    label: "Middle Right Ninth",
    family: "ninths",
  },
  { id: "bottom-left-ninth", label: "Bottom Left Ninth", family: "ninths" },
  {
    id: "bottom-center-ninth",
    label: "Bottom Center Ninth",
    family: "ninths",
  },
  {
    id: "bottom-right-ninth",
    label: "Bottom Right Ninth",
    family: "ninths",
  },
  { id: "maximize", label: "Maximize", family: "sizing" },
  { id: "almost-maximize", label: "Almost Maximize", family: "sizing" },
  { id: "maximize-height", label: "Maximize Height", family: "sizing" },
  { id: "center-half", label: "Center Half", family: "sizing" },
  { id: "center", label: "Center", family: "sizing" },
  { id: "restore", label: "Restore", family: "sizing" },
  { id: "move-left", label: "Move Left", family: "move" },
  { id: "move-right", label: "Move Right", family: "move" },
  { id: "move-up", label: "Move Up", family: "move" },
  { id: "move-down", label: "Move Down", family: "move" },
  { id: "larger", label: "Larger", family: "resize" },
  { id: "smaller", label: "Smaller", family: "resize" },
  { id: "larger-width", label: "Larger Width", family: "resize" },
  { id: "smaller-width", label: "Smaller Width", family: "resize" },
  { id: "larger-height", label: "Larger Height", family: "resize" },
  { id: "smaller-height", label: "Smaller Height", family: "resize" },
  {
    id: "double-width-left",
    label: "Double Width Left",
    family: "halve-double",
  },
  {
    id: "double-width-right",
    label: "Double Width Right",
    family: "halve-double",
  },
  {
    id: "double-height-up",
    label: "Double Height Up",
    family: "halve-double",
  },
  {
    id: "double-height-down",
    label: "Double Height Down",
    family: "halve-double",
  },
  {
    id: "halve-width-left",
    label: "Halve Width Left",
    family: "halve-double",
  },
  {
    id: "halve-width-right",
    label: "Halve Width Right",
    family: "halve-double",
  },
  { id: "halve-height-up", label: "Halve Height Up", family: "halve-double" },
  {
    id: "halve-height-down",
    label: "Halve Height Down",
    family: "halve-double",
  },

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

/** `Gaps` — serde `rename_all = "camelCase"`, persisted under the `gap` key. */
export interface Gaps {
  window: number;
  edgeTop: number;
  edgeBottom: number;
  edgeLeft: number;
  edgeRight: number;
  skipTopEdge: boolean;
  mainScreenOnly: boolean;
}

/** `SubsequentExecutionMode` — what a repeated press of an action does. */
export type SubsequentExecutionMode = "cycle-sizes" | "do-nothing";

/** `CycleSize` — one step of a size cycle. */
export type CycleSize =
  | "one-quarter"
  | "one-third"
  | "one-half"
  | "two-thirds"
  | "three-quarters";

/** Cycle sizes in the canonical cycle order, with their labels. */
export const CYCLE_SIZES: ReadonlyArray<{ id: CycleSize; label: string }> = [
  { id: "one-half", label: "½" },
  { id: "two-thirds", label: "⅔" },
  { id: "three-quarters", label: "¾" },
  { id: "one-quarter", label: "¼" },
  { id: "one-third", label: "⅓" },
];

/** `AnimationConfig` — how a window travels to its new frame. */
export interface AnimationConfig {
  enabled: boolean;
  durationMs: number;
  fps: number;
}

/** `Config` — serde `rename_all = "camelCase"`. */
export interface Config {
  bindings: Partial<Record<WindowAction, Hotkey | null>>;
  gap: Gaps;
  launchOnLogin: boolean;
  showTrayIcon: boolean;
  almostMaximizeWidth: number;
  almostMaximizeHeight: number;
  sizeStep: number;
  widthStep: number;
  moveStep: number;
  minimumWindowWidth: number;
  minimumWindowHeight: number;
  subsequentExecutionMode: SubsequentExecutionMode;
  cycleSizes: CycleSize[];
  animation: AnimationConfig;
}

export type PermissionStatus = "granted" | "denied" | "not-required";

export interface HotkeyFailure {
  hotkey: Hotkey;
  action: WindowAction;
  reason: string;
}
