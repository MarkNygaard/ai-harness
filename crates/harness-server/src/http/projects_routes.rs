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
use harness_persist::{ProjectInput, ProjectRepo};
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
    /// Optional deployed/live site URL for flows that analyze the running site
    /// (e.g. a GEO audit); exposed to runs as `$EXTERNAL_URL`.
    #[serde(default)]
    pub external_url: Option<String>,
    /// `mise` tool specs to provision before runs (e.g. `rust`, `node@22`, `pnpm`).
    #[serde(default)]
    pub toolchains: Vec<String>,
    /// Additional repos for a multi-repo project (frontend + backend, etc.).
    /// Empty = single-repo (the `git_url` above). Each entry needs `url` +
    /// `folder`; blank `base_branch` defaults to `main`.
    #[serde(default)]
    pub repos: Vec<ProjectRepo>,
    /// Per-project build-cache cap in GiB; omitted/`null`/≤0 → env default.
    #[serde(default)]
    pub cargo_target_cap_gb: Option<i32>,
}

/// The first remote listed twice in `repos`, if any.
///
/// A row naming the project's own `git_url` is legitimate — that row chooses the
/// primary repo's folder. Naming the *same* remote twice is not: it would put one
/// repo in two folders on the same `run/<id>` branch, where the second push
/// either rejects as non-fast-forward or clobbers the first.
fn duplicate_remote(repos: &[ProjectRepo]) -> Option<String> {
    let mut seen = std::collections::HashSet::new();
    for r in repos {
        let url = r.url.trim();
        if url.is_empty() {
            continue;
        }
        if !seen.insert(harness_runner::remote_identity(url)) {
            return Some(url.to_string());
        }
    }
    None
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
    // One remote, one checkout: see `duplicate_remote`.
    if let Some(dupe) = duplicate_remote(&req.repos) {
        return err(
            StatusCode::BAD_REQUEST,
            format!(
                "`{dupe}` is listed more than once — a repo gets one folder. To \
                 name the primary repo's folder, list its URL exactly once."
            ),
        );
    }
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
        external_url: req
            .external_url
            .map(|u| u.trim().to_string())
            .filter(|u| !u.is_empty()),
        // Drop blank entries so the form can send a trailing empty input.
        toolchains: req
            .toolchains
            .into_iter()
            .map(|t| t.trim().to_string())
            .filter(|t| !t.is_empty())
            .collect(),
        // Keep only repos with a url + folder; default a blank branch to `main`.
        repos: req
            .repos
            .into_iter()
            .filter_map(|r| {
                let url = r.url.trim().to_string();
                let folder = r.folder.trim().to_string();
                if url.is_empty() || folder.is_empty() {
                    return None;
                }
                let base_branch = match r.base_branch.trim() {
                    "" => "main".to_string(),
                    b => b.to_string(),
                };
                let role = r
                    .role
                    .map(|s| s.trim().to_string())
                    .filter(|s| !s.is_empty());
                Some(ProjectRepo {
                    url,
                    base_branch,
                    folder,
                    role,
                })
            })
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
// The `Err` here IS the finished HTTP response, handed straight back to axum.
// Boxing it to satisfy `result_large_err` would add an allocation, plus an
// unwrap at every call site, to avoid a move axum makes anyway.
#[allow(clippy::result_large_err)]
async fn ensure_idle(state: &RunsState, project: &str) -> Result<(), Response> {
    if state.has_live_run_for_project(project).await {
        return Err(err(
            StatusCode::CONFLICT,
            "a run is active for this project; try again when idle",
        ));
    }
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
    // The dependency and git caches are shared by every project (see `caches`),
    // so they are reported alongside the project's own build cache rather than
    // folded into it — a JS/.NET project has no build cache and used to read as
    // "0 cached" even with a warm pnpm store behind it.
    let deps = super::caches::deps_root(&state.projects_dir);
    let mirrors = super::caches::git_mirror_root(&state.projects_dir);
    let workflow = super::caches::project_cache(&state.projects_dir, &name);
    let sizes = spawn_blocking(move || {
        (
            cache::dir_size(&dir),
            cache::dir_size(&deps),
            cache::dir_size(&mirrors),
            cache::dir_size(&workflow),
        )
    })
    .await;
    let (bytes, deps_bytes, git_bytes, workflow_bytes) = match sizes {
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
        "deps_bytes": deps_bytes,
        "deps_cap_gb": super::caches::deps_cap_gb(),
        "git_bytes": git_bytes,
        "workflow_bytes": workflow_bytes,
        "workflow_cap_gb": super::caches::project_cap_gb(),
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
    let store = match state.project_store().await {
        Ok(s) => s,
        Err(e) => return err(StatusCode::SERVICE_UNAVAILABLE, e),
    };
    match store.get(&name).await {
        Ok(Some(_)) => {}
        Ok(None) => return err(StatusCode::NOT_FOUND, format!("project `{name}` not found")),
        Err(e) => return err(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()),
    }
    if let Err(r) = ensure_idle(&state, &name).await {
        return r;
    }
    let dir = state.projects_dir.join(".cargo-target").join(&name);
    match spawn_blocking(move || {
        if !dir.exists() {
            return Ok((true, 0));
        }
        let size = cache::dir_size(&dir);
        std::fs::remove_dir_all(&dir).map(|_| (true, size))
    })
    .await
    {
        Ok(Ok((cleared, bytes_freed))) => {
            Json(serde_json::json!({ "cleared": cleared, "bytes_freed": bytes_freed }))
                .into_response()
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

#[cfg(test)]
mod tests {
    use super::*;

    fn row(folder: &str, url: &str) -> ProjectRepo {
        ProjectRepo {
            folder: folder.to_string(),
            url: url.to_string(),
            base_branch: "main".to_string(),
            role: None,
        }
    }

    #[test]
    fn one_row_per_remote_and_url_form_does_not_hide_a_duplicate() {
        // The primary's URL listed once is the rename case, not a duplicate —
        // `plan_layout` folds it into the primary checkout.
        assert_eq!(
            duplicate_remote(&[
                row("frontend", "https://github.com/me/front.git"),
                row("backend", "https://github.com/me/api.git"),
            ]),
            None
        );
        // Two folders for one repo would race to push the same run branch, and
        // a different URL spelling must not get past the check.
        assert_eq!(
            duplicate_remote(&[
                row("frontend", "https://github.com/me/front.git"),
                row("also-frontend", "git@github.com:me/front"),
            ]),
            Some("git@github.com:me/front".to_string())
        );
        // A blank trailing row from the form is not a duplicate of anything.
        assert_eq!(duplicate_remote(&[row("a", ""), row("b", "")]), None);
    }
}
