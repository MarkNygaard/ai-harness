//! **Linear read-only discovery** (Phase 8, Slice 1) — surfaces a connected
//! Linear workspace's teams / states / labels and previews the issues a filter
//! would match. **No mutations**: nothing is claimed, transitioned, or run from
//! here; this only powers the future trigger-block dropdowns and a "what would
//! fire" preview before anything is wired up.
//!
//! - `GET /api/linear/discovery`            — teams + workflow states + labels
//! - `GET /api/linear/preview?team=&state=&label=` — matching issues (preview)
//!
//! The Linear API key is read from the encrypted credential store under provider
//! `linear` (field `api_key`); absent → a 4xx telling the operator to connect.

use std::sync::Arc;

use axum::extract::{Extension, Query};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::Json;
use harness_sources::linear::LinearClient;
use serde::Deserialize;

use super::runs_routes::RunsState;

fn err(status: StatusCode, msg: impl Into<String>) -> Response {
    (status, Json(serde_json::json!({ "error": msg.into() }))).into_response()
}

/// Build a Linear client from the stored credential, or a 4xx/5xx response.
async fn linear_client(state: &Arc<RunsState>) -> Result<LinearClient, Response> {
    let store = state
        .cred_store()
        .await
        .map_err(|e| err(StatusCode::SERVICE_UNAVAILABLE, e))?;
    let fields = store
        .get("linear")
        .await
        .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?
        .ok_or_else(|| {
            err(
                StatusCode::BAD_REQUEST,
                "Linear not connected — store an API key under provider `linear`",
            )
        })?;
    let key = fields
        .get("api_key")
        .filter(|k| !k.is_empty())
        .ok_or_else(|| {
            err(
                StatusCode::BAD_REQUEST,
                "Linear credential is missing the `api_key` field",
            )
        })?;
    Ok(LinearClient::new(key))
}

/// `GET /api/linear/discovery` — teams + their workflow states + labels.
pub async fn discovery(Extension(state): Extension<Arc<RunsState>>) -> Response {
    let client = match linear_client(&state).await {
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
    #[serde(default)]
    pub label: Option<String>,
}

/// `GET /api/linear/preview?team=&state=&label=` — issues the filter matches.
/// Read-only; nothing is claimed.
pub async fn preview(
    Extension(state): Extension<Arc<RunsState>>,
    Query(q): Query<PreviewQuery>,
) -> Response {
    if q.team.trim().is_empty() || q.state.trim().is_empty() {
        return err(StatusCode::BAD_REQUEST, "`team` and `state` are required");
    }
    let client = match linear_client(&state).await {
        Ok(c) => c,
        Err(resp) => return resp,
    };
    let label = q.label.as_deref().filter(|l| !l.is_empty());
    match client.preview_issues(&q.team, &q.state, label).await {
        Ok(issues) => {
            Json(serde_json::json!({ "count": issues.len(), "issues": issues })).into_response()
        }
        Err(e) => err(StatusCode::BAD_GATEWAY, e.0),
    }
}
