# Window Tiling

Language for placing and sizing windows through keyboard shortcuts.

## Language

**Base modifier**:
The modifier-key combination held while giving Tile a window-placement command.
_Avoid_: Super combo (which can be confused with the Super/Windows key)

**Display modifier**:
The modifier-key combination held while giving Tile a command to move a window to another display.
_Avoid_: Throw modifier, monitor modifier

**Side placement**:
A full-height window placement anchored to the left or right edge of the available screen area.
_Avoid_: Side snap

**Corner placement**:
A half-height window placement anchored to a top or bottom corner of the available screen area. Its width can vary; a corner placement is not necessarily a quarter of the screen.
_Avoid_: Quarter (when referring to corners of varying widths)

**Corner composition**:
A single-hold gesture with the base modifier where a side placement followed by the maximize or restore key turns the window into a corner placement on that side.

**Center column**:
A full-height window placement centered horizontally in the available screen area, with a variable width.
_Avoid_: Center (which can also mean repositioning a window without resizing it)

**Size cycle**:
The ordered sequence of screen fractions a placement steps through when its shortcut is repeated.
_Avoid_: Fractional sizing, size presets

