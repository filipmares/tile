//! Global Windows hotkeys through native registration with conditional
//! low-level interception.
//!
//! `RegisterHotKey` is the normal path. A `WH_KEYBOARD_LL` hook is installed
//! only while a binding needs extended-key identity or permission to override a
//! shortcut already owned by Windows.

use std::sync::atomic::{AtomicBool, AtomicU64, AtomicU8, Ordering};
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Mutex, OnceLock};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use tile_core::{Hotkey, KeyCode, Modifiers, WindowAction};

use windows::core::{HRESULT, PCWSTR};
use windows::Win32::Foundation::{
    ERROR_HOTKEY_ALREADY_REGISTERED, HINSTANCE, HWND, LPARAM, LRESULT, WPARAM,
};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::System::Threading::GetCurrentThreadId;
use windows::Win32::UI::Input::KeyboardAndMouse::{
    GetAsyncKeyState, RegisterHotKey, SendInput, UnregisterHotKey, HOT_KEY_MODIFIERS, INPUT,
    INPUT_0, INPUT_KEYBOARD, KEYBDINPUT, KEYBD_EVENT_FLAGS, KEYEVENTF_KEYUP, MOD_ALT, MOD_CONTROL,
    MOD_NOREPEAT, MOD_SHIFT, MOD_WIN, VIRTUAL_KEY, VK_ADD, VK_BACK, VK_CONTROL, VK_DECIMAL,
    VK_DELETE, VK_DIVIDE, VK_DOWN, VK_END, VK_ESCAPE, VK_F1, VK_F10, VK_F11, VK_F12, VK_F13,
    VK_F14, VK_F15, VK_F16, VK_F17, VK_F18, VK_F19, VK_F2, VK_F20, VK_F21, VK_F22, VK_F23, VK_F24,
    VK_F3, VK_F4, VK_F5, VK_F6, VK_F7, VK_F8, VK_F9, VK_HOME, VK_INSERT, VK_LEFT, VK_LWIN, VK_MENU,
    VK_MULTIPLY, VK_NEXT, VK_NONAME, VK_NUMPAD0, VK_NUMPAD1, VK_NUMPAD2, VK_NUMPAD3, VK_NUMPAD4,
    VK_NUMPAD5, VK_NUMPAD6, VK_NUMPAD7, VK_NUMPAD8, VK_NUMPAD9, VK_OEM_1, VK_OEM_2, VK_OEM_3,
    VK_OEM_4, VK_OEM_5, VK_OEM_6, VK_OEM_7, VK_OEM_COMMA, VK_OEM_MINUS, VK_OEM_PERIOD, VK_OEM_PLUS,
    VK_PRIOR, VK_RETURN, VK_RIGHT, VK_RWIN, VK_SHIFT, VK_SPACE, VK_SUBTRACT, VK_TAB, VK_UP,
};
use windows::Win32::UI::WindowsAndMessaging::{
    CallNextHookEx, DispatchMessageW, GetMessageW, PeekMessageW, PostThreadMessageW,
    SetWindowsHookExW, TranslateMessage, UnhookWindowsHookEx, HC_ACTION, HHOOK, KBDLLHOOKSTRUCT,
    LLKHF_EXTENDED, MSG, PM_NOREMOVE, WH_KEYBOARD_LL, WM_APP, WM_HOTKEY, WM_KEYDOWN, WM_KEYUP,
    WM_SYSKEYDOWN, WM_SYSKEYUP,
};

use crate::{
    HotkeyApplyReport, HotkeyBackend, HotkeyBinding, HotkeyBindingStatus, HotkeyRoute,
    PlatformError, Result,
};

const COMMAND_MESSAGE: u32 = WM_APP + 0x544;
const COMMAND_TIMEOUT: Duration = Duration::from_secs(5);
const INJECTED_TAG: usize = 0x54_49_4C_45;
const REQUEST_PENDING: u8 = 0;
const REQUEST_COMMITTING: u8 = 1;
const REQUEST_CANCELLED: u8 = 2;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct HookBinding {
    vk: u16,
    extended: Option<bool>,
    mods: Modifiers,
    action: WindowAction,
    repeat: bool,
}

struct HookState {
    sender: Sender<WindowAction>,
    bindings: Vec<HookBinding>,
}

static HOOK_STATE: OnceLock<Mutex<Option<HookState>>> = OnceLock::new();
static SWALLOWED_KEYS: [AtomicU64; 8] = [const { AtomicU64::new(0) }; 8];
static HELD_KEYS: [AtomicU64; 8] = [const { AtomicU64::new(0) }; 8];

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

fn mark_swallowed(vk: u16, extended: bool) {
    set_key_bit(&SWALLOWED_KEYS, vk, extended);
}

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
    events: Sender<WindowAction>,
    registered: Vec<RegisteredBinding>,
    hook: Option<HHOOK>,
    next_id: i32,
    module: HINSTANCE,
}

pub struct WindowsHotkeyBackend {
    commands: Sender<Command>,
    thread: Option<JoinHandle<()>>,
    thread_id: u32,
    shutdown_done: bool,
}

impl WindowsHotkeyBackend {
    pub fn new(events: Sender<WindowAction>) -> Result<Self> {
        let hook_state = HOOK_STATE.get_or_init(|| Mutex::new(None));
        *hook_state
            .lock()
            .map_err(|_| PlatformError::os("hotkey", "hook state mutex poisoned"))? =
            Some(HookState {
                sender: events.clone(),
                bindings: Vec::new(),
            });

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
        if let Some(state) = HOOK_STATE.get() {
            if let Ok(mut guard) = state.lock() {
                *guard = None;
            }
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
    events: Sender<WindowAction>,
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
            events,
            registered: Vec::new(),
            hook: None,
            next_id: 1,
            module: module.into(),
        };
        if startup_cancelled.load(Ordering::Acquire)
            || ready.send(Ok(GetCurrentThreadId())).is_err()
        {
            return;
        }
        if !await_start(&commands, &startup_cancelled) {
            return;
        }
        log::debug!("Windows hotkey owner thread entered its message loop");

        let mut msg: MSG = std::mem::zeroed();
        loop {
            let result = GetMessageW(&mut msg, Some(HWND(std::ptr::null_mut())), 0, 0).0;
            if result == 0 || result == -1 {
                break;
            }
            if msg.message == COMMAND_MESSAGE {
                if !drain_commands(&commands, &mut owner) {
                    break;
                }
                continue;
            }
            if msg.message == WM_HOTKEY {
                owner.dispatch_registered(msg.wParam.0 as i32);
                continue;
            }
            let _ = TranslateMessage(&msg);
            DispatchMessageW(&msg);
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
        let hook = unsafe {
            SetWindowsHookExW(
                WH_KEYBOARD_LL,
                Some(low_level_keyboard_proc),
                Some(self.module),
                0,
            )
            .map_err(|e| PlatformError::os("SetWindowsHookExW", e.message()))?
        };
        self.hook = Some(hook);
        Ok(())
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
        let _ = self.events.send(binding.binding.action);
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
        for binding in self.registered.drain(..) {
            let _ = unsafe { UnregisterHotKey(None, binding.id) };
        }
        if let Some(hook) = self.hook.take() {
            let _ = unsafe { UnhookWindowsHookEx(hook) };
        }
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
    HOOK_STATE
        .get()
        .and_then(|state| state.lock().ok())
        .and_then(|guard| guard.as_ref().map(|state| state.bindings.clone()))
        .unwrap_or_default()
}

fn set_hook_bindings(bindings: Vec<HookBinding>) -> Result<()> {
    let state = HOOK_STATE
        .get()
        .ok_or_else(|| PlatformError::os("hotkey", "hook state is not initialized"))?;
    let mut guard = state
        .lock()
        .map_err(|_| PlatformError::os("hotkey", "hook state mutex poisoned"))?;
    let hook_state = guard
        .as_mut()
        .ok_or_else(|| PlatformError::os("hotkey", "hook state is unavailable"))?;
    hook_state.bindings = bindings;
    Ok(())
}

unsafe extern "system" fn low_level_keyboard_proc(
    code: i32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    if code == HC_ACTION as i32 {
        let message = wparam.0 as u32;
        let kb = &*(lparam.0 as *const KBDLLHOOKSTRUCT);
        if kb.dwExtraInfo != INJECTED_TAG {
            let vk = kb.vkCode as u16;
            let extended = (kb.flags.0 & LLKHF_EXTENDED.0) != 0;
            if message == WM_KEYDOWN || message == WM_SYSKEYDOWN {
                if key_bit_is_set(&SWALLOWED_KEYS, vk, extended) {
                    if !is_modifier_vk(vk) {
                        let mods = current_modifiers();
                        if let Some(binding) = match_binding(vk, extended, mods) {
                            repeat_action(binding)
                                .into_iter()
                                .for_each(send_hook_action);
                        }
                    }
                    return LRESULT(1);
                }

                if !is_modifier_vk(vk) {
                    let mods = current_modifiers();
                    if let Some(binding) = match_binding(vk, extended, mods) {
                        let repeated = !binding.repeat && set_key_bit(&HELD_KEYS, vk, extended);
                        if !repeated {
                            send_hook_action(binding.action);
                        }
                        mark_swallowed(vk, extended);
                        if mods.contains(Modifiers::META) {
                            suppress_start_menu();
                        }
                        return LRESULT(1);
                    }
                }
            } else if message == WM_KEYUP || message == WM_SYSKEYUP {
                take_key_bit(&HELD_KEYS, vk, extended);
                if take_swallowed(vk, extended) {
                    return LRESULT(1);
                }
            }
        }
    }
    CallNextHookEx(Some(HHOOK(std::ptr::null_mut())), code, wparam, lparam)
}

fn repeat_action(binding: HookBinding) -> Option<WindowAction> {
    binding.repeat.then_some(binding.action)
}

fn match_binding(vk: u16, extended: bool, mods: Modifiers) -> Option<HookBinding> {
    let state = HOOK_STATE.get()?;
    let guard = state.lock().ok()?;
    match_binding_in(&guard.as_ref()?.bindings, vk, extended, mods)
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

fn send_hook_action(action: WindowAction) {
    let Some(state) = HOOK_STATE.get() else {
        return;
    };
    let Ok(guard) = state.lock() else {
        return;
    };
    if let Some(hook_state) = guard.as_ref() {
        let _ = hook_state.sender.send(action);
    }
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
}
