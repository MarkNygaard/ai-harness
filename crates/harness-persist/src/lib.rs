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
use harness_dag::{NodeMeta, NodeStatus, RunReport, RunStatus};
use serde::Serialize;
use sqlx::postgres::{PgPool, PgPoolOptions};
use sqlx::types::Json;

mod credentials;
pub use credentials::{CredentialStore, ProviderCredential};

/// A run row for listing (matches `harness_workflow_runs`).
#[derive(Debug, Clone, Serialize, sqlx::FromRow)]
pub struct RunSummary {
    pub id: String,
    pub workflow_name: String,
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
    status        text NOT NULL,
    project       text,
    node_count    int  NOT NULL DEFAULT 0,
    graph         jsonb NOT NULL DEFAULT '[]'::jsonb,
    recorded_at   timestamptz NOT NULL DEFAULT now()
)";

/// Bring older `harness_workflow_runs` tables (created before the topology
/// column existed) up to date. Idempotent.
const ALTER_RUNS_GRAPH: &str =
    "ALTER TABLE harness_workflow_runs ADD COLUMN IF NOT EXISTS graph jsonb NOT NULL DEFAULT '[]'::jsonb";

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
        sqlx::query(CREATE_NODES).execute(&self.pool).await?;
        Ok(())
    }

    /// Persist a run and its per-node records. Idempotent on `run_id`: the run
    /// row is upserted and its node rows are replaced.
    pub async fn record_run(
        &self,
        run_id: &str,
        project: Option<&str>,
        report: &RunReport,
    ) -> Result<(), PersistError> {
        let mut tx = self.pool.begin().await?;

        sqlx::query(
            "INSERT INTO harness_workflow_runs (id, workflow_name, status, project, node_count, graph, recorded_at)
             VALUES ($1, $2, $3, $4, $5, $6, now())
             ON CONFLICT (id) DO UPDATE SET
                workflow_name = excluded.workflow_name,
                status        = excluded.status,
                project       = excluded.project,
                node_count    = excluded.node_count,
                graph         = excluded.graph,
                recorded_at   = now()",
        )
        .bind(run_id)
        .bind(&report.workflow)
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
            "SELECT id, workflow_name, status, project, node_count, recorded_at
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
            "SELECT id, workflow_name, status, project, node_count, recorded_at
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

#[cfg(test)]
mod tests {
    use super::*;
    use harness_dag::{NodeRun, Usage};

    /// Postgres-dependent tests run only when HARNESS_DATABASE_URL is set
    /// (CI provides one; locally `docker compose up -d postgres` + export it).
    fn db_url() -> Option<String> {
        std::env::var("HARNESS_DATABASE_URL").ok()
    }

    fn sample_report() -> RunReport {
        RunReport {
            workflow: "demo".into(),
            status: RunStatus::Completed,
            graph: vec![
                harness_dag::NodeMeta {
                    id: "build".into(),
                    depends_on: vec![],
                },
                harness_dag::NodeMeta {
                    id: "review".into(),
                    depends_on: vec!["build".into()],
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
            .record_run(&run_id, Some("proj-a"), &report)
            .await
            .expect("record");

        assert_eq!(
            store.run_status(&run_id).await.unwrap().as_deref(),
            Some("completed")
        );
        assert_eq!(store.node_count(&run_id).await.unwrap(), 2);

        // Idempotent re-record keeps node count stable.
        store
            .record_run(&run_id, Some("proj-a"), &report)
            .await
            .unwrap();
        assert_eq!(store.node_count(&run_id).await.unwrap(), 2);

        // list_runs includes it; get_run returns ordered node detail.
        let listed = store.list_runs(50).await.unwrap();
        assert!(listed.iter().any(|r| r.id == run_id && r.node_count == 2));
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
        assert!(store.get_run("does-not-exist").await.unwrap().is_none());
    }
}
