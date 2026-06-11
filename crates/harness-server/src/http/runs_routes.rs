//! Control-plane API for workflow **runs** (the `harness-dag` execution model),
//! distinct from the legacy `tasks` surface.
//!
//! - `POST /runs` — submit a workflow file; executes it in a background task and
//!   returns a `run_id`.
//! - `GET /runs` — list recent runs (from `harness-persist`).
//! - `GET /runs/{id}` — run detail + per-node rows.
//! - `GET /runs/{id}/stream` — SSE stream of live `RunEvent`s while it runs.
//!
//! State is attached as an axum `Extension` (a self-contained `RunsState`) so it
//! doesn't entangle the large shared `AppState`.

use chrono::{DateTime, Duration, Utc};
use std::collections::HashMap;
use std::fs::File;
use std::io::Read;
use std::path::{Component, Path, PathBuf};
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use axum::extract::{Extension, Path as AxumPath, Query};
use axum::http::StatusCode;
use axum::response::sse::{Event, KeepAlive, Sse};
use axum::response::{IntoResponse, Response};
use axum::Json;
use futures::StreamExt;
use harness_agents::registry::AgentRegistry;
use harness_dag::{parse_workflow, run_workflow_streaming, RunEvent, VarContext};
use harness_persist::{ProjectStore, RunStore};
use harness_runner::{
    sanitize_branch_component, CodeAgentRunner, CursorAgent, DispatchAgent, EchoAgent, LocalRunner,
    PiAgent, PromptAgent,
};
use serde::{Deserialize, Serialize};
use tokio::sync::{broadcast, Mutex, OnceCell};
use tokio_stream::wrappers::BroadcastStream;

/// How often a running executor renews its run lease (`heartbeat_at`).
const HEARTBEAT_SECS: u64 = 30;
/// A run whose lease is older than this — i.e. no executor is heartbeating it —
/// is treated as orphaned and reaped. Comfortably larger than `HEARTBEAT_SECS`
/// so a live, actively-heartbeating run is never mistaken for stale.
const RECONCILE_STALE_SECS: u64 = 180;
/// How often the server sweeps for stale-lease runs (the periodic reaper).
const RECONCILE_EVERY_SECS: u64 = 60;
const ARTIFACT_MAX_BYTES: usize = 65_536;

/// Read a node's declared artifact (relative to the run's artifacts dir),
/// rejecting traversal/absolute paths and truncating on a char boundary.
/// `None` when undeclared, escaping, or not produced (graceful).
fn read_declared_artifact(artifacts_dir: &Path, rel: &str) -> Option<String> {
    if rel.is_empty() {
        return None;
    }
    let p = Path::new(rel);
    if p.components().any(|c| !matches!(c, Component::Normal(_))) {
        return None; // reject `..`, absolute, prefix
    }
    let path = artifacts_dir.join(p);
    let bytes = std::fs::metadata(&path).ok()?.len();
    if bytes <= ARTIFACT_MAX_BYTES as u64 {
        return std::fs::read_to_string(path).ok();
    }

    let mut file = File::open(path).ok()?;
    let mut buf = vec![0; ARTIFACT_MAX_BYTES];
    let mut read = 0;
    while read < buf.len() {
        let n = file.read(&mut buf[read..]).ok()?;
        if n == 0 {
            break;
        }
        read += n;
    }
    buf.truncate(read);

    match std::str::from_utf8(&buf) {
        Ok(content) => Some(format!("{content}\n[truncated: {bytes} bytes total]")),
        Err(e) if e.error_len().is_none() => {
            let b = e.valid_up_to();
            let content = std::str::from_utf8(&buf[..b]).ok()?;
            Some(format!("{content}\n[truncated: {bytes} bytes total]"))
        }
        Err(_) => None,
    }
}

/// Self-contained state for the runs API.
pub struct RunsState {
    db_url: Option<String>,
    store: OnceCell<RunStore>,
    agent_registry: Arc<AgentRegistry>,
    /// AES key for the credential store (from `HARNESS_SECRET_KEY`), if set.
    secret_key: Option<[u8; 32]>,
    cred_store: OnceCell<harness_persist::CredentialStore>,
    project_store: OnceCell<ProjectStore>,
    category_store: OnceCell<harness_persist::CategoryStore>,
    billing_store: OnceCell<harness_persist::BillingProfileStore>,
    linear_source_store: OnceCell<harness_persist::LinearSourceStore>,
    linear_claim_store: OnceCell<harness_persist::LinearClaimStore>,
    /// Where project repos are cloned (one checkout dir per project).
    pub(crate) projects_dir: PathBuf,
    /// The server's global project root. Custom workflows are global (like
    /// bundled): the editor authors them here, and runs/MCP resolve them here so
    /// they apply to every project.
    pub(crate) project_root: PathBuf,
    /// External base URL of this instance (`HARNESS_PUBLIC_URL` /
    /// `server.public_url`), trailing slash trimmed. `None` => run-link
    /// features no-op.
    pub(crate) public_url: Option<String>,
    /// This server instance's identity, stamped as the `owner` of every run it
    /// starts (lease attribution). Unique per process so a restart/replica is
    /// distinguishable.
    instance_id: String,
    /// Live runs → their event broadcast + task abort handle (present only while
    /// executing). Used by `/stream` to subscribe and by `cancel` to stop the task.
    live: Mutex<HashMap<String, LiveRun>>,
}

/// A currently-executing run's handles.
struct LiveRun {
    tx: broadcast::Sender<RunEvent>,
    abort: tokio::task::AbortHandle,
    /// The project this run belongs to (for per-project idle guards).
    project: String,
}

impl RunsState {
    pub fn new(
        db_url: Option<String>,
        agent_registry: Arc<AgentRegistry>,
        project_root: PathBuf,
        secret_key: Option<[u8; 32]>,
        public_url: Option<String>,
    ) -> Self {
        // Project checkouts live next to the default project root (sibling
        // `projects/` dir), overridable via HARNESS_PROJECTS_DIR.
        let projects_dir = std::env::var_os("HARNESS_PROJECTS_DIR")
            .map(PathBuf::from)
            .unwrap_or_else(|| {
                project_root
                    .parent()
                    .map(|p| p.join("projects"))
                    .unwrap_or_else(|| project_root.join("projects"))
            });
        let project_root_global = project_root.clone();
        let public_url = public_url
            .map(|u| u.trim().trim_end_matches('/').to_string())
            .filter(|u| !u.is_empty());
        // Identity for run-lease attribution: the pod name (k8s sets HOSTNAME)
        // plus a start stamp, so two pods — and a restart of the same pod — get
        // distinct owners.
        let host =
            std::env::var("HOSTNAME").unwrap_or_else(|_| format!("pid{}", std::process::id()));
        let instance_id = format!("{host}-{}", now_millis());
        Self {
            db_url,
            store: OnceCell::new(),
            agent_registry,
            secret_key,
            cred_store: OnceCell::new(),
            project_store: OnceCell::new(),
            category_store: OnceCell::new(),
            billing_store: OnceCell::new(),
            linear_source_store: OnceCell::new(),
            linear_claim_store: OnceCell::new(),
            projects_dir,
            project_root: project_root_global,
            public_url,
            instance_id,
            live: Mutex::new(HashMap::new()),
        }
    }

    /// Lazily connect the project registry store.
    pub(crate) async fn project_store(&self) -> Result<&ProjectStore, String> {
        let url = self
            .db_url
            .as_deref()
            .ok_or("no database configured (set server.database_url)")?;
        self.project_store
            .get_or_try_init(|| async {
                ProjectStore::connect(url).await.map_err(|e| e.to_string())
            })
            .await
    }

    /// Lazily connect the category registry store (seeds defaults on first use).
    pub(crate) async fn category_store(&self) -> Result<&harness_persist::CategoryStore, String> {
        let url = self
            .db_url
            .as_deref()
            .ok_or("no database configured (set server.database_url)")?;
        self.category_store
            .get_or_try_init(|| async {
                harness_persist::CategoryStore::connect(url)
                    .await
                    .map_err(|e| e.to_string())
            })
            .await
    }

    /// Lazily connect the billing-profile store.
    pub(crate) async fn billing_store(
        &self,
    ) -> Result<&harness_persist::BillingProfileStore, String> {
        let url = self
            .db_url
            .as_deref()
            .ok_or("no database configured (set server.database_url)")?;
        self.billing_store
            .get_or_try_init(|| async {
                harness_persist::BillingProfileStore::connect(url)
                    .await
                    .map_err(|e| e.to_string())
            })
            .await
    }

    /// Lazily connect the Linear source binding store.
    pub(crate) async fn linear_source_store(
        &self,
    ) -> Result<&harness_persist::LinearSourceStore, String> {
        let url = self
            .db_url
            .as_deref()
            .ok_or("no database configured (set server.database_url)")?;
        self.linear_source_store
            .get_or_try_init(|| async {
                harness_persist::LinearSourceStore::connect(url)
                    .await
                    .map_err(|e| e.to_string())
            })
            .await
    }

    /// Lazily connect the Linear claim-linkage store (live poller).
    pub(crate) async fn linear_claim_store(
        &self,
    ) -> Result<&harness_persist::LinearClaimStore, String> {
        let url = self
            .db_url
            .as_deref()
            .ok_or("no database configured (set server.database_url)")?;
        self.linear_claim_store
            .get_or_try_init(|| async {
                harness_persist::LinearClaimStore::connect(url)
                    .await
                    .map_err(|e| e.to_string())
            })
            .await
    }

    /// The global GitHub token from the credential store, if configured — used
    /// to clone/fetch private project repos. Best-effort: any error → `None`.
    pub(crate) async fn github_token(&self) -> Option<String> {
        let store = self.cred_store().await.ok()?;
        let fields = store.get("github").await.ok()??;
        fields.get("token").filter(|v| !v.is_empty()).cloned()
    }

    /// GitHub token for `project`: the project-scoped credential if set, else the
    /// global one. Lets a project that lives in a different GitHub account use
    /// its own token for clone/fetch.
    pub(crate) async fn github_token_for_project(&self, project: &str) -> Option<String> {
        let store = self.cred_store().await.ok()?;
        let fields = store.get_for_project(project, "github").await.ok()??;
        fields.get("token").filter(|v| !v.is_empty()).cloned()
    }

    /// Lazily connect (and migrate) the persistence store.
    pub(crate) async fn store(&self) -> Result<&RunStore, String> {
        let url = self
            .db_url
            .as_deref()
            .ok_or("no database configured (set server.database_url)")?;
        self.store
            .get_or_try_init(|| async {
                let store = RunStore::connect(url).await.map_err(|e| e.to_string())?;
                // First connect after (re)start: reap runs whose lease has gone
                // stale (no executor heartbeating them). Lease-scoped — never
                // touches a run another live instance is actively heartbeating.
                match store
                    .reconcile_orphaned_runs(std::time::Duration::from_secs(RECONCILE_STALE_SECS))
                    .await
                {
                    Ok(n) if n > 0 => {
                        tracing::info!("reconciled {n} orphaned run(s) → cancelled")
                    }
                    Ok(_) => {}
                    Err(e) => tracing::warn!("orphaned-run reconcile failed: {e}"),
                }
                Ok(store)
            })
            .await
    }

    /// Lazily connect the encrypted credential store. Requires both a database
    /// and `HARNESS_SECRET_KEY`.
    pub(crate) async fn cred_store(&self) -> Result<&harness_persist::CredentialStore, String> {
        let url = self
            .db_url
            .as_deref()
            .ok_or("no database configured (set server.database_url)")?;
        let key = self
            .secret_key
            .ok_or("credentials disabled (set HARNESS_SECRET_KEY)")?;
        self.cred_store
            .get_or_try_init(|| async {
                harness_persist::CredentialStore::connect(url, key)
                    .await
                    .map_err(|e| e.to_string())
            })
            .await
    }

    /// True if this instance currently has an in-memory live run for `project`.
    pub(crate) async fn has_live_run_for_project(&self, project: &str) -> bool {
        self.live
            .lock()
            .await
            .values()
            .any(|r| r.project == project)
    }
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
pub struct ModelRef {
    /// Agent provider (e.g. `pi`, `claude`, `cursor`).
    pub provider: String,
    /// Model id, as written in the workflow (e.g. `kimi-code/kimi-for-coding`).
    pub model: String,
}

impl ModelRef {
    /// `"provider/model"` — the display label used for an A/B arm.
    fn label(&self) -> String {
        format!("{}/{}", self.provider, self.model)
    }
}

#[derive(Debug, Deserialize)]
pub struct CreateRunRequest {
    /// Workflow path or bundled/project name.
    pub workflow: String,
    /// Human task title — names the run; exposed to nodes as `$TASK_TITLE`.
    #[serde(default)]
    pub title: Option<String>,
    /// The task spec — the substantive input. Fed to nodes as `$ARGUMENTS` /
    /// `$USER_MESSAGE` / `$TASK_DESCRIPTION`. May be long.
    #[serde(default)]
    pub description: String,
    /// Deprecated alias for `description` (back-compat).
    #[serde(default)]
    pub args: String,
    #[serde(default)]
    pub real: bool,
    #[serde(default)]
    pub base_branch: Option<String>,
    /// Project to run within (**required**). Its repo checkout is the workspace;
    /// a per-run worktree is cut off its `base_branch`. Missing → 400.
    #[serde(default)]
    pub project: Option<String>,
    /// A/B model substitution: replace every node — plus the workflow default and
    /// loop bodies — whose `(provider, model)` equals `swap_from` with `swap_to`,
    /// leaving every other step untouched. Both must be set to take effect.
    #[serde(default)]
    pub swap_from: Option<ModelRef>,
    #[serde(default)]
    pub swap_to: Option<ModelRef>,
    /// A/B pairing stamp, set by the paired-trigger endpoint; absent for a normal
    /// run. `ab_arm` is `"a"`/`"b"`, `ab_label` names the arm's substituted model.
    #[serde(default)]
    pub ab_pair_id: Option<String>,
    #[serde(default)]
    pub ab_arm: Option<String>,
    #[serde(default)]
    pub ab_label: Option<String>,
}

/// Owned A/B pairing info carried from trigger → `execute_run_task` → persistence.
#[derive(Debug, Clone)]
struct AbInfo {
    pair_id: String,
    arm: String,
    label: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct ListRunsQuery {
    #[serde(default)]
    pub project: Option<String>,
    #[serde(default)]
    pub unassigned: bool,
}

#[derive(Debug, Deserialize)]
pub struct SummaryQuery {
    /// Trailing window in days (default 14, clamped 1..=90).
    #[serde(default)]
    pub days: Option<i64>,
}

fn summary_window_start(now: DateTime<Utc>, days: i64) -> DateTime<Utc> {
    let days = days.clamp(1, 90);
    let today_start = now.date_naive().and_time(chrono::NaiveTime::MIN).and_utc();
    today_start - Duration::days(days - 1)
}

/// Replace every place a workflow pins `from`'s `(provider, model)` with `to`:
/// the workflow-level default, each node's pin, and each loop body's pin. This is
/// the A/B substitution primitive — it swaps one integration model for another
/// wherever it appears while leaving every other step (e.g. specialist review
/// nodes pinned to a different model) untouched.
fn apply_model_swap(workflow: &mut harness_dag::Workflow, from: &ModelRef, to: &ModelRef) {
    let hits = |provider: &Option<String>, model: &Option<String>| {
        provider.as_deref() == Some(from.provider.as_str())
            && model.as_deref() == Some(from.model.as_str())
    };
    if hits(&workflow.provider, &workflow.model) {
        workflow.provider = Some(to.provider.clone());
        workflow.model = Some(to.model.clone());
    }
    for node in &mut workflow.nodes {
        if hits(&node.provider, &node.model) {
            node.provider = Some(to.provider.clone());
            node.model = Some(to.model.clone());
        }
        if let harness_dag::NodeKind::Loop(cfg) = &mut node.kind {
            if hits(&cfg.provider, &cfg.model) {
                cfg.provider = Some(to.provider.clone());
                cfg.model = Some(to.model.clone());
            }
        }
    }
}

/// The distinct `(provider, model)` pairs a workflow uses — across the
/// workflow-level default, node pins, and loop-body pins. Drives the A/B UI:
/// "which step do you want to swap out?" A pair is only included when both
/// provider and model are concretely set (an unresolved half can't be swapped).
fn workflow_model_pairs(workflow: &harness_dag::Workflow) -> Vec<ModelRef> {
    let mut pairs: Vec<ModelRef> = Vec::new();
    let mut push = |provider: &Option<String>, model: &Option<String>| {
        if let (Some(p), Some(m)) = (provider.as_deref(), model.as_deref()) {
            let r = ModelRef {
                provider: p.to_string(),
                model: m.to_string(),
            };
            if !pairs.contains(&r) {
                pairs.push(r);
            }
        }
    };
    push(&workflow.provider, &workflow.model);
    for node in &workflow.nodes {
        push(&node.provider, &node.model);
        if let harness_dag::NodeKind::Loop(cfg) = &node.kind {
            push(&cfg.provider, &cfg.model);
        }
    }
    pairs
}

#[derive(Debug, Serialize)]
pub struct CreateRunResponse {
    pub run_id: String,
}

fn err(status: StatusCode, msg: impl Into<String>) -> Response {
    (status, Json(serde_json::json!({ "error": msg.into() }))).into_response()
}

fn now_millis() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis())
        .unwrap_or(0)
}

/// Spawn the periodic stale-lease reaper: every `RECONCILE_EVERY_SECS` it cancels
/// runs whose lease is older than `RECONCILE_STALE_SECS` (no executor renewing
/// them) — so a run orphaned by a task panic or a lost pod doesn't linger as
/// `running` forever. Best-effort; a no-op when called outside a Tokio runtime
/// (e.g. a synchronous router-build test).
pub(crate) fn spawn_reaper(state: Arc<RunsState>) {
    if tokio::runtime::Handle::try_current().is_err() {
        return;
    }
    tokio::spawn(async move {
        let mut tick = tokio::time::interval(std::time::Duration::from_secs(RECONCILE_EVERY_SECS));
        tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        loop {
            tick.tick().await;
            if let Ok(store) = state.store().await {
                match store
                    .reconcile_orphaned_runs(std::time::Duration::from_secs(RECONCILE_STALE_SECS))
                    .await
                {
                    Ok(n) if n > 0 => {
                        tracing::info!("reaper: cancelled {n} stale-lease run(s)")
                    }
                    Ok(_) => {}
                    Err(e) => tracing::warn!("reaper: reconcile failed: {e}"),
                }
            }
        }
    });
}

/// How often the cargo-cache sweeper checks the shared build cache.
pub(crate) const CACHE_SWEEP_EVERY_SECS: u64 = 6 * 60 * 60;
/// Don't evict artifacts modified within this window — protects an in-progress
/// build's fresh outputs (even one driven by another replica) from deletion.
pub(crate) const CACHE_SWEEP_SAFETY_FLOOR_SECS: u64 = 2 * 60 * 60;
/// Default size cap **per project** cache dir (each immediate subdirectory of
/// `.cargo-target/`), in GiB. Override with `HARNESS_CARGO_TARGET_CAP_GB`; `0`
/// disables the sweeper. Per-project, so total disk scales with the number of
/// projects but no single project's cache can balloon.
pub(crate) const CACHE_CAP_GB_DEFAULT: u64 = 50;
/// Total size in bytes of all regular files under `root` (recursive, skipping
/// symlinks). Returns 0 if `root` is absent/unreadable. Best-effort.
pub(crate) fn dir_size(root: &std::path::Path) -> u64 {
    let mut total: u64 = 0;
    let mut stack = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        let Ok(rd) = std::fs::read_dir(&dir) else {
            continue;
        };
        for entry in rd.flatten() {
            let Ok(ft) = entry.file_type() else { continue };
            if ft.is_symlink() {
                continue;
            }
            if ft.is_dir() {
                stack.push(entry.path());
            } else if ft.is_file() {
                if let Ok(meta) = entry.metadata() {
                    total = total.saturating_add(meta.len());
                }
            }
        }
    }
    total
}

/// Resolve a project's effective cache cap (GiB): per-project setting (when
/// > 0) → `HARNESS_CARGO_TARGET_CAP_GB` env → [`CACHE_CAP_GB_DEFAULT`].
pub(crate) fn resolve_cap_gb(project_cap: Option<i32>) -> u64 {
    resolve_cap_gb_with_env(
        project_cap,
        std::env::var("HARNESS_CARGO_TARGET_CAP_GB").ok().as_deref(),
    )
}

fn resolve_cap_gb_with_env(project_cap: Option<i32>, env_cap: Option<&str>) -> u64 {
    if let Some(v) = project_cap.filter(|&v| v > 0) {
        return v as u64;
    }
    match env_cap.and_then(|v| v.trim().parse::<u64>().ok()) {
        Some(v) => v,
        None => CACHE_CAP_GB_DEFAULT,
    }
}

/// Periodically bound each project's cargo build cache so none can fill the disk.
///
/// **Per-project + size-gated**: every immediate subdirectory of `.cargo-target/`
/// (one per project) is capped independently. A project's cache under its cap is
/// never touched, so an idle project stays fully warm; pruning only kicks in for
/// a project that grows past the cap, evicting *that project's* oldest artifacts
/// first down to 80% of the cap. Projects never evict each other. Skips files
/// modified within [`CACHE_SWEEP_SAFETY_FLOOR_SECS`] and skips entirely while a
/// run is live on this instance — so it never deletes a build's in-flight
/// outputs. Deleting a stale artifact only forces a rebuild; it can't corrupt
/// the cache. Best-effort; a no-op outside a Tokio runtime.
pub(crate) fn spawn_cache_sweeper(state: Arc<RunsState>) {
    if tokio::runtime::Handle::try_current().is_err() {
        return;
    }
    let env_cap_gb = std::env::var("HARNESS_CARGO_TARGET_CAP_GB")
        .ok()
        .and_then(|v| v.trim().parse::<u64>().ok())
        .unwrap_or(CACHE_CAP_GB_DEFAULT);
    if env_cap_gb == 0 {
        tracing::info!("cache sweeper: disabled (HARNESS_CARGO_TARGET_CAP_GB=0)");
        return;
    }
    let root = state.projects_dir.join(".cargo-target");
    tokio::spawn(async move {
        let mut tick =
            tokio::time::interval(std::time::Duration::from_secs(CACHE_SWEEP_EVERY_SECS));
        tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        loop {
            tick.tick().await;
            // Cheap early-out: never sweep while this instance has a live run.
            if !state.live.lock().await.is_empty() {
                continue;
            }
            let caps: std::collections::HashMap<String, i32> = match state.project_store().await {
                Ok(store) => match store.list().await {
                    Ok(ps) => ps
                        .into_iter()
                        .filter_map(|p| p.cargo_target_cap_gb.map(|c| (p.name, c)))
                        .collect(),
                    Err(e) => {
                        tracing::warn!("cache sweeper: list projects: {e}");
                        Default::default()
                    }
                },
                Err(_) => Default::default(),
            };
            let root = root.clone();
            let caps = caps.clone();
            if let Err(e) = tokio::task::spawn_blocking(move || {
                sweep_project_caches(&root, CACHE_SWEEP_SAFETY_FLOOR_SECS, &caps)
            })
            .await
            {
                tracing::warn!("cache sweeper: join error: {e}");
            }
        }
    });
}

/// Sweep each project cache dir (an immediate subdirectory of `root`)
fn sweep_project_caches(
    root: &std::path::Path,
    floor_secs: u64,
    caps: &std::collections::HashMap<String, i32>,
) {
    let rd = match std::fs::read_dir(root) {
        Ok(r) => r,
        Err(_) => return, // no cache root yet
    };
    for entry in rd.flatten() {
        if !entry.file_type().map(|t| t.is_dir()).unwrap_or(false) {
            continue;
        }
        let name = entry.file_name().to_string_lossy().into_owned();
        let cap_gb = resolve_cap_gb(caps.get(&name).copied());
        if cap_gb == 0 {
            continue;
        }
        let cap = cap_gb.saturating_mul(1024 * 1024 * 1024);
        let target = cap / 5 * 4; // 80% — hysteresis so we don't churn at the edge
        match sweep_cargo_cache(&entry.path(), cap, target, floor_secs) {
            Ok(Some((before, after))) => tracing::info!(
                "cache sweeper: {name} {:.1} GB -> {:.1} GB (cap {cap_gb} GB/project)",
                before as f64 / 1.073_741_824e9,
                after as f64 / 1.073_741_824e9,
            ),
            Ok(None) => {}
            Err(e) => tracing::warn!("cache sweeper: {name}: {e}"),
        }
    }
}

/// Evict oldest files under `root` until total size ≤ `target`, but only when it
/// currently exceeds `cap`, and never touching files modified within
/// `floor_secs`. Returns `(before, after)` bytes when it acted, else `None`.
pub(crate) fn sweep_cargo_cache(
    root: &std::path::Path,
    cap: u64,
    target: u64,
    floor_secs: u64,
) -> std::io::Result<Option<(u64, u64)>> {
    if !root.exists() {
        return Ok(None);
    }
    let now = std::time::SystemTime::now();
    let floor = std::time::Duration::from_secs(floor_secs);
    let mut files: Vec<(std::path::PathBuf, u64, std::time::SystemTime)> = Vec::new();
    let mut total: u64 = 0;
    let mut stack = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        let rd = match std::fs::read_dir(&dir) {
            Ok(r) => r,
            Err(_) => continue,
        };
        for entry in rd.flatten() {
            let ft = match entry.file_type() {
                Ok(t) => t,
                Err(_) => continue,
            };
            if ft.is_symlink() {
                continue;
            }
            if ft.is_dir() {
                stack.push(entry.path());
            } else if ft.is_file() {
                if let Ok(meta) = entry.metadata() {
                    total = total.saturating_add(meta.len());
                    let mtime = meta.modified().unwrap_or(std::time::UNIX_EPOCH);
                    files.push((entry.path(), meta.len(), mtime));
                }
            }
        }
    }
    if total <= cap {
        return Ok(None);
    }
    files.sort_by_key(|(_, _, mtime)| *mtime); // oldest first
    let mut remaining = total;
    for (path, size, mtime) in files {
        if remaining <= target {
            break;
        }
        // Protect fresh artifacts — a (possibly cross-replica) active build.
        if now
            .duration_since(mtime)
            .map(|age| age < floor)
            .unwrap_or(false)
        {
            continue;
        }
        if std::fs::remove_file(&path).is_ok() {
            remaining = remaining.saturating_sub(size);
        }
    }
    Ok(Some((total, remaining)))
}

/// `GET /runs` — list recent runs, optionally filtered by project.
pub async fn list_runs(
    Extension(state): Extension<Arc<RunsState>>,
    Query(q): Query<ListRunsQuery>,
) -> Response {
    let store = match state.store().await {
        Ok(s) => s,
        Err(e) => return err(StatusCode::SERVICE_UNAVAILABLE, e),
    };
    let project = q
        .project
        .as_deref()
        .map(str::trim)
        .filter(|p| !p.is_empty());
    let result = if q.unassigned {
        store.list_unassigned_runs(100).await
    } else if let Some(project) = project {
        store.list_runs_for_project(project, 100).await
    } else {
        store.list_runs(100).await
    };
    match result {
        Ok(runs) => Json(runs).into_response(),
        Err(e) => err(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()),
    }
}

/// `GET /runs/summary?days=N` — per-project, per-day finished-run counts.
pub async fn runs_daily_summary(
    Extension(state): Extension<Arc<RunsState>>,
    Query(q): Query<SummaryQuery>,
) -> Response {
    let days = q.days.unwrap_or(14).clamp(1, 90);
    let since = summary_window_start(Utc::now(), days);
    let store = match state.store().await {
        Ok(s) => s,
        Err(e) => return err(StatusCode::SERVICE_UNAVAILABLE, e),
    };
    match store.runs_daily_summary(since).await {
        Ok(rows) => Json(rows).into_response(),
        Err(e) => err(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()),
    }
}

/// `GET /runs/{id}` — run detail + node rows.
pub async fn get_run(
    Extension(state): Extension<Arc<RunsState>>,
    AxumPath(id): AxumPath<String>,
) -> Response {
    let store = match state.store().await {
        Ok(s) => s,
        Err(e) => return err(StatusCode::SERVICE_UNAVAILABLE, e),
    };
    match store.get_run(&id).await {
        Ok(Some(detail)) => Json(detail).into_response(),
        Ok(None) => err(StatusCode::NOT_FOUND, format!("run `{id}` not found")),
        Err(e) => err(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()),
    }
}

/// `GET /runs/pair/{pair_id}` — both arms of an A/B pair, with full per-node
/// detail, for the side-by-side comparison view. Arms come back ordered a → b.
pub async fn get_run_pair(
    Extension(state): Extension<Arc<RunsState>>,
    AxumPath(pair_id): AxumPath<String>,
) -> Response {
    let store = match state.store().await {
        Ok(s) => s,
        Err(e) => return err(StatusCode::SERVICE_UNAVAILABLE, e),
    };
    let summaries = match store.list_runs_for_pair(&pair_id).await {
        Ok(s) => s,
        Err(e) => return err(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()),
    };
    if summaries.is_empty() {
        return err(
            StatusCode::NOT_FOUND,
            format!("no runs for pair `{pair_id}`"),
        );
    }
    // Hydrate each arm with its node rows + graph (the comparison needs per-node
    // tokens/time/cost). Two short reads — a pair is two runs.
    let mut runs = Vec::with_capacity(summaries.len());
    for s in &summaries {
        match store.get_run(&s.id).await {
            Ok(Some(detail)) => runs.push(detail),
            Ok(None) => {}
            Err(e) => return err(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()),
        }
    }
    // If this pair has been judged, surface the judge run's status + parsed
    // verdict so the comparison can show it inline (null verdict while running).
    let judge = match store.get_pair_judge(&pair_id).await {
        Ok(Some(judge_run_id)) => {
            let (status, verdict) = match store.get_run(&judge_run_id).await {
                Ok(Some(jd)) => (
                    jd.run.status.clone(),
                    jd.nodes
                        .iter()
                        .find(|n| n.node_id == "judge")
                        .and_then(|n| serde_json::from_str::<serde_json::Value>(&n.output).ok()),
                ),
                _ => ("unknown".to_string(), None),
            };
            Some(serde_json::json!({
                "run_id": judge_run_id,
                "status": status,
                "verdict": verdict,
            }))
        }
        _ => None,
    };
    Json(serde_json::json!({ "pair_id": pair_id, "runs": runs, "judge": judge })).into_response()
}

/// Find a GitHub pull-request URL in a run's node outputs, preferring later
/// nodes (the finalize/summary step that opens the PR). Pure text scan — the
/// harness never shells out to `gh`/`git` itself (that lives in agent prompts);
/// the judge agent fetches the diff from this URL.
fn extract_pr_url(detail: &harness_persist::RunDetail) -> Option<String> {
    detail
        .nodes
        .iter()
        .rev()
        .find_map(|n| find_pr_url_in(&n.output))
}

/// First GitHub PR URL in a blob of text, with surrounding punctuation trimmed.
fn find_pr_url_in(text: &str) -> Option<String> {
    text.split(|c: char| {
        c.is_whitespace() || matches!(c, '(' | ')' | '"' | '\'' | '<' | '>' | ',' | '`')
    })
    .find(|raw| raw.contains("github.com") && raw.contains("/pull/"))
    .map(|raw| {
        raw.trim_end_matches(|c: char| matches!(c, '.' | ';' | ':' | ']'))
            .to_string()
    })
}

/// Build the evidence packet a judge run reasons over: the shared task, then for
/// each arm its model, final status, per-step outcomes, PR link, and final
/// output. The judge uses the PR link to read the actual diff.
fn assemble_ab_evidence(runs: &[harness_persist::RunDetail]) -> String {
    let mut s = String::new();
    if let Some(desc) = runs.first().and_then(|r| r.run.description.as_deref()) {
        s.push_str("# Task (identical for both arms)\n");
        s.push_str(desc.trim());
        s.push_str("\n\n");
    }
    for r in runs {
        let arm = r.run.ab_arm.as_deref().unwrap_or("?").to_uppercase();
        let label = r.run.ab_label.as_deref().unwrap_or("(unknown model)");
        s.push_str(&format!("# Arm {arm} — model: {label}\n"));
        s.push_str(&format!("Final status: {}\n", r.run.status));
        match extract_pr_url(r) {
            Some(pr) => s.push_str(&format!("PR: {pr}\n")),
            None => s.push_str("PR: (none — arm produced no pull request)\n"),
        }
        // Include the plan the arm was built against (the `create-plan`
        // artifact) so the judge can score how completely/correctly the
        // implementer fulfilled it — the fair test of the swapped step,
        // separate from absolute quality. Truncated to bound context.
        if let Some(plan) = r
            .nodes
            .iter()
            .find(|n| n.node_id == "create-plan")
            .and_then(|n| n.artifact_content.as_deref())
            .map(str::trim)
            .filter(|p| !p.is_empty())
        {
            let truncated: String = plan.chars().take(4000).collect();
            s.push_str(&format!("\nPlan it was built against:\n{truncated}\n"));
        }
        // Include each step's provider/model so the judge can see which steps
        // (e.g. the late `gpt-review-fix` / `sonnet-final-review` reviewers) ran
        // on which model — needed to weigh how much the shared reviewers shaped
        // each arm versus the swapped implementer.
        s.push_str("\nSteps (id [status] — provider/model):\n");
        for n in &r.nodes {
            let model = match (n.provider.as_deref(), n.model.as_deref()) {
                (Some(p), Some(m)) => format!(" — {p}/{m}"),
                (Some(p), None) => format!(" — {p}"),
                (None, Some(m)) => format!(" — {m}"),
                (None, None) => String::new(),
            };
            s.push_str(&format!("- {} [{}]{}\n", n.node_id, n.status, model));
        }
        // The final node is usually the summary describing the result. Truncate
        // so a long output can't blow the judge's context.
        if let Some(last) = r.nodes.last() {
            let out = last.output.trim();
            if !out.is_empty() {
                let truncated: String = out.chars().take(4000).collect();
                s.push_str(&format!(
                    "\nFinal output ({}):\n{}\n",
                    last.node_id, truncated
                ));
            }
        }
        s.push('\n');
    }
    s
}

/// `POST /runs/pair/{pair_id}/judge` — score both arms with the `judge-ab`
/// workflow. Optional `judge_model` overrides the workflow's default judge.
#[derive(Debug, Deserialize)]
pub struct JudgePairRequest {
    #[serde(default)]
    pub judge_model: Option<ModelRef>,
}

#[derive(Debug, Serialize)]
pub struct JudgePairResponse {
    pub judge_run_id: String,
}

pub async fn judge_run_pair(
    Extension(state): Extension<Arc<RunsState>>,
    AxumPath(pair_id): AxumPath<String>,
    Json(req): Json<JudgePairRequest>,
) -> Response {
    let store = match state.store().await {
        Ok(s) => s,
        Err(e) => return err(StatusCode::SERVICE_UNAVAILABLE, e),
    };
    let summaries = match store.list_runs_for_pair(&pair_id).await {
        Ok(s) => s,
        Err(e) => return err(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()),
    };
    if summaries.len() < 2 {
        return err(
            StatusCode::BAD_REQUEST,
            format!("pair `{pair_id}` needs two arms to judge"),
        );
    }
    let mut runs = Vec::with_capacity(summaries.len());
    for s in &summaries {
        if let Ok(Some(detail)) = store.get_run(&s.id).await {
            runs.push(detail);
        }
    }
    let project = summaries[0].project.clone();
    let evidence = assemble_ab_evidence(&runs);
    // The judge is the workflow's default model unless overridden. We retarget it
    // by swapping the default pair → the chosen model (a no-op when equal).
    let default_pair = resolve_workflow_models(&state, "judge-ab", project.as_deref())
        .ok()
        .and_then(|v| v.into_iter().next());
    let swap_to = req.judge_model.or_else(|| default_pair.clone());
    let title = format!(
        "Judge: {}",
        summaries[0].title.as_deref().unwrap_or(pair_id.as_str())
    );
    let run_req = CreateRunRequest {
        workflow: "judge-ab".to_string(),
        title: Some(title),
        description: evidence,
        args: String::new(),
        real: true,
        base_branch: None,
        project,
        swap_from: default_pair,
        swap_to,
        ab_pair_id: None,
        ab_arm: None,
        ab_label: None,
    };
    match start_run(&state, run_req).await {
        Ok(judge_run_id) => {
            if let Err(e) = store.set_pair_judge(&pair_id, &judge_run_id).await {
                return err(StatusCode::INTERNAL_SERVER_ERROR, e.to_string());
            }
            (
                StatusCode::ACCEPTED,
                Json(JudgePairResponse { judge_run_id }),
            )
                .into_response()
        }
        Err((status, msg)) => err(status, msg),
    }
}

/// `POST /runs` — submit a workflow; execute it in a background task.
pub async fn create_run(
    Extension(state): Extension<Arc<RunsState>>,
    Json(req): Json<CreateRunRequest>,
) -> Response {
    match start_run(&state, req).await {
        Ok(run_id) => (StatusCode::ACCEPTED, Json(CreateRunResponse { run_id })).into_response(),
        Err((status, msg)) => err(status, msg),
    }
}

/// Trigger an A/B pair: two runs of the same task that differ only by which model
/// the `swap_from` steps use. Arm A applies `swap_from → variant_a`, arm B applies
/// `swap_from → variant_b`; both share an `ab_pair_id` so the comparison view can
/// pull them together. Picking `variant_a == swap_from` makes arm A the unchanged
/// baseline.
#[derive(Debug, Deserialize)]
pub struct CreateRunPairRequest {
    pub workflow: String,
    #[serde(default)]
    pub title: Option<String>,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub args: String,
    #[serde(default)]
    pub real: bool,
    #[serde(default)]
    pub base_branch: Option<String>,
    #[serde(default)]
    pub project: Option<String>,
    /// The step(s) under test, named by the `(provider, model)` they currently use.
    pub swap_from: ModelRef,
    /// Arm A's model for those steps (often equal to `swap_from` — the baseline).
    pub variant_a: ModelRef,
    /// Arm B's model for those steps — the challenger.
    pub variant_b: ModelRef,
}

#[derive(Debug, Serialize)]
pub struct CreateRunPairResponse {
    pub pair_id: String,
    pub run_id_a: String,
    pub run_id_b: String,
}

/// `POST /runs/pair` — start both arms of an A/B comparison.
pub async fn create_run_pair(
    Extension(state): Extension<Arc<RunsState>>,
    Json(req): Json<CreateRunPairRequest>,
) -> Response {
    match start_run_pair(&state, req).await {
        Ok(resp) => (StatusCode::ACCEPTED, Json(resp)).into_response(),
        Err((status, msg)) => err(status, msg),
    }
}

/// Start both arms of an A/B pair — the shared core behind `POST /api/runs/pair`
/// and the MCP `run_trigger_pair` tool. Both arms share a generated `pair_id`.
pub(crate) async fn start_run_pair(
    state: &Arc<RunsState>,
    req: CreateRunPairRequest,
) -> Result<CreateRunPairResponse, (StatusCode, String)> {
    let pair_id = format!("ab-{}", now_millis());
    // One arm = the base request with the swap and pairing stamp filled in.
    let arm = |arm: &str, variant: &ModelRef| CreateRunRequest {
        workflow: req.workflow.clone(),
        title: req.title.clone(),
        description: req.description.clone(),
        args: req.args.clone(),
        real: req.real,
        base_branch: req.base_branch.clone(),
        project: req.project.clone(),
        swap_from: Some(req.swap_from.clone()),
        swap_to: Some(variant.clone()),
        ab_pair_id: Some(pair_id.clone()),
        ab_arm: Some(arm.to_string()),
        ab_label: Some(variant.label()),
    };
    let run_id_a = start_run(state, arm("a", &req.variant_a)).await?;
    let run_id_b = start_run(state, arm("b", &req.variant_b)).await?;
    Ok(CreateRunPairResponse {
        pair_id,
        run_id_a,
        run_id_b,
    })
}

/// `GET /runs/workflow-models?workflow=NAME&project=P` — the distinct
/// `(provider, model)` pairs a workflow uses, so the A/B UI can offer them as
/// swap targets. Resolves the workflow project-first, exactly like a run trigger.
#[derive(Debug, Deserialize)]
pub struct WorkflowModelsQuery {
    pub workflow: String,
    #[serde(default)]
    pub project: Option<String>,
}

pub async fn list_workflow_models(
    Extension(state): Extension<Arc<RunsState>>,
    Query(q): Query<WorkflowModelsQuery>,
) -> Response {
    match resolve_workflow_models(&state, &q.workflow, q.project.as_deref()) {
        Ok(pairs) => Json(pairs).into_response(),
        Err((status, msg)) => err(status, msg),
    }
}

/// Resolve a workflow (project-first, same precedence as `start_run`) and return
/// the distinct `(provider, model)` pairs it uses. Shared by `GET
/// /api/runs/workflow-models` and the MCP `workflow_models` tool.
pub(crate) fn resolve_workflow_models(
    state: &Arc<RunsState>,
    workflow: &str,
    project: Option<&str>,
) -> Result<Vec<ModelRef>, (StatusCode, String)> {
    let project = project.map(str::trim).filter(|p| !p.is_empty());
    let ships_per_project = project.is_some_and(|p| {
        state
            .projects_dir
            .join(p)
            .join(".harness")
            .join("workflows")
            .join(format!("{}.yaml", workflow.trim()))
            .is_file()
    });
    let workflow_root = match (ships_per_project, project) {
        (true, Some(p)) => state.projects_dir.join(p),
        _ => state.project_root.clone(),
    };
    let (yaml, _label) = harness_runner::resolve_workflow_source(workflow, &workflow_root)
        .map_err(|e| (StatusCode::BAD_REQUEST, e))?;
    let workflow = parse_workflow(&yaml)
        .map_err(|e| (StatusCode::BAD_REQUEST, format!("invalid workflow: {e}")))?;
    Ok(workflow_model_pairs(&workflow))
}

/// Validate, resolve, and spawn a run in the background — the shared core behind
/// both `POST /api/runs` and the MCP `run_trigger` tool. Returns the `run_id` or
/// a `(status, message)` pair the caller renders however it likes.
pub(crate) async fn start_run(
    state: &Arc<RunsState>,
    req: CreateRunRequest,
) -> Result<String, (StatusCode, String)> {
    // A run must live in a project — the project's checkout is the workspace
    // (there is no global project root). Reject a missing/unknown project.
    let project = match req
        .project
        .as_deref()
        .map(str::trim)
        .filter(|p| !p.is_empty())
    {
        Some(p) => p.to_string(),
        None => return Err((StatusCode::BAD_REQUEST, "project is required".to_string())),
    };
    let store = state
        .project_store()
        .await
        .map_err(|e| (StatusCode::SERVICE_UNAVAILABLE, e))?;
    let project_row = match store.get(&project).await {
        Ok(Some(p)) => p,
        Ok(None) => {
            return Err((
                StatusCode::BAD_REQUEST,
                format!("unknown project `{project}`"),
            ))
        }
        Err(e) => return Err((StatusCode::INTERNAL_SERVER_ERROR, e.to_string())),
    };

    // Empty `workflow` → the project's default; base branch → request or project.
    let workflow_name = {
        let w = req.workflow.trim();
        if w.is_empty() {
            project_row.default_workflow.clone().unwrap_or_default()
        } else {
            w.to_string()
        }
    };
    let base_branch = req
        .base_branch
        .as_deref()
        .map(str::trim)
        .filter(|b| !b.is_empty())
        .map(str::to_string)
        .unwrap_or(project_row.base_branch);

    // Resolve `workflow`: a workflow shipped in the project's own checkout wins,
    // else the global custom workflows (where the editor authors them), else a
    // bundled default. Custom workflows are global — they apply to every project.
    let per_project_root = state.projects_dir.join(&project);
    let name = workflow_name.trim();
    let ships_per_project = !name.is_empty()
        && per_project_root
            .join(".harness")
            .join("workflows")
            .join(format!("{name}.yaml"))
            .is_file();
    let workflow_root = if ships_per_project {
        per_project_root
    } else {
        state.project_root.clone()
    };
    let (yaml, _label) = harness_runner::resolve_workflow_source(&workflow_name, &workflow_root)
        .map_err(|e| (StatusCode::BAD_REQUEST, e))?;
    let mut workflow = parse_workflow(&yaml)
        .map_err(|e| (StatusCode::BAD_REQUEST, format!("invalid workflow: {e}")))?;

    // A/B substitution: swap one integration model for another wherever the
    // workflow pins it, leaving every other step constant. Both halves required.
    if let (Some(from), Some(to)) = (req.swap_from.as_ref(), req.swap_to.as_ref()) {
        apply_model_swap(&mut workflow, from, to);
    }

    // A/B pairing stamp (set by the paired-trigger endpoint; None for normal runs).
    let ab = match (req.ab_pair_id, req.ab_arm) {
        (Some(pair_id), Some(arm)) => Some(AbInfo {
            pair_id,
            arm,
            label: req.ab_label,
        }),
        _ => None,
    };

    let run_id = format!(
        "{}-{}",
        sanitize_branch_component(&workflow.name),
        now_millis()
    );

    // Register a broadcast channel so /stream subscribers see live events.
    let (btx, _) = broadcast::channel::<RunEvent>(256);
    let live_tx = btx.clone();

    let task_state = state.clone();
    let task_run_id = run_id.clone();
    let toolchains = project_row.toolchains.clone();
    let real = req.real;
    // `description` is the task spec; fall back to the deprecated `args` alias.
    let description = if req.description.is_empty() {
        req.args
    } else {
        req.description
    };
    let title = req.title.filter(|t| !t.trim().is_empty());
    let live_project = project.clone();
    let handle = tokio::spawn(async move {
        execute_run_task(
            task_state,
            task_run_id,
            workflow,
            real,
            title,
            description,
            base_branch,
            project,
            toolchains,
            ab,
            btx,
        )
        .await;
    });
    state.live.lock().await.insert(
        run_id.clone(),
        LiveRun {
            tx: live_tx,
            abort: handle.abort_handle(),
            project: live_project,
        },
    );

    Ok(run_id)
}

#[allow(clippy::too_many_arguments)]
async fn execute_run_task(
    state: Arc<RunsState>,
    run_id: String,
    workflow: harness_dag::Workflow,
    real: bool,
    title: Option<String>,
    description: String,
    base_branch: String,
    project: String,
    toolchains: Vec<String>,
    ab: Option<AbInfo>,
    btx: broadcast::Sender<RunEvent>,
) {
    // The workspace is an isolated per-run worktree off the project checkout's
    // `origin/<base_branch>` (so concurrent runs in the same project don't
    // collide). `_worktree` is held for the run's lifetime and removed on drop.
    // A setup failure fails the run visibly rather than running anywhere else.
    let (workspace, _worktree) = match resolve_workspace(&state, &project, &run_id, &base_branch)
        .await
    {
        Ok(v) => v,
        Err(e) => {
            tracing::error!(run_id = %run_id, project = %project, "workspace setup failed: {e}");
            if let Ok(store) = state.store().await {
                let ab_ref = ab.as_ref().map(|a| harness_persist::AbPairing {
                    pair_id: &a.pair_id,
                    arm: &a.arm,
                    label: a.label.as_deref(),
                });
                let _ = store
                    .start_run(
                        &run_id,
                        &workflow.name,
                        title.as_deref(),
                        Some(description.as_str()),
                        Some(&project),
                        0,
                        &[],
                        Some(&state.instance_id),
                        ab_ref.as_ref(),
                    )
                    .await;
                let _ = store
                    .finish_run(&run_id, harness_dag::RunStatus::Failed)
                    .await;
            }
            let _ = btx.send(RunEvent::RunFinished {
                status: harness_dag::RunStatus::Failed,
            });
            state.live.lock().await.remove(&run_id);
            return;
        }
    };
    let artifacts = workspace.join(".harness").join("artifacts");
    let _ = std::fs::create_dir_all(&artifacts);
    let command_dirs = vec![workspace.join(".harness").join("commands")];

    // Warm Rust build cache: point CARGO_TARGET_DIR at a per-project dir on the
    // persistent volume (NOT the per-run worktree, whose `target/` is cold every
    // run). Cargo invoked by any node (bash `install-deps`, agent verify chains)
    // inherits this, so the first run compiles and later runs reuse artifacts.
    let cargo_target = state.projects_dir.join(".cargo-target").join(&project);
    let _ = std::fs::create_dir_all(&cargo_target);
    let mut run_env = std::collections::HashMap::from([
        (
            "CARGO_TARGET_DIR".to_string(),
            cargo_target.display().to_string(),
        ),
        ("CARGO_INCREMENTAL".to_string(), "0".to_string()),
        ("CARGO_PROFILE_DEV_DEBUG".to_string(), "1".to_string()),
        ("CARGO_PROFILE_TEST_DEBUG".to_string(), "1".to_string()),
    ]);

    // Keep that shared cache from ballooning (it grew to ~128 GB on the cluster).
    // The two dominant size drivers of debug builds are full debuginfo and
    // incremental-compilation state, and cargo never prunes either across runs.
    // Trim debuginfo to line tables only (`debug = 1` — still enough for
    // backtraces; the numeric form is accepted by every cargo version) and
    // disable incremental compilation. Set via env so it applies to every cargo
    // invocation a run spawns, for any project, without editing the project's
    // `Cargo.toml` and without affecting developers' local builds.
    // Provision the project's toolchains via mise (cached on the PV — no image
    // rebuild), then put mise's shims on PATH so cargo/pnpm/etc. resolve for every
    // node. Best-effort: a failure is logged; the dependent build step will then
    // surface the missing tool clearly.
    if !toolchains.is_empty() {
        let specs = toolchains.clone();
        match tokio::task::spawn_blocking(move || harness_runner::provision_toolchains(&specs))
            .await
        {
            Ok(Ok(())) => {
                if let Some(shims) = harness_runner::mise_shims_dir() {
                    let shims_s = shims.display().to_string();
                    let path = std::env::var("PATH").unwrap_or_default();
                    if !path.split(':').any(|p| p == shims_s) {
                        let path = format!("{shims_s}:{path}");
                        std::env::set_var("PATH", &path);
                        run_env.insert("PATH".to_string(), path);
                    }
                }
            }
            Ok(Err(e)) => {
                tracing::warn!(run_id = %run_id, "toolchain provisioning failed: {e}")
            }
            Err(e) => tracing::warn!(run_id = %run_id, "toolchain task panicked: {e}"),
        }
    }

    let agent: Arc<dyn PromptAgent> = if real {
        // Materialize UI-entered provider credentials into the agent environment
        // (files in $HOME for claude/codex, env vars for tokens/Kimi) before the
        // CLIs run. Best-effort: a missing store/key just means no creds injected.
        match state.cred_store().await {
            Ok(store) => crate::http::credentials_routes::materialize(store).await,
            Err(e) => tracing::info!("credentials not materialized: {e}"),
        }
        // `provider: pi` → omp; `cursor` → cursor-agent; others → CodeAgent registry.
        let code = Arc::new(CodeAgentRunner::new(state.agent_registry.clone()));
        Arc::new(DispatchAgent::new(
            Arc::new(PiAgent::from_env()),
            Arc::new(CursorAgent::from_env()),
            code,
        ))
    } else {
        Arc::new(EchoAgent)
    };
    let runner = LocalRunner::new(workspace, command_dirs, agent).with_env_vars(run_env);

    // The task spec feeds `$ARGUMENTS`/`$USER_MESSAGE`/`$TASK_DESCRIPTION`; the
    // title is exposed separately as `$TASK_TITLE` (option 1).
    let vars = VarContext::new()
        .set("WORKFLOW_ID", run_id.clone())
        .set("ARTIFACTS_DIR", artifacts.display().to_string())
        .set("BASE_BRANCH", base_branch)
        .set("DOCS_DIR", "docs")
        .set("TASK_TITLE", title.clone().unwrap_or_default())
        .set("TASK_DESCRIPTION", description.clone())
        .set("ARGUMENTS", description.clone())
        .set("USER_MESSAGE", description.clone());
    // Bridge the driver's futures-channel events → the tokio broadcast, and
    // persist incrementally (run row on start, each node as it finishes, status
    // on finish) so the run is durable: visible in the list and refresh-safe
    // *before* it completes, not just after.
    let (tx, mut rx) = futures::channel::mpsc::unbounded::<RunEvent>();
    let persist_state = state.clone();
    let persist_run_id = run_id.clone();
    let persist_title = title.clone();
    let persist_description = description.clone();
    let persist_project = Some(project.clone());
    let persist_owner = state.instance_id.clone();
    let persist_ab = ab;
    let forwarder = tokio::spawn(async move {
        let mut ordinals: HashMap<String, i32> = HashMap::new();
        while let Some(ev) = rx.next().await {
            if let Ok(store) = persist_state.store().await {
                match &ev {
                    RunEvent::RunStarted {
                        workflow,
                        total_nodes,
                        nodes,
                    } => {
                        for (i, n) in nodes.iter().enumerate() {
                            ordinals.insert(n.id.clone(), i as i32);
                        }
                        let ab_ref = persist_ab.as_ref().map(|a| harness_persist::AbPairing {
                            pair_id: &a.pair_id,
                            arm: &a.arm,
                            label: a.label.as_deref(),
                        });
                        let _ = store
                            .start_run(
                                &persist_run_id,
                                workflow,
                                persist_title.as_deref(),
                                Some(persist_description.as_str()),
                                persist_project.as_deref(),
                                *total_nodes,
                                nodes,
                                Some(&persist_owner),
                                ab_ref.as_ref(),
                            )
                            .await;
                    }
                    RunEvent::NodeStarted {
                        node_id,
                        provider,
                        model,
                    } => {
                        let ord = ordinals.get(node_id).copied().unwrap_or(0);
                        let _ = store
                            .start_node(
                                &persist_run_id,
                                ord,
                                node_id,
                                provider.as_deref(),
                                model.as_deref(),
                            )
                            .await;
                    }
                    RunEvent::NodeFinished { node } => {
                        let ord = ordinals.get(&node.id).copied().unwrap_or(0);
                        let _ = store.record_node(&persist_run_id, ord, node).await;
                    }
                    RunEvent::RunFinished { status } => {
                        let _ = store.finish_run(&persist_run_id, *status).await;
                    }
                }
            }
            let _ = btx.send(ev);
        }
    });

    // Renew this run's lease while it executes, so the reaper (which only
    // cancels runs whose lease has gone stale) never reaps it mid-flight.
    let hb_state = state.clone();
    let hb_run_id = run_id.clone();
    let heartbeat = tokio::spawn(async move {
        let mut tick = tokio::time::interval(std::time::Duration::from_secs(HEARTBEAT_SECS));
        tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        loop {
            tick.tick().await;
            match hb_state.store().await {
                Ok(store) => {
                    if store.heartbeat_run(&hb_run_id).await.is_err() {
                        break;
                    }
                }
                Err(_) => break,
            }
        }
    });

    let mut report = run_workflow_streaming(&workflow, &runner, &vars, Some(&tx)).await;
    drop(tx); // end the forwarder
    let _ = forwarder.await;
    heartbeat.abort();
    // Capture declared artifact contents while the worktree still exists.
    if let Ok(report) = &mut report {
        for node in &mut report.nodes {
            if node.status != harness_dag::NodeStatus::Success {
                continue;
            }
            let Some(rel) = workflow
                .nodes
                .iter()
                .find(|n| n.id == node.id)
                .and_then(|n| n.artifact.as_deref())
            else {
                continue;
            };
            node.artifact_content = read_declared_artifact(&artifacts, rel);
        }
    }
    // Final authoritative snapshot (reconciles nodes + sets terminal status).
    if let Ok(report) = &report {
        if let Ok(store) = state.store().await {
            if let Err(e) = store
                .record_run(
                    &run_id,
                    title.as_deref(),
                    Some(description.as_str()),
                    Some(project.as_str()),
                    report,
                )
                .await
            {
                tracing::warn!(run_id = %run_id, "failed to persist run: {e}");
            }
        }
    } else if let Err(e) = &report {
        tracing::warn!(run_id = %run_id, "run failed to execute: {e}");
    }

    state.live.lock().await.remove(&run_id);
}

/// Resolve a run's workspace: fetch the project's checkout and cut an isolated
/// worktree off `origin/<base_branch>`. Returns the worktree dir + a guard that
/// removes it on drop. Errors (missing checkout, bad branch) fail the run — there
/// is no global-root fallback.
async fn resolve_workspace(
    state: &Arc<RunsState>,
    project: &str,
    run_id: &str,
    base_branch: &str,
) -> Result<(PathBuf, harness_runner::Worktree), String> {
    let checkout = state.projects_dir.join(project);
    if !checkout.exists() {
        return Err(format!(
            "project `{project}` has no checkout at {} — re-register it",
            checkout.display()
        ));
    }
    let token = state.github_token_for_project(project).await;
    let base = base_branch.to_string();
    let branch = format!("run/{run_id}");
    let dest = state.projects_dir.join(".worktrees").join(run_id);
    let made = tokio::task::spawn_blocking(move || {
        // Best-effort fetch so the worktree is cut off the latest remote tip.
        let _ = harness_runner::fetch_repo(&checkout, token.as_deref());
        if let Some(parent) = dest.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        harness_runner::Worktree::create(&checkout, &format!("origin/{base}"), &branch, &dest)
    })
    .await;
    match made {
        Ok(Ok(wt)) => Ok((wt.path.clone(), wt)),
        Ok(Err(e)) => Err(format!("worktree create failed: {e}")),
        Err(e) => Err(format!("worktree task panicked: {e}")),
    }
}

/// `GET /runs/{id}/stream` — SSE of live events for a currently-executing run.
pub async fn stream_run(
    Extension(state): Extension<Arc<RunsState>>,
    AxumPath(id): AxumPath<String>,
) -> Response {
    let rx = {
        let live = state.live.lock().await;
        match live.get(&id) {
            Some(run) => run.tx.subscribe(),
            None => {
                return err(
                    StatusCode::NOT_FOUND,
                    format!("run `{id}` is not streaming (finished or unknown — GET /runs/{id})"),
                )
            }
        }
    };

    let stream = BroadcastStream::new(rx).filter_map(|item| async move {
        match item {
            // `json_data` returns Result<Event, axum::Error>, which Sse accepts.
            Ok(event) => Some(Event::default().json_data(&event)),
            // Lagged: skip dropped events rather than terminate the stream.
            Err(_) => None,
        }
    });

    Sse::new(stream)
        .keep_alive(KeepAlive::default())
        .into_response()
}

/// `POST /runs/{id}/cancel` — stop a running run: abort its in-flight task (if
/// this process owns it) and mark the run + its in-flight nodes cancelled.
pub async fn cancel_run(
    Extension(state): Extension<Arc<RunsState>>,
    AxumPath(id): AxumPath<String>,
) -> Response {
    // Abort the live task and drop its broadcast channel, if we own it. Aborting
    // unwinds the task — its worktree guard is dropped and cleaned up.
    if let Some(run) = state.live.lock().await.remove(&id) {
        run.abort.abort();
    }
    let store = match state.store().await {
        Ok(s) => s,
        Err(e) => return err(StatusCode::SERVICE_UNAVAILABLE, e),
    };
    match store.cancel_run(&id).await {
        Ok(true) => StatusCode::NO_CONTENT.into_response(),
        Ok(false) => err(
            StatusCode::CONFLICT,
            format!("run `{id}` is not running (already finished or unknown)"),
        ),
        Err(e) => err(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()),
    }
}

/// `DELETE /runs/{id}` — remove a run and its node rows from the list. Aborts a
/// live task first so deleting a still-running run leaves nothing orphaned.
pub async fn delete_run(
    Extension(state): Extension<Arc<RunsState>>,
    AxumPath(id): AxumPath<String>,
) -> Response {
    if let Some(run) = state.live.lock().await.remove(&id) {
        run.abort.abort();
    }
    let store = match state.store().await {
        Ok(s) => s,
        Err(e) => return err(StatusCode::SERVICE_UNAVAILABLE, e),
    };
    match store.delete_run(&id).await {
        Ok(true) => StatusCode::NO_CONTENT.into_response(),
        Ok(false) => err(StatusCode::NOT_FOUND, format!("run `{id}` not found")),
        Err(e) => err(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()),
    }
}
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn find_pr_url_extracts_and_trims() {
        // Plain URL in a sentence — trailing punctuation stripped.
        assert_eq!(
            find_pr_url_in("Opened https://github.com/o/r/pull/42 for review."),
            Some("https://github.com/o/r/pull/42".to_string())
        );
        // Markdown-wrapped link.
        assert_eq!(
            find_pr_url_in("PR: (https://github.com/acme/app/pull/7)"),
            Some("https://github.com/acme/app/pull/7".to_string())
        );
        // No PR link → None.
        assert_eq!(find_pr_url_in("validation passed; no PR yet"), None);
        // A plain repo/commit URL is not a PR.
        assert_eq!(
            find_pr_url_in("see https://github.com/o/r/commit/abc"),
            None
        );
    }

    #[test]
    fn ab_swap_replaces_every_kimi_occurrence_and_leaves_others() {
        // Real bundled workflow: kimi as the default + many node pins + a loop
        // body, with gpt-5.5 and sonnet pinned on specialist review steps.
        let yaml = harness_runner::default_workflow("idea-to-pr").expect("bundled idea-to-pr");
        let mut wf = parse_workflow(yaml).expect("idea-to-pr parses");

        let before = workflow_model_pairs(&wf);
        let has =
            |ps: &[ModelRef], p: &str, m: &str| ps.iter().any(|r| r.provider == p && r.model == m);
        assert!(
            has(&before, "pi", "kimi-code/kimi-for-coding"),
            "kimi present"
        );
        assert!(
            has(&before, "pi", "openai-codex/gpt-5.5"),
            "gpt-5.5 present"
        );
        assert!(has(&before, "claude", "sonnet"), "sonnet present");

        apply_model_swap(
            &mut wf,
            &ModelRef {
                provider: "pi".into(),
                model: "kimi-code/kimi-for-coding".into(),
            },
            &ModelRef {
                provider: "cursor".into(),
                model: "composer-2.5".into(),
            },
        );

        let after = workflow_model_pairs(&wf);
        // Every kimi occurrence — default, node pins, AND the loop body — is gone.
        assert!(
            !has(&after, "pi", "kimi-code/kimi-for-coding"),
            "all kimi (incl. loop body) swapped out"
        );
        assert!(
            has(&after, "cursor", "composer-2.5"),
            "challenger swapped in"
        );
        // Specialist review steps pinned to other models are untouched.
        assert!(
            has(&after, "pi", "openai-codex/gpt-5.5"),
            "gpt-5.5 unchanged"
        );
        assert!(has(&after, "claude", "sonnet"), "sonnet unchanged");
    }

    #[test]
    fn summary_window_start_uses_visible_utc_calendar_days() {
        let now = DateTime::parse_from_rfc3339("2026-06-08T15:30:00Z")
            .unwrap()
            .with_timezone(&Utc);

        assert_eq!(
            summary_window_start(now, 14),
            DateTime::parse_from_rfc3339("2026-05-26T00:00:00Z")
                .unwrap()
                .with_timezone(&Utc)
        );
        assert_eq!(
            summary_window_start(now, 1),
            DateTime::parse_from_rfc3339("2026-06-08T00:00:00Z")
                .unwrap()
                .with_timezone(&Utc)
        );
        assert_eq!(
            summary_window_start(now, 0),
            DateTime::parse_from_rfc3339("2026-06-08T00:00:00Z")
                .unwrap()
                .with_timezone(&Utc)
        );
    }

    #[test]
    fn dir_size_sums_regular_files_skips_symlinks() {
        let dir = tempfile::tempdir().unwrap();
        let a = dir.path().join("a.txt");
        let sub = dir.path().join("sub");
        std::fs::create_dir(&sub).unwrap();
        std::fs::write(&a, "hello").unwrap();
        std::fs::write(sub.join("b.txt"), "world").unwrap();
        #[cfg(unix)]
        {
            let link = dir.path().join("link.txt");
            std::os::unix::fs::symlink(&a, &link).unwrap();
        }
        assert_eq!(dir_size(dir.path()), 10);
    }

    #[test]
    fn dir_size_missing_path_is_zero() {
        assert_eq!(
            dir_size(std::path::Path::new("/does/not/exist/for/sure")),
            0
        );
    }

    #[test]
    fn resolve_cap_gb_prefers_project_setting() {
        // When a positive per-project cap is set, it wins over everything.
        assert_eq!(resolve_cap_gb(Some(42)), 42);
    }

    #[test]
    fn resolve_cap_gb_falls_back_to_env_then_default() {
        assert_eq!(resolve_cap_gb_with_env(None, Some("25")), 25);
        assert_eq!(resolve_cap_gb_with_env(Some(0), Some("25")), 25);
        assert_eq!(resolve_cap_gb_with_env(Some(-5), Some("25")), 25);
        assert_eq!(resolve_cap_gb_with_env(None, None), CACHE_CAP_GB_DEFAULT);
        assert_eq!(
            resolve_cap_gb_with_env(None, Some("invalid")),
            CACHE_CAP_GB_DEFAULT
        );
    }

    #[test]
    fn resolve_cap_gb_allows_env_zero_to_disable_sweeping() {
        assert_eq!(resolve_cap_gb_with_env(None, Some("0")), 0);
    }

    #[test]
    fn read_declared_artifact_reads_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("test-artifact.txt");
        std::fs::write(&path, "hello artifact").unwrap();
        assert_eq!(
            read_declared_artifact(dir.path(), "test-artifact.txt"),
            Some("hello artifact".into())
        );
    }
    #[test]
    fn read_declared_artifact_rejects_traversal() {
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(read_declared_artifact(dir.path(), "../etc/passwd"), None);
        assert_eq!(read_declared_artifact(dir.path(), "/etc/passwd"), None);
    }
    #[test]
    fn read_declared_artifact_missing_file() {
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(
            read_declared_artifact(dir.path(), "does-not-exist.md"),
            None
        );
    }
    #[test]
    fn read_declared_artifact_rejects_empty_string() {
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(read_declared_artifact(dir.path(), ""), None);
    }
    #[test]
    fn read_declared_artifact_truncates_on_char_boundary() {
        let dir = tempfile::tempdir().unwrap();
        // 3-byte UTF-8 character repeated; truncating at ARTIFACT_MAX_BYTES
        // lands inside a character and must snap to the previous boundary.
        let content = "€".repeat(ARTIFACT_MAX_BYTES / 2 + 1);
        let path = dir.path().join("big-artifact.txt");
        std::fs::write(&path, &content).unwrap();
        let result = read_declared_artifact(dir.path(), "big-artifact.txt").unwrap();
        assert!(result.starts_with("€"));
        assert!(result.contains("[truncated: "));
        assert!(result.contains(" bytes total]"));
    }

    #[test]
    fn sweep_cargo_cache_noop_under_cap() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("a"), vec![0u8; 1024]).unwrap();
        // Cap far above contents → no action, file intact.
        assert!(
            sweep_cargo_cache(dir.path(), 10 * 1024 * 1024, 8 * 1024 * 1024, 0)
                .unwrap()
                .is_none()
        );
        assert!(dir.path().join("a").exists());
    }

    #[test]
    fn sweep_cargo_cache_evicts_down_to_target() {
        let dir = tempfile::tempdir().unwrap();
        for i in 0..10 {
            std::fs::write(dir.path().join(format!("f{i}")), vec![0u8; 1024 * 1024]).unwrap();
        }
        // 10 MiB total; cap 5 MiB, target 4 MiB, floor 0 (protect nothing).
        let (before, after) = sweep_cargo_cache(dir.path(), 5 * 1024 * 1024, 4 * 1024 * 1024, 0)
            .unwrap()
            .expect("acts when over cap");
        assert!(before >= 10 * 1024 * 1024);
        assert!(after <= 4 * 1024 * 1024, "after={after}");
    }

    #[test]
    fn sweep_cargo_cache_safety_floor_protects_fresh() {
        let dir = tempfile::tempdir().unwrap();
        for i in 0..10 {
            std::fs::write(dir.path().join(format!("f{i}")), vec![0u8; 1024 * 1024]).unwrap();
        }
        // Over cap, but a 1h floor protects the just-written (fresh) files →
        // nothing is deleted.
        let (_before, after) =
            sweep_cargo_cache(dir.path(), 5 * 1024 * 1024, 4 * 1024 * 1024, 3600)
                .unwrap()
                .expect("acts (over cap) but protects fresh files");
        assert!(
            after >= 10 * 1024 * 1024,
            "fresh files must be protected, after={after}"
        );
    }
    #[test]
    fn public_url_trims_trailing_slashes() {
        let reg = Arc::new(AgentRegistry::new("codex"));
        let state = RunsState::new(
            None,
            reg,
            std::path::PathBuf::from("/tmp"),
            None,
            Some("https://example.com/".to_string()),
        );
        assert_eq!(state.public_url, Some("https://example.com".to_string()));
    }

    #[test]
    fn public_url_trims_all_trailing_slashes() {
        let reg = Arc::new(AgentRegistry::new("codex"));
        let state = RunsState::new(
            None,
            reg,
            std::path::PathBuf::from("/tmp"),
            None,
            Some("https://example.com///".to_string()),
        );
        assert_eq!(state.public_url, Some("https://example.com".to_string()));
    }

    #[test]
    fn public_url_rejects_whitespace_only() {
        let reg = Arc::new(AgentRegistry::new("codex"));
        let state = RunsState::new(
            None,
            reg,
            std::path::PathBuf::from("/tmp"),
            None,
            Some("   ".to_string()),
        );
        assert_eq!(state.public_url, None);
    }

    #[test]
    fn public_url_rejects_empty_string() {
        let reg = Arc::new(AgentRegistry::new("codex"));
        let state = RunsState::new(
            None,
            reg,
            std::path::PathBuf::from("/tmp"),
            None,
            Some("".to_string()),
        );
        assert_eq!(state.public_url, None);
    }

    #[test]
    fn public_url_none_when_input_none() {
        let reg = Arc::new(AgentRegistry::new("codex"));
        let state = RunsState::new(None, reg, std::path::PathBuf::from("/tmp"), None, None);
        assert_eq!(state.public_url, None);
    }
}
