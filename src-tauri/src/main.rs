// src-tauri/src/main.rs
// M-T4 (Tauri repurpose): full lifecycle — spawns catalog + supervisor at
// startup, registers IPC commands, gracefully shuts down on window close.
//
// Spec: docs/TAURI_APP_SPEC.md Appendix E.1.
//
// Prevents an additional console window on Windows in release builds.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod backends;
mod commands;
mod log_pipe;
mod shutdown;

use std::sync::Mutex;
use tauri::{Emitter, Manager};
use tauri_plugin_log::{Target, TargetKind};
use tokio::process::Child;

/// Shared state holding the spawned backend child handles.
/// `Mutex<Option<Child>>` (not `RwLock`): we only ever `.take()` and `.replace()`,
/// never read concurrently. Mutex is correct + cheaper than RwLock for this access pattern.
pub struct BackendState {
    pub catalog: Mutex<Option<Child>>,
    pub supervisor: Mutex<Option<Child>>,
}

fn main() {
    eprintln!("[rustsim-gcs] main() entered — Tauri builder starting...");
    // Initialise the logger BEFORE Tauri, so panics in setup() are logged.
    let log_dir = dirs::data_dir()
        .unwrap_or_else(|| std::path::PathBuf::from("."))
        .join("rustsim/logs");
    let _ = std::fs::create_dir_all(&log_dir);
    eprintln!("[rustsim-gcs] log_dir = {}", log_dir.display());

    tauri::Builder::default()
        .plugin(
            tauri_plugin_log::Builder::new()
                .level(log::LevelFilter::Info)
                .targets([
                    Target::new(TargetKind::Stdout),
                    Target::new(TargetKind::Folder {
                        path: log_dir.clone(),
                        file_name: Some("rustsim-gcs".to_string()),
                    })
                    .filter(|m| {
                        !m.target().contains("hyper")
                            && !m.target().contains("reqwest")
                            && !m.target().contains("h2")
                            && !m.target().contains("tokio_util")
                    }),
                    Target::new(TargetKind::Webview),
                ])
                .build(),
        )
        .plugin(tauri_plugin_shell::init())
        .plugin(tauri_plugin_window_state::Builder::default().build())
        .plugin(tauri_plugin_fs::init())
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_os::init())
        .plugin(tauri_plugin_process::init())
        .manage(BackendState {
            catalog: Mutex::new(None),
            supervisor: Mutex::new(None),
        })
        .invoke_handler(tauri::generate_handler![
            commands::backend_status,
            commands::restart_catalog,
            commands::restart_supervisor,
            commands::open_install_guide,
            commands::probe_px4,
            commands::reset_ports,
        ])
        .setup(|app| {
            eprintln!("[rustsim-gcs] setup() entered");
            let main_window = app.get_webview_window("main");
            eprintln!("[rustsim-gcs] setup: main window = {:?}", main_window.is_some());
            // Don't spawn backends if the user passed --help or there's no main window.
            if main_window.is_none() {
                eprintln!("[rustsim-gcs] setup: no main window — skipping backend spawn");
                return Ok(());
            }
            eprintln!("[rustsim-gcs] setup: spawning backends...");
            // M-T4: log synchronously here so we can confirm setup() runs even
            // if the async task hasn't been polled yet.
            log::info!("M-T4 setup: spawning backends...");
            let handle = app.handle().clone();
            tauri::async_runtime::spawn(async move {
                log::info!("M-T4 async task: probing ports...");
                eprintln!("[rustsim-gcs] M-T4 async task: probing ports...");
                if let Err(e) = backends::probe_and_clean_ports().await {
                    log::warn!("port probe/clean failed: {e}");
                    eprintln!("[rustsim-gcs] port probe/clean failed: {e}");
                }
                log::info!("M-T4 async task: spawning catalog...");
                eprintln!("[rustsim-gcs] M-T4 async task: spawning catalog...");
                if let Err(e) = backends::spawn_catalog(&handle).await {
                    log::error!("failed to spawn catalog: {e}");
                    eprintln!("[rustsim-gcs] failed to spawn catalog: {e}");
                    let _ = handle.emit(
                        "backend-error",
                        &serde_json::json!({"backend": "catalog", "error": e.to_string()}),
                    );
                }
                log::info!("M-T4 async task: spawning supervisor...");
                eprintln!("[rustsim-gcs] M-T4 async task: spawning supervisor...");
                if let Err(e) = backends::spawn_supervisor(&handle).await {
                    log::error!("failed to spawn supervisor: {e}");
                    eprintln!("[rustsim-gcs] failed to spawn supervisor: {e}");
                    let _ = handle.emit(
                        "backend-error",
                        &serde_json::json!({"backend": "supervisor", "error": e.to_string()}),
                    );
                }
                eprintln!("[rustsim-gcs] M-T4 async task: probing PX4...");
                backends::probe_px4(&handle).await;
                eprintln!("[rustsim-gcs] M-T4 async task: done.");
            });
            Ok(())
        })
        .on_window_event(|window, event| {
            if let tauri::WindowEvent::CloseRequested { api, .. } = event {
                // Prevent the window from closing until backends are torn down.
                api.prevent_close();
                let app = window.app_handle().clone();
                tauri::async_runtime::spawn(async move {
                    shutdown::graceful_shutdown(&app).await;
                    if let Some(win) = app.get_webview_window("main") {
                        let _ = win.close();
                    }
                });
            }
        })
        .run(tauri::generate_context!())
        .expect("error while running RustSim GCS");
}
