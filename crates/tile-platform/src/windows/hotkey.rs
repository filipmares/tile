//! Global Windows hotkeys through native registration with conditional
//! low-level interception.
//!
//! `RegisterHotKey` is the normal path. A `WH_KEYBOARD_LL` hook is installed
//! only while a binding needs extended-key identity or permission to override a
//! shortcut already owned by Windows.
//!
//! # Resilience
//!
//! Windows can silently break both routes without telling the owner:
//!
//! * A low-level hook that does not return within `LowLevelHooksTimeout` is
//!   removed (Windows 7+) without notification.
//! * Sleep, lock and the secure desktop swallow key-ups, so keys we claimed can
//!   look permanently held.
//!
//! The owner thread therefore keeps the hook callback lock-free (its table lives
//! in a thread-local the callback only ever *tries* to borrow), owns a hidden
//! top-level window that receives resume / unlock / display-on notifications,
//! and runs a cheap watchdog timer while the hook is installed. Each of these
//! triggers a [`RecoveryReason`] that clears transient key state and re-arms the
//! hook; lifecycle events also re-validate the native registrations.

use std::cell::{Cell, RefCell};
use std::fmt;
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, AtomicU8, Ordering};
use std::sync::mpsc::{self, Receiver, Sender};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use tile_core::{ActionRequest, Hotkey, KeyCode, Modifiers, WindowAction};

use windows::core::{w, HRESULT, PCWSTR};
use windows::Win32::Foundation::{
    ERROR_HOTKEY_ALREADY_REGISTERED, HANDLE, HINSTANCE, HWND, LPARAM, LRESULT, WPARAM,
};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::System::Power::{
    RegisterPowerSettingNotification, UnregisterPowerSettingNotification, HPOWERNOTIFY,
    POWERBROADCAST_SETTING,
};
use windows::Win32::System::RemoteDesktop::{
    WTSRegisterSessionNotification, WTSUnRegisterSessionNotification, NOTIFY_FOR_THIS_SESSION,
};
use windows::Win32::System::SystemInformation::GetTickCount;
use windows::Win32::System::SystemServices::GUID_CONSOLE_DISPLAY_STATE;
use windows::Win32::System::Threading::GetCurrentThreadId;
use windows::Win32::UI::Input::KeyboardAndMouse::{
    GetAsyncKeyState, GetLastInputInfo, RegisterHotKey, SendInput, UnregisterHotKey,
    HOT_KEY_MODIFIERS, INPUT, INPUT_0, INPUT_KEYBOARD, KEYBDINPUT, KEYBD_EVENT_FLAGS,
    KEYEVENTF_KEYUP, LASTINPUTINFO, MOD_ALT, MOD_CONTROL, MOD_NOREPEAT, MOD_SHIFT, MOD_WIN,
    VIRTUAL_KEY, VK_ADD, VK_BACK, VK_CONTROL, VK_DECIMAL, VK_DELETE, VK_DIVIDE, VK_DOWN, VK_END,
    VK_ESCAPE, VK_F1, VK_F10, VK_F11, VK_F12, VK_F13, VK_F14, VK_F15, VK_F16, VK_F17, VK_F18,
    VK_F19, VK_F2, VK_F20, VK_F21, VK_F22, VK_F23, VK_F24, VK_F3, VK_F4, VK_F5, VK_F6, VK_F7,
    VK_F8, VK_F9, VK_HOME, VK_INSERT, VK_LEFT, VK_LWIN, VK_MENU, VK_MULTIPLY, VK_NEXT, VK_NONAME,
    VK_NUMPAD0, VK_NUMPAD1, VK_NUMPAD2, VK_NUMPAD3, VK_NUMPAD4, VK_NUMPAD5, VK_NUMPAD6, VK_NUMPAD7,
    VK_NUMPAD8, VK_NUMPAD9, VK_OEM_1, VK_OEM_2, VK_OEM_3, VK_OEM_4, VK_OEM_5, VK_OEM_6, VK_OEM_7,
    VK_OEM_COMMA, VK_OEM_MINUS, VK_OEM_PERIOD, VK_OEM_PLUS, VK_PRIOR, VK_RETURN, VK_RIGHT, VK_RWIN,
    VK_SHIFT, VK_SPACE, VK_SUBTRACT, VK_TAB, VK_UP,
};
use windows::Win32::UI::WindowsAndMessaging::{
    CallNextHookEx, CreateWindowExW, DefWindowProcW, DestroyWindow, DispatchMessageW, GetMessageW,
    KillTimer, PeekMessageW, PostThreadMessageW, RegisterClassExW, SetTimer, SetWindowsHookExW,
    TranslateMessage, UnhookWindowsHookEx, UnregisterClassW, DEVICE_NOTIFY_WINDOW_HANDLE,
    HC_ACTION, HHOOK, KBDLLHOOKSTRUCT, LLKHF_EXTENDED, MSG, PBT_APMRESUMEAUTOMATIC,
    PBT_POWERSETTINGCHANGE, PM_NOREMOVE, WH_KEYBOARD_LL, WM_APP, WM_HOTKEY, WM_KEYDOWN, WM_KEYUP,
    WM_POWERBROADCAST, WM_SYSKEYDOWN, WM_SYSKEYUP, WM_TIMER, WM_WTSSESSION_CHANGE, WNDCLASSEXW,
    WS_EX_NOACTIVATE, WS_EX_TOOLWINDOW, WS_POPUP, WTS_CONSOLE_CONNECT, WTS_REMOTE_CONNECT,
    WTS_SESSION_UNLOCK,
};

use crate::{
    HotkeyApplyReport, HotkeyBackend, HotkeyBinding, HotkeyBindingStatus, HotkeyRoute,
    PlatformError, Result,
};

const COMMAND_MESSAGE: u32 = WM_APP + 0x544;
const RECOVER_MESSAGE: u32 = WM_APP + 0x545;
const COMMAND_TIMEOUT: Duration = Duration::from_secs(5);
const INJECTED_TAG: usize = 0x54_49_4C_45;
const REQUEST_PENDING: u8 = 0;
const REQUEST_COMMITTING: u8 = 1;
const REQUEST_CANCELLED: u8 = 2;

/// Key-repeat gaps never exceed the 1 s maximum typematic delay, so a claimed
/// key that has not been seen for longer than this lost its key-up (sleep, lock,
/// secure desktop) and its next key-down is a fresh press.
const STALE_KEY_MS: u32 = 2_000;
/// How late a hook callback, or how long a stretch of owner-thread work, may be
/// before we assume Windows may have timed the hook out. Kept below the
/// documented `LowLevelHooksTimeout` range so we err towards re-arming.
const HOOK_LATENCY_LIMIT_MS: u32 = 200;
/// Watchdog cadence while the hook is installed (one wake-up per interval).
const WATCHDOG_INTERVAL_MS: u32 = 15_000;
/// Timer delivery slack beyond which the owner thread is considered stalled.
const WATCHDOG_STALL_SLACK_MS: u32 = 5_000;
/// How long the hook must be silent while the user is active before it is
/// presumed silently removed.
const HOOK_SILENCE_MS: u32 = 60_000;
/// Back-off between silence-heuristic re-arms (mouse-only use looks the same).
const SILENT_REARM_MIN_BACKOFF_MS: u32 = 60_000;
const SILENT_REARM_MAX_BACKOFF_MS: u32 = 30 * 60_000;
/// Minimum spacing between latency-triggered re-arms.
const LATENCY_REARM_MIN_GAP_MS: u32 = 10_000;
/// Lifecycle notifications often arrive in bursts (resume, display on, unlock).
const LIFECYCLE_COALESCE_MS: u32 = 2_000;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct HookBinding {
    vk: u16,
    extended: Option<bool>,
    mods: Modifiers,
    action: WindowAction,
    repeat: bool,
}

/// Dispatch table read by the hook callback.
///
/// A low-level hook is always called on the thread that installed it, so the
/// table lives in a thread-local of the owner thread instead of behind a lock.
/// The callback only ever `try_borrow`s it and passes the key through when it
/// cannot, so it can never block past `LowLevelHooksTimeout`.
struct HookDispatch {
    sender: Sender<ActionRequest>,
    bindings: Vec<HookBinding>,
}

thread_local! {
    static HOOK_DISPATCH: RefCell<Option<HookDispatch>> = const { RefCell::new(None) };
    static DISPLAY_STATE: Cell<Option<u32>> = const { Cell::new(None) };
}

static SWALLOWED_KEYS: [AtomicU64; 8] = [const { AtomicU64::new(0) }; 8];
static HELD_KEYS: [AtomicU64; 8] = [const { AtomicU64::new(0) }; 8];
static LAST_KEY_DOWN: [AtomicU32; 512] = [const { AtomicU32::new(0) }; 512];
/// Tick (`GetTickCount` base) of the last event the hook observed.
static HOOK_LAST_EVENT: AtomicU32 = AtomicU32::new(0);
/// Set by the callback when it was invoked later than [`HOOK_LATENCY_LIMIT_MS`].
static HOOK_LATE: AtomicU32 = AtomicU32::new(0);

const fn key_index(vk: u16, extended: bool) -> usize {
    (vk as usize & 0xFF) + if extended { 256 } else { 0 }
}

fn set_key_bit(bits: &[AtomicU64; 8], vk: u16, extended: bool) -> bool {
    let index = key_index(vk, extended);
    let bit = 1u64 << (index % 64);
    bits[index / 64].fetch_or(bit, Ordering::Relaxed) & bit != 0
}

fn take_key_bit(bits: &[AtomicU64; 8], vk: u16, extended: bool) -> bool {
    let index = key_index(vk, extended);
    let bit = 1u64 << (index % 64);
    bits[index / 64].fetch_and(!bit, Ordering::Relaxed) & bit != 0
}

fn key_bit_is_set(bits: &[AtomicU64; 8], vk: u16, extended: bool) -> bool {
    let index = key_index(vk, extended);
    let bit = 1u64 << (index % 64);
    bits[index / 64].load(Ordering::Relaxed) & bit != 0
}

fn clear_key_bits(bits: &[AtomicU64; 8]) {
    for word in bits {
        word.store(0, Ordering::Relaxed);
    }
}

#[cfg(test)]
fn mark_swallowed(vk: u16, extended: bool) {
    set_key_bit(&SWALLOWED_KEYS, vk, extended);
}

#[cfg(test)]
fn take_swallowed(vk: u16, extended: bool) -> bool {
    take_key_bit(&SWALLOWED_KEYS, vk, extended)
}

fn clear_transient_keys() {
    clear_key_bits(&SWALLOWED_KEYS);
    clear_key_bits(&HELD_KEYS);
}

#[cfg(test)]
fn clear_swallowed() {
    clear_key_bits(&SWALLOWED_KEYS);
}

#[cfg(test)]
const fn swallow_index(vk: u16, extended: bool) -> usize {
    key_index(vk, extended)
}

/// Milliseconds from `earlier` to `later` on the wrapping 32-bit tick clock.
fn tick_elapsed(later: u32, earlier: u32) -> u32 {
    later.wrapping_sub(earlier)
}

fn now_tick() -> u32 {
    unsafe { GetTickCount() }
}

/// Per-key transient state the hook verdicts read and update.
struct KeyTracker<'a> {
    swallowed: &'a [AtomicU64; 8],
    held: &'a [AtomicU64; 8],
    last_down: &'a [AtomicU32; 512],
}

impl KeyTracker<'static> {
    fn global() -> Self {
        Self {
            swallowed: &SWALLOWED_KEYS,
            held: &HELD_KEYS,
            last_down: &LAST_KEY_DOWN,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum KeyVerdict {
    Pass,
    Swallow {
        action: Option<WindowAction>,
        suppress_start_menu: bool,
    },
}

/// Decides what the hook does with a key-down. Pure apart from `tracker`.
///
/// A key we already claimed is normally an auto-repeat and stays swallowed. It
/// is forgotten instead when its modifiers no longer match (the user let go of
/// them mid-press) or when it has not been seen for [`STALE_KEY_MS`] (its
/// key-up was lost to sleep, lock or the secure desktop) — otherwise a single
/// lost key-up would make that shortcut dead until the key is tapped alone.
fn key_down_verdict(
    bindings: &[HookBinding],
    vk: u16,
    extended: bool,
    now: u32,
    modifiers: impl FnOnce() -> Modifiers,
    tracker: &KeyTracker<'_>,
) -> KeyVerdict {
    if is_modifier_vk(vk) {
        return KeyVerdict::Pass;
    }
    let mods = modifiers();
    let matched = match_binding_in(bindings, vk, extended, mods);
    let last_down = &tracker.last_down[key_index(vk, extended)];

    if key_bit_is_set(tracker.swallowed, vk, extended) {
        let stale = tick_elapsed(now, last_down.load(Ordering::Relaxed)) > STALE_KEY_MS;
        if let (false, Some(binding)) = (stale, matched) {
            last_down.store(now, Ordering::Relaxed);
            return KeyVerdict::Swallow {
                action: repeat_action(binding),
                suppress_start_menu: false,
            };
        }
        take_key_bit(tracker.swallowed, vk, extended);
        take_key_bit(tracker.held, vk, extended);
    }

    let Some(binding) = matched else {
        return KeyVerdict::Pass;
    };
    let repeated = !binding.repeat && set_key_bit(tracker.held, vk, extended);
    set_key_bit(tracker.swallowed, vk, extended);
    last_down.store(now, Ordering::Relaxed);
    KeyVerdict::Swallow {
        action: (!repeated).then_some(binding.action),
        suppress_start_menu: mods.contains(Modifiers::META),
    }
}

/// Returns whether the hook swallows a key-up (only for keys it claimed).
fn key_up_verdict(vk: u16, extended: bool, tracker: &KeyTracker<'_>) -> bool {
    take_key_bit(tracker.held, vk, extended);
    take_key_bit(tracker.swallowed, vk, extended)
}

/// Why the owner thread is re-arming the hotkey machinery.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RecoveryReason {
    Resume,
    SessionUnlock,
    SessionReconnect,
    DisplayOn,
    OwnerStall { ms: u32 },
    LateHookCallback { ms: u32 },
    HookSilent { ms: u32 },
    HookMissing,
}

impl RecoveryReason {
    /// Lifecycle events can leave every route stale; watchdog findings only
    /// concern the hook.
    fn is_lifecycle(self) -> bool {
        matches!(
            self,
            Self::Resume | Self::SessionUnlock | Self::SessionReconnect | Self::DisplayOn
        )
    }

    fn to_wparam(self) -> Option<usize> {
        Some(match self {
            Self::Resume => 1,
            Self::SessionUnlock => 2,
            Self::SessionReconnect => 3,
            Self::DisplayOn => 4,
            _ => return None,
        })
    }

    fn from_wparam(value: usize) -> Option<Self> {
        Some(match value {
            1 => Self::Resume,
            2 => Self::SessionUnlock,
            3 => Self::SessionReconnect,
            4 => Self::DisplayOn,
            _ => return None,
        })
    }
}

impl fmt::Display for RecoveryReason {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Resume => f.write_str("resume from sleep"),
            Self::SessionUnlock => f.write_str("session unlock"),
            Self::SessionReconnect => f.write_str("session reconnect"),
            Self::DisplayOn => f.write_str("display power-on"),
            Self::OwnerStall { ms } => write!(f, "hotkey thread stalled for {ms} ms"),
            Self::LateHookCallback { ms } => {
                write!(f, "keyboard hook callback delivered {ms} ms late")
            }
            Self::HookSilent { ms } => {
                write!(f, "keyboard hook silent for {ms} ms while input was active")
            }
            Self::HookMissing => f.write_str("keyboard hook missing while shortcuts need it"),
        }
    }
}

/// Maps a `WM_WTSSESSION_CHANGE` code to a recovery.
fn session_recovery_reason(code: u32) -> Option<RecoveryReason> {
    match code {
        WTS_SESSION_UNLOCK => Some(RecoveryReason::SessionUnlock),
        WTS_CONSOLE_CONNECT | WTS_REMOTE_CONNECT => Some(RecoveryReason::SessionReconnect),
        _ => None,
    }
}

/// Only an off -> on transition of `GUID_CONSOLE_DISPLAY_STATE` (0 = off,
/// 1 = on, 2 = dimmed) counts; Windows reports the current state once on
/// registration, which must not trigger a recovery.
fn display_turned_on(previous: Option<u32>, current: u32) -> bool {
    previous == Some(0) && current == 1
}

/// One watchdog observation, all on the `GetTickCount` clock.
#[derive(Debug, Clone, Copy)]
struct WatchdogSample {
    now: u32,
    last_input: u32,
    last_hook_event: u32,
}

/// Pure decision logic for detecting a hook Windows removed without telling us.
#[derive(Debug, Clone)]
struct HookWatchdog {
    last_tick: u32,
    last_seen_hook_event: u32,
    last_silent_rearm: Option<u32>,
    silent_backoff_ms: u32,
    last_latency_rearm: Option<u32>,
}

impl HookWatchdog {
    fn new(now: u32, last_hook_event: u32) -> Self {
        Self {
            last_tick: now,
            last_seen_hook_event: last_hook_event,
            last_silent_rearm: None,
            silent_backoff_ms: SILENT_REARM_MIN_BACKOFF_MS,
            last_latency_rearm: None,
        }
    }

    /// Restarts stall measurement, e.g. after the timer was (re)armed or the
    /// hook re-installed.
    fn restart(&mut self, now: u32, last_hook_event: u32) {
        self.last_tick = now;
        self.last_seen_hook_event = last_hook_event;
    }

    fn on_tick(&mut self, sample: WatchdogSample) -> Option<RecoveryReason> {
        let gap = tick_elapsed(sample.now, self.last_tick);
        // A sample older than the last restart is not a stall; it would only
        // wrap into a bogus ~49-day gap.
        let gap = if gap > u32::MAX / 2 { 0 } else { gap };
        self.last_tick = sample.now;

        if sample.last_hook_event != self.last_seen_hook_event {
            self.last_seen_hook_event = sample.last_hook_event;
            self.last_silent_rearm = None;
            self.silent_backoff_ms = SILENT_REARM_MIN_BACKOFF_MS;
        }

        if gap > WATCHDOG_INTERVAL_MS + WATCHDOG_STALL_SLACK_MS {
            return Some(RecoveryReason::OwnerStall { ms: gap });
        }

        let since_input = tick_elapsed(sample.now, sample.last_input);
        let since_hook = tick_elapsed(sample.now, sample.last_hook_event);
        let active = since_input <= WATCHDOG_INTERVAL_MS && since_input < since_hook;
        if !active || since_hook < HOOK_SILENCE_MS {
            return None;
        }
        if let Some(last) = self.last_silent_rearm {
            if tick_elapsed(sample.now, last) < self.silent_backoff_ms {
                return None;
            }
            self.silent_backoff_ms = self
                .silent_backoff_ms
                .saturating_mul(2)
                .min(SILENT_REARM_MAX_BACKOFF_MS);
        }
        self.last_silent_rearm = Some(sample.now);
        Some(RecoveryReason::HookSilent { ms: since_hook })
    }

    /// Rate-limits re-arms triggered by a late callback or a busy owner thread.
    fn allow_latency_rearm(&mut self, now: u32) -> bool {
        if let Some(last) = self.last_latency_rearm {
            if tick_elapsed(now, last) < LATENCY_REARM_MIN_GAP_MS {
                return false;
            }
        }
        self.last_latency_rearm = Some(now);
        true
    }
}

enum Command {
    Start,
    Apply {
        bindings: Vec<HotkeyBinding>,
        deadline: Instant,
        control: std::sync::Arc<ApplyControl>,
        reply: Sender<Result<HotkeyApplyReport>>,
    },
    Shutdown,
}

#[derive(Debug, Clone, Copy)]
struct RegisteredBinding {
    id: i32,
    binding: HotkeyBinding,
    enabled: bool,
}

struct ApplyControl {
    state: AtomicU8,
}

impl ApplyControl {
    fn new() -> Self {
        Self {
            state: AtomicU8::new(REQUEST_PENDING),
        }
    }

    fn cancelled_or_expired(&self, deadline: Instant) -> bool {
        if Instant::now() >= deadline {
            self.cancel();
        }
        self.is_cancelled()
    }

    fn cancel(&self) -> bool {
        self.state
            .compare_exchange(
                REQUEST_PENDING,
                REQUEST_CANCELLED,
                Ordering::AcqRel,
                Ordering::Acquire,
            )
            .is_ok()
    }

    fn is_cancelled(&self) -> bool {
        self.state.load(Ordering::Acquire) == REQUEST_CANCELLED
    }

    fn begin_commit(&self) -> bool {
        self.state
            .compare_exchange(
                REQUEST_PENDING,
                REQUEST_COMMITTING,
                Ordering::AcqRel,
                Ordering::Acquire,
            )
            .is_ok()
    }
}

struct OwnerState {
    events: Sender<ActionRequest>,
    registered: Vec<RegisteredBinding>,
    hook: Option<HHOOK>,
    next_id: i32,
    module: HINSTANCE,
    notifications: Option<NotificationWindow>,
    watchdog: HookWatchdog,
    watchdog_timer: usize,
    last_lifecycle_recovery: Option<u32>,
    /// Bindings whose native re-registration failed transiently during
    /// recovery. Kept apart from `registered` so `apply` never treats them as
    /// live registrations it could reuse; retried on every lifecycle recovery.
    pending: Vec<HotkeyBinding>,
}

const NOTIFICATION_CLASS: PCWSTR = w!("TileHotkeyNotifications");

/// Hidden top-level window that receives the broadcasts a message-only window
/// never sees: `WM_POWERBROADCAST` (resume, display power) and, once
/// registered, `WM_WTSSESSION_CHANGE` (unlock, reconnect).
struct NotificationWindow {
    hwnd: HWND,
    module: HINSTANCE,
    display: Option<HPOWERNOTIFY>,
    session: bool,
}

impl NotificationWindow {
    fn create(module: HINSTANCE) -> Result<Self> {
        unsafe {
            let class = WNDCLASSEXW {
                cbSize: std::mem::size_of::<WNDCLASSEXW>() as u32,
                lpfnWndProc: Some(notification_wndproc),
                hInstance: module,
                lpszClassName: NOTIFICATION_CLASS,
                ..Default::default()
            };
            // A zero atom usually means a previous backend in this process
            // already registered the class; window creation reports real
            // failures.
            RegisterClassExW(&class);
            let hwnd = CreateWindowExW(
                WS_EX_TOOLWINDOW | WS_EX_NOACTIVATE,
                NOTIFICATION_CLASS,
                w!("Tile hotkey notifications"),
                WS_POPUP,
                0,
                0,
                0,
                0,
                None,
                None,
                Some(module),
                None,
            )
            .map_err(|e| PlatformError::os("CreateWindowExW", e.message()))?;

            let session = match WTSRegisterSessionNotification(hwnd, NOTIFY_FOR_THIS_SESSION) {
                Ok(()) => true,
                Err(err) => {
                    log::warn!(
                        "session notifications unavailable; hotkeys will not re-arm on unlock: {}",
                        err.message()
                    );
                    false
                }
            };
            DISPLAY_STATE.with(|state| state.set(None));
            let display = match RegisterPowerSettingNotification(
                HANDLE(hwnd.0),
                &GUID_CONSOLE_DISPLAY_STATE,
                DEVICE_NOTIFY_WINDOW_HANDLE,
            ) {
                Ok(handle) => Some(handle),
                Err(err) => {
                    log::warn!("display power notifications unavailable: {}", err.message());
                    None
                }
            };
            Ok(Self {
                hwnd,
                module,
                display,
                session,
            })
        }
    }

    fn destroy(self) {
        unsafe {
            if let Some(display) = self.display {
                let _ = UnregisterPowerSettingNotification(display);
            }
            if self.session {
                let _ = WTSUnRegisterSessionNotification(self.hwnd);
            }
            let _ = DestroyWindow(self.hwnd);
            let _ = UnregisterClassW(NOTIFICATION_CLASS, Some(self.module));
        }
    }
}

fn post_recovery(reason: RecoveryReason) {
    let Some(code) = reason.to_wparam() else {
        return;
    };
    unsafe {
        let _ = PostThreadMessageW(
            GetCurrentThreadId(),
            RECOVER_MESSAGE,
            WPARAM(code),
            LPARAM(0),
        );
    }
}

unsafe extern "system" fn notification_wndproc(
    hwnd: HWND,
    message: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    match message {
        WM_POWERBROADCAST => {
            let event = wparam.0 as u32;
            if event == PBT_APMRESUMEAUTOMATIC {
                post_recovery(RecoveryReason::Resume);
            } else if event == PBT_POWERSETTINGCHANGE && lparam.0 != 0 {
                let setting = &*(lparam.0 as *const POWERBROADCAST_SETTING);
                if setting.PowerSetting == GUID_CONSOLE_DISPLAY_STATE
                    && setting.DataLength as usize >= std::mem::size_of::<u32>()
                {
                    let current = std::ptr::read_unaligned(setting.Data.as_ptr() as *const u32);
                    let previous = DISPLAY_STATE.with(|state| state.replace(Some(current)));
                    if display_turned_on(previous, current) {
                        post_recovery(RecoveryReason::DisplayOn);
                    }
                }
            }
            LRESULT(1)
        }
        WM_WTSSESSION_CHANGE => {
            if let Some(reason) = session_recovery_reason(wparam.0 as u32) {
                post_recovery(reason);
            }
            LRESULT(0)
        }
        _ => DefWindowProcW(hwnd, message, wparam, lparam),
    }
}

pub struct WindowsHotkeyBackend {
    commands: Sender<Command>,
    thread: Option<JoinHandle<()>>,
    thread_id: u32,
    shutdown_done: bool,
}

impl WindowsHotkeyBackend {
    pub fn new(events: Sender<ActionRequest>) -> Result<Self> {
        let (commands, command_rx) = mpsc::channel();
        let (ready_tx, ready_rx) = mpsc::channel();
        let startup_cancelled = std::sync::Arc::new(AtomicBool::new(false));
        let thread_cancelled = std::sync::Arc::clone(&startup_cancelled);
        let handle = thread::Builder::new()
            .name("tile-hotkey-owner".to_string())
            .spawn(move || owner_thread_main(events, command_rx, ready_tx, thread_cancelled))
            .map_err(|e| PlatformError::os("spawn hotkey thread", e.to_string()))?;

        let thread_id = match ready_rx.recv_timeout(COMMAND_TIMEOUT) {
            Ok(Ok(id)) => id,
            Ok(Err(err)) => {
                let _ = handle.join();
                return Err(err);
            }
            Err(err) => {
                startup_cancelled.store(true, Ordering::Release);
                drop(ready_rx);
                drop(commands);
                let _ = handle.join();
                return Err(PlatformError::os(
                    "hotkey thread readiness",
                    err.to_string(),
                ));
            }
        };
        if let Err(err) = commands.send(Command::Start) {
            startup_cancelled.store(true, Ordering::Release);
            drop(commands);
            let _ = handle.join();
            return Err(PlatformError::os("start hotkey thread", err.to_string()));
        }
        log::debug!("Windows hotkey owner thread ready (thread_id={thread_id})");

        Ok(Self {
            commands,
            thread: Some(handle),
            thread_id,
            shutdown_done: false,
        })
    }

    fn wake_owner(&self) -> Result<()> {
        unsafe {
            PostThreadMessageW(self.thread_id, COMMAND_MESSAGE, WPARAM(0), LPARAM(0))
                .map_err(|e| PlatformError::os("wake hotkey thread", e.message()))
        }
    }
}

impl HotkeyBackend for WindowsHotkeyBackend {
    fn apply(&mut self, bindings: &[HotkeyBinding]) -> Result<HotkeyApplyReport> {
        log::debug!("applying {} Windows hotkeys", bindings.len());
        let (reply_tx, reply_rx) = mpsc::channel();
        let control = std::sync::Arc::new(ApplyControl::new());
        self.commands
            .send(Command::Apply {
                bindings: bindings.to_vec(),
                deadline: Instant::now() + COMMAND_TIMEOUT,
                control: std::sync::Arc::clone(&control),
                reply: reply_tx,
            })
            .map_err(|e| PlatformError::os("queue hotkey apply", e.to_string()))?;
        if let Err(err) = self.wake_owner() {
            control.cancel();
            return Err(err);
        }
        match reply_rx.recv_timeout(COMMAND_TIMEOUT) {
            Ok(result) => result,
            Err(err) => {
                if control.cancel() {
                    reply_rx.recv_timeout(COMMAND_TIMEOUT).unwrap_or_else(|rollback| {
                        Err(PlatformError::HotkeyStateUnknown(format!(
                            "hotkey apply timed out and rollback was not acknowledged: {err}; {rollback}"
                        )))
                    })
                } else {
                    reply_rx
                        .recv_timeout(COMMAND_TIMEOUT)
                        .unwrap_or_else(|late| {
                            Err(PlatformError::HotkeyStateUnknown(format!(
                                "owner began committing but did not acknowledge it: {late}"
                            )))
                        })
                }
            }
        }
    }

    fn shutdown(&mut self) {
        if self.shutdown_done {
            return;
        }
        self.shutdown_done = true;
        log::debug!("shutting down Windows hotkey owner thread");
        let _ = self.commands.send(Command::Shutdown);
        let _ = self.wake_owner();
        if let Some(handle) = self.thread.take() {
            let _ = handle.join();
        }
        clear_transient_keys();
    }
}

impl Drop for WindowsHotkeyBackend {
    fn drop(&mut self) {
        self.shutdown();
    }
}

fn owner_thread_main(
    events: Sender<ActionRequest>,
    commands: Receiver<Command>,
    ready: Sender<Result<u32>>,
    startup_cancelled: std::sync::Arc<AtomicBool>,
) {
    unsafe {
        let module = match GetModuleHandleW(PCWSTR::null()) {
            Ok(module) => module,
            Err(err) => {
                let _ = ready.send(Err(PlatformError::os("GetModuleHandleW", err.message())));
                return;
            }
        };
        let mut seed: MSG = std::mem::zeroed();
        let _ = PeekMessageW(
            &mut seed,
            Some(HWND(std::ptr::null_mut())),
            0,
            0,
            PM_NOREMOVE,
        );

        let mut owner = OwnerState {
            events: events.clone(),
            registered: Vec::new(),
            hook: None,
            next_id: 1,
            module: module.into(),
            notifications: None,
            watchdog: HookWatchdog::new(now_tick(), HOOK_LAST_EVENT.load(Ordering::Relaxed)),
            watchdog_timer: 0,
            last_lifecycle_recovery: None,
            pending: Vec::new(),
        };
        HOOK_DISPATCH.with(|dispatch| {
            *dispatch.borrow_mut() = Some(HookDispatch {
                sender: events,
                bindings: Vec::new(),
            });
        });
        if startup_cancelled.load(Ordering::Acquire)
            || ready.send(Ok(GetCurrentThreadId())).is_err()
        {
            return;
        }
        if !await_start(&commands, &startup_cancelled) {
            return;
        }
        owner.notifications = match NotificationWindow::create(owner.module) {
            Ok(window) => Some(window),
            Err(err) => {
                log::warn!(
                    "hotkeys will not re-arm after sleep or unlock; notification window failed: {err}"
                );
                None
            }
        };
        log::debug!("Windows hotkey owner thread entered its message loop");

        let mut msg: MSG = std::mem::zeroed();
        loop {
            let result = GetMessageW(&mut msg, Some(HWND(std::ptr::null_mut())), 0, 0).0;
            if result == 0 || result == -1 {
                break;
            }
            let started = Instant::now();
            // Recovery work is deliberately thorough and must not count as a
            // stall that triggers another recovery.
            let mut measured = true;
            if msg.message == COMMAND_MESSAGE {
                if !drain_commands(&commands, &mut owner) {
                    break;
                }
            } else if msg.message == WM_HOTKEY {
                owner.dispatch_registered(msg.wParam.0 as i32);
            } else if msg.message == RECOVER_MESSAGE && msg.hwnd.0.is_null() {
                if let Some(reason) = RecoveryReason::from_wparam(msg.wParam.0) {
                    owner.recover(reason);
                }
                measured = false;
            } else if msg.message == WM_TIMER
                && msg.hwnd.0.is_null()
                && owner.watchdog_timer != 0
                && msg.wParam.0 == owner.watchdog_timer
            {
                owner.watchdog_tick();
                measured = false;
            } else {
                let _ = TranslateMessage(&msg);
                DispatchMessageW(&msg);
            }
            owner.check_latency(measured.then(|| started.elapsed()));
        }
        owner.release_all();
        log::debug!("Windows hotkey owner thread released all native state");
    }
}

fn await_start(commands: &Receiver<Command>, cancelled: &AtomicBool) -> bool {
    if cancelled.load(Ordering::Acquire) {
        return false;
    }
    match commands.recv_timeout(COMMAND_TIMEOUT) {
        Ok(Command::Start) => true,
        Ok(Command::Apply { reply, .. }) => {
            let _ = reply.send(Err(PlatformError::os(
                "hotkey thread startup",
                "received apply before startup acknowledgement",
            )));
            false
        }
        Ok(Command::Shutdown) | Err(_) => false,
    }
}

fn drain_commands(commands: &Receiver<Command>, owner: &mut OwnerState) -> bool {
    while let Ok(command) = commands.try_recv() {
        match command {
            Command::Start => {}
            Command::Apply {
                bindings,
                deadline,
                control,
                reply,
            } => {
                let result = if control.is_cancelled() || Instant::now() >= deadline {
                    Err(PlatformError::os(
                        "hotkey apply",
                        "request was cancelled or expired before processing",
                    ))
                } else {
                    owner.apply(&bindings, deadline, &control)
                };
                owner.sync_watchdog();
                let _ = reply.send(result);
            }
            Command::Shutdown => return false,
        }
    }
    true
}

impl OwnerState {
    fn apply(
        &mut self,
        bindings: &[HotkeyBinding],
        deadline: Instant,
        control: &ApplyControl,
    ) -> Result<HotkeyApplyReport> {
        self.cleanup_disabled_registrations()?;
        let old_registered: Vec<_> = self
            .registered
            .iter()
            .copied()
            .filter(|binding| binding.enabled)
            .collect();
        let old_hook_bindings = current_hook_bindings();
        let mut retained = Vec::new();
        let mut removed = Vec::new();

        for old in &old_registered {
            let reusable = bindings
                .iter()
                .any(|new| can_reuse_registration(old.binding, *new));
            if reusable {
                retained.push(*old);
            } else {
                if let Err(err) = unsafe { UnregisterHotKey(None, old.id) } {
                    return self.rollback_apply(
                        &[],
                        &removed,
                        &old_hook_bindings,
                        false,
                        &format!("failed to remove hotkey {}: {}", old.id, err.message()),
                    );
                }
                self.registered.retain(|binding| binding.id != old.id);
                removed.push(*old);
            }
        }

        let mut staged = Vec::new();
        let mut hook_bindings = Vec::new();
        let mut statuses = Vec::with_capacity(bindings.len());

        for binding in bindings {
            if control.cancelled_or_expired(deadline) {
                return self.rollback_apply(
                    &staged,
                    &removed,
                    &old_hook_bindings,
                    false,
                    "request expired",
                );
            }
            if let Some(reason) = impossible_reason(binding.hotkey) {
                statuses.push(unavailable(*binding, reason));
                continue;
            }
            if requires_extended_identity(binding.hotkey.key) {
                hook_bindings.push(to_hook_binding(*binding));
                statuses.push(status(
                    *binding,
                    HotkeyRoute::Intercepted,
                    Some("uses the hook to preserve Enter key identity".to_string()),
                ));
                continue;
            }

            if let Some(existing) = retained
                .iter_mut()
                .find(|old| can_reuse_registration(old.binding, *binding))
            {
                existing.binding = *binding;
                statuses.push(status(*binding, HotkeyRoute::Registered, None));
                continue;
            }

            let id = self.allocate_id();
            match register_binding(id, *binding) {
                Ok(()) => {
                    let registered = RegisteredBinding {
                        id,
                        binding: *binding,
                        enabled: false,
                    };
                    self.registered.push(registered);
                    staged.push(registered);
                    statuses.push(status(*binding, HotkeyRoute::Registered, None));
                }
                Err(err) if is_already_registered(&err) => {
                    hook_bindings.push(to_hook_binding(*binding));
                    statuses.push(status(
                        *binding,
                        HotkeyRoute::Intercepted,
                        Some("already owned by Windows or another application".to_string()),
                    ));
                }
                Err(err) => {
                    let reason = format!("RegisterHotKey failed: {}", err.message());
                    statuses.push(status(*binding, HotkeyRoute::Unavailable, Some(reason)));
                }
            }
        }

        if control.cancelled_or_expired(deadline) {
            return self.rollback_apply(
                &staged,
                &removed,
                &old_hook_bindings,
                false,
                "request expired",
            );
        }

        let installed_for_apply = if !hook_bindings.is_empty() && self.hook.is_none() {
            if let Err(err) = self.install_hook() {
                let rollback = self
                    .rollback_apply(
                        &staged,
                        &removed,
                        &old_hook_bindings,
                        false,
                        "hook installation failed",
                    )
                    .expect_err("rollback always reports the triggering apply failure");
                return match rollback {
                    PlatformError::HotkeyStateUnknown(details) => {
                        Err(PlatformError::HotkeyStateUnknown(format!(
                            "hook installation failed: {err}; {details}"
                        )))
                    }
                    rollback => Err(PlatformError::os(
                        "hotkey apply",
                        format!("{err}; {rollback}"),
                    )),
                };
            }
            true
        } else {
            false
        };

        if control.cancelled_or_expired(deadline) {
            return self.rollback_apply(
                &staged,
                &removed,
                &old_hook_bindings,
                installed_for_apply,
                "request expired before commit",
            );
        }

        if !control.begin_commit() {
            return self.rollback_apply(
                &staged,
                &removed,
                &old_hook_bindings,
                installed_for_apply,
                "request was cancelled before commit",
            );
        }
        // The new binding set supersedes anything recovery was still retrying.
        self.pending.clear();

        set_hook_bindings(hook_bindings.clone()).map_err(|err| {
            PlatformError::HotkeyStateUnknown(format!(
                "native registrations changed but hook dispatch could not be published: {err}"
            ))
        })?;
        for committed in retained.iter().chain(&staged) {
            if let Some(owned) = self
                .registered
                .iter_mut()
                .find(|binding| binding.id == committed.id)
            {
                owned.binding = committed.binding;
                owned.enabled = true;
            }
        }
        clear_transient_keys();

        let mut warning = None;
        if hook_bindings.is_empty() {
            if let Some(hook) = self.hook {
                match unsafe { UnhookWindowsHookEx(hook) } {
                    Ok(()) => {
                        self.hook = None;
                        log::info!("Windows keyboard hook removed; all active shortcuts use native registration");
                    }
                    Err(err) => {
                        warning = Some(format!(
                            "shortcuts were updated, but the obsolete keyboard hook could not be removed: {}",
                            err.message()
                        ));
                    }
                }
            }
        } else if installed_for_apply {
            log::info!(
                "Windows keyboard hook installed for {} shortcut(s)",
                hook_bindings.len()
            );
        }

        let report = HotkeyApplyReport {
            bindings: statuses,
            hook_installed: self.hook.is_some(),
            warning,
        };
        Self::log_apply_report(&report);
        Ok(report)
    }

    fn rollback_apply(
        &mut self,
        staged: &[RegisteredBinding],
        removed: &[RegisteredBinding],
        old_hook_bindings: &[HookBinding],
        provisional_hook: bool,
        reason: &str,
    ) -> Result<HotkeyApplyReport> {
        let mut rollback_errors = Vec::new();
        if provisional_hook {
            if let Some(hook) = self.hook {
                match unsafe { UnhookWindowsHookEx(hook) } {
                    Ok(()) => self.hook = None,
                    Err(err) => rollback_errors.push(format!(
                        "remove provisional keyboard hook: {}",
                        err.message()
                    )),
                }
            }
        }
        for binding in staged {
            match unsafe { UnregisterHotKey(None, binding.id) } {
                Ok(()) => self.registered.retain(|owned| owned.id != binding.id),
                Err(err) => {
                    rollback_errors.push(format!(
                        "unregister staged {}: {}",
                        binding.id,
                        err.message()
                    ));
                }
            }
        }
        for binding in removed {
            match register_binding(binding.id, binding.binding) {
                Ok(()) => {
                    if !self.registered.iter().any(|owned| owned.id == binding.id) {
                        self.registered.push(RegisteredBinding {
                            enabled: true,
                            ..*binding
                        });
                    }
                }
                Err(err) => {
                    rollback_errors.push(format!("restore {}: {}", binding.id, err.message()));
                }
            }
        }
        if let Err(err) = set_hook_bindings(old_hook_bindings.to_vec()) {
            rollback_errors.push(err.to_string());
        }
        if rollback_errors.is_empty() {
            log::warn!("Windows hotkey apply rolled back: {reason}");
            Err(PlatformError::os("hotkey apply", reason))
        } else {
            log::error!(
                "Windows hotkey apply rollback is incomplete: {}; {}",
                reason,
                rollback_errors.join("; ")
            );
            Err(PlatformError::HotkeyStateUnknown(format!(
                "{reason}; rollback incomplete: {}",
                rollback_errors.join("; ")
            )))
        }
    }

    fn install_hook(&mut self) -> Result<()> {
        let hook = Self::set_hook(self.module)?;
        self.hook = Some(hook);
        self.watchdog
            .restart(now_tick(), HOOK_LAST_EVENT.load(Ordering::Relaxed));
        Ok(())
    }

    fn set_hook(module: HINSTANCE) -> Result<HHOOK> {
        let hook = unsafe {
            SetWindowsHookExW(
                WH_KEYBOARD_LL,
                Some(low_level_keyboard_proc),
                Some(module),
                0,
            )
            .map_err(|e| PlatformError::os("SetWindowsHookExW", e.message()))?
        };
        // Silence is measured from (re)installation, not from the last event
        // a previous, possibly dead, hook saw.
        HOOK_LAST_EVENT.store(now_tick(), Ordering::Relaxed);
        Ok(hook)
    }

    /// Brings the hook in line with the dispatch table: a live hook is
    /// replaced (new before old, so there is never a gap; unhooking fails
    /// harmlessly when Windows already removed it), and a missing hook is
    /// installed if any binding still depends on it.
    fn arm_hook(&mut self) -> Result<HookArm> {
        let arm = hook_arm(self.hook.is_some(), !current_hook_bindings().is_empty());
        match arm {
            HookArm::NotNeeded => return Ok(arm),
            HookArm::Installed => self.install_hook()?,
            HookArm::Rearmed => {
                let old = self.hook.take().expect("hook_arm saw a hook");
                match Self::set_hook(self.module) {
                    Ok(new) => self.hook = Some(new),
                    Err(err) => {
                        self.hook = Some(old);
                        return Err(err);
                    }
                }
                if let Err(err) = unsafe { UnhookWindowsHookEx(old) } {
                    log::debug!("previous keyboard hook was already gone: {}", err.message());
                }
            }
        }
        self.watchdog
            .restart(now_tick(), HOOK_LAST_EVENT.load(Ordering::Relaxed));
        Ok(arm)
    }

    fn recover(&mut self, reason: RecoveryReason) {
        let now = now_tick();
        if reason.is_lifecycle() {
            if let Some(last) = self.last_lifecycle_recovery {
                if tick_elapsed(now, last) < LIFECYCLE_COALESCE_MS {
                    log::debug!("Windows hotkeys: {reason} coalesced with a recent recovery");
                    return;
                }
            }
            self.last_lifecycle_recovery = Some(now);
        }

        clear_transient_keys();
        // Re-validate first: a registration that fell back to the hook must be
        // covered by the arming below.
        let summary = reason
            .is_lifecycle()
            .then(|| self.revalidate_registrations());
        let hook = match self.arm_hook() {
            Ok(HookArm::Rearmed) => "keyboard hook re-armed",
            Ok(HookArm::Installed) => "keyboard hook installed",
            Ok(HookArm::NotNeeded) => "no keyboard hook needed",
            Err(err) => {
                log::warn!("Windows hotkeys: could not arm keyboard hook after {reason}: {err}");
                "keyboard hook arm failed"
            }
        };
        match summary {
            Some(summary) => log::info!(
                "Windows hotkeys recovered after {reason}: transient keys cleared, {hook}, \
                 registrations re-validated (kept={}, restored={}, moved_to_hook={}, pending={})",
                summary.kept,
                summary.restored,
                summary.moved_to_hook,
                summary.pending
            ),
            None => log::info!(
                "Windows hotkeys recovered after {reason}: transient keys cleared, {hook}"
            ),
        }
        // Any late callback observed so far is covered by this recovery.
        HOOK_LATE.store(0, Ordering::Relaxed);
        self.sync_watchdog();
    }

    /// Re-registers every active native hotkey and retries the ones a previous
    /// recovery could not restore. A registration another application grabbed
    /// meanwhile falls back to the hook, exactly as it would have on apply; any
    /// other failure is kept pending for the next recovery.
    fn revalidate_registrations(&mut self) -> RevalidateSummary {
        let mut summary = RevalidateSummary::default();
        let active: Vec<_> = self
            .registered
            .iter()
            .copied()
            .filter(|binding| binding.enabled)
            .collect();
        let mut fallback = Vec::new();
        let mut still_pending = Vec::new();
        for binding in active {
            let _ = unsafe { UnregisterHotKey(None, binding.id) };
            match register_binding(binding.id, binding.binding) {
                Ok(()) => summary.kept += 1,
                Err(err) => {
                    self.registered.retain(|owned| owned.id != binding.id);
                    if Self::classify_failure(binding.binding, &err, false) {
                        fallback.push(to_hook_binding(binding.binding));
                    } else {
                        still_pending.push(binding.binding);
                    }
                }
            }
        }
        summary.restored = self.retry_pending(&mut fallback, &mut still_pending);
        summary.moved_to_hook = fallback.len();
        summary.pending = still_pending.len();
        self.pending = still_pending;
        if !fallback.is_empty() {
            let mut table = current_hook_bindings();
            table.extend(fallback);
            if let Err(err) = set_hook_bindings(table) {
                log::warn!("Windows hotkeys: could not publish hook fallback: {err}");
            }
        }
        summary
    }

    /// Retries every pending binding with a fresh id. Returns how many were
    /// restored; the rest land in `fallback` or `still_pending`.
    fn retry_pending(
        &mut self,
        fallback: &mut Vec<HookBinding>,
        still_pending: &mut Vec<HotkeyBinding>,
    ) -> usize {
        let mut restored = 0;
        for binding in std::mem::take(&mut self.pending) {
            let id = self.allocate_id();
            match register_binding(id, binding) {
                Ok(()) => {
                    self.registered.push(RegisteredBinding {
                        id,
                        binding,
                        enabled: true,
                    });
                    restored += 1;
                }
                Err(err) => {
                    if Self::classify_failure(binding, &err, true) {
                        fallback.push(to_hook_binding(binding));
                    } else {
                        still_pending.push(binding);
                    }
                }
            }
        }
        restored
    }

    /// Watchdog-driven retry so a transient failure does not wait for the next
    /// sleep or unlock.
    fn retry_pending_from_watchdog(&mut self) {
        let mut fallback = Vec::new();
        let mut still_pending = Vec::new();
        let restored = self.retry_pending(&mut fallback, &mut still_pending);
        self.pending = still_pending;
        let moved = fallback.len();
        if !fallback.is_empty() {
            let mut table = current_hook_bindings();
            table.extend(fallback);
            if let Err(err) = set_hook_bindings(table) {
                log::warn!("Windows hotkeys: could not publish hook fallback: {err}");
            } else if self.hook.is_none() {
                if let Err(err) = self.install_hook() {
                    log::warn!("Windows hotkeys: could not install hook for fallback: {err}");
                }
            }
        }
        if restored + moved > 0 {
            log::info!(
                "Windows hotkeys: retried pending registrations (restored={restored}, \
                 moved_to_hook={moved}, still_pending={})",
                self.pending.len()
            );
        }
    }

    /// Logs a failed re-registration and returns whether it should fall back
    /// to the hook (`true`) rather than be retried later. Repeated retries log
    /// quietly so a persistent failure cannot flood the log.
    fn classify_failure(binding: HotkeyBinding, err: &windows::core::Error, retry: bool) -> bool {
        if is_already_registered(err) {
            log::warn!(
                "Windows hotkey {} was taken by another application; intercepting it instead",
                binding.hotkey
            );
            true
        } else {
            let level = if retry {
                log::Level::Debug
            } else {
                log::Level::Warn
            };
            log::log!(
                level,
                "Windows hotkey {} could not be re-registered ({}); will retry",
                binding.hotkey,
                err.message()
            );
            false
        }
    }

    fn hook_wanted(&self) -> bool {
        self.hook.is_some() || !current_hook_bindings().is_empty()
    }

    /// Runs the watchdog timer only while there is a hook to watch, a hook that
    /// should exist, or a registration still to retry.
    fn sync_watchdog(&mut self) {
        let wanted = self.hook_wanted() || !self.pending.is_empty();
        match (wanted, self.watchdog_timer) {
            (true, 0) => {
                self.watchdog_timer = unsafe { SetTimer(None, 0, WATCHDOG_INTERVAL_MS, None) };
                if self.watchdog_timer == 0 {
                    log::warn!("Windows hotkeys: keyboard hook watchdog timer unavailable");
                }
                self.watchdog
                    .restart(now_tick(), HOOK_LAST_EVENT.load(Ordering::Relaxed));
            }
            (false, timer) if timer != 0 => {
                let _ = unsafe { KillTimer(None, timer) };
                self.watchdog_timer = 0;
            }
            _ => {}
        }
    }

    fn watchdog_tick(&mut self) {
        if !self.pending.is_empty() {
            self.retry_pending_from_watchdog();
        }
        // Sampled after the retry: installing a fallback hook restarts the
        // watchdog with a newer tick, and an older `now` would wrap into a
        // bogus multi-day "stall".
        let now = now_tick();
        if self.hook.is_none() {
            // Bindings still need a hook that could not be installed earlier.
            if self.hook_wanted() && self.watchdog.allow_latency_rearm(now) {
                self.recover(RecoveryReason::HookMissing);
            }
            self.sync_watchdog();
            return;
        }
        let mut info = LASTINPUTINFO {
            cbSize: std::mem::size_of::<LASTINPUTINFO>() as u32,
            dwTime: 0,
        };
        let last_input = if unsafe { GetLastInputInfo(&mut info) }.as_bool() {
            info.dwTime
        } else {
            now.wrapping_sub(u32::MAX / 2)
        };
        let sample = WatchdogSample {
            now,
            last_input,
            last_hook_event: HOOK_LAST_EVENT.load(Ordering::Relaxed),
        };
        let Some(reason) = self.watchdog.on_tick(sample) else {
            return;
        };
        // Stalls share the latency limiter with busy/late-callback detection so
        // one stall cannot trigger back-to-back recoveries.
        if matches!(reason, RecoveryReason::OwnerStall { .. })
            && !self.watchdog.allow_latency_rearm(now)
        {
            return;
        }
        self.recover(reason);
    }

    /// Re-arms the hook when the callback ran late or the owner thread itself
    /// was busy long enough that Windows may have timed the hook out.
    fn check_latency(&mut self, busy: Option<Duration>) {
        let late = HOOK_LATE.swap(0, Ordering::Relaxed);
        if self.hook.is_none() {
            return;
        }
        let busy_ms = busy.map_or(0, |busy| {
            u32::try_from(busy.as_millis()).unwrap_or(u32::MAX)
        });
        let reason = if busy_ms > HOOK_LATENCY_LIMIT_MS {
            RecoveryReason::OwnerStall { ms: busy_ms }
        } else if late != 0 {
            RecoveryReason::LateHookCallback { ms: late }
        } else {
            return;
        };
        if self.watchdog.allow_latency_rearm(now_tick()) {
            self.recover(reason);
        }
    }

    fn cleanup_disabled_registrations(&mut self) -> Result<()> {
        let disabled: Vec<_> = self
            .registered
            .iter()
            .copied()
            .filter(|binding| !binding.enabled)
            .collect();
        for binding in disabled {
            unsafe { UnregisterHotKey(None, binding.id) }.map_err(|err| {
                PlatformError::HotkeyStateUnknown(format!(
                    "could not clean residual hotkey {}: {}",
                    binding.id,
                    err.message()
                ))
            })?;
            self.registered.retain(|owned| owned.id != binding.id);
        }
        Ok(())
    }

    fn allocate_id(&mut self) -> i32 {
        loop {
            let id = self.next_id;
            self.next_id = if self.next_id >= 0xBFFF {
                1
            } else {
                self.next_id + 1
            };
            if !self.registered.iter().any(|entry| entry.id == id) {
                return id;
            }
        }
    }

    fn dispatch_registered(&self, id: i32) {
        let Some(binding) = self
            .registered
            .iter()
            .find(|entry| entry.id == id && entry.enabled)
        else {
            return;
        };
        log::debug!(
            "Windows registered hotkey {} dispatched {}",
            binding.binding.hotkey,
            binding.binding.action
        );
        let _ = self.events.send(binding.binding.action.into());
    }

    fn log_apply_report(report: &HotkeyApplyReport) {
        let mut registered = 0;
        let mut intercepted = 0;
        let mut unavailable = 0;
        for binding in &report.bindings {
            match binding.route {
                HotkeyRoute::Registered => registered += 1,
                HotkeyRoute::Intercepted => intercepted += 1,
                HotkeyRoute::Unavailable => unavailable += 1,
            }
            log::debug!(
                "Windows hotkey {} -> {}: {:?}{}",
                binding.binding.hotkey,
                binding.binding.action,
                binding.route,
                binding
                    .reason
                    .as_deref()
                    .map(|reason| format!(" ({reason})"))
                    .unwrap_or_default()
            );
        }
        log::info!(
            "Windows hotkeys applied: registered={registered}, intercepted={intercepted}, \
             unavailable={unavailable}, hook_installed={}",
            report.hook_installed
        );
        if let Some(warning) = &report.warning {
            log::warn!("Windows hotkey apply warning: {warning}");
        }
    }

    fn release_all(&mut self) {
        set_hook_bindings(Vec::new()).ok();
        clear_transient_keys();
        self.pending.clear();
        for binding in self.registered.drain(..) {
            let _ = unsafe { UnregisterHotKey(None, binding.id) };
        }
        if let Some(hook) = self.hook.take() {
            let _ = unsafe { UnhookWindowsHookEx(hook) };
        }
        self.sync_watchdog();
        if let Some(window) = self.notifications.take() {
            window.destroy();
        }
    }
}

#[derive(Debug, Default)]
struct RevalidateSummary {
    kept: usize,
    restored: usize,
    moved_to_hook: usize,
    pending: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum HookArm {
    Rearmed,
    Installed,
    NotNeeded,
}

/// A present hook is always replaced (it may be silently dead); a missing one
/// is installed whenever the dispatch table still has bindings for it.
fn hook_arm(hook_present: bool, table_has_bindings: bool) -> HookArm {
    match (hook_present, table_has_bindings) {
        (true, _) => HookArm::Rearmed,
        (false, true) => HookArm::Installed,
        (false, false) => HookArm::NotNeeded,
    }
}

fn register_binding(id: i32, binding: HotkeyBinding) -> windows::core::Result<()> {
    let mut modifiers = native_modifiers(binding.hotkey.modifiers);
    if !binding.repeat {
        modifiers |= MOD_NOREPEAT;
    }
    unsafe {
        RegisterHotKey(
            None,
            id,
            modifiers,
            keycode_to_vk(binding.hotkey.key).0 as u32,
        )
    }
}

fn native_modifiers(modifiers: Modifiers) -> HOT_KEY_MODIFIERS {
    let mut native = HOT_KEY_MODIFIERS(0);
    if modifiers.contains(Modifiers::ALT) {
        native |= MOD_ALT;
    }
    if modifiers.contains(Modifiers::CONTROL) {
        native |= MOD_CONTROL;
    }
    if modifiers.contains(Modifiers::SHIFT) {
        native |= MOD_SHIFT;
    }
    if modifiers.contains(Modifiers::META) {
        native |= MOD_WIN;
    }
    native
}

fn is_already_registered(error: &windows::core::Error) -> bool {
    error.code() == HRESULT::from_win32(ERROR_HOTKEY_ALREADY_REGISTERED.0)
}

fn requires_extended_identity(key: KeyCode) -> bool {
    matches!(key, KeyCode::Enter | KeyCode::NumpadEnter)
}

fn can_reuse_registration(old: HotkeyBinding, new: HotkeyBinding) -> bool {
    old.hotkey == new.hotkey
        && old.repeat == new.repeat
        && !requires_extended_identity(new.hotkey.key)
        && !is_impossible(new.hotkey)
}

fn impossible_reason(hotkey: Hotkey) -> Option<&'static str> {
    if hotkey.key == KeyCode::F12 {
        return Some("F12 is reserved by the Windows debugger");
    }
    if hotkey.key == KeyCode::L && hotkey.modifiers.contains(Modifiers::META) {
        return Some("Win+L is reserved for locking Windows");
    }
    if hotkey.key == KeyCode::Delete
        && hotkey
            .modifiers
            .contains(Modifiers::CONTROL | Modifiers::ALT)
    {
        return Some("Ctrl+Alt+Delete is reserved by Windows");
    }
    None
}

fn is_impossible(hotkey: Hotkey) -> bool {
    impossible_reason(hotkey).is_some()
}

fn to_hook_binding(binding: HotkeyBinding) -> HookBinding {
    HookBinding {
        vk: keycode_to_vk(binding.hotkey.key).0,
        extended: keycode_extended(binding.hotkey.key),
        mods: binding.hotkey.modifiers,
        action: binding.action,
        repeat: binding.repeat,
    }
}

fn status(
    binding: HotkeyBinding,
    route: HotkeyRoute,
    reason: Option<String>,
) -> HotkeyBindingStatus {
    HotkeyBindingStatus {
        binding,
        route,
        reason,
    }
}

fn unavailable(binding: HotkeyBinding, reason: impl Into<String>) -> HotkeyBindingStatus {
    status(binding, HotkeyRoute::Unavailable, Some(reason.into()))
}

fn current_hook_bindings() -> Vec<HookBinding> {
    HOOK_DISPATCH
        .try_with(|dispatch| {
            dispatch
                .try_borrow()
                .ok()
                .and_then(|guard| guard.as_ref().map(|state| state.bindings.clone()))
        })
        .ok()
        .flatten()
        .unwrap_or_default()
}

fn set_hook_bindings(bindings: Vec<HookBinding>) -> Result<()> {
    HOOK_DISPATCH
        .try_with(|dispatch| {
            let mut guard = dispatch
                .try_borrow_mut()
                .map_err(|_| PlatformError::os("hotkey", "hook table is in use"))?;
            let state = guard
                .as_mut()
                .ok_or_else(|| PlatformError::os("hotkey", "hook state is unavailable"))?;
            state.bindings = bindings;
            Ok(())
        })
        .map_err(|_| PlatformError::os("hotkey", "hook state was torn down"))?
}

/// Records liveness and delivery latency for the watchdog. Called for every
/// event, including injected ones, so it must stay trivial.
fn note_hook_event(event_time: u32, now: u32) {
    HOOK_LAST_EVENT.store(now, Ordering::Relaxed);
    let latency = tick_elapsed(now, event_time);
    // Some injectors stamp garbage times; anything absurd is not a delivery
    // delay and is ignored.
    if latency > HOOK_LATENCY_LIMIT_MS && latency < 60_000 {
        HOOK_LATE.fetch_max(latency, Ordering::Relaxed);
    }
}

unsafe extern "system" fn low_level_keyboard_proc(
    code: i32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    if code == HC_ACTION as i32 {
        let kb = &*(lparam.0 as *const KBDLLHOOKSTRUCT);
        note_hook_event(kb.time, now_tick());
        if kb.dwExtraInfo != INJECTED_TAG && handle_hook_key(wparam.0 as u32, kb) {
            return LRESULT(1);
        }
    }
    CallNextHookEx(Some(HHOOK(std::ptr::null_mut())), code, wparam, lparam)
}

/// Returns whether the event is swallowed. Never blocks: if the dispatch table
/// is unavailable the key simply passes through.
fn handle_hook_key(message: u32, kb: &KBDLLHOOKSTRUCT) -> bool {
    let vk = kb.vkCode as u16;
    let extended = (kb.flags.0 & LLKHF_EXTENDED.0) != 0;
    let tracker = KeyTracker::global();
    if message == WM_KEYDOWN || message == WM_SYSKEYDOWN {
        let verdict = HOOK_DISPATCH
            .try_with(|dispatch| {
                let Ok(guard) = dispatch.try_borrow() else {
                    return KeyVerdict::Pass;
                };
                let Some(state) = guard.as_ref() else {
                    return KeyVerdict::Pass;
                };
                let verdict = key_down_verdict(
                    &state.bindings,
                    vk,
                    extended,
                    kb.time,
                    current_modifiers,
                    &tracker,
                );
                if let KeyVerdict::Swallow {
                    action: Some(action),
                    ..
                } = verdict
                {
                    let _ = state.sender.send(action.into());
                }
                verdict
            })
            .unwrap_or(KeyVerdict::Pass);
        match verdict {
            KeyVerdict::Pass => false,
            KeyVerdict::Swallow {
                suppress_start_menu: suppress,
                ..
            } => {
                if suppress {
                    unsafe { suppress_start_menu() };
                }
                true
            }
        }
    } else if message == WM_KEYUP || message == WM_SYSKEYUP {
        key_up_verdict(vk, extended, &tracker)
    } else {
        false
    }
}

fn repeat_action(binding: HookBinding) -> Option<WindowAction> {
    binding.repeat.then_some(binding.action)
}

fn match_binding_in(
    bindings: &[HookBinding],
    vk: u16,
    extended: bool,
    mods: Modifiers,
) -> Option<HookBinding> {
    bindings.iter().copied().find(|binding| {
        binding.vk == vk
            && binding.mods == mods
            && binding
                .extended
                .map_or(true, |required| required == extended)
    })
}

fn current_modifiers() -> Modifiers {
    let mut modifiers = Modifiers::NONE;
    if key_down(VK_CONTROL) {
        modifiers = modifiers | Modifiers::CONTROL;
    }
    if key_down(VK_MENU) {
        modifiers = modifiers | Modifiers::ALT;
    }
    if key_down(VK_SHIFT) {
        modifiers = modifiers | Modifiers::SHIFT;
    }
    if key_down(VK_LWIN) || key_down(VK_RWIN) {
        modifiers = modifiers | Modifiers::META;
    }
    modifiers
}

fn key_down(vk: VIRTUAL_KEY) -> bool {
    (unsafe { GetAsyncKeyState(vk.0 as i32) } as u16 & 0x8000) != 0
}

fn is_modifier_vk(vk: u16) -> bool {
    matches!(
        vk,
        0x10 | 0x11 | 0x12 | 0xA0 | 0xA1 | 0xA2 | 0xA3 | 0xA4 | 0xA5 | 0x5B | 0x5C
    )
}

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

    #[test]
    fn swallowed_key_up_is_consumed_exactly_once() {
        clear_swallowed();
        let vk = 0x47; // 'G'

        // A key-up we never claimed must always pass through.
        assert!(!take_swallowed(vk, false));

        mark_swallowed(vk, false);
        assert!(
            take_swallowed(vk, false),
            "matching key-up must be swallowed"
        );
        assert!(
            !take_swallowed(vk, false),
            "the record must be consumed, so a later key-up passes through"
        );
        clear_swallowed();
    }

    #[test]
    fn swallowing_distinguishes_the_two_enter_keys() {
        // Both report VK_RETURN; only the extended flag separates them, so
        // swallowing numpad Enter must not swallow main Enter's key-up.
        clear_swallowed();
        let vk_return = 0x0D;

        mark_swallowed(vk_return, true); // numpad Enter
        assert!(
            !take_swallowed(vk_return, false),
            "main Enter is unaffected"
        );
        assert!(take_swallowed(vk_return, true), "numpad Enter is swallowed");
        clear_swallowed();
    }

    #[test]
    fn swallowed_records_are_independent_across_keys() {
        clear_swallowed();
        mark_swallowed(0x41, false); // 'A'
        mark_swallowed(0xFF, false); // last slot in the first half
        assert!(!take_swallowed(0x42, false), "'B' was never marked");
        assert!(take_swallowed(0x41, false));
        assert!(take_swallowed(0xFF, false));
        clear_swallowed();
    }

    #[test]
    fn clear_swallowed_drops_every_pending_record() {
        clear_swallowed();
        mark_swallowed(0x47, false);
        mark_swallowed(0x0D, true);
        clear_swallowed();
        assert!(!take_swallowed(0x47, false));
        assert!(!take_swallowed(0x0D, true));
    }

    #[test]
    fn swallow_index_covers_the_bitset_without_overlap() {
        // 512 slots across 8 x 64-bit words, and the extended variant of a key
        // must never alias the non-extended one.
        assert_eq!(swallow_index(0, false), 0);
        assert_eq!(swallow_index(0xFF, false), 255);
        assert_eq!(swallow_index(0, true), 256);
        assert_eq!(swallow_index(0xFF, true), 511);
        assert_ne!(swallow_index(0x0D, false), swallow_index(0x0D, true));
    }

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

    fn table() -> Vec<HookBinding> {
        vec![
            HookBinding {
                vk: VK_LEFT.0,
                extended: None,
                mods: Modifiers::META,
                action: WindowAction::LeftHalf,
                repeat: false,
            },
            HookBinding {
                vk: VK_LEFT.0,
                extended: None,
                mods: Modifiers::META | Modifiers::CONTROL,
                action: WindowAction::TopHalf,
                repeat: false,
            },
        ]
    }

    #[test]
    fn exact_modifier_match_fires_the_right_action() {
        let t = table();
        assert_eq!(
            match_binding_in(&t, VK_LEFT.0, true, Modifiers::META).map(|binding| binding.action),
            Some(WindowAction::LeftHalf)
        );
        assert_eq!(
            match_binding_in(&t, VK_LEFT.0, true, Modifiers::META | Modifiers::CONTROL)
                .map(|binding| binding.action),
            Some(WindowAction::TopHalf)
        );
    }

    #[test]
    fn win_left_does_not_fire_when_ctrl_is_also_held() {
        // The whole point of exact matching: Ctrl+Win+Left must not be treated
        // as Win+Left. A `contains`-style bug would wrongly return LeftHalf.
        let only_win = vec![HookBinding {
            vk: VK_LEFT.0,
            extended: None,
            mods: Modifiers::META,
            action: WindowAction::LeftHalf,
            repeat: false,
        }];
        assert_eq!(
            match_binding_in(
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
        assert_eq!(
            match_binding_in(&t, VK_RIGHT.0, true, Modifiers::META),
            None
        );
        assert_eq!(match_binding_in(&t, VK_LEFT.0, true, Modifiers::NONE), None);
    }

    #[test]
    fn the_two_enter_keys_do_not_trigger_each_other() {
        // Both report VK_RETURN; only LLKHF_EXTENDED tells them apart.
        let bindings = vec![
            HookBinding {
                vk: VK_RETURN.0,
                extended: keycode_extended(KeyCode::Enter),
                mods: Modifiers::META,
                action: WindowAction::Maximize,
                repeat: false,
            },
            HookBinding {
                vk: VK_RETURN.0,
                extended: keycode_extended(KeyCode::NumpadEnter),
                mods: Modifiers::META,
                action: WindowAction::Center,
                repeat: false,
            },
        ];
        assert_eq!(
            match_binding_in(&bindings, VK_RETURN.0, false, Modifiers::META)
                .map(|binding| binding.action),
            Some(WindowAction::Maximize)
        );
        assert_eq!(
            match_binding_in(&bindings, VK_RETURN.0, true, Modifiers::META)
                .map(|binding| binding.action),
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
                match_binding_in(&t, VK_LEFT.0, extended, Modifiers::META)
                    .map(|binding| binding.action),
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

    #[test]
    fn reserved_windows_shortcuts_are_rejected_before_registration() {
        for hotkey in [
            Hotkey::new(Modifiers::CONTROL, KeyCode::F12),
            Hotkey::new(Modifiers::META, KeyCode::L),
            Hotkey::new(Modifiers::CONTROL | Modifiers::ALT, KeyCode::Delete),
        ] {
            assert!(impossible_reason(hotkey).is_some(), "{hotkey} must fail");
        }
        assert!(impossible_reason(Hotkey::new(Modifiers::CONTROL, KeyCode::L)).is_none());
    }

    #[test]
    fn hook_route_preserves_repeat_policy_and_is_exclusive() {
        let binding = HotkeyBinding {
            hotkey: Hotkey::new(Modifiers::META, KeyCode::Left),
            action: WindowAction::MoveLeft,
            repeat: true,
        };
        let repeating = to_hook_binding(binding);
        assert!(repeating.repeat);
        assert_eq!(repeat_action(repeating), Some(WindowAction::MoveLeft));

        let exclusive = to_hook_binding(HotkeyBinding {
            repeat: false,
            ..binding
        });
        assert!(!exclusive.repeat);
        assert_eq!(repeat_action(exclusive), None);
    }

    #[test]
    fn native_modifier_translation_is_complete() {
        let all = Modifiers::CONTROL | Modifiers::ALT | Modifiers::SHIFT | Modifiers::META;
        assert_eq!(
            native_modifiers(all),
            MOD_CONTROL | MOD_ALT | MOD_SHIFT | MOD_WIN
        );
    }

    #[test]
    fn changing_repeat_policy_requires_native_reregistration() {
        let old = HotkeyBinding {
            hotkey: Hotkey::new(Modifiers::CONTROL, KeyCode::M),
            action: WindowAction::MoveLeft,
            repeat: true,
        };
        assert!(can_reuse_registration(
            old,
            HotkeyBinding {
                action: WindowAction::MoveRight,
                ..old
            }
        ));
        assert!(!can_reuse_registration(
            old,
            HotkeyBinding {
                action: WindowAction::Maximize,
                repeat: false,
                ..old
            }
        ));
    }

    #[test]
    fn cancellation_and_commit_have_one_winner() {
        let cancelled = ApplyControl::new();
        assert!(cancelled.cancel());
        assert!(!cancelled.begin_commit());

        let committing = ApplyControl::new();
        assert!(committing.begin_commit());
        assert!(!committing.cancel());
    }

    #[test]
    fn owner_requires_start_acknowledgement_before_message_loop() {
        let cancelled = AtomicBool::new(false);
        let (tx, rx) = mpsc::channel();
        tx.send(Command::Start).unwrap();
        assert!(await_start(&rx, &cancelled));

        let cancelled = AtomicBool::new(true);
        let (_tx, rx) = mpsc::channel();
        assert!(!await_start(&rx, &cancelled));

        let cancelled = AtomicBool::new(false);
        let (tx, rx) = mpsc::channel::<Command>();
        drop(tx);
        assert!(!await_start(&rx, &cancelled));
    }

    #[test]
    fn swallowed_identity_stays_marked_until_key_up() {
        clear_swallowed();
        mark_swallowed(VK_LEFT.0, true);
        assert!(key_bit_is_set(&SWALLOWED_KEYS, VK_LEFT.0, true));
        assert!(!key_bit_is_set(&SWALLOWED_KEYS, VK_LEFT.0, false));
        assert!(take_swallowed(VK_LEFT.0, true));
        assert!(!key_bit_is_set(&SWALLOWED_KEYS, VK_LEFT.0, true));
    }

    struct LocalKeys {
        swallowed: [AtomicU64; 8],
        held: [AtomicU64; 8],
        last_down: [AtomicU32; 512],
    }

    impl LocalKeys {
        fn new() -> Self {
            Self {
                swallowed: [const { AtomicU64::new(0) }; 8],
                held: [const { AtomicU64::new(0) }; 8],
                last_down: [const { AtomicU32::new(0) }; 512],
            }
        }

        fn tracker(&self) -> KeyTracker<'_> {
            KeyTracker {
                swallowed: &self.swallowed,
                held: &self.held,
                last_down: &self.last_down,
            }
        }

        fn down(&self, bindings: &[HookBinding], vk: u16, now: u32, mods: Modifiers) -> KeyVerdict {
            key_down_verdict(bindings, vk, false, now, || mods, &self.tracker())
        }

        fn up(&self, vk: u16) -> bool {
            key_up_verdict(vk, false, &self.tracker())
        }
    }

    fn fired(action: WindowAction, start: bool) -> KeyVerdict {
        KeyVerdict::Swallow {
            action: Some(action),
            suppress_start_menu: start,
        }
    }

    const SWALLOW_SILENTLY: KeyVerdict = KeyVerdict::Swallow {
        action: None,
        suppress_start_menu: false,
    };

    #[test]
    fn exclusive_binding_fires_once_and_swallows_its_repeats_and_key_up() {
        let keys = LocalKeys::new();
        let t = table();
        assert_eq!(
            keys.down(&t, VK_LEFT.0, 1_000, Modifiers::META),
            fired(WindowAction::LeftHalf, true)
        );
        for now in [1_500, 1_533, 1_566] {
            assert_eq!(
                keys.down(&t, VK_LEFT.0, now, Modifiers::META),
                SWALLOW_SILENTLY
            );
        }
        assert!(keys.up(VK_LEFT.0), "claimed key-up is swallowed");
        assert!(!keys.up(VK_LEFT.0), "and only once");
        assert_eq!(
            keys.down(&t, VK_LEFT.0, 1_700, Modifiers::META),
            fired(WindowAction::LeftHalf, true),
            "a new press after key-up fires again"
        );
    }

    #[test]
    fn repeating_binding_fires_on_every_auto_repeat() {
        let keys = LocalKeys::new();
        let t = vec![HookBinding {
            repeat: true,
            ..table()[0]
        }];
        assert_eq!(
            keys.down(&t, VK_LEFT.0, 10, Modifiers::META),
            fired(WindowAction::LeftHalf, true)
        );
        assert_eq!(
            keys.down(&t, VK_LEFT.0, 500, Modifiers::META),
            KeyVerdict::Swallow {
                action: Some(WindowAction::LeftHalf),
                suppress_start_menu: false,
            }
        );
    }

    #[test]
    fn lost_key_up_does_not_kill_the_shortcut() {
        // Sleep / lock / the secure desktop ate the key-up: the next press long
        // after must be a fresh press, not a silently swallowed "repeat".
        let keys = LocalKeys::new();
        let t = table();
        assert_eq!(
            keys.down(&t, VK_LEFT.0, 1_000, Modifiers::META),
            fired(WindowAction::LeftHalf, true)
        );
        assert_eq!(
            keys.down(&t, VK_LEFT.0, 1_000 + STALE_KEY_MS + 1, Modifiers::META),
            fired(WindowAction::LeftHalf, true)
        );
    }

    #[test]
    fn stale_detection_survives_tick_wraparound() {
        let keys = LocalKeys::new();
        let t = table();
        let before_wrap = u32::MAX - 100;
        keys.down(&t, VK_LEFT.0, before_wrap, Modifiers::META);
        assert_eq!(
            keys.down(&t, VK_LEFT.0, 200, Modifiers::META),
            SWALLOW_SILENTLY,
            "301 ms across the wrap is still an auto-repeat"
        );
        assert_eq!(
            keys.down(&t, VK_LEFT.0, 200 + STALE_KEY_MS + 1, Modifiers::META),
            fired(WindowAction::LeftHalf, true)
        );
    }

    #[test]
    fn releasing_modifiers_mid_press_hands_the_key_back() {
        let keys = LocalKeys::new();
        let t = table();
        keys.down(&t, VK_LEFT.0, 100, Modifiers::META);
        assert_eq!(
            keys.down(&t, VK_LEFT.0, 400, Modifiers::NONE),
            KeyVerdict::Pass,
            "plain Left repeats belong to the application"
        );
        assert!(!keys.up(VK_LEFT.0), "its key-up must reach the app too");
    }

    #[test]
    fn modifiers_and_unbound_keys_always_pass() {
        let keys = LocalKeys::new();
        let t = table();
        assert_eq!(
            keys.down(&t, VK_LWIN.0, 1, Modifiers::META),
            KeyVerdict::Pass
        );
        assert_eq!(
            keys.down(&t, VK_RIGHT.0, 1, Modifiers::META),
            KeyVerdict::Pass
        );
        assert!(!keys.up(VK_RIGHT.0));
    }

    #[test]
    fn start_menu_is_only_suppressed_for_win_chords() {
        let keys = LocalKeys::new();
        let t = vec![HookBinding {
            mods: Modifiers::CONTROL,
            ..table()[0]
        }];
        assert_eq!(
            keys.down(&t, VK_LEFT.0, 1, Modifiers::CONTROL),
            fired(WindowAction::LeftHalf, false)
        );
    }

    #[test]
    fn lifecycle_reasons_round_trip_through_the_message_queue() {
        for reason in [
            RecoveryReason::Resume,
            RecoveryReason::SessionUnlock,
            RecoveryReason::SessionReconnect,
            RecoveryReason::DisplayOn,
        ] {
            assert!(reason.is_lifecycle());
            let code = reason.to_wparam().expect("lifecycle reasons are postable");
            assert_eq!(RecoveryReason::from_wparam(code), Some(reason));
        }
        for reason in [
            RecoveryReason::OwnerStall { ms: 1 },
            RecoveryReason::LateHookCallback { ms: 1 },
            RecoveryReason::HookSilent { ms: 1 },
            RecoveryReason::HookMissing,
        ] {
            assert!(!reason.is_lifecycle());
            assert_eq!(reason.to_wparam(), None);
        }
        assert_eq!(RecoveryReason::from_wparam(0), None);
        assert_eq!(RecoveryReason::from_wparam(99), None);
    }

    #[test]
    fn only_unlock_and_reconnect_session_events_recover() {
        assert_eq!(
            session_recovery_reason(WTS_SESSION_UNLOCK),
            Some(RecoveryReason::SessionUnlock)
        );
        assert_eq!(
            session_recovery_reason(WTS_CONSOLE_CONNECT),
            Some(RecoveryReason::SessionReconnect)
        );
        assert_eq!(
            session_recovery_reason(WTS_REMOTE_CONNECT),
            Some(RecoveryReason::SessionReconnect)
        );
        // Lock (7) and disconnects must not trigger anything.
        for code in [2, 4, 5, 6, 7, 9] {
            assert_eq!(session_recovery_reason(code), None, "code {code}");
        }
    }

    #[test]
    fn display_recovery_needs_an_off_to_on_transition() {
        assert!(!display_turned_on(None, 1), "initial report is not a wake");
        assert!(display_turned_on(Some(0), 1));
        assert!(!display_turned_on(Some(2), 1), "undimming is not a wake");
        assert!(!display_turned_on(Some(1), 1));
        assert!(!display_turned_on(Some(1), 0));
    }

    fn sample(now: u32, last_input: u32, last_hook_event: u32) -> WatchdogSample {
        WatchdogSample {
            now,
            last_input,
            last_hook_event,
        }
    }

    #[test]
    fn watchdog_flags_a_stalled_owner_thread() {
        let mut dog = HookWatchdog::new(0, 0);
        assert_eq!(dog.on_tick(sample(WATCHDOG_INTERVAL_MS, 0, 0)), None);
        let late = 2 * WATCHDOG_INTERVAL_MS + WATCHDOG_STALL_SLACK_MS + 1;
        assert_eq!(
            dog.on_tick(sample(late, 0, 0)),
            Some(RecoveryReason::OwnerStall {
                ms: WATCHDOG_INTERVAL_MS + WATCHDOG_STALL_SLACK_MS + 1
            })
        );
    }

    #[test]
    fn watchdog_stays_quiet_while_idle_or_while_the_hook_is_alive() {
        let mut dog = HookWatchdog::new(0, 0);
        let mut now = 0;
        for _ in 0..20 {
            now += WATCHDOG_INTERVAL_MS;
            // Idle user: no recent input at all.
            assert_eq!(dog.on_tick(sample(now, 0, 0)), None);
        }
        // Active user with a live hook: the hook saw the latest keystroke.
        now += WATCHDOG_INTERVAL_MS;
        assert_eq!(dog.on_tick(sample(now, now - 10, now - 10)), None);
        // Recent input but the hook heard something within the silence window.
        now += WATCHDOG_INTERVAL_MS;
        assert_eq!(
            dog.on_tick(sample(now, now - 5, now - HOOK_SILENCE_MS + 1)),
            None
        );
    }

    #[test]
    fn watchdog_rearms_a_silent_hook_with_exponential_backoff() {
        let mut dog = HookWatchdog::new(0, 0);
        let mut now = HOOK_SILENCE_MS;
        let mut rearms = Vec::new();
        // An hour of activity the (dead) hook never hears about.
        while now < 60 * 60_000 {
            dog.restart(now - WATCHDOG_INTERVAL_MS, 0);
            if dog.on_tick(sample(now, now - 1, 0)).is_some() {
                rearms.push(now);
            }
            now += WATCHDOG_INTERVAL_MS;
        }
        assert_eq!(
            rearms.first(),
            Some(&HOOK_SILENCE_MS),
            "first re-arm is immediate"
        );
        let gaps: Vec<u32> = rearms.windows(2).map(|w| w[1] - w[0]).collect();
        assert!(
            gaps.windows(2).all(|w| w[1] >= w[0]),
            "gaps never shrink: {gaps:?}"
        );
        assert!(gaps[0] >= SILENT_REARM_MIN_BACKOFF_MS);
        assert!(gaps
            .iter()
            .all(|&gap| gap <= SILENT_REARM_MAX_BACKOFF_MS + WATCHDOG_INTERVAL_MS));
        assert!(rearms.len() <= 7, "re-arms must stay rare: {rearms:?}");
    }

    #[test]
    fn hook_activity_resets_the_silence_backoff() {
        let mut dog = HookWatchdog::new(HOOK_SILENCE_MS - WATCHDOG_INTERVAL_MS, 0);
        let mut now = HOOK_SILENCE_MS;
        assert_eq!(
            dog.on_tick(sample(now, now - 1, 0)),
            Some(RecoveryReason::HookSilent { ms: now })
        );
        now += WATCHDOG_INTERVAL_MS;
        assert_eq!(dog.on_tick(sample(now, now - 1, 0)), None, "backing off");

        // The hook comes back to life, then goes quiet again.
        let alive_at = now + 1;
        now += WATCHDOG_INTERVAL_MS;
        assert_eq!(dog.on_tick(sample(now, alive_at, alive_at)), None);
        while tick_elapsed(now, alive_at) < HOOK_SILENCE_MS {
            now += WATCHDOG_INTERVAL_MS;
            dog.on_tick(sample(now, alive_at, alive_at));
        }
        assert!(
            dog.on_tick(sample(now, now - 1, alive_at)).is_some(),
            "a fresh silence re-arms without waiting out the old back-off"
        );
    }

    #[test]
    fn a_sample_older_than_the_last_restart_is_not_a_stall() {
        let mut dog = HookWatchdog::new(0, 0);
        dog.restart(10_000, 0);
        assert_eq!(dog.on_tick(sample(9_990, 0, 0)), None);
    }

    #[test]
    fn hook_arm_installs_whenever_bindings_still_need_a_hook() {
        assert_eq!(hook_arm(true, true), HookArm::Rearmed);
        assert_eq!(hook_arm(true, false), HookArm::Rearmed);
        assert_eq!(
            hook_arm(false, true),
            HookArm::Installed,
            "a failed earlier install must be retried, not reported as unneeded"
        );
        assert_eq!(hook_arm(false, false), HookArm::NotNeeded);
    }

    #[test]
    fn latency_rearms_are_rate_limited() {
        let mut dog = HookWatchdog::new(0, 0);
        assert!(dog.allow_latency_rearm(100));
        assert!(!dog.allow_latency_rearm(100 + LATENCY_REARM_MIN_GAP_MS - 1));
        assert!(dog.allow_latency_rearm(100 + LATENCY_REARM_MIN_GAP_MS));
    }

    #[test]
    fn tick_arithmetic_wraps() {
        assert_eq!(tick_elapsed(5, u32::MAX - 4), 10);
        assert_eq!(tick_elapsed(1_000, 400), 600);
    }

    #[test]
    fn hook_table_is_owned_by_the_installing_thread() {
        // The callback runs on the owner thread; other threads see no table and
        // must get a pass-through rather than a block.
        std::thread::spawn(|| {
            assert!(current_hook_bindings().is_empty());
            assert!(set_hook_bindings(Vec::new()).is_err());
            let (tx, _rx) = mpsc::channel();
            HOOK_DISPATCH.with(|dispatch| {
                *dispatch.borrow_mut() = Some(HookDispatch {
                    sender: tx,
                    bindings: Vec::new(),
                });
            });
            set_hook_bindings(table()).unwrap();
            assert_eq!(current_hook_bindings(), table());
            // While the owner holds the table mutably, readers back off.
            HOOK_DISPATCH.with(|dispatch| {
                let _held = dispatch.borrow_mut();
                assert!(current_hook_bindings().is_empty());
            });
        })
        .join()
        .unwrap();
    }
}
