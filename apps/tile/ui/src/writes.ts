// The settings write queue, shared by every window that changes settings.
//
// Each webview has its own copy of this module, so each window queues its own
// writes; the backend's settings transaction serializes the windows against
// each other, and announces every committed write so the other window can
// catch up. Screens plug in through `configureWrites`, which keeps their DOM
// out of this module.

import { listen } from "@tauri-apps/api/event";
import { getCurrentWebview } from "@tauri-apps/api/webview";
import { getConfig } from "./api";
import { settingsErrorMessage } from "./errors";
import { setConfig } from "./state";
import { Config } from "./types";

/** Emitted by the backend after every committed settings write. */
const CONFIG_CHANGED = "tile://config-changed";

interface ConfigChanged {
  config: Config;
  /** Label of the webview that made the change. */
  source: string;
}

interface WriteHooks {
  /** Runs as each write is queued, before it reaches the backend. */
  beforeWrite: () => void;
  /** Re-renders every control from the shared config. */
  render: () => void;
  /** Runs when another window committed a change. */
  changedElsewhere: () => void;
}

const hooks: WriteHooks = {
  beforeWrite: () => {},
  render: () => {},
  changedElsewhere: () => {},
};

/** Plugs the current screen into the queue. Call once, at boot. */
export function configureWrites(screen: Partial<WriteHooks>): void {
  Object.assign(hooks, screen);
}

/**
 * Every settings write runs through here, one at a time and in the order the
 * user made them. Each command replies with the whole saved config, so if two
 * ran at once a slow earlier reply could land after a newer one and put a
 * stale config back on screen. Chaining them means replies arrive in order and
 * the last write is the one `config` ends up holding.
 *
 * Resolves to the saved config, or to `null` when a newer write was queued in
 * the meantime: the caller should then leave the controls alone, because they
 * already show the user's newer choices. Rejects if this write failed;
 * `saveSetting` then reports it and calls `reconcileWhenIdle`.
 */
let writeQueue: Promise<unknown> = Promise.resolve();
let latestWrite = 0;
let pendingWrites = 0;

/** Runs `op` after everything already queued, keeping the queue alive if it fails. */
function enqueue<T>(op: () => Promise<T>): Promise<T> {
  const run = writeQueue.then(op);
  writeQueue = run.catch(() => undefined);
  return run;
}

/** Whether a write from this window is queued or in flight. */
export function hasPendingWrites(): boolean {
  return pendingWrites > 0;
}

function saveConfig(write: () => Promise<Config>): Promise<Config | null> {
  const ticket = ++latestWrite;
  pendingWrites += 1;
  hooks.beforeWrite();
  return enqueue(async () => {
    try {
      const saved = await write();
      setConfig(saved);
      return ticket === latestWrite ? saved : null;
    } finally {
      pendingWrites -= 1;
    }
  });
}

/**
 * Re-reads the settings Tile is actually using and renders them, once no
 * write is left in the queue. A failed write calls this rather than putting
 * its control back on the spot: a newer queued write may not re-render that
 * control on success, and reverting now would trample a choice still on its
 * way. The read is queued too, so it can never land before a write made
 * earlier and hand back an older config.
 */
export async function reconcileWhenIdle(): Promise<void> {
  do {
    await enqueue(async () => {
      try {
        setConfig(await getConfig());
      } catch (err) {
        console.error("could not reload settings", err);
      }
    });
  } while (pendingWrites > 0);
  hooks.render();
}

/** Shows (or, with `null`, clears) the error line beside a control. */
export function setFieldError(target: HTMLElement, text: string | null): void {
  target.textContent = text ?? "";
  target.hidden = text === null;
}

/**
 * Runs a settings command through the write queue and reports a failure
 * beside its control. Resolves to the saved config when it is the latest
 * write, `null` when a newer one was queued meanwhile (the controls already
 * show that newer choice, so leave them), or `false` when it failed. On
 * failure the backend has already left the app on its previous settings, so
 * the controls are re-read from it once the queue drains — never a control
 * showing a value Tile is not actually using.
 */
export async function saveSetting(
  errorTarget: HTMLElement,
  save: () => Promise<Config>,
  message: (err: unknown) => string = settingsErrorMessage,
): Promise<Config | null | false> {
  try {
    const saved = await saveConfig(save);
    setFieldError(errorTarget, null);
    return saved;
  } catch (err) {
    console.error("settings change failed", err);
    setFieldError(errorTarget, message(err));
    await reconcileWhenIdle();
    return false;
  }
}

/**
 * Re-renders when another window commits a change. This window's own writes
 * are skipped: their replies already rendered them, and re-rendering could
 * trample an edit still in progress. The payload's config is not trusted
 * as-is, because announcements from two windows may arrive out of order; the
 * queued re-read always returns the newest truth and only renders once this
 * window's own writes have drained. Reading never writes, so this cannot loop.
 */
export async function followChangesElsewhere(): Promise<void> {
  let self: string;
  try {
    self = getCurrentWebview().label;
  } catch (err) {
    console.error("could not identify this window", err);
    return;
  }
  try {
    await listen<ConfigChanged>(CONFIG_CHANGED, (event) => {
      if (event.payload.source === self) return;
      hooks.changedElsewhere();
      void reconcileWhenIdle();
    });
  } catch (err) {
    console.error("could not listen for settings changes", err);
  }
}
