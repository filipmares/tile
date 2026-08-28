// Settings UI controller. No framework: plain DOM.

import { openUrl } from "@tauri-apps/plugin-opener";
import { getVersion } from "@tauri-apps/api/app";
import { listen } from "@tauri-apps/api/event";
import {
  checkForUpdates,
  getBuildInfo,
  getConfig,
  getHotkeyFailures,
  getPermissionStatus,
  getUpdateStatus,
  installUpdate,
  openUpdateWindow,
  resetToDefaults,
  setAnimation,
  setAnimationDuration,
  setBinding,
  setCycling,
  setGaps,
  setLaunchOnLogin,
  takeOrientation,
} from "./api";
import { formatHotkey, interpret, isMac } from "./hotkey";
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
const isUpdateScreen = new URLSearchParams(window.location.search).has(
  "updates",
);
const updateIntent = window.sessionStorage.getItem("tile-update-intent");
window.sessionStorage.removeItem("tile-update-intent");

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
  shortcutFilter: el<HTMLInputElement>("#shortcut-filter"),
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
  animationDuration: el<HTMLInputElement>("#animation-duration"),
  animationDurationNumber: el<HTMLInputElement>("#animation-duration-number"),
  launch: el<HTMLInputElement>("#launch-on-login"),
  reset: el<HTMLButtonElement>("#reset"),
  permissionPanel: el<HTMLElement>("#permission-panel"),
  orientationPanel: el<HTMLElement>("#orientation-panel"),
  orientationHome: el<HTMLSpanElement>("#orientation-home"),
  orientationKeys: el<HTMLUListElement>("#orientation-keys"),
  orientationDismiss: el<HTMLButtonElement>("#orientation-dismiss"),
  grant: el<HTMLButtonElement>("#grant-permission"),
  openAccessibility: el<HTMLButtonElement>("#open-accessibility"),
  developmentPanel: el<HTMLElement>("#development-panel"),
  developmentConfigDir: el<HTMLParagraphElement>("#development-config-dir"),
  launchDevelopmentNote: el<HTMLParagraphElement>("#launch-development-note"),
  updates: el<HTMLElement>("#updates"),
  updateVersion: el<HTMLParagraphElement>("#update-version"),
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
/** Current text in the shortcut filter. Empty means "show everything". */
let shortcutFilter = "";
/** The user's family open/closed state, stashed while a filter is active. */
let openBeforeFilter: Set<string> | null = null;
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
    count.textContent = `${actions.length} shortcuts · ${assignedCount} assigned`;

    summary.append(heading, count);
    disclosure.append(summary);

    if (family.description) {
      const description = document.createElement("p");
      description.className = "binding-group__description";
      description.textContent = family.description;
      disclosure.append(description);
    }

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
    empty.textContent = `No shortcuts match \u201c${shortcutFilter.trim()}\u201d.`;
    dom.bindings.append(empty);
  }
}

/**
 * The actions the orientation introduces: the four arrows that do the everyday
 * work, then the two display throws. Read from the live config rather than
 * hard-coded keys, so a customised binding is never described wrongly.
 */
const ORIENTATION_ACTIONS: { id: WindowAction; summary: string }[] = [
  { id: "left-half", summary: "Left half of the screen" },
  { id: "right-half", summary: "Right half of the screen" },
  { id: "maximize", summary: "Fill the screen" },
  { id: "restore", summary: "Put it back where it was" },
  { id: "previous-display", summary: "Throw to the display on the left" },
  { id: "next-display", summary: "Throw to the display on the right" },
];

/** Renders the first-run orientation from whatever is currently bound. */
function renderOrientation(cfg: Config): void {
  // Windows puts the icon in the system tray, macOS in the menu bar. This is
  // onboarding copy, so naming the wrong one sends the user hunting.
  dom.orientationHome.textContent = isMac() ? "menu bar" : "system tray";
  dom.orientationKeys.replaceChildren();
  for (const { id, summary } of ORIENTATION_ACTIONS) {
    const hk = cfg.bindings[id];
    // An unbound action has nothing to teach, so it is simply left out.
    if (!hk) continue;

    const row = document.createElement("li");
    row.className = "orientation__key";

    const combo = document.createElement("kbd");
    combo.className = "orientation__combo";
    combo.textContent = formatHotkey(hk);

    const what = document.createElement("span");
    what.className = "orientation__summary";
    what.textContent = summary;

    row.append(combo, what);
    dom.orientationKeys.append(row);
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
  mirrorAnimationDuration(String(config.animation.durationMs));
  setAnimationDurationEnabled(config.animation.enabled);
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
    if (status.status === "unavailable") {
      updatePollTimer = null;
      return;
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

  function focusUpdateScreen(): void {
    dom.updates.hidden = false;
    dom.updates.focus({ preventScroll: true });
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
      const downloadingStatus: UpdateStatus = {
        status: "downloading",
        version: updateState.version,
        downloadedBytes: 0,
        totalBytes: null,
      };
      renderUpdateStatus(downloadingStatus);
      scheduleUpdateRefresh(downloadingStatus);
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

/**
 * Clamps a typed or dragged duration into the range the core crate enforces on
 * save. These bounds match `MIN_ANIMATION_DURATION_MS` and
 * `MAX_ANIMATION_DURATION_MS`, but this is presentation only: `normalize`
 * clamps again on the way to disk and remains the real guard.
 *
 * An empty or unparseable field falls back to the saved value rather than the
 * minimum, so clearing the box and tabbing away restores what was there
 * instead of silently snapping to 40 ms.
 */
function clampAnimationDuration(raw: string): number {
  const parsed = Number(raw.trim());
  if (raw.trim() === "" || !Number.isFinite(parsed)) {
    return config?.animation.durationMs ?? 220;
  }
  return Math.round(Math.min(1000, Math.max(40, parsed)));
}

/** Puts a settled duration into both controls. */
function mirrorAnimationDuration(raw: string): void {
  const ms = String(clampAnimationDuration(raw));
  dom.animationDuration.value = ms;
  dom.animationDurationNumber.value = ms;
}

/** Duration is meaningless while animation is off, so it follows the toggle. */
function setAnimationDurationEnabled(enabled: boolean): void {
  dom.animationDuration.disabled = !enabled;
  dom.animationDurationNumber.disabled = !enabled;
}

async function commitAnimationDuration(): Promise<void> {
  try {
    config = await setAnimationDuration(Number(dom.animationDuration.value));
    mirrorAnimationDuration(String(config.animation.durationMs));
  } catch (err) {
    setRecordingStatus(`Could not update animation duration: ${String(err)}`);
    if (config) mirrorAnimationDuration(String(config.animation.durationMs));
  }
}

/** Refreshes the permission panel, returning whether permission is denied. */
async function refreshPermission(prompt: boolean): Promise<boolean> {
  let status;
  try {
    status = await getPermissionStatus(prompt);
  } catch (err) {
    // An unreadable status is not a denial. The Rust side applies hotkeys
    // anyway in this case, so treating it as denied here would strand the
    // orientation forever.
    console.error("permission check failed", err);
    return false;
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
    // This is also the moment the orientation was waiting for. The shortcuts
    // it describes only started working just now.
    await maybeShowOrientation(false);
  }

  return denied;
}

function wireUpdateEvents(): void {
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
}

function wireEvents(): void {
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
      setAnimationDurationEnabled(config.animation.enabled);
    } catch (err) {
      setRecordingStatus(`Could not update animation: ${String(err)}`);
      // Put the checkbox back where the saved config says it is, so it never
      // shows a state the app is not actually in.
      dom.animate.checked = config?.animation.enabled ?? true;
    }
  });

  // Dragging the slider only ever produces an in-range value, so both controls
  // can track it live.
  dom.animationDuration.addEventListener("input", () =>
    mirrorAnimationDuration(dom.animationDuration.value),
  );
  dom.animationDuration.addEventListener(
    "change",
    () => void commitAnimationDuration(),
  );

  // Typing is different. Rewriting the field on every keystroke would turn "2"
  // into "40" before the user could finish typing "200", so while the edit is
  // in progress only the slider follows along. The field itself is normalized
  // once the edit is committed on blur or Enter.
  dom.animationDurationNumber.addEventListener("input", () => {
    dom.animationDuration.value = String(
      clampAnimationDuration(dom.animationDurationNumber.value),
    );
  });
  dom.animationDurationNumber.addEventListener("change", () => {
    mirrorAnimationDuration(dom.animationDurationNumber.value);
    void commitAnimationDuration();
  });

  dom.launch.addEventListener("change", async () => {
    try {
      config = await setLaunchOnLogin(dom.launch.checked);
    } catch (err) {
      setRecordingStatus(`Could not update launch-on-login: ${String(err)}`);
      dom.launch.checked = config?.launchOnLogin ?? false;
    }
  });

  dom.shortcutFilter.addEventListener("input", () => {
    shortcutFilter = dom.shortcutFilter.value;
    renderBindings();
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

/**
 * Shows the one-time first-run orientation, unless Accessibility permission is
 * still missing.
 *
 * The gate matters because the claim is consumed permanently. Showing
 * "hold this modifier and press an arrow" directly above a panel saying those
 * shortcuts do nothing would contradict itself, and would spend the single
 * orientation at the one moment Tile cannot actually do anything. When
 * permission is denied the claim is left untouched, and `refreshPermission`
 * retries as soon as the user grants it.
 */
async function maybeShowOrientation(permissionDenied: boolean): Promise<void> {
  if (!config || permissionDenied) return;
  let owed = false;
  try {
    owed = await takeOrientation();
  } catch (err) {
    // Orientation is a nicety; never let it stop the settings UI loading.
    console.error("could not check first-run orientation", err);
    return;
  }
  if (!owed) return;

  renderOrientation(config);
  dom.orientationPanel.hidden = false;
  // Claiming already recorded that orientation was shown, so dismissal is
  // purely visual.
  dom.orientationDismiss.addEventListener("click", () => {
    dom.orientationPanel.hidden = true;
  });
}

/** Hides every screen except `screen`, which becomes the whole window. */
function showOnly(screen: HTMLElement, modifier: string): void {
  dom.app.classList.add(modifier);
  for (const child of dom.app.children) {
    if (child !== screen) (child as HTMLElement).hidden = true;
  }
  screen.hidden = false;
}

async function bootAbout(): Promise<void> {
  showOnly(dom.about, "app--about");
  // Wire the actions before awaiting anything, so a slow or failing
  // version lookup can never leave a button dead.
  dom.github.addEventListener("click", () => {
    void openUrl(GITHUB_URL).catch((err) =>
      console.error("could not open the source repository", err),
    );
  });
  // About never updates anything itself: it hands over to the window that
  // owns the whole flow, and asks it to start a check on arrival.
  dom.aboutCheckUpdate.addEventListener("click", async () => {
    dom.aboutCheckUpdate.disabled = true;
    dom.aboutUpdateStatus.textContent = "";
    try {
      await openUpdateWindow(true);
    } catch (err) {
      dom.aboutUpdateStatus.textContent = `Could not open the update window: ${String(err)}`;
    } finally {
      dom.aboutCheckUpdate.disabled = false;
    }
  });
  try {
    dom.aboutVersion.textContent = await getVersion();
  } catch (err) {
    console.error("could not read app version", err);
    dom.aboutVersion.textContent = "Unavailable";
  } finally {
    dom.aboutVersion.removeAttribute("aria-busy");
  }
}

/**
 * The dedicated update screen: check, download, install, relaunch. It is the
 * only place any of that happens, so it re-runs a check whenever the tray
 * asks for one, even if the window was already open.
 */
async function bootUpdates(): Promise<void> {
  showOnly(dom.updates, "app--updates");
  wireUpdateEvents();
  focusUpdateScreen();
  // Opening the screen at all is a request to update, so an unknown intent
  // still checks rather than sitting on a stale "not checked yet".
  const initialUpdateStatus =
    updateIntent === "show" ? refreshUpdateStatus() : runUpdateCheck();
  getVersion()
    .then((version) => {
      dom.updateVersion.textContent = `Tile ${version}`;
    })
    .catch((err) => {
      console.error("could not read app version", err);
      dom.updateVersion.textContent = "Tile";
    });
  // Re-entry from the tray while the window is already open. A failure here
  // must not cost the check that is already running.
  try {
    await listen("tile://check-for-updates", () => {
      focusUpdateScreen();
      void runUpdateCheck();
    });
    await listen("tile://show-updates", () => {
      focusUpdateScreen();
      void refreshUpdateStatus().then(scheduleUpdateRefresh);
    });
  } catch (err) {
    console.error("could not listen for update requests", err);
  }
  scheduleUpdateRefresh(await initialUpdateStatus);
}

async function boot(): Promise<void> {
  if (isAboutScreen) {
    await bootAbout();
    return;
  }
  if (isUpdateScreen) {
    await bootUpdates();
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
  // Permission is resolved first: the orientation must not appear while the
  // shortcuts it describes are still inert.
  const permissionDenied = await refreshPermission(false);
  await maybeShowOrientation(permissionDenied);
}

void boot();
