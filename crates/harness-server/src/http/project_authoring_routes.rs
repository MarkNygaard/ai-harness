//! **Project-scoped** workflow authoring API — the cluster surface a remote MCP
//! (or the web editor) calls to build workflows in a *registered project's*
//! checkout, rather than one global root.
//!
//! Everything resolves the project name to its on-disk checkout
//! (`projects_dir/<project>`) and delegates to the same
//! [`harness_runner::authoring`] core the local tools use, so behaviour is
//! identical everywhere. All routes live under
//! `/api/projects/{project}/authoring/…`.
//!
//! - `GET  …/catalog`               — building blocks (node kinds, providers, …)
//! - `GET  …/workflows`             — list (bundled + project)
//! - `GET  …/workflows/{name}`      — one workflow's YAML
//! - `POST …/validate`             — validate candidate YAML
//! - `POST …/workflows`            — save raw YAML
//! - `POST …/create`               — new empty workflow
//! - `POST …/set-node`             — add/replace a node (JSON spec)
//! - `POST …/remove-node`          — delete a node
//! - `POST …/connect`              — add a dependency edge

use std::path::PathBuf;
use std::sync::Arc;

use axum::extract::{Extension, Path as AxumPath};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::Json;
use harness_runner::authoring;
use serde::Deserialize;

use super::runs_routes::RunsState;

fn err(status: StatusCode, msg: impl Into<String>) -> Response {
    (status, Json(serde_json::json!({ "error": msg.into() }))).into_response()
}

/// Resolve a registered project to its checkout dir, or a 4xx response.
async fn project_dir(state: &Arc<RunsState>, project: &str) -> Result<PathBuf, Response> {
    let store = state
        .project_store()
        .await
        .map_err(|e| err(StatusCode::SERVICE_UNAVAILABLE, e))?;
    match store.get(project).await {
        Ok(Some(_)) => Ok(state.projects_dir.join(project)),
        Ok(None) => Err(err(
            StatusCode::NOT_FOUND,
            format!("unknown project `{project}`"),
        )),
        Err(e) => Err(err(StatusCode::INTERNAL_SERVER_ERROR, e.to_string())),
    }
}

/// Map an authoring `Result<(), String>` to a response, echoing the resulting
/// workflow's node summaries so a client sees the new DAG state.
fn mutation_result(dir: &std::path::Path, name: &str, r: Result<(), String>) -> Response {
    match r {
        Ok(()) => match authoring::get_workflow(dir, name) {
            Ok(src) => Json(authoring::validate_workflow(&src.yaml)).into_response(),
            Err(e) => err(StatusCode::INTERNAL_SERVER_ERROR, e),
        },
        Err(e) => err(StatusCode::BAD_REQUEST, e),
    }
}

pub async fn get_catalog(
    Extension(state): Extension<Arc<RunsState>>,
    AxumPath(project): AxumPath<String>,
) -> Response {
    match project_dir(&state, &project).await {
        Ok(dir) => {
            let creds = crate::http::credentials_routes::connected_clis().await;
            Json(authoring::catalog(&dir, creds)).into_response()
        }
        Err(resp) => resp,
    }
}

pub async fn list_workflows(
    Extension(state): Extension<Arc<RunsState>>,
    AxumPath(project): AxumPath<String>,
) -> Response {
    match project_dir(&state, &project).await {
        Ok(dir) => Json(authoring::list_workflows(&dir)).into_response(),
        Err(resp) => resp,
    }
}

pub async fn get_workflow(
    Extension(state): Extension<Arc<RunsState>>,
    AxumPath((project, name)): AxumPath<(String, String)>,
) -> Response {
    let dir = match project_dir(&state, &project).await {
        Ok(d) => d,
        Err(resp) => return resp,
    };
    match authoring::get_workflow(&dir, &name) {
        Ok(src) => Json(src).into_response(),
        Err(e) => err(StatusCode::NOT_FOUND, e),
    }
}

#[derive(Deserialize)]
pub struct YamlBody {
    pub yaml: String,
}

pub async fn validate_workflow(
    Extension(state): Extension<Arc<RunsState>>,
    AxumPath(project): AxumPath<String>,
    Json(req): Json<YamlBody>,
) -> Response {
    if let Err(resp) = project_dir(&state, &project).await {
        return resp;
    }
    Json(authoring::validate_workflow(&req.yaml)).into_response()
}

#[derive(Deserialize)]
pub struct SaveBody {
    pub name: String,
    pub yaml: String,
}

pub async fn save_workflow(
    Extension(state): Extension<Arc<RunsState>>,
    AxumPath(project): AxumPath<String>,
    Json(req): Json<SaveBody>,
) -> Response {
    let dir = match project_dir(&state, &project).await {
        Ok(d) => d,
        Err(resp) => return resp,
    };
    match authoring::save_workflow(&dir, &req.name, &req.yaml) {
        Ok(()) => Json(serde_json::json!({ "saved": true, "name": req.name })).into_response(),
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

pub async fn create_workflow(
    Extension(state): Extension<Arc<RunsState>>,
    AxumPath(project): AxumPath<String>,
    Json(req): Json<CreateBody>,
) -> Response {
    let dir = match project_dir(&state, &project).await {
        Ok(d) => d,
        Err(resp) => return resp,
    };
    let r = authoring::create_workflow(
        &dir,
        &req.name,
        req.description.as_deref(),
        req.provider.as_deref(),
        req.model.as_deref(),
    );
    mutation_result(&dir, &req.name, r)
}

#[derive(Deserialize)]
pub struct SetNodeBody {
    pub name: String,
    pub node: serde_json::Value,
}

pub async fn set_node(
    Extension(state): Extension<Arc<RunsState>>,
    AxumPath(project): AxumPath<String>,
    Json(req): Json<SetNodeBody>,
) -> Response {
    let dir = match project_dir(&state, &project).await {
        Ok(d) => d,
        Err(resp) => return resp,
    };
    let r = authoring::set_node(&dir, &req.name, req.node);
    mutation_result(&dir, &req.name, r)
}

#[derive(Deserialize)]
pub struct RemoveNodeBody {
    pub name: String,
    pub id: String,
}

pub async fn remove_node(
    Extension(state): Extension<Arc<RunsState>>,
    AxumPath(project): AxumPath<String>,
    Json(req): Json<RemoveNodeBody>,
) -> Response {
    let dir = match project_dir(&state, &project).await {
        Ok(d) => d,
        Err(resp) => return resp,
    };
    let r = authoring::remove_node(&dir, &req.name, &req.id);
    mutation_result(&dir, &req.name, r)
}

#[derive(Deserialize)]
pub struct ConnectBody {
    pub name: String,
    pub from: String,
    pub to: String,
}

pub async fn connect_nodes(
    Extension(state): Extension<Arc<RunsState>>,
    AxumPath(project): AxumPath<String>,
    Json(req): Json<ConnectBody>,
) -> Response {
    let dir = match project_dir(&state, &project).await {
        Ok(d) => d,
        Err(resp) => return resp,
    };
    let r = authoring::connect_nodes(&dir, &req.name, &req.from, &req.to);
    mutation_result(&dir, &req.name, r)
}
