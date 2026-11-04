//! Your Linear account, linked to your harness account.
//!
//! - `GET    /api/me/identities`              — what you have linked, and where you could
//! - `POST   /api/me/identities/linear/start` — `?connection=`; JSON `{ url }` to sign in to Linear
//! - `DELETE /api/me/identities/{id}`         — unlink one
//! - `GET    /api/users/{id}/identities`      — an account's links (administrator)
//! - `DELETE /api/users/{id}/identities/{identity}` — remove one (administrator)
//!
//! People sign in here with GitHub, and use their work address in Linear. A
//! Linear mention or delegation is matched to an account by who sent it, so
//! without a link those two are strangers. Linking is the person signing in to
//! Linear once from their profile: the Linear login is the proof, which is why
//! an administrator can remove a link but never make one.

use std::sync::Arc;

use axum::extract::{Extension, Path as AxumPath, Query};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::Json;
use serde_json::json;

use super::accounts::{authenticated_user, AdminOnly, Mode};
use super::linear_oauth::{can_link, describe, link_authorize_url, ConnectionQuery};
use super::runs_routes::RunsState;

fn err(status: StatusCode, msg: impl Into<String>) -> Response {
    (status, Json(json!({ "error": msg.into() }))).into_response()
}

/// The signed-in person, or a finished response saying why there isn't one.
// The `Err` here IS the response, handed straight back to axum.
#[allow(clippy::result_large_err)]
async fn me(
    state: &Arc<RunsState>,
    headers: &HeaderMap,
) -> Result<harness_persist::User, Response> {
    if super::accounts::mode(state).await != Mode::Accounts {
        return Err(err(
            StatusCode::CONFLICT,
            "this harness has no accounts, so there is nobody to link Linear to",
        ));
    }
    authenticated_user(state, headers).await.ok_or_else(|| {
        err(
            StatusCode::UNAUTHORIZED,
            "sign in to link your Linear account",
        )
    })
}

/// The Linear connections a person could link through: those with an OAuth app.
async fn linkable_connections(state: &Arc<RunsState>) -> Vec<serde_json::Value> {
    let Ok(store) = state.cred_store().await else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for id in super::linear_connections::list_ids(store)
        .await
        .unwrap_or_default()
    {
        if !can_link(store, &id).await {
            continue;
        }
        let summary = describe(store, &id).await;
        out.push(json!({
            "id": summary.id,
            "label": summary.label.or(summary.workspace_name).unwrap_or_else(|| id.as_str().to_string()),
        }));
    }
    out
}

/// `GET /api/me/identities` — your links, and the connections you could link
/// through.
pub async fn list_mine(
    Extension(state): Extension<Arc<RunsState>>,
    headers: HeaderMap,
) -> Response {
    let user = match me(&state, &headers).await {
        Ok(u) => u,
        Err(resp) => return resp,
    };
    let users = match state.user_store().await {
        Ok(u) => u,
        Err(e) => return err(StatusCode::SERVICE_UNAVAILABLE, e),
    };
    match users.list_identities(&user.id).await {
        Ok(identities) => Json(json!({
            "identities": identities,
            "connections": linkable_connections(&state).await,
        }))
        .into_response(),
        Err(e) => err(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()),
    }
}

/// `POST /api/me/identities/linear/start` — where to send the browser to sign
/// in to Linear. JSON rather than a redirect, as for the workspace install: the
/// SPA follows `url` itself.
pub async fn start_linear(
    Extension(state): Extension<Arc<RunsState>>,
    headers: HeaderMap,
    Query(q): Query<ConnectionQuery>,
) -> Response {
    let user = match me(&state, &headers).await {
        Ok(u) => u,
        Err(resp) => return resp,
    };
    let conn = match q.resolve() {
        Ok(c) => c,
        Err(resp) => return resp,
    };
    match link_authorize_url(&state, &conn, &user.id).await {
        Ok(url) => Json(json!({ "url": url })).into_response(),
        Err(e) => err(StatusCode::PRECONDITION_FAILED, e),
    }
}

/// `DELETE /api/me/identities/{id}` — unlink one of yours.
pub async fn unlink_mine(
    Extension(state): Extension<Arc<RunsState>>,
    headers: HeaderMap,
    AxumPath(id): AxumPath<String>,
) -> Response {
    let user = match me(&state, &headers).await {
        Ok(u) => u,
        Err(resp) => return resp,
    };
    unlink(&state, &user.id, &id).await
}

/// `GET /api/users/{id}/identities` — an account's links.
pub async fn list_for_user(
    _: AdminOnly,
    Extension(state): Extension<Arc<RunsState>>,
    AxumPath(user_id): AxumPath<String>,
) -> Response {
    let users = match state.user_store().await {
        Ok(u) => u,
        Err(e) => return err(StatusCode::SERVICE_UNAVAILABLE, e),
    };
    match users.list_identities(&user_id).await {
        Ok(identities) => Json(json!({ "identities": identities })).into_response(),
        Err(e) => err(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()),
    }
}

/// `DELETE /api/users/{id}/identities/{identity}` — remove a link. Removing is
/// all an administrator can do: making one takes the person's own Linear login.
pub async fn unlink_for_user(
    _: AdminOnly,
    Extension(state): Extension<Arc<RunsState>>,
    AxumPath((user_id, id)): AxumPath<(String, String)>,
) -> Response {
    unlink(&state, &user_id, &id).await
}

async fn unlink(state: &Arc<RunsState>, user_id: &str, id: &str) -> Response {
    let users = match state.user_store().await {
        Ok(u) => u,
        Err(e) => return err(StatusCode::SERVICE_UNAVAILABLE, e),
    };
    match users.unlink_identity(user_id, id).await {
        Ok(true) => Json(json!({ "unlinked": true, "id": id })).into_response(),
        Ok(false) => err(StatusCode::NOT_FOUND, "no such link"),
        Err(e) => err(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()),
    }
}
