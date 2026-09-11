// src-tauri/src/main.rs
// M-T2 (Tauri repurpose): minimal skeleton — opens a 1280×800 window
// loading the static-exported Operations Canvas from console/out/.
// Backend orchestration (catalog + supervisor spawning) is added in M-T4.
//
// Prevents an additional console window on Windows in release builds.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

fn main() {
    tauri::Builder::default()
        .plugin(tauri_plugin_shell::init())
        .plugin(tauri_plugin_log::Builder::new().build())
        .plugin(tauri_plugin_window_state::Builder::default().build())
        .plugin(tauri_plugin_fs::init())
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_os::init())
        .plugin(tauri_plugin_process::init())
        .run(tauri::generate_context!())
        .expect("error while running RustSim GCS");
}
