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

/// Runs triggered by **delegation** carry the Linear agent session that asked for
/// them, so status-sync can report progress back into that session's thread
/// instead of as a detached comment. Null for poller-claimed runs.
const ADD_AGENT_SESSION_ID: &str =
    "ALTER TABLE harness_linear_claims ADD COLUMN IF NOT EXISTS agent_session_id text";

/// Node ids already reported into the agent session, comma-separated. Stored
/// rather than derived because status-sync runs every 30s and must not re-post an
/// activity it already sent. An exact set (not a high-water mark) because a DAG
/// finishes nodes out of ordinal order when branches run in parallel.
const ADD_REPORTED_NODES: &str =
    "ALTER TABLE harness_linear_claims ADD COLUMN IF NOT EXISTS reported_nodes text NOT NULL DEFAULT ''";

/// When an activity was last posted into the session. Drives the heartbeat that
/// keeps a session alive through a single long-running node — Linear marks a
/// session stale after 30 minutes without one.
///
/// In the database rather than in memory so a harness restart mid-run neither
/// loses track (letting the session go stale) nor re-posts a burst.
const ADD_LAST_ACTIVITY_AT: &str = "ALTER TABLE harness_linear_claims \
     ADD COLUMN IF NOT EXISTS last_activity_at timestamptz NOT NULL DEFAULT now()";

/// Retire every active claim but the first for each `(issue, workflow)`.
///
/// Runs once, immediately before the unique index below, because a database that
/// pre-dates it can already hold the state that index forbids — the duplicate
/// claims are what this whole guard exists to stop, and the index cannot be
/// built over them. The earliest `(created_at, run_id)` wins: it is the claim
/// that really did take the issue, and the tuple keeps the choice deterministic
/// when two claims land in the same millisecond (which is exactly how they land).
const RETIRE_DUPLICATE_ACTIVE_CLAIMS: &str = "
UPDATE harness_linear_claims AS c SET phase = 'done'
 WHERE c.phase <> 'done'
   AND EXISTS (
        SELECT 1 FROM harness_linear_claims AS winner
         WHERE winner.phase <> 'done'
           AND winner.issue_id = c.issue_id
           AND winner.workflow = c.workflow
           AND (winner.created_at, winner.run_id) < (c.created_at, c.run_id)
   )";

/// At most one **active** claim per `(issue, workflow)`.
///
/// Both entry points — the column poller and the delegation webhook — used to
/// treat "the issue left its source column" as the claim signal, read from
/// Linear and acted on independently. That is a check-then-act with nothing
/// holding the gap, and the only backstop was the binding's concurrency cap,
/// which stops nothing while a slot is free. ECOM-44 arrived through both paths
/// 90ms apart and got two runs, two branches and two pull requests.
///
/// Partial, on `phase <> 'done'`: a claim retires to `done` on every exit —
/// completed, rolled back after failure, or dropped when its run row never
/// appeared — so an issue can still be claimed again later, by this workflow or
/// another. This forbids only two at once.
const ADD_ONE_ACTIVE_CLAIM_PER_ISSUE: &str = "
CREATE UNIQUE INDEX IF NOT EXISTS harness_linear_claims_one_active_per_issue
    ON harness_linear_claims (issue_id, workflow) WHERE phase <> 'done'";

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
    /// Linear agent session that delegated this run, when it came from a
    /// delegation/mention rather than the column poller. Progress is reported
    /// into this session as agent activities.
    pub agent_session_id: Option<String>,
    /// Node ids already reported into the session, comma-separated.
    pub reported_nodes: String,
    /// When an activity was last posted into the session.
    pub last_activity_at: DateTime<Utc>,
    pub created_at: DateTime<Utc>,
}

impl LinearClaim {
    /// Whether `node_id` has already been reported into the session.
    pub fn has_reported(&self, node_id: &str) -> bool {
        self.reported_nodes.split(',').any(|n| n == node_id)
    }

    /// `reported_nodes` with `node_ids` appended, skipping any already present.
    pub fn with_reported(&self, node_ids: &[String]) -> String {
        let mut out: Vec<&str> = self
            .reported_nodes
            .split(',')
            .filter(|n| !n.is_empty())
            .collect();
        for id in node_ids {
            if !out.iter().any(|n| n == id) {
                out.push(id);
            }
        }
        out.join(",")
    }
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
        sqlx::query(ADD_AGENT_SESSION_ID).execute(&pool).await?;
        sqlx::query(ADD_REPORTED_NODES).execute(&pool).await?;
        sqlx::query(ADD_LAST_ACTIVITY_AT).execute(&pool).await?;
        // Order matters: the index cannot be built over the duplicates.
        sqlx::query(RETIRE_DUPLICATE_ACTIVE_CLAIMS)
            .execute(&pool)
            .await?;
        sqlx::query(ADD_ONE_ACTIVE_CLAIM_PER_ISSUE)
            .execute(&pool)
            .await?;
        Ok(Self { pool })
    }

    /// Record a new claim (phase `claimed`). Idempotent on `run_id`.
    ///
    /// `agent_session_id` links the claim to the Linear agent session that
    /// delegated the work; `None` for a run the column poller claimed itself.
    ///
    /// Returns whether the row was written. `false` means the claim was refused:
    /// either this run already had one, or — the case worth reacting to — the
    /// issue already has an active claim for this workflow and something raced
    /// past the checks upstream. A caller that gets `false` has a run it should
    /// not have started.
    #[allow(clippy::too_many_arguments)]
    pub async fn record(
        &self,
        run_id: &str,
        project: &str,
        workflow: &str,
        issue_id: &str,
        identifier: &str,
        original_state_id: &str,
        agent_session_id: Option<&str>,
    ) -> Result<bool, PersistError> {
        // Untargeted `DO NOTHING`: the run_id primary key and the one-active-
        // claim-per-issue index both have to fall through to "not written"
        // rather than an error, and they mean different things to the caller
        // only in the log line it writes.
        let done = sqlx::query(
            "INSERT INTO harness_linear_claims
                (run_id, project, workflow, issue_id, identifier, original_state_id,
                 phase, agent_session_id, created_at)
             VALUES ($1, $2, $3, $4, $5, $6, 'claimed', $7, now())
             ON CONFLICT DO NOTHING",
        )
        .bind(run_id)
        .bind(project)
        .bind(workflow)
        .bind(issue_id)
        .bind(identifier)
        .bind(original_state_id)
        .bind(agent_session_id)
        .execute(&self.pool)
        .await?;
        Ok(done.rows_affected() > 0)
    }

    /// Whether `(issue, workflow)` already has an active claim — i.e. a run is
    /// in flight for this issue and there must not be a second.
    ///
    /// The durable half of the duplicate-claim guard, and the half that survives
    /// a restart: the in-process guard in `linear_agent` closes the concurrent
    /// window inside one container, and this catches a claim recorded by the
    /// process that was running before this one.
    pub async fn claim_exists_for_issue(
        &self,
        issue_id: &str,
        workflow: &str,
    ) -> Result<bool, PersistError> {
        let row: (bool,) = sqlx::query_as(
            "SELECT EXISTS (
                 SELECT 1 FROM harness_linear_claims
                  WHERE issue_id = $1 AND workflow = $2 AND phase <> 'done'
             )",
        )
        .bind(issue_id)
        .bind(workflow)
        .fetch_one(&self.pool)
        .await?;
        Ok(row.0)
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
                    phase, agent_session_id, reported_nodes, last_activity_at, created_at
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

    /// The column an issue was last picked up from to be **built**, ignoring
    /// claims made by `exclude_workflow`.
    ///
    /// This is what "where does a piece of an epic start" resolves to without
    /// anybody configuring it. The poller records `original_state_id` on every
    /// claim, so the column that triggers a build is a fact already written
    /// down: for an epic it is where the epic itself was claimed from, and for a
    /// merged piece it is where that piece was claimed from — the same binding
    /// in both cases, because an epic and its pieces are picked up by one.
    ///
    /// `exclude_workflow` is the supervisor: it claims from the column a merged
    /// piece rests in, which is where work *ends*, and returning that would send
    /// the next piece straight to Done.
    pub async fn build_state_for_issue(
        &self,
        issue_id: &str,
        exclude_workflow: &str,
    ) -> Result<Option<String>, PersistError> {
        let row: Option<(String,)> = sqlx::query_as(
            "SELECT original_state_id
             FROM harness_linear_claims
             WHERE issue_id = $1 AND workflow <> $2 AND original_state_id <> ''
             ORDER BY created_at DESC
             LIMIT 1",
        )
        .bind(issue_id)
        .bind(exclude_workflow)
        .fetch_optional(&self.pool)
        .await?;
        Ok(row.map(|r| r.0))
    }

    /// Whether any claim already exists for this Linear **agent session** — the
    /// idempotency check for a redelivered `AgentSessionEvent`.
    ///
    /// Linear resends a delivery that took over 5 seconds (then again after 1
    /// minute, 1 hour and 6 hours), and each resend carries the same session id.
    /// Without this, a slow first response would start a second run for one
    /// delegation. Deliberately *not* a unique constraint on the column: a Rerun
    /// re-records the same session against a new `run_id` on purpose.
    pub async fn claim_exists_for_session(
        &self,
        agent_session_id: &str,
    ) -> Result<bool, PersistError> {
        let row: (bool,) = sqlx::query_as(
            "SELECT EXISTS (
                 SELECT 1 FROM harness_linear_claims WHERE agent_session_id = $1
             )",
        )
        .bind(agent_session_id)
        .fetch_one(&self.pool)
        .await?;
        Ok(row.0)
    }

    /// The claim a given run was fired for, if any. `None` means the run wasn't
    /// Linear-triggered (no claim linkage) — e.g. a manual or MCP run.
    pub async fn claim_for_run(&self, run_id: &str) -> Result<Option<LinearClaim>, PersistError> {
        let row = sqlx::query_as::<_, LinearClaim>(
            "SELECT run_id, project, workflow, issue_id, identifier, original_state_id,
                    phase, agent_session_id, reported_nodes, last_activity_at, created_at
             FROM harness_linear_claims
             WHERE run_id = $1",
        )
        .bind(run_id)
        .fetch_optional(&self.pool)
        .await?;
        Ok(row)
    }

    /// The claim behind an agent session, if any — the inverse of
    /// [`Self::claim_for_run`], for answering a follow-up that arrives on a session
    /// rather than against a known run.
    ///
    /// Newest first, because a Rerun deliberately records the same session against a
    /// new `run_id` (see [`Self::claim_exists_for_session`]) and a question asked now
    /// is about the attempt running now.
    pub async fn claim_for_session(
        &self,
        agent_session_id: &str,
    ) -> Result<Option<LinearClaim>, PersistError> {
        let row = sqlx::query_as::<_, LinearClaim>(
            "SELECT run_id, project, workflow, issue_id, identifier, original_state_id,
                    phase, agent_session_id, reported_nodes, last_activity_at, created_at
             FROM harness_linear_claims
             WHERE agent_session_id = $1
             ORDER BY created_at DESC
             LIMIT 1",
        )
        .bind(agent_session_id)
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
    /// Record that activities were posted into the session: which nodes have now
    /// been reported, and that "just now" is the last time we spoke.
    ///
    /// Both move together because every activity we post is either a node report
    /// or a heartbeat, and each resets the staleness clock.
    pub async fn set_session_progress(
        &self,
        run_id: &str,
        reported_nodes: &str,
    ) -> Result<(), PersistError> {
        sqlx::query(
            "UPDATE harness_linear_claims
             SET reported_nodes = $2, last_activity_at = now()
             WHERE run_id = $1",
        )
        .bind(run_id)
        .bind(reported_nodes)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

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

    fn claim_with_reported(reported: &str) -> LinearClaim {
        LinearClaim {
            run_id: "r1".into(),
            project: "p".into(),
            workflow: "w".into(),
            issue_id: "i".into(),
            identifier: "ECOM-1".into(),
            original_state_id: "todo".into(),
            phase: "claimed".into(),
            agent_session_id: Some("sess-1".into()),
            reported_nodes: reported.into(),
            last_activity_at: Utc::now(),
            created_at: Utc::now(),
        }
    }

    #[test]
    fn reported_nodes_is_an_exact_set_not_a_high_water_mark() {
        let claim = claim_with_reported("explore,create-plan");
        assert!(claim.has_reported("explore"));
        assert!(claim.has_reported("create-plan"));
        assert!(!claim.has_reported("validate"));
        // Not a prefix or substring match: `plan` must not look reported just
        // because `create-plan` is.
        assert!(!claim.has_reported("plan"));
        assert!(!claim.has_reported(""));
    }

    #[test]
    fn with_reported_appends_without_duplicating() {
        let claim = claim_with_reported("explore");
        assert_eq!(
            claim.with_reported(&["create-plan".into()]),
            "explore,create-plan"
        );
        // Re-reporting a node is a no-op, so a repeated status-sync tick can't
        // grow the column or re-post an activity.
        assert_eq!(claim.with_reported(&["explore".into()]), "explore");
        // Several at once, mixed new and seen.
        assert_eq!(
            claim.with_reported(&["explore".into(), "validate".into()]),
            "explore,validate"
        );
    }

    #[test]
    fn with_reported_handles_an_empty_starting_set() {
        let claim = claim_with_reported("");
        assert_eq!(claim.with_reported(&[]), "");
        assert_eq!(claim.with_reported(&["explore".into()]), "explore");
        // No leading comma from the empty initial value.
        assert!(!claim.with_reported(&["explore".into()]).starts_with(','));
    }

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
        // Retired between attempts, as the poller does: only one claim on an
        // issue may be active at a time, so attempt two exists precisely because
        // attempt one finished.
        for run in [rid("f1"), rid("f2"), rid("ok")] {
            assert!(
                store
                    .record(&run, "proj", "idea-to-pr", &issue, "PROJ-1", "todo", None)
                    .await
                    .unwrap(),
                "a retired claim must not block the next attempt"
            );
            store.set_phase(&run, "done").await.unwrap();
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
            .record("ca-run-1", &project, wf, "iss-1", "P-1", "todo", None)
            .await
            .unwrap();
        store
            .record("ca-run-2", &project, wf, "iss-2", "P-2", "todo", None)
            .await
            .unwrap();
        assert_eq!(store.count_active(&project, wf).await.unwrap(), 2);

        // A `done` claim no longer counts against the cap.
        store.set_phase("ca-run-1", "done").await.unwrap();
        assert_eq!(store.count_active(&project, wf).await.unwrap(), 1);

        // A different workflow on the same project is counted separately.
        assert_eq!(store.count_active(&project, "merge-pr").await.unwrap(), 0);
    }

    /// One active claim per `(issue, workflow)` — the guarantee that stops one
    /// Linear issue producing two runs and two pull requests.
    ///
    /// This is the backstop, not the mechanism: the poller and the delegation
    /// webhook both check `claim_exists_for_issue` under an in-process guard
    /// first. It exists because those two entry points read their claim signal
    /// from Linear independently, and ECOM-44 slipped between them.
    #[tokio::test]
    #[serial_test::serial]
    async fn one_issue_cannot_hold_two_active_claims() {
        let Some(url) = db_url() else {
            eprintln!("skipping: HARNESS_DATABASE_URL not set");
            return;
        };
        let store = LinearClaimStore::connect(&url).await.expect("connect");
        let ts = chrono::Utc::now().timestamp_nanos_opt().unwrap();
        let issue = format!("dup-issue-{ts}");
        let (first, second) = (format!("dup-a-{ts}"), format!("dup-b-{ts}"));

        assert!(!store
            .claim_exists_for_issue(&issue, "idea-to-pr")
            .await
            .unwrap());
        assert!(store
            .record(&first, "proj", "idea-to-pr", &issue, "P-1", "todo", None)
            .await
            .unwrap());
        assert!(store
            .claim_exists_for_issue(&issue, "idea-to-pr")
            .await
            .unwrap());

        // The second entry point, having raced past every check upstream: it
        // gets `false` rather than a second claim.
        assert!(
            !store
                .record(&second, "proj", "idea-to-pr", &issue, "P-1", "todo", None)
                .await
                .unwrap(),
            "a second active claim on one issue must be refused"
        );
        assert!(store.claim_for_run(&second).await.unwrap().is_none());

        // Another workflow is a different stage of the same issue's life, not a
        // duplicate: `merge-pr` claims it after `idea-to-pr` is done with it.
        assert!(store
            .record(
                &format!("dup-merge-{ts}"),
                "proj",
                "merge-pr",
                &issue,
                "P-1",
                "ready",
                None
            )
            .await
            .unwrap());

        // And once the first claim retires, the issue can be claimed again.
        store.set_phase(&first, "done").await.unwrap();
        assert!(!store
            .claim_exists_for_issue(&issue, "idea-to-pr")
            .await
            .unwrap());
        assert!(store
            .record(
                &format!("dup-c-{ts}"),
                "proj",
                "idea-to-pr",
                &issue,
                "P-1",
                "todo",
                None
            )
            .await
            .unwrap());
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
            .record(&rr1, "proj", "wf", &issue, "P-1", "todo", None)
            .await
            .unwrap();
        // The first attempt is over before the second is claimed — two *active*
        // claims on one issue are what the unique index forbids.
        store.set_phase(&rr1, "done").await.unwrap();
        store
            .record(&rr2, "proj", "wf", &issue, "P-1", "todo", None)
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
