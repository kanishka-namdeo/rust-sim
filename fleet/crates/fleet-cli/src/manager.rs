//! The run engine / supervisor composition (spec §2.3, §2.4, §4, §5, §6, §7,
//! §8, §9, §12.3): spawns vehicles, binds links, runs the 10 Hz tick, drives
//! the mission (compile → auction → runners → RunnerCmds on the links),
//! enforces the safety ladder, serves the control plane, and tears
//! everything down — one process invocation, one scenario, exit codes CI can
//! classify (ADR-0006's async half).

#![forbid(unsafe_code)]

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::atomic::Ordering;
use std::sync::Arc;
use std::time::{Duration, Instant};

use tokio::sync::mpsc::UnboundedReceiver;

use fleet_core::events::{fleet_epoch_ms, set_fleet_epoch, EventKind, EventLog};
use fleet_core::fsm::{FsmCause, FsmState};
use fleet_core::health::{compute_flags, FlagEdge};
use fleet_core::registry::Registry;
use fleet_core::state::VehicleState;
use fleet_core::SafetyAction;
use fleet_mavlink::{spawn_link, CmdAck, LinkConfig, LinkHandle, SetpointGoal, TelemetryEvent};
use fleet_mission::report::{unix_now, TaskStatus};
use fleet_mission::runner::{MissionRunner, RunnerCmd, RunnerEvent, RunnerInput, RunnerPhase, RunnerTask};
use fleet_mission::{MissionPlan, Scenario};
use fleet_modes::{MODE_WORD_OFFBOARD, MAV_MODE_FLAG_SAFETY_ARMED};
use fleet_safety::policy::{PolicyEngine, PolicyInput};
use fleet_simctl::{ports_free, SimCtl, SimCtlConfig};

use crate::api;
use crate::report;
use crate::state::{
    AppState, ClearAck, HotLoadAck, OperatorCmd, OperatorTask, OperatorWaypoint, StartAck,
    UploadAck, VehicleTaskInfo,
};

/// Scenario arg parsing failure / infrastructure error exit code.
pub const EXIT_ERROR: i32 = 3;
/// READY gate: home may be missing this long before we open the gate anyway
/// (the interim sim streams HOME_POSITION only on the manager's request).
const HOME_FALLBACK_MS: u64 = 45_000;
/// Timeline fault events retry within this window (ADR-0018): right after
/// a hot restart the sim's control plane can still be booting when the
/// supervisor's first ticks run — a transient "unreachable" must not
/// permanently swallow the injection.
const FAULT_RETRY_WINDOW_MS: u64 = 10_000;
/// RTL → LANDED disarm-observation budget. With real rustsitsim dynamics the
/// full RTL cycle is climb-to-return-alt (RTL_RETURN_ALT, default 30 m AGL,
/// ~30 s) + return leg + descend at MPC_LAND_SPEED (~0.7 m/s, ~45 s from
/// 30 m) + touchdown + PX4 auto-disarm — measured 60–90 s end-to-end in the
/// live F-2 runs. 30 s (the interim-sim value) declared vehicles "LANDED"
/// while they were still ~20 m up (killed mid-air at teardown). Disarm is
/// still the primary signal; the timeout is the watchdog.
const LAND_TIMEOUT_MS: u64 = 120_000;
/// Max re-arm cycles per vehicle (LANDED → READY recovery, spec §4/§6.5).
const MAX_REARM: u8 = 2;
/// Grace between abort and teardown so final ACKs/statustexts land.
const ABORT_GRACE_MS: u64 = 4_000;
/// All vehicles READY budget from run start (spec §10.1: 90 s for 2 vehicles;
/// 150 s covers the worst stagger on 2 cores).
const ALL_READY_BUDGET_MS: u64 = 150_000;

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

pub(crate) fn spawn_rtl(handle: LinkHandle, log: Arc<EventLog>, index: u8) {
    let h = handle;
    let l = log;
    let i = index;
    tokio::spawn(async move {
        let ack = h.set_mode(fleet_modes::MODE_WORD_AUTO_RTL).await;
        log_ack(&l, i, "DO_SET_MODE(AUTO.RTL, 0x05040000)", ack);
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

fn yaw_from_quaternion(q: [f32; 4]) -> f32 {
    // q = [w, x, y, z]; Z-Y-X euler yaw.
    (2.0 * (q[0] * q[3] + q[1] * q[2])).atan2(1.0 - 2.0 * (q[2] * q[2] + q[3] * q[3]))
}

// ---------------------------------------------------------------------------
// Supervisor
// ---------------------------------------------------------------------------

struct VehCtx {
    index: u8,
    handle: LinkHandle,
    runner: Option<MissionRunner>,
    flag_edge: FlagEdge,
    /// Ladder causes already applied (dedup: policies re-trigger every tick).
    applied: HashSet<FsmCause>,
    arm_last_ms: Option<u64>,
    /// NAV_DLL_ACT=0 param write issued once per vehicle (ADR-0009).
    nav_dll_done: bool,
    rtl_since_ms: Option<u64>,
    no_new_tasks: bool,
    rearm_cycles: u8,
    landed_since_ms: Option<u64>,
    standing_down: bool,
    saw_offboard_echo: bool,
    saw_rtl_echo: bool,
    gate_since_ms: Option<u64>,
}

impl VehCtx {
    fn new(index: u8, handle: LinkHandle) -> Self {
        VehCtx {
            index,
            handle,
            runner: None,
            flag_edge: FlagEdge::default(),
            applied: HashSet::new(),
            arm_last_ms: None,
            nav_dll_done: false,
            rtl_since_ms: None,
            no_new_tasks: false,
            rearm_cycles: 0,
            landed_since_ms: None,
            standing_down: false,
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
    scenario: Scenario,
    scenario_path: String,
    plan: MissionPlan,
    runner_tasks: Vec<RunnerTask>,
    registry: Arc<Registry>,
    api: Arc<AppState>,
    log: Arc<EventLog>,
    simctl: SimCtl,
    ctxs: Vec<VehCtx>,
    policy: PolicyEngine,
    pool: Vec<usize>,
    fired: HashSet<usize>,
    /// First-attempt time per timeline fault event (ADR-0018 retry window:
    /// measured from the attempt, never from the fleet epoch — a hot-loaded
    /// scenario's events can already be "past" relative to the process
    /// clock, so an epoch-relative window would expire before the first
    /// try).
    fault_attempts: std::collections::HashMap<usize, u64>,
    mission_started: bool,
    mission_started_at: u64,
    mission_complete: bool,
    abort_grace_until: Option<u64>,
    all_ready_deadline_ms: u64,
    hard_deadline_ms: u64,
    battery_gate_decided: bool,
    unassignable_reported: bool,
    run_dir: PathBuf,
    /// ADR-0017: monotonic id for operator-uploaded tasks (`op1`, `op2`, …).
    operator_seq: u32,
}

fn set_task(api: &AppState, ti: usize, f: impl FnOnce(&mut TaskStatus)) {
    let mut guard = api.tasks.lock().unwrap();
    if let Some(t) = guard.get_mut(ti) {
        f(t);
    }
}

/// Resolve the per-vehicle sim command template. Priority (§12 / ADR-0008):
/// scenario `[sim] command` → `FLEET_SIM_COMMAND` env (handled inside
/// SimCtlConfig) → the interim simulator with an interpreter that actually
/// has pymavlink (the sandbox `python3` venv does not; /usr/bin/python3.13
/// does — probed once at startup).
fn resolve_sim_command(scenario: &Scenario, log: &EventLog) -> Option<String> {
    if let Some(t) = scenario.sim_command() {
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
// run_scenario: the whole lifecycle (§12.3 single invocation)
// ---------------------------------------------------------------------------

pub async fn run_scenario(args: RunArgs) -> i32 {
    set_fleet_epoch();
    // ADR-0018 hot scenario load loop: each iteration is one complete run
    // (spawn -> supervise -> teardown + report). `PUT /api/fleet` stages a
    // validated scenario file; the current run aborts gracefully and this
    // loop restarts with it — same process, same API port (the control
    // plane task is aborted and rebound between runs; clients reconnect).
    let mut scenario_path = args.fleet.clone();
    let mut last_code;
    let base_run_dir: PathBuf = args.run_dir.clone().unwrap_or_else(|| {
        PathBuf::from(format!("fleet-runs/fleet-run-{}", unix_now()))
    });
    let mut iteration = 0u32;
    loop {
        // Each run gets its own directory: the first honors --run-dir (or
        // the default); hot-restarts get sibling dirs with a -hot-N suffix
        // so reports/events never overwrite each other.
        let run_dir = if iteration == 0 {
            base_run_dir.clone()
        } else {
            PathBuf::from(format!("{}-hot-{}", base_run_dir.display(), iteration))
        };
        let (code, api, server) = run_once(&scenario_path, args.api_port, &run_dir).await;
        last_code = code;
        let next = api.take_next_scenario();
        // Drop the old control plane BEFORE the next bind: abort() closes
        // the listener (open WS sockets drop with it — clients reconnect;
        // the console's 12 s live probe tolerates the window).
        server.abort();
        let _ = server.await.is_ok();
        match next {
            Some(path) => {
                println!(
                    "[fleet] hot scenario load: restarting with {}",
                    path.display()
                );
                scenario_path = path;
                iteration += 1;
                continue;
            }
            None => break,
        }
    }
    last_code
}

/// One complete run: parse -> spawn -> supervise -> teardown + report.
/// Returns the exit code, the run's AppState (to read a staged hot-swap
/// scenario after the fact) and the control-plane server task handle.
async fn run_once(
    scenario_file: &Path,
    api_port: u16,
    run_dir: &Path,
) -> (i32, Arc<AppState>, tokio::task::JoinHandle<()>) {
    let (scenario, scenario_path) = match Scenario::parse_file(scenario_file) {
        Ok(v) => v,
        Err(e) => {
            eprintln!("[fleet] scenario error: {e}");
            return (EXIT_ERROR, AppState::new_for_error(), dummy_server());
        }
    };
    if std::fs::create_dir_all(run_dir).is_err() {
        eprintln!("[fleet] cannot create run dir {}", run_dir.display());
        return (EXIT_ERROR, AppState::new_for_error(), dummy_server());
    }
    println!("[fleet] mavfleet run: scenario {}", scenario_file.display());
    println!("[fleet] run dir: {}", run_dir.display());

    let log = EventLog::spawn_writer(run_dir.join("events.ndjson"));
    let plan = MissionPlan::compile(&scenario);
    for (id, reason) in &plan.rejections {
        log.log(
            EventKind::TaskEvent,
            None,
            format!("task '{id}' rejected at compile: {reason}"),
        );
    }

    let count = scenario.count();
    println!(
        "[fleet] fleet: {count} vehicles, {} tasks ({} accepted), supervisor tick 10 Hz, setpoint pump 20 Hz",
        scenario.tasks.len(),
        plan.tasks.len()
    );

    let registry = Registry::new(count);
    let geo_origin = scenario.geo_origin();
    let api = AppState::new(
        Arc::clone(&registry),
        Arc::clone(&log),
        count,
        scenario_path.clone(),
        unix_now(),
        geo_origin,
        plan.fence.clone(),
        run_dir.clone(),
    );

    // Task table: accepted tasks in plan order, rejected appended.
    {
        let z_of = |id: &str| {
            scenario
                .tasks
                .iter()
                .find(|t| t.id == id)
                .map(|t| t.pos_ned_m[2])
                .unwrap_or(0.0)
        };
        let mut table: Vec<TaskStatus> = plan
            .tasks
            .iter()
            .map(|t| TaskStatus {
                id: t.id.clone(),
                pos_ned_m: [t.pos_ned_m[0], t.pos_ned_m[1], z_of(&t.id)],
                assigned: None,
                state: "pending".into(),
                hover_observed: None,
            })
            .collect();
        for (id, _) in &plan.rejections {
            table.push(TaskStatus {
                id: id.clone(),
                pos_ned_m: [0.0, 0.0, z_of(id)],
                assigned: None,
                state: "rejected".into(),
                hover_observed: None,
            });
        }
        *api.tasks.lock().unwrap() = table;
    }

    // Safety engine + battery ladder gating (§8.1).
    let battery_sim = scenario.fleet.battery_sim;
    let policy = PolicyEngine::new(plan.fence.clone(), battery_sim);
    if !battery_sim {
        log.log(
            EventKind::RunBoundary,
            None,
            "battery_sim=false: battery ladder (policies 4/5) disabled with this warning, §8.1",
        );
    }

    // Process supervision config.
    let sim_template = resolve_sim_command(&scenario, &log);
    let mut sim_cfg = SimCtlConfig::from_env(sim_template, scenario.sim_duration_s());
    sim_cfg.process_logs = true;
    sim_cfg.geo_origin = geo_origin; // ADR-0017: RSIM_ORIGIN_* for the sim wrapper
    // Scenario [env] -> sim physics (RSIM_WIND_MS / RSIM_TURBULENCE for the
    // wrapper): the declared wind/turbulence now actually reaches the sims.
    // Before 2026-09-09 the wrapper hard-coded turbulence "off" and no wind,
    // so a scenario's [env] was documentation-only for the physics (found
    // live while debugging the long-idle arming window — see
    // SimCtlConfig::cpu_pinning for the actual arming fix).
    sim_cfg.sim_env = vec![
        (
            "RSIM_WIND_MS".into(),
            format!(
                "{:.3},{:.3},{:.3}",
                scenario.env.wind_steady_ms[0],
                scenario.env.wind_steady_ms[1],
                scenario.env.wind_steady_ms[2]
            ),
        ),
        ("RSIM_TURBULENCE".into(), scenario.env.turbulence.clone()),
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

    log.log(
        EventKind::RunBoundary,
        None,
        format!(
            "run started: scenario {scenario_path}, {count} vehicles, {} tasks, geofence {} vertices / ceiling {} m",
            plan.tasks.len(),
            plan.fence.points.len(),
            plan.fence.ceiling_m
        ),
    );

    // Spawn vehicles (sim first, settle, then px4; 2 s stagger, §2.4/R-10).
    for i in 0..count {
        registry.apply_transition(i, FsmCause::Spawn, &log);
        match simctl.spawn_vehicle(i, &run_dir).await {
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

    // Control plane (§3.4). The task handle is returned to the hot-restart
    // loop so it can be aborted (listener freed) before a rebind.
    let server_task = {
        let api_task = Arc::clone(&api);
        let port = api_port;
        tokio::spawn(async move {
            if let Err(e) = api::serve(api_task, port).await {
                eprintln!("[fleet] control plane error: {e}");
            }
        })
    };
    api.set_phase("BRING_UP");

    let now0 = fleet_epoch_ms();
    let hard_deadline_ms = match scenario.max_time_s() {
        Some(t) => (t * 1000.0) as u64 + 60_000,
        None => 300_000,
    };
    let sim_duration_ms = (scenario.sim_duration_s() * 1000.0) as u64;
    if sim_duration_ms < hard_deadline_ms {
        log.log(
            EventKind::RunBoundary,
            None,
            format!(
                "warning: interim sim duration {:.0}s < run hard deadline {:.0}s — sim may exit first (process_death)",
                scenario.sim_duration_s(),
                hard_deadline_ms / 1000
            ),
        );
    }

    let runner_tasks: Vec<RunnerTask> = plan
        .tasks
        .iter()
        .map(|t| RunnerTask {
            id: t.id.clone(),
            pos_ned_m: [
                t.pos_ned_m[0],
                t.pos_ned_m[1],
                scenario
                    .tasks
                    .iter()
                    .find(|s| s.id == t.id)
                    .map(|s| s.pos_ned_m[2])
                    .unwrap_or(-10.0),
            ],
            hover_s: t.hover_s,
        })
        .collect();

    let sup = Supervisor {
        scenario,
        scenario_path,
        plan,
        runner_tasks,
        registry,
        api: Arc::clone(&api),
        log,
        simctl,
        ctxs,
        policy,
        pool: Vec::new(),
        fired: HashSet::new(),
        fault_attempts: std::collections::HashMap::new(),
        mission_started: false,
        mission_started_at: 0,
        mission_complete: false,
        abort_grace_until: None,
        all_ready_deadline_ms: now0 + ALL_READY_BUDGET_MS,
        hard_deadline_ms,
        battery_gate_decided: !battery_sim,
        unassignable_reported: false,
        run_dir: run_dir.to_path_buf(),
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
    /// The 10 Hz tick loop, then teardown + report.
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
                match self.simctl.restart_vehicle(i, &self.run_dir).await {
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
            // mission state, so REST handlers only queue + await the ack.
            self.drain_operator_cmds(now);

            if let Outcome::Stop = self.tick(now).await {
                break;
            }
        }
        self.finish().await
    }

    // -- one tick ------------------------------------------------------------

    /// Async since ADR-0018: the timeline's `fault` events proxy to the
    /// per-vehicle sim control planes over localhost HTTP (the fault-plane
    /// wire), which is an await away.
    async fn tick(&mut self, now: u64) -> Outcome {
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
            self.stand_down(i, now);
            self.registry.apply_transition(i, FsmCause::ProcessDeath, &self.log);
        }

        // E-stop latch from the API (§3.4 POST /api/estop).
        if self.api.estop_requested() && !self.policy.estop {
            self.policy.trigger_estop();
            self.log.log(
                EventKind::SupervisorAction,
                None,
                "e-stop latched into the policy engine (policy 1)",
            );
        }

        self.fire_timeline(now).await;
        self.battery_gate(now);
        self.mission_start_gate(now);

        // Active positions for the separation monitor (policy 7).
        let positions: Vec<(u8, [f32; 3])> = self
            .ctxs
            .iter()
            .filter(|c| self.registry.fsm(c.index) == Some(FsmState::Active))
            .filter_map(|c| {
                self.registry
                    .snapshot(c.index)
                    .map(|s| (c.index, s.position_ned_m))
            })
            .collect();
        let count = self.ctxs.len() as u8;
        for i in 0..count {
            let others: Vec<(u8, [f32; 3])> =
                positions.iter().filter(|(v, _)| *v != i).cloned().collect();
            self.tick_vehicle(i, now, others);
        }

        self.landing_pass(now);
        self.reallocation_pass(now);

        match self.termination_check(now) {
            Outcome::Stop => Outcome::Stop,
            Outcome::Continue => Outcome::Continue,
        }
    }

    // -- per-vehicle tick ----------------------------------------------------

    fn tick_vehicle(&mut self, index: u8, now: u64, others: Vec<(u8, [f32; 3])>) {
        let Self {
            ctxs,
            registry,
            log,
            api,
            policy,
            plan,
            pool,
            ..
        } = self;
        let ctx = &mut ctxs[index as usize];

        // Link counters → shared state (§5.1 link block).
        let stats = ctx.handle.shared.stats();
        let slot = Arc::clone(&registry.entry(index).unwrap().state);
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

        let mut fsm = registry.fsm(index).unwrap();
        let snap = registry.snapshot(index).unwrap();

        // BOOTING → READY (§4: heartbeats + local position + home).
        if fsm == FsmState::Booting && snap.heartbeat_seen && snap.local_position_seen {
            if snap.home_set {
                registry.apply_transition(index, FsmCause::TelemetryValid, log);
                println!(
                    "[fleet] vehicle {index} READY at {:.1}s (hb+pos+home)",
                    now as f32 / 1000.0
                );
            } else {
                let gate = *ctx.gate_since_ms.get_or_insert(now);
                if now.saturating_sub(gate) > HOME_FALLBACK_MS {
                    log.log(
                        EventKind::SupervisorAction,
                        Some(index),
                        format!(
                            "HOME_POSITION not observed within {}s; gate opened on heartbeat+local-position (interim sim)",
                            HOME_FALLBACK_MS / 1000
                        ),
                    );
                    registry.apply_transition(index, FsmCause::TelemetryValid, log);
                    println!(
                        "[fleet] vehicle {index} READY at {:.1}s (hb+pos; home fallback)",
                        now as f32 / 1000.0
                    );
                }
            }
            fsm = registry.fsm(index).unwrap();
            // PX4 GCS-connection arming gate: NAV_DLL_ACT=0 (the manager's
            // §8 heartbeat ladder is the datalink supervisor; ADR-0009).
            if fsm == FsmState::Ready && !ctx.nav_dll_done {
                ctx.nav_dll_done = true;
                let handle = ctx.handle.clone();
                let log = Arc::clone(log);
                let idx = index;
                tokio::spawn(async move {
                    match handle.set_param("NAV_DLL_ACT", 0.0).await {
                        Some(v) => log.log(
                            EventKind::SupervisorAction,
                            Some(idx),
                            format!(
                                "NAV_DLL_ACT={v} confirmed by PARAM_VALUE echo: PX4 datalink failsafe off, manager §8 ladder owns link-loss (ADR-0009)"
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

        // Mode-echo evidence (V-11 empirical + R-15 verification).
        if !ctx.saw_offboard_echo && snap.mode_word == MODE_WORD_OFFBOARD {
            ctx.saw_offboard_echo = true;
            log.log(
                EventKind::TaskEvent,
                Some(index),
                format!(
                    "heartbeat mode echo: OFFBOARD (custom_mode=0x{:08X}) — V-11 empirical confirmation",
                    snap.mode_word
                ),
            );
        }
        if !ctx.saw_rtl_echo && snap.mode_word == fleet_modes::MODE_WORD_AUTO_RTL {
            ctx.saw_rtl_echo = true;
            log.log(
                EventKind::SupervisorAction,
                Some(index),
                "heartbeat mode echo: AUTO.RTL — supervisor-verified mode change (R-15)",
            );
        }

        // Mission runner step (§7): apply RunnerCmds to the link.
        // ADR-0017: publish whether this vehicle has a live mission —
        // the REST guided-command gate reads it (a user command never
        // fights a runner's setpoint stream).
        let mission_live = !ctx.standing_down
            && ctx
                .runner
                .as_ref()
                .map(|r| {
                    matches!(
                        r.phase(),
                        RunnerPhase::Idle | RunnerPhase::Engaging { .. } | RunnerPhase::Flying
                    )
                })
                .unwrap_or(false);
        api.set_mission_active(index, mission_live);
        if !ctx.standing_down && matches!(fsm, FsmState::Ready | FsmState::Active) {
            if let Some(runner) = ctx.runner.as_mut() {
                let input = RunnerInput {
                    now_ms: now,
                    est_pos: if snap.local_position_seen {
                        Some(snap.position_ned_m)
                    } else {
                        None
                    },
                    est_yaw: Some(yaw_from_quaternion(snap.attitude_q_wxyz)),
                    in_offboard: snap.mode_word == MODE_WORD_OFFBOARD,
                    armed: snap.armed,
                };
                let (cmds, events) = runner.step(&input);
                for ev in &events {
                    match ev {
                        RunnerEvent::TaskStart { task } => {
                            if fsm == FsmState::Ready {
                                registry.apply_transition(index, FsmCause::TaskAccepted, log);
                            }
                            let id = plan.tasks[*task].id.clone();
                            set_task(api, *task, |t| t.state = "active".into());
                            log.log(
                                EventKind::TaskEvent,
                                Some(index),
                                format!("task '{id}' start"),
                            );
                        }
                        RunnerEvent::TaskComplete { task, hover_observed } => {
                            let id = plan.tasks[*task].id.clone();
                            set_task(api, *task, |t| {
                                t.state = "complete".into();
                                t.hover_observed = Some(*hover_observed);
                            });
                            log.log(
                                EventKind::TaskEvent,
                                Some(index),
                                format!("task '{id}' complete (hover_observed={hover_observed})"),
                            );
                        }
                        RunnerEvent::Engaged => log.log(
                            EventKind::TaskEvent,
                            Some(index),
                            "offboard engaged (mode echo OFFBOARD); profile start",
                        ),
                        RunnerEvent::EngageFailed { attempts } => log.log(
                            EventKind::LinkEscalation,
                            Some(index),
                            format!(
                                "offboard engage failed after {attempts} un-ACKed attempts; §3.2 escalation → RTL"
                            ),
                        ),
                        RunnerEvent::OffboardDropout { consecutive } => log.log(
                            EventKind::HealthFlag,
                            Some(index),
                            format!("OFFBOARD_DROPOUT x{consecutive}"),
                        ),
                        RunnerEvent::ReEngaged => log.log(
                            EventKind::TaskEvent,
                            Some(index),
                            "offboard dropout: re-anchored, re-engaging (§7.2)",
                        ),
                        RunnerEvent::StoodDown => {}
                    }
                }
                let engage_failed =
                    events.iter().any(|e| matches!(e, RunnerEvent::EngageFailed { .. }));
                let dropout_rtl =
                    events.iter().any(|e| matches!(e, RunnerEvent::OffboardDropout { .. }));
                for cmd in cmds {
                    match cmd {
                        RunnerCmd::HoldAt { pos, yaw } => ctx.handle.shared.hold_at(pos, yaw),
                        RunnerCmd::Goal { pos, yaw } => {
                            ctx.handle
                                .shared
                                .set_goal(SetpointGoal { position: pos, yaw })
                        }
                        RunnerCmd::Stop => ctx.handle.shared.stop_stream(),
                        RunnerCmd::EnterOffboard => {
                            // Arm supervision (§3.2 semantics): re-issue the
                            // arm ladder every 8 s while the vehicle is still
                            // disarmed — PX4 rejects early arms while EKF2
                            // finishes yaw/GNSS alignment (~15 s), and a
                            // one-shot arm dead-locks the mission behind
                            // that transient.
                            let arm_due = !snap.armed
                                && ctx
                                    .arm_last_ms
                                    .map_or(true, |t| now.saturating_sub(t) >= 8_000);
                            if arm_due {
                                ctx.arm_last_ms = Some(now);
                            }
                            spawn_engage(ctx.handle.clone(), Arc::clone(log), index, arm_due);
                        }
                        RunnerCmd::Rtl => {
                            let cause = if engage_failed {
                                FsmCause::CommandFailure
                            } else if dropout_rtl {
                                FsmCause::OffboardDropout
                            } else {
                                FsmCause::SupervisorRtl
                            };
                            ctx.standing_down = true;
                            ctx.rtl_since_ms = Some(now);
                            registry.apply_transition(index, cause, log);
                            spawn_rtl(ctx.handle.clone(), Arc::clone(log), index);
                            ctx.handle.shared.stop_stream();
                        }
                    }
                }
            }
        }

        // Pool queued tasks of a stood-down vehicle (§6.5; the active task is
        // pooled at landing, below).
        if ctx.standing_down {
            if let Some(runner) = ctx.runner.as_mut() {
                let pooled = runner.pool_remaining();
                if !pooled.is_empty() {
                    let ids: Vec<&str> =
                        pooled.iter().map(|&ti| plan.tasks[ti].id.as_str()).collect();
                    for ti in &pooled {
                        set_task(api, *ti, |t| {
                            t.state = "pending".into();
                            t.assigned = None;
                        });
                    }
                    pool.extend(pooled);
                    log.log(
                        EventKind::SupervisorAction,
                        Some(index),
                        format!("pooled queued tasks {ids:?} from vehicle {index} (§6.5)"),
                    );
                }
            }
        }

        // Safety ladder (§8.1): ordered policy evaluation per tick.
        let input = PolicyInput {
            now_ms: now,
            state: snap.clone(),
            fsm: registry.fsm(index).unwrap(),
            sim_truth_ned: None,
            offboard_expected: ctx
                .runner
                .as_ref()
                .map(|r| {
                    matches!(r.phase(), RunnerPhase::Engaging { .. } | RunnerPhase::Flying)
                })
                .unwrap_or(false),
            other_active_positions: others,
        };
        let verdict = policy.evaluate(index, &input);
        for note in &verdict.notes {
            log.log(
                EventKind::SupervisorAction,
                Some(index),
                format!("policy note: {note}"),
            );
        }
        let flags = compute_flags(&snap, now, verdict.geofence_warn, false);
        let rising = ctx.flag_edge.update(&flags);
        for f in rising {
            log.log(
                EventKind::HealthFlag,
                Some(index),
                format!("health flag raised: {}", f.name()),
            );
        }
        api.set_flags(index, flags);

        match verdict.action {
            SafetyAction::Estop => { /* handled centrally in termination_check */ }
            SafetyAction::Rtl { cause, after_current_task } => {
                if ctx.applied.insert(cause) {
                    if after_current_task {
                        ctx.no_new_tasks = true;
                        log.log(
                            EventKind::SupervisorAction,
                            Some(index),
                            "policy 5: battery low — RTL after current task; no new tasks assigned",
                        );
                    } else {
                        log.log(
                            EventKind::SupervisorAction,
                            Some(index),
                            format!("policy: RTL (cause={})", cause.name()),
                        );
                        ctx.standing_down = true;
                        ctx.rtl_since_ms = Some(now);
                        registry.apply_transition(index, cause, log);
                        spawn_rtl(ctx.handle.clone(), Arc::clone(log), index);
                        ctx.handle.shared.stop_stream();
                    }
                }
            }
            SafetyAction::Land { cause } => {
                if ctx.applied.insert(cause) {
                    log.log(
                        EventKind::SupervisorAction,
                        Some(index),
                        format!("policy: LAND (cause={})", cause.name()),
                    );
                    ctx.standing_down = true;
                    registry.apply_transition(index, cause, log);
                    spawn_land_and_disarm(ctx.handle.clone(), Arc::clone(log), index);
                    ctx.handle.shared.stop_stream();
                }
            }
            SafetyAction::AltitudeDiverge { dz_m } => {
                // Policy 7: altitude-divergence setpoint override, stepped at
                // the §7.1 velocity cap from the slot's current position.
                // Applied only to physically airborne vehicles (armed): with
                // the interim sim the position estimates of grounded vehicles
                // sit at home, and policy 7 would otherwise thrash the
                // runner's goals every tick (ADR-0001 estimate stand-ins).
                if snap.armed {
                    let slot_now = ctx.handle.shared.slot_snapshot();
                    let target = [
                        slot_now.current[0],
                        slot_now.current[1],
                        slot_now.current[2] + dz_m,
                    ];
                    let capped = crate::pump::step_toward(
                        slot_now.current,
                        target,
                        crate::pump::CRUISE_MS,
                        crate::pump::TICK_DT_S,
                    );
                    ctx.handle
                        .shared
                        .set_goal(SetpointGoal { position: capped, yaw: slot_now.yaw });
                }
            }
            SafetyAction::Flag(_) | SafetyAction::None => {}
        }

        // Frame inputs for the control plane.
        let info = VehicleTaskInfo {
            current: ctx
                .runner
                .as_ref()
                .and_then(|r| r.current_task())
                .map(|ti| plan.tasks[ti].id.clone()),
            queue: ctx
                .runner
                .as_ref()
                .map(|r| {
                    r.queue_snapshot()
                        .into_iter()
                        .map(|ti| plan.tasks[ti].id.clone())
                        .collect()
                })
                .unwrap_or_default(),
        };
        api.set_vehicle_tasks(index, info);
    }

    // -- landing / re-arm bookkeeping ----------------------------------------

    fn landing_pass(&mut self, now: u64) {
        let count = self.ctxs.len() as u8;
        for i in 0..count {
            let fsm = self.registry.fsm(i).unwrap();
            let snap = self.registry.snapshot(i);
            let ctx = &mut self.ctxs[i as usize];

            if fsm == FsmState::Rtl {
                let since = ctx.rtl_since_ms.unwrap_or(now);
                let disarmed = snap.as_ref().map(|s| !s.armed).unwrap_or(true);
                if (disarmed && now > since + 2_000) || now > since + LAND_TIMEOUT_MS {
                    if now > since + LAND_TIMEOUT_MS {
                        self.log.log(
                            EventKind::SupervisorAction,
                            Some(i),
                            format!(
                                "disarm not observed within {}s; treating vehicle as LANDED (watchdog — check RTL/land progress and teardown)",
                                LAND_TIMEOUT_MS / 1000
                            ),
                        );
                    } else {
                        self.log.log(
                            EventKind::SupervisorAction,
                            Some(i),
                            "disarm observed after RTL — vehicle LANDED",
                        );
                    }
                    self.registry
                        .apply_transition(i, FsmCause::DisarmObserved, &self.log);
                    ctx.landed_since_ms = Some(now);
                    ctx.arm_last_ms = None; // next mission cycle arms again
                }
            }
            if fsm == FsmState::Landed && ctx.landed_since_ms.is_none() {
                ctx.landed_since_ms = Some(now);
                ctx.arm_last_ms = None;
            }

            // Re-arm recovery (§4 LANDED→READY; §6.5 reallocation path).
            if fsm == FsmState::Landed
                && !self.pool.is_empty()
                && ctx.rearm_cycles < MAX_REARM
                && !self.policy.estop
            {
                if let Some(t) = ctx.landed_since_ms {
                    if now > t + 2_000 {
                        ctx.rearm_cycles += 1;
                        self.registry.apply_transition(i, FsmCause::Rearm, &self.log);
                        self.log.log(
                            EventKind::SupervisorAction,
                            Some(i),
                            format!(
                                "re-arm cycle {}/{}: LANDED→READY, pool has {} task(s) (§6.5 recovery)",
                                ctx.rearm_cycles,
                                MAX_REARM,
                                self.pool.len()
                            ),
                        );
                        // The abandoned active task returns to the pool now
                        // that the vehicle is safely on the ground.
                        if let Some(r) = ctx.runner.as_mut() {
                            if let Some(ti) = r.current_task() {
                                let state =
                                    self.api.tasks.lock().unwrap().get(ti).map(|t| t.state.clone());
                                if state.as_deref() != Some("complete") {
                                    set_task(&self.api, ti, |t| {
                                        t.state = "pending".into();
                                        t.assigned = None;
                                    });
                                    self.pool.push(ti);
                                    self.log.log(
                                        EventKind::SupervisorAction,
                                        Some(i),
                                        format!(
                                            "task '{}' abandoned mid-flight by vehicle {i}; returned to pool at landing",
                                            self.plan.tasks[ti].id
                                        ),
                                    );
                                }
                            }
                        }
                        ctx.runner = None; // fresh runner from the next auction
                    }
                }
            }
        }
    }

    // -- allocation -----------------------------------------------------------

    // -- operator control plane (ADR-0017) -----------------------------------

    /// Drop queued operator tasks: out of the pool, out of runner queues
    /// (pooled back, then filtered), task table state -> "cleared". The
    /// active task of a flying runner is never touched (§6.5 semantics).
    /// Returns how many tasks were dropped.
    fn clear_queued_operator_tasks(&mut self) -> usize {
        // 1. take every runner's queued indices back to the pool (the
        //    runner keeps its current task).
        for ctx in self.ctxs.iter_mut() {
            if let Some(runner) = ctx.runner.as_mut() {
                let pooled = runner.pool_remaining();
                self.pool.extend(pooled);
            }
        }
        // 2. filter operator tasks out of the pool (index-aligned flags —
        //    task indices are stable, the plan only ever appends).
        let is_op: Vec<bool> = self
            .plan
            .tasks
            .iter()
            .map(|t| t.id.starts_with("op"))
            .collect();
        let is_op_at = |ti: usize| is_op.get(ti).copied().unwrap_or(false);
        let pool_now = std::mem::take(&mut self.pool);
        let dropped: Vec<usize> = pool_now.iter().copied().filter(|&ti| is_op_at(ti)).collect();
        self.pool = pool_now.into_iter().filter(|&ti| !is_op_at(ti)).collect();
        // 3. mark them cleared in the shared task table.
        for &ti in &dropped {
            set_task(&self.api, ti, |t| t.state = "cleared".into());
        }
        if !dropped.is_empty() {
            let ids: Vec<&str> = dropped.iter().map(|&ti| self.plan.tasks[ti].id.as_str()).collect();
            self.log.log(
                EventKind::SupervisorAction,
                None,
                format!("operator: cleared queued mission tasks {ids:?} (active task untouched)"),
            );
        }
        dropped.len()
    }

    /// Drain the REST-queued operator commands (ADR-0017). Runs on the tick
    /// loop — the supervisor stays the single writer of plan/pool/runners —
    /// and answers each on its oneshot with the honest result.
    fn drain_operator_cmds(&mut self, now: u64) {
        for cmd in self.api.take_operator_cmds() {
            match cmd {
                OperatorCmd::Upload { items, replace, ack } => {
                    let result = self.operator_upload(items, replace);
                    let _ = ack.send(result);
                }
                OperatorCmd::Start { ack } => {
                    let result = self.operator_start(now);
                    let _ = ack.send(result);
                }
                OperatorCmd::Clear { ack } => {
                    let cleared = self.clear_queued_operator_tasks();
                    let _ = ack.send(ClearAck { cleared });
                }
                OperatorCmd::Append { tasks, ack } => {
                    let result = self.operator_append(tasks);
                    let _ = ack.send(result);
                }
                OperatorCmd::HotLoad { ack } => {
                    let result = self.operator_hotload(now);
                    let _ = ack.send(result);
                }
            }
        }
    }

    /// Validate + inject operator waypoints: geo -> NED against the scenario
    /// origin, the compiler's own rules (fence polygon, altitude box,
    /// reachability), then into the task board + pool. Invalid items are
    /// rejected with reasons — never discovered mid-flight.
    fn operator_upload(&mut self, items: Vec<OperatorWaypoint>, replace: bool) -> UploadAck {
        if replace {
            self.clear_queued_operator_tasks();
        }
        let origin = self.api.geo_origin;
        let reach = fleet_mission::compile::max_reach_m(&self.plan.fence);
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
            // QGC convention: waypoint altitude is AGL -> geodetic
            // alt = origin + AGL -> NED z = -AGL.
            let z = -(item.alt_m as f32);
            let ned = origin.geodetic_to_ned(item.lat_deg, item.lon_deg, origin.alt_m + item.alt_m);
            let xy = [ned[0] as f32, ned[1] as f32];
            if !self.plan.fence.contains_xy(xy) {
                let breach = self.plan.fence.horizontal_breach(xy);
                rejected.push((
                    label,
                    format!(
                        "outside geofence polygon (lat {:.6}, lon {:.6}, breach {:.0} m)",
                        item.lat_deg, item.lon_deg, breach
                    ),
                ));
                continue;
            }
            if !self.plan.fence.altitude_ok(z) {
                rejected.push((
                    label,
                    format!(
                        "outside altitude box (AGL {:.1} m; box {}..{} m)",
                        item.alt_m, self.plan.fence.floor_m, self.plan.fence.ceiling_m
                    ),
                ));
                continue;
            }
            let d = xy[0].hypot(xy[1]);
            if d > reach {
                rejected.push((
                    label,
                    format!("unreachable: {d:.0} m from home, budget {reach:.0} m"),
                ));
                continue;
            }
            // accepted: same three structures the scenario path builds
            // (alloc Task + runner RunnerTask + shared TaskStatus), and the
            // pool — the sequential auction awards it from the next tick.
            let ti = self.plan.tasks.len();
            self.operator_seq += 1;
            let id = format!("op{}", self.operator_seq);
            let task = fleet_alloc::Task {
                id: id.clone(),
                pos_ned_m: [xy[0], xy[1]],
                hover_s: item.hover_s,
                reward: 1.0,
                deadline_s: f32::INFINITY,
            };
            self.plan.tasks.push(task);
            self.runner_tasks.push(RunnerTask {
                id: id.clone(),
                pos_ned_m: [xy[0], xy[1], z],
                hover_s: item.hover_s,
            });
            self.api.tasks.lock().unwrap().push(TaskStatus {
                id: id.clone(),
                pos_ned_m: [xy[0], xy[1], z],
                assigned: None,
                state: "pending".into(),
                hover_observed: None,
            });
            self.pool.push(ti);
            accepted.push(id);
        }
        let pool = self.pool.len();
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

    /// Append NED tasks at runtime (`POST /api/tasks`, ADR-0018): the same
    /// validation rules and the same three structures the upload path
    /// builds — but the positions arrive already in the scenario DSL's own
    /// frame (NED metres, home-relative), so no geo conversion happens.
    /// Appending grows the pool; the next auction round reallocates over
    /// it (spec §6.5), including mid-mission (vehicles that can take more
    /// work pick the new tasks up at their queue boundary).
    fn operator_append(&mut self, tasks: Vec<OperatorTask>) -> UploadAck {
        let reach = fleet_mission::compile::max_reach_m(&self.plan.fence);
        let mut accepted: Vec<String> = Vec::new();
        let mut rejected: Vec<(String, String)> = Vec::new();
        for (k, task) in tasks.into_iter().enumerate() {
            let label = task
                .id
                .clone()
                .unwrap_or_else(|| format!("task[{}]", k));
            if task.hover_s < 0.0 {
                rejected.push((label, "hover_s must be >= 0".into()));
                continue;
            }
            let [x, y, z] = task.pos_ned_m;
            if !self.plan.fence.contains_xy([x, y]) {
                let breach = self.plan.fence.horizontal_breach([x, y]);
                rejected.push((
                    label,
                    format!(
                        "outside geofence polygon (NED [{x:.0}, {y:.0}], breach {:.0} m)",
                        breach
                    ),
                ));
                continue;
            }
            if !self.plan.fence.altitude_ok(z) {
                rejected.push((
                    label,
                    format!(
                        "outside altitude box (NED z {z:.1} m = {:.1} m AGL; box {}..{} m)",
                        -z, self.plan.fence.floor_m, self.plan.fence.ceiling_m
                    ),
                ));
                continue;
            }
            let d = x.hypot(y);
            if d > reach {
                rejected.push((
                    label,
                    format!("unreachable: {d:.0} m from home, budget {reach:.0} m"),
                ));
                continue;
            }
            // id collision guard: the board is the live namespace.
            let id = match task.id {
                Some(explicit) => {
                    if self.plan.tasks.iter().any(|t| t.id == explicit) {
                        rejected.push((label, format!("id '{explicit}' already on the task board")));
                        continue;
                    }
                    if explicit.is_empty() {
                        rejected.push((label, "id must not be empty".into()));
                        continue;
                    }
                    explicit
                }
                None => {
                    self.operator_seq += 1;
                    format!("op{}", self.operator_seq)
                }
            };
            let ti = self.plan.tasks.len();
            let task_row = fleet_alloc::Task {
                id: id.clone(),
                pos_ned_m: [x, y],
                hover_s: task.hover_s,
                reward: task.reward,
                deadline_s: task.deadline_s.unwrap_or(f32::INFINITY),
            };
            self.plan.tasks.push(task_row);
            self.runner_tasks.push(RunnerTask {
                id: id.clone(),
                pos_ned_m: [x, y, z],
                hover_s: task.hover_s,
            });
            self.api.tasks.lock().unwrap().push(TaskStatus {
                id: id.clone(),
                pos_ned_m: [x, y, z],
                assigned: None,
                state: "pending".into(),
                hover_observed: None,
            });
            self.pool.push(ti);
            accepted.push(id);
        }
        let pool = self.pool.len();
        if !accepted.is_empty() {
            self.log.log(
                EventKind::SupervisorAction,
                None,
                format!(
                    "operator task append: {} accepted ({:?}), {} rejected, pool {pool} — reallocation next round",
                    accepted.len(),
                    accepted,
                    rejected.len()
                ),
            );
        }
        UploadAck { accepted, rejected, pool }
    }

    /// Hot scenario load (`PUT /api/fleet`, ADR-0018): the staged file is
    /// already validated by the REST handler; this side re-checks that the
    /// fleet is quiescent (the handler's view can be a hair stale), then
    /// gracefully aborts this run — the `run_scenario` loop restarts with
    /// the staged scenario on the same port. An e-stop pending wins (the
    /// operator asked to STOP, not to swap).
    fn operator_hotload(&mut self, now: u64) -> HotLoadAck {
        if self.api.estop_requested() {
            return HotLoadAck {
                accepted: false,
                reason: Some("e-stop pending — the fleet is shutting down".into()),
            };
        }
        let flying: Vec<u8> = (0..self.api.count)
            .filter(|&i| self.api.mission_active(i))
            .collect();
        if !flying.is_empty() {
            return HotLoadAck {
                accepted: false,
                reason: Some(format!(
                    "vehicles {flying:?} are flying an autonomous mission — land/hold before a scenario swap"
                )),
            };
        }
        if self.mission_started && !self.mission_complete {
            return HotLoadAck {
                accepted: false,
                reason: Some(
                    "mission started but not complete — clear or let it finish before a swap".into(),
                ),
            };
        }
        let staged = self
            .api
            .next_scenario_path()
            .map(|p| p.display().to_string())
            .unwrap_or_else(|| "<staged>".into());
        self.log.log(
            EventKind::RunBoundary,
            None,
            format!(
                "hot scenario load accepted: graceful stop — the fleet restarts with {staged} (PUT /api/fleet)"
            ),
        );
        // Graceful stop through the abort path: any airborne vehicle lands
        // + disarms, the report is written, teardown frees the ports, and
        // `run_scenario`'s loop boots the staged scenario.
        self.abort_run(now, "hot scenario load (PUT /api/fleet): fleet restarts with the staged scenario");
        HotLoadAck { accepted: true, reason: None }
    }

    /// Start the deferred operator mission (the setup-bench path to
    /// RUNNING). The auction + runners fly it exactly like a scenario
    /// mission; an already-running mission is a no-op with a reason.
    fn operator_start(&mut self, now: u64) -> StartAck {
        if self.mission_started {
            return StartAck {
                started: false,
                reason: Some("mission already started".into()),
            };
        }
        if self.mission_complete {
            return StartAck {
                started: false,
                reason: Some("run already complete".into()),
            };
        }
        if self.pool.is_empty() {
            return StartAck {
                started: false,
                reason: Some("no tasks to start — upload a mission first".into()),
            };
        }
        self.mission_started = true;
        self.mission_started_at = now;
        self.api.set_phase("RUNNING");
        let n = self.pool.len();
        self.log.log(
            EventKind::RunBoundary,
            None,
            format!(
                "operator mission start: {n} task(s) — the sequential auction takes it from the next tick"
            ),
        );
        StartAck { started: true, reason: None }
    }

    fn mission_start_gate(&mut self, now: u64) {
        if self.mission_started {
            return;
        }
        // Setup-bench mode (ADR-0016): hold pre-mission indefinitely —
        // vehicles sit READY and disarmed for the configuration workflow;
        // the run ends at max_time_s / SIGINT like any other run.
        if self.scenario.fleet.hold_for_setup {
            if self.api.phase() == "BRING_UP" {
                let ready = self
                    .ctxs
                    .iter()
                    .filter(|c| self.registry.fsm(c.index) == Some(FsmState::Ready))
                    .count();
                if ready > 0 {
                    self.api.set_phase("SETUP_HOLD");
                    self.log.log(
                        EventKind::RunBoundary,
                        None,
                        format!(
                            "setup bench: {}/{} vehicles READY and holding (hold_for_setup) — mission deferred, vehicles disarmed for configuration",
                            ready,
                            self.ctxs.len()
                        ),
                    );
                }
            }
            return;
        }
        let all_ready = self
            .ctxs
            .iter()
            .all(|c| self.registry.fsm(c.index) == Some(FsmState::Ready));
        let any_ready = self
            .ctxs
            .iter()
            .any(|c| self.registry.fsm(c.index) == Some(FsmState::Ready));
        if all_ready || (now > self.all_ready_deadline_ms && any_ready) {
            self.mission_started = true;
            self.mission_started_at = now;
            self.api.set_phase("RUNNING");
            let ready_n = self
                .ctxs
                .iter()
                .filter(|c| self.registry.fsm(c.index) == Some(FsmState::Ready))
                .count();
            self.log.log(
                EventKind::RunBoundary,
                None,
                format!(
                    "mission start: {} tasks, {}/{} vehicles READY{}",
                    self.plan.tasks.len(),
                    ready_n,
                    self.ctxs.len(),
                    if all_ready {
                        String::new()
                    } else {
                        " (all-ready deadline passed; late vehicles join via re-allocation)".into()
                    }
                ),
            );
            self.initial_allocation_logging();
            self.pool = (0..self.plan.tasks.len()).collect();
            // Open-loop profile estimate at §7.1's cruise cap (diagnostic).
            let max_leg = self
                .plan
                .tasks
                .iter()
                .map(|t| t.pos_ned_m[0].hypot(t.pos_ned_m[1]))
                .fold(0.0f32, f32::max);
            let t = crate::pump::profile_time_s(max_leg, crate::pump::CRUISE_MS);
            self.log.log(
                EventKind::RunBoundary,
                None,
                format!(
                    "task pool armed for the sequential auction (§6.3); longest leg {max_leg:.0} m ≈ {t:.0} s at cruise 4 m/s ≈ {} pump steps at 20 Hz",
                    (t / crate::pump::PUMP_DT_S) as u32
                ),
            );
        }
    }

    /// One-time auction evidence (§6.4): realized cost vs the Hungarian
    /// static optimum, logged for CI.
    fn initial_allocation_logging(&self) {
        let bids: Vec<fleet_alloc::BidVehicle> = self
            .ctxs
            .iter()
            .filter(|c| matches!(self.registry.fsm(c.index), Some(FsmState::Ready) | Some(FsmState::Active)))
            .map(|c| {
                let st = self.registry.snapshot(c.index).unwrap();
                fleet_alloc::BidVehicle::new(
                    c.index as usize,
                    [st.position_ned_m[0], st.position_ned_m[1]],
                    st.battery_pct,
                )
            })
            .collect();
        if bids.is_empty() || self.plan.tasks.is_empty() {
            return;
        }
        let a = fleet_alloc::auction(&self.plan.tasks, &bids);
        let opt = fleet_alloc::hungarian_static_optimum(&self.plan.tasks, &bids);
        let real = fleet_alloc::realized_cost(&a, &self.plan.tasks, &bids);
        let gap = if opt > 0.0 { 100.0 * (real - opt) / opt } else { 0.0 };
        self.log.log(
            EventKind::RunBoundary,
            None,
            format!(
                "auction: {} tasks x {} bidders; realized travel {real:.0}s vs hungarian static optimum {opt:.0}s (gap {gap:.1}%, queue effects excluded per §6.4/ADR-0007)",
                self.plan.tasks.len(),
                bids.len()
            ),
        );
    }

    /// Award pooled tasks to vehicles that can accept work (§6.5). The
    /// initial allocation is the same code path with the full task set.
    fn reallocation_pass(&mut self, _now: u64) {
        if self.pool.is_empty() || !self.mission_started {
            return;
        }
        let mut bids: Vec<fleet_alloc::BidVehicle> = Vec::new();
        for ctx in &self.ctxs {
            if ctx.no_new_tasks || ctx.standing_down {
                continue;
            }
            if !matches!(self.registry.fsm(ctx.index), Some(FsmState::Ready) | Some(FsmState::Active)) {
                continue;
            }
            let st = self.registry.snapshot(ctx.index).unwrap();
            bids.push(fleet_alloc::BidVehicle::new(
                ctx.index as usize,
                [st.position_ned_m[0], st.position_ned_m[1]],
                st.battery_pct,
            ));
        }
        if bids.is_empty() {
            return;
        }
        // queue_ahead per bidder (seconds of already-awarded work, §6.2).
        let queue_s: Vec<f32> = bids
            .iter()
            .map(|b| {
                self.ctxs[b.index]
                    .runner
                    .as_ref()
                    .map(|r| {
                        let mut pos = b.position;
                        let mut t = 0.0f32;
                        let mut order = r.queue_snapshot();
                        if let Some(c) = r.current_task() {
                            order.insert(0, c);
                        }
                        for ti in order {
                            t += fleet_alloc::travel_time(pos, self.plan.tasks[ti].pos_ned_m)
                                + self.plan.tasks[ti].hover_s;
                            pos = self.plan.tasks[ti].pos_ned_m;
                        }
                        t
                    })
                    .unwrap_or(0.0)
            })
            .collect();

        let pool_now = std::mem::take(&mut self.pool);
        let assignment = fleet_alloc::reallocate(&pool_now, &self.plan.tasks, &bids);
        let mut awarded = 0usize;
        for (v, queue) in assignment.per_vehicle.iter().enumerate() {
            if queue.is_empty() {
                continue;
            }
            let veh = bids[v].index;
            for &ti in queue {
                let cost = fleet_alloc::bid_cost(
                    bids[v].position,
                    &self.plan.tasks[ti],
                    queue_s[v],
                    bids[v].battery_pct,
                );
                self.log.log(
                    EventKind::TaskAward,
                    Some(veh as u8),
                    format!(
                        "task '{}' awarded to vehicle {veh} (bid {cost:.1}s)",
                        self.plan.tasks[ti].id
                    ),
                );
                set_task(&self.api, ti, |t| {
                    t.assigned = Some(veh as u8);
                    t.state = "queued".into();
                });
                awarded += 1;
            }
            let ctx = &mut self.ctxs[veh];
            if let Some(r) = ctx.runner.as_mut() {
                r.push_tasks(queue.clone());
            } else {
                ctx.runner = Some(MissionRunner::new(
                    veh,
                    queue.clone(),
                    self.runner_tasks.clone(),
                    self.plan.fence.clone(),
                ));
                ctx.standing_down = false;
            }
        }
        if awarded > 0 {
            let summary: Vec<String> = self
                .ctxs
                .iter()
                .map(|c| {
                    let ids: Vec<String> = c
                        .runner
                        .as_ref()
                        .map(|r| {
                            let mut order = r.queue_snapshot();
                            if let Some(cur) = r.current_task() {
                                order.insert(0, cur);
                            }
                            order
                                .into_iter()
                                .map(|ti| self.plan.tasks[ti].id.clone())
                                .collect()
                        })
                        .unwrap_or_default();
                    format!("v{}:{:?}", c.index, ids)
                })
                .collect();
            self.log.log(
                EventKind::RunBoundary,
                None,
                format!("allocation round: {awarded} task(s) awarded ({})", summary.join(", ")),
            );
        }
    }

    // -- timeline / battery gates ---------------------------------------------

    async fn fire_timeline(&mut self, now: u64) {
        for (k, ev) in self.scenario.events.iter().enumerate() {
            if self.fired.contains(&k) {
                continue;
            }
            let start_ms = (ev.start_s.unwrap_or(0.0) * 1000.0) as u64;
            if now < start_ms {
                continue;
            }
            self.fired.insert(k);
            match ev.kind.as_str() {
                "link_loss" => {
                    let v = ev.vehicle.unwrap_or(0);
                    let dur = ev.duration_s.unwrap_or(5.0).max(0.1);
                    if let Some(ctx) = self.ctxs.get(v as usize) {
                        ctx.handle
                            .shared
                            .set_link_loss(Some(Instant::now() + Duration::from_secs_f64(dur)));
                        self.log.log(
                            EventKind::FaultInjection,
                            Some(v),
                            format!(
                                "link_loss injected: manager drops inbound from vehicle {v} for {dur:.0}s (outbound continues, §8.1 policy 3 test)",
                            ),
                        );
                    }
                }
                "fault" => {
                    let v = ev.vehicle.unwrap_or(0);
                    let Some(f) = ev.fault.as_ref() else {
                        self.log.log(
                            EventKind::SimEvent,
                            Some(v),
                            "fault event without a [fault] table — skipped",
                        );
                        continue;
                    };
                    // ADR-0018: real injection — proxy to vehicle v's sim
                    // fault plane (:8200+v, the sim's §7.3 REST endpoint).
                    // The sim owns the catalog (serde-validated) and mints
                    // the runtime-N fault id; both land in the event log.
                    let body = serde_json::json!({
                        "type": f.kind,
                        "duration_ms": f.duration_ms,
                    });
                    let attempt = crate::simproxy::post_json(
                        fleet_simctl::ports::sim_api(v),
                        "/api/faults",
                        &body,
                    )
                    .await;
                    // Boot window retry: an unreachable/early plane right
                    // after a hot restart (or a WAIT-phase sim) is retried
                    // on subsequent ticks for up to 10 s from the FIRST
                    // attempt — silently, to keep the 10 Hz log clean. Only
                    // the final outcome logs.
                    let success = matches!(&attempt, Ok((200, _)));
                    if !success {
                        let first = *self.fault_attempts.entry(k).or_insert(now);
                        if now < first + FAULT_RETRY_WINDOW_MS {
                            self.fired.remove(&k);
                            continue;
                        }
                    }
                    self.fault_attempts.remove(&k);
                    match attempt {
                        Ok((code, sim_body)) if code == 200 => {
                            let id = sim_body["data"]["id"]
                                .as_str()
                                .unwrap_or("?")
                                .to_string();
                            self.log.log(
                                EventKind::FaultInjection,
                                Some(v),
                                format!(
                                    "fault '{}' injected into vehicle {v}'s sim fault plane (id {id}, starts next sim tick)",
                                    f.kind
                                ),
                            );
                        }
                        Ok((code, sim_body)) => {
                            let reason = sim_body["error"]
                                .as_str()
                                .unwrap_or("(no reason)")
                                .to_string();
                            self.log.log(
                                EventKind::FaultInjection,
                                Some(v),
                                format!(
                                    "fault '{}' REJECTED by vehicle {v}'s sim plane: HTTP {} — {}",
                                    f.kind, code, reason
                                ),
                            );
                        }
                        Err(e) => {
                            self.log.log(
                                EventKind::FaultInjection,
                                Some(v),
                                format!(
                                    "fault '{}' proxy to vehicle {v} FAILED: {e}",
                                    f.kind
                                ),
                            );
                        }
                    }
                }
                "estop" => {
                    self.api.request_estop();
                    self.log.log(
                        EventKind::SupervisorAction,
                        None,
                        "scenario estop event fired (policy 1)",
                    );
                }
                _ => {}
            }
        }
    }

    fn battery_gate(&mut self, now: u64) {
        if self.battery_gate_decided {
            return;
        }
        let valid = self.ctxs.iter().any(|c| {
            self.registry
                .snapshot(c.index)
                .map(|s| (1..=100).contains(&s.battery_pct))
                .unwrap_or(false)
        });
        if valid {
            self.battery_gate_decided = true;
            return;
        }
        if self.mission_started && now > self.mission_started_at + 15_000 {
            self.policy.battery_ladder_enabled = false;
            self.battery_gate_decided = true;
            self.log.log(
                EventKind::RunBoundary,
                None,
                "battery telemetry never became valid — policies 4/5 (battery ladder) disabled with this warning per §8.1 (interim sim has no battery model)",
            );
        }
    }

    // -- termination ------------------------------------------------------------

    fn termination_check(&mut self, now: u64) -> Outcome {
        // E-stop abort (policy 1).
        if self.policy.estop && self.abort_grace_until.is_none() {
            self.abort_run(now, "e-stop (policy 1: LAND all, ABORT run)");
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

        // Mission completion: every task complete or rejected.
        if self.mission_started && !self.mission_complete {
            let all_done = {
                let tasks = self.api.tasks.lock().unwrap();
                tasks
                    .iter()
                    .all(|t| t.state == "complete" || t.state == "rejected")
            };
            if all_done && self.pool.is_empty() {
                self.mission_complete = true;
                self.api.set_phase("LANDING");
                self.log.log(
                    EventKind::RunBoundary,
                    None,
                    "mission complete: all tasks resolved — landing sequence",
                );
                let count = self.ctxs.len() as u8;
                for i in 0..count {
                    if self.registry.fsm(i) == Some(FsmState::Active) {
                        let ctx = &mut self.ctxs[i as usize];
                        self.registry
                            .apply_transition(i, FsmCause::TaskComplete, &self.log);
                        ctx.rtl_since_ms = Some(now);
                        ctx.standing_down = true;
                        spawn_rtl(ctx.handle.clone(), Arc::clone(&self.log), i);
                        ctx.handle.shared.stop_stream();
                    }
                }
            }
        }
        if self.mission_complete
            && !self
                .ctxs
                .iter()
                .any(|c| self.registry.fsm(c.index).map(|f| f.is_flying()).unwrap_or(false))
        {
            self.api.set_phase("COMPLETE");
            return Outcome::Stop;
        }

        // Pool drain failure (F-5's "pool drained or explicitly failed").
        if self.mission_started
            && !self.mission_complete
            && !self.pool.is_empty()
            && !self.unassignable_reported
        {
            let any_accepting = self.ctxs.iter().any(|c| {
                matches!(
                    self.registry.fsm(c.index),
                    Some(FsmState::Ready) | Some(FsmState::Active)
                ) && !c.standing_down
                    && !c.no_new_tasks
            });
            let any_rearmable = self.ctxs.iter().any(|c| {
                self.registry.fsm(c.index) == Some(FsmState::Landed) && c.rearm_cycles < MAX_REARM
            });
            let any_working = self.ctxs.iter().any(|c| {
                c.runner
                    .as_ref()
                    .map(|r| matches!(r.phase(), RunnerPhase::Engaging { .. } | RunnerPhase::Flying))
                    .unwrap_or(false)
            });
            if !any_accepting && !any_rearmable && !any_working {
                self.unassignable_reported = true;
                let ids: Vec<String> = self
                    .pool
                    .iter()
                    .map(|&ti| self.plan.tasks[ti].id.clone())
                    .collect();
                self.log.log(
                    EventKind::RunBoundary,
                    None,
                    format!("tasks {ids:?} unassignable: no vehicle can accept work; aborting (F-5 pool-drain rule)"),
                );
                self.abort_run(now, "remaining tasks unassignable");
            }
        }

        Outcome::Continue
    }

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
            ctx.standing_down = true;
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

    /// Stand a vehicle down (ladder/process death): pool its queued tasks.
    fn stand_down(&mut self, index: u8, now: u64) {
        let ctx = &mut self.ctxs[index as usize];
        ctx.standing_down = true;
        ctx.handle.shared.stop_stream();
        ctx.rtl_since_ms = Some(now);
        if let Some(r) = ctx.runner.as_mut() {
            let pooled = r.pool_remaining();
            if !pooled.is_empty() {
                for ti in &pooled {
                    set_task(&self.api, *ti, |t| {
                        t.state = "pending".into();
                        t.assigned = None;
                    });
                }
                self.pool.extend(pooled);
            }
        }
    }

    // -- teardown + report ------------------------------------------------------

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

        // Release THIS run's own sockets before the port probe (ADR-0018 hot
        // restart): the link tasks exit when every LinkHandle drops (their
        // command channel closes) — otherwise the probe below sees our own
        // telemetry/onboard ports as still bound and warns spuriously. The
        // supervisor is the single writer, so this is safe here.
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

        // Run report (§9.2): written regardless of outcome.
        let mut run_report = report::build_report(
            &self.api,
            &self.registry,
            &self.log,
            &self.scenario_path,
            &phase,
            self.api.aborted.load(Ordering::SeqCst),
            now,
        );
        let criteria = report::evaluate(&mut run_report, &self.scenario);
        let report_path = self.run_dir.join("run-report.json");
        if let Err(e) = report::write_report(&run_report, &self.run_dir) {
            eprintln!("[fleet] report write failed: {e}");
        }
        println!("[fleet] report: {}", report_path.display());
        println!(
            "[fleet] events: {} total, {} in ring",
            self.log.total(),
            self.log.tail(1).len()
        );
        for c in &criteria {
            println!(
                "[fleet] criterion {} {}: {}",
                if c.passed { "PASS" } else { "FAIL" },
                c.name,
                c.detail
            );
        }
        for v in &run_report.vehicles {
            println!(
                "[fleet] vehicle {}: fsm {} mode {} tasks done {} remaining {}",
                v.index,
                v.final_fsm,
                v.final_mode,
                v.tasks_completed.len(),
                v.tasks_remaining.len()
            );
        }
        println!("[fleet] exit code {}", run_report.exit_code);
        run_report.exit_code
    }
}
