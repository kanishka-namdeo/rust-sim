//! Shared application state: the read-side view the REST/WS plane serves and
//! the supervisor writes (spec §3.4, §5). The frame builder here is the one
//! code path behind both `GET /api/fleet` and the WS stream, so the wire
//! contract can't drift between them.

#![forbid(unsafe_code)]

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, RwLock};

use fleet_core::events::{fleet_epoch_ms, unix_now, Event, EventLog};
use fleet_core::geo::GeoOrigin;
use fleet_core::health::HealthFlag;
use fleet_core::registry::Registry;
use fleet_core::tick::{FleetFrame, GeofenceView, VehicleView};
use fleet_core::TaskStatus;
use fleet_mavlink::LinkHandle;

/// Per-vehicle task-queue view (written by the supervisor each tick, read by
/// the frame builder — the operator mission upload is not shareable state,
/// it's drained through the OperatorCmd queue).
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

/// Upload ack: accepted task ids + rejected labels with reasons.
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

/// Operator commands queued by the REST plane and drained by the
/// supervisor's tick loop (the supervisor is the single writer of the
/// mission state).
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
    /// Task table (the operator-uploaded task board).
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
    /// Per-vehicle "flying a mission right now" — the gate guided commands
    /// check so a user command can never fight a live offboard setpoint
    /// stream from `POST /api/vehicles/{i}/goto` or
    /// `POST /api/fleet/start`.
    mission_active: Mutex<Vec<bool>>,
    /// M5 (Fleet C2, GCS_SPEC.md §5.4): per-vehicle mission bindings —
    /// one mission ULID per vehicle, `None` = unbound. Populated by
    /// `POST /api/fleet/mission-bindings`, drained by `POST /api/fleet/start`
    /// when it uploads + arms each bound vehicle.
    mission_bindings: Mutex<Vec<Option<String>>>,
    /// M5 (Fleet C2): the catalog server's base URL — `POST /api/fleet/start`
    /// fetches each bound mission from `GET {catalog_url}/api/missions/{id}`.
    /// Defaults to `http://127.0.0.1:8300`; overridden via the
    /// `RSIM_CATALOG_URL` env var (set by `mavfleet run` callers or the
    /// harness).
    catalog_url: String,
    /// Geo origin (ADR-0017): `[env] origin` — published on the frame and
    /// used for all operator-plane geo conversion.
    pub geo_origin: GeoOrigin,
    /// The real fence (for go-to clamping) and the frame-published view
    /// of it (so the console draws the *real* fence, not a client
    /// fallback).
    pub fence: fleet_safety::geofence::Geofence,
    pub fence_view: GeofenceView,
    /// Wall-clock seconds at run start. Originally populated by the
    /// (removed) run-report header; kept on AppState for forward-compat
    /// (operator-facing summaries, run-dir timestamps).
    #[allow(dead_code)]
    pub started_unix: u64,
    /// This run's directory (event log + any staged operator missions).
    pub run_dir: std::path::PathBuf,
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
        AppState::new_with_catalog_url(
            registry,
            log,
            count,
            scenario_path,
            started_unix,
            geo_origin,
            fence,
            run_dir,
            default_catalog_url(),
        )
    }

    /// M5 (Fleet C2) constructor that takes an explicit `catalog_url`.
    /// Production callers go through `AppState::new` (env-var-driven); tests
    /// use this to point at an ephemeral-port catalog server they've spun
    /// up themselves (the env-var path can't safely serialize across
    /// parallel `#[tokio::test]` threads — process-wide env, per-thread
    /// runtimes).
    pub fn new_with_catalog_url(
        registry: Arc<Registry>,
        log: Arc<EventLog>,
        count: u8,
        scenario_path: impl Into<String>,
        started_unix: u64,
        geo_origin: GeoOrigin,
        fence: fleet_safety::geofence::Geofence,
        run_dir: impl Into<std::path::PathBuf>,
        catalog_url: impl Into<String>,
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
            mission_bindings: Mutex::new(vec![None; count as usize]),
            catalog_url: catalog_url.into(),
            geo_origin,
            fence,
            fence_view,
            started_unix,
            run_dir: run_dir.into(),
        })
    }

    /// Minimal state for the early-error paths of `run_once` (fleet config
    /// unreadable / run dir unwritable): keeps the return shape uniform so
    /// `run_scenario`'s loop can still read (empty) state before the
    /// process exits. Never serves a request.
    pub fn new_for_error() -> Arc<AppState> {
        let registry = Registry::new(0);
        let log = EventLog::in_memory();
        AppState::new(
            registry,
            log,
            0,
            "<none>",
            unix_now(),
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

    /// Operator e-stop request (POST /api/estop). The supervisor latches
    /// it into the run-abort path on its next tick.
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

    /// Per-vehicle "mission live" flag (supervisor writes each tick:
    /// the operator engaged the vehicle via go-to / mission upload —
    /// the guided-command gate).
    pub fn set_mission_active(&self, index: u8, active: bool) {
        if let Some(slot) = self.mission_active.lock().unwrap().get_mut(index as usize) {
            *slot = active;
        }
    }

    /// Is vehicle i currently flying an autonomous mission? Guided
    /// commands (go-to, arm) are gated on this so they never fight a
    /// live offboard setpoint stream (ADR-0017).
    pub fn mission_active(&self, index: u8) -> bool {
        self.mission_active.lock().unwrap().get(index as usize).copied().unwrap_or(false)
    }

    // -- M5 (Fleet C2): per-vehicle mission bindings ----------------------

    /// Bind mission `mission_id` to vehicle `index` (M5, GCS_SPEC.md §5.4).
    /// Silently no-ops when the index is out of range (the REST handler
    /// already returns 404 in that case; this is a defensive backstop).
    pub fn set_mission_binding(&self, index: u8, mission_id: String) {
        if let Some(slot) = self.mission_bindings.lock().unwrap().get_mut(index as usize) {
            *slot = Some(mission_id);
        }
    }

    /// Vehicle i's bound mission ULID, if any (None = unbound). The REST
    /// `GET /api/fleet/mission-bindings` handler emits this array-shaped.
    /// Used directly by the binding tests in api.rs (the routes themselves
    /// read `mission_bindings_snapshot()`).
    #[allow(dead_code)]
    pub fn mission_binding(&self, index: u8) -> Option<String> {
        self.mission_bindings
            .lock()
            .unwrap()
            .get(index as usize)
            .and_then(|s| s.clone())
    }

    /// Clear vehicle i's mission binding (DELETE endpoint).
    /// Returns true if a binding was cleared, false if the vehicle was
    /// already unbound (or out of range).
    pub fn clear_mission_binding(&self, index: u8) -> bool {
        if let Some(slot) = self.mission_bindings.lock().unwrap().get_mut(index as usize) {
            let was = slot.is_some();
            *slot = None;
            was
        } else {
            false
        }
    }

    /// Snapshot of all vehicle mission bindings (one Option per slot).
    /// `POST /api/fleet/start` walks this list to decide which vehicles
    /// to upload + arm.
    pub fn mission_bindings_snapshot(&self) -> Vec<Option<String>> {
        self.mission_bindings.lock().unwrap().clone()
    }

    /// The catalog server's base URL (default `http://127.0.0.1:8300`).
    pub fn catalog_url(&self) -> &str {
        &self.catalog_url
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

// ---------------------------------------------------------------------------
// M5 helpers (Fleet C2, GCS_SPEC.md §5.4)
// ---------------------------------------------------------------------------

/// Default catalog server base URL (M5). The `RSIM_CATALOG_URL` env var
/// overrides this; `mavfleet run` callers typically leave the default
/// (the catalog server runs on the same host at :8300).
pub fn default_catalog_url() -> String {
    std::env::var("RSIM_CATALOG_URL").unwrap_or_else(|_| "http://127.0.0.1:8300".into())
}

/// Validate a ULID (26 chars, Crockford base32). Returns true iff the
/// string looks like a ULID — `POST /api/fleet/mission-bindings` uses
/// this to reject malformed mission ids before storing them.
///
/// Crockford base32 excludes I, L, O, U (to avoid confusion with 1, 1,
/// 0, V) — so the alphabet is `0-9 ABCDEFGH JK MN PQRST VWXYZ`.
pub fn looks_like_ulid(s: &str) -> bool {
    s.len() == 26
        && s.chars().all(|c| {
            matches!(
                c,
                '0'..='9'
                    | 'A'..='H'
                    | 'J'..='K'
                    | 'M'..='N'
                    | 'P'..='T'
                    | 'V'..='Z'
            )
        })
}
