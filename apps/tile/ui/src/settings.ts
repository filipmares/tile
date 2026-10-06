// The settings screen: behaviour, motion, startup and advanced controls,
// restore defaults and its undo, the permission panel, and the notice shown
// when saved settings were unreadable. Every change goes through the write
// queue in `writes.ts`.

import { listen } from "@tauri-apps/api/event";
import {
  dismissConfigRecovery,
  getBuildInfo,
  getConfig,
  getAccessibilityHelp,
  getConfigRecovery,
  getPermissionStatus,
  openAccessibilitySettings,
  openWelcome,
  requestAccessibility,
  resetToDefaults,
  revealAppBundle,
  revealConfigBackup,
  setAnimation,
  setAnimationDuration,
  setAdvanced,
  setCycling,
  setGaps,
  setLaunchOnLogin,
  undoResetToDefaults,
} from "./api";
import { confirmDialog } from "./confirm";
import { dom } from "./dom";
import { resetErrorMessage, settingsErrorMessage } from "./errors";
import { isMac } from "./hotkey";
import { applySettingsSearch, wireSettingsSearch } from "./settingsSearch";
import {
  listenForHotkeyStatus,
  refreshHotkeyStatus,
  renderBindings,
  setRecordingStatus,
} from "./shortcuts";
import { config, setConfig } from "./state";
import {
  AdvancedField,
  BuildInfo,
  Config,
  ConfigRecovery,
  CYCLE_SIZES,
  CycleSize,
  Gaps,
  SubsequentExecutionMode,
} from "./types";
import {
  configureWrites,
  followChangesElsewhere,
  saveSetting,
  setFieldError as setSettingsError,
} from "./writes";

export { saveSetting };

/** User-facing copy for the settings screen. */
const STRINGS = {
  resetTitle: "Restore defaults?",
  resetMessage:
    "Every shortcut, the window gaps, animation, repeat-press behaviour, and launch at login go back to their defaults. You can undo this for a few seconds afterwards.",
  resetConfirm: "Restore defaults",
  defaultsRestored: "Defaults restored.",
  undoOffered: "Defaults restored. Undo is available for a few seconds.",
  undoExpired:
    "Settings changed after the reset, so it can no longer be undone.",
  undone: "Previous settings restored.",
  configDir: (dir: string) => `Settings are stored in ${dir}`,
  recoveryReason: {
    corrupt: "Tile could not read your settings, so it started from defaults.",
    "partial-reset":
      "Some settings couldn't be read: they were reset to their defaults, and any unreadable shortcuts were cleared.",
    "newer-version":
      "Your settings were saved by a newer version of Tile. Settings this version understands were kept.",
  } satisfies Record<ConfigRecovery["kind"], string>,
  newerVersionWithResets:
    "Your settings were saved by a newer version of Tile. Some settings couldn't be read: they were reset to their defaults, and any unreadable shortcuts were cleared.",
  recoveredWithBackup: "A copy of the old file was kept.",
  recoveredWithoutBackup:
    "The old file could not be copied, so changes made now will not be saved until Tile restarts — that keeps the old file from being overwritten.",
  openFolderFailed: (err: unknown) =>
    `Could not open the folder: ${String(err)}`,
  loadFailed: (err: unknown) => `Could not load settings: ${String(err)}`,
  grantPrompt: "Grant permission…",
  grantOpenSettings: "Open Accessibility settings…",
  accessibilityOpened:
    "Continue in the settings app that opened, using the steps above.",
  accessibilityFailed: (err: unknown) => String(err),
  revealFailed: (err: unknown) => `Could not show Tile in Finder: ${String(err)}`,
};

/** Emitted by the Rust side whenever the shared permission state changes. */
const PERMISSION_CHANGED_EVENT = "permission-changed";

/**
 * Whether Undo is on offer after a restore-defaults. The backend keeps the
 * snapshot and drops it on any other change from any window, so this only
 * decides whether to show the button.
 */
let undoAvailable = false;
let undoTimer: number | null = null;
const UNDO_WINDOW_MS = 10_000;

let permissionTimer: number | null = null;

function setResetStatus(text: string): void {
  dom.resetStatus.textContent = text;
}

/** Ends the chance to undo a reset, keeping focus off the vanishing button. */
function clearUndo(): void {
  if (undoTimer !== null) {
    window.clearTimeout(undoTimer);
    undoTimer = null;
  }
  if (!undoAvailable) return;
  undoAvailable = false;
  if (document.activeElement === dom.undoReset) dom.reset.focus();
  dom.undoReset.hidden = true;
  setResetStatus(STRINGS.defaultsRestored);
}

/**
 * Re-renders everything a whole-config change can touch. The behaviour
 * controls are skipped while a newer write is queued, since they already show
 * that newer choice; the shortcut list has no such pending edits.
 *
 * The write has already been saved by the time this runs, so a failed status
 * read must not surface as a failed save: the list still renders from the
 * saved config, with the last hotkey status it knew.
 */
export async function renderWholeConfig(latest: boolean): Promise<void> {
  try {
    await refreshHotkeyStatus();
  } catch (err) {
    console.error("could not refresh hotkey status", err);
  }
  renderBindings();
  if (latest) renderBehaviour();
}

async function restoreDefaults(): Promise<void> {
  // Every reset supersedes earlier feedback, including the half-success
  // where only launch at login stays put; `reset-error` reports the outcome.
  setRecordingStatus("");
  for (const target of [
    dom.bindingError,
    dom.cyclingError,
    dom.gapsError,
    dom.motionError,
    dom.launchError,
    dom.advancedError,
  ]) {
    setSettingsError(target, null);
  }
  setResetStatus("");
  // The backend keeps what the reset replaced, and keeps the first snapshot
  // if this is a repeat, so Undo always leads back to the user's settings.
  const saved = await saveSetting(
    dom.resetError,
    resetToDefaults,
    resetErrorMessage,
  );
  if (saved === false) return;
  // A newer change is already queued: it ends the undo and renders its own
  // reply.
  if (saved === null) {
    setResetStatus(STRINGS.defaultsRestored);
    await renderWholeConfig(false);
    return;
  }
  // Offered in the same turn the reply arrives, before any await, so a change
  // made right after the reset still finds the offer and ends it.
  undoAvailable = true;
  dom.undoReset.hidden = false;
  undoTimer = window.setTimeout(clearUndo, UNDO_WINDOW_MS);
  setResetStatus(STRINGS.undoOffered);
  await renderWholeConfig(true);
}

/** Rejection used when another window changed a setting after the reset. */
const UNDO_EXPIRED = Symbol("undo expired");

function undoErrorMessage(err: unknown): string {
  return err === UNDO_EXPIRED
    ? STRINGS.undoExpired
    : settingsErrorMessage(err);
}

async function undoRestoreDefaults(): Promise<void> {
  if (!undoAvailable) return;
  // Undo is one-shot: drop it before the write so a second press cannot
  // queue the same restore twice.
  clearUndo();
  setResetStatus("");
  setSettingsError(dom.resetError, null);
  const saved = await saveSetting(
    dom.resetError,
    async () => {
      const restored = await undoResetToDefaults();
      if (restored === null) throw UNDO_EXPIRED;
      return restored;
    },
    undoErrorMessage,
  );
  if (saved === false) return;
  setResetStatus(STRINGS.undone);
  await renderWholeConfig(saved !== null);
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
  renderAdvanced(config);
  // The cycle sizes are built on first render, after the search may have run.
  applySettingsSearch();
}

/**
 * One Settings ▸ Advanced control. Fractions are shown as whole percentages;
 * `min` and `max` are in the shown unit and match `Config::set_advanced`.
 */
interface AdvancedControl {
  input: HTMLInputElement;
  field: AdvancedField;
  min: number;
  max: number;
  /** The saved value, in the unit the control shows. */
  read: (cfg: Config) => number;
  /** A shown value, in the unit the config stores. */
  toValue: (shown: number) => number;
}

const fromPercent = (shown: number): number => shown / 100;
const asIs = (shown: number): number => shown;

function percentControl(
  input: HTMLInputElement,
  field: AdvancedField,
  read: (cfg: Config) => number,
): AdvancedControl {
  return {
    input,
    field,
    min: 1,
    max: 100,
    read: (cfg) => Math.round(read(cfg) * 100),
    toValue: fromPercent,
  };
}

function stepControl(
  input: HTMLInputElement,
  field: AdvancedField,
  read: (cfg: Config) => number,
): AdvancedControl {
  return {
    input,
    field,
    min: 1,
    max: 1000,
    read: (cfg) => Math.round(read(cfg)),
    toValue: asIs,
  };
}

const ADVANCED_CONTROLS: AdvancedControl[] = [
  percentControl(
    dom.advancedAlmostMaximizeWidth,
    "almostMaximizeWidth",
    (cfg) => cfg.almostMaximizeWidth,
  ),
  percentControl(
    dom.advancedAlmostMaximizeHeight,
    "almostMaximizeHeight",
    (cfg) => cfg.almostMaximizeHeight,
  ),
  stepControl(dom.advancedSizeStep, "sizeStep", (cfg) => cfg.sizeStep),
  stepControl(dom.advancedWidthStep, "widthStep", (cfg) => cfg.widthStep),
  stepControl(dom.advancedMoveStep, "moveStep", (cfg) => cfg.moveStep),
  percentControl(
    dom.advancedMinimumWidth,
    "minimumWindowWidth",
    (cfg) => cfg.minimumWindowWidth,
  ),
  percentControl(
    dom.advancedMinimumHeight,
    "minimumWindowHeight",
    (cfg) => cfg.minimumWindowHeight,
  ),
  {
    input: dom.advancedAnimationFps,
    field: "animationFps",
    min: 15,
    max: 240,
    read: (cfg) => cfg.animation.fps,
    toValue: asIs,
  },
];

function renderAdvanced(cfg: Config): void {
  for (const control of ADVANCED_CONTROLS) {
    control.input.value = String(control.read(cfg));
  }
}

/**
 * Saves one Advanced control once its edit is committed (blur or Enter).
 * An empty or unparseable field goes back to the saved value; anything else
 * is clamped to the control's range here so the field settles at once, and
 * then shows whatever the backend actually saved.
 */
async function commitAdvanced(control: AdvancedControl): Promise<void> {
  const raw = control.input.value.trim();
  const parsed = Number(raw);
  if (raw === "" || !Number.isFinite(parsed)) {
    if (config) control.input.value = String(control.read(config));
    return;
  }
  const shown = Math.round(Math.min(control.max, Math.max(control.min, parsed)));
  control.input.value = String(shown);
  const setting = { field: control.field, value: control.toValue(shown) };
  if (await saveSetting(dom.advancedError, () => setAdvanced(setting))) {
    if (config) control.input.value = String(control.read(config));
  }
}

/** Marks the window as a development build. Installed builds render nothing. */
function renderBuildInfo(info: BuildInfo): void {
  if (info.kind !== "development") return;
  dom.developmentPanel.hidden = false;
  // The launch-on-login toggle is the one control whose behaviour differs, so
  // it says so where it is, not only in the panel at the top.
  dom.launchDevelopmentNote.hidden = false;
  // Linked only here: a description can be read even while hidden, and an
  // installed build must not be told its login item is not applied.
  dom.launch.setAttribute("aria-describedby", dom.launchDevelopmentNote.id);
  if (info.configDir) {
    dom.developmentConfigDir.textContent = STRINGS.configDir(info.configDir);
    dom.developmentConfigDir.hidden = false;
  }
}

/**
 * Tells the user, once, that this launch could not read their saved settings.
 * The panel is wired here rather than in `wireEvents` because it only exists
 * when there is something to say.
 */
async function bootConfigRecovery(): Promise<void> {
  let recovery: ConfigRecovery | null;
  try {
    recovery = await getConfigRecovery();
  } catch (err) {
    console.error("could not read the settings recovery notice", err);
    return;
  }
  if (!recovery) return;

  const reason =
    recovery.kind === "newer-version" && recovery.someFieldsReset
      ? STRINGS.newerVersionWithResets
      : STRINGS.recoveryReason[recovery.kind];
  if (recovery.backupPath) {
    dom.recoveryMessage.textContent = `${reason} ${STRINGS.recoveredWithBackup}`;
    dom.recoveryPath.textContent = recovery.backupPath;
    dom.recoveryPath.hidden = false;
  } else {
    dom.recoveryMessage.textContent = `${reason} ${STRINGS.recoveredWithoutBackup}`;
  }
  dom.recoveryPanel.hidden = false;

  dom.recoveryOpenFolder.addEventListener("click", async () => {
    dom.recoveryStatus.textContent = "";
    try {
      await revealConfigBackup();
    } catch (err) {
      dom.recoveryStatus.textContent = STRINGS.openFolderFailed(err);
    }
  });
  dom.recoveryDismiss.addEventListener("click", async () => {
    dom.recoveryPanel.hidden = true;
    try {
      await dismissConfigRecovery();
    } catch (err) {
      console.error("could not dismiss the settings recovery notice", err);
    }
  });
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
  if (await saveSetting(dom.cyclingError, () => setCycling(mode, sizes))) {
    renderBehaviour();
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
  if (await saveSetting(dom.gapsError, () => setGaps(gaps))) {
    renderBehaviour();
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
  const durationMs = Number(dom.animationDuration.value);
  if (await saveSetting(dom.motionError, () => setAnimationDuration(durationMs))) {
    if (config) mirrorAnimationDuration(String(config.animation.durationMs));
  }
}

/**
 * Fits the panel to where the user is: every press of the primary button opens
 * the Privacy & Security pane, and the first one also asks macOS for its
 * prompt, which it only ever shows once. After that the separate "Open" button
 * would be a duplicate. Finder can only reveal a real app bundle.
 */
async function renderAccessibilityHelp(): Promise<void> {
  let help;
  try {
    help = await getAccessibilityHelp();
  } catch (err) {
    console.error("could not read accessibility help", err);
    return;
  }
  const prompted = help.grantStep === "open-settings";
  dom.grant.textContent = prompted
    ? STRINGS.grantOpenSettings
    : STRINGS.grantPrompt;
  dom.openAccessibility.hidden = prompted;
  dom.permissionPrompted.hidden = !prompted;
  dom.revealApp.hidden = help.appBundle === null;
  dom.permissionUnbundled.hidden = help.appBundle !== null;
}

function setPermissionStatus(text: string): void {
  dom.permissionStatus.textContent = text;
}

async function grantPermission(): Promise<void> {
  setPermissionStatus("");
  let message: string;
  try {
    await requestAccessibility();
    message = STRINGS.accessibilityOpened;
  } catch (err) {
    message = STRINGS.accessibilityFailed(err);
  }
  await renderAccessibilityHelp();
  await refreshPermission();
  // Granted meanwhile: the panel is gone and there is nothing left to say.
  if (!dom.permissionPanel.hidden) setPermissionStatus(message);
}

/** Refreshes the permission panel, polling while permission is denied. */
async function refreshPermission(): Promise<void> {
  let status;
  try {
    status = await getPermissionStatus(false);
  } catch (err) {
    // An unreadable status is not a denial: the Rust side applies hotkeys
    // anyway in that case, so the panel stays quiet rather than accusing.
    console.error("permission check failed", err);
    return;
  }

  const denied = status === "denied";
  if (denied && dom.permissionPanel.hidden) {
    await renderAccessibilityHelp();
  }
  dom.permissionPanel.hidden = !denied;

  if (denied && permissionTimer === null) {
    permissionTimer = window.setInterval(() => void refreshPermission(), 2000);
  } else if (!denied && permissionTimer !== null) {
    window.clearInterval(permissionTimer);
    permissionTimer = null;
    setPermissionStatus("");
    // Permission just became available: surface any late hotkey status.
    await refreshHotkeyStatus();
    renderBindings();
  }
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
    const enabled = dom.animate.checked;
    if (await saveSetting(dom.motionError, () => setAnimation(enabled))) {
      if (config) setAnimationDurationEnabled(config.animation.enabled);
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

  dom.launch.addEventListener("change", () => {
    const enabled = dom.launch.checked;
    void saveSetting(dom.launchError, () => setLaunchOnLogin(enabled));
  });

  for (const control of ADVANCED_CONTROLS) {
    control.input.addEventListener("change", () => void commitAdvanced(control));
  }
  // Steps are in the backend's own unit: physical pixels on Windows, points
  // on macOS.
  if (isMac()) {
    for (const unit of document.querySelectorAll("[data-step-unit]")) {
      unit.textContent = "pt";
    }
  }

  wireSettingsSearch();

  dom.reset.addEventListener("click", () => {
    void confirmDialog({
      title: STRINGS.resetTitle,
      message: STRINGS.resetMessage,
      confirmLabel: STRINGS.resetConfirm,
      danger: true,
      returnFocus: () => dom.reset,
    }).then((ok) => {
      if (ok) void restoreDefaults();
    });
  });
  dom.undoReset.addEventListener("click", () => void undoRestoreDefaults());

  dom.grant.addEventListener("click", () => void grantPermission());
  dom.showWelcome.addEventListener("click", () => {
    void openWelcome().catch((err) =>
      console.error("could not open the welcome window", err),
    );
  });
  dom.openAccessibility.addEventListener("click", async () => {
    setPermissionStatus("");
    try {
      await openAccessibilitySettings();
      setPermissionStatus(STRINGS.accessibilityOpened);
    } catch (err) {
      setPermissionStatus(STRINGS.accessibilityFailed(err));
    }
  });
  dom.revealApp.addEventListener("click", async () => {
    setPermissionStatus("");
    try {
      await revealAppBundle();
    } catch (err) {
      setPermissionStatus(STRINGS.revealFailed(err));
    }
  });
}

let booted = false;

/** Boots the settings screen, the window's default. Runs once. */
export async function bootSettings(): Promise<void> {
  if (booted) return;
  booted = true;
  // Every write, from this window or another, ends the backend's undo
  // snapshot, so the offer goes with it.
  configureWrites({
    beforeWrite: clearUndo,
    changedElsewhere: clearUndo,
    render: () => {
      renderBehaviour();
      renderBindings();
    },
    renderElsewhere: () => {
      // A field mid-edit only commits on change, so nothing is queued for it
      // yet; keep what the user is typing rather than resetting it.
      const active = document.activeElement;
      const editing =
        active instanceof HTMLInputElement && active.type !== "checkbox"
          ? active.value
          : null;
      renderBehaviour();
      renderBindings();
      if (editing !== null) {
        const field = active as HTMLInputElement;
        field.value = editing;
        // Paired controls (slider and number) follow the field through its
        // input listener; re-run it so a later commit reads the edit.
        field.dispatchEvent(new Event("input", { bubbles: true }));
      }
    },
  });
  wireEvents();
  // Subscribed before the first read below, so no change is missed between.
  await followChangesElsewhere();
  // Build provenance is fetched first and separately: if it fails, the rest of
  // the settings UI should still load.
  try {
    renderBuildInfo(await getBuildInfo());
  } catch (err) {
    console.error("could not read build info", err);
  }
  await bootConfigRecovery();
  // A grant or revocation noticed in the background (or by a failed action)
  // shows here straight away, even while the panel is hidden and not polling.
  // Awaited before the first permission check below, so a change in between
  // is not lost.
  try {
    await listen(PERMISSION_CHANGED_EVENT, () => void refreshPermission());
  } catch (err) {
    console.error("could not listen for permission changes", err);
  }
  // Subscribed before the first fetch, so a change in between is not lost.
  try {
    await listenForHotkeyStatus();
  } catch (err) {
    console.error("could not follow hotkey status changes", err);
  }
  try {
    setConfig(await getConfig());
    await refreshHotkeyStatus();
  } catch (err) {
    setRecordingStatus(STRINGS.loadFailed(err));
    return;
  }
  renderBindings();
  renderBehaviour();
  await refreshPermission();
}
