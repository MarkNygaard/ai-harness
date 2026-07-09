//! Persisted per-finding triage state for any workflow's report run.
//!
//! When a user acts on a finding in a run's report — "Build this", "Create
//! issue", or "Ignore" — we remember it (keyed by the run + a stable finding
//! key) so the report shows the same checkmarks / dimmed rows next time it's
//! opened. State is scoped to the run that produced the findings; a fresh run
//! starts clean.
//!
//! This is the unified store behind every report (GEO, review, and any
//! workflow that declares `ui.report`). It supersedes the former per-feature
//! `harness_geo_finding_state` / `harness_review_finding_state` tables, whose
//! rows are migrated in on first connect.

use chrono::{DateTime, Utc};
use serde::Serialize;
use sqlx::PgPool;

use crate::PersistError;

const CREATE_FINDING_STATE: &str = "
CREATE TABLE IF NOT EXISTS harness_finding_state (
    run_id           text NOT NULL,
    finding_key      text NOT NULL,
    action           text NOT NULL,
    ref_run_id       text,
    issue_identifier text,
    issue_url        text,
    created_at       timestamptz NOT NULL DEFAULT now(),
    updated_at       timestamptz NOT NULL DEFAULT now(),
    PRIMARY KEY (run_id, finding_key)
)";

/// One-time, idempotent copy of any rows from the legacy per-feature tables
/// (identical columns/order) into the unified table. Guarded by `to_regclass`
/// so it's a no-op when the old tables never existed (fresh DBs); `ON CONFLICT
/// DO NOTHING` makes re-runs harmless.
const MIGRATE_LEGACY_TABLES: &str = "
DO $$
BEGIN
    IF to_regclass('harness_geo_finding_state') IS NOT NULL THEN
        INSERT INTO harness_finding_state
        SELECT * FROM harness_geo_finding_state
        ON CONFLICT (run_id, finding_key) DO NOTHING;
    END IF;
    IF to_regclass('harness_review_finding_state') IS NOT NULL THEN
        INSERT INTO harness_finding_state
        SELECT * FROM harness_review_finding_state
        ON CONFLICT (run_id, finding_key) DO NOTHING;
    END IF;
END $$";

/// One finding's remembered triage state (matches `harness_finding_state`).
#[derive(Debug, Clone, Serialize, sqlx::FromRow)]
pub struct FindingState {
    /// The run whose report this finding belongs to.
    pub run_id: String,
    /// Stable identifier for the finding within the run (`category::title`).
    pub finding_key: String,
    /// What was done: `built` | `issued` | `ignored`.
    pub action: String,
    /// The `idea-to-pr` run created by "Build this", if `action = built`.
    pub ref_run_id: Option<String>,
    /// The Linear issue identifier (e.g. `COR-42`), if `action = issued`.
    pub issue_identifier: Option<String>,
    /// The Linear issue URL, if `action = issued`.
    pub issue_url: Option<String>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

/// Fields accepted when recording a finding's state.
#[derive(Debug, Clone, Default)]
pub struct FindingStateInput {
    pub action: String,
    pub ref_run_id: Option<String>,
    pub issue_identifier: Option<String>,
    pub issue_url: Option<String>,
}

/// Postgres-backed store for finding triage state, shared across all reports.
pub struct FindingStateStore {
    pool: PgPool,
}

impl FindingStateStore {
    /// Connect to `database_url` and ensure the schema exists.
    pub async fn connect(database_url: &str) -> Result<Self, PersistError> {
        let pool = sqlx::postgres::PgPoolOptions::new()
            .max_connections(3)
            .connect(database_url)
            .await?;
        Self::from_pool(pool).await
    }

    /// Wrap an existing pool; ensures the table exists and migrates legacy rows.
    pub async fn from_pool(pool: PgPool) -> Result<Self, PersistError> {
        sqlx::query(CREATE_FINDING_STATE).execute(&pool).await?;
        sqlx::query(MIGRATE_LEGACY_TABLES).execute(&pool).await?;
        Ok(Self { pool })
    }

    /// All remembered finding states for a run.
    pub async fn list_for_run(&self, run_id: &str) -> Result<Vec<FindingState>, PersistError> {
        let rows = sqlx::query_as::<_, FindingState>(
            "SELECT run_id, finding_key, action, ref_run_id, issue_identifier, issue_url,
                    created_at, updated_at
             FROM harness_finding_state
             WHERE run_id = $1
             ORDER BY finding_key",
        )
        .bind(run_id)
        .fetch_all(&self.pool)
        .await?;
        Ok(rows)
    }

    /// Record (upsert) a finding's state. `created_at` is preserved on conflict;
    /// `updated_at` always advances.
    pub async fn set(
        &self,
        run_id: &str,
        finding_key: &str,
        input: &FindingStateInput,
    ) -> Result<FindingState, PersistError> {
        let row = sqlx::query_as::<_, FindingState>(
            "INSERT INTO harness_finding_state (
                run_id, finding_key, action, ref_run_id, issue_identifier, issue_url,
                created_at, updated_at
            ) VALUES ($1, $2, $3, $4, $5, $6, now(), now())
            ON CONFLICT (run_id, finding_key) DO UPDATE SET
                action           = excluded.action,
                ref_run_id       = excluded.ref_run_id,
                issue_identifier = excluded.issue_identifier,
                issue_url        = excluded.issue_url,
                updated_at       = now()
            RETURNING run_id, finding_key, action, ref_run_id, issue_identifier, issue_url,
                      created_at, updated_at",
        )
        .bind(run_id)
        .bind(finding_key)
        .bind(&input.action)
        .bind(input.ref_run_id.as_deref())
        .bind(input.issue_identifier.as_deref())
        .bind(input.issue_url.as_deref())
        .fetch_one(&self.pool)
        .await?;
        Ok(row)
    }

    /// Forget a finding's state (the "Rebuild" / "Unignore" action — restores
    /// the buttons). Returns `true` if a row was removed.
    pub async fn clear(&self, run_id: &str, finding_key: &str) -> Result<bool, PersistError> {
        let result =
            sqlx::query("DELETE FROM harness_finding_state WHERE run_id = $1 AND finding_key = $2")
                .bind(run_id)
                .bind(finding_key)
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
    async fn round_trip_set_list_clear() {
        let Some(url) = db_url() else {
            eprintln!("skipping: HARNESS_DATABASE_URL not set");
            return;
        };
        let store = FindingStateStore::connect(&url).await.expect("connect");
        let run_id = format!(
            "report-run-{}",
            chrono::Utc::now().timestamp_nanos_opt().unwrap()
        );
        let key = "backend::Unvalidated cart total";

        let input = FindingStateInput {
            action: "issued".into(),
            issue_identifier: Some("COR-42".into()),
            issue_url: Some("https://linear.app/acme/issue/COR-42".into()),
            ..Default::default()
        };
        let saved = store.set(&run_id, key, &input).await.unwrap();
        assert_eq!(saved.action, "issued");
        assert_eq!(saved.issue_identifier.as_deref(), Some("COR-42"));

        let list = store.list_for_run(&run_id).await.unwrap();
        assert_eq!(list.len(), 1);
        assert_eq!(list[0].finding_key, key);

        let updated = store
            .set(
                &run_id,
                key,
                &FindingStateInput {
                    action: "built".into(),
                    ref_run_id: Some("run-xyz".into()),
                    ..Default::default()
                },
            )
            .await
            .unwrap();
        assert_eq!(updated.action, "built");
        assert_eq!(updated.ref_run_id.as_deref(), Some("run-xyz"));
        assert_eq!(updated.created_at, saved.created_at);

        assert!(store.clear(&run_id, key).await.unwrap());
        assert!(!store.clear(&run_id, key).await.unwrap());
        assert!(store.list_for_run(&run_id).await.unwrap().is_empty());
    }
}
