//! `harness sentry …` — read the connected Sentry from a workflow step.
//!
//! Asks the server's run proxy (`/api/run/sentry/*`, see
//! `http::integrations`) with the run's grant, like `harness grafana`. The
//! step never holds the Sentry token, and reads only the connected
//! organization.

use serde::Deserialize;

/// The organization and the run project's Sentry project, from the server.
#[derive(Debug, Deserialize)]
struct Info {
    org: String,
    project: Option<String>,
}

async fn info() -> Result<Info, String> {
    let text = get_raw("info", &[]).await?;
    serde_json::from_str(&text).map_err(|e| format!("unexpected answer from the harness: {e}"))
}

async fn get_raw(path: &str, params: &[(String, String)]) -> Result<String, String> {
    let g = crate::linear_cli::grant()?;
    let client = reqwest::Client::builder()
        // The server allows Sentry 300s; leave room for the hop.
        .timeout(std::time::Duration::from_secs(330))
        .build()
        .map_err(|e| format!("http client: {e}"))?;
    let resp = client
        .get(format!(
            "{}/api/run/sentry/{}",
            g.base,
            path.trim_start_matches('/')
        ))
        .bearer_auth(&g.token)
        .query(params)
        .send()
        .await
        .map_err(|e| format!("could not reach the harness at {}: {e}", g.base))?;
    let status = resp.status();
    let text = resp
        .text()
        .await
        .map_err(|e| format!("could not read the response: {e}"))?;
    if status.is_success() {
        return Ok(text);
    }
    Err(format!(
        "{status}: {}",
        crate::http::integrations::error_message(&text)
    ))
}

/// Read one Sentry API path through the run proxy. `{org}` in the path is
/// replaced with the connected organization. Prints Sentry's body as it is.
pub async fn get(path: &str, params: &[(String, String)]) -> Result<String, String> {
    let path = if path.contains("{org}") {
        path.replace("{org}", &info().await?.org)
    } else {
        path.to_string()
    };
    get_raw(&path, params).await
}

/// One issue search.
#[derive(Debug, Default)]
pub struct IssueSearch {
    /// Sentry search syntax; default `is:unresolved`.
    pub query: Option<String>,
    /// `date`, `new`, `freq` or `user`.
    pub sort: Option<String>,
    /// How far back, e.g. `24h` or `14d`.
    pub period: Option<String>,
    pub limit: Option<u32>,
}

/// Search the organization's issues, limited to the run project's Sentry
/// project when it has one and the query names none. Prints Sentry's JSON.
pub async fn issues(search: &IssueSearch) -> Result<String, String> {
    let info = info().await?;
    let mut params = vec![
        (
            "query".to_string(),
            crate::http::chat_integrations::issue_query(
                search.query.as_deref(),
                info.project.as_deref(),
            ),
        ),
        (
            "limit".to_string(),
            search.limit.unwrap_or(25).clamp(1, 100).to_string(),
        ),
    ];
    if let Some(sort) = &search.sort {
        params.push(("sort".into(), sort.clone()));
    }
    if let Some(period) = &search.period {
        params.push(("statsPeriod".into(), period.clone()));
    }
    get_raw(
        &format!("api/0/organizations/{}/issues/", info.org),
        &params,
    )
    .await
}

/// One issue, or its latest event (with the stack trace) when `latest_event`.
pub async fn issue(id: &str, latest_event: bool) -> Result<String, String> {
    let org = info().await?.org;
    let suffix = if latest_event { "events/latest/" } else { "" };
    get_raw(
        &format!("api/0/organizations/{org}/issues/{}/{suffix}", id.trim()),
        &[],
    )
    .await
}
