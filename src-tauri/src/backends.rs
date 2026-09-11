// src-tauri/src/backends.rs
// M-T4 (Tauri repurpose): spawn the catalog + supervisor Rust binaries as
// tokio::process::Command children with kill_on_drop(true). The webview
// then talks to them directly via http://127.0.0.1:<port> — no IPC needed
// for the data plane.
//
// Spec: docs/TAURI_APP_SPEC.md Appendix E.2 (adapted: actual binary flags
// are --port + --catalog-dir / --fleet-dir, not the env vars the spec used).

use std::path::PathBuf;
use std::process::Stdio;
use tauri::{AppHandle, Emitter, Manager};
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::process::Command;

use crate::BackendState;

pub const CATALOG_PORT: u16 = 8300;
pub const FLEET_PORT: u16 = 8400;
pub const SUPERVISOR_PORT: u16 = 8500;

/// Resolve the path to a fleet-workspace binary. Looks for, in order:
///   1. `<repo_root>/fleet/target/release/<name>`  (release build)
///   2. `<repo_root>/fleet/target/debug/<name>`    (debug build — what stack_up.sh uses)
///   3. `<app_resource_dir>/bin/<name>`            (packaged install — future M-T7)
///   4. `<name>` on PATH                           (rare; for distro-packaged installs)
/// Returns an absolute PathBuf. Errors if none found.
pub fn resolve_binary(name: &str, app: &AppHandle) -> anyhow::Result<PathBuf> {
    // (1) and (2): repo-relative (dev mode)
    let here = PathBuf::from(env!("CARGO_MANIFEST_DIR")); // src-tauri/
    let repo_root = here.parent().unwrap();                 // rust-sim/
    for profile in &["release", "debug"] {
        let dev_path = repo_root.join("fleet/target").join(profile).join(name);
        if dev_path.is_file() {
            return Ok(dev_path);
        }
    }

    // (3) Packaged install (production — M-T7 will wire `resources` in tauri.conf.json)
    if let Ok(resource_dir) = app.path().resource_dir() {
        let pkg_path = resource_dir.join("bin").join(name);
        if pkg_path.is_file() {
            return Ok(pkg_path);
        }
    }

    // (4) PATH lookup (rare — distro install)
    if let Ok(p) = which::which(name) {
        return Ok(p);
    }

    anyhow::bail!(
        "binary '{}' not found in repo ({}/fleet/target/{{release,debug}}/{}), \
         resource dir, or PATH",
        name,
        repo_root.display(),
        name
    )
}

/// Resolve PX4_ROOT. Order:
///   1. `PX4_ROOT` env var if set and the px4 binary exists under it
///   2. `<repo_root>/../PX4-Autopilot`  (matches `scripts/stack_up.sh:43`)
///   3. `<repo_root>/PX4-Autopilot`     (the symlink layout used in the sandbox)
///   4. `~/.rustsim/PX4-Autopilot`      (per-user install for non-developer operators)
///   5. `/opt/rustsim/PX4-Autopilot`   (system-wide install — rare)
/// Returns the directory containing the `build/px4_sitl_default/bin/px4` binary.
pub fn resolve_px4_root() -> Option<PathBuf> {
    let here = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let repo_root = here.parent().unwrap().to_path_buf();

    let mut candidates: Vec<PathBuf> = vec![];
    if let Ok(env) = std::env::var("PX4_ROOT") {
        candidates.push(PathBuf::from(env));
    }
    if let Ok(c) = repo_root.join("../PX4-Autopilot").canonicalize() {
        candidates.push(c);
    }
    candidates.push(repo_root.join("PX4-Autopilot"));
    if let Some(home) = dirs::home_dir() {
        candidates.push(home.join(".rustsim/PX4-Autopilot"));
    }
    candidates.push(PathBuf::from("/opt/rustsim/PX4-Autopilot"));

    for c in candidates {
        let px4_bin = c.join("build/px4_sitl_default/bin/px4");
        if px4_bin.is_file() {
            return Some(c);
        }
    }
    None
}

/// Emit a `px4-status` event to all webview windows with the current PX4 discovery result.
pub async fn probe_px4(app: &AppHandle) {
    let found = resolve_px4_root();
    let _ = app.emit(
        "px4-status",
        &serde_json::json!({
            "found": found.is_some(),
            "path": found.as_ref().map(|p| p.display().to_string()),
        }),
    );
}

/// Spawn the catalog binary on :8300.
/// On success, stores the Child handle in BackendState.
pub async fn spawn_catalog(app: &AppHandle) -> anyhow::Result<()> {
    let bin = resolve_binary("fleet-catalog", app)?;
    let data_dir = dirs::data_dir()
        .ok_or_else(|| anyhow::anyhow!("no data dir"))?
        .join("rustsim/catalog");
    std::fs::create_dir_all(&data_dir)?;

    let mut cmd = Command::new(&bin);
    cmd.arg("--port").arg(CATALOG_PORT.to_string())
       .arg("--catalog-dir").arg(&data_dir)
       .env("RUST_LOG", "info")
       .stdin(Stdio::null())
       .stdout(Stdio::piped())
       .stderr(Stdio::piped())
       .kill_on_drop(true);

    // On Unix, spawn in a new process group so we can SIGTERM the whole tree.
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        cmd.process_group(0);
    }

    log::info!(
        "spawning catalog: {} --port {} --catalog-dir {}",
        bin.display(),
        CATALOG_PORT,
        data_dir.display()
    );
    let mut child = cmd.spawn()?;

    // Pipe stdout/stderr to the log crate + emit to webview.
    if let Some(stdout) = child.stdout.take() {
        let app2 = app.clone();
        tauri::async_runtime::spawn(async move {
            let mut reader = BufReader::new(stdout).lines();
            while let Ok(Some(line)) = reader.next_line().await {
                log::info!("[catalog] {line}");
                let _ = app2.emit(
                    "backend-stdout",
                    &serde_json::json!({"backend": "catalog", "line": line}),
                );
            }
        });
    }
    if let Some(stderr) = child.stderr.take() {
        let app2 = app.clone();
        tauri::async_runtime::spawn(async move {
            let mut reader = BufReader::new(stderr).lines();
            while let Ok(Some(line)) = reader.next_line().await {
                log::warn!("[catalog:err] {line}");
                let _ = app2.emit(
                    "backend-stderr",
                    &serde_json::json!({"backend": "catalog", "line": line}),
                );
            }
        });
    }

    let state: tauri::State<BackendState> = app.state();
    *state.catalog.lock().unwrap() = Some(child);
    Ok(())
}

/// Spawn the supervisor binary on :8500.
pub async fn spawn_supervisor(app: &AppHandle) -> anyhow::Result<()> {
    let bin = resolve_binary("fleet-supervisor", app)?;
    let px4_root = resolve_px4_root();

    // The supervisor needs to know where the fleet workspace is (to find
    // scenarios + the mavfleet binary). Default to the repo's fleet/ dir.
    let here = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let repo_root = here.parent().unwrap();
    let fleet_dir = repo_root.join("fleet");

    let mut cmd = Command::new(&bin);
    cmd.arg("--port").arg(SUPERVISOR_PORT.to_string())
       .arg("--fleet-dir").arg(&fleet_dir)
       .env("RUST_LOG", "info")
       .stdin(Stdio::null())
       .stdout(Stdio::piped())
       .stderr(Stdio::piped())
       .kill_on_drop(true);

    // Propagate PX4_ROOT to the supervisor so it can pass it to mavfleet
    // when the operator starts SITL.
    if let Some(px4) = &px4_root {
        cmd.env("PX4_ROOT", px4);
        cmd.env("FLEET_PX4_DIR", px4);
    }

    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        cmd.process_group(0);
    }

    log::info!(
        "spawning supervisor: {} --port {} --fleet-dir {} (PX4_ROOT={})",
        bin.display(),
        SUPERVISOR_PORT,
        fleet_dir.display(),
        px4_root.as_ref().map(|p| p.display().to_string()).unwrap_or_else(|| "<not found>".into())
    );
    let mut child = cmd.spawn()?;

    if let Some(stdout) = child.stdout.take() {
        let app2 = app.clone();
        tauri::async_runtime::spawn(async move {
            let mut reader = BufReader::new(stdout).lines();
            while let Ok(Some(line)) = reader.next_line().await {
                log::info!("[supervisor] {line}");
                let _ = app2.emit(
                    "backend-stdout",
                    &serde_json::json!({"backend": "supervisor", "line": line}),
                );
            }
        });
    }
    if let Some(stderr) = child.stderr.take() {
        let app2 = app.clone();
        tauri::async_runtime::spawn(async move {
            let mut reader = BufReader::new(stderr).lines();
            while let Ok(Some(line)) = reader.next_line().await {
                log::warn!("[supervisor:err] {line}");
                let _ = app2.emit(
                    "backend-stderr",
                    &serde_json::json!({"backend": "supervisor", "line": line}),
                );
            }
        });
    }

    let state: tauri::State<BackendState> = app.state();
    *state.supervisor.lock().unwrap() = Some(child);
    Ok(())
}

/// Kill any stale processes squatting :8300 / :8400 / :8500. Called on
/// startup before spawning. Returns Ok if all ports are free (after recovery).
pub async fn probe_and_clean_ports() -> anyhow::Result<()> {
    for port in [CATALOG_PORT, FLEET_PORT, SUPERVISOR_PORT] {
        if port_is_listening(port).await {
            log::warn!("port {} already in use — attempting recovery", port);
            if let Some(pid) = pid_listening_on_port(port) {
                log::warn!("port {} held by pid {} — killing", port, pid);
                kill_pid(pid)?;
            } else {
                anyhow::bail!("port {} in use but owner unknown; aborting", port);
            }
        }
    }
    Ok(())
}

async fn port_is_listening(port: u16) -> bool {
    tokio::net::TcpStream::connect(format!("127.0.0.1:{port}"))
        .await
        .is_ok()
}

fn pid_listening_on_port(port: u16) -> Option<u32> {
    #[cfg(unix)]
    {
        let out = std::process::Command::new("lsof")
            .args(["-ti", &format!("tcp:{port}")])
            .output()
            .ok()?;
        let pid_str = String::from_utf8_lossy(&out.stdout);
        let pid_str = pid_str.lines().next()?;
        pid_str.trim().parse().ok()
    }
    #[cfg(windows)]
    {
        let out = std::process::Command::new("netstat")
            .args(["-ano", "-p", "TCP"])
            .output()
            .ok()?;
        let stdout = String::from_utf8_lossy(&out.stdout);
        for line in stdout.lines() {
            if line.contains(&format!(":{}", port)) && line.contains("LISTENING") {
                let pid: u32 = line.split_whitespace().last()?.parse().ok()?;
                return Some(pid);
            }
        }
        None
    }
    #[cfg(not(any(unix, windows)))]
    {
        None
    }
}

fn kill_pid(pid: u32) -> anyhow::Result<()> {
    #[cfg(unix)]
    {
        std::process::Command::new("kill")
            .args(["-TERM", &pid.to_string()])
            .status()?;
        std::thread::sleep(std::time::Duration::from_millis(500));
        std::process::Command::new("kill")
            .args(["-KILL", &pid.to_string()])
            .status()
            .map(|_| ())
            .map_err(|e| anyhow::anyhow!("kill failed: {e}"))
    }
    #[cfg(windows)]
    {
        std::process::Command::new("taskkill")
            .args(["/F", "/T", "/PID", &pid.to_string()])
            .status()
            .map(|_| ())
            .map_err(|e| anyhow::anyhow!("taskkill failed: {e}"))
    }
    #[cfg(not(any(unix, windows)))]
    {
        anyhow::bail!("kill_pid not implemented on this platform")
    }
}
