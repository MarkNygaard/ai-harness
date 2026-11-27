//! **Reading Grafana from a workflow script**, authorised by the run's grant.
//!
//! Agent steps reach Grafana through its MCP server (`mcp: [grafana]`). A
//! `bash` or `script` step has no MCP; it needs plain HTTP, for queries it runs
//! the same way every week. This is that: `GET /api/run/grafana/<path>` is
//! forwarded to the Grafana connected under Settings → Integrations, with the
//! service-account token added here. The step never holds the token, so it
//! cannot leak it into a log, a file or a later agent's context.
//!
//! **Read-only by construction.** Only `GET`, and only the paths in
//! [`allowed`]: datasource lookup, the Loki and Prometheus query endpoints
//! through Grafana's datasource proxy, and dashboard search and reads. The token
//! is a Viewer one as well (see the Grafana setup page), so this is a second
//! fence, not the only one.

use std::sync::Arc;

use axum::body::Body;
use axum::extract::{Extension, Path, RawQuery};
use axum::http::{header, HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::Json;

use super::run_linear_routes::grant;
use super::runs_routes::RunsState;

fn err(status: StatusCode, msg: impl Into<String>) -> Response {
    (status, Json(serde_json::json!({ "error": msg.into() }))).into_response()
}

/// How long one query may take. Loki quantiles over a week are slow, and a
/// script that asked for one would rather wait than retry.
const TIMEOUT: std::time::Duration = std::time::Duration::from_secs(300);

/// Whether a run may read `path` (relative to the Grafana URL, no query).
///
/// Every segment is plain (letters, digits, `.`, `_`, `-`, `:`), which rules
/// out traversal and anything percent-encoded around this check.
pub(crate) fn allowed(path: &str) -> bool {
    let segs: Vec<&str> = path.trim_start_matches('/').split('/').collect();
    let plain = |s: &str| {
        !s.is_empty()
            && s != "."
            && s != ".."
            && s.chars()
                .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-' | ':'))
    };
    if !segs.iter().all(|s| plain(s)) {
        return false;
    }
    match segs.as_slice() {
        ["api", "health"]
        | ["api", "datasources"]
        | ["api", "datasources", "name" | "uid", _]
        | ["api", "search"]
        | ["api", "dashboards", "uid", _] => true,
        ["api", "datasources", "proxy", "uid", _, rest @ ..] => datasource_read(rest),
        _ => false,
    }
}

/// The read endpoints of a Loki or Prometheus datasource, behind Grafana's
/// datasource proxy.
fn datasource_read(rest: &[&str]) -> bool {
    const LOKI: &[&str] = &["query", "query_range", "labels", "series", "patterns"];
    const PROMETHEUS: &[&str] = &["query", "query_range", "labels", "series", "metadata"];
    match rest {
        ["loki", "api", "v1", "label", _, "values"] => true,
        ["loki", "api", "v1", "index", "stats"] => true,
        ["loki", "api", "v1", op] => LOKI.contains(op),
        ["api", "v1", "label", _, "values"] => true,
        ["api", "v1", op] => PROMETHEUS.contains(op),
        _ => false,
    }
}

/// `GET /api/run/grafana/{*path}` — one read from the connected Grafana.
///
/// Grafana's status and body come back as they are, so a script reads a Loki
/// error the way it would have from Grafana itself.
pub async fn get(
    Extension(state): Extension<Arc<RunsState>>,
    headers: HeaderMap,
    Path(path): Path<String>,
    RawQuery(query): RawQuery,
) -> Response {
    let g = match grant(&headers) {
        Ok(g) => g,
        Err((s, m)) => return err(s, m),
    };
    if !allowed(&path) {
        return err(
            StatusCode::FORBIDDEN,
            format!(
                "`/{path}` is not something a run may read from Grafana. Runs may read \
                 /api/datasources, /api/search, /api/dashboards/uid/<uid>, and Loki or \
                 Prometheus queries through /api/datasources/proxy/uid/<uid>/..."
            ),
        );
    }
    let env = state.grafana_env().await;
    let value = |key: &str| env.iter().find(|(k, _)| k == key).map(|(_, v)| v.clone());
    let (Some(base), Some(token)) = (
        value(harness_runner::mcp_servers::GRAFANA_URL_ENV),
        value(harness_runner::mcp_servers::GRAFANA_TOKEN_ENV),
    ) else {
        return err(
            StatusCode::CONFLICT,
            "Grafana isn't connected: an admin connects it under Settings → Integrations → Grafana",
        );
    };

    tracing::info!(run_id = %g.run_id, "grafana: GET /{path}");
    forward(&base, &token, &path, query.as_deref(), TIMEOUT).await
}

/// `GET <base>/<path>?<query>` with the token, answered with Grafana's status,
/// content type and body.
async fn forward(
    base: &str,
    token: &str,
    path: &str,
    query: Option<&str>,
    timeout: std::time::Duration,
) -> Response {
    let mut url = format!("{base}/{}", path.trim_start_matches('/'));
    if let Some(q) = query.filter(|q| !q.is_empty()) {
        url.push('?');
        url.push_str(q);
    }
    let client = match reqwest::Client::builder().timeout(timeout).build() {
        Ok(c) => c,
        Err(e) => {
            return err(
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("http client: {e}"),
            )
        }
    };
    let resp = match client.get(&url).bearer_auth(token).send().await {
        Ok(r) => r,
        Err(e) => {
            // The cause only: reqwest's own message repeats the whole URL,
            // query included, which buries it in the run log.
            let cause = if e.is_timeout() {
                format!("Grafana did not answer within {}s", timeout.as_secs())
            } else if e.is_connect() {
                "could not connect to Grafana".to_string()
            } else {
                "the request to Grafana failed".to_string()
            };
            return err(StatusCode::BAD_GATEWAY, cause);
        }
    };
    let status = StatusCode::from_u16(resp.status().as_u16()).unwrap_or(StatusCode::BAD_GATEWAY);
    let content_type = resp
        .headers()
        .get(reqwest::header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("application/json")
        .to_string();
    // Streamed: a week of Loki series can be large.
    (
        status,
        [(header::CONTENT_TYPE, content_type)],
        Body::from_stream(resp.bytes_stream()),
    )
        .into_response()
}

#[cfg(test)]
mod tests {
    use super::{allowed, forward};
    use axum::http::StatusCode;

    /// A stand-in Grafana that answers with what it was sent.
    async fn echo_grafana() -> String {
        use axum::extract::RawQuery;
        use axum::http::HeaderMap;
        let app = axum::Router::new().route(
            "/grafana/api/datasources/proxy/uid/{uid}/loki/api/v1/query",
            axum::routing::get(|headers: HeaderMap, RawQuery(q): RawQuery| async move {
                axum::Json(serde_json::json!({
                    "auth": headers.get("authorization").and_then(|v| v.to_str().ok()),
                    "query": q,
                }))
            }),
        );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        format!("http://{addr}/grafana")
    }

    #[tokio::test]
    async fn a_read_reaches_grafana_with_the_token_and_the_query() {
        let base = echo_grafana().await;
        let resp = forward(
            &base,
            "glsa_secret",
            "api/datasources/proxy/uid/abc/loki/api/v1/query",
            Some("query=sum%28count_over_time%28%7Bjob%3D%22x%22%7D%5B7d%5D%29%29&time=1795600000"),
            std::time::Duration::from_secs(5),
        )
        .await;
        assert_eq!(resp.status(), StatusCode::OK);
        let body = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap();
        let v: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(v["auth"], "Bearer glsa_secret");
        assert_eq!(
            v["query"],
            "query=sum%28count_over_time%28%7Bjob%3D%22x%22%7D%5B7d%5D%29%29&time=1795600000",
            "the query string arrives as the script sent it"
        );

        // Grafana's own status comes back, not a harness error.
        let missing = forward(
            &base,
            "t",
            "api/search",
            None,
            std::time::Duration::from_secs(5),
        )
        .await;
        assert_eq!(missing.status(), StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn an_unreachable_grafana_is_a_bad_gateway() {
        let resp = forward(
            "http://127.0.0.1:9",
            "t",
            "api/health",
            None,
            std::time::Duration::from_secs(5),
        )
        .await;
        assert_eq!(resp.status(), StatusCode::BAD_GATEWAY);
    }

    #[test]
    fn a_run_may_read_queries_and_lookups() {
        for path in [
            "api/health",
            "api/datasources",
            "api/datasources/name/Loki",
            "api/datasources/uid/P8E80F9AEF21F6940",
            "api/search",
            "api/dashboards/uid/dilling-web-vitals",
            "api/datasources/proxy/uid/P8E80F9AEF21F6940/loki/api/v1/query",
            "api/datasources/proxy/uid/P8E80F9AEF21F6940/loki/api/v1/query_range",
            "api/datasources/proxy/uid/abc/loki/api/v1/label/job/values",
            "api/datasources/proxy/uid/abc/loki/api/v1/index/stats",
            "api/datasources/proxy/uid/prom/api/v1/query",
            "api/datasources/proxy/uid/prom/api/v1/label/__name__/values",
            "/api/datasources",
        ] {
            assert!(allowed(path), "{path} should be readable");
        }
    }

    #[test]
    fn a_run_may_not_write_traverse_or_wander() {
        for path in [
            "",
            "api",
            "api/dashboards/db",
            "api/datasources/1",
            "api/admin/users",
            "api/serviceaccounts",
            "api/ds/query",
            "api/datasources/proxy/uid/abc/loki/api/v1/push",
            "api/datasources/proxy/uid/abc/loki/api/v1/delete",
            "api/datasources/proxy/uid/abc/loki/api/v1/index",
            "api/datasources/proxy/uid/abc/api/v1/admin/tsdb/delete_series",
            "api/datasources/proxy/uid/../../admin/users",
            "api/datasources/proxy/uid/abc/loki/api/v1/../../../../admin",
            "api/datasources/name/Loki%2F..",
            "api//datasources",
            "api/search?x",
        ] {
            assert!(!allowed(path), "{path} should be refused");
        }
    }
}
