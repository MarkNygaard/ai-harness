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

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use axum::extract::{Extension, Path as AxumPath};
use axum::http::StatusCode;
use axum::response::sse::{Event, KeepAlive, Sse};
use axum::response::{IntoResponse, Response};
use axum::Json;
use futures::StreamExt;
use harness_agents::registry::AgentRegistry;
use harness_dag::{parse_workflow, run_workflow_streaming, RunEvent, VarContext};
use harness_persist::{ProjectStore, RunStore};
use harness_runner::{
    sanitize_branch_component, CodeAgentRunner, DispatchAgent, EchoAgent, LocalRunner, PiAgent,
    PromptAgent,
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
    linear_source_store: OnceCell<harness_persist::LinearSourceStore>,
    /// Where project repos are cloned (one checkout dir per project).
    pub(crate) projects_dir: PathBuf,
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
}

impl RunsState {
    pub fn new(
        db_url: Option<String>,
        agent_registry: Arc<AgentRegistry>,
        project_root: PathBuf,
        secret_key: Option<[u8; 32]>,
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
            linear_source_store: OnceCell::new(),
            projects_dir,
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

    /// The global GitHub token from the credential store, if configured — used
    /// to clone/fetch private project repos. Best-effort: any error → `None`.
    pub(crate) async fn github_token(&self) -> Option<String> {
        let store = self.cred_store().await.ok()?;
        let fields = store.get("github").await.ok()??;
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

/// `GET /runs` — list recent runs.
pub async fn list_runs(Extension(state): Extension<Arc<RunsState>>) -> Response {
    let store = match state.store().await {
        Ok(s) => s,
        Err(e) => return err(StatusCode::SERVICE_UNAVAILABLE, e),
    };
    match store.list_runs(100).await {
        Ok(runs) => Json(runs).into_response(),
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

    // Resolve `workflow` against the project's checkout (its `.harness/workflows`)
    // then bundled defaults.
    let workflow_root = state.projects_dir.join(&project);
    let (yaml, _label) = harness_runner::resolve_workflow_source(&workflow_name, &workflow_root)
        .map_err(|e| (StatusCode::BAD_REQUEST, e))?;
    let workflow = parse_workflow(&yaml)
        .map_err(|e| (StatusCode::BAD_REQUEST, format!("invalid workflow: {e}")))?;

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
    // `description` is the task spec; fall back to the deprecated `args` alias.
    let description = if req.description.is_empty() {
        req.args
    } else {
        req.description
    };
    let title = req.title.filter(|t| !t.trim().is_empty());
    let handle = tokio::spawn(async move {
        execute_run_task(
            task_state,
            task_run_id,
            workflow,
            req.real,
            title,
            description,
            base_branch,
            project,
            toolchains,
            btx,
        )
        .await;
    });
    state.live.lock().await.insert(
        run_id.clone(),
        LiveRun {
            tx: live_tx,
            abort: handle.abort_handle(),
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
    std::env::set_var("CARGO_TARGET_DIR", &cargo_target);

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
                        std::env::set_var("PATH", format!("{shims_s}:{path}"));
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
        // `provider: pi` → omp-backed session-aware agent; others → CodeAgent registry.
        let code = Arc::new(CodeAgentRunner::new(state.agent_registry.clone()));
        Arc::new(DispatchAgent::new(Arc::new(PiAgent::from_env()), code))
    } else {
        Arc::new(EchoAgent)
    };
    let runner = LocalRunner::new(workspace, command_dirs, agent);

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

    let report = run_workflow_streaming(&workflow, &runner, &vars, Some(&tx)).await;
    drop(tx); // end the forwarder
    let _ = forwarder.await;
    heartbeat.abort();
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
    let token = state.github_token().await;
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
