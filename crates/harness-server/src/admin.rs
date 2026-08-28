//! **Break-glass account recovery**, for a shell on the server.
//!
//! Signing in can be turned on but never off, so an install with a lost or
//! broken administrator has to be recoverable some other way — otherwise the
//! one-way door is a trap rather than a safeguard. That way is here, and it
//! requires access to the machine, which is the correct bar: whoever can reach
//! the database and the config already owns the deployment.
//!
//! Deliberately narrow. This creates or repairs an administrator; it cannot
//! turn authentication off, because nothing can.

use harness_persist::{NewUser, UserStore};

use crate::http::accounts::{check_password, hash_password};

/// Create an administrator, or promote and re-password an existing account.
///
/// Idempotent on purpose: the person running this has usually just discovered
/// they cannot get in, and should not also have to work out whether their
/// account exists.
pub async fn create_or_promote_admin(
    database_url: &str,
    email: &str,
    name: Option<&str>,
    password: &str,
) -> Result<String, String> {
    check_password(password)?;
    let email = email.trim();
    if email.is_empty() {
        return Err("an email is required".to_string());
    }
    let users = UserStore::connect(database_url)
        .await
        .map_err(|e| format!("could not reach the database: {e}"))?;
    let hash = hash_password(password)?;

    if let Some(existing) = users.get_by_email(email).await.map_err(|e| e.to_string())? {
        users
            .set_password_hash(&existing.id, Some(&hash))
            .await
            .map_err(|e| e.to_string())?;
        users
            .set_role(&existing.id, "admin")
            .await
            .map_err(|e| e.to_string())?;
        // A suspended account that is being recovered should be able to sign in.
        users
            .set_disabled(&existing.id, false)
            .await
            .map_err(|e| e.to_string())?;
        // Old sessions are not this password's sessions.
        users
            .close_sessions_for(&existing.id)
            .await
            .map_err(|e| e.to_string())?;
        return Ok(format!(
            "{email} is now an administrator with a new password; \
             any existing sessions were signed out"
        ));
    }

    let created = users
        .create(&NewUser {
            email: email.to_string(),
            name: name.unwrap_or(email).to_string(),
            role: "admin".into(),
            password_hash: Some(hash),
        })
        .await
        .map_err(|e| e.to_string())?;
    Ok(format!(
        "created administrator {} ({})",
        created.name, created.email
    ))
}
