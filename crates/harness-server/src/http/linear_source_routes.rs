//! Persist-only Linear trigger binding; no poller / claim / transition / trigger here.
//!
//! Project-scoped CRUD for `harness_linear_sources` rows. The Linear API key stays
//! in the encrypted credential store (read by `linear_routes.rs` at discovery time);
//! this table stores no secret.

use std::sync::Arc;

use axum::extract::{Extension, Query};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::Json;
use harness_persist::LinearSourceInput;
use serde::Deserialize;

use super::linear_connections::resolve_for_project;
use super::linear_oauth::linear_client;
use super::runs_routes::RunsState;

fn err(status: StatusCode, msg: impl Into<String>) -> Response {
    (status, Json(serde_json::json!({ "error": msg.into() }))).into_response()
}

/// Verify the project exists, or return a 4xx/5xx response.
// The `Err` here IS the finished HTTP response, handed straight back to axum.
// Boxing it to satisfy `result_large_err` would add an allocation, plus an
// unwrap at every call site, to avoid a move axum makes anyway.
#[allow(clippy::result_large_err)]
async fn ensure_project(state: &Arc<RunsState>, project: &str) -> Result<(), Response> {
    let store = state
        .project_store()
        .await
        .map_err(|e| err(StatusCode::SERVICE_UNAVAILABLE, e))?;
    match store.get(project).await {
        Ok(Some(_)) => Ok(()),
        Ok(None) => Err(err(
            StatusCode::NOT_FOUND,
            format!("project `{project}` not found"),
        )),
        Err(e) => Err(err(
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("database error looking up project: {e}"),
        )),
    }
}

#[derive(Debug, Deserialize)]
pub struct WorkflowQuery {
    pub workflow: String,
}

#[derive(Debug, Deserialize)]
pub struct PutSourceBody {
    pub workflow: String,
    pub team_id: String,
    pub team_name: String,
    pub source_state_id: String,
    /// Label applied on give-up (optional); while present it excludes the issue
    /// from pickup. Removing it re-arms. `None`/empty disables the feature.
    pub failed_label: Option<String>,
    pub in_progress_state_id: Option<String>,
    pub review_state_id: Option<String>,
    pub ready_state_id: Option<String>,
    pub base_branch: Option<String>,
    #[serde(default = "default_poll")]
    pub poll_interval_secs: i32,
    /// How many runs this binding may have in flight at once (default 1).
    #[serde(default = "default_max_concurrent_runs")]
    pub max_concurrent_runs: i32,
    /// How many times an issue is (re-)fired before the poller gives up (default 1).
    #[serde(default = "default_max_attempts")]
    pub max_attempts: i32,
    #[serde(default)]
    pub enabled: bool,
    /// When false (default) the poller dry-runs this binding; true = claim + fire.
    #[serde(default)]
    pub live: bool,
}

fn default_poll() -> i32 {
    60
}

fn default_max_concurrent_runs() -> i32 {
    1
}

fn default_max_attempts() -> i32 {
    1
}

/// `GET /api/projects/{project}/linear-source?workflow=` — return the binding or `null`.
pub async fn get_source(
    Extension(state): Extension<Arc<RunsState>>,
    axum::extract::Path(project): axum::extract::Path<String>,
    Query(q): Query<WorkflowQuery>,
) -> Response {
    let Some(workflow) = trimmed_non_empty(q.workflow) else {
        return err(StatusCode::BAD_REQUEST, "`workflow` is required");
    };
    if let Err(r) = ensure_project(&state, &project).await {
        return r;
    }
    let store = match state.linear_source_store().await {
        Ok(s) => s,
        Err(e) => return err(StatusCode::SERVICE_UNAVAILABLE, e),
    };
    match store.get(&project, &workflow).await {
        Ok(opt) => Json(opt).into_response(),
        Err(e) => err(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()),
    }
}

/// `GET /api/projects/{project}/linear-sources` — list all bindings for a project.
pub async fn list_sources(
    Extension(state): Extension<Arc<RunsState>>,
    axum::extract::Path(project): axum::extract::Path<String>,
) -> Response {
    if let Err(r) = ensure_project(&state, &project).await {
        return r;
    }
    let store = match state.linear_source_store().await {
        Ok(s) => s,
        Err(e) => return err(StatusCode::SERVICE_UNAVAILABLE, e),
    };
    match store.list_by_project(&project).await {
        Ok(rows) => Json(rows).into_response(),
        Err(e) => err(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()),
    }
}

fn trimmed_non_empty(s: String) -> Option<String> {
    let trimmed = s.trim();
    (!trimmed.is_empty()).then(|| trimmed.to_string())
}

#[derive(Debug, Deserialize)]
pub struct CreateIssueBody {
    /// The binding to file against (defaults to `idea-to-pr`). Determines the
    /// team and source status the issue is created in.
    #[serde(default = "default_issue_workflow")]
    pub workflow: String,
    pub title: String,
    #[serde(default)]
    pub description: String,
}

fn default_issue_workflow() -> String {
    "idea-to-pr".to_string()
}

/// `POST /api/projects/{project}/linear-issues` — create a Linear issue from a
/// task (e.g. a GEO finding) in the project's bound team + **source status**.
///
/// The issue is filed unlabelled and *not* started: a human triages it and, if
/// they want the harness on it, delegates it to the app in Linear. Returns the
/// created issue.
pub async fn create_issue(
    Extension(state): Extension<Arc<RunsState>>,
    axum::extract::Path(project): axum::extract::Path<String>,
    Json(body): Json<CreateIssueBody>,
) -> Response {
    let Some(title) = trimmed_non_empty(body.title) else {
        return err(StatusCode::BAD_REQUEST, "`title` is required");
    };
    let workflow = trimmed_non_empty(body.workflow).unwrap_or_else(default_issue_workflow);
    if let Err(r) = ensure_project(&state, &project).await {
        return r;
    }

    // The binding supplies the team + the status to create the issue in.
    let src_store = match state.linear_source_store().await {
        Ok(s) => s,
        Err(e) => return err(StatusCode::SERVICE_UNAVAILABLE, e),
    };
    let binding = match src_store.get(&project, &workflow).await {
        Ok(Some(b)) => b,
        Ok(None) => {
            return err(
                StatusCode::BAD_REQUEST,
                format!(
                    "no Linear source configured for `{project}` / `{workflow}` — \
                     set one up in the project's Linear settings first"
                ),
            )
        }
        Err(e) => return err(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()),
    };

    // The Linear account this project files against, then its app-actor OAuth
    // token if that workspace is connected, else a legacy key.
    let conn = match resolve_for_project(&state, &project).await {
        Ok(c) => c,
        Err(e) => return err(StatusCode::BAD_REQUEST, e),
    };
    let client = match linear_client(&state, &conn).await {
        Ok(c) => c,
        Err(e) => return err(StatusCode::BAD_REQUEST, e),
    };

    // No labels: the issue is filed for a human to triage and — if they want the
    // harness on it — delegate to the app in Linear. Nothing picks it up
    // automatically from a label any more.
    match client
        .create_issue(
            &binding.team_id,
            &title,
            &body.description,
            Some(&binding.source_state_id),
            &[],
        )
        .await
    {
        Ok(issue) => (StatusCode::CREATED, Json(issue)).into_response(),
        Err(e) => err(
            StatusCode::BAD_GATEWAY,
            format!("Linear issueCreate failed: {e}"),
        ),
    }
}

fn optional_trimmed_non_empty(s: Option<String>) -> Option<String> {
    s.and_then(trimmed_non_empty)
}

/// `PUT /api/projects/{project}/linear-source` — create or update a binding.
pub async fn put_source(
    Extension(state): Extension<Arc<RunsState>>,
    axum::extract::Path(project): axum::extract::Path<String>,
    Json(body): Json<PutSourceBody>,
) -> Response {
    // Validation.
    let Some(workflow) = trimmed_non_empty(body.workflow) else {
        return err(StatusCode::BAD_REQUEST, "`workflow` is required");
    };
    let Some(team_id) = trimmed_non_empty(body.team_id) else {
        return err(StatusCode::BAD_REQUEST, "`team_id` is required");
    };
    let Some(team_name) = trimmed_non_empty(body.team_name) else {
        return err(StatusCode::BAD_REQUEST, "`team_name` is required");
    };
    let Some(source_state_id) = trimmed_non_empty(body.source_state_id) else {
        return err(StatusCode::BAD_REQUEST, "`source_state_id` is required");
    };
    if body.poll_interval_secs < 1 || body.poll_interval_secs > 86_400 {
        return err(
            StatusCode::BAD_REQUEST,
            "`poll_interval_secs` must be between 1 and 86400",
        );
    }
    if body.max_concurrent_runs < 1 || body.max_concurrent_runs > 20 {
        return err(
            StatusCode::BAD_REQUEST,
            "`max_concurrent_runs` must be between 1 and 20",
        );
    }
    if body.max_attempts < 1 || body.max_attempts > 10 {
        return err(
            StatusCode::BAD_REQUEST,
            "`max_attempts` must be between 1 and 10",
        );
    }
    // The source (pickup) status is exclusive: claiming an issue MOVES it out of
    // that column, so reusing it as a status-map target (in-progress/review/ready)
    // would mean the issue never leaves the pickup column — and a failed run
    // returning it there would re-claim it forever. Reject the misconfiguration.
    for (slot, st) in [
        ("in_progress_state_id", &body.in_progress_state_id),
        ("review_state_id", &body.review_state_id),
        ("ready_state_id", &body.ready_state_id),
    ] {
        if st.as_deref().map(str::trim) == Some(source_state_id.as_str()) {
            return err(
                StatusCode::BAD_REQUEST,
                format!(
                    "`{slot}` must differ from the source status — the pickup column can't also be a status-map target"
                ),
            );
        }
    }

    if let Err(r) = ensure_project(&state, &project).await {
        return r;
    }

    let store = match state.linear_source_store().await {
        Ok(s) => s,
        Err(e) => return err(StatusCode::SERVICE_UNAVAILABLE, e),
    };

    let input = LinearSourceInput {
        team_id,
        team_name,
        source_state_id,
        failed_label: optional_trimmed_non_empty(body.failed_label),
        in_progress_state_id: optional_trimmed_non_empty(body.in_progress_state_id),
        review_state_id: optional_trimmed_non_empty(body.review_state_id),
        ready_state_id: optional_trimmed_non_empty(body.ready_state_id),
        base_branch: optional_trimmed_non_empty(body.base_branch),
        poll_interval_secs: body.poll_interval_secs,
        max_concurrent_runs: body.max_concurrent_runs,
        max_attempts: body.max_attempts,
        enabled: body.enabled,
        live: body.live,
    };

    match store.upsert(&project, &workflow, &input).await {
        Ok(row) => Json(row).into_response(),
        Err(e) => err(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()),
    }
}

/// `DELETE /api/projects/{project}/linear-source?workflow=` — remove a binding.
pub async fn delete_source(
    Extension(state): Extension<Arc<RunsState>>,
    axum::extract::Path(project): axum::extract::Path<String>,
    Query(q): Query<WorkflowQuery>,
) -> Response {
    let Some(workflow) = trimmed_non_empty(q.workflow) else {
        return err(StatusCode::BAD_REQUEST, "`workflow` is required");
    };
    if let Err(r) = ensure_project(&state, &project).await {
        return r;
    }
    let store = match state.linear_source_store().await {
        Ok(s) => s,
        Err(e) => return err(StatusCode::SERVICE_UNAVAILABLE, e),
    };
    match store.delete(&project, &workflow).await {
        Ok(deleted) => Json(serde_json::json!({
            "deleted": deleted,
            "workflow": workflow,
        }))
        .into_response(),
        Err(e) => err(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()),
    }
}
