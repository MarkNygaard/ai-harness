//! How the harness itself is configured — as opposed to the credentials it uses
//! to do work, which are the Credentials page.
//!
//! - `GET/PUT /api/settings/general`   — the public URL this instance advertises
//! - `GET/PUT /api/settings/mail`      — SMTP
//! - `POST    /api/settings/mail/test` — send a test message
//!
//! Administrator-only, at the route.

use std::collections::BTreeMap;
use std::sync::Arc;

use axum::extract::Extension;
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::Json;
use serde::Deserialize;
use serde_json::json;

use super::accounts::{authenticated_user, AdminOnly};
use super::mail;
use super::runs_routes::RunsState;

/// Where the public URL is stored, once an administrator sets one.
const PUBLIC_URL_KEY: &str = "public_url";

fn err(status: StatusCode, msg: impl Into<String>) -> Response {
    (status, Json(json!({ "error": msg.into() }))).into_response()
}

/// `GET /api/settings/general` — the public URL, and where it came from.
pub async fn general(_: AdminOnly, Extension(state): Extension<Arc<RunsState>>) -> Response {
    let stored = match state.settings_store().await {
        Ok(s) => s.get(PUBLIC_URL_KEY).await.ok().flatten(),
        Err(e) => return err(StatusCode::SERVICE_UNAVAILABLE, e),
    };
    Json(json!({
        // What everything actually uses right now.
        "public_url": state.public_url(),
        // Set here, as opposed to inherited from the environment — so the page
        // can say which one is in force and offer to clear the override.
        "stored": stored,
        "from_environment": std::env::var("HARNESS_PUBLIC_URL").ok().filter(|v| !v.is_empty()),
    }))
    .into_response()
}

#[derive(Debug, Deserialize)]
pub struct GeneralRequest {
    /// The base URL, or `null` to fall back to the environment.
    pub public_url: Option<String>,
}

/// `PUT /api/settings/general` — set or clear the public URL.
///
/// Takes effect immediately: the OAuth callback, the webhook address, the MCP
/// endpoint and every run link are built from it, and none of them should need
/// a redeploy to correct a typo.
pub async fn set_general(
    _: AdminOnly,
    Extension(state): Extension<Arc<RunsState>>,
    Json(req): Json<GeneralRequest>,
) -> Response {
    let value = req
        .public_url
        .map(|u| u.trim().trim_end_matches('/').to_string())
        .filter(|u| !u.is_empty());

    if let Some(url) = &value {
        if !url.starts_with("http://") && !url.starts_with("https://") {
            return err(
                StatusCode::BAD_REQUEST,
                "the URL needs a scheme — start it with https://",
            );
        }
    }

    let store = match state.settings_store().await {
        Ok(s) => s,
        Err(e) => return err(StatusCode::SERVICE_UNAVAILABLE, e),
    };
    let result = match &value {
        Some(url) => store.set(PUBLIC_URL_KEY, url).await,
        None => store.delete(PUBLIC_URL_KEY).await.map(|_| ()),
    };
    if let Err(e) = result {
        return err(StatusCode::INTERNAL_SERVER_ERROR, e.to_string());
    }

    // Cleared means "go back to the environment", not "no public URL".
    let effective = value.or_else(|| {
        std::env::var("HARNESS_PUBLIC_URL")
            .ok()
            .filter(|v| !v.trim().is_empty())
    });
    state.set_public_url(effective);
    general(AdminOnly, Extension(state)).await
}

/// `GET /api/settings/mail` — the SMTP settings, without the password.
pub async fn mail_settings(_: AdminOnly, Extension(state): Extension<Arc<RunsState>>) -> Response {
    match state.cred_store().await {
        Ok(store) => Json(mail::summary(store).await).into_response(),
        Err(e) => err(StatusCode::SERVICE_UNAVAILABLE, e),
    }
}

#[derive(Debug, Deserialize)]
pub struct MailRequest {
    pub host: Option<String>,
    pub port: Option<u16>,
    pub username: Option<String>,
    /// Omitted leaves the stored one alone — the form never shows it, so an
    /// empty field means "unchanged", not "clear it".
    pub password: Option<String>,
    pub from: Option<String>,
    pub encryption: Option<String>,
}

/// `PUT /api/settings/mail` — save SMTP settings.
pub async fn set_mail(
    _: AdminOnly,
    Extension(state): Extension<Arc<RunsState>>,
    Json(req): Json<MailRequest>,
) -> Response {
    let store = match state.cred_store().await {
        Ok(s) => s,
        Err(e) => return err(StatusCode::SERVICE_UNAVAILABLE, e),
    };
    let mut fields: BTreeMap<String, String> = BTreeMap::new();
    if let Some(v) = req.host {
        fields.insert("host".into(), v.trim().to_string());
    }
    if let Some(v) = req.port {
        fields.insert("port".into(), v.to_string());
    }
    if let Some(v) = req.username {
        fields.insert("username".into(), v.trim().to_string());
    }
    if let Some(v) = req.from {
        fields.insert("from".into(), v.trim().to_string());
    }
    if let Some(v) = req.encryption {
        let v = v.trim().to_string();
        if !["starttls", "tls", "none"].contains(&v.as_str()) {
            return err(
                StatusCode::BAD_REQUEST,
                "encryption must be starttls, tls or none",
            );
        }
        fields.insert("encryption".into(), v);
    }
    // A blank password field means "leave it"; the store merges rather than
    // overwrites, so simply omitting it is enough.
    if let Some(v) = req.password.filter(|p| !p.is_empty()) {
        fields.insert("password".into(), v);
    }
    if fields.is_empty() {
        return err(StatusCode::BAD_REQUEST, "nothing to save");
    }
    match store.set(mail::PROVIDER, &fields).await {
        Ok(()) => Json(mail::summary(store).await).into_response(),
        Err(e) => err(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()),
    }
}

/// `POST /api/settings/mail/test` — send a message to whoever asked.
///
/// To the administrator's own address rather than one they type: the question
/// this answers is "can this harness send mail", and a typo in the recipient
/// would answer a different one.
pub async fn test_mail(
    _: AdminOnly,
    Extension(state): Extension<Arc<RunsState>>,
    headers: HeaderMap,
) -> Response {
    let Some(user) = authenticated_user(&state, &headers).await else {
        return err(
            StatusCode::CONFLICT,
            "sign in first — the test goes to your own address",
        );
    };
    let body = format!(
        "This is a test message from your ai-harness.\n\n\
         If you are reading it, mail is working: invites and password resets \
         will reach people.\n\n\
         Sent to {} because that is the account that asked for it.\n",
        user.email
    );
    match mail::send(&state, &user.email, "ai-harness: mail is working", &body).await {
        Ok(()) => Json(json!({ "sent": true, "to": user.email })).into_response(),
        Err(e) => err(StatusCode::BAD_GATEWAY, e),
    }
}

/// Apply the stored public URL at startup, if there is one.
///
/// Spawned rather than awaited: the database may not be up when the router is
/// built, and a harness that cannot reach it should still serve `/health`.
pub(crate) fn spawn_load_public_url(state: Arc<RunsState>) {
    tokio::spawn(async move {
        let Ok(store) = state.settings_store().await else {
            return;
        };
        if let Ok(Some(url)) = store.get(PUBLIC_URL_KEY).await {
            tracing::info!("settings: using the stored public URL `{url}`");
            state.set_public_url(Some(url));
        }
    });
}
