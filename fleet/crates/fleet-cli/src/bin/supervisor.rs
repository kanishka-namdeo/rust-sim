//! fleet-supervisor — the SITL lifecycle manager (ADR-0030).
//!
//! QGC and Mission Planner do NOT auto-spawn SITL when the GCS launches —
//! the operator starts SITL on demand (QGC: separate terminal or Mock Link
//! button; MP: Simulation tab with explicit Start/Stop). The rust-sim GCS
//! matches that pattern: this supervisor is the single REST entry point
//! the GCS UI uses to start/stop the mavfleet process (which spawns N PX4
//! SITL + sim pairs and serves the :8400 control plane).
//!
//! Routes on :8500:
//!   GET  /api/sitl/status     → {running, vehicle_count, pid, started_at_ms, run_dir, scenario}
//!   POST /api/sitl/start      → body {scenario?: string} — spawn mavfleet
//!   POST /api/sitl/stop       → kill the mavfleet child + its px4/sim grandchildren
//!   GET  /api/sitl/scenarios  → list the scenario TOMLs in fleet/tests/
//!   GET  /api/health          → {ok: true, service: "fleet-supervisor"}
//!
//! The supervisor is started by `scripts/stack_up.sh start` BEFORE the
//! fleet; the fleet is started on-demand when the operator clicks the
//! "Start SITL" button in the GCS UI (or POSTs /api/sitl/start directly).

#![forbid(unsafe_code)]

use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use axum::{
    extract::State,
    http::StatusCode,
    response::{IntoResponse, Response},
    routing::{get, post},
    Json, Router,
};
use clap::Parser;
use serde::Serialize;
use serde_json::json;
use tokio::process::{Child, Command};
use tokio::sync::Mutex;

// ---------------------------------------------------------------------------
// CLI
// ---------------------------------------------------------------------------

#[derive(Debug, Parser)]
#[command(
    name = "fleet-supervisor",
    version,
    about = "SITL lifecycle manager — start/stop the mavfleet process from the GCS UI"
)]
struct Cli {
    /// REST port (default :8500).
    #[arg(long, default_value_t = 8500)]
    port: u16,
    /// Path to the fleet workspace root (where mavfleet + scenario TOMLs live).
    /// Defaults to the parent of this binary's target/debug/ dir.
    #[arg(long, value_name = "DIR")]
    fleet_dir: Option<PathBuf>,
    /// Default scenario TOML (relative to fleet_dir/tests/).
    #[arg(long, default_value = "operator_session.toml")]
    default_scenario: String,
}

// ---------------------------------------------------------------------------
// AppState — the mavfleet child + status
// ---------------------------------------------------------------------------

struct AppState {
    /// The mavfleet child process, if running.
    child: Mutex<Option<Child>>,
    /// The PID of the mavfleet process (tracked separately so we can detect
    /// exit even if the `Child` is dropped without `kill`).
    pid: Mutex<Option<u32>>,
    /// Wall-clock ms when the fleet was started.
    started_at_ms: Mutex<Option<u64>>,
    /// The run-dir the mavfleet process is writing to.
    run_dir: Mutex<Option<PathBuf>>,
    /// The scenario TOML the fleet is running.
    scenario: Mutex<Option<String>>,
    /// The fleet workspace root (for the mavfleet binary + scenario TOMLs).
    fleet_dir: PathBuf,
    /// Default scenario TOML name.
    default_scenario: String,
}

#[derive(Debug, Serialize)]
struct SitlStatus {
    running: bool,
    vehicle_count: usize,
    pid: Option<u32>,
    started_at_ms: Option<u64>,
    run_dir: Option<String>,
    scenario: Option<String>,
}

#[derive(Debug, Serialize)]
struct ScenarioList {
    scenarios: Vec<String>,
    default: String,
}

// ---------------------------------------------------------------------------
// REST handlers
// ---------------------------------------------------------------------------

async fn health() -> Json<serde_json::Value> {
    Json(json!({"ok": true, "service": "fleet-supervisor"}))
}

async fn sitl_status(State(state): State<Arc<AppState>>) -> Json<serde_json::Value> {
    let pid_lock = state.pid.lock().await;
    let started_lock = state.started_at_ms.lock().await;
    let run_dir_lock = state.run_dir.lock().await;
    let scenario_lock = state.scenario.lock().await;
    // Check if the process is still alive.
    let mut child_lock = state.child.lock().await;
    let running = match child_lock.as_mut() {
        Some(child) => match child.try_wait() {
            Ok(None) => true,
            _ => {
                *child_lock = None;
                false
            }
        },
        None => false,
    };
    let pid = if running { *pid_lock } else { None };
    let started = if running { *started_lock } else { None };
    let run_dir = if running {
        run_dir_lock.as_ref().map(|p| p.to_string_lossy().to_string())
    } else {
        None
    };
    let scenario = if running { scenario_lock.clone() } else { None };
    let vehicle_count = if running {
        match reqwest::get("http://127.0.0.1:8400/api/fleet").await {
            Ok(resp) => {
                if let Ok(json) = resp.json::<serde_json::Value>().await {
                    json.get("data")
                        .and_then(|d| d.get("vehicles"))
                        .and_then(|v| v.as_array())
                        .map(|a| a.len())
                        .unwrap_or(0)
                } else {
                    0
                }
            }
            Err(_) => 0,
        }
    } else {
        0
    };
    let status = SitlStatus {
        running,
        vehicle_count,
        pid,
        started_at_ms: started,
        run_dir,
        scenario,
    };
    Json(json!({"ok": true, "data": status}))
}

#[derive(serde::Deserialize)]
struct SitlStartBody {
    /// Optional scenario TOML name (relative to fleet_dir/tests/). If absent,
    /// uses the default (operator_session.toml).
    scenario: Option<String>,
}

async fn sitl_start(
    State(state): State<Arc<AppState>>,
    Json(body): Json<SitlStartBody>,
) -> Response {
    // Refuse if already running.
    {
        let mut child = state.child.lock().await;
        if let Some(c) = child.as_mut() {
            if let Ok(None) = c.try_wait() {
                return error_response(
                    StatusCode::CONFLICT,
                    "SITL_RUNNING",
                    "SITL is already running; stop it first",
                );
            }
        }
        *child = None;
    }
    let scenario_name = body.scenario.unwrap_or_else(|| state.default_scenario.clone());
    let scenario_path = state.fleet_dir.join("tests").join(&scenario_name);
    if !scenario_path.exists() {
        return error_response(
            StatusCode::NOT_FOUND,
            "SCENARIO_NOT_FOUND",
            &format!(
                "scenario '{}' not found at {}",
                scenario_name,
                scenario_path.display()
            ),
        );
    }
    // Resolve the mavfleet binary: same dir as this binary (target/debug/).
    let mavfleet_bin = std::env::current_exe()
        .ok()
        .and_then(|p| p.parent().map(|d| d.join("mavfleet")))
        .unwrap_or_else(|| PathBuf::from("mavfleet"));
    if !mavfleet_bin.exists() {
        return error_response(
            StatusCode::INTERNAL_SERVER_ERROR,
            "MAVFLEET_BINARY_MISSING",
            &format!("mavfleet binary not found at {}", mavfleet_bin.display()),
        );
    }
    // Run-dir: <fleet_dir>/../scratch/fleet-run-<unix_s>.
    let run_dir = state
        .fleet_dir
        .parent()
        .unwrap_or(&state.fleet_dir)
        .join("scratch")
        .join(format!("fleet-run-{}", now_secs()));
    std::fs::create_dir_all(&run_dir).ok();

    let log_path = run_dir.join("mavfleet.log");
    let stdout_file = match std::fs::File::create(&log_path) {
        Ok(f) => f,
        Err(e) => {
            return error_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                "LOG_CREATE_FAILED",
                &format!("could not create log file {}: {e}", log_path.display()),
            );
        }
    };
    let stderr_file = match std::fs::OpenOptions::new().create(true).append(true).open(&log_path) {
        Ok(f) => f,
        Err(e) => {
            return error_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                "LOG_OPEN_FAILED",
                &format!("could not open log file {}: {e}", log_path.display()),
            );
        }
    };
    let stdout_stdio = Stdio::from(stdout_file);
    let stderr_stdio = Stdio::from(stderr_file);
    let px4_dir = std::env::var("FLEET_PX4_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|_| {
            state
                .fleet_dir
                .parent()
                .unwrap_or(&state.fleet_dir)
                .parent()
                .unwrap_or(&state.fleet_dir)
                .join("PX4-Autopilot")
        });
    let sim_cfg_dir = std::env::var("FLEET_SIM_CFG_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|_| state.fleet_dir.join("scratch").join("vsims"));
    let mut cmd = Command::new(&mavfleet_bin);
    cmd.arg("run")
        .arg("--fleet")
        .arg(&scenario_path)
        .arg("--api-port")
        .arg("8400")
        .arg("--run-dir")
        .arg(&run_dir)
        .env("FLEET_PX4_DIR", &px4_dir)
        .env("FLEET_SIM_CFG_DIR", &sim_cfg_dir)
        // Set cwd to the fleet workspace so the scenario TOML's relative
        // `scripts/run_sitsim_vehicle.sh` resolves (the script lives at
        // fleet/scripts/run_sitsim_vehicle.sh — repo-relative to the
        // fleet workspace root, NOT the rust-sim repo root).
        .current_dir(&state.fleet_dir)
        .stdout(stdout_stdio)
        .stderr(stderr_stdio)
        .kill_on_drop(true);
    let child = match cmd.spawn() {
        Ok(c) => c,
        Err(e) => {
            return error_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                "SPAWN_FAILED",
                &format!("could not spawn mavfleet: {e}"),
            );
        }
    };
    let pid = child.id();
    *state.child.lock().await = Some(child);
    *state.pid.lock().await = pid;
    *state.started_at_ms.lock().await = Some(now_ms());
    *state.run_dir.lock().await = Some(run_dir.clone());
    *state.scenario.lock().await = Some(scenario_name.clone());

    // Wait for the fleet to come up on :8400 (poll /api/fleet for up to 120s).
    let poll_deadline = std::time::Instant::now() + std::time::Duration::from_secs(120);
    let mut last_err = String::new();
    while std::time::Instant::now() < poll_deadline {
        match reqwest::get("http://127.0.0.1:8400/api/fleet").await {
            Ok(resp) => {
                if resp.status().is_success() {
                    let status = SitlStatus {
                        running: true,
                        vehicle_count: 0,
                        pid,
                        started_at_ms: Some(now_ms()),
                        run_dir: Some(run_dir.to_string_lossy().to_string()),
                        scenario: Some(scenario_name),
                    };
                    return Json(json!({"ok": true, "data": status})).into_response();
                }
            }
            Err(e) => {
                last_err = e.to_string();
            }
        }
        tokio::time::sleep(std::time::Duration::from_millis(500)).await;
    }
    error_response(
        StatusCode::REQUEST_TIMEOUT,
        "FLEET_START_TIMEOUT",
        &format!(
            "mavfleet spawned (pid {pid:?}) but did not answer on :8400 within 120s; last error: {last_err}"
        ),
    )
}

async fn sitl_stop(State(state): State<Arc<AppState>>) -> Response {
    let mut child_lock = state.child.lock().await;
    let child = match child_lock.as_mut() {
        Some(c) => c,
        None => {
            return error_response(StatusCode::CONFLICT, "SITL_NOT_RUNNING", "SITL is not running");
        }
    };
    if let Ok(Some(_status)) = child.try_wait() {
        *child_lock = None;
        *state.pid.lock().await = None;
        *state.started_at_ms.lock().await = None;
        *state.run_dir.lock().await = None;
        *state.scenario.lock().await = None;
        return Json(json!({"ok": true, "data": {"stopped": true, "already_exited": true}}))
            .into_response();
    }
    let _ = child.kill().await;
    let _ = child.wait().await;
    *child_lock = None;
    *state.pid.lock().await = None;
    *state.started_at_ms.lock().await = None;
    *state.run_dir.lock().await = None;
    *state.scenario.lock().await = None;
    // Kill any leaked px4/sitsim processes (defensive — the manager's
    // kill_on_drop should have reaped them, but be thorough).
    #[cfg(unix)]
    {
        let _ = std::process::Command::new("pkill")
            .arg("-f")
            .arg("run_sitsim_vehicle.sh")
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status();
        let _ = std::process::Command::new("pkill")
            .arg("-x")
            .arg("px4")
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status();
        let _ = std::process::Command::new("pkill")
            .arg("-f")
            .arg("sitsim-cli")
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status();
    }
    Json(json!({"ok": true, "data": {"stopped": true}})).into_response()
}

async fn sitl_scenarios(State(state): State<Arc<AppState>>) -> Json<serde_json::Value> {
    let tests_dir = state.fleet_dir.join("tests");
    let mut scenarios = Vec::new();
    if let Ok(entries) = std::fs::read_dir(&tests_dir) {
        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension().and_then(|s| s.to_str()) == Some("toml") {
                if let Some(name) = path.file_name().and_then(|s| s.to_str()) {
                    scenarios.push(name.to_string());
                }
            }
        }
    }
    scenarios.sort();
    Json(json!({
        "ok": true,
        "data": ScenarioList {
            scenarios,
            default: state.default_scenario.clone(),
        }
    }))
}

fn error_response(status: StatusCode, code: &str, message: &str) -> Response {
    (
        status,
        Json(json!({"ok": false, "error": {"code": code, "message": message}})),
    )
        .into_response()
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

fn now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

// ---------------------------------------------------------------------------
// Fleet dir resolution — default is the parent of this binary's dir, then
// walk up until we find the fleet workspace root.
// ---------------------------------------------------------------------------

fn resolve_fleet_dir(override_dir: Option<PathBuf>) -> PathBuf {
    if let Some(d) = override_dir {
        return d;
    }
    // Walk up from the binary's dir (target/debug/) until we find the
    // fleet workspace root (has Cargo.toml + crates/ + tests/).
    if let Some(dir) = std::env::current_exe()
        .ok()
        .and_then(|p| p.parent().map(|p| p.to_path_buf())) // target/debug
        .and_then(|p| p.parent().map(|p| p.to_path_buf())) // target
        .and_then(|p| p.parent().map(|p| p.to_path_buf())) // fleet-cli crate
        .and_then(|p| p.parent().map(|p| p.to_path_buf()))
    {
        // fleet workspace
        if dir.join("tests").exists() && dir.join("crates").exists() {
            return dir;
        }
    }
    // Fallback: assume CWD is the fleet dir.
    PathBuf::from(".")
}

#[tokio::main]
async fn main() {
    let cli = Cli::parse();
    let fleet_dir = resolve_fleet_dir(cli.fleet_dir);
    let state = Arc::new(AppState {
        child: Mutex::new(None),
        pid: Mutex::new(None),
        started_at_ms: Mutex::new(None),
        run_dir: Mutex::new(None),
        scenario: Mutex::new(None),
        fleet_dir: fleet_dir.clone(),
        default_scenario: cli.default_scenario,
    });

    let app = Router::new()
        .route("/api/health", get(health))
        .route("/api/sitl/status", get(sitl_status))
        .route("/api/sitl/start", post(sitl_start))
        .route("/api/sitl/stop", post(sitl_stop))
        .route("/api/sitl/scenarios", get(sitl_scenarios))
        .with_state(state);

    let addr = format!("127.0.0.1:{}", cli.port);
    eprintln!("[fleet-supervisor] listening on http://{addr}");
    eprintln!("[fleet-supervisor] fleet dir: {}", fleet_dir.display());
    let listener = match tokio::net::TcpListener::bind(&addr).await {
        Ok(l) => l,
        Err(e) => {
            eprintln!("[fleet-supervisor] FATAL: could not bind {addr}: {e}");
            std::process::exit(1);
        }
    };
    if let Err(e) = axum::serve(listener, app).await {
        eprintln!("[fleet-supervisor] server error: {e}");
        std::process::exit(1);
    }
}

// Suppress unused-import warnings for `Path` (used only in the type alias
// `Path` vs `PathBuf`; the compiler sometimes flags it). The `let _ = Path`
// line is a no-op to keep the import live without `#[allow]`.
#[allow(dead_code)]
const _: fn() = || {
    let _ = Path::new("");
};
