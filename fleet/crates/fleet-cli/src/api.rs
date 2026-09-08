//! REST + WS control plane (spec §3.4) on 127.0.0.1:8400.
//!
//! Envelope: `{"ok": bool, "data": ..., "error": ...}` — identical to
//! rustsitsim's spec §4.1 shape so dashboard client code is shared.
//!
//! WS path tolerance: the sandbox preview gateway forwards WebSockets as
//! `/?XTransformPort=8400` (the port rides in the *query*, path is `/`), so
//! the WS handler is registered on `/ws/fleet`, `/ws` **and** `/`. The root
//! also answers plain GET with a JSON index (non-upgrade requests).
//!
//! Vehicle-setup plane (ADR-0016): the QGroundControl / Mission-Planner-
//! style endpoints — `/api/airframes`, `/api/modes`, and per-vehicle
//! `setup`, `params`, `airframe`, `calibrate`, `mode` — served from the
//! live per-vehicle param cache. Writes go through the same MAVLink
//! param-protocol (PARAM_SET + PARAM_VALUE echo) and command protocol
//! (COMMAND_LONG + ACK) the supervisor itself uses; nothing is
//! client-side authored.

#![forbid(unsafe_code)]

use std::sync::Arc;
use std::time::Duration;

use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::extract::{FromRequestParts, Path, Query, State};
use axum::http::request::Parts;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Json, Response};
use axum::routing::{get, post};
use axum::Router;
use serde_json::json;

use crate::setup;
use crate::state::{self, AppState};

/// Build the control-plane router (also the unit-test surface).
pub fn router(state: Arc<AppState>) -> Router {
    Router::new()
        .route("/api/fleet", get(fleet_get))
        .route("/api/fleet/estop", post(estop_post))
        .route("/api/estop", post(estop_post))
        .route("/api/vehicles/{index}", get(vehicle_get))
        .route("/api/events", get(events_get))
        .route("/api/tasks", get(tasks_get))
        // -- vehicle-setup plane (ADR-0016, QGC/MP-style) ----------------
        .route("/api/airframes", get(airframes_get))
        .route("/api/modes", get(modes_get))
        .route("/api/vehicles/{index}/setup", get(vehicle_setup_get))
        .route("/api/vehicles/{index}/params", get(vehicle_params_get).post(vehicle_params_post))
        .route("/api/vehicles/{index}/params/refresh", post(vehicle_params_refresh_post))
        .route("/api/vehicles/{index}/airframe", post(vehicle_airframe_post))
        .route("/api/vehicles/{index}/calibrate", post(vehicle_calibrate_post))
        .route("/api/vehicles/{index}/mode", post(vehicle_mode_post))
        // WS plane: the spec path plus the gateway-forwarded root path.
        .route("/ws/fleet", get(ws_entry))
        .route("/ws", get(ws_entry))
        .route("/", get(ws_entry))
        .with_state(state)
}

/// Bind + serve the control plane until the process exits.
pub async fn serve(state: Arc<AppState>, port: u16) -> std::io::Result<()> {
    let app = router(state);
    let listener = tokio::net::TcpListener::bind(("127.0.0.1", port)).await?;
    println!("[fleet] control plane: http://127.0.0.1:{port}/api/fleet (WS: /ws/fleet, /ws, /)");
    axum::serve(listener, app).await
}

// ---------------------------------------------------------------------------
// Envelope
// ---------------------------------------------------------------------------

fn ok(data: serde_json::Value) -> Json<serde_json::Value> {
    Json(json!({ "ok": true, "data": data }))
}

fn err(status: StatusCode, message: &str) -> Response {
    let body = Json(json!({ "ok": false, "error": message }));
    (status, body).into_response()
}

// ---------------------------------------------------------------------------
// REST handlers
// ---------------------------------------------------------------------------

/// `GET /api/fleet` — fleet summary (spec §3.4): per-vehicle FSM state,
/// health, battery, position, plus fleet phase and the events tail.
async fn fleet_get(State(s): State<Arc<AppState>>) -> Json<serde_json::Value> {
    let frame = state::fleet_frame(&s, 64);
    let data = serde_json::to_value(&frame).unwrap_or(json!({}));
    ok(data)
}

/// `GET /api/vehicles/{i}` — full vehicle detail incl. snapshot ages and
/// link counters.
async fn vehicle_get(
    State(s): State<Arc<AppState>>,
    Path(index): Path<u8>,
) -> Response {
    match state::vehicle_detail(&s, index) {
        Some(v) => ok(v).into_response(),
        None => err(
            StatusCode::NOT_FOUND,
            &format!("vehicle index {index} out of range (fleet count {})", s.count),
        ),
    }
}

#[derive(serde::Deserialize, Default)]
struct EventsQuery {
    tail: Option<usize>,
}

/// `GET /api/events` — event-log tail (default 200).
async fn events_get(
    State(s): State<Arc<AppState>>,
    Query(q): Query<EventsQuery>,
) -> Json<serde_json::Value> {
    let n = q.tail.unwrap_or(200).min(10_000);
    let events = state::events_tail(&s, n);
    ok(json!({
        "total": s.log.total(),
        "returned": events.len(),
        "events": events,
    }))
}

/// `GET /api/tasks` — task table (assignment, state, hover observation).
async fn tasks_get(State(s): State<Arc<AppState>>) -> Json<serde_json::Value> {
    ok(json!({ "tasks": s.tasks_snapshot() }))
}

/// `POST /api/estop` (and `/api/fleet/estop`) — operator e-stop: every
/// vehicle LANDs immediately on the supervisor's next tick, run ABORTED.
async fn estop_post(State(s): State<Arc<AppState>>) -> Json<serde_json::Value> {
    s.request_estop();
    s.log.log(
        fleet_core::events::EventKind::SupervisorAction,
        None,
        "operator e-stop requested via API (policy 1)",
    );
    ok(json!({ "estop": true, "phase": s.phase() }))
}

// ---------------------------------------------------------------------------
// Vehicle-setup plane (ADR-0016) — the QGC / Mission-Planner configuration
// workflow: airframe selection, parameter view + edit, sensor calibration
// triggers, power/safety config, and flight-mode switching.
// ---------------------------------------------------------------------------

/// `GET /api/airframes` — the PX4 airframe catalog (ROMFS-derived),
/// grouped QGC-browser style (Multirotor/Plane/VTOL/Rover/Boat/…).
async fn airframes_get() -> Json<serde_json::Value> {
    ok(setup::airframes_json())
}

/// `GET /api/modes` — the switchable flight-mode set (name + mode word).
async fn modes_get() -> Json<serde_json::Value> {
    let modes: Vec<serde_json::Value> = setup::switchable_modes()
        .into_iter()
        .map(|(name, word)| json!({ "name": name, "mode_word": word }))
        .collect();
    ok(json!({ "modes": modes }))
}

/// `GET /api/vehicles/{i}/setup` — QGC's setup Summary for vehicle i:
/// airframe (SYS_AUTOSTART resolved), calibration flags (CAL_*_ID),
/// power (BAT_*), safety (NAV_RCL_ACT & friends), param-download state.
async fn vehicle_setup_get(
    State(s): State<Arc<AppState>>,
    Path(index): Path<u8>,
) -> Response {
    match setup::build_setup_summary(&s, index) {
        Some(v) => ok(v).into_response(),
        None => err(
            StatusCode::NOT_FOUND,
            &format!("vehicle index {index} out of range (fleet count {})", s.count),
        ),
    }
}

/// `GET /api/vehicles/{i}/params` — the live parameter cache with
/// download progress. 404 when the index is out of range; a link-less
/// vehicle reports an empty store (the console shows "no link yet").
async fn vehicle_params_get(
    State(s): State<Arc<AppState>>,
    Path(index): Path<u8>,
) -> Response {
    if index >= s.count {
        return err(
            StatusCode::NOT_FOUND,
            &format!("vehicle index {index} out of range (fleet count {})", s.count),
        );
    }
    let store = s.link(index).map(|h| h.param_store());
    let data = match store {
        Some(st) => setup::build_params_json(&st),
        None => json!({
            "total": 0, "received": 0, "state": "NoLink",
            "requested_ms": null, "last_value_ms": null, "params": [],
        }),
    };
    ok(data).into_response()
}

#[derive(serde::Deserialize)]
struct ParamWriteBody {
    id: String,
    value: f64,
    /// Optional explicit wire type (6 = INT32, 9 = REAL32). When absent
    /// the param's cached type decides — QGC's editor edits the value in
    /// the param's own type, and PX4 rejects type mismatches.
    #[serde(default)]
    param_type: Option<u8>,
}

/// `POST /api/vehicles/{i}/params` — write one parameter (QGC's
/// parameter editor "Write" button): typed PARAM_SET (the param's own
/// wire type, INT32 values bit-cast — what PX4's receiver requires) +
/// PARAM_VALUE echo confirmation. PX4 persists the change (autosave) so
/// it survives the next boot.
async fn vehicle_params_post(
    State(s): State<Arc<AppState>>,
    Path(index): Path<u8>,
    body: axum::extract::Json<ParamWriteBody>,
) -> Response {
    if index >= s.count {
        return err(
            StatusCode::NOT_FOUND,
            &format!("vehicle index {index} out of range (fleet count {})", s.count),
        );
    }
    let Some(link) = s.link(index) else {
        return err(StatusCode::CONFLICT, "vehicle link not registered (spawn failed?)");
    };
    let id = body.id.trim().to_string();
    if id.is_empty() || id.len() > 16 {
        return err(StatusCode::UNPROCESSABLE_ENTITY, "param id must be 1..=16 chars");
    }
    // typing: explicit body type > cached type > REAL32 (the typed
    // protocol, ADR-0016 — NAV_DLL_ACT-style INT32 writes need it)
    let store = link.param_store();
    let wire_type = body.param_type.or(store.type_of(&id)).unwrap_or(9);
    let val = match wire_type {
        6 => {
            let v = body.value as i64;
            if v < i32::MIN as i64 || v > i32::MAX as i64 {
                return err(StatusCode::UNPROCESSABLE_ENTITY, "INT32 param value out of range");
            }
            fleet_mavlink::ParamVal::Int32(v as i32)
        }
        _ => {
            let v = body.value as f32;
            if !v.is_finite() {
                return err(StatusCode::UNPROCESSABLE_ENTITY, "param value must be finite");
            }
            fleet_mavlink::ParamVal::Real32(v)
        }
    };
    // The echo round-trip is the source of truth (protocol §3.2): the
    // returned value is what the vehicle actually took.
    let confirmed = link.set_param_typed(&id, val).await;
    s.log.log(
        fleet_core::events::EventKind::SupervisorAction,
        Some(index),
        format!("setup: param write {id}={val} (type {wire_type}) via API — {}", if confirmed.is_some() { "echo confirmed" } else { "UNCONFIRMED (no PARAM_VALUE echo)" }),
    );
    ok(json!({
        "id": id,
        "sent": setup::val_json(val),
        "confirmed": confirmed.map(setup::val_json),
        "ok": confirmed.is_some(),
    }))
    .into_response()
}

/// `POST /api/vehicles/{i}/params/refresh` — (re)start the QGC-style
/// full parameter download (PARAM_REQUEST_LIST): the PARAM_VALUE burst
/// lands in the link's param store; poll `GET …/params` for progress.
async fn vehicle_params_refresh_post(
    State(s): State<Arc<AppState>>,
    Path(index): Path<u8>,
) -> Response {
    if index >= s.count {
        return err(
            StatusCode::NOT_FOUND,
            &format!("vehicle index {index} out of range (fleet count {})", s.count),
        );
    }
    let Some(link) = s.link(index) else {
        return err(StatusCode::CONFLICT, "vehicle link not registered (spawn failed?)");
    };
    let requested = link.request_param_list().await;
    s.log.log(
        fleet_core::events::EventKind::SupervisorAction,
        Some(index),
        format!("setup: parameter download requested via API — {requested}"),
    );
    ok(json!({ "requested": requested, "index": index })).into_response()
}

#[derive(serde::Deserialize)]
struct AirframeBody {
    sys_autostart: u32,
}

/// `POST /api/vehicles/{i}/airframe` — apply an airframe, QGC's
/// "Apply and Restart" flow: PARAM_SET `SYS_AUTOSTART` (echo-confirmed,
/// PX4 autosaves) then a controlled sim+px4 pair restart (the manager
/// respawns the pair; parameters.bson in the kept workdir makes rcS
/// load the new airframe on boot). Not while armed, exactly QGC's gate.
async fn vehicle_airframe_post(
    State(s): State<Arc<AppState>>,
    Path(index): Path<u8>,
    body: axum::extract::Json<AirframeBody>,
) -> Response {
    if index >= s.count {
        return err(
            StatusCode::NOT_FOUND,
            &format!("vehicle index {index} out of range (fleet count {})", s.count),
        );
    }
    // catalog membership: only real SYS_AUTOSTART ids are accepted
    // (the catalog is generated from the built ROMFS, so every id is
    // one px4's rc.autostart can actually resolve).
    let known = crate::airframes::AIRFRAMES.iter().any(|a| a.id == body.sys_autostart);
    if !known {
        return err(
            StatusCode::UNPROCESSABLE_ENTITY,
            &format!("SYS_AUTOSTART {} is not in the airframe catalog", body.sys_autostart),
        );
    }
    let Some(link) = s.link(index) else {
        return err(StatusCode::CONFLICT, "vehicle link not registered (spawn failed?)");
    };
    if let Some(snap) = s.registry.snapshot(index) {
        if snap.armed {
            return err(StatusCode::CONFLICT, "vehicle is ARMED — land before changing the airframe (QGC gate)");
        }
    }
    // SYS_AUTOSTART is an INT32 param: the wire value is its bit pattern
    // (typed protocol, ADR-0016) — a plain REAL32 write gets rejected by
    // PX4's param-type check and never echoes.
    let confirmed = link
        .set_param_typed("SYS_AUTOSTART", fleet_mavlink::ParamVal::Int32(body.sys_autostart as i32))
        .await;
    let write_ok = confirmed.is_some();
    if write_ok {
        // PX4's param autosave is deferred (autosave.cpp: 300 ms
        // ScheduleDelayed + flash commit); give it 4x that before the
        // pair restart kills px4, or rcS would import the STALE param
        // file and boot the old airframe — the QGC equivalent is the
        // operator clicking "reboot" a beat after the write.
        tokio::time::sleep(Duration::from_millis(1200)).await;
        s.request_restart(index);
        s.log.log(
            fleet_core::events::EventKind::SupervisorAction,
            Some(index),
            format!("setup: airframe apply SYS_AUTOSTART={} — write confirmed, restart queued", body.sys_autostart),
        );
    } else {
        s.log.log(
            fleet_core::events::EventKind::SupervisorAction,
            Some(index),
            format!("setup: airframe apply FAILED — SYS_AUTOSTART={} write unconfirmed", body.sys_autostart),
        );
    }
    ok(json!({
        "index": index,
        "sys_autostart": body.sys_autostart,
        "write_confirmed": write_ok,
        "echo": confirmed.map(setup::val_json),
        "restart_queued": write_ok,
    }))
    .into_response()
}

/// The calibration triggers PX4's MAV_CMD_PREFLIGHT_CALIBRATION (241)
/// actually answers to (commander.cpp v1.16 param matrix) — the same
/// triggers QGroundControl's calibration buttons send.
const CAL_TRIGGERS: &[(&str, &str, [f32; 7])] = &[
    //                sensor         ack description                     param matrix
    ("gyro", "gyroscope", [1.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0]),
    ("accel", "accelerometer", [0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0]),
    ("accel_quick", "accelerometer (quick)", [0.0, 0.0, 0.0, 0.0, 4.0, 0.0, 0.0]),
    ("mag", "magnetometer", [0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 0.0]),
    ("level", "level horizon", [0.0, 0.0, 0.0, 0.0, 2.0, 0.0, 0.0]),
    ("baro", "barometer (ground pressure)", [0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0]),
    ("airspeed", "airspeed", [0.0, 0.0, 0.0, 0.0, 0.0, 2.0, 0.0]),
];

#[derive(serde::Deserialize)]
struct CalibrateBody {
    /// One of the CAL_TRIGGERS keys: gyro|accel|accel_quick|mag|level|…
    sensor: String,
}

/// `POST /api/vehicles/{i}/calibrate` — trigger a sensor calibration
/// (MAV_CMD_PREFLIGHT_CALIBRATION 241, commander's own param matrix).
/// QGC's gates apply: the vehicle must be disarmed and idle; PX4 answers
/// with the COMMAND_ACK and runs the calibration task (SITL completes
/// gyro/accel/mag/level in seconds; CAL_*_ID params flip nonzero in the
/// next param download).
async fn vehicle_calibrate_post(
    State(s): State<Arc<AppState>>,
    Path(index): Path<u8>,
    body: axum::extract::Json<CalibrateBody>,
) -> Response {
    if index >= s.count {
        return err(
            StatusCode::NOT_FOUND,
            &format!("vehicle index {index} out of range (fleet count {})", s.count),
        );
    }
    let Some((name, desc, params)) = CAL_TRIGGERS
        .iter()
        .find(|(n, _, _)| *n == body.sensor)
        .map(|(n, d, p)| (*n, *d, *p))
    else {
        let known: Vec<&str> = CAL_TRIGGERS.iter().map(|(n, _, _)| *n).collect();
        return err(
            StatusCode::UNPROCESSABLE_ENTITY,
            &format!("unknown sensor '{}' — one of {:?}", body.sensor, known),
        );
    };
    let Some(link) = s.link(index) else {
        return err(StatusCode::CONFLICT, "vehicle link not registered (spawn failed?)");
    };
    if let Some(snap) = s.registry.snapshot(index) {
        if snap.armed {
            return err(StatusCode::CONFLICT, "vehicle is ARMED — calibrations are rejected while armed (QGC gate)");
        }
    }
    let ack = link
        .send_command(fleet_mavlink::cmds::PREFLIGHT_CALIBRATION, params)
        .await;
    let (result, accepted) = match &ack {
        fleet_mavlink::CmdAck::Accepted => (0u8, true),
        fleet_mavlink::CmdAck::Rejected { result } => (*result, false),
        fleet_mavlink::CmdAck::Timeout { .. } => (255u8, false),
    };
    s.log.log(
        fleet_core::events::EventKind::SupervisorAction,
        Some(index),
        format!("setup: calibration '{name}' ({desc}) — command 241 {}", if accepted { "ACCEPTED" } else { "not accepted" }),
    );
    ok(json!({
        "index": index,
        "sensor": name,
        "description": desc,
        "command": fleet_mavlink::cmds::PREFLIGHT_CALIBRATION,
        "result": result,
        "accepted": accepted,
    }))
    .into_response()
}

#[derive(serde::Deserialize)]
struct ModeBody {
    /// A switchable mode name (GET /api/modes): MANUAL, POSCTL, AUTO.RTL…
    mode: String,
}

/// `POST /api/vehicles/{i}/mode` — switch flight mode (DO_SET_MODE with
/// the fleet-modes-verified mode word). QGC's flight-mode switch, plus
/// the same arming gate the supervisor's own mode changes respect.
async fn vehicle_mode_post(
    State(s): State<Arc<AppState>>,
    Path(index): Path<u8>,
    body: axum::extract::Json<ModeBody>,
) -> Response {
    if index >= s.count {
        return err(
            StatusCode::NOT_FOUND,
            &format!("vehicle index {index} out of range (fleet count {})", s.count),
        );
    }
    let Some(word) = setup::mode_word_by_name(&body.mode) else {
        let known: Vec<String> = setup::switchable_modes().into_iter().map(|(n, _)| n).collect();
        return err(
            StatusCode::UNPROCESSABLE_ENTITY,
            &format!("unknown mode '{}' — one of {:?}", body.mode, known),
        );
    };
    let Some(link) = s.link(index) else {
        return err(StatusCode::CONFLICT, "vehicle link not registered (spawn failed?)");
    };
    let ack = link.set_mode(word).await;
    let (result, accepted) = match &ack {
        fleet_mavlink::CmdAck::Accepted => (0u8, true),
        fleet_mavlink::CmdAck::Rejected { result } => (*result, false),
        fleet_mavlink::CmdAck::Timeout { .. } => (255u8, false),
    };
    s.log.log(
        fleet_core::events::EventKind::SupervisorAction,
        Some(index),
        format!("setup: mode change {} (0x{word:08X}) via API — {}", body.mode, if accepted { "ACCEPTED" } else { "not accepted" }),
    );
    ok(json!({
        "index": index,
        "mode": body.mode,
        "mode_word": word,
        "result": result,
        "accepted": accepted,
    }))
    .into_response()
}

// ---------------------------------------------------------------------------
// WS plane
// ---------------------------------------------------------------------------

/// Entry for every WS-capable path. The extractor is applied manually so a
/// non-upgrade request gets the JSON index instead of a 405 — path tolerance
/// for the preview gateway, which forwards WebSockets as `/?XTransformPort=8400`
/// (plain GET `/` must still work).
async fn ws_entry(State(s): State<Arc<AppState>>, mut parts: Parts) -> Response {
    let upgrade: Result<WebSocketUpgrade, _> =
        WebSocketUpgrade::from_request_parts(&mut parts, &()).await;
    match upgrade {
        Ok(upgrade) => upgrade.on_upgrade(move |socket| forward_ws(s, socket)),
        Err(_) => ok(json!({
            "service": "mavfleet",
            "scenario": s.scenario_path,
            "endpoints": [
                "GET /api/fleet", "GET /api/vehicles/{i}", "GET /api/events",
                "GET /api/tasks", "POST /api/estop", "POST /api/fleet/estop",
                "GET /api/airframes", "GET /api/modes",
                "GET /api/vehicles/{i}/setup", "GET|POST /api/vehicles/{i}/params",
                "POST /api/vehicles/{i}/params/refresh",
                "POST /api/vehicles/{i}/airframe", "POST /api/vehicles/{i}/calibrate",
                "POST /api/vehicles/{i}/mode",
                "WS /ws/fleet | /ws | /"
            ],
            "phase": s.phase(),
            "vehicles": s.count,
        }))
        .into_response(),
    }
}

/// 10 Hz fleet frames + events over one socket, until the client leaves.
/// (Spec §3.4 says 5 Hz; the task order for this build pins 10 Hz — the
/// frame schema is identical, only the cadence differs. Documented in the
/// run report header.)
async fn forward_ws(state: Arc<AppState>, mut socket: WebSocket) {
    let mut ticker = tokio::time::interval(Duration::from_millis(100));
    ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    loop {
        tokio::select! {
            _ = ticker.tick() => {
                let frame = state::fleet_frame(&state, 16);
                match serde_json::to_string(&frame) {
                    Ok(text) => {
                        if socket.send(Message::Text(text.into())).await.is_err() {
                            break;
                        }
                    }
                    Err(_) => break,
                }
            }
            msg = socket.recv() => {
                match msg {
                    None => break,
                    Some(Ok(Message::Text(t))) => {
                        if t.as_str() == "ping" {
                            if socket.send(Message::Text("pong".into())).await.is_err() {
                                break;
                            }
                        }
                    }
                    Some(Ok(_)) => {}
                    Some(Err(_)) => break,
                }
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Router-level integration tests (handlers against a constructed state)
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use fleet_core::registry::Registry;
    use fleet_core::EventLog;
    use fleet_mission::report::TaskStatus;
    use tower::ServiceExt; // oneshot
    use axum::body::Body;
    use axum::http::{header, Request};

    fn test_state() -> Arc<AppState> {
        let registry = Registry::new(2);
        registry.entry(0).unwrap().state.write(|s| {
            s.sysid = 1;
            s.compid = 1;
            s.heartbeat_seen = true;
            s.local_position_seen = true;
            s.home_set = true;
            s.mode_word = fleet_modes::MODE_WORD_POSCTL;
            s.position_ned_m = [1.0, 2.0, -3.0];
            s.last_msg_ms = 100;
            s.last_heartbeat_ms = 100;
            s.link.recv = 42;
            s.link.recv_rate_hz = 50.0;
            s.msg_counts.insert(0, 5);
            s.msg_counts.insert(30, 120);
        });
        registry.entry(1).unwrap().state.write(|s| {
            s.sysid = 2;
            s.mode_word = fleet_modes::MODE_WORD_OFFBOARD;
            s.last_msg_ms = 100;
            s.last_heartbeat_ms = 100;
        });
        let log = EventLog::in_memory();
        log.log(
            fleet_core::events::EventKind::RunBoundary,
            None,
            "test run started",
        );
        log.log(
            fleet_core::events::EventKind::FsmTransition,
            Some(0),
            "INIT->SPAWNING cause=spawn",
        );
        let s = AppState::new(registry, log, 2, "test-scenario.toml", 0);
        *s.tasks.lock().unwrap() = vec![TaskStatus {
            id: "wp_n".into(),
            pos_ned_m: [0.0, 0.0, -10.0],
            assigned: Some(0),
            state: "queued".into(),
            hover_observed: None,
        }];
        s.set_vehicle_tasks(
            0,
            state::VehicleTaskInfo {
                current: Some("wp_n".into()),
                queue: vec![],
            },
        );
        s.set_phase("RUNNING");
        s
    }

    async fn body_json(resp: Response) -> serde_json::Value {
        let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .expect("body");
        serde_json::from_slice(&bytes).expect("json")
    }

    #[tokio::test]
    async fn fleet_get_envelope_and_fields() {
        let state = test_state();
        let app = router(state);
        let resp = app
            .clone()
            .oneshot(Request::get("/api/fleet").body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let j = body_json(resp).await;
        assert_eq!(j["ok"], true, "envelope ok");
        let d = &j["data"];
        assert_eq!(d["phase"], "RUNNING");
        assert_eq!(d["vehicles"].as_array().unwrap().len(), 2);
        let v0 = &d["vehicles"][0];
        assert_eq!(v0["sysid"], 1);
        assert_eq!(v0["mode"], "POSCTL");
        assert_eq!(v0["fsm"], "INIT");
        assert_eq!(v0["msg_counts"]["HEARTBEAT"], 5);
        assert_eq!(v0["msg_counts"]["ATTITUDE"], 120);
        assert_eq!(v0["link"]["recv"], 42);
        assert_eq!(v0["task_queue"], serde_json::json!([]));
        assert_eq!(v0["current_task"], "wp_n");
    }

    #[tokio::test]
    async fn vehicle_detail_and_404() {
        let state = test_state();
        let app = router(state);
        let resp = app
            .clone()
            .oneshot(Request::get("/api/vehicles/1").body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let j = body_json(resp).await;
        assert_eq!(j["data"]["sysid"], 2);
        assert_eq!(j["data"]["mode"], "OFFBOARD");
        assert!(j["data"]["heartbeat_age_ms"].is_u64(), "age field present");
        // out of range → error envelope with 404
        let resp = app
            .clone()
            .oneshot(Request::get("/api/vehicles/5").body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::NOT_FOUND);
        let j = body_json(resp).await;
        assert_eq!(j["ok"], false);
        assert!(j["error"].as_str().unwrap().contains("out of range"));
    }

    #[tokio::test]
    async fn events_tail_endpoint() {
        let state = test_state();
        let app = router(state);
        let resp = app
            .clone()
            .oneshot(Request::get("/api/events?tail=1").body(Body::empty()).unwrap())
            .await
            .unwrap();
        let j = body_json(resp).await;
        assert_eq!(j["ok"], true);
        assert_eq!(j["data"]["returned"], 1);
        assert_eq!(j["data"]["events"][0]["kind"], "fsm_transition");
        assert_eq!(j["data"]["total"], 2);
    }

    #[tokio::test]
    async fn estop_latches_and_logs() {
        let state = test_state();
        let app = router(Arc::clone(&state));
        // both spec'd paths accept POST
        for uri in ["/api/estop", "/api/fleet/estop"] {
            let resp = app
                .clone()
                .oneshot(
                    Request::post(uri)
                        .header(header::CONTENT_TYPE, "application/json")
                        .body(Body::empty())
                        .unwrap(),
                )
                .await
                .unwrap();
            assert_eq!(resp.status(), StatusCode::OK);
            let j = body_json(resp).await;
            assert_eq!(j["ok"], true);
            assert_eq!(j["data"]["estop"], true);
        }
        assert!(state.estop_requested());
        assert!(state.log.total() >= 4, "estop events logged");
    }

    #[tokio::test]
    async fn root_serves_json_index_for_plain_get() {
        let state = test_state();
        let app = router(state);
        let resp = app
            .clone()
            .oneshot(Request::get("/").body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let j = body_json(resp).await;
        assert_eq!(j["ok"], true);
        assert_eq!(j["data"]["service"], "mavfleet");
        assert_eq!(j["data"]["vehicles"], 2);
    }

    #[tokio::test]
    async fn ws_upgrades_accepted_on_spec_and_root_paths() {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        // Real hyper server: the 101 handshake needs the OnUpgrade extension
        // hyper injects, so this runs the actual serve path on an ephemeral
        // port and drives the upgrade over raw TCP (the sandbox gateway's
        // `/?XTransformPort=8400` shape included).
        let state = test_state();
        let app = router(state);
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            let _ = axum::serve(listener, app).await;
        });
        for path in ["/ws/fleet", "/ws", "/", "/?XTransformPort=8400"] {
            let mut stream = tokio::net::TcpStream::connect(addr).await.unwrap();
            let req = format!(
                "GET {path} HTTP/1.1\r\nHost: localhost\r\nConnection: Upgrade\r\nUpgrade: websocket\r\nSec-WebSocket-Key: dGhlIHNhbXBsZSBub25jZQ==\r\nSec-WebSocket-Version: 13\r\n\r\n"
            );
            stream.write_all(req.as_bytes()).await.unwrap();
            let mut buf = [0u8; 256];
            let n = tokio::time::timeout(std::time::Duration::from_secs(2), stream.read(&mut buf))
                .await
                .expect("read within 2s")
                .expect("read ok");
            let resp = String::from_utf8_lossy(&buf[..n]);
            let first = resp.lines().next().unwrap_or("");
            assert!(first.contains("101"), "WS upgrade on {path} got: {first}");
        }
    }

    #[tokio::test]
    async fn ws_stream_sends_fleet_frames() {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let state = test_state();
        let app = router(state);
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            let _ = axum::serve(listener, app).await;
        });
        let mut stream = tokio::net::TcpStream::connect(addr).await.unwrap();
        let req = "GET /ws/fleet HTTP/1.1\r\nHost: localhost\r\nConnection: Upgrade\r\nUpgrade: websocket\r\nSec-WebSocket-Key: dGhlIHNhbXBsZSBub25jZQ==\r\nSec-WebSocket-Version: 13\r\n\r\n";
        stream.write_all(req.as_bytes()).await.unwrap();
        // Handshake + first frame(s): a fleet frame with phase/vehicles/t_ms
        // must arrive within the 10 Hz cadence.
        let mut buf = vec![0u8; 4096];
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(3);
        let mut seen = String::new();
        while std::time::Instant::now() < deadline {
            match tokio::time::timeout(
                std::time::Duration::from_millis(500),
                stream.read(&mut buf),
            )
            .await
            {
                Ok(Ok(0)) | Err(_) | Ok(Err(_)) => break,
                Ok(Ok(n)) => {
                    seen.push_str(&String::from_utf8_lossy(&buf[..n]));
                    if seen.contains("\"phase\":\"RUNNING\"") && seen.contains("\"t_ms\"") {
                        break;
                    }
                }
            }
        }
        assert!(
            seen.contains("\"phase\":\"RUNNING\"") && seen.contains("\"vehicles\""),
            "no fleet frame observed in WS stream: {:.200}",
            seen
        );
    }

    #[tokio::test]
    async fn tasks_endpoint_lists_table() {
        let state = test_state();
        let app = router(state);
        let resp = app
            .clone()
            .oneshot(Request::get("/api/tasks").body(Body::empty()).unwrap())
            .await
            .unwrap();
        let j = body_json(resp).await;
        assert_eq!(j["data"]["tasks"][0]["id"], "wp_n");
        assert_eq!(j["data"]["tasks"][0]["assigned"], 0);
        assert_eq!(j["data"]["tasks"][0]["state"], "queued");
    }

    // -- vehicle-setup plane (ADR-0016) ---------------------------------

    #[tokio::test]
    async fn airframes_catalog_groups_and_counts() {
        let state = test_state();
        let app = router(state);
        let resp = app
            .clone()
            .oneshot(Request::get("/api/airframes").body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let j = body_json(resp).await;
        assert_eq!(j["ok"], true);
        let groups = j["data"]["groups"].as_array().unwrap();
        let cats: Vec<String> = groups.iter().map(|g| g["category"].as_str().unwrap().into()).collect();
        assert!(cats.contains(&"Multirotor (UAV)".to_string()), "UAV group");
        assert!(cats.contains(&"Boat (USV)".to_string()), "USV group");
        assert!(cats.contains(&"Underwater (UUV)".to_string()), "UUV group");
        assert_eq!(cats[0], "Multirotor (UAV)", "QGC browser order");
        // count matches the static catalog length
        let total: usize = groups.iter().map(|g| g["airframes"].as_array().unwrap().len()).sum();
        assert_eq!(total as u32, j["data"]["count"].as_u64().unwrap() as u32);
        // every entry has an id + name + frame_type
        for g in groups {
            for a in g["airframes"].as_array().unwrap() {
                assert!(a["id"].as_u64().is_some() && a["name"].as_str().is_some());
            }
        }
    }

    #[tokio::test]
    async fn modes_endpoint_lists_switchable_set() {
        let state = test_state();
        let app = router(state);
        let resp = app
            .clone()
            .oneshot(Request::get("/api/modes").body(Body::empty()).unwrap())
            .await
            .unwrap();
        let j = body_json(resp).await;
        let modes = j["data"]["modes"].as_array().unwrap();
        let names: Vec<&str> = modes.iter().map(|m| m["name"].as_str().unwrap()).collect();
        assert!(names.contains(&"MANUAL"));
        assert!(names.contains(&"POSCTL"));
        assert!(names.contains(&"AUTO.RTL"));
        assert!(names.contains(&"OFFBOARD"));
        // mode words are the fleet-modes-verified constants
        let posctl = modes.iter().find(|m| m["name"] == "POSCTL").unwrap();
        assert_eq!(posctl["mode_word"], fleet_modes::MODE_WORD_POSCTL as u64);
    }

    #[tokio::test]
    async fn vehicle_setup_get_shape_and_404() {
        let state = test_state();
        let app = router(state);
        let resp = app
            .clone()
            .oneshot(Request::get("/api/vehicles/0/setup").body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let j = body_json(resp).await;
        let d = &j["data"];
        // no link registered in the unit-test state: param-derived sections
        // are null (the console shows "no link yet"), but identity fields
        // are always present
        assert_eq!(d["index"], 0);
        assert_eq!(d["sysid"], 1);
        assert_eq!(d["mode"], "POSCTL");
        assert_eq!(d["autopilot"]["type"], "PX4");
        assert!(d["airframe"].is_object());
        assert_eq!(d["airframe"]["sys_autostart"], serde_json::Value::Null);
        assert!(d["params"].is_null());
        assert!(d["calibration"].is_null());
        assert!(d["power"].is_null());
        assert!(d["safety"].is_null());
        // 404 shape
        let resp = app
            .clone()
            .oneshot(Request::get("/api/vehicles/9/setup").body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn vehicle_params_get_no_link_is_empty_store() {
        let state = test_state();
        let app = router(state);
        let resp = app
            .clone()
            .oneshot(Request::get("/api/vehicles/1/params").body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let j = body_json(resp).await;
        assert_eq!(j["data"]["state"], "NoLink");
        assert_eq!(j["data"]["params"].as_array().unwrap().len(), 0);
    }

    #[tokio::test]
    async fn setup_write_endpoints_reject_without_link_and_bad_bodies() {
        let state = test_state();
        let app = router(state);
        // param write: no link registered → 409 CONFLICT
        let resp = app
            .clone()
            .oneshot(
                Request::post("/api/vehicles/0/params")
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from(r#"{"id":"BAT_N_CELLS","value":4}"#))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::CONFLICT);
        // airframe: not in catalog → 422
        let resp = app
            .clone()
            .oneshot(
                Request::post("/api/vehicles/0/airframe")
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from(r#"{"sys_autostart":999999}"#))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::UNPROCESSABLE_ENTITY);
        // airframe: catalog member but no link → 409 (catalog check first)
        let resp = app
            .clone()
            .oneshot(
                Request::post("/api/vehicles/0/airframe")
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from(r#"{"sys_autostart":1070}"#))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::CONFLICT);
        // calibrate: unknown sensor → 422 with the known list
        let resp = app
            .clone()
            .oneshot(
                Request::post("/api/vehicles/0/calibrate")
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from(r#"{"sensor":"warp_drive"}"#))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::UNPROCESSABLE_ENTITY);
        let j = body_json(resp).await;
        assert!(j["error"].as_str().unwrap().contains("gyro"));
        // mode: unknown mode name → 422
        let resp = app
            .clone()
            .oneshot(
                Request::post("/api/vehicles/0/mode")
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from(r#"{"mode":"TURBO"}"#))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::UNPROCESSABLE_ENTITY);
        // out of range → 404 for all setup POSTs
        for uri in [
            "/api/vehicles/5/params/refresh",
            "/api/vehicles/5/mode",
            "/api/vehicles/5/calibrate",
        ] {
            let resp = app
                .clone()
                .oneshot(
                    Request::post(uri)
                        .header(header::CONTENT_TYPE, "application/json")
                        .body(Body::from(r#"{"sensor":"gyro","mode":"MANUAL"}"#))
                        .unwrap(),
                )
                .await
                .unwrap();
            assert_eq!(resp.status(), StatusCode::NOT_FOUND, "{uri}");
        }
    }

    #[tokio::test]
    async fn setup_write_endpoints_reject_out_of_range_index() {
        let state = test_state();
        let app = router(state);
        let resp = app
            .clone()
            .oneshot(
                Request::post("/api/vehicles/5/params")
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from(r#"{"id":"BAT_N_CELLS","value":4}"#))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::NOT_FOUND);
        let resp = app
            .clone()
            .oneshot(
                Request::post("/api/vehicles/5/airframe")
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from(r#"{"sys_autostart":4001}"#))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::NOT_FOUND);
    }
}
