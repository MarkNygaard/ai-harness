//! Scheduled runs: a workflow started on a cron schedule, with no Linear
//! issue behind it — "every Monday 08:00, report on INP from Grafana".
//!
//! The harness computes `next_run_at` (UTC) from the cron expression in the
//! schedule's own timezone; this store only keeps it. Firing is a
//! compare-and-swap on that column ([`ScheduleStore::claim`]), so two
//! replicas, or a restart mid-tick, can't start the same occurrence twice.

use chrono::{DateTime, Utc};
use serde::Serialize;
use sqlx::PgPool;

use crate::PersistError;

const CREATE_SCHEDULES: &str = "
CREATE TABLE IF NOT EXISTS harness_schedules (
    id           text PRIMARY KEY,
    name         text NOT NULL,
    project      text NOT NULL,
    workflow     text NOT NULL,
    cron         text NOT NULL,
    timezone     text NOT NULL,
    task         text NOT NULL DEFAULT '',
    enabled      boolean NOT NULL DEFAULT true,
    next_run_at  timestamptz,
    last_run_at  timestamptz,
    last_run_id  text,
    created_by   text,
    created_at   timestamptz NOT NULL DEFAULT now(),
    updated_at   timestamptz NOT NULL DEFAULT now()
)";

const INDEX_SCHEDULES_DUE: &str = "
CREATE INDEX IF NOT EXISTS harness_schedules_due
    ON harness_schedules (next_run_at) WHERE enabled";

/// Filing to Linear came after the table did, so its columns are added rather
/// than declared above.
const ALTER_SCHEDULES_LINEAR: &str = "
ALTER TABLE harness_schedules
    ADD COLUMN IF NOT EXISTS linear_team_id        text,
    ADD COLUMN IF NOT EXISTS last_issue_identifier text,
    ADD COLUMN IF NOT EXISTS last_issue_url        text";

/// One row per run a Linear-filing schedule started: whether its result has
/// been filed yet. `state` is `pending` until the run ends, `filing` while one
/// replica holds it, then `filed`, `skipped` (the run didn't succeed) or
/// `error` (Linear refused it too often).
const CREATE_SCHEDULE_RUNS: &str = "
CREATE TABLE IF NOT EXISTS harness_schedule_runs (
    run_id           text PRIMARY KEY,
    schedule_id      text NOT NULL,
    team_id          text NOT NULL,
    state            text NOT NULL DEFAULT 'pending',
    attempts         integer NOT NULL DEFAULT 0,
    issue_identifier text,
    issue_url        text,
    created_at       timestamptz NOT NULL DEFAULT now(),
    updated_at       timestamptz NOT NULL DEFAULT now()
)";

const INDEX_SCHEDULE_RUNS_OPEN: &str = "
CREATE INDEX IF NOT EXISTS harness_schedule_runs_open
    ON harness_schedule_runs (created_at) WHERE state IN ('pending', 'filing')";

/// How long a `filing` hold lasts. Filing is one Linear call; a hold older
/// than this belongs to a replica that died mid-call, and is taken over.
const FILING_HOLD: &str = "15 minutes";

/// A schedule (matches `harness_schedules`).
#[derive(Debug, Clone, Serialize, sqlx::FromRow)]
pub struct Schedule {
    pub id: String,
    pub name: String,
    pub project: String,
    pub workflow: String,
    /// Five-field cron, read in `timezone`.
    pub cron: String,
    /// IANA zone, e.g. `Europe/Copenhagen`.
    pub timezone: String,
    /// What the run is asked to do: its `$ARGUMENTS`.
    pub task: String,
    pub enabled: bool,
    /// The next occurrence, in UTC. `None` when disabled.
    pub next_run_at: Option<DateTime<Utc>>,
    pub last_run_at: Option<DateTime<Utc>>,
    pub last_run_id: Option<String>,
    /// File each successful run's result as an issue in this Linear team.
    pub linear_team_id: Option<String>,
    /// The issue the latest filed run became.
    pub last_issue_identifier: Option<String>,
    pub last_issue_url: Option<String>,
    pub created_by: Option<String>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

/// A scheduled run whose result is still to be filed in Linear.
#[derive(Debug, Clone, sqlx::FromRow)]
pub struct ScheduleFiling {
    pub run_id: String,
    pub schedule_id: String,
    pub team_id: String,
    pub attempts: i32,
    pub created_at: DateTime<Utc>,
}

/// The fields a person sets.
#[derive(Debug, Clone)]
pub struct ScheduleInput {
    pub name: String,
    pub project: String,
    pub workflow: String,
    pub cron: String,
    pub timezone: String,
    pub task: String,
    pub enabled: bool,
    pub linear_team_id: Option<String>,
}

const COLUMNS: &str = "id, name, project, workflow, cron, timezone, task, enabled, next_run_at, \
     last_run_at, last_run_id, linear_team_id, last_issue_identifier, last_issue_url, created_by, \
     created_at, updated_at";

/// Postgres-backed store for schedules.
pub struct ScheduleStore {
    pool: PgPool,
}

impl ScheduleStore {
    pub async fn connect(database_url: &str) -> Result<Self, PersistError> {
        let pool = sqlx::postgres::PgPoolOptions::new()
            .max_connections(3)
            .connect(database_url)
            .await?;
        Self::from_pool(pool).await
    }

    pub async fn from_pool(pool: PgPool) -> Result<Self, PersistError> {
        sqlx::query(CREATE_SCHEDULES).execute(&pool).await?;
        sqlx::query(INDEX_SCHEDULES_DUE).execute(&pool).await?;
        sqlx::query(ALTER_SCHEDULES_LINEAR).execute(&pool).await?;
        sqlx::query(CREATE_SCHEDULE_RUNS).execute(&pool).await?;
        sqlx::query(INDEX_SCHEDULE_RUNS_OPEN).execute(&pool).await?;
        Ok(Self { pool })
    }

    /// Every schedule, by name.
    pub async fn list(&self) -> Result<Vec<Schedule>, PersistError> {
        let rows = sqlx::query_as::<_, Schedule>(&format!(
            "SELECT {COLUMNS} FROM harness_schedules ORDER BY lower(name), id"
        ))
        .fetch_all(&self.pool)
        .await?;
        Ok(rows)
    }

    pub async fn get(&self, id: &str) -> Result<Option<Schedule>, PersistError> {
        let row = sqlx::query_as::<_, Schedule>(&format!(
            "SELECT {COLUMNS} FROM harness_schedules WHERE id = $1"
        ))
        .bind(id)
        .fetch_optional(&self.pool)
        .await?;
        Ok(row)
    }

    pub async fn create(
        &self,
        id: &str,
        input: &ScheduleInput,
        next_run_at: Option<DateTime<Utc>>,
        created_by: Option<&str>,
    ) -> Result<Schedule, PersistError> {
        let row = sqlx::query_as::<_, Schedule>(&format!(
            "INSERT INTO harness_schedules
                (id, name, project, workflow, cron, timezone, task, enabled, next_run_at, created_by,
                 linear_team_id)
             VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11)
             RETURNING {COLUMNS}"
        ))
        .bind(id)
        .bind(&input.name)
        .bind(&input.project)
        .bind(&input.workflow)
        .bind(&input.cron)
        .bind(&input.timezone)
        .bind(&input.task)
        .bind(input.enabled)
        .bind(next_run_at)
        .bind(created_by)
        .bind(&input.linear_team_id)
        .fetch_one(&self.pool)
        .await?;
        Ok(row)
    }

    /// Replace a schedule's settings and its next occurrence. `None` when it
    /// doesn't exist.
    pub async fn update(
        &self,
        id: &str,
        input: &ScheduleInput,
        next_run_at: Option<DateTime<Utc>>,
    ) -> Result<Option<Schedule>, PersistError> {
        let row = sqlx::query_as::<_, Schedule>(&format!(
            "UPDATE harness_schedules SET
                name = $2, project = $3, workflow = $4, cron = $5, timezone = $6,
                task = $7, enabled = $8, next_run_at = $9, linear_team_id = $10,
                updated_at = now()
             WHERE id = $1
             RETURNING {COLUMNS}"
        ))
        .bind(id)
        .bind(&input.name)
        .bind(&input.project)
        .bind(&input.workflow)
        .bind(&input.cron)
        .bind(&input.timezone)
        .bind(&input.task)
        .bind(input.enabled)
        .bind(next_run_at)
        .bind(&input.linear_team_id)
        .fetch_optional(&self.pool)
        .await?;
        Ok(row)
    }

    pub async fn delete(&self, id: &str) -> Result<bool, PersistError> {
        let res = sqlx::query("DELETE FROM harness_schedules WHERE id = $1")
            .bind(id)
            .execute(&self.pool)
            .await?;
        Ok(res.rows_affected() > 0)
    }

    /// Enabled schedules whose next occurrence is at or before `now`.
    pub async fn due(&self, now: DateTime<Utc>) -> Result<Vec<Schedule>, PersistError> {
        let rows = sqlx::query_as::<_, Schedule>(&format!(
            "SELECT {COLUMNS} FROM harness_schedules
             WHERE enabled AND next_run_at IS NOT NULL AND next_run_at <= $1
             ORDER BY next_run_at"
        ))
        .bind(now)
        .fetch_all(&self.pool)
        .await?;
        Ok(rows)
    }

    /// Take the occurrence at `expected` by moving `next_run_at` on to `next`.
    /// `true` only for the one caller whose `expected` was still current — a
    /// second replica, or this one after an edit, gets `false` and fires
    /// nothing.
    pub async fn claim(
        &self,
        id: &str,
        expected: DateTime<Utc>,
        next: Option<DateTime<Utc>>,
    ) -> Result<bool, PersistError> {
        let res = sqlx::query(
            "UPDATE harness_schedules SET next_run_at = $3, last_run_at = now()
             WHERE id = $1 AND enabled AND next_run_at = $2",
        )
        .bind(id)
        .bind(expected)
        .bind(next)
        .execute(&self.pool)
        .await?;
        Ok(res.rows_affected() == 1)
    }

    /// Record the run a schedule started (by the scheduler or "Run now").
    pub async fn set_last_run(&self, id: &str, run_id: &str) -> Result<(), PersistError> {
        sqlx::query(
            "UPDATE harness_schedules SET last_run_id = $2, last_run_at = now() WHERE id = $1",
        )
        .bind(id)
        .bind(run_id)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    /// Note that `run_id`'s result is to be filed in `team_id` once it ends.
    pub async fn track_filing(
        &self,
        run_id: &str,
        schedule_id: &str,
        team_id: &str,
    ) -> Result<(), PersistError> {
        sqlx::query(
            "INSERT INTO harness_schedule_runs (run_id, schedule_id, team_id)
             VALUES ($1, $2, $3) ON CONFLICT (run_id) DO NOTHING",
        )
        .bind(run_id)
        .bind(schedule_id)
        .bind(team_id)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    /// Runs whose result is still to be filed, oldest first, including any
    /// whose hold has lapsed.
    pub async fn open_filings(&self) -> Result<Vec<ScheduleFiling>, PersistError> {
        let rows = sqlx::query_as::<_, ScheduleFiling>(&format!(
            "SELECT run_id, schedule_id, team_id, attempts, created_at
             FROM harness_schedule_runs
             WHERE state = 'pending'
                OR (state = 'filing' AND updated_at < now() - interval '{FILING_HOLD}')
             ORDER BY created_at"
        ))
        .fetch_all(&self.pool)
        .await?;
        Ok(rows)
    }

    /// Hold `run_id` for filing. `true` only for the one caller that gets it,
    /// so two replicas can't file the same result twice.
    pub async fn hold_filing(&self, run_id: &str) -> Result<bool, PersistError> {
        let res = sqlx::query(&format!(
            "UPDATE harness_schedule_runs SET state = 'filing', updated_at = now()
             WHERE run_id = $1
               AND (state = 'pending'
                    OR (state = 'filing' AND updated_at < now() - interval '{FILING_HOLD}'))"
        ))
        .bind(run_id)
        .execute(&self.pool)
        .await?;
        Ok(res.rows_affected() == 1)
    }

    /// Give a held run back: to `pending` to try again, or to `error` once
    /// `give_up`.
    pub async fn release_filing(&self, run_id: &str, give_up: bool) -> Result<(), PersistError> {
        sqlx::query(
            "UPDATE harness_schedule_runs
             SET state = CASE WHEN $2 THEN 'error' ELSE 'pending' END,
                 attempts = attempts + 1, updated_at = now()
             WHERE run_id = $1",
        )
        .bind(run_id)
        .bind(give_up)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    /// Close a run with nothing to file: it didn't succeed, or it is gone.
    pub async fn skip_filing(&self, run_id: &str) -> Result<(), PersistError> {
        sqlx::query(
            "UPDATE harness_schedule_runs SET state = 'skipped', updated_at = now()
             WHERE run_id = $1",
        )
        .bind(run_id)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    /// Record the issue `run_id` was filed as, on the run and on its schedule.
    pub async fn filed(
        &self,
        run_id: &str,
        identifier: &str,
        url: &str,
    ) -> Result<(), PersistError> {
        let mut tx = self.pool.begin().await?;
        let schedule_id: Option<String> = sqlx::query_scalar(
            "UPDATE harness_schedule_runs
             SET state = 'filed', issue_identifier = $2, issue_url = $3, updated_at = now()
             WHERE run_id = $1
             RETURNING schedule_id",
        )
        .bind(run_id)
        .bind(identifier)
        .bind(url)
        .fetch_optional(&mut *tx)
        .await?;
        if let Some(schedule_id) = schedule_id {
            sqlx::query(
                "UPDATE harness_schedules SET last_issue_identifier = $2, last_issue_url = $3
                 WHERE id = $1",
            )
            .bind(schedule_id)
            .bind(identifier)
            .bind(url)
            .execute(&mut *tx)
            .await?;
        }
        tx.commit().await?;
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

    fn input(name: &str) -> ScheduleInput {
        ScheduleInput {
            name: name.into(),
            project: "dilling-ecom".into(),
            workflow: "grafana-inp-report".into(),
            cron: "0 8 * * 1".into(),
            timezone: "Europe/Copenhagen".into(),
            task: "Weekly INP report".into(),
            enabled: true,
            linear_team_id: None,
        }
    }

    /// One occurrence is fired once, however many callers race for it.
    #[tokio::test]
    #[serial_test::serial]
    async fn an_occurrence_is_claimed_exactly_once() {
        let Some(url) = db_url() else {
            eprintln!("skipping: HARNESS_DATABASE_URL not set");
            return;
        };
        let store = ScheduleStore::connect(&url).await.expect("connect");
        let ts = chrono::Utc::now().timestamp_nanos_opt().unwrap();
        let id = format!("sched-{ts}");
        let due_at = Utc::now() - chrono::Duration::minutes(1);
        let next = due_at + chrono::Duration::days(7);
        store
            .create(&id, &input("INP"), Some(due_at), Some("u1"))
            .await
            .unwrap();

        assert!(store
            .due(Utc::now())
            .await
            .unwrap()
            .iter()
            .any(|s| s.id == id));
        assert!(store.claim(&id, due_at, Some(next)).await.unwrap());
        // The same occurrence, claimed again (a second replica): refused.
        assert!(!store.claim(&id, due_at, Some(next)).await.unwrap());
        assert!(!store
            .due(Utc::now())
            .await
            .unwrap()
            .iter()
            .any(|s| s.id == id));

        store.set_last_run(&id, "run-1").await.unwrap();
        let got = store.get(&id).await.unwrap().unwrap();
        assert_eq!(got.last_run_id.as_deref(), Some("run-1"));
        assert_eq!(
            got.next_run_at.map(|t| t.timestamp()),
            Some(next.timestamp())
        );

        // Disabled schedules are never due and can't be claimed.
        let mut off = input("INP");
        off.enabled = false;
        store.update(&id, &off, None).await.unwrap().unwrap();
        assert!(!store.claim(&id, next, None).await.unwrap());

        // Filing: tracked once, held by one caller, retried after a release,
        // and the issue lands on the schedule.
        let run = format!("run-{ts}");
        let open = |rows: Vec<ScheduleFiling>| rows.iter().any(|f| f.run_id == run);
        store.track_filing(&run, &id, "team-1").await.unwrap();
        store.track_filing(&run, &id, "team-1").await.unwrap();
        assert!(open(store.open_filings().await.unwrap()));
        assert!(store.hold_filing(&run).await.unwrap());
        assert!(!store.hold_filing(&run).await.unwrap(), "held twice");
        assert!(!open(store.open_filings().await.unwrap()));
        store.release_filing(&run, false).await.unwrap();
        assert!(
            store.hold_filing(&run).await.unwrap(),
            "released for a retry"
        );
        store
            .filed(&run, "ECOM-1", "https://linear.app/x/issue/ECOM-1")
            .await
            .unwrap();
        assert!(!store.hold_filing(&run).await.unwrap(), "filed is final");
        let got = store.get(&id).await.unwrap().unwrap();
        assert_eq!(got.last_issue_identifier.as_deref(), Some("ECOM-1"));

        assert!(store.delete(&id).await.unwrap());
        assert!(store.get(&id).await.unwrap().is_none());
    }
}
