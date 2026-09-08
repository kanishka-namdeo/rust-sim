//! Shared application state: the read-side view the REST/WS plane serves and
//! the supervisor writes (spec §3.4, §5). The frame builder here is the one
//! code path behind both `GET /api/fleet` and the WS stream, so the wire
//! contract can't drift between them.

#![forbid(unsafe_code)]

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, RwLock};

use fleet_core::events::{fleet_epoch_ms, Event, EventLog};
use fleet_core::geo::GeoOrigin;
use fleet_core::health::HealthFlag;
use fleet_core::registry::Registry;
use fleet_core::tick::{FleetFrame, GeofenceView, VehicleView};
use fleet_mavlink::LinkHandle;
use fleet_mission::report::TaskStatus;

/// Per-vehicle task-queue view (written by the supervisor each tick, read by
/// the frame builder — the runner itself is not shareable state).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct VehicleTaskInfo {
    pub current: Option<String>,
    pub queue: Vec<String>,
}

// ---------------------------------------------------------------------------
// Operator control plane (ADR-0017)
// ---------------------------------------------------------------------------

/// One operator waypoint as uploaded from the map (`POST /api/mission`).
/// `alt_m` is metres AGL above the scenario origin (QGC's waypoint altitude
/// convention); conversion to NED happens at drain time.
#[derive(Debug, Clone)]
pub struct OperatorWaypoint {
    pub label: Option<String>,
    pub lat_deg: f64,
    pub lon_deg: f64,
    pub alt_m: f64,
    pub hover_s: f32,
}

/// Upload ack: accepted task ids + rejected labels with reasons (the
/// compiler's own validation rules, applied at drain time).
#[derive(Debug, Clone)]
pub struct UploadAck {
    pub accepted: Vec<String>,
    pub rejected: Vec<(String, String)>,
    pub pool: usize,
}

#[derive(Debug, Clone)]
pub struct StartAck {
    pub started: bool,
    pub reason: Option<String>,
}

#[derive(Debug, Clone)]
pub struct ClearAck {
    pub cleared: usize,
}

/// One operator task as appended at runtime (`POST /api/tasks`, ADR-0018):
/// NED metres, home-relative — the direct counterpart of the geo waypoints
/// `POST /api/mission` accepts. `id` is optional (auto `op*` id when
/// omitted); ids must not collide with the live task board.
#[derive(Debug, Clone)]
pub struct OperatorTask {
    pub id: Option<String>,
    pub pos_ned_m: [f32; 3],
    pub hover_s: f32,
    pub reward: f32,
    pub deadline_s: Option<f32>,
}

/// Hot scenario load ack (`PUT /api/fleet`, ADR-0018): accepted means the
/// supervisor has gracefully stopped the current run and the process will
/// re-enter `run_scenario`'s loop with the staged scenario file; the
/// control plane rebinds on the same port within seconds.
#[derive(Debug, Clone)]
pub struct HotLoadAck {
    pub accepted: bool,
    pub reason: Option<String>,
}

/// Operator commands queued by the REST plane and drained by the
/// supervisor's tick loop (the same discipline as ADR-0016 restart
/// requests: the supervisor is the single writer of the mission state).
#[derive(Debug)]
pub enum OperatorCmd {
    /// Upload waypoints (append, or replace queued operator tasks).
    Upload {
        items: Vec<OperatorWaypoint>,
        replace: bool,
        ack: tokio::sync::oneshot::Sender<UploadAck>,
    },
    /// Start the (deferred) mission — the setup-bench path to RUNNING.
    Start { ack: tokio::sync::oneshot::Sender<StartAck> },
    /// Drop queued operator tasks (the active task is never touched).
    Clear { ack: tokio::sync::oneshot::Sender<ClearAck> },
    /// Append NED tasks at runtime (ADR-0018, `POST /api/tasks`) — same
    /// validation + injection path as the mission upload, and the same
    /// reallocation trigger (the next auction round over the pool).
    Append {
        tasks: Vec<OperatorTask>,
        ack: tokio::sync::oneshot::Sender<UploadAck>,
    },
    /// Hot scenario load (ADR-0018, `PUT /api/fleet`): the staged file is
    /// in `next_scenario`; on accept the supervisor aborts this run
    /// gracefully and `run_scenario` restarts with the new scenario.
    HotLoad { ack: tokio::sync::oneshot::Sender<HotLoadAck> },
}

/// The control-plane-visible fleet state.
pub struct AppState {
    pub registry: Arc<Registry>,
    pub log: Arc<EventLog>,
    pub count: u8,
    pub scenario_path: String,
    pub phase: RwLock<String>,
    pub tick_count: AtomicU64,
    pub aborted: AtomicBool,
    estop: AtomicBool,
    /// Task table (the run report's task section, live).
    pub tasks: Mutex<Vec<TaskStatus>>,
    /// Per-vehicle current/queued task ids.
    pub vehicle_tasks: Mutex<Vec<VehicleTaskInfo>>,
    /// Last computed health flag set per vehicle.
    pub flags: Mutex<Vec<Vec<HealthFlag>>>,
    /// Per-vehicle MAVLink link handles — registered by the manager as it
    /// spawns links, read by the setup control plane (ADR-0016). `None`
    /// until the link is up (or if bind failed).
    pub links: Mutex<Vec<Option<LinkHandle>>>,
    /// Pending vehicle restarts (ADR-0016 setup workflow: airframe apply).
    /// POST /api/vehicles/{i}/airframe queues i here after the
    /// `SYS_AUTOSTART` write is confirmed; the manager's run loop drains
    /// it and restarts the sim+px4 pair (not a process-death FAULT).
    restart_requests: Mutex<std::collections::BTreeSet<u8>>,
    /// Operator command queue (ADR-0017): drained by the supervisor tick.
    operator_cmds: Mutex<std::collections::VecDeque<OperatorCmd>>,
    /// Per-vehicle "flying a mission right now" (runner live) — the gate
    /// guided commands check so a user command can never fight a runner.
    mission_active: Mutex<Vec<bool>>,
    /// Geo origin (ADR-0017): `[env] origin` — published on the frame and
    /// used for all operator-plane geo conversion.
    pub geo_origin: GeoOrigin,
    /// The real fence (for go-to clamping, runner rule §7.2) and the
    /// frame-published view of it (so the console draws the *real* fence,
    /// not a client fallback).
    pub fence: fleet_safety::geofence::Geofence,
    pub fence_view: GeofenceView,
    pub started_unix: u64,
    /// This run's directory (report + events + staged hot-swap scenarios).
    pub run_dir: std::path::PathBuf,
    /// Staged next scenario (ADR-0018, `PUT /api/fleet`): set by the REST
    /// handler after validation, consumed by `run_scenario`'s loop after
    /// the supervisor's graceful stop. Cleared on a supervisor nack.
    next_scenario: Mutex<Option<std::path::PathBuf>>,
}

impl AppState {
    pub fn new(
        registry: Arc<Registry>,
        log: Arc<EventLog>,
        count: u8,
        scenario_path: impl Into<String>,
        started_unix: u64,
        geo_origin: GeoOrigin,
        fence: fleet_safety::geofence::Geofence,
        run_dir: impl Into<std::path::PathBuf>,
    ) -> Arc<AppState> {
        let fence_view = GeofenceView {
            points_ned_m: fence.points.clone(),
            ceiling_m: fence.ceiling_m,
            floor_m: fence.floor_m,
        };
        Arc::new(AppState {
            registry,
            log,
            count,
            scenario_path: scenario_path.into(),
            phase: RwLock::new("INIT".into()),
            tick_count: AtomicU64::new(0),
            aborted: AtomicBool::new(false),
            estop: AtomicBool::new(false),
            tasks: Mutex::new(Vec::new()),
            vehicle_tasks: Mutex::new(vec![VehicleTaskInfo::default(); count as usize]),
            flags: Mutex::new(vec![Vec::new(); count as usize]),
            links: Mutex::new(vec![None; count as usize]),
            restart_requests: Mutex::new(std::collections::BTreeSet::new()),
            operator_cmds: Mutex::new(std::collections::VecDeque::new()),
            mission_active: Mutex::new(vec![false; count as usize]),
            geo_origin,
            fence,
            fence_view,
            started_unix,
            run_dir: run_dir.into(),
            next_scenario: Mutex::new(None),
        })
    }

    /// Minimal state for the early-error paths of `run_once` (scenario
    /// unreadable / run dir unwritable): keeps the return shape uniform so
    /// the hot-restart loop can still read (empty) staging state before the
    /// process exits. Never serves a request.
    pub fn new_for_error() -> Arc<AppState> {
        let registry = Registry::new(0);
        let log = EventLog::in_memory();
        AppState::new(
            registry,
            log,
            0,
            "<none>",
            fleet_mission::report::unix_now(),
            GeoOrigin::DEFAULT,
            fleet_safety::geofence::Geofence::default_square(),
            std::env::temp_dir(),
        )
    }

    /// Register vehicle i's link handle (manager, after spawn_link).
    pub fn set_link(&self, index: u8, handle: LinkHandle) {
        if let Some(slot) = self.links.lock().unwrap().get_mut(index as usize) {
            *slot = Some(handle);
        }
    }

    /// Vehicle i's link handle, if registered (setup control plane).
    pub fn link(&self, index: u8) -> Option<LinkHandle> {
        self.links.lock().unwrap().get(index as usize).and_then(|s| s.clone())
    }

    /// Queue a controlled vehicle restart (setup workflow, ADR-0016).
    /// Idempotent; the manager drains via `take_restart_requests`.
    pub fn request_restart(&self, index: u8) {
        if (index as usize) < self.count as usize {
            self.restart_requests.lock().unwrap().insert(index);
        }
    }

    /// Drain pending restart requests (manager run loop, one shot).
    pub fn take_restart_requests(&self) -> Vec<u8> {
        let mut q = self.restart_requests.lock().unwrap();
        let out: Vec<u8> = q.iter().copied().collect();
        q.clear();
        out
    }

    /// Pending (queued, not yet actioned) restart for i?
    pub fn restart_pending(&self, index: u8) -> bool {
        self.restart_requests.lock().unwrap().contains(&index)
    }

    /// Operator e-stop request (POST /api/estop or scenario estop event).
    /// The supervisor latches it into the policy engine on its next tick.
    pub fn request_estop(&self) {
        self.estop.store(true, Ordering::SeqCst);
    }

    /// Queue an operator command (ADR-0017 REST plane). The supervisor
    /// drains it on its next tick (≤ 200 ms at 10 Hz) and answers on the
    /// embedded oneshot.
    pub fn push_operator_cmd(&self, cmd: OperatorCmd) {
        self.operator_cmds.lock().unwrap().push_back(cmd);
    }

    /// Drain pending operator commands (supervisor tick, one shot).
    pub fn take_operator_cmds(&self) -> Vec<OperatorCmd> {
        let mut q = self.operator_cmds.lock().unwrap();
        q.drain(..).collect()
    }

    /// Stage the next scenario (ADR-0018 `PUT /api/fleet`, after
    /// validation). The supervisor acks or nacks; on nack the REST handler
    /// clears this again.
    pub fn set_next_scenario(&self, path: std::path::PathBuf) {
        *self.next_scenario.lock().unwrap() = Some(path);
    }

    /// Take (and clear) the staged next scenario — `run_scenario`'s loop
    /// reads this after a run ends to decide whether to hot-restart.
    pub fn take_next_scenario(&self) -> Option<std::path::PathBuf> {
        self.next_scenario.lock().unwrap().take()
    }

    /// Peek at the staged next scenario without consuming it (the
    /// supervisor's hot-load handler logs where the fleet is heading).
    pub fn next_scenario_path(&self) -> Option<std::path::PathBuf> {
        self.next_scenario.lock().unwrap().clone()
    }

    /// Per-vehicle "mission live" flag (supervisor writes each tick:
    /// runner present and engaging/flying — the guided-command gate).
    pub fn set_mission_active(&self, index: u8, active: bool) {
        if let Some(slot) = self.mission_active.lock().unwrap().get_mut(index as usize) {
            *slot = active;
        }
    }

    /// Is vehicle i currently flying an autonomous mission? Guided
    /// commands (go-to, arm) are gated on this so they never fight a
    /// runner's setpoint stream (ADR-0017).
    pub fn mission_active(&self, index: u8) -> bool {
        self.mission_active.lock().unwrap().get(index as usize).copied().unwrap_or(false)
    }

    pub fn estop_requested(&self) -> bool {
        self.estop.load(Ordering::SeqCst)
    }

    pub fn set_phase(&self, phase: &str) {
        *self.phase.write().unwrap() = phase.to_string();
    }

    pub fn phase(&self) -> String {
        self.phase.read().unwrap().clone()
    }

    pub fn set_flags(&self, index: u8, flags: Vec<HealthFlag>) {
        if let Some(slot) = self.flags.lock().unwrap().get_mut(index as usize) {
            *slot = flags;
        }
    }

    pub fn set_vehicle_tasks(&self, index: u8, info: VehicleTaskInfo) {
        if let Some(slot) = self.vehicle_tasks.lock().unwrap().get_mut(index as usize) {
            *slot = info;
        }
    }

    pub fn tasks_snapshot(&self) -> Vec<TaskStatus> {
        self.tasks.lock().unwrap().clone()
    }

    fn vehicle_tasks_snapshot(&self) -> Vec<VehicleTaskInfo> {
        self.vehicle_tasks.lock().unwrap().clone()
    }

    fn flags_snapshot(&self) -> Vec<Vec<HealthFlag>> {
        self.flags.lock().unwrap().clone()
    }

    /// Health flags for one vehicle, exposed for the pre-arm checklist
    /// (M3 Fly View, `GET /api/vehicles/{i}/prearm-checks`). Empty when
    /// the index is out of range or no flags have been computed yet.
    pub fn vehicle_flags(&self, index: u8) -> Vec<HealthFlag> {
        self.flags_snapshot()
            .get(index as usize)
            .cloned()
            .unwrap_or_default()
    }
}

/// The fleet frame (spec §3.4): the same payload behind `GET /api/fleet`,
/// the WS stream and the report's live view.
pub fn fleet_frame(s: &AppState, events_tail: usize) -> FleetFrame {
    let now_ms = fleet_epoch_ms();
    let vehicle_tasks = s.vehicle_tasks_snapshot();
    let flags = s.flags_snapshot();
    let mut vehicles = Vec::with_capacity(s.count as usize);
    for i in 0..s.count {
        let fsm = s.registry.fsm(i).unwrap_or(fleet_core::fsm::FsmState::Init);
        let snap = match s.registry.snapshot(i) {
            Some(st) => st,
            None => continue,
        };
        let info = vehicle_tasks
            .get(i as usize)
            .cloned()
            .unwrap_or_default();
        let fl = flags.get(i as usize).cloned().unwrap_or_default();
        vehicles.push(VehicleView::from_state(fsm, &snap, &fl, info.queue, info.current));
    }
    let tasks = serde_json::to_value(&s.tasks_snapshot()).ok();
    FleetFrame {
        t_ms: now_ms,
        phase: s.phase(),
        scenario: s.scenario_path.clone(),
        tick_count: s.tick_count.load(Ordering::SeqCst),
        vehicles,
        tasks,
        geo_origin: Some(s.geo_origin),
        geofence: Some(s.fence_view.clone()),
        events_tail: s.log.tail(events_tail),
    }
}

/// Full vehicle detail (spec §3.4 `GET /api/vehicles/{i}`): the frame view
/// plus arrival ages and bring-up gates.
pub fn vehicle_detail(s: &AppState, index: u8) -> Option<serde_json::Value> {
    if index >= s.count {
        return None;
    }
    let now_ms = fleet_epoch_ms();
    let fsm = s.registry.fsm(index)?;
    let snap = s.registry.snapshot(index)?;
    let info = s
        .vehicle_tasks_snapshot()
        .get(index as usize)
        .cloned()
        .unwrap_or_default();
    let flags = s
        .flags_snapshot()
        .get(index as usize)
        .cloned()
        .unwrap_or_default();
    let view = VehicleView::from_state(fsm, &snap, &flags, info.queue, info.current);
    let mut j = serde_json::to_value(&view).ok()?;
    j["now_ms"] = serde_json::json!(now_ms);
    j["telemetry_age_ms"] = serde_json::json!(now_ms.saturating_sub(snap.last_msg_ms));
    j["heartbeat_seen"] = serde_json::json!(snap.heartbeat_seen);
    j["local_position_seen"] = serde_json::json!(snap.local_position_seen);
    j["home_set"] = serde_json::json!(snap.home_set);
    j["position_samples"] = serde_json::json!(snap.position_samples);
    j["heartbeat_age_ms"] = if snap.last_heartbeat_ms > 0 {
        serde_json::json!(now_ms.saturating_sub(snap.last_heartbeat_ms))
    } else {
        serde_json::json!(-1)
    };
    Some(j)
}

/// Event-log tail as JSON values.
pub fn events_tail(s: &AppState, n: usize) -> Vec<Event> {
    s.log.tail(n)
}
