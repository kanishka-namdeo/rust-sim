// src-tauri/src/shutdown.rs
// M-T4 (Tauri repurpose): graceful shutdown — called from
// `on_window_event(CloseRequested)`. Stops SITL via the supervisor, waits
// 250ms, then drops the catalog + supervisor Child handles so kill_on_drop
// fires.
//
// Spec: docs/TAURI_APP_SPEC.md Appendix E.4 + Appendix H.4 (algorithm).

use crate::BackendState;
use tauri::{AppHandle, Manager};
use tokio::process::Child;

/// Graceful shutdown: stop SITL, wait 250ms, then drop the Child handles.
/// Idempotent — safe to call multiple times.
pub async fn graceful_shutdown(app: &AppHandle) {
    log::info!("graceful shutdown starting");

    // Step 1: ask the supervisor to stop SITL (kills mavfleet + px4).
    let _ = reqwest::Client::new()
        .post("http://127.0.0.1:8500/api/sitl/stop")
        .json(&serde_json::json!({}))
        .timeout(std::time::Duration::from_secs(2))
        .send()
        .await;
    log::info!("supervisor /api/sitl/stop sent");

    // Step 2: wait 250ms for the supervisor to clean up the mavfleet/px4 tree.
    tokio::time::sleep(std::time::Duration::from_millis(250)).await;

    // Step 3: drop the Child handles — kill_on_drop takes over.
    let state: tauri::State<BackendState> = app.state();
    let catalog = state.catalog.lock().unwrap().take();
    let supervisor = state.supervisor.lock().unwrap().take();
    if let Some(c) = catalog {
        let _ = kill_child(c).await;
    }
    if let Some(c) = supervisor {
        let _ = kill_child(c).await;
    }
    log::info!("graceful shutdown complete");
}

/// Kill a tokio::process::Child. Try SIGTERM first (whole process group),
/// wait 500ms, then SIGKILL via start_kill.
async fn kill_child(mut child: Child) -> std::io::Result<()> {
    #[cfg(unix)]
    {
        if let Some(pid) = child.id() {
            // SIGTERM the whole process group (pgid = pid because we set
            // process_group(0) in the spawn call).
            let _ = std::process::Command::new("kill")
                .args(["-TERM", "-"])
                .arg(pid.to_string())
                .status();
            tokio::time::sleep(std::time::Duration::from_millis(500)).await;
        }
        // If still alive, SIGKILL via start_kill (tokio's kill() uses SIGKILL on Unix).
        let _ = child.start_kill();
    }
    #[cfg(not(unix))]
    {
        let _ = child.start_kill(); // Windows: TerminateProcess
    }
    let _ = child.wait().await;
    Ok(())
}
