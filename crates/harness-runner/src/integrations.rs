//! **Reading Grafana and Sentry through the server's run proxies.**
//!
//! Both are integrations the server holds the credentials for. A run reaches
//! them only through `/api/run/grafana/*` and `/api/run/sentry/*`, with its own
//! grant (`HARNESS_RUN_URL`, `HARNESS_RUN_TOKEN`), and only to read. This is the
//! one client for that: the `grafana:` and `sentry:` step kinds use it from
//! the runner, and `harness grafana` / `harness sentry` from a script.
//!
//! The query-building pieces are public because the server's chat tools build
//! the same queries against Grafana and Sentry directly.

use serde::Deserialize;

// ── Grafana queries ──────────────────────────────────────────────────────────

/// Which kind of datasource a query is for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum QueryKind {
    Loki,
    Prometheus,
}

impl QueryKind {
    pub fn datasource_type(self) -> &'static str {
        match self {
            QueryKind::Loki => "loki",
            QueryKind::Prometheus => "prometheus",
        }
    }

    /// The datasource's own API under Grafana's proxy.
    pub fn api(self, range: bool) -> &'static str {
        match (self, range) {
            (QueryKind::Loki, false) => "loki/api/v1/query",
            (QueryKind::Loki, true) => "loki/api/v1/query_range",
            (QueryKind::Prometheus, false) => "api/v1/query",
            (QueryKind::Prometheus, true) => "api/v1/query_range",
        }
    }
}

/// One LogQL or PromQL query. Times are Unix seconds, RFC 3339, `now`, or
/// `now-<n><s|m|h|d|w>`.
#[derive(Debug, Default, Clone)]
pub struct Query {
    pub expr: String,
    /// An instant query at this time (default: now).
    pub time: Option<String>,
    /// A range query from `start` to `end` (default: now).
    pub start: Option<String>,
    pub end: Option<String>,
    /// The range query's resolution, e.g. `1h` or `3600`.
    pub step: Option<String>,
    /// Loki: the most log lines to return.
    pub limit: Option<u32>,
    /// The datasource's name or uid. Default: the default one of its kind,
    /// else the first.
    pub datasource: Option<String>,
}

impl Query {
    /// The query a `grafana:` step describes.
    pub fn from_step(step: &harness_dag::GrafanaStep) -> (QueryKind, Query) {
        let (kind, expr) = match (&step.loki, &step.prometheus) {
            (Some(q), _) => (QueryKind::Loki, q.clone()),
            (None, Some(q)) => (QueryKind::Prometheus, q.clone()),
            (None, None) => (QueryKind::Loki, String::new()),
        };
        (
            kind,
            Query {
                expr,
                time: step.time.clone(),
                start: step.start.clone(),
                end: step.end.clone(),
                step: step.step.clone(),
                limit: step.limit,
                datasource: step.datasource.clone(),
            },
        )
    }
}

/// `now` and `now-<n><unit>` as Unix seconds; anything else as written, for
/// the datasource to read (Unix seconds or RFC 3339).
pub fn resolve_time(value: &str, now: i64) -> Result<String, String> {
    let v = value.trim();
    let Some(rest) = v.strip_prefix("now") else {
        return Ok(v.to_string());
    };
    if rest.is_empty() {
        return Ok(now.to_string());
    }
    let ago = rest
        .strip_prefix('-')
        .and_then(|d| {
            let (n, unit) = d.split_at(d.len().checked_sub(1)?);
            let n: i64 = n.parse().ok()?;
            let secs = match unit {
                "s" => 1,
                "m" => 60,
                "h" => 3600,
                "d" => 86_400,
                "w" => 7 * 86_400,
                _ => return None,
            };
            Some(n * secs)
        })
        .ok_or_else(|| {
            format!("`{v}` is not a time: use now, now-<n><s|m|h|d|w>, Unix seconds or RFC 3339")
        })?;
    Ok((now - ago).to_string())
}

/// The request parameters, and whether this is a range query.
pub fn query_params(
    kind: QueryKind,
    q: &Query,
    now: i64,
) -> Result<(bool, Vec<(String, String)>), String> {
    if q.expr.trim().is_empty() {
        return Err("the query is empty".into());
    }
    let range = q.start.is_some() || q.end.is_some();
    if range && q.start.is_none() {
        return Err("a range query needs a start".into());
    }
    if range && q.time.is_some() {
        return Err("`time` is for an instant query; a range takes `start` and `end`".into());
    }
    if !range && q.step.is_some() {
        return Err("`step` is for a range query; give `start` too".into());
    }
    let mut params = vec![("query".to_string(), q.expr.clone())];
    if let Some(t) = &q.time {
        params.push(("time".into(), resolve_time(t, now)?));
    }
    if let Some(t) = &q.start {
        params.push(("start".into(), resolve_time(t, now)?));
        let end = q.end.as_deref().unwrap_or("now");
        params.push(("end".into(), resolve_time(end, now)?));
    }
    if let Some(step) = &q.step {
        params.push(("step".into(), step.clone()));
    }
    if let Some(limit) = q.limit {
        if kind == QueryKind::Prometheus {
            return Err("`limit` is for Loki queries".into());
        }
        params.push(("limit".into(), limit.to_string()));
    }
    Ok((range, params))
}

#[derive(Debug, Clone, Deserialize)]
pub struct Datasource {
    pub uid: String,
    pub name: String,
    #[serde(rename = "type")]
    pub kind: String,
    #[serde(default, rename = "isDefault")]
    pub is_default: bool,
}

/// The datasource to query: the one named (by name or uid), or the default of
/// its kind, else the first of its kind.
pub fn pick_datasource(
    all: &[Datasource],
    kind: QueryKind,
    wanted: Option<&str>,
) -> Result<String, String> {
    let of_kind: Vec<&Datasource> = all
        .iter()
        .filter(|d| d.kind == kind.datasource_type())
        .collect();
    let names = || {
        of_kind
            .iter()
            .map(|d| d.name.as_str())
            .collect::<Vec<_>>()
            .join(", ")
    };
    if let Some(w) = wanted.filter(|w| !w.trim().is_empty()) {
        return of_kind
            .iter()
            .find(|d| d.uid == w || d.name == w)
            .map(|d| d.uid.clone())
            .ok_or_else(|| {
                format!(
                    "no {} datasource named `{w}`; there is: {}",
                    kind.datasource_type(),
                    names()
                )
            });
    }
    of_kind
        .iter()
        .find(|d| d.is_default)
        .or(of_kind.first())
        .map(|d| d.uid.clone())
        .ok_or_else(|| format!("Grafana has no {} datasource", kind.datasource_type()))
}

// ── Sentry queries ───────────────────────────────────────────────────────────

/// The issue search: the given query, or unresolved issues, limited to the
/// project's Sentry project when it has one and the query names none.
pub fn issue_query(query: Option<&str>, project: Option<&str>) -> String {
    let query = query
        .map(str::trim)
        .filter(|q| !q.is_empty())
        .unwrap_or("is:unresolved");
    match project {
        Some(p) if !query.contains("project:") => format!("project:{p} {query}"),
        _ => query.to_string(),
    }
}

/// One issue search.
#[derive(Debug, Default, Clone)]
pub struct IssueSearch {
    /// Sentry search syntax; default `is:unresolved`.
    pub query: Option<String>,
    /// `date`, `new`, `freq` or `user`.
    pub sort: Option<String>,
    /// How far back, e.g. `24h` or `14d`.
    pub period: Option<String>,
    pub limit: Option<u32>,
}

impl IssueSearch {
    /// The request parameters, scoped to `project` (see [`issue_query`]).
    pub fn params(&self, project: Option<&str>) -> Vec<(String, String)> {
        let mut params = vec![
            (
                "query".to_string(),
                issue_query(self.query.as_deref(), project),
            ),
            (
                "limit".to_string(),
                self.limit.unwrap_or(25).clamp(1, 100).to_string(),
            ),
        ];
        if let Some(sort) = &self.sort {
            params.push(("sort".into(), sort.clone()));
        }
        if let Some(period) = &self.period {
            params.push(("statsPeriod".into(), period.clone()));
        }
        params
    }
}

/// The connected organization and the run project's Sentry project.
#[derive(Debug, Deserialize)]
pub struct SentryInfo {
    pub org: String,
    pub project: Option<String>,
}

// ── The client ───────────────────────────────────────────────────────────────

/// The message out of a JSON error body (Grafana's `message`, Loki's,
/// Prometheus's and the harness's `error`, Sentry's `detail`), or the body.
pub fn error_message(body: &str) -> String {
    serde_json::from_str::<serde_json::Value>(body)
        .ok()
        .and_then(|v| {
            ["error", "message", "detail"]
                .iter()
                .find_map(|k| v.get(k).and_then(|e| e.as_str()).map(str::to_string))
        })
        .unwrap_or_else(|| body.trim().chars().take(500).collect())
}

/// A run's way to the proxies: the server's public URL and the run's grant.
pub struct RunProxy {
    base: String,
    token: String,
}

impl RunProxy {
    /// From a run's environment. Without a grant this is not a run, or the
    /// server has no public URL, which is worth saying plainly.
    pub fn from_env(get: impl Fn(&str) -> Option<String>) -> Result<Self, String> {
        let base = get("HARNESS_RUN_URL")
            .map(|b| b.trim().trim_end_matches('/').to_string())
            .filter(|b| !b.is_empty())
            .ok_or(
                "HARNESS_RUN_URL is not set: this runs inside a workflow run, and needs the \
                 harness's public URL (Settings -> General)",
            )?;
        let token = get("HARNESS_RUN_TOKEN")
            .map(|t| t.trim().to_string())
            .filter(|t| !t.is_empty())
            .ok_or("HARNESS_RUN_TOKEN is not set: this is not a workflow run")?;
        Ok(Self { base, token })
    }

    /// `GET /api/run/<service>/<path>` with `params`; the body as text.
    pub async fn get(
        &self,
        service: &str,
        path: &str,
        params: &[(String, String)],
    ) -> Result<String, String> {
        let client = reqwest::Client::builder()
            // The server allows the service 300s; leave room for the hop.
            .timeout(std::time::Duration::from_secs(330))
            .build()
            .map_err(|e| format!("http client: {e}"))?;
        let resp = client
            .get(format!(
                "{}/api/run/{service}/{}",
                self.base,
                path.trim_start_matches('/')
            ))
            .bearer_auth(&self.token)
            .query(params)
            .send()
            .await
            .map_err(|e| format!("could not reach the harness at {}: {e}", self.base))?;
        let status = resp.status();
        let text = resp
            .text()
            .await
            .map_err(|e| format!("could not read the response: {e}"))?;
        if status.is_success() {
            return Ok(text);
        }
        Err(format!("{status}: {}", error_message(&text)))
    }

    /// Run one LogQL or PromQL query, finding the datasource first.
    pub async fn grafana_query(&self, kind: QueryKind, q: &Query) -> Result<String, String> {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs() as i64)
            .unwrap_or(0);
        let (range, params) = query_params(kind, q, now)?;
        let list = self.get("grafana", "api/datasources", &[]).await?;
        let all: Vec<Datasource> = serde_json::from_str(&list)
            .map_err(|e| format!("unexpected datasource list from Grafana: {e}"))?;
        let uid = pick_datasource(&all, kind, q.datasource.as_deref())?;
        self.get(
            "grafana",
            &format!("api/datasources/proxy/uid/{uid}/{}", kind.api(range)),
            &params,
        )
        .await
    }

    pub async fn sentry_info(&self) -> Result<SentryInfo, String> {
        let text = self.get("sentry", "info", &[]).await?;
        serde_json::from_str(&text).map_err(|e| format!("unexpected answer from the harness: {e}"))
    }

    /// Search the organization's issues (see [`IssueSearch`]).
    pub async fn sentry_issues(&self, search: &IssueSearch) -> Result<String, String> {
        let info = self.sentry_info().await?;
        self.get(
            "sentry",
            &format!("api/0/organizations/{}/issues/", info.org),
            &search.params(info.project.as_deref()),
        )
        .await
    }

    /// One issue, or its latest event (with the stack trace).
    pub async fn sentry_issue(&self, id: &str, latest_event: bool) -> Result<String, String> {
        let org = self.sentry_info().await?.org;
        let suffix = if latest_event { "events/latest/" } else { "" };
        self.get(
            "sentry",
            &format!("api/0/organizations/{org}/issues/{}/{suffix}", id.trim()),
            &[],
        )
        .await
    }

    /// What a `grafana:` step reads.
    pub async fn grafana_step(&self, step: &harness_dag::GrafanaStep) -> Result<String, String> {
        let (kind, q) = Query::from_step(step);
        self.grafana_query(kind, &q).await
    }

    /// What a `sentry:` step reads.
    pub async fn sentry_step(&self, step: &harness_dag::SentryStep) -> Result<String, String> {
        match &step.issue {
            Some(id) => self.sentry_issue(id, step.latest_event).await,
            None => {
                self.sentry_issues(&IssueSearch {
                    query: step.issues.clone(),
                    sort: step.sort.clone(),
                    period: step.period.clone(),
                    limit: step.limit,
                })
                .await
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ds(uid: &str, name: &str, kind: &str, is_default: bool) -> Datasource {
        Datasource {
            uid: uid.into(),
            name: name.into(),
            kind: kind.into(),
            is_default,
        }
    }

    #[test]
    fn the_datasource_is_the_named_one_or_the_default_of_its_kind() {
        let all = [
            ds("p1", "Prometheus", "prometheus", true),
            ds("l1", "Loki old", "loki", false),
            ds("l2", "Loki", "loki", true),
        ];
        assert_eq!(pick_datasource(&all, QueryKind::Loki, None).unwrap(), "l2");
        assert_eq!(
            pick_datasource(&all, QueryKind::Loki, Some("Loki old")).unwrap(),
            "l1"
        );
        assert_eq!(
            pick_datasource(&all, QueryKind::Loki, Some("l1")).unwrap(),
            "l1"
        );
        let err = pick_datasource(&all, QueryKind::Loki, Some("Prometheus")).unwrap_err();
        assert!(err.contains("Loki old, Loki"), "{err}");
        assert!(pick_datasource(&all[..1], QueryKind::Loki, None).is_err());
    }

    #[test]
    fn relative_times_become_unix_seconds() {
        let now = 1_795_600_000;
        assert_eq!(resolve_time("now", now).unwrap(), "1795600000");
        assert_eq!(resolve_time("now-1d", now).unwrap(), "1795513600");
        assert_eq!(
            resolve_time("now-2w", now).unwrap(),
            (now - 14 * 86_400).to_string()
        );
        assert_eq!(
            resolve_time("2026-11-20T00:00:00Z", now).unwrap(),
            "2026-11-20T00:00:00Z"
        );
        assert!(resolve_time("now-1y", now).is_err());
        assert!(resolve_time("nowish", now).is_err());
    }

    #[test]
    fn instant_and_range_queries_take_their_own_parameters() {
        let now = 1_795_600_000;
        let instant = Query {
            expr: "sum(rate(x[5m]))".into(),
            time: Some("now-1h".into()),
            ..Query::default()
        };
        let (range, params) = query_params(QueryKind::Prometheus, &instant, now).unwrap();
        assert!(!range);
        assert_eq!(params[1], ("time".into(), (now - 3600).to_string()));

        // A start alone runs to now.
        let ranged = Query {
            expr: "{job=\"web-vitals\"}".into(),
            start: Some("now-7d".into()),
            step: Some("1d".into()),
            limit: Some(10),
            ..Query::default()
        };
        let (range, params) = query_params(QueryKind::Loki, &ranged, now).unwrap();
        assert!(range);
        assert!(params.contains(&("end".into(), now.to_string())));
        assert_eq!(params.len(), 5);
        assert_eq!(QueryKind::Loki.api(range), "loki/api/v1/query_range");

        let end_only = Query {
            expr: "x".into(),
            end: Some("now".into()),
            ..Query::default()
        };
        assert!(query_params(QueryKind::Loki, &end_only, now).is_err());
        let step_only = Query {
            expr: "x".into(),
            step: Some("1h".into()),
            ..Query::default()
        };
        assert!(query_params(QueryKind::Loki, &step_only, now).is_err());
        let prom_limit = Query {
            expr: "x".into(),
            limit: Some(5),
            ..Query::default()
        };
        assert!(query_params(QueryKind::Prometheus, &prom_limit, now).is_err());
        assert!(query_params(QueryKind::Loki, &Query::default(), now).is_err());
    }

    #[test]
    fn the_issue_search_is_scoped_to_the_project_unless_it_names_one() {
        assert_eq!(issue_query(None, None), "is:unresolved");
        assert_eq!(
            issue_query(Some("  "), Some("web")),
            "project:web is:unresolved"
        );
        assert_eq!(
            issue_query(Some("project:api level:error"), Some("web")),
            "project:api level:error"
        );
        let p = IssueSearch {
            limit: Some(500),
            period: Some("24h".into()),
            ..IssueSearch::default()
        }
        .params(None);
        assert!(p.contains(&("limit".into(), "100".into())));
        assert!(p.contains(&("statsPeriod".into(), "24h".into())));
    }

    #[test]
    fn a_step_becomes_its_query() {
        let step = harness_dag::GrafanaStep {
            prometheus: Some("up".into()),
            start: Some("now-1d".into()),
            step: Some("1h".into()),
            ..Default::default()
        };
        let (kind, q) = Query::from_step(&step);
        assert_eq!(kind, QueryKind::Prometheus);
        assert_eq!((q.expr.as_str(), q.step.as_deref()), ("up", Some("1h")));
    }

    #[test]
    fn without_a_grant_it_says_why() {
        let none = RunProxy::from_env(|_| None).err().unwrap();
        assert!(none.contains("HARNESS_RUN_URL"));
        let no_token =
            RunProxy::from_env(|k| (k == "HARNESS_RUN_URL").then(|| "https://h/".into()))
                .err()
                .unwrap();
        assert!(no_token.contains("HARNESS_RUN_TOKEN"));
        assert_eq!(
            error_message(r#"{"detail":"Invalid token"}"#),
            "Invalid token"
        );
    }
}
