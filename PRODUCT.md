# Product

<!-- impeccable:product-schema 1 -->

## Platform

web

Recorded as `web` because every Tile surface is a webview (vanilla TypeScript +
Vite inside a Tauri v2 shell), not because Tile is a website. It is a desktop
application for macOS 10.15+ and Windows, and its UI follows desktop OS
conventions — real windows, keyboard focus, menu-bar/tray behaviour — not mobile
or marketing-web ones.

## Users

Keyboard-first power users on macOS and Windows who already live in shortcuts.
They are working — writing, coding, comparing two documents — and want the
window under their hands to move without leaving the keyboard or thinking about
it. They run Tile on both operating systems and expect the same muscle memory on
each desk.

Tile therefore does not have to teach the idea of window management. It has to
teach *its* keymap, once, and then get out of the way.

## Product Purpose

Tile moves, sizes and throws the focused window from the keyboard. It exists so
that placing a window costs one chord instead of a drag, and so that the same
chord works on both operating systems.

Success is invisibility: after the first day the user stops noticing Tile and
simply expects windows to land where they pressed.

## Positioning

**The press-again size cycle.** One shortcut carries every width — press ← for a
half, again for two thirds, again for a third, then round again — so the whole
keymap stays small enough to hold in one hand. Competitors expose each size as
its own binding; Tile folds them into a repeat. Everything downstream (four
arrows as the entire default set, seventy-plus actions left rebindable rather
than pre-bound) follows from that decision, and future work must not undo it by
spreading sizes back across separate keys.

## Operating Context

- **No main window.** Tile lives in the menu bar (macOS) or system tray
  (Windows). Its menu opens Settings; windows exist only when asked for.
- **Three windows exist:** Settings, About, and the first-run Welcome window
  (reopenable from the bottom of Settings). Tile skips its own windows when
  moving things, so a shortcut pressed while one is focused moves the window
  behind it.
- **Both desks, mixed displays.** Multi-monitor and mixed-DPI setups are normal,
  and placement is computed as fractions of each display's own work area.
- **Permission and precedence are part of the scene.** macOS requires the
  Accessibility permission before Tile can move anything; Windows uses a
  low-level keyboard hook that replaces Aero Snap and cannot see input aimed at
  elevated processes.
- Settings is the only place bindings are edited; `config.json` still owns a few
  knobs (step sizes, minimum window fractions, animation fps) with no UI yet.

## Capabilities and Constraints

- 76 window actions — halves, thirds, two-thirds, fourths, sixths, ninths,
  corners, corner thirds, maximize, maximize-height, almost-maximize, center,
  restore, display throws, and incremental move/resize/halve-double. **Six ship
  bound to keys**; the rest are reachable only by binding them yourself.
- Default keymap: `Control`+`Option` (macOS) or `Win` (Windows) plus the arrows;
  display throws add `Command` (macOS) or `Alt` (Windows).
- Repeat cycles size for the horizontal arrows and corners; the cycle set is
  configurable (½, ⅔, ¾, ¼, ⅓) and can be turned off entirely.
- Animated snapping is on by default, 40–1000 ms, and can be switched off in
  Settings ▸ Behaviour ▸ Motion.
- Architecture forbids shortcuts: `tile-core` is pure and platform-independent,
  `tile-platform` hides every OS API behind traits, `apps/tile` is a thin shell.
  UI work talks to the core through Tauri commands and events only.
- **Not built yet:** per-app rules, drag-snapping, and a tray menu that offers
  actions directly.
- Development builds are deliberately quarantined from an installed Tile: a
  separate config directory, the login item left alone, and every surface
  labelled *(Development)*.

## Brand Commitments

- The name is **Tile**; the tray/menu-bar icon and app icon are the existing
  assets in `apps/tile/icons`.
- Voice is plain, specific and unembellished — the README's register. It states
  what works, names what does not, and does not sell.
- Tile is MIT-licensed and openly credits [Rectangle](https://github.com/rxhanson/Rectangle)
  as inspiration while being an independent Rust implementation.

## Evidence on Hand

- `README.md` — the accurate, current description of behaviour; the shortcut
  table is generated from `crates/tile-core/src/config.rs` (`default_bindings`),
  the single source of truth for defaults.
- GitHub Releases carry signed/notarized macOS `.dmg` and Windows NSIS builds.
- `CONTRIBUTING.md`, `docs/RELEASING.md`, and a thorough CI workflow.
- **No** testimonials, user counts, benchmarks, press, pricing or case studies
  exist. Future work must not invent them.

## Product Principles

1. **Only what actually works ships.** Never show, promise or count an action
   that is not built or not bound. Where the UI can check, it checks.
2. **The keymap stays small.** New capability arrives as a repeat, a cycle or an
   unbound action — not as another default binding to memorize.
3. **Tile stays out of the way.** Tray-only, no Dock or taskbar presence, no
   main window; surfaces appear because the user asked and leave when done.
4. **The same product on both desks.** Platform differences are limited to what
   the OS genuinely demands (modifier, permission model, throw convention).
5. **Prove, don't explain.** Tile is faster to try than to describe; interfaces
   should let the user press the real keys and show the real result.

## Accessibility & Inclusion

- Motion is optional by product design: animated snapping can be switched off in
  Settings, and the UI honours `prefers-reduced-motion` — all authored motion
  sits inside a `prefers-reduced-motion: no-preference` block.
- Tile is a keyboard product first; every surface must be operable and legible
  without a pointer.
- No formal conformance standard has been established for this project.
