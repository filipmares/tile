// Thin typed wrappers over the Tauri command surface (see src/commands.rs).

import { invoke } from "@tauri-apps/api/core";
import {
  BuildInfo,
  Config,
  CycleSize,
  Gaps,
  Hotkey,
  HotkeyFailure,
  PermissionStatus,
  SubsequentExecutionMode,
  UpdateStatus,
  WelcomeStatus,
  WindowAction,
} from "./types";

export const getConfig = (): Promise<Config> => invoke("get_config");

/** Build provenance — fixed for the process, so read once at boot. */
export const getBuildInfo = (): Promise<BuildInfo> => invoke("get_build_info");

export const setBinding = (
  action: WindowAction,
  hotkey: Hotkey | null,
): Promise<Config> => invoke("set_binding", { action, hotkey });

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

export const setLaunchOnLogin = (enabled: boolean): Promise<Config> =>
  invoke("set_launch_on_login", { enabled });

export const resetToDefaults = (): Promise<Config> =>
  invoke("reset_to_defaults");

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

export const getHotkeyFailures = (): Promise<HotkeyFailure[]> =>
  invoke("get_hotkey_failures");

export const getUpdateStatus = (): Promise<UpdateStatus> =>
  invoke("get_update_status");

export const checkForUpdates = (): Promise<UpdateStatus> =>
  invoke("check_for_updates");

export const installUpdate = (
  relaunchAfterInstall: boolean,
): Promise<UpdateStatus> =>
  invoke("install_update", { relaunchAfterInstall });
