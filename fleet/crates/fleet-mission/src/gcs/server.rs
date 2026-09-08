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
use axum::routing::{get, post};
use axum::Router;
use serde_json::json;

use crate::gcs::mission_file::MissionFile;
use crate::gcs::store::{MissionSummary, Store};
use crate::gcs::validation::{validate as validate_mission, ValidationResult};
use crate::gcs::version_check::{check_vehicle_version, VersionCheckResult};

// ---------------------------------------------------------------------------
// AppState
// ---------------------------------------------------------------------------

pub struct AppState {
    pub store: Store,
    pub fleet_base_url: String, // e.g. "http://127.0.0.1:8400"
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
        let state = Arc::new(AppState {
            store,
            fleet_base_url: "http://127.0.0.1:8400".into(),
        });
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
        let state = Arc::new(AppState {
            store,
            fleet_base_url: "http://127.0.0.1:8400".into(),
        });
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
        let state = Arc::new(AppState {
            store,
            fleet_base_url: "http://127.0.0.1:8400".into(),
        });
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
        let state = Arc::new(AppState {
            store,
            fleet_base_url: "http://127.0.0.1:8400".into(),
        });
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
        let state = Arc::new(AppState {
            store,
            fleet_base_url: "http://127.0.0.1:8400".into(),
        });
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
}
