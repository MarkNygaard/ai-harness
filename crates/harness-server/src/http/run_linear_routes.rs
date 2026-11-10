//! **The Linear operations a run may ask for**, authorised by a run grant.
//!
//! Replaces reaching the database from inside a run. The server holds the
//! credential and does the work; the run holds a token that says which project
//! it speaks for and nothing else — see [`super::run_grants`].
//!
//! Every handler takes its project from the grant rather than the request body.
//! A run is never asked which project it is, so it cannot answer wrongly.
//!
//! Deliberately few operations, matching what workflows need: read an issue,
//! read its children, list a team's issues, file a sub-issue, move one,
//! comment on one, label one with an existing label, and file a standalone
//! issue for a workflow's result. No delete, no creating labels, and no way out
//! of the project's own workspace: the operations that name a team can only
//! name a team that workspace has.

use std::sync::Arc;

use axum::extract::Extension;
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::Json;
use serde::Deserialize;

use super::run_grants::{self, Grant};
use super::runs_routes::RunsState;

use super::UPSTREAM_FAILED;

fn err(status: StatusCode, msg: impl Into<String>) -> Response {
    let msg = msg.into();
    // Also into the server's log: an operator looking at why a run's Linear
    // step failed should not need the run's output to find out.
    if status == UPSTREAM_FAILED {
        tracing::warn!("run linear: request failed: {msg}");
    }
    (status, Json(serde_json::json!({ "error": msg }))).into_response()
}

/// Why a request was refused, before it becomes a response.
///
/// The helpers below return this rather than a built `Response`: an axum
/// response is 128 bytes, and carrying one in an `Err` costs that on every
/// call, including the overwhelming majority that succeed.
pub(super) type Refusal = (StatusCode, String);

/// The grant behind this request, or a refusal.
///
/// Not an extractor: every handler needs the grant's *project*, so returning it
/// is the point rather than a side effect of admission.
pub(super) fn grant(headers: &HeaderMap) -> Result<Grant, Refusal> {
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
        Err(e) => err(UPSTREAM_FAILED, e.0),
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
        Err(e) => err(UPSTREAM_FAILED, e.0),
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
        Err(e) => return err(UPSTREAM_FAILED, e.0),
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
        Err(e) => err(UPSTREAM_FAILED, e.0),
    }
}

#[derive(Debug, Deserialize)]
pub struct CreateIssueRequest {
    /// The team's key (`ECOM`), name or id.
    pub team: String,
    pub title: String,
    pub description: String,
    /// A state name or id; omitted = the team's default.
    #[serde(default)]
    pub state: Option<String>,
    /// Label names or ids, from the team's labels.
    #[serde(default)]
    pub labels: Vec<String>,
}

/// Where an issue lands, resolved from names a workflow author can write.
#[derive(Debug, PartialEq, Eq)]
struct Placement {
    team: String,
    state: Option<String>,
    labels: Vec<String>,
}

/// Match `wanted` against an id exactly, or a key/name ignoring case.
pub(crate) fn matches(wanted: &str, id: &str, names: &[&str]) -> bool {
    let w = wanted.trim();
    w == id || names.iter().any(|n| n.eq_ignore_ascii_case(w))
}

/// Resolve a team, state and labels by name within the project's workspace.
///
/// A miss names what *is* there: the message is read in a failed run's log by
/// whoever wrote the workflow, and the fix is nearly always a typo.
fn resolve_placement(
    discovery: &harness_sources::linear::Discovery,
    req: &CreateIssueRequest,
) -> Result<Placement, String> {
    let team = discovery
        .teams
        .iter()
        .find(|t| matches(&req.team, &t.id, &[&t.key, &t.name]))
        .ok_or_else(|| {
            let known: Vec<String> = discovery
                .teams
                .iter()
                .map(|t| format!("{} ({})", t.key, t.name))
                .collect();
            format!(
                "no team `{}` in this project's Linear workspace; teams: {}",
                req.team.trim(),
                known.join(", ")
            )
        })?;
    let state = match req
        .state
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
    {
        None => None,
        Some(wanted) => Some(
            team.states
                .iter()
                .find(|s| matches(wanted, &s.id, &[&s.name]))
                .map(|s| s.id.clone())
                .ok_or_else(|| {
                    let known: Vec<&str> = team.states.iter().map(|s| s.name.as_str()).collect();
                    format!(
                        "team {} has no status `{wanted}`; statuses: {}",
                        team.key,
                        known.join(", ")
                    )
                })?,
        ),
    };
    let labels = req
        .labels
        .iter()
        .map(|wanted| {
            team.labels
                .iter()
                .find(|l| matches(wanted, &l.id, &[&l.name]))
                .map(|l| l.id.clone())
                .ok_or_else(|| {
                    let known: Vec<&str> = team.labels.iter().map(|l| l.name.as_str()).collect();
                    format!(
                        "team {} has no label `{}`; labels: {}",
                        team.key,
                        wanted.trim(),
                        known.join(", ")
                    )
                })
        })
        .collect::<Result<Vec<_>, _>>()?;
    Ok(Placement {
        team: team.id.clone(),
        state,
        labels,
    })
}

/// `POST /api/run/linear/create-issue` — file a standalone issue in a team of
/// the project's own workspace: how a workflow hands its result to people, for
/// instance a scheduled report.
///
/// Unlike a sub-issue, the run names the team — there is no parent to read it
/// from. It can only name one the project's connection can see, so the
/// workspace boundary holds. The issue links back to the run that filed it.
pub async fn create_issue(
    Extension(state): Extension<Arc<RunsState>>,
    headers: HeaderMap,
    Json(req): Json<CreateIssueRequest>,
) -> Response {
    let g = match grant(&headers) {
        Ok(g) => g,
        Err((s, m)) => return err(s, m),
    };
    if req.title.trim().is_empty() {
        return err(StatusCode::BAD_REQUEST, "the issue needs a title");
    }
    let c = match client(&state, &g).await {
        Ok(c) => c,
        Err((s, m)) => return err(s, m),
    };
    let discovery = match c.discover().await {
        Ok(d) => d,
        Err(e) => return err(UPSTREAM_FAILED, e.0),
    };
    let place = match resolve_placement(&discovery, &req) {
        Ok(p) => p,
        Err(m) => return err(StatusCode::BAD_REQUEST, m),
    };
    let description = with_run_link(&req.description, &g.run_id, state.public_url());
    match c
        .create_issue(
            &place.team,
            req.title.trim(),
            &description,
            place.state.as_deref(),
            &place.labels,
            None,
        )
        .await
    {
        Ok(created) => Json(created).into_response(),
        Err(e) => err(UPSTREAM_FAILED, e.0),
    }
}

/// `description` with a closing line naming the run that filed it, so a reader
/// can get from the issue to the full report.
fn with_run_link(description: &str, run_id: &str, base: Option<String>) -> String {
    let run = match base {
        Some(base) => format!("[run `{run_id}`]({base}/runs/{run_id})"),
        None => format!("run `{run_id}`"),
    };
    let body = description.trim_end();
    if body.is_empty() {
        format!("Filed by ai-harness {run}.")
    } else {
        format!("{body}\n\n---\nFiled by ai-harness {run}.")
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
    // Resolve the status *name* to an id before the mutation.
    //
    // `issueUpdate` takes `stateId` and nothing else: handed "In Progress" it
    // answers `Argument Validation Error`, which reads like a bad request
    // rather than "that was a name, not an id". The `linear:` step kind is
    // written in names -- that is the whole point of it -- so this is where the
    // two have to meet. Matching is by id, or name ignoring case, the same as
    // `resolve_placement` does for a created issue.
    //
    // Against the issue's **own team**, not the workspace: a state id from
    // another team is a valid uuid that Linear still refuses, and two teams
    // routinely share status names.
    let issue = match c.issue_labels(&req.issue).await {
        Ok(i) => i,
        Err(e) => return err(StatusCode::NOT_FOUND, e.0),
    };
    let discovery = match c.discover().await {
        Ok(d) => d,
        Err(e) => return err(UPSTREAM_FAILED, e.0),
    };
    let team = match issue
        .team_id
        .as_deref()
        .and_then(|id| discovery.teams.iter().find(|t| t.id == id))
    {
        Some(t) => t,
        None => {
            return err(
                UPSTREAM_FAILED,
                format!(
                    "could not read {}'s team, so `{}` cannot be resolved to a status",
                    issue.identifier,
                    req.state.trim()
                ),
            )
        }
    };
    let Some(target) = team
        .states
        .iter()
        .find(|st| matches(&req.state, &st.id, &[&st.name]))
    else {
        // Name what is there: the message is read in a failed run's log by
        // whoever wrote the workflow, and the fix is nearly always a typo.
        let known: Vec<&str> = team.states.iter().map(|st| st.name.as_str()).collect();
        return err(
            StatusCode::BAD_REQUEST,
            format!(
                "no status `{}` in team {} ({}); statuses: {}",
                req.state.trim(),
                team.key,
                team.name,
                known.join(", ")
            ),
        );
    };

    match c.set_issue_state(&issue.id, &target.id).await {
        Ok(()) => Json(serde_json::json!({
            "moved": issue.identifier,
            "state": target.name,
        }))
        .into_response(),
        Err(e) => err(UPSTREAM_FAILED, e.0),
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
        Err(e) => err(UPSTREAM_FAILED, e.to_string()),
    }
}

/// `POST /api/run/linear/epic-review-state` — where a finished epic goes.
///
/// Read off the binding that started this run, so the column is picked from a
/// dropdown like every other one rather than pasted in as a state id.
/// `EPIC_REVIEW_STATE` still wins in the workflow when it is set.
pub async fn epic_review_state(
    Extension(state): Extension<Arc<RunsState>>,
    headers: HeaderMap,
) -> Response {
    let g = match grant(&headers) {
        Ok(g) => g,
        Err((s, m)) => return err(s, m),
    };
    let claims = match state.linear_claim_store().await {
        Ok(c) => c,
        Err(e) => return err(StatusCode::SERVICE_UNAVAILABLE, e.to_string()),
    };
    let Ok(Some(claim)) = claims.claim_for_run(&g.run_id).await else {
        return err(StatusCode::NOT_FOUND, "this run has no Linear claim");
    };
    let sources = match state.linear_source_store().await {
        Ok(s) => s,
        Err(e) => return err(StatusCode::SERVICE_UNAVAILABLE, e.to_string()),
    };
    match sources.get(&claim.project, &claim.workflow).await {
        Ok(Some(b)) => match b.epic_review_state_id {
            Some(id) => Json(serde_json::json!({ "state": id })).into_response(),
            // Not an error: leaving the epic where it is is a valid choice, and
            // the only other answer would be a guess.
            None => Json(serde_json::json!({ "state": null })).into_response(),
        },
        Ok(None) => err(StatusCode::NOT_FOUND, "no binding started this run"),
        Err(e) => err(UPSTREAM_FAILED, e.to_string()),
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
        Err(e) => err(UPSTREAM_FAILED, e.0),
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
        Err(e) => err(UPSTREAM_FAILED, e.0),
    }
}

/// Most issues one listing returns, and how many when the run doesn't say.
const ISSUES_MAX: usize = 100;
const ISSUES_DEFAULT: usize = 25;

#[derive(Debug, Deserialize)]
pub struct IssuesRequest {
    /// The team's key (`ECOM`), name or id.
    pub team: String,
    /// A status name or id.
    #[serde(default)]
    pub state: Option<String>,
    /// `none` for unassigned issues, or an email address.
    #[serde(default)]
    pub assignee: Option<String>,
    /// Label patterns the issue must all carry.
    #[serde(default)]
    pub labels: Vec<String>,
    /// Label patterns the issue must carry none of.
    #[serde(default)]
    pub without_labels: Vec<String>,
    #[serde(default)]
    pub limit: Option<usize>,
}

/// Turn a listing request into a query against the project's workspace.
fn issue_query(
    discovery: &harness_sources::linear::Discovery,
    req: &IssuesRequest,
) -> Result<harness_sources::linear::IssueQuery, String> {
    use harness_sources::linear::{AssigneeFilter, IssueQuery};
    let place = resolve_placement(
        discovery,
        &CreateIssueRequest {
            team: req.team.clone(),
            title: String::new(),
            description: String::new(),
            state: req.state.clone(),
            labels: Vec::new(),
        },
    )?;
    let assignee = match req
        .assignee
        .as_deref()
        .map(str::trim)
        .filter(|a| !a.is_empty())
    {
        None => AssigneeFilter::Any,
        Some(a) if a.eq_ignore_ascii_case("none") => AssigneeFilter::Unassigned,
        Some(a) if a.contains('@') => AssigneeFilter::Email(a.to_string()),
        Some(a) => {
            return Err(format!(
                "assignee `{a}` is neither `none` nor an email address"
            ))
        }
    };
    Ok(IssueQuery {
        team_id: place.team,
        state_id: place.state,
        assignee,
        with_labels: req.labels.clone(),
        without_labels: req.without_labels.clone(),
        limit: req.limit.unwrap_or(ISSUES_DEFAULT).clamp(1, ISSUES_MAX),
    })
}

/// `POST /api/run/linear/issues` — list a team's issues. Read-only.
///
/// What a triage workflow starts from: which issues are waiting, with enough
/// of each (title, description, labels) to judge it. The team has to be one
/// the project's own connection can see, as for filing an issue.
pub async fn issues(
    Extension(state): Extension<Arc<RunsState>>,
    headers: HeaderMap,
    Json(req): Json<IssuesRequest>,
) -> Response {
    let g = match grant(&headers) {
        Ok(g) => g,
        Err((s, m)) => return err(s, m),
    };
    let c = match client(&state, &g).await {
        Ok(c) => c,
        Err((s, m)) => return err(s, m),
    };
    let discovery = match c.discover().await {
        Ok(d) => d,
        Err(e) => return err(UPSTREAM_FAILED, e.0),
    };
    let query = match issue_query(&discovery, &req) {
        Ok(q) => q,
        Err(m) => return err(StatusCode::BAD_REQUEST, m),
    };
    match c.team_issues(&query).await {
        Ok(issues) => Json(issues).into_response(),
        Err(e) => err(UPSTREAM_FAILED, e.0),
    }
}

#[derive(Debug, Deserialize)]
pub struct LabelsRequest {
    /// The issue's id or identifier (`ECOM-12`).
    pub issue: String,
    /// Label names to put on it.
    #[serde(default)]
    pub add: Vec<String>,
    /// Label names to take off it.
    #[serde(default)]
    pub remove: Vec<String>,
}

/// `POST /api/run/linear/labels` — add and remove an issue's labels, by name.
///
/// **Existing labels only.** A name that isn't a label in the issue's team or
/// workspace is refused before anything changes, so a typo can't half-apply a
/// set or invent a label nobody asked for. Adding one the issue already has, or
/// removing one it lacks, is a no-op rather than an error: a triage pass that
/// runs twice should land in the same place.
pub async fn labels(
    Extension(state): Extension<Arc<RunsState>>,
    headers: HeaderMap,
    Json(req): Json<LabelsRequest>,
) -> Response {
    let g = match grant(&headers) {
        Ok(g) => g,
        Err((s, m)) => return err(s, m),
    };
    let wanted = |names: &[String]| -> Vec<String> {
        names
            .iter()
            .map(|n| n.trim().to_string())
            .filter(|n| !n.is_empty())
            .collect()
    };
    let (add, remove) = (wanted(&req.add), wanted(&req.remove));
    if add.is_empty() && remove.is_empty() {
        return err(StatusCode::BAD_REQUEST, "name a label to add or remove");
    }
    if let Some(both) = add
        .iter()
        .find(|a| remove.iter().any(|r| r.eq_ignore_ascii_case(a)))
    {
        return err(
            StatusCode::BAD_REQUEST,
            format!("`{both}` is both added and removed"),
        );
    }
    let c = match client(&state, &g).await {
        Ok(c) => c,
        Err((s, m)) => return err(s, m),
    };
    let issue = match c.issue_labels(&req.issue).await {
        Ok(i) => i,
        Err(e) => return err(StatusCode::NOT_FOUND, e.0),
    };

    // Resolve every name first, so nothing changes unless all of them exist.
    let mut resolved = Vec::new();
    for (name, adding) in add
        .iter()
        .map(|n| (n, true))
        .chain(remove.iter().map(|n| (n, false)))
    {
        match c.find_label(name, issue.team_id.as_deref()).await {
            Ok(Some(id)) => resolved.push((id, name, adding)),
            Ok(None) => {
                return err(
                    StatusCode::BAD_REQUEST,
                    format!(
                        "no label `{name}` in {}'s team or workspace — labels are never created \
                         from a workflow; add it in Linear first",
                        issue.identifier
                    ),
                )
            }
            Err(e) => return err(UPSTREAM_FAILED, e.0),
        }
    }

    let mut now: Vec<(String, String)> = issue.labels.clone();
    for (id, name, adding) in resolved {
        let has = now.iter().any(|(l, _)| *l == id);
        let done = match (adding, has) {
            (true, false) => c.add_label(&issue.id, &id).await.map(|()| {
                now.push((id.clone(), name.clone()));
            }),
            (false, true) => c.remove_label(&issue.id, &id).await.map(|()| {
                now.retain(|(l, _)| *l != id);
            }),
            _ => Ok(()),
        };
        if let Err(e) = done {
            return err(UPSTREAM_FAILED, e.0);
        }
    }
    Json(serde_json::json!({
        "issue": issue.identifier,
        "labels": now.into_iter().map(|(_, name)| name).collect::<Vec<_>>(),
    }))
    .into_response()
}

#[cfg(test)]
mod tests {
    use super::*;
    use harness_sources::linear::{Discovery, Label, Team, WorkflowState};

    fn workspace() -> Discovery {
        Discovery {
            teams: vec![Team {
                id: "t-ecom".into(),
                name: "E-commerce".into(),
                key: "ECOM".into(),
                states: vec![
                    WorkflowState {
                        id: "s-triage".into(),
                        name: "Triage".into(),
                        kind: "triage".into(),
                        position: 0.0,
                    },
                    WorkflowState {
                        id: "s-todo".into(),
                        name: "Todo".into(),
                        kind: "unstarted".into(),
                        position: 1.0,
                    },
                ],
                labels: vec![Label {
                    id: "l-perf".into(),
                    name: "Performance".into(),
                }],
            }],
        }
    }

    fn req(team: &str, state: Option<&str>, labels: &[&str]) -> CreateIssueRequest {
        CreateIssueRequest {
            team: team.into(),
            title: "Weekly INP report".into(),
            description: String::new(),
            state: state.map(str::to_string),
            labels: labels.iter().map(|l| l.to_string()).collect(),
        }
    }

    #[test]
    fn a_linear_failure_uses_a_status_proxies_pass_through() {
        // Cloudflare swaps a 502, 503 or 504 from the origin for its own page,
        // and the run then never sees why its step failed.
        for replaced in [
            StatusCode::BAD_GATEWAY,
            StatusCode::SERVICE_UNAVAILABLE,
            StatusCode::GATEWAY_TIMEOUT,
        ] {
            assert_ne!(UPSTREAM_FAILED, replaced);
        }
        assert_eq!(err(UPSTREAM_FAILED, "x").status().as_u16(), 424);
    }

    fn issues_req(assignee: Option<&str>, limit: Option<usize>) -> IssuesRequest {
        IssuesRequest {
            team: "ecom".into(),
            state: Some("todo".into()),
            assignee: assignee.map(str::to_string),
            labels: vec![],
            without_labels: vec!["harness:*".into()],
            limit,
        }
    }

    #[test]
    fn a_listing_resolves_its_team_state_and_assignee() {
        use harness_sources::linear::AssigneeFilter;
        let q = issue_query(&workspace(), &issues_req(Some("none"), Some(10))).unwrap();
        assert_eq!(q.team_id, "t-ecom");
        assert_eq!(q.state_id.as_deref(), Some("s-todo"));
        assert_eq!(q.assignee, AssigneeFilter::Unassigned);
        assert_eq!(q.without_labels, vec!["harness:*".to_string()]);
        assert_eq!(q.limit, 10);

        let by_email = issue_query(&workspace(), &issues_req(Some("a@x.test"), None)).unwrap();
        assert_eq!(by_email.assignee, AssigneeFilter::Email("a@x.test".into()));
        assert_eq!(by_email.limit, ISSUES_DEFAULT);
        // A limit is clamped rather than refused.
        let huge = issue_query(&workspace(), &issues_req(None, Some(10_000))).unwrap();
        assert_eq!(huge.limit, ISSUES_MAX);
        assert_eq!(huge.assignee, AssigneeFilter::Any);

        let err = issue_query(&workspace(), &issues_req(Some("mark"), None)).unwrap_err();
        assert!(err.contains("neither `none` nor an email"), "{err}");
        let err = issue_query(
            &workspace(),
            &IssuesRequest {
                team: "nope".into(),
                ..issues_req(None, None)
            },
        )
        .unwrap_err();
        assert!(err.contains("ECOM (E-commerce)"), "{err}");
    }

    #[test]
    fn names_resolve_to_ids_ignoring_case() {
        let place = resolve_placement(&workspace(), &req("ecom", Some("triage"), &["performance"]))
            .unwrap();
        assert_eq!(
            place,
            Placement {
                team: "t-ecom".into(),
                state: Some("s-triage".into()),
                labels: vec!["l-perf".into()],
            }
        );
        // By full name, and by id, too; no state means the team's default.
        let by_name = resolve_placement(&workspace(), &req("E-commerce", None, &[])).unwrap();
        assert_eq!(by_name.team, "t-ecom");
        assert_eq!(by_name.state, None);
        let by_id =
            resolve_placement(&workspace(), &req("t-ecom", Some("s-todo"), &["l-perf"])).unwrap();
        assert_eq!(by_id.state.as_deref(), Some("s-todo"));
    }

    #[test]
    fn a_miss_lists_what_is_there() {
        let team = resolve_placement(&workspace(), &req("SHOP", None, &[])).unwrap_err();
        assert!(team.contains("ECOM (E-commerce)"), "{team}");
        let state = resolve_placement(&workspace(), &req("ECOM", Some("Done"), &[])).unwrap_err();
        assert!(state.contains("Triage, Todo"), "{state}");
        let label = resolve_placement(&workspace(), &req("ECOM", None, &["perf"])).unwrap_err();
        assert!(label.contains("Performance"), "{label}");
    }

    #[test]
    fn the_issue_links_back_to_its_run() {
        assert_eq!(
            with_run_link("Report\n", "r1", Some("https://h.example".into())),
            "Report\n\n---\nFiled by ai-harness [run `r1`](https://h.example/runs/r1)."
        );
        assert_eq!(
            with_run_link("", "r1", None),
            "Filed by ai-harness run `r1`."
        );
    }
    /// `move_state` is written in status *names* by the `linear:` step kind,
    /// but `issueUpdate` takes only `stateId`. Handing Linear a name answers
    /// `Argument Validation Error`, which is how a `task` run died on its very
    /// first node on 2026-11-07. Resolution has to happen before the mutation,
    /// and against the issue's own team.
    #[test]
    fn a_status_resolves_by_name_or_id_within_one_team() {
        let team = &workspace().teams[0];
        let find = |wanted: &str| {
            team.states
                .iter()
                .find(|st| matches(wanted, &st.id, &[&st.name]))
                .map(|st| st.id.as_str())
        };
        assert_eq!(find("Todo"), Some("s-todo"));
        assert_eq!(find("todo"), Some("s-todo"), "case is ignored");
        assert_eq!(find("  Todo "), Some("s-todo"), "padding is trimmed");
        assert_eq!(find("s-todo"), Some("s-todo"), "an id still works");
        // The failure mode that mattered: a name nobody has is a refusal here,
        // not a uuid-shaped string posted to Linear.
        assert_eq!(find("In Progress"), None);
    }
}
