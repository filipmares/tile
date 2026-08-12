//! Live hotkey test: installs the real low-level keyboard hook, injects a
//! synthetic key combination, and checks that the bound action is delivered.
//!
//! Run with `cargo run --example live_hotkey -p tile-platform`. Uses an
//! obscure combination (Ctrl+Alt+Shift+M) so it cannot disturb a real session.

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
    use tile_platform::hotkey_backend;
    use windows::Win32::UI::Input::KeyboardAndMouse::{
        SendInput, INPUT, INPUT_0, INPUT_KEYBOARD, KEYBDINPUT, KEYBD_EVENT_FLAGS, KEYEVENTF_KEYUP,
        VIRTUAL_KEY, VK_CONTROL, VK_MENU, VK_SHIFT,
    };

    const VK_M: VIRTUAL_KEY = VIRTUAL_KEY(0x4D);

    let (tx, rx) = channel();
    let mut backend = hotkey_backend(tx).expect("create hotkey backend");

    let hotkey = Hotkey::new(
        Modifiers::CONTROL | Modifiers::ALT | Modifiers::SHIFT,
        KeyCode::M,
    );
    let failures = backend
        .apply(&[(hotkey, WindowAction::Maximize)])
        .expect("apply bindings");
    println!(
        "registered {hotkey} -> maximize (failures: {})",
        failures.len()
    );
    assert!(failures.is_empty(), "binding should register cleanly");

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
        key(VK_MENU, false),
        key(VK_SHIFT, false),
        key(VK_M, false),
        key(VK_M, true),
        key(VK_SHIFT, true),
        key(VK_MENU, true),
        key(VK_CONTROL, true),
    ];

    // SAFETY: `sequence` is a valid, correctly sized array of INPUT records
    // that lives for the duration of the call.
    let sent = unsafe { SendInput(&sequence, std::mem::size_of::<INPUT>() as i32) };
    println!("injected {sent} of {} key events", sequence.len());

    match rx.recv_timeout(Duration::from_secs(5)) {
        Ok(action) => {
            println!("hook delivered: {}", action.id());
            assert_eq!(action, WindowAction::Maximize);
        }
        Err(err) => panic!("hook did not deliver the action: {err}"),
    }

    // A second press must fire again (the hook must not be one-shot).
    let sent2 = unsafe { SendInput(&sequence, std::mem::size_of::<INPUT>() as i32) };
    println!("injected {sent2} more key events");
    let second = rx.recv_timeout(Duration::from_secs(5));
    assert!(second.is_ok(), "hook stopped delivering after one press");
    println!("hook delivered again: {}", second.unwrap().id());

    // After unbinding, the same keystroke must be ignored.
    backend.apply(&[]).expect("clear bindings");
    std::thread::sleep(Duration::from_millis(300));
    unsafe { SendInput(&sequence, std::mem::size_of::<INPUT>() as i32) };
    let after_unbind = rx.recv_timeout(Duration::from_millis(1500));
    assert!(
        after_unbind.is_err(),
        "unbound hotkey still fired: {after_unbind:?}"
    );
    println!("unbound hotkey correctly ignored");

    backend.shutdown();
    backend.shutdown(); // must be idempotent
    println!("\nlive hotkey test passed");
}
