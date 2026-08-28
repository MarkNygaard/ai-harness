//! Personal access tokens.
//!
//! What a program authenticates with once a login sits in front of the UI: an
//! MCP client, a script, CI. A person can complete a browser flow; a program
//! cannot, so it carries a token that belongs to that person — which is also
//! what lets a run triggered over MCP be attributed to somebody.
//!
//! **Stored as a hash, shown once.** Plain SHA-256 rather than Argon2: these are
//! 256 bits of randomness the server generated, so there is no low-entropy guess
//! to slow down, and the hash is verified on every request. A password is the
//! opposite case on both counts.

use chrono::{DateTime, Utc};
use serde::Serialize;
use sha2::{Digest, Sha256};
use sqlx::PgPool;

use crate::PersistError;

const CREATE_TOKENS: &str = "
CREATE TABLE IF NOT EXISTS harness_tokens (
    id           text PRIMARY KEY,
    user_id      text NOT NULL REFERENCES harness_users(id) ON DELETE CASCADE,
    -- What it is for, in the person's words: 'laptop', 'CI'.
    name         text NOT NULL,
    -- SHA-256 of the token, never the token.
    token_hash   text NOT NULL UNIQUE,
    created_at   timestamptz NOT NULL DEFAULT now(),
    last_used_at timestamptz
)";

const CREATE_TOKENS_USER_IDX: &str =
    "CREATE INDEX IF NOT EXISTS harness_tokens_user_idx ON harness_tokens (user_id)";

/// A token as its owner sees it — never the token itself.
#[derive(Debug, Clone, Serialize, sqlx::FromRow)]
pub struct AccessToken {
    pub id: String,
    pub user_id: String,
    pub name: String,
    pub created_at: DateTime<Utc>,
    /// `None` until something authenticates with it. What tells a live token
    /// from one somebody made and forgot.
    pub last_used_at: Option<DateTime<Utc>>,
}

const TOKEN_COLUMNS: &str = "id, user_id, name, created_at, last_used_at";

/// Postgres-backed personal access tokens.
pub struct TokenStore {
    pool: PgPool,
}

fn hash_token(token: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(token.as_bytes());
    format!("{:x}", hasher.finalize())
}

impl TokenStore {
    /// Connect to `database_url` and ensure the schema exists.
    pub async fn connect(database_url: &str) -> Result<Self, PersistError> {
        let pool = sqlx::postgres::PgPoolOptions::new()
            .max_connections(3)
            .connect(database_url)
            .await?;
        Self::from_pool(pool).await
    }

    /// Wrap an existing pool; ensures the table and index exist.
    ///
    /// The accounts table comes first: this one has a foreign key into it, and
    /// on a fresh install the token store may well be the first thing a request
    /// touches.
    pub async fn from_pool(pool: PgPool) -> Result<Self, PersistError> {
        crate::users::ensure_schema(&pool).await?;
        for stmt in [CREATE_TOKENS, CREATE_TOKENS_USER_IDX] {
            sqlx::query(stmt).execute(&pool).await?;
        }
        Ok(Self { pool })
    }

    /// Record a token for `user_id`. The caller generates it and shows it once;
    /// only the hash is kept.
    pub async fn create(
        &self,
        user_id: &str,
        name: &str,
        token: &str,
    ) -> Result<AccessToken, PersistError> {
        let id = uuid::Uuid::new_v4().to_string();
        let sql = format!(
            "INSERT INTO harness_tokens (id, user_id, name, token_hash)
             VALUES ($1, $2, $3, $4) RETURNING {TOKEN_COLUMNS}"
        );
        Ok(sqlx::query_as::<_, AccessToken>(&sql)
            .bind(&id)
            .bind(user_id)
            .bind(name.trim())
            .bind(hash_token(token))
            .fetch_one(&self.pool)
            .await?)
    }

    /// Every token a person holds, newest first.
    pub async fn list_for_user(&self, user_id: &str) -> Result<Vec<AccessToken>, PersistError> {
        let sql = format!(
            "SELECT {TOKEN_COLUMNS} FROM harness_tokens
             WHERE user_id = $1 ORDER BY created_at DESC"
        );
        Ok(sqlx::query_as::<_, AccessToken>(&sql)
            .bind(user_id)
            .fetch_all(&self.pool)
            .await?)
    }

    /// Whose token this is, marking it used.
    ///
    /// Returns the owner's id. The `last_used_at` touch is what turns a list of
    /// tokens into something you can prune with confidence.
    pub async fn owner_of(&self, token: &str) -> Result<Option<String>, PersistError> {
        let row: Option<(String,)> = sqlx::query_as(
            "UPDATE harness_tokens SET last_used_at = now()
             WHERE token_hash = $1 RETURNING user_id",
        )
        .bind(hash_token(token))
        .fetch_optional(&self.pool)
        .await?;
        Ok(row.map(|r| r.0))
    }

    /// Revoke one token, but only if it belongs to `user_id` — so an id from
    /// somebody else's list is not a way to sign their programs out.
    pub async fn revoke(&self, id: &str, user_id: &str) -> Result<bool, PersistError> {
        let result = sqlx::query("DELETE FROM harness_tokens WHERE id = $1 AND user_id = $2")
            .bind(id)
            .bind(user_id)
            .execute(&self.pool)
            .await?;
        Ok(result.rows_affected() > 0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{NewUser, UserStore};

    fn db_url() -> Option<String> {
        let url = std::env::var("HARNESS_DATABASE_URL").ok()?;
        crate::is_test_db(&url).then_some(url)
    }

    async fn a_user(url: &str, tag: &str) -> (UserStore, String) {
        let users = UserStore::connect(url).await.expect("users");
        let user = users
            .create(&NewUser {
                email: format!(
                    "{tag}-{}@example.test",
                    chrono::Utc::now().timestamp_nanos_opt().unwrap()
                ),
                name: tag.into(),
                role: "member".into(),
                password_hash: None,
            })
            .await
            .expect("create user");
        (users, user.id)
    }

    /// Reproduces a fresh install: the token store connecting before anything
    /// has created the accounts table it references.
    ///
    /// This is the failure CI found and a developer machine hides, because a
    /// database that has ever run the other tests already has `harness_users`.
    #[tokio::test]
    #[serial_test::serial]
    async fn the_token_store_can_be_the_first_thing_to_connect() {
        let Some(url) = db_url() else {
            eprintln!("skipping: HARNESS_DATABASE_URL not set");
            return;
        };
        // Tear the accounts tables down so this store is genuinely first.
        let pool = sqlx::postgres::PgPoolOptions::new()
            .max_connections(1)
            .connect(&url)
            .await
            .expect("connect");
        sqlx::query("DROP TABLE IF EXISTS harness_tokens, harness_sessions, harness_users CASCADE")
            .execute(&pool)
            .await
            .expect("drop");

        // Would fail with `relation "harness_users" does not exist` if the token
        // schema did not ensure its dependency first.
        let store = TokenStore::connect(&url)
            .await
            .expect("connect tokens first");
        let (users, user_id) = a_user(&url, "first").await;
        let secret = "hrn_pat_first_connect";
        store
            .create(&user_id, "laptop", secret)
            .await
            .expect("create");
        assert_eq!(
            store.owner_of(secret).await.unwrap().as_deref(),
            Some(user_id.as_str())
        );
        users.delete(&user_id).await.unwrap();
    }

    #[tokio::test]
    #[serial_test::serial]
    async fn tokens_authenticate_their_owner_and_can_be_revoked() {
        let Some(url) = db_url() else {
            eprintln!("skipping: HARNESS_DATABASE_URL not set");
            return;
        };
        let store = TokenStore::connect(&url).await.expect("connect");
        let (users, user_id) = a_user(&url, "tokens").await;

        let secret = "hrn_pat_test_value_one";
        let token = store
            .create(&user_id, "  laptop  ", secret)
            .await
            .expect("create");
        assert_eq!(token.name, "laptop", "names are trimmed");
        assert!(token.last_used_at.is_none(), "unused until it is used");

        // The stored form is a hash: the token authenticates, its hash does not.
        assert_eq!(
            store.owner_of(secret).await.unwrap().as_deref(),
            Some(user_id.as_str())
        );
        assert_eq!(store.owner_of(&hash_token(secret)).await.unwrap(), None);
        assert_eq!(store.owner_of("not-a-token").await.unwrap(), None);

        // Using it is recorded.
        let listed = store.list_for_user(&user_id).await.unwrap();
        assert_eq!(listed.len(), 1);
        assert!(listed[0].last_used_at.is_some());

        // Someone else's id is not a way to revoke your token.
        let (_, other_id) = a_user(&url, "other").await;
        assert!(!store.revoke(&token.id, &other_id).await.unwrap());
        assert!(store.owner_of(secret).await.unwrap().is_some());

        assert!(store.revoke(&token.id, &user_id).await.unwrap());
        assert_eq!(store.owner_of(secret).await.unwrap(), None);

        // Removing the account takes its tokens with it.
        let live = "hrn_pat_test_value_two";
        store.create(&user_id, "ci", live).await.unwrap();
        users.delete(&user_id).await.unwrap();
        assert_eq!(store.owner_of(live).await.unwrap(), None);
        users.delete(&other_id).await.unwrap();
    }
}
