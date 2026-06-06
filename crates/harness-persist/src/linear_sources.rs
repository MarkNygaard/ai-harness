//! Persisted Linear trigger binding — one row per (project, workflow).
//!
//! This slice only stores configuration; there is no poller, claim, status
//! transition, or run trigger logic here.

use chrono::{DateTime, Utc};
use serde::Serialize;
use sqlx::PgPool;

use crate::PersistError;

const CREATE_LINEAR_SOURCES: &str = "
CREATE TABLE IF NOT EXISTS harness_linear_sources (
    project              text NOT NULL,
    workflow             text NOT NULL,
    team_id              text NOT NULL,
    team_name            text NOT NULL,
    source_state_id      text NOT NULL,
    label                text,
    in_progress_state_id text,
    review_state_id      text,
    ready_state_id       text,
    base_branch          text,
    poll_interval_secs   integer NOT NULL DEFAULT 60,
    enabled              boolean NOT NULL DEFAULT false,
    created_at           timestamptz NOT NULL DEFAULT now(),
    updated_at           timestamptz NOT NULL DEFAULT now(),
    PRIMARY KEY (project, workflow)
)";

/// A persisted Linear trigger binding (matches `harness_linear_sources`).
#[derive(Debug, Clone, Serialize, sqlx::FromRow)]
pub struct LinearSource {
    pub project: String,
    pub workflow: String,
    pub team_id: String,
    pub team_name: String,
    pub source_state_id: String,
    pub label: Option<String>,
    pub in_progress_state_id: Option<String>,
    pub review_state_id: Option<String>,
    pub ready_state_id: Option<String>,
    pub base_branch: Option<String>,
    pub poll_interval_secs: i32,
    pub enabled: bool,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

/// Fields accepted when creating / updating a Linear source binding.
#[derive(Debug, Clone)]
pub struct LinearSourceInput {
    pub team_id: String,
    pub team_name: String,
    pub source_state_id: String,
    pub label: Option<String>,
    pub in_progress_state_id: Option<String>,
    pub review_state_id: Option<String>,
    pub ready_state_id: Option<String>,
    pub base_branch: Option<String>,
    pub poll_interval_secs: i32,
    pub enabled: bool,
}

/// Postgres-backed store for Linear trigger bindings.
pub struct LinearSourceStore {
    pool: PgPool,
}

impl LinearSourceStore {
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
        sqlx::query(CREATE_LINEAR_SOURCES).execute(&pool).await?;
        Ok(Self { pool })
    }

    /// One binding by composite key, if present.
    pub async fn get(
        &self,
        project: &str,
        workflow: &str,
    ) -> Result<Option<LinearSource>, PersistError> {
        let row = sqlx::query_as::<_, LinearSource>(
            "SELECT project, workflow, team_id, team_name, source_state_id, label,
                    in_progress_state_id, review_state_id, ready_state_id, base_branch,
                    poll_interval_secs, enabled, created_at, updated_at
             FROM harness_linear_sources
             WHERE project = $1 AND workflow = $2",
        )
        .bind(project)
        .bind(workflow)
        .fetch_optional(&self.pool)
        .await?;
        Ok(row)
    }

    /// All bindings for a given project, ordered by workflow name.
    pub async fn list_by_project(&self, project: &str) -> Result<Vec<LinearSource>, PersistError> {
        let rows = sqlx::query_as::<_, LinearSource>(
            "SELECT project, workflow, team_id, team_name, source_state_id, label,
                    in_progress_state_id, review_state_id, ready_state_id, base_branch,
                    poll_interval_secs, enabled, created_at, updated_at
             FROM harness_linear_sources
             WHERE project = $1
             ORDER BY workflow",
        )
        .bind(project)
        .fetch_all(&self.pool)
        .await?;
        Ok(rows)
    }

    /// Create or update a binding (upsert on `(project, workflow)`).
    /// `created_at` is preserved on conflict; `updated_at` always advances.
    pub async fn upsert(
        &self,
        project: &str,
        workflow: &str,
        input: &LinearSourceInput,
    ) -> Result<LinearSource, PersistError> {
        let row = sqlx::query_as::<_, LinearSource>(
            "INSERT INTO harness_linear_sources (
                project, workflow, team_id, team_name, source_state_id,
                label, in_progress_state_id, review_state_id, ready_state_id,
                base_branch, poll_interval_secs, enabled, created_at, updated_at
            ) VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, now(), now())
            ON CONFLICT (project, workflow) DO UPDATE SET
                team_id              = excluded.team_id,
                team_name            = excluded.team_name,
                source_state_id      = excluded.source_state_id,
                label                = excluded.label,
                in_progress_state_id = excluded.in_progress_state_id,
                review_state_id      = excluded.review_state_id,
                ready_state_id       = excluded.ready_state_id,
                base_branch          = excluded.base_branch,
                poll_interval_secs   = excluded.poll_interval_secs,
                enabled              = excluded.enabled,
                updated_at           = now()
            RETURNING project, workflow, team_id, team_name, source_state_id, label,
                      in_progress_state_id, review_state_id, ready_state_id, base_branch,
                      poll_interval_secs, enabled, created_at, updated_at",
        )
        .bind(project)
        .bind(workflow)
        .bind(&input.team_id)
        .bind(&input.team_name)
        .bind(&input.source_state_id)
        .bind(input.label.as_deref())
        .bind(input.in_progress_state_id.as_deref())
        .bind(input.review_state_id.as_deref())
        .bind(input.ready_state_id.as_deref())
        .bind(input.base_branch.as_deref())
        .bind(input.poll_interval_secs)
        .bind(input.enabled)
        .fetch_one(&self.pool)
        .await?;
        Ok(row)
    }

    /// Remove a binding. Returns `true` if a row was deleted.
    pub async fn delete(&self, project: &str, workflow: &str) -> Result<bool, PersistError> {
        let result =
            sqlx::query("DELETE FROM harness_linear_sources WHERE project = $1 AND workflow = $2")
                .bind(project)
                .bind(workflow)
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
    async fn round_trip_upsert_get_list_delete() {
        let Some(url) = db_url() else {
            eprintln!("skipping: HARNESS_DATABASE_URL not set");
            return;
        };
        let store = LinearSourceStore::connect(&url).await.expect("connect");

        let project = "test-proj-linear";
        let workflow = format!("wf-{}", chrono::Utc::now().timestamp_nanos_opt().unwrap());

        // Upsert a binding.
        let input = LinearSourceInput {
            team_id: "team-1".into(),
            team_name: "Engineering".into(),
            source_state_id: "state-1".into(),
            label: None,
            in_progress_state_id: None,
            review_state_id: None,
            ready_state_id: None,
            base_branch: Some("main".into()),
            poll_interval_secs: 120,
            enabled: true,
        };
        let created = store.upsert(project, &workflow, &input).await.unwrap();
        assert_eq!(created.team_id, "team-1");
        assert_eq!(created.team_name, "Engineering");
        assert_eq!(created.source_state_id, "state-1");
        assert_eq!(created.label, None);
        assert_eq!(created.poll_interval_secs, 120);
        assert!(created.enabled);
        assert_eq!(created.base_branch, Some("main".into()));

        // Get returns the row.
        let got = store.get(project, &workflow).await.unwrap();
        assert!(got.is_some());
        let got = got.unwrap();
        assert_eq!(got.team_id, "team-1");

        // Get for unknown key returns None.
        assert!(store.get(project, "unknown-wf").await.unwrap().is_none());

        // Upsert again with changed fields.
        let input2 = LinearSourceInput {
            team_id: "team-2".into(),
            team_name: "Design".into(),
            source_state_id: "state-2".into(),
            label: Some("bug".into()),
            in_progress_state_id: Some("inprog".into()),
            review_state_id: Some("review".into()),
            ready_state_id: Some("ready".into()),
            base_branch: None,
            poll_interval_secs: 30,
            enabled: false,
        };
        let updated = store.upsert(project, &workflow, &input2).await.unwrap();
        assert_eq!(updated.team_id, "team-2");
        assert_eq!(updated.label, Some("bug".into()));
        assert_eq!(updated.created_at, created.created_at);
        // list_by_project returns the binding.
        let list = store.list_by_project(project).await.unwrap();
        assert!(list.iter().any(|s| s.workflow == workflow));

        // list_by_project for an unknown project returns empty.
        let empty = store.list_by_project("no-such-project").await.unwrap();
        assert!(empty.is_empty());

        // Delete returns true, then false on second call.
        assert!(store.delete(project, &workflow).await.unwrap());
        assert!(!store.delete(project, &workflow).await.unwrap());
        assert!(store.get(project, &workflow).await.unwrap().is_none());
    }
}
