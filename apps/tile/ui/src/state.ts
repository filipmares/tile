// State shared between the settings controls and the shortcut list.

import { Config } from "./types";

/** The settings Tile last reported, or `null` before the first read. */
export let config: Config | null = null;

export function setConfig(next: Config | null): void {
  config = next;
}
