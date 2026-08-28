//! Managing who has an account here.
//!
//! - `GET    /api/users`            — everyone, with their role and last sign-in
//! - `PUT    /api/users/{id}/role`  — promote or demote
//! - `PUT    /api/users/{id}/disabled` — suspend or restore
//! - `DELETE /api/users/{id}`       — remove
//!
//! All administrator-only, enforced by the [`AdminOnly`] extractor rather than
//! by hiding the page: the nav is presentation, and anyone can type a URL.
//!
//! **Several administrators are expected.** The first is just whoever claimed
//! the install. What keeps that safe is the last-admin guard below: the final
//! administrator cannot be demoted, disabled or removed, so an install can
//! never be left unadministrable through ordinary use — which in turn is what
//! makes handing over by demoting yourself safe.

use std::sync::Arc;

use axum::extract::{Extension, Path as AxumPath};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::Json;
use serde::Deserialize;
use serde_json::json;

use super::accounts::{current_user, valid_role, AdminOnly, ROLE_ADMIN};
use super::runs_routes::RunsState;

fn err(status: StatusCode, msg: impl Into<String>) -> Response {
    (status, Json(json!({ "error": msg.into() }))).into_response()
}

/// `GET /api/users` — everyone with an account here.
pub async fn list_users(_: AdminOnly, Extension(state): Extension<Arc<RunsState>>) -> Response {
    let users = match state.user_store().await {
        Ok(u) => u,
        Err(e) => return err(StatusCode::SERVICE_UNAVAILABLE, e),
    };
    match users.list().await {
        Ok(list) => Json(list).into_response(),
        Err(e) => err(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()),
    }
}

/// Whether `id` is the only administrator left who can still sign in.
///
/// Read before every change that could remove one. Two admins racing to demote
/// each other could in principle slip past this; the cost is a `harness admin`
/// command on the box, which is a far better failure than serialising every
/// role change behind a lock.
async fn is_last_admin(users: &harness_persist::UserStore, id: &str) -> Result<bool, String> {
    let Some(user) = users.get(id).await.map_err(|e| e.to_string())? else {
        return Ok(false);
    };
    if user.role != ROLE_ADMIN || user.disabled_at.is_some() {
        return Ok(false);
    }
    let count = users
        .active_admin_count()
        .await
        .map_err(|e| e.to_string())?;
    Ok(count <= 1)
}

#[derive(Debug, Deserialize)]
pub struct RoleRequest {
    pub role: String,
}

/// `PUT /api/users/{id}/role` — promote to administrator, or demote.
pub async fn set_role(
    _: AdminOnly,
    Extension(state): Extension<Arc<RunsState>>,
    AxumPath(id): AxumPath<String>,
    Json(req): Json<RoleRequest>,
) -> Response {
    if !valid_role(&req.role) {
        return err(
            StatusCode::BAD_REQUEST,
            format!("`{}` is not a role", req.role),
        );
    }
    let users = match state.user_store().await {
        Ok(u) => u,
        Err(e) => return err(StatusCode::SERVICE_UNAVAILABLE, e),
    };
    if req.role != ROLE_ADMIN {
        match is_last_admin(users, &id).await {
            Ok(true) => {
                return err(
                    StatusCode::CONFLICT,
                    "this is the only administrator — promote someone else first",
                )
            }
            Ok(false) => {}
            Err(e) => return err(StatusCode::INTERNAL_SERVER_ERROR, e),
        }
    }
    match users.set_role(&id, &req.role).await {
        Ok(Some(user)) => Json(user).into_response(),
        Ok(None) => err(StatusCode::NOT_FOUND, "no such account"),
        Err(e) => err(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()),
    }
}

#[derive(Debug, Deserialize)]
pub struct DisabledRequest {
    pub disabled: bool,
}

/// `PUT /api/users/{id}/disabled` — suspend an account, or bring it back.
///
/// Suspending also ends every session it holds, so it takes effect now rather
/// than whenever the browser next signs in.
pub async fn set_disabled(
    _: AdminOnly,
    Extension(state): Extension<Arc<RunsState>>,
    headers: HeaderMap,
    AxumPath(id): AxumPath<String>,
    Json(req): Json<DisabledRequest>,
) -> Response {
    let users = match state.user_store().await {
        Ok(u) => u,
        Err(e) => return err(StatusCode::SERVICE_UNAVAILABLE, e),
    };
    if req.disabled {
        if let Some(me) = current_user(&state, &headers).await {
            if me.id == id {
                return err(StatusCode::CONFLICT, "you cannot suspend your own account");
            }
        }
        match is_last_admin(users, &id).await {
            Ok(true) => {
                return err(
                    StatusCode::CONFLICT,
                    "this is the only administrator — promote someone else first",
                )
            }
            Ok(false) => {}
            Err(e) => return err(StatusCode::INTERNAL_SERVER_ERROR, e),
        }
    }
    match users.set_disabled(&id, req.disabled).await {
        Ok(Some(user)) => {
            if req.disabled {
                let _ = users.close_sessions_for(&id).await;
            }
            Json(user).into_response()
        }
        Ok(None) => err(StatusCode::NOT_FOUND, "no such account"),
        Err(e) => err(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()),
    }
}

/// `DELETE /api/users/{id}` — remove an account and its sessions.
pub async fn delete_user(
    _: AdminOnly,
    Extension(state): Extension<Arc<RunsState>>,
    headers: HeaderMap,
    AxumPath(id): AxumPath<String>,
) -> Response {
    let users = match state.user_store().await {
        Ok(u) => u,
        Err(e) => return err(StatusCode::SERVICE_UNAVAILABLE, e),
    };
    if let Some(me) = current_user(&state, &headers).await {
        if me.id == id {
            return err(
                StatusCode::CONFLICT,
                "you cannot remove your own account — ask another administrator",
            );
        }
    }
    match is_last_admin(users, &id).await {
        Ok(true) => {
            return err(
                StatusCode::CONFLICT,
                "this is the only administrator — promote someone else first",
            )
        }
        Ok(false) => {}
        Err(e) => return err(StatusCode::INTERNAL_SERVER_ERROR, e),
    }
    match users.delete(&id).await {
        Ok(true) => Json(json!({ "deleted": true, "id": id })).into_response(),
        Ok(false) => err(StatusCode::NOT_FOUND, "no such account"),
        Err(e) => err(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()),
    }
}
