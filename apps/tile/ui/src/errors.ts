// Turns a failed settings or update command into a sentence a person can act on.
//
// The Rust side rejects with `{ kind, detail }` (see `settings_error.rs` and
// `update.rs`).
// `kind` is a stable tag; `detail` is raw error text for the console only and
// is never shown, because "Access is denied. (os error 5)" next to a checkbox
// explains nothing.

import type { UpdateErrorKind } from "./types";

export type SettingsErrorKind =
  | "notSaved"
  | "loginItem"
  | "outOfSync"
  | "shortcutTaken";

/** The `kind` of a settings command rejection, or `null` for anything else. */
export function settingsErrorKind(err: unknown): SettingsErrorKind | null {
  if (typeof err !== "object" || err === null || !("kind" in err)) return null;
  const kind = (err as { kind: unknown }).kind;
  return kind === "notSaved" ||
    kind === "loginItem" ||
    kind === "outOfSync" ||
    kind === "shortcutTaken"
    ? kind
    : null;
}

/** A plain message for a failed settings change. */
export function settingsErrorMessage(err: unknown): string {
  switch (settingsErrorKind(err)) {
    case "notSaved":
      return "Tile could not save this change, so it was undone. Check that your settings folder is not read-only or full, then try again.";
    case "loginItem":
      return "Tile could not change whether it opens at login. Your system may be blocking it. Try again, or check the login items in your system settings.";
    case "outOfSync":
      return "Tile could not save this change, so it was undone, but whether Tile opens at login may no longer match this setting. Turn Launch Tile at login off and on again to fix it.";
    case "shortcutTaken":
      return "Another action started using this shortcut before it was saved, so nothing changed. Record it again to choose whether to replace it.";
    default:
      return "Tile could not apply this change. Try again.";
  }
}

/** As `settingsErrorMessage`, for Restore defaults, which can half-succeed. */
export function resetErrorMessage(err: unknown): string {
  if (settingsErrorKind(err) === "loginItem") {
    return "Defaults restored, except Launch Tile at login: Tile could not change it, so it stays as it was.";
  }
  return settingsErrorMessage(err);
}

const UPDATE_ERROR_KINDS: readonly UpdateErrorKind[] = [
  "offline",
  "server",
  "signature",
  "interrupted",
  "disk",
  "installer",
  "unknown",
];

/**
 * The `kind` of a failed update command (`update.rs` rejects with
 * `{ kind, detail }`), or `"unknown"` for anything else. The detail goes to
 * the console only.
 */
export function updateErrorKind(err: unknown): UpdateErrorKind {
  console.error("update command failed", err);
  if (typeof err !== "object" || err === null || !("kind" in err)) {
    return "unknown";
  }
  const kind = (err as { kind: unknown }).kind;
  return UPDATE_ERROR_KINDS.includes(kind as UpdateErrorKind)
    ? (kind as UpdateErrorKind)
    : "unknown";
}

/** One plain sentence for a failed update, plus what to do about it. */
export function updateErrorMessage(kind: UpdateErrorKind): string {
  switch (kind) {
    case "offline":
      return "Tile could not reach GitHub. Check your connection and try again.";
    case "server":
      return "GitHub did not send a usable update. Try again later.";
    case "signature":
      return "The update failed Tile's signature check, so it was not installed. Try again later.";
    case "interrupted":
      return "The download did not finish. Check your connection and try again.";
    case "disk":
      return "Tile could not save the update. Check that your disk has free space and that you can write to it, then try again.";
    case "installer":
      return "The installer did not finish or was cancelled. Try again, and allow the installer if your system asks.";
    default:
      return "Tile could not update. Try again later. If it keeps happening, check Tile's log.";
  }
}
