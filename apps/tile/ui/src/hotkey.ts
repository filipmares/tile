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

// `KeyboardEvent.code` -> `KeyCode`. Only the keys tile-core accepts appear.
const CODE_TO_KEY: Readonly<Record<string, KeyCode>> = {
  ArrowLeft: "left",
  ArrowRight: "right",
  ArrowUp: "up",
  ArrowDown: "down",
  Enter: "enter",
  NumpadEnter: "enter",
  Space: "space",
  Backspace: "backspace",
  Delete: "delete",
  Escape: "escape",
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
  KeyC: "c",
  KeyF: "f",
  KeyM: "m",
};

export function keyCodeFromEvent(e: KeyboardEvent): KeyCode | null {
  return CODE_TO_KEY[e.code] ?? null;
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
      message: `"${shown}" can't be used. Choose an arrow, Enter, Space, Backspace, Delete, a Numpad digit, or C / F / M.`,
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
  left: "Left",
  right: "Right",
  up: "Up",
  down: "Down",
  enter: "Enter",
  space: "Space",
  backspace: "Backspace",
  delete: "Delete",
  escape: "Esc",
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
  c: "C",
  f: "F",
  m: "M",
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
