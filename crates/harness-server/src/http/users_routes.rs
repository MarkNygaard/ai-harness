//! Managing who has an account here.
//!
//! - `GET    /api/users`            — everyone, with their role and last sign-in
//! - `PUT    /api/users/{id}`       — change a display name and email
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

/// The refusal when `email` is already some *other* account's, or `None` when
/// it is free.
///
/// Kept separate from the query so the rule can be read in one place, and so
/// the case that actually matters — an account keeping its own address, or
/// only changing its case — is provably not a conflict with itself.
fn email_taken_message(email: &str) -> String {
    format!("{email} already belongs to another account")
}

fn email_conflict(found: Option<&harness_persist::User>, id: &str, email: &str) -> Option<String> {
    match found {
        Some(other) if other.id != id => Some(email_taken_message(email)),
        _ => None,
    }
}

/// Whether a failed write lost the race for an address.
///
/// The pre-check below is not a lock: two administrators can both read a free
/// address and only one can have it. `harness_users_email_key` (the unique
/// index on `lower(email)`, `harness-persist/src/users.rs`) is what actually
/// keeps two accounts from sharing one, and the loser deserves the same 409
/// as anyone else who asked for a taken address — not a 500.
fn is_email_taken(e: &harness_persist::PersistError) -> bool {
    matches!(
        e,
        harness_persist::PersistError::Db(sqlx::Error::Database(db))
            if db.constraint() == Some("harness_users_email_key")
    )
}

/// `PUT /api/users/{id}` — change an account's display name and email.
///
/// The address is what sign-in and invitations match on, so it is held to the
/// same rule they assume: one account per address, stored lower-cased.
pub async fn set_profile(
    _: AdminOnly,
    Extension(state): Extension<Arc<RunsState>>,
    AxumPath(id): AxumPath<String>,
    Json(req): Json<ProfileRequest>,
) -> Response {
    let name = req.name.trim();
    if name.is_empty() {
        return err(StatusCode::BAD_REQUEST, "a name is required");
    }
    let email = req.email.trim().to_lowercase();
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
    // An id nobody holds is a 404, not a conflict — so read the target before
    // reasoning about whether its new address collides with anyone.
    match users.get(&id).await {
        Ok(Some(_)) => {}
        Ok(None) => return err(StatusCode::NOT_FOUND, "no such account"),
        Err(e) => return err(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()),
    }
    match users.get_by_email(&email).await {
        Ok(found) => {
            if let Some(msg) = email_conflict(found.as_ref(), &id, &email) {
                return err(StatusCode::CONFLICT, msg);
            }
        }
        // Unlike the invite path, a lookup that failed is not treated as
        // "free": guessing wrong here is how two accounts end up sharing an
        // address.
        Err(e) => return err(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()),
    }
    match users.set_profile(&id, name, &email).await {
        Ok(Some(user)) => Json(user).into_response(),
        Ok(None) => err(StatusCode::NOT_FOUND, "no such account"),
        Err(e) if is_email_taken(&e) => err(StatusCode::CONFLICT, email_taken_message(&email)),
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

#[cfg(test)]
mod tests {
    use super::*;
    use axum::extract::Extension;
    use axum::Json;
    use chrono::Utc;
    use harness_agents::registry::AgentRegistry;
    use std::sync::Arc;

    fn user(id: &str, email: &str) -> harness_persist::User {
        harness_persist::User {
            id: id.to_string(),
            email: email.to_string(),
            name: "Test".to_string(),
            role: "member".to_string(),
            created_at: Utc::now(),
            last_login_at: None,
            disabled_at: None,
        }
    }

    #[test]
    fn email_conflict_allows_a_free_address() {
        assert_eq!(email_conflict(None, "a", "x@y.test"), None);
    }

    #[test]
    fn email_conflict_allows_an_account_its_own_address() {
        let same_case = user("a", "x@y.test");
        assert_eq!(email_conflict(Some(&same_case), "a", "x@y.test"), None);

        let different_case = user("a", "X@Y.TEST");
        assert_eq!(email_conflict(Some(&different_case), "a", "x@y.test"), None);
    }

    #[test]
    fn email_conflict_refuses_another_accounts_address() {
        let other = user("b", "x@y.test");
        let result = email_conflict(Some(&other), "a", "x@y.test");
        assert!(result.is_some());
        assert!(result.unwrap().contains("x@y.test"));
    }

    /// Only ever touch an obvious test database, never production. Re-implemented
    /// here because `harness_persist::is_test_db` is `pub(crate)` in another
    /// crate and therefore unreachable from this test module.
    fn is_test_db(url: &str) -> bool {
        let db = url.rsplit('/').next().unwrap_or(url);
        let db = db.split(['?', '#']).next().unwrap_or(db);
        db.to_ascii_lowercase().contains("test")
    }

    fn unique_email(tag: &str) -> String {
        format!(
            "aih37-{}-{}@example.test",
            tag,
            Utc::now().timestamp_nanos_opt().unwrap()
        )
    }

    #[tokio::test]
    async fn duplicate_email_is_refused() {
        let Ok(url) = std::env::var("HARNESS_DATABASE_URL") else {
            eprintln!("skipping: HARNESS_DATABASE_URL not set");
            return;
        };
        if !is_test_db(&url) {
            eprintln!("skipping: HARNESS_DATABASE_URL is not a test database");
            return;
        }

        let state = Arc::new(RunsState::new(
            Some(url),
            Arc::new(AgentRegistry::new("codex")),
            std::path::PathBuf::from("/tmp"),
            None,
            None,
        ));

        let users = state.user_store().await.expect("store");

        let email_a = unique_email("a");
        let email_b = unique_email("b");
        let a = users
            .create(&harness_persist::NewUser {
                email: email_a.clone(),
                name: "A".to_string(),
                role: "member".to_string(),
                password_hash: None,
            })
            .await
            .expect("create a");
        let b = users
            .create(&harness_persist::NewUser {
                email: email_b.clone(),
                name: "B".to_string(),
                role: "member".to_string(),
                password_hash: None,
            })
            .await
            .expect("create b");

        // Trying to take another account's address, even with different case,
        // is a 409.
        let resp = set_profile(
            AdminOnly,
            Extension(state.clone()),
            AxumPath(a.id.clone()),
            Json(ProfileRequest {
                name: "Ada".to_string(),
                email: b.email.to_uppercase(),
            }),
        )
        .await;
        assert_eq!(resp.status(), StatusCode::CONFLICT);

        // A fresh mixed-case address is stored lower-cased and can be looked up.
        let fresh = format!(
            "Fresh-{}@Example.test",
            Utc::now().timestamp_nanos_opt().unwrap()
        );
        let resp = set_profile(
            AdminOnly,
            Extension(state.clone()),
            AxumPath(a.id.clone()),
            Json(ProfileRequest {
                name: "Ada".to_string(),
                email: fresh.clone(),
            }),
        )
        .await;
        assert_eq!(resp.status(), StatusCode::OK);

        let stored = users
            .get_by_email(&fresh.to_lowercase())
            .await
            .expect("lookup")
            .expect("found");
        assert_eq!(stored.id, a.id);
        assert_eq!(stored.email, fresh.to_lowercase());

        // Re-using your own address in a different case is fine.
        let resp = set_profile(
            AdminOnly,
            Extension(state.clone()),
            AxumPath(a.id.clone()),
            Json(ProfileRequest {
                name: "Ada".to_string(),
                email: fresh.to_uppercase(),
            }),
        )
        .await;
        assert_eq!(resp.status(), StatusCode::OK);

        // Clean up best-effort so repeated runs against a persistent test DB
        // do not accumulate rows.
        let _ = users.delete(&a.id).await;
        let _ = users.delete(&b.id).await;
    }
}
