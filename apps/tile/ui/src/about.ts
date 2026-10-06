// The About window: version, source link, and a hand-off to the update window.

import { getVersion } from "@tauri-apps/api/app";
import { openUrl } from "@tauri-apps/plugin-opener";
import { openUpdateWindow } from "./api";
import { dom, showOnly } from "./dom";

/** User-facing copy for the About window. */
const STRINGS = {
  openUpdatesFailed: (err: unknown) =>
    `Could not open the update window: ${String(err)}`,
  versionUnavailable: "Unavailable",
};

const GITHUB_URL = "https://github.com/filipmares/tile";

let booted = false;

/** Boots the About window. Runs once. */
export async function bootAbout(): Promise<void> {
  if (booted) return;
  booted = true;
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
      dom.aboutUpdateStatus.textContent = STRINGS.openUpdatesFailed(err);
    } finally {
      dom.aboutCheckUpdate.disabled = false;
    }
  });
  try {
    dom.aboutVersion.textContent = await getVersion();
  } catch (err) {
    console.error("could not read app version", err);
    dom.aboutVersion.textContent = STRINGS.versionUnavailable;
  } finally {
    dom.aboutVersion.removeAttribute("aria-busy");
  }
}
