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

export const setLaunchOnLogin = (enabled: boolean): Promise<Config> =>
  invoke("set_launch_on_login", { enabled });

export const setCheckForUpdates = (enabled: boolean): Promise<Config> =>
  invoke("set_check_for_updates", { enabled });

export const resetToDefaults = (): Promise<Config> =>
  invoke("reset_to_defaults");

export const performAction = (action: WindowAction): Promise<void> =>
  invoke("perform_action", { action });

export const getPermissionStatus = (
  prompt: boolean,
): Promise<PermissionStatus> => invoke("get_permission_status", { prompt });

export const getHotkeyFailures = (): Promise<HotkeyFailure[]> =>
  invoke("get_hotkey_failures");
