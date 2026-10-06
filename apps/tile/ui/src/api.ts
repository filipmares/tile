// Thin typed wrappers over the Tauri command surface (see src/commands.rs).

import { invoke } from "@tauri-apps/api/core";
import {
  AccessibilityHelp,
  AdvancedSetting,
  BuildInfo,
  Config,
  ConfigRecovery,
  CycleSize,
  Gaps,
  GrantStep,
  Hotkey,
  HotkeyStatus,
  PermissionStatus,
  SubsequentExecutionMode,
  UpdateStatus,
  WelcomeStatus,
  WindowAction,
} from "./types";

export const getConfig = (): Promise<Config> => invoke("get_config");

/** Build provenance — fixed for the process, so read once at boot. */
export const getBuildInfo = (): Promise<BuildInfo> => invoke("get_build_info");

/** The unread-settings notice still owed to the user, if any. */
export const getConfigRecovery = (): Promise<ConfigRecovery | null> =>
  invoke("get_config_recovery");

export const dismissConfigRecovery = (): Promise<void> =>
  invoke("dismiss_config_recovery");

/** Shows the kept copy of the unreadable settings file in the file manager. */
export const revealConfigBackup = (): Promise<void> =>
  invoke("reveal_config_backup");

/**
 * Binds `hotkey` to `action`. Rejects with `shortcutTaken` if another action
 * uses it, unless that action is in `replace` (the holders the user agreed
 * to replace), which unbinds it in the same write.
 */
export const setBinding = (
  action: WindowAction,
  hotkey: Hotkey | null,
  replace: WindowAction[] = [],
): Promise<Config> => invoke("set_binding", { action, hotkey, replace });

export const setGaps = (gaps: Gaps): Promise<Config> =>
  invoke("set_gaps", { gaps });

export const setCycling = (
  mode: SubsequentExecutionMode,
  sizes: CycleSize[],
): Promise<Config> => invoke("set_cycling", { mode, sizes });

export const setAnimation = (enabled: boolean): Promise<Config> =>
  invoke("set_animation", { enabled });

export const setAnimationDuration = (durationMs: number): Promise<Config> =>
  invoke("set_animation_duration", { durationMs });

/** Sets one Settings ▸ Advanced knob; the backend clamps it. */
export const setAdvanced = (setting: AdvancedSetting): Promise<Config> =>
  invoke("set_advanced", { setting });

export const setLaunchOnLogin = (enabled: boolean): Promise<Config> =>
  invoke("set_launch_on_login", { enabled });

export const resetToDefaults = (): Promise<Config> =>
  invoke("reset_to_defaults");

/**
 * Puts back the settings the last reset replaced, or resolves to `null` once
 * any other change has been made since, from any window.
 */
export const undoResetToDefaults = (): Promise<Config | null> =>
  invoke("undo_reset_to_defaults");

/**
 * Claims the one-time first-run orientation, and records that it happened.
 * True at most once, ever. The welcome screen claims it as it renders, so a
 * window that never opened leaves the first run owed for next launch.
 */
export const takeOrientation = (): Promise<boolean> =>
  invoke("take_orientation");

/** Opens the settings window, from a window that is not it. */
export const openSettings = (): Promise<void> => invoke("open_settings");

/** Reopens the welcome screen on demand. */
export const openWelcome = (): Promise<void> => invoke("open_welcome");

/** Hands the welcome window the keyboard, for its closing slide only. */
export const focusWelcome = (): Promise<void> => invoke("focus_welcome");

export const closeWelcomeWindow = (): Promise<void> => invoke("close_welcome");

/**
 * What the welcome walkthrough can honestly ask for on this machine: how many
 * displays there are to throw a window to, and whether anything movable is
 * focused right now.
 */
export const getWelcomeStatus = (): Promise<WelcomeStatus> =>
  invoke("get_welcome_status");

export const performAction = (action: WindowAction): Promise<void> =>
  invoke("perform_action", { action });

export const getPermissionStatus = (
  prompt: boolean,
): Promise<PermissionStatus> => invoke("get_permission_status", { prompt });

export const getAccessibilityHelp = (): Promise<AccessibilityHelp> =>
  invoke("get_accessibility_help");

/**
 * The primary grant button: asks macOS for its prompt the first time this
 * session, then opens the Privacy & Security pane. Resolves to what it did.
 */
export const requestAccessibility = (): Promise<GrantStep> =>
  invoke("request_accessibility");

/** Opens System Settings ▸ Privacy & Security ▸ Accessibility, with fallbacks. */
export const openAccessibilitySettings = (): Promise<void> =>
  invoke("open_accessibility_settings");

/** Shows the running Tile.app in Finder. */
export const revealAppBundle = (): Promise<void> => invoke("reveal_app_bundle");

export const getHotkeyStatus = (): Promise<HotkeyStatus> =>
  invoke("get_hotkey_status");

export const getUpdateStatus = (): Promise<UpdateStatus> =>
  invoke("get_update_status");

export const checkForUpdates = (): Promise<UpdateStatus> =>
  invoke("check_for_updates");

export const installUpdate = (
  relaunchAfterInstall: boolean,
): Promise<UpdateStatus> =>
  invoke("install_update", { relaunchAfterInstall });

/** Opens the dedicated update window, optionally checking on arrival. */
export const openUpdateWindow = (
  checkForUpdates: boolean,
): Promise<void> => invoke("open_update_window", { checkForUpdates });
