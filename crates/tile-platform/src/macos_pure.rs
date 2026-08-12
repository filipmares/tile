// Pure, platform-independent helpers for the macOS backend.
//
// NOTE: this file is pulled into `macos.rs` with `include!("macos_pure.rs")`,
// which splices it in *after* other items. That means it must NOT use inner
// doc comments (`//!`) or module-level attributes/`use` statements at all —
// those are only legal at the top of a module and would break the include.
// Everything here is therefore fully path-qualified and documented with plain
// `//` comments.
//
// Why it is a standalone file: the whole `macos` module is gated behind
// `#[cfg(target_os = "macos")]`, so it never compiles on a Windows/Linux CI
// host. The logic here — the AppKit->Tile coordinate flip and the Carbon
// key/modifier lookup tables — is where subtle, silent bugs hide and needs no
// macOS API, so it should be testable everywhere.
//
// It is wired up in two mutually-exclusive ways:
//   * On macOS, `macos.rs` pulls it in with `include!("macos_pure.rs")`, so
//     these functions live in the `macos` module and its tests run on the
//     macOS runner.
//   * On every other host it can additionally be compiled as its own module so
//     the tests run there too — see the crate-level report for the exact
//     one-line addition to `lib.rs` the coordinator can make:
//     `#[cfg(not(target_os = "macos"))] #[path = "macos_pure.rs"] mod macos_pure;`

/// Converts a rectangle expressed in AppKit's screen space into Tile's unified
/// top-left-origin space.
///
/// AppKit uses a **bottom-left** origin: y grows *upwards*, and the primary
/// display (the one carrying the menu bar, i.e. `NSScreen.screens[0]`) has its
/// bottom-left corner at `(0, 0)`. Tile, like `tile-core` and the Accessibility
/// API, uses a **top-left** origin where y grows *downwards*.
///
/// The flip mirrors the rectangle's vertical extent around the top edge of the
/// primary display:
///
/// ```text
/// flipped_y = primary_height - (rect.y + rect.height)
/// ```
///
/// `primary_height` must be the `maxY` of `NSScreen.screens[0].frame`. Since
/// that screen's origin is `(0, 0)` in AppKit space, its `maxY` equals its
/// height. Displays positioned physically *above* the primary have AppKit y
/// values larger than `primary_height` and therefore end up with **negative**
/// y after flipping — which is correct, because in top-left space "above the
/// primary" means "smaller y".
///
/// Only the y coordinate changes; x, width and height are preserved. This makes
/// the function its own inverse for a fixed `primary_height`.
pub(crate) fn flip_rect(
    rect_in_appkit_space: tile_core::Rect,
    primary_height: f64,
) -> tile_core::Rect {
    tile_core::Rect::new(
        rect_in_appkit_space.x,
        primary_height - (rect_in_appkit_space.y + rect_in_appkit_space.height),
        rect_in_appkit_space.width,
        rect_in_appkit_space.height,
    )
}

/// Maps a [`tile_core::KeyCode`] to its Carbon virtual key code (`kVK_*`).
///
/// The match is exhaustive with no wildcard arm on purpose: adding a `KeyCode`
/// to `tile-core` must fail to compile here until it is mapped, rather than
/// silently registering the wrong physical key.
///
/// Values are the classic Carbon `kVK_*` constants from
/// `HIToolbox/Events.h`. Note two easy-to-get-wrong cases:
///   * [`KeyCode::Backspace`](tile_core::KeyCode::Backspace) is `kVK_Delete`
///     (`0x33`) — the key labelled "delete" on a Mac keyboard.
///   * [`KeyCode::Delete`](tile_core::KeyCode::Delete) is `kVK_ForwardDelete`
///     (`0x75`) — forward delete.
pub(crate) fn carbon_key_code(key: tile_core::KeyCode) -> u32 {
    use tile_core::KeyCode;
    match key {
        KeyCode::Left => 0x7B,      // kVK_LeftArrow
        KeyCode::Right => 0x7C,     // kVK_RightArrow
        KeyCode::Down => 0x7D,      // kVK_DownArrow
        KeyCode::Up => 0x7E,        // kVK_UpArrow
        KeyCode::Enter => 0x24,     // kVK_Return
        KeyCode::Space => 0x31,     // kVK_Space
        KeyCode::Backspace => 0x33, // kVK_Delete (Backspace)
        KeyCode::Delete => 0x75,    // kVK_ForwardDelete
        KeyCode::Escape => 0x35,    // kVK_Escape
        KeyCode::Numpad0 => 0x52,   // kVK_ANSI_Keypad0
        KeyCode::Numpad1 => 0x53,   // kVK_ANSI_Keypad1
        KeyCode::Numpad2 => 0x54,   // kVK_ANSI_Keypad2
        KeyCode::Numpad3 => 0x55,   // kVK_ANSI_Keypad3
        KeyCode::Numpad4 => 0x56,   // kVK_ANSI_Keypad4
        KeyCode::Numpad5 => 0x57,   // kVK_ANSI_Keypad5
        KeyCode::Numpad6 => 0x58,   // kVK_ANSI_Keypad6
        KeyCode::Numpad7 => 0x59,   // kVK_ANSI_Keypad7
        KeyCode::Numpad8 => 0x5B,   // kVK_ANSI_Keypad8
        KeyCode::Numpad9 => 0x5C,   // kVK_ANSI_Keypad9
        KeyCode::C => 0x08,         // kVK_ANSI_C
        KeyCode::F => 0x03,         // kVK_ANSI_F
        KeyCode::M => 0x2E,         // kVK_ANSI_M
    }
}

/// Carbon "hot key" modifier mask bits, from `HIToolbox/Events.h`.
///
/// These are the classic Carbon values, **not** the `NSEvent.ModifierFlags`
/// values. `RegisterEventHotKey` expects exactly these.
mod carbon_mod {
    pub const CMD: u32 = 0x0100; // cmdKey
    pub const SHIFT: u32 = 0x0200; // shiftKey
    pub const OPTION: u32 = 0x0800; // optionKey
    pub const CONTROL: u32 = 0x1000; // controlKey
}

/// Maps Tile's modifier bitmask to a Carbon hot-key modifier mask.
///
/// On macOS [`Modifiers::META`](tile_core::Modifiers::META) is Command and
/// [`Modifiers::ALT`](tile_core::Modifiers::ALT) is Option.
pub(crate) fn carbon_modifiers(mods: tile_core::Modifiers) -> u32 {
    use tile_core::Modifiers;
    let mut mask = 0u32;
    if mods.contains(Modifiers::META) {
        mask |= carbon_mod::CMD;
    }
    if mods.contains(Modifiers::SHIFT) {
        mask |= carbon_mod::SHIFT;
    }
    if mods.contains(Modifiers::ALT) {
        mask |= carbon_mod::OPTION;
    }
    if mods.contains(Modifiers::CONTROL) {
        mask |= carbon_mod::CONTROL;
    }
    mask
}

#[cfg(test)]
mod pure_tests {
    use super::{carbon_key_code, carbon_modifiers, flip_rect};
    use tile_core::{KeyCode, Modifiers, Rect};

    // ----- coordinate flip -------------------------------------------------

    #[test]
    fn primary_screen_flips_to_the_origin() {
        // Primary display: AppKit origin (0,0), 1920x1080. Its maxY is 1080.
        let primary = Rect::new(0.0, 0.0, 1920.0, 1080.0);
        assert_eq!(
            flip_rect(primary, 1080.0),
            Rect::new(0.0, 0.0, 1920.0, 1080.0)
        );
    }

    #[test]
    fn secondary_to_the_right_keeps_its_top_at_zero() {
        // Same height, bottoms aligned, sitting to the right of the primary.
        let right = Rect::new(1920.0, 0.0, 1920.0, 1080.0);
        assert_eq!(
            flip_rect(right, 1080.0),
            Rect::new(1920.0, 0.0, 1920.0, 1080.0)
        );
    }

    #[test]
    fn monitor_physically_above_the_primary_goes_negative() {
        // In AppKit, "above" means a larger y. A 1920x1080 display stacked
        // directly on top of the primary has AppKit origin (0, 1080).
        let above = Rect::new(0.0, 1080.0, 1920.0, 1080.0);
        // In top-left space it lives entirely above y=0.
        assert_eq!(
            flip_rect(above, 1080.0),
            Rect::new(0.0, -1080.0, 1920.0, 1080.0)
        );
    }

    #[test]
    fn monitor_physically_below_the_primary_goes_positive() {
        // "Below" in AppKit means a smaller (negative) y.
        let below = Rect::new(0.0, -1080.0, 1920.0, 1080.0);
        assert_eq!(
            flip_rect(below, 1080.0),
            Rect::new(0.0, 1080.0, 1920.0, 1080.0)
        );
    }

    #[test]
    fn taller_secondary_with_aligned_bottoms_flips_above_the_top_edge() {
        // A 1920x1200 panel to the right, bottom-aligned with the 1080 primary
        // (AppKit origin y=0). Its top sticks up 120px above the primary's top,
        // so after flipping its y is -120.
        let taller = Rect::new(1920.0, 0.0, 1920.0, 1200.0);
        assert_eq!(
            flip_rect(taller, 1080.0),
            Rect::new(1920.0, -120.0, 1920.0, 1200.0)
        );
    }

    #[test]
    fn work_area_flip_matches_frame_flip_for_the_menu_bar() {
        // visibleFrame excludes the 25px menu bar at the top and a 60px Dock at
        // the bottom of the primary: AppKit origin (0, 60), height 1080-25-60.
        let visible = Rect::new(0.0, 60.0, 1920.0, 995.0);
        // Flipped: y = 1080 - (60 + 995) = 25 (just below the menu bar).
        assert_eq!(
            flip_rect(visible, 1080.0),
            Rect::new(0.0, 25.0, 1920.0, 995.0)
        );
    }

    #[test]
    fn flip_is_its_own_inverse() {
        let primary_height = 1440.0;
        for rect in [
            Rect::new(10.0, 20.0, 300.0, 400.0),
            Rect::new(-500.0, 1440.0, 1920.0, 1080.0),
            Rect::new(2560.0, -200.0, 1280.0, 800.0),
        ] {
            let there = flip_rect(rect, primary_height);
            let back = flip_rect(there, primary_height);
            assert!(
                back.approx_eq(&rect, 1e-9),
                "flip must round-trip: {rect:?} -> {there:?} -> {back:?}"
            );
        }
    }

    // ----- Carbon key codes ------------------------------------------------

    #[test]
    fn key_codes_have_the_expected_carbon_values() {
        assert_eq!(carbon_key_code(KeyCode::Left), 0x7B);
        assert_eq!(carbon_key_code(KeyCode::Right), 0x7C);
        assert_eq!(carbon_key_code(KeyCode::Down), 0x7D);
        assert_eq!(carbon_key_code(KeyCode::Up), 0x7E);
        assert_eq!(carbon_key_code(KeyCode::Enter), 0x24);
        assert_eq!(carbon_key_code(KeyCode::Space), 0x31);
        // Backspace is kVK_Delete; Delete is kVK_ForwardDelete. Do not swap.
        assert_eq!(carbon_key_code(KeyCode::Backspace), 0x33);
        assert_eq!(carbon_key_code(KeyCode::Delete), 0x75);
        assert_eq!(carbon_key_code(KeyCode::Escape), 0x35);
        assert_eq!(carbon_key_code(KeyCode::C), 0x08);
        assert_eq!(carbon_key_code(KeyCode::F), 0x03);
        assert_eq!(carbon_key_code(KeyCode::M), 0x2E);
    }

    #[test]
    fn keypad_codes_are_contiguous_except_for_the_documented_gap() {
        // kVK_ANSI_Keypad0..7 are 0x52..0x59, then 8 and 9 jump to 0x5B, 0x5C
        // (0x5A is unused). This is a genuine quirk of the Carbon table.
        assert_eq!(carbon_key_code(KeyCode::Numpad0), 0x52);
        assert_eq!(carbon_key_code(KeyCode::Numpad1), 0x53);
        assert_eq!(carbon_key_code(KeyCode::Numpad7), 0x59);
        assert_eq!(carbon_key_code(KeyCode::Numpad8), 0x5B);
        assert_eq!(carbon_key_code(KeyCode::Numpad9), 0x5C);
    }

    #[test]
    fn every_key_code_maps_to_a_unique_value() {
        // A duplicated virtual key code is a silent, nasty bug: two different
        // Tile keys would fire the same physical shortcut.
        let mut seen = std::collections::HashSet::new();
        for key in KeyCode::ALL {
            let code = carbon_key_code(key);
            assert!(
                seen.insert(code),
                "duplicate virtual key code {code:#x} for {key:?}"
            );
        }
        assert_eq!(seen.len(), KeyCode::ALL.len());
    }

    // ----- Carbon modifiers ------------------------------------------------

    #[test]
    fn individual_modifiers_map_to_their_carbon_bits() {
        assert_eq!(carbon_modifiers(Modifiers::META), 0x0100); // cmdKey
        assert_eq!(carbon_modifiers(Modifiers::SHIFT), 0x0200); // shiftKey
        assert_eq!(carbon_modifiers(Modifiers::ALT), 0x0800); // optionKey
        assert_eq!(carbon_modifiers(Modifiers::CONTROL), 0x1000); // controlKey
        assert_eq!(carbon_modifiers(Modifiers::NONE), 0x0000);
    }

    #[test]
    fn combined_modifiers_are_ored_together() {
        let mods = Modifiers::CONTROL | Modifiers::ALT | Modifiers::META;
        assert_eq!(carbon_modifiers(mods), 0x1000 | 0x0800 | 0x0100);
    }

    #[test]
    fn modifier_bits_do_not_overlap() {
        let all = [
            Modifiers::META,
            Modifiers::SHIFT,
            Modifiers::ALT,
            Modifiers::CONTROL,
        ];
        let mut combined = 0u32;
        for m in all {
            let bit = carbon_modifiers(m);
            assert_eq!(combined & bit, 0, "modifier bits must be disjoint");
            combined |= bit;
        }
    }
}
