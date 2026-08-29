//! Invitations, and the tokens that let someone set a password.
//!
//! One table serves both: an invite creates an account, a reset repairs one,
//! and the mechanism is identical — a single-use, expiring secret sent by
//! whatever means is to hand.
//!
//! **The token is hashed**, like a personal access token and for the same
//! reason: it is high-entropy and server-generated, so a hash is enough and a
//! database dump reveals nothing usable.
//!
//! **The link is the mechanism, mail is a convenience.** Nothing here sends
//! anything; the caller decides whether to mail the link or show it. That is
//! what keeps an install without SMTP able to add its second person.

use chrono::{DateTime, Duration, Utc};
use serde::Serialize;
use sha2::{Digest, Sha256};
use sqlx::PgPool;

use crate::PersistError;

const CREATE_INVITES: &str = "
CREATE TABLE IF NOT EXISTS harness_invites (
    id          text PRIMARY KEY,
    -- SHA-256 of the token, never the token.
    token_hash  text NOT NULL UNIQUE,
    email       text NOT NULL,
    -- 'invite' creates an account; 'reset' re-passwords an existing one.
    kind        text NOT NULL,
    -- Role the account gets on acceptance. Ignored by a reset.
    role        text NOT NULL DEFAULT 'member',
    -- Who sent it. NULL for a reset the person asked for themselves.
    created_by  text REFERENCES harness_users(id) ON DELETE SET NULL,
    created_at  timestamptz NOT NULL DEFAULT now(),
    expires_at  timestamptz NOT NULL,
    accepted_at timestamptz
)";

const CREATE_INVITES_EMAIL_IDX: &str =
    "CREATE INDEX IF NOT EXISTS harness_invites_email_idx ON harness_invites (email)";

pub const KIND_INVITE: &str = "invite";
pub const KIND_RESET: &str = "reset";

/// How long an invitation stands. Long enough to survive a weekend.
const INVITE_DAYS: i64 = 7;
/// A reset is a response to a request made moments ago, so it is short.
const RESET_HOURS: i64 = 2;

/// An invitation as an administrator sees it — never the token.
#[derive(Debug, Clone, Serialize, sqlx::FromRow)]
pub struct Invite {
    pub id: String,
    pub email: String,
    pub kind: String,
    pub role: String,
    pub created_by: Option<String>,
    pub created_at: DateTime<Utc>,
    pub expires_at: DateTime<Utc>,
    pub accepted_at: Option<DateTime<Utc>>,
}

const INVITE_COLUMNS: &str =
    "id, email, kind, role, created_by, created_at, expires_at, accepted_at";

pub struct InviteStore {
    pool: PgPool,
}

fn hash_token(token: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(token.as_bytes());
    format!("{:x}", hasher.finalize())
}

impl InviteStore {
    /// Connect to `database_url` and ensure the schema exists.
    pub async fn connect(database_url: &str) -> Result<Self, PersistError> {
        let pool = sqlx::postgres::PgPoolOptions::new()
            .max_connections(3)
            .connect(database_url)
            .await?;
        Self::from_pool(pool).await
    }

    /// Wrap an existing pool.
    ///
    /// The accounts table comes first: `created_by` references it, and on a
    /// fresh install this store may be the first thing a request touches.
    pub async fn from_pool(pool: PgPool) -> Result<Self, PersistError> {
        crate::users::ensure_schema(&pool).await?;
        for stmt in [CREATE_INVITES, CREATE_INVITES_EMAIL_IDX] {
            sqlx::query(stmt).execute(&pool).await?;
        }
        Ok(Self { pool })
    }

    /// Record an invitation or a reset. The caller generates the token and
    /// decides how it reaches the person; only the hash is kept.
    ///
    /// Any earlier unaccepted token for the same address and kind is dropped, so
    /// asking twice cannot leave two live links to the same account.
    pub async fn create(
        &self,
        email: &str,
        kind: &str,
        role: &str,
        created_by: Option<&str>,
        token: &str,
    ) -> Result<Invite, PersistError> {
        let email = email.trim().to_lowercase();
        let mut tx = self.pool.begin().await?;
        sqlx::query(
            "DELETE FROM harness_invites
             WHERE email = $1 AND kind = $2 AND accepted_at IS NULL",
        )
        .bind(&email)
        .bind(kind)
        .execute(&mut *tx)
        .await?;

        let ttl = if kind == KIND_RESET {
            Duration::hours(RESET_HOURS)
        } else {
            Duration::days(INVITE_DAYS)
        };
        let sql = format!(
            "INSERT INTO harness_invites
                 (id, token_hash, email, kind, role, created_by, expires_at)
             VALUES ($1, $2, $3, $4, $5, $6, $7) RETURNING {INVITE_COLUMNS}"
        );
        let invite = sqlx::query_as::<_, Invite>(&sql)
            .bind(uuid::Uuid::new_v4().to_string())
            .bind(hash_token(token))
            .bind(&email)
            .bind(kind)
            .bind(role)
            .bind(created_by)
            .bind(Utc::now() + ttl)
            .fetch_one(&mut *tx)
            .await?;
        tx.commit().await?;
        Ok(invite)
    }

    /// The invitation behind a token, if it is live: not spent, not expired.
    pub async fn find_live(&self, token: &str) -> Result<Option<Invite>, PersistError> {
        let sql = format!(
            "SELECT {INVITE_COLUMNS} FROM harness_invites
             WHERE token_hash = $1 AND accepted_at IS NULL AND expires_at > now()"
        );
        Ok(sqlx::query_as::<_, Invite>(&sql)
            .bind(hash_token(token))
            .fetch_optional(&self.pool)
            .await?)
    }

    /// Spend a token. `false` if it was already spent or has expired — which is
    /// what makes acceptance single-use even when two requests race.
    pub async fn consume(&self, token: &str) -> Result<bool, PersistError> {
        let result = sqlx::query(
            "UPDATE harness_invites SET accepted_at = now()
             WHERE token_hash = $1 AND accepted_at IS NULL AND expires_at > now()",
        )
        .bind(hash_token(token))
        .execute(&self.pool)
        .await?;
        Ok(result.rows_affected() > 0)
    }

    /// Outstanding invitations, newest first. Resets are not listed: they are
    /// somebody's private business, not an administrator's worklist.
    pub async fn list_pending(&self) -> Result<Vec<Invite>, PersistError> {
        let sql = format!(
            "SELECT {INVITE_COLUMNS} FROM harness_invites
             WHERE kind = 'invite' AND accepted_at IS NULL AND expires_at > now()
             ORDER BY created_at DESC"
        );
        Ok(sqlx::query_as::<_, Invite>(&sql)
            .fetch_all(&self.pool)
            .await?)
    }

    /// Withdraw one. Returns `true` if it was still outstanding.
    pub async fn revoke(&self, id: &str) -> Result<bool, PersistError> {
        let result =
            sqlx::query("DELETE FROM harness_invites WHERE id = $1 AND accepted_at IS NULL")
                .bind(id)
                .execute(&self.pool)
                .await?;
        Ok(result.rows_affected() > 0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn db_url() -> Option<String> {
        let url = std::env::var("HARNESS_DATABASE_URL").ok()?;
        crate::is_test_db(&url).then_some(url)
    }

    /// A fresh install, where this store connects before anything has created
    /// the accounts table its `created_by` references.
    #[tokio::test]
    #[serial_test::serial]
    async fn the_invite_store_can_be_the_first_thing_to_connect() {
        let Some(url) = db_url() else {
            eprintln!("skipping: HARNESS_DATABASE_URL not set");
            return;
        };
        let pool = sqlx::postgres::PgPoolOptions::new()
            .max_connections(1)
            .connect(&url)
            .await
            .expect("connect");
        sqlx::query(
            "DROP TABLE IF EXISTS harness_invites, harness_tokens, harness_sessions,
             harness_users CASCADE",
        )
        .execute(&pool)
        .await
        .expect("drop");

        let store = InviteStore::connect(&url).await.expect("connect first");
        let invite = store
            .create("First@Example.test", KIND_INVITE, "member", None, "tok-1")
            .await
            .expect("create");
        assert_eq!(invite.email, "first@example.test", "addresses are folded");
        store.revoke(&invite.id).await.unwrap();
    }

    #[tokio::test]
    #[serial_test::serial]
    async fn a_token_is_single_use_and_replaces_its_predecessor() {
        let Some(url) = db_url() else {
            eprintln!("skipping: HARNESS_DATABASE_URL not set");
            return;
        };
        let store = InviteStore::connect(&url).await.expect("connect");
        let email = format!(
            "invitee-{}@example.test",
            Utc::now().timestamp_nanos_opt().unwrap()
        );

        let first = "tok-first";
        store
            .create(&email, KIND_INVITE, "member", None, first)
            .await
            .unwrap();
        assert!(store.find_live(first).await.unwrap().is_some());

        // Inviting again supersedes rather than accumulates: two live links to
        // one account is a way to lose track of who was let in.
        let second = "tok-second";
        store
            .create(&email, KIND_INVITE, "admin", None, second)
            .await
            .unwrap();
        assert!(store.find_live(first).await.unwrap().is_none());
        let live = store.find_live(second).await.unwrap().expect("live");
        assert_eq!(live.role, "admin");

        // The hash authenticates, not the value it hashes.
        assert!(store
            .find_live(&hash_token(second))
            .await
            .unwrap()
            .is_none());

        // Spending it works once.
        assert!(store.consume(second).await.unwrap());
        assert!(!store.consume(second).await.unwrap());
        assert!(store.find_live(second).await.unwrap().is_none());

        // A spent invitation is no longer outstanding.
        let pending = store.list_pending().await.unwrap();
        assert!(!pending.iter().any(|i| i.email == email));
    }

    #[tokio::test]
    #[serial_test::serial]
    async fn an_expired_token_is_not_live() {
        let Some(url) = db_url() else {
            eprintln!("skipping: HARNESS_DATABASE_URL not set");
            return;
        };
        let store = InviteStore::connect(&url).await.expect("connect");
        let email = format!(
            "expired-{}@example.test",
            Utc::now().timestamp_nanos_opt().unwrap()
        );
        let token = "tok-expired";
        let invite = store
            .create(&email, KIND_INVITE, "member", None, token)
            .await
            .unwrap();

        sqlx::query(
            "UPDATE harness_invites SET expires_at = now() - interval '1 minute' WHERE id = $1",
        )
        .bind(&invite.id)
        .execute(&store.pool)
        .await
        .unwrap();

        assert!(store.find_live(token).await.unwrap().is_none());
        assert!(
            !store.consume(token).await.unwrap(),
            "expired cannot be spent"
        );
        store.revoke(&invite.id).await.unwrap();
    }
}
