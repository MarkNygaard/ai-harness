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

/// Note who just changed a workflow.
///
/// **Best-effort, and deliberately so.** The workflow is already saved to disk
/// by the time this runs; failing the request because a note about it could not
/// be written would throw away work somebody just did, over bookkeeping. An
/// install with no database therefore authors workflows exactly as before, with
/// nobody recorded — which is the same thing a missing row already means.
pub(crate) async fn record_edit(
    runs: &Arc<super::runs_routes::RunsState>,
    headers: &axum::http::HeaderMap,
    name: &str,
    source: &str,
) {
    let (user_id, actor) = super::accounts::caller_trigger(runs, headers).await;
    record_edit_as(runs, name, user_id.as_deref(), actor.as_deref(), source).await
}

/// [`record_edit`] for a caller already resolved — the MCP endpoint works out
/// whose token it is holding once per request, and re-reading the headers here
/// would be a second lookup for an answer it already has.
pub(crate) async fn record_edit_as(
    runs: &Arc<super::runs_routes::RunsState>,
    name: &str,
    user_id: Option<&str>,
    actor: Option<&str>,
    source: &str,
) {
    let store = match runs.workflow_author_store().await {
        Ok(s) => s,
        Err(e) => {
            tracing::debug!("authoring: not recording who edited {name}: {e}");
            return;
        }
    };
    let editor = harness_persist::Editor {
        user_id,
        actor,
        source: Some(source),
    };
    if let Err(e) = store.record_edit(name, &editor).await {
        tracing::warn!("authoring: could not record who edited {name}: {e}");
    }
}

/// Where an authoring change came from. The editor and an MCP client are the
/// only two ways in, and telling them apart is half of what the record is for.
pub(crate) const EDIT_SOURCE_UI: &str = "ui";
pub(crate) const EDIT_SOURCE_MCP: &str = "mcp";
/// Not authored here at all — brought in from the library by whoever pressed
/// Install. Worth its own value: "created by" on a library workflow means
/// "installed by", and conflating it with hand-authoring would misattribute
/// somebody else's work.
pub(crate) const EDIT_SOURCE_LIBRARY: &str = "library";

/// `GET /api/authoring/catalog`
pub async fn get_catalog(State(state): State<Arc<AppState>>) -> Response {
    let creds = crate::http::credentials_routes::connected_clis().await;
    Json(authoring::catalog(&state.core.project_root, creds)).into_response()
}

/// `GET /api/authoring/workflows`
///
/// Each custom workflow carries its authorship where one is recorded. Joined in
/// here rather than stored beside the file, and absent rather than empty when
/// unknown — a bundled workflow has no author at all, and one dropped into the
/// directory by hand has none we know of.
pub async fn list_workflows(
    State(state): State<Arc<AppState>>,
    axum::extract::Extension(runs): axum::extract::Extension<Arc<super::runs_routes::RunsState>>,
) -> Response {
    let workflows = authoring::list_workflows(&state.core.project_root);
    let authors: std::collections::HashMap<String, harness_persist::WorkflowAuthorship> =
        match runs.workflow_author_store().await {
            Ok(store) => store
                .all()
                .await
                .unwrap_or_default()
                .into_iter()
                .map(|a| (a.name.clone(), a))
                .collect(),
            // No database: the list is exactly what it always was.
            Err(_) => Default::default(),
        };
    let rows: Vec<serde_json::Value> = workflows
        .into_iter()
        .map(|w| {
            let author = authors.get(&w.name);
            let mut row = serde_json::to_value(&w).unwrap_or_else(|_| serde_json::json!({}));
            if let (Some(obj), Some(a)) = (row.as_object_mut(), author) {
                obj.insert(
                    "authorship".into(),
                    serde_json::to_value(a).unwrap_or(serde_json::Value::Null),
                );
            }
            row
        })
        .collect();
    Json(rows).into_response()
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
    axum::extract::Extension(runs): axum::extract::Extension<Arc<super::runs_routes::RunsState>>,
    headers: axum::http::HeaderMap,
    Json(req): Json<authoring::SaveWorkflow>,
) -> Response {
    match authoring::save_workflow(&state.core.project_root, &req.name, &req.yaml) {
        Ok(()) => {
            record_edit(&runs, &headers, &req.name, EDIT_SOURCE_UI).await;
            Json(serde_json::json!({ "saved": true, "name": req.name })).into_response()
        }
        Err(e) => err(StatusCode::BAD_REQUEST, e),
    }
}

/// `DELETE /api/authoring/workflows/{name}` — remove a project override so a
/// bundled workflow reverts to its built-in default. A no-op (`reset: false`)
/// when there's no project copy; never deletes a bundled default.
pub async fn delete_workflow(
    State(state): State<Arc<AppState>>,
    axum::extract::Extension(runs): axum::extract::Extension<Arc<super::runs_routes::RunsState>>,
    Path(name): Path<String>,
) -> Response {
    match authoring::delete_project_workflow(&state.core.project_root, &name) {
        Ok(reset) => {
            // Only when a file actually went away. Forgetting on a no-op would
            // discard the provenance of a workflow that is still there.
            if reset {
                if let Ok(store) = runs.workflow_author_store().await {
                    if let Err(e) = store.forget(&name).await {
                        tracing::warn!("authoring: could not forget who wrote {name}: {e}");
                    }
                }
            }
            Json(serde_json::json!({ "reset": reset, "name": name })).into_response()
        }
        Err(e) => err(StatusCode::BAD_REQUEST, e),
    }
}

/// Echo the resulting workflow's node summaries after a mutation so the client
/// sees the new DAG state (the build→validate→fix loop).
///
/// Every node-level authoring change routes through here, which is also why the
/// edit is recorded here: one place, so a new mutation cannot be added that
/// quietly forgets to say who made it.
async fn mutation_result(
    runs: &Arc<super::runs_routes::RunsState>,
    headers: &axum::http::HeaderMap,
    root: &std::path::Path,
    name: &str,
    r: Result<(), String>,
) -> Response {
    match r {
        Ok(()) => {
            record_edit(runs, headers, name, EDIT_SOURCE_UI).await;
            match authoring::get_workflow(root, name) {
                Ok(src) => Json(authoring::validate_workflow(&src.yaml)).into_response(),
                Err(e) => err(StatusCode::INTERNAL_SERVER_ERROR, e),
            }
        }
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
    axum::extract::Extension(runs): axum::extract::Extension<Arc<super::runs_routes::RunsState>>,
    headers: axum::http::HeaderMap,
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
    mutation_result(&runs, &headers, root, &req.name, r).await
}

#[derive(Deserialize)]
pub struct SetNodeBody {
    pub name: String,
    pub node: serde_json::Value,
}

/// `POST /api/authoring/set-node` — add or replace a node by id.
pub async fn set_node(
    State(state): State<Arc<AppState>>,
    axum::extract::Extension(runs): axum::extract::Extension<Arc<super::runs_routes::RunsState>>,
    headers: axum::http::HeaderMap,
    Json(req): Json<SetNodeBody>,
) -> Response {
    let root = &state.core.project_root;
    let r = authoring::set_node(root, &req.name, req.node);
    mutation_result(&runs, &headers, root, &req.name, r).await
}

#[derive(Deserialize)]
pub struct SetUiBody {
    pub name: String,
    /// The `ui` block (`{ nav?, report? }`), or `null` to clear it.
    #[serde(default)]
    pub ui: serde_json::Value,
}

/// `POST /api/authoring/set-ui` — set or clear a workflow's `ui:` block.
pub async fn set_ui(
    State(state): State<Arc<AppState>>,
    axum::extract::Extension(runs): axum::extract::Extension<Arc<super::runs_routes::RunsState>>,
    headers: axum::http::HeaderMap,
    Json(req): Json<SetUiBody>,
) -> Response {
    let root = &state.core.project_root;
    let r = authoring::set_ui(root, &req.name, req.ui);
    mutation_result(&runs, &headers, root, &req.name, r).await
}

#[derive(Deserialize)]
pub struct RemoveNodeBody {
    pub name: String,
    pub id: String,
}

/// `POST /api/authoring/remove-node` — delete a node and strip it from dependents.
pub async fn remove_node(
    State(state): State<Arc<AppState>>,
    axum::extract::Extension(runs): axum::extract::Extension<Arc<super::runs_routes::RunsState>>,
    headers: axum::http::HeaderMap,
    Json(req): Json<RemoveNodeBody>,
) -> Response {
    let root = &state.core.project_root;
    let r = authoring::remove_node(root, &req.name, &req.id);
    mutation_result(&runs, &headers, root, &req.name, r).await
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
    axum::extract::Extension(runs): axum::extract::Extension<Arc<super::runs_routes::RunsState>>,
    headers: axum::http::HeaderMap,
    Json(req): Json<ConnectBody>,
) -> Response {
    let root = &state.core.project_root;
    let r = authoring::connect_nodes(root, &req.name, &req.from, &req.to);
    mutation_result(&runs, &headers, root, &req.name, r).await
}
