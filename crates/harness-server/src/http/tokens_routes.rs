//! Your personal access tokens.
//!
//! - `GET    /api/tokens`      — the ones you hold
//! - `POST   /api/tokens`      — mint one; the value is returned **once**
//! - `DELETE /api/tokens/{id}` — revoke one
//!
//! Not administrator-only: these are yours, and every route is scoped to
//! whoever is signed in — an id from somebody else's list is not a way to sign
//! their programs out.
//!
//! Only meaningful once an install has accounts. Before that, `/mcp` is reached
//! with the shared MCP key and there is nobody for a token to belong to.

use std::sync::Arc;

use axum::extract::{Extension, Path as AxumPath};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::Json;
use serde::Deserialize;
use serde_json::json;

use super::accounts::{authenticated_user, mint_token, Mode};
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
            "this harness has no accounts, so there is nobody for a token to belong to",
        ));
    }
    authenticated_user(state, headers)
        .await
        .ok_or_else(|| err(StatusCode::UNAUTHORIZED, "sign in to manage your tokens"))
}

/// `GET /api/tokens` — the tokens you hold. Never their values.
pub async fn list_tokens(
    Extension(state): Extension<Arc<RunsState>>,
    headers: HeaderMap,
) -> Response {
    let user = match me(&state, &headers).await {
        Ok(u) => u,
        Err(resp) => return resp,
    };
    let tokens = match state.token_store().await {
        Ok(t) => t,
        Err(e) => return err(StatusCode::SERVICE_UNAVAILABLE, e),
    };
    match tokens.list_for_user(&user.id).await {
        Ok(list) => Json(list).into_response(),
        Err(e) => err(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()),
    }
}

#[derive(Debug, Deserialize)]
pub struct CreateTokenRequest {
    /// What it is for, in your words: "laptop", "CI".
    pub name: String,
}

/// `POST /api/tokens` — mint one.
///
/// The response carries the token itself. It is the **only** time it exists
/// outside the caller: the server keeps a hash, so there is nothing to show
/// again later.
pub async fn create_token(
    Extension(state): Extension<Arc<RunsState>>,
    headers: HeaderMap,
    Json(req): Json<CreateTokenRequest>,
) -> Response {
    let user = match me(&state, &headers).await {
        Ok(u) => u,
        Err(resp) => return resp,
    };
    let name = req.name.trim();
    if name.is_empty() {
        return err(
            StatusCode::BAD_REQUEST,
            "give it a name, so you can tell your tokens apart later",
        );
    }
    let tokens = match state.token_store().await {
        Ok(t) => t,
        Err(e) => return err(StatusCode::SERVICE_UNAVAILABLE, e),
    };
    let secret = mint_token();
    match tokens.create(&user.id, name, &secret).await {
        Ok(record) => (
            StatusCode::CREATED,
            Json(json!({ "token": record, "secret": secret })),
        )
            .into_response(),
        Err(e) => err(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()),
    }
}

/// `DELETE /api/tokens/{id}` — revoke one of yours.
pub async fn revoke_token(
    Extension(state): Extension<Arc<RunsState>>,
    headers: HeaderMap,
    AxumPath(id): AxumPath<String>,
) -> Response {
    let user = match me(&state, &headers).await {
        Ok(u) => u,
        Err(resp) => return resp,
    };
    let tokens = match state.token_store().await {
        Ok(t) => t,
        Err(e) => return err(StatusCode::SERVICE_UNAVAILABLE, e),
    };
    // Scoped to the owner, so a wrong or guessed id reads as "not yours"
    // rather than revoking somebody else's.
    match tokens.revoke(&id, &user.id).await {
        Ok(true) => Json(json!({ "revoked": true, "id": id })).into_response(),
        Ok(false) => err(StatusCode::NOT_FOUND, "no such token"),
        Err(e) => err(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()),
    }
}
