//! **Linear OAuth (`actor=app`)** — connect the Linear workspace so the harness
//! writes as *itself* instead of as the person who pasted a personal API key.
//!
//! A personal API key resolves to the human who minted it, so every comment,
//! status move and attachment the poller makes reads as authored by that person.
//! Linear's fix is an OAuth application installed with `actor=app`: "resources
//! are created as the application… for agents and service accounts". Same
//! GraphQL API, app attribution.
//!
//! - `GET  /api/linear/oauth/start`             — JSON `{ url }` to send the browser to
//! - `GET  /api/linear/oauth/callback?code=&state=` — Linear redirects here (auth-exempt)
//! - `GET  /api/linear/oauth/status`            — how the harness authenticates
//! - `POST /api/linear/oauth/disconnect`        — revoke + clear the tokens
//!
//! **One credential per connected workspace.** Each install is a
//! [`ConnectionId`] whose secrets live under its own provider key — see
//! [`super::linear_connections`] — and a project resolves to one of them. The
//! identity being connected is the *app*, so within one workspace a single
//! install still serves every project pointing at it. Any per-project `linear`
//! credential from an earlier version is inert: nothing reads it.
//!
//! **Credential fields** (per connection, encrypted at rest):
//! `client_id` / `client_secret` (the OAuth app, pasted by the operator),
//! `access_token` / `refresh_token` / `expires_at_ms` / `scope` (from the
//! exchange), `workspace_id` / `workspace_name` / `workspace_url_key` (recorded
//! at connect time), `refresh_error` (last refresh failure, `""` when healthy),
//! and the legacy `api_key`. OAuth wins when both are present.

use std::collections::{BTreeMap, HashMap};
use std::sync::{Arc, LazyLock};

use axum::extract::{Extension, Query};
use axum::http::{header, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::Json;
use harness_persist::CredentialStore;
use harness_sources::linear::{LinearAuth, LinearClient};
use serde::Deserialize;

use super::linear_connections::ConnectionId;
use super::runs_routes::RunsState;

/// Path Linear redirects back to. Must match the OAuth app's registered callback
/// **exactly**, and is exempt from the bearer-token middleware (see
/// [`super::auth::is_auth_exempt_path`]) because a browser redirect cannot carry
/// an `Authorization` header — the unguessable single-use `state` nonce is what
/// authenticates the callback.
pub(crate) const CALLBACK_PATH: &str = "/api/linear/oauth/callback";

/// Path Linear delivers agent-session events to. Registered on the OAuth app's
/// webhook, and likewise auth-exempt — authenticated by the `Linear-Signature`
/// HMAC rather than by the API bearer token (see [`super::linear_agent`]).
pub(crate) const WEBHOOK_PATH: &str = "/api/linear/webhook";

/// Where the callback sends the browser afterwards (the Credentials page, which
/// owns the Linear connection).
const UI_RETURN_PATH: &str = "/credentials";

/// Scopes requested. `read` powers discovery/preview; `write` covers everything
/// the poller mutates (issue state, comments, labels, attachments);
/// `app:assignable` lets the app be **delegated** an issue and `app:mentionable`
/// lets it be @-mentioned — the two triggers that open an agent session.
/// Deliberately **not** `admin`.
///
/// Adding a scope means the stored token no longer carries everything we ask
/// for: an install from before the agent scopes existed keeps working for the
/// poller but cannot be delegated to, so [`status`] reports the token's scopes
/// and the UI prompts a reconnect.
const SCOPES: &[&str] = &["read", "write", "app:assignable", "app:mentionable"];

/// Scopes that must be present for delegation/mention to work.
const AGENT_SCOPES: &[&str] = &["app:assignable", "app:mentionable"];

/// Refresh once the access token has less than this left (ms). Linear's access
/// tokens last ~24h; the poller ticks every 30s, so a 5-minute skew means a run
/// never starts on a token that expires mid-flight.
const REFRESH_SKEW_MS: i64 = 5 * 60 * 1000;

/// How long an unused `state` nonce stays valid.
const PENDING_TTL_MS: i64 = 10 * 60 * 1000;

/// Fallback token lifetime when Linear omits `expires_in`.
const DEFAULT_EXPIRES_IN_SECS: i64 = 24 * 3600;

fn authorize_url() -> String {
    env_or(
        "HARNESS_LINEAR_OAUTH_AUTHORIZE_URL",
        "https://linear.app/oauth/authorize",
    )
}

fn token_url() -> String {
    env_or(
        "HARNESS_LINEAR_OAUTH_TOKEN_URL",
        "https://api.linear.app/oauth/token",
    )
}

fn revoke_url() -> String {
    env_or(
        "HARNESS_LINEAR_OAUTH_REVOKE_URL",
        "https://api.linear.app/oauth/revoke",
    )
}

/// Env override with a default — the endpoints are stable, but a self-hosted or
/// test Linear shouldn't require a rebuild (mirrors the Claude OAuth helpers in
/// `credentials_routes`).
fn env_or(var: &str, default: &str) -> String {
    std::env::var(var)
        .ok()
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| default.to_string())
}

fn err(status: StatusCode, msg: impl Into<String>) -> Response {
    (status, Json(serde_json::json!({ "error": msg.into() }))).into_response()
}

fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

// ── Pending authorizations (CSRF `state` nonces) ─────────────────────────────
//
// In-process only: the harness runs as a single container, and the callback
// lands on the same instance that issued the nonce. A restart mid-flow just
// means the user clicks Connect again.

static PENDING: LazyLock<std::sync::Mutex<HashMap<String, i64>>> =
    LazyLock::new(|| std::sync::Mutex::new(HashMap::new()));

/// Mint a single-use `state` nonce, pruning expired ones.
fn issue_state() -> String {
    // Two v4 UUIDs = 256 bits of randomness (the `uuid` crate is already a dep;
    // this avoids pulling `rand` into harness-server just for a nonce).
    let nonce = format!(
        "{}{}",
        uuid::Uuid::new_v4().simple(),
        uuid::Uuid::new_v4().simple()
    );
    let now = now_ms();
    if let Ok(mut map) = PENDING.lock() {
        map.retain(|_, created| now - *created < PENDING_TTL_MS);
        map.insert(nonce.clone(), now);
    }
    nonce
}

/// Consume a `state` nonce. `false` if unknown, already used, or expired — all of
/// which must fail the callback.
fn take_state(nonce: &str) -> bool {
    let Ok(mut map) = PENDING.lock() else {
        return false;
    };
    match map.remove(nonce) {
        Some(created) => now_ms() - created < PENDING_TTL_MS,
        None => false,
    }
}

// ── URL building ─────────────────────────────────────────────────────────────

/// Percent-encode everything outside the unreserved set, so a redirect URI or a
/// comma-separated scope list survives the query string intact.
fn enc(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(b as char)
            }
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

/// The `actor=app` authorization URL. `actor=app` is the whole point: it makes
/// Linear attribute writes to the application instead of the installing user.
fn build_authorize_url(client_id: &str, redirect_uri: &str, state: &str) -> String {
    format!(
        "{}?response_type=code&client_id={}&redirect_uri={}&scope={}&state={}&actor=app&prompt=consent",
        authorize_url(),
        enc(client_id),
        enc(redirect_uri),
        enc(&SCOPES.join(",")),
        enc(state),
    )
}

/// The absolute callback URL to register in the Linear OAuth app. Requires
/// `HARNESS_PUBLIC_URL` / `server.public_url` — Linear needs an exact match.
fn redirect_uri(state: &Arc<RunsState>) -> Result<String, String> {
    let base = state.public_url.as_deref().ok_or(
        "no public URL configured — set `HARNESS_PUBLIC_URL` (or `server.public_url`) \
         so Linear has a callback address to redirect to",
    )?;
    Ok(format!("{base}{CALLBACK_PATH}"))
}

// ── Token exchange / refresh (pure parsing + HTTP) ───────────────────────────

/// A token set as returned by Linear's token endpoint.
#[derive(Debug, Clone, PartialEq)]
struct TokenSet {
    access_token: String,
    /// Absent when the OAuth app doesn't have refresh tokens enabled (those
    /// access tokens are long-lived instead).
    refresh_token: Option<String>,
    expires_in_secs: i64,
    scope: String,
}

/// Parse Linear's `{access_token, token_type, expires_in, scope, refresh_token}`.
fn parse_token_response(body: &[u8]) -> Result<TokenSet, String> {
    let v: serde_json::Value =
        serde_json::from_slice(body).map_err(|e| format!("bad token response: {e}"))?;
    // Linear reports OAuth failures as `{"error": …, "error_description": …}`.
    if let Some(e) = v.get("error").and_then(|e| e.as_str()) {
        let desc = v
            .get("error_description")
            .and_then(|d| d.as_str())
            .unwrap_or(e);
        return Err(format!("{e}: {desc}"));
    }
    let access_token = v
        .get("access_token")
        .and_then(|t| t.as_str())
        .filter(|t| !t.is_empty())
        .ok_or("token response had no access_token")?
        .to_string();
    Ok(TokenSet {
        access_token,
        refresh_token: v
            .get("refresh_token")
            .and_then(|t| t.as_str())
            .filter(|t| !t.is_empty())
            .map(str::to_string),
        expires_in_secs: v
            .get("expires_in")
            .and_then(|e| e.as_i64())
            .filter(|e| *e > 0)
            .unwrap_or(DEFAULT_EXPIRES_IN_SECS),
        scope: v
            .get("scope")
            .and_then(|s| s.as_str())
            .unwrap_or_default()
            .to_string(),
    })
}

/// POST form-encoded params to Linear's token endpoint and parse the result.
async fn post_token(form: &[(&str, &str)]) -> Result<TokenSet, String> {
    let resp = reqwest::Client::new()
        .post(token_url())
        .form(form)
        .send()
        .await
        .map_err(|e| format!("token request failed: {e}"))?;
    let status = resp.status();
    let body = resp.bytes().await.map_err(|e| format!("read body: {e}"))?;
    match parse_token_response(&body) {
        Ok(t) => Ok(t),
        // A non-2xx without the expected error shape: report the status, and
        // truncate the body so a token can't sprawl across the logs.
        Err(e) if !status.is_success() => {
            let snippet: String = String::from_utf8_lossy(&body).chars().take(200).collect();
            Err(format!("HTTP {}: {e} ({snippet})", status.as_u16()))
        }
        Err(e) => Err(e),
    }
}

/// Exchange an authorization code for tokens.
async fn exchange_code(
    code: &str,
    redirect_uri: &str,
    client_id: &str,
    client_secret: &str,
) -> Result<TokenSet, String> {
    post_token(&[
        ("grant_type", "authorization_code"),
        ("code", code),
        ("redirect_uri", redirect_uri),
        ("client_id", client_id),
        ("client_secret", client_secret),
    ])
    .await
}

/// Exchange a refresh token for a fresh pair. Linear invalidates the presented
/// refresh token, so the new one must be persisted.
async fn refresh_tokens(
    refresh_token: &str,
    client_id: &str,
    client_secret: &str,
) -> Result<TokenSet, String> {
    post_token(&[
        ("grant_type", "refresh_token"),
        ("refresh_token", refresh_token),
        ("client_id", client_id),
        ("client_secret", client_secret),
    ])
    .await
}

/// Best-effort revocation so a disconnected token stops working immediately.
async fn revoke_token(access_token: &str) {
    let sent = reqwest::Client::new()
        .post(revoke_url())
        .header(header::AUTHORIZATION, format!("Bearer {access_token}"))
        .form(&[("token", access_token)])
        .send()
        .await;
    match sent {
        Ok(r) if r.status().is_success() => {}
        Ok(r) => tracing::warn!("linear oauth: revoke returned HTTP {}", r.status().as_u16()),
        Err(e) => tracing::warn!("linear oauth: revoke failed: {e}"),
    }
}

// ── Stored-credential helpers ────────────────────────────────────────────────

fn field(fields: &BTreeMap<String, String>, key: &str) -> Option<String> {
    fields.get(key).filter(|v| !v.is_empty()).cloned()
}

/// Whether a credential blob can actually authenticate (as opposed to holding
/// only half-configured OAuth client details).
fn has_usable_auth(fields: &BTreeMap<String, String>) -> bool {
    field(fields, "access_token").is_some() || field(fields, "api_key").is_some()
}

/// The stored credential for `conn`, or `None` when nothing is stored.
async fn credential(
    store: &CredentialStore,
    conn: &ConnectionId,
) -> Option<BTreeMap<String, String>> {
    store.get(&conn.provider_key()).await.ok().flatten()
}

/// The OAuth app's client id + secret for `conn`, if both are stored.
async fn client_credentials(
    store: &CredentialStore,
    conn: &ConnectionId,
) -> Option<(String, String)> {
    let fields = credential(store, conn).await?;
    Some((
        field(&fields, "client_id")?,
        field(&fields, "client_secret")?,
    ))
}

/// What identifies one connection's inbound webhook deliveries: the workspace
/// they come from, and the secret they are signed with.
pub(crate) struct WebhookIdentity {
    pub(crate) id: ConnectionId,
    /// Linear's `organizationId` for this install, recorded at connect time.
    /// `None` on an install made before it was captured — such a connection can
    /// still be identified by its signature, just not by the payload.
    pub(crate) workspace_id: Option<String>,
    /// The OAuth app's webhook signing secret.
    pub(crate) secret: String,
}

/// Every connection that can verify an inbound webhook, i.e. has a signing
/// secret stored. Connections without one are omitted: they cannot authenticate
/// a delivery, so they can never be the answer.
pub(crate) async fn webhook_identities(store: &CredentialStore) -> Vec<WebhookIdentity> {
    let mut out = Vec::new();
    for id in super::linear_connections::list_ids(store)
        .await
        .unwrap_or_default()
    {
        let Some(fields) = credential(store, &id).await else {
            continue;
        };
        let Some(secret) = field(&fields, "webhook_secret") else {
            continue;
        };
        out.push(WebhookIdentity {
            id,
            workspace_id: field(&fields, "workspace_id"),
            secret,
        });
    }
    out
}

/// The harness's own app user id in the workspace — the delegate the poller
/// matches issues against. Recorded at connect time; `None` on an install made
/// before it was captured, which the poller treats as "claim nothing".
pub(crate) async fn app_user_id(state: &Arc<RunsState>, conn: &ConnectionId) -> Option<String> {
    let store = state.cred_store().await.ok()?;
    field(&credential(store, conn).await?, "app_user_id")
}

/// Whether the stored token actually carries the agent scopes — an install made
/// before they were requested keeps working for the poller but cannot be
/// delegated to. Linear returns granted scopes space-separated; tolerate commas.
fn has_agent_scopes(granted: Option<&str>) -> bool {
    let Some(granted) = granted else {
        return false;
    };
    let granted: Vec<&str> = granted
        .split([' ', ','])
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .collect();
    AGENT_SCOPES.iter().all(|need| granted.contains(need))
}

/// Serialize refreshes: Linear invalidates the presented refresh token, so two
/// concurrent refreshes would leave one caller holding a dead token. Refreshes
/// are rare (once a day), so one lock is enough.
static REFRESH_LOCK: LazyLock<tokio::sync::Mutex<()>> =
    LazyLock::new(|| tokio::sync::Mutex::new(()));

/// Return a usable access token, refreshing first if it is at/near expiry. `Err`
/// only when there is no way to get a live token (no refresh token, or the
/// refresh was rejected).
async fn ensure_fresh_token(
    store: &CredentialStore,
    conn: &ConnectionId,
    fields: &BTreeMap<String, String>,
) -> Result<String, String> {
    let access = field(fields, "access_token").ok_or("credential has no access_token")?;
    // Unknown expiry (or comfortably valid) → use what we have. Apps without
    // refresh tokens enabled get long-lived tokens and never take this path.
    match fields
        .get("expires_at_ms")
        .and_then(|v| v.parse::<i64>().ok())
    {
        Some(exp) if exp - now_ms() <= REFRESH_SKEW_MS => {}
        _ => return Ok(access),
    }

    let _guard = REFRESH_LOCK.lock().await;
    // Re-read under the lock: a concurrent caller may have just refreshed.
    let current = credential(store, conn).await.unwrap_or_default();
    let access = field(&current, "access_token").unwrap_or(access);
    if let Some(exp) = current
        .get("expires_at_ms")
        .and_then(|v| v.parse::<i64>().ok())
    {
        if exp - now_ms() > REFRESH_SKEW_MS {
            return Ok(access);
        }
    }

    let refresh = field(&current, "refresh_token").ok_or_else(|| {
        "Linear access token expired and the credential has no refresh token — reconnect the \
         workspace"
            .to_string()
    })?;
    let (client_id, client_secret) = client_credentials(store, conn)
        .await
        .ok_or("cannot refresh: the Linear OAuth client_id/client_secret are not stored")?;

    match refresh_tokens(&refresh, &client_id, &client_secret).await {
        Ok(tokens) => {
            let mut update = token_fields(&tokens);
            update.insert("refresh_error".into(), String::new());
            store
                .set(&conn.provider_key(), &update)
                .await
                .map_err(|e| e.to_string())?;
            tracing::info!(
                "linear oauth: refreshed access token (expires in {}s)",
                tokens.expires_in_secs
            );
            Ok(tokens.access_token)
        }
        Err(e) => {
            // Record it so the UI can say "reconnect" instead of failing silently
            // every poll. Linear answers a spent/revoked refresh token with
            // `invalid_grant`, which no retry will fix.
            let _ = store
                .set(
                    &conn.provider_key(),
                    &BTreeMap::from([("refresh_error".to_string(), e.clone())]),
                )
                .await;
            tracing::warn!("linear oauth: token refresh failed: {e}");
            Err(format!("Linear token refresh failed: {e}"))
        }
    }
}

/// The credential fields describing a freshly obtained token set.
fn token_fields(tokens: &TokenSet) -> BTreeMap<String, String> {
    let mut out = BTreeMap::from([
        ("access_token".to_string(), tokens.access_token.clone()),
        (
            "expires_at_ms".to_string(),
            (now_ms() + tokens.expires_in_secs * 1000).to_string(),
        ),
        ("scope".to_string(), tokens.scope.clone()),
        // Recorded so the UI (and a future migration) can tell an app-actor
        // install from a personal key without inspecting the token itself.
        ("actor".to_string(), "app".to_string()),
    ]);
    if let Some(r) = &tokens.refresh_token {
        out.insert("refresh_token".to_string(), r.clone());
    }
    out
}

// ── The shared client constructor ────────────────────────────────────────────

/// Build a Linear client, preferring the app-actor OAuth token and falling back
/// to a legacy personal API key. **Every** Linear call site goes through here, so
/// attribution and token freshness are decided in one place.
pub(crate) async fn linear_client(
    state: &Arc<RunsState>,
    conn: &ConnectionId,
) -> Result<LinearClient, String> {
    let store = state.cred_store().await?;
    let fields = credential(store, conn)
        .await
        .filter(has_usable_auth)
        .ok_or("Linear is not connected — connect the workspace on the Credentials page")?;
    if field(&fields, "access_token").is_some() {
        let token = ensure_fresh_token(store, conn, &fields).await?;
        return Ok(LinearClient::with_auth(LinearAuth::OauthToken(token)));
    }
    let key = field(&fields, "api_key").ok_or("credential has neither access_token nor api_key")?;
    Ok(LinearClient::new(key))
}

/// [`linear_client`] for background callers that just skip the work when Linear
/// isn't connected (the poller logs and moves on).
pub(crate) async fn linear_client_or_none(
    state: &Arc<RunsState>,
    conn: &ConnectionId,
) -> Option<LinearClient> {
    match linear_client(state, conn).await {
        Ok(c) => Some(c),
        Err(e) => {
            // `warn`, not `debug`: the poller skips every claim when this returns
            // None, so at debug level the harness silently stops transitioning
            // issues and reporting progress with nothing in the logs to explain it.
            tracing::warn!("linear: no usable credential: {e}");
            None
        }
    }
}

// ── Routes ───────────────────────────────────────────────────────────────────

/// `GET /api/linear/oauth/start` — the authorization URL to send the browser to.
///
/// Returns JSON rather than a 302 because the SPA calls it with the API bearer
/// token; a plain navigation to this path would be rejected by the auth
/// middleware. The client follows `url` itself.
pub async fn start(Extension(state): Extension<Arc<RunsState>>) -> Response {
    // Until the connection-management API lands, these routes address the
    // legacy connection — which is the only one an existing install has.
    let conn = ConnectionId::default();
    let store = match state.cred_store().await {
        Ok(s) => s,
        Err(e) => return err(StatusCode::SERVICE_UNAVAILABLE, e),
    };
    let redirect = match redirect_uri(&state) {
        Ok(u) => u,
        Err(e) => return err(StatusCode::PRECONDITION_FAILED, e),
    };
    let Some((client_id, _)) = client_credentials(store, &conn).await else {
        return err(
            StatusCode::PRECONDITION_FAILED,
            "no Linear OAuth app configured — save a client ID and client secret first \
             (Linear → Settings → API → OAuth applications)",
        );
    };
    let url = build_authorize_url(&client_id, &redirect, &issue_state());
    Json(serde_json::json!({ "url": url, "callback_url": redirect })).into_response()
}

#[derive(Debug, Deserialize)]
pub struct CallbackQuery {
    #[serde(default)]
    pub code: Option<String>,
    #[serde(default)]
    pub state: Option<String>,
    /// Present when the user denied consent.
    #[serde(default)]
    pub error: Option<String>,
    #[serde(default)]
    pub error_description: Option<String>,
}

/// Send the browser back to the Credentials page with the outcome. Relative
/// `Location` — the browser is already on this origin, so no public URL needed.
fn back_to_ui(status: &str, detail: Option<&str>) -> Response {
    let mut location = format!("{UI_RETURN_PATH}?linear={}", enc(status));
    if let Some(d) = detail {
        // Truncate: this ends up in a URL bar and an on-screen banner.
        let short: String = d.chars().take(200).collect();
        location.push_str(&format!("&linear_message={}", enc(&short)));
    }
    (StatusCode::SEE_OTHER, [(header::LOCATION, location)]).into_response()
}

/// `GET /api/linear/oauth/callback` — Linear's redirect. Validates the
/// single-use `state`, exchanges the code, probes the token, and stores it.
pub async fn callback(
    Extension(state): Extension<Arc<RunsState>>,
    Query(q): Query<CallbackQuery>,
) -> Response {
    if let Some(e) = q.error.as_deref() {
        let detail = q.error_description.as_deref().unwrap_or(e);
        tracing::warn!("linear oauth: authorization denied: {detail}");
        return back_to_ui("denied", Some(detail));
    }
    let (Some(code), Some(nonce)) = (q.code.as_deref(), q.state.as_deref()) else {
        return back_to_ui("error", Some("callback missing `code` or `state`"));
    };
    // Single-use nonce: this is what authenticates an unauthenticated callback.
    if !take_state(nonce) {
        tracing::warn!("linear oauth: callback with unknown/expired state");
        return back_to_ui(
            "error",
            Some("authorization expired or was already used — start again"),
        );
    }

    // Until the connection-management API lands, these routes address the
    // legacy connection — which is the only one an existing install has.
    let conn = ConnectionId::default();
    let store = match state.cred_store().await {
        Ok(s) => s,
        Err(e) => return back_to_ui("error", Some(&e)),
    };
    let Ok(redirect) = redirect_uri(&state) else {
        return back_to_ui("error", Some("no public URL configured"));
    };
    let Some((client_id, client_secret)) = client_credentials(store, &conn).await else {
        return back_to_ui("error", Some("Linear OAuth client credentials are missing"));
    };

    let tokens = match exchange_code(code, &redirect, &client_id, &client_secret).await {
        Ok(t) => t,
        Err(e) => {
            tracing::warn!("linear oauth: code exchange failed: {e}");
            return back_to_ui("error", Some(&e));
        }
    };

    // Probe with the new token before storing it: a credential that can't even
    // name its workspace should not replace a working one.
    let client = LinearClient::with_auth(LinearAuth::OauthToken(tokens.access_token.clone()));
    let workspace = match client.organization().await {
        Ok(w) => w,
        Err(e) => {
            tracing::warn!("linear oauth: token probe failed: {}", e.0);
            return back_to_ui("error", Some(&e.0));
        }
    };
    // The app's own user id in this workspace — who delegated issues are
    // assigned to. Best-effort: an older app without the agent scopes still
    // connects fine for the poller, just without delegation.
    let app_user_id = match client.app_user_id().await {
        Ok(id) => Some(id),
        Err(e) => {
            tracing::warn!("linear oauth: could not read the app user id: {}", e.0);
            None
        }
    };

    let mut fields = token_fields(&tokens);
    fields.insert("workspace_id".into(), workspace.id);
    fields.insert("workspace_name".into(), workspace.name.clone());
    fields.insert("workspace_url_key".into(), workspace.url_key);
    fields.insert("refresh_error".into(), String::new());
    fields.insert("app_user_id".into(), app_user_id.unwrap_or_default());
    if let Err(e) = store.set(&conn.provider_key(), &fields).await {
        return back_to_ui("error", Some(&e.to_string()));
    }
    tracing::info!(
        "linear oauth: connected to workspace `{}` as app actor",
        workspace.name
    );
    back_to_ui("connected", Some(&workspace.name))
}

/// `GET /api/linear/oauth/status` — how the harness talks to Linear, and what
/// still needs doing. Never returns secrets.
pub async fn status(Extension(state): Extension<Arc<RunsState>>) -> Response {
    // Until the connection-management API lands, these routes address the
    // legacy connection — which is the only one an existing install has.
    let conn = ConnectionId::default();
    let store = match state.cred_store().await {
        Ok(s) => s,
        Err(e) => return err(StatusCode::SERVICE_UNAVAILABLE, e),
    };
    let fields = credential(store, &conn).await.unwrap_or_default();
    let mode = if field(&fields, "access_token").is_some() {
        "app"
    } else if field(&fields, "api_key").is_some() {
        "personal_key"
    } else {
        "none"
    };
    let token_scope = field(&fields, "scope");
    Json(serde_json::json!({
        "mode": mode,
        "workspace_name": field(&fields, "workspace_name"),
        "workspace_url_key": field(&fields, "workspace_url_key"),
        "token_scope": token_scope,
        "expires_at_ms": fields
            .get("expires_at_ms")
            .and_then(|v| v.parse::<i64>().ok()),
        "refresh_error": field(&fields, "refresh_error"),
        "client_configured": field(&fields, "client_id").is_some()
            && field(&fields, "client_secret").is_some(),
        "callback_url": redirect_uri(&state).ok(),
        // Delegation readiness: the token must carry the agent scopes, and the
        // webhook must be registered with its signing secret stored here.
        "agent_scopes_granted": has_agent_scopes(token_scope.as_deref()),
        "webhook_secret_configured": field(&fields, "webhook_secret").is_some(),
        "webhook_url": state
            .public_url
            .as_deref()
            .map(|b| format!("{b}{WEBHOOK_PATH}")),
        "app_user_id": field(&fields, "app_user_id"),
    }))
    .into_response()
}

/// `POST /api/linear/oauth/disconnect` — revoke the token at Linear and clear it,
/// **keeping** the OAuth client id/secret so reconnecting is one click. Use the
/// credential's Clear button to remove those too.
pub async fn disconnect(Extension(state): Extension<Arc<RunsState>>) -> Response {
    // Until the connection-management API lands, these routes address the
    // legacy connection — which is the only one an existing install has.
    let conn = ConnectionId::default();
    let store = match state.cred_store().await {
        Ok(s) => s,
        Err(e) => return err(StatusCode::SERVICE_UNAVAILABLE, e),
    };
    let current = credential(store, &conn).await.unwrap_or_default();
    if let Some(token) = field(&current, "access_token") {
        revoke_token(&token).await;
    }
    // `set` merges, so blanking is how a field is cleared.
    let cleared = BTreeMap::from([
        ("access_token".to_string(), String::new()),
        ("refresh_token".to_string(), String::new()),
        ("expires_at_ms".to_string(), String::new()),
        ("scope".to_string(), String::new()),
        ("actor".to_string(), String::new()),
        ("workspace_id".to_string(), String::new()),
        ("workspace_name".to_string(), String::new()),
        ("workspace_url_key".to_string(), String::new()),
        ("refresh_error".to_string(), String::new()),
        ("app_user_id".to_string(), String::new()),
    ]);
    match store.set(&conn.provider_key(), &cleared).await {
        Ok(()) => Json(serde_json::json!({ "disconnected": true })).into_response(),
        Err(e) => err(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn authorize_url_requests_app_actor_and_encodes_params() {
        let url = build_authorize_url(
            "cid-1",
            "https://harness.example.com/api/linear/oauth/callback",
            "nonce-1",
        );
        // The reason this feature exists: without actor=app, Linear attributes
        // every write to the installing user.
        assert!(url.contains("&actor=app"), "{url}");
        assert!(url.contains("response_type=code"), "{url}");
        assert!(url.contains("client_id=cid-1"), "{url}");
        // Comma-separated scopes, percent-encoded — including the agent scopes
        // that make the app delegatable and mentionable.
        assert!(
            url.contains("scope=read%2Cwrite%2Capp%3Aassignable%2Capp%3Amentionable"),
            "{url}"
        );
        // The redirect URI must survive encoding intact.
        assert!(
            url.contains(
                "redirect_uri=https%3A%2F%2Fharness.example.com%2Fapi%2Flinear%2Foauth%2Fcallback"
            ),
            "{url}"
        );
        assert!(url.contains("state=nonce-1"), "{url}");
        // `admin` is deliberately never requested.
        assert!(!url.contains("admin"), "{url}");
    }

    #[test]
    fn agent_scope_detection_reads_granted_scopes() {
        // Linear returns granted scopes space-separated.
        assert!(has_agent_scopes(Some(
            "read write app:assignable app:mentionable"
        )));
        // Commas tolerated, order irrelevant, extras fine.
        assert!(has_agent_scopes(Some(
            "app:mentionable,app:assignable,read,write,customer:read"
        )));
        // An install from before the agent scopes were requested: works for the
        // poller, cannot be delegated to → the UI must prompt a reconnect.
        assert!(!has_agent_scopes(Some("read write")));
        assert!(!has_agent_scopes(Some("read write app:assignable")));
        assert!(!has_agent_scopes(Some("")));
        assert!(!has_agent_scopes(None));
    }

    #[test]
    fn enc_leaves_unreserved_and_escapes_the_rest() {
        assert_eq!(enc("aZ09-_.~"), "aZ09-_.~");
        assert_eq!(enc("a b"), "a%20b");
        assert_eq!(enc("read,write"), "read%2Cwrite");
        assert_eq!(enc("https://x/y?z=1&w"), "https%3A%2F%2Fx%2Fy%3Fz%3D1%26w");
    }

    #[test]
    fn state_nonce_is_single_use() {
        let a = issue_state();
        let b = issue_state();
        assert_ne!(a, b, "each authorization gets its own nonce");
        assert!(take_state(&a));
        // Replaying it must fail — this is the callback's only authentication.
        assert!(!take_state(&a));
        assert!(!take_state("never-issued"));
        assert!(take_state(&b));
    }

    #[test]
    fn expired_state_nonce_is_rejected() {
        let nonce = issue_state();
        // Backdate it past the TTL.
        if let Ok(mut map) = PENDING.lock() {
            if let Some(created) = map.get_mut(&nonce) {
                *created -= PENDING_TTL_MS + 1;
            }
        }
        assert!(!take_state(&nonce));
    }

    #[test]
    fn parse_token_response_reads_full_payload() {
        let body = br#"{"access_token":"lin_oauth_a","token_type":"Bearer",
            "expires_in":86399,"scope":"read write","refresh_token":"lin_ref_b"}"#;
        let t = parse_token_response(body).unwrap();
        assert_eq!(t.access_token, "lin_oauth_a");
        assert_eq!(t.refresh_token.as_deref(), Some("lin_ref_b"));
        assert_eq!(t.expires_in_secs, 86399);
        assert_eq!(t.scope, "read write");
    }

    #[test]
    fn parse_token_response_handles_missing_refresh_and_expiry() {
        // Apps without refresh tokens enabled get a long-lived access token.
        let t = parse_token_response(br#"{"access_token":"tok"}"#).unwrap();
        assert_eq!(t.refresh_token, None);
        assert_eq!(t.expires_in_secs, DEFAULT_EXPIRES_IN_SECS);
        // A zero/negative expiry would make every call refresh; fall back too.
        let t = parse_token_response(br#"{"access_token":"tok","expires_in":0}"#).unwrap();
        assert_eq!(t.expires_in_secs, DEFAULT_EXPIRES_IN_SECS);
    }

    #[test]
    fn parse_token_response_surfaces_oauth_errors() {
        let e = parse_token_response(
            br#"{"error":"invalid_grant","error_description":"refresh token is invalid"}"#,
        )
        .unwrap_err();
        assert!(e.contains("invalid_grant"), "{e}");
        assert!(e.contains("refresh token is invalid"), "{e}");
        // No access_token and no error field is still an error, not a panic.
        assert!(parse_token_response(br#"{"token_type":"Bearer"}"#).is_err());
        assert!(parse_token_response(b"not json").is_err());
    }

    #[test]
    fn token_fields_stamp_actor_and_absolute_expiry() {
        let before = now_ms();
        let fields = token_fields(&TokenSet {
            access_token: "a".into(),
            refresh_token: Some("r".into()),
            expires_in_secs: 3600,
            scope: "read,write".into(),
        });
        assert_eq!(fields["access_token"], "a");
        assert_eq!(fields["refresh_token"], "r");
        assert_eq!(fields["actor"], "app");
        let exp: i64 = fields["expires_at_ms"].parse().unwrap();
        assert!(exp >= before + 3600 * 1000, "expiry is absolute, in ms");
        // No refresh token stored when Linear didn't issue one.
        let fields = token_fields(&TokenSet {
            access_token: "a".into(),
            refresh_token: None,
            expires_in_secs: 60,
            scope: String::new(),
        });
        assert!(!fields.contains_key("refresh_token"));
    }

    #[test]
    fn usable_auth_requires_a_non_empty_token_or_key() {
        let f = |pairs: &[(&str, &str)]| {
            pairs
                .iter()
                .map(|(k, v)| (k.to_string(), v.to_string()))
                .collect::<BTreeMap<_, _>>()
        };
        assert!(has_usable_auth(&f(&[("access_token", "t")])));
        assert!(has_usable_auth(&f(&[("api_key", "k")])));
        // Client details alone are configuration, not credentials.
        assert!(!has_usable_auth(&f(&[
            ("client_id", "c"),
            ("client_secret", "s")
        ])));
        // A disconnected row keeps its (blank) keys — must not read as usable.
        assert!(!has_usable_auth(&f(&[
            ("access_token", ""),
            ("api_key", "")
        ])));
        assert!(!has_usable_auth(&BTreeMap::new()));
    }

    #[test]
    fn callback_path_matches_the_built_redirect_uri() {
        // Guard: the registered callback, the auth exemption and the route must
        // agree — a mismatch fails the OAuth exchange with an opaque error.
        assert_eq!(CALLBACK_PATH, "/api/linear/oauth/callback");
        assert!(super::super::auth::is_auth_exempt_path(CALLBACK_PATH));
    }

    #[test]
    fn back_to_ui_redirects_to_credentials_with_encoded_detail() {
        let resp = back_to_ui("connected", Some("Acme Inc"));
        assert_eq!(resp.status(), StatusCode::SEE_OTHER);
        let loc = resp
            .headers()
            .get(header::LOCATION)
            .and_then(|v| v.to_str().ok())
            .unwrap();
        assert_eq!(
            loc,
            "/credentials?linear=connected&linear_message=Acme%20Inc"
        );
    }

    #[test]
    fn oauth_endpoint_defaults() {
        for var in [
            "HARNESS_LINEAR_OAUTH_AUTHORIZE_URL",
            "HARNESS_LINEAR_OAUTH_TOKEN_URL",
            "HARNESS_LINEAR_OAUTH_REVOKE_URL",
        ] {
            std::env::remove_var(var);
        }
        assert_eq!(authorize_url(), "https://linear.app/oauth/authorize");
        assert_eq!(token_url(), "https://api.linear.app/oauth/token");
        assert_eq!(revoke_url(), "https://api.linear.app/oauth/revoke");
    }
}
