//! Encrypted-at-rest provider credentials.
//!
//! Provider tokens (Claude OAuth, Codex `auth.json`, the Kimi API key) are
//! entered in the UI and stored here — **never** in cluster Secrets/SOPS. Each
//! provider's fields are a `field → value` map, serialized to JSON and sealed
//! with **AES-256-GCM** under a key supplied at construction (the
//! `HARNESS_SECRET_KEY` env var, base64 of 32 bytes). The stored blob is
//! `nonce(12) || ciphertext`, so a DB dump alone never reveals tokens.

use std::collections::BTreeMap;

use aes_gcm::aead::Aead;
use aes_gcm::{Aes256Gcm, KeyInit, Nonce};
use base64::Engine;
use rand::RngCore;
use sqlx::postgres::PgPool;

use crate::PersistError;

const NONCE_LEN: usize = 12;

const CREATE_CREDENTIALS: &str = "
CREATE TABLE IF NOT EXISTS harness_credentials (
    provider    text PRIMARY KEY,
    data        bytea NOT NULL,
    updated_at  timestamptz NOT NULL DEFAULT now()
)";

/// Project-scoped credentials (e.g. a per-project Linear/GitHub token for a
/// project that lives in a different account). Resolution is project-first with
/// a fallback to the global [`harness_credentials`] row — see
/// [`CredentialStore::get_for_project`].
const CREATE_PROJECT_CREDENTIALS: &str = "
CREATE TABLE IF NOT EXISTS harness_project_credentials (
    project     text NOT NULL,
    provider    text NOT NULL,
    data        bytea NOT NULL,
    updated_at  timestamptz NOT NULL DEFAULT now(),
    PRIMARY KEY (project, provider)
)";

/// A provider's stored credential, as returned to the API (never the secrets).
#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProviderCredential {
    pub provider: String,
    pub configured: bool,
    /// Whether this provider's dashboard usage card is shown (default true).
    /// A non-secret per-credential preference; only meaningful for providers
    /// that have a usage card (claude, codex, pi/kimi, cursor).
    pub show_usage_card: bool,
}

/// AES-256-GCM-encrypted credential store over Postgres.
pub struct CredentialStore {
    pool: PgPool,
    key: [u8; 32],
}

impl CredentialStore {
    /// Decode a base64 `HARNESS_SECRET_KEY` into a 32-byte AES key.
    pub fn key_from_base64(b64: &str) -> Result<[u8; 32], PersistError> {
        let bytes = base64::engine::general_purpose::STANDARD
            .decode(b64.trim())
            .map_err(|e| PersistError::BadKey(format!("not valid base64: {e}")))?;
        let arr: [u8; 32] = bytes.try_into().map_err(|_| {
            PersistError::BadKey("HARNESS_SECRET_KEY must decode to 32 bytes".into())
        })?;
        Ok(arr)
    }

    /// Wrap an existing pool with a key; ensures the table exists.
    pub async fn from_pool(pool: PgPool, key: [u8; 32]) -> Result<Self, PersistError> {
        let store = Self { pool, key };
        sqlx::query(CREATE_CREDENTIALS).execute(&store.pool).await?;
        sqlx::query(CREATE_PROJECT_CREDENTIALS)
            .execute(&store.pool)
            .await?;
        Ok(store)
    }

    /// Connect to `database_url` with `key` and ensure the schema exists.
    pub async fn connect(database_url: &str, key: [u8; 32]) -> Result<Self, PersistError> {
        let pool = sqlx::postgres::PgPoolOptions::new()
            .max_connections(3)
            .connect(database_url)
            .await?;
        Self::from_pool(pool, key).await
    }

    /// Store a provider's credential fields, **merging** into any already
    /// stored. Editors only ever send the fields the user just typed (secret
    /// values are never returned, so the form is blank), so a plain overwrite
    /// would wipe sibling fields — e.g. saving a commit-author email would drop
    /// the token. Merge keeps the others; clear the whole credential via
    /// [`Self::delete`].
    pub async fn set(
        &self,
        provider: &str,
        fields: &BTreeMap<String, String>,
    ) -> Result<(), PersistError> {
        let mut merged = self.get(provider).await?.unwrap_or_default();
        merged.extend(fields.iter().map(|(k, v)| (k.clone(), v.clone())));
        let json = serde_json::to_vec(&merged).map_err(|e| PersistError::Crypto(e.to_string()))?;
        let blob = encrypt(&self.key, &json)?;
        sqlx::query(
            "INSERT INTO harness_credentials (provider, data, updated_at)
             VALUES ($1, $2, now())
             ON CONFLICT (provider) DO UPDATE SET data = excluded.data, updated_at = now()",
        )
        .bind(provider)
        .bind(blob)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    /// Fetch + decrypt a provider's fields, if present.
    pub async fn get(
        &self,
        provider: &str,
    ) -> Result<Option<BTreeMap<String, String>>, PersistError> {
        let row: Option<(Vec<u8>,)> =
            sqlx::query_as("SELECT data FROM harness_credentials WHERE provider = $1")
                .bind(provider)
                .fetch_optional(&self.pool)
                .await?;
        let Some((blob,)) = row else {
            return Ok(None);
        };
        let plaintext = decrypt(&self.key, &blob)?;
        let fields: BTreeMap<String, String> =
            serde_json::from_slice(&plaintext).map_err(|e| PersistError::Crypto(e.to_string()))?;
        Ok(Some(fields))
    }

    /// Provider names that have a stored credential.
    pub async fn list_configured(&self) -> Result<Vec<String>, PersistError> {
        let rows: Vec<(String,)> =
            sqlx::query_as("SELECT provider FROM harness_credentials ORDER BY provider")
                .fetch_all(&self.pool)
                .await?;
        Ok(rows.into_iter().map(|r| r.0).collect())
    }

    /// Remove a provider's credential.
    pub async fn delete(&self, provider: &str) -> Result<(), PersistError> {
        sqlx::query("DELETE FROM harness_credentials WHERE provider = $1")
            .bind(provider)
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    // ── Project-scoped credentials (project-first, global fallback) ──────────

    /// Store a **project-scoped** credential, **merging** into any already
    /// stored for this (project, provider) — see [`Self::set`] for why.
    pub async fn set_project(
        &self,
        project: &str,
        provider: &str,
        fields: &BTreeMap<String, String>,
    ) -> Result<(), PersistError> {
        let mut merged = self
            .get_project(project, provider)
            .await?
            .unwrap_or_default();
        merged.extend(fields.iter().map(|(k, v)| (k.clone(), v.clone())));
        let json = serde_json::to_vec(&merged).map_err(|e| PersistError::Crypto(e.to_string()))?;
        let blob = encrypt(&self.key, &json)?;
        sqlx::query(
            "INSERT INTO harness_project_credentials (project, provider, data, updated_at)
             VALUES ($1, $2, $3, now())
             ON CONFLICT (project, provider) DO UPDATE SET data = excluded.data, updated_at = now()",
        )
        .bind(project)
        .bind(provider)
        .bind(blob)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    /// Fetch a **project-scoped** credential (no fallback).
    pub async fn get_project(
        &self,
        project: &str,
        provider: &str,
    ) -> Result<Option<BTreeMap<String, String>>, PersistError> {
        let row: Option<(Vec<u8>,)> = sqlx::query_as(
            "SELECT data FROM harness_project_credentials WHERE project = $1 AND provider = $2",
        )
        .bind(project)
        .bind(provider)
        .fetch_optional(&self.pool)
        .await?;
        let Some((blob,)) = row else {
            return Ok(None);
        };
        let plaintext = decrypt(&self.key, &blob)?;
        let fields: BTreeMap<String, String> =
            serde_json::from_slice(&plaintext).map_err(|e| PersistError::Crypto(e.to_string()))?;
        Ok(Some(fields))
    }

    /// Resolve a credential for `project`: the project-scoped row if present,
    /// otherwise the global [`Self::get`] one. This is the lookup integrations
    /// (Linear, GitHub) use so a project in a different account can override the
    /// shared default.
    pub async fn get_for_project(
        &self,
        project: &str,
        provider: &str,
    ) -> Result<Option<BTreeMap<String, String>>, PersistError> {
        if let Some(fields) = self.get_project(project, provider).await? {
            return Ok(Some(fields));
        }
        self.get(provider).await
    }

    /// Project-scoped provider names configured for `project`.
    pub async fn list_project_configured(
        &self,
        project: &str,
    ) -> Result<Vec<String>, PersistError> {
        let rows: Vec<(String,)> = sqlx::query_as(
            "SELECT provider FROM harness_project_credentials WHERE project = $1 ORDER BY provider",
        )
        .bind(project)
        .fetch_all(&self.pool)
        .await?;
        Ok(rows.into_iter().map(|r| r.0).collect())
    }

    /// Remove a project-scoped credential.
    pub async fn delete_project(&self, project: &str, provider: &str) -> Result<(), PersistError> {
        sqlx::query("DELETE FROM harness_project_credentials WHERE project = $1 AND provider = $2")
            .bind(project)
            .bind(provider)
            .execute(&self.pool)
            .await?;
        Ok(())
    }
}

/// AES-256-GCM seal: returns `nonce(12) || ciphertext`.
fn encrypt(key: &[u8; 32], plaintext: &[u8]) -> Result<Vec<u8>, PersistError> {
    let cipher = Aes256Gcm::new(key.into());
    let mut nonce = [0u8; NONCE_LEN];
    rand::rngs::OsRng.fill_bytes(&mut nonce);
    let ct = cipher
        .encrypt(Nonce::from_slice(&nonce), plaintext)
        .map_err(|e| PersistError::Crypto(format!("encrypt: {e}")))?;
    let mut out = Vec::with_capacity(NONCE_LEN + ct.len());
    out.extend_from_slice(&nonce);
    out.extend_from_slice(&ct);
    Ok(out)
}

/// Open a `nonce(12) || ciphertext` blob produced by [`encrypt`].
fn decrypt(key: &[u8; 32], blob: &[u8]) -> Result<Vec<u8>, PersistError> {
    if blob.len() < NONCE_LEN {
        return Err(PersistError::Crypto("ciphertext too short".into()));
    }
    let (nonce, ct) = blob.split_at(NONCE_LEN);
    Aes256Gcm::new(key.into())
        .decrypt(Nonce::from_slice(nonce), ct)
        .map_err(|e| PersistError::Crypto(format!("decrypt (wrong key?): {e}")))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn key_from_base64_round_trips_and_rejects_bad_length() {
        let b64 = base64::engine::general_purpose::STANDARD.encode([9u8; 32]);
        assert_eq!(CredentialStore::key_from_base64(&b64).unwrap(), [9u8; 32]);
        let short = base64::engine::general_purpose::STANDARD.encode([1u8; 16]);
        assert!(CredentialStore::key_from_base64(&short).is_err());
        assert!(CredentialStore::key_from_base64("not base64!!!").is_err());
    }

    #[test]
    fn encrypt_decrypt_round_trips_and_is_nondeterministic() {
        let key = [7u8; 32];
        let pt = b"super-secret-token";
        let a = encrypt(&key, pt).unwrap();
        let b = encrypt(&key, pt).unwrap();
        assert_ne!(a, b, "random nonce → different ciphertext each time");
        assert_eq!(decrypt(&key, &a).unwrap(), pt);
        assert_eq!(decrypt(&key, &b).unwrap(), pt);
        // Wrong key fails.
        assert!(decrypt(&[8u8; 32], &a).is_err());
        // Truncated blob fails cleanly.
        assert!(decrypt(&key, &[0u8; 4]).is_err());
    }

    /// Postgres-dependent: runs only when HARNESS_DATABASE_URL is set (CI).
    #[tokio::test]
    #[serial_test::serial]
    async fn store_set_get_list_delete_round_trip() {
        let Ok(url) = std::env::var("HARNESS_DATABASE_URL") else {
            eprintln!("skipping: HARNESS_DATABASE_URL not set");
            return;
        };
        // Only ever touch an obvious test database, never production.
        if !crate::is_test_db(&url) {
            eprintln!("skipping: HARNESS_DATABASE_URL is not a test database");
            return;
        }
        let store = CredentialStore::connect(&url, [3u8; 32])
            .await
            .expect("connect");
        let provider = format!(
            "test-pi-{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        );
        let mut fields = BTreeMap::new();
        fields.insert("moonshot_api_key".to_string(), "sk-secret-123".to_string());

        store.set(&provider, &fields).await.unwrap();
        let got = store.get(&provider).await.unwrap().expect("present");
        assert_eq!(got.get("moonshot_api_key").unwrap(), "sk-secret-123");
        assert!(store.list_configured().await.unwrap().contains(&provider));

        // A different key cannot decrypt the stored blob.
        let wrong = CredentialStore::connect(&url, [9u8; 32]).await.unwrap();
        assert!(wrong.get(&provider).await.is_err());

        store.delete(&provider).await.unwrap();
        assert!(store.get(&provider).await.unwrap().is_none());
    }

    #[tokio::test]
    #[serial_test::serial]
    async fn set_merges_fields_preserving_siblings() {
        let Ok(url) = std::env::var("HARNESS_DATABASE_URL") else {
            eprintln!("skipping: HARNESS_DATABASE_URL not set");
            return;
        };
        if !crate::is_test_db(&url) {
            eprintln!("skipping: HARNESS_DATABASE_URL is not a test database");
            return;
        }
        let store = CredentialStore::connect(&url, [4u8; 32]).await.unwrap();
        let provider = format!(
            "github-{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        );

        // Set the token, then later add an author email (token field omitted).
        store
            .set(
                &provider,
                &BTreeMap::from([("token".into(), "ghp_x".into())]),
            )
            .await
            .unwrap();
        store
            .set(
                &provider,
                &BTreeMap::from([(
                    "git_author_email".into(),
                    "me@users.noreply.github.com".into(),
                )]),
            )
            .await
            .unwrap();

        let got = store.get(&provider).await.unwrap().expect("present");
        // The token survives the second save (merge, not overwrite).
        assert_eq!(got.get("token").unwrap(), "ghp_x");
        assert_eq!(
            got.get("git_author_email").unwrap(),
            "me@users.noreply.github.com"
        );

        store.delete(&provider).await.unwrap();
    }

    #[tokio::test]
    #[serial_test::serial]
    async fn project_scoped_overrides_global_with_fallback() {
        let Ok(url) = std::env::var("HARNESS_DATABASE_URL") else {
            eprintln!("skipping: HARNESS_DATABASE_URL not set");
            return;
        };
        if !crate::is_test_db(&url) {
            eprintln!("skipping: HARNESS_DATABASE_URL is not a test database");
            return;
        }
        let store = CredentialStore::connect(&url, [5u8; 32]).await.unwrap();
        let provider = format!(
            "linear-{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        );
        let project = "proj-A";
        let field = |v: &str| BTreeMap::from([("api_key".to_string(), v.to_string())]);

        // No creds anywhere → None.
        assert!(store
            .get_for_project(project, &provider)
            .await
            .unwrap()
            .is_none());

        // Global only → resolves to global.
        store.set(&provider, &field("GLOBAL")).await.unwrap();
        assert_eq!(
            store
                .get_for_project(project, &provider)
                .await
                .unwrap()
                .unwrap()["api_key"],
            "GLOBAL"
        );

        // Project override wins.
        store
            .set_project(project, &provider, &field("PROJECT"))
            .await
            .unwrap();
        assert_eq!(
            store
                .get_for_project(project, &provider)
                .await
                .unwrap()
                .unwrap()["api_key"],
            "PROJECT"
        );
        // A different project still falls back to global.
        assert_eq!(
            store
                .get_for_project("proj-B", &provider)
                .await
                .unwrap()
                .unwrap()["api_key"],
            "GLOBAL"
        );

        // Deleting the override falls back to global again.
        store.delete_project(project, &provider).await.unwrap();
        assert_eq!(
            store
                .get_for_project(project, &provider)
                .await
                .unwrap()
                .unwrap()["api_key"],
            "GLOBAL"
        );

        store.delete(&provider).await.unwrap();
    }
}
