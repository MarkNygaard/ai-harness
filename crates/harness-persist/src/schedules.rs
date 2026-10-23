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
    pub created_by: Option<String>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
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
}

const COLUMNS: &str = "id, name, project, workflow, cron, timezone, task, enabled, next_run_at, \
     last_run_at, last_run_id, created_by, created_at, updated_at";

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
                (id, name, project, workflow, cron, timezone, task, enabled, next_run_at, created_by)
             VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10)
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
                task = $7, enabled = $8, next_run_at = $9, updated_at = now()
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

        assert!(store.delete(&id).await.unwrap());
        assert!(store.get(&id).await.unwrap().is_none());
    }
}
