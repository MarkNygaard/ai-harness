//! Workflow **authoring** API for the visual editor (and, later, the MCP
//! server) — thin HTTP over [`harness_runner::authoring`], the shared core. All
//! handlers operate on the project's `.harness/workflows` + the bundled defaults.
//!
//! - `GET  /api/authoring/catalog`          — building blocks (kinds, providers, commands)
//! - `GET  /api/authoring/workflows`        — list (bundled + project)
//! - `GET  /api/authoring/workflows/{name}` — a workflow's editable source
//! - `POST /api/authoring/validate`         — `{yaml}` → structural validation
//! - `POST /api/authoring/workflows`        — `{name, yaml}` → validate + save

use std::sync::Arc;

use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::Json;
use harness_runner::authoring;

use super::state::AppState;

fn err(status: StatusCode, msg: impl Into<String>) -> Response {
    (status, Json(serde_json::json!({ "error": msg.into() }))).into_response()
}

/// `GET /api/authoring/catalog`
pub async fn get_catalog(State(state): State<Arc<AppState>>) -> Response {
    let creds = crate::http::credentials_routes::connected_clis().await;
    Json(authoring::catalog(&state.core.project_root, creds)).into_response()
}

/// `GET /api/authoring/workflows`
pub async fn list_workflows(State(state): State<Arc<AppState>>) -> Response {
    Json(authoring::list_workflows(&state.core.project_root)).into_response()
}

/// `GET /api/authoring/workflows/{name}`
pub async fn get_workflow(
    State(state): State<Arc<AppState>>,
    Path(name): Path<String>,
) -> Response {
    match authoring::get_workflow(&state.core.project_root, &name) {
        Ok(src) => Json(src).into_response(),
        Err(e) => err(StatusCode::NOT_FOUND, e),
    }
}

/// `POST /api/authoring/validate`
pub async fn validate_workflow(Json(req): Json<authoring::WorkflowYaml>) -> Response {
    Json(authoring::validate_workflow(&req.yaml)).into_response()
}

/// `POST /api/authoring/workflows`
pub async fn save_workflow(
    State(state): State<Arc<AppState>>,
    Json(req): Json<authoring::SaveWorkflow>,
) -> Response {
    match authoring::save_workflow(&state.core.project_root, &req.name, &req.yaml) {
        Ok(()) => Json(serde_json::json!({ "saved": true, "name": req.name })).into_response(),
        Err(e) => err(StatusCode::BAD_REQUEST, e),
    }
}

/// `DELETE /api/authoring/workflows/{name}` — remove a project override so a
/// bundled workflow reverts to its built-in default. A no-op (`reset: false`)
/// when there's no project copy; never deletes a bundled default.
pub async fn delete_workflow(
    State(state): State<Arc<AppState>>,
    Path(name): Path<String>,
) -> Response {
    match authoring::delete_project_workflow(&state.core.project_root, &name) {
        Ok(reset) => Json(serde_json::json!({ "reset": reset, "name": name })).into_response(),
        Err(e) => err(StatusCode::BAD_REQUEST, e),
    }
}
