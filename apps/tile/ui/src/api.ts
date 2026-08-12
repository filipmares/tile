// Thin typed wrappers over the Tauri command surface (see src/commands.rs).

import { invoke } from "@tauri-apps/api/core";
import {
  Config,
  Hotkey,
  HotkeyFailure,
  PermissionStatus,
  WindowAction,
} from "./types";

export const getConfig = (): Promise<Config> => invoke("get_config");

export const setBinding = (
  action: WindowAction,
  hotkey: Hotkey | null,
): Promise<Config> => invoke("set_binding", { action, hotkey });

export const setGap = (gap: number): Promise<Config> =>
  invoke("set_gap", { gap });

export const setLaunchOnLogin = (enabled: boolean): Promise<Config> =>
  invoke("set_launch_on_login", { enabled });

export const resetToDefaults = (): Promise<Config> =>
  invoke("reset_to_defaults");

export const performAction = (action: WindowAction): Promise<void> =>
  invoke("perform_action", { action });

export const getPermissionStatus = (
  prompt: boolean,
): Promise<PermissionStatus> => invoke("get_permission_status", { prompt });

export const getHotkeyFailures = (): Promise<HotkeyFailure[]> =>
  invoke("get_hotkey_failures");
