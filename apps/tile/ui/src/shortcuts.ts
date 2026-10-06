// The shortcut list: every action, its binding, the filter over the list,
// and the recorder that captures a new chord.

import { listen } from "@tauri-apps/api/event";
import { getHotkeyStatus, setBinding } from "./api";
import { dom } from "./dom";
import {
  formatHotkey,
  hasAltGrRisk,
  interpret,
  isMac,
  knownWindowsShortcutWarning,
  windowsHotkeyProblem,
} from "./hotkey";
import { renderWholeConfig, saveSetting } from "./settings";
import { config } from "./state";
import {
  ACTIONS,
  Config,
  FAMILIES,
  Hotkey,
  HotkeyBindingStatus,
  HotkeyStatus,
  WindowAction,
} from "./types";

/** User-facing copy for the shortcut list and the recorder. */
const STRINGS = {
  counts: (total: number, assigned: number) =>
    `${total} shortcuts · ${assigned} assigned`,
  noneAssigned: "No shortcuts assigned yet.",
  noMatch: (filter: string) => `No shortcuts match \u201c${filter}\u201d.`,
  applyError: (error: string) =>
    `Windows could not apply the latest shortcut change. A previously active shortcut may still be in effect. ${error}`,
  hookUnavailable:
    "Some shortcuts aren't working right now. Tile will keep retrying.",
  pressKeys: "Press keys…",
  unbound: "Unbound",
  clearLabel: (label: string) => `Clear shortcut for ${label}`,
  clearGlyph: "✕",
  conflict: "This shortcut is used by more than one action.",
  unavailable: (reason: string | null | undefined) =>
    `This shortcut is unavailable: ${reason ?? "Windows rejected it."}`,
  unconfirmed:
    "This shortcut is configured, but its active route could not be confirmed.",
  recording: (label: string) =>
    `Recording ${label}. Press a shortcut, Esc to cancel, Backspace to clear.`,
  cancelled: "Recording cancelled.",
  cleared: "Shortcut cleared.",
  altGrConfirm:
    "Windows treats Ctrl+Alt as AltGr on many keyboard layouts. This shortcut may prevent typing characters such as @, €, {, or }. Save it anyway?",
  warningConfirm: (warning: string) => `${warning}\n\nSave it anyway?`,
  notSaved: "Shortcut was not saved.",
};

let hotkeyStatus: HotkeyStatus = {
  bindings: [],
  hookInstalled: false,
  hookUnavailable: false,
  applyError: null,
};
let recording: WindowAction | null = null;
/** Current text in the shortcut filter. Empty means "show everything". */
let shortcutFilter = "";
/** The user's family open/closed state, stashed while a filter is active. */
let openBeforeFilter: Set<string> | null = null;

/** Actions sharing a hotkey (only possible from a hand-edited config). */
function conflictingActions(cfg: Config): Set<WindowAction> {
  const seen = new Map<string, WindowAction[]>();
  for (const { id } of ACTIONS) {
    const hk = cfg.bindings[id];
    if (!hk) continue;
    const key = `${hk.modifiers}:${hk.key}`;
    const list = seen.get(key) ?? [];
    list.push(id);
    seen.set(key, list);
  }
  const clashing = new Set<WindowAction>();
  for (const list of seen.values()) {
    if (list.length > 1) list.forEach((a) => clashing.add(a));
  }
  return clashing;
}

function sameHotkey(left: Hotkey, right: Hotkey): boolean {
  return left.modifiers === right.modifiers && left.key === right.key;
}

function statusFor(
  action: WindowAction,
  hotkey: Hotkey | null,
): HotkeyBindingStatus | undefined {
  if (!hotkey) return undefined;
  return hotkeyStatus.bindings.find(
    (status) =>
      status.action === action && sameHotkey(status.hotkey, hotkey),
  );
}

export async function refreshHotkeyStatus(): Promise<void> {
  hotkeyStatus = await getHotkeyStatus();
}

function renderHotkeyHealth(): void {
  dom.hotkeyHookWarning.hidden = !hotkeyStatus.hookUnavailable;
  dom.hotkeyHookWarning.textContent = hotkeyStatus.hookUnavailable
    ? STRINGS.hookUnavailable
    : "";
}

/**
 * Follows the status the app pushes after every apply and every recovery.
 * The list itself is only rebuilt when a route changed and nothing is being
 * recorded, so a recovery in the background never steals focus or ends a
 * recording.
 */
export async function listenForHotkeyStatus(): Promise<void> {
  await listen<HotkeyStatus>("hotkey-status-changed", ({ payload }) => {
    const routesChanged =
      JSON.stringify(payload.bindings) !==
        JSON.stringify(hotkeyStatus.bindings) ||
      payload.applyError !== hotkeyStatus.applyError;
    hotkeyStatus = payload;
    renderHotkeyHealth();
    if (routesChanged && config && recording === null) renderBindings();
  });
}

export function renderBindings(): void {
  if (!config) return;
  renderHotkeyHealth();
  renderHotkeyApplyError();
  const cfg = config;
  const conflicts = conflictingActions(cfg);
  const assignedActions = ACTIONS.filter(({ id }) => cfg.bindings[id]);
  dom.assignedBindings.replaceChildren();
  dom.allShortcutsCount.textContent = STRINGS.counts(
    ACTIONS.length,
    assignedActions.length,
  );

  if (assignedActions.length === 0) {
    const empty = document.createElement("li");
    empty.className = "shortcut-empty";
    empty.textContent = STRINGS.noneAssigned;
    dom.assignedBindings.append(empty);
  } else {
    for (const action of assignedActions) {
      dom.assignedBindings.append(
        renderBinding(cfg, conflicts, action.id, action.label, "assigned"),
      );
    }

  }

  const filter = shortcutFilter.trim().toLowerCase();
  const matchesFilter = (label: string): boolean =>
    label.toLowerCase().includes(filter);

  const hasRendered = dom.bindings.childElementCount > 0;
  const currentlyOpen = new Set(
    [...dom.bindings.querySelectorAll<HTMLDetailsElement>("details[open]")]
      .map((details) => details.dataset.family)
      .filter((family): family is string => family !== undefined),
  );

  // Filtering force-opens every matching family, which would otherwise
  // overwrite the user's own open/closed state. Stash it on the way in and
  // put it back when the filter clears. The stash has to be read into a local
  // first: clearing it before the read would silently discard it.
  const restore = filter ? null : openBeforeFilter;
  if (filter && openBeforeFilter === null) {
    openBeforeFilter = currentlyOpen;
  } else if (!filter) {
    openBeforeFilter = null;
  }

  function renderHotkeyApplyError(): void {
    const error = hotkeyStatus.applyError;
    dom.hotkeyApplyError.hidden = error === null;
    dom.hotkeyApplyError.textContent =
      error === null
        ? ""
        : STRINGS.applyError(error);
  }
  const openFamilies = filter ? currentlyOpen : (restore ?? currentlyOpen);

  dom.bindings.replaceChildren();
  let shown = 0;

  for (const family of FAMILIES) {
    const actions = ACTIONS.filter((a) => a.family === family.id);
    if (actions.length === 0) continue;

    const matches = filter ? actions.filter((a) => matchesFilter(a.label)) : actions;
    // A family with nothing to show is noise while filtering.
    if (matches.length === 0) continue;

    const group = document.createElement("li");
    group.className = "binding-group";

    const disclosure = document.createElement("details");
    disclosure.className = "binding-group__disclosure";
    disclosure.dataset.family = family.id;
    // While filtering, every surviving family opens: a match hidden inside a
    // collapsed group is the one thing a filter must never do. The user's own
    // open/closed state is restored as soon as the filter is cleared.
    disclosure.open = filter
      ? true
      : hasRendered
        ? openFamilies.has(family.id)
        : family.id === "halves" || actions.some(({ id }) => cfg.bindings[id]);

    const summary = document.createElement("summary");
    summary.className = "binding-group__summary";

    const heading = document.createElement("span");
    heading.className = "binding-group__title";
    heading.textContent = family.label;

    // Counts describe the family, not the filter. A number that moved while
    // typing would read as a bug rather than as information.
    const assignedCount = actions.filter(({ id }) => cfg.bindings[id]).length;
    const count = document.createElement("span");
    count.className = "binding-group__count";
    count.textContent = STRINGS.counts(actions.length, assignedCount);

    summary.append(heading, count);
    disclosure.append(summary);

    const list = document.createElement("ul");
    list.className = "binding-group__list";

    for (const { id, label } of matches) {
      list.append(renderBinding(cfg, conflicts, id, label, "all"));
    }

    disclosure.append(list);
    group.append(disclosure);
    dom.bindings.append(group);
    shown += matches.length;
  }

  if (filter && shown === 0) {
    const empty = document.createElement("li");
    empty.className = "shortcut-empty";
    empty.textContent = STRINGS.noMatch(shortcutFilter.trim());
    dom.bindings.append(empty);
  }
}

function renderBinding(
  cfg: Config,
  conflicts: Set<WindowAction>,
  id: WindowAction,
  label: string,
  scope: "assigned" | "all",
): HTMLLIElement {
  const hk = cfg.bindings[id] ?? null;

  const li = document.createElement("li");
  li.className = "binding";

  const name = document.createElement("span");
  name.className = "binding__label";
  name.textContent = label;
  name.id = `label-${scope}-${id}`;

  const controls = document.createElement("div");
  controls.className = "binding__controls";

  const record = document.createElement("button");
  record.type = "button";
  record.className = "binding__key";
  record.setAttribute("aria-labelledby", `label-${scope}-${id} key-${scope}-${id}`);
  record.id = `key-${scope}-${id}`;
  if (recording === id) {
    record.classList.add("binding__key--recording");
    record.textContent = STRINGS.pressKeys;
  } else {
    record.textContent = hk ? formatHotkey(hk) : STRINGS.unbound;
    if (!hk) record.classList.add("binding__key--empty");
  }
  record.addEventListener("click", () => startRecording(id));
  controls.append(record);

  if (hk && recording !== id) {
    const clear = document.createElement("button");
    clear.type = "button";
    clear.className = "binding__clear";
    clear.setAttribute("aria-label", STRINGS.clearLabel(label));
    clear.textContent = STRINGS.clearGlyph;
    clear.addEventListener("click", () => void applyBinding(id, null));
    controls.append(clear);
  }

  li.append(name, controls);

  const route = statusFor(id, hk);
  if (conflicts.has(id)) {
    li.append(note(STRINGS.conflict, "error"));
  } else if (route?.route === "unavailable") {
    li.append(
      note(STRINGS.unavailable(route.reason), "error"),
    );
  } else if (hk && hotkeyStatus.applyError) {
    li.append(
      note(STRINGS.unconfirmed, "error"),
    );
  }

  return li;
}

function note(text: string, kind: "error" | "info"): HTMLElement {
  const p = document.createElement("p");
  p.className = `binding__note binding__note--${kind}`;
  p.textContent = text;
  return p;
}

export function setRecordingStatus(text: string): void {
  dom.recordingStatus.textContent = text;
}

function startRecording(action: WindowAction): void {
  recording = action;
  const label = ACTIONS.find((a) => a.id === action)?.label ?? action;
  setRecordingStatus(STRINGS.recording(label));
  renderBindings();
  window.addEventListener("keydown", onRecordKey, { capture: true });
}

function stopRecording(): void {
  recording = null;
  window.removeEventListener("keydown", onRecordKey, { capture: true });
  renderBindings();
}

function onRecordKey(e: KeyboardEvent): void {
  if (recording === null) return;
  e.preventDefault();
  e.stopPropagation();

  const outcome = interpret(e);
  switch (outcome.kind) {
    case "pending":
      return;
    case "cancel":
      setRecordingStatus(STRINGS.cancelled);
      stopRecording();
      return;
    case "error":
      setRecordingStatus(outcome.message);
      return;
    case "clear": {
      const action = recording;
      stopRecording();
      setRecordingStatus(STRINGS.cleared);
      void applyBinding(action, null);
      return;
    }
    case "bound": {
      const action = recording;
      if (!isMac()) {
        const problem = windowsHotkeyProblem(outcome.hotkey);
        if (problem) {
          setRecordingStatus(problem);
          return;
        }
        if (
          hasAltGrRisk(outcome.hotkey) &&
          !window.confirm(STRINGS.altGrConfirm)
        ) {
          setRecordingStatus(STRINGS.notSaved);
          return;
        }
        const warning = knownWindowsShortcutWarning(outcome.hotkey);
        if (warning && !window.confirm(STRINGS.warningConfirm(warning))) {
          setRecordingStatus(STRINGS.notSaved);
          return;
        }
      }
      stopRecording();
      setRecordingStatus("");
      void applyBinding(action, outcome.hotkey);
      return;
    }
  }
}

async function applyBinding(
  action: WindowAction,
  hotkey: Hotkey | null,
): Promise<void> {
  const saved = await saveSetting(dom.bindingError, () =>
    setBinding(action, hotkey),
  );
  // A failed save has already re-rendered from the settings Tile kept.
  if (saved === false) return;
  await renderWholeConfig(saved !== null);
}

/** Re-renders the list as the user types into the filter. */
export function wireShortcutEvents(): void {
  dom.shortcutFilter.addEventListener("input", () => {
    shortcutFilter = dom.shortcutFilter.value;
    renderBindings();
  });
}
