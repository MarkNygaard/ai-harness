//! Triggers: what starts a run by itself, in the model the Triggers page uses.
//!
//! A trigger is a *source* (a Linear status, a webhook) plus a *target* (a
//! project and a workflow). Unlike a classic Linear binding it carries no
//! status map: what happens to the issue once the run starts is the workflow's
//! business, through its `linear:` steps. Kind-specific settings live in
//! `config` so a new kind is a new shape of JSON, not a new table.
//!
//! Two side tables keep the firing rules honest:
//!
//! - `harness_trigger_entries` — "once per entry". A Linear trigger fires when an
//!   issue comes to match it and not again until the issue has stopped matching
//!   (left the status, or was un-delegated) and come back. One row per issue
//!   that currently matches and has fired; the poller drops the rows of issues
//!   it no longer sees. The row also names the run, which is how the status
//!   sync tells a trigger's run from a classic binding's.
//! - `harness_trigger_deliveries` — a webhook delivery id seen once is not acted
//!   on twice, so a sender's retry doesn't start a second run.

use chrono::{DateTime, Utc};
use serde::Serialize;
use serde_json::Value;
use sqlx::types::Json;
use sqlx::PgPool;

use crate::PersistError;

pub const KIND_LINEAR: &str = "linear";
pub const KIND_WEBHOOK: &str = "webhook";

const CREATE_TRIGGERS: &str = "
CREATE TABLE IF NOT EXISTS harness_triggers (
    id            text PRIMARY KEY,
    name          text NOT NULL,
    kind          text NOT NULL,
    project       text NOT NULL,
    workflow      text NOT NULL,
    enabled       boolean NOT NULL DEFAULT false,
    config        jsonb NOT NULL DEFAULT '{}'::jsonb,
    last_fired_at timestamptz,
    last_run_id   text,
    last_error    text,
    created_by    text,
    created_at    timestamptz NOT NULL DEFAULT now(),
    updated_at    timestamptz NOT NULL DEFAULT now()
)";

/// Who last saved a trigger, added after the table shipped.
const ADD_UPDATED_BY: &str =
    "ALTER TABLE harness_triggers ADD COLUMN IF NOT EXISTS updated_by text";

const CREATE_ENTRIES: &str = "
CREATE TABLE IF NOT EXISTS harness_trigger_entries (
    trigger_id text NOT NULL,
    issue_id   text NOT NULL,
    run_id     text,
    entered_at timestamptz NOT NULL DEFAULT now(),
    PRIMARY KEY (trigger_id, issue_id)
)";

const INDEX_ENTRIES_RUN: &str = "
CREATE INDEX IF NOT EXISTS harness_trigger_entries_run
    ON harness_trigger_entries (run_id)";

const CREATE_DELIVERIES: &str = "
CREATE TABLE IF NOT EXISTS harness_trigger_deliveries (
    trigger_id  text NOT NULL,
    delivery_id text NOT NULL,
    received_at timestamptz NOT NULL DEFAULT now(),
    PRIMARY KEY (trigger_id, delivery_id)
)";

/// How long a delivery id is remembered. Senders retry within minutes or
/// hours; a week is far past any of them and keeps the table small.
const DELIVERY_RETENTION_DAYS: i32 = 7;

const COLUMNS: &str = "id, name, kind, project, workflow, enabled, config, last_fired_at, \
                       last_run_id, last_error, created_by, created_at, updated_at, updated_by";

/// A trigger (matches `harness_triggers`).
#[derive(Debug, Clone, Serialize, sqlx::FromRow)]
pub struct RunTrigger {
    pub id: String,
    pub name: String,
    /// [`KIND_LINEAR`] or [`KIND_WEBHOOK`].
    pub kind: String,
    pub project: String,
    pub workflow: String,
    pub enabled: bool,
    /// The kind's own settings. Never holds a secret — a webhook's lives in
    /// the encrypted credential store.
    pub config: Json<Value>,
    pub last_fired_at: Option<DateTime<Utc>>,
    pub last_run_id: Option<String>,
    /// Why the last attempt to fire didn't start a run, until one does.
    pub last_error: Option<String>,
    pub created_by: Option<String>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    /// Who saved it last: the creator until someone edits it. `None` for a
    /// trigger last saved before this was recorded, or by no person.
    pub updated_by: Option<String>,
}

/// The fields a person sets.
#[derive(Debug, Clone)]
pub struct TriggerInput {
    pub name: String,
    pub kind: String,
    pub project: String,
    pub workflow: String,
    pub enabled: bool,
    pub config: Value,
}

pub struct TriggerStore {
    pool: PgPool,
}

impl TriggerStore {
    pub async fn connect(database_url: &str) -> Result<Self, PersistError> {
        let pool = sqlx::postgres::PgPoolOptions::new()
            .max_connections(3)
            .connect(database_url)
            .await?;
        Self::from_pool(pool).await
    }

    pub async fn from_pool(pool: PgPool) -> Result<Self, PersistError> {
        sqlx::query(CREATE_TRIGGERS).execute(&pool).await?;
        sqlx::query(ADD_UPDATED_BY).execute(&pool).await?;
        sqlx::query(CREATE_ENTRIES).execute(&pool).await?;
        sqlx::query(INDEX_ENTRIES_RUN).execute(&pool).await?;
        sqlx::query(CREATE_DELIVERIES).execute(&pool).await?;
        Ok(Self { pool })
    }

    /// Every trigger, by workflow then project then name — what it starts is
    /// what people scan for.
    pub async fn list(&self) -> Result<Vec<RunTrigger>, PersistError> {
        let rows = sqlx::query_as::<_, RunTrigger>(&format!(
            "SELECT {COLUMNS} FROM harness_triggers ORDER BY workflow, project, name, id"
        ))
        .fetch_all(&self.pool)
        .await?;
        Ok(rows)
    }

    /// The enabled triggers of one kind.
    pub async fn list_enabled(&self, kind: &str) -> Result<Vec<RunTrigger>, PersistError> {
        let rows = sqlx::query_as::<_, RunTrigger>(&format!(
            "SELECT {COLUMNS} FROM harness_triggers WHERE enabled AND kind = $1
             ORDER BY project, workflow, id"
        ))
        .bind(kind)
        .fetch_all(&self.pool)
        .await?;
        Ok(rows)
    }

    pub async fn get(&self, id: &str) -> Result<Option<RunTrigger>, PersistError> {
        let row = sqlx::query_as::<_, RunTrigger>(&format!(
            "SELECT {COLUMNS} FROM harness_triggers WHERE id = $1"
        ))
        .bind(id)
        .fetch_optional(&self.pool)
        .await?;
        Ok(row)
    }

    pub async fn create(
        &self,
        id: &str,
        input: &TriggerInput,
        created_by: Option<&str>,
    ) -> Result<RunTrigger, PersistError> {
        let row = sqlx::query_as::<_, RunTrigger>(&format!(
            "INSERT INTO harness_triggers
                (id, name, kind, project, workflow, enabled, config, created_by, updated_by)
             VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $8)
             RETURNING {COLUMNS}"
        ))
        .bind(id)
        .bind(&input.name)
        .bind(&input.kind)
        .bind(&input.project)
        .bind(&input.workflow)
        .bind(input.enabled)
        .bind(Json(&input.config))
        .bind(created_by)
        .fetch_one(&self.pool)
        .await?;
        Ok(row)
    }

    /// Replace a trigger's settings; its kind never changes. `updated_by` is
    /// who is saving it. `None` when it doesn't exist.
    pub async fn update(
        &self,
        id: &str,
        input: &TriggerInput,
        updated_by: Option<&str>,
    ) -> Result<Option<RunTrigger>, PersistError> {
        let row = sqlx::query_as::<_, RunTrigger>(&format!(
            "UPDATE harness_triggers SET
                name = $2, project = $3, workflow = $4, enabled = $5, config = $6,
                updated_at = now(), updated_by = $7
             WHERE id = $1
             RETURNING {COLUMNS}"
        ))
        .bind(id)
        .bind(&input.name)
        .bind(&input.project)
        .bind(&input.workflow)
        .bind(input.enabled)
        .bind(Json(&input.config))
        .bind(updated_by)
        .fetch_optional(&self.pool)
        .await?;
        Ok(row)
    }

    /// Delete a trigger and its bookkeeping. `false` when it didn't exist.
    pub async fn delete(&self, id: &str) -> Result<bool, PersistError> {
        let mut tx = self.pool.begin().await?;
        sqlx::query("DELETE FROM harness_trigger_entries WHERE trigger_id = $1")
            .bind(id)
            .execute(&mut *tx)
            .await?;
        sqlx::query("DELETE FROM harness_trigger_deliveries WHERE trigger_id = $1")
            .bind(id)
            .execute(&mut *tx)
            .await?;
        let res = sqlx::query("DELETE FROM harness_triggers WHERE id = $1")
            .bind(id)
            .execute(&mut *tx)
            .await?;
        tx.commit().await?;
        Ok(res.rows_affected() > 0)
    }

    /// A run started: remember it, and clear the last error.
    pub async fn record_fired(&self, id: &str, run_id: &str) -> Result<(), PersistError> {
        sqlx::query(
            "UPDATE harness_triggers
             SET last_fired_at = now(), last_run_id = $2, last_error = NULL
             WHERE id = $1",
        )
        .bind(id)
        .bind(run_id)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    /// Firing was attempted and failed; shown on the trigger until one works.
    pub async fn record_error(&self, id: &str, error: &str) -> Result<(), PersistError> {
        sqlx::query("UPDATE harness_triggers SET last_error = $2 WHERE id = $1")
            .bind(id)
            .bind(error)
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    // ── Once per entry ──────────────────────────────────────────────────────

    /// The issues this trigger has fired for and that still match it.
    pub async fn entries(&self, trigger_id: &str) -> Result<Vec<String>, PersistError> {
        let rows: Vec<(String,)> =
            sqlx::query_as("SELECT issue_id FROM harness_trigger_entries WHERE trigger_id = $1")
                .bind(trigger_id)
                .fetch_all(&self.pool)
                .await?;
        Ok(rows.into_iter().map(|r| r.0).collect())
    }

    /// Record that `issue_id` fired this trigger. `false` when it already had —
    /// the caller lost a race and should not start a second run.
    pub async fn enter(
        &self,
        trigger_id: &str,
        issue_id: &str,
        run_id: Option<&str>,
    ) -> Result<bool, PersistError> {
        let res = sqlx::query(
            "INSERT INTO harness_trigger_entries (trigger_id, issue_id, run_id)
             VALUES ($1, $2, $3) ON CONFLICT DO NOTHING",
        )
        .bind(trigger_id)
        .bind(issue_id)
        .bind(run_id)
        .execute(&self.pool)
        .await?;
        Ok(res.rows_affected() > 0)
    }

    /// Attach the run to an entry recorded before the run had an id.
    pub async fn set_entry_run(
        &self,
        trigger_id: &str,
        issue_id: &str,
        run_id: &str,
    ) -> Result<(), PersistError> {
        sqlx::query(
            "UPDATE harness_trigger_entries SET run_id = $3
             WHERE trigger_id = $1 AND issue_id = $2",
        )
        .bind(trigger_id)
        .bind(issue_id)
        .bind(run_id)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    /// Forget an entry, e.g. when the run it was for could not start.
    pub async fn leave(&self, trigger_id: &str, issue_id: &str) -> Result<(), PersistError> {
        sqlx::query("DELETE FROM harness_trigger_entries WHERE trigger_id = $1 AND issue_id = $2")
            .bind(trigger_id)
            .bind(issue_id)
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    /// Drop the entries of issues that no longer match — they left, so coming
    /// back is a new entry. `still_matching` is every issue that matches now.
    pub async fn prune_entries(
        &self,
        trigger_id: &str,
        still_matching: &[String],
    ) -> Result<u64, PersistError> {
        let res = sqlx::query(
            "DELETE FROM harness_trigger_entries
             WHERE trigger_id = $1 AND NOT (issue_id = ANY($2))",
        )
        .bind(trigger_id)
        .bind(still_matching)
        .execute(&self.pool)
        .await?;
        Ok(res.rows_affected())
    }

    /// The trigger that started `run_id`, if a Linear trigger did.
    pub async fn trigger_for_run(&self, run_id: &str) -> Result<Option<String>, PersistError> {
        let row: Option<(String,)> = sqlx::query_as(
            "SELECT trigger_id FROM harness_trigger_entries WHERE run_id = $1 LIMIT 1",
        )
        .bind(run_id)
        .fetch_optional(&self.pool)
        .await?;
        Ok(row.map(|r| r.0))
    }

    // ── Webhook deliveries ──────────────────────────────────────────────────

    /// Note a delivery id. `false` when it was seen before — a retry, not new
    /// work. Old ids are forgotten as new ones arrive.
    pub async fn first_delivery(
        &self,
        trigger_id: &str,
        delivery_id: &str,
    ) -> Result<bool, PersistError> {
        sqlx::query(&format!(
            "DELETE FROM harness_trigger_deliveries
             WHERE received_at < now() - interval '{DELIVERY_RETENTION_DAYS} days'"
        ))
        .execute(&self.pool)
        .await?;
        let res = sqlx::query(
            "INSERT INTO harness_trigger_deliveries (trigger_id, delivery_id)
             VALUES ($1, $2) ON CONFLICT DO NOTHING",
        )
        .bind(trigger_id)
        .bind(delivery_id)
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

    fn input(kind: &str) -> TriggerInput {
        TriggerInput {
            name: "Todo → task".into(),
            kind: kind.into(),
            project: "ai-harness".into(),
            workflow: "task".into(),
            enabled: true,
            config: serde_json::json!({ "team_id": "t1", "state_id": "s1" }),
        }
    }

    fn unique(prefix: &str) -> String {
        format!(
            "{prefix}-{}",
            chrono::Utc::now().timestamp_nanos_opt().unwrap()
        )
    }

    #[tokio::test]
    #[serial_test::serial]
    async fn an_issue_fires_once_per_entry() {
        let Some(url) = db_url() else {
            eprintln!("skipping: HARNESS_DATABASE_URL not set");
            return;
        };
        let store = TriggerStore::connect(&url).await.expect("connect");
        let id = unique("trg");
        store.create(&id, &input(KIND_LINEAR), None).await.unwrap();

        assert!(store.enter(&id, "issue-1", Some("run-1")).await.unwrap());
        assert!(
            !store.enter(&id, "issue-1", Some("run-2")).await.unwrap(),
            "still in the status: no second run"
        );
        assert_eq!(
            store.trigger_for_run("run-1").await.unwrap().as_deref(),
            Some(id.as_str())
        );

        // It left (no longer matching), so coming back is a new entry.
        store.prune_entries(&id, &[]).await.unwrap();
        assert!(store.enter(&id, "issue-1", Some("run-3")).await.unwrap());

        // An issue that still matches keeps its entry through a prune.
        store
            .prune_entries(&id, &["issue-1".to_string()])
            .await
            .unwrap();
        assert_eq!(store.entries(&id).await.unwrap(), vec!["issue-1"]);

        assert!(store.delete(&id).await.unwrap());
        assert!(store.entries(&id).await.unwrap().is_empty());
    }

    #[tokio::test]
    #[serial_test::serial]
    async fn a_delivery_is_acted_on_once() {
        let Some(url) = db_url() else {
            eprintln!("skipping: HARNESS_DATABASE_URL not set");
            return;
        };
        let store = TriggerStore::connect(&url).await.expect("connect");
        let id = unique("hook");
        store.create(&id, &input(KIND_WEBHOOK), None).await.unwrap();
        assert!(store.first_delivery(&id, "d-1").await.unwrap());
        assert!(!store.first_delivery(&id, "d-1").await.unwrap());
        assert!(store.first_delivery(&id, "d-2").await.unwrap());
        store.delete(&id).await.unwrap();
    }

    #[tokio::test]
    #[serial_test::serial]
    async fn update_keeps_the_kind_and_clears_nothing_else() {
        let Some(url) = db_url() else {
            eprintln!("skipping: HARNESS_DATABASE_URL not set");
            return;
        };
        let store = TriggerStore::connect(&url).await.expect("connect");
        let id = unique("upd");
        let created = store
            .create(&id, &input(KIND_LINEAR), Some("author"))
            .await
            .unwrap();
        // Until someone edits it, the last to save it is the one who made it.
        assert_eq!(created.updated_by.as_deref(), Some("author"));
        store.record_fired(&id, "run-9").await.unwrap();
        let mut changed = input(KIND_WEBHOOK);
        changed.name = "renamed".into();
        let row = store
            .update(&id, &changed, Some("editor"))
            .await
            .unwrap()
            .unwrap();
        assert_eq!(row.kind, KIND_LINEAR, "kind is fixed at creation");
        assert_eq!(row.name, "renamed");
        assert_eq!(row.last_run_id.as_deref(), Some("run-9"));
        assert_eq!(row.created_by.as_deref(), Some("author"));
        assert_eq!(row.updated_by.as_deref(), Some("editor"));
        assert!(row.updated_at >= created.updated_at);
        store.delete(&id).await.unwrap();
    }
}
