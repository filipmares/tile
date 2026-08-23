// Settings UI controller. No framework: plain DOM.

import { openUrl } from "@tauri-apps/plugin-opener";
import { getVersion } from "@tauri-apps/api/app";
import {
  checkForUpdates,
  getBuildInfo,
  getConfig,
  getHotkeyFailures,
  getPermissionStatus,
  getUpdateStatus,
  installUpdate,
  resetToDefaults,
  setAnimation,
  setBinding,
  setCycling,
  setGaps,
  setLaunchOnLogin,
} from "./api";
import { formatHotkey, interpret } from "./hotkey";
import {
  ACTIONS,
  BuildInfo,
  Config,
  CYCLE_SIZES,
  CycleSize,
  FAMILIES,
  Gaps,
  Hotkey,
  HotkeyFailure,
  SubsequentExecutionMode,
  UpdateStatus,
  WindowAction,
} from "./types";

const ACCESSIBILITY_URL =
  "x-apple.systempreferences:com.apple.preference.security?Privacy_Accessibility";
const GITHUB_URL = "https://github.com/filipmares/tile";
const isAboutScreen = new URLSearchParams(window.location.search).has("about");

/** Non-null `querySelector`, throwing at boot if the markup is wrong. */
function el<T extends HTMLElement>(selector: string): T {
  const node = document.querySelector<T>(selector);
  if (!node) throw new Error(`missing element: ${selector}`);
  return node;
}

const dom = {
  app: el<HTMLElement>("#app"),
  about: el<HTMLElement>("#about"),
  aboutVersion: el<HTMLParagraphElement>("#about-version"),
  aboutCheckUpdate: el<HTMLButtonElement>("#about-check-update"),
  aboutUpdateStatus: el<HTMLParagraphElement>("#about-update-status"),
  github: el<HTMLButtonElement>("#github"),
  bindings: el<HTMLUListElement>("#bindings"),
  assignedBindings: el<HTMLUListElement>("#assigned-bindings"),
  allShortcutsCount: el<HTMLSpanElement>("#all-shortcuts-count"),
  recordingStatus: el<HTMLParagraphElement>("#recording-status"),
  gapWindow: el<HTMLInputElement>("#gap-window"),
  gapWindowNumber: el<HTMLInputElement>("#gap-window-number"),
  gapEdgeTop: el<HTMLInputElement>("#gap-edge-top"),
  gapEdgeBottom: el<HTMLInputElement>("#gap-edge-bottom"),
  gapEdgeLeft: el<HTMLInputElement>("#gap-edge-left"),
  gapEdgeRight: el<HTMLInputElement>("#gap-edge-right"),
  gapSkipTop: el<HTMLInputElement>("#gap-skip-top"),
  gapMainOnly: el<HTMLInputElement>("#gap-main-only"),
  subsequentMode: el<HTMLSelectElement>("#subsequent-mode"),
  cycleSizes: el<HTMLFieldSetElement>("#cycle-sizes"),
  cycleSizesGrid: el<HTMLDivElement>("#cycle-sizes-grid"),
  animate: el<HTMLInputElement>("#animate-moves"),
  launch: el<HTMLInputElement>("#launch-on-login"),
  reset: el<HTMLButtonElement>("#reset"),
  permissionPanel: el<HTMLElement>("#permission-panel"),
  grant: el<HTMLButtonElement>("#grant-permission"),
  openAccessibility: el<HTMLButtonElement>("#open-accessibility"),
  developmentPanel: el<HTMLElement>("#development-panel"),
  developmentConfigDir: el<HTMLParagraphElement>("#development-config-dir"),
  launchDevelopmentNote: el<HTMLParagraphElement>("#launch-development-note"),
  updatePanel: el<HTMLElement>("#update-panel"),
  updateStatus: el<HTMLParagraphElement>("#update-status"),
  updateNotes: el<HTMLParagraphElement>("#update-notes"),
  updateProgress: el<HTMLProgressElement>("#update-progress"),
  updateProgressDetail: el<HTMLParagraphElement>("#update-progress-detail"),
  checkUpdate: el<HTMLButtonElement>("#check-update"),
  installUpdate: el<HTMLButtonElement>("#install-update"),
  updateConfirmation: el<HTMLDialogElement>("#update-confirmation"),
  updateConfirmationMessage: el<HTMLParagraphElement>(
    "#update-confirmation-message",
  ),
  confirmUpdate: el<HTMLButtonElement>("#confirm-update"),
  cancelUpdate: el<HTMLButtonElement>("#cancel-update"),
};

let config: Config | null = null;
let failures: HotkeyFailure[] = [];
let recording: WindowAction | null = null;
let permissionTimer: number | null = null;
let updatePollTimer: number | null = null;
let updateState: UpdateStatus = { status: "idle" };
let updateNotes: string | null = null;

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
  const assignedActions = ACTIONS.filter(({ id }) => cfg.bindings[id]);
  dom.assignedBindings.replaceChildren();
  dom.allShortcutsCount.textContent = `${ACTIONS.length} shortcuts · ${assignedActions.length} assigned`;

  if (assignedActions.length === 0) {
    const empty = document.createElement("li");
    empty.className = "shortcut-empty";
    empty.textContent = "No shortcuts assigned yet.";
    dom.assignedBindings.append(empty);
  } else {
    for (const action of assignedActions) {
      dom.assignedBindings.append(
        renderBinding(cfg, conflicts, action.id, action.label, "assigned"),
      );
    }
  }

  const hasRendered = dom.bindings.childElementCount > 0;
  const openFamilies = new Set(
    [...dom.bindings.querySelectorAll<HTMLDetailsElement>("details[open]")]
      .map((details) => details.dataset.family)
      .filter((family): family is string => family !== undefined),
  );
  dom.bindings.replaceChildren();

  for (const family of FAMILIES) {
    const actions = ACTIONS.filter((a) => a.family === family.id);
    if (actions.length === 0) continue;

    const group = document.createElement("li");
    group.className = "binding-group";

    const disclosure = document.createElement("details");
    disclosure.className = "binding-group__disclosure";
    disclosure.dataset.family = family.id;
    disclosure.open = hasRendered
      ? openFamilies.has(family.id)
      : family.id === "halves" || actions.some(({ id }) => cfg.bindings[id]);

    const summary = document.createElement("summary");
    summary.className = "binding-group__summary";

    const heading = document.createElement("span");
    heading.className = "binding-group__title";
    heading.textContent = family.label;

    const assignedCount = actions.filter(({ id }) => cfg.bindings[id]).length;
    const count = document.createElement("span");
    count.className = "binding-group__count";
    count.textContent = `${actions.length} shortcuts · ${assignedCount} assigned`;

    summary.append(heading, count);
    disclosure.append(summary);

    const list = document.createElement("ul");
    list.className = "binding-group__list";

    for (const { id, label } of actions) {
      list.append(renderBinding(cfg, conflicts, id, label, "all"));
    }

    disclosure.append(list);
    group.append(disclosure);
    dom.bindings.append(group);
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

  return li;
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
  const g = config.gap;
  dom.gapWindow.value = String(g.window);
  dom.gapWindowNumber.value = String(g.window);
  dom.gapEdgeTop.value = String(g.edgeTop);
  dom.gapEdgeBottom.value = String(g.edgeBottom);
  dom.gapEdgeLeft.value = String(g.edgeLeft);
  dom.gapEdgeRight.value = String(g.edgeRight);
  dom.gapSkipTop.checked = g.skipTopEdge;
  dom.gapMainOnly.checked = g.mainScreenOnly;
  dom.subsequentMode.value = config.subsequentExecutionMode;
  renderCycleSizes(config);
  dom.animate.checked = config.animation.enabled;
  dom.launch.checked = config.launchOnLogin;
}

/** Marks the window as a development build. Installed builds render nothing. */
function renderBuildInfo(info: BuildInfo): void {
  if (info.kind !== "development") return;
  dom.developmentPanel.hidden = false;
  // The launch-on-login toggle is the one control whose behaviour differs, so
  // it says so where it is, not only in the panel at the top.
  dom.launchDevelopmentNote.hidden = false;
  if (info.configDir) {
    dom.developmentConfigDir.textContent = `Settings are stored in ${info.configDir}`;
    dom.developmentConfigDir.hidden = false;
  }
}

function renderUpdateStatus(status: UpdateStatus): void {
    updateState = status;
    if (status.status !== "available" && dom.updateConfirmation.open) {
      dom.updateConfirmation.close();
    }
    dom.updatePanel.hidden = false;
    dom.updateProgress.hidden = true;
    dom.updateProgressDetail.hidden = true;
    dom.updateProgress.removeAttribute("value");
    dom.installUpdate.hidden = true;
    dom.checkUpdate.disabled = false;
    dom.updateNotes.hidden = updateNotes === null;
    dom.updateNotes.textContent = updateNotes ?? "";

    switch (status.status) {
      case "unavailable":
        setUpdateAnnouncement(
          "Production update checks are unavailable in this development build.",
        );
        dom.checkUpdate.disabled = true;
        break;
      case "idle":
        setUpdateAnnouncement("Tile has not checked for updates yet.");
        break;
      case "checking":
        setUpdateAnnouncement("Checking for updates…");
        dom.checkUpdate.disabled = true;
        break;
      case "current":
        setUpdateAnnouncement("Tile is up to date.");
        updateNotes = null;
        dom.updateNotes.hidden = true;
        break;
      case "available":
        updateNotes = status.notes;
        dom.updateNotes.textContent = updateNotes ?? "";
        dom.updateNotes.hidden = updateNotes === null;
        setUpdateAnnouncement(`Tile ${status.version} is available.`);
        dom.installUpdate.textContent = "Update now";
        dom.installUpdate.hidden = false;
        break;
      case "downloading": {
        const total = status.totalBytes;
        const downloadedMb = (status.downloadedBytes / 1_048_576).toFixed(1);
        setUpdateAnnouncement(`Downloading Tile ${status.version}.`);
        dom.updateProgressDetail.textContent =
          total === null
            ? `${downloadedMb} MB downloaded`
            : `${Math.min(100, Math.round((status.downloadedBytes / total) * 100))}% downloaded`;
        dom.updateProgressDetail.hidden = false;
        dom.updateProgress.hidden = false;
        if (total !== null) {
          dom.updateProgress.max = total;
          dom.updateProgress.value = status.downloadedBytes;
        }
        dom.checkUpdate.disabled = true;
        break;
      }
      case "ready-to-relaunch":
        setUpdateAnnouncement(
          `Tile ${status.version} is installed and ready to relaunch.`,
        );
        dom.installUpdate.textContent = "Relaunch Tile";
        dom.installUpdate.hidden = false;
        dom.checkUpdate.disabled = true;
        break;
      case "error":
        setUpdateAnnouncement(`Update error: ${status.message}`);
        dom.checkUpdate.textContent = "Retry";
        break;
    }

    if (status.status !== "error") {
      dom.checkUpdate.textContent = "Check for updates";
    }
}

function setUpdateAnnouncement(text: string): void {
  if (dom.updateStatus.textContent !== text) {
    dom.updateStatus.textContent = text;
  }
}

function describeAboutUpdateStatus(status: UpdateStatus): string {
  switch (status.status) {
    case "unavailable":
      return "Update checks are unavailable in this development build.";
    case "idle":
      return "Tile has not checked for updates yet.";
    case "checking":
      return "Tile is already checking for updates.";
    case "current":
      return "Tile is up to date.";
    case "available":
      return `Tile ${status.version} is available. Open Settings or the tray menu to update.`;
    case "downloading":
      return `Tile ${status.version} is downloading.`;
    case "ready-to-relaunch":
      return `Tile ${status.version} is installed and ready to relaunch.`;
    case "error":
      return `Could not check for updates: ${status.message}`;
  }
}

  async function refreshUpdateStatus(): Promise<UpdateStatus> {
    try {
      const status = await getUpdateStatus();
      renderUpdateStatus(status);
      return status;
    } catch (err) {
      const status: UpdateStatus = { status: "error", message: String(err) };
      renderUpdateStatus(status);
      return status;
    }
  }

  function scheduleUpdateRefresh(status: UpdateStatus): void {
    if (updatePollTimer !== null) {
      window.clearTimeout(updatePollTimer);
    }
    const delay =
      status.status === "checking" || status.status === "downloading"
        ? 1000
        : 60_000;
    updatePollTimer = window.setTimeout(async () => {
      scheduleUpdateRefresh(await refreshUpdateStatus());
    }, delay);
  }

  async function runUpdateCheck(): Promise<UpdateStatus> {
    renderUpdateStatus({ status: "checking" });
    try {
      const status = await checkForUpdates();
      renderUpdateStatus(status);
      scheduleUpdateRefresh(status);
      return status;
    } catch (err) {
      const status: UpdateStatus = { status: "error", message: String(err) };
      renderUpdateStatus(status);
      scheduleUpdateRefresh(status);
      return status;
    }
  }

  function showUpdateConfirmation(): void {
    if (updateState.status !== "available") return;
    const windows = navigator.userAgent.includes("Windows");
    dom.updateConfirmationMessage.textContent = windows
      ? "Tile will close, install the update, and reopen automatically. Continue?"
      : "Tile will install the update. You can relaunch after it finishes. Continue?";
    if (!dom.updateConfirmation.open) {
      dom.updateConfirmation.showModal();
    }
    dom.confirmUpdate.focus();
  }

  async function applyUpdate(): Promise<void> {
    dom.updateConfirmation.close();
    if (updateState.status === "available") {
      renderUpdateStatus({
        status: "downloading",
        version: updateState.version,
        downloadedBytes: 0,
        totalBytes: null,
      });
      scheduleUpdateRefresh(updateState);
    }
    try {
      const status = await installUpdate(false);
      renderUpdateStatus(status);
      scheduleUpdateRefresh(status);
    } catch (err) {
      const status: UpdateStatus = { status: "error", message: String(err) };
      renderUpdateStatus(status);
      scheduleUpdateRefresh(status);
    }
  }

/** Builds the cycle-size checkboxes once, then reflects the saved selection. */
function renderCycleSizes(cfg: Config): void {
  if (dom.cycleSizesGrid.childElementCount === 0) {
    for (const size of CYCLE_SIZES) {
      const label = document.createElement("label");
      label.className = "field__sub";
      label.htmlFor = `cycle-size-${size.id}`;
      label.textContent = size.label;

      const input = document.createElement("input");
      input.type = "checkbox";
      input.id = `cycle-size-${size.id}`;
      input.dataset.size = size.id;
      input.addEventListener("change", () => void commitCycling());

      dom.cycleSizesGrid.append(label, input);
    }
  }

  const selected = new Set(cfg.cycleSizes);
  for (const input of cycleSizeInputs()) {
    input.checked = selected.has(input.dataset.size as CycleSize);
  }
  // With cycling switched off the sizes have no effect, so say so rather than
  // leaving controls that silently do nothing.
  dom.cycleSizes.disabled = cfg.subsequentExecutionMode !== "cycle-sizes";
}

function cycleSizeInputs(): HTMLInputElement[] {
  return [...dom.cycleSizesGrid.querySelectorAll<HTMLInputElement>("input")];
}

async function commitCycling(): Promise<void> {
  const mode = dom.subsequentMode.value as SubsequentExecutionMode;
  const sizes = cycleSizeInputs()
    .filter((input) => input.checked)
    .map((input) => input.dataset.size as CycleSize);
  try {
    config = await setCycling(mode, sizes);
    renderBehaviour();
  } catch (err) {
    setRecordingStatus(`Could not save cycling settings: ${String(err)}`);
  }
}

function clampGap(value: number): number {
  if (!Number.isFinite(value)) return 0;
  return Math.min(200, Math.max(0, Math.round(value)));
}

/** Reads the current gap controls into a `Gaps` payload. */
function readGaps(): Gaps {
  return {
    window: clampGap(Number(dom.gapWindow.value)),
    edgeTop: clampGap(Number(dom.gapEdgeTop.value)),
    edgeBottom: clampGap(Number(dom.gapEdgeBottom.value)),
    edgeLeft: clampGap(Number(dom.gapEdgeLeft.value)),
    edgeRight: clampGap(Number(dom.gapEdgeRight.value)),
    skipTopEdge: dom.gapSkipTop.checked,
    mainScreenOnly: dom.gapMainOnly.checked,
  };
}

async function commitGaps(): Promise<void> {
  const gaps = readGaps();
  try {
    config = await setGaps(gaps);
    renderBehaviour();
  } catch (err) {
    setRecordingStatus(`Could not save gaps: ${String(err)}`);
  }
}

/** Keeps the window-gap slider and its number field in sync while dragging. */
function mirrorWindowGap(raw: string): void {
  const gap = clampGap(Number(raw));
  dom.gapWindow.value = String(gap);
  dom.gapWindowNumber.value = String(gap);
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
  dom.checkUpdate.addEventListener("click", () => void runUpdateCheck());
  dom.installUpdate.addEventListener("click", () => {
    if (updateState.status === "ready-to-relaunch") {
      void installUpdate(true);
    } else {
      showUpdateConfirmation();
    }
  });
  dom.confirmUpdate.addEventListener("click", () => void applyUpdate());
  dom.cancelUpdate.addEventListener("click", () => {
    dom.updateConfirmation.close();
    dom.installUpdate.focus();
  });
  dom.updateConfirmation.addEventListener("cancel", () => {
    dom.installUpdate.focus();
  });
  dom.gapWindow.addEventListener("input", () =>
    mirrorWindowGap(dom.gapWindow.value),
  );
  dom.gapWindow.addEventListener("change", () => void commitGaps());
  dom.gapWindowNumber.addEventListener("input", () =>
    mirrorWindowGap(dom.gapWindowNumber.value),
  );
  dom.gapWindowNumber.addEventListener("change", () => void commitGaps());
  for (const input of [
    dom.gapEdgeTop,
    dom.gapEdgeBottom,
    dom.gapEdgeLeft,
    dom.gapEdgeRight,
  ]) {
    input.addEventListener("change", () => void commitGaps());
  }
  dom.gapSkipTop.addEventListener("change", () => void commitGaps());
  dom.gapMainOnly.addEventListener("change", () => void commitGaps());
  dom.subsequentMode.addEventListener("change", () => void commitCycling());

  dom.animate.addEventListener("change", async () => {
    try {
      config = await setAnimation(dom.animate.checked);
    } catch (err) {
      setRecordingStatus(`Could not update animation: ${String(err)}`);
      // Put the checkbox back where the saved config says it is, so it never
      // shows a state the app is not actually in.
      dom.animate.checked = config?.animation.enabled ?? true;
    }
  });

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
  if (isAboutScreen) {
    dom.app.classList.add("app--about");
    for (const child of dom.app.children) {
      if (child !== dom.about) (child as HTMLElement).hidden = true;
    }
    dom.about.hidden = false;
    // Wire the action before awaiting anything, so a slow or failing
    // version lookup can never leave the button dead.
    dom.github.addEventListener("click", () => {
      void openUrl(GITHUB_URL).catch((err) =>
        console.error("could not open the source repository", err),
      );
    });
    dom.aboutCheckUpdate.addEventListener("click", async () => {
      dom.aboutCheckUpdate.disabled = true;
      dom.aboutUpdateStatus.textContent = "Checking for updates…";
      try {
        dom.aboutUpdateStatus.textContent = describeAboutUpdateStatus(
          await checkForUpdates(),
        );
      } catch (err) {
        dom.aboutUpdateStatus.textContent =
          `Could not check for updates: ${String(err)}`;
      } finally {
        dom.aboutCheckUpdate.disabled = false;
      }
    });
    try {
      const version = await getVersion();
      dom.aboutVersion.textContent = version;
    } catch (err) {
      console.error("could not read app version", err);
      dom.aboutVersion.textContent = "Unavailable";
    } finally {
      dom.aboutVersion.removeAttribute("aria-busy");
    }
    return;
  }

  wireEvents();
  // Build provenance is fetched first and separately: if it fails, the rest of
  // the settings UI should still load.
  try {
    renderBuildInfo(await getBuildInfo());
  } catch (err) {
    console.error("could not read build info", err);
  }
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
  scheduleUpdateRefresh(await refreshUpdateStatus());
}

void boot();
