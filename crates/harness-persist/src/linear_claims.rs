//! Linear claim linkage — records which run was fired for which Linear issue,
//! so the live poller can:
//!   - enforce **one claim at a time per binding** (don't fire a second run for
//!     `(project, workflow)` while one is still active),
//!   - drive **status transitions** as the run progresses (`phase`), and
//!   - **roll back** the issue to its original state on failure.
//!
//! A claim is keyed by `run_id` (one run per claimed issue). `phase` advances
//! `claimed → in_review → done`; `done` marks the claim inactive.

use chrono::{DateTime, Utc};
use serde::Serialize;
use sqlx::PgPool;

use crate::PersistError;

const CREATE_LINEAR_CLAIMS: &str = "
CREATE TABLE IF NOT EXISTS harness_linear_claims (
    run_id            text PRIMARY KEY,
    project           text NOT NULL,
    workflow          text NOT NULL,
    issue_id          text NOT NULL,
    identifier        text NOT NULL,
    original_state_id text NOT NULL,
    phase             text NOT NULL DEFAULT 'claimed',
    created_at        timestamptz NOT NULL DEFAULT now()
)";

/// A claim row (matches `harness_linear_claims`).
#[derive(Debug, Clone, Serialize, sqlx::FromRow)]
pub struct LinearClaim {
    pub run_id: String,
    pub project: String,
    pub workflow: String,
    /// Linear internal issue id (for state/comment mutations).
    pub issue_id: String,
    /// Human identifier (e.g. `COR-12`), for logs/comments.
    pub identifier: String,
    /// State the issue was in when claimed — restored on failure.
    pub original_state_id: String,
    /// `claimed` → `in_review` → `done`.
    pub phase: String,
    pub created_at: DateTime<Utc>,
}

/// Postgres-backed store for Linear claim linkage.
pub struct LinearClaimStore {
    pool: PgPool,
}

impl LinearClaimStore {
    pub async fn connect(database_url: &str) -> Result<Self, PersistError> {
        let pool = sqlx::postgres::PgPoolOptions::new()
            .max_connections(3)
            .connect(database_url)
            .await?;
        Self::from_pool(pool).await
    }

    pub async fn from_pool(pool: PgPool) -> Result<Self, PersistError> {
        sqlx::query(CREATE_LINEAR_CLAIMS).execute(&pool).await?;
        Ok(Self { pool })
    }

    /// Record a new claim (phase `claimed`). Idempotent on `run_id`.
    #[allow(clippy::too_many_arguments)]
    pub async fn record(
        &self,
        run_id: &str,
        project: &str,
        workflow: &str,
        issue_id: &str,
        identifier: &str,
        original_state_id: &str,
    ) -> Result<(), PersistError> {
        sqlx::query(
            "INSERT INTO harness_linear_claims
                (run_id, project, workflow, issue_id, identifier, original_state_id, phase, created_at)
             VALUES ($1, $2, $3, $4, $5, $6, 'claimed', now())
             ON CONFLICT (run_id) DO NOTHING",
        )
        .bind(run_id)
        .bind(project)
        .bind(workflow)
        .bind(issue_id)
        .bind(identifier)
        .bind(original_state_id)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    /// Whether `(project, workflow)` has an active (non-`done`) claim — the
    /// one-at-a-time gate.
    pub async fn has_active(&self, project: &str, workflow: &str) -> Result<bool, PersistError> {
        let row: (bool,) = sqlx::query_as(
            "SELECT EXISTS(
                SELECT 1 FROM harness_linear_claims
                WHERE project = $1 AND workflow = $2 AND phase <> 'done'
            )",
        )
        .bind(project)
        .bind(workflow)
        .fetch_one(&self.pool)
        .await?;
        Ok(row.0)
    }

    /// All active (non-`done`) claims — the status-sync work-list.
    pub async fn list_active(&self) -> Result<Vec<LinearClaim>, PersistError> {
        let rows = sqlx::query_as::<_, LinearClaim>(
            "SELECT run_id, project, workflow, issue_id, identifier, original_state_id,
                    phase, created_at
             FROM harness_linear_claims
             WHERE phase <> 'done'
             ORDER BY created_at",
        )
        .fetch_all(&self.pool)
        .await?;
        Ok(rows)
    }

    /// Advance a claim's phase.
    pub async fn set_phase(&self, run_id: &str, phase: &str) -> Result<(), PersistError> {
        sqlx::query("UPDATE harness_linear_claims SET phase = $2 WHERE run_id = $1")
            .bind(run_id)
            .bind(phase)
            .execute(&self.pool)
            .await?;
        Ok(())
    }
}
