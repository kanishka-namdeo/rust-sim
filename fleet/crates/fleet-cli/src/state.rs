//! Shared application state: the read-side view the REST/WS plane serves and
//! the supervisor writes (spec §3.4, §5). The frame builder here is the one
//! code path behind both `GET /api/fleet` and the WS stream, so the wire
//! contract can't drift between them.

#![forbid(unsafe_code)]

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, RwLock};

use fleet_core::events::{fleet_epoch_ms, Event, EventLog};
use fleet_core::health::HealthFlag;
use fleet_core::registry::Registry;
use fleet_core::tick::{FleetFrame, VehicleView};
use fleet_mavlink::LinkHandle;
use fleet_mission::report::TaskStatus;

/// Per-vehicle task-queue view (written by the supervisor each tick, read by
/// the frame builder — the runner itself is not shareable state).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct VehicleTaskInfo {
    pub current: Option<String>,
    pub queue: Vec<String>,
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
    pub started_unix: u64,
}

impl AppState {
    pub fn new(
        registry: Arc<Registry>,
        log: Arc<EventLog>,
        count: u8,
        scenario_path: impl Into<String>,
        started_unix: u64,
    ) -> Arc<AppState> {
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
            started_unix,
        })
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
        tick_count: s.tick_count.load(Ordering::SeqCst),
        vehicles,
        tasks,
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
