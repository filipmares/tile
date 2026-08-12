// Hide the console window on Windows in release builds; the app is tray-only.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

fn main() {
    tile_app::run();
}
