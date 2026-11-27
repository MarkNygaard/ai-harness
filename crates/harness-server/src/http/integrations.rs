//! **Reading Grafana and Sentry through the harness.**
//!
//! Both are integrations (Settings → Integrations): the harness holds the
//! credentials, and nothing it starts ever does. A workflow's `bash` or
//! `script` step reads them through the run proxies here
//! (`GET /api/run/grafana/<path>`, `GET /api/run/sentry/<path>`, authorised by
//! the run's grant, driven by `harness grafana` / `harness sentry`). The chat
//! reads them through the harness's own MCP tools, which call [`read`].
//!
//! **Read-only by construction.** Only `GET`, and only the paths the
//! allowlists below name. The tokens the setup pages ask for are read-only as
//! well (a Grafana Viewer, a Sentry token with Read scopes), so this is a
//! second fence, not the only one. Anyone who also wants an integration's own
//! MCP server adds it under Settings → MCP servers like any other.

use std::sync::Arc;
use std::time::Duration;

use axum::body::Body;
use axum::extract::{Extension, Path, RawQuery};
use axum::http::{header, HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::Json;

use super::run_linear_routes::grant;
use super::runs_routes::RunsState;

/// The connected Grafana.
#[derive(Debug, Clone)]
pub(crate) struct Grafana {
    /// No trailing slash.
    pub url: String,
    pub token: String,
}

/// The connected Sentry, as one project sees it.
#[derive(Debug, Clone)]
pub(crate) struct Sentry {
    /// `https://sentry.io` unless the integration names another (self-hosted,
    /// or a region such as `https://de.sentry.io`). No trailing slash.
    pub url: String,
    pub token: String,
    /// Every read is limited to this organization.
    pub org: String,
    /// The project's own Sentry project, when it names one: the default for
    /// `harness sentry issues` and the chat's issue search.
    pub project: Option<String>,
}

/// How long one read may take. Loki quantiles over a week are slow, and a
/// script that asked for one would rather wait than retry.
pub(crate) const TIMEOUT: Duration = Duration::from_secs(300);

fn err(status: StatusCode, msg: impl Into<String>) -> Response {
    (status, Json(serde_json::json!({ "error": msg.into() }))).into_response()
}

/// The path's segments, when every one is plain: letters, digits and
/// `. _ - : @ +` (Sentry release names use the last two). That rules out
/// traversal and anything percent-encoded around the allowlists. One trailing
/// slash is allowed, since Sentry's paths end in one.
fn segments(path: &str) -> Option<Vec<&str>> {
    let path = path.strip_prefix('/').unwrap_or(path);
    let path = path.strip_suffix('/').unwrap_or(path);
    let segs: Vec<&str> = path.split('/').collect();
    segs.iter()
        .all(|s| {
            !s.is_empty()
                && *s != "."
                && *s != ".."
                && s.chars().all(|c| {
                    c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-' | ':' | '@' | '+')
                })
        })
        .then_some(segs)
}

// ── Grafana ──────────────────────────────────────────────────────────────────

/// Whether `path` (relative to the Grafana URL, no query) may be read:
/// datasource lookups, the Loki and Prometheus read endpoints through
/// Grafana's datasource proxy, and dashboard search and reads.
pub(crate) fn grafana_allowed(path: &str) -> bool {
    let Some(segs) = segments(path) else {
        return false;
    };
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

const GRAFANA_REFUSAL: &str = "Runs may read /api/datasources, /api/search, \
     /api/dashboards/uid/<uid>, and Loki or Prometheus queries through \
     /api/datasources/proxy/uid/<uid>/...";

/// `GET /api/run/grafana/{*path}` — one read from the connected Grafana.
pub async fn grafana_get(
    Extension(state): Extension<Arc<RunsState>>,
    headers: HeaderMap,
    Path(path): Path<String>,
    RawQuery(query): RawQuery,
) -> Response {
    let g = match grant(&headers) {
        Ok(g) => g,
        Err((s, m)) => return err(s, m),
    };
    if !grafana_allowed(&path) {
        return err(
            StatusCode::FORBIDDEN,
            format!("`/{path}` is not something a run may read from Grafana. {GRAFANA_REFUSAL}"),
        );
    }
    let Some(grafana) = state.grafana().await else {
        return err(StatusCode::CONFLICT, GRAFANA_NOT_CONNECTED);
    };
    tracing::info!(run_id = %g.run_id, "grafana: GET /{path}");
    let url = format!("{}/{}", grafana.url, path.trim_matches('/'));
    forward(&url, &grafana.token, query.as_deref(), TIMEOUT).await
}

pub(crate) const GRAFANA_NOT_CONNECTED: &str =
    "Grafana isn't connected: an admin connects it under Settings → Integrations → Grafana";

// ── Sentry ───────────────────────────────────────────────────────────────────

/// Whether `path` (relative to the Sentry URL, no query) may be read, for the
/// organization `org`: issues, their events and tags, events and traces,
/// projects and releases. Every path names `org`, so a token that can see
/// several organizations still reads only the connected one.
pub(crate) fn sentry_allowed(path: &str, org: &str) -> bool {
    let Some(segs) = segments(path) else {
        return false;
    };
    match segs.as_slice() {
        ["api", "0", "organizations", o, rest @ ..] if *o == org => matches!(
            rest,
            ["projects"]
                | ["issues"]
                | ["issues", _]
                | ["issues", _, "events"]
                | ["issues", _, "events", _]
                | ["issues", _, "tags"]
                | ["issues", _, "tags", _]
                | ["issues", _, "hashes"]
                | ["events"]
                | ["events", _]
                | ["events-stats"]
                | ["events-trace", _]
                | ["events-trace-light", _]
                | ["trace-meta", _]
                | ["releases"]
                | ["releases", _]
        ),
        ["api", "0", "projects", o, _, rest @ ..] if *o == org => {
            matches!(rest, ["events", _] | ["releases"])
        }
        _ => false,
    }
}

/// `GET /api/run/sentry/{*path}` — one read from the connected Sentry, or
/// `info` for the organization and the run's Sentry project.
pub async fn sentry_get(
    Extension(state): Extension<Arc<RunsState>>,
    headers: HeaderMap,
    Path(path): Path<String>,
    RawQuery(query): RawQuery,
) -> Response {
    let g = match grant(&headers) {
        Ok(g) => g,
        Err((s, m)) => return err(s, m),
    };
    let Some(sentry) = state.sentry(Some(&g.project)).await else {
        return err(StatusCode::CONFLICT, SENTRY_NOT_CONNECTED);
    };
    if path.trim_matches('/') == "info" {
        return Json(serde_json::json!({ "org": sentry.org, "project": sentry.project }))
            .into_response();
    }
    if !sentry_allowed(&path, &sentry.org) {
        return err(
            StatusCode::FORBIDDEN,
            format!(
                "`/{path}` is not something a run may read from Sentry. Runs may read \
                 /api/0/organizations/{org}/issues/, its events and tags, events, traces, \
                 projects and releases (organization `{org}` only).",
                org = sentry.org
            ),
        );
    }
    tracing::info!(run_id = %g.run_id, "sentry: GET /{path}");
    forward(
        &sentry_url(&sentry, &path),
        &sentry.token,
        query.as_deref(),
        TIMEOUT,
    )
    .await
}

/// Sentry's paths end in a slash; without one it answers with a redirect.
pub(crate) fn sentry_url(sentry: &Sentry, path: &str) -> String {
    format!("{}/{}/", sentry.url, path.trim_matches('/'))
}

pub(crate) const SENTRY_NOT_CONNECTED: &str =
    "Sentry isn't connected: an admin connects it under Settings → Integrations → Sentry";

// ── Forwarding ───────────────────────────────────────────────────────────────

fn client(timeout: Duration) -> Result<reqwest::Client, String> {
    reqwest::Client::builder()
        .timeout(timeout)
        .build()
        .map_err(|e| format!("http client: {e}"))
}

/// The cause of a failed request, for a run log or the chat: reqwest's own
/// message repeats the whole URL, query included, which buries it.
fn cause(e: &reqwest::Error, timeout: Duration) -> String {
    if e.is_timeout() {
        format!("no answer within {}s", timeout.as_secs())
    } else if e.is_connect() {
        "could not connect".to_string()
    } else {
        "the request failed".to_string()
    }
}

/// `GET <url>?<query>` with the token, answered with the service's status,
/// content type and body, streamed.
async fn forward(url: &str, token: &str, query: Option<&str>, timeout: Duration) -> Response {
    let mut url = url.to_string();
    if let Some(q) = query.filter(|q| !q.is_empty()) {
        url.push('?');
        url.push_str(q);
    }
    let client = match client(timeout) {
        Ok(c) => c,
        Err(e) => return err(StatusCode::INTERNAL_SERVER_ERROR, e),
    };
    let resp = match client.get(&url).bearer_auth(token).send().await {
        Ok(r) => r,
        Err(e) => return err(StatusCode::BAD_GATEWAY, cause(&e, timeout)),
    };
    let status = StatusCode::from_u16(resp.status().as_u16()).unwrap_or(StatusCode::BAD_GATEWAY);
    let content_type = resp
        .headers()
        .get(reqwest::header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("application/json")
        .to_string();
    (
        status,
        [(header::CONTENT_TYPE, content_type)],
        Body::from_stream(resp.bytes_stream()),
    )
        .into_response()
}

/// The most of one answer the chat is shown. A week of raw log lines would
/// fill its context and say less than a tighter query.
const CHAT_MAX_CHARS: usize = 60_000;

/// One read for the chat's tools: the body as text, or what went wrong. The
/// path must already have passed its allowlist.
pub(crate) async fn read(
    url: &str,
    token: &str,
    params: &[(String, String)],
) -> Result<String, String> {
    let client = client(TIMEOUT)?;
    let resp = client
        .get(url)
        .bearer_auth(token)
        .query(params)
        .send()
        .await
        .map_err(|e| cause(&e, TIMEOUT))?;
    let status = resp.status();
    let text = resp
        .text()
        .await
        .map_err(|e| format!("reading the answer: {e}"))?;
    if !status.is_success() {
        return Err(format!("{status}: {}", error_message(&text)));
    }
    Ok(truncate(text))
}

fn truncate(mut text: String) -> String {
    if text.len() > CHAT_MAX_CHARS {
        let mut cut = CHAT_MAX_CHARS;
        while !text.is_char_boundary(cut) {
            cut -= 1;
        }
        text.truncate(cut);
        text.push_str(
            "\n… [cut: the answer was longer. Narrow the query: a shorter time range, \
             fewer series, or a limit.]",
        );
    }
    text
}

pub use harness_runner::integrations::error_message;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_run_may_read_grafana_queries_and_lookups() {
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
            assert!(grafana_allowed(path), "{path} should be readable");
        }
    }

    #[test]
    fn a_run_may_not_write_traverse_or_wander_in_grafana() {
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
            assert!(!grafana_allowed(path), "{path} should be refused");
        }
    }

    #[test]
    fn a_run_reads_only_the_connected_sentry_organization() {
        for path in [
            "api/0/organizations/dilling/issues/",
            "api/0/organizations/dilling/issues/4512/",
            "api/0/organizations/dilling/issues/4512/events/latest/",
            "api/0/organizations/dilling/issues/4512/tags/browser/",
            "api/0/organizations/dilling/events/",
            "api/0/organizations/dilling/events-trace/abc123/",
            "api/0/organizations/dilling/releases/frontend@1.2.3+abc/",
            "api/0/organizations/dilling/projects/",
            "api/0/projects/dilling/web/events/0f1e2d/",
            "/api/0/organizations/dilling/issues",
        ] {
            assert!(sentry_allowed(path, "dilling"), "{path} should be readable");
        }
        for path in [
            "api/0/organizations/other/issues/",
            "api/0/organizations/dilling/",
            "api/0/organizations/dilling/members/",
            "api/0/organizations/dilling/issues/4512/comments/",
            "api/0/issues/4512/",
            "api/0/projects/other/web/events/0f1e2d/",
            "api/0/projects/dilling/web/keys/",
            "api/0/organizations/dilling/issues/../../other/issues/",
            "api/0/organizations/dilling/issues//",
        ] {
            assert!(!sentry_allowed(path, "dilling"), "{path} should be refused");
        }
    }

    #[test]
    fn sentry_paths_get_their_trailing_slash() {
        let s = Sentry {
            url: "https://sentry.io".into(),
            token: "t".into(),
            org: "dilling".into(),
            project: None,
        };
        assert_eq!(
            sentry_url(&s, "/api/0/organizations/dilling/issues"),
            "https://sentry.io/api/0/organizations/dilling/issues/"
        );
        assert_eq!(
            sentry_url(&s, "api/0/organizations/dilling/issues/"),
            "https://sentry.io/api/0/organizations/dilling/issues/"
        );
    }

    /// A stand-in service that answers with what it was sent.
    async fn echo() -> String {
        use axum::extract::RawQuery;
        let app = axum::Router::new().route(
            "/svc/api/datasources/proxy/uid/{uid}/loki/api/v1/query",
            axum::routing::get(|headers: HeaderMap, RawQuery(q): RawQuery| async move {
                Json(serde_json::json!({
                    "auth": headers.get("authorization").and_then(|v| v.to_str().ok()),
                    "query": q,
                }))
            }),
        );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        format!("http://{addr}/svc")
    }

    #[tokio::test]
    async fn a_read_reaches_the_service_with_the_token_and_the_query() {
        let base = echo().await;
        let path = "api/datasources/proxy/uid/abc/loki/api/v1/query";
        let query =
            "query=sum%28count_over_time%28%7Bjob%3D%22x%22%7D%5B7d%5D%29%29&time=1795600000";
        let resp = forward(
            &format!("{base}/{path}"),
            "glsa_secret",
            Some(query),
            Duration::from_secs(5),
        )
        .await;
        assert_eq!(resp.status(), StatusCode::OK);
        let body = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap();
        let v: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(v["auth"], "Bearer glsa_secret");
        assert_eq!(v["query"], query, "the query string arrives as sent");

        // The service's own status comes back, not a harness error.
        let missing = forward(
            &format!("{base}/api/search"),
            "t",
            None,
            Duration::from_secs(5),
        )
        .await;
        assert_eq!(missing.status(), StatusCode::NOT_FOUND);

        // The chat's read: the body as text, the parameters encoded.
        let text = read(
            &format!("{base}/{path}"),
            "t",
            &[("query".into(), "{job=\"x\"}".into())],
        )
        .await
        .unwrap();
        assert!(text.contains("query=%7Bjob%3D%22x%22%7D"), "{text}");
        let refused = read(&format!("{base}/api/search"), "t", &[]).await;
        assert!(refused.unwrap_err().starts_with("404"));
    }

    #[tokio::test]
    async fn an_unreachable_service_is_a_bad_gateway() {
        let resp = forward(
            "http://127.0.0.1:9/api/health",
            "t",
            None,
            Duration::from_secs(5),
        )
        .await;
        assert_eq!(resp.status(), StatusCode::BAD_GATEWAY);
    }

    #[test]
    fn long_answers_are_cut_for_the_chat_and_errors_read_out_of_json() {
        let long = "é".repeat(CHAT_MAX_CHARS);
        let cut = truncate(long);
        assert!(cut.ends_with("or a limit.]"));
        assert!(cut.len() < CHAT_MAX_CHARS + 200);
        assert_eq!(
            error_message(r#"{"detail":"Invalid token"}"#),
            "Invalid token"
        );
        assert_eq!(
            error_message(r#"{"message":"Permission denied"}"#),
            "Permission denied"
        );
        assert_eq!(error_message("bad gateway\n"), "bad gateway");
    }
}
