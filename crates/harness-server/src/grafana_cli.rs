//! `harness grafana …` — read the connected Grafana from a workflow step.
//!
//! Asks the server's run proxy (`/api/run/grafana/*`, see
//! `http::integrations`) with the run's grant, through the same client the
//! `grafana:` step kind uses ([`harness_runner::integrations`]). The step
//! never holds the Grafana token.

use harness_runner::integrations::RunProxy;
pub use harness_runner::integrations::{Query, QueryKind};

fn proxy() -> Result<RunProxy, String> {
    RunProxy::from_env(|k| std::env::var(k).ok())
}

/// Read one Grafana path through the run proxy, with query parameters.
/// Prints Grafana's body as it is.
pub async fn get(path: &str, params: &[(String, String)]) -> Result<String, String> {
    proxy()?.get("grafana", path, params).await
}

/// Run `q` against a Loki or Prometheus datasource. Prints the datasource's
/// JSON answer.
pub async fn query(kind: QueryKind, q: &Query) -> Result<String, String> {
    proxy()?.grafana_query(kind, q).await
}
