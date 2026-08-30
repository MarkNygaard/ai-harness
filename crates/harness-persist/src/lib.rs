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

mod billing;
pub use billing::{BillingProfile, BillingProfileInput, BillingProfileStore};

mod projects;
pub use projects::{Project, ProjectInput, ProjectRepo, ProjectStore};

mod categories;
pub use categories::{Category, CategoryInput, CategoryStore};
mod linear_sources;
pub use linear_sources::{LinearSource, LinearSourceInput, LinearSourceStore};

mod linear_claims;
pub use linear_claims::{LinearClaim, LinearClaimStore};

mod settings;
pub use settings::SettingsStore;

mod users;
pub use users::{NewUser, ProfileUpdate, User, UserStore};

mod tokens;
pub use tokens::{AccessToken, TokenStore};

mod invites;
pub use invites::{Invite, InviteStore, KIND_INVITE, KIND_RESET};

mod finding_state;
pub use finding_state::{FindingState, FindingStateInput, FindingStateStore};

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
    /// Earliest node start across the run's nodes (`MIN(n.started_at)`); `None`
    /// when no node has timing (e.g. a run with no started nodes).
    pub started_at: Option<DateTime<Utc>>,
    /// Latest node end across the run's nodes (`MAX(n.ended_at)`); `None` when
    /// no node has finished.
    pub ended_at: Option<DateTime<Utc>>,
    /// A/B pairing: shared id linking the two arms of a comparison; `None` for a
    /// normal (non-paired) run.
    pub ab_pair_id: Option<String>,
    /// Which arm of the pair this run is — `"a"` or `"b"`; `None` if not paired.
    pub ab_arm: Option<String>,
    /// Display label for the arm's substituted model (e.g. `"cursor/composer-2.5"`),
    /// so the comparison view can name each arm without re-reading node rows.
    pub ab_label: Option<String>,
    /// Who asked for this run — a user id, or a label like `linear` for a run
    /// nothing signed-in started. `None` on runs from before accounts existed.
    pub triggered_by: Option<String>,
}
/// A/B pairing metadata stamped on a run at start time (borrowed for binding).
#[derive(Debug, Clone, Copy)]
pub struct AbPairing<'a> {
    /// Shared id linking the two arms of a comparison.
    pub pair_id: &'a str,
    /// Which arm this run is — `"a"` or `"b"`.
    pub arm: &'a str,
    /// Display label for the arm's substituted model (e.g. `"cursor/composer-2.5"`).
    pub label: Option<&'a str>,
}

/// One (project, day, status) tally for the dashboard aggregate.
#[derive(Debug, Clone, Serialize, sqlx::FromRow)]
pub struct RunDailyCount {
    /// Project the runs belong to; `None` for older/CLI runs.
    pub project: Option<String>,
    /// Day bucket (UTC, midnight) the runs were recorded in.
    pub day: DateTime<Utc>,
    /// Terminal run status: "completed" | "failed" | "cancelled".
    pub status: String,
    pub count: i64,
}

/// Summed token usage for one model over a time window — the input to billing
/// calibration (roll per-node usage up to the subscription that pays for it).
#[derive(Debug, Clone, Serialize, sqlx::FromRow)]
pub struct ModelTokenSum {
    pub model: String,
    pub input_tokens: i64,
    pub output_tokens: i64,
    pub cache_read: i64,
    pub cache_write: i64,
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
    pub artifact_content: Option<String>,
}

/// One raw error line joined to its run — the input to the grouping below.
///
/// `message` is the failure text the query already coalesced: a failing tool
/// result carries an empty `activity` and puts its output in `detail` (see
/// [`harness_dag::Activity::tool_result`]), and since that constructor is the
/// only producer of `is_error` rows, reading `activity` alone would leave every
/// message blank — and blank messages all share one fingerprint, collapsing
/// unrelated failures into a single meaningless group.
#[derive(Debug, sqlx::FromRow)]
struct ActivityErrorRow {
    project: Option<String>,
    workflow_name: String,
    node_id: String,
    message: String,
    run_id: String,
    created_at: DateTime<Utc>,
}

/// One recurring agent-side failure, aggregated across runs.
///
/// The activity table records every tool result an agent produced, but it could
/// only ever be read one run at a time — enough to render a live feed, useless for
/// answering "what do the agents keep tripping over?". This is that second
/// question: identical failures collapsed into one row with a count, so a
/// repeated obstacle is visible as a pattern rather than as a line in a feed
/// nobody re-reads.
#[derive(Debug, Clone, Serialize)]
pub struct ActivityErrorGroup {
    /// How many times this failure appeared in the window.
    pub count: i64,
    /// How many distinct runs hit it — a high count over one run is one agent
    /// looping; over many runs it is a property of the project.
    pub runs: i64,
    pub project: Option<String>,
    pub workflow: String,
    /// Node ids where it appeared (capped, most frequent first).
    pub nodes: Vec<String>,
    /// A representative message, verbatim.
    pub sample: String,
    pub first_seen: DateTime<Utc>,
    pub last_seen: DateTime<Utc>,
}

/// Collapse a failure message to a coarse fingerprint so near-identical ones group.
///
/// Only run-specific noise is removed: digits become `N` and long hex runs become
/// `ID`, so "took 1243ms" and "took 87ms" are one pattern and a uuid does not split
/// a group per run. Deliberately crude — it is a grouping key, not a parser, and
/// the group carries a verbatim `sample` so nothing is lost to the normalisation.
pub fn error_fingerprint(message: &str) -> String {
    let mut out = String::with_capacity(message.len());
    let mut hex_run = 0usize;
    let mut pending_hex = String::new();
    let flush = |out: &mut String, pending: &mut String, run: usize| {
        if run >= 8 {
            out.push_str("ID");
        } else {
            out.push_str(pending);
        }
        pending.clear();
    };
    for c in message.chars() {
        // A long unbroken hex-ish token is an id; a short one is a word.
        if c.is_ascii_hexdigit() || c == '-' {
            hex_run += 1;
            pending_hex.push(c);
            continue;
        }
        flush(&mut out, &mut pending_hex, hex_run);
        hex_run = 0;
        if c.is_ascii_digit() {
            if !out.ends_with('N') {
                out.push('N');
            }
        } else {
            out.push(c);
        }
    }
    flush(&mut out, &mut pending_hex, hex_run);
    // Digits inside a short hex token still collapse, so the two passes agree.
    let collapsed: String = out
        .chars()
        .scan(false, |prev_digit, c| {
            let is_digit = c.is_ascii_digit();
            let keep = !(is_digit && *prev_digit);
            *prev_digit = is_digit;
            Some((c, keep))
        })
        .filter(|(_, keep)| *keep)
        .map(|(c, _)| if c.is_ascii_digit() { 'N' } else { c })
        .collect();
    collapsed.trim().chars().take(200).collect()
}

/// One persisted activity line (matches `harness_run_activity`). `id` is the
/// cursor the UI pages through to fetch only lines newer than what it has.
/// `kind` is one of `text` / `tool` / `tool_result`; `detail` carries a tool's
/// input summary or a result snippet (`None` for plain text).
#[derive(Debug, Clone, Serialize, sqlx::FromRow)]
pub struct ActivityEvent {
    pub id: i64,
    pub node_id: String,
    pub kind: String,
    /// The primary line (assistant text, or a tool name). Selected as `text`
    /// from the `activity` column so the JSON matches the live `Activity` shape.
    pub text: String,
    pub detail: Option<String>,
    /// Tool-call correlation id (pairs a `tool` with its `tool_result`).
    pub tool_id: Option<String>,
    /// Whether a `tool_result` reported failure (✗ vs ✓ in the UI).
    pub is_error: bool,
    pub created_at: DateTime<Utc>,
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
    -- Who asked for this run: a user id, or a label like `linear` for a run
    -- nothing signed-in started. NULL on every run from before accounts.
    triggered_by  text,
    heartbeat_at  timestamptz,
    recorded_at   timestamptz NOT NULL DEFAULT now()
)";

/// Bring older `harness_workflow_runs` tables up to date. Idempotent.
const ALTER_RUNS_GRAPH: &str =
    "ALTER TABLE harness_workflow_runs ADD COLUMN IF NOT EXISTS graph jsonb NOT NULL DEFAULT '[]'::jsonb";
/// Who asked for a run. Idempotent.
const ALTER_RUNS_TRIGGERED_BY: &str =
    "ALTER TABLE harness_workflow_runs ADD COLUMN IF NOT EXISTS triggered_by text";
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
const ALTER_NODES_ARTIFACT: &str =
    "ALTER TABLE harness_run_nodes ADD COLUMN IF NOT EXISTS artifact_content text";
/// A/B pairing columns: `ab_pair_id` links the two arms, `ab_arm` is 'a'/'b',
/// `ab_label` names the arm's substituted model for display. Idempotent.
const ALTER_RUNS_AB_PAIR_ID: &str =
    "ALTER TABLE harness_workflow_runs ADD COLUMN IF NOT EXISTS ab_pair_id text";
const ALTER_RUNS_AB_ARM: &str =
    "ALTER TABLE harness_workflow_runs ADD COLUMN IF NOT EXISTS ab_arm text";
const ALTER_RUNS_AB_LABEL: &str =
    "ALTER TABLE harness_workflow_runs ADD COLUMN IF NOT EXISTS ab_label text";
const INDEX_RUNS_AB_PAIR: &str =
    "CREATE INDEX IF NOT EXISTS idx_harness_workflow_runs_ab_pair ON harness_workflow_runs(ab_pair_id) WHERE ab_pair_id IS NOT NULL";
const INDEX_RUNS_RECORDED_AT: &str =
    "CREATE INDEX IF NOT EXISTS idx_harness_workflow_runs_recorded_at ON harness_workflow_runs(recorded_at DESC)";
const INDEX_RUNS_PROJECT_RECORDED_AT: &str =
    "CREATE INDEX IF NOT EXISTS idx_harness_workflow_runs_project_recorded_at ON harness_workflow_runs(project, recorded_at DESC)";

/// Links an A/B pair to the run that judged it (`judge-ab`). One judgement per
/// pair; re-judging upserts the row to point at the new judge run.
const CREATE_PAIR_JUDGE: &str = "
CREATE TABLE IF NOT EXISTS harness_ab_pair_judge (
    pair_id      text PRIMARY KEY,
    judge_run_id text NOT NULL,
    created_at   timestamptz NOT NULL DEFAULT now()
)";

/// Per-node live "activity" lines (the throttled progress overlay an agent
/// emits as it works). Unlike `harness_run_nodes` this is an append-only log —
/// one row per emitted line — so a late-connecting, refreshed, or cross-process
/// viewer can replay what happened instead of depending on a perfectly-timed
/// live SSE subscription (which 404s the moment it's missed). `id` is a
/// monotonic per-table cursor the UI pages through (`activity_since`). Rows
/// cascade-delete with their run.
const CREATE_ACTIVITY: &str = "
CREATE TABLE IF NOT EXISTS harness_run_activity (
    id          bigserial PRIMARY KEY,
    run_id      text NOT NULL REFERENCES harness_workflow_runs(id) ON DELETE CASCADE,
    node_id     text NOT NULL,
    kind        text NOT NULL DEFAULT 'text',
    activity    text NOT NULL,
    detail      text,
    tool_id     text,
    is_error    boolean NOT NULL DEFAULT false,
    created_at  timestamptz NOT NULL DEFAULT now()
)";
const INDEX_ACTIVITY_RUN_ID: &str =
    "CREATE INDEX IF NOT EXISTS idx_harness_run_activity_run_id ON harness_run_activity(run_id, id)";
/// Bring a Phase-1 activity table (text-only) up to the typed schema. Idempotent.
const ALTER_ACTIVITY_KIND: &str =
    "ALTER TABLE harness_run_activity ADD COLUMN IF NOT EXISTS kind text NOT NULL DEFAULT 'text'";
const ALTER_ACTIVITY_DETAIL: &str =
    "ALTER TABLE harness_run_activity ADD COLUMN IF NOT EXISTS detail text";
const ALTER_ACTIVITY_TOOL_ID: &str =
    "ALTER TABLE harness_run_activity ADD COLUMN IF NOT EXISTS tool_id text";
const ALTER_ACTIVITY_IS_ERROR: &str =
    "ALTER TABLE harness_run_activity ADD COLUMN IF NOT EXISTS is_error boolean NOT NULL DEFAULT false";

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
    artifact_content text,
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
        sqlx::query(ALTER_RUNS_TRIGGERED_BY)
            .execute(&self.pool)
            .await?;
        sqlx::query(ALTER_RUNS_TITLE).execute(&self.pool).await?;
        sqlx::query(ALTER_RUNS_DESCRIPTION)
            .execute(&self.pool)
            .await?;
        sqlx::query(ALTER_RUNS_OWNER).execute(&self.pool).await?;
        sqlx::query(ALTER_RUNS_HEARTBEAT)
            .execute(&self.pool)
            .await?;
        sqlx::query(ALTER_RUNS_AB_PAIR_ID)
            .execute(&self.pool)
            .await?;
        sqlx::query(ALTER_RUNS_AB_ARM).execute(&self.pool).await?;
        sqlx::query(ALTER_RUNS_AB_LABEL).execute(&self.pool).await?;
        sqlx::query(CREATE_NODES).execute(&self.pool).await?;
        sqlx::query(ALTER_NODES_ARTIFACT)
            .execute(&self.pool)
            .await?;
        sqlx::query(CREATE_ACTIVITY).execute(&self.pool).await?;
        sqlx::query(ALTER_ACTIVITY_KIND).execute(&self.pool).await?;
        sqlx::query(ALTER_ACTIVITY_DETAIL)
            .execute(&self.pool)
            .await?;
        sqlx::query(ALTER_ACTIVITY_TOOL_ID)
            .execute(&self.pool)
            .await?;
        sqlx::query(ALTER_ACTIVITY_IS_ERROR)
            .execute(&self.pool)
            .await?;
        sqlx::query(INDEX_ACTIVITY_RUN_ID)
            .execute(&self.pool)
            .await?;
        sqlx::query(INDEX_RUNS_AB_PAIR).execute(&self.pool).await?;
        sqlx::query(CREATE_PAIR_JUDGE).execute(&self.pool).await?;
        sqlx::query(INDEX_RUNS_RECORDED_AT)
            .execute(&self.pool)
            .await?;
        sqlx::query(INDEX_RUNS_PROJECT_RECORDED_AT)
            .execute(&self.pool)
            .await?;
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
                status        = CASE
                                  WHEN harness_workflow_runs.status = 'cancelled'
                                  THEN harness_workflow_runs.status
                                  ELSE excluded.status
                                END,
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
                    started_at, ended_at, artifact_content)
                 VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,$12,$13,$14,$15,$16,$17)",
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
            .bind(node.artifact_content.as_deref())
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
        ab: Option<&AbPairing<'_>>,
        triggered_by: Option<&str>,
    ) -> Result<(), PersistError> {
        // Stamp the lease (`owner` + fresh `heartbeat_at`) so this run is
        // claimed by the current instance and protected from reconcile until its
        // heartbeat goes stale. A/B fields are stamped once at start and preserved
        // on conflict (COALESCE) so a re-`start_run` never drops the pairing.
        sqlx::query(
            "INSERT INTO harness_workflow_runs (id, workflow_name, title, description, status, project, node_count, graph, owner, ab_pair_id, ab_arm, ab_label, triggered_by, heartbeat_at, recorded_at)
             VALUES ($1, $2, $3, $4, 'running', $5, $6, $7, $8, $9, $10, $11, $12, now(), now())
             ON CONFLICT (id) DO UPDATE SET
                workflow_name = excluded.workflow_name,
                title         = COALESCE(excluded.title, harness_workflow_runs.title),
                description   = COALESCE(excluded.description, harness_workflow_runs.description),
                node_count    = excluded.node_count,
                graph         = excluded.graph,
                owner         = excluded.owner,
                ab_pair_id    = COALESCE(harness_workflow_runs.ab_pair_id, excluded.ab_pair_id),
                ab_arm        = COALESCE(harness_workflow_runs.ab_arm, excluded.ab_arm),
                ab_label      = COALESCE(harness_workflow_runs.ab_label, excluded.ab_label),
                -- Preserve-first, like the A/B fields: a re-`start_run` must not
                -- reattribute a run to whoever restarted it.
                triggered_by  = COALESCE(harness_workflow_runs.triggered_by, excluded.triggered_by),
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
        .bind(ab.map(|a| a.pair_id))
        .bind(ab.map(|a| a.arm))
        .bind(ab.and_then(|a| a.label))
        .bind(triggered_by)
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
                started_at, ended_at, artifact_content)
             VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,$12,$13,$14,$15,$16,$17)
             ON CONFLICT (run_id, node_id) DO UPDATE SET
                ordinal=excluded.ordinal, status=excluded.status, provider=excluded.provider,
                model=excluded.model, output=excluded.output, iterations=excluded.iterations,
                converged=excluded.converged, note=excluded.note,
                input_tokens=excluded.input_tokens, output_tokens=excluded.output_tokens,
                cache_read=excluded.cache_read, cache_write=excluded.cache_write,
                started_at=excluded.started_at, ended_at=excluded.ended_at,
                artifact_content=excluded.artifact_content",
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
        .bind(node.artifact_content.as_deref())
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    /// Append one live activity for a node (the durable backing for the progress
    /// feed): its `kind` (text / tool / tool_result), primary line, and optional
    /// detail. Best-effort and append-only: a dropped write just loses one line.
    /// Skips persisting if the run row is gone (the FK would reject it) — e.g. a
    /// late event after a delete.
    pub async fn record_activity(
        &self,
        run_id: &str,
        node_id: &str,
        activity: &harness_dag::Activity,
    ) -> Result<(), PersistError> {
        sqlx::query(
            "INSERT INTO harness_run_activity (run_id, node_id, kind, activity, detail, tool_id, is_error)
             SELECT $1, $2, $3, $4, $5, $6, $7
             WHERE EXISTS (SELECT 1 FROM harness_workflow_runs WHERE id = $1)",
        )
        .bind(run_id)
        .bind(node_id)
        .bind(activity_kind_str(&activity.kind))
        .bind(&activity.text)
        .bind(activity.detail.as_deref())
        .bind(activity.tool_id.as_deref())
        .bind(activity.is_error)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    /// Activity lines for a run with `id > after`, oldest first, capped at
    /// `limit`. The UI polls this with the highest `id` it has seen as `after`
    /// (`0` for the first fetch) so each poll returns only what's new.
    pub async fn activity_since(
        &self,
        run_id: &str,
        after: i64,
        limit: i64,
    ) -> Result<Vec<ActivityEvent>, PersistError> {
        let rows = sqlx::query_as::<_, ActivityEvent>(
            "SELECT id, node_id, kind, activity AS text, detail, tool_id, is_error, created_at
             FROM harness_run_activity
             WHERE run_id = $1 AND id > $2
             ORDER BY id ASC
             LIMIT $3",
        )
        .bind(run_id)
        .bind(after)
        .bind(limit)
        .fetch_all(&self.pool)
        .await?;
        Ok(rows)
    }

    /// Recurring agent-side failures across runs, newest window first.
    ///
    /// Reads the same rows the live feed shows, but across every run instead of
    /// one, and collapses them by [`error_fingerprint`] so a repeated obstacle
    /// reads as a count rather than as scattered lines. `project` narrows to one
    /// project; `days` bounds the window; `scan_limit` bounds how many raw rows
    /// are considered (newest first) so a noisy month cannot pull an unbounded
    /// result set into memory.
    pub async fn activity_error_groups(
        &self,
        project: Option<&str>,
        days: i32,
        scan_limit: i64,
    ) -> Result<Vec<ActivityErrorGroup>, PersistError> {
        let rows: Vec<ActivityErrorRow> = sqlx::query_as(
            // A failing tool result keeps its text in `detail` and leaves
            // `activity` empty, so coalesce. A row with neither carries no
            // failure to report: dropping it beats grouping every one of them
            // under the single fingerprint that a blank message shares.
            "SELECT r.project, r.workflow_name, a.node_id, a.run_id, a.created_at,
                    NULLIF(btrim(COALESCE(NULLIF(a.activity, ''), a.detail, '')), '') AS message
                 FROM harness_run_activity a
                 JOIN harness_workflow_runs r ON r.id = a.run_id
                 WHERE a.is_error
                   AND NULLIF(btrim(COALESCE(NULLIF(a.activity, ''), a.detail, '')), '') IS NOT NULL
                   AND a.created_at >= now() - make_interval(days => $1)
                   AND ($2::text IS NULL OR r.project = $2)
                 ORDER BY a.created_at DESC
                 LIMIT $3",
        )
        .bind(days.max(1))
        .bind(project)
        .bind(scan_limit.max(1))
        .fetch_all(&self.pool)
        .await?;

        // Group in Rust rather than SQL: the fingerprint is the grouping key and
        // it is a plain function, so it stays testable without a database.
        use std::collections::HashMap;
        struct Acc {
            count: i64,
            runs: std::collections::HashSet<String>,
            nodes: HashMap<String, i64>,
            sample: String,
            first_seen: DateTime<Utc>,
            last_seen: DateTime<Utc>,
        }
        let mut acc: HashMap<(Option<String>, String, String), Acc> = HashMap::new();
        for row in rows {
            let ActivityErrorRow {
                project,
                workflow_name: workflow,
                node_id,
                message,
                run_id,
                created_at: at,
            } = row;
            let key = (
                project.clone(),
                workflow.clone(),
                error_fingerprint(&message),
            );
            let e = acc.entry(key).or_insert_with(|| Acc {
                count: 0,
                runs: std::collections::HashSet::new(),
                nodes: HashMap::new(),
                sample: message.clone(),
                first_seen: at,
                last_seen: at,
            });
            e.count += 1;
            e.runs.insert(run_id);
            *e.nodes.entry(node_id).or_insert(0) += 1;
            if at < e.first_seen {
                e.first_seen = at;
            }
            if at > e.last_seen {
                e.last_seen = at;
                e.sample = message;
            }
        }
        let mut out: Vec<ActivityErrorGroup> = acc
            .into_iter()
            .map(|((project, workflow, _), a)| {
                let mut nodes: Vec<(String, i64)> = a.nodes.into_iter().collect();
                nodes.sort_by(|x, y| y.1.cmp(&x.1).then_with(|| x.0.cmp(&y.0)));
                ActivityErrorGroup {
                    count: a.count,
                    runs: a.runs.len() as i64,
                    project,
                    workflow,
                    nodes: nodes.into_iter().take(6).map(|(n, _)| n).collect(),
                    sample: a.sample,
                    first_seen: a.first_seen,
                    last_seen: a.last_seen,
                }
            })
            .collect();
        // Most-repeated first, then most recent — the reading order for "what
        // should I write down in CLAUDE.md?".
        out.sort_by(|x, y| {
            y.count
                .cmp(&x.count)
                .then_with(|| y.last_seen.cmp(&x.last_seen))
        });
        Ok(out)
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

    /// IDs of every run currently in `running` status (cross-instance). The
    /// orphan-worktree sweeper keeps these and reclaims all other worktree dirs:
    /// the stale-lease reaper flips hard-killed runs to `cancelled` first, so a
    /// run still `running` here is genuinely live (or about to be reaped) and its
    /// worktree must not be deleted.
    pub async fn running_run_ids(&self) -> Result<std::collections::HashSet<String>, PersistError> {
        let rows: Vec<(String,)> =
            sqlx::query_as("SELECT id FROM harness_workflow_runs WHERE status = 'running'")
                .fetch_all(&self.pool)
                .await?;
        Ok(rows.into_iter().map(|(id,)| id).collect())
    }

    /// Count of runs for `project` currently in `running` status (cross-instance:
    /// reads persisted state, not just this process's live map).
    pub async fn count_active_runs(&self, project: &str) -> Result<i64, PersistError> {
        let row: (i64,) = sqlx::query_as(
            "SELECT count(*) FROM harness_workflow_runs
             WHERE project = $1 AND status = 'running'",
        )
        .bind(project)
        .fetch_one(&self.pool)
        .await?;
        Ok(row.0)
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
            "SELECT r.id, r.workflow_name, r.title, NULL::text AS description, r.status, r.project,
                    r.node_count, r.recorded_at,
                    MIN(n.started_at) AS started_at, MAX(n.ended_at) AS ended_at,
                    r.ab_pair_id, r.ab_arm, r.ab_label, r.triggered_by
             FROM harness_workflow_runs r
             LEFT JOIN harness_run_nodes n ON n.run_id = r.id
             GROUP BY r.id
             ORDER BY r.recorded_at DESC
             LIMIT $1",
        )
        .bind(limit)
        .fetch_all(&self.pool)
        .await?;
        Ok(rows)
    }

    /// List the most recently recorded runs for a project (newest first).
    pub async fn list_runs_for_project(
        &self,
        project: &str,
        limit: i64,
    ) -> Result<Vec<RunSummary>, PersistError> {
        let rows = sqlx::query_as::<_, RunSummary>(
            "SELECT r.id, r.workflow_name, r.title, NULL::text AS description, r.status, r.project,
                    r.node_count, r.recorded_at,
                    MIN(n.started_at) AS started_at, MAX(n.ended_at) AS ended_at,
                    r.ab_pair_id, r.ab_arm, r.ab_label, r.triggered_by
             FROM harness_workflow_runs r
             LEFT JOIN harness_run_nodes n ON n.run_id = r.id
             WHERE r.project = $1
             GROUP BY r.id
             ORDER BY r.recorded_at DESC
             LIMIT $2",
        )
        .bind(project)
        .bind(limit)
        .fetch_all(&self.pool)
        .await?;
        Ok(rows)
    }

    /// The runs of one A/B pair, ordered by arm (`a` then `b`). Usually two.
    pub async fn list_runs_for_pair(&self, pair_id: &str) -> Result<Vec<RunSummary>, PersistError> {
        let rows = sqlx::query_as::<_, RunSummary>(
            "SELECT r.id, r.workflow_name, r.title, r.description, r.status, r.project,
                    r.node_count, r.recorded_at,
                    MIN(n.started_at) AS started_at, MAX(n.ended_at) AS ended_at,
                    r.ab_pair_id, r.ab_arm, r.ab_label, r.triggered_by
             FROM harness_workflow_runs r
             LEFT JOIN harness_run_nodes n ON n.run_id = r.id
             WHERE r.ab_pair_id = $1
             GROUP BY r.id
             ORDER BY r.ab_arm ASC, r.recorded_at ASC",
        )
        .bind(pair_id)
        .fetch_all(&self.pool)
        .await?;
        Ok(rows)
    }

    /// Record (upsert) which run judged an A/B pair. Re-judging replaces it.
    pub async fn set_pair_judge(
        &self,
        pair_id: &str,
        judge_run_id: &str,
    ) -> Result<(), PersistError> {
        sqlx::query(
            "INSERT INTO harness_ab_pair_judge (pair_id, judge_run_id, created_at)
             VALUES ($1, $2, now())
             ON CONFLICT (pair_id) DO UPDATE SET
                judge_run_id = excluded.judge_run_id,
                created_at   = now()",
        )
        .bind(pair_id)
        .bind(judge_run_id)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    /// The run id that judged a pair, if any.
    pub async fn get_pair_judge(&self, pair_id: &str) -> Result<Option<String>, PersistError> {
        let row: Option<(String,)> =
            sqlx::query_as("SELECT judge_run_id FROM harness_ab_pair_judge WHERE pair_id = $1")
                .bind(pair_id)
                .fetch_optional(&self.pool)
                .await?;
        Ok(row.map(|(id,)| id))
    }

    /// List the most recently recorded runs without a project (newest first).
    pub async fn list_unassigned_runs(&self, limit: i64) -> Result<Vec<RunSummary>, PersistError> {
        let rows = sqlx::query_as::<_, RunSummary>(
            "SELECT r.id, r.workflow_name, r.title, NULL::text AS description, r.status, r.project,
                    r.node_count, r.recorded_at,
                    MIN(n.started_at) AS started_at, MAX(n.ended_at) AS ended_at,
                    r.ab_pair_id, r.ab_arm, r.ab_label, r.triggered_by
             FROM harness_workflow_runs r
             LEFT JOIN harness_run_nodes n ON n.run_id = r.id
             WHERE r.project IS NULL OR btrim(r.project) = ''
             GROUP BY r.id
             ORDER BY r.recorded_at DESC
             LIMIT $1",
        )
        .bind(limit)
        .fetch_all(&self.pool)
        .await?;
        Ok(rows)
    }

    /// Per-project, per-day counts of **finished** runs since `since`, for the
    /// dashboard. Buckets on `recorded_at` (this table has no `ended_at`; on a
    /// terminal transition `recorded_at` is set to `now()`), in UTC, and counts
    /// only terminal statuses so in-flight runs don't inflate "what got done".
    pub async fn runs_daily_summary(
        &self,
        since: DateTime<Utc>,
    ) -> Result<Vec<RunDailyCount>, PersistError> {
        let rows = sqlx::query_as::<_, RunDailyCount>(
            "SELECT project, date_trunc('day', recorded_at AT TIME ZONE 'UTC') AT TIME ZONE 'UTC' AS day, status, count(*) AS count
             FROM harness_workflow_runs
             WHERE recorded_at >= $1
               AND status IN ('completed', 'failed', 'cancelled')
             GROUP BY project, day, status
             ORDER BY day DESC",
        )
        .bind(since)
        .fetch_all(&self.pool)
        .await?;
        Ok(rows)
    }

    /// Summed token usage per model across all nodes finished since `since`.
    /// Feeds billing calibration: roll usage up by model lane, price it, and
    /// compare against how much of a subscription window it consumed.
    pub async fn token_sums_by_model_since(
        &self,
        since: DateTime<Utc>,
    ) -> Result<Vec<ModelTokenSum>, PersistError> {
        let rows = sqlx::query_as::<_, ModelTokenSum>(
            // SUM() over a BIGINT column yields NUMERIC in Postgres; cast back
            // to BIGINT so it decodes into the i64 fields of ModelTokenSum.
            "SELECT model,
                    COALESCE(SUM(input_tokens), 0)::BIGINT  AS input_tokens,
                    COALESCE(SUM(output_tokens), 0)::BIGINT AS output_tokens,
                    COALESCE(SUM(cache_read), 0)::BIGINT    AS cache_read,
                    COALESCE(SUM(cache_write), 0)::BIGINT   AS cache_write
             FROM harness_run_nodes
             WHERE model IS NOT NULL AND ended_at >= $1
             GROUP BY model",
        )
        .bind(since)
        .fetch_all(&self.pool)
        .await?;
        Ok(rows)
    }

    /// Fetch a run plus its node rows (ordered by declaration order).
    pub async fn get_run(&self, run_id: &str) -> Result<Option<RunDetail>, PersistError> {
        let run = sqlx::query_as::<_, RunSummary>(
            "SELECT r.id, r.workflow_name, r.title, r.description, r.status, r.project,
                    r.node_count, r.recorded_at,
                    MIN(n.started_at) AS started_at, MAX(n.ended_at) AS ended_at,
                    r.ab_pair_id, r.ab_arm, r.ab_label, r.triggered_by
             FROM harness_workflow_runs r
             LEFT JOIN harness_run_nodes n ON n.run_id = r.id
             WHERE r.id = $1
             GROUP BY r.id",
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
                    note, input_tokens, output_tokens, cache_read, cache_write, started_at, ended_at,
                    artifact_content
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

fn activity_kind_str(k: &harness_dag::ActivityKind) -> &'static str {
    match k {
        harness_dag::ActivityKind::Text => "text",
        harness_dag::ActivityKind::Tool => "tool",
        harness_dag::ActivityKind::ToolResult => "tool_result",
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
mod fingerprint_tests {
    use super::error_fingerprint as fp;

    /// The point of the fingerprint is that the *same obstacle* groups even when
    /// the message carries per-run noise. These pairs must land in one group.
    #[test]
    fn run_specific_noise_does_not_split_a_group() {
        assert_eq!(fp("timed out after 1243ms"), fp("timed out after 87ms"));
        assert_eq!(
            fp("run 4f3a9c2e1b7d8a05 failed to start"),
            fp("run 91bc0de4a2f36587 failed to start")
        );
        assert_eq!(fp("retry 1 of 3"), fp("retry 2 of 3"));
    }

    /// And genuinely different obstacles must not collapse into one, or the count
    /// stops meaning anything.
    #[test]
    fn different_failures_stay_apart() {
        assert_ne!(fp("permission denied"), fp("path not found"));
        assert_ne!(
            fp("cannot find module ./stores"),
            fp("cannot find module ./sanity")
        );
    }

    #[test]
    fn the_shape_is_still_readable_and_bounded() {
        // A fingerprint is a key, but it should not be gibberish when logged.
        assert!(fp("typecheck failed with 42 errors").contains("typecheck failed with"));
        assert!(fp(&"x".repeat(500)).chars().count() <= 200);
        assert_eq!(
            fp("   trailing and leading   ").trim(),
            fp("trailing and leading")
        );
    }
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
                    artifact: None,
                },
                harness_dag::NodeMeta {
                    id: "review".into(),
                    depends_on: vec!["build".into()],
                    category: None,
                    artifact: None,
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
                    artifact_content: Some("# explore\nsample".into()),
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
                    artifact_content: None,
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
        // The "build" node carries start/end timestamps, so the run's derived
        // timing (MIN start / MAX end across nodes) must be populated.
        assert!(
            listed_run.started_at.is_some(),
            "list_runs must derive started_at from node timings"
        );
        assert!(
            listed_run.ended_at.is_some(),
            "list_runs must derive ended_at from node timings"
        );
        let detail = store.get_run(&run_id).await.unwrap().expect("detail");
        assert_eq!(detail.run.status, "completed");
        assert_eq!(detail.nodes.len(), 2);
        assert_eq!(detail.nodes[0].node_id, "build");
        assert_eq!(detail.nodes[0].input_tokens, Some(100));
        assert_eq!(
            detail.nodes[0].artifact_content.as_deref(),
            Some("# explore\nsample")
        );
        assert_eq!(detail.nodes[1].artifact_content, None);
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
    async fn runs_daily_summary_groups_by_project_day_and_status() {
        let Some(url) = db_url() else {
            eprintln!("skipping: HARNESS_DATABASE_URL not set");
            return;
        };
        let store = RunStore::connect(&url).await.expect("connect");

        let uid = |prefix: &str| {
            format!(
                "{prefix}-{}",
                chrono::Utc::now().timestamp_nanos_opt().unwrap()
            )
        };
        // Unique project names: `runs_daily_summary` buckets by project over a
        // 1-day window of the SHARED test DB, so fixed names would also count
        // rows other runs left behind (the "left: 3, right: 2" flake).
        let proj_a = uid("proj-a");
        let proj_b = uid("proj-b");

        // Two completed runs in proj-a.
        let mut report_a = sample_report();
        report_a.status = RunStatus::Completed;
        for i in 0..2 {
            let run_id = uid(&format!("test-dash-a-{i}"));
            store
                .record_run(
                    &run_id,
                    Some("task-a"),
                    None,
                    Some(proj_a.as_str()),
                    &report_a,
                )
                .await
                .unwrap();
        }

        // One failed run in proj-b.
        let mut report_b = sample_report();
        report_b.status = RunStatus::Failed;
        let run_id_b = uid("test-dash-b");
        store
            .record_run(
                &run_id_b,
                Some("task-b"),
                None,
                Some(proj_b.as_str()),
                &report_b,
            )
            .await
            .unwrap();

        // One running run (should be excluded from the aggregate).
        let run_id_c = uid("test-dash-c");
        store
            .start_run(
                &run_id_c,
                &report_a.workflow,
                Some("task-c"),
                None,
                Some(proj_a.as_str()),
                2,
                &report_a.graph,
                None,
                None,
                None,
            )
            .await
            .unwrap();

        let rows = store
            .runs_daily_summary(chrono::Utc::now() - chrono::Duration::days(1))
            .await
            .unwrap();

        let proj_a_completed: i64 = rows
            .iter()
            .filter(|r| r.project.as_deref() == Some(proj_a.as_str()) && r.status == "completed")
            .map(|r| r.count)
            .sum();
        let proj_b_failed: i64 = rows
            .iter()
            .filter(|r| r.project.as_deref() == Some(proj_b.as_str()) && r.status == "failed")
            .map(|r| r.count)
            .sum();
        let any_running = rows.iter().any(|r| r.status == "running");

        assert_eq!(proj_a_completed, 2, "proj-a should have 2 completed runs");
        assert_eq!(proj_b_failed, 1, "proj-b should have 1 failed run");
        assert!(!any_running, "running runs must be excluded from aggregate");
        for r in &rows {
            assert_eq!(
                r.day.time(),
                chrono::NaiveTime::from_hms_opt(0, 0, 0).unwrap(),
                "day bucket must be at UTC midnight"
            );
        }
    }

    #[tokio::test]
    #[serial_test::serial]
    async fn runs_daily_summary_excludes_runs_outside_window() {
        let Some(url) = db_url() else {
            eprintln!("skipping: HARNESS_DATABASE_URL not set");
            return;
        };
        let store = RunStore::connect(&url).await.expect("connect");

        let uid = |prefix: &str| {
            format!(
                "{prefix}-{}",
                chrono::Utc::now().timestamp_nanos_opt().unwrap()
            )
        };

        let mut report = sample_report();
        report.status = RunStatus::Completed;
        let run_id = uid("test-dash-window");
        store
            .record_run(&run_id, Some("task"), None, Some("proj"), &report)
            .await
            .unwrap();

        // Backdate the run so it falls outside a 1-hour window.
        sqlx::query(
            "UPDATE harness_workflow_runs SET recorded_at = now() - interval '2 hours' WHERE id = $1",
        )
        .bind(&run_id)
        .execute(&store.pool)
        .await
        .unwrap();

        let rows = store
            .runs_daily_summary(chrono::Utc::now() - chrono::Duration::hours(1))
            .await
            .unwrap();

        assert!(
            rows.iter().all(|r| r.project.as_deref() != Some("proj")),
            "runs older than the since window must be excluded"
        );
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
                None,
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
    async fn activity_log_appends_and_pages_by_cursor() {
        let Some(url) = db_url() else {
            eprintln!("skipping: HARNESS_DATABASE_URL not set");
            return;
        };
        let store = RunStore::connect(&url).await.expect("connect");
        let run_id = format!(
            "test-activity-{}",
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
                None,
                None,
            )
            .await
            .unwrap();

        // Append a typed feed for the "build" node: a tool call, its result
        // (same tool_id, for pairing), then text.
        let acts = [
            harness_dag::Activity::tool("bash", Some("cargo test".into()), Some("toolu_1".into())),
            harness_dag::Activity::tool_result("test result: ok", Some("toolu_1".into()), false),
            harness_dag::Activity::text("📋 1/3 first task"),
        ];
        for a in &acts {
            store.record_activity(&run_id, "build", a).await.unwrap();
        }

        // First fetch (after=0) returns all three, oldest first, with kind/detail/tool_id.
        let all = store.activity_since(&run_id, 0, 100).await.unwrap();
        assert_eq!(all.len(), 3);
        assert_eq!(all[0].kind, "tool");
        assert_eq!(all[0].text, "bash");
        assert_eq!(all[0].detail.as_deref(), Some("cargo test"));
        assert_eq!(all[0].tool_id.as_deref(), Some("toolu_1"));
        assert_eq!(all[1].kind, "tool_result");
        assert_eq!(all[1].detail.as_deref(), Some("test result: ok"));
        assert_eq!(all[1].tool_id.as_deref(), Some("toolu_1")); // pairs with the call
        assert!(!all[1].is_error);
        assert_eq!(all[2].kind, "text");
        assert_eq!(all[2].text, "📋 1/3 first task");
        assert!(all[0].id < all[1].id && all[1].id < all[2].id, "ids ascend");

        // Paging by cursor returns only newer lines.
        let after = all[1].id;
        let tail = store.activity_since(&run_id, after, 100).await.unwrap();
        assert_eq!(tail.len(), 1);
        assert_eq!(tail[0].text, "📋 1/3 first task");

        // A write for an unknown run is a no-op (FK guard), not an error.
        store
            .record_activity("no-such-run", "n", &harness_dag::Activity::text("x"))
            .await
            .unwrap();
        assert!(store
            .activity_since("no-such-run", 0, 100)
            .await
            .unwrap()
            .is_empty());

        // Deleting the run cascades its activity away.
        store.delete_run(&run_id).await.unwrap();
        assert!(store
            .activity_since(&run_id, 0, 100)
            .await
            .unwrap()
            .is_empty());
    }

    #[tokio::test]
    #[serial_test::serial]
    async fn activity_error_groups_read_the_message_a_failing_tool_actually_wrote() {
        let Some(url) = db_url() else {
            eprintln!("skipping: HARNESS_DATABASE_URL not set");
            return;
        };
        let store = RunStore::connect(&url).await.expect("connect");
        let stamp = chrono::Utc::now().timestamp_nanos_opt().unwrap();
        let run_id = format!("test-errgroup-{stamp}");
        // A project of this run's own, so the window filter isolates it from
        // whatever else the suite left in the table.
        let project = format!("errgroup-{stamp}");
        let report = sample_report();
        store
            .start_run(
                &run_id,
                &report.workflow,
                None,
                None,
                Some(&project),
                2,
                &report.graph,
                None,
                None,
                None,
            )
            .await
            .unwrap();

        // `Activity::tool_result` is the only producer of `is_error` rows and it
        // leaves `activity` empty, putting the message in `detail` — so a reader
        // that consults `activity` alone sees nothing but blanks.
        let acts = [
            harness_dag::Activity::tool_result(
                "module not found: @/i18n/stores",
                Some("t1".into()),
                true,
            ),
            harness_dag::Activity::tool_result(
                "module not found: @/i18n/stores",
                Some("t2".into()),
                true,
            ),
            harness_dag::Activity::tool_result("permission denied: .env", Some("t3".into()), true),
            // Not a failure: must not be counted.
            harness_dag::Activity::tool_result("tests: 930 passed", Some("t4".into()), false),
            // A failure with nothing to say: dropped, not grouped as blank.
            harness_dag::Activity::tool_result("   ", Some("t5".into()), true),
        ];
        for a in &acts {
            store.record_activity(&run_id, "build", a).await.unwrap();
        }

        let groups = store
            .activity_error_groups(Some(&project), 1, 1000)
            .await
            .unwrap();

        assert_eq!(groups.len(), 2, "two distinct failures: {groups:?}");
        for g in &groups {
            assert!(
                !g.sample.trim().is_empty(),
                "a group with no sample tells nobody what to fix: {g:?}"
            );
            assert_eq!(g.runs, 1);
            assert_eq!(g.nodes, vec!["build".to_string()]);
        }
        // Most-repeated first, and the repeat is collapsed rather than listed twice.
        assert_eq!(groups[0].count, 2);
        assert!(groups[0].sample.contains("@/i18n/stores"), "{groups:?}");
        assert_eq!(groups[1].count, 1);
        assert!(groups[1].sample.contains("permission denied"), "{groups:?}");

        store.delete_run(&run_id).await.unwrap();
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
                None,
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

        // A late final snapshot must not resurrect a run cancelled by the API
        // or stale-lease reaper.
        store
            .record_run(&run_id, None, None, None, &report)
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
                None,
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

    #[tokio::test]
    #[serial_test::serial]
    async fn running_run_ids_tracks_running_runs() {
        let Some(url) = db_url() else {
            eprintln!("skipping: HARNESS_DATABASE_URL not set to a test database");
            return;
        };
        let store = RunStore::connect(&url).await.expect("connect");
        let run_id = format!(
            "test-running-{}",
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
                None,
                None,
            )
            .await
            .unwrap();

        // A freshly-started run is `running` → present (its worktree is protected).
        assert!(store.running_run_ids().await.unwrap().contains(&run_id));

        // Once cancelled it drops out → the sweeper may reclaim its worktree.
        store
            .reconcile_orphaned_runs(std::time::Duration::ZERO)
            .await
            .unwrap();
        assert!(!store.running_run_ids().await.unwrap().contains(&run_id));
    }
}
