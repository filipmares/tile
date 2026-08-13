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
/// Returns `None` for the handful of keys macOS has no virtual key code for
/// (`F21`–`F24`: Carbon's table stops at `kVK_F20`). The caller reports those
/// as a per-binding failure rather than registering a wrong physical key.
///
/// The match is exhaustive with no wildcard arm on purpose: adding a `KeyCode`
/// to `tile-core` must fail to compile here until it is mapped, rather than
/// silently registering the wrong physical key.
///
/// Values are the classic Carbon `kVK_*` constants from
/// `HIToolbox/Events.h`. Several are easy to get wrong:
///   * [`KeyCode::Backspace`](tile_core::KeyCode::Backspace) is `kVK_Delete`
///     (`0x33`) — the key labelled "delete" on a Mac keyboard.
///   * [`KeyCode::Delete`](tile_core::KeyCode::Delete) is `kVK_ForwardDelete`
///     (`0x75`) — forward delete.
///   * [`KeyCode::Insert`](tile_core::KeyCode::Insert) is `kVK_Help` (`0x72`):
///     Apple keyboards put Help where PC keyboards put Insert, and a USB PC
///     keyboard's Insert reports that code.
///   * The letter and function-key blocks are **not** in alphabetical or
///     numerical order — they follow the original Apple Extended Keyboard
///     wiring (`A` is `0x00`, `S` is `0x01`, `F5` is `0x60`, `F1` is `0x7A`).
pub(crate) fn carbon_key_code(key: tile_core::KeyCode) -> Option<u32> {
    use tile_core::KeyCode;
    let code = match key {
        // --- navigation and editing ---
        KeyCode::Left => 0x7B,      // kVK_LeftArrow
        KeyCode::Right => 0x7C,     // kVK_RightArrow
        KeyCode::Down => 0x7D,      // kVK_DownArrow
        KeyCode::Up => 0x7E,        // kVK_UpArrow
        KeyCode::Enter => 0x24,     // kVK_Return
        KeyCode::Space => 0x31,     // kVK_Space
        KeyCode::Backspace => 0x33, // kVK_Delete (Backspace)
        KeyCode::Delete => 0x75,    // kVK_ForwardDelete
        KeyCode::Escape => 0x35,    // kVK_Escape
        KeyCode::Tab => 0x30,       // kVK_Tab
        KeyCode::Insert => 0x72,    // kVK_Help (Insert on a PC keyboard)
        KeyCode::Home => 0x73,      // kVK_Home
        KeyCode::End => 0x77,       // kVK_End
        KeyCode::PageUp => 0x74,    // kVK_PageUp
        KeyCode::PageDown => 0x79,  // kVK_PageDown

        // --- letters (Apple Extended Keyboard order, not alphabetical) ---
        KeyCode::A => 0x00, // kVK_ANSI_A
        KeyCode::B => 0x0B, // kVK_ANSI_B
        KeyCode::C => 0x08, // kVK_ANSI_C
        KeyCode::D => 0x02, // kVK_ANSI_D
        KeyCode::E => 0x0E, // kVK_ANSI_E
        KeyCode::F => 0x03, // kVK_ANSI_F
        KeyCode::G => 0x05, // kVK_ANSI_G
        KeyCode::H => 0x04, // kVK_ANSI_H
        KeyCode::I => 0x22, // kVK_ANSI_I
        KeyCode::J => 0x26, // kVK_ANSI_J
        KeyCode::K => 0x28, // kVK_ANSI_K
        KeyCode::L => 0x25, // kVK_ANSI_L
        KeyCode::M => 0x2E, // kVK_ANSI_M
        KeyCode::N => 0x2D, // kVK_ANSI_N
        KeyCode::O => 0x1F, // kVK_ANSI_O
        KeyCode::P => 0x23, // kVK_ANSI_P
        KeyCode::Q => 0x0C, // kVK_ANSI_Q
        KeyCode::R => 0x0F, // kVK_ANSI_R
        KeyCode::S => 0x01, // kVK_ANSI_S
        KeyCode::T => 0x11, // kVK_ANSI_T
        KeyCode::U => 0x20, // kVK_ANSI_U
        KeyCode::V => 0x09, // kVK_ANSI_V
        KeyCode::W => 0x0D, // kVK_ANSI_W
        KeyCode::X => 0x07, // kVK_ANSI_X
        KeyCode::Y => 0x10, // kVK_ANSI_Y
        KeyCode::Z => 0x06, // kVK_ANSI_Z

        // --- top-row digits (5/6 and 7/8/9 are famously out of order) ---
        KeyCode::Digit0 => 0x1D, // kVK_ANSI_0
        KeyCode::Digit1 => 0x12, // kVK_ANSI_1
        KeyCode::Digit2 => 0x13, // kVK_ANSI_2
        KeyCode::Digit3 => 0x14, // kVK_ANSI_3
        KeyCode::Digit4 => 0x15, // kVK_ANSI_4
        KeyCode::Digit5 => 0x17, // kVK_ANSI_5
        KeyCode::Digit6 => 0x16, // kVK_ANSI_6
        KeyCode::Digit7 => 0x1A, // kVK_ANSI_7
        KeyCode::Digit8 => 0x1C, // kVK_ANSI_8
        KeyCode::Digit9 => 0x19, // kVK_ANSI_9

        // --- punctuation ---
        KeyCode::Backtick => 0x32,     // kVK_ANSI_Grave
        KeyCode::Minus => 0x1B,        // kVK_ANSI_Minus
        KeyCode::Equals => 0x18,       // kVK_ANSI_Equal
        KeyCode::LeftBracket => 0x21,  // kVK_ANSI_LeftBracket
        KeyCode::RightBracket => 0x1E, // kVK_ANSI_RightBracket
        KeyCode::Backslash => 0x2A,    // kVK_ANSI_Backslash
        KeyCode::Semicolon => 0x29,    // kVK_ANSI_Semicolon
        KeyCode::Quote => 0x27,        // kVK_ANSI_Quote
        KeyCode::Comma => 0x2B,        // kVK_ANSI_Comma
        KeyCode::Period => 0x2F,       // kVK_ANSI_Period
        KeyCode::Slash => 0x2C,        // kVK_ANSI_Slash

        // --- function keys (scattered; F1 is 0x7A, F5 is 0x60) ---
        KeyCode::F1 => 0x7A,  // kVK_F1
        KeyCode::F2 => 0x78,  // kVK_F2
        KeyCode::F3 => 0x63,  // kVK_F3
        KeyCode::F4 => 0x76,  // kVK_F4
        KeyCode::F5 => 0x60,  // kVK_F5
        KeyCode::F6 => 0x61,  // kVK_F6
        KeyCode::F7 => 0x62,  // kVK_F7
        KeyCode::F8 => 0x64,  // kVK_F8
        KeyCode::F9 => 0x65,  // kVK_F9
        KeyCode::F10 => 0x6D, // kVK_F10
        KeyCode::F11 => 0x67, // kVK_F11
        KeyCode::F12 => 0x6F, // kVK_F12
        KeyCode::F13 => 0x69, // kVK_F13
        KeyCode::F14 => 0x6B, // kVK_F14
        KeyCode::F15 => 0x71, // kVK_F15
        KeyCode::F16 => 0x6A, // kVK_F16
        KeyCode::F17 => 0x40, // kVK_F17
        KeyCode::F18 => 0x4F, // kVK_F18
        KeyCode::F19 => 0x50, // kVK_F19
        KeyCode::F20 => 0x5A, // kVK_F20
        // Carbon has no constants beyond kVK_F20.
        KeyCode::F21 | KeyCode::F22 | KeyCode::F23 | KeyCode::F24 => return None,

        // --- numeric keypad ---
        KeyCode::Numpad0 => 0x52,        // kVK_ANSI_Keypad0
        KeyCode::Numpad1 => 0x53,        // kVK_ANSI_Keypad1
        KeyCode::Numpad2 => 0x54,        // kVK_ANSI_Keypad2
        KeyCode::Numpad3 => 0x55,        // kVK_ANSI_Keypad3
        KeyCode::Numpad4 => 0x56,        // kVK_ANSI_Keypad4
        KeyCode::Numpad5 => 0x57,        // kVK_ANSI_Keypad5
        KeyCode::Numpad6 => 0x58,        // kVK_ANSI_Keypad6
        KeyCode::Numpad7 => 0x59,        // kVK_ANSI_Keypad7
        KeyCode::Numpad8 => 0x5B,        // kVK_ANSI_Keypad8
        KeyCode::Numpad9 => 0x5C,        // kVK_ANSI_Keypad9
        KeyCode::NumpadAdd => 0x45,      // kVK_ANSI_KeypadPlus
        KeyCode::NumpadSubtract => 0x4E, // kVK_ANSI_KeypadMinus
        KeyCode::NumpadMultiply => 0x43, // kVK_ANSI_KeypadMultiply
        KeyCode::NumpadDivide => 0x4B,   // kVK_ANSI_KeypadDivide
        KeyCode::NumpadDecimal => 0x41,  // kVK_ANSI_KeypadDecimal
        KeyCode::NumpadEnter => 0x4C,    // kVK_ANSI_KeypadEnter
    };
    Some(code)
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

/// One entry from the CoreGraphics on-screen window list, reduced to just the
/// fields Tile needs to decide whether a window is a normal, foreign window.
///
/// `CGWindowListCopyWindowInfo` returns entries in **front-to-back Z-order**,
/// so a `Vec<CgWindowInfo>` built by preserving that order can be scanned from
/// the front exactly like Windows' `EnumWindows` walk.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct CgWindowInfo {
    /// `kCGWindowOwnerPID`: the process that owns the window.
    pub pid: i64,
    /// `kCGWindowLayer`: 0 for ordinary application windows; non-zero for the
    /// menu bar, Dock, panels, overlays, etc.
    pub layer: i64,
    /// `kCGWindowNumber`: the `CGWindowID`, used to find the matching
    /// `AXUIElement` on the owning application.
    pub window_id: u32,
}

/// Filters a front-to-back CoreGraphics window list down to the windows Tile
/// could move: ordinary windows (`layer == 0`) that belong to some *other*
/// process, preserving Z-order so the caller can take the front-most first.
///
/// This is the macOS analogue of the Windows `EnumWindows` scan: when the user
/// clicks Tile's menu-bar item, Tile becomes frontmost and the AX "focused
/// application" is Tile itself, so the caller falls back to this list and picks
/// the window that was active immediately before the menu opened. Excluding our
/// own `pid` is what stops Tile from picking up its own status-bar window.
///
/// Kept pure (no CoreGraphics types) so it is unit-testable on any host.
pub(crate) fn foreign_normal_windows(windows: &[CgWindowInfo], own_pid: i64) -> Vec<CgWindowInfo> {
    windows
        .iter()
        .copied()
        .filter(|w| w.pid != own_pid && w.layer == 0)
        .collect()
}

#[cfg(test)]
mod pure_tests {
    use super::{
        carbon_key_code, carbon_modifiers, flip_rect, foreign_normal_windows, CgWindowInfo,
    };
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
        assert_eq!(carbon_key_code(KeyCode::Left), Some(0x7B));
        assert_eq!(carbon_key_code(KeyCode::Right), Some(0x7C));
        assert_eq!(carbon_key_code(KeyCode::Down), Some(0x7D));
        assert_eq!(carbon_key_code(KeyCode::Up), Some(0x7E));
        assert_eq!(carbon_key_code(KeyCode::Enter), Some(0x24));
        assert_eq!(carbon_key_code(KeyCode::Space), Some(0x31));
        // Backspace is kVK_Delete; Delete is kVK_ForwardDelete. Do not swap.
        assert_eq!(carbon_key_code(KeyCode::Backspace), Some(0x33));
        assert_eq!(carbon_key_code(KeyCode::Delete), Some(0x75));
        assert_eq!(carbon_key_code(KeyCode::Escape), Some(0x35));
        assert_eq!(carbon_key_code(KeyCode::C), Some(0x08));
        assert_eq!(carbon_key_code(KeyCode::F), Some(0x03));
        assert_eq!(carbon_key_code(KeyCode::M), Some(0x2E));
    }

    #[test]
    fn letter_codes_follow_the_apple_extended_keyboard_layout() {
        // The letter block is wired in physical order, so it is *not*
        // alphabetical. These are the values that catch a lazy `0x00 + index`.
        assert_eq!(carbon_key_code(KeyCode::A), Some(0x00));
        assert_eq!(carbon_key_code(KeyCode::S), Some(0x01));
        assert_eq!(carbon_key_code(KeyCode::D), Some(0x02));
        assert_eq!(carbon_key_code(KeyCode::H), Some(0x04));
        assert_eq!(carbon_key_code(KeyCode::G), Some(0x05));
        assert_eq!(carbon_key_code(KeyCode::Z), Some(0x06));
        assert_eq!(carbon_key_code(KeyCode::Q), Some(0x0C));
        assert_eq!(carbon_key_code(KeyCode::Y), Some(0x10));
        assert_eq!(carbon_key_code(KeyCode::T), Some(0x11));
    }

    #[test]
    fn digit_codes_include_the_transposed_pairs() {
        // 5 and 6 are swapped relative to intuition, as are 7/8/9.
        assert_eq!(carbon_key_code(KeyCode::Digit1), Some(0x12));
        assert_eq!(carbon_key_code(KeyCode::Digit5), Some(0x17));
        assert_eq!(carbon_key_code(KeyCode::Digit6), Some(0x16));
        assert_eq!(carbon_key_code(KeyCode::Digit7), Some(0x1A));
        assert_eq!(carbon_key_code(KeyCode::Digit8), Some(0x1C));
        assert_eq!(carbon_key_code(KeyCode::Digit9), Some(0x19));
        assert_eq!(carbon_key_code(KeyCode::Digit0), Some(0x1D));
    }

    #[test]
    fn function_key_codes_are_scattered_and_stop_at_f20() {
        assert_eq!(carbon_key_code(KeyCode::F1), Some(0x7A));
        assert_eq!(carbon_key_code(KeyCode::F5), Some(0x60));
        assert_eq!(carbon_key_code(KeyCode::F10), Some(0x6D));
        assert_eq!(carbon_key_code(KeyCode::F12), Some(0x6F));
        assert_eq!(carbon_key_code(KeyCode::F17), Some(0x40));
        assert_eq!(carbon_key_code(KeyCode::F20), Some(0x5A));
        // Carbon has no kVK_F21..kVK_F24, so these are honestly unmappable.
        for key in [KeyCode::F21, KeyCode::F22, KeyCode::F23, KeyCode::F24] {
            assert_eq!(carbon_key_code(key), None, "{key:?} must be unmappable");
        }
    }

    #[test]
    fn punctuation_and_extra_navigation_codes_are_correct() {
        assert_eq!(carbon_key_code(KeyCode::Backtick), Some(0x32));
        assert_eq!(carbon_key_code(KeyCode::Minus), Some(0x1B));
        assert_eq!(carbon_key_code(KeyCode::Equals), Some(0x18));
        assert_eq!(carbon_key_code(KeyCode::LeftBracket), Some(0x21));
        assert_eq!(carbon_key_code(KeyCode::RightBracket), Some(0x1E));
        assert_eq!(carbon_key_code(KeyCode::Backslash), Some(0x2A));
        assert_eq!(carbon_key_code(KeyCode::Semicolon), Some(0x29));
        assert_eq!(carbon_key_code(KeyCode::Quote), Some(0x27));
        assert_eq!(carbon_key_code(KeyCode::Comma), Some(0x2B));
        assert_eq!(carbon_key_code(KeyCode::Period), Some(0x2F));
        assert_eq!(carbon_key_code(KeyCode::Slash), Some(0x2C));
        assert_eq!(carbon_key_code(KeyCode::Tab), Some(0x30));
        assert_eq!(carbon_key_code(KeyCode::Home), Some(0x73));
        assert_eq!(carbon_key_code(KeyCode::End), Some(0x77));
        assert_eq!(carbon_key_code(KeyCode::PageUp), Some(0x74));
        assert_eq!(carbon_key_code(KeyCode::PageDown), Some(0x79));
        // Insert is kVK_Help on Apple hardware.
        assert_eq!(carbon_key_code(KeyCode::Insert), Some(0x72));
    }

    #[test]
    fn keypad_codes_are_contiguous_except_for_the_documented_gap() {
        // kVK_ANSI_Keypad0..7 are 0x52..0x59, then 8 and 9 jump to 0x5B, 0x5C
        // (0x5A is kVK_F20). This is a genuine quirk of the Carbon table.
        assert_eq!(carbon_key_code(KeyCode::Numpad0), Some(0x52));
        assert_eq!(carbon_key_code(KeyCode::Numpad1), Some(0x53));
        assert_eq!(carbon_key_code(KeyCode::Numpad7), Some(0x59));
        assert_eq!(carbon_key_code(KeyCode::Numpad8), Some(0x5B));
        assert_eq!(carbon_key_code(KeyCode::Numpad9), Some(0x5C));
        assert_eq!(carbon_key_code(KeyCode::NumpadDecimal), Some(0x41));
        assert_eq!(carbon_key_code(KeyCode::NumpadMultiply), Some(0x43));
        assert_eq!(carbon_key_code(KeyCode::NumpadAdd), Some(0x45));
        assert_eq!(carbon_key_code(KeyCode::NumpadDivide), Some(0x4B));
        assert_eq!(carbon_key_code(KeyCode::NumpadEnter), Some(0x4C));
        assert_eq!(carbon_key_code(KeyCode::NumpadSubtract), Some(0x4E));
    }

    #[test]
    fn every_key_code_maps_to_a_unique_value() {
        // A duplicated virtual key code is a silent, nasty bug: two different
        // Tile keys would fire the same physical shortcut.
        let mut seen = std::collections::HashSet::new();
        let mut mapped = 0usize;
        for key in KeyCode::ALL {
            let Some(code) = carbon_key_code(key) else {
                continue;
            };
            mapped += 1;
            assert!(
                seen.insert(code),
                "duplicate virtual key code {code:#x} for {key:?}"
            );
        }
        assert_eq!(seen.len(), mapped);
        // Only F21..F24 are allowed to be unmapped.
        assert_eq!(mapped, KeyCode::COUNT - 4);
    }

    #[test]
    fn unmapped_keys_are_the_only_ones_without_a_code() {
        let unmapped: Vec<KeyCode> = KeyCode::ALL
            .iter()
            .copied()
            .filter(|&k| carbon_key_code(k).is_none())
            .collect();
        assert_eq!(
            unmapped,
            vec![KeyCode::F21, KeyCode::F22, KeyCode::F23, KeyCode::F24]
        );
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

    // ----- CoreGraphics window-list filtering ------------------------------

    fn win(pid: i64, layer: i64, window_id: u32) -> CgWindowInfo {
        CgWindowInfo {
            pid,
            layer,
            window_id,
        }
    }

    #[test]
    fn skips_our_own_windows() {
        // Tile (pid 42) is frontmost because its menu-bar item was clicked, so
        // its own windows head the list. They must be skipped in favour of the
        // user's window behind them.
        let own = 42;
        let list = [win(own, 0, 1), win(own, 0, 2), win(99, 0, 3)];
        let found = foreign_normal_windows(&list, own);
        assert_eq!(found, vec![win(99, 0, 3)]);
    }

    #[test]
    fn skips_non_zero_layers() {
        // The menu bar and Dock live on non-zero layers and must never be
        // treated as movable windows, even when owned by another process.
        let own = 42;
        let list = [win(50, 25, 1), win(50, 20, 2), win(50, 0, 3)];
        let found = foreign_normal_windows(&list, own);
        assert_eq!(found, vec![win(50, 0, 3)]);
    }

    #[test]
    fn preserves_front_to_back_order() {
        // The list must stay in Z-order so the caller can take the front-most
        // (most recently active) foreign window first.
        let own = 42;
        let list = [win(7, 0, 10), win(8, 0, 11), win(9, 0, 12)];
        let found = foreign_normal_windows(&list, own);
        assert_eq!(found, vec![win(7, 0, 10), win(8, 0, 11), win(9, 0, 12)]);
    }

    #[test]
    fn empty_when_only_our_windows_exist() {
        let own = 42;
        let list = [win(own, 0, 1), win(own, 0, 2)];
        assert!(foreign_normal_windows(&list, own).is_empty());
    }

    #[test]
    fn empty_list_yields_empty() {
        assert!(foreign_normal_windows(&[], 42).is_empty());
    }
}
