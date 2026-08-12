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
    KEYEVENTF_KEYUP, VIRTUAL_KEY, VK_BACK, VK_CONTROL, VK_DELETE, VK_DOWN, VK_ESCAPE, VK_LEFT,
    VK_LWIN, VK_MENU, VK_NONAME, VK_NUMPAD0, VK_NUMPAD1, VK_NUMPAD2, VK_NUMPAD3, VK_NUMPAD4,
    VK_NUMPAD5, VK_NUMPAD6, VK_NUMPAD7, VK_NUMPAD8, VK_NUMPAD9, VK_RETURN, VK_RIGHT, VK_RWIN,
    VK_SHIFT, VK_SPACE, VK_UP,
};
use windows::Win32::UI::WindowsAndMessaging::{
    CallNextHookEx, DispatchMessageW, GetMessageW, PostThreadMessageW, SetWindowsHookExW,
    TranslateMessage, UnhookWindowsHookEx, HC_ACTION, HHOOK, KBDLLHOOKSTRUCT, MSG, WH_KEYBOARD_LL,
    WM_KEYDOWN, WM_QUIT, WM_SYSKEYDOWN,
};

use crate::{HotkeyBackend, HotkeyFailure, PlatformError, Result};

/// One resolved binding, in the form the hook callback compares against.
#[derive(Debug, Clone, Copy, PartialEq)]
struct Binding {
    vk: u16,
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

        let hook = match SetWindowsHookExW(WH_KEYBOARD_LL, Some(low_level_keyboard_proc), hmod, 0) {
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
            let result = GetMessageW(&mut msg, HWND(std::ptr::null_mut()), 0, 0).0;
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
                    if dispatch(vk, mods) {
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
    CallNextHookEx(HHOOK(std::ptr::null_mut()), code, wparam, lparam)
}

/// Looks up `(vk, mods)` in the shared table and, on an exact match, sends the
/// bound action. Returns whether a binding matched (and should be swallowed).
fn dispatch(vk: u16, mods: Modifiers) -> bool {
    let Some(state) = HOOK_STATE.get() else {
        return false;
    };
    let Ok(guard) = state.lock() else {
        return false;
    };
    let Some(hs) = guard.as_ref() else {
        return false;
    };
    match match_binding(&hs.bindings, vk, mods) {
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
fn match_binding(bindings: &[Binding], vk: u16, mods: Modifiers) -> Option<WindowAction> {
    bindings
        .iter()
        .find(|b| b.vk == vk && b.mods == mods)
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
fn keycode_to_vk(key: KeyCode) -> VIRTUAL_KEY {
    match key {
        KeyCode::Left => VK_LEFT,
        KeyCode::Right => VK_RIGHT,
        KeyCode::Up => VK_UP,
        KeyCode::Down => VK_DOWN,
        KeyCode::Enter => VK_RETURN,
        KeyCode::Space => VK_SPACE,
        KeyCode::Backspace => VK_BACK,
        KeyCode::Delete => VK_DELETE,
        KeyCode::Escape => VK_ESCAPE,
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
        // Letters use their ASCII-uppercase code as the virtual-key value.
        KeyCode::C => VIRTUAL_KEY(0x43),
        KeyCode::F => VIRTUAL_KEY(0x46),
        KeyCode::M => VIRTUAL_KEY(0x4D),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Reverse mapping, defined only for the round-trip test.
    fn vk_to_keycode(vk: u16) -> Option<KeyCode> {
        KeyCode::ALL
            .iter()
            .copied()
            .find(|&k| keycode_to_vk(k).0 == vk)
    }

    #[test]
    fn keycode_to_vk_is_total_and_unique() {
        let mut seen = std::collections::HashSet::new();
        for key in KeyCode::ALL {
            let vk = keycode_to_vk(key).0;
            assert_ne!(vk, 0, "{key:?} mapped to VK 0");
            assert!(seen.insert(vk), "duplicate VK {vk:#x} for {key:?}");
        }
        assert_eq!(seen.len(), KeyCode::ALL.len());
    }

    #[test]
    fn keycode_vk_round_trips() {
        for key in KeyCode::ALL {
            let vk = keycode_to_vk(key).0;
            assert_eq!(vk_to_keycode(vk), Some(key));
        }
    }

    #[test]
    fn known_keys_map_to_expected_virtual_keys() {
        assert_eq!(keycode_to_vk(KeyCode::Left), VK_LEFT);
        assert_eq!(keycode_to_vk(KeyCode::C).0, 0x43);
        assert_eq!(keycode_to_vk(KeyCode::M).0, 0x4D);
        assert_eq!(keycode_to_vk(KeyCode::Numpad5), VK_NUMPAD5);
    }

    fn table() -> Vec<Binding> {
        vec![
            Binding {
                vk: VK_LEFT.0,
                mods: Modifiers::META,
                action: WindowAction::LeftHalf,
            },
            Binding {
                vk: VK_LEFT.0,
                mods: Modifiers::META | Modifiers::CONTROL,
                action: WindowAction::TopHalf,
            },
        ]
    }

    #[test]
    fn exact_modifier_match_fires_the_right_action() {
        let t = table();
        assert_eq!(
            match_binding(&t, VK_LEFT.0, Modifiers::META),
            Some(WindowAction::LeftHalf)
        );
        assert_eq!(
            match_binding(&t, VK_LEFT.0, Modifiers::META | Modifiers::CONTROL),
            Some(WindowAction::TopHalf)
        );
    }

    #[test]
    fn win_left_does_not_fire_when_ctrl_is_also_held() {
        // The whole point of exact matching: Ctrl+Win+Left must not be treated
        // as Win+Left. A `contains`-style bug would wrongly return LeftHalf.
        let only_win = vec![Binding {
            vk: VK_LEFT.0,
            mods: Modifiers::META,
            action: WindowAction::LeftHalf,
        }];
        assert_eq!(
            match_binding(&only_win, VK_LEFT.0, Modifiers::META | Modifiers::CONTROL),
            None
        );
    }

    #[test]
    fn no_match_for_unbound_key_or_bare_modifier() {
        let t = table();
        assert_eq!(match_binding(&t, VK_RIGHT.0, Modifiers::META), None);
        assert_eq!(match_binding(&t, VK_LEFT.0, Modifiers::NONE), None);
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
