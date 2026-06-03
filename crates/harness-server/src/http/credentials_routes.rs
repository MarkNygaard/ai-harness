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

/// Providers the UI can configure (matches the agent backends).
const PROVIDERS: &[&str] = &["claude", "codex", "pi"];

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
    let out: Vec<ProviderCredential> = PROVIDERS
        .iter()
        .map(|p| ProviderCredential {
            provider: p.to_string(),
            configured: configured.iter().any(|c| c == p),
        })
        .collect();
    Json(out).into_response()
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

fn home_dir() -> PathBuf {
    std::env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("/home/harness"))
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
/// - **pi**: `moonshot_api_key` → `MOONSHOT_API_KEY`.
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
            write_secret_file(home.join(".codex").join("auth.json"), json);
        }
    }
    if let Ok(Some(pi)) = store.get("pi").await {
        if let Some(key) = pi.get("moonshot_api_key").filter(|v| !v.is_empty()) {
            std::env::set_var("MOONSHOT_API_KEY", key);
        }
    }
}
