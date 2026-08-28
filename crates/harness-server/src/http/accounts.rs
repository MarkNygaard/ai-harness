//! **Accounts** — how the harness decides who is asking.
//!
//! # Three modes
//!
//! | mode | who gets in |
//! |---|---|
//! | `open` | anyone who can reach it |
//! | `token` | anyone holding `HARNESS_API_TOKEN` |
//! | `accounts` | named people, signed in |
//!
//! The first two are what the harness already did; `accounts` is new. The
//! initial mode is **derived from what is already effective** — a token set
//! means `token`, no token means `open` — so upgrading changes nothing for
//! anyone. Only an explicit claim moves an install to `accounts`.
//!
//! **The door only swings one way.** There is no code path from `accounts` back
//! to `open`, in the UI or the CLI, because turning authentication off is not a
//! feature. Getting back *in* after a lockout is a different thing and is what
//! the break-glass CLI is for.
//!
//! # Claiming an install
//!
//! First-to-the-URL-wins is a race a scanner can win, so it is not how this
//! works. The server prints a one-time setup token at boot and writes it beside
//! its data; claiming requires presenting it. **Reading the log or the disk is
//! the proof** — `kubectl logs` on a cluster, a file on bare Docker. An install
//! that already sets `HARNESS_API_TOKEN` may use that instead, since whoever
//! deployed it necessarily knows it.

use std::path::PathBuf;
use std::sync::Arc;

use argon2::password_hash::{rand_core::OsRng, PasswordHash, PasswordHasher, SaltString};
use argon2::{Argon2, PasswordVerifier};
use axum::http::HeaderMap;
use chrono::Duration;
use harness_persist::{SettingsStore, User, UserStore};
use subtle::ConstantTimeEq;

use super::runs_routes::RunsState;

/// Settings key holding the mode. Absent means "never decided".
const MODE_KEY: &str = "auth_mode";
/// Settings key holding the SHA-256 of the current setup token.
const SETUP_TOKEN_KEY: &str = "setup_token_sha256";

/// Cookie carrying the session id.
pub(crate) const SESSION_COOKIE: &str = "harness_session";

/// How long a browser session lasts before it has to sign in again.
const SESSION_TTL_DAYS: i64 = 30;

/// How the harness authenticates callers.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Mode {
    Open,
    Token,
    Accounts,
}

impl Mode {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Mode::Open => "open",
            Mode::Token => "token",
            Mode::Accounts => "accounts",
        }
    }

    fn parse(raw: &str) -> Option<Self> {
        match raw {
            "open" => Some(Mode::Open),
            "token" => Some(Mode::Token),
            "accounts" => Some(Mode::Accounts),
            _ => None,
        }
    }
}

/// The mode this install is in.
///
/// Falls back to what is already effective when nothing is stored — which is
/// both the upgrade path and what happens when the database is unreachable, so
/// a database outage can never silently open an install that had accounts.
pub(crate) async fn mode(state: &Arc<RunsState>) -> Mode {
    let derived = if state.api_token().is_some() {
        Mode::Token
    } else {
        Mode::Open
    };
    let Ok(settings) = state.settings_store().await else {
        return derived;
    };
    match settings.get(MODE_KEY).await {
        Ok(Some(raw)) => Mode::parse(&raw).unwrap_or(derived),
        _ => derived,
    }
}

// ── Passwords ────────────────────────────────────────────────────────────────

/// Hash a password with Argon2id at the crate's default parameters.
pub(crate) fn hash_password(password: &str) -> Result<String, String> {
    let salt = SaltString::generate(&mut OsRng);
    Argon2::default()
        .hash_password(password.as_bytes(), &salt)
        .map(|h| h.to_string())
        .map_err(|e| format!("could not hash password: {e}"))
}

/// Whether `password` matches a stored hash. `false` for a malformed hash
/// rather than an error: a corrupt row must not become a way in.
pub(crate) fn verify_password(password: &str, hash: &str) -> bool {
    let Ok(parsed) = PasswordHash::new(hash) else {
        return false;
    };
    Argon2::default()
        .verify_password(password.as_bytes(), &parsed)
        .is_ok()
}

/// The shortest password worth allowing. Length beats composition rules, which
/// mostly teach people to end passwords with `1!`.
pub(crate) const MIN_PASSWORD_LEN: usize = 12;

/// `Err` with a sentence the person can act on, or `Ok` if it will do.
pub(crate) fn check_password(password: &str) -> Result<(), String> {
    if password.chars().count() < MIN_PASSWORD_LEN {
        return Err(format!(
            "password must be at least {MIN_PASSWORD_LEN} characters"
        ));
    }
    Ok(())
}

// ── The setup token ──────────────────────────────────────────────────────────

fn sha256_hex(value: &str) -> String {
    use sha2::{Digest, Sha256};
    let mut hasher = Sha256::new();
    hasher.update(value.as_bytes());
    format!("{:x}", hasher.finalize())
}

fn mint_setup_token() -> String {
    format!(
        "{}{}",
        uuid::Uuid::new_v4().simple(),
        uuid::Uuid::new_v4().simple()
    )
}

/// Where the setup token is written, for deployments where reading the log is
/// awkward. Beside the project root, which is already a persistent volume.
fn setup_token_path(state: &Arc<RunsState>) -> PathBuf {
    state.projects_dir.join(".harness-setup-token")
}

/// Whether anyone has claimed this install yet.
pub(crate) async fn is_claimed(state: &Arc<RunsState>) -> bool {
    match state.user_store().await {
        Ok(users) => users.count().await.unwrap_or(0) > 0,
        // No database: nothing can have been claimed, and nothing can be.
        Err(_) => false,
    }
}

/// Ensure an unclaimed install has a setup token, announcing it once.
///
/// Called at startup. A claimed install has nothing to announce, and the token
/// is removed the moment it is spent.
pub(crate) fn spawn_setup_token(state: Arc<RunsState>) {
    tokio::spawn(async move {
        if is_claimed(&state).await {
            return;
        }
        let Ok(settings) = state.settings_store().await else {
            return;
        };
        let token = mint_setup_token();
        // `set_if_absent`: two replicas starting together must not each mint a
        // token and each believe theirs is the one.
        match settings
            .set_if_absent(SETUP_TOKEN_KEY, &sha256_hex(&token))
            .await
        {
            Ok(true) => {}
            // Somebody already minted one — theirs is live, and it was printed
            // in that process's log.
            Ok(false) => return,
            Err(e) => {
                tracing::warn!("accounts: could not store a setup token: {e}");
                return;
            }
        }
        let path = setup_token_path(&state);
        let wrote = std::fs::write(&path, format!("{token}\n")).is_ok();
        tracing::warn!(
            "accounts: this harness has no accounts yet. To claim it, open \
             /setup and enter this one-time token:\n\n    {token}\n\n{}",
            if wrote {
                format!("(also written to {})", path.display())
            } else {
                "(could not write it to disk; copy it from here)".to_string()
            }
        );
    });
}

/// Whether `provided` is the live setup token — or, where one is configured,
/// the deployment's `HARNESS_API_TOKEN`.
pub(crate) async fn setup_token_valid(state: &Arc<RunsState>, provided: &str) -> bool {
    let provided = provided.trim();
    if provided.is_empty() {
        return false;
    }
    if let Some(api) = state.api_token() {
        let (a, b) = (api.as_bytes(), provided.as_bytes());
        if a.len() == b.len() && bool::from(a.ct_eq(b)) {
            return true;
        }
    }
    let Ok(settings) = state.settings_store().await else {
        return false;
    };
    let Ok(Some(stored)) = settings.get(SETUP_TOKEN_KEY).await else {
        return false;
    };
    let computed = sha256_hex(provided);
    let (a, b) = (stored.as_bytes(), computed.as_bytes());
    a.len() == b.len() && bool::from(a.ct_eq(b))
}

/// Spend the setup token and switch the install to `accounts`.
///
/// Both are one-way. The token is removed from the database and from disk so it
/// cannot be replayed, and the mode is written so the middleware starts
/// requiring a session.
pub(crate) async fn finish_claim(state: &Arc<RunsState>) -> Result<(), String> {
    let settings = state.settings_store().await?;
    settings
        .set(MODE_KEY, Mode::Accounts.as_str())
        .await
        .map_err(|e| e.to_string())?;
    let _ = settings.delete(SETUP_TOKEN_KEY).await;
    let _ = std::fs::remove_file(setup_token_path(state));
    tracing::info!("accounts: install claimed; authentication is now required");
    Ok(())
}

// ── Sessions ─────────────────────────────────────────────────────────────────

/// Read the session id out of a `Cookie` header.
pub(crate) fn session_id(headers: &HeaderMap) -> Option<String> {
    headers
        .get(axum::http::header::COOKIE)
        .and_then(|v| v.to_str().ok())?
        .split(';')
        .map(str::trim)
        .find_map(|pair| pair.strip_prefix(&format!("{SESSION_COOKIE}=")))
        .map(str::to_string)
        .filter(|v| !v.is_empty())
}

/// The `Set-Cookie` value that starts a session.
///
/// `HttpOnly` keeps it away from scripts, and `SameSite=Lax` is what answers
/// the CSRF that cookie sessions reintroduce and a header-borne token never
/// had. `Secure` is conditional: a harness reached over plain HTTP on a private
/// network would otherwise be handed a cookie the browser refuses to send back.
pub(crate) fn session_cookie(id: &str, secure: bool) -> String {
    let max_age = SESSION_TTL_DAYS * 24 * 3600;
    format!(
        "{SESSION_COOKIE}={id}; Path=/; HttpOnly; SameSite=Lax; Max-Age={max_age}{}",
        if secure { "; Secure" } else { "" }
    )
}

/// The `Set-Cookie` value that ends one.
pub(crate) fn clear_cookie(secure: bool) -> String {
    format!(
        "{SESSION_COOKIE}=; Path=/; HttpOnly; SameSite=Lax; Max-Age=0{}",
        if secure { "; Secure" } else { "" }
    )
}

/// Whether cookies should carry `Secure`, inferred from the public URL.
pub(crate) fn secure_cookies(state: &Arc<RunsState>) -> bool {
    state
        .public_url
        .as_deref()
        .is_some_and(|u| u.starts_with("https://"))
}

/// Start a session for `user`, returning the cookie to set.
pub(crate) async fn open_session(
    state: &Arc<RunsState>,
    users: &UserStore,
    user: &User,
) -> Result<String, String> {
    // A fresh, unguessable id per login — never one the client supplied, which
    // is what makes session fixation impossible rather than merely unlikely.
    let id = format!(
        "{}{}",
        uuid::Uuid::new_v4().simple(),
        uuid::Uuid::new_v4().simple()
    );
    users
        .open_session(&id, &user.id, Duration::days(SESSION_TTL_DAYS))
        .await
        .map_err(|e| e.to_string())?;
    Ok(session_cookie(&id, secure_cookies(state)))
}

/// The signed-in person, if there is one.
pub(crate) async fn current_user(state: &Arc<RunsState>, headers: &HeaderMap) -> Option<User> {
    let id = session_id(headers)?;
    let users = state.user_store().await.ok()?;
    users.user_for_session(&id).await.ok().flatten()
}

/// Settings store handle, or a sentence saying why there isn't one.
pub(crate) async fn settings(state: &Arc<RunsState>) -> Result<&SettingsStore, String> {
    state.settings_store().await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_password_round_trips_and_a_wrong_one_does_not() {
        let hash = hash_password("correct horse battery staple").unwrap();
        assert!(verify_password("correct horse battery staple", &hash));
        assert!(!verify_password("Correct horse battery staple", &hash));
        assert!(!verify_password("", &hash));
        // Two hashes of the same password differ: the salt is doing its job.
        let other = hash_password("correct horse battery staple").unwrap();
        assert_ne!(hash, other);
        assert!(verify_password("correct horse battery staple", &other));
    }

    #[test]
    fn a_malformed_hash_is_never_a_way_in() {
        for junk in ["", "not-a-hash", "$argon2id$v=19$broken"] {
            assert!(!verify_password("anything", junk), "{junk}");
        }
    }

    #[test]
    fn password_length_is_the_only_rule() {
        assert!(check_password(&"a".repeat(MIN_PASSWORD_LEN)).is_ok());
        assert!(check_password(&"a".repeat(MIN_PASSWORD_LEN - 1)).is_err());
        // Counted in characters, not bytes, so a short passphrase in another
        // script isn't accepted just because it encodes long.
        assert!(check_password(&"é".repeat(MIN_PASSWORD_LEN - 1)).is_err());
        assert!(check_password(&"é".repeat(MIN_PASSWORD_LEN)).is_ok());
    }

    #[test]
    fn the_session_cookie_is_read_out_of_a_crowded_header() {
        let mut headers = HeaderMap::new();
        assert_eq!(session_id(&headers), None);

        headers.insert(
            axum::http::header::COOKIE,
            format!("theme=dark; {SESSION_COOKIE}=abc123; other=1")
                .parse()
                .unwrap(),
        );
        assert_eq!(session_id(&headers).as_deref(), Some("abc123"));

        // A cookie whose name merely ends with ours is not ours.
        headers.insert(
            axum::http::header::COOKIE,
            format!("not_{SESSION_COOKIE}=abc123").parse().unwrap(),
        );
        assert_eq!(session_id(&headers), None);

        // Present but empty carries nothing.
        headers.insert(
            axum::http::header::COOKIE,
            format!("{SESSION_COOKIE}=").parse().unwrap(),
        );
        assert_eq!(session_id(&headers), None);
    }

    #[test]
    fn cookies_say_what_they_must() {
        let set = session_cookie("abc", true);
        assert!(set.contains("HttpOnly"), "{set}");
        assert!(set.contains("SameSite=Lax"), "{set}");
        assert!(set.contains("Secure"), "{set}");
        assert!(set.contains("Path=/"), "{set}");

        // Over plain HTTP, `Secure` would stop the browser sending it back.
        assert!(!session_cookie("abc", false).contains("Secure"));

        // Clearing is the same cookie with no life left.
        assert!(clear_cookie(true).contains("Max-Age=0"));
        assert!(clear_cookie(true).contains("HttpOnly"));
    }

    #[test]
    fn modes_round_trip_through_their_stored_form() {
        for m in [Mode::Open, Mode::Token, Mode::Accounts] {
            assert_eq!(Mode::parse(m.as_str()), Some(m));
        }
        assert_eq!(Mode::parse("nonsense"), None);
    }
}
