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
    /// Optional. When omitted/empty, the repo's default branch (`origin/HEAD`) is
    /// auto-detected after clone (falling back to `main`).
    #[serde(default)]
    pub base_branch: Option<String>,
    #[serde(default)]
    pub default_workflow: Option<String>,
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

    // Bring the on-disk checkout in line first (clone if absent, else fetch), so
    // we can auto-detect the repo's default branch when the caller didn't pick one.
    let want_branch = req
        .base_branch
        .as_deref()
        .map(str::trim)
        .filter(|b| !b.is_empty())
        .map(str::to_string);
    let detect = want_branch.is_none();
    let dest: PathBuf = state.projects_dir.join(&req.name);
    let token = state.github_token().await;
    let git_url = req.git_url.trim().to_string();
    let clone_url = git_url.clone();
    let exists = dest.exists();
    let git_result = tokio::task::spawn_blocking(
        move || -> Result<Option<String>, harness_runner::WorktreeError> {
            if exists {
                harness_runner::fetch_repo(&dest, token.as_deref())?;
            } else {
                if let Some(parent) = dest.parent() {
                    std::fs::create_dir_all(parent).map_err(|e| {
                        harness_runner::WorktreeError(format!("create projects dir: {e}"))
                    })?;
                }
                harness_runner::clone_repo(&clone_url, &dest, token.as_deref())?;
            }
            // Detect origin/HEAD only when the caller didn't specify a branch.
            Ok(if detect {
                harness_runner::default_branch(&dest)
            } else {
                None
            })
        },
    )
    .await;

    // Resolve the branch to store, and any non-fatal git warning.
    let (base_branch, warning) = match git_result {
        Ok(Ok(detected)) => (
            want_branch
                .or(detected)
                .unwrap_or_else(|| "main".to_string()),
            None,
        ),
        // Save the row anyway so the user can fix creds/URL and re-register.
        Ok(Err(e)) => (
            want_branch.unwrap_or_else(|| "main".to_string()),
            Some(format!("registered, but repo sync failed: {e}")),
        ),
        Err(e) => {
            return err(
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("git task panicked: {e}"),
            )
        }
    };

    let input = ProjectInput {
        git_url,
        base_branch,
        default_workflow: req.default_workflow.filter(|w| !w.trim().is_empty()),
    };
    let project = match store.upsert(&req.name, &input).await {
        Ok(p) => p,
        Err(e) => return err(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()),
    };

    match warning {
        None => Json(project).into_response(),
        Some(w) => (
            StatusCode::ACCEPTED,
            Json(serde_json::json!({ "project": project, "warning": w })),
        )
            .into_response(),
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
