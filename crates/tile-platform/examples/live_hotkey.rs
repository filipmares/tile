//! Live hotkey test: exercises the real Windows registration route, injects a
//! synthetic key combination, and checks that the bound action is delivered.
//!
//! Run with `cargo run --example live_hotkey -p tile-platform`. Uses an
//! obscure combination (Ctrl+Shift+M) so it cannot disturb a real session.

fn main() {
    #[cfg(windows)]
    windows_hotkey();
    #[cfg(not(windows))]
    println!("live_hotkey only runs on Windows");
}

#[cfg(windows)]
fn windows_hotkey() {
    use std::sync::mpsc::channel;
    use std::time::Duration;
    use tile_core::{Hotkey, KeyCode, Modifiers, WindowAction};
    use tile_platform::{hotkey_backend, HotkeyBinding, HotkeyRoute};
    use windows::Win32::UI::Input::KeyboardAndMouse::{
        SendInput, INPUT, INPUT_0, INPUT_KEYBOARD, KEYBDINPUT, KEYBD_EVENT_FLAGS, KEYEVENTF_KEYUP,
        VIRTUAL_KEY, VK_CONTROL, VK_LEFT, VK_LWIN, VK_MENU, VK_SHIFT,
    };

    const VK_M: VIRTUAL_KEY = VIRTUAL_KEY(0x4D);

    let (tx, rx) = channel();
    let mut backend = hotkey_backend(tx).expect("create hotkey backend");

    let hotkey = Hotkey::new(Modifiers::CONTROL | Modifiers::SHIFT, KeyCode::M);
    let report = backend
        .apply(&[HotkeyBinding {
            hotkey,
            action: WindowAction::Maximize,
            repeat: false,
        }])
        .expect("apply bindings");
    println!(
        "applied {hotkey} -> maximize via {:?} (hook installed: {})",
        report.bindings[0].route, report.hook_installed
    );
    assert_eq!(report.bindings[0].route, HotkeyRoute::Registered);
    assert!(
        !report.hook_installed,
        "a grantable shortcut must not install the hook"
    );

    // Give the hook thread a moment to install the hook.
    std::thread::sleep(Duration::from_millis(500));

    fn key(vk: VIRTUAL_KEY, up: bool) -> INPUT {
        INPUT {
            r#type: INPUT_KEYBOARD,
            Anonymous: INPUT_0 {
                ki: KEYBDINPUT {
                    wVk: vk,
                    wScan: 0,
                    dwFlags: if up {
                        KEYEVENTF_KEYUP
                    } else {
                        KEYBD_EVENT_FLAGS(0)
                    },
                    time: 0,
                    dwExtraInfo: 0,
                },
            },
        }
    }

    let sequence = [
        key(VK_CONTROL, false),
        key(VK_SHIFT, false),
        key(VK_M, false),
        key(VK_M, true),
        key(VK_SHIFT, true),
        key(VK_CONTROL, true),
    ];

    // SAFETY: `sequence` is a valid, correctly sized array of INPUT records
    // that lives for the duration of the call.
    let sent = unsafe { SendInput(&sequence, std::mem::size_of::<INPUT>() as i32) };
    println!("injected {sent} of {} key events", sequence.len());

    match rx.recv_timeout(Duration::from_secs(5)) {
        Ok(action) => {
            println!("registered route delivered: {}", action.id());
            assert_eq!(action, WindowAction::Maximize);
        }
        Err(err) => panic!("hook did not deliver the action: {err}"),
    }

    // A second press must fire again (the hook must not be one-shot).
    let sent2 = unsafe { SendInput(&sequence, std::mem::size_of::<INPUT>() as i32) };
    println!("injected {sent2} more key events");
    let second = rx.recv_timeout(Duration::from_secs(5));
    assert!(second.is_ok(), "hook stopped delivering after one press");
    println!("registered route delivered again: {}", second.unwrap().id());

    // Extra modifiers must not turn a base binding into a loose subset match.
    let superset = [
        key(VK_CONTROL, false),
        key(VK_SHIFT, false),
        key(VK_MENU, false),
        key(VK_M, false),
        key(VK_M, true),
        key(VK_MENU, true),
        key(VK_SHIFT, true),
        key(VK_CONTROL, true),
    ];
    unsafe { SendInput(&superset, std::mem::size_of::<INPUT>() as i32) };
    assert!(
        rx.recv_timeout(Duration::from_millis(500)).is_err(),
        "registered route fired for a modifier superset"
    );
    println!("registered route ignored an extra Alt modifier");

    // After unbinding, the same keystroke must be ignored.
    let cleared = backend.apply(&[]).expect("clear bindings");
    assert!(!cleared.hook_installed);
    std::thread::sleep(Duration::from_millis(300));
    unsafe { SendInput(&sequence, std::mem::size_of::<INPUT>() as i32) };
    let after_unbind = rx.recv_timeout(Duration::from_millis(1500));
    assert!(
        after_unbind.is_err(),
        "unbound hotkey still fired: {after_unbind:?}"
    );
    println!("unbound hotkey correctly ignored");

    // Win+Left is owned by Aero Snap, so Tile must route it through the hook and
    // consume it before the shell can move the foreground window.
    let contested = Hotkey::new(Modifiers::META, KeyCode::Left);
    let contested_binding = HotkeyBinding {
        hotkey: contested,
        action: WindowAction::LeftHalf,
        repeat: false,
    };
    let intercepted = backend
        .apply(&[contested_binding])
        .expect("apply contested binding");
    assert_eq!(
        intercepted.bindings[0].route,
        HotkeyRoute::Intercepted,
        "Win+Left should fall back to interception"
    );
    assert!(intercepted.hook_installed);

    let win_left = [
        key(VK_LWIN, false),
        key(VK_LEFT, false),
        key(VK_LEFT, true),
        key(VK_LWIN, true),
    ];
    unsafe { SendInput(&win_left, std::mem::size_of::<INPUT>() as i32) };
    assert_eq!(
        rx.recv_timeout(Duration::from_secs(5))
            .expect("intercepted Win+Left was not delivered"),
        WindowAction::LeftHalf
    );
    println!("intercepted route delivered left-half");

    backend.shutdown();
    backend.shutdown(); // must be idempotent
    println!("\nlive hotkey test passed");
}
