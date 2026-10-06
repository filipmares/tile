// The one search over Settings. It works on the markup rather than on a list
// of known settings, so anything added later is found without touching this
// file:
//
// - An item is a `.field` (a labelled control), a `.binding` (a shortcut
//   row), or anything marked `data-search-item`. Its text is everything it
//   shows, its controls' aria-labels, and any visible help text it points to
//   with aria-describedby.
// - A group is a `.section`, a `.settings-group`, or anything marked
//   `data-search-group`. Its heading is the `data-search-heading` elements
//   that belong to it, or, for a <details>, its <summary>.
//
// An item shows when every word of the query is found in its own text plus
// the headings of the groups around it, so "motion" shows every motion
// setting and "spacing top" narrows to the top gap. Matching is pure and
// lives in searchText.ts.

import { dom } from "./dom";
import { isMac } from "./hotkey";
import { matchesTerms, queryTerms } from "./searchText";

const STRINGS = {
  empty: (query: string) => `No settings match \u201c${query}\u201d`,
  count: (n: number) => (n === 1 ? "1 setting matches" : `${n} settings match`),
};

const ITEM = ".field, .binding, [data-search-item]";
const GROUP = ".section, .settings-group, [data-search-group]";
/** Set on whatever the search is hiding; the stylesheet does the hiding. */
const HIDDEN = "searchHidden";
/** A <details>' own open state, kept while the search forces it open. */
const OPEN_BEFORE = "searchOpenBefore";
const ANNOUNCE_DELAY_MS = 500;

let terms: string[] = [];
let announceTimer: number | null = null;
/** The last thing queued for the live region; a re-render repeats nothing. */
let pendingAnnouncement = "";

/** Every section the search covers: the settings sections of the window. */
function sections(): HTMLElement[] {
  return [...dom.app.querySelectorAll<HTMLElement>(":scope > .section")];
}

/** The group that directly contains `node`, if any. */
function parentGroup(node: Element): HTMLElement | null {
  return node.parentElement?.closest<HTMLElement>(GROUP) ?? null;
}

/** Groups around `node`, innermost first, up to and including `stop`. */
function groupsAround(node: Element, stop: Element | null = null): HTMLElement[] {
  const groups: HTMLElement[] = [];
  for (let g = parentGroup(node); g; g = parentGroup(g)) {
    groups.push(g);
    if (g === stop) break;
  }
  return groups;
}

function headingText(group: HTMLElement): string {
  const marked = [...group.querySelectorAll<HTMLElement>("[data-search-heading]")]
    .filter((heading) => heading.closest(GROUP) === group);
  if (marked.length > 0) return marked.map((h) => h.textContent ?? "").join(" ");
  if (group instanceof HTMLDetailsElement) {
    const summary = group.querySelector(":scope > summary");
    return summary?.textContent ?? "";
  }
  return "";
}

function itemText(item: HTMLElement): string {
  const parts = [item.textContent ?? ""];
  for (const control of item.querySelectorAll<HTMLElement>(
    "[aria-label], [aria-describedby]",
  )) {
    parts.push(control.getAttribute("aria-label") ?? "");
    for (const id of (control.getAttribute("aria-describedby") ?? "").split(/\s+/)) {
      const help = id ? document.getElementById(id) : null;
      if (help && !help.hidden) parts.push(help.textContent ?? "");
    }
  }
  return parts.join(" ");
}

function withHeadings(text: string, groups: HTMLElement[]): string {
  return [text, ...groups.map(headingText)].join(" ");
}

function setHidden(node: HTMLElement, hidden: boolean): void {
  if (hidden) node.dataset[HIDDEN] = "";
  else delete node.dataset[HIDDEN];
}

/**
 * Whether a <details> is open by the user's own choice. While a search is
 * forcing it open this is the state it had before, so code that re-renders a
 * disclosure keeps the user's choice rather than the search's.
 */
export function openByUser(details: HTMLDetailsElement): boolean {
  const before = details.dataset[OPEN_BEFORE];
  return before === undefined ? details.open : before === "true";
}

/**
 * Re-applies the current query to the page. Safe to call after any part of
 * Settings re-renders; with no query it puts everything back as it was.
 */
export function applySettingsSearch(): void {
  const active = terms.length > 0;
  const groups = sections().flatMap((section) => [
    section,
    ...section.querySelectorAll<HTMLElement>(GROUP),
  ]);
  const items = sections().flatMap((section) => [
    ...section.querySelectorAll<HTMLElement>(ITEM),
  ]);

  const shown = new Set<HTMLElement>();
  const counted = new Set<string | HTMLElement>();
  for (const item of items) {
    const visible =
      !active || matchesTerms(withHeadings(itemText(item), groupsAround(item)), terms);
    setHidden(item, !visible);
    if (!visible) continue;
    shown.add(item);
    // An item shown in more than one place shares a key; count it once.
    counted.add(item.dataset.searchKey ?? item);
  }

  for (const group of groups) {
    // Headings already count toward the items under them, so a group shows
    // only for an item it holds; an empty group is not a result.
    const visible = !active || [...shown].some((item) => group.contains(item));
    setHidden(group, !visible);

    if (!(group instanceof HTMLDetailsElement)) continue;
    if (!active) {
      const before = group.dataset[OPEN_BEFORE];
      if (before !== undefined) {
        group.open = before === "true";
        delete group.dataset[OPEN_BEFORE];
      }
      continue;
    }
    if (group.dataset[OPEN_BEFORE] === undefined) {
      group.dataset[OPEN_BEFORE] = String(group.open);
    }
    // Opened only for what matched inside it, not for a heading further
    // out: "shortcuts" should not unfold all eighty actions. Terms the outer
    // headings already supply are set aside, and the rest must be found
    // here, so "behaviour top" still opens Window spacing.
    const outer = withHeadings("", groupsAround(group));
    const inner = terms.filter((term) => !matchesTerms(outer, [term]));
    const holdsMatch =
      inner.length > 0 &&
      (matchesTerms(headingText(group), inner) ||
        [...shown].some(
          (item) =>
            group.contains(item) &&
            matchesTerms(withHeadings(itemText(item), groupsAround(item, group)), inner),
        ));
    group.open = group.dataset[OPEN_BEFORE] === "true" || holdsMatch;
  }

  const query = dom.settingsSearch.value.trim();
  dom.settingsSearchEmpty.hidden = !active || counted.size > 0;
  dom.settingsSearchEmpty.textContent = active ? STRINGS.empty(query) : "";
  scheduleAnnouncement(
    active ? (counted.size > 0 ? STRINGS.count(counted.size) : STRINGS.empty(query)) : "",
  );
}

/** Says the result count once typing pauses, not on every keystroke. */
function scheduleAnnouncement(text: string): void {
  if (text === pendingAnnouncement) return;
  pendingAnnouncement = text;
  if (announceTimer !== null) window.clearTimeout(announceTimer);
  announceTimer = window.setTimeout(() => {
    announceTimer = null;
    dom.settingsSearchStatus.textContent = text;
  }, ANNOUNCE_DELAY_MS);
}

function setQuery(value: string): void {
  // Re-applied even when the terms are unchanged: the empty state quotes the
  // query as typed.
  terms = queryTerms(value);
  applySettingsSearch();
}

function isFindShortcut(e: KeyboardEvent): boolean {
  const primary = isMac() ? e.metaKey && !e.ctrlKey : e.ctrlKey && !e.metaKey;
  return primary && !e.altKey && !e.shiftKey && e.key.toLowerCase() === "f";
}

export function wireSettingsSearch(): void {
  dom.settingsSearch.setAttribute(
    "aria-keyshortcuts",
    isMac() ? "Meta+F" : "Control+F",
  );
  dom.settingsSearch.addEventListener("input", () =>
    setQuery(dom.settingsSearch.value),
  );
  dom.settingsSearch.addEventListener("keydown", (e) => {
    if (e.key !== "Escape" || dom.settingsSearch.value === "") return;
    e.preventDefault();
    dom.settingsSearch.value = "";
    setQuery("");
  });
  window.addEventListener("keydown", (e) => {
    if (!isFindShortcut(e) || e.defaultPrevented) return;
    e.preventDefault();
    dom.settingsSearch.focus();
    dom.settingsSearch.select();
  });
}
