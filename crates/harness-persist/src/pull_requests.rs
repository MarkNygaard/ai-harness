//! Pull requests the harness's runs opened, and what became of them.
//!
//! A run that ends with a PR link in its output is recorded here
//! (`harness_run_prs`, one row per PR URL). A background poller then asks
//! GitHub when each open PR was opened and whether it has merged or closed,
//! until it has. The dashboard reads the result: PRs opened and merged per
//! day, how long a task took from kickoff to its PR, and how long a PR waited
//! to merge.
//!
//! The first run to mention a PR keeps it. That is the run that opened it in
//! the usual case (`task` opens it, `task-merge` mentions it again later). A
//! PR some person opened, which a run merely worked on, is kept too, but its
//! `opened_at` comes before its run started, and the dashboard leaves it out
//! of kickoff-to-PR rather than reporting a negative time.

use chrono::{DateTime, Utc};
use serde::Serialize;
use sqlx::PgPool;

use crate::PersistError;

pub const PR_OPEN: &str = "open";
pub const PR_MERGED: &str = "merged";
pub const PR_CLOSED: &str = "closed";

const CREATE: &str = "
CREATE TABLE IF NOT EXISTS harness_run_prs (
    url            text PRIMARY KEY,
    run_id         text NOT NULL,
    project        text,
    workflow       text NOT NULL,
    run_started_at timestamptz NOT NULL,
    state          text NOT NULL DEFAULT 'open',
    opened_at      timestamptz,
    merged_at      timestamptz,
    checked_at     timestamptz,
    created_at     timestamptz NOT NULL DEFAULT now()
)";

// Added after the table first shipped, so existing installs gain it too.
const ADD_FIRST_REVIEW: &str =
    "ALTER TABLE harness_run_prs ADD COLUMN IF NOT EXISTS first_review_at timestamptz";

const INDEX_OPEN: &str = "
CREATE INDEX IF NOT EXISTS harness_run_prs_open
    ON harness_run_prs (checked_at NULLS FIRST) WHERE state = 'open'";

const COLUMNS: &str = "url, run_id, project, workflow, run_started_at, state, opened_at, \
     merged_at, first_review_at, checked_at";

/// One pull request a run produced.
#[derive(Debug, Clone, Serialize, sqlx::FromRow)]
pub struct RunPr {
    pub url: String,
    pub run_id: String,
    pub project: Option<String>,
    pub workflow: String,
    /// When the run that recorded it started: the task's kickoff.
    pub run_started_at: DateTime<Utc>,
    /// [`PR_OPEN`], [`PR_MERGED`] or [`PR_CLOSED`].
    pub state: String,
    /// GitHub's `created_at`; `None` until the poller has read it.
    pub opened_at: Option<DateTime<Utc>>,
    pub merged_at: Option<DateTime<Utc>>,
    /// When someone other than its author first reviewed it.
    pub first_review_at: Option<DateTime<Utc>>,
    /// When the poller last asked GitHub; `None` before the first time.
    pub checked_at: Option<DateTime<Utc>>,
}

pub struct PrStore {
    pool: PgPool,
}

impl PrStore {
    pub async fn connect(database_url: &str) -> Result<Self, PersistError> {
        let pool = sqlx::postgres::PgPoolOptions::new()
            .max_connections(2)
            .connect(database_url)
            .await?;
        Self::from_pool(pool).await
    }

    pub async fn from_pool(pool: PgPool) -> Result<Self, PersistError> {
        sqlx::query(CREATE).execute(&pool).await?;
        sqlx::query(ADD_FIRST_REVIEW).execute(&pool).await?;
        sqlx::query(INDEX_OPEN).execute(&pool).await?;
        Ok(Self { pool })
    }

    /// Record a PR a run produced. A PR already recorded keeps its first run;
    /// `true` when this call added it.
    pub async fn record(
        &self,
        url: &str,
        run_id: &str,
        project: Option<&str>,
        workflow: &str,
        run_started_at: DateTime<Utc>,
    ) -> Result<bool, PersistError> {
        let done = sqlx::query(
            "INSERT INTO harness_run_prs (url, run_id, project, workflow, run_started_at)
             VALUES ($1, $2, $3, $4, $5)
             ON CONFLICT (url) DO NOTHING",
        )
        .bind(url)
        .bind(run_id)
        .bind(project)
        .bind(workflow)
        .bind(run_started_at)
        .execute(&self.pool)
        .await?;
        Ok(done.rows_affected() == 1)
    }

    /// Open PRs to ask GitHub about, least recently checked first.
    pub async fn open_prs(&self, limit: i64) -> Result<Vec<RunPr>, PersistError> {
        Ok(sqlx::query_as::<_, RunPr>(&format!(
            "SELECT {COLUMNS} FROM harness_run_prs
             WHERE state = 'open'
             ORDER BY checked_at NULLS FIRST
             LIMIT $1"
        ))
        .bind(limit)
        .fetch_all(&self.pool)
        .await?)
    }

    /// What GitHub said about a PR. `state` is [`PR_OPEN`], [`PR_MERGED`] or
    /// [`PR_CLOSED`]; anything else is stored as open, to be asked again. A
    /// time that is `None` leaves what was stored.
    pub async fn update(
        &self,
        url: &str,
        state: &str,
        opened_at: Option<DateTime<Utc>>,
        merged_at: Option<DateTime<Utc>>,
        first_review_at: Option<DateTime<Utc>>,
    ) -> Result<(), PersistError> {
        let state = match state {
            PR_MERGED | PR_CLOSED => state,
            _ => PR_OPEN,
        };
        sqlx::query(
            "UPDATE harness_run_prs
             SET state = $2,
                 opened_at = COALESCE($3, opened_at),
                 merged_at = COALESCE($4, merged_at),
                 first_review_at = COALESCE($5, first_review_at),
                 checked_at = now()
             WHERE url = $1",
        )
        .bind(url)
        .bind(state)
        .bind(opened_at)
        .bind(merged_at)
        .bind(first_review_at)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    /// Note a check that got no answer, so the PR goes to the back of the
    /// queue instead of being asked about first, forever.
    pub async fn touch(&self, url: &str) -> Result<(), PersistError> {
        sqlx::query("UPDATE harness_run_prs SET checked_at = now() WHERE url = $1")
            .bind(url)
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    /// PRs opened, first reviewed or merged since `since` (or recorded since
    /// then, before the poller has read them), optionally one project's.
    pub async fn dashboard_prs(
        &self,
        since: DateTime<Utc>,
        project: Option<&str>,
    ) -> Result<Vec<RunPr>, PersistError> {
        Ok(sqlx::query_as::<_, RunPr>(&format!(
            "SELECT {COLUMNS} FROM harness_run_prs
             WHERE (COALESCE(opened_at, created_at) >= $1
                    OR merged_at >= $1
                    OR first_review_at >= $1)
               AND ($2::text IS NULL OR project = $2)"
        ))
        .bind(since)
        .bind(project)
        .fetch_all(&self.pool)
        .await?)
    }
}
