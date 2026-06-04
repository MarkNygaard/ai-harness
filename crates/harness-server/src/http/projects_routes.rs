//! Project registry API.
//!
//! A **project** scopes runs to a git repo (the harness-dag run model). Runs are
//! triggered *within* a project and operate on its checkout; the registry here
//! owns the rows + the on-disk working copy, while credentials stay global.
//!
//! - `GET    /api/projects`        — list registered projects
//! - `POST   /api/projects`        — register (or update) a project; clones its repo
//! - `GET    /api/projects/{name}` — one project
//! - `DELETE /api/projects/{name}` — deregister (also removes the checkout)
//!
//! Cloning a private repo uses the **global** `github` credential's `token`.

use std::path::PathBuf;
use std::sync::Arc;

use axum::extract::{Extension, Path as AxumPath};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::Json;
use harness_persist::ProjectInput;
use serde::Deserialize;

use super::runs_routes::RunsState;

fn err(status: StatusCode, msg: impl Into<String>) -> Response {
    (status, Json(serde_json::json!({ "error": msg.into() }))).into_response()
}

/// A project name must be a safe slug (it's also an on-disk directory name).
fn valid_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 64
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
}

/// `GET /api/projects` — list registered projects.
pub async fn list_projects(Extension(state): Extension<Arc<RunsState>>) -> Response {
    let store = match state.project_store().await {
        Ok(s) => s,
        Err(e) => return err(StatusCode::SERVICE_UNAVAILABLE, e),
    };
    match store.list().await {
        Ok(projects) => Json(projects).into_response(),
        Err(e) => err(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()),
    }
}

/// `GET /api/projects/{name}` — one project.
pub async fn get_project(
    Extension(state): Extension<Arc<RunsState>>,
    AxumPath(name): AxumPath<String>,
) -> Response {
    let store = match state.project_store().await {
        Ok(s) => s,
        Err(e) => return err(StatusCode::SERVICE_UNAVAILABLE, e),
    };
    match store.get(&name).await {
        Ok(Some(p)) => Json(p).into_response(),
        Ok(None) => err(StatusCode::NOT_FOUND, format!("project `{name}` not found")),
        Err(e) => err(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()),
    }
}

#[derive(Debug, Deserialize)]
pub struct RegisterProjectRequest {
    pub name: String,
    pub git_url: String,
    #[serde(default = "default_branch")]
    pub base_branch: String,
    #[serde(default)]
    pub default_workflow: Option<String>,
}

fn default_branch() -> String {
    "main".to_string()
}

/// `POST /api/projects` — register/update a project and ensure its repo is
/// cloned into `projects_dir/<name>`. Idempotent: re-registering an existing
/// project updates the row and `git fetch`es the existing checkout.
pub async fn register_project(
    Extension(state): Extension<Arc<RunsState>>,
    Json(req): Json<RegisterProjectRequest>,
) -> Response {
    if !valid_name(&req.name) {
        return err(
            StatusCode::BAD_REQUEST,
            "name must be 1–64 chars of [A-Za-z0-9_-]",
        );
    }
    if req.git_url.trim().is_empty() {
        return err(StatusCode::BAD_REQUEST, "git_url is required");
    }

    let store = match state.project_store().await {
        Ok(s) => s,
        Err(e) => return err(StatusCode::SERVICE_UNAVAILABLE, e),
    };

    let input = ProjectInput {
        git_url: req.git_url.trim().to_string(),
        base_branch: req.base_branch.trim().to_string(),
        default_workflow: req.default_workflow.filter(|w| !w.trim().is_empty()),
    };
    let project = match store.upsert(&req.name, &input).await {
        Ok(p) => p,
        Err(e) => return err(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()),
    };

    // Bring the on-disk checkout in line: clone if absent, else fetch.
    let dest: PathBuf = state.projects_dir.join(&req.name);
    let token = state.github_token().await;
    let git_url = project.git_url.clone();
    let exists = dest.exists();
    let git_result = tokio::task::spawn_blocking(move || {
        if exists {
            harness_runner::fetch_repo(&dest, token.as_deref())
        } else {
            if let Some(parent) = dest.parent() {
                std::fs::create_dir_all(parent).map_err(|e| {
                    harness_runner::WorktreeError(format!("create projects dir: {e}"))
                })?;
            }
            harness_runner::clone_repo(&git_url, &dest, token.as_deref())
        }
    })
    .await;

    match git_result {
        Ok(Ok(())) => Json(project).into_response(),
        // Registry row is saved; surface the git failure so the UI can show it
        // (e.g. bad URL / missing token) without losing the registration.
        Ok(Err(e)) => (
            StatusCode::ACCEPTED,
            Json(serde_json::json!({
                "project": project,
                "warning": format!("registered, but repo sync failed: {e}"),
            })),
        )
            .into_response(),
        Err(e) => err(
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("git task panicked: {e}"),
        ),
    }
}

/// `DELETE /api/projects/{name}` — deregister and remove the checkout.
pub async fn delete_project(
    Extension(state): Extension<Arc<RunsState>>,
    AxumPath(name): AxumPath<String>,
) -> Response {
    if !valid_name(&name) {
        return err(StatusCode::BAD_REQUEST, "invalid project name");
    }
    let store = match state.project_store().await {
        Ok(s) => s,
        Err(e) => return err(StatusCode::SERVICE_UNAVAILABLE, e),
    };
    if let Err(e) = store.delete(&name).await {
        return err(StatusCode::INTERNAL_SERVER_ERROR, e.to_string());
    }
    // Best-effort: remove the checkout so a re-register clones fresh.
    let dest = state.projects_dir.join(&name);
    if dest.exists() {
        let _ = tokio::task::spawn_blocking(move || std::fs::remove_dir_all(dest)).await;
    }
    Json(serde_json::json!({ "deleted": true, "project": name })).into_response()
}
