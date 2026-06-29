//! Linear claim linkage — records which run was fired for which Linear issue,
//! so the live poller can:
//!   - enforce the **per-binding concurrency cap** (don't fire more than the
//!     binding's `max_concurrent_runs` for `(project, workflow)` at once;
//!     defaults to 1 — one at a time),
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

    /// How many active (non-`done`) claims `(project, workflow)` currently has —
    /// the concurrency gate. Compared against the binding's `max_concurrent_runs`
    /// to decide whether the poller may claim another issue.
    pub async fn count_active(&self, project: &str, workflow: &str) -> Result<i64, PersistError> {
        let row: (i64,) = sqlx::query_as(
            "SELECT count(*) FROM harness_linear_claims
             WHERE project = $1 AND workflow = $2 AND phase <> 'done'",
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

    /// How many times this issue has been claimed **for `workflow`** (one row per
    /// attempt) — the retry counter the poller uses to stop a perpetually-failing
    /// issue from looping back into a binding's source column forever.
    ///
    /// Scoped per `(issue, workflow)`, not per issue: the cap guards a *single*
    /// binding from looping, but must not exhaust an issue that legitimately
    /// flows through several bindings across pipeline stages (e.g. `idea-to-pr`
    /// claims it in `Todo`, then `merge-pr` claims it in `Ready for merge`).
    pub async fn attempts_for_issue(
        &self,
        issue_id: &str,
        workflow: &str,
    ) -> Result<i64, PersistError> {
        let row: (i64,) = sqlx::query_as(
            "SELECT count(*) FROM harness_linear_claims WHERE issue_id = $1 AND workflow = $2",
        )
        .bind(issue_id)
        .bind(workflow)
        .fetch_one(&self.pool)
        .await?;
        Ok(row.0)
    }

    /// The claim a given run was fired for, if any. `None` means the run wasn't
    /// Linear-triggered (no claim linkage) — e.g. a manual or MCP run.
    pub async fn claim_for_run(&self, run_id: &str) -> Result<Option<LinearClaim>, PersistError> {
        let row = sqlx::query_as::<_, LinearClaim>(
            "SELECT run_id, project, workflow, issue_id, identifier, original_state_id,
                    phase, created_at
             FROM harness_linear_claims
             WHERE run_id = $1",
        )
        .bind(run_id)
        .fetch_optional(&self.pool)
        .await?;
        Ok(row)
    }

    /// Delete every claim for `(issue, workflow)` — resets the attempt counter so
    /// the issue is fully re-armed (used by the Rerun button). Returns the number
    /// of rows removed.
    pub async fn clear_claims(&self, issue_id: &str, workflow: &str) -> Result<u64, PersistError> {
        let res =
            sqlx::query("DELETE FROM harness_linear_claims WHERE issue_id = $1 AND workflow = $2")
                .bind(issue_id)
                .bind(workflow)
                .execute(&self.pool)
                .await?;
        Ok(res.rows_affected())
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

#[cfg(test)]
mod tests {
    use super::*;

    fn db_url() -> Option<String> {
        let url = std::env::var("HARNESS_DATABASE_URL").ok()?;
        crate::is_test_db(&url).then_some(url)
    }

    /// The retry cap is per `(issue, workflow)`: claims for one binding's
    /// workflow must not count against another's, so an issue can flow
    /// `idea-to-pr → merge-pr` without an earlier binding exhausting it.
    #[tokio::test]
    #[serial_test::serial]
    async fn attempts_are_scoped_per_workflow() {
        let Some(url) = db_url() else {
            eprintln!("skipping: HARNESS_DATABASE_URL not set");
            return;
        };
        let store = LinearClaimStore::connect(&url).await.expect("connect");
        let issue = format!(
            "issue-{}",
            chrono::Utc::now().timestamp_nanos_opt().unwrap()
        );

        // Two idea-to-pr claims for this issue (e.g. one retry).
        store
            .record("run-a", "proj", "idea-to-pr", &issue, "PROJ-1", "todo")
            .await
            .unwrap();
        store
            .record("run-b", "proj", "idea-to-pr", &issue, "PROJ-1", "todo")
            .await
            .unwrap();

        // idea-to-pr sees both; a different binding's workflow sees zero.
        assert_eq!(
            store
                .attempts_for_issue(&issue, "idea-to-pr")
                .await
                .unwrap(),
            2
        );
        assert_eq!(
            store.attempts_for_issue(&issue, "merge-pr").await.unwrap(),
            0
        );
    }

    /// `count_active` counts non-`done` claims per `(project, workflow)` — the
    /// number the poller compares against a binding's `max_concurrent_runs`.
    #[tokio::test]
    #[serial_test::serial]
    async fn count_active_tracks_in_flight_claims_per_binding() {
        let Some(url) = db_url() else {
            eprintln!("skipping: HARNESS_DATABASE_URL not set");
            return;
        };
        let store = LinearClaimStore::connect(&url).await.expect("connect");
        // Unique project so the count isn't polluted by other rows.
        let project = format!("proj-{}", chrono::Utc::now().timestamp_nanos_opt().unwrap());
        let wf = "idea-to-pr";

        assert_eq!(store.count_active(&project, wf).await.unwrap(), 0);

        store
            .record("ca-run-1", &project, wf, "iss-1", "P-1", "todo")
            .await
            .unwrap();
        store
            .record("ca-run-2", &project, wf, "iss-2", "P-2", "todo")
            .await
            .unwrap();
        assert_eq!(store.count_active(&project, wf).await.unwrap(), 2);

        // A `done` claim no longer counts against the cap.
        store.set_phase("ca-run-1", "done").await.unwrap();
        assert_eq!(store.count_active(&project, wf).await.unwrap(), 1);

        // A different workflow on the same project is counted separately.
        assert_eq!(store.count_active(&project, "merge-pr").await.unwrap(), 0);
    }

    /// `claim_for_run` resolves a run's claim; `clear_claims` removes every claim
    /// for `(issue, workflow)`, resetting the attempt counter — the Rerun reset.
    #[tokio::test]
    #[serial_test::serial]
    async fn claim_for_run_and_clear_claims() {
        let Some(url) = db_url() else {
            eprintln!("skipping: HARNESS_DATABASE_URL not set");
            return;
        };
        let store = LinearClaimStore::connect(&url).await.expect("connect");
        let issue = format!(
            "issue-{}",
            chrono::Utc::now().timestamp_nanos_opt().unwrap()
        );
        store
            .record("rr-1", "proj", "wf", &issue, "P-1", "todo")
            .await
            .unwrap();
        store
            .record("rr-2", "proj", "wf", &issue, "P-1", "todo")
            .await
            .unwrap();

        // claim_for_run returns the linked claim; unknown run → None.
        let c = store.claim_for_run("rr-1").await.unwrap().expect("claim");
        assert_eq!(c.issue_id, issue);
        assert_eq!(c.original_state_id, "todo");
        assert!(store.claim_for_run("no-such-run").await.unwrap().is_none());

        // clear_claims removes both rows → the counter resets to 0.
        assert_eq!(store.attempts_for_issue(&issue, "wf").await.unwrap(), 2);
        assert!(store.clear_claims(&issue, "wf").await.unwrap() >= 2);
        assert_eq!(store.attempts_for_issue(&issue, "wf").await.unwrap(), 0);
        assert!(store.claim_for_run("rr-1").await.unwrap().is_none());
    }
}
