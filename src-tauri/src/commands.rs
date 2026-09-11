// src-tauri/src/commands.rs
// M-T4 (Tauri repurpose): the 6 IPC commands exposed to the frontend.
// Spec: docs/TAURI_APP_SPEC.md Appendix E.3 + Appendix G (contracts).
//
// The frontend calls these via `invoke('command_name', {args})` from
// @tauri-apps/api/core. All other backend traffic (REST + WebSocket) goes
// direct from the webview to http://127.0.0.1:<port> — no IPC needed.

use crate::backends::{self, resolve_px4_root};
use crate::BackendState;
use serde::Serialize;
use tauri::{AppHandle, Emitter, Manager};
use tauri_plugin_shell::ShellExt;

#[derive(Serialize)]
pub struct BackendStatus {
    pub catalog: BackendHealth,
    pub supervisor: BackendHealth,
    pub px4: Px4Status,
}

#[derive(Serialize)]
pub struct BackendHealth {
    pub port: u16,
    pub online: bool,
    pub pid: Option<u32>,
}

#[derive(Serialize)]
pub struct Px4Status {
    pub found: bool,
    pub path: Option<String>,
}

/// `invoke('backend_status')` — called by the frontend's Settings → About
/// panel every 2s. Returns the health of catalog, supervisor, and PX4.
/// Does NOT spawn; this is a pure probe.
#[tauri::command]
pub async fn backend_status(app: AppHandle) -> Result<BackendStatus, String> {
    let catalog = probe_health(8300, &app).await;
    let supervisor = probe_health(8500, &app).await;
    let px4_path = resolve_px4_root();
    Ok(BackendStatus {
        catalog,
        supervisor,
        px4: Px4Status {
            found: px4_path.is_some(),
            path: px4_path.as_ref().map(|p| p.display().to_string()),
        },
    })
}

async fn probe_health(port: u16, app: &AppHandle) -> BackendHealth {
    let url = format!("http://127.0.0.1:{port}/api/health");
    let online = match reqwest::Client::new()
        .get(&url)
        .timeout(std::time::Duration::from_millis(500))
        .send()
        .await
    {
        Ok(r) => r.status().is_success(),
        Err(_) => false,
    };
    let pid = {
        let state: tauri::State<BackendState> = app.state();
        let m = if port == 8300 {
            &state.catalog
        } else {
            &state.supervisor
        };
        let pid = m.lock()
            .unwrap()
            .as_ref()
            .and_then(|c| c.id());
        pid
    };
    BackendHealth { port, online, pid }
}

/// `invoke('restart_catalog')` — kills + respawns the catalog. Called from
/// Settings → Restart Catalog button.
#[tauri::command]
pub async fn restart_catalog(app: AppHandle) -> Result<(), String> {
    // Take the child out of the shared state in an inner block so the
    // MutexGuard (and its borrow of `app.state()`) is dropped BEFORE we await
    // child.kill() — otherwise the compiler complains `state` doesn't live
    // long enough. Binding to `child` (then returning it from the block)
    // forces the MutexGuard temporary to drop at the `;` rather than at the
    // end of the block, which is what the borrow-checker needs.
    let child = {
        let state: tauri::State<BackendState> = app.state();
        let child = state.catalog.lock().unwrap().take();
        child
    };
    if let Some(mut child) = child {
        let _ = child.kill().await;
    }
    backends::spawn_catalog(&app).await.map_err(|e| e.to_string())
}

/// `invoke('restart_supervisor')` — kills + respawns the supervisor.
/// DANGER: also kills any running SITL.
#[tauri::command]
pub async fn restart_supervisor(app: AppHandle) -> Result<(), String> {
    // First, ask the supervisor to stop SITL gracefully (if running).
    let _ = reqwest::Client::new()
        .post("http://127.0.0.1:8500/api/sitl/stop")
        .json(&serde_json::json!({}))
        .timeout(std::time::Duration::from_secs(2))
        .send()
        .await;
    // Then kill + respawn the supervisor itself.
    let child = {
        let state: tauri::State<BackendState> = app.state();
        let child = state.supervisor.lock().unwrap().take();
        child
    };
    if let Some(mut child) = child {
        let _ = child.kill().await;
    }
    backends::spawn_supervisor(&app).await.map_err(|e| e.to_string())
}

/// `invoke('open_install_guide')` — opens the PX4 install doc URL in the OS browser.
#[tauri::command]
pub async fn open_install_guide(app: AppHandle) -> Result<(), String> {
    app.shell()
        .open(
            "https://github.com/kanishka-namdeo/rust-sim/blob/main/docs/SANDBOX_SETUP.md",
            None,
        )
        .map_err(|e| e.to_string())
}

/// `invoke('probe_px4')` — re-probe PX4_ROOT (e.g. after the operator
/// installed PX4). Emits `px4-status` event.
#[tauri::command]
pub async fn probe_px4(app: AppHandle) -> Result<(), String> {
    backends::probe_px4(&app).await;
    Ok(())
}

/// `invoke('reset_ports')` — kill any stale process holding :8300 / :8400 / :8500. DANGER.
#[tauri::command]
pub async fn reset_ports(_app: AppHandle) -> Result<(), String> {
    backends::probe_and_clean_ports()
        .await
        .map_err(|e| e.to_string())
}
