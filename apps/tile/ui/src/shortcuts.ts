// The shortcut editor: two groups (Window placement and Displays), each with a
// curated modifier preset dropdown + Custom… toggles, single-key capture rows,
// and a live corner-composition hint.

import { listen } from "@tauri-apps/api/event";
import {
  beginHotkeyCapture,
  CapturedHotkey,
  endHotkeyCapture,
  getHotkeyStatus,
  HOTKEY_CAPTURED,
  setGroupModifiers,
  setKey,
} from "./api";
import { confirmDialog } from "./confirm";
import { dom } from "./dom";
import {
  actionsUsing,
  formatHotkey,
  formatKey,
  formatModifiers,
  hotkeyProblem,
  hotkeyWarnings,
  interpretKey,
  isMac,
  KeyRecordOutcome,
  modifierCollisions,
  modifierPresets,
  modifierToggles,
  modifierWarnings,
} from "./hotkey";
import { renderWholeConfig, saveSetting } from "./settings";
import { config } from "./state";
import {
  ACTIONS,
  Config,
  groupModifiers,
  Hotkey,
  HotkeyBindingStatus,
  HotkeyStatus,
  KeyCode,
  SHORTCUT_GROUPS,
  ShortcutGroup,
  WindowAction,
} from "./types";
import { setFieldError } from "./writes";

const CUSTOM_OPTION = "custom";

/** User-facing copy for the shortcut groups and recorder. */
const STRINGS = {
  applyError: (error: string) =>
    `Windows could not apply the latest shortcut change. A previously active shortcut may still be in effect. ${error}`,
  hookUnavailable:
    "Some shortcuts aren't working right now. Tile will keep retrying.",
  customOption: "Custom…",
  pressKey: "Press a key…",
  unbound: "Unbound",
  clearLabel: (label: string) => `Clear key for ${label}`,
  clearGlyph: "✕",
  unavailable: (reason: string | null | undefined) =>
    `This shortcut is unavailable: ${reason ?? "Windows rejected it."}`,
  unconfirmed:
    "This shortcut is configured, but its active route could not be confirmed.",
  recording: (label: string) =>
    `Recording ${label}. Press a key, Esc to cancel, Backspace to clear.`,
  cancelled: "Recording cancelled.",
  cleared: "Key cleared.",
  wrongModifier: "Press just the key — change the modifier above.",
  warningTitle: (label: string) => `Use ${label} anyway?`,
  saveAnyway: "Use it anyway",
  takenTitle: (chord: string, others: string) =>
    `${chord} is already used by ${others}.`,
  takenMessage: (label: string, others: string) =>
    `Replace moves it to ${label}, and ${others} will have no key.`,
  replace: "Replace",
  notSaved: "Shortcut was not saved.",
  modifierCollision: (pairs: string) =>
    `Both groups cannot share this modifier while ${pairs} use the same key. Change those keys first or pick a different modifier.`,
  modifierImpossible: (chord: string, reason: string) =>
    `Cannot use this modifier because ${chord} is unavailable: ${reason}`,
  cornerLabel: "Corners",
  cornerGesture: (mods: string, sides: string, verticals: string) =>
    `Hold ${mods}, press ${sides} then ${verticals} for a corner on that side.`,
  cornerInactive:
    "Assign Left or Right together with Maximize or Restore to compose corners.",
};

function listLabels(labels: string[]): string {
  if (labels.length <= 1) return labels.join("");
  return `${labels.slice(0, -1).join(", ")} and ${labels[labels.length - 1]}`;
}

function labelFor(action: WindowAction): string {
  return ACTIONS.find((a) => a.id === action)?.label ?? action;
}

function groupFor(action: WindowAction): ShortcutGroup {
  return ACTIONS.find((a) => a.id === action)?.group ?? "placement";
}

let hotkeyStatus: HotkeyStatus = {
  bindings: [],
  hookInstalled: false,
  hookUnavailable: false,
  applyError: null,
};
let recording: WindowAction | null = null;
/** Groups whose Custom… modifier toggles are currently expanded by user choice. */
const customOpen = new Set<ShortcutGroup>();

/** Collapses user-expanded Custom… modifier rows (e.g. after Restore defaults). */
export function resetCustomModifierGroups(): void {
  customOpen.clear();
}

/**
 * Stops an active key recording without re-rendering or stealing focus, for
 * when the user clicks straight into another control (like a modifier dropdown
 * or Restore defaults).
 */
export function cancelRecordingInPlace(): void {
  if (recording === null) return;
  const action = recording;
  recording = null;
  captureSession++;
  window.removeEventListener("keydown", onRecordKey, { capture: true });
  window.removeEventListener("blur", onRecordBlur);
  queueCapture(null);
  setRecordingStatus("");

  const button = document.getElementById(`key-${action}`);
  if (button && config) {
    const key = config.keys[action] ?? null;
    button.classList.remove("binding__key--recording");
    if (key) {
      button.textContent = formatKey(key);
      button.classList.remove("binding__key--empty");
    } else {
      button.textContent = STRINGS.unbound;
      button.classList.add("binding__key--empty");
    }
  }
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
    (status) => status.action === action && sameHotkey(status.hotkey, hotkey),
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

function restoreBindingFocus(id: string): void {
  const direct = document.getElementById(id);
  if (direct) {
    direct.focus();
    return;
  }
  const clearMatch = /^clear-(.+)$/.exec(id);
  if (clearMatch) {
    document.getElementById(`key-${clearMatch[1]}`)?.focus();
  }
}

export function renderBindings(): void {
  if (!config) return;
  renderHotkeyHealth();
  renderHotkeyApplyError();

  const active = document.activeElement;
  const focusedId =
    active instanceof HTMLElement &&
    active.id &&
    dom.shortcutGroups.contains(active)
      ? active.id
      : null;

  renderGroups(config);
  if (focusedId) restoreBindingFocus(focusedId);
}

function renderHotkeyApplyError(): void {
  const error = hotkeyStatus.applyError;
  dom.hotkeyApplyError.hidden = error === null;
  dom.hotkeyApplyError.textContent =
    error === null ? "" : STRINGS.applyError(error);
}

function renderGroups(cfg: Config): void {
  const mac = isMac();
  const presets = modifierPresets(mac);
  const toggles = modifierToggles(mac);

  dom.shortcutGroups.replaceChildren();

  for (const group of SHORTCUT_GROUPS) {
    const mods = groupModifiers(cfg, group.id);
    const matchesPreset = presets.some((p) => p.modifiers === mods);
    const showCustom = customOpen.has(group.id) || !matchesPreset;

    const card = document.createElement("div");
    card.className = "shortcut-group";

    const header = document.createElement("div");
    header.className = "shortcut-group__header";

    const headingWrap = document.createElement("div");
    headingWrap.className = "shortcut-group__heading";

    const title = document.createElement("h3");
    title.className = "shortcut-group__title";
    title.textContent = group.label;

    const desc = document.createElement("p");
    desc.className = "shortcut-group__description";
    desc.textContent = group.description;

    headingWrap.append(title, desc);

    const picker = document.createElement("div");
    picker.className = "modifier-picker";

    const selectId = `modifier-${group.id}`;
    const selectLabel = document.createElement("label");
    selectLabel.className = "modifier-picker__label";
    selectLabel.htmlFor = selectId;
    selectLabel.textContent = group.modifierLabel;

    const select = document.createElement("select");
    select.id = selectId;
    select.className = "modifier-picker__select";

    for (const preset of presets) {
      const opt = document.createElement("option");
      opt.value = String(preset.modifiers);
      opt.textContent = preset.label;
      select.append(opt);
    }
    const customOpt = document.createElement("option");
    customOpt.value = CUSTOM_OPTION;
    customOpt.textContent = STRINGS.customOption;
    select.append(customOpt);

    select.value = showCustom ? CUSTOM_OPTION : String(mods);
    select.addEventListener("pointerdown", () => cancelRecordingInPlace());
    select.addEventListener("focus", () => cancelRecordingInPlace());
    select.addEventListener("change", () => {
      cancelRecordingInPlace();
      if (select.value === CUSTOM_OPTION) {
        customOpen.add(group.id);
        renderBindings();
        return;
      }
      const nextMods = Number(select.value);
      void confirmAndApplyModifiers(
        group.id,
        nextMods,
        true,
        () => document.getElementById(selectId),
      );
    });

    picker.append(selectLabel, select);
    header.append(headingWrap, picker);
    card.append(header);

    if (showCustom) {
      const toggleRow = document.createElement("div");
      toggleRow.className = "modifier-toggles";
      toggleRow.setAttribute("role", "group");
      toggleRow.setAttribute("aria-label", `${group.modifierLabel} keys`);

      const activeBits = toggles.filter((t) => (mods & t.bit) !== 0).length;

      for (const toggle of toggles) {
        const active = (mods & toggle.bit) !== 0;
        const button = document.createElement("button");
        button.type = "button";
        button.id = `mod-toggle-${group.id}-${toggle.bit}`;
        button.className = "modifier-toggle";
        if (active) button.classList.add("modifier-toggle--active");
        button.setAttribute("aria-pressed", String(active));
        button.setAttribute("aria-label", toggle.ariaLabel);
        button.textContent = toggle.label;
        // Keep at least one modifier selected.
        if (active && activeBits <= 1) {
          button.disabled = true;
        }
        button.addEventListener("click", () => {
          cancelRecordingInPlace();
          const nextMods = active ? mods & ~toggle.bit : mods | toggle.bit;
          if (nextMods === 0) return;
          customOpen.add(group.id);
          void confirmAndApplyModifiers(
            group.id,
            nextMods,
            false,
            () => document.getElementById(button.id),
          );
        });
        toggleRow.append(button);
      }

      card.append(toggleRow);
    }

    const list = document.createElement("ul");
    list.className = "bindings bindings--group";
    for (const action of ACTIONS.filter((a) => a.group === group.id)) {
      list.append(renderActionRow(cfg, action.id, action.label, mods, mac));
    }
    card.append(list);

    if (group.id === "placement") {
      card.append(renderCornerHint(cfg, mac));
    }

    dom.shortcutGroups.append(card);
  }
}

function renderCornerHint(cfg: Config, mac: boolean): HTMLDivElement {
  const hint = document.createElement("div");
  hint.className = "corner-hint";

  const title = document.createElement("span");
  title.className = "corner-hint__label";
  title.textContent = STRINGS.cornerLabel;

  const text = document.createElement("span");
  text.className = "corner-hint__text";

  const sides = (["left-half", "right-half"] as const)
    .map((id) => cfg.keys[id])
    .filter((k): k is KeyCode => Boolean(k))
    .map(formatKey);
  const verticals = (["maximize", "restore"] as const)
    .map((id) => cfg.keys[id])
    .filter((k): k is KeyCode => Boolean(k))
    .map(formatKey);

  if (sides.length > 0 && verticals.length > 0) {
    text.textContent = STRINGS.cornerGesture(
      formatModifiers(cfg.placementModifiers, mac),
      sides.join(" or "),
      verticals.join(" or "),
    );
  } else {
    text.textContent = STRINGS.cornerInactive;
  }

  hint.append(title, text);
  return hint;
}

function renderActionRow(
  cfg: Config,
  id: WindowAction,
  label: string,
  mods: number,
  mac: boolean,
): HTMLLIElement {
  const key = cfg.keys[id] ?? null;
  const hk: Hotkey | null = key ? { modifiers: mods, key } : null;

  const li = document.createElement("li");
  li.className = "binding";

  const name = document.createElement("span");
  name.className = "binding__label";
  name.textContent = label;
  name.id = `label-${id}`;

  const controls = document.createElement("div");
  controls.className = "binding__controls";

  if (key && recording !== id) {
    const prefix = document.createElement("span");
    prefix.className = "binding__prefix";
    prefix.setAttribute("aria-hidden", "true");
    const modText = formatModifiers(mods, mac);
    prefix.textContent = mac ? modText : `${modText}+`;
    controls.append(prefix);
  }

  const record = document.createElement("button");
  record.type = "button";
  record.className = "binding__key";
  record.id = `key-${id}`;
  record.setAttribute("aria-labelledby", `label-${id} key-${id}`);
  if (recording === id) {
    record.classList.add("binding__key--recording");
    record.textContent = STRINGS.pressKey;
  } else if (key) {
    record.textContent = formatKey(key);
  } else {
    record.textContent = STRINGS.unbound;
    record.classList.add("binding__key--empty");
  }
  record.addEventListener("click", () => startRecording(id));
  controls.append(record);

  if (key && recording !== id) {
    const clear = document.createElement("button");
    clear.type = "button";
    clear.className = "binding__clear";
    clear.id = `clear-${id}`;
    clear.setAttribute("aria-label", STRINGS.clearLabel(label));
    clear.textContent = STRINGS.clearGlyph;
    clear.addEventListener("click", () => {
      cancelRecordingInPlace();
      void applyKey(id, null);
    });
    controls.append(clear);
  }

  li.append(name, controls);

  const route = statusFor(id, hk);
  let text: string | null = null;
  let invalid = false;
  if (route?.route === "unavailable") {
    text = STRINGS.unavailable(route.reason);
    invalid = true;
  } else if (hk && hotkeyStatus.applyError) {
    text = STRINGS.unconfirmed;
  }
  if (text !== null) {
    const noteId = `note-${id}`;
    const p = document.createElement("p");
    p.id = noteId;
    p.className = "binding__note binding__note--error";
    p.textContent = text;
    li.append(p);
    record.setAttribute("aria-describedby", noteId);
    if (invalid) record.setAttribute("aria-invalid", "true");
  }

  return li;
}

export function setRecordingStatus(text: string): void {
  dom.recordingStatus.textContent = text;
}

function startRecording(action: WindowAction): void {
  setFieldError(dom.bindingError, null);
  recording = action;
  setRecordingStatus(STRINGS.recording(labelFor(action)));
  renderBindings();
  window.addEventListener("keydown", onRecordKey, { capture: true });
  window.addEventListener("blur", onRecordBlur);
  // Native capture catches modified chords on Windows (e.g. holding Win out of
  // muscle memory while pressing an arrow key) so they don't trigger Aero Snap.
  queueCapture(++captureSession);
}

let captureSession = 0;
let captureQueue: Promise<void> = Promise.resolve();

function queueCapture(session: number | null): void {
  captureQueue = captureQueue
    .then(async () => {
      if (session !== null) await beginHotkeyCapture(session);
      else await endHotkeyCapture();
    })
    .catch((err) =>
      console.warn(
        session !== null
          ? "native shortcut capture is unavailable"
          : "could not stop native shortcut capture",
        err,
      ),
    );
}

function stopRecording(): void {
  const action = recording;
  recording = null;
  captureSession++;
  window.removeEventListener("keydown", onRecordKey, { capture: true });
  window.removeEventListener("blur", onRecordBlur);
  queueCapture(null);
  renderBindings();
  if (action !== null) document.getElementById(`key-${action}`)?.focus();
}

function onRecordBlur(): void {
  if (recording === null) return;
  setRecordingStatus(STRINGS.cancelled);
  stopRecording();
}

export async function listenForCapturedHotkeys(): Promise<void> {
  await listen<CapturedHotkey>(HOTKEY_CAPTURED, ({ payload }) => {
    if (recording === null || !config || payload.session !== captureSession) {
      return;
    }
    const expectedMods = groupModifiers(config, groupFor(recording));
    if (payload.hotkey.modifiers !== expectedMods) {
      setRecordingStatus(STRINGS.wrongModifier);
      return;
    }
    handleKeyOutcome({ kind: "key", key: payload.hotkey.key });
  });
}

function onRecordKey(e: KeyboardEvent): void {
  if (recording === null || !config) return;
  e.preventDefault();
  e.stopPropagation();
  const expectedMods = groupModifiers(config, groupFor(recording));
  handleKeyOutcome(interpretKey(e, expectedMods));
}

function handleKeyOutcome(outcome: KeyRecordOutcome): void {
  if (recording === null || !config) return;
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
      void applyKey(action, null);
      return;
    }
    case "key": {
      const action = recording;
      const mods = groupModifiers(config, groupFor(action));
      const hotkey: Hotkey = { modifiers: mods, key: outcome.key };
      const problem = hotkeyProblem(hotkey, isMac());
      if (problem) {
        setRecordingStatus(problem);
        return;
      }
      stopRecording();
      setRecordingStatus("");
      void confirmAndApplyKey(action, outcome.key, hotkey);
      return;
    }
  }
}

async function confirmAndApplyModifiers(
  group: ShortcutGroup,
  modifiers: number,
  collapseCustom: boolean,
  returnFocus: () => HTMLElement | null,
): Promise<void> {
  if (!config) return;
  setFieldError(dom.bindingError, null);
  const mac = isMac();

  const collisions = modifierCollisions(config, group, modifiers);
  if (collisions.length > 0) {
    const pairs = listLabels(
      collisions.map(([a, b]) => `${labelFor(a)} and ${labelFor(b)}`),
    );
    setFieldError(dom.bindingError, STRINGS.modifierCollision(pairs));
    renderBindings();
    returnFocus()?.focus();
    return;
  }

  const groupKeys = ACTIONS.filter((a) => a.group === group)
    .map((a) => config?.keys[a.id])
    .filter((k): k is KeyCode => Boolean(k));

  for (const key of groupKeys) {
    const hk: Hotkey = { modifiers, key };
    const problem = hotkeyProblem(hk, mac);
    if (problem) {
      setFieldError(
        dom.bindingError,
        STRINGS.modifierImpossible(formatHotkey(hk), problem),
      );
      renderBindings();
      returnFocus()?.focus();
      return;
    }
  }

  const label = formatModifiers(modifiers, mac);
  for (const warning of modifierWarnings(modifiers, groupKeys, mac)) {
    const ok = await confirmDialog({
      title: STRINGS.warningTitle(label),
      message: warning,
      confirmLabel: STRINGS.saveAnyway,
      returnFocus,
    });
    if (!ok) {
      setRecordingStatus(STRINGS.notSaved);
      renderBindings();
      return;
    }
  }

  const saved = await saveSetting(dom.bindingError, () =>
    setGroupModifiers(group, modifiers),
  );
  if (saved === false) {
    renderBindings();
    return;
  }
  if (collapseCustom) customOpen.delete(group);
  await renderWholeConfig(saved !== null);
}

async function confirmAndApplyKey(
  action: WindowAction,
  key: KeyCode,
  hotkey: Hotkey,
): Promise<void> {
  const chord = formatHotkey(hotkey);
  const returnFocus = () => document.getElementById(`key-${action}`);

  for (const warning of hotkeyWarnings(hotkey, isMac())) {
    const ok = await confirmDialog({
      title: STRINGS.warningTitle(chord),
      message: warning,
      confirmLabel: STRINGS.saveAnyway,
      returnFocus,
    });
    if (!ok) {
      setRecordingStatus(STRINGS.notSaved);
      return;
    }
  }

  const holders = config ? actionsUsing(config, hotkey, action) : [];
  if (holders.length > 0) {
    const others = listLabels(holders.map(labelFor));
    const ok = await confirmDialog({
      title: STRINGS.takenTitle(chord, others),
      message: STRINGS.takenMessage(labelFor(action), others),
      confirmLabel: STRINGS.replace,
      returnFocus,
    });
    if (!ok) {
      setRecordingStatus(STRINGS.notSaved);
      return;
    }
  }

  await applyKey(action, key, holders);
}

async function applyKey(
  action: WindowAction,
  key: KeyCode | null,
  replace: WindowAction[] = [],
): Promise<void> {
  const saved = await saveSetting(dom.bindingError, () =>
    setKey(action, key, replace),
  );
  if (saved === false) return;
  await renderWholeConfig(saved !== null);
}
