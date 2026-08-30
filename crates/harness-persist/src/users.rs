//! Accounts and browser sessions.
//!
//! Two roles, `admin` and `member`: the split is *who may change how the harness
//! authenticates and what it authenticates as* — credentials, users, sign-in,
//! mail, domain. Members trigger runs, read reports and author workflows.
//!
//! **Sessions are rows, not tokens.** A session id is stored as a SHA-256 hash
//! and looked up by that hash, so a database dump does not hand over live
//! sessions — the same reason a password is not stored in plaintext. It is also
//! why these are rows at all rather than signed JWTs: signing someone out has to
//! actually sign them out.
//!
//! Password hashing lives in the server crate, which owns the Argon2
//! parameters; this store only ever sees the finished hash.

use chrono::{DateTime, Duration, Utc};
use serde::Serialize;
use sha2::{Digest, Sha256};
use sqlx::PgPool;

use crate::PersistError;

const CREATE_USERS: &str = "
CREATE TABLE IF NOT EXISTS harness_users (
    id            text PRIMARY KEY,
    email         text NOT NULL,
    name          text NOT NULL,
    role          text NOT NULL DEFAULT 'member',
    -- NULL for an account that only signs in through SSO.
    password_hash text,
    created_at    timestamptz NOT NULL DEFAULT now(),
    updated_at    timestamptz NOT NULL DEFAULT now(),
    last_login_at timestamptz,
    disabled_at   timestamptz
)";

/// Case-insensitive uniqueness without depending on the `citext` extension,
/// which is not installed everywhere. Emails are lowercased on write; this stops
/// two rows differing only in case from getting in another way.
const CREATE_USERS_EMAIL_IDX: &str =
    "CREATE UNIQUE INDEX IF NOT EXISTS harness_users_email_key ON harness_users (lower(email))";

const CREATE_SESSIONS: &str = "
CREATE TABLE IF NOT EXISTS harness_sessions (
    -- SHA-256 of the id in the cookie, never the id itself.
    id_hash      text PRIMARY KEY,
    user_id      text NOT NULL REFERENCES harness_users(id) ON DELETE CASCADE,
    created_at   timestamptz NOT NULL DEFAULT now(),
    last_seen_at timestamptz NOT NULL DEFAULT now(),
    expires_at   timestamptz NOT NULL
)";

const CREATE_SESSIONS_USER_IDX: &str =
    "CREATE INDEX IF NOT EXISTS harness_sessions_user_idx ON harness_sessions (user_id)";

/// A registered person. Never carries the password hash outward.
#[derive(Debug, Clone, Serialize, sqlx::FromRow)]
pub struct User {
    pub id: String,
    pub email: String,
    pub name: String,
    /// `admin` or `member`.
    pub role: String,
    pub created_at: DateTime<Utc>,
    pub last_login_at: Option<DateTime<Utc>>,
    /// Set means the account cannot sign in; its rows are kept for attribution.
    pub disabled_at: Option<DateTime<Utc>>,
}

const USER_COLUMNS: &str = "id, email, name, role, created_at, last_login_at, disabled_at";

/// What a profile write did: the account as it now stands, and whether its
/// sessions were ended.
///
/// `sessions_closed` is true when the address changed — an address is an
/// identity, so every session the account held was deleted in the same
/// transaction. It stays true when there was nothing to delete: what the
/// caller needs to know is that the account has to sign in again, not how
/// many browsers it had open.
#[derive(Debug, Clone, Serialize)]
pub struct ProfileUpdate {
    pub user: User,
    pub sessions_closed: bool,
}

/// Fields accepted when creating an account.
#[derive(Debug, Clone)]
pub struct NewUser {
    pub email: String,
    pub name: String,
    pub role: String,
    /// `None` for an SSO-only account.
    pub password_hash: Option<String>,
}

/// Postgres-backed accounts and sessions.
pub struct UserStore {
    pool: PgPool,
}

/// Create the accounts table, if it is not there.
///
/// Shared rather than private to [`UserStore`]: every table with a foreign key
/// into `harness_users` has to be able to ensure it exists first, because
/// whichever store a request happens to touch first is the one that creates its
/// own schema — and on a fresh install that may not be this one.
pub(crate) async fn ensure_schema(pool: &PgPool) -> Result<(), PersistError> {
    sqlx::query(CREATE_USERS).execute(pool).await?;
    sqlx::query(CREATE_USERS_EMAIL_IDX).execute(pool).await?;
    Ok(())
}

fn hash_session_id(id: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(id.as_bytes());
    format!("{:x}", hasher.finalize())
}

impl UserStore {
    /// Connect to `database_url` and ensure the schema exists.
    pub async fn connect(database_url: &str) -> Result<Self, PersistError> {
        let pool = sqlx::postgres::PgPoolOptions::new()
            .max_connections(3)
            .connect(database_url)
            .await?;
        Self::from_pool(pool).await
    }

    /// Wrap an existing pool; ensures the tables and indexes exist.
    pub async fn from_pool(pool: PgPool) -> Result<Self, PersistError> {
        ensure_schema(&pool).await?;
        sqlx::query(CREATE_SESSIONS).execute(&pool).await?;
        sqlx::query(CREATE_SESSIONS_USER_IDX).execute(&pool).await?;
        Ok(Self { pool })
    }

    // ── Accounts ────────────────────────────────────────────────────────────

    /// How many accounts exist. Zero means the install has never been claimed.
    pub async fn count(&self) -> Result<i64, PersistError> {
        let row: (i64,) = sqlx::query_as("SELECT count(*) FROM harness_users")
            .fetch_one(&self.pool)
            .await?;
        Ok(row.0)
    }

    /// How many admins can still sign in. The guard against an install becoming
    /// unadministrable: the last one cannot be demoted, disabled or removed.
    pub async fn active_admin_count(&self) -> Result<i64, PersistError> {
        let row: (i64,) = sqlx::query_as(
            "SELECT count(*) FROM harness_users WHERE role = 'admin' AND disabled_at IS NULL",
        )
        .fetch_one(&self.pool)
        .await?;
        Ok(row.0)
    }

    /// Every account, oldest first.
    pub async fn list(&self) -> Result<Vec<User>, PersistError> {
        let sql = format!("SELECT {USER_COLUMNS} FROM harness_users ORDER BY created_at");
        Ok(sqlx::query_as::<_, User>(&sql)
            .fetch_all(&self.pool)
            .await?)
    }

    /// One account by id.
    pub async fn get(&self, id: &str) -> Result<Option<User>, PersistError> {
        let sql = format!("SELECT {USER_COLUMNS} FROM harness_users WHERE id = $1");
        Ok(sqlx::query_as::<_, User>(&sql)
            .bind(id)
            .fetch_optional(&self.pool)
            .await?)
    }

    /// One account by email, case-insensitively.
    pub async fn get_by_email(&self, email: &str) -> Result<Option<User>, PersistError> {
        let sql =
            format!("SELECT {USER_COLUMNS} FROM harness_users WHERE lower(email) = lower($1)");
        Ok(sqlx::query_as::<_, User>(&sql)
            .bind(email)
            .fetch_optional(&self.pool)
            .await?)
    }

    /// The stored password hash for an email, if the account has one and can
    /// sign in. Separate from [`Self::get_by_email`] so a hash is only read
    /// where it is about to be verified.
    pub async fn password_hash_for(&self, email: &str) -> Result<Option<String>, PersistError> {
        let row: Option<(Option<String>,)> = sqlx::query_as(
            "SELECT password_hash FROM harness_users
             WHERE lower(email) = lower($1) AND disabled_at IS NULL",
        )
        .bind(email)
        .fetch_optional(&self.pool)
        .await?;
        Ok(row.and_then(|r| r.0))
    }

    /// Create an account. Fails if the email is taken.
    pub async fn create(&self, input: &NewUser) -> Result<User, PersistError> {
        let id = uuid::Uuid::new_v4().to_string();
        let sql = format!(
            "INSERT INTO harness_users (id, email, name, role, password_hash)
             VALUES ($1, lower($2), $3, $4, $5)
             RETURNING {USER_COLUMNS}"
        );
        Ok(sqlx::query_as::<_, User>(&sql)
            .bind(&id)
            .bind(input.email.trim())
            .bind(input.name.trim())
            .bind(&input.role)
            .bind(input.password_hash.as_deref())
            .fetch_one(&self.pool)
            .await?)
    }

    /// Change an account's role.
    pub async fn set_role(&self, id: &str, role: &str) -> Result<Option<User>, PersistError> {
        let sql = format!(
            "UPDATE harness_users SET role = $2, updated_at = now()
             WHERE id = $1 RETURNING {USER_COLUMNS}"
        );
        Ok(sqlx::query_as::<_, User>(&sql)
            .bind(id)
            .bind(role)
            .fetch_optional(&self.pool)
            .await?)
    }

    /// Disable or re-enable an account. A disabled account keeps its rows, so
    /// runs it triggered stay attributed.
    pub async fn set_disabled(
        &self,
        id: &str,
        disabled: bool,
    ) -> Result<Option<User>, PersistError> {
        let sql = format!(
            "UPDATE harness_users
             SET disabled_at = CASE WHEN $2 THEN now() ELSE NULL END, updated_at = now()
             WHERE id = $1 RETURNING {USER_COLUMNS}"
        );
        Ok(sqlx::query_as::<_, User>(&sql)
            .bind(id)
            .bind(disabled)
            .fetch_optional(&self.pool)
            .await?)
    }

    /// Replace an account's password hash.
    pub async fn set_password_hash(
        &self,
        id: &str,
        hash: Option<&str>,
    ) -> Result<(), PersistError> {
        sqlx::query(
            "UPDATE harness_users SET password_hash = $2, updated_at = now() WHERE id = $1",
        )
        .bind(id)
        .bind(hash)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    /// Update the display name and email on an account, ending every session it
    /// holds when the address changed.
    ///
    /// An address is an identity, not a label: sign-in matches on it, invitations
    /// match on it, and both SSO providers link an account by it. A session
    /// opened under an address the account no longer holds belongs to an identity
    /// that no longer exists. So the rename and the sign-out are one transaction —
    /// a session cannot survive because a follow-up statement failed, and a
    /// refused rename (a lost race for a taken address) cannot sign anyone out.
    ///
    /// Changing only the name ends nothing. Signing someone out over a corrected
    /// spelling would be a worse bug than the one this prevents.
    ///
    /// `None` means no account holds `id`.
    pub async fn set_profile(
        &self,
        id: &str,
        name: &str,
        email: &str,
    ) -> Result<Option<ProfileUpdate>, PersistError> {
        let name = name.trim();
        let email = email.trim();
        let mut tx = self.pool.begin().await?;
        // Lock the row and compare in the same breath, using the same `lower()`
        // the write below uses. Reading the old address in an earlier statement
        // would compare against a value another administrator may already have
        // replaced — and concluding "unchanged" about a write that does change
        // the address is exactly the bug this method exists to prevent.
        let found: Option<(bool,)> = sqlx::query_as(
            "SELECT lower(email) IS DISTINCT FROM lower($2) FROM harness_users
             WHERE id = $1 FOR UPDATE",
        )
        .bind(id)
        .bind(email)
        .fetch_optional(&mut *tx)
        .await?;
        let Some((email_changed,)) = found else {
            return Ok(None);
        };
        let sql = format!(
            "UPDATE harness_users SET name = $2, email = lower($3), updated_at = now()
             WHERE id = $1 RETURNING {USER_COLUMNS}"
        );
        // Cannot be `None` while the row above is locked, but the contract says
        // `None` means no such account, so honour it rather than assume.
        let Some(user) = sqlx::query_as::<_, User>(&sql)
            .bind(id)
            .bind(name)
            .bind(email)
            .fetch_optional(&mut *tx)
            .await?
        else {
            return Ok(None);
        };
        if email_changed {
            sqlx::query("DELETE FROM harness_sessions WHERE user_id = $1")
                .bind(id)
                .execute(&mut *tx)
                .await?;
        }
        tx.commit().await?;
        Ok(Some(ProfileUpdate {
            user,
            sessions_closed: email_changed,
        }))
    }

    /// Remove an account and every session it holds.
    pub async fn delete(&self, id: &str) -> Result<bool, PersistError> {
        let result = sqlx::query("DELETE FROM harness_users WHERE id = $1")
            .bind(id)
            .execute(&self.pool)
            .await?;
        Ok(result.rows_affected() > 0)
    }

    // ── Sessions ────────────────────────────────────────────────────────────

    /// Open a session for the current identity of `user_id`, returning whether
    /// the account still holds `expected_email`.
    ///
    /// The caller supplies the id so the raw value never has to travel back out
    /// of this store; only its hash is written. The account write is deliberately
    /// first: it locks the row and re-checks the email after any concurrent
    /// profile change commits. If this transaction wins the lock, a following
    /// email change waits and then deletes the session before it commits.
    pub async fn open_session(
        &self,
        session_id: &str,
        user_id: &str,
        expected_email: &str,
        ttl: Duration,
    ) -> Result<bool, PersistError> {
        let mut tx = self.pool.begin().await?;
        let current = sqlx::query(
            "UPDATE harness_users SET last_login_at = now()
             WHERE id = $1 AND lower(email) = lower($2) AND disabled_at IS NULL",
        )
        .bind(user_id)
        .bind(expected_email)
        .execute(&mut *tx)
        .await?;
        if current.rows_affected() == 0 {
            return Ok(false);
        }
        sqlx::query(
            "INSERT INTO harness_sessions (id_hash, user_id, expires_at)
             VALUES ($1, $2, $3)",
        )
        .bind(hash_session_id(session_id))
        .bind(user_id)
        .bind(Utc::now() + ttl)
        .execute(&mut *tx)
        .await?;
        tx.commit().await?;
        Ok(true)
    }

    /// The account behind a session cookie, if the session is live and the
    /// account can still sign in.
    ///
    /// **Using a session extends it.** `idle` is the window measured from this
    /// moment, so someone who visits daily is never signed out; `max_age` is
    /// measured from when the session was opened and is the ceiling that window
    /// cannot pass. Without the ceiling a stolen cookie would live forever,
    /// since an attacker holding one can keep it warm as easily as its owner.
    ///
    /// The write happens in the same statement that touches `last_seen_at`,
    /// which this already did on every request, so extending costs nothing.
    pub async fn user_for_session(
        &self,
        session_id: &str,
        idle: Duration,
        max_age: Duration,
    ) -> Result<Option<User>, PersistError> {
        let hash = hash_session_id(session_id);
        let row: Option<(String,)> = sqlx::query_as(
            "UPDATE harness_sessions
                SET last_seen_at = now(),
                    expires_at = least($2, created_at + ($3::double precision * interval '1 second'))
             WHERE id_hash = $1 AND expires_at > now()
             RETURNING user_id",
        )
        .bind(&hash)
        .bind(Utc::now() + idle)
        .bind(max_age.num_seconds() as f64)
        .fetch_optional(&self.pool)
        .await?;
        let Some((user_id,)) = row else {
            return Ok(None);
        };
        let sql = format!(
            "SELECT {USER_COLUMNS} FROM harness_users WHERE id = $1 AND disabled_at IS NULL"
        );
        Ok(sqlx::query_as::<_, User>(&sql)
            .bind(&user_id)
            .fetch_optional(&self.pool)
            .await?)
    }

    /// End one session.
    pub async fn close_session(&self, session_id: &str) -> Result<(), PersistError> {
        sqlx::query("DELETE FROM harness_sessions WHERE id_hash = $1")
            .bind(hash_session_id(session_id))
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    /// End every session a user holds — what disabling or removing an account,
    /// or changing its password, has to do.
    pub async fn close_sessions_for(&self, user_id: &str) -> Result<u64, PersistError> {
        let result = sqlx::query("DELETE FROM harness_sessions WHERE user_id = $1")
            .bind(user_id)
            .execute(&self.pool)
            .await?;
        Ok(result.rows_affected())
    }

    /// Drop expired sessions. Cheap, and keeps the table from growing forever.
    pub async fn prune_sessions(&self) -> Result<u64, PersistError> {
        let result = sqlx::query("DELETE FROM harness_sessions WHERE expires_at <= now()")
            .execute(&self.pool)
            .await?;
        Ok(result.rows_affected())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Windows long enough not to interfere with whatever a line is checking.
    const HOUR: Duration = Duration::hours(1);
    const NINETY_DAYS: Duration = Duration::days(90);

    fn db_url() -> Option<String> {
        let url = std::env::var("HARNESS_DATABASE_URL").ok()?;
        crate::is_test_db(&url).then_some(url)
    }

    fn unique_email(tag: &str) -> String {
        format!(
            "{tag}-{}@example.test",
            chrono::Utc::now().timestamp_nanos_opt().unwrap()
        )
    }

    #[tokio::test]
    #[serial_test::serial]
    async fn accounts_round_trip() {
        let Some(url) = db_url() else {
            eprintln!("skipping: HARNESS_DATABASE_URL not set");
            return;
        };
        let store = UserStore::connect(&url).await.expect("connect");
        let email = unique_email("round-trip");

        let created = store
            .create(&NewUser {
                email: email.to_uppercase(),
                name: "  Ada  ".into(),
                role: "admin".into(),
                password_hash: Some("hash-1".into()),
            })
            .await
            .expect("create");
        // Emails are stored lowercased and names trimmed.
        assert_eq!(created.email, email.to_lowercase());
        assert_eq!(created.name, "Ada");
        assert_eq!(created.role, "admin");
        assert!(created.last_login_at.is_none());

        // Lookup is case-insensitive, so a login form's casing never matters.
        let found = store
            .get_by_email(&email.to_uppercase())
            .await
            .unwrap()
            .expect("found by email");
        assert_eq!(found.id, created.id);
        assert_eq!(
            store.password_hash_for(&email).await.unwrap().as_deref(),
            Some("hash-1")
        );

        // A second account with the same email in another case is refused.
        let clash = store
            .create(&NewUser {
                email: email.to_uppercase(),
                name: "Impostor".into(),
                role: "member".into(),
                password_hash: None,
            })
            .await;
        assert!(clash.is_err(), "email uniqueness is case-insensitive");

        let demoted = store
            .set_role(&created.id, "member")
            .await
            .unwrap()
            .expect("exists");
        assert_eq!(demoted.role, "member");

        // A disabled account cannot be authenticated against.
        store.set_disabled(&created.id, true).await.unwrap();
        assert_eq!(store.password_hash_for(&email).await.unwrap(), None);
        store.set_disabled(&created.id, false).await.unwrap();
        assert!(store.password_hash_for(&email).await.unwrap().is_some());

        store.delete(&created.id).await.unwrap();
        assert!(store.get(&created.id).await.unwrap().is_none());
    }

    #[tokio::test]
    #[serial_test::serial]
    async fn sessions_are_looked_up_by_hash_and_can_be_revoked() {
        let Some(url) = db_url() else {
            eprintln!("skipping: HARNESS_DATABASE_URL not set");
            return;
        };
        let store = UserStore::connect(&url).await.expect("connect");
        let user = store
            .create(&NewUser {
                email: unique_email("session"),
                name: "Grace".into(),
                role: "member".into(),
                password_hash: Some("hash".into()),
            })
            .await
            .expect("create");

        let sid = uuid::Uuid::new_v4().to_string();
        store
            .open_session(&sid, &user.id, &user.email, Duration::hours(1))
            .await
            .expect("open");

        let who = store
            .user_for_session(&sid, HOUR, NINETY_DAYS)
            .await
            .unwrap()
            .expect("live");
        assert_eq!(who.id, user.id);
        // Opening a session records the login.
        assert!(store
            .get(&user.id)
            .await
            .unwrap()
            .unwrap()
            .last_login_at
            .is_some());

        // An unknown id is not a session, and the raw id is not the stored key.
        assert!(store
            .user_for_session("nope", HOUR, NINETY_DAYS)
            .await
            .unwrap()
            .is_none());
        assert!(store
            .user_for_session(&hash_session_id(&sid), HOUR, NINETY_DAYS)
            .await
            .unwrap()
            .is_none());

        // Disabling the account takes its sessions with it, effectively.
        store.set_disabled(&user.id, true).await.unwrap();
        assert!(store
            .user_for_session(&sid, HOUR, NINETY_DAYS)
            .await
            .unwrap()
            .is_none());
        store.set_disabled(&user.id, false).await.unwrap();
        assert!(store
            .user_for_session(&sid, HOUR, NINETY_DAYS)
            .await
            .unwrap()
            .is_some());

        // Signing out actually signs out.
        store.close_session(&sid).await.unwrap();
        assert!(store
            .user_for_session(&sid, HOUR, NINETY_DAYS)
            .await
            .unwrap()
            .is_none());

        // An expired session is not a session, and pruning removes it.
        let expired = uuid::Uuid::new_v4().to_string();
        store
            .open_session(&expired, &user.id, &user.email, Duration::seconds(-1))
            .await
            .unwrap();
        assert!(store
            .user_for_session(&expired, HOUR, NINETY_DAYS)
            .await
            .unwrap()
            .is_none());
        assert!(store.prune_sessions().await.unwrap() >= 1);

        // Deleting the account cascades to its sessions.
        let live = uuid::Uuid::new_v4().to_string();
        store
            .open_session(&live, &user.id, &user.email, Duration::hours(1))
            .await
            .unwrap();
        store.delete(&user.id).await.unwrap();
        assert!(store
            .user_for_session(&live, HOUR, NINETY_DAYS)
            .await
            .unwrap()
            .is_none());
    }

    /// Using a session carries it forward, up to a ceiling.
    #[tokio::test]
    #[serial_test::serial]
    async fn a_session_slides_while_it_is_used_but_not_past_its_ceiling() {
        let Some(url) = db_url() else {
            eprintln!("skipping: HARNESS_DATABASE_URL not set");
            return;
        };
        let store = UserStore::connect(&url).await.expect("connect");
        let user = store
            .create(&NewUser {
                email: unique_email("slider"),
                name: "Slider".into(),
                role: "member".into(),
                password_hash: Some("hash".into()),
            })
            .await
            .expect("create");

        // This one would lapse in a second if using it did nothing.
        let carried = uuid::Uuid::new_v4().to_string();
        store
            .open_session(&carried, &user.id, &user.email, Duration::seconds(1))
            .await
            .unwrap();
        store
            .user_for_session(&carried, HOUR, NINETY_DAYS)
            .await
            .unwrap()
            .expect("live when first used");
        tokio::time::sleep(std::time::Duration::from_millis(1500)).await;
        assert!(
            store
                .user_for_session(&carried, HOUR, NINETY_DAYS)
                .await
                .unwrap()
                .is_some(),
            "using it should have pushed the window past the original second"
        );

        // The ceiling wins over the slide. With a zero maximum age the window
        // cannot move past the moment the session was opened, so the very next
        // request finds it expired however recently it was used.
        let capped = uuid::Uuid::new_v4().to_string();
        store
            .open_session(&capped, &user.id, &user.email, Duration::hours(1))
            .await
            .unwrap();
        assert!(store
            .user_for_session(&capped, HOUR, Duration::seconds(0))
            .await
            .unwrap()
            .is_some());
        assert!(
            store
                .user_for_session(&capped, HOUR, Duration::seconds(0))
                .await
                .unwrap()
                .is_none(),
            "the ceiling should have clamped the window back to creation time"
        );

        store.delete(&user.id).await.unwrap();
    }

    #[tokio::test]
    #[serial_test::serial]
    async fn admin_count_ignores_members_and_disabled_admins() {
        let Some(url) = db_url() else {
            eprintln!("skipping: HARNESS_DATABASE_URL not set");
            return;
        };
        let store = UserStore::connect(&url).await.expect("connect");
        let before = store.active_admin_count().await.unwrap();

        let admin = store
            .create(&NewUser {
                email: unique_email("admin"),
                name: "Admin".into(),
                role: "admin".into(),
                password_hash: None,
            })
            .await
            .unwrap();
        assert_eq!(store.active_admin_count().await.unwrap(), before + 1);

        // A disabled admin cannot administer anything, so it does not count.
        store.set_disabled(&admin.id, true).await.unwrap();
        assert_eq!(store.active_admin_count().await.unwrap(), before);

        store.delete(&admin.id).await.unwrap();
    }

    #[tokio::test]
    #[serial_test::serial]
    async fn changing_the_email_ends_every_session() {
        let Some(url) = db_url() else {
            eprintln!("skipping: HARNESS_DATABASE_URL not set");
            return;
        };
        let store = UserStore::connect(&url).await.expect("connect");
        let user = store
            .create(&NewUser {
                email: unique_email("mover"),
                name: "Mover".into(),
                role: "member".into(),
                password_hash: Some("hash".into()),
            })
            .await
            .expect("create");

        let sid1 = uuid::Uuid::new_v4().to_string();
        let sid2 = uuid::Uuid::new_v4().to_string();
        store
            .open_session(&sid1, &user.id, &user.email, Duration::hours(1))
            .await
            .expect("open first session");
        store
            .open_session(&sid2, &user.id, &user.email, Duration::hours(1))
            .await
            .expect("open second session");
        assert!(store
            .user_for_session(&sid1, HOUR, NINETY_DAYS)
            .await
            .unwrap()
            .is_some());
        assert!(store
            .user_for_session(&sid2, HOUR, NINETY_DAYS)
            .await
            .unwrap()
            .is_some());

        let new_email = unique_email("moved");
        let update = store
            .set_profile(&user.id, "Moved", &new_email)
            .await
            .expect("set_profile")
            .expect("user exists");
        assert!(update.sessions_closed);
        assert_eq!(update.user.email, new_email.to_lowercase());
        assert_eq!(update.user.name, "Moved");

        assert!(store
            .user_for_session(&sid1, HOUR, NINETY_DAYS)
            .await
            .unwrap()
            .is_none());
        assert!(store
            .user_for_session(&sid2, HOUR, NINETY_DAYS)
            .await
            .unwrap()
            .is_none());
        assert_eq!(store.close_sessions_for(&user.id).await.unwrap(), 0);

        store.delete(&user.id).await.unwrap();
    }

    #[tokio::test]
    #[serial_test::serial]
    async fn a_stale_identity_cannot_open_a_session() {
        let Some(url) = db_url() else {
            eprintln!("skipping: HARNESS_DATABASE_URL not set");
            return;
        };
        let store = UserStore::connect(&url).await.expect("connect");
        let user = store
            .create(&NewUser {
                email: unique_email("stale-session"),
                name: "Stale".into(),
                role: "member".into(),
                password_hash: Some("hash".into()),
            })
            .await
            .expect("create");

        let new_email = unique_email("current-session");
        let update = store
            .set_profile(&user.id, &user.name, &new_email)
            .await
            .expect("set_profile")
            .expect("user exists");
        assert!(update.sessions_closed);

        let stale_sid = uuid::Uuid::new_v4().to_string();
        assert!(!store
            .open_session(&stale_sid, &user.id, &user.email, Duration::hours(1),)
            .await
            .expect("reject stale identity"));
        assert!(store
            .user_for_session(&stale_sid, HOUR, NINETY_DAYS)
            .await
            .unwrap()
            .is_none());

        let current_sid = uuid::Uuid::new_v4().to_string();
        assert!(store
            .open_session(
                &current_sid,
                &user.id,
                &update.user.email,
                Duration::hours(1),
            )
            .await
            .expect("open current identity"));
        assert!(store
            .user_for_session(&current_sid, HOUR, NINETY_DAYS)
            .await
            .unwrap()
            .is_some());

        store.delete(&user.id).await.unwrap();
    }

    #[tokio::test]
    #[serial_test::serial]
    async fn changing_only_the_name_keeps_sessions() {
        let Some(url) = db_url() else {
            eprintln!("skipping: HARNESS_DATABASE_URL not set");
            return;
        };
        let store = UserStore::connect(&url).await.expect("connect");
        let email = unique_email("keeper");
        let user = store
            .create(&NewUser {
                email: email.clone(),
                name: "Keeper".into(),
                role: "member".into(),
                password_hash: Some("hash".into()),
            })
            .await
            .expect("create");

        let sid = uuid::Uuid::new_v4().to_string();
        store
            .open_session(&sid, &user.id, &user.email, Duration::hours(1))
            .await
            .expect("open session");
        assert!(store
            .user_for_session(&sid, HOUR, NINETY_DAYS)
            .await
            .unwrap()
            .is_some());

        // A new name with the same address ends nothing.
        let update = store
            .set_profile(&user.id, "Renamed", &email)
            .await
            .expect("set_profile")
            .expect("user exists");
        assert!(!update.sessions_closed);
        assert_eq!(update.user.name, "Renamed");
        assert!(store
            .user_for_session(&sid, HOUR, NINETY_DAYS)
            .await
            .unwrap()
            .is_some());

        // Re-typing the same address in another case is also not a change.
        let update = store
            .set_profile(&user.id, "Renamed Again", &email.to_uppercase())
            .await
            .expect("set_profile")
            .expect("user exists");
        assert!(!update.sessions_closed);
        assert!(store
            .user_for_session(&sid, HOUR, NINETY_DAYS)
            .await
            .unwrap()
            .is_some());

        // A missing account returns None, not an error.
        assert!(store
            .set_profile("no-such-id", "X", "x@example.test")
            .await
            .unwrap()
            .is_none());

        store.delete(&user.id).await.unwrap();
    }
}
