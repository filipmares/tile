//! Global hotkeys on Windows via a low-level keyboard hook.
//!
//! We deliberately use `WH_KEYBOARD_LL` rather than `RegisterHotKey`: the shell
//! already owns `Win+Left/Right/Up/Down` for Aero Snap, so `RegisterHotKey`
//! fails with `ERROR_HOTKEY_ALREADY_REGISTERED` for exactly the bindings Tile
//! most wants. A low-level hook sees the keystrokes first and can *swallow*
//! them (return 1) so Aero Snap never fires.
//!
//! A low-level hook is only serviced while its owning thread pumps messages, so
//! the hook lives on a dedicated thread with a `GetMessageW` loop. The hook
//! callback is `extern "system"` and cannot borrow `self`, so the binding table
//! and the action `Sender` live in a `static`. The callback does nothing but a
//! cheap table lookup and a non-blocking channel send — Windows silently drops
//! hooks whose callback exceeds `LowLevelHooksTimeout` (~300ms), so it must
//! never block, allocate heavily or do I/O.
//!
//! Note: a low-level hook cannot intercept input directed at an elevated
//! process or the secure desktop unless Tile itself runs elevated.

use std::sync::mpsc::Sender;
use std::sync::{Mutex, OnceLock};
use std::thread::{self, JoinHandle};

use tile_core::{Hotkey, KeyCode, Modifiers, WindowAction};

use windows::core::PCWSTR;
use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, WPARAM};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::System::Threading::GetCurrentThreadId;
use windows::Win32::UI::Input::KeyboardAndMouse::{
    GetAsyncKeyState, SendInput, INPUT, INPUT_0, INPUT_KEYBOARD, KEYBDINPUT, KEYBD_EVENT_FLAGS,
    KEYEVENTF_KEYUP, VIRTUAL_KEY, VK_ADD, VK_BACK, VK_CONTROL, VK_DECIMAL, VK_DELETE, VK_DIVIDE,
    VK_DOWN, VK_END, VK_ESCAPE, VK_F1, VK_F10, VK_F11, VK_F12, VK_F13, VK_F14, VK_F15, VK_F16,
    VK_F17, VK_F18, VK_F19, VK_F2, VK_F20, VK_F21, VK_F22, VK_F23, VK_F24, VK_F3, VK_F4, VK_F5,
    VK_F6, VK_F7, VK_F8, VK_F9, VK_HOME, VK_INSERT, VK_LEFT, VK_LWIN, VK_MENU, VK_MULTIPLY,
    VK_NEXT, VK_NONAME, VK_NUMPAD0, VK_NUMPAD1, VK_NUMPAD2, VK_NUMPAD3, VK_NUMPAD4, VK_NUMPAD5,
    VK_NUMPAD6, VK_NUMPAD7, VK_NUMPAD8, VK_NUMPAD9, VK_OEM_1, VK_OEM_2, VK_OEM_3, VK_OEM_4,
    VK_OEM_5, VK_OEM_6, VK_OEM_7, VK_OEM_COMMA, VK_OEM_MINUS, VK_OEM_PERIOD, VK_OEM_PLUS, VK_PRIOR,
    VK_RETURN, VK_RIGHT, VK_RWIN, VK_SHIFT, VK_SPACE, VK_SUBTRACT, VK_TAB, VK_UP,
};
use windows::Win32::UI::WindowsAndMessaging::{
    CallNextHookEx, DispatchMessageW, GetMessageW, PostThreadMessageW, SetWindowsHookExW,
    TranslateMessage, UnhookWindowsHookEx, HC_ACTION, HHOOK, KBDLLHOOKSTRUCT, LLKHF_EXTENDED, MSG,
    WH_KEYBOARD_LL, WM_KEYDOWN, WM_QUIT, WM_SYSKEYDOWN,
};

use crate::{HotkeyBackend, HotkeyFailure, PlatformError, Result};

/// One resolved binding, in the form the hook callback compares against.
#[derive(Debug, Clone, Copy, PartialEq)]
struct Binding {
    vk: u16,
    /// Required state of the keystroke's *extended* flag, or `None` when the
    /// flag does not matter.
    ///
    /// Windows has no virtual key of its own for the numeric keypad's Enter:
    /// it reports `VK_RETURN` just like the main Enter, and the only thing
    /// telling them apart is `LLKHF_EXTENDED`. Without this field the two keys
    /// would be indistinguishable and one binding would fire for both.
    extended: Option<bool>,
    mods: Modifiers,
    action: WindowAction,
}

/// Shared state read by the hook callback and written by `apply`/`shutdown`.
struct HookState {
    sender: Sender<WindowAction>,
    bindings: Vec<Binding>,
}

/// The hook callback cannot capture state, so it reaches the binding table and
/// action channel through this global. A `Mutex` keeps the critical section
/// tiny; the callback never holds it across a `SendInput`.
static HOOK_STATE: OnceLock<Mutex<Option<HookState>>> = OnceLock::new();

/// Marker stamped into `dwExtraInfo` of the keystrokes we inject ourselves so
/// the hook can recognise and ignore them (and so it never recurses).
const INJECTED_TAG: usize = 0x54_49_4C_45; // "TILE"

pub struct WindowsHotkeyBackend {
    thread: Option<JoinHandle<()>>,
    thread_id: u32,
    shutdown_done: bool,
}

impl WindowsHotkeyBackend {
    pub fn new(events: Sender<WindowAction>) -> Result<Self> {
        let state = HOOK_STATE.get_or_init(|| Mutex::new(None));
        {
            let mut guard = state
                .lock()
                .map_err(|_| PlatformError::os("hotkey", "hook state mutex poisoned"))?;
            *guard = Some(HookState {
                sender: events,
                bindings: Vec::new(),
            });
        }

        // The hook thread reports back its thread id (needed to post WM_QUIT)
        // once the hook is installed, or an error if installation failed.
        let (ready_tx, ready_rx) = std::sync::mpsc::channel::<Result<u32>>();
        let handle = thread::Builder::new()
            .name("tile-hotkey-hook".to_string())
            .spawn(move || hook_thread_main(ready_tx))
            .map_err(|e| PlatformError::os("spawn hook thread", e.to_string()))?;

        let thread_id = match ready_rx.recv() {
            Ok(Ok(id)) => id,
            Ok(Err(e)) => {
                let _ = handle.join();
                return Err(e);
            }
            Err(_) => {
                let _ = handle.join();
                return Err(PlatformError::os(
                    "hook thread",
                    "thread exited before installing the keyboard hook",
                ));
            }
        };

        Ok(Self {
            thread: Some(handle),
            thread_id,
            shutdown_done: false,
        })
    }
}

impl HotkeyBackend for WindowsHotkeyBackend {
    fn apply(&mut self, bindings: &[(Hotkey, WindowAction)]) -> Result<Vec<HotkeyFailure>> {
        // Every `KeyCode` maps to a virtual key (see `keycode_to_vk`, which is
        // exhaustive), so there is nothing the hook cannot represent and no
        // per-binding failures to report.
        let table: Vec<Binding> = bindings
            .iter()
            .map(|(hk, action)| Binding {
                vk: keycode_to_vk(hk.key).0,
                extended: keycode_extended(hk.key),
                mods: hk.modifiers,
                action: *action,
            })
            .collect();

        if let Some(state) = HOOK_STATE.get() {
            if let Ok(mut guard) = state.lock() {
                if let Some(hs) = guard.as_mut() {
                    hs.bindings = table;
                }
            }
        }
        Ok(Vec::new())
    }

    fn shutdown(&mut self) {
        // Idempotent: guard against a second call (e.g. explicit shutdown then
        // Drop).
        if self.shutdown_done {
            return;
        }
        self.shutdown_done = true;

        if self.thread_id != 0 {
            // SAFETY: posting WM_QUIT to our own hook thread's queue; the
            // wparam/lparam are inert. Failure only means the thread already
            // exited, which we tolerate.
            unsafe {
                let _ = PostThreadMessageW(self.thread_id, WM_QUIT, WPARAM(0), LPARAM(0));
            }
        }

        // The actual `UnhookWindowsHookEx` runs on the hook thread as it leaves
        // its message loop: `HHOOK` is a non-Send raw handle, so unhooking where
        // it was created avoids smuggling it across threads. Joining here waits
        // for that unhook to complete.
        if let Some(handle) = self.thread.take() {
            let _ = handle.join();
        }

        if let Some(state) = HOOK_STATE.get() {
            if let Ok(mut guard) = state.lock() {
                *guard = None;
            }
        }
    }
}

impl Drop for WindowsHotkeyBackend {
    fn drop(&mut self) {
        self.shutdown();
    }
}

/// Entry point for the dedicated hook thread: installs the hook, reports
/// readiness, then pumps messages until WM_QUIT.
fn hook_thread_main(ready: Sender<Result<u32>>) {
    // SAFETY: standard hook-installation sequence. `low_level_keyboard_proc` is
    // a valid `extern "system"` callback; `hmod` is this module's instance
    // handle (required for a WH_KEYBOARD_LL hook); thread id 0 makes it global.
    unsafe {
        let hmod = match GetModuleHandleW(PCWSTR(std::ptr::null())) {
            Ok(h) => h,
            Err(e) => {
                let _ = ready.send(Err(PlatformError::os("GetModuleHandleW", e.message())));
                return;
            }
        };

        let hook = match SetWindowsHookExW(
            WH_KEYBOARD_LL,
            Some(low_level_keyboard_proc),
            Some(hmod.into()),
            0,
        ) {
            Ok(h) => h,
            Err(e) => {
                let _ = ready.send(Err(PlatformError::HotkeyRegistration {
                    hotkey: "WH_KEYBOARD_LL".to_string(),
                    reason: e.message(),
                }));
                return;
            }
        };

        let _ = ready.send(Ok(GetCurrentThreadId()));

        // Message loop. A low-level hook is only dispatched while this thread
        // pumps messages; WM_QUIT (posted by `shutdown`) makes GetMessageW
        // return 0 and ends the loop.
        let mut msg: MSG = std::mem::zeroed();
        loop {
            let result = GetMessageW(&mut msg, Some(HWND(std::ptr::null_mut())), 0, 0).0;
            if result == 0 || result == -1 {
                // 0 = WM_QUIT, -1 = error; either way we stop.
                break;
            }
            let _ = TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }

        let _ = UnhookWindowsHookEx(hook);
    }
}

/// The low-level keyboard hook. Runs on the hook thread for every key event.
///
/// # Safety
/// Called by the OS with a valid `KBDLLHOOKSTRUCT` pointer in `lparam` when
/// `code == HC_ACTION`. Must stay fast and non-blocking (see module docs).
unsafe extern "system" fn low_level_keyboard_proc(
    code: i32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    if code == HC_ACTION as i32 {
        let message = wparam.0 as u32;
        // Alt-modified keys arrive as WM_SYSKEYDOWN, everything else as
        // WM_KEYDOWN; we care about both.
        if message == WM_KEYDOWN || message == WM_SYSKEYDOWN {
            let kb = &*(lparam.0 as *const KBDLLHOOKSTRUCT);

            // Skip our own injected suppressor keystrokes to avoid recursion.
            if kb.dwExtraInfo != INJECTED_TAG {
                let vk = kb.vkCode as u16;

                // Never treat a modifier key press itself as a hotkey trigger.
                // Auto-repeat is intentionally allowed to pass through as
                // repeated triggers: re-applying a tiling action is idempotent
                // (the engine reports "already in position"), so holding a key
                // cannot cause runaway behaviour and no de-bounce is needed.
                if !is_modifier_vk(vk) {
                    let mods = current_modifiers();
                    let extended = (kb.flags.0 & LLKHF_EXTENDED.0) != 0;
                    if dispatch(vk, extended, mods) {
                        // A Win-based combo was consumed. Without this, releasing
                        // the Win key would open the Start menu, because the shell
                        // opens Start on Win-*up* when no other key was seen as
                        // pressed with it. Injecting an inert keystroke makes the
                        // shell treat Win as a held modifier, not a tap.
                        if mods.contains(Modifiers::META) {
                            suppress_start_menu();
                        }
                        // Return 1 to swallow the event so Aero Snap never fires.
                        return LRESULT(1);
                    }
                }
            }
        }
    }

    // SAFETY: forwarding to the next hook with the OS-provided parameters is
    // always sound; the ignored `HHOOK` argument is documented as unused.
    CallNextHookEx(Some(HHOOK(std::ptr::null_mut())), code, wparam, lparam)
}

/// Looks up `(vk, extended, mods)` in the shared table and, on an exact match,
/// sends the bound action. Returns whether a binding matched (and should be
/// swallowed).
fn dispatch(vk: u16, extended: bool, mods: Modifiers) -> bool {
    let Some(state) = HOOK_STATE.get() else {
        return false;
    };
    let Ok(guard) = state.lock() else {
        return false;
    };
    let Some(hs) = guard.as_ref() else {
        return false;
    };
    match match_binding(&hs.bindings, vk, extended, mods) {
        Some(action) => {
            // Unbounded std channel: `send` never blocks, so this is safe inside
            // the hook's tight time budget.
            let _ = hs.sender.send(action);
            true
        }
        None => false,
    }
}

/// Pure binding lookup with **exact** modifier matching.
///
/// Exactness matters: a `Win+Left` binding must NOT fire while `Ctrl+Win+Left`
/// is held. Using a subset/`contains` check here is a classic bug that makes
/// unrelated chords steal each other's keys.
fn match_binding(
    bindings: &[Binding],
    vk: u16,
    extended: bool,
    mods: Modifiers,
) -> Option<WindowAction> {
    bindings
        .iter()
        .find(|b| b.vk == vk && b.mods == mods && b.extended.map_or(true, |want| want == extended))
        .map(|b| b.action)
}

/// Reads the current modifier state. Uses `GetAsyncKeyState` for the modifiers
/// only (never for the pressed key itself). The Win flag reflects the physical
/// left/right Win keys.
fn current_modifiers() -> Modifiers {
    let mut m = Modifiers::NONE;
    if key_down(VK_CONTROL) {
        m = m | Modifiers::CONTROL;
    }
    if key_down(VK_MENU) {
        m = m | Modifiers::ALT;
    }
    if key_down(VK_SHIFT) {
        m = m | Modifiers::SHIFT;
    }
    if key_down(VK_LWIN) || key_down(VK_RWIN) {
        m = m | Modifiers::META;
    }
    m
}

fn key_down(vk: VIRTUAL_KEY) -> bool {
    // SAFETY: `GetAsyncKeyState` is a pure, side-effect-free state query with no
    // pointer arguments. The high bit of the result means the key is down.
    (unsafe { GetAsyncKeyState(vk.0 as i32) } as u16 & 0x8000) != 0
}

/// True for any left/right/generic modifier virtual-key, which must never be
/// treated as a hotkey's main key.
fn is_modifier_vk(vk: u16) -> bool {
    matches!(
        vk,
        0x10 | 0x11 | 0x12 // VK_SHIFT, VK_CONTROL, VK_MENU (generic)
            | 0xA0 | 0xA1  // VK_LSHIFT, VK_RSHIFT
            | 0xA2 | 0xA3  // VK_LCONTROL, VK_RCONTROL
            | 0xA4 | 0xA5  // VK_LMENU, VK_RMENU
            | 0x5B | 0x5C // VK_LWIN, VK_RWIN
    )
}

/// Injects a tagged, inert keystroke so the shell does not open Start when the
/// Win key is released after a swallowed Win-combo.
///
/// # Safety
/// Builds a well-formed `INPUT` array and passes its true byte size to
/// `SendInput`; no borrowing or lifetime concerns.
unsafe fn suppress_start_menu() {
    let inputs = [
        make_key_input(VK_NONAME, false),
        make_key_input(VK_NONAME, true),
    ];
    SendInput(&inputs, std::mem::size_of::<INPUT>() as i32);
}

fn make_key_input(vk: VIRTUAL_KEY, key_up: bool) -> INPUT {
    INPUT {
        r#type: INPUT_KEYBOARD,
        Anonymous: INPUT_0 {
            ki: KEYBDINPUT {
                wVk: vk,
                wScan: 0,
                dwFlags: if key_up {
                    KEYEVENTF_KEYUP
                } else {
                    KEYBD_EVENT_FLAGS(0)
                },
                time: 0,
                dwExtraInfo: INJECTED_TAG,
            },
        },
    }
}

/// Exhaustive `KeyCode` -> virtual-key mapping. Deliberately has no wildcard arm
/// so adding a `KeyCode` is a compile error until it is mapped here.
///
/// Virtual keys are **physical**: `VK_C` is the key in the C position on any
/// keyboard layout, which is what we want — the same reason the recorder reads
/// `KeyboardEvent.code` rather than `.key`.
///
/// Two families are worth calling out:
///   * Letters and digits have no `VK_*` constants; their virtual-key values
///     are documented to equal their ASCII uppercase character.
///   * The `VK_OEM_*` values are named after their US-layout legend but are
///     positional, so `VK_OEM_1` is the key that carries `;` on a US layout
///     wherever it sits on the user's.
fn keycode_to_vk(key: KeyCode) -> VIRTUAL_KEY {
    match key {
        // --- navigation and editing ---
        KeyCode::Left => VK_LEFT,
        KeyCode::Right => VK_RIGHT,
        KeyCode::Up => VK_UP,
        KeyCode::Down => VK_DOWN,
        // Numpad Enter shares VK_RETURN; see `keycode_extended`.
        KeyCode::Enter => VK_RETURN,
        KeyCode::Space => VK_SPACE,
        KeyCode::Backspace => VK_BACK,
        KeyCode::Delete => VK_DELETE,
        KeyCode::Escape => VK_ESCAPE,
        KeyCode::Tab => VK_TAB,
        KeyCode::Insert => VK_INSERT,
        KeyCode::Home => VK_HOME,
        KeyCode::End => VK_END,
        KeyCode::PageUp => VK_PRIOR,
        KeyCode::PageDown => VK_NEXT,

        // --- letters: virtual key == ASCII uppercase ---
        KeyCode::A => VIRTUAL_KEY(b'A' as u16),
        KeyCode::B => VIRTUAL_KEY(b'B' as u16),
        KeyCode::C => VIRTUAL_KEY(b'C' as u16),
        KeyCode::D => VIRTUAL_KEY(b'D' as u16),
        KeyCode::E => VIRTUAL_KEY(b'E' as u16),
        KeyCode::F => VIRTUAL_KEY(b'F' as u16),
        KeyCode::G => VIRTUAL_KEY(b'G' as u16),
        KeyCode::H => VIRTUAL_KEY(b'H' as u16),
        KeyCode::I => VIRTUAL_KEY(b'I' as u16),
        KeyCode::J => VIRTUAL_KEY(b'J' as u16),
        KeyCode::K => VIRTUAL_KEY(b'K' as u16),
        KeyCode::L => VIRTUAL_KEY(b'L' as u16),
        KeyCode::M => VIRTUAL_KEY(b'M' as u16),
        KeyCode::N => VIRTUAL_KEY(b'N' as u16),
        KeyCode::O => VIRTUAL_KEY(b'O' as u16),
        KeyCode::P => VIRTUAL_KEY(b'P' as u16),
        KeyCode::Q => VIRTUAL_KEY(b'Q' as u16),
        KeyCode::R => VIRTUAL_KEY(b'R' as u16),
        KeyCode::S => VIRTUAL_KEY(b'S' as u16),
        KeyCode::T => VIRTUAL_KEY(b'T' as u16),
        KeyCode::U => VIRTUAL_KEY(b'U' as u16),
        KeyCode::V => VIRTUAL_KEY(b'V' as u16),
        KeyCode::W => VIRTUAL_KEY(b'W' as u16),
        KeyCode::X => VIRTUAL_KEY(b'X' as u16),
        KeyCode::Y => VIRTUAL_KEY(b'Y' as u16),
        KeyCode::Z => VIRTUAL_KEY(b'Z' as u16),

        // --- top-row digits: virtual key == ASCII digit ---
        KeyCode::Digit0 => VIRTUAL_KEY(b'0' as u16),
        KeyCode::Digit1 => VIRTUAL_KEY(b'1' as u16),
        KeyCode::Digit2 => VIRTUAL_KEY(b'2' as u16),
        KeyCode::Digit3 => VIRTUAL_KEY(b'3' as u16),
        KeyCode::Digit4 => VIRTUAL_KEY(b'4' as u16),
        KeyCode::Digit5 => VIRTUAL_KEY(b'5' as u16),
        KeyCode::Digit6 => VIRTUAL_KEY(b'6' as u16),
        KeyCode::Digit7 => VIRTUAL_KEY(b'7' as u16),
        KeyCode::Digit8 => VIRTUAL_KEY(b'8' as u16),
        KeyCode::Digit9 => VIRTUAL_KEY(b'9' as u16),

        // --- punctuation ---
        KeyCode::Backtick => VK_OEM_3,     // `~
        KeyCode::Minus => VK_OEM_MINUS,    // -_
        KeyCode::Equals => VK_OEM_PLUS,    // =+
        KeyCode::LeftBracket => VK_OEM_4,  // [{
        KeyCode::RightBracket => VK_OEM_6, // ]}
        KeyCode::Backslash => VK_OEM_5,    // \|
        KeyCode::Semicolon => VK_OEM_1,    // ;:
        KeyCode::Quote => VK_OEM_7,        // '"
        KeyCode::Comma => VK_OEM_COMMA,    // ,<
        KeyCode::Period => VK_OEM_PERIOD,  // .>
        KeyCode::Slash => VK_OEM_2,        // /?

        // --- function keys ---
        KeyCode::F1 => VK_F1,
        KeyCode::F2 => VK_F2,
        KeyCode::F3 => VK_F3,
        KeyCode::F4 => VK_F4,
        KeyCode::F5 => VK_F5,
        KeyCode::F6 => VK_F6,
        KeyCode::F7 => VK_F7,
        KeyCode::F8 => VK_F8,
        KeyCode::F9 => VK_F9,
        KeyCode::F10 => VK_F10,
        KeyCode::F11 => VK_F11,
        KeyCode::F12 => VK_F12,
        KeyCode::F13 => VK_F13,
        KeyCode::F14 => VK_F14,
        KeyCode::F15 => VK_F15,
        KeyCode::F16 => VK_F16,
        KeyCode::F17 => VK_F17,
        KeyCode::F18 => VK_F18,
        KeyCode::F19 => VK_F19,
        KeyCode::F20 => VK_F20,
        KeyCode::F21 => VK_F21,
        KeyCode::F22 => VK_F22,
        KeyCode::F23 => VK_F23,
        KeyCode::F24 => VK_F24,

        // --- numeric keypad ---
        KeyCode::Numpad0 => VK_NUMPAD0,
        KeyCode::Numpad1 => VK_NUMPAD1,
        KeyCode::Numpad2 => VK_NUMPAD2,
        KeyCode::Numpad3 => VK_NUMPAD3,
        KeyCode::Numpad4 => VK_NUMPAD4,
        KeyCode::Numpad5 => VK_NUMPAD5,
        KeyCode::Numpad6 => VK_NUMPAD6,
        KeyCode::Numpad7 => VK_NUMPAD7,
        KeyCode::Numpad8 => VK_NUMPAD8,
        KeyCode::Numpad9 => VK_NUMPAD9,
        KeyCode::NumpadAdd => VK_ADD,
        KeyCode::NumpadSubtract => VK_SUBTRACT,
        KeyCode::NumpadMultiply => VK_MULTIPLY,
        KeyCode::NumpadDivide => VK_DIVIDE,
        KeyCode::NumpadDecimal => VK_DECIMAL,
        // Windows reports the keypad's Enter as VK_RETURN with the extended
        // flag set; `keycode_extended` is what actually separates the two.
        KeyCode::NumpadEnter => VK_RETURN,
    }
}

/// The `LLKHF_EXTENDED` state a keystroke must have to satisfy this key, or
/// `None` when the flag is irrelevant.
///
/// Only the two Enter keys need this: they share `VK_RETURN`, and the keypad's
/// Enter is the extended one. Every other `KeyCode` maps to a unique virtual
/// key, so constraining the flag there would only risk rejecting genuine
/// keystrokes (the flag is also set for the navigation cluster, `VK_DIVIDE`,
/// and right-hand modifiers).
fn keycode_extended(key: KeyCode) -> Option<bool> {
    match key {
        KeyCode::Enter => Some(false),
        KeyCode::NumpadEnter => Some(true),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Reverse mapping, defined only for the round-trip test.
    fn vk_to_keycode(vk: u16, extended: bool) -> Option<KeyCode> {
        KeyCode::ALL.iter().copied().find(|&k| {
            keycode_to_vk(k).0 == vk && keycode_extended(k).map_or(true, |want| want == extended)
        })
    }

    #[test]
    fn keycode_to_vk_is_total_and_unique() {
        // A duplicated (virtual key, extended) pair is a silent bug: two Tile
        // keys would fire on the same physical keystroke.
        let mut seen = std::collections::HashSet::new();
        for key in KeyCode::ALL {
            let vk = keycode_to_vk(key).0;
            assert_ne!(vk, 0, "{key:?} mapped to VK 0");
            let entry = (vk, keycode_extended(key));
            assert!(seen.insert(entry), "duplicate VK {vk:#x} for {key:?}");
        }
        assert_eq!(seen.len(), KeyCode::ALL.len());
    }

    #[test]
    fn only_the_two_enter_keys_share_a_virtual_key() {
        // Everything else must be distinguishable by virtual key alone, so the
        // extended flag never has to be consulted for it.
        let mut counts = std::collections::HashMap::new();
        for key in KeyCode::ALL {
            *counts.entry(keycode_to_vk(key).0).or_insert(0usize) += 1;
        }
        let shared: Vec<u16> = counts
            .iter()
            .filter(|(_, &n)| n > 1)
            .map(|(&vk, _)| vk)
            .collect();
        assert_eq!(shared, vec![VK_RETURN.0]);
        assert_eq!(keycode_extended(KeyCode::Enter), Some(false));
        assert_eq!(keycode_extended(KeyCode::NumpadEnter), Some(true));
    }

    #[test]
    fn keycode_vk_round_trips() {
        for key in KeyCode::ALL {
            let vk = keycode_to_vk(key).0;
            let extended = keycode_extended(key).unwrap_or(false);
            assert_eq!(vk_to_keycode(vk, extended), Some(key));
        }
    }

    #[test]
    fn known_keys_map_to_expected_virtual_keys() {
        assert_eq!(keycode_to_vk(KeyCode::Left), VK_LEFT);
        assert_eq!(keycode_to_vk(KeyCode::C).0, 0x43);
        assert_eq!(keycode_to_vk(KeyCode::M).0, 0x4D);
        assert_eq!(keycode_to_vk(KeyCode::Numpad5), VK_NUMPAD5);
        // Letters and digits are their ASCII values; A..Z is 0x41..0x5A and
        // 0..9 is 0x30..0x39.
        assert_eq!(keycode_to_vk(KeyCode::A).0, 0x41);
        assert_eq!(keycode_to_vk(KeyCode::Z).0, 0x5A);
        assert_eq!(keycode_to_vk(KeyCode::Digit0).0, 0x30);
        assert_eq!(keycode_to_vk(KeyCode::Digit9).0, 0x39);
        // F1..F24 is a contiguous 0x70..0x87 block.
        assert_eq!(keycode_to_vk(KeyCode::F1).0, 0x70);
        assert_eq!(keycode_to_vk(KeyCode::F12).0, 0x7B);
        assert_eq!(keycode_to_vk(KeyCode::F24).0, 0x87);
        // The OEM keys are easy to transpose, so pin the awkward ones.
        assert_eq!(keycode_to_vk(KeyCode::Semicolon).0, 0xBA); // VK_OEM_1
        assert_eq!(keycode_to_vk(KeyCode::Equals).0, 0xBB); // VK_OEM_PLUS
        assert_eq!(keycode_to_vk(KeyCode::Minus).0, 0xBD); // VK_OEM_MINUS
        assert_eq!(keycode_to_vk(KeyCode::Slash).0, 0xBF); // VK_OEM_2
        assert_eq!(keycode_to_vk(KeyCode::Backtick).0, 0xC0); // VK_OEM_3
        assert_eq!(keycode_to_vk(KeyCode::LeftBracket).0, 0xDB); // VK_OEM_4
        assert_eq!(keycode_to_vk(KeyCode::Backslash).0, 0xDC); // VK_OEM_5
        assert_eq!(keycode_to_vk(KeyCode::RightBracket).0, 0xDD); // VK_OEM_6
        assert_eq!(keycode_to_vk(KeyCode::Quote).0, 0xDE); // VK_OEM_7
    }

    fn table() -> Vec<Binding> {
        vec![
            Binding {
                vk: VK_LEFT.0,
                extended: None,
                mods: Modifiers::META,
                action: WindowAction::LeftHalf,
            },
            Binding {
                vk: VK_LEFT.0,
                extended: None,
                mods: Modifiers::META | Modifiers::CONTROL,
                action: WindowAction::TopHalf,
            },
        ]
    }

    #[test]
    fn exact_modifier_match_fires_the_right_action() {
        let t = table();
        assert_eq!(
            match_binding(&t, VK_LEFT.0, true, Modifiers::META),
            Some(WindowAction::LeftHalf)
        );
        assert_eq!(
            match_binding(&t, VK_LEFT.0, true, Modifiers::META | Modifiers::CONTROL),
            Some(WindowAction::TopHalf)
        );
    }

    #[test]
    fn win_left_does_not_fire_when_ctrl_is_also_held() {
        // The whole point of exact matching: Ctrl+Win+Left must not be treated
        // as Win+Left. A `contains`-style bug would wrongly return LeftHalf.
        let only_win = vec![Binding {
            vk: VK_LEFT.0,
            extended: None,
            mods: Modifiers::META,
            action: WindowAction::LeftHalf,
        }];
        assert_eq!(
            match_binding(
                &only_win,
                VK_LEFT.0,
                true,
                Modifiers::META | Modifiers::CONTROL
            ),
            None
        );
    }

    #[test]
    fn no_match_for_unbound_key_or_bare_modifier() {
        let t = table();
        assert_eq!(match_binding(&t, VK_RIGHT.0, true, Modifiers::META), None);
        assert_eq!(match_binding(&t, VK_LEFT.0, true, Modifiers::NONE), None);
    }

    #[test]
    fn the_two_enter_keys_do_not_trigger_each_other() {
        // Both report VK_RETURN; only LLKHF_EXTENDED tells them apart.
        let bindings = vec![
            Binding {
                vk: VK_RETURN.0,
                extended: keycode_extended(KeyCode::Enter),
                mods: Modifiers::META,
                action: WindowAction::Maximize,
            },
            Binding {
                vk: VK_RETURN.0,
                extended: keycode_extended(KeyCode::NumpadEnter),
                mods: Modifiers::META,
                action: WindowAction::Center,
            },
        ];
        assert_eq!(
            match_binding(&bindings, VK_RETURN.0, false, Modifiers::META),
            Some(WindowAction::Maximize)
        );
        assert_eq!(
            match_binding(&bindings, VK_RETURN.0, true, Modifiers::META),
            Some(WindowAction::Center)
        );
    }

    #[test]
    fn keys_that_ignore_the_extended_flag_match_either_way() {
        // Nav-cluster keys arrive extended, their numpad twins do not; a key
        // with no extended requirement must accept both.
        let t = table();
        for extended in [false, true] {
            assert_eq!(
                match_binding(&t, VK_LEFT.0, extended, Modifiers::META),
                Some(WindowAction::LeftHalf)
            );
        }
    }

    #[test]
    fn modifier_virtual_keys_are_recognised() {
        for vk in [
            0x10u16, 0x11, 0x12, 0xA0, 0xA1, 0xA2, 0xA3, 0xA4, 0xA5, 0x5B, 0x5C,
        ] {
            assert!(is_modifier_vk(vk), "{vk:#x} should be a modifier");
        }
        assert!(!is_modifier_vk(VK_LEFT.0));
        assert!(!is_modifier_vk(VK_NUMPAD0.0));
    }
}
