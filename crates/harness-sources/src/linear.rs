//! Linear GraphQL client — read-only discovery + issue preview (Slice 1).
//!
//! Auth is either an **OAuth access token** from an `actor=app` install (sent as
//! `Bearer …`, so Linear attributes writes to the application) or a legacy
//! **personal API key** (sent verbatim, which attributes every write to the
//! human who minted the key) — see [`LinearAuth`]. Parsing is split from the
//! HTTP call so the response shaping is unit-tested with fixtures (no mock
//! server), matching the `intake/github_issues` pattern.

use serde::{Deserialize, Serialize};

const LINEAR_GRAPHQL_URL: &str = "https://api.linear.app/graphql";

#[derive(Debug, thiserror::Error)]
#[error("linear: {0}")]
pub struct LinearError(pub String);

// ── Public, clean types (Serialize → these are what the API returns) ─────────

/// A Linear workspace's teams + their states and labels — the dropdown data for
/// the trigger block.
#[derive(Debug, Clone, Serialize)]
pub struct Discovery {
    pub teams: Vec<Team>,
}

#[derive(Debug, Clone, Serialize)]
pub struct Team {
    pub id: String,
    pub name: String,
    pub key: String,
    pub states: Vec<WorkflowState>,
    pub labels: Vec<Label>,
}

/// A workflow state (column). `kind` is Linear's state type — one of
/// `triage` / `backlog` / `unstarted` / `started` / `completed` / `canceled` —
/// which the UI uses to order/group the status pickers.
#[derive(Debug, Clone, Serialize)]
pub struct WorkflowState {
    pub id: String,
    pub name: String,
    pub kind: String,
    pub position: f64,
}

#[derive(Debug, Clone, Serialize)]
pub struct Label {
    pub id: String,
    pub name: String,
}

/// The Linear workspace a credential belongs to — recorded at connect time so
/// the UI can name what it's talking to.
#[derive(Debug, Clone, Serialize)]
pub struct Workspace {
    pub id: String,
    pub name: String,
    /// The workspace's URL slug (`linear.app/<url_key>/…`).
    pub url_key: String,
}

/// A newly created issue — the fields surfaced back to the caller.
#[derive(Debug, Clone, Serialize)]
pub struct CreatedIssue {
    pub id: String,
    pub identifier: String,
    pub url: String,
}

/// An issue matched by a preview filter.
#[derive(Debug, Clone, Serialize)]
pub struct Issue {
    /// Linear internal id (used for state/comment mutations).
    pub id: String,
    pub identifier: String,
    pub title: String,
    pub url: String,
    /// Issue description (markdown) — the task spec fed to the fired run.
    pub body: Option<String>,
    pub labels: Vec<String>,
}
/// A sub-issue of an epic, in board order.
///
/// Ordered because the epic orchestrator builds them one at a time and has to
/// know which is next; `state` because the board *is* the progress ledger, so
/// "which column is this in" is the only record of what has been done.
#[derive(Debug, Clone, Serialize)]
pub struct SubIssue {
    /// Linear internal id (what mutations take).
    pub id: String,
    /// Human identifier, e.g. `AIH-12`.
    pub identifier: String,
    pub title: String,
    pub url: String,
    /// Acceptance criteria live in the body — the supervisor grades against it.
    pub body: Option<String>,
    /// Workflow state id, for moving it on.
    pub state_id: String,
    /// Workflow state name, e.g. `Queued`.
    pub state: String,
    /// Linear's own category for that state: `backlog`, `unstarted`, `started`,
    /// `completed` or `canceled`.
    ///
    /// What makes "which piece has not begun" answerable without configuration.
    /// A name is whatever somebody typed; the type is what Linear means by it.
    pub state_type: String,
    pub labels: Vec<String>,
    /// When this piece reached a completed status, if it has.
    ///
    /// The order pieces were *built* in, which `sortOrder` stops being: Linear
    /// reassigns it when an issue is moved between columns, so by the time an
    /// epic finishes it reflects where the cards ended up on the board rather
    /// than what happened first.
    pub completed_at: Option<String>,
    /// Linear's own ordering within the parent. Ascending is board order.
    pub sort_order: f64,
}

/// A comment on a Linear issue (reviewer feedback fed to `revise-pr`).
#[derive(Debug, Clone, Serialize)]
pub struct Comment {
    /// Markdown body of the comment.
    pub body: String,
    /// Display name of the author, or "unknown" if absent.
    pub author: String,
    /// ISO-8601 creation timestamp (Linear `createdAt`).
    pub created_at: String,
}

// ── GraphQL wire types (private) ─────────────────────────────────────────────

#[derive(Deserialize)]
struct Conn<T> {
    #[serde(default = "Vec::new")]
    nodes: Vec<T>,
}

#[derive(Deserialize)]
struct DiscoveryData {
    teams: Conn<TeamNode>,
}

#[derive(Deserialize)]
struct TeamNode {
    id: String,
    name: String,
    key: String,
    states: Conn<StateNode>,
    labels: Conn<LabelNode>,
}

#[derive(Deserialize)]
struct StateNode {
    id: String,
    name: String,
    #[serde(rename = "type")]
    kind: String,
    position: f64,
}

#[derive(Deserialize)]
struct LabelNode {
    id: String,
    name: String,
}

#[derive(Deserialize)]
struct OrganizationData {
    organization: OrganizationNode,
}

#[derive(Deserialize)]
struct OrganizationNode {
    id: String,
    name: String,
    #[serde(rename = "urlKey")]
    url_key: String,
}

#[derive(Deserialize)]
struct IssuesData {
    issues: Conn<IssueNode>,
}

#[derive(Deserialize)]
struct IssueNode {
    id: String,
    identifier: String,
    title: String,
    url: String,
    #[serde(default)]
    description: Option<String>,
    #[serde(default)]
    labels: Option<Conn<IssueLabelNode>>,
}

// Issue labels are fetched by name only (the issues query omits label ids).
#[derive(Deserialize)]
struct IssueLabelNode {
    name: String,
}
#[derive(Deserialize)]
struct ChildrenData {
    // `issue` is null when the id doesn't resolve.
    #[serde(default)]
    issue: Option<IssueChildrenNode>,
}
#[derive(Deserialize)]
struct IssueChildrenNode {
    children: Conn<ChildNode>,
}
#[derive(Deserialize)]
struct ChildNode {
    id: String,
    identifier: String,
    title: String,
    url: String,
    #[serde(default)]
    description: Option<String>,
    // A sub-issue always has a state in practice; treated as optional so one
    // malformed node cannot fail the whole epic read.
    #[serde(default)]
    state: Option<ChildStateNode>,
    #[serde(default)]
    labels: Option<Conn<IssueLabelNode>>,
    #[serde(rename = "sortOrder", default)]
    sort_order: f64,
    #[serde(rename = "completedAt", default)]
    completed_at: Option<String>,
}
#[derive(Deserialize)]
struct ChildStateNode {
    id: String,
    name: String,
    #[serde(rename = "type", default)]
    kind: Option<String>,
}

#[derive(Deserialize)]
struct CommentsData {
    // `issue` is null when the id doesn't resolve.
    #[serde(default)]
    issue: Option<IssueCommentsNode>,
}
#[derive(Deserialize)]
struct IssueCommentsNode {
    comments: Conn<CommentNode>,
}
#[derive(Deserialize)]
struct CommentNode {
    body: String,
    #[serde(default)]
    user: Option<CommentUserNode>,
    #[serde(rename = "createdAt")]
    created_at: String,
}
#[derive(Deserialize)]
struct CommentUserNode {
    name: String,
}

// ── GraphQL documents ────────────────────────────────────────────────────────

// Linear caps GraphQL query complexity at 10,000, charged on the *requested*
// page sizes (not the rows actually returned). `teams(first: 250)` with the
// nested `states`/`labels` connections (default 50 each) costs ~32,800 and is
// rejected with HTTP 400 "Query too complex". Capping teams at 50 keeps it
// ~6,500 — comfortably under the limit, while still covering far more teams
// than any realistic workspace.
const DISCOVERY_QUERY: &str = r#"
query Discovery {
  teams(first: 50) {
    nodes {
      id name key
      states { nodes { id name type position } }
      labels { nodes { id name } }
    }
  }
}"#;

// Cheapest possible authenticated query — used as the connect-time probe that a
// freshly exchanged token works, and to record which workspace it belongs to.
const ORGANIZATION_QUERY: &str = r#"
query Organization {
  organization { id name urlKey }
}"#;

// The app's own user id in this workspace. Under an `actor=app` token `viewer`
// resolves to the app user, which is who delegated issues are assigned to.
const ME_QUERY: &str = r#"
query Me {
  viewer { id }
}"#;

const AGENT_ACTIVITY_MUTATION: &str = r#"
mutation AgentActivityCreate($input: AgentActivityCreateInput!) {
  agentActivityCreate(input: $input) { success }
}"#;

// Open an agent session on an issue the harness picked up itself. Delegation
// creates one for us; a poller-claimed run has none, and without a session there
// is nowhere to stream progress — so it opens its own.
//
// `agentSessionCreateOnIssue`, NOT `agentSessionCreate`. The latter takes an
// explicit `appUserId`, which looked preferable — the session would be
// unambiguously ours rather than inferred from the calling credential — but the
// schema marks it `[Internal] Creates a new agent session on behalf of the
// current user`, and Linear answers a third-party app with `Access denied`.
// Naming an arbitrary app user is precisely the privilege it withholds.
// `agentSessionCreateOnIssue` is the public mutation and infers the app from the
// token, which is why `create_agent_session` requires an app-actor credential.
const AGENT_SESSION_CREATE_MUTATION: &str = r#"
mutation AgentSessionCreateOnIssue($input: AgentSessionCreateOnIssue!) {
  agentSessionCreateOnIssue(input: $input) { success agentSession { id } }
}"#;

// Where a single issue sits — the team that maps it to a project binding, and the
// status that binding gates on. `state` is non-null on Issue; `delegate` is null
// until an agent is delegated to it.
const ISSUE_CONTEXT_QUERY: &str = r#"
query IssueContext($id: String!) {
  issue(id: $id) {
    identifier
    title
    team { id }
    state { id name }
    delegate { id }
    parent { id identifier }
    labels { nodes { name } }
  }
}"#;

// Issues in a team's column that are **delegated to this app**. `delegate`
// (`IssueFilter.delegate: NullableUserFilter`) is Linear's agent-delegation
// field — "the agent user that is delegated to work on this issue" — and is what
// replaced the old eligibility label as the pickup signal. Filtering server-side
// keeps the response small and makes the gate unmissable.
const ISSUES_QUERY: &str = r#"
query Preview($teamId: ID!, $stateId: ID!, $delegateId: ID!) {
  issues(first: 50, filter: {
    team: { id: { eq: $teamId } },
    state: { id: { eq: $stateId } },
    delegate: { id: { eq: $delegateId } }
  }) {
    nodes { id identifier title url description labels { nodes { name } } }
  }
}"#;
// A single issue's comments connection is flat (no nested connections), so
// even first: 50 stays well under Linear's 10k complexity cap. Linear returns
// comments in createdAt order (oldest first in practice), so the reviewer's
// narrative reads top-to-bottom.
const COMMENTS_QUERY: &str = r#"
query Comments($id: String!) {
  issue(id: $id) {
    comments(first: 50) {
      nodes { body createdAt user { name } }
    }
  }
}"#;

// An epic's sub-issues. `first: 100` because an epic that decomposes into more
// than a hundred pieces is a planning failure, not a paging problem — and the
// orchestrator caps corrective sub-issues well below that.
const CHILDREN_QUERY: &str = r#"
query Children($id: String!) {
  issue(id: $id) {
    children(first: 100) {
      nodes {
        id identifier title url description sortOrder completedAt
        state { id name type }
        labels { nodes { name } }
      }
    }
  }
}"#;

// ── Parsing (pure, fixture-tested) ───────────────────────────────────────────

fn gql_data<T: serde::de::DeserializeOwned>(json: &[u8]) -> Result<T, LinearError> {
    let v: serde_json::Value =
        serde_json::from_slice(json).map_err(|e| LinearError(format!("bad response: {e}")))?;
    if let Some(errs) = v
        .get("errors")
        .and_then(|e| e.as_array())
        .filter(|a| !a.is_empty())
    {
        let msg = errs
            .iter()
            .filter_map(|e| e.get("message").and_then(|m| m.as_str()))
            .collect::<Vec<_>>()
            .join("; ");
        return Err(LinearError(format!("graphql error: {msg}")));
    }
    let data = v
        .get("data")
        .cloned()
        .ok_or_else(|| LinearError("response had no data".into()))?;
    serde_json::from_value(data).map_err(|e| LinearError(format!("bad response: {e}")))
}

/// Summarize an error response body for inclusion in a `LinearError`. Linear
/// returns a JSON `{ "errors": [{ "message": … }] }` even on HTTP 4xx (e.g. an
/// invalid API key yields a 400, not a 401), so surfacing those messages is the
/// difference between a useless "HTTP 400" and an actionable cause. Falls back
/// to a truncated raw body when it isn't the expected shape.
fn error_detail(bytes: &[u8]) -> String {
    if let Ok(v) = serde_json::from_slice::<serde_json::Value>(bytes) {
        if let Some(msgs) = v.get("errors").and_then(|e| e.as_array()) {
            let joined = msgs
                .iter()
                .filter_map(|e| e.get("message").and_then(|m| m.as_str()))
                .collect::<Vec<_>>()
                .join("; ");
            if !joined.is_empty() {
                return joined;
            }
        }
    }
    let raw = String::from_utf8_lossy(bytes);
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        "(empty response body)".to_string()
    } else {
        trimmed.chars().take(300).collect()
    }
}

/// Parse a discovery response body into clean types.
pub fn parse_discovery(json: &[u8]) -> Result<Discovery, LinearError> {
    let data: DiscoveryData = gql_data(json)?;
    let teams = data
        .teams
        .nodes
        .into_iter()
        .map(|t| Team {
            id: t.id,
            name: t.name,
            key: t.key,
            states: t
                .states
                .nodes
                .into_iter()
                .map(|s| WorkflowState {
                    id: s.id,
                    name: s.name,
                    kind: s.kind,
                    position: s.position,
                })
                .collect(),
            labels: t
                .labels
                .nodes
                .into_iter()
                .map(|l| Label {
                    id: l.id,
                    name: l.name,
                })
                .collect(),
        })
        .collect();
    Ok(Discovery { teams })
}

/// Parse an organization response into a [`Workspace`].
pub fn parse_organization(json: &[u8]) -> Result<Workspace, LinearError> {
    let data: OrganizationData = gql_data(json)?;
    Ok(Workspace {
        id: data.organization.id,
        name: data.organization.name,
        url_key: data.organization.url_key,
    })
}

/// Parse an issues response. `labels` are still returned per issue — the
/// failed-label lifecycle filters on them — but there is no eligibility-label
/// gate any more: work reaches the harness by being delegated to it in Linear.
pub fn parse_issues(json: &[u8]) -> Result<Vec<Issue>, LinearError> {
    let data: IssuesData = gql_data(json)?;
    let issues = data
        .issues
        .nodes
        .into_iter()
        .map(|i| Issue {
            id: i.id,
            identifier: i.identifier,
            title: i.title,
            url: i.url,
            body: i.description,
            labels: i
                .labels
                .map(|c| c.nodes.into_iter().map(|l| l.name).collect())
                .unwrap_or_default(),
        })
        .collect();
    Ok(issues)
}
/// Parse an issue's comments response into clean types. Returns an empty vec
/// if the issue id didn't resolve (`issue: null`).
pub fn parse_comments(json: &[u8]) -> Result<Vec<Comment>, LinearError> {
    let data: CommentsData = gql_data(json)?;
    let comments = data
        .issue
        .map(|i| i.comments.nodes)
        .unwrap_or_default()
        .into_iter()
        .map(|c| Comment {
            body: c.body,
            author: c.user.map(|u| u.name).unwrap_or_else(|| "unknown".into()),
            created_at: c.created_at,
        })
        .collect();
    Ok(comments)
}

/// Parse an epic's `children` connection into board-ordered [`SubIssue`]s.
///
/// Sorted here rather than trusted from the response: Linear returns children in
/// creation order, and the orchestrator's whole model is "build them in the
/// order the plan set", which is `sortOrder`.
pub fn parse_children(json: &[u8]) -> Result<Vec<SubIssue>, LinearError> {
    let data: ChildrenData = gql_data(json)?;
    let mut out: Vec<SubIssue> = data
        .issue
        .map(|i| i.children.nodes)
        .unwrap_or_default()
        .into_iter()
        .map(|c| {
            let (state_id, state, state_type) = c
                .state
                .map(|s| (s.id, s.name, s.kind.unwrap_or_default()))
                .unwrap_or_else(|| (String::new(), "unknown".into(), String::new()));
            SubIssue {
                id: c.id,
                identifier: c.identifier,
                title: c.title,
                url: c.url,
                body: c.description.filter(|d| !d.trim().is_empty()),
                state_id,
                state,
                state_type,
                labels: c
                    .labels
                    .map(|l| l.nodes.into_iter().map(|n| n.name).collect())
                    .unwrap_or_default(),
                sort_order: c.sort_order,
                completed_at: c.completed_at,
            }
        })
        .collect();
    // `total_cmp`, not `partial_cmp().unwrap()`: a NaN from a malformed
    // response would panic, and an epic read must not.
    out.sort_by(|a, b| a.sort_order.total_cmp(&b.sort_order));
    Ok(out)
}

// ── Agent sessions (delegation / @-mention) ──────────────────────────────────
//
// Delegating an issue to the app — or @-mentioning it — makes Linear open an
// **agent session** and deliver an `AgentSessionEvent` webhook. The session is
// the conversation surface: the harness reports progress into it as *agent
// activities* rather than as plain comments, and Linear marks a session
// unresponsive if no activity arrives within 10 seconds of `created`.

/// Which agent-session event arrived.
#[derive(Debug, Clone, PartialEq)]
pub enum AgentSessionAction {
    /// A session was opened (the issue was delegated, or the app was mentioned).
    Created,
    /// A follow-up message was posted into an existing session.
    Prompted,
    /// Any other action Linear may add; carried through so callers can ignore it
    /// without the parse failing.
    Other(String),
}

/// A parsed `AgentSessionEvent` webhook — the trigger for a delegated run.
///
/// Every field except `action` and `session_id` is optional: Linear's payload
/// varies by how the session was opened (delegation carries the issue, a mention
/// in a thread carries the comment), and being permissive here means an
/// unexpected shape degrades into a session we can still acknowledge instead of
/// a rejected webhook.
#[derive(Debug, Clone)]
pub struct AgentSessionEvent {
    pub action: AgentSessionAction,
    /// Target for [`LinearClient::create_agent_activity`].
    pub session_id: String,
    pub issue_id: Option<String>,
    pub issue_identifier: Option<String>,
    pub issue_title: Option<String>,
    pub issue_description: Option<String>,
    /// Body of the comment that opened the session, when it was a mention.
    pub comment_body: Option<String>,
    /// Linear's pre-formatted summary of the session's context.
    pub prompt_context: Option<String>,
    /// Workspace/team-level instructions for agents.
    pub guidance: Option<String>,
    /// The new message on a `prompted` event.
    pub prompt_body: Option<String>,
}

impl AgentSessionEvent {
    /// The task text to hand a workflow: prefer the issue's own description,
    /// falling back to Linear's formatted context, then the triggering comment.
    pub fn task_text(&self) -> Option<&str> {
        [
            self.issue_description.as_deref(),
            self.prompt_context.as_deref(),
            self.comment_body.as_deref(),
        ]
        .into_iter()
        .flatten()
        .find(|s| !s.trim().is_empty())
    }
}

/// An activity the agent emits into a session. `Response` is what tells Linear
/// the work is finished — sessions complete on it, so it is sent exactly once.
#[derive(Debug, Clone)]
pub enum AgentActivity {
    /// Internal reasoning. The `created` acknowledgement must be one of these.
    Thought { body: String },
    /// A step being taken. `result` is filled in when it's known.
    Action {
        action: String,
        parameter: String,
        result: Option<String>,
    },
    /// Terminal success — completes the session.
    Response { body: String },
    /// Terminal failure.
    Error { body: String },
}

impl AgentActivity {
    /// The `content` object of an `agentActivityCreate` input.
    fn content(&self) -> serde_json::Value {
        match self {
            Self::Thought { body } => serde_json::json!({ "type": "thought", "body": body }),
            Self::Action {
                action,
                parameter,
                result,
            } => {
                let mut v = serde_json::json!({
                    "type": "action", "action": action, "parameter": parameter,
                });
                if let Some(r) = result {
                    v["result"] = serde_json::Value::String(r.clone());
                }
                v
            }
            Self::Response { body } => serde_json::json!({ "type": "response", "body": body }),
            Self::Error { body } => serde_json::json!({ "type": "error", "body": body }),
        }
    }
}

// Wire types for the webhook payload. All nested pieces are optional — see
// `AgentSessionEvent`.
#[derive(Deserialize)]
struct AgentSessionEventWire {
    #[serde(rename = "type")]
    kind: String,
    #[serde(default)]
    action: Option<String>,
    #[serde(default)]
    #[serde(rename = "agentSession")]
    agent_session: Option<AgentSessionWire>,
    #[serde(default)]
    #[serde(rename = "agentActivity")]
    agent_activity: Option<AgentActivityWire>,
    #[serde(default)]
    #[serde(rename = "promptContext")]
    prompt_context: Option<String>,
    #[serde(default)]
    guidance: Option<String>,
}

#[derive(Deserialize)]
struct AgentSessionWire {
    #[serde(default)]
    id: Option<String>,
    #[serde(default)]
    issue: Option<SessionIssueWire>,
    #[serde(default)]
    comment: Option<SessionCommentWire>,
    // Linear also supplies these at session level on some payloads.
    #[serde(default)]
    #[serde(rename = "promptContext")]
    prompt_context: Option<String>,
    #[serde(default)]
    guidance: Option<String>,
}

#[derive(Deserialize)]
struct SessionIssueWire {
    #[serde(default)]
    id: Option<String>,
    #[serde(default)]
    identifier: Option<String>,
    #[serde(default)]
    title: Option<String>,
    #[serde(default)]
    description: Option<String>,
}

#[derive(Deserialize)]
struct SessionCommentWire {
    #[serde(default)]
    body: Option<String>,
}

#[derive(Deserialize)]
struct AgentActivityWire {
    #[serde(default)]
    body: Option<String>,
}

/// Parse an agent-session webhook body.
///
/// `Ok(None)` means "not an agent-session event" — Linear delivers other event
/// types to the same endpoint, and those are acknowledged and ignored rather
/// than treated as errors.
pub fn parse_agent_session_event(json: &[u8]) -> Result<Option<AgentSessionEvent>, LinearError> {
    let wire: AgentSessionEventWire =
        serde_json::from_slice(json).map_err(|e| LinearError(format!("bad webhook: {e}")))?;
    if wire.kind != "AgentSessionEvent" {
        return Ok(None);
    }
    let action = match wire.action.as_deref() {
        Some("created") => AgentSessionAction::Created,
        Some("prompted") => AgentSessionAction::Prompted,
        Some(other) => AgentSessionAction::Other(other.to_string()),
        None => return Err(LinearError("agent session event had no action".into())),
    };
    let session = wire.agent_session;
    let session_id = session
        .as_ref()
        .and_then(|s| s.id.clone())
        .ok_or_else(|| LinearError("agent session event had no agentSession.id".into()))?;
    let issue = session.as_ref().and_then(|s| s.issue.as_ref());
    Ok(Some(AgentSessionEvent {
        action,
        session_id,
        issue_id: issue.and_then(|i| i.id.clone()),
        issue_identifier: issue.and_then(|i| i.identifier.clone()),
        issue_title: issue.and_then(|i| i.title.clone()),
        issue_description: issue.and_then(|i| i.description.clone()),
        comment_body: session
            .as_ref()
            .and_then(|s| s.comment.as_ref())
            .and_then(|c| c.body.clone()),
        // Top-level wins; Linear has carried this at either level.
        prompt_context: wire
            .prompt_context
            .or_else(|| session.as_ref().and_then(|s| s.prompt_context.clone())),
        guidance: wire
            .guidance
            .or_else(|| session.as_ref().and_then(|s| s.guidance.clone())),
        prompt_body: wire.agent_activity.and_then(|a| a.body),
    }))
}

/// Where an issue sits: the team it belongs to, the status it is in, and the
/// agent (if any) it is delegated to. Every field is optional so an unresolvable
/// id degrades instead of erroring.
#[derive(Debug, Clone, Default, PartialEq, Serialize)]
pub struct IssueContext {
    /// Human identifier, e.g. `AIH-12`. Names the epic's integration branch.
    pub identifier: Option<String>,
    /// The issue's title, which becomes the epic's pull request title.
    pub title: Option<String>,
    pub team_id: Option<String>,
    pub state_id: Option<String>,
    /// Human-readable status name, for messages ("… is in Backlog").
    pub state_name: Option<String>,
    /// The agent user delegated to this issue, if any.
    pub delegate_id: Option<String>,
    /// The epic this is a piece of, if it is one.
    ///
    /// The supervisor fires on a *sub-issue* and has to reach its epic — to
    /// count what else is under it, and to file a corrective beside it. Nothing
    /// else in the client goes upward.
    pub parent_id: Option<String>,
    /// The epic's human identifier, which is what its branch is named
    /// after. Carried beside `parent_id` so naming a branch costs no
    /// second read.
    pub parent_identifier: Option<String>,
    /// Label names on the issue. `corrective` is how a fix round is counted,
    /// since the count lives in Linear rather than in a table here.
    pub labels: Vec<String>,
}

/// Parse an issue's team / status / delegate. An unresolvable issue id yields a
/// default (all-`None`) context rather than an error.
pub fn parse_issue_context(json: &[u8]) -> Result<IssueContext, LinearError> {
    #[derive(Deserialize)]
    struct Data {
        #[serde(default)]
        issue: Option<IssueNode>,
    }
    #[derive(Deserialize)]
    struct IssueNode {
        #[serde(default)]
        identifier: Option<String>,
        #[serde(default)]
        title: Option<String>,
        #[serde(default)]
        team: Option<IdRef>,
        #[serde(default)]
        state: Option<StateRef>,
        #[serde(default)]
        delegate: Option<IdRef>,
        #[serde(default)]
        parent: Option<NamedRef>,
        #[serde(default)]
        labels: Option<Conn<IssueLabelNode>>,
    }
    #[derive(Deserialize)]
    struct IdRef {
        id: String,
    }
    #[derive(Deserialize)]
    struct NamedRef {
        id: String,
        #[serde(default)]
        identifier: Option<String>,
    }
    #[derive(Deserialize)]
    struct StateRef {
        id: String,
        #[serde(default)]
        name: Option<String>,
    }
    let data: Data = gql_data(json)?;
    let Some(issue) = data.issue else {
        return Ok(IssueContext::default());
    };
    let parent = issue.parent;
    Ok(IssueContext {
        identifier: issue.identifier,
        title: issue.title,
        team_id: issue.team.map(|t| t.id),
        state_id: issue.state.as_ref().map(|s| s.id.clone()),
        state_name: issue.state.and_then(|s| s.name),
        delegate_id: issue.delegate.map(|d| d.id),
        parent_identifier: parent.as_ref().and_then(|p| p.identifier.clone()),
        parent_id: parent.map(|p| p.id),
        labels: issue
            .labels
            .map(|l| l.nodes.into_iter().map(|n| n.name).collect())
            .unwrap_or_default(),
    })
}

/// Parse `query Me { viewer { id } }` — the app's user id in this workspace.
pub fn parse_app_user_id(json: &[u8]) -> Result<String, LinearError> {
    #[derive(Deserialize)]
    struct Data {
        viewer: Viewer,
    }
    #[derive(Deserialize)]
    struct Viewer {
        id: String,
    }
    let data: Data = gql_data(json)?;
    Ok(data.viewer.id)
}

// ── Uploaded files (images pasted into issues and comments) ──────────────────

/// Host serving Linear's uploaded files. **The only host we ever fetch from.**
///
/// Issue and comment text is written by anyone who can file an issue, so treating
/// the URLs in it as fetchable would be an SSRF hole. Everything else found in the
/// markdown is left alone.
const UPLOADS_HOST: &str = "https://uploads.linear.app/";

/// A downloaded upload.
#[derive(Debug, Clone)]
pub struct Upload {
    pub bytes: Vec<u8>,
    /// The `Content-Type` Linear served it as.
    pub content_type: String,
}

impl Upload {
    /// File extension for the served content type, or `None` for a type we don't
    /// hand to models. Deliberately no SVG: it can carry script, and no model
    /// needs it.
    pub fn extension(&self) -> Option<&'static str> {
        match self.content_type.split(';').next()?.trim() {
            "image/png" => Some("png"),
            "image/jpeg" => Some("jpg"),
            "image/gif" => Some("gif"),
            "image/webp" => Some("webp"),
            _ => None,
        }
    }
}

/// Every `uploads.linear.app` URL in some markdown, in order of appearance and
/// deduplicated.
///
/// Matches bare occurrences rather than only markdown image syntax, so a link
/// (`[file](…)`) or a plain pasted URL is picked up too. Trailing markdown or
/// sentence punctuation is trimmed — Linear's upload paths are UUID segments, so
/// nothing legitimate ends in `)`, `"` or `.`.
pub fn extract_upload_urls(markdown: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for (idx, _) in markdown.match_indices(UPLOADS_HOST) {
        let tail = &markdown[idx..];
        let end = tail
            .find(|c: char| c.is_whitespace() || matches!(c, ')' | '"' | '\'' | '>' | '<' | '|'))
            .unwrap_or(tail.len());
        let url = tail[..end].trim_end_matches(['.', ',', ';', ':', '!', '?']);
        // A bare host with no path is not a file.
        if url.len() > UPLOADS_HOST.len() && !out.iter().any(|u| u == url) {
            out.push(url.to_string());
        }
    }
    out
}

// ── HTTP client ──────────────────────────────────────────────────────────────

/// How a [`LinearClient`] authenticates — and therefore **who Linear records as
/// the author** of everything the harness writes.
#[derive(Debug, Clone)]
pub enum LinearAuth {
    /// An OAuth access token from an `actor=app` install. Sent as `Bearer …`;
    /// comments, status moves and attachments are attributed to the *application*.
    /// This is the intended mode.
    OauthToken(String),
    /// A personal API key, sent in `Authorization` **verbatim** (not `Bearer`).
    /// Legacy: Linear resolves the key to the person who minted it, so the
    /// harness's comments read as written by that human.
    PersonalKey(String),
}

impl LinearAuth {
    /// The `Authorization` header value for this scheme.
    fn header_value(&self) -> String {
        match self {
            // Linear's OAuth tokens are ordinary bearer tokens.
            Self::OauthToken(t) => format!("Bearer {t}"),
            Self::PersonalKey(k) => k.clone(),
        }
    }

    /// Whether writes made with this credential are attributed to the app
    /// rather than a human — what the UI reports as the connection mode.
    pub fn is_app_actor(&self) -> bool {
        matches!(self, Self::OauthToken(_))
    }
}

/// How long a single Linear API call may take before it is abandoned.
///
/// reqwest applies **no** timeout by default, so a connection that opens and then
/// stalls waits forever. That is not merely slow: the poller awaits these calls
/// inline in its tick loop, so one hung request stops every claim from being
/// swept — no progress activities, no status transitions — for the lifetime of the
/// process, with nothing logged. Generous enough for a slow GraphQL query, finite
/// so a wedged socket cannot outlive it.
const REQUEST_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(30);
// A zero or absent timeout is the bug this constant exists to prevent, so it is
// checked at compile time rather than trusted to review.
const _: () = assert!(REQUEST_TIMEOUT.as_secs() > 0 && REQUEST_TIMEOUT.as_secs() <= 120);

/// A Linear GraphQL client.
pub struct LinearClient {
    http: reqwest::Client,
    auth: LinearAuth,
}

impl LinearClient {
    /// Build a client from a **personal API key** (legacy attribution — see
    /// [`LinearAuth::PersonalKey`]). Prefer [`Self::with_auth`] with an
    /// [`LinearAuth::OauthToken`].
    pub fn new(api_key: impl Into<String>) -> Self {
        Self::with_auth(LinearAuth::PersonalKey(api_key.into()))
    }

    /// Build a client for an explicit auth scheme.
    pub fn with_auth(auth: LinearAuth) -> Self {
        Self {
            // Falls back to an untimed client only if the builder fails, which it
            // does not for a timeout-only config.
            http: reqwest::Client::builder()
                .timeout(REQUEST_TIMEOUT)
                .build()
                .unwrap_or_else(|_| reqwest::Client::new()),
            auth,
        }
    }

    async fn post(&self, body: serde_json::Value) -> Result<Vec<u8>, LinearError> {
        let resp = self
            .http
            .post(LINEAR_GRAPHQL_URL)
            .header(reqwest::header::AUTHORIZATION, self.auth.header_value())
            .json(&body)
            .send()
            .await
            .map_err(|e| LinearError(format!("request failed: {e}")))?;
        let status = resp.status();
        let bytes = resp
            .bytes()
            .await
            .map_err(|e| LinearError(format!("read body failed: {e}")))?;
        if !status.is_success() {
            return Err(LinearError(format!(
                "HTTP {} from Linear: {}",
                status.as_u16(),
                error_detail(&bytes)
            )));
        }
        Ok(bytes.to_vec())
    }

    /// Download an uploaded file (an image pasted into an issue or comment).
    ///
    /// Linear's file storage takes the same credential as the GraphQL API — an
    /// unauthenticated request is refused with 401 — so this reuses the client's
    /// auth header. `max_bytes` guards against a pathological file; note it is
    /// *not* a model-facing size limit, since the agent's own tooling downscales
    /// (a plain screenshot is a few hundred KB, a photo can be tens of MB).
    ///
    /// Refuses any URL outside [`UPLOADS_HOST`]: the markdown it came from is
    /// user-authored, so fetching arbitrary hosts would be an SSRF hole.
    pub async fn download_upload(
        &self,
        url: &str,
        max_bytes: usize,
    ) -> Result<Upload, LinearError> {
        if !url.starts_with(UPLOADS_HOST) {
            return Err(LinearError(format!(
                "refusing to download `{url}`: not a {UPLOADS_HOST} URL"
            )));
        }
        let resp = self
            .http
            .get(url)
            .header(reqwest::header::AUTHORIZATION, self.auth.header_value())
            .send()
            .await
            .map_err(|e| LinearError(format!("upload request failed: {e}")))?;
        let status = resp.status();
        if !status.is_success() {
            return Err(LinearError(format!(
                "HTTP {} downloading upload",
                status.as_u16()
            )));
        }
        let content_type = resp
            .headers()
            .get(reqwest::header::CONTENT_TYPE)
            .and_then(|v| v.to_str().ok())
            .unwrap_or_default()
            .to_string();
        // Reject on the advertised length before reading, when it's given.
        if let Some(len) = resp.content_length() {
            if len as usize > max_bytes {
                return Err(LinearError(format!(
                    "upload is {len} bytes, over the {max_bytes} limit"
                )));
            }
        }
        let bytes = resp
            .bytes()
            .await
            .map_err(|e| LinearError(format!("read upload failed: {e}")))?;
        // …and again on what actually arrived, since the header is a claim.
        if bytes.len() > max_bytes {
            return Err(LinearError(format!(
                "upload is {} bytes, over the {max_bytes} limit",
                bytes.len()
            )));
        }
        Ok(Upload {
            bytes: bytes.to_vec(),
            content_type,
        })
    }

    /// The workspace this credential belongs to — also the cheapest probe that
    /// the credential authenticates at all.
    pub async fn organization(&self) -> Result<Workspace, LinearError> {
        let body = serde_json::json!({ "query": ORGANIZATION_QUERY });
        parse_organization(&self.post(body).await?)
    }

    /// The app's own user id in this workspace (`viewer` under an app-actor
    /// token). Recorded at connect time so delegated issues can be recognised.
    pub async fn app_user_id(&self) -> Result<String, LinearError> {
        let body = serde_json::json!({ "query": ME_QUERY });
        parse_app_user_id(&self.post(body).await?)
    }

    /// Where an issue sits — team, status and delegated agent.
    pub async fn issue_context(&self, issue_id: &str) -> Result<IssueContext, LinearError> {
        let body = serde_json::json!({
            "query": ISSUE_CONTEXT_QUERY,
            "variables": { "id": issue_id },
        });
        parse_issue_context(&self.post(body).await?)
    }

    /// Emit an activity into an agent session.
    ///
    /// Time-critical for the first one: Linear marks a session unresponsive if
    /// nothing arrives within 10 seconds of the `created` webhook, so the
    /// acknowledging [`AgentActivity::Thought`] is sent before any slower work.
    /// Open an agent session on `issue_id` for our own app user, returning its id.
    ///
    /// Delegation creates a session for us; a run the **poller** claimed has none,
    /// so it opens one and streams progress into it rather than posting detached
    /// comments. A fresh session per claim is deliberate: a re-claim after a
    /// failure is a new attempt, and reusing the previous session would mean
    /// posting into one that has already terminated (`complete`/`error`), which
    /// Linear does not document as accepted.
    /// Requires an app-actor (OAuth) credential: the mutation attributes the
    /// session to whoever is calling, so a personal API key would either be
    /// refused or open a session belonging to that human. Refusing up front beats
    /// a round-trip that fails, and the caller falls back to plain comments.
    pub async fn create_agent_session(&self, issue_id: &str) -> Result<String, LinearError> {
        session_auth_check(&self.auth)?;
        let body = serde_json::json!({
            "query": AGENT_SESSION_CREATE_MUTATION,
            "variables": { "input": { "issueId": issue_id } },
        });
        parse_created_session(&self.post(body).await?)
    }

    pub async fn create_agent_activity(
        &self,
        session_id: &str,
        activity: &AgentActivity,
    ) -> Result<(), LinearError> {
        let body = serde_json::json!({
            "query": AGENT_ACTIVITY_MUTATION,
            "variables": { "input": {
                "agentSessionId": session_id,
                "content": activity.content(),
            }},
        });
        expect_mutation_success(&self.post(body).await?, "agentActivityCreate")
    }

    /// List the workspace's teams + states + labels.
    pub async fn discover(&self) -> Result<Discovery, LinearError> {
        let body = serde_json::json!({ "query": DISCOVERY_QUERY });
        parse_discovery(&self.post(body).await?)
    }

    /// Issues in `team + state` that are **delegated to `delegate_id`** (the
    /// harness's own app user). Read-only — does not claim or modify anything.
    ///
    /// Both gates are deliberate: delegation says a human wants the harness on
    /// it, the status says the work is ready to start.
    pub async fn preview_issues(
        &self,
        team_id: &str,
        state_id: &str,
        delegate_id: &str,
    ) -> Result<Vec<Issue>, LinearError> {
        let body = serde_json::json!({
            "query": ISSUES_QUERY,
            "variables": {
                "teamId": team_id, "stateId": state_id, "delegateId": delegate_id,
            },
        });
        parse_issues(&self.post(body).await?)
    }
    /// List an issue's comments (read-only). `issue_id` is the Linear internal
    /// id (the `id` field of a previewed [`Issue`]), not the `COR-12` identifier.
    pub async fn list_comments(&self, issue_id: &str) -> Result<Vec<Comment>, LinearError> {
        let body = serde_json::json!({
            "query": COMMENTS_QUERY,
            "variables": { "id": issue_id },
        });
        parse_comments(&self.post(body).await?)
    }

    /// An epic's sub-issues, in board order.
    ///
    /// The epic orchestrator's only memory: which pieces exist, which column
    /// each is in, and which is next.
    pub async fn list_children(&self, issue_id: &str) -> Result<Vec<SubIssue>, LinearError> {
        let body = serde_json::json!({
            "query": CHILDREN_QUERY,
            "variables": { "id": issue_id },
        });
        parse_children(&self.post(body).await?)
    }

    /// Move an issue to a workflow state (write). `issue_id` is the Linear
    /// internal id (the `id` field from a previewed [`Issue`]), not the
    /// identifier (`COR-12`).
    pub async fn set_issue_state(&self, issue_id: &str, state_id: &str) -> Result<(), LinearError> {
        let body = serde_json::json!({
            "query": "mutation($id:String!,$s:String!){ issueUpdate(id:$id, input:{stateId:$s}){ success } }",
            "variables": { "id": issue_id, "s": state_id },
        });
        expect_mutation_success(&self.post(body).await?, "issueUpdate")
    }

    /// Give up an issue: clear its delegate (write).
    ///
    /// The poller selects on `delegate = the app user`, so this is what takes a
    /// finished issue out of consideration for good. It exists because a
    /// workflow that deliberately leaves an issue where it found it — the epic
    /// supervisor grading a piece that is already `Done` — has no column move to
    /// signal completion with, and the poller would otherwise re-pick it every
    /// tick, forever. Six identical reviews of one merged piece is how that was
    /// found.
    ///
    /// Delegation is how work arrives; releasing it is how the agent says it is
    /// finished. A person can always delegate again, which is the retry.
    pub async fn clear_delegate(&self, issue_id: &str) -> Result<(), LinearError> {
        let body = serde_json::json!({
            "query": "mutation($id:String!){ issueUpdate(id:$id, input:{delegateId:null}){ success } }",
            "variables": { "id": issue_id },
        });
        expect_mutation_success(&self.post(body).await?, "issueUpdate")
    }

    /// Add a comment to an issue (write).
    pub async fn add_comment(&self, issue_id: &str, body_md: &str) -> Result<(), LinearError> {
        let body = serde_json::json!({
            "query": "mutation($id:String!,$b:String!){ commentCreate(input:{issueId:$id, body:$b}){ success } }",
            "variables": { "id": issue_id, "b": body_md },
        });
        expect_mutation_success(&self.post(body).await?, "commentCreate")
    }

    /// Attach a linked resource (URL) to an issue (write). Shows up under the
    /// issue's "Links" like the auto-linked GitHub PR.
    pub async fn add_attachment(
        &self,
        issue_id: &str,
        url: &str,
        title: &str,
    ) -> Result<(), LinearError> {
        let body = serde_json::json!({
            "query": "mutation($id:String!,$u:String!,$t:String!){ attachmentCreate(input:{issueId:$id, url:$u, title:$t}){ success } }",
            "variables": { "id": issue_id, "u": url, "t": title },
        });
        expect_mutation_success(&self.post(body).await?, "attachmentCreate")
    }

    /// Add a single label to an issue (write) without disturbing its other
    /// labels. `label_id` is the Linear internal id (resolve a name via
    /// [`Self::discover`]).
    pub async fn add_label(&self, issue_id: &str, label_id: &str) -> Result<(), LinearError> {
        let body = serde_json::json!({
            "query": "mutation($id:String!,$l:String!){ issueAddLabel(id:$id, labelId:$l){ success } }",
            "variables": { "id": issue_id, "l": label_id },
        });
        expect_mutation_success(&self.post(body).await?, "issueAddLabel")
    }

    /// Remove a single label from an issue (write). `label_id` is the Linear
    /// internal id (resolve a name via [`Self::discover`]). Used by the Rerun
    /// button to clear a binding's failed-label when re-arming an issue.
    pub async fn remove_label(&self, issue_id: &str, label_id: &str) -> Result<(), LinearError> {
        let body = serde_json::json!({
            "query": "mutation($id:String!,$l:String!){ issueRemoveLabel(id:$id, labelId:$l){ success } }",
            "variables": { "id": issue_id, "l": label_id },
        });
        expect_mutation_success(&self.post(body).await?, "issueRemoveLabel")
    }

    /// Create an issue in `team_id` (write), with an optional initial workflow
    /// state and labels. `state_id` / `label_ids` are Linear internal ids
    /// (resolve label names via [`Self::discover`]). Returns the new issue.
    /// File an issue. `parent_id` makes it a sub-issue of that epic.
    ///
    /// `parentId` is sent as a nullable variable rather than by building two
    /// query strings: Linear treats an explicit `null` as "no parent", which is
    /// what every caller outside the epic orchestrator wants.
    pub async fn create_issue(
        &self,
        team_id: &str,
        title: &str,
        description: &str,
        state_id: Option<&str>,
        label_ids: &[String],
        parent_id: Option<&str>,
    ) -> Result<CreatedIssue, LinearError> {
        let body = serde_json::json!({
            "query": "mutation($t:String!,$ti:String!,$d:String!,$s:String,$l:[String!],$p:String){ \
                issueCreate(input:{teamId:$t, title:$ti, description:$d, stateId:$s, labelIds:$l, parentId:$p}){ \
                success issue { id identifier url } } }",
            "variables": {
                "t": team_id, "ti": title, "d": description,
                "s": state_id, "l": label_ids, "p": parent_id,
            },
        });
        parse_created_issue(&self.post(body).await?)
    }
}

/// Parse an `issueCreate` mutation response into a [`CreatedIssue`].
fn parse_created_issue(json: &[u8]) -> Result<CreatedIssue, LinearError> {
    #[derive(Deserialize)]
    struct Data {
        #[serde(rename = "issueCreate")]
        issue_create: Payload,
    }
    #[derive(Deserialize)]
    struct Payload {
        success: bool,
        issue: Option<Node>,
    }
    #[derive(Deserialize)]
    struct Node {
        id: String,
        identifier: String,
        url: String,
    }
    let data: Data = gql_data(json)?;
    if !data.issue_create.success {
        return Err(LinearError("issueCreate did not report success".into()));
    }
    data.issue_create
        .issue
        .map(|n| CreatedIssue {
            id: n.id,
            identifier: n.identifier,
            url: n.url,
        })
        .ok_or_else(|| LinearError("issueCreate returned no issue".into()))
}

/// Whether a credential may open an agent session at all.
///
/// `agentSessionCreateOnIssue` attributes the session to the caller, so a
/// personal API key would open one belonging to the human who minted it. Refusing
/// here beats spending a round-trip to be told no, and the caller falls back to
/// plain comments. Split out as a plain function so the rule is unit-testable
/// without HTTP, like the parsers in this module.
fn session_auth_check(auth: &LinearAuth) -> Result<(), LinearError> {
    if auth.is_app_actor() {
        return Ok(());
    }
    Err(LinearError(
        "agent sessions need the workspace connected as an app (OAuth); \
         a personal API key cannot open one"
            .into(),
    ))
}

/// Parse an `agentSessionCreateOnIssue` response into the new session's id.
pub fn parse_created_session(json: &[u8]) -> Result<String, LinearError> {
    #[derive(Deserialize)]
    struct Data {
        #[serde(rename = "agentSessionCreateOnIssue")]
        create: Payload,
    }
    #[derive(Deserialize)]
    struct Payload {
        success: bool,
        #[serde(default, rename = "agentSession")]
        agent_session: Option<Node>,
    }
    #[derive(Deserialize)]
    struct Node {
        id: String,
    }
    // `agentSession` is non-null in the schema, but treat it as optional so a
    // shape change degrades to an error rather than a deserialize panic.
    let data: Data = gql_data(json)?;
    if !data.create.success {
        return Err(LinearError(
            "agentSessionCreate did not report success".into(),
        ));
    }
    data.create
        .agent_session
        .map(|n| n.id)
        .ok_or_else(|| LinearError("agentSessionCreate returned no session".into()))
}

/// Check a mutation response reported `{ <field>: { success: true } }`.
fn expect_mutation_success(json: &[u8], field: &str) -> Result<(), LinearError> {
    let data: serde_json::Value = gql_data(json)?;
    let ok = data
        .get(field)
        .and_then(|f| f.get("success"))
        .and_then(|v| v.as_bool())
        .unwrap_or(false);
    if ok {
        Ok(())
    } else {
        Err(LinearError(format!("{field} did not report success")))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn oauth_tokens_are_bearer_personal_keys_are_verbatim() {
        // The whole point of the OAuth switch: an app-actor token must go out as
        // `Bearer …`, while a legacy personal key is sent raw (Linear rejects a
        // `Bearer`-prefixed personal key).
        let oauth = LinearAuth::OauthToken("lin_oauth_abc".into());
        assert_eq!(oauth.header_value(), "Bearer lin_oauth_abc");
        assert!(oauth.is_app_actor());

        let personal = LinearAuth::PersonalKey("lin_api_xyz".into());
        assert_eq!(personal.header_value(), "lin_api_xyz");
        assert!(!personal.is_app_actor());

        // `new` stays the legacy personal-key constructor.
        assert!(!LinearClient::new("lin_api_xyz").auth.is_app_actor());
        assert!(LinearClient::with_auth(LinearAuth::OauthToken("t".into()))
            .auth
            .is_app_actor());
    }

    #[test]
    fn parse_agent_session_event_reads_a_delegation() {
        let json = br#"{
            "type":"AgentSessionEvent","action":"created",
            "promptContext":"Formatted context for the session",
            "guidance":"Always open a PR",
            "agentSession":{"id":"sess-1","issue":{
                "id":"iss-1","identifier":"COR-12","title":"Fix the thing",
                "description":"The thing is broken."}}}"#;
        let ev = parse_agent_session_event(json).unwrap().unwrap();
        assert_eq!(ev.action, AgentSessionAction::Created);
        assert_eq!(ev.session_id, "sess-1");
        assert_eq!(ev.issue_id.as_deref(), Some("iss-1"));
        assert_eq!(ev.issue_identifier.as_deref(), Some("COR-12"));
        assert_eq!(ev.issue_title.as_deref(), Some("Fix the thing"));
        assert_eq!(ev.guidance.as_deref(), Some("Always open a PR"));
        // The issue's own description is the preferred task text.
        assert_eq!(ev.task_text(), Some("The thing is broken."));
    }

    #[test]
    fn parse_agent_session_event_reads_a_follow_up_prompt() {
        let json = br#"{
            "type":"AgentSessionEvent","action":"prompted",
            "agentSession":{"id":"sess-2"},
            "agentActivity":{"body":"Also update the docs"}}"#;
        let ev = parse_agent_session_event(json).unwrap().unwrap();
        assert_eq!(ev.action, AgentSessionAction::Prompted);
        assert_eq!(ev.session_id, "sess-2");
        assert_eq!(ev.prompt_body.as_deref(), Some("Also update the docs"));
        assert_eq!(ev.issue_id, None);
    }

    #[test]
    fn parse_agent_session_event_tolerates_sparse_and_unknown_payloads() {
        // A mention in a thread: comment, no issue description. Falls back
        // through prompt_context to the comment body for the task text.
        let json = br#"{
            "type":"AgentSessionEvent","action":"created",
            "agentSession":{"id":"s","comment":{"body":"@harness please look"},
                            "issue":{"id":"i"}},
            "somethingLinearAddedLater":{"nested":true}}"#;
        let ev = parse_agent_session_event(json).unwrap().unwrap();
        assert_eq!(ev.task_text(), Some("@harness please look"));
        assert_eq!(ev.issue_id.as_deref(), Some("i"));
        assert_eq!(ev.issue_description, None);

        // An empty description must not win over the comment body.
        let json = br#"{"type":"AgentSessionEvent","action":"created",
            "agentSession":{"id":"s","comment":{"body":"use this"},
                            "issue":{"description":"   "}}}"#;
        assert_eq!(
            parse_agent_session_event(json)
                .unwrap()
                .unwrap()
                .task_text(),
            Some("use this")
        );

        // An unknown action parses (callers ignore it) rather than failing.
        let json = br#"{"type":"AgentSessionEvent","action":"resumed",
            "agentSession":{"id":"s"}}"#;
        assert_eq!(
            parse_agent_session_event(json).unwrap().unwrap().action,
            AgentSessionAction::Other("resumed".into())
        );
    }

    #[test]
    fn parse_agent_session_event_ignores_other_webhook_types() {
        // Linear delivers Issue/Comment/OAuthApp events to the same endpoint.
        let json = br#"{"type":"Issue","action":"update","data":{"id":"x"}}"#;
        assert!(parse_agent_session_event(json).unwrap().is_none());
        // AppUserNotification is the older agent webhook — also not ours.
        let json = br#"{"type":"AppUserNotification","appUserId":"u"}"#;
        assert!(parse_agent_session_event(json).unwrap().is_none());
    }

    #[test]
    fn parse_agent_session_event_rejects_unusable_payloads() {
        // No session id → nothing to acknowledge into.
        let json = br#"{"type":"AgentSessionEvent","action":"created","agentSession":{}}"#;
        assert!(parse_agent_session_event(json).is_err());
        // Missing action.
        let json = br#"{"type":"AgentSessionEvent","agentSession":{"id":"s"}}"#;
        assert!(parse_agent_session_event(json).is_err());
        assert!(parse_agent_session_event(b"not json").is_err());
    }

    #[test]
    fn agent_activity_content_matches_linear_shapes() {
        assert_eq!(
            AgentActivity::Thought {
                body: "looking".into()
            }
            .content(),
            serde_json::json!({ "type": "thought", "body": "looking" })
        );
        assert_eq!(
            AgentActivity::Response {
                body: "opened PR".into()
            }
            .content(),
            serde_json::json!({ "type": "response", "body": "opened PR" })
        );
        assert_eq!(
            AgentActivity::Error {
                body: "run failed".into()
            }
            .content(),
            serde_json::json!({ "type": "error", "body": "run failed" })
        );
        // `result` is omitted while a step is still in flight, present after.
        assert_eq!(
            AgentActivity::Action {
                action: "run".into(),
                parameter: "idea-to-pr".into(),
                result: None,
            }
            .content(),
            serde_json::json!({ "type": "action", "action": "run", "parameter": "idea-to-pr" })
        );
        assert_eq!(
            AgentActivity::Action {
                action: "run".into(),
                parameter: "idea-to-pr".into(),
                result: Some("done".into()),
            }
            .content(),
            serde_json::json!({
                "type": "action", "action": "run",
                "parameter": "idea-to-pr", "result": "done"
            })
        );
    }

    #[test]
    fn parse_issue_context_reads_team_state_and_delegate() {
        let json = br#"{"data":{"issue":{
            "team":{"id":"team-1"},
            "state":{"id":"state-1","name":"To Do"},
            "delegate":{"id":"app-user-1"}}}}"#;
        let ctx = parse_issue_context(json).unwrap();
        assert_eq!(ctx.team_id.as_deref(), Some("team-1"));
        assert_eq!(ctx.state_id.as_deref(), Some("state-1"));
        assert_eq!(ctx.state_name.as_deref(), Some("To Do"));
        assert_eq!(ctx.delegate_id.as_deref(), Some("app-user-1"));
    }

    #[test]
    fn a_sub_issue_names_the_epic_above_it() {
        // The supervisor fires on a piece and has to reach its epic; every
        // other read in this client goes downward.
        let ctx = parse_issue_context(
            br#"{"data":{"issue":{
                 "team":{"id":"t1"},"state":{"id":"s1","name":"Built"},
                 "parent":{"id":"epic-9"},
                 "labels":{"nodes":[{"name":"corrective"},{"name":"AI Eligible"}]}}}}"#,
        )
        .unwrap();
        assert_eq!(ctx.parent_id.as_deref(), Some("epic-9"));
        assert_eq!(ctx.labels, ["corrective", "AI Eligible"]);
        assert_eq!(ctx.state_name.as_deref(), Some("Built"));
    }

    #[test]
    fn a_top_level_issue_has_no_parent_and_that_is_not_an_error() {
        // An epic itself, or any issue filed on its own. The supervisor uses
        // the absence to tell "this is not a piece of anything" from a read
        // that failed.
        let ctx = parse_issue_context(
            br#"{"data":{"issue":{"team":{"id":"t1"},"state":{"id":"s1","name":"Todo"}}}}"#,
        )
        .unwrap();
        assert!(ctx.parent_id.is_none());
        assert!(ctx.labels.is_empty());
    }

    #[test]
    fn parse_issue_context_degrades_on_missing_pieces() {
        // Not delegated to anyone — `delegate` is null until an agent is assigned.
        let json = br#"{"data":{"issue":{"team":{"id":"t"},
            "state":{"id":"s","name":"Backlog"},"delegate":null}}}"#;
        let ctx = parse_issue_context(json).unwrap();
        assert_eq!(ctx.delegate_id, None);
        assert_eq!(ctx.state_name.as_deref(), Some("Backlog"));
        // An unresolvable id gives an empty context, not an error.
        assert_eq!(
            parse_issue_context(br#"{"data":{"issue":null}}"#).unwrap(),
            IssueContext::default()
        );
        assert!(parse_issue_context(br#"{"errors":[{"message":"no"}]}"#).is_err());
    }

    #[test]
    fn preview_query_gates_on_delegation_and_status() {
        // Regression guard for the two gates. `delegate` replaced the old
        // eligibility label; dropping it would make the poller claim anything in
        // the column, and dropping `state` would ignore the configured trigger.
        assert!(
            ISSUES_QUERY.contains("delegate: { id: { eq: $delegateId } }"),
            "the poller must only claim issues delegated to the app"
        );
        assert!(
            ISSUES_QUERY.contains("state: { id: { eq: $stateId } }"),
            "the poller must only claim issues in the binding's source status"
        );
        assert!(ISSUES_QUERY.contains("$delegateId: ID!"));
    }

    #[test]
    fn parse_app_user_id_reads_viewer() {
        let json = br#"{"data":{"viewer":{"id":"app-user-1"}}}"#;
        assert_eq!(parse_app_user_id(json).unwrap(), "app-user-1");
        assert!(parse_app_user_id(br#"{"errors":[{"message":"nope"}]}"#).is_err());
    }

    #[test]
    fn agent_activity_mutation_success_parsing() {
        assert!(expect_mutation_success(
            br#"{"data":{"agentActivityCreate":{"success":true}}}"#,
            "agentActivityCreate"
        )
        .is_ok());
        assert!(expect_mutation_success(
            br#"{"data":{"agentActivityCreate":{"success":false}}}"#,
            "agentActivityCreate"
        )
        .is_err());
    }

    #[test]
    fn extract_upload_urls_finds_images_links_and_bare_urls() {
        let md = "Repro below.\n\n\
            ![shot](https://uploads.linear.app/a/b/c)\n\
            Also [the log](https://uploads.linear.app/d/e/f) and bare \
            https://uploads.linear.app/g/h/i here.\n";
        assert_eq!(
            extract_upload_urls(md),
            vec![
                "https://uploads.linear.app/a/b/c",
                "https://uploads.linear.app/d/e/f",
                "https://uploads.linear.app/g/h/i",
            ]
        );
    }

    #[test]
    fn extract_upload_urls_ignores_other_hosts_and_dedupes() {
        // Only the uploads host is ever collected — the markdown is user-authored,
        // so treating any URL in it as fetchable would be an SSRF hole.
        let md = "![a](https://evil.example.com/x.png) \
                  ![b](https://linear.app/acme/issue/COR-1) \
                  ![c](https://uploads.linear.app/keep/me) \
                  ![c again](https://uploads.linear.app/keep/me)";
        assert_eq!(
            extract_upload_urls(md),
            vec!["https://uploads.linear.app/keep/me"]
        );
        // A bare host with no path isn't a file.
        assert!(extract_upload_urls("https://uploads.linear.app/").is_empty());
        assert!(extract_upload_urls("no links here").is_empty());
    }

    #[test]
    fn extract_upload_urls_trims_trailing_punctuation() {
        // Upload paths are UUID segments, so nothing legitimate ends in these.
        for (md, want) in [
            (
                "see https://uploads.linear.app/a/b.",
                "https://uploads.linear.app/a/b",
            ),
            (
                "see https://uploads.linear.app/a/b, then",
                "https://uploads.linear.app/a/b",
            ),
            (
                "<https://uploads.linear.app/a/b>",
                "https://uploads.linear.app/a/b",
            ),
        ] {
            assert_eq!(extract_upload_urls(md), vec![want.to_string()], "for {md}");
        }
    }

    #[test]
    fn upload_extension_allowlists_model_safe_image_types() {
        let up = |ct: &str| Upload {
            bytes: vec![],
            content_type: ct.to_string(),
        };
        assert_eq!(up("image/png").extension(), Some("png"));
        assert_eq!(up("image/jpeg").extension(), Some("jpg"));
        assert_eq!(up("image/gif").extension(), Some("gif"));
        assert_eq!(up("image/webp").extension(), Some("webp"));
        // Charset parameters are tolerated.
        assert_eq!(up("image/png; charset=binary").extension(), Some("png"));
        // SVG can carry script; PDFs and everything else are not images we pass on.
        assert_eq!(up("image/svg+xml").extension(), None);
        assert_eq!(up("application/pdf").extension(), None);
        assert_eq!(up("").extension(), None);
    }

    #[test]
    fn parse_created_session_reads_the_id() {
        let json = br#"{"data":{"agentSessionCreateOnIssue":{"success":true,
            "agentSession":{"id":"sess-abc"}}}}"#;
        assert_eq!(parse_created_session(json).unwrap(), "sess-abc");
    }

    #[test]
    fn parse_created_session_rejects_failure_and_missing_session() {
        assert!(parse_created_session(
            br#"{"data":{"agentSessionCreateOnIssue":{"success":false,"agentSession":null}}}"#
        )
        .is_err());
        assert!(parse_created_session(
            br#"{"data":{"agentSessionCreateOnIssue":{"success":true,"agentSession":null}}}"#
        )
        .is_err());
        // A GraphQL error — e.g. a credential without the agent scopes — surfaces
        // rather than being read as a session id.
        let err =
            parse_created_session(br#"{"errors":[{"message":"not authorized"}]}"#).unwrap_err();
        assert!(err.0.contains("not authorized"));
    }

    /// `agentSessionCreate` is `[Internal]` in Linear's schema and answers a
    /// third-party app with `Access denied`, which cost a deploy to discover.
    /// `agentSessionCreateOnIssue` is the public mutation; keep it that way.
    #[test]
    fn session_create_uses_the_public_mutation_not_the_internal_one() {
        assert!(AGENT_SESSION_CREATE_MUTATION.contains("agentSessionCreateOnIssue(input:"));
        assert!(AGENT_SESSION_CREATE_MUTATION.contains("$input: AgentSessionCreateOnIssue!"));
        assert!(AGENT_SESSION_CREATE_MUTATION.contains("agentSession { id }"));
        // The internal variant's input type must not creep back in — its
        // `appUserId` is the privilege Linear withholds.
        assert!(!AGENT_SESSION_CREATE_MUTATION.contains("AgentSessionCreateInput"));
        assert!(!AGENT_SESSION_CREATE_MUTATION.contains("appUserId"));
    }

    /// A personal key would open a session belonging to the human who minted it,
    /// so the client refuses before spending a round-trip.
    #[test]
    fn only_an_app_actor_can_open_a_session() {
        assert!(session_auth_check(&LinearAuth::OauthToken("tok".into())).is_ok());
        let err = session_auth_check(&LinearAuth::PersonalKey("lin_api_x".into())).unwrap_err();
        assert!(err.0.contains("app (OAuth)"), "{}", err.0);
    }

    #[test]
    fn parse_organization_maps_workspace() {
        let json = br#"{"data":{"organization":{"id":"org1","name":"Acme","urlKey":"acme"}}}"#;
        let w = parse_organization(json).unwrap();
        assert_eq!(w.id, "org1");
        assert_eq!(w.name, "Acme");
        assert_eq!(w.url_key, "acme");
    }

    #[test]
    fn parse_organization_surfaces_graphql_errors() {
        let json = br#"{"errors":[{"message":"Authentication required, not authenticated"}]}"#;
        let err = parse_organization(json).unwrap_err();
        assert!(err.0.contains("not authenticated"));
    }

    #[test]
    fn mutation_success_parsing() {
        assert!(expect_mutation_success(
            br#"{"data":{"issueUpdate":{"success":true}}}"#,
            "issueUpdate"
        )
        .is_ok());
        assert!(expect_mutation_success(
            br#"{"data":{"attachmentCreate":{"success":true}}}"#,
            "attachmentCreate"
        )
        .is_ok());
        assert!(expect_mutation_success(
            br#"{"data":{"issueUpdate":{"success":false}}}"#,
            "issueUpdate"
        )
        .is_err());
        assert!(expect_mutation_success(
            br#"{"data":{"attachmentCreate":{"success":false}}}"#,
            "attachmentCreate"
        )
        .is_err());
        // A GraphQL error surfaces through gql_data.
        assert!(expect_mutation_success(
            br#"{"errors":[{"message":"not authorized"}]}"#,
            "issueUpdate"
        )
        .is_err());
    }

    #[test]
    fn create_issue_parses_payload() {
        let json = br#"{"data":{"issueCreate":{"success":true,"issue":{
            "id":"i1","identifier":"COR-42",
            "url":"https://linear.app/acme/issue/COR-42"}}}}"#;
        let c = parse_created_issue(json).unwrap();
        assert_eq!(c.id, "i1");
        assert_eq!(c.identifier, "COR-42");
        assert_eq!(c.url, "https://linear.app/acme/issue/COR-42");
    }

    #[test]
    fn create_issue_rejects_unsuccessful_or_errored() {
        // success:false → error.
        assert!(
            parse_created_issue(br#"{"data":{"issueCreate":{"success":false,"issue":null}}}"#)
                .is_err()
        );
        // success but no issue → error.
        assert!(
            parse_created_issue(br#"{"data":{"issueCreate":{"success":true,"issue":null}}}"#)
                .is_err()
        );
        // GraphQL error surfaces through gql_data.
        assert!(parse_created_issue(br#"{"errors":[{"message":"team not found"}]}"#).is_err());
    }

    #[test]
    fn parse_discovery_maps_teams_states_labels() {
        let json = br#"{"data":{"teams":{"nodes":[
            {"id":"t1","name":"Core","key":"COR",
             "states":{"nodes":[
                {"id":"s1","name":"To Do","type":"unstarted","position":1.0},
                {"id":"s2","name":"In Progress","type":"started","position":2.0}]},
             "labels":{"nodes":[{"id":"l1","name":"bug"}]}}
        ]}}}"#;
        let d = parse_discovery(json).unwrap();
        assert_eq!(d.teams.len(), 1);
        let t = &d.teams[0];
        assert_eq!(t.key, "COR");
        assert_eq!(t.states.len(), 2);
        assert_eq!(t.states[1].kind, "started");
        assert_eq!(t.labels[0].name, "bug");
    }

    #[test]
    fn parse_discovery_surfaces_graphql_errors() {
        let json = br#"{"errors":[{"message":"authentication required"}]}"#;
        let err = parse_discovery(json).unwrap_err();
        assert!(err.0.contains("authentication required"));
    }

    #[test]
    fn discovery_query_stays_under_linear_complexity_cap() {
        // Regression guard: Linear caps complexity at 10k (charged on requested
        // page sizes). `teams(first: 250)` blew it at ~32.8k in production.
        assert!(
            DISCOVERY_QUERY.contains("teams(first: 50)"),
            "cap teams at 50 to stay under Linear's 10k complexity limit"
        );
        assert!(
            !DISCOVERY_QUERY.contains("first: 250"),
            "teams(first: 250) exceeds Linear's complexity cap"
        );
    }

    #[test]
    fn error_detail_extracts_graphql_messages() {
        // Linear returns this shape even on HTTP 400 (e.g. a bad API key).
        let body = br#"{"errors":[{"message":"Authentication required, not authenticated"}]}"#;
        assert_eq!(
            error_detail(body),
            "Authentication required, not authenticated"
        );
    }

    #[test]
    fn error_detail_falls_back_to_truncated_raw_body() {
        assert_eq!(error_detail(b""), "(empty response body)");
        assert_eq!(error_detail(b"  Bad Request  "), "Bad Request");
        let long = vec![b'x'; 500];
        assert_eq!(error_detail(&long).len(), 300);
    }

    #[test]
    fn parse_issues_returns_every_issue_with_its_labels() {
        // No eligibility gate any more — every issue in the column comes back,
        // but labels are still carried so the failed-label lifecycle can filter.
        let json = br#"{"data":{"issues":{"nodes":[
            {"id":"i1","identifier":"COR-1","title":"One","url":"u1",
             "labels":{"nodes":[{"name":"ai-failed"}]}},
            {"id":"i2","identifier":"COR-2","title":"Two","url":"u2",
             "labels":{"nodes":[{"name":"bug"}]}}
        ]}}}"#;
        let all = parse_issues(json).unwrap();
        assert_eq!(all.len(), 2);
        assert_eq!(all[0].labels, vec!["ai-failed".to_string()]);
    }

    #[test]
    fn parse_issues_handles_missing_labels() {
        let json = br#"{"data":{"issues":{"nodes":[
            {"id":"i1","identifier":"COR-3","title":"No labels field","url":"u3"}
        ]}}}"#;
        let issues = parse_issues(json).unwrap();
        assert_eq!(issues.len(), 1);
        assert!(issues[0].labels.is_empty());
    }
    fn children(raw: &str) -> Vec<SubIssue> {
        parse_children(
            format!(r#"{{"data":{{"issue":{{"children":{{"nodes":{raw}}}}}}}}}"#).as_bytes(),
        )
        .expect("parse")
    }

    #[test]
    fn children_come_back_in_board_order_not_response_order() {
        // The orchestrator builds them one at a time, so "which is next" is the
        // whole question — and Linear returns children in creation order.
        let list = children(
            r#"[
              {"id":"c","identifier":"AIH-3","title":"Third","url":"u3","sortOrder":3.5,
               "state":{"id":"s1","name":"Queued"},"labels":{"nodes":[]}},
              {"id":"a","identifier":"AIH-1","title":"First","url":"u1","sortOrder":1.0,
               "state":{"id":"s2","name":"Done"},"labels":{"nodes":[]}},
              {"id":"b","identifier":"AIH-2","title":"Second","url":"u2","sortOrder":2.25,
               "state":{"id":"s1","name":"Queued"},"labels":{"nodes":[]}}
            ]"#,
        );
        assert_eq!(
            list.iter()
                .map(|c| c.identifier.as_str())
                .collect::<Vec<_>>(),
            ["AIH-1", "AIH-2", "AIH-3"]
        );
        // The next piece to build is the first one not yet finished.
        let next = list.iter().find(|c| c.state == "Queued").unwrap();
        assert_eq!(next.identifier, "AIH-2");
    }

    #[test]
    fn a_child_carries_when_it_finished() {
        // The order pieces were *built* in, which `sortOrder` stops describing:
        // Linear reassigns it as cards move between columns, so by the time an
        // epic finishes it says where the cards ended up. The first real epic's
        // pull request listed its three pieces in exactly reverse order.
        let list = children(
            r#"[
              {"id":"a","identifier":"A-1","title":"first","url":"u","sortOrder":3.0,
               "completedAt":"2026-08-30T13:12:42.471Z",
               "state":{"id":"s1","name":"Done","type":"completed"},"labels":{"nodes":[]}},
              {"id":"b","identifier":"A-2","title":"still going","url":"u","sortOrder":1.0,
               "state":{"id":"s2","name":"In Progress","type":"started"},"labels":{"nodes":[]}}
            ]"#,
        );
        let by_id = |id: &str| list.iter().find(|c| c.identifier == id).unwrap();
        assert_eq!(
            by_id("A-1").completed_at.as_deref(),
            Some("2026-08-30T13:12:42.471Z")
        );
        // Absent, not an error: a piece that has not finished has no time.
        assert!(by_id("A-2").completed_at.is_none());
    }

    #[test]
    fn a_child_says_whether_it_has_begun() {
        // "Which piece starts next" has to be answerable without knowing what
        // this workspace calls its columns. Linear's own category is what makes
        // that possible: a name is whatever somebody typed.
        let list = children(
            r#"[
              {"id":"a","identifier":"A-1","title":"done","url":"u","sortOrder":1.0,
               "state":{"id":"s1","name":"Done","type":"completed"},"labels":{"nodes":[]}},
              {"id":"b","identifier":"A-2","title":"building","url":"u","sortOrder":2.0,
               "state":{"id":"s2","name":"In Progress","type":"started"},"labels":{"nodes":[]}},
              {"id":"c","identifier":"A-3","title":"waiting","url":"u","sortOrder":3.0,
               "state":{"id":"s3","name":"Backlog","type":"backlog"},"labels":{"nodes":[]}}
            ]"#,
        );
        assert_eq!(
            list.iter()
                .map(|c| c.state_type.as_str())
                .collect::<Vec<_>>(),
            ["completed", "started", "backlog"]
        );
        // The next piece to begin is the first that has not.
        let next = list
            .iter()
            .find(|c| matches!(c.state_type.as_str(), "backlog" | "unstarted"));
        assert_eq!(next.unwrap().identifier, "A-3");
    }

    #[test]
    fn a_child_carries_what_the_supervisor_grades_against() {
        let list = children(
            r#"[{"id":"x","identifier":"AIH-9","title":"Add the thing",
                 "url":"https://l/x","sortOrder":1.0,
                 "description":"Acceptance:\n- it works",
                 "state":{"id":"st","name":"Built"},
                 "labels":{"nodes":[{"name":"corrective"},{"name":"AI Eligible"}]}}]"#,
        );
        let c = &list[0];
        assert_eq!(c.id, "x");
        assert_eq!(c.state_id, "st");
        assert_eq!(c.state, "Built");
        assert_eq!(c.body.as_deref(), Some("Acceptance:\n- it works"));
        assert_eq!(c.labels, ["corrective", "AI Eligible"]);
    }

    #[test]
    fn an_epic_with_no_children_is_empty_not_an_error() {
        assert!(children("[]").is_empty());
        // And an id that resolves to nothing at all.
        assert!(parse_children(br#"{"data":{"issue":null}}"#)
            .unwrap()
            .is_empty());
    }

    #[test]
    fn one_malformed_child_does_not_fail_the_epic_read() {
        // A missing state or label set must not cost the caller every other
        // sub-issue: the orchestrator would lose its entire memory of the epic.
        let list = children(
            r#"[{"id":"y","identifier":"AIH-4","title":"Odd one","url":"u","sortOrder":1.0}]"#,
        );
        assert_eq!(list.len(), 1);
        assert_eq!(list[0].state, "unknown");
        assert!(list[0].state_id.is_empty());
        assert!(list[0].labels.is_empty());
        assert!(list[0].body.is_none());
    }

    #[test]
    fn an_empty_description_reads_as_absent() {
        // "no acceptance criteria" and "whitespace" must not be different cases
        // to the supervisor.
        let list = children(
            r#"[{"id":"z","identifier":"AIH-5","title":"T","url":"u","sortOrder":1.0,
                 "description":"   ","state":{"id":"s","name":"Queued"},
                 "labels":{"nodes":[]}}]"#,
        );
        assert!(list[0].body.is_none());
    }

    #[test]
    fn parse_comments_maps_body_author_and_time() {
        let json = br#"{"data":{"issue":{"comments":{"nodes":[
            {"body":"First pass looks good","createdAt":"2026-06-01T10:00:00Z","user":{"name":"Alice"}},
            {"body":"Please fix the edge case","createdAt":"2026-06-02T14:30:00Z","user":{"name":"Bob"}}
        ]}}}}"#;
        let comments = parse_comments(json).unwrap();
        assert_eq!(comments.len(), 2);
        assert_eq!(comments[0].author, "Alice");
        assert_eq!(comments[0].body, "First pass looks good");
        assert_eq!(comments[0].created_at, "2026-06-01T10:00:00Z");
    }
    #[test]
    fn parse_comments_handles_missing_user() {
        let json = br#"{"data":{"issue":{"comments":{"nodes":[
            {"body":"Anonymous note","createdAt":"2026-06-03T09:00:00Z","user":null}
        ]}}}}"#;
        let comments = parse_comments(json).unwrap();
        assert_eq!(comments.len(), 1);
        assert_eq!(comments[0].author, "unknown");
    }
    #[test]
    fn parse_comments_handles_null_issue() {
        let json = br#"{"data":{"issue":null}}"#;
        let comments = parse_comments(json).unwrap();
        assert!(comments.is_empty());
    }
    #[test]
    fn parse_comments_handles_empty_nodes() {
        let json = br#"{"data":{"issue":{"comments":{"nodes":[]}}}}"#;
        let comments = parse_comments(json).unwrap();
        assert!(comments.is_empty());
    }
    #[test]
    fn parse_comments_surfaces_graphql_errors() {
        let json = br#"{"errors":[{"message":"authentication required"}]}"#;
        let err = parse_comments(json).unwrap_err();
        assert!(err.0.contains("authentication required"));
    }
}
