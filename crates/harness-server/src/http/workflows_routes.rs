//! Workflow **authoring** API (visual editor + MCP) — thin HTTP over
//! [`harness_runner::authoring`], the shared core. Workflows are **global**: all
//! handlers operate on the cluster's `.harness/workflows` + the bundled defaults
//! (there is no per-project workflow storage).
//!
//! - `GET  /api/authoring/catalog`          — building blocks (kinds, providers, commands)
//! - `GET  /api/authoring/workflows`        — list (bundled + custom)
//! - `GET  /api/authoring/workflows/{name}` — a workflow's editable source
//! - `POST /api/authoring/validate`         — `{yaml}` → structural validation
//! - `POST /api/authoring/workflows`        — `{name, yaml}` → validate + save
//! - `POST /api/authoring/create`           — `{name, …}` → new empty workflow
//! - `POST /api/authoring/set-node`         — `{name, node}` → add/replace a node
//! - `POST /api/authoring/set-ui`           — `{name, ui}` → set/clear the `ui:` block
//! - `POST /api/authoring/remove-node`      — `{name, id}` → delete a node
//! - `POST /api/authoring/connect`          — `{name, from, to}` → add an edge

use std::sync::Arc;

use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::Json;
use harness_runner::authoring;
use serde::Deserialize;

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

/// Echo the resulting workflow's node summaries after a mutation so the client
/// sees the new DAG state (the build→validate→fix loop).
fn mutation_result(root: &std::path::Path, name: &str, r: Result<(), String>) -> Response {
    match r {
        Ok(()) => match authoring::get_workflow(root, name) {
            Ok(src) => Json(authoring::validate_workflow(&src.yaml)).into_response(),
            Err(e) => err(StatusCode::INTERNAL_SERVER_ERROR, e),
        },
        Err(e) => err(StatusCode::BAD_REQUEST, e),
    }
}

#[derive(Deserialize)]
pub struct CreateBody {
    pub name: String,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub provider: Option<String>,
    #[serde(default)]
    pub model: Option<String>,
}

/// `POST /api/authoring/create` — new empty workflow.
pub async fn create_workflow(
    State(state): State<Arc<AppState>>,
    Json(req): Json<CreateBody>,
) -> Response {
    let root = &state.core.project_root;
    let r = authoring::create_workflow(
        root,
        &req.name,
        req.description.as_deref(),
        req.provider.as_deref(),
        req.model.as_deref(),
    );
    mutation_result(root, &req.name, r)
}

#[derive(Deserialize)]
pub struct SetNodeBody {
    pub name: String,
    pub node: serde_json::Value,
}

/// `POST /api/authoring/set-node` — add or replace a node by id.
pub async fn set_node(
    State(state): State<Arc<AppState>>,
    Json(req): Json<SetNodeBody>,
) -> Response {
    let root = &state.core.project_root;
    let r = authoring::set_node(root, &req.name, req.node);
    mutation_result(root, &req.name, r)
}

#[derive(Deserialize)]
pub struct SetUiBody {
    pub name: String,
    /// The `ui` block (`{ nav?, report? }`), or `null` to clear it.
    #[serde(default)]
    pub ui: serde_json::Value,
}

/// `POST /api/authoring/set-ui` — set or clear a workflow's `ui:` block.
pub async fn set_ui(State(state): State<Arc<AppState>>, Json(req): Json<SetUiBody>) -> Response {
    let root = &state.core.project_root;
    let r = authoring::set_ui(root, &req.name, req.ui);
    mutation_result(root, &req.name, r)
}

#[derive(Deserialize)]
pub struct RemoveNodeBody {
    pub name: String,
    pub id: String,
}

/// `POST /api/authoring/remove-node` — delete a node and strip it from dependents.
pub async fn remove_node(
    State(state): State<Arc<AppState>>,
    Json(req): Json<RemoveNodeBody>,
) -> Response {
    let root = &state.core.project_root;
    let r = authoring::remove_node(root, &req.name, &req.id);
    mutation_result(root, &req.name, r)
}

#[derive(Deserialize)]
pub struct ConnectBody {
    pub name: String,
    pub from: String,
    pub to: String,
}

/// `POST /api/authoring/connect` — add a dependency edge (`to` depends on `from`).
pub async fn connect_nodes(
    State(state): State<Arc<AppState>>,
    Json(req): Json<ConnectBody>,
) -> Response {
    let root = &state.core.project_root;
    let r = authoring::connect_nodes(root, &req.name, &req.from, &req.to);
    mutation_result(root, &req.name, r)
}
