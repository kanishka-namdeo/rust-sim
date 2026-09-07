//! Control and telemetry plane (SPEC §4): axum REST + WebSocket on the
//! api_port, JSON envelope `{"ok", "data", "error"}` on all JSON endpoints.
//!
//! The WebSocket accepts connections at `/ws/telemetry` **and** at `/`
//! (any GET with upgrade headers): the sandbox preview gateway forwards
//! WebSockets as `/?XTransformPort=<port>`, so the router is path-tolerant
//! (ADR-015). A plain GET `/` returns a small JSON landing document.

use std::sync::atomic::{AtomicU8, AtomicU64, Ordering};
use std::sync::{Arc, RwLock};
use std::time::Instant;

use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::extract::{Path, State};
use axum::http::{header, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::{delete, get, post};
use axum::{Json, Router};
use serde::Deserialize;
use serde_json::json;
use sitsim_sdk::ScenarioConfig;
use sitsim_transport::LinkStats;

use crate::run::SimCommand;
use crate::simthread::SimPlane;

/// Run phase (§2.5), as stored in an atomic and serialized as a string.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum Phase {
    Wait = 0,
    Run = 1,
    Draining = 2,
    Done = 3,
}

impl Phase {
    fn from_u8(v: u8) -> &'static str {
        match v {
            0 => "WAIT",
            1 => "RUN",
            2 => "DRAINING",
            _ => "DONE",
        }
    }
}

/// Shared control-plane state.
pub struct AppState {
    pub cfg: RwLock<ScenarioConfig>,
    pub stats: Arc<LinkStats>,
    pub phase: AtomicU8,
    pub plane: tokio::sync::watch::Receiver<SimPlane>,
    pub cmd_tx: tokio::sync::mpsc::UnboundedSender<SimCommand>,
    pub replay_path: std::path::PathBuf,
    pub scenario_hash: [u8; 32],
    pub inject_counter: AtomicU64,
    pub started: Instant,
    pub shut_tx: tokio::sync::watch::Sender<bool>,
}

pub type SharedState = Arc<AppState>;

/// Serve the control plane until the process exits.
pub async fn serve(listener: tokio::net::TcpListener, state: SharedState) {
    let app = Router::new()
        .route("/api/status", get(status))
        .route("/api/scenario", get(get_scenario).put(put_scenario))
        .route("/api/faults", get(get_faults).post(post_fault))
        .route("/api/faults/:id", delete(delete_fault))
        .route("/api/estop", post(estop))
        .route("/api/replay", get(get_replay))
        .route("/ws/telemetry", get(ws_handler))
        .route("/", get(ws_or_landing))
        .with_state(state);
    if let Err(e) = axum::serve(listener, app).await {
        tracing::error!(error = %e, "control plane server ended");
    }
}

// ---------------------------------------------------------------- envelope

fn ok<T: serde::Serialize>(data: T) -> Response {
    (StatusCode::OK, Json(json!({"ok": true, "data": data, "error": null}))).into_response()
}

fn err(status: StatusCode, msg: &str) -> Response {
    (
        status,
        Json(json!({"ok": false, "data": null, "error": msg})),
    )
        .into_response()
}

fn phase_str(state: &AppState) -> String {
    Phase::from_u8(state.phase.load(Ordering::SeqCst)).to_string()
}

// ---------------------------------------------------------------- handlers

/// GET /api/status (§4.1).
async fn status(State(state): State<SharedState>) -> Response {
    let plane = state.plane.borrow().clone();
    let link = state.stats.snapshot();
    let running = state.phase.load(Ordering::SeqCst) == Phase::Run as u8;
    let replay_bytes = std::fs::metadata(&state.replay_path).map(|m| m.len()).unwrap_or(0);
    ok(json!({
        "phase": phase_str(&state),
        "rate_hz": state.cfg.read().map(|c| c.sim.rate_hz).unwrap_or(0.0),
        "t_us": plane.t_us,
        "tick": plane.tick,
        "px4_connected": link.connected,
        "loop_closed": link.loop_closed,
        "peer_port": link.peer_port,
        "uptime_s": state.started.elapsed().as_secs_f64(),
        "tick_us": {
            "p50": plane.tick_p50_us,
            "p95": plane.tick_p95_us,
            "p99": plane.tick_p99_us,
            "p999": plane.tick_p999_us,
            "max": plane.tick_max_us,
        },
        "counters": {
            "sent": {
                "hil_sensor": plane.sent_hil_sensor,
                "hil_state_quaternion": plane.sent_hil_state_quaternion,
                "hil_gps": plane.sent_hil_gps,
                "command_ack": link.sent_command_ack,
                "bytes": link.sent_bytes,
            },
            "recv": {
                "hil_actuator_controls": link.recv_hil_actuator_controls,
                "command_long": link.recv_command_long,
                "unknown_msgid": link.recv_unknown_msgid,
                "bad_crc": link.recv_bad_crc,
                "v1_frames": link.recv_v1,
            }
        },
        "battery_pct": if running { plane.snapshot.battery_pct } else { 100.0 },
        "replay": {
            "path": state.replay_path.display().to_string(),
            "bytes": replay_bytes,
        },
        "scenario_sha256": hex(&state.scenario_hash),
        "telemetry_hash": plane.telemetry_hash.map(|h| format!("{h:016x}")),
    }))
}

/// GET /api/scenario (§4.1).
async fn get_scenario(State(state): State<SharedState>) -> Response {
    match state.cfg.read() {
        Ok(cfg) => ok(cfg.clone()),
        Err(_) => err(StatusCode::INTERNAL_SERVER_ERROR, "config lock poisoned"),
    }
}

/// PUT /api/scenario (§4.1): replacement only in the WAIT phase (409 in
/// RUN, §4.3). The replacement's `[io]` table is accepted but NOT applied
/// (sockets are already bound) — see ADR-016.
async fn put_scenario(State(state): State<SharedState>, body: Option<Json<ScenarioConfig>>) -> Response {
    let Json(cfg_in) = match body {
        Some(b) => b,
        None => return err(StatusCode::BAD_REQUEST, "body must be a scenario JSON object"),
    };
    let phase = state.phase.load(Ordering::SeqCst);
    if phase != Phase::Wait as u8 {
        return err(
            StatusCode::CONFLICT,
            &format!("scenario replacement only allowed in WAIT (currently {})", Phase::from_u8(phase)),
        );
    }
    if let Err(e) = sitsim_sdk::config::validate(&cfg_in) {
        return err(StatusCode::BAD_REQUEST, &e.to_string());
    }
    if cfg_in.io.tcp_port != state.cfg.read().map(|c| c.io.tcp_port).unwrap_or(0)
        || cfg_in.io.api_port != state.cfg.read().map(|c| c.io.api_port).unwrap_or(0)
    {
        tracing::warn!("replacement scenario changes [io] ports; already-bound ports stay in effect");
    }
    match state.cfg.write() {
        Ok(mut w) => {
            *w = cfg_in;
            ok(json!({"replaced": true}))
        }
        Err(_) => err(StatusCode::INTERNAL_SERVER_ERROR, "config lock poisoned"),
    }
}

/// GET /api/faults (§4.1): active fault effects + pending timeline events.
async fn get_faults(State(state): State<SharedState>) -> Response {
    let plane = state.plane.borrow().clone();
    let active: Vec<serde_json::Value> = plane
        .snapshot
        .faults_active
        .iter()
        .map(|(id, t)| json!({"id": id, "type": fault_type_str(t)}))
        .collect();
    let pending: Vec<serde_json::Value> = plane
        .snapshot
        .faults_pending
        .iter()
        .map(|(id, t, start_ms)| json!({"id": id, "type": fault_type_str(t), "start_ms": start_ms}))
        .collect();
    ok(json!({"active": active, "pending": pending}))
}

/// POST /api/faults (§7.3): inject now; `start_ms` in the body is
/// interpreted as "now" (the engine anchors it at the next tick).
async fn post_fault(State(state): State<SharedState>, body: Option<Json<FaultBody>>) -> Response {
    let Json(body) = match body {
        Some(b) => b,
        None => return err(StatusCode::BAD_REQUEST, "body must be a fault event JSON object"),
    };
    if state.phase.load(Ordering::SeqCst) == Phase::Wait as u8 {
        return err(StatusCode::SERVICE_UNAVAILABLE, "sim not running (WAIT phase)");
    }
    let mut spec = body.into_spec();
    if spec.id.is_empty() {
        let n = state.inject_counter.fetch_add(1, Ordering::SeqCst) + 1;
        spec.id = format!("runtime-{n}");
    }
    let id = spec.id.clone();
    let _ = state.cmd_tx.send(SimCommand::InjectFault(spec));
    ok(json!({"id": id, "starts": "next tick"}))
}

/// DELETE /api/faults/{id} (§7.3): clear a persistent fault effect.
async fn delete_fault(State(state): State<SharedState>, Path(id): Path<String>) -> Response {
    if state.phase.load(Ordering::SeqCst) == Phase::Wait as u8 {
        return err(StatusCode::SERVICE_UNAVAILABLE, "sim not running (WAIT phase)");
    }
    let known: Vec<String> = {
        let plane = state.plane.borrow();
        let mut ids: Vec<String> = plane.snapshot.faults_active.iter().map(|(i, _)| i.clone()).collect();
        ids.extend(plane.snapshot.faults_pending.iter().map(|(i, _, _)| i.clone()));
        ids
    };
    if !known.contains(&id) {
        return err(StatusCode::NOT_FOUND, &format!("unknown fault id `{id}`"));
    }
    let _ = state.cmd_tx.send(SimCommand::ClearFault(id));
    ok(json!({"cleared": true}))
}

/// POST /api/estop (§4.1). Also works in WAIT (terminates the process
/// cleanly — useful for harnesses); interpretation recorded in ADR-014.
async fn estop(State(state): State<SharedState>) -> Response {
    let _ = state.cmd_tx.send(SimCommand::EStop);
    // In WAIT there is no sim thread listening: stop the run loop directly.
    if state.phase.load(Ordering::SeqCst) == Phase::Wait as u8 {
        let _ = state.shut_tx.send(true);
    }
    ok(json!({"stopping": true}))
}

/// GET /api/replay (§4.1): the current replay file as raw bytes. This is
/// the one non-JSON endpoint (binary download; ADR-017).
async fn get_replay(State(state): State<SharedState>) -> Response {
    match std::fs::read(&state.replay_path) {
        Ok(bytes) => {
            let mut resp = Response::new(axum::body::Body::from(bytes));
            *resp.status_mut() = StatusCode::OK;
            let headers = resp.headers_mut();
            headers.insert(header::CONTENT_TYPE, header::HeaderValue::from_static("application/octet-stream"));
            if let Ok(v) = header::HeaderValue::from_str(&format!(
                "attachment; filename=\"{}\"",
                state.replay_path.display()
            )) {
                headers.insert(header::CONTENT_DISPOSITION, v);
            }
            resp
        }
        Err(e) => err(StatusCode::NOT_FOUND, &format!("replay not readable: {e}")),
    }
}

// ---------------------------------------------------------------- websocket

/// GET /ws/telemetry.
async fn ws_handler(State(state): State<SharedState>, ws: Option<WebSocketUpgrade>) -> Response {
    match ws {
        Some(w) => w.on_upgrade(move |socket| ws_loop(socket, state)),
        None => err(StatusCode::BAD_REQUEST, "expected a WebSocket upgrade request"),
    }
}

/// GET / (root): WebSocket if upgraded (preview-gateway forwards as
/// `/?XTransformPort=...`), JSON landing otherwise.
async fn ws_or_landing(State(state): State<SharedState>, ws: Option<WebSocketUpgrade>) -> Response {
    match ws {
        Some(w) => w.on_upgrade(move |socket| ws_loop(socket, state)),
        None => ok(json!({
            "service": "rustsitsim",
            "ws": "/ws/telemetry",
            "status": "/api/status",
            "docs": "docs/PROTOCOL.md",
        })),
    }
}

/// Push one JSON frame per 10 Hz snapshot (§4.2 schema, exact).
async fn ws_loop(socket: WebSocket, state: SharedState) {
    let mut plane = state.plane.clone();
    let mut rx = socket;
    loop {
        tokio::select! {
            changed = plane.changed() => {
                if changed.is_err() {
                    break; // sim gone
                }
                let frame = ws_frame(&state, &plane.borrow());
                let text = serde_json::to_string(&frame).unwrap_or_default();
                if rx.send(Message::Text(text)).await.is_err() {
                    break;
                }
            }
            msg = rx.recv() => {
                match msg {
                    None | Some(Ok(Message::Close(_))) => break,
                    Some(Ok(_)) => {} // ping/pong handled by axum
                    Some(Err(_)) => break,
                }
            }
        }
    }
}

/// The §4.2 frame, built from the latest sim plane + link snapshot.
fn ws_frame(state: &AppState, plane: &SimPlane) -> serde_json::Value {
    let link = state.stats.snapshot();
    let snap = &plane.snapshot;
    json!({
        "t_us": plane.t_us,
        "phase": phase_str(state),
        "state": {
            "pos_ned_m": snap.pos_ned_m,
            "vel_ned_ms": snap.vel_ned_ms,
            "q_wxyz": snap.q_wxyz,
            "omega_body_rads": snap.omega_body_rads,
            "motors": snap.motors,
            "battery_pct": snap.battery_pct,
        },
        "sensors": {
            "accel_ms2": snap.sensors.accel_body_ms2,
            "gyro_rads": snap.sensors.gyro_body_rads,
            "baro_alt_m": snap.sensors.baro_alt_m,
            "gps_fix": snap.sensors.gps_fix_type,
            "gps_sat": snap.sensors.gps_sats,
        },
        "faults_active": snap.faults_active.iter()
            .map(|(id, t)| json!({"type": fault_type_str(t), "id": id}))
            .collect::<Vec<_>>(),
        "stats": {
            "tick_p95_us": plane.tick_p95_us,
            "sent": {"hil_sensor": plane.sent_hil_sensor},
            "recv": {"hil_actuator_controls": link.recv_hil_actuator_controls},
        }
    })
}

// ---------------------------------------------------------------- helpers

fn hex(b: &[u8]) -> String {
    b.iter().map(|x| format!("{x:02x}")).collect()
}

fn fault_type_str(t: &sitsim_fault::FaultType) -> &'static str {
    use sitsim_fault::FaultType::*;
    match t {
        MotorEfficiency => "motor_efficiency",
        MotorCut => "motor_cut",
        ImuBiasRamp => "imu_bias_ramp",
        ImuSaturation => "imu_saturation",
        GpsDenial => "gps_denial",
        GpsGlitch => "gps_glitch",
        BaroDrift => "baro_drift",
        WindEvent => "wind_event",
        TransportDelay => "transport_delay",
        PacketDrop => "packet_drop",
    }
}

/// POST /api/faults body (§7.3): the fault event schema; `start_ms` is
/// optional and interpreted as "now".
#[derive(Debug, Deserialize)]
struct FaultBody {
    #[serde(default)]
    id: String,
    #[serde(rename = "type")]
    ftype: sitsim_fault::FaultType,
    #[serde(default)]
    start_ms: Option<u64>,
    #[serde(default)]
    duration_ms: Option<u64>,
    #[serde(default)]
    persistent: bool,
    #[serde(default, flatten)]
    params: sitsim_fault::FaultParams,
}

impl FaultBody {
    fn into_spec(self) -> sitsim_fault::FaultSpec {
        sitsim_fault::FaultSpec {
            id: self.id,
            ftype: self.ftype,
            start_ms: self.start_ms.unwrap_or(0),
            duration_ms: self.duration_ms,
            persistent: self.persistent,
            params: self.params,
        }
    }
}
