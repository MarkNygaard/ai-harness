//! Provider-credential API + run-time materialization.
//!
//! Credentials (Claude OAuth, Codex `auth.json`, the Kimi API key) are entered
//! in the UI, stored **encrypted** in Postgres (`harness_persist::CredentialStore`),
//! and never placed in cluster Secrets/SOPS. [`materialize`] injects them into the
//! agent environment just before a real run: token-style values become env vars,
//! file-style values are written into `$HOME` where the CLIs read them.
//!
//! - `GET    /api/credentials`            — which providers are configured (no secrets)
//! - `PUT    /api/credentials/{provider}` — set a provider's fields
//! - `DELETE /api/credentials/{provider}` — clear a provider

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::Arc;

use axum::extract::{Extension, Path as AxumPath};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::Json;
use harness_persist::{CredentialStore, ProviderCredential};
use serde::Deserialize;

use super::accounts::AdminOnly;
use super::runs_routes::RunsState;

/// Providers the UI can configure (the agent backends, GitHub for repo access,
/// and Linear — whose fields are the OAuth app's `client_id`/`client_secret`,
/// with the tokens themselves written by the `actor=app` connect flow in
/// [`super::linear_oauth`]).
///
/// `linear` here is the *first* Linear account. Additional ones store their own
/// OAuth application under `linear:<id>` and are surfaced by the connections
/// API rather than as provider cards — see [`is_known_provider`].
const PROVIDERS: &[&str] = &["claude", "codex", "pi", "github", "cursor", "linear"];

/// Whether `provider` is one the UI may write to.
///
/// Beyond the fixed list, every named Linear connection has its own credential
/// key (`linear:<id>`) holding that account's OAuth app details. Those are not
/// listed as provider cards — the connections API surfaces them — but they must
/// be writable, since that is where a second account's `client_id`,
/// `client_secret` and `webhook_secret` are saved.
fn is_known_provider(provider: &str) -> bool {
    PROVIDERS.contains(&provider)
        || super::linear_connections::ConnectionId::from_provider_key(provider).is_some()
}

fn err(status: StatusCode, msg: impl Into<String>) -> Response {
    (status, Json(serde_json::json!({ "error": msg.into() }))).into_response()
}

/// Whether a provider's dashboard usage card is enabled (default: shown). Stored
/// as a non-secret `show_usage_card` field in the credential blob; absent — or
/// any value other than `"false"` — means shown.
pub(crate) async fn usage_card_visible(store: &CredentialStore, provider: &str) -> bool {
    store
        .get(provider)
        .await
        .ok()
        .flatten()
        .and_then(|f| f.get("show_usage_card").map(|v| v != "false"))
        .unwrap_or(true)
}

/// `GET /api/credentials` — list providers + whether each is configured.
/// Never returns secret values.
pub async fn list_credentials(
    _: AdminOnly,
    Extension(state): Extension<Arc<RunsState>>,
) -> Response {
    let store = match state.cred_store().await {
        Ok(s) => s,
        Err(e) => return err(StatusCode::SERVICE_UNAVAILABLE, e),
    };
    let configured = match store.list_configured().await {
        Ok(c) => c,
        Err(e) => return err(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()),
    };
    // Also count a provider as configured when the credential the CLIs actually
    // use is present on disk, even if the encrypted store has no row (e.g. Kimi
    // connected before the store persisted it, or the DB volume was reset while
    // the agent's auth files on the PV survived). Otherwise the badge can read
    // "not connected" for a provider that runs use fine.
    let mut out: Vec<ProviderCredential> = PROVIDERS
        .iter()
        .map(|p| ProviderCredential {
            provider: p.to_string(),
            configured: configured.iter().any(|c| c == p),
            show_usage_card: true,
        })
        .collect();
    for c in out.iter_mut() {
        if !c.configured {
            c.configured = provider_native_present(&c.provider).await;
        }
        c.show_usage_card = usage_card_visible(store, &c.provider).await;
    }
    Json(out).into_response()
}

/// Whether the credential a provider's CLI actually reads is present on disk —
/// the source of truth for "connected" independent of the encrypted store.
async fn provider_native_present(provider: &str) -> bool {
    let home = home_dir();
    match provider {
        "pi" => {
            crate::http::kimi_routes::agent_db_has_provider(
                home.join(".omp").join("agent").join("agent.db"),
                &["kimi-code", "openai-codex"],
            )
            .await
        }
        // Codex: ChatGPT OAuth tokens / api key live in auth.json.
        "codex" => file_non_empty(home.join(".codex").join("auth.json")),
        // Claude: the CLI reads ~/.claude/.credentials.json.
        "claude" => file_non_empty(home.join(".claude").join(".credentials.json")),
        _ => false,
    }
}

fn file_non_empty(path: PathBuf) -> bool {
    std::fs::metadata(&path)
        .map(|m| m.len() > 2)
        .unwrap_or(false)
}

/// Which agent CLIs have a usable credential **on disk** right now — the same
/// source of truth as [`provider_native_present`], shaped as the
/// [`ConnectedCreds`](harness_runner::authoring::ConnectedCreds) the authoring
/// catalog needs to gate its per-CLI model lists.
pub async fn connected_clis() -> harness_runner::authoring::ConnectedCreds {
    let home = home_dir();
    let agent_db = home.join(".omp").join("agent").join("agent.db");
    // "Codex (ChatGPT)" powers both the Codex CLI (auth.json) and omp's
    // `openai-codex`; treat either as connected.
    let codex = file_non_empty(home.join(".codex").join("auth.json"))
        || crate::http::kimi_routes::agent_db_has_provider(agent_db.clone(), &["openai-codex"])
            .await;
    let kimi = crate::http::kimi_routes::agent_db_has_provider(agent_db, &["kimi-code"]).await;
    let claude = file_non_empty(home.join(".claude").join(".credentials.json"));
    // Cursor CLI is usable when an API key is in the environment (materialized
    // from the stored credential / set by the operator) or an interactive
    // `cursor-agent login` left its config on disk.
    let cursor = std::env::var_os("CURSOR_API_KEY").is_some_and(|v| !v.is_empty())
        || file_non_empty(home.join(".cursor").join("cli-config.json"));
    harness_runner::authoring::ConnectedCreds {
        codex,
        kimi,
        claude,
        cursor,
    }
}

#[derive(Debug, Deserialize)]
pub struct SetCredentialRequest {
    /// `field → value` map (e.g. `oauth_token`, `auth_json`, `moonshot_api_key`).
    pub fields: BTreeMap<String, String>,
}

/// `PUT /api/credentials/{provider}` — store a provider's credential fields.
pub async fn set_credential(
    _: AdminOnly,
    Extension(state): Extension<Arc<RunsState>>,
    AxumPath(provider): AxumPath<String>,
    Json(req): Json<SetCredentialRequest>,
) -> Response {
    if !is_known_provider(&provider) {
        return err(
            StatusCode::BAD_REQUEST,
            format!("unknown provider `{provider}`"),
        );
    }
    if req.fields.is_empty() {
        return err(StatusCode::BAD_REQUEST, "no fields provided");
    }
    let store = match state.cred_store().await {
        Ok(s) => s,
        Err(e) => return err(StatusCode::SERVICE_UNAVAILABLE, e),
    };
    match store.set(&provider, &req.fields).await {
        Ok(()) => {
            // Write-through on paste: a freshly pasted credential must take effect
            // and REPLACE any stale on-disk file now — because `materialize` only
            // seeds-if-missing (it won't clobber the CLI's self-refreshing file).
            // This is the one authoritative point where the DB copy overwrites
            // disk; thereafter the CLI owns and rotates the token in place.
            let home = home_dir();
            match provider.as_str() {
                "claude" => {
                    if let Some(json) = req.fields.get("credentials_json").filter(|v| !v.is_empty())
                    {
                        write_secret_file(home.join(".claude").join(".credentials.json"), json);
                    }
                }
                "codex" => {
                    if let Some(json) = req.fields.get("auth_json").filter(|v| !v.is_empty()) {
                        write_secret_file(
                            home.join(".codex").join("auth.json"),
                            &strip_codex_auth_mode(json),
                        );
                    }
                }
                _ => {}
            }
            // A usage-card visibility toggle takes effect on the next dashboard
            // poll rather than after the ~3-min usage cache expires.
            super::usage_routes::invalidate_cache().await;
            Json(serde_json::json!({ "saved": true, "provider": provider })).into_response()
        }
        Err(e) => err(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()),
    }
}

/// `DELETE /api/credentials/{provider}` — clear a provider's credential.
pub async fn delete_credential(
    _: AdminOnly,
    Extension(state): Extension<Arc<RunsState>>,
    AxumPath(provider): AxumPath<String>,
) -> Response {
    let store = match state.cred_store().await {
        Ok(s) => s,
        Err(e) => return err(StatusCode::SERVICE_UNAVAILABLE, e),
    };
    match store.delete(&provider).await {
        Ok(()) => {
            // Also remove the materialized on-disk file so "Clear" actually
            // clears everywhere. The CLIs — and the usage probe + the "connected"
            // badge (`provider_native_present`) — read these files directly, and
            // `materialize` only seeds them when missing, so leaving the file
            // makes Clear a no-op for auth (the stale credential keeps being
            // used). Best-effort; absence is fine.
            let home = home_dir();
            match provider.as_str() {
                "claude" => {
                    let _ = std::fs::remove_file(home.join(".claude").join(".credentials.json"));
                }
                "codex" => {
                    let _ = std::fs::remove_file(home.join(".codex").join("auth.json"));
                }
                _ => {}
            }
            super::usage_routes::invalidate_cache().await;
            Json(serde_json::json!({ "deleted": true, "provider": provider })).into_response()
        }
        Err(e) => err(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()),
    }
}

// ── Per-project credentials (github only; project-first, global fallback) ────

/// Providers that may be overridden **per project** — the external integrations
/// whose account can differ by project. AI provider keys stay global only.
///
/// **Linear is deliberately not here.** Linear accounts are a shared registry
/// that projects *point into* (`harness_projects.linear_connection`), not
/// per-project overrides of one credential — several projects normally share an
/// account, and the identity connected is the app, not the project. See
/// [`super::linear_connections`]. Any per-project `linear` row left over from an
/// earlier version is inert: nothing reads it.
const PROJECT_PROVIDERS: &[&str] = &["github"];

/// `GET /api/projects/{project}/credentials` — which per-project providers are
/// configured for this project (no secrets).
pub async fn list_project_credentials(
    _: AdminOnly,
    Extension(state): Extension<Arc<RunsState>>,
    AxumPath(project): AxumPath<String>,
) -> Response {
    let store = match state.cred_store().await {
        Ok(s) => s,
        Err(e) => return err(StatusCode::SERVICE_UNAVAILABLE, e),
    };
    let configured = match store.list_project_configured(&project).await {
        Ok(c) => c,
        Err(e) => return err(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()),
    };
    let out: Vec<ProviderCredential> = PROJECT_PROVIDERS
        .iter()
        .map(|p| ProviderCredential {
            provider: p.to_string(),
            configured: configured.iter().any(|c| c == p),
            // Per-project providers (github) have no usage card.
            show_usage_card: true,
        })
        .collect();
    Json(out).into_response()
}

/// `PUT /api/projects/{project}/credentials/{provider}` — set a project-scoped
/// credential (allowlisted to `github`).
pub async fn set_project_credential(
    _: AdminOnly,
    Extension(state): Extension<Arc<RunsState>>,
    AxumPath((project, provider)): AxumPath<(String, String)>,
    Json(req): Json<SetCredentialRequest>,
) -> Response {
    if !PROJECT_PROVIDERS.contains(&provider.as_str()) {
        return err(
            StatusCode::BAD_REQUEST,
            format!("provider `{provider}` is not configurable per project (allowed: github)"),
        );
    }
    if req.fields.is_empty() {
        return err(StatusCode::BAD_REQUEST, "no fields provided");
    }
    let store = match state.cred_store().await {
        Ok(s) => s,
        Err(e) => return err(StatusCode::SERVICE_UNAVAILABLE, e),
    };
    match store.set_project(&project, &provider, &req.fields).await {
        Ok(()) => {
            Json(serde_json::json!({ "saved": true, "project": project, "provider": provider }))
                .into_response()
        }
        Err(e) => err(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()),
    }
}

/// `DELETE /api/projects/{project}/credentials/{provider}` — clear a
/// project-scoped credential (falls back to the global one thereafter).
pub async fn delete_project_credential(
    _: AdminOnly,
    Extension(state): Extension<Arc<RunsState>>,
    AxumPath((project, provider)): AxumPath<(String, String)>,
) -> Response {
    let store = match state.cred_store().await {
        Ok(s) => s,
        Err(e) => return err(StatusCode::SERVICE_UNAVAILABLE, e),
    };
    match store.delete_project(&project, &provider).await {
        Ok(()) => {
            Json(serde_json::json!({ "deleted": true, "project": project, "provider": provider }))
                .into_response()
        }
        Err(e) => err(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()),
    }
}

// ── Per-project build environment variables ─────────────────────────────────
//
// Free-form KEY=VALUE pairs injected into a run's process environment, so
// build/codegen (Next.js, drizzle, .NET, …) can read them — process env is the
// universal delivery, so no per-stack dotenv file is written. Encrypted at rest
// in the project credential store under provider `env`
// (the whole map serialized as one JSON field, so a save replaces the set).
// Unlike the other credentials these ARE returned to the editor (viewable +
// editable), by deliberate choice — they're a project's own build config.

/// Provider key under which a project's env map is stored.
const ENV_PROVIDER: &str = "env";
/// The single field holding the JSON-encoded env map (one field ⇒ save replaces).
const ENV_FIELD: &str = "vars";

#[derive(Deserialize)]
pub struct ProjectEnvRequest {
    pub vars: BTreeMap<String, String>,
}

/// A project's stored build env vars (decrypted) — empty map if none set.
pub(crate) async fn project_env_vars(
    store: &CredentialStore,
    project: &str,
) -> BTreeMap<String, String> {
    store
        .get_project(project, ENV_PROVIDER)
        .await
        .ok()
        .flatten()
        .and_then(|f| f.get(ENV_FIELD).cloned())
        .and_then(|j| serde_json::from_str(&j).ok())
        .unwrap_or_default()
}

/// `GET /api/projects/{project}/env` — the project's env vars, values included.
pub async fn list_project_env(
    _: AdminOnly,
    Extension(state): Extension<Arc<RunsState>>,
    AxumPath(project): AxumPath<String>,
) -> Response {
    let store = match state.cred_store().await {
        Ok(s) => s,
        Err(e) => return err(StatusCode::SERVICE_UNAVAILABLE, e),
    };
    let vars = project_env_vars(store, &project).await;
    Json(serde_json::json!({ "vars": vars })).into_response()
}

/// `PUT /api/projects/{project}/env` — replace the project's env vars wholesale.
pub async fn set_project_env(
    _: AdminOnly,
    Extension(state): Extension<Arc<RunsState>>,
    AxumPath(project): AxumPath<String>,
    Json(req): Json<ProjectEnvRequest>,
) -> Response {
    let store = match state.cred_store().await {
        Ok(s) => s,
        Err(e) => return err(StatusCode::SERVICE_UNAVAILABLE, e),
    };
    // An empty set clears the credential entirely.
    if req.vars.is_empty() {
        return match store.delete_project(&project, ENV_PROVIDER).await {
            Ok(()) => Json(serde_json::json!({ "saved": true, "count": 0 })).into_response(),
            Err(e) => err(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()),
        };
    }
    let json = match serde_json::to_string(&req.vars) {
        Ok(j) => j,
        Err(e) => return err(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()),
    };
    // One field only ⇒ set_project's field-merge replaces the whole map.
    let fields = BTreeMap::from([(ENV_FIELD.to_string(), json)]);
    match store.set_project(&project, ENV_PROVIDER, &fields).await {
        Ok(()) => {
            Json(serde_json::json!({ "saved": true, "count": req.vars.len() })).into_response()
        }
        Err(e) => err(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()),
    }
}

/// `DELETE /api/projects/{project}/env` — clear all of a project's env vars.
pub async fn delete_project_env(
    _: AdminOnly,
    Extension(state): Extension<Arc<RunsState>>,
    AxumPath(project): AxumPath<String>,
) -> Response {
    let store = match state.cred_store().await {
        Ok(s) => s,
        Err(e) => return err(StatusCode::SERVICE_UNAVAILABLE, e),
    };
    match store.delete_project(&project, ENV_PROVIDER).await {
        Ok(()) => Json(serde_json::json!({ "deleted": true, "project": project })).into_response(),
        Err(e) => err(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()),
    }
}

fn home_dir() -> PathBuf {
    std::env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("/home/harness"))
}

/// Remove a stale top-level `auth_mode` field from a codex `auth.json` blob.
/// The codex CLI parses `auth_mode` as an enum and rejects the old `"ChatGPT"`
/// value; the canonical file omits it (mode is inferred from `tokens`). Returns
/// the input unchanged if it isn't a JSON object.
fn strip_codex_auth_mode(json: &str) -> String {
    match serde_json::from_str::<serde_json::Value>(json) {
        Ok(serde_json::Value::Object(mut map)) if map.contains_key("auth_mode") => {
            map.remove("auth_mode");
            serde_json::Value::Object(map).to_string()
        }
        _ => json.to_string(),
    }
}

fn write_secret_file(path: PathBuf, contents: &str) {
    if let Some(dir) = path.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    if let Err(e) = std::fs::write(&path, contents) {
        tracing::warn!("failed to write credential file {}: {e}", path.display());
    }
}

/// Inject stored credentials into the agent environment for a real run:
/// - **claude**: `oauth_token` → `CLAUDE_CODE_OAUTH_TOKEN`; `credentials_json`
///   → `$HOME/.claude/.credentials.json`.
/// - **codex**: `auth_json` → `$HOME/.codex/auth.json`.
/// - **pi**: `kimi_oauth` (from the "Connect Kimi" device login) → re-seeds omp's
///   `~/.omp/agent/agent.db` if missing (for `kimi-code/*` models); `moonshot_api_key`
///   → `MOONSHOT_API_KEY` (per-token Moonshot API, `moonshotai/*`).
/// - **github**: `token` → `GH_TOKEN` + `GITHUB_TOKEN` (so `gh` + git push work).
///
/// Best-effort: missing providers/fields are skipped. Subprocesses spawned by
/// the agent adapters inherit these (the control plane is single-operator, and
/// materialization runs at the start of each real run).
pub async fn materialize(store: &CredentialStore) {
    let home = home_dir();

    if let Ok(Some(claude)) = store.get("claude").await {
        if let Some(token) = claude.get("oauth_token").filter(|v| !v.is_empty()) {
            std::env::set_var("CLAUDE_CODE_OAUTH_TOKEN", token);
        }
        if let Some(json) = claude.get("credentials_json").filter(|v| !v.is_empty()) {
            // SEED ONLY IF MISSING — never clobber a file the CLI is already
            // managing. Claude Code refreshes the access token on each run and
            // **rotates** the (single-use) refresh token in place; overwriting it
            // with the stale DB copy each run re-presents a consumed refresh token
            // and 401s within a day. A fresh paste still takes effect because
            // `set_credential` write-throughs to this file. (Mirrors the Kimi
            // `reseed_agent_db_if_missing` pattern below.)
            let path = home.join(".claude").join(".credentials.json");
            if !file_non_empty(path.clone()) {
                write_secret_file(path, json);
            }
        }
    }
    if let Ok(Some(codex)) = store.get("codex").await {
        if let Some(json) = codex.get("auth_json").filter(|v| !v.is_empty()) {
            // Seed-if-missing, same as claude above: the codex CLI also rotates
            // its refresh token in `auth.json`, so clobbering it each run breaks
            // auth. Defensive `strip_codex_auth_mode`: older Connect Codex wrote
            // an `auth_mode` field the codex CLI rejects ("unknown variant
            // ChatGPT") — strip it so pre-fix credentials still work.
            let path = home.join(".codex").join("auth.json");
            if !file_non_empty(path.clone()) {
                write_secret_file(path, &strip_codex_auth_mode(json));
            }
        }
    }
    if let Ok(Some(pi)) = store.get("pi").await {
        // Kimi-for-Coding (kimi-code/* models) is an OAuth device login, not an
        // API key — the "Connect Kimi" flow stores the credential and writes omp's
        // ~/.omp/agent/agent.db. Re-seed that db from the stored credential if the
        // volume lost it (omp self-refreshes the tokens thereafter).
        crate::http::kimi_routes::reseed_agent_db_if_missing(&pi).await;
        // Per-token Moonshot API (moonshotai/* models) — a real API key.
        if let Some(key) = pi.get("moonshot_api_key").filter(|v| !v.is_empty()) {
            std::env::set_var("MOONSHOT_API_KEY", key);
        }
    }
    if let Ok(Some(gh)) = store.get("github").await {
        if let Some(token) = gh.get("token").filter(|v| !v.is_empty()) {
            std::env::set_var("GH_TOKEN", token);
            std::env::set_var("GITHUB_TOKEN", token);
        }
    }
    if let Ok(Some(cursor)) = store.get("cursor").await {
        // Cursor CLI (`cursor-agent`) reads CURSOR_API_KEY for headless auth — a
        // user API key from the Cursor dashboard.
        if let Some(key) = cursor.get("api_key").filter(|v| !v.is_empty()) {
            std::env::set_var("CURSOR_API_KEY", key);
        }
    }

    // Keep the Claude subscription token fresh for this run: its OAuth access
    // token is short-lived (~8h) and the CLI doesn't reliably refresh it in
    // headless subscription runs, so an idle gap would otherwise 401 the run
    // mid-pipeline. No-op when there's no Claude credential or it's still valid.
    let _ = ensure_fresh_claude_token(store).await;
}

// ── Claude OAuth token refresh (keep-warm) ───────────────────────────────────
//
// The Claude Code subscription credential (`~/.claude/.credentials.json`) is a
// short-lived (~8h) OAuth access token plus a single-use refresh token. The CLI
// is meant to refresh on each run, but headless subscription runs don't reliably
// do so — the access token expires and both agent runs and the usage card 401
// until a human re-pastes. We refresh it ourselves: when the token is read (the
// usage probe) or a run starts, if it's within `CLAUDE_REFRESH_SKEW_MS` of expiry
// we exchange the refresh token for a new pair and write it back to disk AND the
// DB (so the seed-if-missing fallback is never stale). The endpoint + client_id
// are verified against the installed CLI; both are env-overridable in case
// Anthropic moves them.

/// Refresh once the access token has less than this before expiry (ms).
const CLAUDE_REFRESH_SKEW_MS: i64 = 5 * 60 * 1000;

/// Claude Code OAuth token endpoint (verified: `platform.claude.com`).
fn claude_oauth_token_url() -> String {
    std::env::var("HARNESS_CLAUDE_OAUTH_TOKEN_URL")
        .ok()
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "https://platform.claude.com/v1/oauth/token".to_string())
}

/// Claude Code OAuth client id (verified present in the installed CLI).
fn claude_oauth_client_id() -> String {
    std::env::var("HARNESS_CLAUDE_OAUTH_CLIENT_ID")
        .ok()
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "9d1c250a-e61b-44d9-88ed-5944d1962f5e".to_string())
}

fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

/// Serialize refreshes so concurrent callers (usage probe + run start) can't
/// double-spend the single-use refresh token.
static CLAUDE_REFRESH_LOCK: std::sync::LazyLock<tokio::sync::Mutex<()>> =
    std::sync::LazyLock::new(|| tokio::sync::Mutex::new(()));

fn oauth_str(root: &serde_json::Value, key: &str) -> Option<String> {
    root.get("claudeAiOauth")?
        .get(key)?
        .as_str()
        .map(str::to_string)
}

fn oauth_expires_ms(root: &serde_json::Value) -> Option<i64> {
    root.get("claudeAiOauth")?.get("expiresAt")?.as_i64()
}

/// Load the current Claude credential JSON: the CLI's live on-disk copy first,
/// then the DB `credentials_json` fallback.
async fn load_claude_creds_json(store: &CredentialStore) -> Option<String> {
    let path = home_dir().join(".claude").join(".credentials.json");
    if let Ok(s) = std::fs::read_to_string(&path) {
        if !s.trim().is_empty() {
            return Some(s);
        }
    }
    store
        .get("claude")
        .await
        .ok()
        .flatten()?
        .get("credentials_json")
        .filter(|v| !v.is_empty())
        .cloned()
}

/// Overwrite accessToken/refreshToken/expiresAt in the parsed credential JSON,
/// preserving `scopes`/`subscriptionType` and any other fields. False if the
/// `claudeAiOauth` object is absent.
fn apply_refreshed_tokens(
    root: &mut serde_json::Value,
    access: &str,
    refresh: &str,
    expires_at_ms: i64,
) -> bool {
    let Some(oauth) = root
        .get_mut("claudeAiOauth")
        .and_then(|v| v.as_object_mut())
    else {
        return false;
    };
    oauth.insert(
        "accessToken".into(),
        serde_json::Value::String(access.into()),
    );
    oauth.insert(
        "refreshToken".into(),
        serde_json::Value::String(refresh.into()),
    );
    oauth.insert(
        "expiresAt".into(),
        serde_json::Value::Number(expires_at_ms.into()),
    );
    true
}

/// Ensure the Claude access token is fresh, refreshing via the OAuth refresh
/// token when it's within `CLAUDE_REFRESH_SKEW_MS` of expiry. Returns the current
/// (possibly just-refreshed) access token. Best-effort: on refresh failure it
/// logs and returns the existing token; `None` only when no credential exists.
pub(crate) async fn ensure_fresh_claude_token(store: &CredentialStore) -> Option<String> {
    let raw = load_claude_creds_json(store).await?;
    let root: serde_json::Value = serde_json::from_str(&raw).ok()?;
    let access = oauth_str(&root, "accessToken")?;
    match oauth_expires_ms(&root) {
        Some(exp) if exp - now_ms() > CLAUDE_REFRESH_SKEW_MS => return Some(access),
        // Unknown expiry → don't churn refreshes; use what we have.
        None => return Some(access),
        Some(_) => {} // expired or within the skew window → refresh below
    }

    let _guard = CLAUDE_REFRESH_LOCK.lock().await;
    // Re-read after locking: a concurrent caller may have just refreshed.
    let raw = load_claude_creds_json(store).await?;
    let mut root: serde_json::Value = serde_json::from_str(&raw).ok()?;
    let access = oauth_str(&root, "accessToken")?;
    if let Some(exp) = oauth_expires_ms(&root) {
        if exp - now_ms() > CLAUDE_REFRESH_SKEW_MS {
            return Some(access);
        }
    }
    let Some(refresh) = oauth_str(&root, "refreshToken") else {
        tracing::warn!("claude token refresh: credential has no refresh token");
        return Some(access);
    };

    match refresh_claude_oauth(&refresh).await {
        Ok((new_access, new_refresh, expires_in)) => {
            let new_expires = now_ms() + expires_in.max(0) * 1000;
            if apply_refreshed_tokens(&mut root, &new_access, &new_refresh, new_expires) {
                if let Ok(json) = serde_json::to_string(&root) {
                    write_secret_file(home_dir().join(".claude").join(".credentials.json"), &json);
                    // Write-back so the seed-if-missing fallback is never stale.
                    let _ = store
                        .set(
                            "claude",
                            &BTreeMap::from([("credentials_json".to_string(), json)]),
                        )
                        .await;
                }
            }
            tracing::info!("claude token refresh: succeeded (expires in {expires_in}s)");
            Some(new_access)
        }
        Err(e) => {
            tracing::warn!("claude token refresh failed: {e}");
            Some(access)
        }
    }
}

/// Exchange a refresh token for a new access+refresh pair at Claude's OAuth
/// token endpoint. Returns `(access_token, refresh_token, expires_in_secs)`.
async fn refresh_claude_oauth(refresh_token: &str) -> Result<(String, String, i64), String> {
    let resp = reqwest::Client::new()
        .post(claude_oauth_token_url())
        .json(&serde_json::json!({
            "grant_type": "refresh_token",
            "refresh_token": refresh_token,
            "client_id": claude_oauth_client_id(),
        }))
        .send()
        .await
        .map_err(|e| format!("request failed: {e}"))?;
    let status = resp.status();
    let body = resp.text().await.unwrap_or_default();
    if !status.is_success() {
        // Truncate so a token in an error body can't sprawl across logs.
        let snippet: String = body.chars().take(200).collect();
        return Err(format!("HTTP {status}: {snippet}"));
    }
    let v: serde_json::Value =
        serde_json::from_str(&body).map_err(|e| format!("bad JSON response: {e}"))?;
    let access = v
        .get("access_token")
        .and_then(|x| x.as_str())
        .ok_or_else(|| "response missing access_token".to_string())?
        .to_string();
    // Claude rotates the refresh token; keep the old one only if omitted.
    let refresh = v
        .get("refresh_token")
        .and_then(|x| x.as_str())
        .map(str::to_string)
        .unwrap_or_else(|| refresh_token.to_string());
    let expires_in = v
        .get("expires_in")
        .and_then(|x| x.as_i64())
        .unwrap_or(8 * 3600);
    Ok((access, refresh, expires_in))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn apply_refreshed_tokens_updates_and_preserves_siblings() {
        let mut root: serde_json::Value = serde_json::from_str(
            r#"{"claudeAiOauth":{"accessToken":"old","refreshToken":"oldR","expiresAt":1,"scopes":["a","b"],"subscriptionType":"team"}}"#,
        )
        .unwrap();
        assert!(apply_refreshed_tokens(&mut root, "newA", "newR", 999));
        let o = &root["claudeAiOauth"];
        assert_eq!(o["accessToken"], "newA");
        assert_eq!(o["refreshToken"], "newR");
        assert_eq!(o["expiresAt"], 999);
        // Untouched fields survive the rewrite.
        assert_eq!(o["subscriptionType"], "team");
        assert_eq!(o["scopes"], serde_json::json!(["a", "b"]));
    }

    #[test]
    fn apply_refreshed_tokens_false_without_oauth_block() {
        let mut root = serde_json::json!({ "other": 1 });
        assert!(!apply_refreshed_tokens(&mut root, "a", "b", 1));
    }

    #[test]
    fn oauth_helpers_extract_token_and_expiry() {
        let root: serde_json::Value = serde_json::from_str(
            r#"{"claudeAiOauth":{"accessToken":"tok","refreshToken":"r","expiresAt":1782821199284}}"#,
        )
        .unwrap();
        assert_eq!(oauth_str(&root, "accessToken").as_deref(), Some("tok"));
        assert_eq!(oauth_str(&root, "refreshToken").as_deref(), Some("r"));
        assert_eq!(oauth_expires_ms(&root), Some(1782821199284));
        // Missing pieces → None (drives the "unknown expiry, don't churn" path).
        assert_eq!(oauth_expires_ms(&serde_json::json!({})), None);
        assert_eq!(oauth_str(&serde_json::json!({}), "accessToken"), None);
    }

    #[test]
    fn oauth_endpoint_and_client_id_defaults() {
        // Defaults are the values verified against the installed CLI (env can
        // override, but a clean env yields these).
        std::env::remove_var("HARNESS_CLAUDE_OAUTH_TOKEN_URL");
        std::env::remove_var("HARNESS_CLAUDE_OAUTH_CLIENT_ID");
        assert_eq!(
            claude_oauth_token_url(),
            "https://platform.claude.com/v1/oauth/token"
        );
        assert_eq!(
            claude_oauth_client_id(),
            "9d1c250a-e61b-44d9-88ed-5944d1962f5e"
        );
    }
}
