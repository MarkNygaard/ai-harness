//! **The Linear operations a run may ask for**, authorised by a run grant.
//!
//! Replaces reaching the database from inside a run. The server holds the
//! credential and does the work; the run holds a token that says which project
//! it speaks for and nothing else — see [`super::run_grants`].
//!
//! Every handler takes its project from the grant rather than the request body.
//! A run is never asked which project it is, so it cannot answer wrongly.
//!
//! Deliberately five operations, matching what the epic workflows need: read an
//! issue, read its children, file a sub-issue, move one, comment on one. No
//! delete, no way to name a team, no way out of the project's own workspace.

use std::sync::Arc;

use axum::extract::Extension;
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::Json;
use serde::Deserialize;

use super::run_grants::{self, Grant};
use super::runs_routes::RunsState;

fn err(status: StatusCode, msg: impl Into<String>) -> Response {
    (status, Json(serde_json::json!({ "error": msg.into() }))).into_response()
}

/// The grant behind this request, or a refusal.
///
/// Not an extractor: every handler needs the grant's *project*, so returning it
/// is the point rather than a side effect of admission.
fn grant(headers: &HeaderMap) -> Result<Grant, Response> {
    let token = headers
        .get(axum::http::header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "))
        .map(str::trim)
        .filter(|t| !t.is_empty())
        .ok_or_else(|| {
            err(
                StatusCode::UNAUTHORIZED,
                "this endpoint is for a workflow run — send its run token",
            )
        })?;
    run_grants::redeem(token).ok_or_else(|| {
        err(
            StatusCode::UNAUTHORIZED,
            "that run token is unknown or has expired",
        )
    })
}

/// Build a Linear client for the grant's project.
async fn client(
    state: &Arc<RunsState>,
    g: &Grant,
) -> Result<harness_sources::linear::LinearClient, Response> {
    let conn = super::linear_connections::resolve_for_project(state, &g.project)
        .await
        .map_err(|e| err(StatusCode::CONFLICT, e))?;
    super::linear_oauth::linear_client(state, &conn)
        .await
        .map_err(|e| err(StatusCode::SERVICE_UNAVAILABLE, e))
}

#[derive(Debug, Deserialize)]
pub struct IssueRef {
    pub issue: String,
}

/// `POST /api/run/linear/issue` — one issue's team, state, parent and labels.
pub async fn issue(
    Extension(state): Extension<Arc<RunsState>>,
    headers: HeaderMap,
    Json(req): Json<IssueRef>,
) -> Response {
    let g = match grant(&headers) {
        Ok(g) => g,
        Err(r) => return r,
    };
    let c = match client(&state, &g).await {
        Ok(c) => c,
        Err(r) => return r,
    };
    match c.issue_context(&req.issue).await {
        Ok(ctx) => Json(ctx).into_response(),
        Err(e) => err(StatusCode::BAD_GATEWAY, e.0),
    }
}

/// `POST /api/run/linear/children` — an epic's sub-issues, in board order.
pub async fn children(
    Extension(state): Extension<Arc<RunsState>>,
    headers: HeaderMap,
    Json(req): Json<IssueRef>,
) -> Response {
    let g = match grant(&headers) {
        Ok(g) => g,
        Err(r) => return r,
    };
    let c = match client(&state, &g).await {
        Ok(c) => c,
        Err(r) => return r,
    };
    match c.list_children(&req.issue).await {
        Ok(kids) => Json(kids).into_response(),
        Err(e) => err(StatusCode::BAD_GATEWAY, e.0),
    }
}

#[derive(Debug, Deserialize)]
pub struct SubIssueRequest {
    pub parent: String,
    pub title: String,
    pub description: String,
    #[serde(default)]
    pub state: Option<String>,
    #[serde(default)]
    pub labels: Vec<String>,
}

/// `POST /api/run/linear/sub-issue` — file one under `parent`, in its team.
///
/// The team is read from the parent rather than accepted from the run: a
/// sub-issue of an epic belongs to that epic's team by definition, and letting a
/// run name one would let a typo scatter an epic across a workspace.
pub async fn sub_issue(
    Extension(state): Extension<Arc<RunsState>>,
    headers: HeaderMap,
    Json(req): Json<SubIssueRequest>,
) -> Response {
    let g = match grant(&headers) {
        Ok(g) => g,
        Err(r) => return r,
    };
    let c = match client(&state, &g).await {
        Ok(c) => c,
        Err(r) => return r,
    };
    let team = match c.issue_context(&req.parent).await {
        Ok(ctx) => match ctx.team_id {
            Some(t) => t,
            None => {
                return err(
                    StatusCode::NOT_FOUND,
                    "that parent issue does not resolve — check the id",
                )
            }
        },
        Err(e) => return err(StatusCode::BAD_GATEWAY, e.0),
    };
    match c
        .create_issue(
            &team,
            &req.title,
            &req.description,
            req.state.as_deref(),
            &req.labels,
            Some(&req.parent),
        )
        .await
    {
        Ok(created) => Json(created).into_response(),
        Err(e) => err(StatusCode::BAD_GATEWAY, e.0),
    }
}

#[derive(Debug, Deserialize)]
pub struct MoveRequest {
    pub issue: String,
    pub state: String,
}

/// `POST /api/run/linear/state` — move an issue to a workflow state.
pub async fn move_state(
    Extension(state): Extension<Arc<RunsState>>,
    headers: HeaderMap,
    Json(req): Json<MoveRequest>,
) -> Response {
    let g = match grant(&headers) {
        Ok(g) => g,
        Err(r) => return r,
    };
    let c = match client(&state, &g).await {
        Ok(c) => c,
        Err(r) => return r,
    };
    match c.set_issue_state(&req.issue, &req.state).await {
        Ok(()) => Json(serde_json::json!({ "moved": req.issue })).into_response(),
        Err(e) => err(StatusCode::BAD_GATEWAY, e.0),
    }
}

#[derive(Debug, Deserialize)]
pub struct CommentRequest {
    pub issue: String,
    pub body: String,
}

/// `POST /api/run/linear/comment` — append to an issue.
pub async fn comment(
    Extension(state): Extension<Arc<RunsState>>,
    headers: HeaderMap,
    Json(req): Json<CommentRequest>,
) -> Response {
    let g = match grant(&headers) {
        Ok(g) => g,
        Err(r) => return r,
    };
    // Ahead of everything else: a step that produced nothing should fail loudly
    // rather than post a blank comment on somebody's epic.
    if req.body.trim().is_empty() {
        return err(StatusCode::BAD_REQUEST, "refusing to post an empty comment");
    }
    let c = match client(&state, &g).await {
        Ok(c) => c,
        Err(r) => return r,
    };
    match c.add_comment(&req.issue, &req.body).await {
        Ok(()) => Json(serde_json::json!({ "commented": req.issue })).into_response(),
        Err(e) => err(StatusCode::BAD_GATEWAY, e.0),
    }
}
