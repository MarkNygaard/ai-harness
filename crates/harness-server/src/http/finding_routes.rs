//! Per-finding triage state for any report run (GEO, review, or a workflow that
//! declares `ui.report`).
//!
//! The report (rendered from a run's verdict) lets the user act on each finding
//! — "Build this", "Create issue", or "Ignore". These routes persist that state
//! (keyed by the run + a stable `finding_key`) so the report shows the same
//! checkmarks / dimmed rows on the next visit. "Rebuild" / "Unignore" clear a
//! finding's state, restoring its buttons. Backed by the unified
//! [`harness_persist::FindingStateStore`], served at `/api/runs/{id}/findings`.

use std::sync::Arc;

use axum::extract::{Extension, Path, Query};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::Json;
use harness_persist::FindingStateInput;
use serde::Deserialize;

use super::runs_routes::RunsState;

fn err(status: StatusCode, msg: impl Into<String>) -> Response {
    (status, Json(serde_json::json!({ "error": msg.into() }))).into_response()
}

/// Recognized triage actions.
fn valid_action(action: &str) -> bool {
    matches!(action, "built" | "issued" | "ignored")
}

/// `GET /api/runs/{run_id}/findings` — all remembered finding states.
pub async fn list_findings(
    Extension(state): Extension<Arc<RunsState>>,
    Path(run_id): Path<String>,
) -> Response {
    let store = match state.finding_store().await {
        Ok(s) => s,
        Err(e) => return err(StatusCode::SERVICE_UNAVAILABLE, e),
    };
    match store.list_for_run(&run_id).await {
        Ok(rows) => Json(rows).into_response(),
        Err(e) => err(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()),
    }
}

#[derive(Debug, Deserialize)]
pub struct SetFindingBody {
    pub finding_key: String,
    pub action: String,
    pub ref_run_id: Option<String>,
    pub issue_identifier: Option<String>,
    pub issue_url: Option<String>,
}

/// `PUT /api/runs/{run_id}/findings` — record one finding's state.
pub async fn set_finding(
    Extension(state): Extension<Arc<RunsState>>,
    Path(run_id): Path<String>,
    Json(body): Json<SetFindingBody>,
) -> Response {
    if body.finding_key.trim().is_empty() {
        return err(StatusCode::BAD_REQUEST, "`finding_key` is required");
    }
    if !valid_action(&body.action) {
        return err(
            StatusCode::BAD_REQUEST,
            "`action` must be one of: built, issued, ignored",
        );
    }
    let store = match state.finding_store().await {
        Ok(s) => s,
        Err(e) => return err(StatusCode::SERVICE_UNAVAILABLE, e),
    };
    let input = FindingStateInput {
        action: body.action,
        ref_run_id: body.ref_run_id,
        issue_identifier: body.issue_identifier,
        issue_url: body.issue_url,
    };
    match store.set(&run_id, &body.finding_key, &input).await {
        Ok(row) => (StatusCode::OK, Json(row)).into_response(),
        Err(e) => err(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()),
    }
}

#[derive(Debug, Deserialize)]
pub struct ClearFindingQuery {
    pub key: String,
}

/// `DELETE /api/runs/{run_id}/findings?key=` — forget a finding's state.
pub async fn clear_finding(
    Extension(state): Extension<Arc<RunsState>>,
    Path(run_id): Path<String>,
    Query(q): Query<ClearFindingQuery>,
) -> Response {
    if q.key.trim().is_empty() {
        return err(StatusCode::BAD_REQUEST, "`key` is required");
    }
    let store = match state.finding_store().await {
        Ok(s) => s,
        Err(e) => return err(StatusCode::SERVICE_UNAVAILABLE, e),
    };
    match store.clear(&run_id, &q.key).await {
        Ok(deleted) => Json(serde_json::json!({ "deleted": deleted })).into_response(),
        Err(e) => err(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()),
    }
}
