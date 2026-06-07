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

use super::runs_routes::{self as cache, RunsState};
use tokio::task::spawn_blocking;

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
    /// `mise` tool specs to provision before runs (e.g. `rust`, `node@22`, `pnpm`).
    #[serde(default)]
    pub toolchains: Vec<String>,
    /// Per-project build-cache cap in GiB; omitted/`null`/≤0 → env default.
    #[serde(default)]
    pub cargo_target_cap_gb: Option<i32>,
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
        // Drop blank entries so the form can send a trailing empty input.
        toolchains: req
            .toolchains
            .into_iter()
            .map(|t| t.trim().to_string())
            .filter(|t| !t.is_empty())
            .collect(),
        cargo_target_cap_gb: req.cargo_target_cap_gb.filter(|v| *v > 0),
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

/// `Err(Response)` (409) when a run is active for `project`, else `Ok(())`.
async fn ensure_idle(state: &RunsState, project: &str) -> Result<(), Response> {
    let store = state
        .store()
        .await
        .map_err(|e| err(StatusCode::SERVICE_UNAVAILABLE, e))?;
    match store.count_active_runs(project).await {
        Ok(0) => Ok(()),
        Ok(_) => Err(err(
            StatusCode::CONFLICT,
            "a run is active for this project; try again when idle",
        )),
        Err(e) => Err(err(StatusCode::INTERNAL_SERVER_ERROR, e.to_string())),
    }
}

/// `GET /api/projects/{name}/cache-size` — current cache bytes + effective cap.
pub async fn get_cache_size(
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
    let project = match store.get(&name).await {
        Ok(Some(p)) => p,
        Ok(None) => return err(StatusCode::NOT_FOUND, format!("project `{name}` not found")),
        Err(e) => return err(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()),
    };
    let dir = state.projects_dir.join(".cargo-target").join(&name);
    let bytes = match spawn_blocking(move || cache::dir_size(&dir)).await {
        Ok(n) => n,
        Err(e) => {
            return err(
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("dir_size failed: {e}"),
            )
        }
    };
    Json(serde_json::json!({
        "bytes": bytes,
        "cap_gb": cache::resolve_cap_gb(project.cargo_target_cap_gb),
    }))
    .into_response()
}

#[derive(Debug, Deserialize)]
pub struct SetCapRequest {
    #[serde(default)]
    pub cap_gb: Option<i32>,
}

/// `PUT /api/projects/{name}/cache-cap` — set or clear the per-project cap.
pub async fn set_cache_cap(
    Extension(state): Extension<Arc<RunsState>>,
    AxumPath(name): AxumPath<String>,
    Json(req): Json<SetCapRequest>,
) -> Response {
    if !valid_name(&name) {
        return err(StatusCode::BAD_REQUEST, "invalid project name");
    }
    let cap = req.cap_gb.filter(|&v| v > 0);
    let store = match state.project_store().await {
        Ok(s) => s,
        Err(e) => return err(StatusCode::SERVICE_UNAVAILABLE, e),
    };
    match store.set_cargo_target_cap(&name, cap).await {
        Ok(Some(p)) => Json(p).into_response(),
        Ok(None) => err(StatusCode::NOT_FOUND, format!("project `{name}` not found")),
        Err(e) => err(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()),
    }
}

/// `POST /api/projects/{name}/cache/clear` — delete the project's cache dir.
pub async fn clear_cache(
    Extension(state): Extension<Arc<RunsState>>,
    AxumPath(name): AxumPath<String>,
) -> Response {
    if !valid_name(&name) {
        return err(StatusCode::BAD_REQUEST, "invalid project name");
    }
    if let Err(r) = ensure_idle(&state, &name).await {
        return r;
    }
    let dir = state.projects_dir.join(".cargo-target").join(&name);
    if !dir.exists() {
        return Json(serde_json::json!({ "cleared": true, "bytes_freed": 0 })).into_response();
    }
    let size = cache::dir_size(&dir);
    match spawn_blocking(move || std::fs::remove_dir_all(&dir)).await {
        Ok(Ok(())) => {
            Json(serde_json::json!({ "cleared": true, "bytes_freed": size })).into_response()
        }
        Ok(Err(e)) => err(
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("remove_dir_all failed: {e}"),
        ),
        Err(e) => err(
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("task failed: {e}"),
        ),
    }
}

/// `POST /api/projects/{name}/cache/sweep` — run the sweeper on demand.
pub async fn sweep_cache(
    Extension(state): Extension<Arc<RunsState>>,
    AxumPath(name): AxumPath<String>,
) -> Response {
    if !valid_name(&name) {
        return err(StatusCode::BAD_REQUEST, "invalid project name");
    }
    if let Err(r) = ensure_idle(&state, &name).await {
        return r;
    }
    let store = match state.project_store().await {
        Ok(s) => s,
        Err(e) => return err(StatusCode::SERVICE_UNAVAILABLE, e),
    };
    let project = match store.get(&name).await {
        Ok(Some(p)) => p,
        Ok(None) => return err(StatusCode::NOT_FOUND, format!("project `{name}` not found")),
        Err(e) => return err(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()),
    };
    let cap_gb = cache::resolve_cap_gb(project.cargo_target_cap_gb);
    if cap_gb == 0 {
        return Json(serde_json::json!({ "swept": false })).into_response();
    }
    let cap = cap_gb.saturating_mul(1024 * 1024 * 1024);
    let target = cap / 5 * 4;
    let dir = state.projects_dir.join(".cargo-target").join(&name);
    match spawn_blocking(move || {
        cache::sweep_cargo_cache(&dir, cap, target, cache::CACHE_SWEEP_SAFETY_FLOOR_SECS)
    })
    .await
    {
        Ok(Ok(Some((before, after)))) => Json(serde_json::json!({
            "swept": true,
            "before": before,
            "after": after,
        }))
        .into_response(),
        Ok(Ok(None)) => Json(serde_json::json!({ "swept": false })).into_response(),
        Ok(Err(e)) => err(
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("sweep failed: {e}"),
        ),
        Err(e) => err(
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("task failed: {e}"),
        ),
    }
}
