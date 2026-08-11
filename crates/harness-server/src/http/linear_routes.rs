//! **Linear read-only discovery** (Phase 8, Slice 1) — surfaces a connected
//! Linear workspace's teams / states / labels and previews the issues a filter
//! would match. **No mutations**: nothing is claimed, transitioned, or run from
//! here; this only powers the future trigger-block dropdowns and a "what would
//! fire" preview before anything is wired up.
//!
//! - `GET /api/linear/discovery`            — teams + workflow states + labels
//! - `GET /api/linear/preview?team=&state=` — matching issues (preview)
//!
//! Auth comes from the global `linear` credential in the encrypted store: the
//! `actor=app` OAuth token once the workspace is connected, else a legacy
//! `api_key`; neither → a 4xx telling the operator to connect. The `{project}`
//! in the paths scopes the *bindings*, not the credential.

use std::sync::Arc;

use axum::extract::{Extension, Path as AxumPath, Query};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::Json;
use harness_sources::linear::LinearClient;
use serde::Deserialize;

use super::runs_routes::RunsState;

fn err(status: StatusCode, msg: impl Into<String>) -> Response {
    (status, Json(serde_json::json!({ "error": msg.into() }))).into_response()
}

/// Build a Linear client from the stored credential, or a 4xx response.
/// Attribution and token freshness are decided in
/// [`linear_client`](super::linear_oauth::linear_client).
async fn client(state: &Arc<RunsState>) -> Result<LinearClient, Response> {
    super::linear_oauth::linear_client(state)
        .await
        .map_err(|e| err(StatusCode::BAD_REQUEST, e))
}

/// `GET /api/projects/{project}/linear/discovery` — teams + states + labels.
pub async fn discovery(
    Extension(state): Extension<Arc<RunsState>>,
    AxumPath(_project): AxumPath<String>,
) -> Response {
    let client = match client(&state).await {
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
    AxumPath(_project): AxumPath<String>,
    Query(q): Query<PreviewQuery>,
) -> Response {
    if q.team.trim().is_empty() || q.state.trim().is_empty() {
        return err(StatusCode::BAD_REQUEST, "`team` and `state` are required");
    }
    let client = match client(&state).await {
        Ok(c) => c,
        Err(resp) => return resp,
    };
    // Mirrors the poller's gate exactly, so the preview can't promise more than
    // the poller would take.
    let Some(delegate_id) = super::linear_oauth::app_user_id(&state).await else {
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
