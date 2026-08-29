---
name: Tile
description: Keyboard-driven window management for macOS and Windows — a system-native utility that stays out of the way.
colors:
  accent: "#2563eb"
  accent-text: "#ffffff"
  accent-dark: "#3b82f6"
  ink: "#1f2937"
  ink-dim: "#667085"
  desk: "#f7f8fa"
  surface: "#ffffff"
  surface-sunken: "#f1f3f6"
  hairline: "#dfe3e8"
  danger: "#b42318"
  caution-bg: "#fff9eb"
  caution-border: "#f1c56b"
  focus: "#2563eb"
typography:
  display:
    fontFamily: "-apple-system, BlinkMacSystemFont, Segoe UI, Roboto, sans-serif"
    fontSize: "30px"
    fontWeight: 700
    lineHeight: 1.1
    letterSpacing: "-0.04em"
  title:
    fontFamily: "-apple-system, BlinkMacSystemFont, Segoe UI, Roboto, sans-serif"
    fontSize: "16px"
    fontWeight: 700
    lineHeight: 1.3
    letterSpacing: "-0.01em"
  body:
    fontFamily: "-apple-system, BlinkMacSystemFont, Segoe UI, Roboto, sans-serif"
    fontSize: "14px"
    fontWeight: 400
    lineHeight: 1.45
    letterSpacing: "normal"
  label:
    fontFamily: "-apple-system, BlinkMacSystemFont, Segoe UI, Roboto, sans-serif"
    fontSize: "12px"
    fontWeight: 400
    lineHeight: 1.45
    letterSpacing: "normal"
    fontFeature: "tabular-nums"
rounded:
  control: "6px"
  inner: "8px"
  card: "10px"
  mark: "13px"
spacing:
  hairline-gap: "6px"
  tight: "8px"
  related: "12px"
  block: "20px"
  section: "28px"
components:
  button:
    backgroundColor: "{colors.surface-sunken}"
    textColor: "{colors.ink}"
    rounded: "{rounded.control}"
    padding: "8px 14px"
    typography: "{typography.body}"
  button-hover:
    backgroundColor: "{colors.hairline}"
  button-primary:
    backgroundColor: "{colors.accent}"
    textColor: "{colors.accent-text}"
    rounded: "{rounded.control}"
    padding: "8px 14px"
  button-danger:
    textColor: "{colors.danger}"
    backgroundColor: "{colors.surface-sunken}"
  binding-key:
    backgroundColor: "{colors.surface-sunken}"
    textColor: "{colors.ink}"
    rounded: "{rounded.control}"
    padding: "6px 10px"
    width: "126px"
  binding-key-empty:
    textColor: "{colors.ink-dim}"
  binding-key-recording:
    textColor: "{colors.accent}"
  panel:
    backgroundColor: "{colors.surface}"
    rounded: "{rounded.card}"
    padding: "16px 18px"
  panel-warning:
    backgroundColor: "{colors.caution-bg}"
  card:
    backgroundColor: "{colors.surface}"
    rounded: "{rounded.card}"
    padding: "26px 24px 22px"
  slide-key:
    backgroundColor: "{colors.surface-sunken}"
    textColor: "{colors.ink}"
    rounded: "{rounded.control}"
    padding: "7px 13px"
  slide-key-done:
    backgroundColor: "{colors.accent}"
    textColor: "{colors.accent-ink}"
  keycap:
    backgroundColor: "{colors.surface-sunken}"
    textColor: "{colors.ink}"
    rounded: "{rounded.control}"
    padding: "2px 7px"
    typography: "{typography.label}"
---

# Design System: Tile

## Overview

**Creative North Star: "The Quiet Utility"**

Tile's job is to move windows, and its interface's job is to be forgotten. Every
surface is built from the host operating system's own vocabulary — the system UI
font, a neutral desk-grey page, white cards, hairline borders, one blue accent —
so that Settings feels like a preference pane rather than a website that happens
to live in a window. Nothing here competes with the user's actual work; the
windows on screen are the content, and Tile is the hand that moves them.

The system is deliberately narrow. One accent colour, one radius family, one
font stack, four type steps, and a single strip of authored motion. That
restraint is not minimalism as a style — it is what lets a tray utility open a
window for eight seconds and close it again without leaving an impression. Where
the UI does become expressive, it earns it by being literal: the welcome
screen's miniature desk is not an illustration, it is a live mirror of where the
real window went.

Density follows the task. Settings is a long, scannable list of shortcut rows
where scanability outranks air; About is a small, centred card, and Welcome is a
centred composition with no card at all, because in both there is only one thing
to read. All three derive from the same tokens.

**Key Characteristics:**

- System-native by default — the OS font stack, no webfonts, no brand type.
- One accent (`#2563eb`), used only to mean *active, focused, or done*.
- Fully flat: tonal layering and 1px hairlines carry every boundary.
- Automatic light and dark via `prefers-color-scheme` — no manual theme switch.
- Motion is rare, monotonic, and always inside `prefers-reduced-motion: no-preference`.

## Colors

A three-step neutral stack in cool grey, one blue accent, and two semantic
colours that appear only when something is wrong or needs care.

### Primary

- **Signal Blue** (`#2563eb`; `#3b82f6` in dark): the only accent. It marks the
  focus ring, the recording state of a shortcut, the keycaps of a shortcut the
  app just watched work,
  the tinted window in the welcome stage, the primary button, and the
  development-build panel border. It never decorates a heading, a divider, or a
  background field.

### Neutral

- **Desk Grey** (`#f7f8fa`; `#17191c` dark): the page behind everything. Named
  for what it is — the desk the cards sit on.
- **Card White** (`#ffffff`; `#22262b` dark): every panel, card, list and input
  fill.
- **Sunken Grey** (`#f1f3f6`; `#2b3036` dark): the recessed tone — secondary
  buttons, keycaps, shortcut keys, the mini display in the stage, and the page
  behind the centred About card. It is not the Welcome window's page: there the
  mini displays need it, so it is spent inside the composition rather than under
  it.
- **Ink** (`#1f2937`; `#f3f4f6` dark): all primary text.
- **Dim Ink** (`#667085`; `#a8b0bb` dark): descriptions, hints, units, counts,
  footnotes — anything the eye may skip.
- **Hairline** (`#dfe3e8`; `#3a414a` dark): every border and divider, always 1px.

### Tertiary

- **Fault Red** (`#b42318`; `#f97066` dark): conflict notes, destructive
  actions, the clear-binding button on hover. Text only — never a fill.
- **Caution Sand** (`#fff9eb` fill / `#f1c56b` border; `#332a16` / `#6b5520`
  dark): the warning panel, and the border a refused pane borrows for the length
  of its shake. The welcome deck's note is dim ink, not caution: by the time it
  speaks, the shake has already said no, and all the words have left to do is
  say what to press instead.

### Named Rules

**The One Blue Rule.** Blue means *this is live*: focused, recording, active,
done. If an element is not in one of those states, it is grey. A blue that means
"important" or "on brand" is a bug.

**The Three Tones Rule.** Depth is exactly three greys deep — desk, card, sunken
— plus a hairline. A fourth tone means the layout, not the palette, needs
fixing.

**The Semantic Pair Rule.** Red and sand are reserved: red states a problem,
sand asks for attention. Neither is available for emphasis.

## Typography

**Display Font:** the host system UI stack (`-apple-system`,
`BlinkMacSystemFont`, `Segoe UI`, `Roboto`, sans-serif)
**Body Font:** the same stack — there is only one.
**Label/Mono Font:** none. Keycaps and shortcut keys use the body face with
tabular figures.

**Character:** whatever the operating system considers its own voice. San
Francisco on macOS, Segoe on Windows. The pairing has no personality by design,
which is the personality: Tile reads as part of the system it is adjusting.

### Hierarchy

- **Display** (700, 30px, 1.1, -0.04em): the app name on the About card. One per
  window, at most.
- **Title** (700, 16px, 1.3, -0.01em): section and panel headings in Settings,
  and the instruction line on a welcome slide — the one thing on that slide the
  user must read.
- **Body** (400, 14px, 1.45): the root size and everything unspecified. Settings
  is capped at a 680px column, the centred cards at 468px, so the measure stays
  well inside a comfortable read.
- **Label** (400, 12px, tabular figures): descriptions, hints, counts, keycaps,
  footnotes. 11px exists for the single quietest line on a card and nowhere else.

Semi-bold weights (550/650) mark a row label or a small subheading where 700
would shout at 13px. The welcome wordmark is one of them: 15px/650, a step
*below* the instruction it sits above, because the branding is the frame and the
instruction is the point.

### Named Rules

**The System Face Rule.** No webfont, ever. Tile inherits the OS UI font so an
update to the OS updates Tile.

**The Tabular Numbers Rule.** Anything that changes in place while the user
watches — a step count, a version, a millisecond value, a shortcut — uses
`font-variant-numeric: tabular-nums` so it cannot twitch.

## Layout

Two spatial models, both from the same tokens.

**Settings (and All Shortcuts)** is a single 680px column, centred, with 32px of
top padding and 28px between sections. Content is a vertical stack of panels and
list rows; there is no grid, no sidebar, and no second column at any width. Rows
are `1fr auto` grids — label left, control right — collapsing to a single column
below 560px, where the label moves above its control and shortcut keys stretch
full width.

**The About window** is a centred composition: a single card of
`min(100%, 360px)`, auto-margined inside a sunken-grey page so it sits optically
centred in a window taller than itself and still scrolls from the top when it is
not.

**The Welcome window** is the same centred column at `min(100%, 468px)`, but
with no card: no fill, no border, no radius, sitting directly on card white. A
card exists to separate its contents from a page, and this window has no page
and nothing else on it — the border was an outline drawn around the whole
contents of a window that already has a frame. The window is the card. The page
tone changes with it: sunken grey is needed *inside* the composition, for the
stage's mini displays, so it cannot also be the ground they sit on.

Spacing rhythm runs 6 / 8 / 12 / 20 / 28: hairline gaps inside a control cluster,
8 between siblings, 12 between related blocks, 20 between the parts of a card,
28 between whole sections. Space above a heading always exceeds the space below
it.

The only breakpoint is 560px — the width at which a Settings window stops being
a two-column form. There is no mobile; these are desktop windows the user can
drag narrow.

## Elevation & Depth

**There are no shadows.** Depth is entirely tonal: the desk grey recedes, the
card white advances, the sunken grey pushes controls back into the surface, and
a 1px hairline draws every boundary. A card is legible as a card because it is a
different tone than what is behind it, not because it floats.

The one exception is not decoration and is not elevation in the visual sense:
inside the welcome screen's stage, the miniature window carries a shadow that
*encodes state* — a lifted, diffuse shadow when the window is floating, a tight,
close one when it is snapped flush to an edge. Remove it and the stage loses the
difference between before and after. It is a picture of a window, not a UI
surface, and it is the only place the rule bends.

### Shadow Vocabulary

Both are black, never `--text`. A foreground-tinted shadow inverts with the
theme, and in dark mode a window would wear a white halo — a glow, not a window
lifted off a desk.

- **Floating window** (`box-shadow: 0 7px 14px -6px rgb(0 0 0 / 55%)`):
  the stage's pane before it is snapped.
- **Snapped window** (`box-shadow: 0 2px 4px -2px rgb(0 0 0 / 40%)`):
  the same pane once Tile has placed it.

A focus ring (`box-shadow: 0 0 0 2px` of 30% accent, on a recording shortcut) is
a state indicator, not depth, and is exempt.

### Named Rules

**The Flat Rule.** Surfaces do not cast shadows. If two things need separating,
change the tone or draw a hairline. A shadow is only permitted when it carries
information a tone cannot.

## Shapes

One radius family, scaled to the size of the thing: 6px for controls (buttons,
inputs, selects, keycaps, shortcut keys), 8px for inner blocks (notes, the
deck, the stage's screens), 10px — the `--radius` token — for every card,
panel and grouped list, and 13–14px for the app mark, which is the icon's own
squircle and not a UI radius.

Borders are always exactly 1px and always the hairline colour; the one deviation
is the settings keycap, whose bottom border is 2px so it reads as a physical
key. Grouped
lists share a single outer border and separate their rows with internal
hairlines rather than gaps, so a list of shortcuts reads as one object.

Nothing is circular except the welcome deck's progress dots (6px), which stretch
to a 16px pill on the slide the user is on.

## Components

### Buttons

- **Shape:** softly rounded (6px), 8px × 14px padding, body type at inherited
  size.
- **Default:** sunken grey fill, ink text, hairline border. Hover deepens the
  fill to the hairline colour; there is no lift, scale, or shadow.
- **Primary:** accent fill, white text, transparent border. Hover brightens it
  5% rather than changing hue. One primary per window, at the point of exit.
- **Danger:** the default button with red text. Destructive actions never get a
  filled red button.
- Buttons in a card row stretch to equal width (`flex: 1`); buttons in a footer
  sit at their natural width.

### Cards / Containers

- **Corner Style:** 10px.
- **Background:** card white on a desk-grey (Settings) or sunken-grey (About,
  Welcome) page.
- **Shadow Strategy:** none — see Elevation & Depth.
- **Border:** 1px hairline.
- **Internal Padding:** 16–18px for a Settings panel, 26/24/22 for a centred
  card.

### Inputs / Fields

- **Style:** card-white fill, 1px hairline, 6px radius, inherited font. Selects
  are 190px minimum; number fields 64–72px; ranges take the accent colour.
- **Focus:** a 2px accent outline offset by 2px — the same ring everywhere,
  applied via `:focus-visible` so pointer users never see it.
- **Disabled:** 0.55 opacity and `cursor: not-allowed`, inherited by whole
  fieldsets rather than styled per control.
- **Field row:** a 170px label column and a flexible control column, collapsing
  to stacked blocks below 560px.

### Signature Component — the shortcut row

The unit Settings is made of. A `1fr auto` grid: action name left, then a
126px-wide keycap button showing the bound chord in tabular figures, then a
clear button. Empty bindings read as dim italic placeholder text; a row being
recorded turns its keycap accent-coloured and wraps it in the accent ring.
Conflicts and hints drop onto a full-width third line beneath, red for a
conflict and dim for information. Rows never move or resize between states — only
their colour changes — so recording a shortcut cannot reflow the list under the
user's cursor.

### Signature Component — the shortcut slide

The welcome deck deals one shortcut at a time, and each slide is exactly three
tiers, top to bottom: an instruction in 15px/650 telling the user what to do
("Snap the window left."), the keycaps that do it, and one dim 12px line saying
what the press actually gets them ("It takes the left half of whichever display
it is on."). The order is deliberate — ask, show the key, then explain the
result — so a user who already knows the answer never has to read past the
keycaps. Nothing on a slide is decoration; drop any tier and the slide either
stops asking, stops teaching, or stops meaning anything.

Above the deck sits the smallest possible frame: a 28px mark and a "Tile"
wordmark on one line, then a single dim lede that counts the deck out loud
("Four shortcuts, pressed for real — each one moves the window behind this
card."). The count is read from the slides that were actually built, never
hardcoded, so a machine missing a binding gets a shorter deck and an honest
sentence. The lede is spent once the walkthrough is over: on the closing slide
it goes `visibility: hidden`, not `display: none`, so the composition below it
does not jump at the last moment. The deck is four shortcuts and a close: snap left, snap
right, cycle the width, fill the screen, done. Every one of the five is a key —
Return finishes it — because a walkthrough that teaches the keyboard and then
asks for the mouse has stopped believing its own lesson. The cycle slide is the only one
that outlasts a single press; a row of size glyphs (½ ⅔ ⅓) lights up one at a
time and the slide stands until every one of them has. The keyboard is the only way forward — the slide is
left behind when the backend reports that its shortcut really moved a window,
never on a click. A window that was *already* where the slide asked counts too:
Tile agreeing with the user is not Tile failing them, and treating a no-op as a
miss makes the first slide unpassable for anyone whose window happened to start
left-snapped. Proving it fills the keycaps with accent, and after a 900ms
hold the track slides on, long enough for the pane above to arrive first. Slides
are laid out but `visibility: hidden` off-screen, so the track is as tall as its
tallest slide and advancing never resizes the card under the user.

### Signature Component — the stage

A miniature of the user's desk: one 16:10 mini display per real display, each
with a menu-bar strip (flipped to the bottom on Windows) and a single accent dot
standing for Tile itself. A tinted pane inside is positioned against the
measured boxes of those displays and travels to its new slot in 420ms whenever
the real window moves. A dashed hairline outline, drawn in dim ink and never in
accent, shows where the current slide's shortcut would put it. The outline is a
promise about the next press and the pane is a report of the last one.

The pane may show a move that did not happen in exactly one case: the press was
real but there was no window to move. An empty desk is common on a first run and
must not dead-end the walkthrough, so the pane travels anyway and the note under
the deck says the move was a preview. The rule the exception keeps is the real
one — the pane never *silently* shows a hypothetical. Whenever it is not
reporting, the screen says so in words.

The pane may also decline to show a move that *did* happen: a press the current
slide did not ask for. The deck is a lesson with an order, and mirroring a
detour would teach the detour. Instead the pane shakes in place — translation
only, 340ms, no travel, because not going anywhere is the whole message — takes
the caution border for the same beat, and a dim line under the deck names the
key that was wanted. This leaves the stage briefly disagreeing with the real
desk, which is why it is written down: it costs one wrong press, is repaired by
the next right one, and is the smaller lie than a walkthrough that quietly
rewards ignoring it. A wrong key is answered whether or not it moved anything,
so the maximize slide — where the window is often already maximized — is not the
one slide that says nothing.

The note under the deck holds its line whether or not it is speaking. The
composition is vertically centred, so a message that appears from nothing would
shove the very thing it is talking about.

The welcome window does not take focus while it is teaching, so the window the
user was already working in stays the one Tile moves. It floats above that
window instead, because being seen and being focused are different needs: an
accessory app's window is not raised for it, but a walkthrough that took focus
would make itself the target and move its proof out of sight.

It takes focus exactly once, on arrival at the closing slide, and the reason is
the same reason it refused focus before. The deck is keyboard-driven end to
end — but the first four steps are driven by *global* shortcuts, which do not
need this window listening, and the last step is an ordinary Return, which does.
By then there is no shortcut left to demonstrate and nothing left to move, so
the cost of being frontmost has gone to zero while the need has appeared. The
dismiss button takes the focus ring on the same beat, so the key is visible
before it is pressed.

Focus here means three things, not one, and the first two are the ones that
look like they worked. Tile is an accessory app, and AppKit will not make a
window key while the app behind it is inactive — so ordering the window forward
succeeds and changes nothing. Activating the app is the missing half, but it has
to happen on the main thread, and a command handler is not on it, so the call
returns cleanly from the wrong thread and does nothing at all. And on macOS 14
and later, activation is *cooperative*: a background accessory app asks, and the
window server is entitled to say no. It does.

So Tile becomes a regular app for as long as the welcome window is open, and
hands the policy back the moment it closes. **A Dock icon appears** — the honest
price of a window that can be typed into, paid once, by the only window in the
app that ever needs the keyboard. It is promoted at the closing slide rather
than at open, because the transition can itself activate the app, and doing that
mid-walkthrough would steal the very focus the deck spends four slides
protecting.

There is one earlier exit, and it opens only when the deck has failed. Three
wrong keys in a row is not someone who needs the instruction repeated a fourth
time; it is someone who wants out and has no way to say so, because Skip is a
mouse target precisely for the reason above. But that reason has just failed on
its own terms — the shortcuts are not landing here anyway — so the deck stops
insisting, takes the keyboard, and puts the exit under Esc, the key someone
stuck is already reaching for. The Esc hint stays hidden until that moment: a
key that the window cannot yet receive would be a lie. The deck offers the exit;
it does not take it, and one right key puts the count back to zero.

A key is credited to whichever unfinished slide teaches it, not only to the
slide showing. Someone who already knows Tile should not be refused for proving
it in a different order than the deck happened to choose, so an early press ticks
its own slide off and the deck resumes at the first one still standing. The size
cycle is left out of this: it is a lesson about pressing one key repeatedly, and
a single press of it has not shown that.

## Do's and Don'ts

### Do:

- **Do** build from the four CSS custom properties that already exist
  (`--surface`, `--surface-2`, `--border`, `--text`) before introducing a value.
- **Do** separate surfaces with a tone change plus a 1px hairline.
- **Do** reserve the accent for live states: focus, recording, active, done.
- **Do** use `:focus-visible` with the 2px accent outline for every interactive
  element — the app is keyboard-first and the ring is the product.
- **Do** put every transition and animation inside
  `@media (prefers-reduced-motion: no-preference)`, and keep easing monotonic
  (`cubic-bezier(0.22, 1, 0.36, 1)` or a plain `ease-out`).
- **Do** give anything that updates in place tabular figures.
- **Do** keep both light and dark correct in the same edit; the dark palette is
  a `prefers-color-scheme` block a few lines above, not a later task.

### Don't:

- **Don't** add a `box-shadow` or `drop-shadow` to a UI surface. The two shadows
  in the codebase encode window state inside the stage; that is the whole budget.
- **Don't** introduce a webfont, an icon font, or an icon package. Icons are
  inline SVG; type is the system stack.
- **Don't** use blue for emphasis, headings, or brand presence.
- **Don't** fill anything with red. Red is text.
- **Don't** add a fourth neutral tone or a second radius family.
- **Don't** animate an entrance on more than one element per window; one
  authored moment, and on the welcome screen it is already spent on the payoff
  of a press — the pane travels, the keycaps light, the deck advances.
- **Don't** let a state change reflow a list. Colour changes; geometry holds.
- **Don't** show a control, count, or claim the app cannot verify — the UI is
  bound by the product's honesty rule as tightly as by its palette.
