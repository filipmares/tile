// Settings UI controller. No framework: plain DOM.

import { openUrl } from "@tauri-apps/plugin-opener";
import { getVersion } from "@tauri-apps/api/app";
import { listen } from "@tauri-apps/api/event";
import {
  checkForUpdates,
  getBuildInfo,
  getConfig,
  getHotkeyFailures,
  getPermissionStatus,
  getUpdateStatus,
  getWelcomeStatus,
  installUpdate,
  focusWelcome,
  closeWelcomeWindow,
  openWelcome,
  resetToDefaults,
  setAnimation,
  setAnimationDuration,
  setBinding,
  setCycling,
  setGaps,
  setLaunchOnLogin,
  takeOrientation,
} from "./api";
import { formatHotkey, interpret, isMac } from "./hotkey";
import {
  ACTIONS,
  ActionPerformed,
  BuildInfo,
  Config,
  CYCLE_SIZES,
  CycleSize,
  FAMILIES,
  Gaps,
  Hotkey,
  HotkeyFailure,
  SubsequentExecutionMode,
  UpdateStatus,
  WindowAction,
} from "./types";

const ACCESSIBILITY_URL =
  "x-apple.systempreferences:com.apple.preference.security?Privacy_Accessibility";
const GITHUB_URL = "https://github.com/filipmares/tile";
const isAboutScreen = new URLSearchParams(window.location.search).has("about");
const isWelcomeScreen = new URLSearchParams(window.location.search).has(
  "welcome",
);
const updateIntent = window.sessionStorage.getItem("tile-update-intent");
window.sessionStorage.removeItem("tile-update-intent");

/** Non-null `querySelector`, throwing at boot if the markup is wrong. */
function el<T extends HTMLElement>(selector: string): T {
  const node = document.querySelector<T>(selector);
  if (!node) throw new Error(`missing element: ${selector}`);
  return node;
}

const dom = {
  app: el<HTMLElement>("#app"),
  about: el<HTMLElement>("#about"),
  aboutVersion: el<HTMLParagraphElement>("#about-version"),
  aboutCheckUpdate: el<HTMLButtonElement>("#about-check-update"),
  aboutUpdateStatus: el<HTMLParagraphElement>("#about-update-status"),
  github: el<HTMLButtonElement>("#github"),
  bindings: el<HTMLUListElement>("#bindings"),
  assignedBindings: el<HTMLUListElement>("#assigned-bindings"),
  allShortcutsCount: el<HTMLSpanElement>("#all-shortcuts-count"),
  shortcutFilter: el<HTMLInputElement>("#shortcut-filter"),
  recordingStatus: el<HTMLParagraphElement>("#recording-status"),
  gapWindow: el<HTMLInputElement>("#gap-window"),
  gapWindowNumber: el<HTMLInputElement>("#gap-window-number"),
  gapEdgeTop: el<HTMLInputElement>("#gap-edge-top"),
  gapEdgeBottom: el<HTMLInputElement>("#gap-edge-bottom"),
  gapEdgeLeft: el<HTMLInputElement>("#gap-edge-left"),
  gapEdgeRight: el<HTMLInputElement>("#gap-edge-right"),
  gapSkipTop: el<HTMLInputElement>("#gap-skip-top"),
  gapMainOnly: el<HTMLInputElement>("#gap-main-only"),
  subsequentMode: el<HTMLSelectElement>("#subsequent-mode"),
  cycleSizes: el<HTMLFieldSetElement>("#cycle-sizes"),
  cycleSizesGrid: el<HTMLDivElement>("#cycle-sizes-grid"),
  animate: el<HTMLInputElement>("#animate-moves"),
  animationDuration: el<HTMLInputElement>("#animation-duration"),
  animationDurationNumber: el<HTMLInputElement>("#animation-duration-number"),
  launch: el<HTMLInputElement>("#launch-on-login"),
  reset: el<HTMLButtonElement>("#reset"),
  showWelcome: el<HTMLButtonElement>("#show-welcome"),
  permissionPanel: el<HTMLElement>("#permission-panel"),
  welcome: el<HTMLElement>("#welcome"),
  welcomeHome: el<HTMLSpanElement>("#welcome-home"),
  welcomeStage: el<HTMLDivElement>("#welcome-stage"),
  welcomeScreens: el<HTMLDivElement>("#welcome-screens"),
  welcomeGhost: el<HTMLDivElement>("#welcome-ghost"),
  welcomePane: el<HTMLDivElement>("#welcome-pane"),
  welcomeTrack: el<HTMLDivElement>("#welcome-track"),
  welcomeEnd: el<HTMLDivElement>("#welcome-end"),
  welcomeEndLine: el<HTMLParagraphElement>("#welcome-end-line"),
  welcomeLede: el<HTMLParagraphElement>("#welcome-lede"),
  welcomeEndAside: el<HTMLParagraphElement>("#welcome-end-aside"),
  welcomeDots: el<HTMLDivElement>("#welcome-dots"),
  welcomeSkip: el<HTMLButtonElement>("#welcome-skip"),
  welcomeSkipKey: el<HTMLSpanElement>("#welcome-skip-key"),
  welcomeProgress: el<HTMLParagraphElement>("#welcome-progress"),
  welcomeNote: el<HTMLParagraphElement>("#welcome-note"),
  welcomeActionCount: el<HTMLSpanElement>("#welcome-action-count"),
  welcomeDismiss: el<HTMLButtonElement>("#welcome-dismiss"),
  grant: el<HTMLButtonElement>("#grant-permission"),
  openAccessibility: el<HTMLButtonElement>("#open-accessibility"),
  developmentPanel: el<HTMLElement>("#development-panel"),
  developmentConfigDir: el<HTMLParagraphElement>("#development-config-dir"),
  launchDevelopmentNote: el<HTMLParagraphElement>("#launch-development-note"),
  updatePanel: el<HTMLElement>("#update-panel"),
  updateStatus: el<HTMLParagraphElement>("#update-status"),
  updateNotes: el<HTMLParagraphElement>("#update-notes"),
  updateProgress: el<HTMLProgressElement>("#update-progress"),
  updateProgressDetail: el<HTMLParagraphElement>("#update-progress-detail"),
  checkUpdate: el<HTMLButtonElement>("#check-update"),
  installUpdate: el<HTMLButtonElement>("#install-update"),
  updateConfirmation: el<HTMLDialogElement>("#update-confirmation"),
  updateConfirmationMessage: el<HTMLParagraphElement>(
    "#update-confirmation-message",
  ),
  confirmUpdate: el<HTMLButtonElement>("#confirm-update"),
  cancelUpdate: el<HTMLButtonElement>("#cancel-update"),
};

let config: Config | null = null;
let failures: HotkeyFailure[] = [];
let recording: WindowAction | null = null;
/** Current text in the shortcut filter. Empty means "show everything". */
let shortcutFilter = "";
/** The user's family open/closed state, stashed while a filter is active. */
let openBeforeFilter: Set<string> | null = null;
let permissionTimer: number | null = null;
let updatePollTimer: number | null = null;
let updateState: UpdateStatus = { status: "idle" };
let updateNotes: string | null = null;

/** Actions sharing a hotkey (only possible from a hand-edited config). */
function conflictingActions(cfg: Config): Set<WindowAction> {
  const seen = new Map<string, WindowAction[]>();
  for (const { id } of ACTIONS) {
    const hk = cfg.bindings[id];
    if (!hk) continue;
    const key = `${hk.modifiers}:${hk.key}`;
    const list = seen.get(key) ?? [];
    list.push(id);
    seen.set(key, list);
  }
  const clashing = new Set<WindowAction>();
  for (const list of seen.values()) {
    if (list.length > 1) list.forEach((a) => clashing.add(a));
  }
  return clashing;
}

function failureFor(action: WindowAction): HotkeyFailure | undefined {
  return failures.find((f) => f.action === action);
}

function renderBindings(): void {
  if (!config) return;
  const cfg = config;
  const conflicts = conflictingActions(cfg);
  const assignedActions = ACTIONS.filter(({ id }) => cfg.bindings[id]);
  dom.assignedBindings.replaceChildren();
  dom.allShortcutsCount.textContent = `${ACTIONS.length} shortcuts · ${assignedActions.length} assigned`;

  if (assignedActions.length === 0) {
    const empty = document.createElement("li");
    empty.className = "shortcut-empty";
    empty.textContent = "No shortcuts assigned yet.";
    dom.assignedBindings.append(empty);
  } else {
    for (const action of assignedActions) {
      dom.assignedBindings.append(
        renderBinding(cfg, conflicts, action.id, action.label, "assigned"),
      );
    }
  }

  const filter = shortcutFilter.trim().toLowerCase();
  const matchesFilter = (label: string): boolean =>
    label.toLowerCase().includes(filter);

  const hasRendered = dom.bindings.childElementCount > 0;
  const currentlyOpen = new Set(
    [...dom.bindings.querySelectorAll<HTMLDetailsElement>("details[open]")]
      .map((details) => details.dataset.family)
      .filter((family): family is string => family !== undefined),
  );

  // Filtering force-opens every matching family, which would otherwise
  // overwrite the user's own open/closed state. Stash it on the way in and
  // put it back when the filter clears. The stash has to be read into a local
  // first: clearing it before the read would silently discard it.
  const restore = filter ? null : openBeforeFilter;
  if (filter && openBeforeFilter === null) {
    openBeforeFilter = currentlyOpen;
  } else if (!filter) {
    openBeforeFilter = null;
  }
  const openFamilies = filter ? currentlyOpen : (restore ?? currentlyOpen);

  dom.bindings.replaceChildren();
  let shown = 0;

  for (const family of FAMILIES) {
    const actions = ACTIONS.filter((a) => a.family === family.id);
    if (actions.length === 0) continue;

    const matches = filter ? actions.filter((a) => matchesFilter(a.label)) : actions;
    // A family with nothing to show is noise while filtering.
    if (matches.length === 0) continue;

    const group = document.createElement("li");
    group.className = "binding-group";

    const disclosure = document.createElement("details");
    disclosure.className = "binding-group__disclosure";
    disclosure.dataset.family = family.id;
    // While filtering, every surviving family opens: a match hidden inside a
    // collapsed group is the one thing a filter must never do. The user's own
    // open/closed state is restored as soon as the filter is cleared.
    disclosure.open = filter
      ? true
      : hasRendered
        ? openFamilies.has(family.id)
        : family.id === "halves" || actions.some(({ id }) => cfg.bindings[id]);

    const summary = document.createElement("summary");
    summary.className = "binding-group__summary";

    const heading = document.createElement("span");
    heading.className = "binding-group__title";
    heading.textContent = family.label;

    // Counts describe the family, not the filter. A number that moved while
    // typing would read as a bug rather than as information.
    const assignedCount = actions.filter(({ id }) => cfg.bindings[id]).length;
    const count = document.createElement("span");
    count.className = "binding-group__count";
    count.textContent = `${actions.length} shortcuts · ${assignedCount} assigned`;

    summary.append(heading, count);
    disclosure.append(summary);

    const list = document.createElement("ul");
    list.className = "binding-group__list";

    for (const { id, label } of matches) {
      list.append(renderBinding(cfg, conflicts, id, label, "all"));
    }

    disclosure.append(list);
    group.append(disclosure);
    dom.bindings.append(group);
    shown += matches.length;
  }

  if (filter && shown === 0) {
    const empty = document.createElement("li");
    empty.className = "shortcut-empty";
    empty.textContent = `No shortcuts match \u201c${shortcutFilter.trim()}\u201d.`;
    dom.bindings.append(empty);
  }
}

/* ---------------------------------------------------------------------- *\
 * The welcome deck.
 *
 * Tile cannot be explained faster than it can be tried, so the welcome screen
 * does not describe the shortcuts — it deals them one at a time and waits.
 * The keyboard is the only way forward: the backend reports every action it
 * performs (see `tile://action-performed`), and a slide is only left behind
 * once the shortcut on it really moved a window. The stage above shows where
 * the next press will land, then where the window actually went.
\* ---------------------------------------------------------------------- */

/** A rectangle in work-area fractions: 0..1 of one mini display. */
interface PaneRect {
  x: number;
  y: number;
  w: number;
  h: number;
}

/** Where the mini window sits: which display, and where on it. */
interface PaneFrame extends PaneRect {
  screen: number;
}

/**
 * The unplaced window the stage starts with: off-centre and lifted, the way a
 * window looks before anyone has tidied it.
 */
const FLOATING: PaneFrame = { screen: 0, x: 0.14, y: 0.14, w: 0.56, h: 0.64 };

/**
 * How the stage draws an action, as a function of the size the cycle is
 * currently on. Only actions with an unambiguous shape are here; anything
 * else leaves the pane where it is rather than guessing at it.
 */
const PANE_SHAPES: Partial<Record<WindowAction, (f: number) => PaneRect>> = {
  "left-half": (f) => ({ x: 0, y: 0, w: f, h: 1 }),
  "right-half": (f) => ({ x: 1 - f, y: 0, w: f, h: 1 }),
  "top-half": (f) => ({ x: 0, y: 0, w: 1, h: f }),
  "bottom-half": (f) => ({ x: 0, y: 1 - f, w: 1, h: f }),
  "top-left": (f) => ({ x: 0, y: 0, w: f, h: 0.5 }),
  "top-right": (f) => ({ x: 1 - f, y: 0, w: f, h: 0.5 }),
  "bottom-left": (f) => ({ x: 0, y: 0.5, w: f, h: 0.5 }),
  "bottom-right": (f) => ({ x: 1 - f, y: 0.5, w: f, h: 0.5 }),
  "first-third": () => ({ x: 0, y: 0, w: 1 / 3, h: 1 }),
  "center-third": () => ({ x: 1 / 3, y: 0, w: 1 / 3, h: 1 }),
  "last-third": () => ({ x: 2 / 3, y: 0, w: 1 / 3, h: 1 }),
  "first-two-thirds": () => ({ x: 0, y: 0, w: 2 / 3, h: 1 }),
  "last-two-thirds": () => ({ x: 1 / 3, y: 0, w: 2 / 3, h: 1 }),
  maximize: () => ({ x: 0, y: 0, w: 1, h: 1 }),
  "almost-maximize": () => ({ x: 0.05, y: 0.05, w: 0.9, h: 0.9 }),
  center: () => ({ x: 0.2, y: 0.15, w: 0.6, h: 0.7 }),
};

/** The width (or height) each cycle size takes, as a fraction. */
const CYCLE_FRACTIONS: Record<CycleSize, number> = {
  "one-quarter": 0.25,
  "one-third": 1 / 3,
  "one-half": 0.5,
  "two-thirds": 2 / 3,
  "three-quarters": 0.75,
};

/** How each cycle size is written on the slide that teaches the cycle. */
const CYCLE_GLYPHS: Record<CycleSize, string> = {
  "one-quarter": "\u00bc",
  "one-third": "\u2153",
  "one-half": "\u00bd",
  "two-thirds": "\u2154",
  "three-quarters": "\u00be",
};

/**
 * Deck lengths spelled out. The count is a sentence, not a statistic, and a
 * numeral in a line of plain prose reads as a figure to be checked.
 */
const COUNT_WORDS: Record<number, string> = {
  2: "Two",
  3: "Three",
  4: "Four",
};

/** Actions whose repeat walks the size cycle rather than doing nothing. */const CYCLING_ACTIONS: WindowAction[] = [
  "left-half",
  "right-half",
  "top-half",
  "bottom-half",
  "top-left",
  "top-right",
  "bottom-left",
  "bottom-right",
];

const DISPLAY_ACTIONS: WindowAction[] = ["previous-display", "next-display"];

/** How long the pane and the tick are given before the deck moves on. */
const ADVANCE_DELAY = 900;

/** One slide: a shortcut to try, and what counts as having tried it. */
interface Slide {
  id: "snap-left" | "snap-right" | "cycle" | "maximize";
  /** The instruction: what to do, in the imperative. */
  line: string;
  /** What the press will actually do — the part the keycap cannot say. */
  detail: string;
  combos: string[];
  /** Satisfied by any of these, or by a repeat of one for the cycle slide. */
  actions: WindowAction[];
  needsRepeat: boolean;
  done: boolean;
  card?: HTMLDivElement;
  dot?: HTMLSpanElement;
  /** One per cycle size, on the slide that teaches the cycle. */
  pips?: HTMLSpanElement[];
}

/** Everything the deck needs to remember between key presses. */
const walk = {
  slides: [] as Slide[],
  /** Which slide is showing; `slides.length` is the closing one. */
  at: 0,
  cycleSizes: [] as CycleSize[],
  cycles: false,
  screens: [] as HTMLElement[],
  pane: FLOATING,
  /** The last action performed, for spotting a repeat. */
  lastAction: null as WindowAction | null,
  /** Where in the configured size cycle that action currently sits. */
  sizeIndex: -1,
  /** Which cycle sizes the user has actually been shown, by index. */
  cycleSeen: new Set<number>(),
  /** Set once the user skips: presses still mirror, but nothing advances. */
  skipped: false,
  timer: 0,
  refusal: 0,
  /** Wrong keys in a row. Enough of them turns the exit into a real offer. */
  refusals: 0,
  /** Whether this window has already taken the keyboard. */
  hasKeyboard: false,
};

/** Wrong keys in a row before the deck stops insisting and offers the exit. */
const REFUSALS_BEFORE_EXIT = 3;

/**
 * Builds the slides this machine can actually complete. A shortcut nobody has
 * bound and a size cycle the user switched off are left out rather than taught
 * and then disproved.
 *
 * The order is one idea per slide, each one leaning on the last: left, then
 * right so the mirror image is obvious, then the same key again to show that
 * repeating resizes rather than doing nothing, then the whole screen.
 */
function buildSlides(cfg: Config | null): Slide[] {
  const slides: Slide[] = [];
  const combo = (action: WindowAction): string | null => {
    const hk = cfg?.bindings[action];
    return hk ? formatHotkey(hk) : null;
  };

  const add = (
    id: Slide["id"],
    action: WindowAction,
    line: string,
    detail: string,
    needsRepeat = false,
  ): void => {
    const keys = combo(action);
    if (!keys) return;
    slides.push({
      id,
      line,
      detail,
      combos: [keys],
      actions: [action],
      needsRepeat,
      done: false,
    });
  };

  add(
    "snap-left",
    "left-half",
    "Snap the window left.",
    "It takes the left half of whichever display it is on.",
  );
  add(
    "snap-right",
    "right-half",
    "Now send it right.",
    "The same chord, the other arrow. That is the whole pattern.",
  );
  // Only worth a slide if repeating actually resizes. With cycling off, or a
  // cycle of one size, a second press changes nothing, and a slide the
  // keyboard cannot satisfy would strand the deck.
  if (walk.cycles && walk.cycleSizes.length > 1 && combo("right-half")) {
    add(
      "cycle",
      "right-half",
      "Press it again, and again.",
      // The sizes themselves are named by the pips below, which light up as
      // each one is seen; repeating them here would print the same row twice.
      // What the pips cannot say is why one press is not enough.
      "Every repeat is a new width. This step waits for the whole cycle.",
      true,
    );
  }
  add(
    "maximize",
    "maximize",
    "Fill the screen.",
    isMac()
      ? "The work area, not full-screen: your menu bar stays put."
      : "The work area, not full-screen: your taskbar stays put.",
  );

  return slides;
}

/** Draws the mini displays. More than three would be scenery, not a mirror. */
function renderStage(screenCount: number): void {
  dom.welcomeScreens.replaceChildren();
  walk.screens = [];
  for (let i = 0; i < Math.min(Math.max(screenCount, 1), 3); i += 1) {
    const screen = document.createElement("div");
    screen.className = "stage__screen";
    dom.welcomeScreens.append(screen);
    walk.screens.push(screen);
  }
  // Windows keeps its tray at the bottom of the screen; macOS its menu bar at
  // the top. The pane's work area follows whichever this machine has.
  dom.welcomeStage.classList.toggle("stage--tray-bottom", !isMac());
}

/** The share of a mini display taken by the menu bar or taskbar. */
const STAGE_BAR = 0.1;

/**
 * Positions `el` over the mini display it belongs to, measuring the real boxes
 * the browser laid out so one display and three behave identically.
 */
function placeOnStage(el: HTMLElement, frame: PaneFrame): void {
  const screen = walk.screens[frame.screen] ?? walk.screens[0];
  if (!screen) return;

  const workY =
    screen.offsetTop + (isMac() ? screen.offsetHeight * STAGE_BAR : 0);
  const workH = screen.offsetHeight * (1 - STAGE_BAR);
  const { style } = el;
  style.left = `${screen.offsetLeft + frame.x * screen.offsetWidth}px`;
  style.top = `${workY + frame.y * workH}px`;
  style.width = `${frame.w * screen.offsetWidth}px`;
  style.height = `${frame.h * workH}px`;
}

/** Re-places the pane and the outline, in silence, after a resize. */
function placePane(): void {
  placeOnStage(dom.welcomePane, walk.pane);
  renderGhost();
}

/** Moves the pane to `frame`, then re-aims the outline at what comes next. */
function movePane(frame: PaneFrame, snapped: boolean): void {
  walk.pane = frame;
  dom.welcomePane.classList.toggle("stage__pane--snapped", snapped);
  placeOnStage(dom.welcomePane, frame);
  renderGhost();
}

/**
 * Where the current slide's shortcut would put the window. This is a promise
 * the engine keeps: the same shapes, the same cycle order, the same wrap.
 */
function ghostFrame(): PaneFrame | null {
  const slide = walk.slides[walk.at];
  if (!slide || slide.done) return null;

  const action = slide.actions[0];
  if (!action) return null;
  const shape = PANE_SHAPES[action];
  if (!shape) return null;

  if (slide.id === "cycle") {
    const sizes = walk.cycleSizes;
    if (sizes.length === 0 || walk.sizeIndex < 0) return null;
    // Only a repeat advances the cycle. If anything else moved the window
    // since — including a press this slide turned down — the engine starts the
    // cycle from a half again, and the promise has to say so rather than
    // pointing at a size the next press will not produce.
    if (walk.lastAction !== action) {
      return { screen: walk.pane.screen, ...shape(0.5) };
    }
    const next = sizes[(walk.sizeIndex + 1) % sizes.length];
    if (!next) return null;
    return { screen: walk.pane.screen, ...shape(CYCLE_FRACTIONS[next]) };
  }

  // A first press is always a half, whatever the cycle is configured to do.
  return { screen: walk.pane.screen, ...shape(0.5) };
}

/** Draws — or hides — the outline showing where the next press will land. */
function renderGhost(): void {
  const frame = ghostFrame();
  dom.welcomeGhost.hidden = frame === null;
  if (frame) placeOnStage(dom.welcomeGhost, frame);
}

/**
 * Mirrors `action` on the stage, following the same size cycle the engine
 * walks so a second press shows the size the real window actually took.
 *
 * `place` is false for a press the current slide did not ask for. The
 * bookkeeping still runs either way — the engine's cycle state moved whether
 * or not the walkthrough wanted the press, and the promise drawn for the next
 * one has to be built on what the engine now believes — but the pane stays
 * where the lesson left it instead of following the window off course.
 */
function reflectOnStage(action: WindowAction, place: boolean): void {
  const repeat = action === walk.lastAction;
  const cycles = walk.cycles && CYCLING_ACTIONS.includes(action);

  if (DISPLAY_ACTIONS.includes(action)) {
    const count = walk.screens.length;
    const step = action === "next-display" ? 1 : count - 1;
    const thrown = { ...walk.pane, screen: (walk.pane.screen + step) % count };
    const snapped = walk.pane !== FLOATING;
    walk.lastAction = action;
    if (place) movePane(thrown, snapped);
    else renderGhost();
    return;
  }

  if (repeat && cycles) {
    walk.sizeIndex = (walk.sizeIndex + 1) % walk.cycleSizes.length;
  } else {
    // A first press is always a half, which is where the cycle starts.
    walk.sizeIndex = walk.cycleSizes.indexOf("one-half");
  }
  const size = walk.cycleSizes[walk.sizeIndex];
  const fraction = repeat && cycles && size ? CYCLE_FRACTIONS[size] : 0.5;

  walk.lastAction = action;
  // Only the cycle slide's own key counts towards its progress. Walking the
  // cycle on the other side proves the same thing, but this slide asks for a
  // specific key and the pips must answer that question, not a neighbouring
  // one — and never a press the slide turned down.
  const teaches = walk.slides.find((s) => s.id === "cycle");
  if (place && teaches?.actions.includes(action) && walk.sizeIndex >= 0) {
    walk.cycleSeen.add(walk.sizeIndex);
  }
  if (!place) {
    renderGhost();
    return;
  }
  if (action === "restore") {
    movePane(FLOATING, false);
    return;
  }
  const shape = PANE_SHAPES[action];
  if (shape) movePane({ screen: walk.pane.screen, ...shape(fraction) }, true);
}

/** How long the pane spends turning a press down. */
const REFUSAL_MS = 340;

/**
 * Turns down a press the current slide did not ask for.
 *
 * The pane refuses to move and shakes where it stands, so the deck reads as
 * having a mind of its own rather than as being broken. The stage is
 * `aria-hidden`, so the shake alone would say nothing to a screen reader —
 * the note repeats the refusal in words, and makes it the instruction rather
 * than a scolding.
 */
function refusePress(slide: Slide): void {
  const pane = dom.welcomePane;
  pane.classList.remove("stage__pane--refused");
  // Restart the animation rather than let a second wrong press land in the
  // middle of the first one's shake, where it would look like nothing
  // happened at all.
  void pane.offsetWidth;
  pane.classList.add("stage__pane--refused");
  window.clearTimeout(walk.refusal);
  walk.refusal = window.setTimeout(
    () => pane.classList.remove("stage__pane--refused"),
    REFUSAL_MS,
  );

  const combo = slide.combos[0];
  if (combo) setWalkNote(`Try ${combo}.`);

  // Three wrong keys in a row is not a user who needs the instruction repeated
  // a fourth time — it is a user who wants out and has no way to say so. Skip
  // is a mouse target only because this window deliberately holds no keyboard,
  // and that reason has just failed on its own terms: the shortcuts are not
  // landing here anyway. So the deck stops insisting, takes the keyboard, and
  // puts the exit under the very key someone stuck would already be reaching
  // for. It offers the exit; it does not take it.
  walk.refusals += 1;
  if (walk.refusals >= REFUSALS_BEFORE_EXIT && !walk.hasKeyboard) {
    void claimKeyboard(dom.welcomeSkip);
    dom.welcomeSkipKey.hidden = false;
    setWalkNote(combo ? `Try ${combo}, or press Esc to skip.` : null);
  }
}

/** Builds the deck: one card per slide, ahead of the closing one. */
function renderDeck(): void {
  dom.welcomeDots.replaceChildren();

  for (const slide of walk.slides) {
    const card = document.createElement("div");
    card.className = "slide";
    card.dataset.state = "waiting";

    const keys = document.createElement("p");
    keys.className = "slide__keys";
    slide.combos.forEach((text, i) => {
      if (i > 0) {
        const or = document.createElement("span");
        or.className = "slide__or";
        or.textContent = "or";
        keys.append(or);
      }
      const kbd = document.createElement("kbd");
      kbd.className = "slide__key";
      kbd.textContent = text;
      keys.append(kbd);
    });

    const line = document.createElement("p");
    line.className = "slide__line";
    line.textContent = slide.line;

    // Instruction, then the keys, then what they will do. The eye needs to know
    // what it is being asked before the chord means anything, and what the
    // chord produces only after it has read the chord.
    card.append(line, keys);

    const detail = document.createElement("p");
    detail.className = "slide__detail";
    detail.textContent = slide.detail;
    card.append(detail);

    // The cycle is the one slide whose end is not obvious from the key: the
    // same press keeps working, so without the sizes laid out the user cannot
    // tell whether they are halfway or finished. Showing the actual fractions
    // rather than blank pips teaches the cycle at the same time.
    if (slide.id === "cycle") {
      const sizes = document.createElement("p");
      sizes.className = "slide__sizes";
      slide.pips = walk.cycleSizes.map((size) => {
        const pip = document.createElement("span");
        pip.className = "slide__size";
        pip.textContent = CYCLE_GLYPHS[size];
        pip.dataset.state = "ahead";
        sizes.append(pip);
        return pip;
      });
      card.append(sizes);
    }

    dom.welcomeTrack.insertBefore(card, dom.welcomeEnd);
    slide.card = card;

    const dot = document.createElement("span");
    dot.className = "dots__dot";
    dom.welcomeDots.append(dot);
    slide.dot = dot;
  }

  if (walk.slides.length === 0) {
    // Nothing to try means nothing to promise. The closing slide says so and
    // sends the user to the one screen that can fix it.
    dom.welcomeEndLine.textContent = "Tile is waiting for keys.";
    dom.welcomeEndAside.textContent =
      `None of the default shortcuts are bound. Assign your own in Settings, from the ${isMac() ? "menu bar" : "system tray"}.`;
    dom.welcomeSkip.hidden = true;
    dom.welcomeLede.textContent = "";
  } else {
    // Counted from the slides that were actually built, never from the four
    // this deck usually has: a machine missing a binding gets a shorter deck,
    // and a promise of four steps it is not going to deliver would be the one
    // dishonest line on the screen.
    dom.welcomeLede.textContent =
      walk.slides.length === 1
        ? "One shortcut, pressed for real — it moves the window behind this card."
        : `${COUNT_WORDS[walk.slides.length] ?? walk.slides.length} shortcuts, pressed for real — each one moves the window behind this card.`;
  }
  showSlide(0);
}

/** Whether the user has now been shown every size in the cycle. */
function cycleComplete(): boolean {
  return (
    walk.cycleSizes.length > 0 && walk.cycleSeen.size >= walk.cycleSizes.length
  );
}

/** Marks each size on the cycle slide as seen, current, or still to come. */
function renderCyclePips(): void {
  const slide = walk.slides.find((s) => s.id === "cycle");
  if (!slide?.pips) return;
  slide.pips.forEach((pip, i) => {
    pip.dataset.state =
      i === walk.sizeIndex ? "at" : walk.cycleSeen.has(i) ? "done" : "ahead";
  });
}

/**
 * Hands the keyboard to this window for the closing slide.
 *
 * The deck spends four slides refusing focus on purpose: whatever the user was
 * last in has to stay the thing Tile moves, or the proof lands somewhere they
 * cannot see. That reason expires exactly here. There is no shortcut left to
 * demonstrate and nothing left to move, so being frontmost costs nothing — and
 * it buys the last step the same keyboard the other four were taught with.
 * Return finishing the walkthrough is the deck keeping its own promise.
 */
async function claimKeyboard(target: HTMLElement): Promise<void> {
  try {
    await focusWelcome();
  } catch (err) {
    console.error("could not focus the welcome window", err);
  }
  walk.hasKeyboard = true;
  // After the window has the keyboard, not before: a focus ring drawn in a
  // window that is not frontmost points at a control no key can reach.
  //
  // `preventScroll` because this button is off-screen inside the track at the
  // moment it is focused, and the browser's reflex is to scroll its container
  // until it is visible. The deck is scrolled by transform, never by scroll
  // offset, so that help arrives as the closing slide sliding away under its
  // own animation.
  target.focus({ preventScroll: true });
}

/** Closes the welcome window. The walkthrough is over, however it ended. */
function closeWelcome(): void {
  void closeWelcomeWindow().catch((err) =>
    console.error("could not close the welcome window", err),
  );
}

/** Moves the deck to `index` and reflects it in the dots and the outline. */
function showSlide(index: number): void {
  walk.at = Math.min(Math.max(index, 0), walk.slides.length);
  dom.welcomeTrack.style.transform = `translateX(${walk.at * -100}%)`;

  const last = walk.at === walk.slides.length;
  walk.slides.forEach((slide, i) => {
    if (slide.card) slide.card.setAttribute("aria-hidden", String(i !== walk.at));
    if (slide.dot) {
      slide.dot.dataset.state = slide.done
        ? "done"
        : i === walk.at
          ? "at"
          : "ahead";
    }
  });
  renderCyclePips();
  // The lede frames the deck — "each one moves the window behind this card" —
  // and on the closing slide there is no next press for it to be about. It goes
  // quiet rather than away: the composition is vertically centred, so removing
  // a line would lift everything under it by that line's own height.
  dom.welcomeLede.classList.toggle("welcome__lede--spent", last);
  dom.welcomeEnd.setAttribute("aria-hidden", String(!last));
  dom.welcomeSkip.hidden = last || walk.slides.length === 0;
  if (last) void claimKeyboard(dom.welcomeDismiss);

  dom.welcomeProgress.textContent =
    walk.slides.length === 0
      ? ""
      : last
        ? "Done"
        : `Step ${walk.at + 1} of ${walk.slides.length}`;
  renderGhost();
}

/**
 * Shows, replaces or clears the line under the deck. The element is never
 * hidden — its blank line is part of the layout, so speaking and falling
 * silent cost nothing above or below it.
 */
function setWalkNote(text: string | null): void {
  dom.welcomeNote.textContent = text ?? "";
}

/** Leaves the deck where it is: the user asked to stop being taught. */
function skipDeck(): void {
  walk.skipped = true;
  window.clearTimeout(walk.timer);
  setWalkNote(null);
  showSlide(walk.slides.length);
}

/**
 * Handles one performed action: mirror it, tick off whatever it completed,
 * and say something useful when it did nothing.
 */
function onActionPerformed(event: ActionPerformed): void {
  // An empty desk is not a failed press. The key still reached Tile, which is
  // the half that actually goes wrong on a first run — the permission and the
  // registration. Dead-ending the deck there would strand exactly the machine
  // this screen matters most on, so mirror the move the action *would* have
  // made and carry on, while saying plainly that it was a preview rather than
  // a report. The note outlives the slide on purpose: it stays true until a
  // real window moves, and clears itself the moment one does.
  const empty = !event.hadWindow;

  // Off the deck — skipped, or on the closing slide — there is no lesson left
  // to follow, so every press is simply mirrored and none can be wrong.
  const showing = walk.skipped ? undefined : walk.slides[walk.at];

  // A press the current slide did not ask for may still be a slide's own key,
  // pressed early. Someone who already knows Tile should not be refused for
  // proving it in a different order than the deck happened to choose, so a key
  // is credited to whichever unfinished slide teaches it. The cycle is left
  // out: it is a lesson about pressing one key repeatedly, and a single press
  // of it out of order has not shown that.
  const current =
    showing && !showing.actions.includes(event.action)
      ? walk.slides.find(
          (slide) =>
            !slide.done &&
            !slide.needsRepeat &&
            slide.id !== "cycle" &&
            slide.actions.includes(event.action),
        )
      : showing;

  // A wrong key is wrong whether or not it moved anything. Answering it before
  // the moved-check matters on the maximize slide, where the window is often
  // already where the wrong key would put it: without this, the one press most
  // likely to be a mistake is the one press that gets no answer at all.
  if (showing && !current) {
    reflectOnStage(event.action, false);
    refusePress(showing);
    return;
  }

  // A right key that moved nothing is still a right key. `moved: false` with a
  // window present is a no-op, which means the window was already exactly where
  // the slide asked it to go — Tile agreeing with the user rather than failing
  // them. Crediting only movement stranded the deck whenever someone's window
  // happened to start in the position being taught, and on the first slide that
  // is a walkthrough which cannot be finished at all.
  walk.refusals = 0;
  setWalkNote(empty ? "No window open to move — so that was a preview." : null);

  const repeat = event.action === walk.lastAction;
  reflectOnStage(event.action, true);
  renderCyclePips();
  if (!current) return;

  // The slide asked for this key, but not every press of it finishes the
  // slide: the cycle slide is the whole cycle, not one repeat of it, so until
  // every size has been shown the press counts on the stage and on the pips
  // and leaves the slide standing.
  if (current.needsRepeat && !repeat) return;
  if (current.id === "cycle" && !cycleComplete()) return;
  current.done = true;

  if (current.card) current.card.dataset.state = "done";
  if (current.dot) current.dot.dataset.state = "done";
  renderGhost();

  // Hold long enough for the pane to arrive and the keys to light up. Being
  // dealt the next slide mid-animation would read as a glitch, not a reward.
  // Where it lands is the first slide still standing, which is the next one in
  // the ordinary case and skips over anything already earned out of order.
  const next = walk.slides.findIndex((slide) => !slide.done);
  window.clearTimeout(walk.timer);
  walk.timer = window.setTimeout(
    () => showSlide(next === -1 ? walk.slides.length : next),
    ADVANCE_DELAY,
  );
}

function renderBinding(
  cfg: Config,
  conflicts: Set<WindowAction>,
  id: WindowAction,
  label: string,
  scope: "assigned" | "all",
): HTMLLIElement {
  const hk = cfg.bindings[id] ?? null;

  const li = document.createElement("li");
  li.className = "binding";

  const name = document.createElement("span");
  name.className = "binding__label";
  name.textContent = label;
  name.id = `label-${scope}-${id}`;

  const controls = document.createElement("div");
  controls.className = "binding__controls";

  const record = document.createElement("button");
  record.type = "button";
  record.className = "binding__key";
  record.setAttribute("aria-labelledby", `label-${scope}-${id} key-${scope}-${id}`);
  record.id = `key-${scope}-${id}`;
  if (recording === id) {
    record.classList.add("binding__key--recording");
    record.textContent = "Press keys…";
  } else {
    record.textContent = hk ? formatHotkey(hk) : "Unbound";
    if (!hk) record.classList.add("binding__key--empty");
  }
  record.addEventListener("click", () => startRecording(id));
  controls.append(record);

  if (hk && recording !== id) {
    const clear = document.createElement("button");
    clear.type = "button";
    clear.className = "binding__clear";
    clear.setAttribute("aria-label", `Clear shortcut for ${label}`);
    clear.textContent = "✕";
    clear.addEventListener("click", () => void applyBinding(id, null));
    controls.append(clear);
  }

  li.append(name, controls);

  const failure = failureFor(id);
  if (conflicts.has(id)) {
    li.append(note("This shortcut is used by more than one action.", "error"));
  } else if (failure) {
    li.append(note(`The system rejected this shortcut: ${failure.reason}`, "error"));
  }

  return li;
}

function note(text: string, kind: "error" | "info"): HTMLElement {
  const p = document.createElement("p");
  p.className = `binding__note binding__note--${kind}`;
  p.textContent = text;
  return p;
}

function setRecordingStatus(text: string): void {
  dom.recordingStatus.textContent = text;
}

function startRecording(action: WindowAction): void {
  recording = action;
  const label = ACTIONS.find((a) => a.id === action)?.label ?? action;
  setRecordingStatus(
    `Recording ${label}. Press a shortcut, Esc to cancel, Backspace to clear.`,
  );
  renderBindings();
  window.addEventListener("keydown", onRecordKey, { capture: true });
}

function stopRecording(): void {
  recording = null;
  window.removeEventListener("keydown", onRecordKey, { capture: true });
  renderBindings();
}

function onRecordKey(e: KeyboardEvent): void {
  if (recording === null) return;
  e.preventDefault();
  e.stopPropagation();

  const outcome = interpret(e);
  switch (outcome.kind) {
    case "pending":
      return;
    case "cancel":
      setRecordingStatus("Recording cancelled.");
      stopRecording();
      return;
    case "error":
      setRecordingStatus(outcome.message);
      return;
    case "clear": {
      const action = recording;
      stopRecording();
      setRecordingStatus("Shortcut cleared.");
      void applyBinding(action, null);
      return;
    }
    case "bound": {
      const action = recording;
      stopRecording();
      setRecordingStatus("");
      void applyBinding(action, outcome.hotkey);
      return;
    }
  }
}

async function applyBinding(
  action: WindowAction,
  hotkey: Hotkey | null,
): Promise<void> {
  try {
    config = await setBinding(action, hotkey);
    failures = await getHotkeyFailures();
    renderBindings();
  } catch (err) {
    setRecordingStatus(`Could not save shortcut: ${String(err)}`);
  }
}

function renderBehaviour(): void {
  if (!config) return;
  const g = config.gap;
  dom.gapWindow.value = String(g.window);
  dom.gapWindowNumber.value = String(g.window);
  dom.gapEdgeTop.value = String(g.edgeTop);
  dom.gapEdgeBottom.value = String(g.edgeBottom);
  dom.gapEdgeLeft.value = String(g.edgeLeft);
  dom.gapEdgeRight.value = String(g.edgeRight);
  dom.gapSkipTop.checked = g.skipTopEdge;
  dom.gapMainOnly.checked = g.mainScreenOnly;
  dom.subsequentMode.value = config.subsequentExecutionMode;
  renderCycleSizes(config);
  dom.animate.checked = config.animation.enabled;
  mirrorAnimationDuration(String(config.animation.durationMs));
  setAnimationDurationEnabled(config.animation.enabled);
  dom.launch.checked = config.launchOnLogin;
}

/** Marks the window as a development build. Installed builds render nothing. */
function renderBuildInfo(info: BuildInfo): void {
  if (info.kind !== "development") return;
  dom.developmentPanel.hidden = false;
  // The launch-on-login toggle is the one control whose behaviour differs, so
  // it says so where it is, not only in the panel at the top.
  dom.launchDevelopmentNote.hidden = false;
  if (info.configDir) {
    dom.developmentConfigDir.textContent = `Settings are stored in ${info.configDir}`;
    dom.developmentConfigDir.hidden = false;
  }
}

function renderUpdateStatus(status: UpdateStatus): void {
    updateState = status;
    if (status.status !== "available" && dom.updateConfirmation.open) {
      dom.updateConfirmation.close();
    }
    dom.updatePanel.hidden = false;
    dom.updateProgress.hidden = true;
    dom.updateProgressDetail.hidden = true;
    dom.updateProgress.removeAttribute("value");
    dom.installUpdate.hidden = true;
    dom.checkUpdate.disabled = false;
    dom.updateNotes.hidden = updateNotes === null;
    dom.updateNotes.textContent = updateNotes ?? "";

    switch (status.status) {
      case "unavailable":
        setUpdateAnnouncement(
          "Production update checks are unavailable in this development build.",
        );
        dom.checkUpdate.disabled = true;
        break;
      case "idle":
        setUpdateAnnouncement("Tile has not checked for updates yet.");
        break;
      case "checking":
        setUpdateAnnouncement("Checking for updates…");
        dom.checkUpdate.disabled = true;
        break;
      case "current":
        setUpdateAnnouncement("Tile is up to date.");
        updateNotes = null;
        dom.updateNotes.hidden = true;
        break;
      case "available":
        updateNotes = status.notes;
        dom.updateNotes.textContent = updateNotes ?? "";
        dom.updateNotes.hidden = updateNotes === null;
        setUpdateAnnouncement(`Tile ${status.version} is available.`);
        dom.installUpdate.textContent = "Update now";
        dom.installUpdate.hidden = false;
        break;
      case "downloading": {
        const total = status.totalBytes;
        const downloadedMb = (status.downloadedBytes / 1_048_576).toFixed(1);
        setUpdateAnnouncement(`Downloading Tile ${status.version}.`);
        dom.updateProgressDetail.textContent =
          total === null
            ? `${downloadedMb} MB downloaded`
            : `${Math.min(100, Math.round((status.downloadedBytes / total) * 100))}% downloaded`;
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
        setUpdateAnnouncement(
          `Tile ${status.version} is installed and ready to relaunch.`,
        );
        dom.installUpdate.textContent = "Relaunch Tile";
        dom.installUpdate.hidden = false;
        dom.checkUpdate.disabled = true;
        break;
      case "error":
        setUpdateAnnouncement(`Update error: ${status.message}`);
        dom.checkUpdate.textContent = "Retry";
        break;
    }

    if (status.status !== "error") {
      dom.checkUpdate.textContent = "Check for updates";
    }
}

function setUpdateAnnouncement(text: string): void {
  if (dom.updateStatus.textContent !== text) {
    dom.updateStatus.textContent = text;
  }
}

function describeAboutUpdateStatus(status: UpdateStatus): string {
  switch (status.status) {
    case "unavailable":
      return "Update checks are unavailable in this development build.";
    case "idle":
      return "Tile has not checked for updates yet.";
    case "checking":
      return "Tile is already checking for updates.";
    case "current":
      return "Tile is up to date.";
    case "available":
      return `Tile ${status.version} is available. Open Settings or the tray menu to update.`;
    case "downloading":
      return `Tile ${status.version} is downloading.`;
    case "ready-to-relaunch":
      return `Tile ${status.version} is installed and ready to relaunch.`;
    case "error":
      return `Could not check for updates: ${status.message}`;
  }
}

  async function refreshUpdateStatus(): Promise<UpdateStatus> {
    try {
      const status = await getUpdateStatus();
      renderUpdateStatus(status);
      return status;
    } catch (err) {
      const status: UpdateStatus = { status: "error", message: String(err) };
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
      const status: UpdateStatus = { status: "error", message: String(err) };
      renderUpdateStatus(status);
      scheduleUpdateRefresh(status);
      return status;
    }
  }

  function focusUpdatePanel(): void {
    dom.updatePanel.hidden = false;
    dom.updatePanel.scrollIntoView({ behavior: "smooth", block: "start" });
    dom.updatePanel.focus({ preventScroll: true });
  }

  function showUpdateConfirmation(): void {
    if (updateState.status !== "available") return;
    const windows = navigator.userAgent.includes("Windows");
    dom.updateConfirmationMessage.textContent = windows
      ? "Tile will close, install the update, and reopen automatically. Continue?"
      : "Tile will install the update. You can relaunch after it finishes. Continue?";
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
      const status: UpdateStatus = { status: "error", message: String(err) };
      renderUpdateStatus(status);
      scheduleUpdateRefresh(status);
    }
  }

/** Builds the cycle-size checkboxes once, then reflects the saved selection. */
function renderCycleSizes(cfg: Config): void {
  if (dom.cycleSizesGrid.childElementCount === 0) {
    for (const size of CYCLE_SIZES) {
      const label = document.createElement("label");
      label.className = "field__sub";
      label.htmlFor = `cycle-size-${size.id}`;
      label.textContent = size.label;

      const input = document.createElement("input");
      input.type = "checkbox";
      input.id = `cycle-size-${size.id}`;
      input.dataset.size = size.id;
      input.addEventListener("change", () => void commitCycling());

      dom.cycleSizesGrid.append(label, input);
    }
  }

  const selected = new Set(cfg.cycleSizes);
  for (const input of cycleSizeInputs()) {
    input.checked = selected.has(input.dataset.size as CycleSize);
  }
  // With cycling switched off the sizes have no effect, so say so rather than
  // leaving controls that silently do nothing.
  dom.cycleSizes.disabled = cfg.subsequentExecutionMode !== "cycle-sizes";
}

function cycleSizeInputs(): HTMLInputElement[] {
  return [...dom.cycleSizesGrid.querySelectorAll<HTMLInputElement>("input")];
}

async function commitCycling(): Promise<void> {
  const mode = dom.subsequentMode.value as SubsequentExecutionMode;
  const sizes = cycleSizeInputs()
    .filter((input) => input.checked)
    .map((input) => input.dataset.size as CycleSize);
  try {
    config = await setCycling(mode, sizes);
    renderBehaviour();
  } catch (err) {
    setRecordingStatus(`Could not save cycling settings: ${String(err)}`);
  }
}

function clampGap(value: number): number {
  if (!Number.isFinite(value)) return 0;
  return Math.min(200, Math.max(0, Math.round(value)));
}

/** Reads the current gap controls into a `Gaps` payload. */
function readGaps(): Gaps {
  return {
    window: clampGap(Number(dom.gapWindow.value)),
    edgeTop: clampGap(Number(dom.gapEdgeTop.value)),
    edgeBottom: clampGap(Number(dom.gapEdgeBottom.value)),
    edgeLeft: clampGap(Number(dom.gapEdgeLeft.value)),
    edgeRight: clampGap(Number(dom.gapEdgeRight.value)),
    skipTopEdge: dom.gapSkipTop.checked,
    mainScreenOnly: dom.gapMainOnly.checked,
  };
}

async function commitGaps(): Promise<void> {
  const gaps = readGaps();
  try {
    config = await setGaps(gaps);
    renderBehaviour();
  } catch (err) {
    setRecordingStatus(`Could not save gaps: ${String(err)}`);
  }
}

/** Keeps the window-gap slider and its number field in sync while dragging. */
function mirrorWindowGap(raw: string): void {
  const gap = clampGap(Number(raw));
  dom.gapWindow.value = String(gap);
  dom.gapWindowNumber.value = String(gap);
}

/**
 * Clamps a typed or dragged duration into the range the core crate enforces on
 * save. These bounds match `MIN_ANIMATION_DURATION_MS` and
 * `MAX_ANIMATION_DURATION_MS`, but this is presentation only: `normalize`
 * clamps again on the way to disk and remains the real guard.
 *
 * An empty or unparseable field falls back to the saved value rather than the
 * minimum, so clearing the box and tabbing away restores what was there
 * instead of silently snapping to 40 ms.
 */
function clampAnimationDuration(raw: string): number {
  const parsed = Number(raw.trim());
  if (raw.trim() === "" || !Number.isFinite(parsed)) {
    return config?.animation.durationMs ?? 220;
  }
  return Math.round(Math.min(1000, Math.max(40, parsed)));
}

/** Puts a settled duration into both controls. */
function mirrorAnimationDuration(raw: string): void {
  const ms = String(clampAnimationDuration(raw));
  dom.animationDuration.value = ms;
  dom.animationDurationNumber.value = ms;
}

/** Duration is meaningless while animation is off, so it follows the toggle. */
function setAnimationDurationEnabled(enabled: boolean): void {
  dom.animationDuration.disabled = !enabled;
  dom.animationDurationNumber.disabled = !enabled;
}

async function commitAnimationDuration(): Promise<void> {
  try {
    config = await setAnimationDuration(Number(dom.animationDuration.value));
    mirrorAnimationDuration(String(config.animation.durationMs));
  } catch (err) {
    setRecordingStatus(`Could not update animation duration: ${String(err)}`);
    if (config) mirrorAnimationDuration(String(config.animation.durationMs));
  }
}

/** Refreshes the permission panel, polling while permission is denied. */
async function refreshPermission(prompt: boolean): Promise<void> {
  let status;
  try {
    status = await getPermissionStatus(prompt);
  } catch (err) {
    // An unreadable status is not a denial: the Rust side applies hotkeys
    // anyway in that case, so the panel stays quiet rather than accusing.
    console.error("permission check failed", err);
    return;
  }

  const denied = status === "denied";
  dom.permissionPanel.hidden = !denied;

  if (denied && permissionTimer === null) {
    permissionTimer = window.setInterval(() => void refreshPermission(false), 2000);
  } else if (!denied && permissionTimer !== null) {
    window.clearInterval(permissionTimer);
    permissionTimer = null;
    // Permission just became available: surface any late hotkey failures.
    failures = await getHotkeyFailures();
    renderBindings();
  }
}

function wireEvents(): void {
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
  dom.gapWindow.addEventListener("input", () =>
    mirrorWindowGap(dom.gapWindow.value),
  );
  dom.gapWindow.addEventListener("change", () => void commitGaps());
  dom.gapWindowNumber.addEventListener("input", () =>
    mirrorWindowGap(dom.gapWindowNumber.value),
  );
  dom.gapWindowNumber.addEventListener("change", () => void commitGaps());
  for (const input of [
    dom.gapEdgeTop,
    dom.gapEdgeBottom,
    dom.gapEdgeLeft,
    dom.gapEdgeRight,
  ]) {
    input.addEventListener("change", () => void commitGaps());
  }
  dom.gapSkipTop.addEventListener("change", () => void commitGaps());
  dom.gapMainOnly.addEventListener("change", () => void commitGaps());
  dom.subsequentMode.addEventListener("change", () => void commitCycling());

  dom.animate.addEventListener("change", async () => {
    try {
      config = await setAnimation(dom.animate.checked);
      setAnimationDurationEnabled(config.animation.enabled);
    } catch (err) {
      setRecordingStatus(`Could not update animation: ${String(err)}`);
      // Put the checkbox back where the saved config says it is, so it never
      // shows a state the app is not actually in.
      dom.animate.checked = config?.animation.enabled ?? true;
    }
  });

  // Dragging the slider only ever produces an in-range value, so both controls
  // can track it live.
  dom.animationDuration.addEventListener("input", () =>
    mirrorAnimationDuration(dom.animationDuration.value),
  );
  dom.animationDuration.addEventListener(
    "change",
    () => void commitAnimationDuration(),
  );

  // Typing is different. Rewriting the field on every keystroke would turn "2"
  // into "40" before the user could finish typing "200", so while the edit is
  // in progress only the slider follows along. The field itself is normalized
  // once the edit is committed on blur or Enter.
  dom.animationDurationNumber.addEventListener("input", () => {
    dom.animationDuration.value = String(
      clampAnimationDuration(dom.animationDurationNumber.value),
    );
  });
  dom.animationDurationNumber.addEventListener("change", () => {
    mirrorAnimationDuration(dom.animationDurationNumber.value);
    void commitAnimationDuration();
  });

  dom.launch.addEventListener("change", async () => {
    try {
      config = await setLaunchOnLogin(dom.launch.checked);
    } catch (err) {
      setRecordingStatus(`Could not update launch-on-login: ${String(err)}`);
      dom.launch.checked = config?.launchOnLogin ?? false;
    }
  });

  dom.shortcutFilter.addEventListener("input", () => {
    shortcutFilter = dom.shortcutFilter.value;
    renderBindings();
  });

  dom.reset.addEventListener("click", async () => {
    try {
      config = await resetToDefaults();
      failures = await getHotkeyFailures();
      renderBindings();
      renderBehaviour();
      setRecordingStatus("Defaults restored.");
    } catch (err) {
      setRecordingStatus(`Could not restore defaults: ${String(err)}`);
    }
  });

  dom.grant.addEventListener("click", () => void refreshPermission(true));
  dom.showWelcome.addEventListener("click", () => {
    void openWelcome().catch((err) =>
      console.error("could not open the welcome window", err),
    );
  });
  dom.openAccessibility.addEventListener("click", async () => {
    try {
      await openUrl(ACCESSIBILITY_URL);
    } catch (err) {
      console.error("could not open Accessibility settings", err);
    }
  });
}

/**
 * Boots the welcome screen: its own window, and the only place Tile teaches
 * its defaults. Settings carries the controls and links back here.
 *
 * The order matters. Everything that can dead-end the screen — the buttons,
 * the event subscription — is wired before the first `await`, so a slow or
 * failing backend leaves a screen that is merely quiet rather than broken.
 */
async function bootWelcome(): Promise<void> {
  dom.app.classList.add("app--welcome");
  for (const child of dom.app.children) {
    if (child !== dom.welcome) (child as HTMLElement).hidden = true;
  }
  dom.welcome.hidden = false;
  // Windows puts the icon in the system tray, macOS in the menu bar. This is
  // onboarding copy, so naming the wrong one sends the user hunting.
  dom.welcomeHome.textContent = isMac() ? "menu bar" : "system tray";
  dom.welcomeActionCount.textContent = String(ACTIONS.length);

  dom.welcomeDismiss.addEventListener("click", closeWelcome);
  dom.welcomeSkip.addEventListener("click", skipDeck);

  // Escape reaches this window only when it holds the keyboard, which happens
  // in exactly two places: the closing slide, and the moment the deck gives up
  // insisting after repeated wrong keys. Each has its own exit — one is done,
  // the other is leaving early — so the same key means the nearest true thing
  // rather than one of them dressed as the other. Return is already handled:
  // the button is a button, and it is focused when it matters.
  window.addEventListener("keydown", (event) => {
    if (event.key !== "Escape" || event.defaultPrevented) return;
    event.preventDefault();
    if (walk.at < walk.slides.length && !walk.skipped) skipDeck();
    else closeWelcome();
  });

  // A resized window relays out the mini displays under a pane that is
  // positioned in pixels, so re-measure — without animating a move the user
  // did not make.
  const observer = new ResizeObserver(() => {
    dom.welcomeStage.classList.add("stage--measuring");
    placePane();
    requestAnimationFrame(() =>
      dom.welcomeStage.classList.remove("stage--measuring"),
    );
  });
  observer.observe(dom.welcomeStage);

  void listen<ActionPerformed>("tile://action-performed", (event) =>
    onActionPerformed(event.payload),
  ).catch((err) =>
    console.error("could not listen for performed actions", err),
  );

  let cfg: Config | null = null;
  try {
    cfg = await getConfig();
  } catch (err) {
    console.error("could not load settings for the welcome screen", err);
  }
  walk.cycleSizes = cfg?.cycleSizes ?? [];
  walk.cycles =
    cfg?.subsequentExecutionMode === "cycle-sizes" &&
    walk.cycleSizes.length > 0;

  let status = { screenCount: 1, hasMovableWindow: true };
  try {
    status = await getWelcomeStatus();
  } catch (err) {
    // One display and something to move is the modest guess: it teaches the
    // two steps that always exist rather than promising a second screen.
    console.error("could not read the welcome status", err);
  }

  renderStage(status.screenCount);
  walk.slides = buildSlides(cfg);
  renderDeck();
  movePane(FLOATING, false);
  if (!status.hasMovableWindow && walk.slides.length > 0) {
    setWalkNote(
      "No window open yet — presses will preview. Open one for the real thing.",
    );
  }

  // Claim last, so the first run is only spent once the screen it owes has
  // actually been rendered. Claiming records itself immediately, so quitting
  // without dismissing does not bring the window back.
  try {
    await takeOrientation();
  } catch (err) {
    console.error("could not record the first-run welcome", err);
  }
}

async function boot(): Promise<void> {
  if (isWelcomeScreen) {
    await bootWelcome();
    return;
  }

  if (isAboutScreen) {
    dom.app.classList.add("app--about");
    for (const child of dom.app.children) {
      if (child !== dom.about) (child as HTMLElement).hidden = true;
    }
    dom.about.hidden = false;
    // Wire the action before awaiting anything, so a slow or failing
    // version lookup can never leave the button dead.
    dom.github.addEventListener("click", () => {
      void openUrl(GITHUB_URL).catch((err) =>
        console.error("could not open the source repository", err),
      );
    });
    dom.aboutCheckUpdate.addEventListener("click", async () => {
      dom.aboutCheckUpdate.disabled = true;
      dom.aboutUpdateStatus.textContent = "Checking for updates…";
      try {
        dom.aboutUpdateStatus.textContent = describeAboutUpdateStatus(
          await checkForUpdates(),
        );
      } catch (err) {
        dom.aboutUpdateStatus.textContent =
          `Could not check for updates: ${String(err)}`;
      } finally {
        dom.aboutCheckUpdate.disabled = false;
      }
    });
    try {
      const version = await getVersion();
      dom.aboutVersion.textContent = version;
    } catch (err) {
      console.error("could not read app version", err);
      dom.aboutVersion.textContent = "Unavailable";
    } finally {
      dom.aboutVersion.removeAttribute("aria-busy");
    }
    return;
  }

  wireEvents();
  await listen("tile://check-for-updates", () => {
    focusUpdatePanel();
    void runUpdateCheck();
  });
  await listen("tile://show-updates", () => {
    focusUpdatePanel();
    void refreshUpdateStatus().then(scheduleUpdateRefresh);
  });
  const initialUpdateStatus = updateIntent === "check"
    ? runUpdateCheck()
    : refreshUpdateStatus();
  if (updateIntent === "check" || updateIntent === "show") {
    focusUpdatePanel();
  }
  // Build provenance is fetched first and separately: if it fails, the rest of
  // the settings UI should still load.
  try {
    renderBuildInfo(await getBuildInfo());
  } catch (err) {
    console.error("could not read build info", err);
  }
  try {
    config = await getConfig();
    failures = await getHotkeyFailures();
  } catch (err) {
    setRecordingStatus(`Could not load settings: ${String(err)}`);
    return;
  }
  renderBindings();
  renderBehaviour();
  await refreshPermission(false);
  scheduleUpdateRefresh(await initialUpdateStatus);
}

void boot();
