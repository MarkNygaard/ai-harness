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

use super::runs_routes::RunsState;

fn err(status: StatusCode, msg: impl Into<String>) -> Response {
    (status, Json(serde_json::json!({ "error": msg.into() }))).into_response()
}

/// Verify the project exists, or return a 4xx/5xx response.
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
    pub label: Option<String>,
    pub in_progress_state_id: Option<String>,
    pub review_state_id: Option<String>,
    pub ready_state_id: Option<String>,
    pub base_branch: Option<String>,
    #[serde(default = "default_poll")]
    pub poll_interval_secs: i32,
    #[serde(default)]
    pub enabled: bool,
}

fn default_poll() -> i32 {
    60
}

/// `GET /api/projects/{project}/linear-source?workflow=` — return the binding or `null`.
pub async fn get_source(
    Extension(state): Extension<Arc<RunsState>>,
    axum::extract::Path(project): axum::extract::Path<String>,
    Query(q): Query<WorkflowQuery>,
) -> Response {
    if let Err(r) = ensure_project(&state, &project).await {
        return r;
    }
    let store = match state.linear_source_store().await {
        Ok(s) => s,
        Err(e) => return err(StatusCode::SERVICE_UNAVAILABLE, e),
    };
    match store.get(&project, &q.workflow).await {
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

fn non_empty(s: Option<String>) -> Option<String> {
    s.filter(|s| !s.trim().is_empty())
}

/// `PUT /api/projects/{project}/linear-source` — create or update a binding.
pub async fn put_source(
    Extension(state): Extension<Arc<RunsState>>,
    axum::extract::Path(project): axum::extract::Path<String>,
    Json(body): Json<PutSourceBody>,
) -> Response {
    // Validation.
    if body.workflow.trim().is_empty() {
        return err(StatusCode::BAD_REQUEST, "`workflow` is required");
    }
    if body.team_id.trim().is_empty() {
        return err(StatusCode::BAD_REQUEST, "`team_id` is required");
    }
    if body.team_name.trim().is_empty() {
        return err(StatusCode::BAD_REQUEST, "`team_name` is required");
    }
    if body.source_state_id.trim().is_empty() {
        return err(StatusCode::BAD_REQUEST, "`source_state_id` is required");
    }
    if body.poll_interval_secs < 1 || body.poll_interval_secs > 86_400 {
        return err(
            StatusCode::BAD_REQUEST,
            "`poll_interval_secs` must be between 1 and 86400",
        );
    }

    if let Err(r) = ensure_project(&state, &project).await {
        return r;
    }

    let store = match state.linear_source_store().await {
        Ok(s) => s,
        Err(e) => return err(StatusCode::SERVICE_UNAVAILABLE, e),
    };

    let input = LinearSourceInput {
        team_id: body.team_id,
        team_name: body.team_name,
        source_state_id: body.source_state_id,
        label: non_empty(body.label),
        in_progress_state_id: non_empty(body.in_progress_state_id),
        review_state_id: non_empty(body.review_state_id),
        ready_state_id: non_empty(body.ready_state_id),
        base_branch: non_empty(body.base_branch),
        poll_interval_secs: body.poll_interval_secs,
        enabled: body.enabled,
    };

    match store.upsert(&project, &body.workflow, &input).await {
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
    if let Err(r) = ensure_project(&state, &project).await {
        return r;
    }
    let store = match state.linear_source_store().await {
        Ok(s) => s,
        Err(e) => return err(StatusCode::SERVICE_UNAVAILABLE, e),
    };
    match store.delete(&project, &q.workflow).await {
        Ok(deleted) => Json(serde_json::json!({
            "deleted": deleted,
            "workflow": q.workflow,
        }))
        .into_response(),
        Err(e) => err(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()),
    }
}
