// The welcome window: the onboarding deck that teaches Tile's default
// shortcuts by having the user press them.

import { listen } from "@tauri-apps/api/event";
import {
  closeWelcomeWindow,
  focusWelcome,
  getConfig,
  getWelcomeStatus,
  setLaunchOnLogin,
  takeOrientation,
} from "./api";
import { dom } from "./dom";
import { formatHotkey, isMac } from "./hotkey";
import { config as sharedConfig, setConfig } from "./state";
import {
  ActionPerformed,
  Config,
  CycleSize,
  WelcomeStatus,
  WindowAction,
} from "./types";
import {
  configureWrites,
  followChangesElsewhere,
  hasPendingWrites,
  saveSetting,
} from "./writes";

/** User-facing copy for the welcome deck. */
const STRINGS = {
  snapLeft: "Snap the window left.",
  snapLeftDetail: "It takes the left half of whichever display it is on.",
  snapRight: "Now send it right.",
  snapRightDetail:
    "The same chord, the other arrow. That is the whole pattern.",
  cycle: "Press it again, and again.",
  // The sizes themselves are named by the pips below, which light up as
  // each one is seen; repeating them here would print the same row twice.
  // What the pips cannot say is why one press is not enough.
  cycleDetail:
    "Every repeat is a new width. This step waits for the whole cycle.",
  maximize: "Fill the screen.",
  maximizeDetailMac:
    "The work area, not full-screen: your menu bar stays put.",
  maximizeDetailOther:
    "The work area, not full-screen: your taskbar stays put.",
  display: "Send the window to an adjacent display.",
  displayDetail: "It keeps its shape while moving across monitors.",
  or: "or",
  noKeys: "Tile is waiting for keys.",
  noKeysAside: (home: string) =>
    `None of the default shortcuts are bound. Assign your own in Settings, from the ${home}.`,
  ledeOne:
    "One shortcut, pressed for real — it moves the window behind this card.",
  ledeMany: (count: string | number) =>
    `${count} shortcuts, pressed for real — each one moves the window behind this card.`,
  tryCombo: (combo: string) => `Try ${combo}.`,
  tryComboOrSkip: (combo: string) => `Try ${combo}, or press Esc to skip.`,
  done: "Done",
  step: (at: number, total: number) => `Step ${at} of ${total}`,
  preview: "No window open to move — so that was a preview.",
  noWindowYet:
    "No window open yet — presses will preview. Open one for the real thing.",
  homeMac: "menu bar",
  homeOther: "system tray",
};

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
  5: "Five",
};

/** Actions whose repeat walks the size cycle rather than doing nothing. */
const CYCLING_ACTIONS: WindowAction[] = [
  "left-half",
  "right-half",
  "top-half",
  "bottom-half",
  "top-left",
  "top-right",
  "bottom-left",
  "bottom-right",
];

const DISPLAY_ACTIONS: WindowAction[] = [
  "display-left",
  "display-right",
  "display-up",
  "display-down",
  "previous-display",
  "next-display",
];

/**
 * How long the pane and the tick are given before the deck moves on. The pane
 * takes 420ms to travel and the keycaps light in 220ms, so this is that
 * animation plus a beat to register it — not a pause on top of it.
 */
const ADVANCE_DELAY = 520;

/** One slide: a shortcut to try, and what counts as having tried it. */
interface Slide {
  id: "snap-left" | "snap-right" | "cycle" | "maximize" | "display";
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
  /** Real display count and current index; the stage may render at most three. */
  screenCount: 1,
  currentScreen: 0,
  displayNeighbors: [] as WelcomeStatus["displayNeighbors"],
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

let booted = false;

/** Wrong keys in a row before the deck stops insisting and offers the exit. */
const REFUSALS_BEFORE_EXIT = 3;

/**
 * Builds the slides this machine can actually complete. A shortcut nobody has
 * bound and a size cycle the user switched off are left out rather than taught
 * and then disproved.
 *
 * The order is one idea per slide, each one leaning on the last: left, then
 * right so the mirror image is obvious, then the same key again to show that
 * repeating resizes rather than doing nothing, then the whole screen and the
 * adjacent display when one is available.
 */
function buildSlides(
  cfg: Config | null,
  screenCount: number,
  currentScreen: number,
): Slide[] {
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
    STRINGS.snapLeft,
    STRINGS.snapLeftDetail,
  );
  add(
    "snap-right",
    "right-half",
    STRINGS.snapRight,
    STRINGS.snapRightDetail,
  );
  // Only worth a slide if repeating actually resizes. With cycling off, or a
  // cycle of one size, a second press changes nothing, and a slide the
  // keyboard cannot satisfy would strand the deck.
  if (walk.cycles && walk.cycleSizes.length > 1 && combo("right-half")) {
    add(
      "cycle",
      "right-half",
      STRINGS.cycle,
      STRINGS.cycleDetail,
      true,
    );
  }
  add(
    "maximize",
    "maximize",
    STRINGS.maximize,
    isMac() ? STRINGS.maximizeDetailMac : STRINGS.maximizeDetailOther,
  );
  if (screenCount > 1) {
    const displayAction = DISPLAY_ACTIONS.find(
      (action) => combo(action) && displayTarget(action, currentScreen) !== null,
    );
    if (displayAction) {
      add(
        "display",
        displayAction,
        STRINGS.display,
        STRINGS.displayDetail,
      );
    }
  }

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

/**
 * The miniature standing in for real display `index`, counted left to right.
 *
 * The stage draws at most three, so a window on a fourth display is shown on
 * the last miniature rather than on one that was never drawn.
 */
function onStage(index: number): number {
  if (walk.screens.length === 0) return 0;
  return Math.min(Math.max(index, 0), walk.screens.length - 1);
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

/**
 * Whether the pane is still the untidied window the stage opened with.
 *
 * Compared by shape rather than by identity: the pane starts life as a copy of
 * [`FLOATING`] placed on the user's own display, and a restore puts it back on
 * whichever display the window is on now, so neither is ever the same object.
 */
function isFloating(frame: PaneFrame): boolean {
  return (
    frame.x === FLOATING.x &&
    frame.y === FLOATING.y &&
    frame.w === FLOATING.w &&
    frame.h === FLOATING.h
  );
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

function displayTarget(action: WindowAction, from: number): number | null {
  if (action === "previous-display" || action === "next-display") {
    const step = action === "next-display" ? 1 : walk.screenCount - 1;
    return (from + step) % walk.screenCount;
  }
  return walk.displayNeighbors[from]?.[action] ?? null;
}

/**
 * Where the current slide's shortcut would put the window, using the engine's
 * directional destinations and the same shapes and cycle order.
 */
function ghostFrame(): PaneFrame | null {
  const slide = walk.slides[walk.at];
  if (!slide || slide.done) return null;

  const action = slide.actions[0];
  if (!action) return null;
  if (DISPLAY_ACTIONS.includes(action)) {
    const target = displayTarget(action, walk.currentScreen);
    if (target === null) return null;
    return {
      ...walk.pane,
      screen: onStage(target),
    };
  }
  const shape = PANE_SHAPES[action];
  if (!shape) return null;

  if (slide.id === "cycle") {
    const sizes = walk.cycleSizes;
    if (sizes.length === 0) return null;
    // Only a repeat advances the cycle. If anything else moved the window
    // since — including a press this slide turned down — the engine starts the
    // cycle from a half again, and the promise has to say so rather than
    // pointing at a size the next press will not produce.
    if (walk.lastAction !== action) {
      return { screen: walk.pane.screen, ...shape(0.5) };
    }
    // `sizeIndex` is -1 when the window sits at a half the configured cycle
    // does not contain, and that is a real position rather than a missing one:
    // the engine recovers its step from geometry, matches nothing, and starts
    // from the first configured size. `(-1 + 1) % len` is 0, so the line below
    // already promises exactly that — returning early here instead would blank
    // the outline for anyone whose cycle omits a half.
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
function reflectOnStage(
  action: WindowAction,
  place: boolean,
  screen: number | null,
): void {
  const repeat = action === walk.lastAction;
  const cycles = walk.cycles && CYCLING_ACTIONS.includes(action);
  // Where Tile actually put the window, which is the only thing that knows
  // *which* window moved. The walkthrough opened from Settings never had focus
  // to begin with, so the stage cannot work this out for itself. A press that
  // moved nothing reports no display, and the stage keeps its own reckoning.
  const realScreen =
    screen === null
      ? null
      : Math.min(Math.max(screen, 0), walk.screenCount - 1);
  const reported = realScreen === null ? null : onStage(realScreen);

  if (DISPLAY_ACTIONS.includes(action)) {
    const target =
      realScreen ?? displayTarget(action, walk.currentScreen) ?? walk.currentScreen;
    walk.currentScreen = target;
    const thrown = {
      ...walk.pane,
      screen: onStage(target),
    };
    const snapped = !isFloating(walk.pane);
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
  if (realScreen !== null) walk.currentScreen = realScreen;
  const onScreen = reported ?? walk.pane.screen;
  if (action === "restore") {
    movePane({ ...FLOATING, screen: onScreen }, false);
    return;
  }
  const shape = PANE_SHAPES[action];
  if (shape) movePane({ screen: onScreen, ...shape(fraction) }, true);
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
  if (combo) setWalkNote(STRINGS.tryCombo(combo));

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
    setWalkNote(combo ? STRINGS.tryComboOrSkip(combo) : null);
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
        or.textContent = STRINGS.or;
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
    dom.welcomeEndLine.textContent = STRINGS.noKeys;
    dom.welcomeEndAside.textContent = STRINGS.noKeysAside(
      isMac() ? STRINGS.homeMac : STRINGS.homeOther,
    );
    dom.welcomeSkip.hidden = true;
    dom.welcomeLede.textContent = "";
  } else {
    // Counted from the slides that were actually built, never from the four
    // this deck usually has: a machine missing a binding gets a shorter deck,
    // and a promise of four steps it is not going to deliver would be the one
    // dishonest line on the screen.
    dom.welcomeLede.textContent =
      walk.slides.length === 1
        ? STRINGS.ledeOne
        : STRINGS.ledeMany(COUNT_WORDS[walk.slides.length] ?? walk.slides.length);
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
        ? STRINGS.done
        : STRINGS.step(walk.at + 1, walk.slides.length);
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
    reflectOnStage(event.action, false, event.screen);
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
  setWalkNote(empty ? STRINGS.preview : null);

  const repeat = event.action === walk.lastAction;
  reflectOnStage(event.action, true, event.screen);
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

/**
 * Boots the welcome screen: its own window, and the only place Tile teaches
 * its defaults. Settings carries the controls and links back here.
 *
 * The order matters. Everything that can dead-end the screen — the buttons,
 * the event subscription — is wired before the first `await`, so a slow or
 * failing backend leaves a screen that is merely quiet rather than broken.
 * Runs once.
 */
export async function bootWelcome(): Promise<void> {
  if (booted) return;
  booted = true;
  dom.app.classList.add("app--welcome");
  for (const child of dom.app.children) {
    if (child !== dom.welcome) (child as HTMLElement).hidden = true;
  }
  dom.welcome.hidden = false;
  // Windows puts the icon in the system tray, macOS in the menu bar. This is
  // onboarding copy, so naming the wrong one sends the user hunting.
  dom.welcomeHome.textContent = isMac() ? STRINGS.homeMac : STRINGS.homeOther;
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
    setConfig(cfg);
  } catch (err) {
    console.error("could not load settings for the welcome screen", err);
  }
  // The toggle goes through the same queue as the settings window, so a
  // change made there while this window is open shows up here too.
  configureWrites({
    render: () => {
      if (sharedConfig) dom.welcomeLaunch.checked = sharedConfig.launchOnLogin;
    },
  });
  void followChangesElsewhere();
  dom.welcomeLaunch.checked = cfg?.launchOnLogin ?? true;
  dom.welcomeLaunch.addEventListener("change", async () => {
    const desired = dom.welcomeLaunch.checked;
    // Closing mid-write would leave the user not knowing whether it stuck.
    dom.welcomeDismiss.disabled = true;
    try {
      const saved = await saveSetting(dom.welcomeLaunchError, () =>
        setLaunchOnLogin(desired),
      );
      if (saved) dom.welcomeLaunch.checked = saved.launchOnLogin;
    } finally {
      dom.welcomeDismiss.disabled = hasPendingWrites();
    }
  });
  dom.welcomeLaunch.disabled = false;
  walk.cycleSizes = cfg?.cycleSizes ?? [];
  walk.cycles =
    cfg?.subsequentExecutionMode === "cycle-sizes" &&
    walk.cycleSizes.length > 0;

  let status: WelcomeStatus = {
    screenCount: 1,
    hasMovableWindow: true,
    currentScreen: 0,
    displayNeighbors: [],
  };
  try {
    status = await getWelcomeStatus();
  } catch (err) {
    // One display and something to move is the modest guess: it teaches the
    // two steps that always exist rather than promising a second screen.
    console.error("could not read the welcome status", err);
  }

  renderStage(status.screenCount);
  walk.screenCount = Math.max(status.screenCount, 1);
  walk.displayNeighbors = status.displayNeighbors;
  walk.currentScreen = Math.min(
    Math.max(status.currentScreen, 0),
    walk.screenCount - 1,
  );
  walk.slides = buildSlides(cfg, walk.screenCount, walk.currentScreen);
  renderDeck();
  // Start the pane on the display the window really is on. The miniatures run
  // left to right, and the main display is often not the leftmost one, so
  // starting at zero would mirror the first shortcut on the wrong screen.
  movePane(
    { ...FLOATING, screen: onStage(walk.currentScreen) },
    false,
  );
  if (!status.hasMovableWindow && walk.slides.length > 0) {
    setWalkNote(STRINGS.noWindowYet);
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
