//! Linear GraphQL client — read-only discovery + issue preview (Slice 1).
//!
//! Auth uses a Linear **personal API key** in the `Authorization` header (raw
//! key, not `Bearer`). Parsing is split from the HTTP call so the response
//! shaping is unit-tested with fixtures (no mock server), matching the
//! `intake/github_issues` pattern.

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

const ISSUES_QUERY: &str = r#"
query Preview($teamId: ID!, $stateId: ID!) {
  issues(first: 50, filter: { team: { id: { eq: $teamId } }, state: { id: { eq: $stateId } } }) {
    nodes { id identifier title url description labels { nodes { name } } }
  }
}"#;
// A single issue's comments connection is flat (no nested connections), so
// even first: 50 stays well under Linear's 10k complexity cap. Ordered
// oldest→newest so the reviewer's narrative reads top-to-bottom.
const COMMENTS_QUERY: &str = r#"
query Comments($id: String!) {
  issue(id: $id) {
    comments(first: 50) {
      nodes { body createdAt user { name } }
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

/// Parse an issues response, optionally keeping only issues carrying `label`.
pub fn parse_issues(json: &[u8], label: Option<&str>) -> Result<Vec<Issue>, LinearError> {
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
        .filter(|i| match label {
            Some(l) => i.labels.iter().any(|x| x == l),
            None => true,
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

// ── HTTP client ──────────────────────────────────────────────────────────────

/// A read-only Linear GraphQL client.
pub struct LinearClient {
    http: reqwest::Client,
    api_key: String,
}

impl LinearClient {
    pub fn new(api_key: impl Into<String>) -> Self {
        Self {
            http: reqwest::Client::new(),
            api_key: api_key.into(),
        }
    }

    async fn post(&self, body: serde_json::Value) -> Result<Vec<u8>, LinearError> {
        let resp = self
            .http
            .post(LINEAR_GRAPHQL_URL)
            // Personal API keys go in `Authorization` verbatim (not `Bearer`).
            .header(reqwest::header::AUTHORIZATION, &self.api_key)
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

    /// List the workspace's teams + states + labels.
    pub async fn discover(&self) -> Result<Discovery, LinearError> {
        let body = serde_json::json!({ "query": DISCOVERY_QUERY });
        parse_discovery(&self.post(body).await?)
    }

    /// Preview the issues a `team + state (+ optional label)` filter matches.
    /// Read-only — does not claim or modify anything.
    pub async fn preview_issues(
        &self,
        team_id: &str,
        state_id: &str,
        label: Option<&str>,
    ) -> Result<Vec<Issue>, LinearError> {
        let body = serde_json::json!({
            "query": ISSUES_QUERY,
            "variables": { "teamId": team_id, "stateId": state_id },
        });
        parse_issues(&self.post(body).await?, label)
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
    fn parse_discovery_maps_teams_states_labels() {
        let json = br#"{"data":{"teams":{"nodes":[
            {"id":"t1","name":"Core","key":"COR",
             "states":{"nodes":[
                {"id":"s1","name":"To Do","type":"unstarted","position":1.0},
                {"id":"s2","name":"In Progress","type":"started","position":2.0}]},
             "labels":{"nodes":[{"id":"l1","name":"AI Eligible"}]}}
        ]}}}"#;
        let d = parse_discovery(json).unwrap();
        assert_eq!(d.teams.len(), 1);
        let t = &d.teams[0];
        assert_eq!(t.key, "COR");
        assert_eq!(t.states.len(), 2);
        assert_eq!(t.states[1].kind, "started");
        assert_eq!(t.labels[0].name, "AI Eligible");
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
    fn parse_issues_filters_by_label() {
        let json = br#"{"data":{"issues":{"nodes":[
            {"id":"i1","identifier":"COR-1","title":"Eligible one","url":"u1",
             "labels":{"nodes":[{"name":"AI Eligible"}]}},
            {"id":"i2","identifier":"COR-2","title":"Not tagged","url":"u2",
             "labels":{"nodes":[{"name":"bug"}]}}
        ]}}}"#;
        let all = parse_issues(json, None).unwrap();
        assert_eq!(all.len(), 2);
        let eligible = parse_issues(json, Some("AI Eligible")).unwrap();
        assert_eq!(eligible.len(), 1);
        assert_eq!(eligible[0].identifier, "COR-1");
    }

    #[test]
    fn parse_issues_handles_missing_labels() {
        let json = br#"{"data":{"issues":{"nodes":[
            {"id":"i1","identifier":"COR-3","title":"No labels field","url":"u3"}
        ]}}}"#;
        let issues = parse_issues(json, None).unwrap();
        assert_eq!(issues.len(), 1);
        assert!(issues[0].labels.is_empty());
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
    fn parse_comments_surfaces_graphql_errors() {
        let json = br#"{"errors":[{"message":"authentication required"}]}"#;
        let err = parse_comments(json).unwrap_err();
        assert!(err.0.contains("authentication required"));
    }
}
