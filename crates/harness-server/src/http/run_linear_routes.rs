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

/// Why a request was refused, before it becomes a response.
///
/// The helpers below return this rather than a built `Response`: an axum
/// response is 128 bytes, and carrying one in an `Err` costs that on every
/// call, including the overwhelming majority that succeed.
type Refusal = (StatusCode, String);

/// The grant behind this request, or a refusal.
///
/// Not an extractor: every handler needs the grant's *project*, so returning it
/// is the point rather than a side effect of admission.
fn grant(headers: &HeaderMap) -> Result<Grant, Refusal> {
    let token = headers
        .get(axum::http::header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "))
        .map(str::trim)
        .filter(|t| !t.is_empty())
        .ok_or((
            StatusCode::UNAUTHORIZED,
            "this endpoint is for a workflow run — send its run token".to_string(),
        ))?;
    run_grants::redeem(token).ok_or((
        StatusCode::UNAUTHORIZED,
        "that run token is unknown or has expired".to_string(),
    ))
}

/// Build a Linear client for the grant's project.
async fn client(
    state: &Arc<RunsState>,
    g: &Grant,
) -> Result<harness_sources::linear::LinearClient, Refusal> {
    let conn = super::linear_connections::resolve_for_project(state, &g.project)
        .await
        .map_err(|e| (StatusCode::CONFLICT, e))?;
    super::linear_oauth::linear_client(state, &conn)
        .await
        .map_err(|e| (StatusCode::SERVICE_UNAVAILABLE, e))
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
        Err((s, m)) => return err(s, m),
    };
    let c = match client(&state, &g).await {
        Ok(c) => c,
        Err((s, m)) => return err(s, m),
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
        Err((s, m)) => return err(s, m),
    };
    let c = match client(&state, &g).await {
        Ok(c) => c,
        Err((s, m)) => return err(s, m),
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
        Err((s, m)) => return err(s, m),
    };
    let c = match client(&state, &g).await {
        Ok(c) => c,
        Err((s, m)) => return err(s, m),
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
        Err((s, m)) => return err(s, m),
    };
    let c = match client(&state, &g).await {
        Ok(c) => c,
        Err((s, m)) => return err(s, m),
    };
    match c.set_issue_state(&req.issue, &req.state).await {
        Ok(()) => Json(serde_json::json!({ "moved": req.issue })).into_response(),
        Err(e) => err(StatusCode::BAD_GATEWAY, e.0),
    }
}

#[derive(Debug, Deserialize)]
pub struct ReadyStateRequest {
    /// The piece whose build column to read. Optional: without it the answer
    /// comes from the asking run's own claim, which is what an epic being
    /// started has.
    #[serde(default)]
    pub issue: Option<String>,
}

/// `POST /api/run/linear/ready-state` — the column a piece must be moved to in
/// order to be built.
///
/// Exists so nobody has to paste a Linear state UUID into a project environment
/// variable. The poller writes `original_state_id` on every claim, so the column
/// that triggers a build is already recorded: for an epic, the column the epic
/// itself was claimed from; for a merged piece, the column that piece was
/// claimed from. One binding picks up both, so the two agree.
///
/// The supervisor's own claims are excluded — it triggers from the column a
/// merged piece rests in, and handing that back would move the next piece
/// straight to Done.
pub async fn ready_state(
    Extension(state): Extension<Arc<RunsState>>,
    headers: HeaderMap,
    Json(req): Json<ReadyStateRequest>,
) -> Response {
    let g = match grant(&headers) {
        Ok(g) => g,
        Err((s, m)) => return err(s, m),
    };
    let claims = match state.linear_claim_store().await {
        Ok(c) => c,
        Err(e) => return err(StatusCode::SERVICE_UNAVAILABLE, e.to_string()),
    };
    let supervisor = super::linear_agent::EPIC_SUPERVISOR;

    // The named piece first: it was built by the binding we are looking for.
    if let Some(issue) = req.issue.as_deref().filter(|s| !s.trim().is_empty()) {
        if let Ok(Some(found)) = claims.build_state_for_issue(issue, supervisor).await {
            return Json(serde_json::json!({ "state": found })).into_response();
        }
    }
    // Otherwise this run's own claim — an epic being started was itself picked
    // up from the column its pieces start in.
    match claims.claim_for_run(&g.run_id).await {
        Ok(Some(c)) if c.workflow != supervisor && !c.original_state_id.is_empty() => {
            Json(serde_json::json!({ "state": c.original_state_id })).into_response()
        }
        Ok(Some(c)) => match claims.build_state_for_issue(&c.issue_id, supervisor).await {
            Ok(Some(found)) => Json(serde_json::json!({ "state": found })).into_response(),
            _ => err(
                StatusCode::NOT_FOUND,
                "no build column recorded for this issue yet",
            ),
        },
        Ok(None) => err(StatusCode::NOT_FOUND, "this run has no Linear claim"),
        Err(e) => err(StatusCode::BAD_GATEWAY, e.to_string()),
    }
}

/// `POST /api/run/linear/release` — give up an issue by clearing its delegate.
///
/// What a workflow calls when it is finished with an issue it deliberately left
/// where it found it. The poller selects on the delegate field, so without this
/// such an issue is re-picked every tick: the epic supervisor reviewed one
/// merged piece six times before anyone noticed, each review an Opus run, and
/// each one advanced the epic another piece.
pub async fn release(
    Extension(state): Extension<Arc<RunsState>>,
    headers: HeaderMap,
    Json(req): Json<IssueRef>,
) -> Response {
    let g = match grant(&headers) {
        Ok(g) => g,
        Err((s, m)) => return err(s, m),
    };
    let c = match client(&state, &g).await {
        Ok(c) => c,
        Err((s, m)) => return err(s, m),
    };
    match c.clear_delegate(&req.issue).await {
        Ok(()) => Json(serde_json::json!({ "released": req.issue })).into_response(),
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
        Err((s, m)) => return err(s, m),
    };
    // Ahead of everything else: a step that produced nothing should fail loudly
    // rather than post a blank comment on somebody's epic.
    if req.body.trim().is_empty() {
        return err(StatusCode::BAD_REQUEST, "refusing to post an empty comment");
    }
    let c = match client(&state, &g).await {
        Ok(c) => c,
        Err((s, m)) => return err(s, m),
    };
    match c.add_comment(&req.issue, &req.body).await {
        Ok(()) => Json(serde_json::json!({ "commented": req.issue })).into_response(),
        Err(e) => err(StatusCode::BAD_GATEWAY, e.0),
    }
}
