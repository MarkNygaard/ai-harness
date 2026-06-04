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

/// A provider's stored credential, as returned to the API (never the secrets).
#[derive(Debug, Clone, serde::Serialize)]
pub struct ProviderCredential {
    pub provider: String,
    pub configured: bool,
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

    /// Store (upsert) a provider's credential fields.
    pub async fn set(
        &self,
        provider: &str,
        fields: &BTreeMap<String, String>,
    ) -> Result<(), PersistError> {
        let json = serde_json::to_vec(fields).map_err(|e| PersistError::Crypto(e.to_string()))?;
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
}
