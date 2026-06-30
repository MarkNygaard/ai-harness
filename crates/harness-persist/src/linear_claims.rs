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

    /// How many **failed/cancelled** attempts this issue has had **for
    /// `workflow`** — the retry counter the poller compares against the binding's
    /// attempt budget to stop a perpetually-failing issue from looping back into
    /// the source column forever.
    ///
    /// Counts only claims whose linked run ended `failed` or `cancelled` (joined
    /// via `run_id`): a **successful** run does not consume the budget, so an
    /// issue that completes once can be picked up again for a legitimately new
    /// round (e.g. `revise-pr` when a reviewer requests changes a second time).
    /// In-flight runs (still `running`) and runs whose row is gone don't count.
    ///
    /// Scoped per `(issue, workflow)`, not per issue: the cap guards a *single*
    /// binding from looping, but must not exhaust an issue that legitimately
    /// flows through several bindings across pipeline stages (e.g. `idea-to-pr`
    /// claims it in `Todo`, then `merge-pr` claims it in `Ready for merge`).
    pub async fn failed_attempts_for_issue(
        &self,
        issue_id: &str,
        workflow: &str,
    ) -> Result<i64, PersistError> {
        let row: (i64,) = sqlx::query_as(
            "SELECT count(*)
             FROM harness_linear_claims c
             JOIN harness_workflow_runs r ON r.id = c.run_id
             WHERE c.issue_id = $1 AND c.workflow = $2
               AND r.status IN ('failed', 'cancelled')",
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

    /// Insert a run row with a terminal `status` so a claim's
    /// `failed_attempts_for_issue` JOIN can see whether its run failed.
    async fn put_run(pool: &PgPool, run_id: &str, status: &str) {
        sqlx::query(
            "INSERT INTO harness_workflow_runs (id, workflow_name, status)
             VALUES ($1, 'wf', $2)
             ON CONFLICT (id) DO UPDATE SET status = EXCLUDED.status",
        )
        .bind(run_id)
        .bind(status)
        .execute(pool)
        .await
        .unwrap();
    }

    /// The retry counter counts only **failed/cancelled** attempts (a success
    /// doesn't burn the budget) and is scoped per `(issue, workflow)` so claims
    /// for one binding's workflow don't count against another's — an issue can
    /// flow `idea-to-pr → merge-pr` without an earlier binding exhausting it.
    #[tokio::test]
    #[serial_test::serial]
    async fn failed_attempts_scoped_per_workflow_and_exclude_success() {
        let Some(url) = db_url() else {
            eprintln!("skipping: HARNESS_DATABASE_URL not set");
            return;
        };
        // Ensure the runs table exists for the JOIN.
        crate::RunStore::connect(&url).await.expect("runs schema");
        let store = LinearClaimStore::connect(&url).await.expect("connect");
        let pool = sqlx::postgres::PgPoolOptions::new()
            .max_connections(2)
            .connect(&url)
            .await
            .expect("pool");
        let ts = chrono::Utc::now().timestamp_nanos_opt().unwrap();
        let issue = format!("issue-{ts}");
        let rid = |s: &str| format!("{s}-{ts}");

        // Two failed/cancelled idea-to-pr attempts and one success for the issue.
        put_run(&pool, &rid("f1"), "failed").await;
        put_run(&pool, &rid("f2"), "cancelled").await;
        put_run(&pool, &rid("ok"), "completed").await;
        for run in [rid("f1"), rid("f2"), rid("ok")] {
            store
                .record(&run, "proj", "idea-to-pr", &issue, "PROJ-1", "todo")
                .await
                .unwrap();
        }

        // Only the failed/cancelled runs count; the success does NOT consume the
        // budget. A different binding's workflow sees zero (scoping).
        assert_eq!(
            store
                .failed_attempts_for_issue(&issue, "idea-to-pr")
                .await
                .unwrap(),
            2
        );
        assert_eq!(
            store
                .failed_attempts_for_issue(&issue, "merge-pr")
                .await
                .unwrap(),
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
        crate::RunStore::connect(&url).await.expect("runs schema");
        let store = LinearClaimStore::connect(&url).await.expect("connect");
        let pool = sqlx::postgres::PgPoolOptions::new()
            .max_connections(2)
            .connect(&url)
            .await
            .expect("pool");
        let ts = chrono::Utc::now().timestamp_nanos_opt().unwrap();
        let issue = format!("issue-{ts}");
        let (rr1, rr2) = (format!("rr-1-{ts}"), format!("rr-2-{ts}"));
        // Both runs failed, so both count toward the budget.
        put_run(&pool, &rr1, "failed").await;
        put_run(&pool, &rr2, "failed").await;
        store
            .record(&rr1, "proj", "wf", &issue, "P-1", "todo")
            .await
            .unwrap();
        store
            .record(&rr2, "proj", "wf", &issue, "P-1", "todo")
            .await
            .unwrap();

        // claim_for_run returns the linked claim; unknown run → None.
        let c = store.claim_for_run(&rr1).await.unwrap().expect("claim");
        assert_eq!(c.issue_id, issue);
        assert_eq!(c.original_state_id, "todo");
        assert!(store.claim_for_run("no-such-run").await.unwrap().is_none());

        // clear_claims removes both rows → the failed-attempt counter resets to 0.
        assert_eq!(
            store.failed_attempts_for_issue(&issue, "wf").await.unwrap(),
            2
        );
        assert!(store.clear_claims(&issue, "wf").await.unwrap() >= 2);
        assert_eq!(
            store.failed_attempts_for_issue(&issue, "wf").await.unwrap(),
            0
        );
        assert!(store.claim_for_run(&rr1).await.unwrap().is_none());
    }
}
