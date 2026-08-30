//! Managing who has an account here.
//!
//! - `GET    /api/users`            — everyone, with their role and last sign-in
//! - `PUT    /api/users/{id}`       — change a name and email
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

#[derive(Debug, Deserialize)]
pub struct ProfileRequest {
    pub name: String,
    pub email: String,
}

/// `PUT /api/users/{id}` — change an account's display name and email.
///
/// The email is the account's identity, not a label: `get_by_email` is what
/// sign-in and invites match on, so two accounts sharing one address is an
/// authentication ambiguity rather than a display bug. It is normalised the
/// same way [`harness_persist::UserStore::create`] normalises it — trimmed
/// and lower-cased — and checked against every other account before the
/// write. The unique index on `lower(email)` is the backstop for the window
/// between that check and the update.
pub async fn set_profile(
    _: AdminOnly,
    Extension(state): Extension<Arc<RunsState>>,
    AxumPath(id): AxumPath<String>,
    Json(req): Json<ProfileRequest>,
) -> Response {
    let name = req.name.trim().to_string();
    let email = req.email.trim().to_lowercase();
    if name.is_empty() {
        return err(StatusCode::BAD_REQUEST, "a name is required");
    }
    if email.is_empty() {
        return err(StatusCode::BAD_REQUEST, "an email address is required");
    }
    if !email.contains('@') {
        return err(StatusCode::BAD_REQUEST, "that is not an email address");
    }

    let users = match state.user_store().await {
        Ok(u) => u,
        Err(e) => return err(StatusCode::SERVICE_UNAVAILABLE, e),
    };

    // Held by somebody else. Re-saving one's own address — in any case, with
    // any padding — is not a clash, which is why the id is compared rather
    // than just the lookup succeeding.
    match users.get_by_email(&email).await {
        Ok(Some(other)) if other.id != id => {
            return err(
                StatusCode::CONFLICT,
                format!("{email} already has an account here"),
            )
        }
        Ok(_) => {}
        Err(e) => return err(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()),
    }

    match users.set_profile(&id, &name, &email).await {
        Ok(Some(user)) => Json(user).into_response(),
        Ok(None) => err(StatusCode::NOT_FOUND, "no such account"),
        Err(e) if is_duplicate_email(&e) => err(
            StatusCode::CONFLICT,
            format!("{email} already has an account here"),
        ),
        Err(e) => err(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()),
    }
}

/// Whether a store error is the `lower(email)` unique index refusing a second
/// account for one address. Two administrators editing at the same moment can
/// both pass the check above; the index is what actually makes it impossible,
/// and this turns its error into the 409 that check would have given instead
/// of a 500 carrying a Postgres constraint name.
fn is_duplicate_email(e: &harness_persist::PersistError) -> bool {
    matches!(
        e,
        harness_persist::PersistError::Db(sqlx::Error::Database(db))
            if db.code().as_deref() == Some("23505")
    )
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

#[cfg(test)]
mod tests {
    use super::*;
    use harness_agents::registry::AgentRegistry;
    use harness_persist::NewUser;

    /// A Postgres URL, but only when it names a *test* database — the same
    /// guard `harness-persist`'s own DB tests use, so running the suite can
    /// never write to a real install. (`harness_persist::is_test_db` is
    /// `pub(crate)`, hence the copy.)
    fn db_url() -> Option<String> {
        let url = std::env::var("HARNESS_DATABASE_URL").ok()?;
        let name = url.rsplit('/').next().unwrap_or(&url);
        let name = name.split(['?', '#']).next().unwrap_or(name);
        let is_test = name.to_ascii_lowercase().contains("test");
        is_test.then_some(url)
    }

    fn test_state(db_url: Option<String>) -> Arc<RunsState> {
        Arc::new(RunsState::new(
            db_url,
            Arc::new(AgentRegistry::new("test")),
            std::path::PathBuf::from("/tmp"),
            None,
            None,
        ))
    }

    fn unique_email(tag: &str) -> String {
        format!("{tag}-{}@example.test", uuid::Uuid::new_v4())
    }

    async fn body_json(resp: Response) -> serde_json::Value {
        let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .expect("read body");
        serde_json::from_slice(&bytes).expect("json body")
    }

    /// With no database configured, anything that gets past validation answers
    /// 503 — so a 400 here can only have come from the checks above it, which is
    /// also what pins them *before* the store lookup.
    #[tokio::test]
    async fn a_blank_name_or_email_never_reaches_the_database() {
        let state = test_state(None);

        async fn call(state: Arc<RunsState>, name: &str, email: &str) -> StatusCode {
            set_profile(
                AdminOnly,
                Extension(state),
                AxumPath("some-id".to_string()),
                Json(ProfileRequest {
                    name: name.to_string(),
                    email: email.to_string(),
                }),
            )
            .await
            .status()
        }

        assert_eq!(
            call(state.clone(), "   ", "ada@example.test").await,
            StatusCode::BAD_REQUEST
        );
        assert_eq!(
            call(state.clone(), "", "ada@example.test").await,
            StatusCode::BAD_REQUEST
        );
        assert_eq!(
            call(state.clone(), "Ada", "   ").await,
            StatusCode::BAD_REQUEST
        );
        assert_eq!(
            call(state.clone(), "Ada", "").await,
            StatusCode::BAD_REQUEST
        );
        assert_eq!(
            call(state.clone(), "Ada", "not-an-email").await,
            StatusCode::BAD_REQUEST
        );
        // Valid input gets all the way to the missing store, not a 400.
        assert_eq!(
            call(state.clone(), "Ada", "ada@example.test").await,
            StatusCode::SERVICE_UNAVAILABLE
        );
    }

    /// The address is the account's identity, so a second account must not be
    /// able to take one that is already spoken for — `get_by_email` returning two
    /// rows is an authentication ambiguity, not a display bug. Also covers the
    /// three things that must *not* be conflicts or losses: re-saving your own
    /// address in another case, the stored form being trimmed and lower-cased,
    /// and an unknown id being a 404.
    #[tokio::test]
    async fn an_email_another_account_holds_is_refused() {
        let Some(url) = db_url() else {
            eprintln!("skipping: HARNESS_DATABASE_URL not set");
            return;
        };
        let state = test_state(Some(url));
        let users = state.user_store().await.expect("user store");

        let ada_email = unique_email("ada");
        let grace_email = unique_email("grace");
        let member = |email: &str, name: &str| NewUser {
            email: email.to_string(),
            name: name.to_string(),
            role: "member".to_string(),
            password_hash: None,
        };
        let ada = users
            .create(&member(&ada_email, "Ada"))
            .await
            .expect("create ada");
        let grace = users
            .create(&member(&grace_email, "Grace"))
            .await
            .expect("create grace");

        // Grace's address, in another case, is still Grace's.
        let resp = set_profile(
            AdminOnly,
            Extension(state.clone()),
            AxumPath(ada.id.clone()),
            Json(ProfileRequest {
                name: "Ada".to_string(),
                email: grace_email.to_uppercase(),
            }),
        )
        .await;
        let status = resp.status();
        let body = body_json(resp).await;
        assert_eq!(status, StatusCode::CONFLICT, "{body}");
        assert!(
            body["error"]
                .as_str()
                .unwrap_or_default()
                .contains(&grace_email),
            "the message names the address: {body}"
        );
        // ...and nothing moved on either account.
        assert_eq!(users.get(&ada.id).await.unwrap().unwrap().email, ada_email);
        assert_eq!(
            users.get_by_email(&grace_email).await.unwrap().unwrap().id,
            grace.id
        );

        // Ada's own address, padded and re-cased, is not a clash — and is stored
        // trimmed and lower-cased, so `get_by_email` still finds her.
        let resp = set_profile(
            AdminOnly,
            Extension(state.clone()),
            AxumPath(ada.id.clone()),
            Json(ProfileRequest {
                name: "  Ada Lovelace  ".to_string(),
                email: format!("  {}  ", ada_email.to_uppercase()),
            }),
        )
        .await;
        let status = resp.status();
        let body = body_json(resp).await;
        assert_eq!(status, StatusCode::OK, "{body}");
        assert_eq!(body["email"].as_str(), Some(ada_email.as_str()));
        assert_eq!(body["name"].as_str(), Some("Ada Lovelace"));
        assert!(
            body.get("password_hash").is_none(),
            "no hash goes out: {body}"
        );
        assert_eq!(
            users.get_by_email(&ada_email).await.unwrap().unwrap().id,
            ada.id
        );

        // An id nobody holds is a 404, not a silent success.
        let resp = set_profile(
            AdminOnly,
            Extension(state.clone()),
            AxumPath("no-such-id".to_string()),
            Json(ProfileRequest {
                name: "Nobody".to_string(),
                email: unique_email("nobody"),
            }),
        )
        .await;
        assert_eq!(resp.status(), StatusCode::NOT_FOUND);

        users.delete(&ada.id).await.unwrap();
        users.delete(&grace.id).await.unwrap();
    }
}
