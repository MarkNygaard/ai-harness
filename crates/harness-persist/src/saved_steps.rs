//! Steps people saved from the workflow builder, to reuse in other workflows.
//!
//! A saved step is a **copy**: inserting one pastes its node into the
//! workflow, and changing or deleting the saved step later leaves every
//! workflow that used it as it was.

use chrono::{DateTime, Utc};
use serde::Serialize;
use sqlx::PgPool;

use crate::PersistError;

const CREATE_SAVED_STEPS: &str = "
CREATE TABLE IF NOT EXISTS harness_saved_steps (
    id               text PRIMARY KEY,
    label            text NOT NULL,
    description      text NOT NULL DEFAULT '',
    node             text NOT NULL,
    created_by       text,
    created_by_label text,
    created_at       timestamptz NOT NULL DEFAULT now()
)";

/// One label per step, ignoring case, so the palette never shows two that
/// look the same.
const INDEX_SAVED_STEPS_LABEL: &str = "
CREATE UNIQUE INDEX IF NOT EXISTS harness_saved_steps_label
    ON harness_saved_steps (lower(label))";

/// A saved step (matches `harness_saved_steps`).
#[derive(Debug, Clone, Serialize, sqlx::FromRow)]
pub struct SavedStep {
    pub id: String,
    pub label: String,
    pub description: String,
    /// The node, as the flat JSON the editor and `set_node` use.
    pub node: String,
    /// The account that saved it; `None` on an install without accounts.
    pub created_by: Option<String>,
    /// Who saved it, for a reader: `Name <email>`.
    pub created_by_label: Option<String>,
    pub created_at: DateTime<Utc>,
}

/// Why a save was refused.
#[derive(Debug, thiserror::Error)]
pub enum SaveStepError {
    #[error("a step called `{0}` already exists")]
    DuplicateLabel(String),
    #[error(transparent)]
    Persist(#[from] PersistError),
}

const COLUMNS: &str = "id, label, description, node, created_by, created_by_label, created_at";

/// Postgres-backed store for saved steps.
pub struct SavedStepStore {
    pool: PgPool,
}

impl SavedStepStore {
    pub async fn connect(database_url: &str) -> Result<Self, PersistError> {
        let pool = sqlx::postgres::PgPoolOptions::new()
            .max_connections(2)
            .connect(database_url)
            .await?;
        Self::from_pool(pool).await
    }

    pub async fn from_pool(pool: PgPool) -> Result<Self, PersistError> {
        sqlx::query(CREATE_SAVED_STEPS).execute(&pool).await?;
        sqlx::query(INDEX_SAVED_STEPS_LABEL).execute(&pool).await?;
        Ok(Self { pool })
    }

    /// Every saved step, by label.
    pub async fn list(&self) -> Result<Vec<SavedStep>, PersistError> {
        let rows = sqlx::query_as::<_, SavedStep>(&format!(
            "SELECT {COLUMNS} FROM harness_saved_steps ORDER BY lower(label), id"
        ))
        .fetch_all(&self.pool)
        .await?;
        Ok(rows)
    }

    pub async fn get(&self, id: &str) -> Result<Option<SavedStep>, PersistError> {
        let row = sqlx::query_as::<_, SavedStep>(&format!(
            "SELECT {COLUMNS} FROM harness_saved_steps WHERE id = $1"
        ))
        .bind(id)
        .fetch_optional(&self.pool)
        .await?;
        Ok(row)
    }

    pub async fn create(
        &self,
        id: &str,
        label: &str,
        description: &str,
        node: &str,
        created_by: Option<&str>,
        created_by_label: Option<&str>,
    ) -> Result<SavedStep, SaveStepError> {
        let row = sqlx::query_as::<_, SavedStep>(&format!(
            "INSERT INTO harness_saved_steps
                (id, label, description, node, created_by, created_by_label)
             VALUES ($1, $2, $3, $4, $5, $6)
             RETURNING {COLUMNS}"
        ))
        .bind(id)
        .bind(label)
        .bind(description)
        .bind(node)
        .bind(created_by)
        .bind(created_by_label)
        .fetch_one(&self.pool)
        .await;
        match row {
            Ok(r) => Ok(r),
            Err(sqlx::Error::Database(e)) if e.is_unique_violation() => {
                Err(SaveStepError::DuplicateLabel(label.to_string()))
            }
            Err(e) => Err(SaveStepError::Persist(e.into())),
        }
    }

    pub async fn delete(&self, id: &str) -> Result<bool, PersistError> {
        let res = sqlx::query("DELETE FROM harness_saved_steps WHERE id = $1")
            .bind(id)
            .execute(&self.pool)
            .await?;
        Ok(res.rows_affected() > 0)
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
    async fn a_step_is_saved_listed_and_deleted_and_labels_are_unique() {
        let Some(url) = db_url() else {
            eprintln!("skipping: HARNESS_DATABASE_URL not set");
            return;
        };
        let store = SavedStepStore::connect(&url).await.expect("connect");
        let ts = Utc::now().timestamp_nanos_opt().unwrap();
        let id = format!("step-{ts}");
        let label = format!("File report {ts}");
        let node = r#"{"id":"file-issue","bash":"harness linear create-issue"}"#;
        let saved = store
            .create(
                &id,
                &label,
                "Files the report",
                node,
                Some("u1"),
                Some("Ann"),
            )
            .await
            .unwrap();
        assert_eq!(saved.node, node);
        assert!(store.list().await.unwrap().iter().any(|s| s.id == id));

        // The same label in another case is the same step.
        let dup = store
            .create(
                &format!("{id}-2"),
                &label.to_uppercase(),
                "",
                node,
                None,
                None,
            )
            .await;
        assert!(matches!(dup, Err(SaveStepError::DuplicateLabel(_))));

        assert!(store.delete(&id).await.unwrap());
        assert!(store.get(&id).await.unwrap().is_none());
    }
}
