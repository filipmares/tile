// The update window: check, download, install, relaunch.

import { getVersion } from "@tauri-apps/api/app";
import { listen } from "@tauri-apps/api/event";
import { openUrl } from "@tauri-apps/plugin-opener";
import { checkForUpdates, getUpdateStatus, installUpdate } from "./api";
import { dom, showOnly } from "./dom";
import { updateErrorKind, updateErrorMessage } from "./errors";
import { UpdateStatus } from "./types";

/** User-facing copy for the update window. */
const STRINGS = {
  unavailable:
    "Production update checks are unavailable in this development build.",
  idle: "Tile has not checked for updates yet.",
  checking: "Checking for updates…",
  current: "Tile is up to date.",
  available: (version: string) => `Tile ${version} is available.`,
  updateNow: "Update now",
  downloading: (version: string) => `Downloading Tile ${version}.`,
  downloadedMb: (mb: string) => `${mb} MB downloaded`,
  downloadedPercent: (percent: number) => `${percent}% downloaded`,
  readyToRelaunch: (version: string) =>
    `Tile ${version} is installed and ready to relaunch.`,
  relaunch: "Relaunch Tile",
  retry: "Retry",
  checkForUpdates: "Check for updates",
  confirmWindows:
    "Tile will close, install the update, and reopen automatically. Continue?",
  confirmOther:
    "Tile will install the update. You can relaunch after it finishes. Continue?",
  version: (version: string) => `Tile ${version}`,
  versionUnknown: "Tile",
};

const RELEASE_NOTE_URL = /https?:\/\/[^\s<>"']+/g;

const updateIntent = window.sessionStorage.getItem("tile-update-intent");
window.sessionStorage.removeItem("tile-update-intent");

let updatePollTimer: number | null = null;
let updateState: UpdateStatus = { status: "idle" };
let updateNotes: string | null = null;

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
    renderUpdateNotes(updateNotes);

    switch (status.status) {
      case "unavailable":
        setUpdateAnnouncement(STRINGS.unavailable);
        dom.checkUpdate.disabled = true;
        break;
      case "idle":
        setUpdateAnnouncement(STRINGS.idle);
        break;
      case "checking":
        setUpdateAnnouncement(STRINGS.checking);
        dom.checkUpdate.disabled = true;
        break;
      case "current":
        setUpdateAnnouncement(STRINGS.current);
        updateNotes = null;
        renderUpdateNotes(updateNotes);
        break;
      case "available":
        updateNotes = status.notes;
        renderUpdateNotes(updateNotes);
        setUpdateAnnouncement(STRINGS.available(status.version));
        dom.installUpdate.textContent = STRINGS.updateNow;
        dom.installUpdate.hidden = false;
        break;
      case "downloading": {
        const total = status.totalBytes;
        const downloadedMb = (status.downloadedBytes / 1_048_576).toFixed(1);
        setUpdateAnnouncement(STRINGS.downloading(status.version));
        dom.updateProgressDetail.textContent =
          total === null
            ? STRINGS.downloadedMb(downloadedMb)
            : STRINGS.downloadedPercent(
                Math.min(100, Math.round((status.downloadedBytes / total) * 100)),
              );
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
        setUpdateAnnouncement(STRINGS.readyToRelaunch(status.version));
        dom.installUpdate.textContent = STRINGS.relaunch;
        dom.installUpdate.hidden = false;
        dom.checkUpdate.disabled = true;
        break;
      case "error":
        setUpdateAnnouncement(updateErrorMessage(status.kind));
        dom.checkUpdate.textContent = STRINGS.retry;
        break;
    }

    if (status.status !== "error") {
      dom.checkUpdate.textContent = STRINGS.checkForUpdates;
    }
}

function renderUpdateNotes(notes: string | null): void {
  dom.updateNotes.replaceChildren();
  dom.updateNotes.hidden = notes === null;
  if (notes === null) return;

  let cursor = 0;
  for (const match of notes.matchAll(RELEASE_NOTE_URL)) {
    const url = match[0];
    const index = match.index;
    dom.updateNotes.append(document.createTextNode(notes.slice(cursor, index)));

    const link = document.createElement("a");
    link.className = "updates__notes-link";
    link.href = url;
    link.textContent = url;
    link.addEventListener("click", (event) => {
      event.preventDefault();
      void openUrl(url).catch((err) =>
        console.error("could not open the update changelog", err),
      );
    });
    dom.updateNotes.append(link);
    cursor = index + url.length;
  }

  dom.updateNotes.append(document.createTextNode(notes.slice(cursor)));
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
      const status: UpdateStatus = {
        status: "error",
        kind: updateErrorKind(err),
      };
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
      const status: UpdateStatus = {
        status: "error",
        kind: updateErrorKind(err),
      };
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
      ? STRINGS.confirmWindows
      : STRINGS.confirmOther;
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
      const status: UpdateStatus = {
        status: "error",
        kind: updateErrorKind(err),
      };
      renderUpdateStatus(status);
      scheduleUpdateRefresh(status);
    }
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

let booted = false;

/**
 * The dedicated update screen: check, download, install, relaunch. It is the
 * only place any of that happens, so it re-runs a check whenever the tray
 * asks for one, even if the window was already open. Runs once.
 */
export async function bootUpdates(): Promise<void> {
  if (booted) return;
  booted = true;
  showOnly(dom.updates, "app--updates");
  wireUpdateEvents();
  focusUpdateScreen();
  // Opening the screen at all is a request to update, so an unknown intent
  // still checks rather than sitting on a stale "not checked yet".
  const initialUpdateStatus =
    updateIntent === "show" ? refreshUpdateStatus() : runUpdateCheck();
  getVersion()
    .then((version) => {
      dom.updateVersion.textContent = STRINGS.version(version);
    })
    .catch((err) => {
      console.error("could not read app version", err);
      dom.updateVersion.textContent = STRINGS.versionUnknown;
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
