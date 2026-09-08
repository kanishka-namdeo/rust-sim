//! Per-vehicle process supervision (spec §2.4, §12): spawn and tear down the
//! px4 + simulator pair for each vehicle instance, using the verified
//! multi-instance recipe (`Tools/simulation/sitl_multiple_run.sh` pattern).
//!
//! Bring-up order per vehicle i (0-based):
//! 1. create the instance working directory under the run's sandbox;
//! 2. start the simulator with its HIL TCP port 4560+i assigned;
//! 3. wait for the sim to be listening (the interim sim has no status API,
//!    so this is a settle delay — ADR-0001);
//! 4. start `px4 -i i -d <build>/etc` with `PX4_SIM_MODEL=gazebo-classic_iris`
//!    and the instance directory as cwd;
//! 5. telemetry readiness (BOOTING -> READY) is gated by the manager's link
//!    task, not by this crate.
//!
//! The per-vehicle **simulator command is a template** (spec §16's
//! rustsitsim runtime dependency is deferred, ADR-0001): `{hil_port}`,
//! `{instance}`, `{sysid}`, `{duration_s}` and `{sitsim}` /
//! `{sim_script}` placeholders are substituted per vehicle. The template is
//! split on whitespace into an argv (no shell); quoting hazards are avoided
//! by construction.
//!
//! Default resolution (first match wins):
//! 1. the scenario's `[sim] command` template (ADR-0008);
//! 2. the `FLEET_SIM_COMMAND` environment variable (same template shape);
//! 3. the interim simulator: `python3 {sim_script} {hil_port} {duration_s}`
//!    where `{sim_script}` is the vendored `scripts/sim_stream.py` (pinned
//!    CLI: two positional arguments, LISTEN_PORT and DURATION seconds).
//!    `FLEET_SITSIM_BIN`, when set, additionally provides the `{sitsim}`
//!    substitution for scenarios that want to pin the rustsitsim binary
//!    explicitly once its CLI lands.

#![forbid(unsafe_code)]

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::Duration;

use tokio::process::{Child, Command};

/// Per-instance port map (spec §17, appendix A).
pub mod ports {
    /// HIL TCP port the simulator listens on and px4 connects to.
    pub const fn hil_tcp(instance: u8) -> u16 {
        4560 + instance as u16
    }
    /// Telemetry UDP port the manager's link binds (PX4 streams here).
    pub const fn telemetry_udp(instance: u8) -> u16 {
        14540 + instance as u16
    }
    /// PX4's onboard MAVLink listen port (manager sends commands here).
    pub const fn onboard_udp(instance: u8) -> u16 {
        14580 + instance as u16
    }
    /// Simulator control API (rustsitsim; reserved, ADR-0001).
    pub const fn sim_api(instance: u8) -> u16 {
        8200 + instance as u16
    }
}

/// Environment keys consulted by the default resolution.
pub const ENV_PX4_DIR: &str = "FLEET_PX4_DIR";
pub const ENV_SIM_COMMAND: &str = "FLEET_SIM_COMMAND";
pub const ENV_SITSIM_BIN: &str = "FLEET_SITSIM_BIN";
pub const ENV_SIM_SCRIPT: &str = "FLEET_SIM_SCRIPT";

/// Default px4 checkout location in the sandbox.
pub const DEFAULT_PX4_DIR: &str = "/home/z/my-project/PX4-Autopilot";

/// How long the sim gets to bind its TCP listener before px4 starts.
pub const SIM_SETTLE_MS: u64 = 1500;

#[derive(Debug, Clone)]
pub struct SimCtlConfig {
    /// PX4 checkout root (contains `build/px4_sitl_default`).
    pub px4_dir: PathBuf,
    /// Explicit sim command template override (scenario `[sim] command`).
    pub sim_command_template: Option<String>,
    /// Seconds the interim sim should stream (must outlast the run).
    pub sim_duration_s: f64,
    /// Settle delay before px4 launch.
    pub sim_settle: Duration,
    /// When true, px4/sim stdout+stderr are redirected to
    /// `<workdir>/px4.log` / `<workdir>/sim.log` instead of /dev/null —
    /// the run's diagnostics (F-1 failure dumps need the px4 console tail).
    pub process_logs: bool,
}

impl SimCtlConfig {
    pub fn from_env(sim_command_template: Option<String>, sim_duration_s: f64) -> Self {
        let px4_dir = std::env::var(ENV_PX4_DIR)
            .map(PathBuf::from)
            .unwrap_or_else(|_| PathBuf::from(DEFAULT_PX4_DIR));
        SimCtlConfig {
            px4_dir,
            sim_command_template: sim_command_template
                .or_else(|| std::env::var(ENV_SIM_COMMAND).ok()),
            sim_duration_s,
            sim_settle: Duration::from_millis(SIM_SETTLE_MS),
            process_logs: false,
        }
    }

    /// Resolve the sim argv template for one vehicle.
    pub fn sim_template(&self) -> String {
        if let Some(t) = &self.sim_command_template {
            return t.clone();
        }
        let script = std::env::var(ENV_SIM_SCRIPT)
            .unwrap_or_else(|_| find_repo_file("scripts/sim_stream.py"));
        format!("python3 {script} {{hil_port}} {{duration_s}}")
    }

    pub fn px4_binary(&self) -> PathBuf {
        self.px4_dir
            .join("build/px4_sitl_default/bin/px4")
    }

    pub fn px4_etc(&self) -> PathBuf {
        self.px4_dir.join("build/px4_sitl_default/etc")
    }

    /// Stdio for a child process: appended log file under the instance
    /// workdir when `process_logs`, /dev/null otherwise (Stdio::from a
    /// File is seekable-append, so shared clones are fine).
    fn process_stdio(&self, workdir: &Path, name: &str) -> Result<Stdio, SpawnError> {
        if !self.process_logs {
            return Ok(Stdio::null());
        }
        let path = workdir.join(name);
        let file = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&path)
            .map_err(SpawnError::Io)?;
        Ok(Stdio::from(file))
    }
}

/// Locate a repo-relative file by walking up from the executable / cwd.
fn find_repo_file(rel: &str) -> String {
    let mut candidates: Vec<PathBuf> = Vec::new();
    if let Ok(manifest) = std::env::var("CARGO_MANIFEST_DIR") {
        candidates.push(PathBuf::from(manifest).join("../../").join(rel));
    }
    if let Ok(exe) = std::env::current_exe() {
        // target/debug/mavfleet -> repo root is 3 levels up
        let mut p: Option<&Path> = exe.parent();
        for _ in 0..3 {
            if let Some(parent) = p {
                candidates.push(parent.join(rel));
                p = parent.parent();
            }
        }
    }
    if let Ok(cwd) = std::env::current_dir() {
        candidates.push(cwd.join(rel));
        candidates.push(cwd.join("mavfleet").join(rel));
    }
    for c in &candidates {
        if c.exists() {
            return c.display().to_string();
        }
    }
    // fall back to the relative path; the spawn error will be legible
    rel.to_string()
}

/// Substitute template placeholders for one vehicle.
pub fn render_template(
    template: &str,
    instance: u8,
    duration_s: f64,
) -> Vec<String> {
    let hil_port = ports::hil_tcp(instance).to_string();
    let mut map: HashMap<&str, String> = HashMap::new();
    map.insert("hil_port", hil_port.clone());
    map.insert("instance", instance.to_string());
    map.insert("sysid", (instance + 1).to_string());
    map.insert("duration_s", format!("{duration_s:.0}"));
    if let Ok(bin) = std::env::var(ENV_SITSIM_BIN) {
        map.insert("sitsim", bin);
    } else {
        map.insert("sitsim", "sitsim-cli".to_string());
    }
    map.insert(
        "sim_script",
        std::env::var(ENV_SIM_SCRIPT)
            .unwrap_or_else(|_| find_repo_file("scripts/sim_stream.py")),
    );
    let rendered = template.replace("{hil_port}", &hil_port);
    let rendered = rendered
        .replace("{instance}", &map["instance"])
        .replace("{sysid}", &map["sysid"])
        .replace("{duration_s}", &map["duration_s"])
        .replace("{sitsim}", &map["sitsim"])
        .replace("{sim_script}", &map["sim_script"]);
    rendered.split_whitespace().map(|s| s.to_string()).collect()
}

/// One vehicle's supervised process pair.
#[derive(Debug)]
pub struct VehicleProcesses {
    pub instance: u8,
    pub sim: Child,
    pub px4: Child,
    /// Working directory (ulogs, params, airframe state land here).
    pub workdir: PathBuf,
}

impl VehicleProcesses {
    /// `true` when either process has exited (try_wait without reaping).
    pub fn any_dead(&mut self) -> Option<(bool, bool)> {
        let sim = self.sim.try_wait().ok().flatten().is_some();
        let px4 = self.px4.try_wait().ok().flatten().is_some();
        if sim || px4 {
            Some((sim, px4))
        } else {
            None
        }
    }

    /// SIGKILL both children and reap them. Idempotent.
    pub async fn kill(&mut self) {
        let _ = self.sim.start_kill();
        let _ = self.px4.start_kill();
        let _ = tokio::time::timeout(Duration::from_secs(5), async {
            let _ = self.sim.wait().await;
            let _ = self.px4.wait().await;
        })
        .await;
    }
}

/// The supervisor for one fleet's processes.
pub struct SimCtl {
    pub cfg: SimCtlConfig,
    vehicles: Vec<VehicleProcesses>,
}

#[derive(Debug)]
pub enum SpawnError {
    Io(std::io::Error),
    MissingPx4(PathBuf),
}

impl std::fmt::Display for SpawnError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            SpawnError::Io(e) => write!(f, "spawn io error: {e}"),
            SpawnError::MissingPx4(p) => write!(
                f,
                "px4 binary not found at {} (set FLEET_PX4_DIR)",
                p.display()
            ),
        }
    }
}

impl SimCtl {
    pub fn new(cfg: SimCtlConfig) -> Self {
        SimCtl {
            cfg,
            vehicles: Vec::new(),
        }
    }

    pub fn len(&self) -> usize {
        self.vehicles.len()
    }

    pub fn is_empty(&self) -> bool {
        self.vehicles.is_empty()
    }

    /// Spawn the sim+px4 pair for `instance`, with `run_dir/vehicle_{i}` as
    /// the px4 working directory (the sitl_multiple_run.sh pattern, but
    /// sandboxed per run so there is no cross-run state, spec §12.3).
    pub async fn spawn_vehicle(&mut self, instance: u8, run_dir: &Path) -> Result<(), SpawnError> {
        let px4_bin = self.cfg.px4_binary();
        if !px4_bin.exists() {
            return Err(SpawnError::MissingPx4(px4_bin));
        }
        let workdir = run_dir.join(format!("vehicle_{instance}"));
        std::fs::create_dir_all(&workdir)
            .map_err(SpawnError::Io)?;

        // 1-2: simulator first (px4's rcS blocks on the HIL stream).
        let template = self.cfg.sim_template();
        let argv = render_template(&template, instance, self.cfg.sim_duration_s);
        let program = argv.first().cloned().unwrap_or_else(|| "python3".into());
        let sim_out = self.cfg.process_stdio(&workdir, "sim.log")?;
        let sim_err = self.cfg.process_stdio(&workdir, "sim.log")?;
        let sim = Command::new(program)
            .args(&argv[1..])
            .stdout(sim_out)
            .stderr(sim_err)
            .kill_on_drop(true)
            .spawn()
            .map_err(SpawnError::Io)?;

        // 3: settle so the sim's TCP listener is bound (ADR-0001: the
        // interim sim has no status API to poll; do NOT probe-connect,
        // which would consume its single accept()).
        tokio::time::sleep(self.cfg.sim_settle).await;

        // 4: px4 with the shared etc dir, cwd = per-run instance dir.
        let px4_etc = self.cfg.px4_etc();
        let px4_out = self.cfg.process_stdio(&workdir, "px4.log")?;
        let px4_err = self.cfg.process_stdio(&workdir, "px4.log")?;
        let px4 = Command::new(&px4_bin)
            .arg("-i")
            .arg(instance.to_string())
            .arg("-d")
            .arg(&px4_etc)
            .current_dir(&workdir)
            .env("PX4_SIM_MODEL", "gazebo-classic_iris")
            .stdout(px4_out)
            .stderr(px4_err)
            .kill_on_drop(true)
            .spawn()
            .map_err(SpawnError::Io)?;

        self.vehicles.push(VehicleProcesses {
            instance,
            sim,
            px4,
            workdir,
        });
        Ok(())
    }

    /// Poll all pairs; returns instances whose processes died.
    pub fn dead_vehicles(&mut self) -> Vec<(u8, bool, bool)> {
        let mut dead = Vec::new();
        for v in &mut self.vehicles {
            if let Some((sim_dead, px4_dead)) = v.any_dead() {
                dead.push((v.instance, sim_dead, px4_dead));
            }
        }
        dead
    }

    /// Remove a vehicle's handle from the list (after a fault, restart
    /// spawns a fresh pair under the same instance).
    pub fn drop_vehicle(&mut self, instance: u8) {
        self.vehicles.retain(|v| v.instance != instance);
    }

    /// Controlled vehicle restart for the vehicle-setup workflow
    /// (ADR-0016: apply airframe = write `SYS_AUTOSTART` + restart the
    /// sim+px4 pair, exactly QGroundControl's "apply and reboot" flow).
    ///
    /// The whole pair restarts (the interim sim accepts a single HIL TCP
    /// connection — a respawned px4 cannot reuse it), but the **working
    /// directory is preserved**, so the persisted `parameters.bson` keeps
    /// the just-written `SYS_AUTOSTART` (PX4 autosaves param changes) and
    /// rcS applies the new airframe on boot. Because the pair is dropped
    /// from the tracked list *before* the kill, the process-death
    /// supervision (§2.4) does not trip a FAULT — the restart is an
    /// intentional operator action, logged by the caller.
    ///
    /// Returns the spawn error, if any (the old pair is already gone).
    pub async fn restart_vehicle(&mut self, instance: u8, run_dir: &Path) -> Result<(), SpawnError> {
        if let Some(mut v) = self.take_vehicle(instance) {
            v.kill().await;
        }
        self.spawn_vehicle(instance, run_dir).await
    }

    /// Take (remove) one vehicle's process pair out of the tracked list.
    fn take_vehicle(&mut self, instance: u8) -> Option<VehicleProcesses> {
        let idx = self.vehicles.iter().position(|v| v.instance == instance)?;
        Some(self.vehicles.remove(idx))
    }

    /// Kill and reap everything (teardown, F-1's probe requirement).
    pub async fn kill_all(&mut self) {
        for v in &mut self.vehicles {
            v.kill().await;
        }
        self.vehicles.clear();
    }
}

/// Post-teardown port verification (F-1): all per-instance TCP/UDP ports in
/// §17's map must be free. UDP bind checks apply on 0.0.0.0 (the manager
/// binds 127.0.0.1, but a leaked socket on any address is a failure).
pub fn ports_free(instances: &[u8]) -> (bool, Vec<String>) {
    let mut failures = Vec::new();
    for &i in instances {
        let hil = ports::hil_tcp(i);
        let tel = ports::telemetry_udp(i);
        let onb = ports::onboard_udp(i);
        // TCP: connect must be refused (nothing listening)
        if std::net::TcpStream::connect(("127.0.0.1", hil)).is_ok() {
            failures.push(format!("tcp {hil} still accepting"));
        }
        // UDP: bindable means free
        for (port, name) in [(tel, "telemetry"), (onb, "onboard")] {
            if std::net::UdpSocket::bind(("0.0.0.0", port)).is_err() {
                failures.push(format!("udp {name} {port} still bound"));
            }
        }
    }
    (failures.is_empty(), failures)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn port_map_per_spec_17() {
        assert_eq!(ports::hil_tcp(0), 4560);
        assert_eq!(ports::hil_tcp(1), 4561);
        assert_eq!(ports::telemetry_udp(0), 14540);
        assert_eq!(ports::telemetry_udp(1), 14541);
        assert_eq!(ports::onboard_udp(2), 14582);
        assert_eq!(ports::sim_api(3), 8203);
    }

    #[test]
    fn template_renders_placeholders() {
        let argv = render_template(
            "python3 {sim_script} {hil_port} {duration_s}",
            1,
            240.0,
        );
        assert_eq!(argv[0], "python3");
        assert!(argv[1].ends_with("sim_stream.py") || argv[1].contains("sim_stream"));
        assert_eq!(argv[2], "4561");
        assert_eq!(argv[3], "240");
        let argv = render_template("{sitsim} run --port {hil_port} --sysid {sysid}", 2, 10.0);
        assert_eq!(argv[3], "4562");
        assert!(argv.contains(&"--sysid".to_string()));
        assert!(argv.contains(&"3".to_string()));
    }

    #[test]
    fn ports_free_on_quiet_machine() {
        // 14549/4559 are NOT in the per-instance map for instances 0..1 but
        // instance 7's map should be free on an idle test machine (ports
        // 4567, 14547, 14587 are never bound in unit tests). If something
        // legitimately squats one of these on a CI host this test is the
        // canary, which is the point of the probe.
        let (ok, failures) = ports_free(&[7]);
        if !ok {
            eprintln!("port probe failed (host squatting?): {failures:?}");
        }
        assert!(ok, "unexpectedly busy ports: {failures:?}");
    }

    #[tokio::test]
    async fn spawn_missing_px4_is_a_clean_error() {
        let mut cfg = SimCtlConfig::from_env(None, 10.0);
        cfg.px4_dir = PathBuf::from("/nonexistent/px4");
        cfg.sim_settle = Duration::from_millis(0);
        let mut ctl = SimCtl::new(cfg);
        let dir = std::env::temp_dir().join("mavfleet-simctl-test");
        let err = ctl.spawn_vehicle(0, &dir).await.err().unwrap();
        assert!(matches!(err, SpawnError::MissingPx4(_)));
        assert_eq!(ctl.len(), 0);
    }
}
