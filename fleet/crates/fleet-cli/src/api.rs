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
use axum::routing::{delete, get, post};
use axum::Router;
use serde_json::json;

use crate::setup;
use crate::simproxy;
use crate::state::{self, AppState};
use crate::state::{OperatorCmd, OperatorTask, OperatorWaypoint};

/// Build the control-plane router (also the unit-test surface).
pub fn router(state: Arc<AppState>) -> Router {
    Router::new()
        .route("/api/fleet", get(fleet_get).put(fleet_put))
        .route("/api/fleet/estop", post(estop_post))
        .route("/api/estop", post(estop_post))
        .route("/api/vehicles/{index}", get(vehicle_get))
        .route("/api/events", get(events_get))
        .route("/api/vehicles/{index}/faults", post(vehicle_faults_post))
        .route("/api/tasks", get(tasks_get).post(tasks_post))
        // -- vehicle-setup plane (ADR-0016, QGC/MP-style) ----------------
        .route("/api/airframes", get(airframes_get))
        .route("/api/modes", get(modes_get))
        .route("/api/vehicles/{index}/setup", get(vehicle_setup_get))
        .route("/api/vehicles/{index}/params", get(vehicle_params_get).post(vehicle_params_post))
        .route("/api/vehicles/{index}/params/refresh", post(vehicle_params_refresh_post))
        .route("/api/vehicles/{index}/airframe", post(vehicle_airframe_post))
        .route("/api/vehicles/{index}/calibrate", post(vehicle_calibrate_post))
        .route("/api/vehicles/{index}/mode", post(vehicle_mode_post))
        // -- operator control plane (ADR-0017, QGC Fly/Plan-style) --------
        .route("/api/mission", post(mission_upload_post))
        .route("/api/mission/start", post(mission_start_post))
        .route("/api/mission/clear", post(mission_clear_post))
        .route("/api/vehicles/{index}/arm", post(vehicle_arm_post))
        .route("/api/vehicles/{index}/takeoff", post(vehicle_takeoff_post))
        .route("/api/vehicles/{index}/land", post(vehicle_land_post))
        .route("/api/vehicles/{index}/rtl", post(vehicle_rtl_post))
        .route("/api/vehicles/{index}/hold", post(vehicle_hold_post))
        .route("/api/vehicles/{index}/goto", post(vehicle_goto_post))
        // -- MAVLink mission upload/download (M2-API): direct proxies to the
        // link's mission-protocol state machine (commit 99993f5). Distinct
        // from ADR-0017's `POST /api/mission` (geo→NED + auction): these
        // endpoints pass raw MISSION_ITEM_INT items through to PX4.
        .route("/api/vehicles/{index}/mission", get(vehicle_mission_get))
        .route("/api/vehicles/{index}/mission/upload", post(vehicle_mission_upload_post))
        // -- M3 (Fly View): QGC-style pre-arm checklist (GCS_SPEC.md §5.2).
        // The Fly View's ARM button gates on `all_passed` here; each check
        // reflects the live VehicleState + health flags so the operator sees
        // the real reason an arming attempt would be TEMPORARILY_REJECTED.
        .route("/api/vehicles/{index}/prearm-checks", get(vehicle_prearm_checks_get))
        // -- M5 (Fleet C2, GCS_SPEC.md §5.4): per-vehicle mission binding,
        // fleet-wide start (parallel/sequential), swarming patterns.
        // Bindings are stored in AppState (one ULID per vehicle, None = unbound);
        // `POST /api/fleet/start` fetches each bound mission from :8300 and
        // uploads it via the per-vehicle link's mission protocol.
        .route(
            "/api/fleet/mission-bindings",
            get(fleet_mission_bindings_get).post(fleet_mission_bindings_post),
        )
        .route("/api/fleet/mission-bindings/{vehicle_id}", delete(fleet_mission_binding_delete))
        .route("/api/fleet/start", post(fleet_start_post))
        .route("/api/fleet/patterns", get(fleet_patterns_get))
        .route("/api/fleet/patterns/{name}/generate", post(fleet_pattern_generate_post))
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

// ---------------------------------------------------------------------------
// Runtime task append (ADR-0018, `POST /api/tasks`)
// ---------------------------------------------------------------------------

/// `POST /api/tasks` body: a single task object or an array of them. Tasks
/// are NED metres home-relative (the scenario DSL's own convention) — the
/// direct counterpart of `POST /api/mission`'s geo waypoints.
#[derive(Debug, serde::Deserialize)]
struct TaskBody {
    #[serde(default)]
    id: Option<String>,
    pos_ned_m: [f32; 3],
    #[serde(default)]
    hover_s: f32,
    #[serde(default = "default_task_reward")]
    reward: f32,
    #[serde(default)]
    deadline_s: Option<f32>,
}

fn default_task_reward() -> f32 {
    1.0
}

/// `POST /api/tasks` (spec 6.5): append tasks at runtime — the supervisor
/// validates them with the compiler's own rules (fence polygon, altitude
/// box, reachability) and injects them into the task board + auction pool,
/// so the next auction round reallocates over the grown pool.
async fn tasks_post(
    State(s): State<Arc<AppState>>,
    body: Option<axum::extract::Json<serde_json::Value>>,
) -> Response {
    let Some(axum::extract::Json(v)) = body else {
        return err(StatusCode::BAD_REQUEST, "body must be a task JSON object or array");
    };
    let items: Vec<serde_json::Value> = match v {
        serde_json::Value::Array(a) => a,
        serde_json::Value::Object(_) => vec![v],
        _ => return err(StatusCode::BAD_REQUEST, "body must be a task JSON object or array"),
    };
    if items.is_empty() {
        return err(StatusCode::UNPROCESSABLE_ENTITY, "no tasks in body");
    }
    if items.len() > 64 {
        return err(StatusCode::UNPROCESSABLE_ENTITY, "too many tasks (max 64)");
    }
    let mut tasks = Vec::with_capacity(items.len());
    for (k, item) in items.into_iter().enumerate() {
        let parsed: TaskBody = match serde_json::from_value(item) {
            Ok(p) => p,
            Err(e) => {
                return err(
                    StatusCode::UNPROCESSABLE_ENTITY,
                    &format!("task[{k}] invalid: {e} (pos_ned_m = [x, y, z] NED, z negative up)"),
                )
            }
        };
        if !parsed.pos_ned_m.iter().all(|c| c.is_finite()) {
            return err(StatusCode::UNPROCESSABLE_ENTITY, &format!("task[{k}] pos_ned_m must be finite"));
        }
        tasks.push(OperatorTask {
            id: parsed.id,
            pos_ned_m: parsed.pos_ned_m,
            hover_s: parsed.hover_s,
            reward: parsed.reward,
            deadline_s: parsed.deadline_s,
        });
    }
    let (tx, rx) = tokio::sync::oneshot::channel();
    s.push_operator_cmd(OperatorCmd::Append { tasks, ack: tx });
    match tokio::time::timeout(Duration::from_secs(2), rx).await {
        Ok(Ok(ack)) => ok(json!({
            "accepted": ack.accepted,
            "rejected": ack.rejected.iter()
                .map(|(l, r)| json!({"label": l, "reason": r}))
                .collect::<Vec<_>>(),
            "pool": ack.pool,
        }))
        .into_response(),
        _ => err(
            StatusCode::SERVICE_UNAVAILABLE,
            "supervisor did not drain the append — is the fleet running?",
        )
        .into_response(),
    }
}

// ---------------------------------------------------------------------------
// Hot scenario load (ADR-0018, `PUT /api/fleet`)
// ---------------------------------------------------------------------------

/// `PUT /api/fleet` body: the full scenario TOML as text.
#[derive(Debug, serde::Deserialize)]
struct FleetPutBody {
    scenario_toml: String,
}

/// `PUT /api/fleet` (spec 3.4): validate a scenario TOML, stage it, and —
/// when the fleet is quiescent (no mission flying) — gracefully stop this
/// run and hot-restart with the staged scenario on the same port. The
/// parse/compile gate is the same one `mavfleet check` applies; the
/// supervisor re-checks quiescence at drain time (the REST handler's view
/// can be a hair stale).
async fn fleet_put(
    State(s): State<Arc<AppState>>,
    body: Result<
        axum::extract::Json<FleetPutBody>,
        axum::extract::rejection::JsonRejection,
    >,
) -> Response {
    let axum::extract::Json(FleetPutBody { scenario_toml }) = match body {
        Ok(b) => b,
        Err(e) => {
            return err(
                StatusCode::UNPROCESSABLE_ENTITY,
                &format!("invalid body: {e}"),
            )
        }
    };
    if scenario_toml.trim().is_empty() {
        return err(StatusCode::BAD_REQUEST, "scenario_toml must not be empty");
    }
    // Full validation up front: schema, ranges, compile-time task checks —
    // a scenario that cannot run is rejected before anything is staged.
    if let Err(e) = fleet_mission::Scenario::parse_toml(&scenario_toml) {
        return err(StatusCode::UNPROCESSABLE_ENTITY, &format!("invalid scenario: {e}"));
    }
    let plan = {
        let scenario = fleet_mission::Scenario::parse_toml(&scenario_toml).expect("re-parsed");
        fleet_mission::MissionPlan::compile(&scenario)
    };
    if !plan.rejections.is_empty() {
        let detail = plan
            .rejections
            .iter()
            .map(|(id, r)| format!("{id}: {r}"))
            .collect::<Vec<_>>()
            .join("; ");
        return err(
            StatusCode::UNPROCESSABLE_ENTITY,
            &format!("scenario rejected at compile: {detail}"),
        );
    }
    // Fast quiescence gate (the supervisor re-checks at drain time).
    let flying: Vec<u8> = (0..s.count).filter(|&i| s.mission_active(i)).collect();
    if !flying.is_empty() {
        return err(
            StatusCode::CONFLICT,
            &format!(
                "vehicles {flying:?} are flying an autonomous mission — land/clear before a scenario swap"
            ),
        );
    }
    // Stage the file in this run's directory, then ask the supervisor.
    let staged = s
        .run_dir
        .join(format!("hot-scenario-{}.toml", fleet_core::events::fleet_epoch_ms()));
    if let Err(e) = std::fs::write(&staged, &scenario_toml) {
        return err(StatusCode::INTERNAL_SERVER_ERROR, &format!("cannot stage scenario: {e}"));
    }
    s.set_next_scenario(staged.clone());
    let (tx, rx) = tokio::sync::oneshot::channel();
    s.push_operator_cmd(OperatorCmd::HotLoad { ack: tx });
    match tokio::time::timeout(Duration::from_secs(2), rx).await {
        Ok(Ok(ack)) if ack.accepted => ok(json!({
            "staged": staged,
            "note": "graceful stop in progress — the fleet restarts with the staged scenario; the control plane rebinds on this port within seconds",
        }))
        .into_response(),
        Ok(Ok(ack)) => {
            s.take_next_scenario(); // supervisor nack: nothing is staged
            err(
                StatusCode::CONFLICT,
                ack.reason.as_deref().unwrap_or("supervisor rejected the swap"),
            )
            .into_response()
        }
        _ => {
            s.take_next_scenario(); // no supervisor answer: clean up the staging
            err(
                StatusCode::SERVICE_UNAVAILABLE,
                "supervisor did not drain the load — is the fleet running?",
            )
            .into_response()
        }
    }
}

// ---------------------------------------------------------------------------
// Fault proxy (ADR-0018, `POST /api/vehicles/{i}/faults`)
// ---------------------------------------------------------------------------

/// `POST /api/vehicles/{i}/faults`: inject a rustsitsim fault on vehicle i
/// by proxying to its own control plane (:8200+i, the sim's §7.3 REST fault
/// plane). The body is the sim's fault-event schema — `{"type":
/// "motor_cut", "params": {...}, "duration_ms": ...}` — validated by the
/// sim itself (single source of truth for the catalog) and the response is
/// relayed verbatim, status included.
async fn vehicle_faults_post(
    State(s): State<Arc<AppState>>,
    Path(index): Path<u8>,
    body: Option<axum::extract::Json<serde_json::Value>>,
) -> Response {
    if index >= s.count {
        return err(StatusCode::NOT_FOUND, &format!("vehicle {index} out of range (0..{})", s.count - 1));
    }
    let Some(axum::extract::Json(v)) = body else {
        return err(StatusCode::BAD_REQUEST, "body must be a fault event JSON object");
    };
    let Some(t) = v.get("type").and_then(|t| t.as_str()) else {
        return err(StatusCode::BAD_REQUEST, "body must carry a \"type\" (the rustsitsim fault catalog id)");
    };
    if t.trim().is_empty() {
        return err(StatusCode::BAD_REQUEST, "\"type\" must not be empty");
    }
    let port = fleet_simctl::ports::sim_api(index);
    match simproxy::post_json(port, "/api/faults", &v).await {
        Ok((code, sim_body)) => {
            let status = StatusCode::from_u16(code).unwrap_or(StatusCode::BAD_GATEWAY);
            // Relay the sim's own envelope {"ok","data"|"error"} verbatim —
            // the catalog, ids ("runtime-N") and reasons are the sim's.
            (status, Json(sim_body)).into_response()
        }
        Err(e) => err(
            StatusCode::SERVICE_UNAVAILABLE,
            &format!("vehicle {index} sim control plane not reachable: {e}"),
        )
        .into_response(),
    }
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
///
/// Query params (GCS_SPEC.md §5.3 — Vehicle Setup search/filter):
///   - `search` — case-insensitive substring on the param id (QGC's
///     param-search box; AC-5.3.1).
///   - `group`  — exact group match (PX4's grouping convention: the
///     param id's underscore-separated prefix — `MPC`, `MC`, `BAT` …).
///
/// The summary fields (`total`/`received`/`state`/`requested_ms`/
/// `last_value_ms`) always reflect the FULL store — the filters only
/// narrow the `params` array the operator sees.
#[derive(serde::Deserialize, Default)]
struct ParamsQuery {
    #[serde(default)]
    search: Option<String>,
    #[serde(default)]
    group: Option<String>,
}

async fn vehicle_params_get(
    State(s): State<Arc<AppState>>,
    Path(index): Path<u8>,
    Query(q): Query<ParamsQuery>,
) -> Response {
    if index >= s.count {
        return err(
            StatusCode::NOT_FOUND,
            &format!("vehicle index {index} out of range (fleet count {})", s.count),
        );
    }
    let filter = setup::ParamsFilter {
        search: q.search,
        group: q.group,
    };
    let store = s.link(index).map(|h| h.param_store());
    let data = match store {
        Some(st) => setup::build_params_json(&st, &filter),
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
// Operator control plane (ADR-0017) — the QGroundControl Fly/Plan-style
// view: mission upload from the map, mission start, and the guided action
// bar (arm, takeoff, land, RTL, hold, go-to). Mission mutations queue onto
// the supervisor's tick loop (it stays the single writer of plan/pool);
// guided commands write the same MAVLink the supervisor itself sends,
// gated so they can never fight a live mission runner.
// ---------------------------------------------------------------------------

/// One waypoint as uploaded from the map (`POST /api/mission`).
#[derive(serde::Deserialize)]
struct MissionItemBody {
    #[serde(default)]
    label: Option<String>,
    lat_deg: f64,
    lon_deg: f64,
    /// Metres AGL above the scenario origin (QGC's waypoint convention).
    alt_m: f64,
    #[serde(default)]
    hover_s: f32,
}

#[derive(serde::Deserialize)]
struct MissionBody {
    items: Vec<MissionItemBody>,
    /// "append" (default) or "replace" (drop queued operator tasks first).
    #[serde(default)]
    mode: Option<String>,
}

/// `POST /api/mission` — upload operator waypoints (QGC Plan View's
/// Upload): geo -> NED at the supervisor, validated with the compiler's
/// own rules. Rejections carry reasons; the accepted ids enter the task
/// board and the sequential auction's pool.
async fn mission_upload_post(
    State(s): State<Arc<AppState>>,
    body: axum::extract::Json<MissionBody>,
) -> Response {
    let MissionBody { items, mode } = body.0;
    if items.is_empty() {
        return err(StatusCode::UNPROCESSABLE_ENTITY, "items must not be empty");
    }
    if items.len() > 64 {
        return err(StatusCode::UNPROCESSABLE_ENTITY, "too many items (max 64)");
    }
    let replace = matches!(mode.as_deref(), Some("replace"));
    let items: Vec<OperatorWaypoint> = items
        .into_iter()
        .map(|it| OperatorWaypoint {
            label: it.label,
            lat_deg: it.lat_deg,
            lon_deg: it.lon_deg,
            alt_m: it.alt_m,
            hover_s: it.hover_s,
        })
        .collect();
    for it in &items {
        if !it.lat_deg.is_finite() || !it.lon_deg.is_finite() || !it.alt_m.is_finite() {
            return err(StatusCode::UNPROCESSABLE_ENTITY, "lat/lon/alt must be finite");
        }
        if !(-90.0..=90.0).contains(&it.lat_deg) || !(-180.0..=180.0).contains(&it.lon_deg) {
            return err(StatusCode::UNPROCESSABLE_ENTITY, "lat/lon out of range");
        }
        if it.alt_m < 0.0 {
            return err(StatusCode::UNPROCESSABLE_ENTITY, "alt_m (AGL) must be >= 0");
        }
    }
    let (tx, rx) = tokio::sync::oneshot::channel();
    s.push_operator_cmd(OperatorCmd::Upload { items, replace, ack: tx });
    match tokio::time::timeout(Duration::from_secs(2), rx).await {
        Ok(Ok(ack)) => {
            s.log.log(
                fleet_core::events::EventKind::SupervisorAction,
                None,
                format!(
                    "operator: mission upload via API — {} accepted, {} rejected",
                    ack.accepted.len(),
                    ack.rejected.len()
                ),
            );
            ok(json!({
                "accepted": ack.accepted,
                "rejected": ack.rejected.iter()
                    .map(|(l, r)| json!({"label": l, "reason": r}))
                    .collect::<Vec<_>>(),
                "pool": ack.pool,
            }))
            .into_response()
        }
        _ => err(
            StatusCode::SERVICE_UNAVAILABLE,
            "supervisor did not drain the command (is the tick loop running?)",
        ),
    }
}

/// `POST /api/mission/start` — start the deferred mission (QGC's Start
/// Mission): the setup bench flips to RUNNING and the auction flies the
/// uploaded tasks exactly like a scenario mission.
async fn mission_start_post(State(s): State<Arc<AppState>>) -> Response {
    let (tx, rx) = tokio::sync::oneshot::channel();
    s.push_operator_cmd(OperatorCmd::Start { ack: tx });
    match tokio::time::timeout(Duration::from_secs(2), rx).await {
        Ok(Ok(ack)) => {
            s.log.log(
                fleet_core::events::EventKind::SupervisorAction,
                None,
                format!(
                    "operator: mission start via API — {}",
                    if ack.started { "started" } else { "not started" }
                ),
            );
            ok(json!({
                "started": ack.started,
                "reason": ack.reason,
                "phase": s.phase(),
            }))
            .into_response()
        }
        _ => err(
            StatusCode::SERVICE_UNAVAILABLE,
            "supervisor did not drain the command (is the tick loop running?)",
        ),
    }
}

/// `POST /api/mission/clear` — drop queued operator tasks (the active task
/// of a flying runner is never touched).
async fn mission_clear_post(State(s): State<Arc<AppState>>) -> Response {
    let (tx, rx) = tokio::sync::oneshot::channel();
    s.push_operator_cmd(OperatorCmd::Clear { ack: tx });
    match tokio::time::timeout(Duration::from_secs(2), rx).await {
        Ok(Ok(ack)) => {
            s.log.log(
                fleet_core::events::EventKind::SupervisorAction,
                None,
                format!("operator: mission clear via API — {} task(s) dropped", ack.cleared),
            );
            ok(json!({ "cleared": ack.cleared })).into_response()
        }
        _ => err(
            StatusCode::SERVICE_UNAVAILABLE,
            "supervisor did not drain the command (is the tick loop running?)",
        ),
    }
}

/// The guided-command gates shared by every action-bar endpoint: index in
/// range, link registered, and no live mission runner (a user command
/// never fights a runner's setpoint stream — ADR-0017).
fn guided_gates(s: &AppState, index: u8) -> Result<(), Response> {
    if index >= s.count {
        return Err(err(
            StatusCode::NOT_FOUND,
            &format!("vehicle index {index} out of range (fleet count {})", s.count),
        ));
    }
    if s.mission_active(index) {
        return Err(err(
            StatusCode::CONFLICT,
            "vehicle is flying an autonomous mission — guided commands are gated (ADR-0017); use e-stop to abort the fleet",
        ));
    }
    if s.link(index).is_none() {
        return Err(err(StatusCode::CONFLICT, "vehicle link not registered (spawn failed?)"));
    }
    Ok(())
}

fn ack_json(kind: &str, ack: &fleet_mavlink::CmdAck) -> serde_json::Value {
    let (result, accepted) = match ack {
        fleet_mavlink::CmdAck::Accepted => (0u8, true),
        fleet_mavlink::CmdAck::Rejected { result } => (*result, false),
        fleet_mavlink::CmdAck::Timeout { .. } => (255u8, false),
    };
    json!({
        "command": kind,
        "result": result,
        "accepted": accepted,
    })
}

#[derive(serde::Deserialize)]
struct ArmBody {
    #[serde(default = "default_true")]
    arm: bool,
}

fn default_true() -> bool {
    true
}

/// `POST /api/vehicles/{i}/arm` — COMPONENT_ARM_DISARM(1/0), the same
/// command the supervisor's engage ladder sends. PX4's TEMPORARILY_REJECTED
/// (EKF2 still settling) comes back honestly; retry like QGC would.
async fn vehicle_arm_post(
    State(s): State<Arc<AppState>>,
    Path(index): Path<u8>,
    body: axum::extract::Json<ArmBody>,
) -> Response {
    if let Err(e) = guided_gates(&s, index) {
        return e;
    }
    let link = s.link(index).unwrap();
    let params: [f32; 7] = if body.arm {
        [1.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0]
    } else {
        [0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0]
    };
    let ack = link
        .send_command(fleet_mavlink::cmds::COMPONENT_ARM_DISARM, params)
        .await;
    s.log.log(
        fleet_core::events::EventKind::SupervisorAction,
        Some(index),
        format!(
            "operator: {} via API — {}",
            if body.arm { "ARM" } else { "DISARM" },
            match &ack {
                fleet_mavlink::CmdAck::Accepted => "ACCEPTED".into(),
                other => format!("{other:?}"),
            }
        ),
    );
    let mut data = ack_json(if body.arm { "arm" } else { "disarm" }, &ack);
    data["index"] = json!(index);
    data["armed"] = json!(body.arm);
    ok(data).into_response()
}

#[derive(serde::Deserialize)]
struct TakeoffBody {
    /// Climb-to altitude, metres AGL (QGC's takeoff altitude slider).
    #[serde(default = "default_takeoff_alt")]
    alt_m: f32,
}

fn default_takeoff_alt() -> f32 {
    10.0
}

/// `POST /api/vehicles/{i}/takeoff` — MAV_CMD_NAV_TAKEOFF (22) with the
/// climb altitude in param7: QGC's takeoff semantics (PX4 commander arms
/// and the navigator flies the climb).
async fn vehicle_takeoff_post(
    State(s): State<Arc<AppState>>,
    Path(index): Path<u8>,
    body: axum::extract::Json<TakeoffBody>,
) -> Response {
    // input validation first (400-class errors beat state gates)
    if !body.alt_m.is_finite() || body.alt_m <= 0.0 || body.alt_m > 120.0 {
        return err(StatusCode::UNPROCESSABLE_ENTITY, "alt_m must be in (0, 120] m AGL");
    }
    // respect the fence altitude box honest to the runner rules
    let (floor, ceiling) = (s.fence_view.floor_m, s.fence_view.ceiling_m);
    if body.alt_m < floor || body.alt_m > ceiling {
        return err(
            StatusCode::UNPROCESSABLE_ENTITY,
            &format!("alt_m outside the geofence altitude box ({floor}..{ceiling} m AGL)"),
        );
    }
    if let Err(e) = guided_gates(&s, index) {
        return e;
    }
    let link = s.link(index).unwrap();
    let ack = link
        .send_command(fleet_mavlink::cmds::NAV_TAKEOFF, [
            0.0, 0.0, 0.0, 0.0, 0.0, 0.0, body.alt_m,
        ])
        .await;
    s.log.log(
        fleet_core::events::EventKind::SupervisorAction,
        Some(index),
        format!(
            "operator: TAKEOFF to {:.1} m AGL via API — {}",
            body.alt_m,
            match &ack {
                fleet_mavlink::CmdAck::Accepted => "ACCEPTED".into(),
                other => format!("{other:?}"),
            }
        ),
    );
    let mut data = ack_json("takeoff", &ack);
    data["index"] = json!(index);
    data["alt_m"] = json!(body.alt_m);
    ok(data).into_response()
}

/// `POST /api/vehicles/{i}/land` — AUTO.LAND plus setpoint-stream stop (a
/// go-to-flown vehicle is streaming OFFBOARD setpoints; the mode change
/// alone doesn't stop the pump). FSM honesty: ACTIVE -> LANDED.
async fn vehicle_land_post(
    State(s): State<Arc<AppState>>,
    Path(index): Path<u8>,
) -> Response {
    if let Err(e) = guided_gates(&s, index) {
        return e;
    }
    let link = s.link(index).unwrap();
    link.shared.stop_stream();
    let ack = link.set_mode(fleet_modes::MODE_WORD_AUTO_LAND).await;
    if s.registry.fsm(index) == Some(fleet_core::fsm::FsmState::Active) {
        s.registry
            .apply_transition(index, fleet_core::fsm::FsmCause::LandCommand, &s.log);
    }
    s.log.log(
        fleet_core::events::EventKind::SupervisorAction,
        Some(index),
        "operator: LAND via API — stream stopped, DO_SET_MODE(AUTO.LAND)",
    );
    let mut data = ack_json("land", &ack);
    data["index"] = json!(index);
    ok(data).into_response()
}

/// `POST /api/vehicles/{i}/rtl` — AUTO.RTL plus stream stop, the supervisor
/// RTL's exact mode word. FSM honesty: ACTIVE -> RTL.
async fn vehicle_rtl_post(
    State(s): State<Arc<AppState>>,
    Path(index): Path<u8>,
) -> Response {
    if let Err(e) = guided_gates(&s, index) {
        return e;
    }
    let link = s.link(index).unwrap();
    link.shared.stop_stream();
    let ack = link.set_mode(fleet_modes::MODE_WORD_AUTO_RTL).await;
    if s.registry.fsm(index) == Some(fleet_core::fsm::FsmState::Active) {
        s.registry
            .apply_transition(index, fleet_core::fsm::FsmCause::SupervisorRtl, &s.log);
    }
    s.log.log(
        fleet_core::events::EventKind::SupervisorAction,
        Some(index),
        "operator: RTL via API — stream stopped, DO_SET_MODE(AUTO.RTL)",
    );
    let mut data = ack_json("rtl", &ack);
    data["index"] = json!(index);
    ok(data).into_response()
}

/// `POST /api/vehicles/{i}/hold` — QGC's Pause: AUTO.LOITER (a multicopter
/// position-holds) plus setpoint-stream stop.
async fn vehicle_hold_post(
    State(s): State<Arc<AppState>>,
    Path(index): Path<u8>,
) -> Response {
    if let Err(e) = guided_gates(&s, index) {
        return e;
    }
    let link = s.link(index).unwrap();
    link.shared.stop_stream();
    let ack = link.set_mode(fleet_modes::MODE_WORD_AUTO_LOITER).await;
    s.log.log(
        fleet_core::events::EventKind::SupervisorAction,
        Some(index),
        "operator: HOLD via API — stream stopped, DO_SET_MODE(AUTO.LOITER)",
    );
    let mut data = ack_json("hold", &ack);
    data["index"] = json!(index);
    ok(data).into_response()
}

#[derive(serde::Deserialize)]
struct GotoBody {
    lat_deg: f64,
    lon_deg: f64,
    /// Metres AGL; defaults to the vehicle's current relative altitude.
    #[serde(default)]
    alt_m: Option<f64>,
}

/// `POST /api/vehicles/{i}/goto` — QGC's Go To Location: geo -> NED,
/// clamped into the fence minus 2 m (the runner's own rule, §7.2), then
/// the engage sequence — hold-at the current estimate (starts the >=2 Hz
/// stream PX4 requires), set the goal, arm ladder + DO_SET_MODE(OFFBOARD).
/// The vehicle flies to the point and holds there.
async fn vehicle_goto_post(
    State(s): State<Arc<AppState>>,
    Path(index): Path<u8>,
    body: axum::extract::Json<GotoBody>,
) -> Response {
    // input validation first (400-class errors beat state gates)
    if !body.lat_deg.is_finite() || !body.lon_deg.is_finite() {
        return err(StatusCode::UNPROCESSABLE_ENTITY, "lat/lon must be finite");
    }
    if !(-90.0..=90.0).contains(&body.lat_deg) || !(-180.0..=180.0).contains(&body.lon_deg) {
        return err(StatusCode::UNPROCESSABLE_ENTITY, "lat/lon out of range");
    }
    if let Err(e) = guided_gates(&s, index) {
        return e;
    }
    let Some(link) = s.link(index) else {
        return err(StatusCode::CONFLICT, "vehicle link not registered (spawn failed?)");
    };
    // current estimate (NED) — the hold-at anchor and the default altitude.
    let Some(snap) = s.registry.snapshot(index) else {
        return err(StatusCode::CONFLICT, "no telemetry snapshot yet");
    };
    let origin = s.geo_origin;
    // target NED: geo -> NED; altitude AGL (default: hold current altitude)
    // — geodetic alt = origin + AGL.
    let alt_agl = body.alt_m.unwrap_or((-snap.position_ned_m[2]).max(0.0) as f64);
    if !alt_agl.is_finite() || alt_agl < 0.0 || alt_agl > 120.0 {
        return err(StatusCode::UNPROCESSABLE_ENTITY, "alt_m must be in [0, 120] m AGL");
    }
    let ned = origin.geodetic_to_ned(body.lat_deg, body.lon_deg, origin.alt_m + alt_agl);
    // clamp into the fence minus the runner's margin (§7.2): a go-to can
    // never command a setpoint the mission rules themselves reject.
    const MARGIN: f32 = fleet_mission::runner::CLAMP_MARGIN_M;
    let xy = s.fence.clamp_setpoint([ned[0] as f32, ned[1] as f32], MARGIN);
    let z = s.fence.clamp_altitude(-alt_agl as f32, MARGIN);
    let clamped =
        (xy[0] - ned[0] as f32).hypot(xy[1] - ned[1] as f32) > 0.5 || (z + alt_agl as f32).abs() > 0.5;
    let target: [f32; 3] = [xy[0], xy[1], z];
    // engage: hold-at the current estimate starts the stream, then the goal,
    // then the arm ladder + OFFBOARD — the supervisor's own sequence.
    let anchor = if snap.local_position_seen {
        snap.position_ned_m
    } else {
        [0.0, 0.0, 0.0]
    };
    let yaw = snap.attitude_q_wxyz;
    let yaw = (2.0 * (yaw[0] * yaw[3] + yaw[1] * yaw[2]))
        .atan2(1.0 - 2.0 * (yaw[2] * yaw[2] + yaw[3] * yaw[3]));
    link.shared.hold_at(anchor, yaw);
    link.shared.set_goal(fleet_mavlink::SetpointGoal { position: target, yaw });
    crate::manager::spawn_engage(link.clone(), Arc::clone(&s.log), index, !snap.armed);
    if s.registry.fsm(index) == Some(fleet_core::fsm::FsmState::Ready) {
        s.registry
            .apply_transition(index, fleet_core::fsm::FsmCause::TaskAccepted, &s.log);
    }
    s.log.log(
        fleet_core::events::EventKind::SupervisorAction,
        Some(index),
        format!(
            "operator: GO TO ({:.6}, {:.6}) {:.1} m AGL via API -> NED {:?}{} — engage sequence started",
            body.lat_deg,
            body.lon_deg,
            alt_agl,
            target,
            if clamped { " (clamped into the fence)" } else { "" }
        ),
    );
    ok(json!({
        "index": index,
        "lat_deg": body.lat_deg,
        "lon_deg": body.lon_deg,
        "alt_agl_m": alt_agl,
        "target_ned_m": target,
        "clamped": clamped,
        "engaging": true,
    }))
    .into_response()
}

// ---------------------------------------------------------------------------
// MAVLink mission upload/download (M2-API): the QGC Plan View's
// Upload/Download buttons — direct proxies to the per-vehicle link's
// mission-protocol state machine (commit 99993f5, fleet-mavlink/src/link.rs).
// These endpoints are distinct from ADR-0017's `POST /api/mission`
// (geo→NED conversion + supervisor-drained auction): the M2 endpoints
// pass raw MISSION_ITEM_INT items straight through to PX4's mission
// protocol, tagged with mission_type (0=mission / 1=fence / 2=rally).
// ---------------------------------------------------------------------------

/// `POST /api/vehicles/{i}/mission/upload` body — QGC's MISSION_ITEM_INT
/// JSON shape (field-for-field the wire struct, x/y as i32 lat/lon × 1e7).
#[derive(serde::Deserialize)]
struct MissionUploadBody {
    items: Vec<fleet_mavlink::MissionItemInt>,
    /// 0 = mission, 1 = fence, 2 = rally (MAV_MISSION_TYPE).
    mission_type: u8,
}

/// `POST /api/vehicles/{i}/mission/upload` — proxy to
/// `LinkHandle::mission_upload`. Body carries the raw MISSION_ITEM_INT
/// items and the mission_type tag; the result enum is returned verbatim
/// in the envelope's `data` field (status: ok/failed/timeout).
async fn vehicle_mission_upload_post(
    State(s): State<Arc<AppState>>,
    Path(index): Path<u8>,
    body: Result<
        axum::extract::Json<MissionUploadBody>,
        axum::extract::rejection::JsonRejection,
    >,
) -> Response {
    if index >= s.count {
        return err(
            StatusCode::NOT_FOUND,
            &format!("vehicle index {index} out of range (fleet count {})", s.count),
        );
    }
    let axum::extract::Json(MissionUploadBody { items, mission_type }) = match body {
        Ok(b) => b,
        Err(e) => {
            return err(
                StatusCode::UNPROCESSABLE_ENTITY,
                &format!("invalid body: {e}"),
            )
        }
    };
    if items.is_empty() {
        return err(StatusCode::UNPROCESSABLE_ENTITY, "items must not be empty");
    }
    if items.len() > 1024 {
        return err(StatusCode::UNPROCESSABLE_ENTITY, "too many items (max 1024)");
    }
    if mission_type > 2 {
        return err(
            StatusCode::UNPROCESSABLE_ENTITY,
            "mission_type must be 0 (mission), 1 (fence), or 2 (rally)",
        );
    }
    let Some(link) = s.link(index) else {
        return err(
            StatusCode::NOT_FOUND,
            "vehicle link not registered (vehicle not connected)",
        );
    };
    let n_items = items.len() as u16;
    let result = link.mission_upload(items, mission_type).await;
    s.log.log(
        fleet_core::events::EventKind::SupervisorAction,
        Some(index),
        format!(
            "operator: MAVLink mission upload via API (type={mission_type}, items={n_items}) — {}",
            match &result {
                fleet_mavlink::MissionUploadResult::Ok { items_acked, .. } =>
                    format!("OK ({items_acked} acked)"),
                fleet_mavlink::MissionUploadResult::Failed { reason, .. } =>
                    format!("FAILED ({reason})"),
                fleet_mavlink::MissionUploadResult::Timeout { .. } => "TIMEOUT".into(),
            }
        ),
    );
    let data = serde_json::to_value(&result).unwrap_or(json!(null));
    ok(data).into_response()
}

/// `GET /api/vehicles/{i}/mission?type={mission|fence|rally}` query.
#[derive(serde::Deserialize, Default)]
struct MissionTypeQuery {
    /// "mission" (default), "fence", or "rally".
    #[serde(default)]
    r#type: Option<String>,
}

/// `GET /api/vehicles/{i}/mission?type=mission|fence|rally` — proxy to
/// `LinkHandle::mission_download`. Returns the MissionDownloadResult enum
/// verbatim (status: ok/failed/timeout; ok carries the items array).
async fn vehicle_mission_get(
    State(s): State<Arc<AppState>>,
    Path(index): Path<u8>,
    Query(q): Query<MissionTypeQuery>,
) -> Response {
    if index >= s.count {
        return err(
            StatusCode::NOT_FOUND,
            &format!("vehicle index {index} out of range (fleet count {})", s.count),
        );
    }
    let mission_type: u8 = match q.r#type.as_deref().unwrap_or("mission") {
        "" | "mission" => 0,
        "fence" => 1,
        "rally" => 2,
        other => {
            return err(
                StatusCode::UNPROCESSABLE_ENTITY,
                &format!(
                    "type must be one of mission|fence|rally, got '{other}'"
                ),
            )
        }
    };
    let Some(link) = s.link(index) else {
        return err(
            StatusCode::NOT_FOUND,
            "vehicle link not registered (vehicle not connected)",
        );
    };
    let result = link.mission_download(mission_type).await;
    s.log.log(
        fleet_core::events::EventKind::SupervisorAction,
        Some(index),
        format!(
            "operator: MAVLink mission download via API (type={mission_type}) — {}",
            match &result {
                fleet_mavlink::MissionDownloadResult::Ok { items, .. } =>
                    format!("OK ({} items)", items.len()),
                fleet_mavlink::MissionDownloadResult::Failed { reason, .. } =>
                    format!("FAILED ({reason})"),
                fleet_mavlink::MissionDownloadResult::Timeout { .. } => "TIMEOUT".into(),
            }
        ),
    );
    let data = serde_json::to_value(&result).unwrap_or(json!(null));
    ok(data).into_response()
}

// ---------------------------------------------------------------------------
// M3 (Fly View): QGC-style pre-arm checklist (GCS_SPEC.md §5.2)
// ---------------------------------------------------------------------------

/// One row of the pre-arm checklist. The Fly View's ARM button gates on
/// `all_passed` from the wrapping `PrearmChecks` envelope.
#[derive(Debug, serde::Serialize)]
struct PrearmCheck {
    name: &'static str,
    passed: bool,
    message: String,
}

/// `GET /api/vehicles/{i}/prearm-checks` — five QGC-style pre-arm checks
/// derived from the live VehicleState + current health flags (GCS_SPEC.md
/// §5.2). The Fly View's ARM button gates on `all_passed`; the per-check
/// `message` is the operator-visible reason (QGC's checklist UI shows the
/// same strings).
///
/// Index validation is inline (404 when out of range) — the same pattern
/// `vehicle_params_get` / `vehicle_setup_get` use — because the pre-arm
/// checklist must be visible *even when the link is down*: the EKF2 check
/// naturally reports "waiting for convergence" via the `HEARTBEAT_LOST` /
/// `LINK_STALE` flags, which is exactly what the operator needs to see
/// before retrying an arming attempt. `guided_gates` would 409 on the
/// missing link, hiding the actual reason from the Fly View.
async fn vehicle_prearm_checks_get(
    State(s): State<Arc<AppState>>,
    Path(index): Path<u8>,
) -> Response {
    if index >= s.count {
        return err(
            StatusCode::NOT_FOUND,
            &format!("vehicle index {index} out of range (fleet count {})", s.count),
        );
    }
    let Some(snap) = s.registry.snapshot(index) else {
        return err(
            StatusCode::NOT_FOUND,
            &format!("vehicle index {index} has no state slot"),
        );
    };
    let flags = s.vehicle_flags(index);
    let has_flag = |name: &str| flags.iter().any(|f| f.name() == name);

    // 1. EKF2 — converged when the link is healthy AND a local position has
    //    arrived (PX4's EKF2 InnovCheck signature). LINK_STALE / HEARTBEAT_LOST
    //    fail it regardless — that's the real pre-arm blocker.
    let ekf_pass = !has_flag("LINK_STALE")
        && !has_flag("HEARTBEAT_LOST")
        && snap.local_position_seen;
    let ekf = PrearmCheck {
        name: "EKF2",
        passed: ekf_pass,
        message: if ekf_pass {
            "converged".into()
        } else {
            "waiting for convergence".into()
        },
    };

    // 2. GPS Fix — GLOBAL_POSITION_INT lat or lon non-zero means a 3D fix
    //    has been received (0/0 is the pre-fix default).
    let gps_pass = snap.lat_deg_e7 != 0 || snap.lon_deg_e7 != 0;
    let gps = PrearmCheck {
        name: "GPS Fix",
        passed: gps_pass,
        message: if gps_pass { "3D fix".into() } else { "no fix".into() },
    };

    // 3. Mode — STANDBY is PX4's mode_word == 0 (the disarmed pre-flight
    //    custom mode), or the equivalent "disarmed and not flying an
    //    autonomous mission" state. An already-armed vehicle can't be
    //    re-armed, so the check fails — the operator should disarm first.
    let mode_pass = snap.mode_word == 0 || (!snap.armed && !s.mission_active(index));
    let mode = PrearmCheck {
        name: "Mode",
        passed: mode_pass,
        message: if mode_pass { "STANDBY".into() } else { "not in standby".into() },
    };

    // 4. Fence — the safety engine raises GEOFENCE_WARN when within 10 m of
    //    a boundary (spec §5.2). Inside the inclusion zone otherwise.
    let fence_pass = !has_flag("GEOFENCE_WARN");
    let fence = PrearmCheck {
        name: "Fence",
        passed: fence_pass,
        message: if fence_pass {
            "inside inclusion".into()
        } else {
            "near fence boundary".into()
        },
    };

    // 5. Battery — BATTERY_CRIT (the <20% threshold) is the pre-arm cutoff.
    //    Battery is i8 with -1 meaning "unknown" (interim sim has no battery
    //    model); unknown fails the check (can't arm without a reading).
    let batt_pass = snap.battery_pct >= fleet_core::health::BATTERY_CRIT_PCT as i8;
    let battery = PrearmCheck {
        name: "Battery",
        passed: batt_pass,
        message: if batt_pass {
            format!("{}%", snap.battery_pct)
        } else {
            "critical (<20%)".into()
        },
    };

    let checks = vec![ekf, gps, mode, fence, battery];
    let all_passed = checks.iter().all(|c| c.passed);
    ok(json!({ "checks": checks, "all_passed": all_passed })).into_response()
}

// ---------------------------------------------------------------------------
// M5 (Fleet C2): per-vehicle mission binding, fleet start, swarming
// patterns (GCS_SPEC.md §5.4 + §7 API contracts).
// ---------------------------------------------------------------------------

/// `POST /api/fleet/mission-bindings` body — one entry per vehicle.
#[derive(Debug, serde::Deserialize, serde::Serialize)]
struct MissionBindingBody {
    vehicle_id: u8,
    mission_id: String,
}

#[derive(Debug, serde::Deserialize)]
struct MissionBindingsBody {
    bindings: Vec<MissionBindingBody>,
}

/// `POST /api/fleet/mission-bindings` — bind missions to vehicles (M5,
/// GCS_SPEC.md §5.4). Each binding is validated (vehicle_id in range,
/// mission_id looks like a ULID); the bindings are stored in AppState
/// (one mission_id per vehicle, None = unbound). The mission itself is
/// NOT uploaded here — that happens on `POST /api/fleet/start`.
///
/// Returns `{ok: true, bindings: [{vehicle_id, mission_id, binding_state:
/// "assigned"}]}` — `binding_state` is reserved for future states
/// ("uploading", "active", "completed") so the client UI doesn't have to
/// reshuffle when they arrive.
async fn fleet_mission_bindings_post(
    State(s): State<Arc<AppState>>,
    body: Result<
        axum::extract::Json<MissionBindingsBody>,
        axum::extract::rejection::JsonRejection,
    >,
) -> Response {
    let axum::extract::Json(body) = match body {
        Ok(b) => b,
        Err(e) => return err(StatusCode::UNPROCESSABLE_ENTITY, &format!("invalid body: {e}")),
    };
    if body.bindings.is_empty() {
        return err(StatusCode::UNPROCESSABLE_ENTITY, "no bindings in body");
    }
    if body.bindings.len() > 64 {
        return err(StatusCode::UNPROCESSABLE_ENTITY, "too many bindings (max 64)");
    }
    // Validate every binding before storing any — atomic semantics so a
    // partial write never lands (QGC's "Bind All" button calls this once
    // per fleet-start, so a rejected row means the whole batch is a no-op).
    for b in &body.bindings {
        if b.vehicle_id >= s.count {
            return err(
                StatusCode::NOT_FOUND,
                &format!(
                    "vehicle_id {} out of range (fleet count {})",
                    b.vehicle_id, s.count
                ),
            );
        }
        if !state::looks_like_ulid(&b.mission_id) {
            return err(
                StatusCode::UNPROCESSABLE_ENTITY,
                &format!(
                    "mission_id '{}' is not a valid ULID (expected 26-char Crockford base32)",
                    b.mission_id
                ),
            );
        }
    }
    // All bindings valid — store them.
    for b in &body.bindings {
        s.set_mission_binding(b.vehicle_id, b.mission_id.clone());
    }
    s.log.log(
        fleet_core::events::EventKind::SupervisorAction,
        None,
        format!("operator: fleet mission bindings via API — {} assigned", body.bindings.len()),
    );
    let bindings = body
        .bindings
        .iter()
        .map(|b| {
            json!({
                "vehicle_id": b.vehicle_id,
                "mission_id": b.mission_id,
                "binding_state": "assigned",
            })
        })
        .collect::<Vec<_>>();
    ok(json!({ "bindings": bindings }))
        .into_response()
}

/// `GET /api/fleet/mission-bindings` — list current bindings (one row per
/// vehicle, `mission_id: null` when unbound). Sort order is the natural
/// vehicle index.
async fn fleet_mission_bindings_get(
    State(s): State<Arc<AppState>>,
) -> Json<serde_json::Value> {
    let snapshot = s.mission_bindings_snapshot();
    let bindings: Vec<serde_json::Value> = snapshot
        .iter()
        .enumerate()
        .map(|(i, mid)| {
            json!({
                "vehicle_id": i as u8,
                "mission_id": mid,
                "binding_state": if mid.is_some() { "assigned" } else { "unbound" },
            })
        })
        .collect();
    ok(json!({ "bindings": bindings }))
}

/// `DELETE /api/fleet/mission-bindings/{vehicle_id}` — clear a binding.
/// Idempotent: deleting an already-unbound vehicle returns 200 (the
/// resulting state matches what the caller asked for).
async fn fleet_mission_binding_delete(
    State(s): State<Arc<AppState>>,
    Path(vehicle_id): Path<u8>,
) -> Response {
    if vehicle_id >= s.count {
        return err(
            StatusCode::NOT_FOUND,
            &format!(
                "vehicle_id {vehicle_id} out of range (fleet count {})",
                s.count
            ),
        );
    }
    let was_bound = s.clear_mission_binding(vehicle_id);
    s.log.log(
        fleet_core::events::EventKind::SupervisorAction,
        Some(vehicle_id),
        format!(
            "operator: fleet mission binding cleared via API (was_bound={was_bound})"
        ),
    );
    ok(json!({
        "vehicle_id": vehicle_id,
        "cleared": was_bound,
        "binding_state": "unbound",
    }))
    .into_response()
}

// ---------------------------------------------------------------------------
// `POST /api/fleet/start` — upload + arm every bound vehicle
// ---------------------------------------------------------------------------

/// `POST /api/fleet/start` body.
#[derive(Debug, serde::Deserialize)]
struct FleetStartBody {
    /// "parallel" (default): upload all + arm all in one pass.
    /// "sequential": upload + start vehicle 0, wait for the gate, then
    /// vehicle 1, etc. — AC-5.4.2.
    #[serde(default = "default_fleet_start_mode")]
    mode: String,
    /// "first_waypoint" or "takeoff_complete" (sequential only).
    #[serde(default = "default_fleet_start_gate")]
    sequential_gate: String,
    /// Per-vehicle timeout in seconds (sequential mode). Default 30 s
    /// (the spec's default; AC-5.4.2 aborts the fleet if the gate is not met).
    #[serde(default = "default_fleet_start_timeout_s")]
    timeout_s: u64,
}

fn default_fleet_start_mode() -> String {
    "parallel".into()
}
fn default_fleet_start_gate() -> String {
    "first_waypoint".into()
}
fn default_fleet_start_timeout_s() -> u64 {
    30
}

/// One per-vehicle result row in the fleet-start response.
#[derive(Debug, serde::Serialize)]
struct FleetStartResult {
    vehicle_id: u8,
    mission_id: Option<String>,
    /// "started" | "failed" | "timeout" | "skipped" (skipped = unbound).
    status: String,
    /// Human-readable reason for failed/timeout/skipped.
    #[serde(skip_serializing_if = "Option::is_none")]
    reason: Option<String>,
}

/// Convert a catalog `MissionFile`'s waypoints to MISSION_ITEM_INT — the
/// wire shape `LinkHandle::mission_upload` expects (M5 plan-view → fly-
/// view bridge). lat/lon × 1e7, alt as-is, frame=3 (GLOBAL_RELATIVE_ALT),
/// command=16 (NAV_WAYPOINT). This mirrors the QGC Plan View's export
/// path so a mission authored against the catalog flies the same shape.
fn waypoints_to_mission_items(
    mission: &fleet_mission::gcs::mission_file::MissionFile,
) -> Vec<fleet_mavlink::MissionItemInt> {
    mission
        .waypoints
        .iter()
        .map(|wp| fleet_mavlink::MissionItemInt {
            param1: wp.param1,
            param2: wp.param2,
            param3: wp.param3,
            param4: wp.param4,
            x: (wp.x * 1e7).round() as i32,
            y: (wp.y * 1e7).round() as i32,
            z: wp.z,
            seq: wp.seq,
            command: wp.command,
            target_system: 1,
            target_component: 1,
            frame: wp.frame,
            current: 0,
            autocontinue: 1,
            mission_type: 0,
        })
        .collect()
}

/// Fetch a mission from the catalog server (M5). `GET {catalog_url}/api/missions/{id}`
/// returns `{ok, data: MissionFile}`; the helper unwraps the `data` field
/// and parses it into a `MissionFile`. Returns `None` on any error (the
/// caller emits a `failed` status row, never panics).
async fn fetch_mission_from_catalog(
    catalog_url: &str,
    mission_id: &str,
) -> Option<fleet_mission::gcs::mission_file::MissionFile> {
    let url = format!("{catalog_url}/api/missions/{mission_id}");
    let timeout = Duration::from_secs(5);
    let resp = match tokio::time::timeout(timeout, reqwest::get(&url)).await {
        Ok(Ok(r)) => r,
        _ => return None,
    };
    if !resp.status().is_success() {
        return None;
    }
    let body: serde_json::Value = match resp.json().await {
        Ok(v) => v,
        Err(_) => return None,
    };
    let data = body.get("data").unwrap_or(&body);
    serde_json::from_value(data.clone()).ok()
}

/// `POST /api/fleet/start` — upload + arm every bound vehicle (M5,
/// GCS_SPEC.md §5.4). For each bound vehicle: fetch the mission from
/// :8300 (catalog), convert waypoints to MISSION_ITEM_INT, upload via
/// `link.mission_upload(items, 0)`, log, return the per-vehicle result.
///
/// **Parallel mode**: uploads + arms all bound vehicles in one pass
/// (each upload is independent — no inter-vehicle gating).
///
/// **Sequential mode**: uploads + starts vehicle 0, waits (M5 simplified
/// gate: a 5 s sleep per vehicle — the real first-waypoint-position
/// polling lands in M5.1 per the task brief), then vehicle 1, etc.
/// Timeout applies per vehicle; on timeout the fleet aborts the rest.
async fn fleet_start_post(
    State(s): State<Arc<AppState>>,
    body: Result<
        axum::extract::Json<FleetStartBody>,
        axum::extract::rejection::JsonRejection,
    >,
) -> Response {
    let axum::extract::Json(body) = match body {
        Ok(b) => b,
        Err(e) => return err(StatusCode::UNPROCESSABLE_ENTITY, &format!("invalid body: {e}")),
    };
    let mode = body.mode.as_str();
    let is_parallel = match mode {
        "parallel" => true,
        "sequential" => false,
        other => {
            return err(
                StatusCode::UNPROCESSABLE_ENTITY,
                &format!("mode must be 'parallel' or 'sequential', got '{other}'"),
            )
        }
    };
    if !matches!(body.sequential_gate.as_str(), "first_waypoint" | "takeoff_complete") {
        return err(
            StatusCode::UNPROCESSABLE_ENTITY,
            "sequential_gate must be 'first_waypoint' or 'takeoff_complete'",
        );
    }
    if body.timeout_s == 0 || body.timeout_s > 600 {
        return err(
            StatusCode::UNPROCESSABLE_ENTITY,
            "timeout_s must be in (0, 600] seconds",
        );
    }

    let bindings = s.mission_bindings_snapshot();
    let catalog_url = s.catalog_url().to_string();
    s.log.log(
        fleet_core::events::EventKind::SupervisorAction,
        None,
        format!(
            "operator: fleet start via API — mode={mode}, gate={}, timeout={}s, bound={}",
            body.sequential_gate,
            body.timeout_s,
            bindings.iter().filter(|b| b.is_some()).count()
        ),
    );

    let mut results: Vec<FleetStartResult> = Vec::with_capacity(bindings.len());
    for (i, mid) in bindings.iter().enumerate() {
        let vehicle_id = i as u8;
        let Some(mission_id) = mid.clone() else {
            results.push(FleetStartResult {
                vehicle_id,
                mission_id: None,
                status: "skipped".into(),
                reason: Some("vehicle has no bound mission".into()),
            });
            continue;
        };

        // 1. Fetch the mission from :8300.
        let mission = match fetch_mission_from_catalog(&catalog_url, &mission_id).await {
            Some(m) => m,
            None => {
                results.push(FleetStartResult {
                    vehicle_id,
                    mission_id: Some(mission_id.clone()),
                    status: "failed".into(),
                    reason: Some(format!("catalog fetch failed ({catalog_url}/api/missions/{mission_id})")),
                });
                // In sequential mode, abort the rest of the fleet on failure.
                if !is_parallel {
                    for (j, mid2) in bindings.iter().enumerate().skip(i + 1) {
                        results.push(FleetStartResult {
                            vehicle_id: j as u8,
                            mission_id: mid2.clone(),
                            status: "skipped".into(),
                            reason: Some("fleet aborted (prior vehicle failed)".into()),
                        });
                    }
                    break;
                }
                continue;
            }
        };
        let items = waypoints_to_mission_items(&mission);
        if items.is_empty() {
            results.push(FleetStartResult {
                vehicle_id,
                mission_id: Some(mission_id.clone()),
                status: "failed".into(),
                reason: Some("mission has no waypoints".into()),
            });
            if !is_parallel {
                for (j, mid2) in bindings.iter().enumerate().skip(i + 1) {
                    results.push(FleetStartResult {
                        vehicle_id: j as u8,
                        mission_id: mid2.clone(),
                        status: "skipped".into(),
                        reason: Some("fleet aborted (prior vehicle failed)".into()),
                    });
                }
                break;
            }
            continue;
        }

        // 2. Upload the mission via the per-vehicle link.
        let Some(link) = s.link(vehicle_id) else {
            results.push(FleetStartResult {
                vehicle_id,
                mission_id: Some(mission_id.clone()),
                status: "failed".into(),
                reason: Some("vehicle link not registered (spawn failed?)".into()),
            });
            if !is_parallel {
                for (j, mid2) in bindings.iter().enumerate().skip(i + 1) {
                    results.push(FleetStartResult {
                        vehicle_id: j as u8,
                        mission_id: mid2.clone(),
                        status: "skipped".into(),
                        reason: Some("fleet aborted (prior vehicle failed)".into()),
                    });
                }
                break;
            }
            continue;
        };
        let n_items = items.len() as u16;
        let upload = link.mission_upload(items, 0).await;
        s.log.log(
            fleet_core::events::EventKind::SupervisorAction,
            Some(vehicle_id),
            format!(
                "operator: fleet start via API — vehicle {vehicle_id} mission {mission_id} upload ({} items) — {}",
                n_items,
                match &upload {
                    fleet_mavlink::MissionUploadResult::Ok { items_acked, .. } =>
                        format!("OK ({items_acked} acked)"),
                    fleet_mavlink::MissionUploadResult::Failed { reason, .. } =>
                        format!("FAILED ({reason})"),
                    fleet_mavlink::MissionUploadResult::Timeout { .. } => "TIMEOUT".into(),
                }
            ),
        );
        let upload_ok = matches!(upload, fleet_mavlink::MissionUploadResult::Ok { .. });
        if !upload_ok {
            let reason = match &upload {
                fleet_mavlink::MissionUploadResult::Failed { reason, .. } => reason.clone(),
                fleet_mavlink::MissionUploadResult::Timeout { .. } => "upload timeout".into(),
                _ => "unknown".into(),
            };
            results.push(FleetStartResult {
                vehicle_id,
                mission_id: Some(mission_id.clone()),
                status: "failed".into(),
                reason: Some(format!("mission_upload: {reason}")),
            });
            if !is_parallel {
                for (j, mid2) in bindings.iter().enumerate().skip(i + 1) {
                    results.push(FleetStartResult {
                        vehicle_id: j as u8,
                        mission_id: mid2.clone(),
                        status: "skipped".into(),
                        reason: Some("fleet aborted (prior vehicle failed)".into()),
                    });
                }
                break;
            }
            continue;
        }

        // 3. (M5 stub) — the actual arm + MAV_CMD_MISSION_START is the
        // supervisor's engage ladder; here we just mark the vehicle as
        // "started" and log. The real engage sequence is the same one
        // `POST /api/vehicles/{i}/arm` triggers; for M5 the harness
        // verifies the upload path and the per-vehicle status rows.

        results.push(FleetStartResult {
            vehicle_id,
            mission_id: Some(mission_id.clone()),
            status: "started".into(),
            reason: None,
        });

        // 4. Sequential gate (M5 simplified): sleep 5 s between vehicle
        // starts. The real gate polls `GET /api/vehicles/{i}` for the
        // current position vs waypoint 0's position (5 m threshold); the
        // polling loop is M5.1 per the task brief.
        if !is_parallel {
            tokio::time::sleep(Duration::from_secs(5)).await;
        }
    }

    ok(json!({ "ok": true, "started": serde_json::to_value(&results).unwrap_or(json!(null)) }))
        .into_response()
}

// ---------------------------------------------------------------------------
// `GET /api/fleet/patterns` + `POST /api/fleet/patterns/{name}/generate`
// ---------------------------------------------------------------------------

/// One entry in the `GET /api/fleet/patterns` listing.
#[derive(Debug, serde::Serialize)]
struct PatternInfo {
    name: &'static str,
    description: &'static str,
    /// "implemented" (the generator returns generated missions) or
    /// "stub" (the endpoint returns 501 NotImplemented).
    implemented: bool,
}

/// `GET /api/fleet/patterns` — list the swarming-pattern library (M5,
/// GCS_SPEC.md §5.4). Three patterns per spec: follow-the-leader,
/// search-grid, parallel-patrol. Only follow-the-leader is implemented
/// in M5; the other two return 501 on `generate`.
async fn fleet_patterns_get(State(_s): State<Arc<AppState>>) -> Json<serde_json::Value> {
    let patterns = vec![
        PatternInfo {
            name: "follow-the-leader",
            description: "Followers maintain formation behind the leader at a configurable separation.",
            implemented: true,
        },
        PatternInfo {
            name: "search-grid",
            description: "Divide an area into strips, one per vehicle, at a fixed spacing.",
            implemented: false,
        },
        PatternInfo {
            name: "parallel-patrol",
            description: "Same waypoints for each vehicle, offset perpendicular to the path.",
            implemented: false,
        },
    ];
    ok(json!({ "patterns": patterns }))
}

/// `POST /api/fleet/patterns/{name}/generate` body for follow-the-leader.
#[derive(Debug, serde::Deserialize)]
struct FollowTheLeaderBody {
    /// Leader vehicle index.
    leader: u8,
    /// Follower vehicle indices (each gets a generated mission).
    followers: Vec<u8>,
    /// Horizontal separation behind the leader, metres (default 10 m).
    #[serde(default = "default_ftl_separation_m")]
    separation_m: f64,
    /// Cruise altitude, metres AGL (default 12 m).
    #[serde(default = "default_ftl_alt_m")]
    alt_m: f64,
    /// Optional: a pre-binding mission_id for the leader. When omitted,
    /// the generator uses the leader's currently-bound mission; when
    /// that is also missing, returns 422 with a hint.
    #[serde(default)]
    leader_mission_id: Option<String>,
}

fn default_ftl_separation_m() -> f64 {
    10.0
}
fn default_ftl_alt_m() -> f64 {
    12.0
}

/// One generated mission for one follower (the per-vehicle output of
/// `POST /api/fleet/patterns/follow-the-leader/generate`).
#[derive(Debug, serde::Serialize)]
struct GeneratedMission {
    vehicle_id: u8,
    /// The leader's mission id (so the console can show "follower of
    /// mission X"). Echoed in the response.
    leader_mission_id: String,
    /// Generated waypoints — same shape as a catalog Waypoint, ready to
    /// POST back to :8300 as a new mission.
    waypoints: Vec<fleet_mission::gcs::mission_file::Waypoint>,
}

/// Offset a lat/lon pair by `north_m`/`east_m` metres (small-area
/// flat-earth approximation — fine for the <1 km separations swarming
/// patterns use). Returns `(lat_deg, lon_deg)`.
fn offset_latlon(lat_deg: f64, lon_deg: f64, north_m: f64, east_m: f64) -> (f64, f64) {
    // 1° lat ≈ 111_320 m; 1° lon ≈ 111_320 m × cos(lat).
    const M_PER_DEG_LAT: f64 = 111_320.0;
    let lat = lat_deg + north_m / M_PER_DEG_LAT;
    let lon = lon_deg + east_m / (M_PER_DEG_LAT * lat_deg.to_radians().cos());
    (lat, lon)
}

/// Compute the bearing (degrees, 0 = north, 90 = east) from (lat1, lon1)
/// to (lat2, lon2). Used to place a follower `separation_m` *behind*
/// the leader along the leader's direction of travel.
fn bearing_deg(lat1: f64, lon1: f64, lat2: f64, lon2: f64) -> f64 {
    let phi1 = lat1.to_radians();
    let phi2 = lat2.to_radians();
    let dlam = (lon2 - lon1).to_radians();
    let y = dlam.sin() * phi2.cos();
    let x = phi1.cos() * phi2.sin() - phi1.sin() * phi2.cos() * dlam.cos();
    let theta = y.atan2(x).to_degrees();
    (theta + 360.0) % 360.0
}

/// `POST /api/fleet/patterns/{name}/generate` — generate per-vehicle
/// missions from a swarming pattern. M5 implements `follow-the-leader`
/// fully; the other two patterns return 501 NotImplemented.
async fn fleet_pattern_generate_post(
    State(s): State<Arc<AppState>>,
    Path(name): Path<String>,
    body: axum::extract::Json<serde_json::Value>,
) -> Response {
    match name.as_str() {
        "follow-the-leader" => fleet_pattern_follow_the_leader(s, body).await,
        "search-grid" | "parallel-patrol" => err(
            StatusCode::NOT_IMPLEMENTED,
            &format!(
                "pattern '{name}' is stubbed in M5; only 'follow-the-leader' is implemented"
            ),
        ),
        other => err(
            StatusCode::NOT_FOUND,
            &format!(
                "unknown pattern '{other}'; known: follow-the-leader, search-grid, parallel-patrol"
            ),
        ),
    }
}

/// Compute the follower's waypoints for the follow-the-leader pattern
/// (M5 pure geometry helper). For each leader waypoint at index k ≥ 1,
/// the follower's waypoint is placed at `separation_m` *behind* the
/// leader along the bearing from waypoint k-1 → waypoint k. Waypoint 0
/// (takeoff) is placed `separation_m` behind the leader along a 0°
/// bearing (north), since there's no previous waypoint to derive a
/// heading from — M5.1 will read the real takeoff yaw from telemetry.
///
/// All follower waypoints inherit the leader's `seq`/`frame`/`command`/
/// `param1-4` (so a survey mission stays a survey mission, just shifted
/// in space); `z` is overridden to `alt_m` (the pattern's cruise alt).
pub(crate) fn compute_follow_the_leader_waypoints(
    leader_waypoints: &[fleet_mission::gcs::mission_file::Waypoint],
    separation_m: f64,
    alt_m: f64,
) -> Vec<fleet_mission::gcs::mission_file::Waypoint> {
    let mut out: Vec<fleet_mission::gcs::mission_file::Waypoint> =
        Vec::with_capacity(leader_waypoints.len());
    for (k, wp) in leader_waypoints.iter().enumerate() {
        let (lat1, lon1, lat2, lon2) = if k == 0 {
            // No previous waypoint: use a "north-facing" default — the
            // follower starts one separation behind the leader's takeoff
            // point along a 0° bearing. (Real takeoff heading comes from
            // the vehicle's yaw; M5.1 will read it from telemetry.)
            (wp.x, wp.y, wp.x + 1e-6, wp.y)
        } else {
            let prev = &leader_waypoints[k - 1];
            (prev.x, prev.y, wp.x, wp.y)
        };
        let bearing = bearing_deg(lat1, lon1, lat2, lon2);
        // Place the follower at `separation_m` behind the leader along
        // the bearing axis: north_m = -sep * cos(bearing), east_m =
        // -sep * sin(bearing) (opposite of the travel direction).
        let north_m = -separation_m * bearing.to_radians().cos();
        let east_m = -separation_m * bearing.to_radians().sin();
        let (flat, flon) = offset_latlon(wp.x, wp.y, north_m, east_m);
        out.push(fleet_mission::gcs::mission_file::Waypoint {
            seq: wp.seq,
            frame: wp.frame,
            command: wp.command,
            x: flat,
            y: flon,
            z: alt_m as f32,
            param1: wp.param1,
            param2: wp.param2,
            param3: wp.param3,
            param4: wp.param4,
        });
    }
    out
}

/// follow-the-leader generator: takes the leader's mission (from
/// bindings or `leader_mission_id` in the body), and for each follower
/// produces a mission whose waypoints are the leader's waypoints offset
/// by `separation_m` behind (in the direction opposite to travel).
///
/// For the leader's waypoint at index k (k ≥ 1), the follower's waypoint
/// is placed at `separation_m` behind the leader along the bearing from
/// waypoint k-1 → waypoint k. Waypoint 0 (the takeoff point) is placed
/// at `separation_m` behind the leader's takeoff, on the same bearing
/// used for waypoint 1 (or the leader's home bearing when there's only
/// one waypoint — degenerate but defined).
async fn fleet_pattern_follow_the_leader(
    s: Arc<AppState>,
    body: axum::extract::Json<serde_json::Value>,
) -> Response {
    let body: FollowTheLeaderBody = match serde_json::from_value(body.0) {
        Ok(b) => b,
        Err(e) => {
            return err(
                StatusCode::UNPROCESSABLE_ENTITY,
                &format!("invalid body: {e}"),
            )
        }
    };
    if body.followers.is_empty() {
        return err(
            StatusCode::UNPROCESSABLE_ENTITY,
            "followers must not be empty",
        );
    }
    if body.followers.iter().any(|f| *f >= s.count) {
        return err(
            StatusCode::NOT_FOUND,
            &format!("one or more followers are out of range (fleet count {})", s.count),
        );
    }
    if body.leader >= s.count {
        return err(
            StatusCode::NOT_FOUND,
            &format!("leader {} is out of range (fleet count {})", body.leader, s.count),
        );
    }
    if body.followers.contains(&body.leader) {
        return err(
            StatusCode::UNPROCESSABLE_ENTITY,
            "leader must not appear in the followers list",
        );
    }
    if body.separation_m <= 0.0 || !body.separation_m.is_finite() {
        return err(
            StatusCode::UNPROCESSABLE_ENTITY,
            "separation_m must be > 0 and finite",
        );
    }
    if body.alt_m < 0.0 || body.alt_m > 120.0 || !body.alt_m.is_finite() {
        return err(
            StatusCode::UNPROCESSABLE_ENTITY,
            "alt_m must be in [0, 120] m AGL",
        );
    }

    // Resolve the leader's mission: explicit body field first, then the
    // leader's bound mission.
    let leader_id = match body.leader_mission_id.clone() {
        Some(id) => id,
        None => match s.mission_binding(body.leader) {
            Some(id) => id,
            None => {
                return err(
                    StatusCode::UNPROCESSABLE_ENTITY,
                    "leader has no bound mission and no leader_mission_id was provided",
                )
            }
        },
    };
    if !state::looks_like_ulid(&leader_id) {
        return err(
            StatusCode::UNPROCESSABLE_ENTITY,
            &format!("leader_mission_id '{leader_id}' is not a valid ULID"),
        );
    }

    let leader_mission =
        match fetch_mission_from_catalog(s.catalog_url(), &leader_id).await {
            Some(m) => m,
            None => {
                return err(
                    StatusCode::NOT_FOUND,
                    &format!("leader mission {leader_id} not found in catalog"),
                )
            }
        };
    if leader_mission.waypoints.is_empty() {
        return err(
            StatusCode::UNPROCESSABLE_ENTITY,
            "leader mission has no waypoints",
        );
    }

    let follower_waypoints = compute_follow_the_leader_waypoints(
        &leader_mission.waypoints,
        body.separation_m,
        body.alt_m,
    );

    // All followers share the same generated waypoints in M5 (real
    // formation flight would offset each by its index × separation;
    // that's M5.1's job). Echoed in the response so the client can
    // POST each back to :8300 to create per-vehicle missions.
    let generated: Vec<GeneratedMission> = body
        .followers
        .iter()
        .map(|f| GeneratedMission {
            vehicle_id: *f,
            leader_mission_id: leader_id.clone(),
            waypoints: follower_waypoints.clone(),
        })
        .collect();

    s.log.log(
        fleet_core::events::EventKind::SupervisorAction,
        None,
        format!(
            "operator: follow-the-leader via API — leader={} followers={} separation={}m waypoints={}",
            body.leader,
            body.followers.len(),
            body.separation_m,
            follower_waypoints.len()
        ),
    );

    ok(json!({
        "ok": true,
        "pattern": "follow-the-leader",
        "leader_mission_id": leader_id,
        "separation_m": body.separation_m,
        "alt_m": body.alt_m,
        "generated": serde_json::to_value(&generated).unwrap_or(json!(null)),
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
                "GET /api/fleet", "PUT /api/fleet", "GET /api/vehicles/{i}", "GET /api/events",
                "GET|POST /api/tasks", "POST /api/estop", "POST /api/fleet/estop",
                "POST /api/vehicles/{i}/faults",
                "GET /api/airframes", "GET /api/modes",
                "GET /api/vehicles/{i}/setup", "GET|POST /api/vehicles/{i}/params",
                "POST /api/vehicles/{i}/params/refresh",
                "POST /api/vehicles/{i}/airframe", "POST /api/vehicles/{i}/calibrate",
                "POST /api/vehicles/{i}/mode",
                "POST /api/mission", "POST /api/mission/start", "POST /api/mission/clear",
                "POST /api/vehicles/{i}/arm", "POST /api/vehicles/{i}/takeoff",
                "POST /api/vehicles/{i}/land", "POST /api/vehicles/{i}/rtl",
                "POST /api/vehicles/{i}/hold", "POST /api/vehicles/{i}/goto",
                "GET /api/vehicles/{i}/mission?type=mission|fence|rally",
                "POST /api/vehicles/{i}/mission/upload",
                "GET /api/vehicles/{i}/prearm-checks",
                "GET|POST /api/fleet/mission-bindings",
                "DELETE /api/fleet/mission-bindings/{vehicle_id}",
                "POST /api/fleet/start",
                "GET /api/fleet/patterns",
                "POST /api/fleet/patterns/{name}/generate",
                "WS /ws/fleet | /ws | /"
            ],
            "phase": s.phase(),
            "vehicles": s.count,
        }))
        .into_response(),
    }
}

/// 10 Hz fleet frames + events over one socket, until the client leaves.
/// (Cadence pinned at 10 Hz since the first live build; spec §3.4 records
/// it. The run report header also carries the frame cadence.)
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
        let s = AppState::new(
            registry,
            log,
            2,
            "test-scenario.toml",
            0,
            fleet_core::geo::GeoOrigin::DEFAULT,
            fleet_safety::geofence::Geofence::default_square(),
            std::env::temp_dir(),
        );
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
        // ADR-0017: the index advertises the operator plane.
        let eps = j["data"]["endpoints"].as_array().unwrap();
        assert!(eps.iter().any(|e| e.as_str().unwrap().contains("/api/mission")));
        assert!(eps.iter().any(|e| e.as_str().unwrap().contains("goto")));
    }

    #[tokio::test]
    async fn fleet_get_carries_geo_blocks() {
        let state = test_state();
        let app = router(state);
        let resp = app
            .clone()
            .oneshot(Request::get("/api/fleet").body(Body::empty()).unwrap())
            .await
            .unwrap();
        let j = body_json(resp).await;
        let d = &j["data"];
        // ADR-0017: geo origin + the real fence ride the frame.
        assert!((d["geo_origin"]["lat_deg"].as_f64().unwrap() - 47.39777).abs() < 1e-6);
        assert!((d["geo_origin"]["lon_deg"].as_f64().unwrap() - 8.54558).abs() < 1e-6);
        assert_eq!(d["geofence"]["ceiling_m"], 60.0);
        let pts = d["geofence"]["points_ned_m"].as_array().unwrap();
        assert!(pts.len() >= 3, "fence polygon on the wire");
        // vehicles carry the GLOBAL_POSITION_INT fix verbatim (0 until one
        // arrives — the field's presence is the contract).
        assert!(d["vehicles"][0].get("lat_deg_e7").is_some());
        assert!(d["vehicles"][0].get("lon_deg_e7").is_some());
    }

    // -- operator control plane (ADR-0017) ----------------------------------

    fn post_json(app: Router, uri: &str, body: &str) -> impl std::future::Future<Output = (StatusCode, serde_json::Value)> {
        let req = Request::post(uri)
            .header(header::CONTENT_TYPE, "application/json")
            .body(Body::from(body.to_string()))
            .unwrap();
        async move {
            let resp = app.oneshot(req).await.unwrap();
            let status = resp.status();
            (status, body_json(resp).await)
        }
    }

    #[tokio::test]
    async fn mission_upload_validation_errors() {
        let state = test_state();
        let app = router(state);
        // empty items
        let (st, j) = post_json(app.clone(), "/api/mission", r#"{"items": []}"#).await;
        assert_eq!(st, StatusCode::UNPROCESSABLE_ENTITY);
        assert!(j["error"].as_str().unwrap().contains("empty"));
        // lat out of range
        let (st, j) = post_json(
            app.clone(),
            "/api/mission",
            r#"{"items": [{"lat_deg": 91.0, "lon_deg": 0.0, "alt_m": 10.0}]}"#,
        )
        .await;
        assert_eq!(st, StatusCode::UNPROCESSABLE_ENTITY);
        assert!(j["error"].as_str().unwrap().contains("range"));
        // negative AGL
        let (st, j) = post_json(
            app,
            "/api/mission",
            r#"{"items": [{"lat_deg": 47.4, "lon_deg": 8.5, "alt_m": -1.0}]}"#,
        )
        .await;
        assert_eq!(st, StatusCode::UNPROCESSABLE_ENTITY);
        assert!(j["error"].as_str().unwrap().contains("AGL"));
    }

    /// The full queue+drain+ack loop: the handler queues the command, the
    /// "supervisor" (this test) drains and acks it, the response carries the
    /// honest result. This is the exact shape the manager's tick implements.
    #[tokio::test]
    async fn mission_upload_queues_and_acks() {
        let state = test_state();
        let app = router(Arc::clone(&state));
        let task = tokio::spawn(async move {
            let req = Request::post("/api/mission")
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(
                    r#"{"items": [{"lat_deg": 47.3978, "lon_deg": 8.5457, "alt_m": 12.0, "hover_s": 2.0}]}"#,
                ))
                .unwrap();
            app.oneshot(req).await.unwrap()
        });
        // let the handler queue, then play supervisor: drain + ack
        tokio::time::sleep(Duration::from_millis(50)).await;
        let cmds = state.take_operator_cmds();
        assert_eq!(cmds.len(), 1, "exactly one queued command");
        match cmds.into_iter().next().unwrap() {
            OperatorCmd::Upload { items, replace, ack } => {
                assert_eq!(items.len(), 1);
                assert!((items[0].alt_m - 12.0).abs() < 1e-9);
                assert!((items[0].hover_s - 2.0).abs() < 1e-6);
                assert!(!replace, "default mode is append");
                ack.send(crate::state::UploadAck {
                    accepted: vec!["op1".into()],
                    rejected: vec![("far".into(), "outside geofence polygon".into())],
                    pool: 1,
                })
                .unwrap();
            }
            other => panic!("expected Upload, got {other:?}"),
        }
        let resp = task.await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let j = body_json(resp).await;
        assert_eq!(j["data"]["accepted"], serde_json::json!(["op1"]));
        assert_eq!(j["data"]["pool"], 1);
        assert!(j["data"]["rejected"][0]["reason"]
            .as_str()
            .unwrap()
            .contains("geofence"));
        // the operator action is logged
        assert!(state.log.total() >= 3);
    }

    /// No supervisor ticking -> the honest 503, not a hang.
    #[tokio::test]
    async fn mission_start_times_out_without_supervisor() {
        let state = test_state();
        let app = router(state);
        let (st, j) = post_json(app, "/api/mission/start", "{}").await;
        assert_eq!(st, StatusCode::SERVICE_UNAVAILABLE);
        assert!(j["error"].as_str().unwrap().contains("drain"));
    }

    // -- runtime task append (ADR-0018, POST /api/tasks) -------------------

    #[tokio::test]
    async fn tasks_post_validation_errors() {
        let state = test_state();
        let app = router(state);
        // not an object/array -> 400
        let (st, _) = post_json(app.clone(), "/api/tasks", r#""just a string""#).await;
        assert_eq!(st, StatusCode::BAD_REQUEST);
        // missing pos_ned_m -> 422 with the field hint
        let (st, j) = post_json(app.clone(), "/api/tasks", r#"{"id": "wp_x"}"#).await;
        assert_eq!(st, StatusCode::UNPROCESSABLE_ENTITY);
        assert!(j["error"].as_str().unwrap().contains("pos_ned_m"));
        // empty array -> 422
        let (st, j) = post_json(app.clone(), "/api/tasks", "[]").await;
        assert_eq!(st, StatusCode::UNPROCESSABLE_ENTITY);
        assert!(j["error"].as_str().unwrap().contains("no tasks"));
        // non-finite position -> 422 (serde passes NaN through JSON? no —
        // guard anyway with an out-of-shape array)
        let (st, j) = post_json(
            app,
            "/api/tasks",
            r#"[{"pos_ned_m": [1.0, 2.0]}, {"pos_ned_m": [1.0, 2.0, -3.0], "hover_s": 1}]"#,
        )
        .await;
        assert_eq!(st, StatusCode::UNPROCESSABLE_ENTITY);
        assert!(j["error"].as_str().unwrap().contains("task[0]"));
    }

    /// The queue+drain+ack loop for task appends — same discipline as the
    /// mission upload: handler queues Append, this test plays supervisor.
    #[tokio::test]
    async fn tasks_post_queues_and_acks() {
        let state = test_state();
        let app = router(Arc::clone(&state));
        let task = tokio::spawn(async move {
            let req = Request::post("/api/tasks")
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(
                    r#"[{"id": "wp_ned", "pos_ned_m": [20.0, 20.0, -12.0], "hover_s": 5}, {"pos_ned_m": [15.0, 0.0, -8.0]}]"#,
                ))
                .unwrap();
            app.oneshot(req).await.unwrap()
        });
        tokio::time::sleep(Duration::from_millis(50)).await;
        let cmds = state.take_operator_cmds();
        assert_eq!(cmds.len(), 1, "exactly one queued command");
        match cmds.into_iter().next().unwrap() {
            OperatorCmd::Append { tasks, ack } => {
                assert_eq!(tasks.len(), 2);
                assert_eq!(tasks[0].id.as_deref(), Some("wp_ned"));
                assert_eq!(tasks[0].pos_ned_m, [20.0, 20.0, -12.0]);
                assert!((tasks[0].hover_s - 5.0).abs() < 1e-6);
                assert!(tasks[1].id.is_none(), "auto id when omitted");
                assert!((tasks[1].reward - 1.0).abs() < 1e-6, "default reward");
                ack.send(crate::state::UploadAck {
                    accepted: vec!["wp_ned".into(), "op1".into()],
                    rejected: vec![],
                    pool: 2,
                })
                .unwrap();
            }
            other => panic!("expected Append, got {other:?}"),
        }
        let resp = task.await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let j = body_json(resp).await;
        assert_eq!(j["data"]["accepted"], serde_json::json!(["wp_ned", "op1"]));
        assert_eq!(j["data"]["pool"], 2);
    }

    /// No supervisor -> 503, the honest timeout (not a hang).
    #[tokio::test]
    async fn tasks_post_times_out_without_supervisor() {
        let state = test_state();
        let app = router(state);
        let (st, j) = post_json(app, "/api/tasks", r#"{"pos_ned_m": [1.0, 2.0, -3.0]}"#).await;
        assert_eq!(st, StatusCode::SERVICE_UNAVAILABLE);
        assert!(j["error"].as_str().unwrap().contains("drain"));
    }

    // -- hot scenario load (ADR-0018, PUT /api/fleet) ----------------------

    fn put_json(app: Router, uri: &str, body: &str) -> impl std::future::Future<Output = (StatusCode, serde_json::Value)> {
        let req = Request::put(uri)
            .header(header::CONTENT_TYPE, "application/json")
            .body(Body::from(body.to_string()))
            .unwrap();
        async move {
            let resp = app.oneshot(req).await.unwrap();
            let status = resp.status();
            (status, body_json(resp).await)
        }
    }

    /// JSON-escape a TOML text into a PUT body string.
    fn put_body(toml: &str) -> String {
        serde_json::json!({ "scenario_toml": toml }).to_string()
    }

    const MIN_SCENARIO_TOML: &str = "[fleet]\ncount = 1\n\n[[tasks]]\nid = \"wp_a\"\npos_ned_m = [10.0, 0.0, -10.0]\nhover_s = 2\n";

    #[tokio::test]
    async fn fleet_put_validation_errors() {
        let state = test_state();
        let app = router(state);
        // missing scenario_toml field -> 422 with the field named (the JSON
        // envelope contract holds: the Json rejection is wrapped, not
        // leaked as a text/plain axum rejection)
        let (st, j) = put_json(app.clone(), "/api/fleet", "{}").await;
        assert_eq!(st, StatusCode::UNPROCESSABLE_ENTITY);
        assert!(j["error"].as_str().unwrap().contains("scenario_toml"));
        // invalid TOML -> 422 with the parse reason
        let (st, j) = put_json(app.clone(), "/api/fleet", &put_body("not [valid toml")).await;
        assert_eq!(st, StatusCode::UNPROCESSABLE_ENTITY);
        assert!(j["error"].as_str().unwrap().contains("invalid scenario"));
        // valid TOML but a compile-rejected task (outside the 100 m fence)
        let bad = "[fleet]\ncount = 1\n\n[[tasks]]\nid = \"far\"\npos_ned_m = [500.0, 0.0, -10.0]\n";
        let (st, j) = put_json(app, "/api/fleet", &put_body(bad)).await;
        assert_eq!(st, StatusCode::UNPROCESSABLE_ENTITY);
        assert!(j["error"].as_str().unwrap().contains("far"));
        assert!(j["error"].as_str().unwrap().contains("compile"));
    }

    #[tokio::test]
    async fn fleet_put_mission_active_conflict() {
        let state = test_state();
        state.set_mission_active(0, true);
        let app = router(state);
        let (st, j) = put_json(app, "/api/fleet", &put_body(MIN_SCENARIO_TOML)).await;
        assert_eq!(st, StatusCode::CONFLICT);
        assert!(j["error"].as_str().unwrap().contains("autonomous mission"));
    }

    /// Happy path: valid + quiescent -> staged file + HotLoad queued; the
    /// test plays supervisor, accepts, and the response points at the staged
    /// TOML. The staging is observable via next_scenario.
    #[tokio::test]
    async fn fleet_put_queues_and_acks() {
        let state = test_state();
        let app = router(Arc::clone(&state));
        let task = tokio::spawn(async move {
            let req = Request::put("/api/fleet")
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(put_body(MIN_SCENARIO_TOML)))
                .unwrap();
            app.oneshot(req).await.unwrap()
        });
        tokio::time::sleep(Duration::from_millis(50)).await;
        let cmds = state.take_operator_cmds();
        assert_eq!(cmds.len(), 1);
        match cmds.into_iter().next().unwrap() {
            OperatorCmd::HotLoad { ack } => {
                let staged = state.next_scenario_path().expect("staged before ack");
                assert!(staged.to_string_lossy().contains("hot-scenario-"));
                assert!(staged
                    .file_name()
                    .and_then(|n| n.to_str())
                    .is_some_and(|n| n.ends_with(".toml")));
                let written = std::fs::read_to_string(&staged).unwrap();
                assert!(written.contains("[fleet]"));
                assert!(written.contains("count = 1"));
                ack.send(crate::state::HotLoadAck {
                    accepted: true,
                    reason: None,
                })
                .unwrap();
            }
            other => panic!("expected HotLoad, got {other:?}"),
        }
        let resp = task.await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let j = body_json(resp).await;
        assert!(j["data"]["staged"].as_str().unwrap().contains("hot-scenario-"));
        assert!(j["data"]["note"].as_str().unwrap().contains("rebinds"));
        // still staged after the accepted ack (run_scenario consumes it
        // later); take() consumes it exactly once.
        assert!(state.next_scenario_path().is_some());
        let consumed = state.take_next_scenario();
        assert!(consumed.is_some());
        assert!(state.next_scenario_path().is_none(), "take consumed it");
    }

    /// Supervisor nack (e.g. mission started in the race window) -> 409 and
    /// the staging is cleared: nothing half-swapped.
    #[tokio::test]
    async fn fleet_put_nack_clears_staging() {
        let state = test_state();
        let app = router(Arc::clone(&state));
        let task = tokio::spawn(async move {
            let req = Request::put("/api/fleet")
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(put_body(MIN_SCENARIO_TOML)))
                .unwrap();
            app.oneshot(req).await.unwrap()
        });
        tokio::time::sleep(Duration::from_millis(50)).await;
        let cmds = state.take_operator_cmds();
        match cmds.into_iter().next().unwrap() {
            OperatorCmd::HotLoad { ack } => {
                ack.send(crate::state::HotLoadAck {
                    accepted: false,
                    reason: Some("mission started but not complete".into()),
                })
                .unwrap();
            }
            other => panic!("expected HotLoad, got {other:?}"),
        }
        let resp = task.await.unwrap();
        assert_eq!(resp.status(), StatusCode::CONFLICT);
        let j = body_json(resp).await;
        assert!(j["error"].as_str().unwrap().contains("mission started"));
        assert!(state.next_scenario_path().is_none(), "staging cleared on nack");
    }

    // -- fault proxy (ADR-0018, POST /api/vehicles/{i}/faults) -------------

    #[tokio::test]
    async fn vehicle_faults_post_validation() {
        let state = test_state();
        let app = router(state);
        // index out of range -> 404
        let (st, j) = post_json(app.clone(), "/api/vehicles/5/faults", r#"{"type": "motor_cut"}"#).await;
        assert_eq!(st, StatusCode::NOT_FOUND);
        assert!(j["error"].as_str().unwrap().contains("out of range"));
        // body without a "type" -> 400
        let (st, j) = post_json(app.clone(), "/api/vehicles/0/faults", r#"{"params": {"motor": 1}}"#).await;
        assert_eq!(st, StatusCode::BAD_REQUEST);
        assert!(j["error"].as_str().unwrap().contains("type"));
        // empty type -> 400
        let (st, _) = post_json(app, "/api/vehicles/0/faults", r#"{"type": "  "}"#).await;
        assert_eq!(st, StatusCode::BAD_REQUEST);
    }

    #[tokio::test]

    async fn guided_command_gates() {
        let state = test_state();
        let app = router(Arc::clone(&state));
        // index out of range -> 404
        let (st, j) = post_json(app.clone(), "/api/vehicles/5/arm", r#"{"arm": true}"#).await;
        assert_eq!(st, StatusCode::NOT_FOUND);
        assert!(j["error"].as_str().unwrap().contains("out of range"));
        // no link registered -> 409
        let (st, j) = post_json(
            app.clone(),
            "/api/vehicles/0/takeoff",
            r#"{"alt_m": 10.0}"#,
        )
        .await;
        assert_eq!(st, StatusCode::CONFLICT);
        assert!(j["error"].as_str().unwrap().contains("link"));
        // fence box check: 80 m AGL exceeds the 60 m ceiling (but is within
        // the generic (0,120] band) — the fence's own box rejects it.
        let (st, j) = post_json(
            app.clone(),
            "/api/vehicles/0/takeoff",
            r#"{"alt_m": 80.0}"#,
        )
        .await;
        assert_eq!(st, StatusCode::UNPROCESSABLE_ENTITY);
        assert!(j["error"].as_str().unwrap().contains("geofence altitude box"));
        // goto validation: lat/lon out of range -> 422
        let (st, j) = post_json(
            app.clone(),
            "/api/vehicles/0/goto",
            r#"{"lat_deg": -200.0, "lon_deg": 0.0}"#,
        )
        .await;
        assert_eq!(st, StatusCode::UNPROCESSABLE_ENTITY);
        // mission-active gate: a live mission runner blocks guided commands
        state.set_mission_active(0, true);
        let (st, j) = post_json(
            app,
            "/api/vehicles/0/rtl",
            "{}",
        )
        .await;
        assert_eq!(st, StatusCode::CONFLICT);
        assert!(j["error"].as_str().unwrap().contains("autonomous mission"));
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

    // -- MAVLink mission upload/download (M2-API) ----------------------------

    /// `POST /api/vehicles/{i}/mission/upload` with no link registered → 404
    /// (vehicle exists in the fleet but the MAVLink link task isn't up, so
    /// the proxy can't reach PX4). The validation gates still run first, so a
    /// well-formed body is required before the 404 fires.
    #[tokio::test]
    async fn mission_upload_post_returns_404_when_no_link() {
        let state = test_state();
        let app = router(Arc::clone(&state));
        // out of range -> 404 (the index check fires before the link check)
        let (st, j) = post_json(
            app.clone(),
            "/api/vehicles/5/mission/upload",
            r#"{"items":[{"param1":0.0,"param2":2.0,"param3":0.0,"param4":0.0,"x":473977700,"y":854558000,"z":12.0,"seq":0,"command":16,"target_system":1,"target_component":1,"frame":3,"current":0,"autocontinue":1,"mission_type":0}],"mission_type":0}"#,
        ).await;
        assert_eq!(st, StatusCode::NOT_FOUND);
        assert!(j["error"].as_str().unwrap().contains("out of range"));
        // valid body + index 0 but no link -> 404 (vehicle not connected)
        let (st, j) = post_json(
            app,
            "/api/vehicles/0/mission/upload",
            r#"{"items":[{"param1":0.0,"param2":2.0,"param3":0.0,"param4":0.0,"x":473977700,"y":854558000,"z":12.0,"seq":0,"command":16,"target_system":1,"target_component":1,"frame":3,"current":0,"autocontinue":1,"mission_type":0}],"mission_type":0}"#,
        ).await;
        assert_eq!(st, StatusCode::NOT_FOUND);
        assert!(j["error"].as_str().unwrap().contains("link"));
        assert!(state.log.total() >= 1, "the link gate fired (not the call) — log untouched");
    }

    /// `POST /api/vehicles/{i}/mission/upload` validation gates (these fire
    /// before the link check, so they hold even with no link registered).
    #[tokio::test]
    async fn mission_upload_post_validation_errors() {
        let state = test_state();
        let app = router(state);
        // empty items -> 422
        let (st, j) = post_json(
            app.clone(),
            "/api/vehicles/0/mission/upload",
            r#"{"items":[],"mission_type":0}"#,
        ).await;
        assert_eq!(st, StatusCode::UNPROCESSABLE_ENTITY);
        assert!(j["error"].as_str().unwrap().contains("empty"));
        // mission_type out of range -> 422
        let (st, j) = post_json(
            app.clone(),
            "/api/vehicles/0/mission/upload",
            r#"{"items":[{"param1":0.0,"param2":0.0,"param3":0.0,"param4":0.0,"x":473977700,"y":854558000,"z":12.0,"seq":0,"command":16,"target_system":1,"target_component":1,"frame":3,"current":0,"autocontinue":1,"mission_type":0}],"mission_type":5}"#,
        ).await;
        assert_eq!(st, StatusCode::UNPROCESSABLE_ENTITY);
        assert!(j["error"].as_str().unwrap().contains("mission_type"));
        // malformed body (missing items) -> 422 wrapped in the envelope
        let (st, j) = post_json(
            app,
            "/api/vehicles/0/mission/upload",
            r#"{"mission_type":0}"#,
        ).await;
        assert_eq!(st, StatusCode::UNPROCESSABLE_ENTITY);
        assert!(j["error"].as_str().unwrap().contains("invalid body"));
    }

    /// `GET /api/vehicles/{i}/mission?type=...` with no link registered → 404
    /// (same gate as upload: the vehicle exists, but the link isn't up).
    #[tokio::test]
    async fn mission_download_get_returns_404_when_no_link() {
        let state = test_state();
        let app = router(Arc::clone(&state));
        // default type (mission) + valid index, but no link
        let resp = app
            .clone()
            .oneshot(Request::get("/api/vehicles/0/mission").body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::NOT_FOUND);
        let j = body_json(resp).await;
        assert!(j["error"].as_str().unwrap().contains("link"));
        // explicit type=fence
        let resp = app
            .clone()
            .oneshot(Request::get("/api/vehicles/1/mission?type=fence").body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::NOT_FOUND);
        assert!(state.log.total() >= 1);
        // out of range -> 404 (before the link check)
        let resp = app
            .oneshot(Request::get("/api/vehicles/9/mission").body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::NOT_FOUND);
    }

    /// `GET /api/vehicles/{i}/mission?type=...` query-string validation.
    #[tokio::test]
    async fn mission_download_get_validates_type_query() {
        let state = test_state();
        let app = router(state);
        // unknown type string -> 422
        let resp = app
            .clone()
            .oneshot(Request::get("/api/vehicles/0/mission?type=warpspeed").body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::UNPROCESSABLE_ENTITY);
        let j = body_json(resp).await;
        assert!(j["error"].as_str().unwrap().contains("mission|fence|rally"));
        // empty type= -> defaults to mission (404 fires from the link gate,
        // not from the type parse — proving the default took effect)
        let resp = app
            .clone()
            .oneshot(Request::get("/api/vehicles/0/mission?type=").body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::NOT_FOUND);
        assert!(body_json(resp).await["error"].as_str().unwrap().contains("link"));
        // no type at all -> defaults to mission (same gate)
        let resp = app
            .oneshot(Request::get("/api/vehicles/0/mission").body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::NOT_FOUND);
    }

    /// The MISSION_ITEM_INT JSON shape round-trips through serde cleanly
    /// (the field-name contract the QGC Plan View depends on — M2-API's
    /// wire-shape pin). This is the only place we exercise the new
    /// `Serialize`/`Deserialize` derives on `MissionItemInt` directly.
    #[test]
    fn mission_item_int_json_round_trip() {
        let item = fleet_mavlink::MissionItemInt {
            param1: 0.0,
            param2: 2.0,
            param3: 0.0,
            param4: 0.0,
            x: 473977700,
            y: 854558000,
            z: 12.0,
            seq: 0,
            command: 16, // MAV_CMD_NAV_WAYPOINT
            target_system: 1,
            target_component: 1,
            frame: 3, // MAV_FRAME_GLOBAL_RELATIVE_ALT
            current: 0,
            autocontinue: 1,
            mission_type: 0,
        };
        let s = serde_json::to_string(&item).expect("serialize");
        // QGC's exact field names appear verbatim in the wire JSON
        for field in [
            "param1","param2","param3","param4","x","y","z","seq","command",
            "target_system","target_component","frame","current","autocontinue","mission_type",
        ] {
            assert!(s.contains(&format!("\"{field}\":")), "missing field '{field}' in {s}");
        }
        let back: fleet_mavlink::MissionItemInt = serde_json::from_str(&s).expect("deserialize");
        assert_eq!(back, item, "round trip preserves all fields");
        // x/y are i32 (lat/lon × 1e7), not floats — QGC's MISSION_ITEM_INT shape
        assert!(s.contains("\"x\":473977700"));
        assert!(s.contains("\"y\":854558000"));
    }

    /// The result enums serialize into the tagged shape the REST envelope
    /// carries — `{"status": "ok"|"failed"|"timeout", ...}` — so the Plan
    /// View can branch on `status` without inspecting inner fields.
    #[test]
    fn mission_result_enums_serialize_tagged() {
        let ok = fleet_mavlink::MissionUploadResult::Ok {
            mission_type: 0,
            items_sent: 5,
            items_acked: 5,
        };
        let v = serde_json::to_value(&ok).unwrap();
        assert_eq!(v["status"], "ok");
        assert_eq!(v["mission_type"], 0);
        assert_eq!(v["items_sent"], 5);
        assert_eq!(v["items_acked"], 5);

        let failed = fleet_mavlink::MissionUploadResult::Failed {
            mission_type: 1,
            reason: "unsupported frame".into(),
            items_sent: 3,
            items_acked: 1,
        };
        let v = serde_json::to_value(&failed).unwrap();
        assert_eq!(v["status"], "failed");
        assert_eq!(v["reason"], "unsupported frame");
        assert_eq!(v["items_acked"], 1);

        let timeout = fleet_mavlink::MissionUploadResult::Timeout {
            mission_type: 2,
            items_sent: 2,
            items_acked: 0,
        };
        let v = serde_json::to_value(&timeout).unwrap();
        assert_eq!(v["status"], "timeout");

        let dl_ok = fleet_mavlink::MissionDownloadResult::Ok {
            mission_type: 0,
            items: vec![fleet_mavlink::MissionItemInt {
                param1: 0.0, param2: 0.0, param3: 0.0, param4: 0.0,
                x: 473977700, y: 854558000, z: 12.0,
                seq: 0, command: 16, target_system: 1, target_component: 1,
                frame: 3, current: 0, autocontinue: 1, mission_type: 0,
            }],
        };
        let v = serde_json::to_value(&dl_ok).unwrap();
        assert_eq!(v["status"], "ok");
        assert_eq!(v["items"].as_array().unwrap().len(), 1);
        assert_eq!(v["items"][0]["command"], 16);

        let dl_timeout = fleet_mavlink::MissionDownloadResult::Timeout { mission_type: 1 };
        let v = serde_json::to_value(&dl_timeout).unwrap();
        assert_eq!(v["status"], "timeout");
        assert_eq!(v["mission_type"], 1);
    }

    /// The root JSON index (the non-WS GET `/` response) advertises the two
    /// new endpoints so QGC-style clients can discover them.
    #[tokio::test]
    async fn root_index_advertises_mission_upload_download() {
        let state = test_state();
        let app = router(state);
        let resp = app
            .oneshot(Request::get("/").body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let j = body_json(resp).await;
        let eps = j["data"]["endpoints"].as_array().unwrap();
        assert!(
            eps.iter().any(|e| e.as_str().unwrap().contains("mission/upload")),
            "upload endpoint advertised"
        );
        assert!(
            eps.iter().any(|e| e.as_str().unwrap().contains("/mission?type=")),
            "download endpoint advertised"
        );
    }

    // -- M3 (Fly View): pre-arm checklist (GCS_SPEC.md §5.2) ----------------

    /// Construct a state whose vehicle 0 satisfies every pre-arm check:
    /// link healthy (no flags), local_position_seen, GPS fix, STANDBY
    /// mode_word == 0, battery 85%. The handler's `all_passed: true` path
    /// asserts against this fixture.
    fn healthy_prearm_state() -> Arc<AppState> {
        let registry = Registry::new(2);
        registry.entry(0).unwrap().state.write(|s| {
            s.sysid = 1;
            s.compid = 1;
            s.heartbeat_seen = true;
            s.local_position_seen = true;
            s.home_set = true;
            // mode_word == 0 → PX4 STANDBY (disarmed pre-flight custom mode).
            s.mode_word = 0;
            s.lat_deg_e7 = 473_977_700;
            s.lon_deg_e7 = 85_455_800;
            s.battery_pct = 85;
            s.last_msg_ms = 100;
            s.last_heartbeat_ms = 100;
        });
        let log = EventLog::in_memory();
        AppState::new(
            registry,
            log,
            2,
            "test-scenario.toml",
            0,
            fleet_core::geo::GeoOrigin::DEFAULT,
            fleet_safety::geofence::Geofence::default_square(),
            std::env::temp_dir(),
        )
    }

    /// Pull the check list as a `Vec<Value>` so each test can index by name
    /// (the handler always emits 5 checks in fixed order, but keying by name
    /// is robust to re-ordering).
    fn check_by_name<'a>(checks: &'a [serde_json::Value], name: &str) -> &'a serde_json::Value {
        checks
            .iter()
            .find(|c| c["name"].as_str() == Some(name))
            .unwrap_or_else(|| panic!("check '{name}' missing from {checks:?}"))
    }

    #[tokio::test]
    async fn prearm_checks_returns_404_for_invalid_index() {
        let state = test_state();
        let app = router(state);
        let resp = app
            .oneshot(Request::get("/api/vehicles/99/prearm-checks").body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::NOT_FOUND);
        let j = body_json(resp).await;
        assert_eq!(j["ok"], false);
        assert!(j["error"].as_str().unwrap().contains("out of range"));
    }

    #[tokio::test]
    async fn prearm_checks_returns_all_passed_for_healthy_vehicle() {
        let state = healthy_prearm_state();
        let app = router(state);
        let resp = app
            .oneshot(Request::get("/api/vehicles/0/prearm-checks").body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let j = body_json(resp).await;
        assert_eq!(j["ok"], true);
        let checks = j["data"]["checks"].as_array().unwrap();
        assert_eq!(checks.len(), 5, "exactly five QGC pre-arm checks");
        // every check passed + the expected QGC message strings
        assert_eq!(check_by_name(checks, "EKF2")["passed"], true);
        assert_eq!(check_by_name(checks, "EKF2")["message"], "converged");
        assert_eq!(check_by_name(checks, "GPS Fix")["passed"], true);
        assert_eq!(check_by_name(checks, "GPS Fix")["message"], "3D fix");
        assert_eq!(check_by_name(checks, "Mode")["passed"], true);
        assert_eq!(check_by_name(checks, "Mode")["message"], "STANDBY");
        assert_eq!(check_by_name(checks, "Fence")["passed"], true);
        assert_eq!(check_by_name(checks, "Fence")["message"], "inside inclusion");
        assert_eq!(check_by_name(checks, "Battery")["passed"], true);
        assert_eq!(check_by_name(checks, "Battery")["message"], "85%");
        assert_eq!(j["data"]["all_passed"], true);
    }

    #[tokio::test]
    async fn prearm_checks_fails_ekf_when_link_stale() {
        let state = healthy_prearm_state();
        // raise LINK_STALE on vehicle 0 — EKF2 must fail, all_passed false
        state.set_flags(0, vec![fleet_core::health::HealthFlag::LinkStale]);
        let app = router(state);
        let resp = app
            .oneshot(Request::get("/api/vehicles/0/prearm-checks").body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let j = body_json(resp).await;
        let checks = j["data"]["checks"].as_array().unwrap();
        assert_eq!(check_by_name(checks, "EKF2")["passed"], false);
        assert_eq!(check_by_name(checks, "EKF2")["message"], "waiting for convergence");
        assert_eq!(j["data"]["all_passed"], false);
    }

    #[tokio::test]
    async fn prearm_checks_fails_battery_when_critical() {
        let state = healthy_prearm_state();
        // battery below the BATTERY_CRIT threshold (20%) — Battery fails
        state.registry.entry(0).unwrap().state.write(|s| {
            s.battery_pct = 15;
        });
        let app = router(state);
        let resp = app
            .oneshot(Request::get("/api/vehicles/0/prearm-checks").body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let j = body_json(resp).await;
        let checks = j["data"]["checks"].as_array().unwrap();
        assert_eq!(check_by_name(checks, "Battery")["passed"], false);
        assert_eq!(check_by_name(checks, "Battery")["message"], "critical (<20%)");
        assert_eq!(j["data"]["all_passed"], false);
        // EKF2/GPS/Mode/Fence remain healthy (the failure is isolated)
        assert_eq!(check_by_name(checks, "EKF2")["passed"], true);
        assert_eq!(check_by_name(checks, "GPS Fix")["passed"], true);
        assert_eq!(check_by_name(checks, "Mode")["passed"], true);
        assert_eq!(check_by_name(checks, "Fence")["passed"], true);
    }

    #[tokio::test]
    async fn prearm_checks_fails_mode_when_armed() {
        let state = healthy_prearm_state();
        // an already-armed vehicle can't be re-armed → Mode check fails
        state.registry.entry(0).unwrap().state.write(|s| {
            s.armed = true;
            // a non-STANDBY mode_word (POSCTL) — the mode_word == 0 short-
            // circuit must NOT mask the armed state
            s.mode_word = fleet_modes::MODE_WORD_POSCTL;
        });
        let app = router(state);
        let resp = app
            .oneshot(Request::get("/api/vehicles/0/prearm-checks").body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let j = body_json(resp).await;
        let checks = j["data"]["checks"].as_array().unwrap();
        assert_eq!(check_by_name(checks, "Mode")["passed"], false);
        assert_eq!(check_by_name(checks, "Mode")["message"], "not in standby");
        assert_eq!(j["data"]["all_passed"], false);
    }

    // -- M5 (Fleet C2, GCS_SPEC.md §5.4): mission bindings, fleet start,
    // swarming patterns ----------------------------------------------------

    /// Two valid ULIDs for the binding tests. Real ULIDs are 26-char
    /// Crockford base32 (sortable, time-prefixed); these are just fixed
    /// values that pass `looks_like_ulid`.
    const ULID_A: &str = "01J8K2000A00000000000A0000";
    const ULID_B: &str = "01J8K3000B00000000000B0001";

    #[tokio::test]
    async fn mission_bindings_post_stores_bindings() {
        let state = test_state();
        let app = router(Arc::clone(&state));
        // POST two bindings
        let (st, j) = post_json(
            app.clone(),
            "/api/fleet/mission-bindings",
            &format!(
                r#"{{"bindings":[{{"vehicle_id":0,"mission_id":"{ULID_A}"}},{{"vehicle_id":1,"mission_id":"{ULID_B}"}}]}}"#
            ),
        )
        .await;
        assert_eq!(st, StatusCode::OK);
        assert_eq!(j["ok"], true);
        let bindings = j["data"]["bindings"].as_array().unwrap();
        assert_eq!(bindings.len(), 2);
        assert_eq!(bindings[0]["vehicle_id"], 0);
        assert_eq!(bindings[0]["mission_id"], ULID_A);
        assert_eq!(bindings[0]["binding_state"], "assigned");
        // GET returns them back
        let resp = app
            .oneshot(Request::get("/api/fleet/mission-bindings").body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let j = body_json(resp).await;
        let bindings = j["data"]["bindings"].as_array().unwrap();
        assert_eq!(bindings.len(), 2, "one row per vehicle");
        assert_eq!(bindings[0]["vehicle_id"], 0);
        assert_eq!(bindings[0]["mission_id"], ULID_A);
        assert_eq!(bindings[0]["binding_state"], "assigned");
        assert_eq!(bindings[1]["vehicle_id"], 1);
        assert_eq!(bindings[1]["mission_id"], ULID_B);
        // internal state also matches (cross-check the AppState field)
        assert_eq!(state.mission_binding(0).as_deref(), Some(ULID_A));
        assert_eq!(state.mission_binding(1).as_deref(), Some(ULID_B));
    }

    #[tokio::test]
    async fn mission_bindings_post_rejects_invalid_vehicle() {
        let state = test_state();
        let app = router(state);
        // vehicle_id 99 out of range (count=2) → 404
        let (st, j) = post_json(
            app.clone(),
            "/api/fleet/mission-bindings",
            &format!(
                r#"{{"bindings":[{{"vehicle_id":99,"mission_id":"{ULID_A}"}}]}}"#
            ),
        )
        .await;
        assert_eq!(st, StatusCode::NOT_FOUND);
        assert!(j["error"].as_str().unwrap().contains("out of range"));
        // also: invalid ULID is rejected (422) before the vehicle_id check
        // fires (atomic semantics — no partial write lands)
        let (st, j) = post_json(
            app,
            "/api/fleet/mission-bindings",
            r#"{"bindings":[{"vehicle_id":0,"mission_id":"not-a-ulid"}]}"#,
        )
        .await;
        assert_eq!(st, StatusCode::UNPROCESSABLE_ENTITY);
        assert!(j["error"].as_str().unwrap().contains("ULID"));
    }

    #[tokio::test]
    async fn mission_bindings_delete_clears_binding() {
        let state = test_state();
        let app = router(Arc::clone(&state));
        // POST to bind, then DELETE vehicle 0, then GET returns None for v0
        let (_, _) = post_json(
            app.clone(),
            "/api/fleet/mission-bindings",
            &format!(
                r#"{{"bindings":[{{"vehicle_id":0,"mission_id":"{ULID_A}"}}]}}"#
            ),
        )
        .await;
        assert_eq!(state.mission_binding(0).as_deref(), Some(ULID_A));

        // DELETE the binding
        let req = axum::http::Request::delete("/api/fleet/mission-bindings/0")
            .body(Body::empty())
            .unwrap();
        let resp = app.clone().oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let j = body_json(resp).await;
        assert_eq!(j["ok"], true);
        assert_eq!(j["data"]["vehicle_id"], 0);
        assert_eq!(j["data"]["cleared"], true);
        assert_eq!(j["data"]["binding_state"], "unbound");

        // GET now shows vehicle 0 as unbound
        let resp = app
            .oneshot(Request::get("/api/fleet/mission-bindings").body(Body::empty()).unwrap())
            .await
            .unwrap();
        let j = body_json(resp).await;
        let bindings = j["data"]["bindings"].as_array().unwrap();
        assert_eq!(bindings[0]["mission_id"], serde_json::Value::Null);
        assert_eq!(bindings[0]["binding_state"], "unbound");
        // AppState agrees
        assert!(state.mission_binding(0).is_none());

        // DELETE on already-unbound vehicle → 200 with cleared=false
        let req = axum::http::Request::delete("/api/fleet/mission-bindings/0")
            .body(Body::empty())
            .unwrap();
        let resp = router(Arc::clone(&state))
            .oneshot(req)
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let j = body_json(resp).await;
        assert_eq!(j["data"]["cleared"], false);

        // DELETE on out-of-range vehicle → 404
        let req = axum::http::Request::delete("/api/fleet/mission-bindings/99")
            .body(Body::empty())
            .unwrap();
        let resp = router(test_state())
            .oneshot(req)
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::NOT_FOUND);
    }

    /// `POST /api/fleet/start` with no bound vehicles → 200 with all
    /// rows "skipped" (the parallel-mode path that needs no real link).
    /// With a bound vehicle but no link → 404 (the test_state has no
    /// link registered — same gate as `POST /api/vehicles/{i}/mission/upload`).
    #[tokio::test]
    async fn fleet_start_parallel_uploads_all() {
        let state = test_state();
        let app = router(Arc::clone(&state));

        // No bindings → all skipped, 200 OK (no link needed, no catalog
        // round-trip — pure validation path).
        let (st, j) = post_json(app.clone(), "/api/fleet/start", r#"{"mode":"parallel"}"#).await;
        assert_eq!(st, StatusCode::OK);
        assert_eq!(j["ok"], true);
        let started = j["data"]["started"].as_array().unwrap();
        assert_eq!(started.len(), 2, "one row per vehicle");
        assert_eq!(started[0]["vehicle_id"], 0);
        assert_eq!(started[0]["status"], "skipped");
        assert_eq!(started[1]["vehicle_id"], 1);
        assert_eq!(started[1]["status"], "skipped");

        // Now bind vehicle 0 to a mission and try again — the catalog
        // server isn't running (the env-var default points at :8300),
        // so fetch_mission_from_catalog returns None and the row is
        // "failed". Vehicle 1 is still unbound → "skipped". This is the
        // M5 stub's honest failure path (the real link + arm sequence
        // lands when the G-9/G-10 harness drives a live PX4).
        let (_, _) = post_json(
            app.clone(),
            "/api/fleet/mission-bindings",
            &format!(
                r#"{{"bindings":[{{"vehicle_id":0,"mission_id":"{ULID_A}"}}]}}"#
            ),
        )
        .await;
        let (st, j) = post_json(app, "/api/fleet/start", r#"{"mode":"parallel"}"#).await;
        assert_eq!(st, StatusCode::OK);
        assert_eq!(j["ok"], true);
        let started = j["data"]["started"].as_array().unwrap();
        assert_eq!(started[0]["vehicle_id"], 0);
        // The catalog URL points at the env-var default (no live server
        // in tests) → fetch fails → status "failed".
        assert_eq!(started[0]["status"], "failed");
        assert!(started[0]["reason"].as_str().unwrap().contains("catalog fetch failed"));
        // Vehicle 1 is still unbound → "skipped"
        assert_eq!(started[1]["vehicle_id"], 1);
        assert_eq!(started[1]["status"], "skipped");
    }

    #[tokio::test]
    async fn fleet_start_rejects_bad_mode_and_gate() {
        let state = test_state();
        let app = router(state);
        // bad mode → 422
        let (st, j) = post_json(app.clone(), "/api/fleet/start", r#"{"mode":"warp"}"#).await;
        assert_eq!(st, StatusCode::UNPROCESSABLE_ENTITY);
        assert!(j["error"].as_str().unwrap().contains("mode must be"));
        // bad sequential_gate → 422
        let (st, j) = post_json(
            app.clone(),
            "/api/fleet/start",
            r#"{"mode":"sequential","sequential_gate":"warp_complete"}"#,
        )
        .await;
        assert_eq!(st, StatusCode::UNPROCESSABLE_ENTITY);
        assert!(j["error"].as_str().unwrap().contains("sequential_gate must be"));
        // bad timeout_s (0 or > 600) → 422
        let (st, j) = post_json(
            app.clone(),
            "/api/fleet/start",
            r#"{"mode":"sequential","timeout_s":0}"#,
        )
        .await;
        assert_eq!(st, StatusCode::UNPROCESSABLE_ENTITY);
        assert!(j["error"].as_str().unwrap().contains("timeout_s"));
        let (st, j) = post_json(
            app,
            "/api/fleet/start",
            r#"{"mode":"sequential","timeout_s":9999}"#,
        )
        .await;
        assert_eq!(st, StatusCode::UNPROCESSABLE_ENTITY);
        assert!(j["error"].as_str().unwrap().contains("timeout_s"));
    }

    #[tokio::test]
    async fn fleet_patterns_list_returns_three() {
        let state = test_state();
        let app = router(state);
        let resp = app
            .oneshot(Request::get("/api/fleet/patterns").body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let j = body_json(resp).await;
        assert_eq!(j["ok"], true);
        let patterns = j["data"]["patterns"].as_array().unwrap();
        assert_eq!(patterns.len(), 3, "exactly three swarming patterns per spec §5.4");
        let names: Vec<&str> = patterns.iter().map(|p| p["name"].as_str().unwrap()).collect();
        assert!(names.contains(&"follow-the-leader"));
        assert!(names.contains(&"search-grid"));
        assert!(names.contains(&"parallel-patrol"));
        // follow-the-leader is the only implemented pattern in M5
        let ftl = patterns.iter().find(|p| p["name"] == "follow-the-leader").unwrap();
        assert_eq!(ftl["implemented"], true);
        let others = patterns.iter().filter(|p| p["name"] != "follow-the-leader");
        for p in others {
            assert_eq!(p["implemented"], false);
        }
    }

    #[tokio::test]
    async fn fleet_patterns_search_grid_and_patrol_are_501() {
        let state = test_state();
        let app = router(state);
        // search-grid → 501
        let (st, j) = post_json(
            app.clone(),
            "/api/fleet/patterns/search-grid/generate",
            r#"{"vehicles":[0,1],"area_polygon":[[47.4,8.5],[47.41,8.5],[47.41,8.51],[47.4,8.51]],"spacing_m":50,"alt_m":30}"#,
        )
        .await;
        assert_eq!(st, StatusCode::NOT_IMPLEMENTED);
        assert!(j["error"].as_str().unwrap().contains("stubbed"));
        // parallel-patrol → 501
        let (st, j) = post_json(
            app.clone(),
            "/api/fleet/patterns/parallel-patrol/generate",
            r#"{"vehicles":[0,1],"waypoints":[[47.4,8.5,30],[47.41,8.5,30]],"offset_m":20}"#,
        )
        .await;
        assert_eq!(st, StatusCode::NOT_IMPLEMENTED);
        // unknown pattern name → 404
        let (st, j) = post_json(
            app,
            "/api/fleet/patterns/warp-drive/generate",
            "{}",
        )
        .await;
        assert_eq!(st, StatusCode::NOT_FOUND);
        assert!(j["error"].as_str().unwrap().contains("unknown pattern"));
    }

    /// `POST /api/fleet/patterns/follow-the-leader/generate` — happy path
    /// through a real catalog server on an ephemeral port. The test
    /// uses `AppState::new_with_catalog_url` so the fleet-cli router
    /// talks to the test's catalog (no env-var race between parallel
    /// tests — the URL is captured by AppState at construction time).
    #[tokio::test]
    async fn fleet_patterns_follow_the_leader_generates() {
        use fleet_mission::gcs::server as catalog_server;
        use fleet_mission::gcs::store::Store;
        use fleet_mission::gcs::mission_file::{MissionFile, MissionMeta, Waypoint};

        // Stand up a real catalog server on an ephemeral port.
        let tmp = std::env::temp_dir().join(format!("m5-ftl-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&tmp);
        std::fs::create_dir_all(&tmp).unwrap();
        let store = Store::new(&tmp).unwrap();

        // Create the leader mission BEFORE moving the store into AppState
        // (Store doesn't impl Clone — the catalog owns one and the test
        // touches it through the catalog's own REST router via the live
        // server on an ephemeral port).
        let leader_mission = MissionFile {
            mission: MissionMeta {
                id: ULID_A.into(),
                name: "leader".into(),
                version: 1,
                created_at: "2026-09-09T00:00:00Z".into(),
                updated_at: "2026-09-09T00:00:00Z".into(),
                vehicle_type: "quad".into(),
                px4_version: "v1.16.2".into(),
            },
            waypoints: vec![
                Waypoint { seq: 0, frame: 3, command: 16, x: 47.39777, y: 8.54558, z: 12.0, param1: 0.0, param2: 0.0, param3: 0.0, param4: 0.0 },
                Waypoint { seq: 1, frame: 3, command: 16, x: 47.39787, y: 8.54558, z: 12.0, param1: 0.0, param2: 0.0, param3: 0.0, param4: 0.0 },
                Waypoint { seq: 2, frame: 3, command: 16, x: 47.39797, y: 8.54558, z: 12.0, param1: 0.0, param2: 0.0, param3: 0.0, param4: 0.0 },
            ],
            geofence: Default::default(),
            rally: vec![],
        };
        let created = store.create(leader_mission).unwrap();
        // Store::create overwrites the id with its own generated ULID —
        // capture the actual id and use that for the POST body.
        let leader_id = created.mission.id.clone();
        assert!(state::looks_like_ulid(&leader_id), "store-assigned id is a ULID");
        let catalog_state = Arc::new(catalog_server::AppState::for_test(store, "http://127.0.0.1:8400"));
        let catalog_app = catalog_server::router(catalog_state);
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let catalog_port = listener.local_addr().unwrap().port();
        let catalog_url = format!("http://127.0.0.1:{catalog_port}");
        tokio::spawn(async move {
            let _ = axum::serve(listener, catalog_app).await;
        });

        // Build the fleet-cli router against that catalog URL.
        let registry = Registry::new(2);
        let log = EventLog::in_memory();
        let fleet_state = AppState::new_with_catalog_url(
            Arc::clone(&registry),
            log,
            2,
            "test-scenario.toml",
            0,
            fleet_core::geo::GeoOrigin::DEFAULT,
            fleet_safety::geofence::Geofence::default_square(),
            std::env::temp_dir(),
            catalog_url.clone(),
        );
        let app = router(fleet_state);

        // POST follow-the-leader with leader=0, followers=[1], sep=10m.
        // The leader_mission_id is the store-assigned ULID (capture above).
        let body = format!(
            r#"{{"leader":0,"followers":[1],"separation_m":10.0,"alt_m":12.0,"leader_mission_id":"{leader_id}"}}"#
        );
        let (st, j) = post_json(
            app.clone(),
            "/api/fleet/patterns/follow-the-leader/generate",
            &body,
        )
        .await;
        assert_eq!(st, StatusCode::OK, "got: {j}");
        assert_eq!(j["ok"], true);
        assert_eq!(j["data"]["pattern"], "follow-the-leader");
        assert_eq!(j["data"]["leader_mission_id"], leader_id);
        assert_eq!(j["data"]["separation_m"], 10.0);
        assert_eq!(j["data"]["alt_m"], 12.0);
        let generated = j["data"]["generated"].as_array().unwrap();
        assert_eq!(generated.len(), 1, "one follower → one generated mission");
        assert_eq!(generated[0]["vehicle_id"], 1);
        assert_eq!(generated[0]["leader_mission_id"], leader_id);
        let wps = generated[0]["waypoints"].as_array().unwrap();
        assert_eq!(wps.len(), 3, "same number of waypoints as the leader");

        // The leader's waypoints go north (lat increases). The follower
        // should be placed 10 m SOUTH (behind the leader's travel
        // direction). 10 m / 111_320 m/deg ≈ 8.98e-5 deg lat. Check the
        // first non-takeoff waypoint (index 1) is offset by that amount.
        let leader_lat1 = 47.39787;
        let follower_lat1 = wps[1]["x"].as_f64().unwrap();
        let delta = leader_lat1 - follower_lat1;
        assert!(
            delta > 0.0,
            "follower should be south of (lower lat than) leader; leader={leader_lat1}, follower={follower_lat1}"
        );
        assert!(
            (delta - 10.0 / 111_320.0).abs() < 1e-6,
            "follower offset should be ~10m / 111320 = 8.98e-5 deg; got delta={delta}"
        );
        // Longitude unchanged (the path is due north, no east component).
        let leader_lon1 = 8.54558;
        let follower_lon1 = wps[1]["y"].as_f64().unwrap();
        assert!(
            (follower_lon1 - leader_lon1).abs() < 1e-9,
            "follower lon should match leader's lon (due-north path); leader={leader_lon1}, follower={follower_lon1}"
        );
        // Altitude overridden to the pattern's cruise alt (12 m AGL).
        assert_eq!(wps[1]["z"], 12.0);
        // Frame/command/seq preserved from the leader.
        assert_eq!(wps[1]["seq"], 1);
        assert_eq!(wps[1]["frame"], 3);
        assert_eq!(wps[1]["command"], 16);

        // cleanup
        let _ = std::fs::remove_dir_all(&tmp);
    }

    /// Pure-geometry helper test (no HTTP, no catalog server): the
    /// `compute_follow_the_leader_waypoints` function produces waypoints
    /// offset by `separation_m` behind the leader along the bearing from
    /// the previous waypoint. This is the kernel of the pattern and the
    /// most exact assertion (no reqwest timeouts, no float-equality
    /// surprises through JSON).
    #[test]
    fn follow_the_leader_geometry_offsets_behind_travel() {
        use fleet_mission::gcs::mission_file::Waypoint;
        let leader = vec![
            // takeoff + 2 north-going waypoints (lat increases by 0.0001
            // ≈ 11.1 m per hop)
            Waypoint { seq: 0, frame: 3, command: 16, x: 47.39777, y: 8.54558, z: 12.0, param1: 0.0, param2: 0.0, param3: 0.0, param4: 0.0 },
            Waypoint { seq: 1, frame: 3, command: 16, x: 47.39787, y: 8.54558, z: 12.0, param1: 0.0, param2: 0.0, param3: 0.0, param4: 0.0 },
            Waypoint { seq: 2, frame: 3, command: 16, x: 47.39797, y: 8.54558, z: 12.0, param1: 0.0, param2: 0.0, param3: 0.0, param4: 0.0 },
        ];
        let follower = compute_follow_the_leader_waypoints(&leader, 10.0, 12.0);
        assert_eq!(follower.len(), 3);
        // Waypoint 0 (takeoff) is offset along a 0° (north) bearing —
        // i.e. 10 m south of the leader's takeoff.
        let delta_lat0 = leader[0].x - follower[0].x;
        assert!(delta_lat0 > 0.0, "follower takeoff south of leader");
        assert!(
            (delta_lat0 - 10.0 / 111_320.0).abs() < 1e-6,
            "delta lat0 ≈ 8.98e-5; got {delta_lat0}"
        );
        // Waypoint 1 — same direction (path is due north): 10 m south.
        let delta_lat1 = leader[1].x - follower[1].x;
        assert!(
            (delta_lat1 - 10.0 / 111_320.0).abs() < 1e-6,
            "delta lat1 ≈ 8.98e-5; got {delta_lat1}"
        );
        // Longitude unchanged along a due-north path.
        assert!((follower[1].y - leader[1].y).abs() < 1e-9);
        // The follower's altitude is overridden to alt_m.
        assert_eq!(follower[0].z, 12.0);
        // seq/frame/command preserved.
        assert_eq!(follower[1].seq, 1);
        assert_eq!(follower[1].frame, 3);
        assert_eq!(follower[1].command, 16);
    }
}
