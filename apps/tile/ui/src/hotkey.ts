// Hotkey recording and display, mapping browser keyboard events onto the closed
// `KeyCode` set from `tile-core/src/hotkey.rs`.

import { Hotkey, KeyCode, MOD } from "./types";

export function isMac(): boolean {
  return /mac/i.test(navigator.userAgent) || /mac/i.test(navigator.platform);
}

/** Extracts the modifier bitmask from a keyboard event. */
export function modifiersFromEvent(e: KeyboardEvent): number {
  let m = 0;
  if (e.ctrlKey) m |= MOD.CONTROL;
  if (e.altKey) m |= MOD.ALT;
  if (e.shiftKey) m |= MOD.SHIFT;
  if (e.metaKey) m |= MOD.META;
  return m;
}

// `KeyboardEvent.code` -> `KeyCode`.
//
// `code` is the **physical** key (`KeyC` is the same key on QWERTY, AZERTY and
// Dvorak); `key` is what the layout produces and would give a different — and
// unmappable — answer per layout. `VK_*` and `kVK_*` are physical too, so this
// keeps the recorder honest with what the backends actually register.
//
// Layout-specific extras (`IntlBackslash`, `IntlRo`, `IntlYen`) have no
// `KeyCode` and are deliberately absent.
const CODE_TO_KEY = {
  // navigation and editing
  ArrowLeft: "left",
  ArrowRight: "right",
  ArrowUp: "up",
  ArrowDown: "down",
  Enter: "enter",
  Space: "space",
  Backspace: "backspace",
  Delete: "delete",
  Escape: "escape",
  Tab: "tab",
  Insert: "insert",
  Home: "home",
  End: "end",
  PageUp: "page-up",
  PageDown: "page-down",
  // letters
  KeyA: "a",
  KeyB: "b",
  KeyC: "c",
  KeyD: "d",
  KeyE: "e",
  KeyF: "f",
  KeyG: "g",
  KeyH: "h",
  KeyI: "i",
  KeyJ: "j",
  KeyK: "k",
  KeyL: "l",
  KeyM: "m",
  KeyN: "n",
  KeyO: "o",
  KeyP: "p",
  KeyQ: "q",
  KeyR: "r",
  KeyS: "s",
  KeyT: "t",
  KeyU: "u",
  KeyV: "v",
  KeyW: "w",
  KeyX: "x",
  KeyY: "y",
  KeyZ: "z",
  // top-row digits
  Digit0: "digit0",
  Digit1: "digit1",
  Digit2: "digit2",
  Digit3: "digit3",
  Digit4: "digit4",
  Digit5: "digit5",
  Digit6: "digit6",
  Digit7: "digit7",
  Digit8: "digit8",
  Digit9: "digit9",
  // punctuation
  Backquote: "backtick",
  Minus: "minus",
  Equal: "equals",
  BracketLeft: "left-bracket",
  BracketRight: "right-bracket",
  Backslash: "backslash",
  Semicolon: "semicolon",
  Quote: "quote",
  Comma: "comma",
  Period: "period",
  Slash: "slash",
  // function keys
  F1: "f1",
  F2: "f2",
  F3: "f3",
  F4: "f4",
  F5: "f5",
  F6: "f6",
  F7: "f7",
  F8: "f8",
  F9: "f9",
  F10: "f10",
  F11: "f11",
  F12: "f12",
  F13: "f13",
  F14: "f14",
  F15: "f15",
  F16: "f16",
  F17: "f17",
  F18: "f18",
  F19: "f19",
  F20: "f20",
  F21: "f21",
  F22: "f22",
  F23: "f23",
  F24: "f24",
  // numeric keypad. NumpadEnter is its own physical key, not an alias for
  // Enter — the Windows hook tells them apart via the extended flag.
  Numpad0: "numpad0",
  Numpad1: "numpad1",
  Numpad2: "numpad2",
  Numpad3: "numpad3",
  Numpad4: "numpad4",
  Numpad5: "numpad5",
  Numpad6: "numpad6",
  Numpad7: "numpad7",
  Numpad8: "numpad8",
  Numpad9: "numpad9",
  NumpadAdd: "numpad-add",
  NumpadSubtract: "numpad-subtract",
  NumpadMultiply: "numpad-multiply",
  NumpadDivide: "numpad-divide",
  NumpadDecimal: "numpad-decimal",
  NumpadEnter: "numpad-enter",
} as const;

// Typing the lookup separately does two jobs the object literal cannot: it
// proves every value above is a real `KeyCode`, and it lets us index with an
// arbitrary `event.code`.
const CODE_LOOKUP: Readonly<Record<string, KeyCode | undefined>> = CODE_TO_KEY;

/** Every `KeyCode` some `KeyboardEvent.code` maps to. */
type RecordableKey = (typeof CODE_TO_KEY)[keyof typeof CODE_TO_KEY];

/** Fails to compile unless `T` is `never`. */
type AssertNever<T extends never> = T;

/**
 * Compile-time proof that the recorder can produce every `KeyCode`. If a key is
 * added to `tile-core` but not to `CODE_TO_KEY` above, this stops being `never`
 * and the build fails naming the missing key.
 *
 * Exported only because `noUnusedLocals` would otherwise reject it.
 */
export type UnrecordableKeys = AssertNever<Exclude<KeyCode, RecordableKey>>;

export function keyCodeFromEvent(e: KeyboardEvent): KeyCode | null {
  return CODE_LOOKUP[e.code] ?? null;
}

const PURE_MODIFIERS = new Set([
  "Control",
  "Alt",
  "AltGraph",
  "Shift",
  "Meta",
  "OS",
  "CapsLock",
]);

export type RecordOutcome =
  | { kind: "bound"; hotkey: Hotkey }
  | { kind: "clear" }
  | { kind: "cancel" }
  | { kind: "pending" }
  | { kind: "error"; message: string };

/** Interprets one keydown during recording. */
export function interpret(e: KeyboardEvent): RecordOutcome {
  if (PURE_MODIFIERS.has(e.key)) {
    return { kind: "pending" };
  }
  if (e.code === "Escape") {
    return { kind: "cancel" };
  }

  const mods = modifiersFromEvent(e);

  if ((e.code === "Backspace" || e.code === "Delete") && mods === 0) {
    return { kind: "clear" };
  }

  const key = keyCodeFromEvent(e);
  if (!key) {
    const shown = e.key.length === 1 ? e.key.toUpperCase() : e.key;
    return {
      kind: "error",
      message: `"${shown}" can't be used as a shortcut key.`,
    };
  }
  if (mods === 0) {
    return {
      kind: "error",
      message: "Add at least one modifier (⌘ / Ctrl / Alt / Shift) — a bare key would swallow normal typing.",
    };
  }

  return { kind: "bound", hotkey: { modifiers: mods, key } };
}

const KEY_LABELS: Readonly<Record<KeyCode, string>> = {
  // navigation and editing
  left: "Left",
  right: "Right",
  up: "Up",
  down: "Down",
  enter: "Enter",
  space: "Space",
  backspace: "Backspace",
  delete: "Delete",
  escape: "Esc",
  tab: "Tab",
  insert: "Insert",
  home: "Home",
  end: "End",
  "page-up": "PageUp",
  "page-down": "PageDown",
  // letters
  a: "A",
  b: "B",
  c: "C",
  d: "D",
  e: "E",
  f: "F",
  g: "G",
  h: "H",
  i: "I",
  j: "J",
  k: "K",
  l: "L",
  m: "M",
  n: "N",
  o: "O",
  p: "P",
  q: "Q",
  r: "R",
  s: "S",
  t: "T",
  u: "U",
  v: "V",
  w: "W",
  x: "X",
  y: "Y",
  z: "Z",
  // top-row digits
  digit0: "0",
  digit1: "1",
  digit2: "2",
  digit3: "3",
  digit4: "4",
  digit5: "5",
  digit6: "6",
  digit7: "7",
  digit8: "8",
  digit9: "9",
  // Punctuation is shown as the US-layout symbol here, which is friendlier in
  // the UI than the word used by `KeyCode::label()`. The two never have to
  // agree: this string is display-only, and the config stores the `KeyCode`.
  backtick: "`",
  minus: "-",
  equals: "=",
  "left-bracket": "[",
  "right-bracket": "]",
  backslash: "\\",
  semicolon: ";",
  quote: "'",
  comma: ",",
  period: ".",
  slash: "/",
  // function keys
  f1: "F1",
  f2: "F2",
  f3: "F3",
  f4: "F4",
  f5: "F5",
  f6: "F6",
  f7: "F7",
  f8: "F8",
  f9: "F9",
  f10: "F10",
  f11: "F11",
  f12: "F12",
  f13: "F13",
  f14: "F14",
  f15: "F15",
  f16: "F16",
  f17: "F17",
  f18: "F18",
  f19: "F19",
  f20: "F20",
  f21: "F21",
  f22: "F22",
  f23: "F23",
  f24: "F24",
  // numeric keypad
  numpad0: "Num0",
  numpad1: "Num1",
  numpad2: "Num2",
  numpad3: "Num3",
  numpad4: "Num4",
  numpad5: "Num5",
  numpad6: "Num6",
  numpad7: "Num7",
  numpad8: "Num8",
  numpad9: "Num9",
  // Numpad operators keep their word labels (matching `KeyCode::label()`)
  // rather than symbols, so no rendered hotkey ever contains a `+`.
  "numpad-add": "NumAdd",
  "numpad-subtract": "NumSubtract",
  "numpad-multiply": "NumMultiply",
  "numpad-divide": "NumDivide",
  "numpad-decimal": "NumDecimal",
  "numpad-enter": "NumEnter",
};

/** Renders a hotkey with platform-correct modifier symbols. */
export function formatHotkey(hk: Hotkey): string {
  const mac = isMac();
  const parts: string[] = [];
  // macOS convention orders modifiers ⌃⌥⇧⌘.
  if (mac) {
    if (hk.modifiers & MOD.CONTROL) parts.push("⌃");
    if (hk.modifiers & MOD.ALT) parts.push("⌥");
    if (hk.modifiers & MOD.SHIFT) parts.push("⇧");
    if (hk.modifiers & MOD.META) parts.push("⌘");
    return parts.join("") + KEY_LABELS[hk.key];
  }
  if (hk.modifiers & MOD.CONTROL) parts.push("Ctrl");
  if (hk.modifiers & MOD.ALT) parts.push("Alt");
  if (hk.modifiers & MOD.SHIFT) parts.push("Shift");
  if (hk.modifiers & MOD.META) parts.push("Win");
  parts.push(KEY_LABELS[hk.key]);
  return parts.join("+");
}
