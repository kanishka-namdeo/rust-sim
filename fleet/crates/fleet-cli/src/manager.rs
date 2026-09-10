//! The mavfleet run engine / supervisor (Task 7b lean version):
//! - spawn N PX4 SITL + sim pairs (via `fleet-simctl`)
//! - bind N MAVLink links (via `fleet-mavlink`)
//! - aggregate per-vehicle state into FleetFrame at 10 Hz
//! - serve the REST + WS control plane on :8400 (via `crate::api`)
//! - drain operator commands (mission upload / start / clear) queued by
//!   the REST plane (the supervisor is the single writer of the task
//!   table)
//! - handle controlled vehicle restarts (airframe apply, ADR-0016)
//! - e-stop → land + disarm + abort
//! - teardown: kill all processes, verify §17 ports are free
//!
//! What this supervisor does NOT do (the autonomy removed in Task 7b):
//! - drive the auction allocator (no `auction()` / `reallocate()` calls —
//!   `fleet-alloc` is gone)
//! - drive the mission runner (no `MissionRunner::tick()` —
//!   `fleet-mission::runner` is gone)
//! - drive the 8-policy safety ladder (no `policy.evaluate()` —
//!   `fleet-safety::policy` is gone; the geofence stays as a passive
//!   observation that updates the FleetFrame's geofence block)
//! - fire timeline fault events (no `fire_timeline()` for fault events —
//!   the sim fault plane (`/api/faults`) is gone from this side; the
//!   operator or the sim itself owns fault injection now)
//! - drain hot-load scenario swaps (no `PUT /api/fleet` hot-restart loop)
//! - write a run-report.json (the run-report / success-criteria module
//!   is gone; the operator-facing summary is the events.ndjson + the
//!   `GET /api/events` tail)

#![forbid(unsafe_code)]

use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::atomic::Ordering;
use std::sync::Arc;
use std::time::Duration;

use tokio::sync::mpsc::UnboundedReceiver;

use fleet_core::events::{fleet_epoch_ms, set_fleet_epoch, unix_now, EventKind, EventLog};
use fleet_core::fsm::{FsmCause, FsmState};
use fleet_core::health::{compute_flags, FlagEdge};
use fleet_core::registry::Registry;
use fleet_core::state::VehicleState;
use fleet_core::TaskStatus;
use fleet_mavlink::{spawn_link, CmdAck, LinkConfig, LinkHandle, TelemetryEvent};
use fleet_modes::{decode_mode_word, MainMode, MODE_WORD_OFFBOARD, MAV_MODE_FLAG_SAFETY_ARMED};
use fleet_safety::geofence::Geofence;
use fleet_simctl::{ports_free, SimCtl, SimCtlConfig};

use crate::api;
use crate::config::FleetConfig;
use crate::state::{
    AppState, ClearAck, OperatorCmd, OperatorWaypoint, StartAck, UploadAck, VehicleTaskInfo,
};

/// Scenario arg parsing failure / infrastructure error exit code.
pub const EXIT_ERROR: i32 = 3;
/// Aborted (e-stop or catastrophic all-fault) exit code.
pub const EXIT_ABORTED: i32 = 2;
/// READY gate: home may be missing this long before we open the gate anyway
/// (the interim sim streams HOME_POSITION only on the manager's request).
const HOME_FALLBACK_MS: u64 = 45_000;
/// RTL → LANDED disarm-observation budget. With real rustsitsim dynamics the
/// full RTL cycle is climb-to-return-alt + return leg + descend + touchdown
/// + PX4 auto-disarm — measured 60–90 s end-to-end. 120 s is the watchdog.
const LAND_TIMEOUT_MS: u64 = 120_000;
/// Grace between abort and teardown so final ACKs/statustexts land.
const ABORT_GRACE_MS: u64 = 4_000;
/// Default hard wall-clock cap (5 minutes). Scenarios with a `[sim]
/// duration_s` that exceeds this still cap at this; scenarios without one
/// use this directly. The lean manager has no `[success] max_time_s` to
/// respect (the run-report / success-criteria module is gone).
const HARD_DEADLINE_MS: u64 = 300_000;

pub struct RunArgs {
    pub fleet: PathBuf,
    pub api_port: u16,
    pub run_dir: Option<PathBuf>,
}

// ---------------------------------------------------------------------------
// Command plumbing (§3.2): async tasks, results logged, never awaited in tick
// ---------------------------------------------------------------------------

fn result_name(r: u8) -> &'static str {
    match r {
        0 => "ACCEPTED",
        1 => "TEMPORARILY_REJECTED",
        2 => "DENIED",
        3 => "UNSUPPORTED",
        4 => "FAILED",
        5 => "IN_PROGRESS",
        _ => "UNKNOWN",
    }
}

fn log_ack(log: &Arc<EventLog>, index: u8, label: &str, ack: CmdAck) {
    match ack {
        CmdAck::Accepted => log.log(
            EventKind::SupervisorAction,
            Some(index),
            format!("{label} ACKed (MAV_RESULT_ACCEPTED)"),
        ),
        CmdAck::Rejected { result } => log.log(
            EventKind::LinkEscalation,
            Some(index),
            format!("{label} rejected: result={result} ({})", result_name(result)),
        ),
        CmdAck::Timeout { attempts } => log.log(
            EventKind::LinkEscalation,
            Some(index),
            format!("{label}: no COMMAND_ACK after {attempts} attempts (500 ms retry ladder, §3.2)"),
        ),
    }
}

/// §3.3 engage sequence: the HoldAt command already started the setpoint
/// stream; arm (if needed), wait 200 ms, then DO_SET_MODE(OFFBOARD).
/// TEMPORARILY_REJECTED arms are retried in-ladder: PX4 rejects early
/// attempts while sensors/EKF2 settle, and a one-shot arm dead-locks the
/// mission behind a transient health state (§3.2 retry semantics).
/// ADR-0017: the operator REST plane reuses these exact engage sequences
/// (crate-visible), so a go-to arm ladder is byte-identical to the
/// supervisor's own mission engage.
pub(crate) fn spawn_engage(handle: LinkHandle, log: Arc<EventLog>, index: u8, arm: bool) {
    tokio::spawn(async move {
        if arm {
            const ARM_ATTEMPTS: u8 = 4;
            const ARM_RETRY_MS: u64 = 1500;
            let mut attempt: u8 = 0;
            loop {
                attempt += 1;
                let ack = handle
                    .send_command(fleet_mavlink::cmds::COMPONENT_ARM_DISARM, [1.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0])
                    .await;
                let transient = matches!(
                    &ack,
                    CmdAck::Rejected { result: 1 } | CmdAck::Rejected { result: 5 }
                );
                log_ack(&log, index, "COMPONENT_ARM_DISARM(arm)", ack);
                if !transient || attempt >= ARM_ATTEMPTS {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(ARM_RETRY_MS)).await;
            }
        }
        tokio::time::sleep(Duration::from_millis(200)).await;
        let ack = handle.set_mode(MODE_WORD_OFFBOARD).await;
        log_ack(&log, index, "DO_SET_MODE(OFFBOARD, 0x00060000)", ack);
    });
}

pub(crate) fn spawn_land_and_disarm(handle: LinkHandle, log: Arc<EventLog>, index: u8) {
    tokio::spawn(async move {
        let ack = handle.set_mode(fleet_modes::MODE_WORD_AUTO_LAND).await;
        log_ack(&log, index, "DO_SET_MODE(AUTO.LAND, 0x06040000)", ack);
        let ack = handle
            .send_command(fleet_mavlink::cmds::COMPONENT_ARM_DISARM, [0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0])
            .await;
        log_ack(&log, index, "COMPONENT_ARM_DISARM(disarm)", ack);
    });
}

// ---------------------------------------------------------------------------
// Telemetry aggregation (single writer per vehicle slot, §5.1)
// ---------------------------------------------------------------------------

async fn aggregate(
    mut rx: UnboundedReceiver<TelemetryEvent>,
    slot: Arc<fleet_core::state::Slot<VehicleState>>,
    index: u8,
) {
    while let Some(ev) = rx.recv().await {
        let now = fleet_epoch_ms();
        slot.write(|s| {
            s.last_msg_ms = now;
            match ev {
                TelemetryEvent::Heartbeat { sysid, compid, base_mode, custom_mode } => {
                    if sysid == index + 1 {
                        s.sysid = sysid;
                        s.compid = compid as u16;
                        s.heartbeat_seen = true;
                        s.mode_word = custom_mode;
                        s.armed = base_mode & MAV_MODE_FLAG_SAFETY_ARMED != 0;
                        s.last_heartbeat_ms = now;
                    }
                }
                TelemetryEvent::Attitude { roll, pitch, yaw, .. } => {
                    s.set_attitude_euler(roll, pitch, yaw)
                }
                TelemetryEvent::LocalPositionNed { x, y, z, vx, vy, vz, .. } => {
                    s.position_ned_m = [x, y, z];
                    s.velocity_ned_ms = [vx, vy, vz];
                    s.local_position_seen = true;
                    s.position_samples = s.position_samples.saturating_add(1);
                }
                TelemetryEvent::GlobalPositionInt { lat, lon, alt, relative_alt } => {
                    s.lat_deg_e7 = lat;
                    s.lon_deg_e7 = lon;
                    s.alt_mm = alt;
                    s.relative_alt_mm = relative_alt;
                }
                TelemetryEvent::SysStatus { battery_remaining_pct, voltage_battery_mv, .. } => {
                    s.battery_pct = battery_remaining_pct;
                    s.voltage_v = voltage_battery_mv as f32 / 1000.0;
                }
                TelemetryEvent::BatteryStatus { battery_remaining_pct, voltage_mv, .. } => {
                    s.battery_pct = battery_remaining_pct;
                    s.voltage_v = voltage_mv as f32 / 1000.0;
                }
                TelemetryEvent::HomePosition { x, y, z, .. } => {
                    s.home_ned_m = [x, y, z];
                    s.home_set = true;
                }
                TelemetryEvent::StatusText { severity, text } => {
                    s.last_statustext = Some(format!("[sev {severity}] {text}"));
                }
                _ => {}
            }
        });
    }
}

// ---------------------------------------------------------------------------
// Supervisor
// ---------------------------------------------------------------------------

struct VehCtx {
    index: u8,
    handle: LinkHandle,
    flag_edge: FlagEdge,
    /// NAV_DLL_ACT=0 param write issued once per vehicle (ADR-0009).
    nav_dll_done: bool,
    rtl_since_ms: Option<u64>,
    landed_since_ms: Option<u64>,
    saw_offboard_echo: bool,
    saw_rtl_echo: bool,
    gate_since_ms: Option<u64>,
}

impl VehCtx {
    fn new(index: u8, handle: LinkHandle) -> Self {
        VehCtx {
            index,
            handle,
            flag_edge: FlagEdge::default(),
            nav_dll_done: false,
            rtl_since_ms: None,
            landed_since_ms: None,
            saw_offboard_echo: false,
            saw_rtl_echo: false,
            gate_since_ms: None,
        }
    }
}

enum Outcome {
    Continue,
    Stop,
}

struct Supervisor {
    fence: Geofence,
    registry: Arc<Registry>,
    api: Arc<AppState>,
    log: Arc<EventLog>,
    simctl: SimCtl,
    ctxs: Vec<VehCtx>,
    abort_grace_until: Option<u64>,
    hard_deadline_ms: u64,
    /// ADR-0017: monotonic id for operator-uploaded tasks (`op1`, `op2`, …).
    operator_seq: u32,
}

/// Resolve the per-vehicle sim command template. Priority (§12 / ADR-0008):
/// scenario `[sim] command` → `FLEET_SIM_COMMAND` env (handled inside
/// SimCtlConfig) → the interim simulator with an interpreter that actually
/// has pymavlink (the sandbox `python3` venv does not; /usr/bin/python3.13
/// does — probed once at startup).
fn resolve_sim_command(config: &FleetConfig, log: &EventLog) -> Option<String> {
    if let Some(t) = config.sim_command() {
        return Some(t);
    }
    if std::env::var(fleet_simctl::ENV_SIM_COMMAND).is_ok() {
        return None; // SimCtlConfig picks the env template up
    }
    let mut candidates: Vec<String> = Vec::new();
    if let Ok(p) = std::env::var("FLEET_SIM_PYTHON") {
        candidates.push(p);
    }
    candidates.push("python3.13".into());
    candidates.push("python3".into());
    for c in candidates {
        let ok = std::process::Command::new(&c)
            .arg("-c")
            .arg("import pymavlink")
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .map(|s| s.success())
            .unwrap_or(false);
        if ok {
            log.log(
                EventKind::RunBoundary,
                None,
                format!("sim interpreter: {c} (pymavlink import verified at startup)"),
            );
            return Some(format!("{c} {{sim_script}} {{hil_port}} {{duration_s}}"));
        }
    }
    None
}

// ---------------------------------------------------------------------------
// run_scenario: the whole lifecycle
// ---------------------------------------------------------------------------

pub async fn run_scenario(args: RunArgs) -> i32 {
    set_fleet_epoch();
    // Task 7b: the hot scenario-load loop (PUT /api/fleet) was part of the
    // removed scenario DSL; the lean manager is a single run.
    let scenario_path = args.fleet.clone();
    let run_dir: PathBuf = args.run_dir.clone().unwrap_or_else(|| {
        PathBuf::from(format!("fleet-runs/fleet-run-{}", unix_now()))
    });
    let (code, _api, server) = run_once(&scenario_path, args.api_port, &run_dir).await;
    // Drop the control plane (close the listener; open WS sockets drop
    // with it — clients reconnect on the next run).
    server.abort();
    let _ = server.await.is_ok();
    code
}

/// One complete run: parse -> spawn -> supervise -> teardown.
async fn run_once(
    config_path: &Path,
    api_port: u16,
    run_dir: &Path,
) -> (i32, Arc<AppState>, tokio::task::JoinHandle<()>) {
    let config = match FleetConfig::parse_file(config_path) {
        Ok(c) => c,
        Err(e) => {
            eprintln!("[fleet] config error: {e}");
            return (EXIT_ERROR, AppState::new_for_error(), dummy_server());
        }
    };
    if std::fs::create_dir_all(run_dir).is_err() {
        eprintln!("[fleet] cannot create run dir {}", run_dir.display());
        return (EXIT_ERROR, AppState::new_for_error(), dummy_server());
    }
    println!("[fleet] mavfleet run: config {}", config_path.display());
    println!("[fleet] run dir: {}", run_dir.display());

    let log = EventLog::spawn_writer(run_dir.join("events.ndjson"));
    let count = config.count();
    let geo_origin = config.geo_origin();
    let fence = config.fence();

    let registry = Registry::new(count);
    let api = AppState::new(
        Arc::clone(&registry),
        Arc::clone(&log),
        count,
        config_path.display().to_string(),
        unix_now(),
        geo_origin,
        fence.clone(),
        run_dir.to_path_buf(),
    );

    log.log(
        EventKind::RunBoundary,
        None,
        format!(
            "run started: config {}, {count} vehicles, geofence {} vertices / ceiling {} m, tick 10 Hz",
            config_path.display(),
            fence.points.len(),
            fence.ceiling_m
        ),
    );

    // Process supervision config.
    let sim_template = resolve_sim_command(&config, &log);
    let mut sim_cfg = SimCtlConfig::from_env(sim_template, config.sim_duration_s());
    sim_cfg.process_logs = true;
    sim_cfg.geo_origin = geo_origin; // ADR-0017: RSIM_ORIGIN_* for the sim wrapper
    // Scenario [env] -> sim physics (RSIM_WIND_MS / RSIM_TURBULENCE for the
    // wrapper): the declared wind/turbulence now actually reaches the sims.
    sim_cfg.sim_env = vec![
        (
            "RSIM_WIND_MS".into(),
            format!(
                "{:.3},{:.3},{:.3}",
                config.wind_steady_ms()[0],
                config.wind_steady_ms()[1],
                config.wind_steady_ms()[2]
            ),
        ),
        ("RSIM_TURBULENCE".into(), config.turbulence().into()),
    ];
    let mut simctl = SimCtl::new(sim_cfg);

    // Links (bind 14540+i BEFORE px4 boots, §3.1) + aggregators.
    let mut ctxs = Vec::new();
    for i in 0..count {
        match spawn_link(LinkConfig::for_instance(i)) {
            Ok((handle, ev_rx)) => {
                let slot = Arc::clone(&registry.entry(i).unwrap().state);
                tokio::spawn(aggregate(ev_rx, slot, i));
                // register for the setup control plane (ADR-0016)
                api.set_link(i, handle.clone());
                ctxs.push(VehCtx::new(i, handle));
            }
            Err(e) => {
                eprintln!("[fleet] FATAL: telemetry link bind failed for vehicle {i}: {e}");
                return (EXIT_ERROR, Arc::clone(&api), dummy_server());
            }
        }
    }

    // Spawn vehicles (sim first, settle, then px4; 2 s stagger, §2.4/R-10).
    for i in 0..count {
        registry.apply_transition(i, FsmCause::Spawn, &log);
        match simctl.spawn_vehicle(i, run_dir).await {
            Ok(()) => {
                registry.apply_transition(i, FsmCause::Px4Launched, &log);
                println!(
                    "[fleet] vehicle {i}: sim + px4 spawned (hil tcp {}, telemetry {}, onboard {})",
                    fleet_simctl::ports::hil_tcp(i),
                    fleet_simctl::ports::telemetry_udp(i),
                    fleet_simctl::ports::onboard_udp(i)
                );
            }
            Err(e) => {
                registry.apply_transition(i, FsmCause::SpawnFailure, &log);
                log.log(
                    EventKind::SupervisorAction,
                    Some(i),
                    format!("spawn failed: {e}"),
                );
                eprintln!("[fleet] vehicle {i} spawn FAILED: {e}");
            }
        }
        if i + 1 < count {
            tokio::time::sleep(Duration::from_secs(2)).await;
        }
    }

    // Control plane (§3.4). The task handle is returned to the run loop
    // so it can be aborted (listener freed) before a rebind.
    let server_task = {
        let api_task = Arc::clone(&api);
        let port = api_port;
        tokio::spawn(async move {
            if let Err(e) = api::serve(api_task, port).await {
                eprintln!("[fleet] control plane error: {e}");
            }
        })
    };
    api.set_phase(if config.hold_for_setup() { "SETUP_HOLD" } else { "BRING_UP" });

    let now0 = fleet_epoch_ms();
    let hard_deadline_ms = now0 + HARD_DEADLINE_MS;

    let sup = Supervisor {
        fence,
        registry,
        api: Arc::clone(&api),
        log,
        simctl,
        ctxs,
        abort_grace_until: None,
        hard_deadline_ms,
        operator_seq: 0,
    };
    let code = sup.run().await;
    (code, api, server_task)
}

/// A never-scheduled no-op server handle for the early-error paths (the
/// process exits immediately after; the handle is only there to keep the
/// return shape uniform).
fn dummy_server() -> tokio::task::JoinHandle<()> {
    tokio::spawn(async {})
}

impl Supervisor {
    /// The 10 Hz tick loop, then teardown.
    async fn run(mut self) -> i32 {
        let mut ticker = tokio::time::interval(Duration::from_millis(100));
        ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        loop {
            ticker.tick().await;
            let now = fleet_epoch_ms();

            // Controlled vehicle restarts (ADR-0016 setup workflow): the
            // airframe-apply endpoint writes SYS_AUTOSTART and queues a
            // restart; respawn the sim+px4 pair here — before the sync
            // tick, so the dead-pair window is never observed by the
            // process-death supervision (restart ≠ process death).
            for i in self.api.take_restart_requests() {
                self.log.log(
                    EventKind::SupervisorAction,
                    Some(i),
                    "vehicle restart: airframe change (SYS_AUTOSTART) — respawn pair",
                );
                match self.simctl.restart_vehicle(i, &self.api.run_dir).await {
                    Ok(()) => {
                        self.log.log(
                            EventKind::SupervisorAction,
                            Some(i),
                            "vehicle restart: pair respawned — booting with persisted params",
                        );
                        println!("[fleet] vehicle {i}: restarted (airframe apply, ADR-0016)");
                    }
                    Err(e) => {
                        self.log.log(
                            EventKind::SupervisorAction,
                            Some(i),
                            format!("vehicle restart FAILED: {e}"),
                        );
                        eprintln!("[fleet] vehicle {i} restart FAILED: {e}");
                    }
                }
            }

            // Operator control plane (ADR-0017): mission upload / start /
            // clear queue here; the supervisor is the single writer of the
            // task table, so REST handlers only queue + await the ack.
            self.drain_operator_cmds(now);

            if let Outcome::Stop = self.tick(now) {
                break;
            }
        }
        self.finish().await
    }

    // -- one tick ------------------------------------------------------------
    fn tick(&mut self, now: u64) -> Outcome {
        self.api.tick_count.fetch_add(1, Ordering::Relaxed);

        // Process supervision (§2.4): dead px4/sim → FAULT + event.
        let dead = self.simctl.dead_vehicles();
        for (i, sim_dead, px4_dead) in dead {
            if self.registry.fsm(i) == Some(FsmState::Fault) {
                continue;
            }
            self.log.log(
                EventKind::SupervisorAction,
                Some(i),
                format!("process death (sim_dead={sim_dead}, px4_dead={px4_dead})"),
            );
            self.registry.apply_transition(i, FsmCause::ProcessDeath, &self.log);
        }

        // E-stop latch from the API (§3.4 POST /api/estop).
        if self.api.estop_requested() && self.abort_grace_until.is_none() {
            self.abort_run(now, "e-stop (operator requested via API)");
        }
        if let Some(g) = self.abort_grace_until {
            if now > g {
                return Outcome::Stop;
            }
            return Outcome::Continue;
        }

        // Hard wall-clock cap.
        if now > self.hard_deadline_ms {
            self.api.set_phase("TIMEOUT");
            self.log.log(
                EventKind::RunBoundary,
                None,
                format!(
                    "run hit the hard wall-clock cap ({:.0}s) — phase TIMEOUT",
                    self.hard_deadline_ms / 1000
                ),
            );
            return Outcome::Stop;
        }

        // Catastrophic: every vehicle FAULT.
        if !self.ctxs.is_empty()
            && self
                .ctxs
                .iter()
                .all(|c| self.registry.fsm(c.index) == Some(FsmState::Fault))
        {
            self.abort_run(now, "all vehicles FAULT");
            return Outcome::Continue;
        }

        let count = self.ctxs.len() as u8;
        for i in 0..count {
            self.tick_vehicle(i, now);
        }

        Outcome::Continue
    }

    // -- per-vehicle tick ----------------------------------------------------
    fn tick_vehicle(&mut self, index: u8, now: u64) {
        let ctx = &mut self.ctxs[index as usize];

        // Link counters → shared state (§5.1 link block).
        let stats = ctx.handle.shared.stats();
        let slot = Arc::clone(&self.registry.entry(index).unwrap().state);
        slot.write(|s| {
            s.link = fleet_core::state::LinkCounters {
                sent: stats.sent,
                recv: stats.recv,
                retries: stats.retries,
                cmd_failures: stats.cmd_failures,
                commands_sent: stats.commands_sent,
                setpoints_sent: stats.setpoints_sent,
                heartbeats_sent: stats.heartbeats_sent,
                dropped_link_loss: stats.dropped_link_loss,
                recv_rate_hz: stats.recv_rate_hz,
            };
            s.msg_counts = stats.msg_counts.clone();
        });

        let mut fsm = self.registry.fsm(index).unwrap();
        let snap = self.registry.snapshot(index).unwrap();

        // BOOTING → READY (§4: heartbeats + local position + home).
        if fsm == FsmState::Booting && snap.heartbeat_seen && snap.local_position_seen {
            if snap.home_set {
                self.registry.apply_transition(index, FsmCause::TelemetryValid, &self.log);
                println!(
                    "[fleet] vehicle {index} READY at {:.1}s (hb+pos+home)",
                    now as f32 / 1000.0
                );
            } else {
                let gate = *ctx.gate_since_ms.get_or_insert(now);
                if now.saturating_sub(gate) > HOME_FALLBACK_MS {
                    self.log.log(
                        EventKind::SupervisorAction,
                        Some(index),
                        format!(
                            "HOME_POSITION not observed within {}s; gate opened on heartbeat+local-position (interim sim)",
                            HOME_FALLBACK_MS / 1000
                        ),
                    );
                    self.registry.apply_transition(index, FsmCause::TelemetryValid, &self.log);
                    println!(
                        "[fleet] vehicle {index} READY at {:.1}s (hb+pos; home fallback)",
                        now as f32 / 1000.0
                    );
                }
            }
            fsm = self.registry.fsm(index).unwrap();
            // PX4 GCS-connection arming gate: NAV_DLL_ACT=0 (ADR-0009).
            if fsm == FsmState::Ready && !ctx.nav_dll_done {
                ctx.nav_dll_done = true;
                let handle = ctx.handle.clone();
                let log = Arc::clone(&self.log);
                let idx = index;
                tokio::spawn(async move {
                    match handle.set_param("NAV_DLL_ACT", 0.0).await {
                        Some(v) => log.log(
                            EventKind::SupervisorAction,
                            Some(idx),
                            format!(
                                "NAV_DLL_ACT={v} confirmed by PARAM_VALUE echo: PX4 datalink failsafe off (ADR-0009)"
                            ),
                        ),
                        None => log.log(
                            EventKind::LinkEscalation,
                            Some(idx),
                            "NAV_DLL_ACT=0 write unconfirmed (no PARAM_VALUE echo): PX4 GCS-connection arming gate may block flight",
                        ),
                    }
                });
            }
        }

        // Mode-echo evidence (passive observation only — the operator is
        // in the loop; the supervisor no longer drives mode changes).
        if !ctx.saw_offboard_echo && snap.mode_word == MODE_WORD_OFFBOARD {
            ctx.saw_offboard_echo = true;
            self.log.log(
                EventKind::TaskEvent,
                Some(index),
                format!(
                    "heartbeat mode echo: OFFBOARD (custom_mode=0x{:08X}) — operator go-to engaged",
                    snap.mode_word
                ),
            );
        }
        if !ctx.saw_rtl_echo && snap.mode_word == fleet_modes::MODE_WORD_AUTO_RTL {
            ctx.saw_rtl_echo = true;
            self.log.log(
                EventKind::SupervisorAction,
                Some(index),
                "heartbeat mode echo: AUTO.RTL — operator-verified mode change",
            );
        }

        // mission_active (Task 7b): true when the vehicle is armed AND in
        // a mode the operator can't safely send a competing guided
        // command into (AUTO* mission / OFFBOARD setpoint stream). The
        // REST `guided_gates` reads this so a `POST /api/vehicles/{i}/goto`
        // never fights a running MAVLink mission (uploaded by
        // `POST /api/fleet/start`).
        let (main, _sub) = decode_mode_word(snap.mode_word);
        let in_auto_or_offboard =
            main == MainMode::Auto as u8 || main == MainMode::Offboard as u8;
        self.api.set_mission_active(index, snap.armed && in_auto_or_offboard);

        // Geofence breach detection (passive observation — the 8-policy
        // safety ladder is gone; we just raise the GEOFENCE_WARN flag so
        // the Fly View's pre-arm checklist reflects the real fence
        // proximity, exactly the way QGC does).
        let geofence_warn = self.fence.boundary_warning(snap.position_ned_m);
        let flags = compute_flags(&snap, now, geofence_warn, false);
        let rising = ctx.flag_edge.update(&flags);
        for f in rising {
            self.log.log(
                EventKind::HealthFlag,
                Some(index),
                format!("health flag raised: {}", f.name()),
            );
        }
        self.api.set_flags(index, flags);

        // RTL → LANDED observation (watchdog on disarm).
        if fsm == FsmState::Rtl {
            let since = ctx.rtl_since_ms.unwrap_or(now);
            let disarmed = !snap.armed;
            if (disarmed && now > since + 2_000) || now > since + LAND_TIMEOUT_MS {
                if now > since + LAND_TIMEOUT_MS {
                    self.log.log(
                        EventKind::SupervisorAction,
                        Some(index),
                        format!(
                            "disarm not observed within {}s; treating vehicle as LANDED (watchdog — check RTL/land progress and teardown)",
                            LAND_TIMEOUT_MS / 1000
                        ),
                    );
                } else {
                    self.log.log(
                        EventKind::SupervisorAction,
                        Some(index),
                        "disarm observed after RTL — vehicle LANDED",
                    );
                }
                self.registry
                    .apply_transition(index, FsmCause::DisarmObserved, &self.log);
                ctx.landed_since_ms = Some(now);
                ctx.rtl_since_ms = None;
            }
        }
        if fsm == FsmState::Landed && ctx.landed_since_ms.is_none() {
            ctx.landed_since_ms = Some(now);
        }

        // Frame inputs for the control plane: surface the operator-uploaded
        // task board (no mission runner; `current` is whatever the operator
        // is flying, which the lean manager doesn't track per-vehicle).
        let info = VehicleTaskInfo {
            current: None,
            queue: self
                .api
                .tasks
                .lock()
                .unwrap()
                .iter()
                .filter(|t| t.assigned == Some(index) && t.state == "queued")
                .map(|t| t.id.clone())
                .collect(),
        };
        self.api.set_vehicle_tasks(index, info);
    }

    // -- operator control plane (ADR-0017) -----------------------------------

    /// Drain the REST-queued operator commands. Runs on the tick loop —
    /// the supervisor stays the single writer of the task table — and
    /// answers each on its oneshot with the honest result.
    fn drain_operator_cmds(&mut self, _now: u64) {
        for cmd in self.api.take_operator_cmds() {
            match cmd {
                OperatorCmd::Upload { items, replace, ack } => {
                    let result = self.operator_upload(items, replace);
                    let _ = ack.send(result);
                }
                OperatorCmd::Start { ack } => {
                    let result = self.operator_start();
                    let _ = ack.send(result);
                }
                OperatorCmd::Clear { ack } => {
                    let cleared = self.clear_queued_operator_tasks();
                    let _ = ack.send(ClearAck { cleared });
                }
            }
        }
    }

    /// Validate + inject operator waypoints: geo -> NED against the
    /// scenario origin, the fence's own polygon + altitude box. Invalid
    /// items are rejected with reasons — never discovered mid-flight.
    /// (Task 7b: the lean manager surfaces them in the task table for
    /// display; the actual flight happens through `POST /api/fleet/start`
    /// which uploads the bound catalog mission via the MAVLink mission
    /// protocol, or through `POST /api/vehicles/{i}/goto`.)
    fn operator_upload(&mut self, items: Vec<OperatorWaypoint>, replace: bool) -> UploadAck {
        if replace {
            self.clear_queued_operator_tasks();
        }
        let origin = self.api.geo_origin;
        let mut accepted: Vec<String> = Vec::new();
        let mut rejected: Vec<(String, String)> = Vec::new();
        for (k, item) in items.into_iter().enumerate() {
            let label = item
                .label
                .clone()
                .unwrap_or_else(|| format!("item[{}]", k));
            if item.hover_s < 0.0 {
                rejected.push((label, "hover_s must be >= 0".into()));
                continue;
            }
            if !item.lat_deg.is_finite() || !item.lon_deg.is_finite() || !item.alt_m.is_finite() {
                rejected.push((label, "lat/lon/alt must be finite".into()));
                continue;
            }
            if !(-90.0..=90.0).contains(&item.lat_deg) || !(-180.0..=180.0).contains(&item.lon_deg) {
                rejected.push((label, "lat/lon out of range".into()));
                continue;
            }
            if item.alt_m < 0.0 {
                rejected.push((label, "alt_m (AGL) must be >= 0".into()));
                continue;
            }
            // QGC convention: waypoint altitude is AGL -> geodetic
            // alt = origin + AGL -> NED z = -AGL.
            let z = -(item.alt_m as f32);
            let ned = origin.geodetic_to_ned(item.lat_deg, item.lon_deg, origin.alt_m + item.alt_m);
            let xy = [ned[0] as f32, ned[1] as f32];
            if !self.fence.contains_xy(xy) {
                let breach = self.fence.horizontal_breach(xy);
                rejected.push((
                    label,
                    format!(
                        "outside geofence polygon (lat {:.6}, lon {:.6}, breach {:.0} m)",
                        item.lat_deg, item.lon_deg, breach
                    ),
                ));
                continue;
            }
            if !self.fence.altitude_ok(z) {
                rejected.push((
                    label,
                    format!(
                        "outside altitude box (AGL {:.1} m; box {}..{} m)",
                        item.alt_m, self.fence.floor_m, self.fence.ceiling_m
                    ),
                ));
                continue;
            }
            self.operator_seq += 1;
            let id = format!("op{}", self.operator_seq);
            self.api.tasks.lock().unwrap().push(TaskStatus {
                id: id.clone(),
                pos_ned_m: [xy[0], xy[1], z],
                assigned: None,
                state: "pending".into(),
                hover_observed: None,
            });
            accepted.push(id);
        }
        let pool = self
            .api
            .tasks
            .lock()
            .unwrap()
            .iter()
            .filter(|t| t.state == "pending")
            .count();
        if !accepted.is_empty() {
            self.log.log(
                EventKind::SupervisorAction,
                None,
                format!(
                    "operator mission upload: {} waypoint(s) accepted ({:?}), {} rejected, pool {pool}",
                    accepted.len(),
                    accepted,
                    rejected.len()
                ),
            );
        }
        UploadAck { accepted, rejected, pool }
    }

    /// Start the deferred operator mission (the setup-bench path to
    /// RUNNING — flips the phase; the operator is in the loop from here).
    fn operator_start(&mut self) -> StartAck {
        if self.api.phase() == "RUNNING" {
            return StartAck {
                started: false,
                reason: Some("mission already started".into()),
            };
        }
        if self.api.phase() == "ABORTED" || self.api.phase() == "TIMEOUT" {
            return StartAck {
                started: false,
                reason: Some(format!("run is {}", self.api.phase())),
            };
        }
        self.api.set_phase("RUNNING");
        self.log.log(
            EventKind::RunBoundary,
            None,
            "operator mission start: phase RUNNING — operator in the loop (use POST /api/fleet/start to upload + arm the bound catalog missions, or POST /api/vehicles/{i}/goto for a single waypoint)",
        );
        StartAck {
            started: true,
            reason: None,
        }
    }

    /// Drop queued operator tasks (state -> "cleared").
    fn clear_queued_operator_tasks(&mut self) -> usize {
        let mut count = 0usize;
        for t in self.api.tasks.lock().unwrap().iter_mut() {
            if t.state == "pending" || t.state == "queued" {
                t.state = "cleared".into();
                count += 1;
            }
        }
        if count > 0 {
            self.log.log(
                EventKind::SupervisorAction,
                None,
                format!("operator: cleared {count} queued mission task(s)"),
            );
        }
        count
    }

    // -- termination / teardown ----------------------------------------------

    fn abort_run(&mut self, now: u64, reason: &str) {
        if self.abort_grace_until.is_some() {
            return;
        }
        self.api.aborted.store(true, Ordering::SeqCst);
        self.api.set_phase("ABORTED");
        self.log.log(
            EventKind::RunBoundary,
            None,
            format!("run ABORTED: {reason}"),
        );
        let count = self.ctxs.len() as u8;
        for i in 0..count {
            let fsm = self.registry.fsm(i).unwrap();
            let ctx = &mut self.ctxs[i as usize];
            ctx.handle.shared.stop_stream();
            match fsm {
                FsmState::Active => {
                    self.registry.apply_transition(i, FsmCause::Estop, &self.log);
                    spawn_land_and_disarm(ctx.handle.clone(), Arc::clone(&self.log), i);
                }
                FsmState::Rtl => {
                    self.registry.apply_transition(i, FsmCause::Estop, &self.log);
                    spawn_land_and_disarm(ctx.handle.clone(), Arc::clone(&self.log), i);
                }
                FsmState::Ready | FsmState::Booting | FsmState::Spawning => {
                    self.registry.apply_transition(i, FsmCause::Estop, &self.log);
                }
                _ => {}
            }
        }
        self.abort_grace_until = Some(now + ABORT_GRACE_MS);
    }

    async fn finish(mut self) -> i32 {
        let phase = self.api.phase();
        let now = fleet_epoch_ms();
        self.log.log(
            EventKind::RunBoundary,
            None,
            format!("run end (phase {phase}) after {:.1}s", now as f32 / 1000.0),
        );
        for ctx in &self.ctxs {
            ctx.handle.shared.stop_stream();
        }

        // Release THIS run's own sockets before the port probe: the link
        // tasks exit when every LinkHandle drops (their command channel
        // closes) — otherwise the probe below sees our own
        // telemetry/onboard ports as still bound and warns spuriously.
        self.ctxs.clear();
        {
            let mut links = self.api.links.lock().unwrap();
            links.clear();
        }
        tokio::time::sleep(Duration::from_millis(300)).await;

        // Teardown (§12.3): reap every child, verify §17 ports free (F-1).
        self.simctl.kill_all().await;
        println!("[fleet] teardown: processes killed; verifying ports free");
        let instances: Vec<u8> = (0..self.api.count).collect();
        let mut free = false;
        let mut last_failures = Vec::new();
        for _ in 0..20 {
            tokio::time::sleep(Duration::from_millis(500)).await;
            let (ok, failures) = ports_free(&instances);
            if ok {
                free = true;
                break;
            }
            last_failures = failures;
        }
        if free {
            self.log.log(
                EventKind::RunBoundary,
                None,
                "teardown verified: no processes, all §17 ports free (F-1 probe)",
            );
            println!("[fleet] teardown verified: ports free");
        } else {
            self.log.log(
                EventKind::RunBoundary,
                None,
                format!("TEARDOWN WARNING: ports still bound after 10 s: {last_failures:?}"),
            );
            eprintln!("[fleet] teardown WARNING: ports still bound: {last_failures:?}");
        }

        println!(
            "[fleet] events: {} total, {} in ring",
            self.log.total(),
            self.log.tail(1).len()
        );
        let exit = if self.api.aborted.load(Ordering::SeqCst) {
            EXIT_ABORTED
        } else {
            0
        };
        println!("[fleet] exit code {exit}");
        exit
    }
}
