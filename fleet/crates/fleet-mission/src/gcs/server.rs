//! ADR-0027: The `:8300` mission-catalog + replay server.
//!
//! Exposes the REST API defined in GCS_SPEC.md §7. All mission CRUD
//! goes through the filesystem `Store` (ADR-0020); all validation goes
//! through `validation::validate` (ADR-0026); upload-time version
//! check goes through `version_check::check_vehicle_version` (ADR-0029).
//!
//! The server is a thin axum app; the catalog logic lives in the
//! library modules so unit tests do not need HTTP.

#![forbid(unsafe_code)]

use std::sync::Arc;

use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Json, Response};
use axum::routing::{delete, get, post};
use axum::Router;
use serde_json::json;

use crate::gcs::mission_file::MissionFile;
use crate::gcs::preset::PresetParam;
use crate::gcs::replay::{self, ReplayMeta, ReplaySummary, ReplayTopicData};
use crate::gcs::store::{MissionSummary, Store};
use crate::gcs::validation::{validate as validate_mission, ValidationResult};
use crate::gcs::version_check::{check_vehicle_version, VersionCheckResult};

// ---------------------------------------------------------------------------
// AppState
// ---------------------------------------------------------------------------

pub struct AppState {
    pub store: Store,
    pub fleet_base_url: String, // e.g. "http://127.0.0.1:8400"
    /// Directory scanned by `/api/ulogs` (GCS_SPEC.md §5.5 / ADR-0021).
    /// Defaults to `RSIM_ULOG_DIR` env var or the catalog's own `ulogs/`
    /// subdir; set explicitly in `main.rs` for the catalog binary.
    pub ulogs_dir: std::path::PathBuf,
}

impl AppState {
    /// Build an AppState for tests: ULog dir defaults to the store's own
    /// `ulogs/` subdir so a single `Store::new(temp_dir)` is sufficient.
    pub fn for_test(store: Store, fleet_base_url: &str) -> Self {
        let ulogs_dir = store.ulogs_dir();
        AppState {
            store,
            fleet_base_url: fleet_base_url.into(),
            ulogs_dir,
        }
    }
}

// ---------------------------------------------------------------------------
// Router
// ---------------------------------------------------------------------------

pub fn router(state: Arc<AppState>) -> Router {
    Router::new()
        .route("/", get(index))
        // Mission CRUD
        .route("/api/missions", get(list_missions).post(create_mission))
        .route("/api/missions/{id}", get(get_mission).put(update_mission).delete(delete_mission))
        .route("/api/missions/{id}/validate", post(validate_mission_endpoint))
        .route("/api/missions/{id}/versions/{version}", get(get_mission_version))
        .route("/api/missions/{id}/rollback", post(rollback_mission))
        // Vehicle mission upload/download (version-checked)
        .route("/api/vehicles/{vehicle_id}/mission/upload", post(upload_mission))
        // Vehicle param presets (GCS_SPEC.md §5.3, ADR-0025 — Vehicle Setup)
        .route(
            "/api/vehicles/{vehicle_id}/param-presets",
            get(list_param_presets).post(save_param_preset),
        )
        .route(
            "/api/vehicles/{vehicle_id}/param-presets/{name}",
            delete(delete_param_preset),
        )
        .route(
            "/api/vehicles/{vehicle_id}/param-presets/{name}/load",
            post(load_param_preset),
        )
        // Analyze View (GCS_SPEC.md §5.5 — ULog browse + replay scrub)
        .route("/api/replays", get(list_replays))
        .route("/api/replays/{file}/meta", get(replay_meta))
        .route("/api/replays/{file}/topics", get(replay_topics))
        .route("/api/replays/{file}/data", get(replay_data))
        .route("/api/ulogs", get(list_ulogs))
        .route("/api/ulogs/{file}/topics", get(ulog_topics))
        .route(
            "/api/ulogs/{file}/topics/{topic}/data",
            get(ulog_topic_data),
        )
        // Health
        .route("/api/health", get(health))
        .with_state(state)
}

pub async fn serve(state: Arc<AppState>, port: u16) -> std::io::Result<()> {
    let app = router(state);
    let listener = tokio::net::TcpListener::bind(("127.0.0.1", port)).await?;
    println!(
        "[fleet-catalog] mission catalog: http://127.0.0.1:{port}/api/missions"
    );
    axum::serve(listener, app).await
}

// ---------------------------------------------------------------------------
// Handlers
// ---------------------------------------------------------------------------

async fn index() -> Json<serde_json::Value> {
    Json(json!({
        "service": "fleet-catalog",
        "version": env!("CARGO_PKG_VERSION"),
        "endpoints": [
            "GET /api/missions",
            "POST /api/missions",
            "GET /api/missions/{id}",
            "PUT /api/missions/{id}",
            "DELETE /api/missions/{id}",
            "POST /api/missions/{id}/validate",
            "GET /api/missions/{id}/versions/{version}",
            "POST /api/missions/{id}/rollback",
            "POST /api/vehicles/{vehicle_id}/mission/upload",
            "GET /api/vehicles/{vehicle_id}/param-presets",
            "POST /api/vehicles/{vehicle_id}/param-presets",
            "POST /api/vehicles/{vehicle_id}/param-presets/{name}/load",
            "DELETE /api/vehicles/{vehicle_id}/param-presets/{name}",
            "GET /api/replays",
            "GET /api/replays/{file}/meta",
            "GET /api/replays/{file}/topics",
            "GET /api/replays/{file}/data?from_tick&to_tick&topic",
            "GET /api/ulogs",
            "GET /api/ulogs/{file}/topics",
            "GET /api/ulogs/{file}/topics/{topic}/data?from_s&to_s",
        ]
    }))
}

async fn health() -> Json<serde_json::Value> {
    Json(json!({"ok": true, "service": "fleet-catalog"}))
}

// ---- List missions ---------------------------------------------------------

#[derive(serde::Deserialize)]
struct ListQuery {
    #[serde(default)]
    include_deleted: bool,
}

async fn list_missions(
    State(state): State<Arc<AppState>>,
    Query(q): Query<ListQuery>,
) -> Json<Vec<MissionSummary>> {
    let list = if q.include_deleted {
        state.store.list_all().unwrap_or_default()
    } else {
        state.store.list().unwrap_or_default()
    };
    Json(list)
}

// ---- Create mission -------------------------------------------------------

async fn create_mission(
    State(state): State<Arc<AppState>>,
    body: String,
) -> Response {
    // Accept either TOML or JSON (content-sniff by first non-whitespace char).
    // JSON object starts with '{', JSON array with '[', TOML starts with '['
    // for tables but those are always followed by a word — JSON '[' is followed
    // by '{' or '"' or a digit. The simplest reliable check: try JSON first
    // (fast fail), fall back to TOML.
    let trimmed = body.trim_start();
    let mission: Result<MissionFile, String> = if trimmed.starts_with('{') {
        MissionFile::from_json_str(&body).map_err(|e| e.to_string())
    } else {
        // Try TOML (handles '[mission]', '[[waypoints]]', etc.)
        MissionFile::from_toml_str(&body).map_err(|e| e.to_string())
    };

    let mission = match mission {
        Ok(m) => m,
        Err(e) => {
            return error_response(
                StatusCode::BAD_REQUEST,
                "PARSE_ERROR",
                &e,
            )
        }
    };

    match state.store.create(mission) {
        Ok(created) => (StatusCode::CREATED, Json(json!({"ok": true, "data": created.to_json_value()}))).into_response(),
        Err(e) => error_response(StatusCode::INTERNAL_SERVER_ERROR, "STORE_ERROR", &format!("{e}")),
    }
}

// ---- Get mission ----------------------------------------------------------

async fn get_mission(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> Response {
    match state.store.get(&id) {
        Ok(m) => Json(json!({"ok": true, "data": m.to_json_value()})).into_response(),
        Err(e) => error_response(StatusCode::NOT_FOUND, "NOT_FOUND", &format!("{e}")),
    }
}

async fn get_mission_version(
    State(state): State<Arc<AppState>>,
    Path((id, version)): Path<(String, u32)>,
) -> Response {
    match state.store.get_version(&id, version) {
        Ok(m) => Json(json!({"ok": true, "data": m.to_json_value(), "version": version})).into_response(),
        Err(e) => error_response(StatusCode::NOT_FOUND, "NOT_FOUND", &format!("{e}")),
    }
}

// ---- Update mission -------------------------------------------------------

async fn update_mission(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
    body: String,
) -> Response {
    let mission: Result<MissionFile, String> = if body.trim_start().starts_with('{') {
        MissionFile::from_json_str(&body).map_err(|e| e.to_string())
    } else {
        MissionFile::from_toml_str(&body).map_err(|e| e.to_string())
    };

    let mission = match mission {
        Ok(m) => m,
        Err(e) => return error_response(StatusCode::BAD_REQUEST, "PARSE_ERROR", &e),
    };

    match state.store.update(&id, mission) {
        Ok(updated) => Json(json!({"ok": true, "data": updated.to_json_value()})).into_response(),
        Err(e) => error_response(StatusCode::NOT_FOUND, "NOT_FOUND", &format!("{e}")),
    }
}

// ---- Delete mission -------------------------------------------------------

async fn delete_mission(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> Response {
    match state.store.delete(&id) {
        Ok(()) => Json(json!({"ok": true, "deleted": id})).into_response(),
        Err(e) => error_response(StatusCode::NOT_FOUND, "NOT_FOUND", &format!("{e}")),
    }
}

// ---- Validate mission -----------------------------------------------------

async fn validate_mission_endpoint(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> Response {
    let mission = match state.store.get(&id) {
        Ok(m) => m,
        Err(e) => return error_response(StatusCode::NOT_FOUND, "NOT_FOUND", &format!("{e}")),
    };
    let result: ValidationResult = validate_mission(&mission);
    let status = if result.valid { StatusCode::OK } else { StatusCode::UNPROCESSABLE_ENTITY };
    (status, Json(json!({"ok": result.valid, "data": result}))).into_response()
}

// ---- Rollback mission -----------------------------------------------------

async fn rollback_mission(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
    Query(q): Query<RollbackQuery>,
) -> Response {
    let to_version = match q.to_version {
        Some(v) => v,
        None => return error_response(StatusCode::BAD_REQUEST, "MISSING_PARAM", "query param 'to_version' is required"),
    };
    match state.store.rollback(&id, to_version) {
        Ok(m) => Json(json!({"ok": true, "data": m.to_json_value()})).into_response(),
        Err(e) => error_response(StatusCode::NOT_FOUND, "NOT_FOUND", &format!("{e}")),
    }
}

#[derive(serde::Deserialize)]
struct RollbackQuery {
    to_version: Option<u32>,
}

// ---- Upload mission to vehicle (version-checked) -------------------------

#[derive(serde::Deserialize)]
struct UploadQuery {
    mission_id: String,
}

async fn upload_mission(
    State(state): State<Arc<AppState>>,
    Path(vehicle_id): Path<u8>,
    Query(q): Query<UploadQuery>,
) -> Response {
    // 1. Fetch the mission
    let mission = match state.store.get(&q.mission_id) {
        Ok(m) => m,
        Err(e) => return error_response(StatusCode::NOT_FOUND, "MISSION_NOT_FOUND", &format!("{e}")),
    };

    // 2. Validate
    let vr = validate_mission(&mission);
    if !vr.valid {
        return (
            StatusCode::UNPROCESSABLE_ENTITY,
            Json(json!({
                "ok": false,
                "error": {
                    "code": "VALIDATION_FAILED",
                    "message": "mission failed validation; cannot upload",
                    "details": vr,
                }
            })),
        ).into_response();
    }

    // 3. Version check (ADR-0029)
    let vcr = check_vehicle_version(&state.fleet_base_url, vehicle_id).await;
    if vcr.is_blocking() {
        let (status, code, message) = match &vcr {
            VersionCheckResult::Mismatch { reported_version, required_version, .. } => (
                StatusCode::UPGRADE_REQUIRED,
                "PX4_VERSION_MISMATCH",
                format!("vehicle {vehicle_id} reports PX4 {reported_version}; RustSim GCS v1 requires {required_version}"),
            ),
            VersionCheckResult::Unavailable { timeout_s } => (
                StatusCode::SERVICE_UNAVAILABLE,
                "PX4_VERSION_UNAVAILABLE",
                format!("vehicle {vehicle_id} PX4 version not available after {timeout_s}s"),
            ),
            VersionCheckResult::Ok { .. } => unreachable!(),
        };
        return (
            status,
            Json(json!({
                "ok": false,
                "error": {
                    "code": code,
                    "message": message,
                    "details": vcr,
                }
            })),
        ).into_response();
    }

    // 4. Upload would go here — M1 stub (M2 implements the MAVLink protocol).
    //    For now, return success with a note that the mission is valid and
    //    version-checked; the actual MAVLink upload is M2 scope.
    Json(json!({
        "ok": true,
        "data": {
            "vehicle_id": vehicle_id,
            "mission_id": q.mission_id,
            "status": "validated + version-checked (M1 stub — MAVLink upload is M2)",
            "version_check": vcr,
        }
    })).into_response()
}

// ---------------------------------------------------------------------------
// Vehicle param presets (GCS_SPEC.md §5.3, ADR-0025 — Vehicle Setup)
// ---------------------------------------------------------------------------
//
// The four endpoints QGC's Vehicle-Setup "Parameters" tab calls for its
// preset picker:
//
//   GET    /api/vehicles/{i}/param-presets          — list summaries
//   POST   /api/vehicles/{i}/param-presets          — save current as preset
//   POST   /api/vehicles/{i}/param-presets/{name}/load — load preset body
//   DELETE /api/vehicles/{i}/param-presets/{name}   — delete a preset
//
// All persistence goes through `Store::save_preset`/`load_preset`/… which
// writes TOML at `<catalog>/presets/vehicle_<i>/<name>.toml` (ADR-0025).
// The catalog server is a thin axum layer over the library; the store
// is the source of truth (and the unit-test surface).

/// `GET /api/vehicles/{i}/param-presets` — list saved presets for vehicle
/// `i`. Returns the summary form: `[{name, created_at, param_count}]`
/// (QGC's preset picker shows name + count; the full body is fetched on
/// load). Always 200 — an empty list when the vehicle has no presets.
async fn list_param_presets(
    State(state): State<Arc<AppState>>,
    Path(vehicle_id): Path<u8>,
) -> Response {
    match state.store.list_presets(vehicle_id) {
        Ok(list) => Json(json!({"ok": true, "data": list})).into_response(),
        Err(e) => error_response(
            StatusCode::INTERNAL_SERVER_ERROR,
            "STORE_ERROR",
            &format!("{e}"),
        ),
    }
}

/// `POST /api/vehicles/{i}/param-presets` — save a preset (QGC's "Save As").
///
/// Body: `{"name": "aggressive-corners", "params": [{"id": "...",
/// "value": <f64>, "type": <u8>}, …]}`. `type` is the MAVLink param_type
/// byte (6 = INT32, 9 = REAL32) — the same byte PARAM_SET carries on the
/// wire, so a preset can faithfully reproduce a typed param write.
///
/// Returns: `{"ok": true, "name": "...", "param_count": N}` (GCS_SPEC.md
/// §5.3 — the preset-picker summary form). HTTP 422 + INVALID_NAME when
/// the name fails `validate_preset_name` (path-injection guard).
#[derive(serde::Deserialize)]
struct SavePresetBody {
    name: String,
    #[serde(default)]
    params: Vec<PresetParam>,
}

async fn save_param_preset(
    State(state): State<Arc<AppState>>,
    Path(vehicle_id): Path<u8>,
    body: axum::extract::Json<SavePresetBody>,
) -> Response {
    match state.store.save_preset(vehicle_id, &body.name, body.params.clone()) {
        Ok(saved) => Json(json!({
            "ok": true,
            "name": saved.name,
            "param_count": saved.params.len(),
        })).into_response(),
        Err(e) => match e {
            crate::gcs::store::StoreError::InvalidId(msg) => error_response(
                StatusCode::UNPROCESSABLE_ENTITY,
                "INVALID_NAME",
                &msg,
            ),
            other => error_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                "STORE_ERROR",
                &format!("{other}"),
            ),
        },
    }
}

/// `POST /api/vehicles/{i}/param-presets/{name}/load` — load a preset
/// (QGC's "Load"). Returns the full preset body so the console can
/// apply it via `POST /api/vehicles/{i}/params` one param at a time
/// (the spec's exact flow — §5.3 AC-5.3.2).
///
/// Returns: `{"ok": true, "params": [...]}` — the param list the console
/// posts back to `:8400`. HTTP 404 when the preset does not exist.
async fn load_param_preset(
    State(state): State<Arc<AppState>>,
    Path((vehicle_id, name)): Path<(u8, String)>,
) -> Response {
    match state.store.load_preset(vehicle_id, &name) {
        Ok(preset) => Json(json!({
            "ok": true,
            "name": preset.name,
            "created_at": preset.created_at,
            "params": preset.params,
        })).into_response(),
        Err(e) => match e {
            crate::gcs::store::StoreError::InvalidId(msg) => error_response(
                StatusCode::UNPROCESSABLE_ENTITY,
                "INVALID_NAME",
                &msg,
            ),
            crate::gcs::store::StoreError::NotFound(_) => error_response(
                StatusCode::NOT_FOUND,
                "PRESET_NOT_FOUND",
                &format!("param preset '{name}' not found for vehicle {vehicle_id}"),
            ),
            other => error_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                "STORE_ERROR",
                &format!("{other}"),
            ),
        },
    }
}

/// `DELETE /api/vehicles/{i}/param-presets/{name}` — delete a preset.
/// HTTP 404 when the preset does not exist (QGC's "Delete" button only
/// appears on existing presets, so a 404 here surfaces a real race the
/// operator should see, unlike the idempotent delete some APIs prefer).
async fn delete_param_preset(
    State(state): State<Arc<AppState>>,
    Path((vehicle_id, name)): Path<(u8, String)>,
) -> Response {
    match state.store.delete_preset(vehicle_id, &name) {
        Ok(()) => Json(json!({"ok": true, "deleted": name})).into_response(),
        Err(e) => match e {
            crate::gcs::store::StoreError::InvalidId(msg) => error_response(
                StatusCode::UNPROCESSABLE_ENTITY,
                "INVALID_NAME",
                &msg,
            ),
            crate::gcs::store::StoreError::NotFound(_) => error_response(
                StatusCode::NOT_FOUND,
                "PRESET_NOT_FOUND",
                &format!("param preset '{name}' not found for vehicle {vehicle_id}"),
            ),
            other => error_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                "STORE_ERROR",
                &format!("{other}"),
            ),
        },
    }
}

// ---------------------------------------------------------------------------
// Analyze View — replay browse + scrub (GCS_SPEC.md §5.5)
// ---------------------------------------------------------------------------
//
// The six endpoints the Analyze View calls to (a) list `.replay` + `.ulg`
// artifacts, (b) load a replay's header metadata, (c) enumerate the
// topics a replay exposes (mapped from the 17-float state vector), and
// (d) fetch a topic data range for the strip charts.
//
// All replay parsing goes through `gcs::replay` (a vendored copy of
// `sitsim-sdk::replay`, kept self-contained so `fleet-mission` does not
// pull in the full sim workspace). ULog parsing is delegated to `pyulog`
// per ADR-0021; in this M6 pass the ULog subprocess shim returns 503
// (parser unavailable) when `pyulog` is not on PATH — the file listing
// endpoint still works filesystem-only.

/// `GET /api/replays` — list `.replay` files in the catalog's
/// `replays/` directory. Returns `{ok, data: [ReplaySummary]}` per
/// GCS_SPEC.md §5.5.
async fn list_replays(State(state): State<Arc<AppState>>) -> Response {
    let _: Vec<ReplaySummary> = Vec::new(); // touch import
    match replay::list_replay_files(&state.store.replays_dir()) {
        Ok(list) => Json(json!({"ok": true, "data": list})).into_response(),
        Err(e) => error_response(
            StatusCode::INTERNAL_SERVER_ERROR,
            "STORE_ERROR",
            &format!("{e}"),
        ),
    }
}

/// `GET /api/replays/{file}/meta` — parse the 64-byte header + count
/// records. Returns the header metadata (tick_rate_hz, seed,
/// scenario_sha256, records, virtual_duration_s) per GCS_SPEC.md §5.5.
async fn replay_meta(
    State(state): State<Arc<AppState>>,
    Path(file): Path<String>,
) -> Response {
    let _: Option<ReplayMeta> = None; // touch import
    match replay::read_replay_meta(&state.store.replays_dir(), &file) {
        Ok(meta) => Json(json!({"ok": true, "data": meta})).into_response(),
        Err(e) => match e {
            replay::ReplayError::NotFound(_) => error_response(
                StatusCode::NOT_FOUND,
                "REPLAY_NOT_FOUND",
                &format!("replay file '{file}' not found"),
            ),
            replay::ReplayError::BadHeader(m) => error_response(
                StatusCode::UNPROCESSABLE_ENTITY,
                "REPLAY_PARSE_ERROR",
                &m,
            ),
            _ => error_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                "REPLAY_ERROR",
                &format!("{e}"),
            ),
        },
    }
}

/// `GET /api/replays/{file}/topics` — return the static topic catalogue
/// (mapped from the 17-float state vector layout). The list is constant
/// for any well-formed v1 replay file.
async fn replay_topics(
    State(state): State<Arc<AppState>>,
    Path(file): Path<String>,
) -> Response {
    // Verify the file exists (returns 404 if not).
    if let Err(e) = replay::read_replay_meta(&state.store.replays_dir(), &file) {
        return match e {
            replay::ReplayError::NotFound(_) => error_response(
                StatusCode::NOT_FOUND,
                "REPLAY_NOT_FOUND",
                &format!("replay file '{file}' not found"),
            ),
            replay::ReplayError::BadHeader(m) => error_response(
                StatusCode::UNPROCESSABLE_ENTITY,
                "REPLAY_PARSE_ERROR",
                &m,
            ),
            _ => error_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                "REPLAY_ERROR",
                &format!("{e}"),
            ),
        };
    }
    let topics: Vec<serde_json::Value> = replay::TOPICS
        .iter()
        .map(|t| {
            json!({
                "name": t.name,
                "label": t.label,
                "unit": t.unit,
                "components": t.components,
            })
        })
        .collect();
    Json(json!({"ok": true, "data": topics})).into_response()
}

#[derive(serde::Deserialize)]
struct ReplayDataQuery {
    from_tick: u64,
    to_tick: u64,
    topic: String,
}

/// `GET /api/replays/{file}/data?from_tick&to_tick&topic` — fetch a
/// topic's values across `[from_tick, to_tick]` (inclusive). Returns
/// `{ok, data: ReplayTopicData}` with one data point per tick.
async fn replay_data(
    State(state): State<Arc<AppState>>,
    Path(file): Path<String>,
    Query(q): Query<ReplayDataQuery>,
) -> Response {
    let _: Option<ReplayTopicData> = None; // touch import
    match replay::read_replay_topic(
        &state.store.replays_dir(),
        &file,
        &q.topic,
        q.from_tick,
        q.to_tick,
    ) {
        Ok(data) => Json(json!({"ok": true, "data": data})).into_response(),
        Err(e) => match e {
            replay::ReplayError::NotFound(_) => error_response(
                StatusCode::NOT_FOUND,
                "REPLAY_NOT_FOUND",
                &format!("replay file '{file}' not found"),
            ),
            replay::ReplayError::TopicNotFound(t) => error_response(
                StatusCode::NOT_FOUND,
                "TOPIC_NOT_FOUND",
                &format!("topic '{t}' not found in replay file"),
            ),
            replay::ReplayError::RangeOutOfBounds { from, to, records } => {
                error_response(
                    StatusCode::BAD_REQUEST,
                    "RANGE_OUT_OF_BOUNDS",
                    &format!(
                        "tick range [{from}, {to}] out of bounds (file has {records} records)"
                    ),
                )
            }
            replay::ReplayError::BadHeader(m) => error_response(
                StatusCode::UNPROCESSABLE_ENTITY,
                "REPLAY_PARSE_ERROR",
                &m,
            ),
            replay::ReplayError::BadRecord(m) => error_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                "REPLAY_CRC_ERROR",
                &m,
            ),
            _ => error_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                "REPLAY_ERROR",
                &format!("{e}"),
            ),
        },
    }
}

/// `GET /api/ulogs` — list `.ulg` files in the catalog's ULog
/// directory (GCS_SPEC.md §5.5 / ADR-0021). Filesystem-only — no
/// `pyulog` invocation. The ULog dir is `AppState.ulogs_dir` (defaults
/// to the catalog's own `ulogs/` subdir; overridable via `RSIM_ULOG_DIR`).
async fn list_ulogs(State(state): State<Arc<AppState>>) -> Response {
    match replay::list_ulog_files(&state.ulogs_dir) {
        Ok(list) => Json(json!({"ok": true, "data": list})).into_response(),
        Err(e) => error_response(
            StatusCode::INTERNAL_SERVER_ERROR,
            "STORE_ERROR",
            &format!("{e}"),
        ),
    }
}

/// `GET /api/ulogs/{file}/topics` — return the topic list for a ULog.
/// File existence is checked first (404 if missing). Topic enumeration
/// shells out to `pyulog` per ADR-0021; if `pyulog` is not installed,
/// returns 503 `ULOG_PARSER_UNAVAILABLE` (the documented fallback).
async fn ulog_topics(
    State(state): State<Arc<AppState>>,
    Path(file): Path<String>,
) -> Response {
    let path = state.ulogs_dir.join(&file);
    if !path.exists() {
        return error_response(
            StatusCode::NOT_FOUND,
            "ULOG_NOT_FOUND",
            &format!("ulog file '{file}' not found"),
        );
    }
    if which_pyulog().is_none() {
        return error_response(
            StatusCode::SERVICE_UNAVAILABLE,
            "ULOG_PARSER_UNAVAILABLE",
            "pyulog is not installed on the catalog host; ULog topic enumeration is unavailable (ADR-0021 fallback)",
        );
    }
    match pyulog_topics(&path) {
        Ok(topics) => Json(json!({"ok": true, "data": topics})).into_response(),
        Err(e) => error_response(
            StatusCode::UNPROCESSABLE_ENTITY,
            "ULOG_PARSE_ERROR",
            &e,
        ),
    }
}

#[derive(serde::Deserialize)]
struct ULogTopicDataQuery {
    from_s: Option<f64>,
    to_s: Option<f64>,
}

/// `GET /api/ulogs/{file}/topics/{topic}/data?from_s&to_s` — fetch a
/// topic's data range. Same pyulog-or-503 path as `ulog_topics`.
async fn ulog_topic_data(
    State(state): State<Arc<AppState>>,
    Path((file, topic)): Path<(String, String)>,
    Query(q): Query<ULogTopicDataQuery>,
) -> Response {
    let path = state.ulogs_dir.join(&file);
    if !path.exists() {
        return error_response(
            StatusCode::NOT_FOUND,
            "ULOG_NOT_FOUND",
            &format!("ulog file '{file}' not found"),
        );
    }
    if which_pyulog().is_none() {
        return error_response(
            StatusCode::SERVICE_UNAVAILABLE,
            "ULOG_PARSER_UNAVAILABLE",
            "pyulog is not installed on the catalog host; ULog topic data is unavailable (ADR-0021 fallback)",
        );
    }
    match pyulog_topic_data(&path, &topic, q.from_s, q.to_s) {
        Ok(data) => Json(json!({"ok": true, "data": data})).into_response(),
        Err(e) => error_response(
            StatusCode::UNPROCESSABLE_ENTITY,
            "ULOG_PARSE_ERROR",
            &e,
        ),
    }
}

/// Locate `pyulog` on PATH. Returns `Some` if the `python3 -c 'import
/// pyulog'` probe succeeds (so the catalog can decide between the 503
/// fallback and the 422 parse-error path).
fn which_pyulog() -> Option<()> {
    let out = std::process::Command::new("python3")
        .arg("-c")
        .arg("import pyulog; print(pyulog.__file__)")
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    Some(())
}

/// Spawn `pyulog` to enumerate topics. Returns a JSON array of topic
/// names. (M6 stub — the actual subprocess wiring is the M6.x follow-up;
/// the stub returns an empty list if pyulog succeeds but produces no
/// parseable output.)
fn pyulog_topics(path: &std::path::Path) -> Result<Vec<String>, String> {
    let out = std::process::Command::new("python3")
        .arg("-c")
        .arg(
            "import sys, pyulog; ulg = pyulog.ULog(sys.argv[1]); \
             print('\\n'.join(sorted(d.name for d in ulg.data_list)))",
        )
        .arg(path)
        .output()
        .map_err(|e| format!("pyulog spawn failed: {e}"))?;
    if !out.status.success() {
        return Err(String::from_utf8_lossy(&out.stderr).into_owned());
    }
    let stdout = String::from_utf8_lossy(&out.stdout);
    let topics: Vec<String> = stdout
        .lines()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .collect();
    Ok(topics)
}

/// Spawn `pyulog` to fetch a topic's data range. (M6 stub — returns an
/// empty data array if pyulog succeeds.)
fn pyulog_topic_data(
    path: &std::path::Path,
    topic: &str,
    _from_s: Option<f64>,
    _to_s: Option<f64>,
) -> Result<serde_json::Value, String> {
    let path_str = path.to_string_lossy();
    let _ = std::process::Command::new("python3")
        .arg("-c")
        .arg(format!("import pyulog; pyulog.ULog('{path_str}'); '{topic}'"))
        .output()
        .map_err(|e| format!("pyulog spawn failed: {e}"))?;
    Ok(json!({
        "topic": topic,
        "t": [],
        "values": [],
    }))
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn error_response(status: StatusCode, code: &str, message: &str) -> Response {
    (
        status,
        Json(json!({
            "ok": false,
            "error": {
                "code": code,
                "message": message,
            }
        })),
    ).into_response()
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU64, Ordering};
    use axum::body::Body;
    use axum::http::{Request, Method};
    use tower::ServiceExt;

    static TEST_COUNTER: AtomicU64 = AtomicU64::new(0);

    fn test_store() -> Store {
        let n = TEST_COUNTER.fetch_add(1, Ordering::SeqCst);
        let dir = std::env::temp_dir().join(format!("rustsim_server_test_{}_{n}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        Store::new(&dir).unwrap()
    }

    fn sample_toml(name: &str) -> String {
        format!(
            r#"
[mission]
id = ""
name = "{name}"
version = 0
created_at = ""
updated_at = ""
vehicle_type = "quad"
px4_version = "v1.16.2"

[[waypoints]]
seq = 0
frame = 3
command = 16
x = 47.3975
y = 8.5455
z = 12.0
param2 = 2.0

[geofence]
ceiling_m = 60.0
floor_m = 0.0
inclusion = [
    [47.3970, 8.5450],
    [47.3980, 8.5450],
    [47.3980, 8.5460],
    [47.3970, 8.5460],
]
"#
        )
    }

    async fn send_request(app: Router, method: &str, path: &str, body: &str) -> (StatusCode, serde_json::Value) {
        let req = Request::builder()
            .method(method.parse::<Method>().unwrap_or(Method::GET))
            .uri(path)
            .header("content-type", if body.starts_with('{') { "application/json" } else { "application/toml" })
            .body(Body::from(body.to_string()))
            .unwrap();
        let resp = app.oneshot(req).await.unwrap();
        let status = resp.status();
        let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX).await.unwrap();
        let json: serde_json::Value = serde_json::from_slice(&bytes).unwrap_or(serde_json::Value::Null);
        if status != StatusCode::OK && status != StatusCode::CREATED {
            println!("[test] {method} {path} → {status}: {json}");
        }
        (status, json)
    }

    #[tokio::test]
    async fn create_and_get_mission() {
        let store = test_store();
        let state = Arc::new(AppState::for_test(store, "http://127.0.0.1:8400"));
        let app = router(state);

        // Create
        let (status, body) = send_request(app.clone(), "POST", "/api/missions", &sample_toml("test1")).await;
        assert_eq!(status, StatusCode::CREATED);
        assert!(body["ok"].as_bool().unwrap());
        let id = body["data"]["mission"]["id"].as_str().unwrap().to_string();
        assert!(!id.is_empty());

        // Get
        let path = format!("/api/missions/{id}");
        let (status, body) = send_request(app, "GET", &path, "").await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body["data"]["mission"]["name"], "test1");
    }

    #[tokio::test]
    async fn list_missions() {
        let store = test_store();
        let state = Arc::new(AppState::for_test(store, "http://127.0.0.1:8400"));
        let app = router(state);

        for name in &["m1", "m2"] {
            let _ = send_request(app.clone(), "POST", "/api/missions", &sample_toml(name)).await;
        }

        let (status, body) = send_request(app, "GET", "/api/missions", "").await;
        assert_eq!(status, StatusCode::OK);
        let arr = body.as_array().unwrap();
        assert_eq!(arr.len(), 2);
    }

    #[tokio::test]
    async fn validate_valid_mission() {
        let store = test_store();
        let state = Arc::new(AppState::for_test(store, "http://127.0.0.1:8400"));
        let app = router(state);

        let (_, body) = send_request(app.clone(), "POST", "/api/missions", &sample_toml("valid")).await;
        let id = body["data"]["mission"]["id"].as_str().unwrap();

        let path = format!("/api/missions/{id}/validate");
        let (status, body) = send_request(app, "POST", &path, "").await;
        assert_eq!(status, StatusCode::OK);
        assert!(body["ok"].as_bool().unwrap());
        assert!(body["data"]["valid"].as_bool().unwrap());
    }

    #[tokio::test]
    async fn update_creates_new_version() {
        let store = test_store();
        let state = Arc::new(AppState::for_test(store, "http://127.0.0.1:8400"));
        let app = router(state);

        let (_, body) = send_request(app.clone(), "POST", "/api/missions", &sample_toml("v1")).await;
        let id = body["data"]["mission"]["id"].as_str().unwrap().to_string();

        let path = format!("/api/missions/{id}");
        let (status, body) = send_request(app.clone(), "PUT", &path, &sample_toml("v2")).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body["data"]["mission"]["version"], 2);

        let v1_path = format!("/api/missions/{id}/versions/1");
        let (status, body) = send_request(app, "GET", &v1_path, "").await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body["data"]["mission"]["name"], "v1");
    }

    #[tokio::test]
    async fn delete_soft_deletes() {
        let store = test_store();
        let state = Arc::new(AppState::for_test(store, "http://127.0.0.1:8400"));
        let app = router(state);

        let (_, body) = send_request(app.clone(), "POST", "/api/missions", &sample_toml("del")).await;
        let id = body["data"]["mission"]["id"].as_str().unwrap().to_string();

        let path = format!("/api/missions/{id}");
        let (status, _) = send_request(app.clone(), "DELETE", &path, "").await;
        assert_eq!(status, StatusCode::OK);

        let (status, body) = send_request(app.clone(), "GET", "/api/missions", "").await;
        let arr = body.as_array().unwrap();
        assert_eq!(arr.len(), 0);

        let (status, body) = send_request(app, "GET", "/api/missions?include_deleted=true", "").await;
        let arr = body.as_array().unwrap();
        assert_eq!(arr.len(), 1);
        assert!(arr[0]["deleted"].as_bool().unwrap());
    }

    // -----------------------------------------------------------------
    // Param preset endpoints (M4 — GCS_SPEC.md §5.3)
    // -----------------------------------------------------------------

    fn sample_preset_json(name: &str) -> String {
        serde_json::json!({
            "name": name,
            "params": [
                {"id": "MPC_XY_VEL_MAX", "value": 8.0, "type": 9},
                {"id": "MC_ROLLRATE_P", "value": 7.5, "type": 9},
                {"id": "BAT_N_CELLS", "value": 6.0, "type": 6},
            ]
        }).to_string()
    }

    #[tokio::test]
    async fn param_preset_save_list_load_roundtrip() {
        let store = test_store();
        let state = Arc::new(AppState::for_test(store, "http://127.0.0.1:8400"));
        let app = router(state);

        // Save
        let (status, body) = send_request(
            app.clone(),
            "POST",
            "/api/vehicles/0/param-presets",
            &sample_preset_json("aggressive-corners"),
        ).await;
        assert_eq!(status, StatusCode::OK);
        assert!(body["ok"].as_bool().unwrap());
        assert_eq!(body["name"], "aggressive-corners");
        assert_eq!(body["param_count"], 3);

        // List
        let (status, body) = send_request(
            app.clone(),
            "GET",
            "/api/vehicles/0/param-presets",
            "",
        ).await;
        assert_eq!(status, StatusCode::OK);
        assert!(body["ok"].as_bool().unwrap());
        let arr = body["data"].as_array().unwrap();
        assert_eq!(arr.len(), 1);
        assert_eq!(arr[0]["name"], "aggressive-corners");
        assert_eq!(arr[0]["param_count"], 3);

        // Load
        let (status, body) = send_request(
            app.clone(),
            "POST",
            "/api/vehicles/0/param-presets/aggressive-corners/load",
            "",
        ).await;
        assert_eq!(status, StatusCode::OK);
        assert!(body["ok"].as_bool().unwrap());
        let params = body["params"].as_array().unwrap();
        assert_eq!(params.len(), 3);
        assert_eq!(params[0]["id"], "MPC_XY_VEL_MAX");
        assert_eq!(params[0]["value"], 8.0);
        assert_eq!(params[0]["type"], 9);
        assert_eq!(params[2]["id"], "BAT_N_CELLS");
        assert_eq!(params[2]["type"], 6); // INT32 preserved
    }

    #[tokio::test]
    async fn param_preset_delete_removes_file() {
        let store = test_store();
        let state = Arc::new(AppState::for_test(store, "http://127.0.0.1:8400"));
        let app = router(state);

        // Save then list — 1 preset
        send_request(
            app.clone(),
            "POST",
            "/api/vehicles/0/param-presets",
            &sample_preset_json("to-delete"),
        ).await;
        let (status, body) = send_request(app.clone(), "GET", "/api/vehicles/0/param-presets", "").await;
        assert_eq!(body["data"].as_array().unwrap().len(), 1);

        // Delete then list — 0 presets
        let (status, _) = send_request(
            app.clone(),
            "DELETE",
            "/api/vehicles/0/param-presets/to-delete",
            "",
        ).await;
        assert_eq!(status, StatusCode::OK);
        let (status, body) = send_request(app.clone(), "GET", "/api/vehicles/0/param-presets", "").await;
        assert_eq!(body["data"].as_array().unwrap().len(), 0);

        // Deleting again returns 404
        let (status, body) = send_request(
            app,
            "DELETE",
            "/api/vehicles/0/param-presets/to-delete",
            "",
        ).await;
        assert_eq!(status, StatusCode::NOT_FOUND);
        assert_eq!(body["error"]["code"], "PRESET_NOT_FOUND");
    }

    #[tokio::test]
    async fn param_preset_load_nonexistent_returns_404() {
        let store = test_store();
        let state = Arc::new(AppState::for_test(store, "http://127.0.0.1:8400"));
        let app = router(state);

        let (status, body) = send_request(
            app,
            "POST",
            "/api/vehicles/0/param-presets/never-saved/load",
            "",
        ).await;
        assert_eq!(status, StatusCode::NOT_FOUND);
        assert_eq!(body["error"]["code"], "PRESET_NOT_FOUND");
    }

    #[tokio::test]
    async fn param_preset_invalid_name_returns_422() {
        let store = test_store();
        let state = Arc::new(AppState::for_test(store, "http://127.0.0.1:8400"));
        let app = router(state);

        // Path-traversal name → 422 INVALID_NAME
        let bad_body = sample_preset_json("../escape");
        let (status, body) = send_request(
            app.clone(),
            "POST",
            "/api/vehicles/0/param-presets",
            &bad_body,
        ).await;
        assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
        assert_eq!(body["error"]["code"], "INVALID_NAME");

        // The list is still empty (nothing saved)
        let (_, body) = send_request(app, "GET", "/api/vehicles/0/param-presets", "").await;
        assert_eq!(body["data"].as_array().unwrap().len(), 0);
    }

    #[tokio::test]
    async fn param_presets_isolated_per_vehicle() {
        let store = test_store();
        let state = Arc::new(AppState::for_test(store, "http://127.0.0.1:8400"));
        let app = router(state);

        // Vehicle 0 saves "v0-preset"; vehicle 1 saves "v1-preset"
        send_request(app.clone(), "POST", "/api/vehicles/0/param-presets", &sample_preset_json("v0-preset")).await;
        send_request(app.clone(), "POST", "/api/vehicles/1/param-presets", &sample_preset_json("v1-preset")).await;

        // Each vehicle sees only its own presets
        let (_, v0) = send_request(app.clone(), "GET", "/api/vehicles/0/param-presets", "").await;
        let (_, v1) = send_request(app.clone(), "GET", "/api/vehicles/1/param-presets", "").await;
        assert_eq!(v0["data"].as_array().unwrap().len(), 1);
        assert_eq!(v1["data"].as_array().unwrap().len(), 1);
        assert_eq!(v0["data"].as_array().unwrap()[0]["name"], "v0-preset");
        assert_eq!(v1["data"].as_array().unwrap()[0]["name"], "v1-preset");

        // Vehicle 2 has no presets → empty list, not an error
        let (status, v2) = send_request(app, "GET", "/api/vehicles/2/param-presets", "").await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(v2["data"].as_array().unwrap().len(), 0);
    }

    // ---- Analyze View (GCS_SPEC.md §5.5) ----------------------------------

    /// Helper: write a small valid v1 .replay file into `dir`.
    fn write_test_replay(dir: &std::path::Path, name: &str, n_records: usize) {
        use std::fs;
        use std::io::Write;
        fs::create_dir_all(dir).unwrap();
        let path = dir.join(name);
        let mut f = fs::File::create(&path).unwrap();
        // Header
        let mut header = vec![0u8; crate::gcs::replay::HEADER_LEN];
        header[0..8].copy_from_slice(crate::gcs::replay::MAGIC);
        header[8..10].copy_from_slice(&crate::gcs::replay::VERSION.to_le_bytes());
        header[10..12].copy_from_slice(&(crate::gcs::replay::HEADER_LEN as u16).to_le_bytes());
        let millihz = (200.0f64 * 1000.0).round() as u32;
        header[12..16].copy_from_slice(&millihz.to_le_bytes());
        header[16..24].copy_from_slice(&42u64.to_le_bytes());
        header[24..32].copy_from_slice(&0u64.to_le_bytes());
        for (i, b) in header[32..64].iter_mut().enumerate() {
            *b = (i as u8).wrapping_mul(7);
        }
        f.write_all(&header).unwrap();
        // Records
        for tick in 0..n_records as u64 {
            let mut rec = [0u8; crate::gcs::replay::RECORD_LEN];
            let t_us = tick * 5_000;
            rec[0..8].copy_from_slice(&t_us.to_le_bytes());
            for i in 0..16 {
                rec[8 + i] = (tick as u8).wrapping_add(i as u8);
            }
            let mut state = [0f32; 17];
            for i in 0..3 {
                state[i] = (tick as f32) * 0.1 + i as f32;
            }
            for i in 3..6 {
                state[i] = (tick as f32) * 0.05 + (i - 3) as f32;
            }
            state[6] = 1.0;
            for i in 7..10 {
                state[i] = 0.0;
            }
            for i in 10..17 {
                state[i] = (tick as f32) * 0.01 + (i - 10) as f32;
            }
            for (i, &v) in state.iter().enumerate() {
                rec[24 + 4 * i..28 + 4 * i].copy_from_slice(&v.to_le_bytes());
            }
            rec[92..94].copy_from_slice(&0u16.to_le_bytes());
            // CRC-16/X.25 — same algorithm as gcs::replay::x25_crc
            let mut crc: u16 = 0xFFFF;
            for &b in &rec[..94] {
                crc ^= b as u16;
                for _ in 0..8 {
                    if crc & 1 != 0 {
                        crc = (crc >> 1) ^ 0x8408;
                    } else {
                        crc >>= 1;
                    }
                }
            }
            rec[94..96].copy_from_slice(&crc.to_le_bytes());
            f.write_all(&rec).unwrap();
        }
        f.sync_all().unwrap();
    }

    #[tokio::test]
    async fn replay_list_returns_empty_when_no_files() {
        let store = test_store();
        let state = Arc::new(AppState::for_test(store, "http://127.0.0.1:8400"));
        let app = router(state);
        let (status, body) = send_request(app, "GET", "/api/replays", "").await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body["ok"], true);
        assert_eq!(body["data"].as_array().unwrap().len(), 0);
    }

    #[tokio::test]
    async fn replay_list_returns_files_in_catalog() {
        let store = test_store();
        write_test_replay(&store.replays_dir(), "a.replay", 10);
        write_test_replay(&store.replays_dir(), "b.replay", 5);
        let state = Arc::new(AppState::for_test(store, "http://127.0.0.1:8400"));
        let app = router(state);
        let (status, body) = send_request(app, "GET", "/api/replays", "").await;
        assert_eq!(status, StatusCode::OK);
        let arr = body["data"].as_array().unwrap();
        assert_eq!(arr.len(), 2);
        assert_eq!(arr[0]["filename"], "a.replay");
        assert_eq!(arr[1]["filename"], "b.replay");
        assert_eq!(arr[0]["size_bytes"], (64 + 10 * 96) as u64);
    }

    #[tokio::test]
    async fn replay_meta_returns_header_fields() {
        let store = test_store();
        write_test_replay(&store.replays_dir(), "test.replay", 100);
        let state = Arc::new(AppState::for_test(store, "http://127.0.0.1:8400"));
        let app = router(state);
        let (status, body) = send_request(app, "GET", "/api/replays/test.replay/meta", "").await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body["data"]["filename"], "test.replay");
        assert_eq!(body["data"]["records"], 100);
        assert!((body["data"]["tick_rate_hz"].as_f64().unwrap() - 200.0).abs() < 0.001);
        assert!((body["data"]["virtual_duration_s"].as_f64().unwrap() - 0.5).abs() < 0.001);
        assert_eq!(body["data"]["seed"], 42);
        assert_eq!(body["data"]["scenario_sha256"].as_str().unwrap().len(), 64);
    }

    #[tokio::test]
    async fn replay_meta_missing_file_returns_404() {
        let store = test_store();
        let state = Arc::new(AppState::for_test(store, "http://127.0.0.1:8400"));
        let app = router(state);
        let (status, body) = send_request(app, "GET", "/api/replays/nonexistent.replay/meta", "").await;
        assert_eq!(status, StatusCode::NOT_FOUND);
        assert_eq!(body["error"]["code"], "REPLAY_NOT_FOUND");
    }

    #[tokio::test]
    async fn replay_topics_returns_catalog() {
        let store = test_store();
        write_test_replay(&store.replays_dir(), "test.replay", 5);
        let state = Arc::new(AppState::for_test(store, "http://127.0.0.1:8400"));
        let app = router(state);
        let (status, body) = send_request(app, "GET", "/api/replays/test.replay/topics", "").await;
        assert_eq!(status, StatusCode::OK);
        let arr = body["data"].as_array().unwrap();
        let names: Vec<_> = arr.iter().map(|t| t["name"].as_str().unwrap().to_string()).collect();
        assert!(names.contains(&"pos_ned_m".to_string()));
        assert!(names.contains(&"vel_ned_ms".to_string()));
        assert!(names.contains(&"q_wxyz".to_string()));
        assert!(names.contains(&"omega_rads".to_string()));
        assert!(names.contains(&"rotors".to_string()));
    }

    #[tokio::test]
    async fn replay_data_returns_topic_range() {
        let store = test_store();
        write_test_replay(&store.replays_dir(), "test.replay", 100);
        let state = Arc::new(AppState::for_test(store, "http://127.0.0.1:8400"));
        let app = router(state);
        let path = "/api/replays/test.replay/data?from_tick=0&to_tick=50&topic=pos_ned_m";
        let (status, body) = send_request(app, "GET", path, "").await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body["data"]["topic"], "pos_ned_m");
        assert_eq!(body["data"]["from_tick"], 0);
        assert_eq!(body["data"]["to_tick"], 50);
        let points = body["data"]["points"].as_array().unwrap();
        assert_eq!(points.len(), 51);
        assert_eq!(points[0]["tick"], 0);
        assert_eq!(points[50]["tick"], 50);
        // pos_ned_m at tick 0 = [0.0, 1.0, 2.0]; at tick 1 = [0.1, 1.0, 2.0] (i as f32 + tick * 0.1)
        let v0 = points[0]["values"].as_array().unwrap();
        assert_eq!(v0.len(), 3);
        assert!((v0[0].as_f64().unwrap() - 0.0).abs() < 0.001);
    }

    #[tokio::test]
    async fn replay_data_unknown_topic_returns_404() {
        let store = test_store();
        write_test_replay(&store.replays_dir(), "test.replay", 5);
        let state = Arc::new(AppState::for_test(store, "http://127.0.0.1:8400"));
        let app = router(state);
        let path = "/api/replays/test.replay/data?from_tick=0&to_tick=4&topic=not_a_topic";
        let (status, body) = send_request(app, "GET", path, "").await;
        assert_eq!(status, StatusCode::NOT_FOUND);
        assert_eq!(body["error"]["code"], "TOPIC_NOT_FOUND");
    }

    #[tokio::test]
    async fn replay_data_range_out_of_bounds_returns_400() {
        let store = test_store();
        write_test_replay(&store.replays_dir(), "test.replay", 10);
        let state = Arc::new(AppState::for_test(store, "http://127.0.0.1:8400"));
        let app = router(state);
        let path = "/api/replays/test.replay/data?from_tick=0&to_tick=10&topic=pos_ned_m";
        let (status, body) = send_request(app, "GET", path, "").await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
        assert_eq!(body["error"]["code"], "RANGE_OUT_OF_BOUNDS");
    }

    #[tokio::test]
    async fn ulogs_list_returns_empty_when_no_files() {
        let store = test_store();
        let state = Arc::new(AppState::for_test(store, "http://127.0.0.1:8400"));
        let app = router(state);
        let (status, body) = send_request(app, "GET", "/api/ulogs", "").await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body["ok"], true);
        assert_eq!(body["data"].as_array().unwrap().len(), 0);
    }

    #[tokio::test]
    async fn ulog_topics_missing_file_returns_404() {
        let store = test_store();
        let state = Arc::new(AppState::for_test(store, "http://127.0.0.1:8400"));
        let app = router(state);
        let (status, body) = send_request(app, "GET", "/api/ulogs/nonexistent.ulg/topics", "").await;
        assert_eq!(status, StatusCode::NOT_FOUND);
        assert_eq!(body["error"]["code"], "ULOG_NOT_FOUND");
    }
}
