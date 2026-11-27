//! **The chat's Grafana and Sentry tools.**
//!
//! The chat agent has no shell, so it can't run `harness grafana`; it gets the
//! same reads as harness MCP tools instead. They are offered to chat turns
//! only, not to every MCP client: an MCP key is for driving runs and
//! workflows, and should not double as a way into the company's metrics and
//! errors. Each read goes through the allowlists in [`super::integrations`],
//! with the credentials the harness holds.

use std::sync::Arc;

use serde_json::{json, Value};

use super::chat_routes::ChatCaller;
use super::integrations::{self, Grafana, Sentry};
use super::mcp_routes::{to_result, tool_error};
use super::runs_routes::RunsState;
use crate::grafana_cli::{pick_datasource, query_params, Datasource, Query, QueryKind};

/// The tools, by name.
pub(crate) const TOOL_NAMES: &[&str] = &[
    "grafana_query",
    "grafana_get",
    "sentry_issues",
    "sentry_get",
];

/// Their definitions, for a chat turn's `tools/list`.
pub(crate) fn tool_defs() -> Vec<Value> {
    let params = json!({
        "type": "object",
        "description": "Query parameters, as name → value.",
        "additionalProperties": { "type": "string" },
    });
    vec![
        json!({
            "name": "grafana_query",
            "description": "Run one LogQL (Loki) or PromQL (Prometheus) query against the connected Grafana. Instant at `time` (default now), or a range with `start`, `end` and `step`. Times are Unix seconds or RFC 3339. Returns the datasource's JSON. Read-only.",
            "inputSchema": {
                "type": "object",
                "additionalProperties": false,
                "required": ["kind", "query"],
                "properties": {
                    "kind": { "type": "string", "enum": ["loki", "prometheus"] },
                    "query": { "type": "string", "description": "The LogQL or PromQL expression." },
                    "time": { "type": "string" },
                    "start": { "type": "string" },
                    "end": { "type": "string" },
                    "step": { "type": "string", "description": "Range resolution, e.g. 1h." },
                    "limit": { "type": "integer", "description": "Loki: the most log lines to return." },
                    "datasource": { "type": "string", "description": "Name or uid; default: the default datasource of that kind." },
                },
            },
            "annotations": { "readOnlyHint": true },
        }),
        json!({
            "name": "grafana_get",
            "description": "Read one Grafana API path: api/datasources, api/search (dashboards), api/dashboards/uid/<uid>, or a Loki/Prometheus read under api/datasources/proxy/uid/<uid>/ (labels, label values, series). Returns Grafana's JSON. Read-only.",
            "inputSchema": {
                "type": "object",
                "additionalProperties": false,
                "required": ["path"],
                "properties": { "path": { "type": "string" }, "params": params },
            },
            "annotations": { "readOnlyHint": true },
        }),
        json!({
            "name": "sentry_issues",
            "description": "Search the connected Sentry organization's issues, with Sentry search syntax (default `is:unresolved`). Limited to the conversation's Sentry project when the project names one. Returns Sentry's JSON. Read-only.",
            "inputSchema": {
                "type": "object",
                "additionalProperties": false,
                "properties": {
                    "query": { "type": "string", "description": "e.g. `is:unresolved level:error release:1.4.2`." },
                    "sort": { "type": "string", "enum": ["date", "new", "freq", "user"] },
                    "period": { "type": "string", "description": "How far back, e.g. 24h or 14d." },
                    "limit": { "type": "integer", "description": "At most 100." },
                },
            },
            "annotations": { "readOnlyHint": true },
        }),
        json!({
            "name": "sentry_get",
            "description": "Read one Sentry API path in the connected organization: api/0/organizations/<org>/issues/<id>/, .../issues/<id>/events/latest/, .../events/, .../events-trace/<trace>/, .../releases/, .../projects/, or api/0/projects/<org>/<project>/events/<id>/. Returns Sentry's JSON. Read-only.",
            "inputSchema": {
                "type": "object",
                "additionalProperties": false,
                "required": ["path"],
                "properties": { "path": { "type": "string" }, "params": params },
            },
            "annotations": { "readOnlyHint": true },
        }),
    ]
}

fn params_of(args: &Value) -> Vec<(String, String)> {
    args.get("params")
        .and_then(Value::as_object)
        .map(|m| {
            m.iter()
                .map(|(k, v)| {
                    let v = v
                        .as_str()
                        .map(str::to_string)
                        .unwrap_or_else(|| v.to_string());
                    (k.clone(), v)
                })
                .collect()
        })
        .unwrap_or_default()
}

fn text(answer: Result<String, String>) -> Value {
    match answer {
        Ok(body) => to_result(body, &Value::Null),
        Err(e) => tool_error(e),
    }
}

/// Answer one of [`TOOL_NAMES`] for a chat turn.
pub(crate) async fn call(
    state: &Arc<RunsState>,
    caller: &ChatCaller,
    name: &str,
    args: &Value,
) -> Value {
    let s = |k: &str| args.get(k).and_then(Value::as_str).map(str::to_string);
    match name {
        "grafana_query" | "grafana_get" => {
            let Some(grafana) = state.grafana().await else {
                return tool_error(integrations::GRAFANA_NOT_CONNECTED);
            };
            if name == "grafana_get" {
                let path = s("path").unwrap_or_default();
                return text(grafana_get(&grafana, &path, &params_of(args)).await);
            }
            let kind = match s("kind").as_deref() {
                Some("loki") => QueryKind::Loki,
                Some("prometheus") => QueryKind::Prometheus,
                _ => return tool_error("`kind` is `loki` or `prometheus`"),
            };
            let q = Query {
                expr: s("query").unwrap_or_default(),
                time: s("time"),
                start: s("start"),
                end: s("end"),
                step: s("step"),
                limit: args.get("limit").and_then(Value::as_u64).map(|n| n as u32),
                datasource: s("datasource"),
            };
            text(grafana_query(&grafana, kind, &q).await)
        }
        "sentry_issues" | "sentry_get" => {
            let project = conversation_project(state, caller).await;
            let Some(sentry) = state.sentry(project.as_deref()).await else {
                return tool_error(integrations::SENTRY_NOT_CONNECTED);
            };
            if name == "sentry_get" {
                let path = s("path").unwrap_or_default();
                return text(sentry_get(&sentry, &path, &params_of(args)).await);
            }
            let limit = args
                .get("limit")
                .and_then(Value::as_u64)
                .unwrap_or(25)
                .clamp(1, 100);
            let mut params = vec![
                (
                    "query".to_string(),
                    issue_query(s("query").as_deref(), sentry.project.as_deref()),
                ),
                ("limit".to_string(), limit.to_string()),
            ];
            if let Some(sort) = s("sort") {
                params.push(("sort".into(), sort));
            }
            if let Some(period) = s("period") {
                params.push(("statsPeriod".into(), period));
            }
            let path = format!("api/0/organizations/{}/issues/", sentry.org);
            text(sentry_get(&sentry, &path, &params).await)
        }
        other => tool_error(format!("unknown tool: {other}")),
    }
}

/// The issue search: the person's query, or unresolved issues, limited to the
/// project's Sentry project when it has one and the query names none.
pub(crate) fn issue_query(query: Option<&str>, project: Option<&str>) -> String {
    let query = query
        .map(str::trim)
        .filter(|q| !q.is_empty())
        .unwrap_or("is:unresolved");
    match project {
        Some(p) if !query.contains("project:") => format!("project:{p} {query}"),
        _ => query.to_string(),
    }
}

async fn conversation_project(state: &Arc<RunsState>, caller: &ChatCaller) -> Option<String> {
    let store = state.chat_store().await.ok()?;
    let conv = store.get(&caller.conversation_id).await.ok()??;
    conv.context
        .0
        .get("project")
        .and_then(Value::as_str)
        .map(str::to_string)
}

async fn grafana_get(
    grafana: &Grafana,
    path: &str,
    params: &[(String, String)],
) -> Result<String, String> {
    if !integrations::grafana_allowed(path) {
        return Err(format!(
            "`{path}` is not a Grafana read the chat may make: datasources, dashboard search \
             and reads, and Loki or Prometheus reads under api/datasources/proxy/uid/<uid>/"
        ));
    }
    let url = format!("{}/{}", grafana.url, path.trim_matches('/'));
    integrations::read(&url, &grafana.token, params).await
}

async fn grafana_query(grafana: &Grafana, kind: QueryKind, q: &Query) -> Result<String, String> {
    if q.expr.trim().is_empty() {
        return Err("`query` is empty".into());
    }
    let list = grafana_get(grafana, "api/datasources", &[]).await?;
    let all: Vec<Datasource> = serde_json::from_str(&list)
        .map_err(|e| format!("unexpected datasource list from Grafana: {e}"))?;
    let uid = pick_datasource(&all, kind, q.datasource.as_deref())?;
    let (range, params) = query_params(kind, q)?;
    grafana_get(
        grafana,
        &format!("api/datasources/proxy/uid/{uid}/{}", kind.api(range)),
        &params,
    )
    .await
}

async fn sentry_get(
    sentry: &Sentry,
    path: &str,
    params: &[(String, String)],
) -> Result<String, String> {
    if !integrations::sentry_allowed(path, &sentry.org) {
        return Err(format!(
            "`{path}` is not a Sentry read the chat may make. Paths start with \
             api/0/organizations/{org}/ (issues, their events and tags, events, traces, \
             releases, projects) or api/0/projects/{org}/<project>/events/<id>/",
            org = sentry.org
        ));
    }
    integrations::read(
        &integrations::sentry_url(sentry, path),
        &sentry.token,
        params,
    )
    .await
}

/// What the chat is told when Grafana is connected.
pub(crate) fn grafana_note() -> String {
    "Grafana is connected: `grafana_query` runs LogQL or PromQL, read-only, and `grafana_get` \
     reads datasources, dashboards and label values. Use it for questions about traffic, \
     latency, error rates, logs, or what a dashboard shows. Find the datasource and the metric \
     or label names first (`grafana_get` on api/datasources, then label values) rather than \
     guessing, keep time ranges tight, and give the query you ran so a person can check it."
        .to_string()
}

/// What the chat is told when Sentry is connected.
pub(crate) fn sentry_note(sentry: &Sentry) -> String {
    let scope = match &sentry.project {
        Some(p) => format!("issue searches are limited to the Sentry project `{p}`"),
        None => format!("across the organization `{}`", sentry.org),
    };
    format!(
        "Sentry is connected, read-only ({scope}): `sentry_issues` searches issues with Sentry \
         query syntax (e.g. `is:unresolved level:error`), and `sentry_get` reads an issue, its \
         latest event and stack trace, traces and releases, under \
         api/0/organizations/{org}/. Use it when the question is about an error, a crash, a \
         spike, or what broke after a release, and link to the issue \
         ({url}/organizations/{org}/issues/<id>/).",
        org = sentry.org,
        url = sentry.url,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_issue_search_is_scoped_to_the_project_unless_it_names_one() {
        assert_eq!(issue_query(None, None), "is:unresolved");
        assert_eq!(
            issue_query(Some("  "), Some("web")),
            "project:web is:unresolved"
        );
        assert_eq!(
            issue_query(Some("level:error"), Some("web")),
            "project:web level:error"
        );
        assert_eq!(
            issue_query(Some("project:api level:error"), Some("web")),
            "project:api level:error"
        );
    }

    #[test]
    fn every_tool_is_defined_and_read_only() {
        let defs = tool_defs();
        let names: Vec<&str> = defs.iter().filter_map(|d| d["name"].as_str()).collect();
        assert_eq!(names, TOOL_NAMES);
        assert!(defs
            .iter()
            .all(|d| d["annotations"]["readOnlyHint"] == true));
    }

    #[test]
    fn the_notes_say_how_far_sentry_reaches_and_ask_for_checkable_queries() {
        let mut s = Sentry {
            url: "https://sentry.io".into(),
            token: "t".into(),
            org: "dilling".into(),
            project: Some("webshop".into()),
        };
        assert!(sentry_note(&s).contains("limited to the Sentry project `webshop`"));
        s.project = None;
        let n = sentry_note(&s);
        assert!(n.contains("across the organization `dilling`") && n.contains("read-only"));
        assert!(grafana_note().contains("give the query you ran"));
    }

    #[test]
    fn query_params_come_through_as_strings() {
        let p = params_of(&json!({ "params": { "since": "24h", "limit": 5 } }));
        assert!(p.contains(&("since".to_string(), "24h".to_string())));
        assert!(p.contains(&("limit".to_string(), "5".to_string())));
        assert!(params_of(&json!({})).is_empty());
    }
}
