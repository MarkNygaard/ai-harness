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
use std::path::{Path, PathBuf};
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
use harness_persist::RunStore;
use harness_runner::{
    sanitize_branch_component, CodeAgentRunner, EchoAgent, LocalRunner, PromptAgent,
};
use serde::{Deserialize, Serialize};
use tokio::sync::{broadcast, Mutex, OnceCell};
use tokio_stream::wrappers::BroadcastStream;

/// Self-contained state for the runs API.
pub struct RunsState {
    db_url: Option<String>,
    store: OnceCell<RunStore>,
    agent_registry: Arc<AgentRegistry>,
    project_root: PathBuf,
    /// Live runs → broadcast of their events (present only while executing).
    live: Mutex<HashMap<String, broadcast::Sender<RunEvent>>>,
}

impl RunsState {
    pub fn new(
        db_url: Option<String>,
        agent_registry: Arc<AgentRegistry>,
        project_root: PathBuf,
    ) -> Self {
        Self {
            db_url,
            store: OnceCell::new(),
            agent_registry,
            project_root,
            live: Mutex::new(HashMap::new()),
        }
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
}

#[derive(Debug, Deserialize)]
pub struct CreateRunRequest {
    /// Workflow YAML path, relative to the server's project root.
    pub workflow: String,
    #[serde(default)]
    pub args: String,
    #[serde(default)]
    pub real: bool,
    #[serde(default)]
    pub base_branch: Option<String>,
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

/// Resolve a workflow path within the project root (no escaping).
fn resolve_workflow(project_root: &Path, workflow: &str) -> Result<PathBuf, String> {
    let candidate = project_root.join(workflow);
    let root = project_root
        .canonicalize()
        .map_err(|e| format!("bad project root: {e}"))?;
    let path = candidate
        .canonicalize()
        .map_err(|e| format!("workflow not found: {e}"))?;
    if !path.starts_with(&root) {
        return Err("workflow path escapes project root".into());
    }
    Ok(path)
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
    let path = match resolve_workflow(&state.project_root, &req.workflow) {
        Ok(p) => p,
        Err(e) => return err(StatusCode::BAD_REQUEST, e),
    };
    let yaml = match std::fs::read_to_string(&path) {
        Ok(y) => y,
        Err(e) => return err(StatusCode::BAD_REQUEST, format!("read workflow: {e}")),
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
    let base_branch = req
        .base_branch
        .clone()
        .unwrap_or_else(|| "main".to_string());
    tokio::spawn(async move {
        execute_run_task(
            task_state,
            task_run_id,
            workflow,
            req.real,
            req.args,
            base_branch,
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
    args: String,
    base_branch: String,
    btx: broadcast::Sender<RunEvent>,
) {
    let workspace = state.project_root.clone();
    let artifacts = workspace.join(".harness").join("artifacts");
    let _ = std::fs::create_dir_all(&artifacts);
    let command_dirs = vec![workspace.join(".harness").join("commands")];

    let agent: Arc<dyn PromptAgent> = if real {
        Arc::new(CodeAgentRunner::new(state.agent_registry.clone()))
    } else {
        Arc::new(EchoAgent)
    };
    let runner = LocalRunner::new(workspace, command_dirs, agent);

    let vars = VarContext::new()
        .set("WORKFLOW_ID", run_id.clone())
        .set("ARTIFACTS_DIR", artifacts.display().to_string())
        .set("BASE_BRANCH", base_branch)
        .set("DOCS_DIR", "docs")
        .set("ARGUMENTS", args.clone())
        .set("USER_MESSAGE", args);

    // Bridge the driver's futures-channel events → the tokio broadcast.
    let (tx, mut rx) = futures::channel::mpsc::unbounded::<RunEvent>();
    let forwarder = tokio::spawn(async move {
        while let Some(ev) = rx.next().await {
            let _ = btx.send(ev);
        }
    });

    let report = run_workflow_streaming(&workflow, &runner, &vars, Some(&tx)).await;
    drop(tx); // end the forwarder
    let _ = forwarder.await;

    if let Ok(report) = &report {
        if let Ok(store) = state.store().await {
            if let Err(e) = store.record_run(&run_id, None, report).await {
                tracing::warn!(run_id = %run_id, "failed to persist run: {e}");
            }
        }
    } else if let Err(e) = &report {
        tracing::warn!(run_id = %run_id, "run failed to execute: {e}");
    }

    state.live.lock().await.remove(&run_id);
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
