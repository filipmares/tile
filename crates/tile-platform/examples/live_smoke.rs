//! Live smoke test: drives the real Windows backend against a real window.
//!
//! Run with `cargo run --example live_smoke -p tile-platform`. It is an
//! example rather than a test because it needs an interactive desktop session
//! and a focused window, neither of which exist on a CI runner.

fn main() {
    #[cfg(windows)]
    windows_smoke();
    #[cfg(not(windows))]
    println!("live_smoke only runs on Windows");
}

#[cfg(windows)]
fn windows_smoke() {
    use tile_core::{Engine, Plan, WindowAction, WindowSnapshot};
    use tile_platform::window_backend;

    let backend = window_backend().expect("create backend");

    let screens = backend.screens().expect("enumerate screens");
    println!("screens: {}", screens.len());
    for s in &screens {
        println!(
            "  {} frame={:?} work={:?} scale={} primary={}",
            s.id, s.frame, s.work_area, s.scale_factor, s.is_primary
        );
    }

    // An explicit HWND can be passed as argv[1] so this can be driven from a
    // non-interactive shell, where there is no foreground window to detect.
    let explicit = std::env::args().nth(1).and_then(|a| a.parse::<u64>().ok());

    let window = match explicit {
        Some(id) => {
            let frame = backend
                .set_window_frame(id, tile_core::Rect::new(200.0, 200.0, 900.0, 700.0))
                .expect("seed the window position");
            println!("targeting explicit window id={id} frame={frame:?}");
            WindowSnapshot { id, frame }
        }
        None => match backend.focused_window().expect("focused window") {
            Some(w) => w,
            None => {
                println!("no focused window; pass an HWND as an argument instead");
                return;
            }
        },
    };
    println!("start: id={} frame={:?}", window.id, window.frame);

    let mut engine = Engine::default();
    let mut current = window;

    for action in [
        WindowAction::LeftHalf,
        WindowAction::RightHalf,
        WindowAction::TopHalf,
        WindowAction::BottomHalf,
        WindowAction::Maximize,
        WindowAction::Center,
        WindowAction::Restore,
    ] {
        match engine.plan(action, &current, &screens) {
            Plan::Move { id, target } => {
                let actual = backend.set_window_frame(id, target).expect("move window");
                engine.commit(action, &current, actual);
                let delta = (actual.x - target.x).abs()
                    + (actual.y - target.y).abs()
                    + (actual.width - target.width).abs()
                    + (actual.height - target.height).abs();
                println!(
                    "{:<11} target={target:?}\n            actual={actual:?} delta={delta}",
                    action.id()
                );
                assert!(
                    delta < 4.0,
                    "{} landed too far from its target",
                    action.id()
                );
                current = WindowSnapshot { id, frame: actual };
            }
            Plan::NoOp(reason) => println!("{:<11} no-op: {reason:?}", action.id()),
        }
        std::thread::sleep(std::time::Duration::from_millis(300));
    }

    println!("\nlive smoke test passed");
}
