//! **Linear read-only discovery** (Phase 8, Slice 1) — surfaces a connected
//! Linear workspace's teams / states / labels and previews the issues a filter
//! would match. **No mutations**: nothing is claimed, transitioned, or run from
//! here; this only powers the future trigger-block dropdowns and a "what would
//! fire" preview before anything is wired up.
//!
//! - `GET /api/linear/discovery`            — teams + workflow states + labels
//! - `GET /api/linear/preview?team=&state=` — matching issues (preview)
//!
//! Auth comes from the Linear connection the `{project}` in the path resolves
//! to: the `actor=app` OAuth token once that workspace is connected, else a
//! legacy `api_key`; neither → a 4xx telling the operator to connect.

use std::sync::Arc;

use axum::extract::{Extension, Path as AxumPath, Query};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::Json;
use harness_sources::linear::LinearClient;
use serde::Deserialize;

use super::linear_connections::{resolve_for_project, ConnectionId};
use super::runs_routes::RunsState;

fn err(status: StatusCode, msg: impl Into<String>) -> Response {
    (status, Json(serde_json::json!({ "error": msg.into() }))).into_response()
}

/// The Linear connection `project` belongs to, or a 4xx response.
// The `Err` here IS the finished HTTP response, handed straight back to axum.
// Boxing it to satisfy `result_large_err` would add an allocation, plus an
// unwrap at every call site, to avoid a move axum makes anyway.
#[allow(clippy::result_large_err)]
async fn connection(state: &Arc<RunsState>, project: &str) -> Result<ConnectionId, Response> {
    resolve_for_project(state, project)
        .await
        .map_err(|e| err(StatusCode::BAD_REQUEST, e))
}

/// Build a Linear client for `conn`, or a 4xx response. Attribution and token
/// freshness are decided in [`linear_client`](super::linear_oauth::linear_client).
#[allow(clippy::result_large_err)]
async fn client(state: &Arc<RunsState>, conn: &ConnectionId) -> Result<LinearClient, Response> {
    super::linear_oauth::linear_client(state, conn)
        .await
        .map_err(|e| err(StatusCode::BAD_REQUEST, e))
}

/// `GET /api/projects/{project}/linear/discovery` — teams + states + labels.
pub async fn discovery(
    Extension(state): Extension<Arc<RunsState>>,
    AxumPath(project): AxumPath<String>,
) -> Response {
    let conn = match connection(&state, &project).await {
        Ok(c) => c,
        Err(resp) => return resp,
    };
    let client = match client(&state, &conn).await {
        Ok(c) => c,
        Err(resp) => return resp,
    };
    match client.discover().await {
        Ok(d) => Json(d).into_response(),
        Err(e) => err(StatusCode::BAD_GATEWAY, e.0),
    }
}

#[derive(Debug, Deserialize)]
pub struct PreviewQuery {
    pub team: String,
    pub state: String,
}

/// `GET /api/projects/{project}/linear/preview?team=&state=` — the issues a
/// binding would actually claim: in that status **and** delegated to the harness.
/// Read-only; nothing is claimed.
pub async fn preview(
    Extension(state): Extension<Arc<RunsState>>,
    AxumPath(project): AxumPath<String>,
    Query(q): Query<PreviewQuery>,
) -> Response {
    if q.team.trim().is_empty() || q.state.trim().is_empty() {
        return err(StatusCode::BAD_REQUEST, "`team` and `state` are required");
    }
    let conn = match connection(&state, &project).await {
        Ok(c) => c,
        Err(resp) => return resp,
    };
    let client = match client(&state, &conn).await {
        Ok(c) => c,
        Err(resp) => return resp,
    };
    // Mirrors the poller's gate exactly, so the preview can't promise more than
    // the poller would take.
    let Some(delegate_id) = super::linear_oauth::app_user_id(&state, &conn).await else {
        return err(
            StatusCode::PRECONDITION_FAILED,
            "the harness's Linear app user id is unknown — reconnect the workspace on the \
             Credentials page so delegated issues can be identified",
        );
    };
    match client.preview_issues(&q.team, &q.state, &delegate_id).await {
        Ok(issues) => {
            Json(serde_json::json!({ "count": issues.len(), "issues": issues })).into_response()
        }
        Err(e) => err(StatusCode::BAD_GATEWAY, e.0),
    }
}
