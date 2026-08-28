//! Small key/value store for instance settings.
//!
//! Things the operator configures about the harness *itself* — which
//! authentication mode it is in, and later its public URL and mail settings.
//! Distinct from [`crate::CredentialStore`], which holds secrets and encrypts
//! them: nothing here is a secret, so nothing here is encrypted, and a value can
//! be read without a key.

use sqlx::PgPool;

use crate::PersistError;

const CREATE_SETTINGS: &str = "
CREATE TABLE IF NOT EXISTS harness_settings (
    key        text PRIMARY KEY,
    value      text NOT NULL,
    updated_at timestamptz NOT NULL DEFAULT now()
)";

/// Postgres-backed instance settings.
pub struct SettingsStore {
    pool: PgPool,
}

impl SettingsStore {
    /// Connect to `database_url` and ensure the schema exists.
    pub async fn connect(database_url: &str) -> Result<Self, PersistError> {
        let pool = sqlx::postgres::PgPoolOptions::new()
            .max_connections(3)
            .connect(database_url)
            .await?;
        Self::from_pool(pool).await
    }

    /// Wrap an existing pool; ensures the table exists.
    pub async fn from_pool(pool: PgPool) -> Result<Self, PersistError> {
        sqlx::query(CREATE_SETTINGS).execute(&pool).await?;
        Ok(Self { pool })
    }

    /// One setting, or `None` if it has never been written.
    pub async fn get(&self, key: &str) -> Result<Option<String>, PersistError> {
        let row: Option<(String,)> =
            sqlx::query_as("SELECT value FROM harness_settings WHERE key = $1")
                .bind(key)
                .fetch_optional(&self.pool)
                .await?;
        Ok(row.map(|r| r.0))
    }

    /// Write a setting, replacing any current value.
    pub async fn set(&self, key: &str, value: &str) -> Result<(), PersistError> {
        sqlx::query(
            "INSERT INTO harness_settings (key, value, updated_at)
             VALUES ($1, $2, now())
             ON CONFLICT (key) DO UPDATE SET value = excluded.value, updated_at = now()",
        )
        .bind(key)
        .bind(value)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    /// Write a setting **only if it has none yet**, and report whether this call
    /// was the one that wrote it.
    ///
    /// This is how a one-way decision is made exactly once: two processes
    /// racing to claim an install both call it, and Postgres decides which one
    /// wins rather than a read-then-write in either of them.
    pub async fn set_if_absent(&self, key: &str, value: &str) -> Result<bool, PersistError> {
        let result = sqlx::query(
            "INSERT INTO harness_settings (key, value) VALUES ($1, $2)
             ON CONFLICT (key) DO NOTHING",
        )
        .bind(key)
        .bind(value)
        .execute(&self.pool)
        .await?;
        Ok(result.rows_affected() > 0)
    }

    /// Remove a setting. Returns `true` if one was there.
    pub async fn delete(&self, key: &str) -> Result<bool, PersistError> {
        let result = sqlx::query("DELETE FROM harness_settings WHERE key = $1")
            .bind(key)
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

    #[tokio::test]
    #[serial_test::serial]
    async fn settings_round_trip_and_claim_once() {
        let Some(url) = db_url() else {
            eprintln!("skipping: HARNESS_DATABASE_URL not set");
            return;
        };
        let store = SettingsStore::connect(&url).await.expect("connect");
        let key = format!("test-{}", chrono::Utc::now().timestamp_nanos_opt().unwrap());

        assert_eq!(store.get(&key).await.unwrap(), None);

        store.set(&key, "first").await.unwrap();
        assert_eq!(store.get(&key).await.unwrap().as_deref(), Some("first"));

        store.set(&key, "second").await.unwrap();
        assert_eq!(store.get(&key).await.unwrap().as_deref(), Some("second"));

        // Only the first writer wins — the claim that can only happen once.
        assert!(!store.set_if_absent(&key, "third").await.unwrap());
        assert_eq!(store.get(&key).await.unwrap().as_deref(), Some("second"));

        assert!(store.delete(&key).await.unwrap());
        assert!(!store.delete(&key).await.unwrap());
        assert!(store.set_if_absent(&key, "fresh").await.unwrap());
        assert_eq!(store.get(&key).await.unwrap().as_deref(), Some("fresh"));

        store.delete(&key).await.unwrap();
    }
}
