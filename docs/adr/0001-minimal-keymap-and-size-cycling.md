# Minimal keymap with two modifiers and size cycling

Tile drops its 71 unbound actions (explicit thirds, fourths, sixths, ninths, direct corners, vertical halves, incremental move/resize/halve-double, and secondary display commands) and keeps only 9 bindable actions — `Left`, `Right`, `Maximize`, `Restore`, `Center Column`, and the four directional display throws — plus single-hold corner composition. Rather than freeform per-action chords, shortcuts use two configurable group modifiers (**Base modifier** for placement, **Display modifier** for displays) paired with per-action keys, and every fractional width is reached by repeating a placement shortcut through the configurable **Size cycle**.

## Considered Options

- **Keep an 80-action catalogue with freeform per-action chords**: Matches Rectangle's exhaustive direct-binding model, but clutters Settings (requiring collapsible family accordions, search, and an Advanced menu for sizing/step knobs), duplicates tray entries across explicit fractions, and breaks corner composition whenever shortcuts are rebound.
- **Keep only explicit fractional-grid actions removed while retaining Move/Resize/Halve-Double**: Cuts 37 actions, yet preserves most of the UI complexity and 7 of 8 Advanced settings.

## Consequences

- Corner composition now tracks the user's configured **Base modifier** and `Left` / `Right` / `Maximize` / `Restore` keys instead of hardcoded arrows; on Windows this keeps the low-level keyboard hook active whenever corner composition is possible.
- Grid cells that cannot be reached by cycling a side, corner, or center column (such as ninths, middle fourths, middle vertical third, and center sixths) are no longer supported.
- The tray/menu-bar menu drops its explicit `½ / ⅔ / ⅓` submenus and cycles on repeated clicks of `Left`, `Right`, and `Center Column`.
- `Settings ▸ Advanced` and `animation.fps` are removed; animation pacing matches each display's refresh rate automatically (with a 45 fps Accessibility cap retained on macOS).
- `config.json` bumps to `schemaVersion: 2`; legacy v1 configs migrate silently on load by dropping retired keys/fields and unifying each group's bindings onto its majority modifier.
