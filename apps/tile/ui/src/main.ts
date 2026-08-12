// Settings UI controller. No framework: plain DOM.

import "./styles.css";
import { openUrl } from "@tauri-apps/plugin-opener";
import {
  getConfig,
  getHotkeyFailures,
  getPermissionStatus,
  resetToDefaults,
  setBinding,
  setGap,
  setLaunchOnLogin,
} from "./api";
import { formatHotkey, interpret } from "./hotkey";
import {
  ACTIONS,
  Config,
  Hotkey,
  HotkeyFailure,
  WindowAction,
} from "./types";

const ACCESSIBILITY_URL =
  "x-apple.systempreferences:com.apple.preference.security?Privacy_Accessibility";

/** Non-null `querySelector`, throwing at boot if the markup is wrong. */
function el<T extends HTMLElement>(selector: string): T {
  const node = document.querySelector<T>(selector);
  if (!node) throw new Error(`missing element: ${selector}`);
  return node;
}

const dom = {
  bindings: el<HTMLUListElement>("#bindings"),
  recordingStatus: el<HTMLParagraphElement>("#recording-status"),
  gap: el<HTMLInputElement>("#gap"),
  gapNumber: el<HTMLInputElement>("#gap-number"),
  launch: el<HTMLInputElement>("#launch-on-login"),
  reset: el<HTMLButtonElement>("#reset"),
  permissionPanel: el<HTMLElement>("#permission-panel"),
  grant: el<HTMLButtonElement>("#grant-permission"),
  openAccessibility: el<HTMLButtonElement>("#open-accessibility"),
};

let config: Config | null = null;
let failures: HotkeyFailure[] = [];
let recording: WindowAction | null = null;
let permissionTimer: number | null = null;

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

function failureFor(action: WindowAction): HotkeyFailure | undefined {
  return failures.find((f) => f.action === action);
}

function renderBindings(): void {
  if (!config) return;
  const cfg = config;
  const conflicts = conflictingActions(cfg);
  dom.bindings.replaceChildren();

  for (const { id, label } of ACTIONS) {
    const hk = cfg.bindings[id] ?? null;

    const li = document.createElement("li");
    li.className = "binding";

    const name = document.createElement("span");
    name.className = "binding__label";
    name.textContent = label;
    name.id = `label-${id}`;

    const controls = document.createElement("div");
    controls.className = "binding__controls";

    const record = document.createElement("button");
    record.type = "button";
    record.className = "binding__key";
    record.setAttribute("aria-labelledby", `label-${id} key-${id}`);
    record.id = `key-${id}`;
    if (recording === id) {
      record.classList.add("binding__key--recording");
      record.textContent = "Press keys…";
    } else {
      record.textContent = hk ? formatHotkey(hk) : "Unbound";
      if (!hk) record.classList.add("binding__key--empty");
    }
    record.addEventListener("click", () => startRecording(id));
    controls.append(record);

    if (hk && recording !== id) {
      const clear = document.createElement("button");
      clear.type = "button";
      clear.className = "binding__clear";
      clear.setAttribute("aria-label", `Clear shortcut for ${label}`);
      clear.textContent = "✕";
      clear.addEventListener("click", () => void applyBinding(id, null));
      controls.append(clear);
    }

    li.append(name, controls);

    const failure = failureFor(id);
    if (conflicts.has(id)) {
      li.append(note("This shortcut is used by more than one action.", "error"));
    } else if (failure) {
      li.append(note(`The system rejected this shortcut: ${failure.reason}`, "error"));
    }

    dom.bindings.append(li);
  }
}

function note(text: string, kind: "error" | "info"): HTMLElement {
  const p = document.createElement("p");
  p.className = `binding__note binding__note--${kind}`;
  p.textContent = text;
  return p;
}

function setRecordingStatus(text: string): void {
  dom.recordingStatus.textContent = text;
}

function startRecording(action: WindowAction): void {
  recording = action;
  const label = ACTIONS.find((a) => a.id === action)?.label ?? action;
  setRecordingStatus(
    `Recording ${label}. Press a shortcut, Esc to cancel, Backspace to clear.`,
  );
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
      setRecordingStatus("Recording cancelled.");
      stopRecording();
      return;
    case "error":
      setRecordingStatus(outcome.message);
      return;
    case "clear": {
      const action = recording;
      stopRecording();
      setRecordingStatus("Shortcut cleared.");
      void applyBinding(action, null);
      return;
    }
    case "bound": {
      const action = recording;
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
  try {
    config = await setBinding(action, hotkey);
    failures = await getHotkeyFailures();
    renderBindings();
  } catch (err) {
    setRecordingStatus(`Could not save shortcut: ${String(err)}`);
  }
}

function renderBehaviour(): void {
  if (!config) return;
  dom.gap.value = String(config.gap);
  dom.gapNumber.value = String(config.gap);
  dom.launch.checked = config.launchOnLogin;
}

function clampGap(value: number): number {
  if (!Number.isFinite(value)) return 0;
  return Math.min(200, Math.max(0, Math.round(value)));
}

async function commitGap(raw: string): Promise<void> {
  const gap = clampGap(Number(raw));
  dom.gap.value = String(gap);
  dom.gapNumber.value = String(gap);
  try {
    config = await setGap(gap);
  } catch (err) {
    setRecordingStatus(`Could not save gap: ${String(err)}`);
  }
}

function mirrorGap(raw: string): void {
  const gap = clampGap(Number(raw));
  dom.gap.value = String(gap);
  dom.gapNumber.value = String(gap);
}

async function refreshPermission(prompt: boolean): Promise<void> {
  let status;
  try {
    status = await getPermissionStatus(prompt);
  } catch (err) {
    console.error("permission check failed", err);
    return;
  }

  const denied = status === "denied";
  dom.permissionPanel.hidden = !denied;

  if (denied && permissionTimer === null) {
    permissionTimer = window.setInterval(() => void refreshPermission(false), 2000);
  } else if (!denied && permissionTimer !== null) {
    window.clearInterval(permissionTimer);
    permissionTimer = null;
    // Permission just became available: surface any late hotkey failures.
    failures = await getHotkeyFailures();
    renderBindings();
  }
}

function wireEvents(): void {
  dom.gap.addEventListener("input", () => mirrorGap(dom.gap.value));
  dom.gap.addEventListener("change", () => void commitGap(dom.gap.value));
  dom.gapNumber.addEventListener("input", () => mirrorGap(dom.gapNumber.value));
  dom.gapNumber.addEventListener("change", () => void commitGap(dom.gapNumber.value));

  dom.launch.addEventListener("change", async () => {
    try {
      config = await setLaunchOnLogin(dom.launch.checked);
    } catch (err) {
      setRecordingStatus(`Could not update launch-on-login: ${String(err)}`);
      dom.launch.checked = config?.launchOnLogin ?? false;
    }
  });

  dom.reset.addEventListener("click", async () => {
    try {
      config = await resetToDefaults();
      failures = await getHotkeyFailures();
      renderBindings();
      renderBehaviour();
      setRecordingStatus("Defaults restored.");
    } catch (err) {
      setRecordingStatus(`Could not restore defaults: ${String(err)}`);
    }
  });

  dom.grant.addEventListener("click", () => void refreshPermission(true));
  dom.openAccessibility.addEventListener("click", async () => {
    try {
      await openUrl(ACCESSIBILITY_URL);
    } catch (err) {
      console.error("could not open Accessibility settings", err);
    }
  });
}

async function boot(): Promise<void> {
  wireEvents();
  try {
    config = await getConfig();
    failures = await getHotkeyFailures();
  } catch (err) {
    setRecordingStatus(`Could not load settings: ${String(err)}`);
    return;
  }
  renderBindings();
  renderBehaviour();
  await refreshPermission(false);
}

void boot();
