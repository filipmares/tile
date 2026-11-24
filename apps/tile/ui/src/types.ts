// TypeScript mirrors of the Rust types that cross the Tauri bridge. These MUST
// match the serde representations in `tile-core` and the app's DTOs exactly.

/** Shortcut modifier groups, mirroring `ShortcutGroup::ALL`. */
export type ShortcutGroup = "placement" | "displays";

export const SHORTCUT_GROUPS: ReadonlyArray<{
  id: ShortcutGroup;
  label: string;
  modifierLabel: string;
  description: string;
}> = [
  {
    id: "placement",
    label: "Window placement",
    modifierLabel: "Base modifier",
    description:
      "Hold the base modifier and press a key. Repeat Left, Right, or Center Column to cycle widths.",
  },
  {
    id: "displays",
    label: "Displays",
    modifierLabel: "Display modifier",
    description:
      "Hold the display modifier and press a key to move the window to another display.",
  },
];

/**
 * Order, labels and groups mirror `WindowAction::ALL`, `WindowAction::label`
 * and `WindowAction::group`.
 */
export const ACTIONS = [
  { id: "left-half", label: "Left", group: "placement" },
  { id: "right-half", label: "Right", group: "placement" },
  { id: "maximize", label: "Maximize", group: "placement" },
  { id: "restore", label: "Restore", group: "placement" },
  { id: "center-half", label: "Center Column", group: "placement" },
  { id: "display-left", label: "Display Left", group: "displays" },
  { id: "display-right", label: "Display Right", group: "displays" },
  { id: "display-up", label: "Display Above", group: "displays" },
  { id: "display-down", label: "Display Below", group: "displays" },
] as const satisfies readonly {
  id: string;
  label: string;
  group: ShortcutGroup;
}[];

/** `WindowAction` — serde `rename_all = "kebab-case"`. */
export type WindowAction = (typeof ACTIONS)[number]["id"];

/** Composed-only corner identifiers reported by `tile://action-performed`. */
export type CornerAction =
  | "top-left"
  | "top-right"
  | "bottom-left"
  | "bottom-right";

/** `ActionTarget` — either a bindable action or a composed corner. */
export type ActionTarget = WindowAction | CornerAction;

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
}

/** `Config` — serde `rename_all = "camelCase"`, schema v2. */
export interface Config {
  placementModifiers: number;
  displayModifiers: number;
  keys: Partial<Record<WindowAction, KeyCode | null>>;
  gap: Gaps;
  launchOnLogin: boolean;
  subsequentExecutionMode: SubsequentExecutionMode;
  cycleSizes: CycleSize[];
  orientationShown: boolean;
  animation: AnimationConfig;
}

/** Returns the modifier bitmask for `group`. */
export function groupModifiers(cfg: Config, group: ShortcutGroup): number {
  return group === "placement" ? cfg.placementModifiers : cfg.displayModifiers;
}

/** Returns the full `Hotkey` bound to `action`, or `null` if unbound. */
export function bindingFor(cfg: Config, action: WindowAction): Hotkey | null {
  const key = cfg.keys[action];
  if (!key) return null;
  const meta = ACTIONS.find((a) => a.id === action);
  if (!meta) return null;
  return { modifiers: groupModifiers(cfg, meta.group), key };
}

export type PermissionStatus = "granted" | "denied" | "not-required";

export type GrantStep = "prompt" | "open-settings";

export interface AccessibilityHelp {
  grantStep: GrantStep;
  appBundle: string | null;
}

export type HotkeyRoute = "registered" | "intercepted" | "unavailable";

export interface HotkeyBindingStatus {
  hotkey: Hotkey;
  action: WindowAction;
  route: HotkeyRoute;
  reason: string | null;
}

export interface HotkeyStatus {
  bindings: HotkeyBindingStatus[];
  hookInstalled: boolean;
  hookUnavailable: boolean;
  applyError: string | null;
}

export type BuildKind = "development" | "installed";

export interface BuildInfo {
  kind: BuildKind;
  configDir: string | null;
}

export interface ConfigRecovery {
  kind: "corrupt" | "partial-reset" | "newer-version";
  someFieldsReset: boolean;
  backupPath: string | null;
}

export type UpdateStatus =
  | { status: "unavailable" }
  | { status: "idle" }
  | { status: "checking" }
  | { status: "current" }
  | {
      status: "available";
      version: string;
      notes: string | null;
      date: string | null;
    }
  | {
      status: "downloading";
      version: string;
      downloadedBytes: number;
      totalBytes: number | null;
    }
  | { status: "ready-to-relaunch"; version: string }
  | { status: "error"; kind: UpdateErrorKind };

export type UpdateErrorKind =
  | "offline"
  | "server"
  | "signature"
  | "interrupted"
  | "disk"
  | "installer"
  | "unknown";

export interface WelcomeStatus {
  screenCount: number;
  hasMovableWindow: boolean;
  currentScreen: number;
  displayNeighbors: Partial<Record<WindowAction, number>>[];
}

export interface ActionPerformed {
  action: ActionTarget;
  moved: boolean;
  hadWindow: boolean;
  screen: number | null;
}
