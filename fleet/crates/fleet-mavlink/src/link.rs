//! Per-vehicle MAVLink UDP link task.
//!
//! Topology per spec §3.1: the link **binds** 127.0.0.1:14540+i (PX4's rcS
//! streams telemetry to whatever socket holds that port — the pattern proven
//! by `scripts/mavlink_check.py`) and **sends** commands/setpoints to PX4's
//! onboard listen port 127.0.0.1:14580+i from the same socket. Manager
//! addressing is sysid 255 / compid 190 on every link, one channel per
//! vehicle with independent sequence numbers.
//!
//! The task owns three cadences (spec §2.3): decode as-arrives, a 20 Hz
//! setpoint pump, a 1 Hz heartbeat, plus the command client (500 ms retry,
//! three attempts, spec §3.2). Commands are serialised: one COMMAND_LONG in
//! flight at a time; later requests queue behind it.

use std::collections::VecDeque;
use std::net::SocketAddr;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use tokio::net::UdpSocket;
use tokio::sync::{mpsc, oneshot};
use tokio::time::interval;

pub use crate::messages::TelemetryEvent;
use crate::messages::{cmds, enums, ids, CommandLong, Heartbeat, MissionAck, MissionCount, MissionItemInt, MissionRequest, MissionRequestInt, MissionRequestList, Ping, SetPositionTargetLocalNed};
use crate::frame::{crc_extra, Decoder, Frame};
use fleet_modes::{POSITION_ONLY, VELOCITY_YAWRATE};

#[derive(Debug, Clone)]
pub struct LinkConfig {
    /// Vehicle index i (0-based).
    pub instance: u8,
    /// Local bind "127.0.0.1:14540+i".
    pub bind_addr: String,
    /// PX4 onboard listen endpoint "127.0.0.1:14580+i".
    pub remote_addr: String,
    pub manager_sysid: u8,
    pub manager_compid: u8,
    pub heartbeat_ms: u64,
    pub setpoint_ms: u64,
    pub cmd_timeout_ms: u64,
    pub cmd_attempts: u32,
}

impl LinkConfig {
    /// Canonical per-instance config from spec §3.1 / §17.
    pub fn for_instance(instance: u8) -> Self {
        LinkConfig {
            instance,
            bind_addr: format!("127.0.0.1:{}", 14540 + instance as u16),
            remote_addr: format!("127.0.0.1:{}", 14580 + instance as u16),
            manager_sysid: 255,
            manager_compid: 190,
            heartbeat_ms: 1000,
            setpoint_ms: 50,
            cmd_timeout_ms: 500,
            cmd_attempts: 3,
        }
    }
}

/// Live link counters, written by the link task, read by the supervisor.
#[derive(Debug, Clone, Default)]
pub struct LinkStats {
    pub sent: u64,
    pub recv: u64,
    pub dropped_link_loss: u64,
    pub crc_or_unknown_dropped: u64,
    pub retries: u64,
    pub cmd_failures: u64,
    pub commands_sent: u64,
    pub setpoints_sent: u64,
    pub heartbeats_sent: u64,
    /// SET_MESSAGE_INTERVAL subscription requests sent (V-10 evidence).
    pub subscribe_requests_sent: u64,
    /// PARAM_REQUEST_LIST full downloads requested (setup workflow).
    pub param_list_requests_sent: u64,
    /// Total PARAM_VALUE frames ingested into the param store.
    pub param_values_recv: u64,
    /// Message ids observed **before** the subscription requests went out
    /// (the rcS default stream set — V-10's empirical answer).
    pub pre_request_msg_ids: std::collections::BTreeMap<u32, u64>,
    /// Inbound message counts by message id.
    pub msg_counts: std::collections::BTreeMap<u32, u64>,
    /// Average inbound message rate since link start (Hz).
    pub recv_rate_hz: f32,
}

/// The 20 Hz setpoint slot: the mission runner writes the goal, the link's
/// pump steps toward it with a velocity-capped profile and transmits
/// (spec §3.3 / §7.1).
#[derive(Debug, Clone)]
pub struct SetpointSlot {
    pub active: bool,
    pub goal: Option<SetpointGoal>,
    pub current: [f32; 3],
    pub yaw: f32,
    /// Velocity cap for the profile step (m/s, default cruise 4.0).
    pub cruise_ms: f32,
    /// Selects POSITION_ONLY or VELOCITY_YAWRATE semantics.
    pub velocity_mode: bool,
    pub velocity: [f32; 3],
    pub yaw_rate: f32,
}

impl Default for SetpointSlot {
    fn default() -> Self {
        SetpointSlot {
            active: false,
            goal: None,
            current: [0.0, 0.0, 0.0],
            yaw: 0.0,
            cruise_ms: 4.0,
            velocity_mode: false,
            velocity: [0.0, 0.0, 0.0],
            yaw_rate: 0.0,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SetpointGoal {
    pub position: [f32; 3],
    pub yaw: f32,
}

#[derive(Debug)]
pub enum LinkCommand {
    /// COMMAND_LONG with ACK correlation; queued, one in flight at a time.
    SendCommand {
        command: u16,
        params: [f32; 7],
        ack: oneshot::Sender<CmdAck>,
    },
    /// PARAM_SET write, confirmed by the PARAM_VALUE echo (v0.1 uses this
    /// for exactly one parameter, so a single in-flight slot suffices; a
    /// concurrent write replaces the pending one and its caller gets None).
    SetParam {
        param_id: String,
        value: f32,
        /// Wire param_type for the PARAM_SET (typed protocol, ADR-0016);
        /// 6 = INT32 (value = bit pattern), 9 = REAL32.
        param_type: u8,
        reply: oneshot::Sender<Option<ParamVal>>,
    },
    /// PARAM_REQUEST_LIST: the QGC-style full parameter download. Every
    /// PARAM_VALUE frame (solicited or not) lands in the link's param store;
    /// the caller tracks progress by polling the store snapshot.
    RequestParamList {
        ack: oneshot::Sender<bool>,
    },
    /// Upload a set of mission items to PX4 via the MAVLink mission
    /// protocol (MISSION_COUNT → MISSION_REQUEST → MISSION_ITEM_INT →
    /// MISSION_ACK). Three transactions: mission (0), fence (1), rally (2).
    MissionUpload {
        items: Vec<MissionItemInt>,
        mission_type: u8,
        ack: oneshot::Sender<MissionUploadResult>,
    },
    /// Download mission items from PX4 (MISSION_REQUEST_LIST →
    /// MISSION_COUNT → MISSION_REQUEST → MISSION_ITEM_INT → MISSION_ACK).
    MissionDownload {
        mission_type: u8,
        reply: oneshot::Sender<MissionDownloadResult>,
    },
}

/// Result of a mission upload transaction.
///
/// `Serialize` (M2-API): the REST plane returns this verbatim as the
/// `data` field of `POST /api/vehicles/{i}/mission/upload`'s envelope.
/// The `status` tag (`ok` / `failed` / `timeout`) lets the client
/// branch without inspecting the inner fields.
#[derive(Debug, Clone, serde::Serialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum MissionUploadResult {
    Ok {
        mission_type: u8,
        items_sent: u16,
        items_acked: u16,
    },
    Failed {
        mission_type: u8,
        reason: String,
        items_sent: u16,
        items_acked: u16,
    },
    Timeout {
        mission_type: u8,
        items_sent: u16,
        items_acked: u16,
    },
}

/// Result of a mission download transaction.
///
/// `Serialize` (M2-API): the REST plane returns this verbatim as the
/// `data` field of `GET /api/vehicles/{i}/mission`'s envelope.
#[derive(Debug, Clone, serde::Serialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum MissionDownloadResult {
    Ok {
        mission_type: u8,
        items: Vec<MissionItemInt>,
    },
    Failed {
        mission_type: u8,
        reason: String,
    },
    Timeout {
        mission_type: u8,
    },
}

/// Result of a COMMAND_LONG exchange (spec §3.2 contract).
#[derive(Debug, Clone, PartialEq)]
pub enum CmdAck {
    Accepted,
    /// MAV_RESULT_TEMPORARILY_REJECTED / DENIED / UNSUPPORTED / FAILED.
    Rejected { result: u8 },
    /// No COMMAND_ACK after the full retry ladder — supervisor escalation
    /// territory (policy 8).
    Timeout { attempts: u32 },
}

struct Pending {
    command: u16,
    params: [f32; 7],
    ack: Option<oneshot::Sender<CmdAck>>,
    attempts: u32,
    deadline: Instant,
}

struct Queued {
    command: u16,
    params: [f32; 7],
    ack: oneshot::Sender<CmdAck>,
}

struct PendingParam {
    param_id: [u8; 16],
    value: f32,
    /// Wire param_type carried by the PARAM_SET (typed protocol, ADR-0016).
    param_type: u8,
    reply: oneshot::Sender<Option<ParamVal>>,
    attempts: u32,
    deadline: Instant,
}

/// Mission upload state: the link has sent MISSION_COUNT and is waiting
/// for MISSION_REQUESTs from PX4, responding with MISSION_ITEM_INT for
/// each requested seq. The transaction ends with MISSION_ACK.
struct PendingMissionUpload {
    items: Vec<MissionItemInt>,
    mission_type: u8,
    ack: Option<oneshot::Sender<MissionUploadResult>>,
    items_sent: u16,
    items_acked: u16,
    deadline: Instant,
    started: Instant,
}

/// Mission download state: the link has sent MISSION_REQUEST_LIST and is
/// waiting for MISSION_COUNT, then sends MISSION_REQUESTs and collects
/// MISSION_ITEM_INTs. Ends with MISSION_ACK from the GCS.
struct PendingMissionDownload {
    mission_type: u8,
    reply: Option<oneshot::Sender<MissionDownloadResult>>,
    expected_count: u16,
    items: Vec<MissionItemInt>,
    deadline: Instant,
    started: Instant,
}

/// One parameter as last seen on the wire.
#[derive(Debug, Clone, PartialEq)]
pub struct ParamEntry {
    /// Raw wire value — MAVLink carries int params as their **bit pattern**
    /// in the f32 field (mavlink_parameters.cpp memcpy's the int onto the
    /// float), so `value` for an INT32 param is meaningless until decoded
    /// through `typed_value`.
    pub value: f32,
    /// MAVLink param type — PX4 v2 dialect sends the *wire type* constants:
    /// 6 = INT32, 9 = REAL32 (the v2 MAV_PARAM_TYPE_REAL32; **not** the
    /// v1 enum's 7). INT32 bit-casts are the rule (PX4's own code sets
    /// `param_type = MAVLINK_TYPE_INT32_T / MAVLINK_TYPE_FLOAT`).
    pub param_type: u8,
    /// The vehicle's own index for this parameter (download ordering).
    pub param_index: u16,
    /// Wall-clock ms (link start) when this entry was last refreshed.
    pub last_seen_ms: u64,
}

/// A parameter's value in its own type — the typed view the setup plane
/// serves (QGC's parameter editor does exactly this bit-cast).
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum ParamVal {
    Real32(f32),
    Int32(i32),
    /// Unknown/superset types we still cache raw.
    Other(f32),
}

impl ParamVal {
    /// The wire f32 for a PARAM_SET of this value: int params travel as
    /// their bit pattern, floats as themselves.
    pub fn to_wire(self) -> f32 {
        match self {
            ParamVal::Real32(v) | ParamVal::Other(v) => v,
            ParamVal::Int32(v) => f32::from_bits(v as u32),
        }
    }
    /// The MAVLink param_type byte a PARAM_SET of this value must carry
    /// (PX4's receiver rejects mismatches — mavlink_parameters.cpp:129).
    pub fn wire_type(self) -> u8 {
        match self {
            ParamVal::Int32(_) => 6, // MAVLINK_TYPE_INT32_T / MAV_PARAM_TYPE_INT32
            _ => 9,                  // MAVLINK_TYPE_FLOAT / v2 MAV_PARAM_TYPE_REAL32
        }
    }

    pub fn as_f32(self) -> f32 {
        match self {
            ParamVal::Real32(v) | ParamVal::Other(v) => v,
            ParamVal::Int32(v) => v as f32,
        }
    }
}

impl std::fmt::Display for ParamVal {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ParamVal::Int32(v) => write!(f, "{v}"),
            ParamVal::Real32(v) | ParamVal::Other(v) => write!(f, "{v}"),
        }
    }
}

impl ParamEntry {
    /// Decode the wire value into the param's own type: INT32 params are
    /// bit-cast out of the f32 field (the inverse of PX4's memcpy).
    pub fn typed_value(&self) -> ParamVal {
        match self.param_type {
            6 => ParamVal::Int32(self.value.to_bits() as i32),
            9 => ParamVal::Real32(self.value),
            other => ParamVal::Other(self.value), // incl. 0 = unset
        }
    }
}

/// Download progress of the last PARAM_REQUEST_LIST.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ParamDownloadState {
    /// No request issued yet (the store may still hold unsolicited values).
    #[default]
    Idle,
    /// Request sent; PARAM_VALUE stream in flight.
    Downloading,
    /// `received_unique >= total` (both from PARAM_VALUE frames).
    Complete,
    /// A download was started but no PARAM_VALUE arrived for >3 s while
    /// incomplete; re-request heals it (the protocol's own remedy).
    Stalled,
}

/// The per-vehicle parameter store: every decoded PARAM_VALUE lands here
/// (unsolicited echoes included — those are how PARAM_SET writes confirm),
/// keyed by param id, with the last download's progress bookkeeping.
/// This is the SITL/QGC-compatible parameter cache the setup control plane
/// serves from.
#[derive(Debug, Clone, Default)]
pub struct ParamStore {
    pub params: std::collections::BTreeMap<String, ParamEntry>,
    /// Total parameter count as reported by PARAM_VALUE.param_count.
    pub total: u16,
    /// Unique ids received since the last request (<= total when sane).
    pub received_unique: u16,
    pub state: ParamDownloadState,
    /// Link-elapsed ms when PARAM_REQUEST_LIST was last sent.
    pub requested_ms: Option<u64>,
    /// Link-elapsed ms of the last PARAM_VALUE frame.
    pub last_value_ms: Option<u64>,
}

impl ParamStore {
    /// Ingest one decoded PARAM_VALUE into the store.
    pub fn ingest(&mut self, param_id: &str, value: f32, param_type: u8, param_count: u16, param_index: u16, now_ms: u64) {
        if !param_id.is_empty() {
            let n = self.params.len() as u16;
            self.params.entry(param_id.to_string())
                .and_modify(|e| {
                    e.value = value;
                    e.param_type = param_type;
                    e.param_index = param_index;
                    e.last_seen_ms = now_ms;
                })
                .or_insert_with(|| ParamEntry {
                    value,
                    param_type,
                    param_index,
                    last_seen_ms: now_ms,
                });
            if self.params.len() as u16 > n {
                self.received_unique = self.received_unique.saturating_add(1);
            }
        }
        if param_count > 0 {
            self.total = param_count;
        }
        self.last_value_ms = Some(now_ms);
        if self.state == ParamDownloadState::Downloading
            || self.state == ParamDownloadState::Stalled
        {
            // completion: unique ids cover the reported total.
            if self.total > 0 && self.params.len() >= self.total as usize {
                self.state = ParamDownloadState::Complete;
            }
        }
    }

    /// Mark a fresh PARAM_REQUEST_LIST issued at `now_ms`.
    pub fn mark_requested(&mut self, now_ms: u64) {
        self.requested_ms = Some(now_ms);
        self.state = ParamDownloadState::Downloading;
    }

    /// Stalled = downloading + no value for 3 s + incomplete.
    pub fn refresh_staleness(&mut self, now_ms: u64) {
        if self.state == ParamDownloadState::Downloading {
            let stale = self
                .last_value_ms
                .map_or(true, |t| now_ms.saturating_sub(t) > 3_000);
            if stale {
                self.state = ParamDownloadState::Stalled;
            }
        }
    }

    /// A named parameter's current value, if the store has it (raw f32 —
    /// for INT32 params prefer `typed`/`typed_f32`).
    pub fn get(&self, id: &str) -> Option<f32> {
        self.params.get(id).map(|e| e.value)
    }

    /// A named parameter's value decoded through its own type.
    pub fn typed(&self, id: &str) -> Option<ParamVal> {
        self.params.get(id).map(|e| e.typed_value())
    }

    /// A named INT32-typed parameter's integer value (0 when the id is
    /// absent — QGC's CAL-check treats "absent" and "not calibrated"
    /// alike, but `None` vs 0 matters to callers, so they choose).
    pub fn get_i32(&self, id: &str) -> Option<i32> {
        self.params.get(id).and_then(|e| match e.typed_value() {
            ParamVal::Int32(v) => Some(v),
            ParamVal::Real32(v) | ParamVal::Other(v) => Some(v as i32),
        })
    }

    /// The param_type byte the store last saw for this id (write typing).
    pub fn type_of(&self, id: &str) -> Option<u8> {
        self.params.get(id).map(|e| e.param_type)
    }
}

/// Shared state between the link task and the rest of the manager.
pub struct LinkShared {
    pub stats: Mutex<LinkStats>,
    pub slot: Mutex<SetpointSlot>,
    /// When set, inbound datagrams are dropped entirely (scenario
    /// `link_loss` event) while outbound continues.
    pub link_loss_until: Mutex<Option<Instant>>,
    /// Arrival timestamps (staleness inputs aggregated here so the
    /// supervisor does not need to watch the event channel).
    pub last_recv: Mutex<Option<Instant>>,
    pub last_heartbeat_in: Mutex<Option<Instant>>,
    /// The QGC-style parameter cache (ADR-0016).
    pub params: Mutex<ParamStore>,
}

impl LinkShared {
    pub fn new() -> Arc<Self> {
        Arc::new(LinkShared {
            stats: Mutex::new(LinkStats::default()),
            slot: Mutex::new(SetpointSlot::default()),
            link_loss_until: Mutex::new(None),
            last_recv: Mutex::new(None),
            last_heartbeat_in: Mutex::new(None),
            params: Mutex::new(ParamStore::default()),
        })
    }

    pub fn stats(&self) -> LinkStats {
        self.stats.lock().unwrap().clone()
    }

    pub fn set_link_loss(&self, until: Option<Instant>) {
        *self.link_loss_until.lock().unwrap() = until;
    }

    /// Install a position-hold goal at the given NED point (offboard engage
    /// re-anchor, spec §3.3 / §7.2).
    pub fn hold_at(&self, pos: [f32; 3], yaw: f32) {
        let mut s = self.slot.lock().unwrap();
        s.active = true;
        s.velocity_mode = false;
        s.current = pos;
        s.yaw = yaw;
        s.goal = Some(SetpointGoal { position: pos, yaw });
    }

    pub fn set_goal(&self, goal: SetpointGoal) {
        let mut s = self.slot.lock().unwrap();
        s.velocity_mode = false;
        s.goal = Some(goal);
    }

    pub fn set_velocity(&self, v: [f32; 3], yaw_rate: f32) {
        let mut s = self.slot.lock().unwrap();
        s.velocity_mode = true;
        s.velocity = v;
        s.yaw_rate = yaw_rate;
    }

    pub fn stop_stream(&self) {
        self.slot.lock().unwrap().active = false;
    }

    pub fn slot_snapshot(&self) -> SetpointSlot {
        self.slot.lock().unwrap().clone()
    }
}

/// Handle used by the manager to drive one vehicle's link.
#[derive(Clone)]
pub struct LinkHandle {
    pub shared: Arc<LinkShared>,
    cmd_tx: mpsc::Sender<LinkCommand>,
}

impl LinkHandle {
    pub async fn send_command(&self, command: u16, params: [f32; 7]) -> CmdAck {
        let (tx, rx) = oneshot::channel();
        if self
            .cmd_tx
            .send(LinkCommand::SendCommand { command, params, ack: tx })
            .await
            .is_err()
        {
            return CmdAck::Timeout { attempts: 0 };
        }
        // 3 attempts x 500 ms plus slack for queueing behind other commands.
        match tokio::time::timeout(Duration::from_millis(2200), rx).await {
            Ok(Ok(ack)) => ack,
            _ => CmdAck::Timeout { attempts: 3 },
        }
    }

    /// Write one parameter, typed: the PARAM_SET carries the param's own
    /// wire type and (for INT32) the value as its bit pattern — exactly
    /// what PX4's receiver requires (mavlink_parameters.cpp:129 rejects
    /// type mismatches) and what QGC sends. The returned value is the
    /// PARAM_VALUE echo decoded through the same typing; None = no echo
    /// (rejected or lost).
    pub async fn set_param_typed(&self, param_id: &str, val: ParamVal) -> Option<ParamVal> {
        let (tx, rx) = oneshot::channel();
        if self
            .cmd_tx
            .send(LinkCommand::SetParam {
                param_id: param_id.to_string(),
                value: val.to_wire(),
                param_type: val.wire_type(),
                reply: tx,
            })
            .await
            .is_err()
        {
            return None;
        }
        match tokio::time::timeout(Duration::from_millis(2600), rx).await {
            Ok(Ok(v)) => v,
            _ => None,
        }
    }

    /// Legacy float write with store-inferred typing: if the param cache
    /// knows the id's type, the write is typed accordingly (INT32 params
    /// get the value bit-cast); unknown ids go out as REAL32. This keeps
    /// the supervisor's plain-float calls (e.g. NAV_DLL_ACT) PX4-correct.
    pub async fn set_param(&self, param_id: &str, value: f32) -> Option<ParamVal> {
        let val = match self.param_store().type_of(param_id) {
            Some(6) => ParamVal::Int32(value as i32),
            _ => ParamVal::Real32(value),
        };
        self.set_param_typed(param_id, val).await
    }

    /// Request the full parameter dump (PARAM_REQUEST_LIST — the QGC-style
    /// initial download, ADR-0016). The PARAM_VALUE stream lands in the
    /// link's param store; poll `param_store()` for progress. Returns true
    /// when the request was queued and sent.
    pub async fn request_param_list(&self) -> bool {
        let (tx, rx) = oneshot::channel();
        if self
            .cmd_tx
            .send(LinkCommand::RequestParamList { ack: tx })
            .await
            .is_err()
        {
            return false;
        }
        matches!(
            tokio::time::timeout(Duration::from_millis(1500), rx).await,
            Ok(Ok(true)),
        )
    }

    /// Snapshot of the per-vehicle parameter store (QGC-style cache).
    pub fn param_store(&self) -> ParamStore {
        self.shared.params.lock().unwrap().clone()
    }

    /// Upload a set of mission items to PX4 via the MAVLink mission protocol
    /// (MISSION_COUNT → MISSION_REQUEST → MISSION_ITEM_INT → MISSION_ACK).
    /// `mission_type`: 0 = mission, 1 = fence, 2 = rally.
    /// Timeout: 35 s (the link task has a 30 s deadline + 5 s slack).
    pub async fn mission_upload(
        &self,
        items: Vec<MissionItemInt>,
        mission_type: u8,
    ) -> MissionUploadResult {
        let (tx, rx) = oneshot::channel();
        if self
            .cmd_tx
            .send(LinkCommand::MissionUpload {
                items,
                mission_type,
                ack: tx,
            })
            .await
            .is_err()
        {
            return MissionUploadResult::Failed {
                mission_type,
                reason: "link task dropped".into(),
                items_sent: 0,
                items_acked: 0,
            };
        }
        match tokio::time::timeout(Duration::from_secs(35), rx).await {
            Ok(Ok(r)) => r,
            _ => MissionUploadResult::Timeout {
                mission_type,
                items_sent: 0,
                items_acked: 0,
            },
        }
    }

    /// Download mission items from PX4 via the MAVLink mission protocol
    /// (MISSION_REQUEST_LIST → MISSION_COUNT → MISSION_REQUEST →
    /// MISSION_ITEM_INT → MISSION_ACK). `mission_type`: 0/1/2.
    pub async fn mission_download(&self, mission_type: u8) -> MissionDownloadResult {
        let (tx, rx) = oneshot::channel();
        if self
            .cmd_tx
            .send(LinkCommand::MissionDownload {
                mission_type,
                reply: tx,
            })
            .await
            .is_err()
        {
            return MissionDownloadResult::Failed {
                mission_type,
                reason: "link task dropped".into(),
            };
        }
        match tokio::time::timeout(Duration::from_secs(35), rx).await {
            Ok(Ok(r)) => r,
            _ => MissionDownloadResult::Timeout { mission_type },
        }
    }

    /// Convenience wrapper for DO_SET_MODE (spec §3.2).
    ///
    /// PX4's commander decodes VEHICLE_CMD_DO_SET_MODE as SEPARATE params —
    /// `custom_main_mode = (uint8_t)cmd.param2; custom_sub_mode =
    /// (uint8_t)cmd.param3` (Commander.cpp:788-790) — NOT the packed 32-bit
    /// custom-mode word. Sending the packed word truncates to 0 in the uint8
    /// cast and silently no-ops (still ACKed). The mode_word passed here
    /// (and compared against heartbeat echoes, which DO use the packed form)
    /// is decomposed into main/sub params.
    pub async fn set_mode(&self, mode_word: u32) -> CmdAck {
        let main_mode = ((mode_word >> 16) & 0xFF) as u8;
        let sub_mode = ((mode_word >> 24) & 0xFF) as u8;
        self.send_command(
            cmds::DO_SET_MODE,
            [
                enums::MAV_MODE_FLAG_CUSTOM_ENABLED as f32,
                main_mode as f32,
                sub_mode as f32,
                0.0,
                0.0,
                0.0,
                0.0,
            ],
        )
        .await
    }
}

/// Spawn the link task; returns the manager-side handle and the telemetry
/// event receiver.
pub fn spawn_link(cfg: LinkConfig) -> std::io::Result<(LinkHandle, mpsc::UnboundedReceiver<TelemetryEvent>)> {
    let shared = LinkShared::new();
    let (cmd_tx, cmd_rx) = mpsc::channel(64);
    let (ev_tx, ev_rx) = mpsc::unbounded_channel();
    let handle = LinkHandle {
        shared: Arc::clone(&shared),
        cmd_tx,
    };
    let cfg2 = cfg.clone();
    let instance = cfg.instance;
    tokio::spawn(async move {
        if let Err(e) = run(cfg2, shared, cmd_rx, ev_tx).await {
            eprintln!("[fleet-mavlink] link i={instance} terminated: {e}");
        }
    });
    Ok((handle, ev_rx))
}

/// The link task body.
pub async fn run(
    cfg: LinkConfig,
    shared: Arc<LinkShared>,
    mut cmd_rx: mpsc::Receiver<LinkCommand>,
    events: mpsc::UnboundedSender<TelemetryEvent>,
) -> std::io::Result<()> {
    let sock = UdpSocket::bind(&cfg.bind_addr).await?;
    let remote: SocketAddr = cfg.remote_addr.parse().expect("valid remote addr");
    let mut seq: u8 = 0;
    let mut decoder = Decoder::new();
    let mut buf = vec![0u8; 2048];

    let started = Instant::now();
    let mut hb = interval(Duration::from_millis(cfg.heartbeat_ms));
    let mut sp = interval(Duration::from_millis(cfg.setpoint_ms));
    let mut watchdog = interval(Duration::from_millis(1000));
    // Drives the pending-command retry ladder at sub-second cadence so the
    // 500 ms retry contract is not quantised to the 1 s watchdog.
    let mut cmd_timer = interval(Duration::from_millis(200));

    let mut pending: Option<Pending> = None;
    let mut queue: VecDeque<Queued> = VecDeque::new();
    let mut pending_param: Option<PendingParam> = None;
    let mut pending_mission_upload: Option<PendingMissionUpload> = None;
    let mut pending_mission_download: Option<PendingMissionDownload> = None;
    // V-10: telemetry subscription (spec 3.1) sent once, right after the
    // first inbound frame proves PX4's mavlink instance is alive.
    let mut subscribed = false;

    let send_frame = |sock: &UdpSocket, frame: &Frame, seq: u8| -> std::io::Result<()> {
        let bytes = frame.encode_v2(seq, crc_extra(frame.msgid).unwrap_or(0));
        sock.try_send_to(&bytes, remote)?;
        Ok(())
    };

    loop {
        // Pending-command retry ladder (500 ms, three attempts, spec §3.2).
        if let Some(p) = pending.as_mut() {
            if Instant::now() >= p.deadline {
                if p.attempts < cfg.cmd_attempts {
                    p.attempts += 1;
                    p.deadline = Instant::now() + Duration::from_millis(cfg.cmd_timeout_ms);
                    let frame = command_frame(&cfg, p.command, &p.params);
                    send_frame(&sock, &frame, seq)?;
                    seq = seq.wrapping_add(1);
                    let mut st = shared.stats.lock().unwrap();
                    st.retries += 1;
                    st.commands_sent += 1;
                    st.sent += 1;
                } else {
                    let p = pending.take().unwrap();
                    let mut st = shared.stats.lock().unwrap();
                    st.cmd_failures += 1;
                    if let Some(ack) = p.ack {
                        let _ = ack.send(CmdAck::Timeout { attempts: p.attempts });
                    }
                }
            }
        }
        // Pending param write: 600 ms retry, three attempts; confirmed by
        // the PARAM_VALUE echo, not COMMAND_ACK.
        if let Some(pp) = pending_param.as_mut() {
            if Instant::now() >= pp.deadline {
                if pp.attempts < cfg.cmd_attempts {
                    pp.attempts += 1;
                    pp.deadline = Instant::now() + Duration::from_millis(600);
                    let frame = param_set_frame(&cfg, &pp.param_id, pp.value, pp.param_type);
                    send_frame(&sock, &frame, seq)?;
                    seq = seq.wrapping_add(1);
                    shared.stats.lock().unwrap().sent += 1;
                } else {
                    let pp = pending_param.take().unwrap();
                    let _ = pp.reply.send(None);
                }
            }
        }
        // Mission upload timeout (30 s — missions can be large; PX4 may
        // take time to process each item).
        if let Some(mu) = pending_mission_upload.as_mut() {
            if Instant::now() >= mu.deadline {
                let mu = pending_mission_upload.take().unwrap();
                if let Some(tx) = mu.ack {
                    let _ = tx.send(MissionUploadResult::Timeout {
                        mission_type: mu.mission_type,
                        items_sent: mu.items_sent,
                        items_acked: mu.items_acked,
                    });
                }
            }
        }
        // Mission download timeout (30 s).
        if let Some(md) = pending_mission_download.as_mut() {
            if Instant::now() >= md.deadline {
                let md = pending_mission_download.take().unwrap();
                if let Some(tx) = md.reply {
                    let _ = tx.send(MissionDownloadResult::Timeout {
                        mission_type: md.mission_type,
                    });
                }
            }
        }
        // Start the next queued command if the pipe is free.
        if pending.is_none() {
            if let Some(q) = queue.pop_front() {
                let frame = command_frame(&cfg, q.command, &q.params);
                send_frame(&sock, &frame, seq)?;
                seq = seq.wrapping_add(1);
                {
                    let mut st = shared.stats.lock().unwrap();
                    st.commands_sent += 1;
                    st.sent += 1;
                }
                pending = Some(Pending {
                    command: q.command,
                    params: q.params,
                    ack: Some(q.ack),
                    attempts: 1,
                    deadline: Instant::now() + Duration::from_millis(cfg.cmd_timeout_ms),
                });
            }
        }

        tokio::select! {
            r = sock.recv_from(&mut buf) => {
                let (n, _src) = r?;
                // link_loss injection: drop the datagram entirely.
                let loss = *shared.link_loss_until.lock().unwrap();
                if let Some(until) = loss {
                    if Instant::now() < until {
                        shared.stats.lock().unwrap().dropped_link_loss += 1;
                        continue;
                    } else {
                        *shared.link_loss_until.lock().unwrap() = None;
                    }
                }
                decoder.push(&buf[..n]);
                while let Some(frame) = decoder.next_frame() {
                    if !frame.crc_ok {
                        shared.stats.lock().unwrap().crc_or_unknown_dropped += 1;
                    }
                    {
                        let mut st = shared.stats.lock().unwrap();
                        st.recv += 1;
                        *st.msg_counts.entry(frame.msgid).or_insert(0) += 1;
                        *shared.last_recv.lock().unwrap() = Some(Instant::now());
                        if frame.msgid == ids::HEARTBEAT {
                            *shared.last_heartbeat_in.lock().unwrap() = Some(Instant::now());
                        }
                    }
                    // V-10: on first contact from the vehicle, subscribe to
                    // the telemetry set (spec 3.1): ATTITUDE and
                    // LOCAL_POSITION_NED at 20 Hz, the rest at their default
                    // cadence. The pre-request stream set is snapshotted for
                    // the V-10 evidence event.
                    if !subscribed && frame.sysid == cfg.instance + 1 {
                        subscribed = true;
                        {
                            let mut st = shared.stats.lock().unwrap();
                            st.pre_request_msg_ids = st.msg_counts.clone();
                        }
                        for (msgid, interval_us) in SUBSCRIBE_SET {
                            queue.push_back(Queued {
                                command: cmds::SET_MESSAGE_INTERVAL,
                                params: [
                                    msgid as f32,
                                    interval_us,
                                    0.0,
                                    0.0,
                                    0.0,
                                    0.0,
                                    0.0,
                                ],
                                ack: {
                                    // fire-and-forget: dropped receiver.
                                    let (tx, _rx) = oneshot::channel();
                                    tx
                                },
                            });
                            shared.stats.lock().unwrap().subscribe_requests_sent += 1;
                        }
                    }
                    // Command-ACK correlation (before publishing so the
                    // exchange completes even if the aggregator is slow).
                    if frame.msgid == ids::COMMAND_ACK {
                        if let TelemetryEvent::CommandAck { command, result, target_system, target_component, .. } =
                            crate::messages::decode_event(&frame).unwrap()
                        {
                            let _ = target_component;
                            let for_us = target_system == 0 || target_system == cfg.manager_sysid;
                            if for_us {
                                if let Some(p) = pending.take() {
                                    if p.command == command {
                                        let ack = match result {
                                            enums::MAV_RESULT_ACCEPTED => CmdAck::Accepted,
                                            r => CmdAck::Rejected { result: r },
                                        };
                                        if let Some(tx) = p.ack {
                                            let _ = tx.send(ack);
                                        }
                                    } else {
                                        // ACK for a different command (stale
                                        // retry): keep waiting for ours.
                                        pending = Some(p);
                                    }
                                }
                            }
                        }
                    }
                    // Param write confirmation: resolve the pending write
                    // when the echo's id matches (unsolicited echoes restore).
                    // Every PARAM_VALUE (solicited download frame or write
                    // echo) also lands in the link's param store — the
                    // QGC-style cache behind the setup control plane
                    // (ADR-0016).
                    if frame.msgid == ids::PARAM_VALUE {
                        if let TelemetryEvent::ParamValue { param_id, value, param_type, param_count, param_index } =
                            crate::messages::decode_event(&frame).unwrap()
                        {
                            {
                                let now_ms = started.elapsed().as_millis() as u64;
                                let mut store = shared.params.lock().unwrap();
                                store.ingest(&param_id, value, param_type, param_count, param_index, now_ms);
                            }
                            shared.stats.lock().unwrap().param_values_recv += 1;
                            let ours = pending_param.as_ref().map_or(false, |pp| {
                                crate::messages::param_id_to_string(&pp.param_id) == param_id
                            });
                            if ours {
                                let pp = pending_param.take().unwrap();
                                // decode the echo through ITS type (the
                                // store ingest above already updated it)
                                let typed = ParamEntry {
                                    value,
                                    param_type,
                                    param_index,
                                    last_seen_ms: 0,
                                }
                                .typed_value();
                                let _ = pp.reply.send(Some(typed));
                            }
                        }
                    }
                    // Respond to pings (link liveness evidence).
                    if frame.msgid == ids::PING {
                        if let TelemetryEvent::Ping { time_usec, seq: pseq } =
                            crate::messages::decode_event(&frame).unwrap()
                        {
                            let reply = Frame {
                                seq: 0,
                                sysid: cfg.manager_sysid,
                                compid: cfg.manager_compid,
                                msgid: ids::PING,
                                payload: Ping {
                                    time_usec,
                                    seq: pseq,
                                    target_system: frame.sysid,
                                    target_component: frame.compid,
                                }
                                .pack(),
                                crc_ok: true,
                            };
                            send_frame(&sock, &reply, seq)?;
                            seq = seq.wrapping_add(1);
                            shared.stats.lock().unwrap().sent += 1;
                        }
                    }
                    // ---- Mission protocol inbound handling ----
                    // MISSION_REQUEST / MISSION_REQUEST_INT: PX4 wants item at seq
                    if frame.msgid == ids::MISSION_REQUEST || frame.msgid == ids::MISSION_REQUEST_INT {
                        let req = if frame.msgid == ids::MISSION_REQUEST {
                            MissionRequest::unpack(&frame.payload)
                        } else {
                            MissionRequestInt::unpack(&frame.payload).map(|r| MissionRequest {
                                seq: r.seq,
                                target_system: r.target_system,
                                target_component: r.target_component,
                                mission_type: r.mission_type,
                            })
                        };
                        if let Some(req) = req {
                            if let Some(mu) = pending_mission_upload.as_mut() {
                                if req.mission_type == mu.mission_type {
                                    if let Some(item) = mu.items.get(req.seq as usize) {
                                        let mut item = item.clone();
                                        item.target_system = cfg.instance + 1;
                                        item.target_component = 1;
                                        item.seq = req.seq;
                                        item.mission_type = mu.mission_type;
                                        let reply = Frame {
                                            seq: 0,
                                            sysid: cfg.manager_sysid,
                                            compid: cfg.manager_compid,
                                            msgid: ids::MISSION_ITEM_INT,
                                            payload: item.pack(),
                                            crc_ok: true,
                                        };
                                        send_frame(&sock, &reply, seq)?;
                                        seq = seq.wrapping_add(1);
                                        shared.stats.lock().unwrap().sent += 1;
                                        mu.items_sent = mu.items_sent.max(req.seq + 1);
                                        mu.deadline = Instant::now() + Duration::from_secs(30);
                                    }
                                }
                            }
                        }
                    }
                    // MISSION_COUNT: PX4 announces how many items it has (download)
                    if frame.msgid == ids::MISSION_COUNT {
                        if let Some(mc) = MissionCount::unpack(&frame.payload) {
                            if let Some(md) = pending_mission_download.as_mut() {
                                if mc.mission_type == md.mission_type {
                                    md.expected_count = mc.count;
                                    md.items.clear();
                                    md.items.reserve(mc.count as usize);
                                    // Request the first item
                                    let reply = Frame {
                                        seq: 0,
                                        sysid: cfg.manager_sysid,
                                        compid: cfg.manager_compid,
                                        msgid: ids::MISSION_REQUEST,
                                        payload: MissionRequest {
                                            seq: 0,
                                            target_system: cfg.instance + 1,
                                            target_component: 1,
                                            mission_type: md.mission_type,
                                        }.pack(),
                                        crc_ok: true,
                                    };
                                    send_frame(&sock, &reply, seq)?;
                                    seq = seq.wrapping_add(1);
                                    shared.stats.lock().unwrap().sent += 1;
                                    md.deadline = Instant::now() + Duration::from_secs(30);
                                }
                            }
                        }
                    }
                    // MISSION_ITEM_INT: PX4 sends an item during download
                    if frame.msgid == ids::MISSION_ITEM_INT {
                        if let Some(item) = MissionItemInt::unpack(&frame.payload) {
                            let mut download_done = false;
                            let mut download_items: Vec<MissionItemInt> = Vec::new();
                            let mut download_type = 0u8;
                            if let Some(md) = pending_mission_download.as_mut() {
                                if item.mission_type == md.mission_type {
                                    md.items.push(item.clone());
                                    download_type = md.mission_type;
                                    let next_seq = (item.seq + 1) as u16;
                                    if next_seq < md.expected_count {
                                        // Request next item
                                        let reply = Frame {
                                            seq: 0,
                                            sysid: cfg.manager_sysid,
                                            compid: cfg.manager_compid,
                                            msgid: ids::MISSION_REQUEST,
                                            payload: MissionRequest {
                                                seq: next_seq,
                                                target_system: cfg.instance + 1,
                                                target_component: 1,
                                                mission_type: md.mission_type,
                                            }.pack(),
                                            crc_ok: true,
                                        };
                                        send_frame(&sock, &reply, seq)?;
                                        seq = seq.wrapping_add(1);
                                        shared.stats.lock().unwrap().sent += 1;
                                    } else {
                                        // All items received → send MISSION_ACK
                                        let reply = Frame {
                                            seq: 0,
                                            sysid: cfg.manager_sysid,
                                            compid: cfg.manager_compid,
                                            msgid: ids::MISSION_ACK,
                                            payload: MissionAck {
                                                target_system: cfg.instance + 1,
                                                target_component: 1,
                                                mission_result: enums::MAV_MISSION_ACCEPTED,
                                                mission_type: md.mission_type,
                                            }.pack(),
                                            crc_ok: true,
                                        };
                                        send_frame(&sock, &reply, seq)?;
                                        seq = seq.wrapping_add(1);
                                        shared.stats.lock().unwrap().sent += 1;
                                        download_done = true;
                                        download_items = md.items.clone();
                                    }
                                    md.deadline = Instant::now() + Duration::from_secs(30);
                                }
                            }
                            if download_done {
                                let md = pending_mission_download.take().unwrap();
                                if let Some(tx) = md.reply {
                                    let _ = tx.send(MissionDownloadResult::Ok {
                                        mission_type: download_type,
                                        items: download_items,
                                    });
                                }
                            }
                        }
                    }
                    // MISSION_ACK: PX4 acknowledges upload completion
                    if frame.msgid == ids::MISSION_ACK {
                        if let Some(ack) = MissionAck::unpack(&frame.payload) {
                            if let Some(mu) = pending_mission_upload.as_mut() {
                                if ack.mission_type == mu.mission_type {
                                    mu.items_acked = mu.items_sent;
                                    let mu = pending_mission_upload.take().unwrap();
                                    if let Some(tx) = mu.ack {
                                        if ack.mission_result == enums::MAV_MISSION_ACCEPTED {
                                            let _ = tx.send(MissionUploadResult::Ok {
                                                mission_type: mu.mission_type,
                                                items_sent: mu.items_sent,
                                                items_acked: mu.items_acked,
                                            });
                                        } else {
                                            let _ = tx.send(MissionUploadResult::Failed {
                                                mission_type: mu.mission_type,
                                                reason: format!("PX4 rejected mission (result={})", ack.mission_result),
                                                items_sent: mu.items_sent,
                                                items_acked: mu.items_acked,
                                            });
                                        }
                                    }
                                }
                            }
                        }
                    }
                    let ev = crate::messages::decode_event(&frame)
                        .unwrap_or(TelemetryEvent::Unknown { msgid: frame.msgid, len: frame.payload.len() });
                    let _ = events.send(ev);
                }
            }
            cmd = cmd_rx.recv() => {
                match cmd {
                    None => return Ok(()), // manager dropped the link
                    Some(LinkCommand::SendCommand { command, params, ack }) => {
                        queue.push_back(Queued { command, params, ack });
                    }
                    Some(LinkCommand::SetParam { param_id, value, param_type, reply }) => {
                        let mut id = [0u8; 16];
                        for (i, b) in param_id.as_bytes().iter().take(15).enumerate() {
                            id[i] = *b;
                        }
                        let frame = param_set_frame(&cfg, &id, value, param_type);
                        send_frame(&sock, &frame, seq)?;
                        seq = seq.wrapping_add(1);
                        shared.stats.lock().unwrap().sent += 1;
                        pending_param = Some(PendingParam {
                            param_id: id,
                            value,
                            param_type,
                            reply,
                            attempts: 1,
                            deadline: Instant::now() + Duration::from_millis(600),
                        });
                    }
                    Some(LinkCommand::RequestParamList { ack }) => {
                        // QGC-style full parameter download (ADR-0016):
                        // fire-and-track — progress lands in the param store
                        // as PARAM_VALUE frames arrive.
                        let frame = Frame {
                            seq: 0,
                            sysid: cfg.manager_sysid,
                            compid: cfg.manager_compid,
                            msgid: ids::PARAM_REQUEST_LIST,
                            payload: crate::messages::ParamRequestList {
                                target_system: cfg.instance + 1,
                                target_component: 1,
                            }
                            .pack(),
                            crc_ok: true,
                        };
                        send_frame(&sock, &frame, seq)?;
                        seq = seq.wrapping_add(1);
                        {
                            let mut st = shared.stats.lock().unwrap();
                            st.sent += 1;
                            st.param_list_requests_sent += 1;
                        }
                        shared
                            .params
                            .lock()
                            .unwrap()
                            .mark_requested(started.elapsed().as_millis() as u64);
                        let _ = ack.send(true);
                    }
                    Some(LinkCommand::MissionUpload { items, mission_type, ack }) => {
                        // Send MISSION_COUNT to PX4; the link task will
                        // respond to MISSION_REQUESTs with MISSION_ITEM_INTs
                        // and resolve `ack` when MISSION_ACK arrives.
                        let count = items.len() as u16;
                        let frame = Frame {
                            seq: 0,
                            sysid: cfg.manager_sysid,
                            compid: cfg.manager_compid,
                            msgid: ids::MISSION_COUNT,
                            payload: MissionCount {
                                count,
                                target_system: cfg.instance + 1,
                                target_component: 1,
                                mission_type,
                            }.pack(),
                            crc_ok: true,
                        };
                        send_frame(&sock, &frame, seq)?;
                        seq = seq.wrapping_add(1);
                        shared.stats.lock().unwrap().sent += 1;
                        pending_mission_upload = Some(PendingMissionUpload {
                            items,
                            mission_type,
                            ack: Some(ack),
                            items_sent: 0,
                            items_acked: 0,
                            deadline: Instant::now() + Duration::from_secs(30),
                            started: Instant::now(),
                        });
                    }
                    Some(LinkCommand::MissionDownload { mission_type, reply }) => {
                        // Send MISSION_REQUEST_LIST to PX4; the link task
                        // will collect MISSION_ITEM_INTs and resolve `reply`
                        // when all items are received.
                        let frame = Frame {
                            seq: 0,
                            sysid: cfg.manager_sysid,
                            compid: cfg.manager_compid,
                            msgid: ids::MISSION_REQUEST_LIST,
                            payload: MissionRequestList {
                                target_system: cfg.instance + 1,
                                target_component: 1,
                                mission_type,
                            }.pack(),
                            crc_ok: true,
                        };
                        send_frame(&sock, &frame, seq)?;
                        seq = seq.wrapping_add(1);
                        shared.stats.lock().unwrap().sent += 1;
                        pending_mission_download = Some(PendingMissionDownload {
                            mission_type,
                            reply: Some(reply),
                            expected_count: 0,
                            items: Vec::new(),
                            deadline: Instant::now() + Duration::from_secs(30),
                            started: Instant::now(),
                        });
                    }
                }
            }
            _ = hb.tick() => {
                let frame = Frame {
                    seq: 0,
                    sysid: cfg.manager_sysid,
                    compid: cfg.manager_compid,
                    msgid: ids::HEARTBEAT,
                    payload: Heartbeat {
                        custom_mode: 0,
                        mavtype: enums::MAV_TYPE_GCS,
                        autopilot: enums::MAV_AUTOPILOT_INVALID,
                        base_mode: enums::MAV_MODE_FLAG_CUSTOM_ENABLED,
                        system_status: enums::MAV_STATE_ACTIVE,
                        mavlink_version: 3,
                    }
                    .pack(),
                    crc_ok: true,
                };
                send_frame(&sock, &frame, seq)?;
                seq = seq.wrapping_add(1);
                let mut st = shared.stats.lock().unwrap();
                st.sent += 1;
                st.heartbeats_sent += 1;
            }
            _ = sp.tick() => {
                let slot = {
                    let mut s = shared.slot.lock().unwrap();
                    if !s.active {
                        continue;
                    }
                    if !s.velocity_mode {
                        if let Some(goal) = s.goal {
                            // Velocity-capped profile step (spec §7.1):
                            // saturate displacement per 20 Hz step.
                            let max_step = s.cruise_ms * (cfg.setpoint_ms as f32 / 1000.0);
                            for i in 0..3 {
                                let d = goal.position[i] - s.current[i];
                                let step = d.clamp(-max_step, max_step);
                                s.current[i] += step;
                            }
                            s.yaw = goal.yaw;
                        }
                    }
                    s.clone()
                };
                let msg = if slot.velocity_mode {
                    SetPositionTargetLocalNed {
                        time_boot_ms: started.elapsed().as_millis() as u32,
                        target_system: cfg.instance + 1,
                        target_component: 1,
                        coordinate_frame: enums::MAV_FRAME_LOCAL_NED,
                        type_mask: VELOCITY_YAWRATE,
                        vx: slot.velocity[0],
                        vy: slot.velocity[1],
                        vz: slot.velocity[2],
                        yaw_rate: slot.yaw_rate,
                        ..Default::default()
                    }
                } else {
                    SetPositionTargetLocalNed {
                        time_boot_ms: started.elapsed().as_millis() as u32,
                        target_system: cfg.instance + 1,
                        target_component: 1,
                        coordinate_frame: enums::MAV_FRAME_LOCAL_NED,
                        type_mask: POSITION_ONLY,
                        x: slot.current[0],
                        y: slot.current[1],
                        z: slot.current[2],
                        yaw: slot.yaw,
                        ..Default::default()
                    }
                };
                let frame = Frame {
                    seq: 0,
                    sysid: cfg.manager_sysid,
                    compid: cfg.manager_compid,
                    msgid: ids::SET_POSITION_TARGET_LOCAL_NED,
                    payload: msg.pack(),
                    crc_ok: true,
                };
                send_frame(&sock, &frame, seq)?;
                seq = seq.wrapping_add(1);
                let mut st = shared.stats.lock().unwrap();
                st.sent += 1;
                st.setpoints_sent += 1;
            }
            _ = cmd_timer.tick() => {
                // wake the loop: retry/timeout logic at the top runs now.
            }
            _ = watchdog.tick() => {
                let mut st = shared.stats.lock().unwrap();
                let el = started.elapsed().as_secs_f32().max(0.001);
                st.recv_rate_hz = st.recv as f32 / el;
                drop(st);
                // param download staleness (3 s without a value while
                // incomplete -> Stalled; the API reports it and a
                // re-request heals it, the protocol's own remedy)
                shared
                    .params
                    .lock()
                    .unwrap()
                    .refresh_staleness(started.elapsed().as_millis() as u64);
            }
        }
    }
}

fn command_frame(cfg: &LinkConfig, command: u16, params: &[f32; 7]) -> Frame {
    Frame {
        seq: 0,
        sysid: cfg.manager_sysid,
        compid: cfg.manager_compid,
        msgid: ids::COMMAND_LONG,
        payload: CommandLong {
            target_system: cfg.instance + 1,
            target_component: 1,
            command,
            confirmation: 0,
            params: *params,
        }
        .pack(),
        crc_ok: true,
    }
}

fn param_set_frame(cfg: &LinkConfig, param_id: &[u8; 16], value: f32, param_type: u8) -> Frame {
    let mut ps = crate::messages::ParamSet::from_str("", value, cfg.instance + 1, 1);
    ps.param_id = *param_id;
    ps.param_type = param_type;
    Frame {
        seq: 0,
        sysid: cfg.manager_sysid,
        compid: cfg.manager_compid,
        msgid: ids::PARAM_SET,
        payload: ps.pack(),
        crc_ok: true,
    }
}

/// The telemetry subscription set (spec 3.1): (message id, interval us).
/// 20 Hz for the estimator-grade streams, `0` = request the default rate
/// for the rest. STATUSTEXT is event-driven but subscription is conformant.
pub const SUBSCRIBE_SET: [(u32, f32); 7] = [
    (ids::ATTITUDE, 50_000.0),
    (ids::LOCAL_POSITION_NED, 50_000.0),
    (ids::SYS_STATUS, 0.0),
    (ids::GLOBAL_POSITION_INT, 0.0),
    (ids::BATTERY_STATUS, 0.0),
    (ids::HOME_POSITION, 0.0),
    (ids::STATUSTEXT, 0.0),
];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn link_config_ports_per_spec() {
        let c = LinkConfig::for_instance(0);
        assert_eq!(c.bind_addr, "127.0.0.1:14540");
        assert_eq!(c.remote_addr, "127.0.0.1:14580");
        assert_eq!(c.manager_sysid, 255);
        assert_eq!(c.manager_compid, 190);
        let c1 = LinkConfig::for_instance(1);
        assert_eq!(c1.bind_addr, "127.0.0.1:14541");
        assert_eq!(c1.remote_addr, "127.0.0.1:14581");
    }

    #[test]
    fn setpoint_slot_velocity_cap() {
        // 20 Hz, cruise 4 m/s -> max step 0.2 m
        let mut slot = SetpointSlot {
            active: true,
            cruise_ms: 4.0,
            ..Default::default()
        };
        slot.goal = Some(SetpointGoal { position: [100.0, 0.0, -10.0], yaw: 0.0 });
        let max_step = slot.cruise_ms * (50.0 / 1000.0);
        for i in 0..3 {
            let d = slot.goal.unwrap().position[i] - slot.current[i];
            let step = d.clamp(-max_step, max_step);
            slot.current[i] += step;
        }
        assert_eq!(slot.current, [0.2, 0.0, -0.2]);
    }

    #[tokio::test]
    async fn hold_at_anchors_slot() {
        let shared = LinkShared::new();
        shared.hold_at([1.0, 2.0, -3.0], 0.5);
        let s = shared.slot_snapshot();
        assert!(s.active);
        assert_eq!(s.current, [1.0, 2.0, -3.0]);
        assert_eq!(s.goal.unwrap().position, [1.0, 2.0, -3.0]);
        shared.stop_stream();
        assert!(!shared.slot_snapshot().active);
    }

    /// Param store ingest: unique ids advance received_unique, re-values
    /// do not, total tracks param_count, completion flips at coverage.
    #[test]
    fn param_store_ingest_tracks_download() {
        let mut s = ParamStore::default();
        assert_eq!(s.state, ParamDownloadState::Idle);
        s.mark_requested(10);
        assert_eq!(s.state, ParamDownloadState::Downloading);
        s.ingest("SYS_AUTOSTART", 4001.0, 9, 3, 0, 12);
        s.ingest("BAT_N_CELLS", 4.0, 9, 3, 1, 13);
        assert_eq!(s.total, 3);
        assert_eq!(s.received_unique, 2);
        assert_eq!(s.state, ParamDownloadState::Downloading);
        // a write echo re-delivers an existing id: no double count
        s.ingest("BAT_N_CELLS", 4.0, 9, 3, 1, 20);
        assert_eq!(s.received_unique, 2);
        assert_eq!(s.params.len(), 2);
        s.ingest("NAV_DLL_ACT", 0.0, 9, 3, 2, 21);
        assert_eq!(s.state, ParamDownloadState::Complete);
        assert_eq!(s.get("BAT_N_CELLS"), Some(4.0));
        assert_eq!(s.get("NOPE"), None);
    }

    /// Stalled: downloading with no PARAM_VALUE for >3 s and incomplete.
    #[test]
    fn param_store_staleness_flips_to_stalled() {
        let mut s = ParamStore::default();
        s.mark_requested(0);
        s.ingest("A", 1.0, 9, 100, 0, 100);
        s.refresh_staleness(1_000); // 900 ms since last value: still fine
        assert_eq!(s.state, ParamDownloadState::Downloading);
        s.refresh_staleness(4_500); // 4.4 s since last value: stalled
        assert_eq!(s.state, ParamDownloadState::Stalled);
        // a late value un-stalls (ingest accepts Stalled state too)
        s.ingest("B", 2.0, 9, 100, 1, 4_600);
        assert_eq!(s.state, ParamDownloadState::Stalled);
        // and a re-request resets to Downloading
        s.mark_requested(4_700);
        assert_eq!(s.state, ParamDownloadState::Downloading);
    }

    /// Empty param ids (PX4's unknown-param reply shape) must not create
    /// entries; their count/index bookkeeping still lands.
    #[test]
    fn param_store_ignores_empty_ids() {
        let mut s = ParamStore::default();
        s.mark_requested(0);
        s.ingest("", 0.0, 9, 5, 0, 10);
        assert_eq!(s.params.len(), 0);
        assert_eq!(s.received_unique, 0);
        assert_eq!(s.total, 5);
        assert_eq!(s.last_value_ms, Some(10));
    }
}
