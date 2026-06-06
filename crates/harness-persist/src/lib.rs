//! # harness-persist
//!
//! Postgres persistence for workflow runs. Takes a [`harness_dag::RunReport`]
//! (the output of the DAG driver) and records it as one `harness_workflow_runs`
//! row plus one `harness_run_nodes` row per node — including per-node status,
//! provider/model, token usage, iterations, and `started_at`/`ended_at`
//! timestamps (for the UI duration badge + task-overview waterfall).
//!
//! Depends only on `harness-dag` + `sqlx` (not the agent crates) so the control
//! plane / server can reuse it. Schema is created on connect via idempotent
//! `CREATE TABLE IF NOT EXISTS`; a richer migration story can replace this later.

use chrono::{DateTime, Utc};
use harness_dag::{NodeMeta, NodeRun, NodeStatus, RunReport, RunStatus};
use serde::Serialize;
use sqlx::postgres::{PgPool, PgPoolOptions};
use sqlx::types::Json;

mod credentials;
pub use credentials::{CredentialStore, ProviderCredential};

mod projects;
pub use projects::{Project, ProjectInput, ProjectStore};

mod categories;
pub use categories::{Category, CategoryInput, CategoryStore};
mod linear_sources;
pub use linear_sources::{LinearSource, LinearSourceInput, LinearSourceStore};

/// A run row (matches `harness_workflow_runs`).
#[derive(Debug, Clone, Serialize, sqlx::FromRow)]
pub struct RunSummary {
    pub id: String,
    pub workflow_name: String,
    /// Human task name (the trigger title); `None` for older/CLI runs.
    pub title: Option<String>,
    /// The task spec submitted with the run. Populated by detail reads; redacted
    /// from list reads to avoid repeatedly shipping long or sensitive prompts.
    pub description: Option<String>,
    pub status: String,
    pub project: Option<String>,
    pub node_count: i32,
    pub recorded_at: DateTime<Utc>,
}

/// A persisted per-node row (matches `harness_run_nodes`).
#[derive(Debug, Clone, Serialize, sqlx::FromRow)]
pub struct PersistedNode {
    pub node_id: String,
    pub ordinal: i32,
    pub status: String,
    pub provider: Option<String>,
    pub model: Option<String>,
    pub output: String,
    pub iterations: i32,
    pub converged: Option<bool>,
    pub note: Option<String>,
    pub input_tokens: Option<i64>,
    pub output_tokens: Option<i64>,
    pub cache_read: Option<i64>,
    pub cache_write: Option<i64>,
    pub started_at: Option<DateTime<Utc>>,
    pub ended_at: Option<DateTime<Utc>>,
}

/// A run plus its node rows, for the run-detail endpoint.
#[derive(Debug, Clone, Serialize)]
pub struct RunDetail {
    #[serde(flatten)]
    pub run: RunSummary,
    pub nodes: Vec<PersistedNode>,
    /// Static DAG topology (edges), so the UI can draw the graph for a
    /// historical run without re-parsing the workflow.
    pub graph: Vec<NodeMeta>,
}

/// Error from a persistence operation.
#[derive(Debug, thiserror::Error)]
pub enum PersistError {
    #[error("database error: {0}")]
    Db(#[from] sqlx::Error),
    #[error("credential encryption error: {0}")]
    Crypto(String),
    #[error("bad secret key: {0}")]
    BadKey(String),
}

const CREATE_RUNS: &str = "
CREATE TABLE IF NOT EXISTS harness_workflow_runs (
    id            text PRIMARY KEY,
    workflow_name text NOT NULL,
    title         text,
    description   text,
    status        text NOT NULL,
    project       text,
    node_count    int  NOT NULL DEFAULT 0,
    graph         jsonb NOT NULL DEFAULT '[]'::jsonb,
    owner         text,
    heartbeat_at  timestamptz,
    recorded_at   timestamptz NOT NULL DEFAULT now()
)";

/// Bring older `harness_workflow_runs` tables up to date. Idempotent.
const ALTER_RUNS_GRAPH: &str =
    "ALTER TABLE harness_workflow_runs ADD COLUMN IF NOT EXISTS graph jsonb NOT NULL DEFAULT '[]'::jsonb";
const ALTER_RUNS_TITLE: &str =
    "ALTER TABLE harness_workflow_runs ADD COLUMN IF NOT EXISTS title text";
const ALTER_RUNS_DESCRIPTION: &str =
    "ALTER TABLE harness_workflow_runs ADD COLUMN IF NOT EXISTS description text";
/// Run-lease columns: `owner` stamps the server instance that started a run and
/// `heartbeat_at` is renewed while it executes, so reconcile can reap only runs
/// whose lease has gone stale — never a live run owned by another instance.
const ALTER_RUNS_OWNER: &str =
    "ALTER TABLE harness_workflow_runs ADD COLUMN IF NOT EXISTS owner text";
const ALTER_RUNS_HEARTBEAT: &str =
    "ALTER TABLE harness_workflow_runs ADD COLUMN IF NOT EXISTS heartbeat_at timestamptz";

const CREATE_NODES: &str = "
CREATE TABLE IF NOT EXISTS harness_run_nodes (
    run_id        text NOT NULL REFERENCES harness_workflow_runs(id) ON DELETE CASCADE,
    ordinal       int  NOT NULL,
    node_id       text NOT NULL,
    status        text NOT NULL,
    provider      text,
    model         text,
    output        text NOT NULL DEFAULT '',
    iterations    int  NOT NULL DEFAULT 0,
    converged     boolean,
    note          text,
    input_tokens  bigint,
    output_tokens bigint,
    cache_read    bigint,
    cache_write   bigint,
    started_at    timestamptz,
    ended_at      timestamptz,
    PRIMARY KEY (run_id, node_id)
)";

/// A Postgres-backed store for workflow runs.
pub struct RunStore {
    pool: PgPool,
}

impl RunStore {
    /// Connect to `database_url` and ensure the schema exists.
    pub async fn connect(database_url: &str) -> Result<Self, PersistError> {
        let pool = PgPoolOptions::new()
            .max_connections(5)
            .connect(database_url)
            .await?;
        let store = Self { pool };
        store.migrate().await?;
        Ok(store)
    }

    /// Wrap an existing pool (schema must already exist or call [`Self::migrate`]).
    pub fn from_pool(pool: PgPool) -> Self {
        Self { pool }
    }

    /// Create the tables if they don't exist.
    pub async fn migrate(&self) -> Result<(), PersistError> {
        sqlx::query(CREATE_RUNS).execute(&self.pool).await?;
        sqlx::query(ALTER_RUNS_GRAPH).execute(&self.pool).await?;
        sqlx::query(ALTER_RUNS_TITLE).execute(&self.pool).await?;
        sqlx::query(ALTER_RUNS_DESCRIPTION)
            .execute(&self.pool)
            .await?;
        sqlx::query(ALTER_RUNS_OWNER).execute(&self.pool).await?;
        sqlx::query(ALTER_RUNS_HEARTBEAT)
            .execute(&self.pool)
            .await?;
        sqlx::query(CREATE_NODES).execute(&self.pool).await?;
        Ok(())
    }
    /// Persist a run and its per-node records. Idempotent on `run_id`: the run
    /// row is upserted and its node rows are replaced.
    pub async fn record_run(
        &self,
        run_id: &str,
        title: Option<&str>,
        description: Option<&str>,
        project: Option<&str>,
        report: &RunReport,
    ) -> Result<(), PersistError> {
        let mut tx = self.pool.begin().await?;

        // COALESCE keeps a title/description set at start time if this final write passes None.
        sqlx::query(
            "INSERT INTO harness_workflow_runs (id, workflow_name, title, description, status, project, node_count, graph, recorded_at)
             VALUES ($1, $2, $3, $4, $5, $6, $7, $8, now())
             ON CONFLICT (id) DO UPDATE SET
                workflow_name = excluded.workflow_name,
                title         = COALESCE(excluded.title, harness_workflow_runs.title),
                description   = COALESCE(excluded.description, harness_workflow_runs.description),
                status        = excluded.status,
                project       = excluded.project,
                node_count    = excluded.node_count,
                graph         = excluded.graph,
                recorded_at   = now()",
        )
        .bind(run_id)
        .bind(&report.workflow)
        .bind(title)
        .bind(description)
        .bind(run_status_str(report.status))
        .bind(project)
        .bind(report.nodes.len() as i32)
        .bind(Json(&report.graph))
        .execute(&mut *tx)
        .await?;

        // Replace node rows so re-recording a run id is clean.
        sqlx::query("DELETE FROM harness_run_nodes WHERE run_id = $1")
            .bind(run_id)
            .execute(&mut *tx)
            .await?;

        for (ordinal, node) in report.nodes.iter().enumerate() {
            sqlx::query(
                "INSERT INTO harness_run_nodes
                   (run_id, ordinal, node_id, status, provider, model, output, iterations,
                    converged, note, input_tokens, output_tokens, cache_read, cache_write,
                    started_at, ended_at)
                 VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,$12,$13,$14,$15,$16)",
            )
            .bind(run_id)
            .bind(ordinal as i32)
            .bind(&node.id)
            .bind(node_status_str(node.status))
            .bind(node.provider.as_deref())
            .bind(node.model.as_deref())
            .bind(&node.output)
            .bind(node.iterations as i32)
            .bind(node.converged)
            .bind(node.note.as_deref())
            .bind(node.usage.input.map(|v| v as i64))
            .bind(node.usage.output.map(|v| v as i64))
            .bind(node.usage.cache_read.map(|v| v as i64))
            .bind(node.usage.cache_write.map(|v| v as i64))
            .bind(node.started_at)
            .bind(node.ended_at)
            .execute(&mut *tx)
            .await?;
        }

        tx.commit().await?;
        Ok(())
    }

    /// Record a run as **running** at submission time, so it shows in the list
    /// and its detail endpoint resolves (instead of 404ing) before it finishes.
    /// Idempotent: a re-submit of the same id refreshes the topology but never
    /// clobbers an already-terminal status.
    #[allow(clippy::too_many_arguments)]
    pub async fn start_run(
        &self,
        run_id: &str,
        workflow: &str,
        title: Option<&str>,
        description: Option<&str>,
        project: Option<&str>,
        total_nodes: usize,
        graph: &[NodeMeta],
        owner: Option<&str>,
    ) -> Result<(), PersistError> {
        // Stamp the lease (`owner` + fresh `heartbeat_at`) so this run is
        // claimed by the current instance and protected from reconcile until its
        // heartbeat goes stale.
        sqlx::query(
            "INSERT INTO harness_workflow_runs (id, workflow_name, title, description, status, project, node_count, graph, owner, heartbeat_at, recorded_at)
             VALUES ($1, $2, $3, $4, 'running', $5, $6, $7, $8, now(), now())
             ON CONFLICT (id) DO UPDATE SET
                workflow_name = excluded.workflow_name,
                title         = COALESCE(excluded.title, harness_workflow_runs.title),
                description   = COALESCE(excluded.description, harness_workflow_runs.description),
                node_count    = excluded.node_count,
                graph         = excluded.graph,
                owner         = excluded.owner,
                heartbeat_at  = now()",
        )
        .bind(run_id)
        .bind(workflow)
        .bind(title)
        .bind(description)
        .bind(project)
        .bind(total_nodes as i32)
        .bind(Json(graph))
        .bind(owner)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    /// Renew a running run's lease (`heartbeat_at = now()`). Called periodically
    /// by the owning executor; a no-op once the run is terminal.
    pub async fn heartbeat_run(&self, run_id: &str) -> Result<(), PersistError> {
        sqlx::query(
            "UPDATE harness_workflow_runs SET heartbeat_at = now()
             WHERE id = $1 AND status = 'running'",
        )
        .bind(run_id)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    /// Mark a node **running** (on `NodeStarted`) so a reloaded or late-subscribed
    /// run view shows in-flight steps as running rather than pending. Never
    /// downgrades a node that already reached a terminal state.
    pub async fn start_node(
        &self,
        run_id: &str,
        ordinal: i32,
        node_id: &str,
        provider: Option<&str>,
        model: Option<&str>,
    ) -> Result<(), PersistError> {
        sqlx::query(
            "INSERT INTO harness_run_nodes (run_id, ordinal, node_id, status, provider, model, started_at)
             VALUES ($1, $2, $3, 'running', $4, $5, now())
             ON CONFLICT (run_id, node_id) DO UPDATE SET
                status = CASE
                    WHEN harness_run_nodes.status IN ('success','failed','skipped','cancelled')
                    THEN harness_run_nodes.status ELSE 'running' END,
                provider = excluded.provider,
                model = excluded.model,
                started_at = COALESCE(harness_run_nodes.started_at, excluded.started_at)",
        )
        .bind(run_id)
        .bind(ordinal)
        .bind(node_id)
        .bind(provider)
        .bind(model)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    /// Upsert a single node row as it reaches a terminal state — so a run's
    /// detail reflects progress live and survives a page refresh.
    pub async fn record_node(
        &self,
        run_id: &str,
        ordinal: i32,
        node: &NodeRun,
    ) -> Result<(), PersistError> {
        sqlx::query(
            "INSERT INTO harness_run_nodes
               (run_id, ordinal, node_id, status, provider, model, output, iterations,
                converged, note, input_tokens, output_tokens, cache_read, cache_write,
                started_at, ended_at)
             VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,$12,$13,$14,$15,$16)
             ON CONFLICT (run_id, node_id) DO UPDATE SET
                ordinal=excluded.ordinal, status=excluded.status, provider=excluded.provider,
                model=excluded.model, output=excluded.output, iterations=excluded.iterations,
                converged=excluded.converged, note=excluded.note,
                input_tokens=excluded.input_tokens, output_tokens=excluded.output_tokens,
                cache_read=excluded.cache_read, cache_write=excluded.cache_write,
                started_at=excluded.started_at, ended_at=excluded.ended_at",
        )
        .bind(run_id)
        .bind(ordinal)
        .bind(&node.id)
        .bind(node_status_str(node.status))
        .bind(node.provider.as_deref())
        .bind(node.model.as_deref())
        .bind(&node.output)
        .bind(node.iterations as i32)
        .bind(node.converged)
        .bind(node.note.as_deref())
        .bind(node.usage.input.map(|v| v as i64))
        .bind(node.usage.output.map(|v| v as i64))
        .bind(node.usage.cache_read.map(|v| v as i64))
        .bind(node.usage.cache_write.map(|v| v as i64))
        .bind(node.started_at)
        .bind(node.ended_at)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    /// Mark a run's terminal status (run finished). Only updates a run that is
    /// still `running` — so a run cancelled out from under the executor (via
    /// [`Self::cancel_run`] or [`Self::reconcile_orphaned_runs`]) stays cancelled
    /// even if the in-flight task later completes.
    pub async fn finish_run(&self, run_id: &str, status: RunStatus) -> Result<(), PersistError> {
        sqlx::query(
            "UPDATE harness_workflow_runs SET status = $2, recorded_at = now()
             WHERE id = $1 AND status = 'running'",
        )
        .bind(run_id)
        .bind(run_status_str(status))
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    /// Cancel a still-running run: flip the run and any in-flight (`running`)
    /// node rows to `cancelled`. No-op (returns `false`) if the run is absent or
    /// already terminal, so a finished run can't be "un-finished".
    pub async fn cancel_run(&self, run_id: &str) -> Result<bool, PersistError> {
        let res = sqlx::query(
            "UPDATE harness_workflow_runs SET status = 'cancelled', recorded_at = now()
             WHERE id = $1 AND status = 'running'",
        )
        .bind(run_id)
        .execute(&self.pool)
        .await?;
        if res.rows_affected() == 0 {
            return Ok(false);
        }
        sqlx::query(
            "UPDATE harness_run_nodes SET status = 'cancelled', ended_at = now()
             WHERE run_id = $1 AND status = 'running'",
        )
        .bind(run_id)
        .execute(&self.pool)
        .await?;
        Ok(true)
    }

    /// Delete a run and its node rows entirely (for clearing old runs from the
    /// list). Returns `false` if no such run existed.
    pub async fn delete_run(&self, run_id: &str) -> Result<bool, PersistError> {
        sqlx::query("DELETE FROM harness_run_nodes WHERE run_id = $1")
            .bind(run_id)
            .execute(&self.pool)
            .await?;
        let res = sqlx::query("DELETE FROM harness_workflow_runs WHERE id = $1")
            .bind(run_id)
            .execute(&self.pool)
            .await?;
        Ok(res.rows_affected() > 0)
    }

    /// Reap runs whose **lease has gone stale**: a run still marked `running`
    /// whose `heartbeat_at` is older than `stale_after` (or never set) has no
    /// live executor renewing it — flip it (and its in-flight node rows) to
    /// `cancelled`. Returns the run count.
    ///
    /// Crucially this is **scoped by liveness**, not "all running": a run a
    /// different instance is actively heartbeating is left alone, so a second
    /// replica's startup — or any stray connection (a test, a manual `psql`)
    /// running this — can no longer cancel live runs. Pass `Duration::ZERO` to
    /// reap every running run regardless of heartbeat (the old behaviour).
    pub async fn reconcile_orphaned_runs(
        &self,
        stale_after: std::time::Duration,
    ) -> Result<u64, PersistError> {
        let secs = stale_after.as_secs_f64();
        let mut tx = self.pool.begin().await?;
        // Node rows first (while their runs are still `running` so the subquery
        // matches), scoped to exactly the stale runs we're about to cancel.
        sqlx::query(
            "UPDATE harness_run_nodes SET status = 'cancelled', ended_at = now()
             WHERE status = 'running' AND run_id IN (
                 SELECT id FROM harness_workflow_runs
                 WHERE status = 'running'
                   AND (heartbeat_at IS NULL OR heartbeat_at < now() - ($1 * interval '1 second')))",
        )
        .bind(secs)
        .execute(&mut *tx)
        .await?;
        let res = sqlx::query(
            "UPDATE harness_workflow_runs SET status = 'cancelled', recorded_at = now()
             WHERE status = 'running'
               AND (heartbeat_at IS NULL OR heartbeat_at < now() - ($1 * interval '1 second'))",
        )
        .bind(secs)
        .execute(&mut *tx)
        .await?;
        tx.commit().await?;
        Ok(res.rows_affected())
    }

    /// Fetch a run's status string, if present (for read-back / tests).
    pub async fn run_status(&self, run_id: &str) -> Result<Option<String>, PersistError> {
        let row: Option<(String,)> =
            sqlx::query_as("SELECT status FROM harness_workflow_runs WHERE id = $1")
                .bind(run_id)
                .fetch_optional(&self.pool)
                .await?;
        Ok(row.map(|r| r.0))
    }

    /// Count persisted node rows for a run.
    pub async fn node_count(&self, run_id: &str) -> Result<i64, PersistError> {
        let row: (i64,) =
            sqlx::query_as("SELECT count(*) FROM harness_run_nodes WHERE run_id = $1")
                .bind(run_id)
                .fetch_one(&self.pool)
                .await?;
        Ok(row.0)
    }

    /// List the most recently recorded runs (newest first).
    pub async fn list_runs(&self, limit: i64) -> Result<Vec<RunSummary>, PersistError> {
        let rows = sqlx::query_as::<_, RunSummary>(
            "SELECT id, workflow_name, title, NULL::text AS description, status, project, node_count, recorded_at
             FROM harness_workflow_runs
             ORDER BY recorded_at DESC
             LIMIT $1",
        )
        .bind(limit)
        .fetch_all(&self.pool)
        .await?;
        Ok(rows)
    }

    /// Fetch a run plus its node rows (ordered by declaration order).
    pub async fn get_run(&self, run_id: &str) -> Result<Option<RunDetail>, PersistError> {
        let run = sqlx::query_as::<_, RunSummary>(
            "SELECT id, workflow_name, title, description, status, project, node_count, recorded_at
             FROM harness_workflow_runs WHERE id = $1",
        )
        .bind(run_id)
        .fetch_optional(&self.pool)
        .await?;
        let Some(run) = run else {
            return Ok(None);
        };
        let graph: (Json<Vec<NodeMeta>>,) =
            sqlx::query_as("SELECT graph FROM harness_workflow_runs WHERE id = $1")
                .bind(run_id)
                .fetch_one(&self.pool)
                .await?;
        let nodes = sqlx::query_as::<_, PersistedNode>(
            "SELECT node_id, ordinal, status, provider, model, output, iterations, converged,
                    note, input_tokens, output_tokens, cache_read, cache_write, started_at, ended_at
             FROM harness_run_nodes WHERE run_id = $1 ORDER BY ordinal",
        )
        .bind(run_id)
        .fetch_all(&self.pool)
        .await?;
        Ok(Some(RunDetail {
            run,
            nodes,
            graph: graph.0 .0,
        }))
    }
}

fn run_status_str(s: RunStatus) -> &'static str {
    match s {
        RunStatus::Completed => "completed",
        RunStatus::Failed => "failed",
        RunStatus::Cancelled => "cancelled",
    }
}

fn node_status_str(s: NodeStatus) -> &'static str {
    match s {
        NodeStatus::Success => "success",
        NodeStatus::Failed => "failed",
        NodeStatus::Skipped => "skipped",
        NodeStatus::Cancelled => "cancelled",
    }
}

/// Guard for Postgres-dependent tests: only treat a URL as a test database when
/// its name clearly says so (CI uses `harness_test`). A production DB name like
/// `harness` returns false, so pointing `HARNESS_DATABASE_URL` at the cluster can
/// never let a test create rows or run a destructive statement against it. Used
/// by every persist crate's test `db_url()` gate.
#[cfg(test)]
pub(crate) fn is_test_db(url: &str) -> bool {
    let db = url.rsplit('/').next().unwrap_or(url);
    let db = db.split(['?', '#']).next().unwrap_or(db);
    db.to_ascii_lowercase().contains("test")
}

#[cfg(test)]
mod tests {
    use super::*;
    use harness_dag::{NodeRun, Usage};

    /// Postgres-dependent tests run only when HARNESS_DATABASE_URL is set to a
    /// **test** database (CI provides `harness_test`; locally `docker compose up
    /// -d postgres` + export it). A non-test URL is ignored so these tests can
    /// never touch a production DB.
    fn db_url() -> Option<String> {
        let url = std::env::var("HARNESS_DATABASE_URL").ok()?;
        is_test_db(&url).then_some(url)
    }

    fn sample_report() -> RunReport {
        RunReport {
            workflow: "demo".into(),
            status: RunStatus::Completed,
            graph: vec![
                harness_dag::NodeMeta {
                    id: "build".into(),
                    depends_on: vec![],
                    category: Some("implementation".into()),
                },
                harness_dag::NodeMeta {
                    id: "review".into(),
                    depends_on: vec!["build".into()],
                    category: None,
                },
            ],
            nodes: vec![
                NodeRun {
                    id: "build".into(),
                    status: NodeStatus::Success,
                    provider: Some("claude".into()),
                    model: Some("sonnet".into()),
                    output: "built".into(),
                    usage: Usage {
                        input: Some(100),
                        output: Some(20),
                        cache_read: None,
                        cache_write: None,
                    },
                    iterations: 1,
                    converged: None,
                    note: None,
                    started_at: Some(chrono::Utc::now()),
                    ended_at: Some(chrono::Utc::now()),
                },
                NodeRun {
                    id: "review".into(),
                    status: NodeStatus::Skipped,
                    provider: None,
                    model: None,
                    output: String::new(),
                    usage: Usage::default(),
                    iterations: 0,
                    converged: None,
                    note: Some("dependency failed".into()),
                    started_at: None,
                    ended_at: None,
                },
            ],
        }
    }

    #[tokio::test]
    #[serial_test::serial]
    async fn records_run_and_reads_back() {
        let Some(url) = db_url() else {
            eprintln!("skipping: HARNESS_DATABASE_URL not set");
            return;
        };
        let store = RunStore::connect(&url).await.expect("connect");
        let run_id = format!(
            "test-run-{}",
            chrono::Utc::now().timestamp_nanos_opt().unwrap()
        );
        let report = sample_report();

        store
            .record_run(
                &run_id,
                Some("My task"),
                Some("Implement X end to end"),
                Some("proj-a"),
                &report,
            )
            .await
            .expect("record");

        assert_eq!(
            store.run_status(&run_id).await.unwrap().as_deref(),
            Some("completed")
        );
        assert_eq!(store.node_count(&run_id).await.unwrap(), 2);

        // Idempotent re-record keeps node count stable.
        store
            .record_run(
                &run_id,
                Some("My task"),
                Some("Implement X end to end"),
                Some("proj-a"),
                &report,
            )
            .await
            .unwrap();
        assert_eq!(store.node_count(&run_id).await.unwrap(), 2);

        // list_runs includes metadata only; get_run returns ordered node detail and description.
        let listed = store.list_runs(50).await.unwrap();
        let listed_run = listed
            .iter()
            .find(|r| r.id == run_id && r.node_count == 2)
            .expect("listed run");
        assert!(
            listed_run.description.is_none(),
            "list_runs must redact long task descriptions"
        );
        let detail = store.get_run(&run_id).await.unwrap().expect("detail");
        assert_eq!(detail.run.status, "completed");
        assert_eq!(detail.nodes.len(), 2);
        assert_eq!(detail.nodes[0].node_id, "build");
        assert_eq!(detail.nodes[0].input_tokens, Some(100));
        assert_eq!(detail.nodes[1].node_id, "review");
        assert_eq!(detail.nodes[1].status, "skipped");
        // Topology round-trips: review depends on build.
        assert_eq!(detail.graph.len(), 2);
        assert_eq!(detail.graph[1].id, "review");
        assert_eq!(detail.graph[1].depends_on, vec!["build".to_string()]);
        assert_eq!(
            detail.run.description.as_deref(),
            Some("Implement X end to end")
        );
        assert!(store.get_run("does-not-exist").await.unwrap().is_none());
    }

    #[tokio::test]
    #[serial_test::serial]
    async fn incremental_start_node_finish_round_trip() {
        let Some(url) = db_url() else {
            eprintln!("skipping: HARNESS_DATABASE_URL not set");
            return;
        };
        let store = RunStore::connect(&url).await.expect("connect");
        let run_id = format!(
            "test-incr-{}",
            chrono::Utc::now().timestamp_nanos_opt().unwrap()
        );
        let report = sample_report();

        // start_run → visible as `running` with the topology, before any node.
        store
            .start_run(
                &run_id,
                &report.workflow,
                Some("Incremental task"),
                Some("Incremental description"),
                None,
                report.nodes.len(),
                &report.graph,
                None,
            )
            .await
            .unwrap();
        let detail = store
            .get_run(&run_id)
            .await
            .unwrap()
            .expect("running detail");
        assert_eq!(detail.run.status, "running");
        assert_eq!(detail.run.title.as_deref(), Some("Incremental task"));
        assert_eq!(
            detail.run.description.as_deref(),
            Some("Incremental description")
        );
        assert_eq!(detail.run.node_count, 2);
        assert_eq!(detail.graph.len(), 2);
        assert!(detail.nodes.is_empty(), "no nodes finished yet");

        // record_node → the node appears immediately.
        store
            .record_node(&run_id, 0, &report.nodes[0])
            .await
            .unwrap();
        let detail = store.get_run(&run_id).await.unwrap().unwrap();
        assert_eq!(detail.nodes.len(), 1);
        assert_eq!(detail.nodes[0].node_id, "build");

        // record_run with None description must NOT clobber the start-time value (COALESCE).
        store
            .record_run(&run_id, None, None, None, &report)
            .await
            .unwrap();
        let detail = store.get_run(&run_id).await.unwrap().unwrap();
        assert_eq!(
            detail.run.description.as_deref(),
            Some("Incremental description"),
            "COALESCE must preserve start-time description"
        );

        // finish_run → terminal status.
        store
            .finish_run(&run_id, RunStatus::Completed)
            .await
            .unwrap();
        assert_eq!(
            store.run_status(&run_id).await.unwrap().as_deref(),
            Some("completed")
        );
    }

    #[tokio::test]
    #[serial_test::serial]
    async fn cancel_run_marks_running_run_and_node() {
        let Some(url) = db_url() else {
            eprintln!("skipping: HARNESS_DATABASE_URL not set");
            return;
        };
        let store = RunStore::connect(&url).await.expect("connect");
        let run_id = format!(
            "test-cancel-{}",
            chrono::Utc::now().timestamp_nanos_opt().unwrap()
        );
        let report = sample_report();
        store
            .start_run(
                &run_id,
                &report.workflow,
                None,
                None,
                None,
                2,
                &report.graph,
                None,
            )
            .await
            .unwrap();
        store
            .start_node(&run_id, 0, "build", Some("claude"), Some("sonnet"))
            .await
            .unwrap();

        assert!(store.cancel_run(&run_id).await.unwrap(), "first cancel");
        let detail = store.get_run(&run_id).await.unwrap().unwrap();
        assert_eq!(detail.run.status, "cancelled");
        assert_eq!(detail.nodes[0].status, "cancelled");

        // Cancelling an already-terminal run is a no-op.
        assert!(!store.cancel_run(&run_id).await.unwrap(), "second cancel");

        // finish_run must NOT resurrect a cancelled run.
        store
            .finish_run(&run_id, RunStatus::Completed)
            .await
            .unwrap();
        assert_eq!(
            store.run_status(&run_id).await.unwrap().as_deref(),
            Some("cancelled")
        );
    }

    #[tokio::test]
    #[serial_test::serial]
    async fn delete_run_removes_run_and_nodes() {
        let Some(url) = db_url() else {
            eprintln!("skipping: HARNESS_DATABASE_URL not set");
            return;
        };
        let store = RunStore::connect(&url).await.expect("connect");
        let run_id = format!(
            "test-delete-{}",
            chrono::Utc::now().timestamp_nanos_opt().unwrap()
        );
        store
            .record_run(&run_id, None, None, None, &sample_report())
            .await
            .unwrap();
        assert!(store.delete_run(&run_id).await.unwrap(), "deleted");
        assert!(store.get_run(&run_id).await.unwrap().is_none());
        assert_eq!(store.node_count(&run_id).await.unwrap(), 0);
        // Deleting again is a no-op.
        assert!(!store.delete_run(&run_id).await.unwrap());
    }

    #[tokio::test]
    #[serial_test::serial]
    async fn reconcile_reaps_only_stale_leases() {
        // `db_url()` already refuses a non-test DB, so this destructive test
        // (`reconcile_orphaned_runs(ZERO)` cancels every running run) can only
        // ever run against an obvious test database.
        let Some(url) = db_url() else {
            eprintln!("skipping: HARNESS_DATABASE_URL not set to a test database");
            return;
        };
        let store = RunStore::connect(&url).await.expect("connect");
        let run_id = format!(
            "test-orphan-{}",
            chrono::Utc::now().timestamp_nanos_opt().unwrap()
        );
        let report = sample_report();
        store
            .start_run(
                &run_id,
                &report.workflow,
                None,
                None,
                None,
                2,
                &report.graph,
                None,
            )
            .await
            .unwrap();

        // A fresh lease survives reconcile with a generous staleness window —
        // this is what protects a live run from a stray reconcile call.
        let n = store
            .reconcile_orphaned_runs(std::time::Duration::from_secs(3600))
            .await
            .unwrap();
        assert_eq!(
            store.run_status(&run_id).await.unwrap().as_deref(),
            Some("running"),
            "fresh-heartbeat run must NOT be reaped (reaped {n})"
        );

        // With a zero window every running run is stale → reaped.
        let n = store
            .reconcile_orphaned_runs(std::time::Duration::ZERO)
            .await
            .unwrap();
        assert!(n >= 1, "reconciled at least our orphan");
        assert_eq!(
            store.run_status(&run_id).await.unwrap().as_deref(),
            Some("cancelled")
        );
    }
}
