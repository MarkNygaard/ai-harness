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

use super::runs_routes::RunsState;

/// Providers the UI can configure (the agent backends + GitHub for repo access).
const PROVIDERS: &[&str] = &["claude", "codex", "pi", "github"];

fn err(status: StatusCode, msg: impl Into<String>) -> Response {
    (status, Json(serde_json::json!({ "error": msg.into() }))).into_response()
}

/// `GET /api/credentials` — list providers + whether each is configured.
/// Never returns secret values.
pub async fn list_credentials(Extension(state): Extension<Arc<RunsState>>) -> Response {
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
        })
        .collect();
    for c in out.iter_mut() {
        if !c.configured {
            c.configured = provider_native_present(&c.provider).await;
        }
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

#[derive(Debug, Deserialize)]
pub struct SetCredentialRequest {
    /// `field → value` map (e.g. `oauth_token`, `auth_json`, `moonshot_api_key`).
    pub fields: BTreeMap<String, String>,
}

/// `PUT /api/credentials/{provider}` — store a provider's credential fields.
pub async fn set_credential(
    Extension(state): Extension<Arc<RunsState>>,
    AxumPath(provider): AxumPath<String>,
    Json(req): Json<SetCredentialRequest>,
) -> Response {
    if !PROVIDERS.contains(&provider.as_str()) {
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
        Ok(()) => Json(serde_json::json!({ "saved": true, "provider": provider })).into_response(),
        Err(e) => err(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()),
    }
}

/// `DELETE /api/credentials/{provider}` — clear a provider's credential.
pub async fn delete_credential(
    Extension(state): Extension<Arc<RunsState>>,
    AxumPath(provider): AxumPath<String>,
) -> Response {
    let store = match state.cred_store().await {
        Ok(s) => s,
        Err(e) => return err(StatusCode::SERVICE_UNAVAILABLE, e),
    };
    match store.delete(&provider).await {
        Ok(()) => {
            Json(serde_json::json!({ "deleted": true, "provider": provider })).into_response()
        }
        Err(e) => err(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()),
    }
}

// ── Per-project credentials (linear / github only; project-first, global fallback) ──

/// Providers that may be overridden **per project** — the external integrations
/// whose account can differ by project. AI provider keys stay global only.
const PROJECT_PROVIDERS: &[&str] = &["linear", "github"];

/// `GET /api/projects/{project}/credentials` — which per-project providers are
/// configured for this project (no secrets).
pub async fn list_project_credentials(
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
        })
        .collect();
    Json(out).into_response()
}

/// `PUT /api/projects/{project}/credentials/{provider}` — set a project-scoped
/// credential (allowlisted to `linear` / `github`).
pub async fn set_project_credential(
    Extension(state): Extension<Arc<RunsState>>,
    AxumPath((project, provider)): AxumPath<(String, String)>,
    Json(req): Json<SetCredentialRequest>,
) -> Response {
    if !PROJECT_PROVIDERS.contains(&provider.as_str()) {
        return err(
            StatusCode::BAD_REQUEST,
            format!(
                "provider `{provider}` is not configurable per project (allowed: linear, github)"
            ),
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
            write_secret_file(home.join(".claude").join(".credentials.json"), json);
        }
    }
    if let Ok(Some(codex)) = store.get("codex").await {
        if let Some(json) = codex.get("auth_json").filter(|v| !v.is_empty()) {
            // Defensive: older Connect Codex wrote an `auth_mode` field the codex
            // CLI rejects ("unknown variant ChatGPT"). Strip it so credentials
            // stored before the fix still work without a reconnect.
            write_secret_file(
                home.join(".codex").join("auth.json"),
                &strip_codex_auth_mode(json),
            );
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
}
