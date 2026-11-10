use super::AppState;
use axum::{
    extract::State,
    middleware::Next,
    response::{IntoResponse, Response},
    Json,
};
use serde_json::json;
use std::sync::Arc;
use subtle::ConstantTimeEq;

/// Percent-decode a query-string value (RFC 3986 §2.1).
///
/// `encodeURIComponent` on the browser side encodes characters such as `+`,
/// `/`, `=`, and `%` as `%2B`, `%2F`, `%3D`, `%25`. Without decoding, the
/// constant-time comparison against the stored token would fail for any token
/// that contains those characters.
fn percent_decode(s: &str) -> String {
    let mut out = Vec::with_capacity(s.len());
    let bytes = s.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' {
            if let (Some(hi), Some(lo)) = (
                hex_nibble(bytes.get(i + 1).copied()),
                hex_nibble(bytes.get(i + 2).copied()),
            ) {
                out.push(hi << 4 | lo);
                i += 3;
                continue;
            }
        }
        out.push(bytes[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

fn hex_nibble(b: Option<u8>) -> Option<u8> {
    match b? {
        c @ b'0'..=b'9' => Some(c - b'0'),
        c @ b'a'..=b'f' => Some(c - b'a' + 10),
        c @ b'A'..=b'F' => Some(c - b'A' + 10),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn percent_decode_plain_token_unchanged() {
        assert_eq!(percent_decode("mytoken"), "mytoken");
    }

    #[test]
    fn percent_decode_plus_sign() {
        // encodeURIComponent("+") → "%2B"; must decode to literal "+"
        assert_eq!(percent_decode("sec%2Bret"), "sec+ret");
    }

    #[test]
    fn percent_decode_slash_equals() {
        // encodeURIComponent("/abc=") → "%2Fabc%3D"
        assert_eq!(percent_decode("%2Fabc%3D"), "/abc=");
    }

    #[test]
    fn percent_decode_percent_sign() {
        // encodeURIComponent("%") → "%25"
        assert_eq!(percent_decode("%25"), "%");
    }

    #[test]
    fn percent_decode_full_base64_style_token() {
        // Simulate encodeURIComponent("sec+ret/abc=") → "sec%2Bret%2Fabc%3D"
        assert_eq!(percent_decode("sec%2Bret%2Fabc%3D"), "sec+ret/abc=");
    }

    #[test]
    fn percent_decode_invalid_escape_left_as_is() {
        // Incomplete %XX should be left as-is, not panic
        assert_eq!(percent_decode("tok%2"), "tok%2");
        assert_eq!(percent_decode("tok%"), "tok%");
    }
}

/// Resolve the effective API token from server config or `HARNESS_API_TOKEN` env var.
///
/// Filters empty strings *before* the env-var fallback so that an explicit
/// `api_token = ""` in server.toml does not shadow `HARNESS_API_TOKEN`.
pub(crate) fn resolve_api_token(
    config: &harness_core::config::server::ServerConfig,
) -> Option<String> {
    config
        .api_token
        .as_deref()
        .filter(|t| !t.is_empty())
        .map(|t| t.to_owned())
        .or_else(|| {
            std::env::var("HARNESS_API_TOKEN")
                .ok()
                .filter(|t| !t.is_empty())
        })
}

pub(crate) fn is_auth_exempt_path(path: &str) -> bool {
    matches!(
        path,
        "/health"
            | "/webhook"
            | "/webhook/feishu"
            | "/signals"
            | "/favicon.ico"
            | "/auth/reset-password"
            // Claiming an install and signing in: the routes that exist to get
            // you *past* this middleware. Each authenticates itself — the
            // one-time setup token, a password, or in `status`'s case nothing
            // worth protecting (see `auth_routes`).
            | "/api/auth/status"
            | "/api/auth/setup"
            | "/api/auth/login"
            // Linear's OAuth redirect: a browser navigation cannot carry an
            // `Authorization` header. Authenticated instead by the single-use,
            // unguessable `state` nonce the server minted (see `linear_oauth`).
            | "/api/linear/oauth/callback"
            // The identity provider's redirect, and what the sign-in page needs
            // before anybody has signed in. Both are browser navigations that
            // cannot carry a bearer token; the callback is authenticated by its
            // single-use `state` nonce.
            | "/api/auth/oidc/callback"
            | "/api/auth/oidc/start"
            | "/api/auth/oidc/status"
            | "/api/auth/github/callback"
            | "/api/auth/github/start"
            | "/api/auth/github/status"
            // Linear's agent-session webhook (delegation / @-mention). Also a
            // third party we cannot hand a bearer token; authenticated by the
            // `Linear-Signature` HMAC over the raw body (see `linear_agent`).
            | "/api/linear/webhook"
            // What a workflow run asks of the server; its Linear routes are the
            // prefix below. Exempt from *this* middleware because a run holds
            // no session and no API token — it holds a grant scoped to its own
            // project, which this middleware knows nothing about. Every one of
            // these authenticates itself against that grant and refuses
            // without one (see `run_grants`).
            | "/api/run/s3/upload-url"
            | "/api/run/improvements/evidence"
            | "/api/run/facts/evidence"
            | "/api/run/guard"
            // An MCP server's OAuth redirect: a browser navigation, so no
            // header. Authenticated by the single-use `state` nonce the server
            // minted when an admin started the sign-in (see `mcp_registry`).
            | "/api/mcp-servers/oauth/callback"
            // MCP-over-HTTP. Exempt from *this* middleware so an editor holding
            // only the MCP key can reach it — a key this middleware knows
            // nothing about. `handle_mcp` then authenticates every request
            // itself, accepting that key or the legacy shared token, and is
            // never open when either exists (see `mcp_key::authorized`).
            | "/mcp"
            | "/"
            | "/overview"
            | "/worktrees"
            | "/ws"
    ) || path.starts_with("/assets/")
        // Redeeming an invitation or a reset link. Whoever holds one has not
        // signed in yet — that is the point of it — and the token in the path is
        // what authenticates them: single-use, expiring, and hashed at rest.
        // A prefix, because the token is a path segment.
        || path.starts_with("/api/invites/token/")
        // A webhook trigger's delivery. The sender is a third party with no
        // session; each trigger verifies its own deliveries against its own
        // secret, and an unknown id is a 404 (see `triggers_routes`).
        || path.starts_with("/api/hooks/")
        // A workflow step's MCP gateway. Opening a session authenticates with
        // the run's grant; every other call with the step's own session token,
        // which buys only that step's tools (see `mcp_gateway`). The server
        // name is a path segment, hence a prefix.
        || path.starts_with("/api/run/mcp/")
        // What a workflow run asks of Linear: moves, comments, labels, and the
        // epic's pieces. A prefix, not a list: the routes were once listed one
        // by one, five added later were missed, and every call a run made to
        // them was refused wherever a sign-in or an API token is required.
        // Each handler authenticates against the run's grant itself
        // (`run_linear_routes::grant`).
        || path.starts_with("/api/run/linear/")
}

#[cfg(test)]
mod run_route_tests {
    use super::is_auth_exempt_path;

    /// A run carries only its grant, so every `/api/run/` route the router
    /// serves must get past this middleware and authenticate against the
    /// grant itself. Read from the router's source, so a route added there
    /// without a matching exemption fails here rather than in production.
    #[test]
    fn every_run_route_is_reachable_with_a_grant() {
        let router = include_str!("http_router.rs");
        let routes: Vec<&str> = router
            .split('"')
            .filter(|s| s.starts_with("/api/run/"))
            .collect();
        assert!(routes.len() >= 15, "found only {routes:?}");
        for route in routes {
            // A path parameter stands in for any value.
            let concrete = route.replace("{server}", "github");
            assert!(
                is_auth_exempt_path(&concrete),
                "`{route}` is served to runs but needs a session or API token"
            );
        }
        // The prefix covers what runs ask of Linear, not the rest of the API.
        assert!(!is_auth_exempt_path("/api/linear/sources"));
    }
}

/// Bearer token authentication middleware.
///
/// Exempts `/health`, `/webhook`, `/webhook/feishu`, `/signals`, `/favicon.ico`,
/// `/auth/reset-password`, `/api/linear/oauth/callback` (Linear's OAuth
/// redirect — a browser navigation, authenticated by its single-use `state`
/// nonce), `/` (dashboard HTML), `/overview` (system overview HTML),
/// `/assets/*` (hashed React bundle assets), and `/ws` (WebSocket upgrade).
/// The dashboard HTML no longer embeds the token, so it is safe to serve without
/// auth. `/ws` is exempt from *this middleware* because the WebSocket upgrade
/// cannot carry a body and must be handled before axum reads headers twice;
/// however `ws_handler` performs its own two-layer access control: (1) Origin
/// header validation to prevent CSWH, and (2) Bearer token verification for
/// **all** clients (including those that present a localhost Origin) when a token
/// is configured.  Origin alone is not trusted for auth because it can be forged
/// by non-browser tools.
/// All other endpoints require an `Authorization: Bearer <token>` header when
/// `api_token` is configured. When no token is configured the middleware is a
/// no-op (backward compat).
pub(crate) async fn api_auth_middleware(
    State(state): State<Arc<AppState>>,
    req: axum::extract::Request,
    next: Next,
) -> Response {
    let path = req.uri().path();
    // Exempt paths: static assets, public health probes, and endpoints that
    // carry no secrets (/ no longer embeds the token; /ws streams only
    // task-status events).
    if is_auth_exempt_path(path) {
        return next.run(req).await;
    }

    // In `accounts` mode a signed-in session is what authorises a request, and
    // the shared token stops being a way in for a person. Checked before the
    // bearer logic because it *replaces* that rather than adding to it.
    if let Some(runs) = req
        .extensions()
        .get::<Arc<super::runs_routes::RunsState>>()
        .cloned()
    {
        if super::accounts::mode(&runs).await == super::accounts::Mode::Accounts {
            if super::auth_routes::permits(&runs, req.headers()).await {
                return next.run(req).await;
            }
            return (
                axum::http::StatusCode::UNAUTHORIZED,
                Json(json!({"error": "sign in to use this harness"})),
            )
                .into_response();
        }
    }

    let Some(expected) = resolve_api_token(&state.core.server.config.server) else {
        // No token configured — skip auth for backward compatibility.
        return next.run(req).await;
    };

    // Bearer header takes priority; fall back to ?token= query param only for
    // SSE stream endpoints — EventSource/browser navigation cannot set custom
    // headers. Leaking bearer tokens through arbitrary query strings is a
    // credential-logging risk, so other endpoints must use the header.
    let provided = req
        .headers()
        .get(axum::http::header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .and_then(|s| s.strip_prefix("Bearer "))
        .map(str::to_string)
        .or_else(|| {
            if path.starts_with("/tasks/") && path.ends_with("/stream") {
                req.uri().query().and_then(|q| {
                    q.split('&')
                        .find(|p| p.starts_with("token="))
                        .and_then(|p| p.strip_prefix("token="))
                        .map(percent_decode)
                })
            } else {
                None
            }
        });

    let authorized = provided
        .as_deref()
        .map(|tok| tok.as_bytes().ct_eq(expected.as_bytes()).into())
        .unwrap_or(false);

    if authorized {
        next.run(req).await
    } else {
        (
            axum::http::StatusCode::UNAUTHORIZED,
            Json(json!({"error": "unauthorized"})),
        )
            .into_response()
    }
}
