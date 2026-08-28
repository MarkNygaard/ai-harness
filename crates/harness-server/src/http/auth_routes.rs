//! Signing in, and claiming an install.
//!
//! - `GET  /api/auth/status`  — mode, whether it is claimed, who you are (auth-exempt)
//! - `POST /api/auth/setup`   — claim it with the setup token (auth-exempt)
//! - `POST /api/auth/login`   — email + password (auth-exempt)
//! - `POST /api/auth/logout`  — end this session
//!
//! The first three are exempt from the bearer middleware for the obvious
//! reason: they are how you get past it. Each authenticates itself — the setup
//! token, the password, or nothing at all in the case of `status`, which never
//! returns anything a stranger could not already infer from being served the
//! login page.

use std::sync::{Arc, LazyLock};

use axum::extract::Extension;
use axum::http::{header, HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::Json;
use harness_persist::NewUser;
use serde::Deserialize;
use serde_json::json;

use super::accounts::{
    self, clear_cookie, current_user, secure_cookies, session_id, Mode, MIN_PASSWORD_LEN,
};
use super::runs_routes::RunsState;

fn err(status: StatusCode, msg: impl Into<String>) -> Response {
    (status, Json(json!({ "error": msg.into() }))).into_response()
}

/// `GET /api/auth/status` — what the UI needs before it can render anything.
pub async fn status(Extension(state): Extension<Arc<RunsState>>, headers: HeaderMap) -> Response {
    let mode = accounts::mode(&state).await;
    let claimed = accounts::is_claimed(&state).await;
    let user = current_user(&state, &headers).await;
    Json(json!({
        "mode": mode.as_str(),
        // False only on an install nobody has claimed, which is what sends the
        // browser to /setup instead of /login.
        "claimed": claimed,
        "user": user,
        "min_password_len": MIN_PASSWORD_LEN,
    }))
    .into_response()
}

#[derive(Debug, Deserialize)]
pub struct SetupRequest {
    pub setup_token: String,
    pub name: String,
    pub email: String,
    pub password: String,
}

/// `POST /api/auth/setup` — claim an unclaimed install.
///
/// Creates the first admin and switches the harness to `accounts`. Both happen
/// once: a claimed install refuses this outright, so the window in which the
/// setup token means anything closes the moment it is used.
pub async fn setup(
    Extension(state): Extension<Arc<RunsState>>,
    Json(req): Json<SetupRequest>,
) -> Response {
    if accounts::is_claimed(&state).await {
        return err(
            StatusCode::CONFLICT,
            "this harness already has accounts — sign in instead",
        );
    }
    if !accounts::setup_token_valid(&state, &req.setup_token).await {
        // Deliberately vague, and deliberately not saying whether a token
        // exists: the only thing a caller learns is that theirs is not it.
        tracing::warn!("accounts: /setup attempted with an invalid setup token");
        return err(StatusCode::UNAUTHORIZED, "that setup token is not valid");
    }
    let name = req.name.trim();
    let email = req.email.trim();
    if name.is_empty() || email.is_empty() {
        return err(StatusCode::BAD_REQUEST, "name and email are required");
    }
    if let Err(e) = accounts::check_password(&req.password) {
        return err(StatusCode::BAD_REQUEST, e);
    }
    let hash = match accounts::hash_password(&req.password) {
        Ok(h) => h,
        Err(e) => return err(StatusCode::INTERNAL_SERVER_ERROR, e),
    };
    let users = match state.user_store().await {
        Ok(u) => u,
        Err(e) => return err(StatusCode::SERVICE_UNAVAILABLE, e),
    };
    let user = match users
        .create(&NewUser {
            email: email.to_string(),
            name: name.to_string(),
            role: "admin".into(),
            password_hash: Some(hash),
        })
        .await
    {
        Ok(u) => u,
        Err(e) => return err(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()),
    };
    if let Err(e) = accounts::finish_claim(&state).await {
        // The admin exists but the mode did not stick — better to say so than
        // to leave someone believing the harness is protected when it is not.
        return err(
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("account created, but the harness could not be switched on: {e}"),
        );
    }
    let cookie = match accounts::open_session(&state, users, &user).await {
        Ok(c) => c,
        Err(e) => return err(StatusCode::INTERNAL_SERVER_ERROR, e),
    };
    (
        StatusCode::CREATED,
        [(header::SET_COOKIE, cookie)],
        Json(json!({ "user": user })),
    )
        .into_response()
}

#[derive(Debug, Deserialize)]
pub struct LoginRequest {
    pub email: String,
    pub password: String,
}

/// `POST /api/auth/login` — sign in with a local password.
pub async fn login(
    Extension(state): Extension<Arc<RunsState>>,
    Json(req): Json<LoginRequest>,
) -> Response {
    let users = match state.user_store().await {
        Ok(u) => u,
        Err(e) => return err(StatusCode::SERVICE_UNAVAILABLE, e),
    };
    let email = req.email.trim();
    let stored = users.password_hash_for(email).await.ok().flatten();

    // Verify against a dummy hash when the account is unknown, so a missing
    // account and a wrong password take the same time — otherwise the response
    // time enumerates who has an account here.
    let hash = stored.unwrap_or_else(|| DUMMY_HASH.clone());
    let ok = accounts::verify_password(&req.password, &hash);
    if !ok {
        tracing::info!("accounts: failed sign-in for {email}");
        return err(
            StatusCode::UNAUTHORIZED,
            "that email and password do not match",
        );
    }
    let Ok(Some(user)) = users.get_by_email(email).await else {
        return err(
            StatusCode::UNAUTHORIZED,
            "that email and password do not match",
        );
    };
    let cookie = match accounts::open_session(&state, users, &user).await {
        Ok(c) => c,
        Err(e) => return err(StatusCode::INTERNAL_SERVER_ERROR, e),
    };
    (
        [(header::SET_COOKIE, cookie)],
        Json(json!({ "user": user })),
    )
        .into_response()
}

/// A real Argon2id hash of a value nobody knows, so verifying against a
/// non-existent account costs the same as verifying against a real one.
///
/// Computed rather than written down: a hardcoded string that failed to parse
/// would make `verify_password` return *immediately*, which is precisely the
/// timing difference this exists to remove.
static DUMMY_HASH: LazyLock<String> = LazyLock::new(|| {
    let nobody = format!(
        "{}{}",
        uuid::Uuid::new_v4().simple(),
        uuid::Uuid::new_v4().simple()
    );
    accounts::hash_password(&nobody).unwrap_or_default()
});

/// `POST /api/auth/logout` — end this session.
pub async fn logout(Extension(state): Extension<Arc<RunsState>>, headers: HeaderMap) -> Response {
    if let Some(id) = session_id(&headers) {
        if let Ok(users) = state.user_store().await {
            let _ = users.close_session(&id).await;
        }
    }
    (
        [(header::SET_COOKIE, clear_cookie(secure_cookies(&state)))],
        Json(json!({ "ok": true })),
    )
        .into_response()
}

/// Whether this request may proceed under the current mode.
///
/// Used by the middleware. `accounts` mode wants a session; the other two are
/// the behaviour the harness already had, decided by the bearer token.
pub(crate) async fn permits(state: &Arc<RunsState>, headers: &HeaderMap) -> bool {
    match accounts::mode(state).await {
        Mode::Accounts => current_user(state, headers).await.is_some(),
        // Not this function's decision — the bearer middleware still runs.
        Mode::Open | Mode::Token => true,
    }
}
