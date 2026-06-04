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

/// Self-contained state for the runs API.
pub struct RunsState {
    db_url: Option<String>,
    store: OnceCell<RunStore>,
    agent_registry: Arc<AgentRegistry>,
    pub(crate) project_root: PathBuf,
    /// AES key for the credential store (from `HARNESS_SECRET_KEY`), if set.
    secret_key: Option<[u8; 32]>,
    cred_store: OnceCell<harness_persist::CredentialStore>,
    project_store: OnceCell<ProjectStore>,
    /// Where project repos are cloned (one checkout dir per project).
    pub(crate) projects_dir: PathBuf,
    /// Live runs → broadcast of their events (present only while executing).
    live: Mutex<HashMap<String, broadcast::Sender<RunEvent>>>,
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
        Self {
            db_url,
            store: OnceCell::new(),
            agent_registry,
            project_root,
            secret_key,
            cred_store: OnceCell::new(),
            project_store: OnceCell::new(),
            projects_dir,
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

    /// The global GitHub token from the credential store, if configured — used
    /// to clone/fetch private project repos. Best-effort: any error → `None`.
    pub(crate) async fn github_token(&self) -> Option<String> {
        let store = self.cred_store().await.ok()?;
        let fields = store.get("github").await.ok()??;
        fields.get("token").filter(|v| !v.is_empty()).cloned()
    }

    /// Lazily connect (and migrate) the persistence store.
    async fn store(&self) -> Result<&RunStore, String> {
        let url = self
            .db_url
            .as_deref()
            .ok_or("no database configured (set server.database_url)")?;
        self.store
            .get_or_try_init(|| async { RunStore::connect(url).await.map_err(|e| e.to_string()) })
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
    /// Project to run within. Its repo checkout becomes the workspace and a
    /// per-run worktree is cut off its `base_branch`. Omitted → the global
    /// `project_root` (back-compat).
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
    // Resolve the project (if any) up front: reject unknown projects, and let a
    // project's `default_workflow` stand in when the request names none.
    let project = req.project.clone().filter(|p| !p.trim().is_empty());
    let mut workflow_name = req.workflow.clone();
    let mut base_default = req.base_branch.clone();
    if let Some(name) = project.as_deref() {
        let store = match state.project_store().await {
            Ok(s) => s,
            Err(e) => return err(StatusCode::SERVICE_UNAVAILABLE, e),
        };
        match store.get(name).await {
            Ok(Some(p)) => {
                if workflow_name.trim().is_empty() {
                    if let Some(def) = p.default_workflow {
                        workflow_name = def;
                    }
                }
                base_default.get_or_insert(p.base_branch);
            }
            Ok(None) => return err(StatusCode::BAD_REQUEST, format!("unknown project `{name}`")),
            Err(e) => return err(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()),
        }
    }

    // `workflow` is a path or a bare name (project `.harness/workflows` then a
    // bundled default); empty uses the default pipeline.
    let (yaml, _label) =
        match harness_runner::resolve_workflow_source(&workflow_name, &state.project_root) {
            Ok(v) => v,
            Err(e) => return err(StatusCode::BAD_REQUEST, e),
        };
    let workflow = match parse_workflow(&yaml) {
        Ok(w) => w,
        Err(e) => return err(StatusCode::BAD_REQUEST, format!("invalid workflow: {e}")),
    };

    let run_id = format!(
        "{}-{}",
        sanitize_branch_component(&workflow.name),
        now_millis()
    );

    // Register a broadcast channel so /stream subscribers see live events.
    let (btx, _) = broadcast::channel::<RunEvent>(256);
    state.live.lock().await.insert(run_id.clone(), btx.clone());

    let task_state = state.clone();
    let task_run_id = run_id.clone();
    let base_branch = base_default.unwrap_or_else(|| "main".to_string());
    // `description` is the task spec; fall back to the deprecated `args` alias.
    let description = if req.description.is_empty() {
        req.args
    } else {
        req.description
    };
    let title = req.title.filter(|t| !t.trim().is_empty());
    tokio::spawn(async move {
        execute_run_task(
            task_state,
            task_run_id,
            workflow,
            req.real,
            title,
            description,
            base_branch,
            project,
            btx,
        )
        .await;
    });

    (StatusCode::ACCEPTED, Json(CreateRunResponse { run_id })).into_response()
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
    project: Option<String>,
    btx: broadcast::Sender<RunEvent>,
) {
    // Resolve the workspace: a project run gets an isolated per-run worktree off
    // its checkout's `origin/<base_branch>` (so concurrent runs in the same
    // project don't collide); otherwise the global project root. `_worktree` is
    // held for the run's lifetime and removed on drop.
    let (workspace, _worktree) =
        resolve_workspace(&state, project.as_deref(), &run_id, &base_branch).await;
    let artifacts = workspace.join(".harness").join("artifacts");
    let _ = std::fs::create_dir_all(&artifacts);
    let command_dirs = vec![workspace.join(".harness").join("commands")];

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
        .set("USER_MESSAGE", description);

    // Bridge the driver's futures-channel events → the tokio broadcast, and
    // persist incrementally (run row on start, each node as it finishes, status
    // on finish) so the run is durable: visible in the list and refresh-safe
    // *before* it completes, not just after.
    let (tx, mut rx) = futures::channel::mpsc::unbounded::<RunEvent>();
    let persist_state = state.clone();
    let persist_run_id = run_id.clone();
    let persist_title = title.clone();
    let persist_project = project.clone();
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
                                persist_project.as_deref(),
                                *total_nodes,
                                nodes,
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
                    _ => {}
                }
            }
            let _ = btx.send(ev);
        }
    });

    let report = run_workflow_streaming(&workflow, &runner, &vars, Some(&tx)).await;
    drop(tx); // end the forwarder
    let _ = forwarder.await;

    // Final authoritative snapshot (reconciles nodes + sets terminal status).
    if let Ok(report) = &report {
        if let Ok(store) = state.store().await {
            if let Err(e) = store
                .record_run(&run_id, title.as_deref(), project.as_deref(), report)
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

/// Resolve a run's workspace. For a project run, fetch the checkout and cut an
/// isolated worktree off `origin/<base_branch>`; on any failure (or no project)
/// fall back to the global project root. Returns the workspace dir and an
/// optional worktree guard that cleans up on drop.
async fn resolve_workspace(
    state: &Arc<RunsState>,
    project: Option<&str>,
    run_id: &str,
    base_branch: &str,
) -> (PathBuf, Option<harness_runner::Worktree>) {
    let Some(name) = project else {
        return (state.project_root.clone(), None);
    };
    let checkout = state.projects_dir.join(name);
    if !checkout.exists() {
        tracing::warn!(
            run_id,
            project = name,
            "project checkout missing; using project root"
        );
        return (state.project_root.clone(), None);
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
        Ok(Ok(wt)) => (wt.path.clone(), Some(wt)),
        Ok(Err(e)) => {
            tracing::warn!(
                run_id,
                project = name,
                "worktree create failed: {e}; using project root"
            );
            (state.project_root.clone(), None)
        }
        Err(e) => {
            tracing::warn!(run_id, "worktree task panicked: {e}; using project root");
            (state.project_root.clone(), None)
        }
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
            Some(btx) => btx.subscribe(),
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
