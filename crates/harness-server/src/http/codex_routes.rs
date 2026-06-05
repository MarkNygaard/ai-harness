//! "Connect Codex" — the OpenAI/ChatGPT OAuth **device-code** flow, run
//! server-side so the operator authenticates from the harness UI instead of
//! running `codex login` on the box.
//!
//! Mirrors `oh-my-pi`'s `loginOpenAICodexDevice` exactly (the same flow the
//! `codex` CLI's `--device-auth` uses): POST
//! `auth.openai.com/api/accounts/deviceauth/usercode` for a `user_code` +
//! `device_auth_id`; the operator approves at `auth.openai.com/codex/device`;
//! we poll `…/deviceauth/token` until it returns an `authorization_code` +
//! `code_verifier`, then exchange those at `…/oauth/token` for the ChatGPT
//! tokens. On success we (a) store the credential in the encrypted store under
//! `codex.auth_json` (durable / materialized on a fresh volume) and (b) write
//! codex's native `~/.codex/auth.json` so the `codex` CLI uses it immediately
//! (and self-refreshes thereafter). The UI drives the poll cadence, so the
//! server stays stateless — `connect_poll` carries the `device_auth_id`.

use std::path::PathBuf;
use std::sync::Arc;

use axum::extract::Extension;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::Json;
use base64::Engine as _;
use serde::{Deserialize, Serialize};

use super::runs_routes::RunsState;

const CLIENT_ID: &str = "app_EMoamEEZ73f0CkXaXp7hrann";
const USERCODE_URL: &str = "https://auth.openai.com/api/accounts/deviceauth/usercode";
const DEVICE_TOKEN_URL: &str = "https://auth.openai.com/api/accounts/deviceauth/token";
const OAUTH_TOKEN_URL: &str = "https://auth.openai.com/oauth/token";
const DEVICE_REDIRECT_URI: &str = "https://auth.openai.com/deviceauth/callback";
const VERIFICATION_URL: &str = "https://auth.openai.com/codex/device";
/// Custom JWT claim that carries the ChatGPT account id.
const JWT_CLAIM_PATH: &str = "https://api.openai.com/auth";

fn err(status: StatusCode, msg: impl Into<String>) -> Response {
    (status, Json(serde_json::json!({ "error": msg.into() }))).into_response()
}

fn home_dir() -> PathBuf {
    std::env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("/home/harness"))
}

// ── Device-authorization start ───────────────────────────────────────────────

#[derive(Deserialize, Default)]
struct UserCodeResponse {
    device_auth_id: Option<String>,
    user_code: Option<String>,
    /// OpenAI returns this as a string or a number depending on version.
    interval: Option<serde_json::Value>,
}

#[derive(Serialize)]
pub struct ConnectStartResponse {
    pub user_code: String,
    pub verification_uri: String,
    pub device_auth_id: String,
    pub interval: i64,
    pub expires_in: i64,
}

/// `POST /api/credentials/codex/connect/start` — begin the device flow.
pub async fn connect_start(Extension(state): Extension<Arc<RunsState>>) -> Response {
    if let Err(e) = state.cred_store().await {
        return err(StatusCode::SERVICE_UNAVAILABLE, e);
    }
    let client = reqwest::Client::new();
    let resp = client
        .post(USERCODE_URL)
        .json(&serde_json::json!({ "client_id": CLIENT_ID }))
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
    let payload: UserCodeResponse = match resp.json().await {
        Ok(p) => p,
        Err(e) => {
            return err(
                StatusCode::BAD_GATEWAY,
                format!("bad device authorization response: {e}"),
            )
        }
    };
    let (Some(device_auth_id), Some(user_code)) = (payload.device_auth_id, payload.user_code)
    else {
        return err(
            StatusCode::BAD_GATEWAY,
            "device authorization response missing fields",
        );
    };
    let interval = match payload.interval {
        Some(serde_json::Value::Number(n)) => n.as_i64().unwrap_or(5),
        Some(serde_json::Value::String(s)) => s.trim().parse().unwrap_or(5),
        _ => 5,
    }
    .max(1);
    Json(ConnectStartResponse {
        user_code,
        verification_uri: VERIFICATION_URL.to_string(),
        device_auth_id,
        interval,
        expires_in: 900,
    })
    .into_response()
}

// ── Poll for approval ────────────────────────────────────────────────────────

#[derive(Deserialize)]
pub struct ConnectPollRequest {
    pub device_auth_id: String,
    pub user_code: String,
}

#[derive(Deserialize, Default)]
struct DevicePollResponse {
    authorization_code: Option<String>,
    code_verifier: Option<String>,
}

#[derive(Deserialize, Default)]
struct TokenResponse {
    access_token: Option<String>,
    refresh_token: Option<String>,
    id_token: Option<String>,
    error: Option<String>,
    error_description: Option<String>,
}

/// `POST /api/credentials/codex/connect/poll` — one device-token poll. The UI
/// calls this on the flow's `interval`. Returns `{status}`: `pending` (keep
/// polling), `connected` (done), or `error` (stop).
pub async fn connect_poll(
    Extension(state): Extension<Arc<RunsState>>,
    Json(req): Json<ConnectPollRequest>,
) -> Response {
    let store = match state.cred_store().await {
        Ok(s) => s,
        Err(e) => return err(StatusCode::SERVICE_UNAVAILABLE, e),
    };
    let client = reqwest::Client::new();

    // 1) Poll the device-token endpoint. 403/404 = authorization pending.
    let resp = client
        .post(DEVICE_TOKEN_URL)
        .json(&serde_json::json!({
            "device_auth_id": req.device_auth_id,
            "user_code": req.user_code,
        }))
        .send()
        .await;
    let resp = match resp {
        Ok(r) => r,
        Err(e) => return err(StatusCode::BAD_GATEWAY, format!("device poll failed: {e}")),
    };
    let status = resp.status();
    if status.as_u16() == 403 || status.as_u16() == 404 {
        return Json(serde_json::json!({ "status": "pending" })).into_response();
    }
    if !status.is_success() {
        let body = resp.text().await.unwrap_or_default();
        return Json(
            serde_json::json!({ "status": "error", "message": format!("device poll failed: {status} {body}") }),
        )
        .into_response();
    }
    let poll: DevicePollResponse = resp.json().await.unwrap_or_default();
    let (Some(code), Some(verifier)) = (poll.authorization_code, poll.code_verifier) else {
        // 200 without the code yet — treat as still pending.
        return Json(serde_json::json!({ "status": "pending" })).into_response();
    };

    // 2) Exchange the authorization code for ChatGPT tokens.
    let tok = client
        .post(OAUTH_TOKEN_URL)
        .form(&[
            ("grant_type", "authorization_code"),
            ("client_id", CLIENT_ID),
            ("code", code.as_str()),
            ("code_verifier", verifier.as_str()),
            ("redirect_uri", DEVICE_REDIRECT_URI),
        ])
        .send()
        .await;
    let tok = match tok {
        Ok(r) => r,
        Err(e) => {
            return err(
                StatusCode::BAD_GATEWAY,
                format!("token exchange failed: {e}"),
            )
        }
    };
    let tok_ok = tok.status().is_success();
    let payload: TokenResponse = tok.json().await.unwrap_or_default();
    if !tok_ok {
        let msg = payload
            .error_description
            .or(payload.error)
            .unwrap_or_else(|| "token exchange failed".to_string());
        return Json(serde_json::json!({ "status": "error", "message": msg })).into_response();
    }
    let (Some(access), Some(refresh), Some(id_token)) = (
        payload.access_token,
        payload.refresh_token,
        payload.id_token,
    ) else {
        return Json(
            serde_json::json!({ "status": "error", "message": "token response missing fields" }),
        )
        .into_response();
    };
    let Some(account_id) = account_id_from_jwt(&access) else {
        return Json(
            serde_json::json!({ "status": "error", "message": "could not read account id from token" }),
        )
        .into_response();
    };

    // codex's native auth.json shape (auth_mode ChatGPT + the OAuth tokens).
    let auth_json = serde_json::json!({
        "auth_mode": "ChatGPT",
        "OPENAI_API_KEY": serde_json::Value::Null,
        "tokens": {
            "id_token": id_token,
            "access_token": access,
            "refresh_token": refresh,
            "account_id": account_id,
        },
        "last_refresh": chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true),
    })
    .to_string();

    // Persist to the encrypted store (durable / re-materialized on a fresh
    // volume) under the existing `codex` provider's `auth_json` field.
    let mut fields = store.get("codex").await.ok().flatten().unwrap_or_default();
    fields.insert("auth_json".to_string(), auth_json.clone());
    if let Err(e) = store.set("codex", &fields).await {
        return err(
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("store credential: {e}"),
        );
    }
    // Write codex's native auth.json so the CLI uses it immediately.
    if let Err(e) = write_codex_auth_json(&auth_json) {
        return err(
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("write auth.json: {e}"),
        );
    }
    Json(serde_json::json!({ "status": "connected" })).into_response()
}

/// Extract `chatgpt_account_id` from a Codex access-token JWT (the OpenAI
/// `https://api.openai.com/auth` claim). Best-effort.
fn account_id_from_jwt(access_token: &str) -> Option<String> {
    let payload_b64 = access_token.split('.').nth(1)?;
    let decoded = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(payload_b64)
        .ok()?;
    let value: serde_json::Value = serde_json::from_slice(&decoded).ok()?;
    value
        .get(JWT_CLAIM_PATH)?
        .get("chatgpt_account_id")?
        .as_str()
        .filter(|s| !s.is_empty())
        .map(str::to_string)
}

/// Write codex's `~/.codex/auth.json` (mode 0600 on unix).
fn write_codex_auth_json(json: &str) -> Result<(), String> {
    let path = home_dir().join(".codex").join("auth.json");
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).map_err(|e| format!("create dir: {e}"))?;
    }
    std::fs::write(&path, json).map_err(|e| format!("write: {e}"))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        let _ = std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600));
    }
    Ok(())
}
