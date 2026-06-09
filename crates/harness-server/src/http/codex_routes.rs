//! "Connect Codex" — the OpenAI/ChatGPT OAuth **authorization-code + PKCE** flow,
//! run server-side so the operator authenticates from the harness UI.
//!
//! We deliberately use the browser/PKCE flow (what `codex login` and omp's
//! `loginOpenAICodex` use), NOT the device-code flow: OpenAI gates device-code
//! auth behind a ChatGPT *workspace* security setting ("device code
//! authorization"), so on workspaces where it's disabled the device flow errors
//! with "contact your workspace admin". PKCE has no such requirement.
//!
//! Headless flow (no localhost callback server): `start` returns the OpenAI
//! authorize URL plus the PKCE `verifier` + `state`; the operator signs in in
//! their own browser, gets redirected to `http://localhost:1455/auth/callback?
//! code=…&state=…` (which won't load — there's no server there — but the URL is
//! in the address bar), and pastes that URL back to `complete`, which exchanges
//! the code for tokens and writes codex's `~/.codex/auth.json`. Stateless: the
//! client carries `verifier`/`state` between the two calls (mirrors Connect Kimi).

use std::path::PathBuf;
use std::sync::Arc;

use axum::extract::Extension;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::Json;
use base64::Engine as _;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use super::runs_routes::RunsState;

const CLIENT_ID: &str = "app_EMoamEEZ73f0CkXaXp7hrann";
const AUTHORIZE_URL: &str = "https://auth.openai.com/oauth/authorize";
const TOKEN_URL: &str = "https://auth.openai.com/oauth/token";
const REDIRECT_URI: &str = "http://localhost:1455/auth/callback";
const SCOPE: &str = "openid profile email offline_access";
const ORIGINATOR: &str = "codex_cli_rs";
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

const B64: base64::engine::general_purpose::GeneralPurpose =
    base64::engine::general_purpose::URL_SAFE_NO_PAD;

/// Percent-encode a query-string value (RFC 3986 unreserved set kept as-is).
fn pe(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' => {
                out.push(b as char)
            }
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

// ── Start: build the authorize URL + PKCE ────────────────────────────────────

#[derive(Serialize)]
pub struct ConnectStartResponse {
    pub authorize_url: String,
    pub state: String,
    /// PKCE verifier — the client passes it back to `complete` (single-use).
    pub verifier: String,
    pub redirect_uri: String,
}

/// `POST /api/credentials/codex/connect/start` — begin the PKCE flow.
pub async fn connect_start(Extension(state): Extension<Arc<RunsState>>) -> Response {
    if let Err(e) = state.cred_store().await {
        return err(StatusCode::SERVICE_UNAVAILABLE, e);
    }
    // PKCE: verifier = base64url(96 random bytes); challenge = base64url(sha256(verifier)).
    let mut bytes = Vec::with_capacity(96);
    for _ in 0..6 {
        bytes.extend_from_slice(uuid::Uuid::new_v4().as_bytes());
    }
    let verifier = B64.encode(&bytes);
    let challenge = B64.encode(Sha256::digest(verifier.as_bytes()));
    let csrf = uuid::Uuid::new_v4().simple().to_string();

    let authorize_url = format!(
        "{AUTHORIZE_URL}?response_type=code&client_id={}&redirect_uri={}&scope={}\
         &code_challenge={}&code_challenge_method=S256&state={}\
         &id_token_add_organizations=true&codex_cli_simplified_flow=true&originator={}",
        pe(CLIENT_ID),
        pe(REDIRECT_URI),
        pe(SCOPE),
        pe(&challenge),
        pe(&csrf),
        pe(ORIGINATOR),
    );
    Json(ConnectStartResponse {
        authorize_url,
        state: csrf,
        verifier,
        redirect_uri: REDIRECT_URI.to_string(),
    })
    .into_response()
}

// ── Complete: exchange the pasted redirect for tokens ────────────────────────

#[derive(Deserialize)]
pub struct ConnectCompleteRequest {
    /// The full redirect URL the browser landed on (or a bare `code`).
    pub redirect: String,
    pub state: String,
    pub verifier: String,
}

#[derive(Deserialize, Default)]
struct TokenResponse {
    access_token: Option<String>,
    refresh_token: Option<String>,
    id_token: Option<String>,
    error: Option<String>,
    error_description: Option<String>,
}

/// `POST /api/credentials/codex/connect/complete` — exchange the authorization
/// code (from the pasted redirect URL) for ChatGPT tokens and store them.
pub async fn connect_complete(
    Extension(state): Extension<Arc<RunsState>>,
    Json(req): Json<ConnectCompleteRequest>,
) -> Response {
    let store = match state.cred_store().await {
        Ok(s) => s,
        Err(e) => return err(StatusCode::SERVICE_UNAVAILABLE, e),
    };

    // Accept either the full redirect URL or a bare code pasted by the operator.
    let code = match extract_param(&req.redirect, "code") {
        Some(c) => {
            // When a full URL was pasted, guard against a mismatched state.
            if let Some(returned) = extract_param(&req.redirect, "state") {
                if returned != req.state {
                    return err(StatusCode::BAD_REQUEST, "state mismatch — start again");
                }
            }
            c
        }
        None => req.redirect.trim().to_string(),
    };
    if code.is_empty() {
        return err(
            StatusCode::BAD_REQUEST,
            "no authorization code found in the pasted value",
        );
    }

    let client = reqwest::Client::new();
    let tok = client
        .post(TOKEN_URL)
        .form(&[
            ("grant_type", "authorization_code"),
            ("client_id", CLIENT_ID),
            ("code", code.as_str()),
            ("code_verifier", req.verifier.as_str()),
            ("redirect_uri", REDIRECT_URI),
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
    let ok = tok.status().is_success();
    let payload: TokenResponse = tok.json().await.unwrap_or_default();
    if !ok {
        let msg = payload
            .error_description
            .or(payload.error)
            .unwrap_or_else(|| "token exchange failed".to_string());
        return err(StatusCode::BAD_GATEWAY, msg);
    }
    let (Some(access), Some(refresh), Some(id_token)) = (
        payload.access_token,
        payload.refresh_token,
        payload.id_token,
    ) else {
        return err(StatusCode::BAD_GATEWAY, "token response missing fields");
    };
    let Some(account_id) = account_id_from_jwt(&access) else {
        return err(
            StatusCode::BAD_GATEWAY,
            "could not read account id from token",
        );
    };

    // Copies for omp provisioning below — `auth_json` consumes the originals.
    let omp_access = access.clone();
    let omp_refresh = refresh.clone();
    let omp_id_token = id_token.clone();
    let omp_account_id = account_id.clone();

    // Exactly the shape `codex login` writes: no `auth_mode` (the CLI parses it
    // as an enum and infers ChatGPT mode from the presence of `tokens`).
    let auth_json = serde_json::json!({
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

    let mut fields = store.get("codex").await.ok().flatten().unwrap_or_default();
    fields.insert("auth_json".to_string(), auth_json.clone());
    if let Err(e) = store.set("codex", &fields).await {
        return err(
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("store credential: {e}"),
        );
    }
    if let Err(e) = write_codex_auth_json(&auth_json) {
        return err(
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("write auth.json: {e}"),
        );
    }
    // One "Connect Codex" also provisions omp's `openai-codex` from the same
    // ChatGPT tokens, so the `pi` provider can run `openai-codex/*` models
    // without a separate login. Fail loud if it doesn't take.
    if let Err(e) =
        provision_omp_codex(&omp_access, &omp_refresh, &omp_id_token, &omp_account_id).await
    {
        return err(
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("connected the Codex CLI, but provisioning omp's openai-codex failed: {e}"),
        );
    }
    Json(serde_json::json!({ "status": "connected" })).into_response()
}

/// Provision omp's `openai-codex` credential from the ChatGPT OAuth tokens, so a
/// single "Connect Codex" powers both the Codex CLI (`~/.codex/auth.json`) and
/// omp (the `pi` provider running `openai-codex/*` models). We hand the tokens to
/// omp via `omp auth-broker import` rather than writing its SQLite vault
/// directly: omp owns that schema, and `import` transparently targets the local
/// store or a configured auth-broker (`OMP_AUTH_BROKER_URL`).
async fn provision_omp_codex(
    access: &str,
    refresh: &str,
    id_token: &str,
    account_id: &str,
) -> Result<(), String> {
    // CLIProxyAPI-style entry: omp maps `type: "codex"` → provider `openai-codex`.
    let expired = jwt_exp_rfc3339(access).unwrap_or_else(|| {
        (chrono::Utc::now() + chrono::Duration::hours(1))
            .to_rfc3339_opts(chrono::SecondsFormat::Secs, true)
    });
    let mut entry = serde_json::json!({
        "type": "codex",
        "access_token": access,
        "refresh_token": refresh,
        "expired": expired,
        "account_id": account_id,
    });
    if let Some(email) = jwt_claim_str(id_token, "email") {
        entry["email"] = serde_json::Value::String(email);
    }

    let dir = tempfile::tempdir().map_err(|e| format!("tempdir: {e}"))?;
    let path = dir.path().join("codex-connect.json");
    std::fs::write(&path, entry.to_string()).map_err(|e| format!("write import file: {e}"))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        let _ = std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600));
    }

    let omp = std::env::var_os("OMP_CLI")
        .or_else(|| std::env::var_os("OMP_PATH"))
        .unwrap_or_else(|| std::ffi::OsString::from("omp"));
    let out = tokio::process::Command::new(&omp)
        .arg("auth-broker")
        .arg("import")
        .arg(&path)
        .output()
        .await
        .map_err(|e| format!("spawn `omp auth-broker import`: {e}"))?;
    if !out.status.success() {
        return Err(format!(
            "`omp auth-broker import` exited {}: {}",
            out.status,
            String::from_utf8_lossy(&out.stderr).trim()
        ));
    }
    Ok(())
}

/// Read a string claim from a JWT payload (best-effort).
fn jwt_claim_str(jwt: &str, claim: &str) -> Option<String> {
    let payload_b64 = jwt.split('.').nth(1)?;
    let decoded = B64.decode(payload_b64).ok()?;
    let value: serde_json::Value = serde_json::from_slice(&decoded).ok()?;
    value
        .get(claim)?
        .as_str()
        .filter(|s| !s.is_empty())
        .map(str::to_string)
}

/// Read the `exp` (epoch seconds) claim from a JWT and format it RFC3339 — the
/// `expired` field omp's import expects.
fn jwt_exp_rfc3339(jwt: &str) -> Option<String> {
    let payload_b64 = jwt.split('.').nth(1)?;
    let decoded = B64.decode(payload_b64).ok()?;
    let value: serde_json::Value = serde_json::from_slice(&decoded).ok()?;
    let exp = value.get("exp")?.as_i64()?;
    chrono::DateTime::from_timestamp(exp, 0)
        .map(|dt| dt.to_rfc3339_opts(chrono::SecondsFormat::Secs, true))
}

/// Extract a query parameter from a redirect URL (or `key=value&…` string),
/// percent-decoding the value. Returns `None` if absent.
fn extract_param(input: &str, key: &str) -> Option<String> {
    let query = input.split_once('?').map(|(_, q)| q).unwrap_or(input);
    let query = query.split('#').next().unwrap_or(query);
    for pair in query.split('&') {
        if let Some((k, v)) = pair.split_once('=') {
            if k == key {
                return Some(percent_decode(v));
            }
        }
    }
    None
}

/// Minimal percent-decoder for query values (`%XX` and `+` → space).
fn percent_decode(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'%' if i + 2 < bytes.len() => {
                let hex = std::str::from_utf8(&bytes[i + 1..i + 3]).ok();
                match hex.and_then(|h| u8::from_str_radix(h, 16).ok()) {
                    Some(b) => {
                        out.push(b);
                        i += 3;
                    }
                    None => {
                        out.push(bytes[i]);
                        i += 1;
                    }
                }
            }
            b'+' => {
                out.push(b' ');
                i += 1;
            }
            b => {
                out.push(b);
                i += 1;
            }
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// Extract `chatgpt_account_id` from a Codex access-token JWT. Best-effort.
fn account_id_from_jwt(access_token: &str) -> Option<String> {
    let payload_b64 = access_token.split('.').nth(1)?;
    let decoded = B64.decode(payload_b64).ok()?;
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
