//! Getting a second person in, and getting a forgotten password back.
//!
//! - `GET/POST /api/invites`             — outstanding invitations; send one
//! - `DELETE   /api/invites/{id}`        — withdraw one
//! - `GET      /api/invites/{token}`     — what a link is for (public)
//! - `POST     /api/invites/{token}`     — accept it (public)
//! - `POST     /auth/reset-password`     — ask for a reset link (public)
//!
//! **The link is always returned to the administrator**, whether or not mail
//! went out. SMTP is configured in this same UI, so requiring it to invite the
//! first colleague would be a circle; a link you can paste into Slack breaks it.
//!
//! The public routes answer identically for addresses that exist and addresses
//! that do not, so neither is a way to learn who has an account here.

use std::sync::Arc;

use axum::extract::{Extension, Path as AxumPath};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::Json;
use harness_persist::{NewUser, KIND_INVITE, KIND_RESET};
use serde::Deserialize;
use serde_json::json;

use super::accounts::{
    authenticated_user, check_password, hash_password, mint_token, valid_role, AdminOnly, Mode,
    ROLE_MEMBER,
};
use super::mail;
use super::runs_routes::RunsState;

fn err(status: StatusCode, msg: impl Into<String>) -> Response {
    (status, Json(json!({ "error": msg.into() }))).into_response()
}

/// Where a token is redeemed. Built from the public URL so it can be pasted.
fn accept_link(state: &Arc<RunsState>, token: &str) -> Option<String> {
    state
        .public_url()
        .map(|base| format!("{base}/invite/{token}"))
}

/// `GET /api/invites` — outstanding invitations.
pub async fn list_invites(_: AdminOnly, Extension(state): Extension<Arc<RunsState>>) -> Response {
    match state.invite_store().await {
        Ok(store) => match store.list_pending().await {
            Ok(list) => Json(list).into_response(),
            Err(e) => err(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()),
        },
        Err(e) => err(StatusCode::SERVICE_UNAVAILABLE, e),
    }
}

#[derive(Debug, Deserialize)]
pub struct InviteRequest {
    pub email: String,
    #[serde(default)]
    pub role: Option<String>,
}

/// `POST /api/invites` — invite someone.
///
/// Always returns the link. Mail is attempted when it is configured, and a
/// failure to send is reported alongside the link rather than instead of it —
/// the invitation exists either way.
pub async fn create_invite(
    _: AdminOnly,
    Extension(state): Extension<Arc<RunsState>>,
    headers: HeaderMap,
    Json(req): Json<InviteRequest>,
) -> Response {
    if super::accounts::mode(&state).await != Mode::Accounts {
        return err(
            StatusCode::CONFLICT,
            "this harness has no accounts yet — claim it at /setup first",
        );
    }
    let email = req.email.trim().to_lowercase();
    if !email.contains('@') {
        return err(StatusCode::BAD_REQUEST, "that is not an email address");
    }
    let role = req.role.unwrap_or_else(|| ROLE_MEMBER.to_string());
    if !valid_role(&role) {
        return err(StatusCode::BAD_REQUEST, format!("`{role}` is not a role"));
    }

    let users = match state.user_store().await {
        Ok(u) => u,
        Err(e) => return err(StatusCode::SERVICE_UNAVAILABLE, e),
    };
    if matches!(users.get_by_email(&email).await, Ok(Some(_))) {
        return err(
            StatusCode::CONFLICT,
            format!("{email} already has an account here"),
        );
    }

    let store = match state.invite_store().await {
        Ok(s) => s,
        Err(e) => return err(StatusCode::SERVICE_UNAVAILABLE, e),
    };
    let inviter = authenticated_user(&state, &headers).await;
    let token = mint_token();
    let invite = match store
        .create(
            &email,
            KIND_INVITE,
            &role,
            inviter.as_ref().map(|u| u.id.as_str()),
            &token,
        )
        .await
    {
        Ok(i) => i,
        Err(e) => return err(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()),
    };

    let link = accept_link(&state, &token);
    let mut mail_error: Option<String> = None;
    let mut mailed = false;
    if let Some(link) = &link {
        let who = inviter
            .as_ref()
            .map(|u| u.name.as_str())
            .unwrap_or("An administrator");
        let body = format!(
            "{who} has invited you to an ai-harness.\n\n\
             Set a password and sign in:\n{link}\n\n\
             The link works once, and expires in a week.\n"
        );
        match mail::send(&state, &email, "You have been invited to ai-harness", &body).await {
            Ok(()) => mailed = true,
            // Not fatal: the invitation is real, and the link is right there.
            Err(e) => mail_error = Some(e),
        }
    }

    (
        StatusCode::CREATED,
        Json(json!({
            "invite": invite,
            // The mechanism, not a fallback. `None` only when no public URL is
            // set, in which case there is no link to give anybody.
            "link": link,
            "mailed": mailed,
            "mail_error": mail_error,
        })),
    )
        .into_response()
}

/// `DELETE /api/invites/{id}` — withdraw one.
pub async fn revoke_invite(
    _: AdminOnly,
    Extension(state): Extension<Arc<RunsState>>,
    AxumPath(id): AxumPath<String>,
) -> Response {
    let store = match state.invite_store().await {
        Ok(s) => s,
        Err(e) => return err(StatusCode::SERVICE_UNAVAILABLE, e),
    };
    match store.revoke(&id).await {
        Ok(true) => Json(json!({ "revoked": true, "id": id })).into_response(),
        Ok(false) => err(StatusCode::NOT_FOUND, "no such invitation"),
        Err(e) => err(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()),
    }
}

/// `GET /api/invites/{token}` — what this link is for, before redeeming it.
///
/// Public, and says nothing an attacker holding the token does not already
/// have. An unknown or spent token is a 404, not an explanation.
pub async fn describe_invite(
    Extension(state): Extension<Arc<RunsState>>,
    AxumPath(token): AxumPath<String>,
) -> Response {
    let store = match state.invite_store().await {
        Ok(s) => s,
        Err(e) => return err(StatusCode::SERVICE_UNAVAILABLE, e),
    };
    match store.find_live(&token).await {
        Ok(Some(invite)) => Json(json!({
            "email": invite.email,
            "kind": invite.kind,
            "expires_at": invite.expires_at,
        }))
        .into_response(),
        Ok(None) => err(
            StatusCode::NOT_FOUND,
            "this link has expired or has already been used",
        ),
        Err(e) => err(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()),
    }
}

#[derive(Debug, Deserialize)]
pub struct AcceptRequest {
    #[serde(default)]
    pub name: Option<String>,
    pub password: String,
}

/// `POST /api/invites/{token}` — redeem a link.
///
/// Creates the account for an invitation, or re-passwords it for a reset.
/// Spending the token comes first: two requests racing must not both get in.
pub async fn accept_invite(
    Extension(state): Extension<Arc<RunsState>>,
    AxumPath(token): AxumPath<String>,
    Json(req): Json<AcceptRequest>,
) -> Response {
    if let Err(e) = check_password(&req.password) {
        return err(StatusCode::BAD_REQUEST, e);
    }
    let store = match state.invite_store().await {
        Ok(s) => s,
        Err(e) => return err(StatusCode::SERVICE_UNAVAILABLE, e),
    };
    let Ok(Some(invite)) = store.find_live(&token).await else {
        return err(
            StatusCode::NOT_FOUND,
            "this link has expired or has already been used",
        );
    };
    // Spend first. If two requests race, exactly one wins here.
    match store.consume(&token).await {
        Ok(true) => {}
        Ok(false) => {
            return err(
                StatusCode::CONFLICT,
                "this link has expired or has already been used",
            )
        }
        Err(e) => return err(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()),
    }

    let users = match state.user_store().await {
        Ok(u) => u,
        Err(e) => return err(StatusCode::SERVICE_UNAVAILABLE, e),
    };
    let hash = match hash_password(&req.password) {
        Ok(h) => h,
        Err(e) => return err(StatusCode::INTERNAL_SERVER_ERROR, e),
    };

    if invite.kind == KIND_RESET {
        let Ok(Some(existing)) = users.get_by_email(&invite.email).await else {
            return err(StatusCode::NOT_FOUND, "that account no longer exists");
        };
        if let Err(e) = users.set_password_hash(&existing.id, Some(&hash)).await {
            return err(StatusCode::INTERNAL_SERVER_ERROR, e.to_string());
        }
        // Every other session was opened with the old password.
        let _ = users.close_sessions_for(&existing.id).await;
        return Json(json!({ "accepted": true, "email": invite.email })).into_response();
    }

    let name = req
        .name
        .map(|n| n.trim().to_string())
        .filter(|n| !n.is_empty())
        .unwrap_or_else(|| invite.email.clone());
    match users
        .create(&NewUser {
            email: invite.email.clone(),
            name,
            role: invite.role.clone(),
            password_hash: Some(hash),
        })
        .await
    {
        Ok(user) => Json(json!({ "accepted": true, "email": user.email })).into_response(),
        Err(e) => err(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()),
    }
}

/// Mint a reset link for `email` and try to mail it.
///
/// Deliberately returns nothing. The caller answers identically whether or not
/// this did anything: an unauthenticated endpoint that distinguishes a known
/// address from an unknown one is a way to enumerate who has an account here.
/// Rate limiting lives with the route, in [`super::misc_routes`].
pub(crate) async fn send_reset_link(state: &Arc<RunsState>, email: &str) {
    let email = email.trim().to_lowercase();
    if !email.contains('@') {
        return;
    }
    let (Ok(users), Ok(store)) = (state.user_store().await, state.invite_store().await) else {
        return;
    };
    // Everything past here is best-effort and silent, so that a failure to send
    // does not become a signal about whether the address exists.
    if let Ok(Some(user)) = users.get_by_email(&email).await {
        if user.disabled_at.is_none() {
            let token = mint_token();
            if store
                .create(&email, KIND_RESET, ROLE_MEMBER, None, &token)
                .await
                .is_ok()
            {
                if let Some(link) = accept_link(state, &token) {
                    let body = format!(
                        "Someone asked to reset the password for this ai-harness account.\n\n\
                         Set a new one:\n{link}\n\n\
                         The link works once, and expires in two hours. \
                         If this was not you, nothing has changed.\n"
                    );
                    if let Err(e) =
                        mail::send(state, &email, "ai-harness: reset your password", &body).await
                    {
                        tracing::warn!("reset: could not send to {email}: {e}");
                    }
                }
            }
        }
    }
}
