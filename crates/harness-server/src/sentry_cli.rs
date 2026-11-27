//! `harness sentry …` — read the connected Sentry from a workflow step.
//!
//! Asks the server's run proxy (`/api/run/sentry/*`, see
//! `http::integrations`) with the run's grant, through the same client the
//! `sentry:` step kind uses ([`harness_runner::integrations`]). The step never
//! holds the Sentry token, and reads only the connected organization.

pub use harness_runner::integrations::IssueSearch;
use harness_runner::integrations::RunProxy;

fn proxy() -> Result<RunProxy, String> {
    RunProxy::from_env(|k| std::env::var(k).ok())
}

/// Read one Sentry API path through the run proxy. `{org}` in the path is
/// replaced with the connected organization. Prints Sentry's body as it is.
pub async fn get(path: &str, params: &[(String, String)]) -> Result<String, String> {
    let proxy = proxy()?;
    let path = if path.contains("{org}") {
        path.replace("{org}", &proxy.sentry_info().await?.org)
    } else {
        path.to_string()
    };
    proxy.get("sentry", &path, params).await
}

/// Search the organization's issues, limited to the run project's Sentry
/// project when it has one and the query names none. Prints Sentry's JSON.
pub async fn issues(search: &IssueSearch) -> Result<String, String> {
    proxy()?.sentry_issues(search).await
}

/// One issue, or its latest event (with the stack trace) when `latest_event`.
pub async fn issue(id: &str, latest_event: bool) -> Result<String, String> {
    proxy()?.sentry_issue(id, latest_event).await
}
