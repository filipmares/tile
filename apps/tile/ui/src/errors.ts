// Turns a failed settings command into a sentence a person can act on.
//
// The Rust side rejects with `{ kind, detail }` (see `settings_error.rs`).
// `kind` is a stable tag; `detail` is raw error text for the console only and
// is never shown, because "Access is denied. (os error 5)" next to a checkbox
// explains nothing.

export type SettingsErrorKind = "notSaved" | "loginItem" | "outOfSync";

/** The `kind` of a settings command rejection, or `null` for anything else. */
export function settingsErrorKind(err: unknown): SettingsErrorKind | null {
  if (typeof err !== "object" || err === null || !("kind" in err)) return null;
  const kind = (err as { kind: unknown }).kind;
  return kind === "notSaved" || kind === "loginItem" || kind === "outOfSync"
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
