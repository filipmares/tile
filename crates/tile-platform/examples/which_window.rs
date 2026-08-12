//! Read-only diagnostic: prints which window Tile would act on, without
//! moving anything.
//!
//! Run with `cargo run --example which_window -p tile-platform`. Useful for
//! checking that Tile resolves the *user's* window rather than its own tray
//! window when an action is triggered from the tray menu.

fn main() {
    #[cfg(windows)]
    windows_report();
    #[cfg(not(windows))]
    println!("which_window only runs on Windows");
}

#[cfg(windows)]
fn windows_report() {
    use tile_platform::window_backend;
    use windows::Win32::Foundation::HWND;
    use windows::Win32::UI::WindowsAndMessaging::{
        GetClassNameW, GetForegroundWindow, GetWindowTextW, GetWindowThreadProcessId,
    };

    fn describe(hwnd: HWND) -> String {
        let mut title = [0u16; 256];
        let mut class = [0u16; 256];
        let mut pid = 0u32;
        // SAFETY: read-only queries against a handle supplied by the OS, with
        // correctly sized buffers.
        unsafe {
            let tlen = GetWindowTextW(hwnd, &mut title) as usize;
            let clen = GetClassNameW(hwnd, &mut class) as usize;
            GetWindowThreadProcessId(hwnd, Some(&mut pid as *mut u32));
            format!(
                "hwnd={:?} pid={pid} class='{}' title='{}'",
                hwnd.0,
                String::from_utf16_lossy(&class[..clen]),
                String::from_utf16_lossy(&title[..tlen])
            )
        }
    }

    // SAFETY: takes no arguments and returns a handle or null.
    let fg = unsafe { GetForegroundWindow() };
    if fg.0.is_null() {
        println!("foreground window: <none>");
    } else {
        println!("foreground window: {}", describe(fg));
    }

    let backend = window_backend().expect("create backend");
    match backend.focused_window().expect("resolve focused window") {
        Some(w) => {
            let hwnd = HWND(w.id as *mut std::ffi::c_void);
            println!("Tile would act on: {}", describe(hwnd));
            println!("             frame: {:?}", w.frame);
        }
        None => println!("Tile would act on: <nothing>"),
    }

    // The tray-menu path: when Tile itself is foreground, this Z-order scan is
    // what resolves the user's window instead.
    match tile_platform::windows::topmost_manageable_window() {
        Some(hwnd) => println!("tray fallback picks: {}", describe(hwnd)),
        None => println!("tray fallback picks: <nothing>"),
    }
}
