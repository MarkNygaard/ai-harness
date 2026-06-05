//! "Connect Kimi" — the Kimi-for-Coding OAuth **device-authorization** flow
//! (RFC 8628), run server-side so the operator authenticates from the harness UI
//! instead of running `omp /login` locally.
//!
//! Mirrors `oh-my-pi`'s `loginKimi` exactly: POST `auth.kimi.com/api/oauth/
//! device_authorization` for a `user_code` + `device_code`, the operator approves
//! at the verification URL, then we poll `…/oauth/token` until approved. On
//! success we (a) store the credential in the encrypted store and (b) write the
//! omp-native `~/.omp/agent/agent.db` row so `omp` picks it up (and self-refreshes
//! thereafter). The UI drives the poll cadence, so the server stays stateless.

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use axum::extract::Extension;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::Json;
use serde::{Deserialize, Serialize};

use super::runs_routes::RunsState;

const CLIENT_ID: &str = "17e5f671-d194-4dfb-9706-5516cb48c098";
const OAUTH_HOST: &str = "https://auth.kimi.com";
const PROVIDER: &str = "kimi-code";
const EXPIRY_SKEW_MS: i64 = 5 * 60 * 1000;

fn err(status: StatusCode, msg: impl Into<String>) -> Response {
    (status, Json(serde_json::json!({ "error": msg.into() }))).into_response()
}

fn home_dir() -> PathBuf {
    std::env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("/home/harness"))
}

fn agent_db_path() -> PathBuf {
    home_dir().join(".omp/agent/agent.db")
}

fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

/// `X-Msh-*` headers omp sends. The device id is persisted to match omp's own
fn kimi_headers() -> reqwest::header::HeaderMap {
    use reqwest::header::{HeaderMap, HeaderName, HeaderValue};
    let device_id = device_id();
    let host = std::env::var("HOSTNAME").unwrap_or_else(|_| "ai-harness".to_string());
    let mut h = HeaderMap::new();
    let mut set = |k: &'static str, v: &str| {
        if let Ok(val) = HeaderValue::from_str(v) {
            h.insert(HeaderName::from_static(k), val);
        }
    };
    set("user-agent", "KimiCLI/ai-harness");
    set("x-msh-platform", "kimi_cli");
    set("x-msh-version", "ai-harness");
    set("x-msh-device-name", &host);
    set("x-msh-device-model", "Linux");
    set("x-msh-os-version", "Linux");
    set("x-msh-device-id", &device_id);
    h
}
/// Read or generate the persistent device id (UUID without hyphens), stored where
/// omp keeps it so a later omp invocation reuses the same identity.
fn device_id() -> String {
    let path = home_dir().join(".omp/agent/kimi-device-id");
    if let Ok(existing) = std::fs::read_to_string(&path) {
        if !existing.trim().is_empty() {
            return existing.trim().to_string();
        }
    }
    let id = uuid::Uuid::new_v4().simple().to_string();
    if let Some(dir) = path.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    let _ = std::fs::write(&path, format!("{id}\n"));
    id
}

// ── Device-authorization start ───────────────────────────────────────────────

#[derive(Deserialize)]
struct DeviceAuthResponse {
    device_code: Option<String>,
    verification_uri: Option<String>,
    verification_uri_complete: Option<String>,
    expires_in: Option<i64>,
    interval: Option<i64>,
}

#[derive(Serialize)]
pub struct ConnectStartResponse {
    pub user_code: String,
    pub verification_uri: String,
    pub device_code: String,
    pub interval: i64,
    pub expires_in: i64,
}

/// `POST /api/credentials/kimi/connect/start` — begin the device flow.
pub async fn connect_start(Extension(state): Extension<Arc<RunsState>>) -> Response {
    // Require the credential store so we can persist the result.
    if let Err(e) = state.cred_store().await {
        return err(StatusCode::SERVICE_UNAVAILABLE, e);
    }
    let client = reqwest::Client::new();
    let resp = client
        .post(format!("{OAUTH_HOST}/api/oauth/device_authorization"))
        .headers(kimi_headers())
        .form(&[("client_id", CLIENT_ID)])
        .send()
        .await;
    let resp = match resp {
        Ok(r) => r,
        Err(e) => {
            return err(
                StatusCode::BAD_GATEWAY,
                format!("device authorization request failed: {e}"),
            )
        }
    };
    if !resp.status().is_success() {
        let code = resp.status();
        let body = resp.text().await.unwrap_or_default();
        return err(
            StatusCode::BAD_GATEWAY,
            format!("device authorization failed: {code} {body}"),
        );
    }
    let payload: DeviceAuthResponse = match resp.json().await {
        Ok(p) => p,
        Err(e) => {
            return err(
                StatusCode::BAD_GATEWAY,
                format!("bad device authorization response: {e}"),
            )
        }
    };
    let (Some(user_code), Some(device_code), Some(verification_uri)) = (
        payload.user_code,
        payload.device_code,
        payload.verification_uri,
    ) else {
        return err(
            StatusCode::BAD_GATEWAY,
            "device authorization response missing fields",
        );
    };
    let verification = payload
        .verification_uri_complete
        .unwrap_or(verification_uri);
    Json(ConnectStartResponse {
        user_code,
        verification_uri: verification,
        device_code,
        interval: payload.interval.filter(|i| *i > 0).unwrap_or(5),
        expires_in: payload.expires_in.unwrap_or(900),
    })
    .into_response()
}

// ── Poll for approval ────────────────────────────────────────────────────────

#[derive(Deserialize, Default)]
struct TokenResponse {
    access_token: Option<String>,
    refresh_token: Option<String>,
    expires_in: Option<i64>,
    error: Option<String>,
    error_description: Option<String>,
}

#[derive(Deserialize)]
pub struct ConnectPollRequest {
    pub device_code: String,
}

/// `POST /api/credentials/kimi/connect/poll` — one token-endpoint poll. The UI
/// calls this on the device flow's `interval`. Returns `{status}`:
/// `pending` (keep polling), `connected` (done), or `error` (stop).
pub async fn connect_poll(
    Extension(state): Extension<Arc<RunsState>>,
    Json(req): Json<ConnectPollRequest>,
) -> Response {
    let store = match state.cred_store().await {
        Ok(s) => s,
        Err(e) => return err(StatusCode::SERVICE_UNAVAILABLE, e),
    };
    let client = reqwest::Client::new();
    let resp = client
        .post(format!("{OAUTH_HOST}/api/oauth/token"))
        .headers(kimi_headers())
        .form(&[
            ("client_id", CLIENT_ID),
            ("device_code", req.device_code.as_str()),
            ("grant_type", "urn:ietf:params:oauth:grant-type:device_code"),
        ])
        .send()
        .await;
    let resp = match resp {
        Ok(r) => r,
        Err(e) => return err(StatusCode::BAD_GATEWAY, format!("token poll failed: {e}")),
    };
    let ok = resp.status().is_success();
    let payload: TokenResponse = resp.json().await.unwrap_or_default();

    if ok {
        if let (Some(access), Some(expires_in)) = (payload.access_token.clone(), payload.expires_in)
        {
            let refresh = payload.refresh_token.unwrap_or_default();
            let expires = now_ms() + expires_in * 1000 - EXPIRY_SKEW_MS;
            // omp stores the oauth `data` JSON as the credential minus `type`.
            let data =
                serde_json::json!({ "access": access, "refresh": refresh, "expires": expires });
            let data_str = data.to_string();

            // Persist to the encrypted store (durable / re-seed) merged into `pi`.
            let mut fields = store.get("pi").await.ok().flatten().unwrap_or_default();
            fields.insert("kimi_oauth".to_string(), data_str.clone());
            if let Err(e) = store.set("pi", &fields).await {
                return err(
                    StatusCode::INTERNAL_SERVER_ERROR,
                    format!("store credential: {e}"),
                );
            }
            // Write the omp-native agent.db so omp uses it immediately.
            if let Err(e) = write_agent_db(&data_str).await {
                return err(
                    StatusCode::INTERNAL_SERVER_ERROR,
                    format!("write agent.db: {e}"),
                );
            }
            return Json(serde_json::json!({ "status": "connected" })).into_response();
        }
    }

    match payload.error.as_deref() {
        Some("authorization_pending") | Some("slow_down") => {
            Json(serde_json::json!({ "status": "pending" })).into_response()
        }
        Some("expired_token") => {
            Json(serde_json::json!({ "status": "error", "message": "code expired — start again" }))
                .into_response()
        }
        Some("access_denied") => {
            Json(serde_json::json!({ "status": "error", "message": "access denied" }))
                .into_response()
        }
        other => {
            let msg = payload
                .error_description
                .or_else(|| other.map(str::to_string))
                .unwrap_or_else(|| "unknown error".to_string());
            Json(serde_json::json!({ "status": "error", "message": msg })).into_response()
        }
    }
}

/// Write the Kimi-for-Coding credential into omp's `~/.omp/agent/agent.db`
/// (SQLite, schema v4) so `omp` reads it like a native `/login`. Replaces any
/// existing `kimi-code` row; omp self-refreshes the tokens from there on.
pub(crate) async fn write_agent_db(data_json: &str) -> Result<(), String> {
    let path = agent_db_path();
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).map_err(|e| format!("create dir: {e}"))?;
    }
    let opts = sqlx::sqlite::SqliteConnectOptions::new()
        .filename(&path)
        .create_if_missing(true);
    let pool = sqlx::SqlitePool::connect_with(opts)
        .await
        .map_err(|e| format!("open agent.db: {e}"))?;

    // Schema mirrors oh-my-pi's SqliteAuthCredentialStore (auth schema v4).
    for stmt in [
        "CREATE TABLE IF NOT EXISTS auth_schema_version (id INTEGER PRIMARY KEY CHECK (id = 1), version INTEGER NOT NULL)",
        "INSERT OR REPLACE INTO auth_schema_version(id, version) VALUES (1, 4)",
        "CREATE TABLE IF NOT EXISTS auth_credentials (\
            credential_type TEXT NOT NULL, data TEXT NOT NULL, \
            disabled_cause TEXT DEFAULT NULL, identity_key TEXT DEFAULT NULL, \
            created_at INTEGER NOT NULL DEFAULT (CAST(strftime('%s','now') AS INTEGER)), \
            updated_at INTEGER NOT NULL DEFAULT (CAST(strftime('%s','now') AS INTEGER)))",
        "CREATE INDEX IF NOT EXISTS idx_auth_provider ON auth_credentials(provider)",
        "CREATE INDEX IF NOT EXISTS idx_auth_provider_identity ON auth_credentials(provider, identity_key) WHERE identity_key IS NOT NULL",
        "DELETE FROM auth_credentials WHERE provider = 'kimi-code'",
    ] {
        sqlx::query(stmt)
            .execute(&pool)
            .await
            .map_err(|e| format!("agent.db schema: {e}"))?;
    }
    sqlx::query(
        "INSERT INTO auth_credentials (provider, credential_type, data, identity_key, created_at, updated_at) \
         VALUES (?, 'oauth', ?, NULL, CAST(strftime('%s','now') AS INTEGER), CAST(strftime('%s','now') AS INTEGER))",
    )
    .bind(PROVIDER)
    .bind(data_json)
    .execute(&pool)
    .await
    .map_err(|e| format!("agent.db insert: {e}"))?;
    pool.close().await;
    Ok(())
}

/// Re-seed `agent.db` from the stored `pi.kimi_oauth` if the file is missing
/// (e.g. a fresh volume). Called from credential materialization. Best-effort.
pub(crate) async fn reseed_agent_db_if_missing(fields: &BTreeMap<String, String>) {
    let path = agent_db_path();
    if path.exists() {
        return;
    }
    if let Some(data) = fields.get("kimi_oauth").filter(|v| !v.is_empty()) {
        if let Err(e) = write_agent_db(data).await {
            tracing::warn!("failed to reseed kimi agent.db: {e}");
        }
    }
}
