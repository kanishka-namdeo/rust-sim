//! REST + WS control plane (spec §3.4) on 127.0.0.1:8400.
//!
//! Envelope: `{"ok": bool, "data": ..., "error": ...}` — identical to
//! rustsitsim's spec §4.1 shape so dashboard client code is shared.
//!
//! WS path tolerance: the sandbox preview gateway forwards WebSockets as
//! `/?XTransformPort=8400` (the port rides in the *query*, path is `/`), so
//! the WS handler is registered on `/ws/fleet`, `/ws` **and** `/`. The root
//! also answers plain GET with a JSON index (non-upgrade requests).

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
}
