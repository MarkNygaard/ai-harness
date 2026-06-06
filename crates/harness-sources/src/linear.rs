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

// ── GraphQL documents ────────────────────────────────────────────────────────

const DISCOVERY_QUERY: &str = r#"
query Discovery {
  teams(first: 250) {
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
            return Err(LinearError(format!("HTTP {} from Linear", status.as_u16())));
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
            br#"{"data":{"issueUpdate":{"success":false}}}"#,
            "issueUpdate"
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
}
