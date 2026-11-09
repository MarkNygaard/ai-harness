//! The DAG execution driver.
//!
//! [`run_workflow`] walks the topological layers from
//! [`crate::graph::topological_layers`] and drives each node, delegating the
//! actual *execution of a node body* to a [`NodeRunner`]. That trait is the
//! seam between this environment-agnostic orchestration and the concrete
//! backends (a local subprocess/worktree executor, a Kubernetes executor, or a
//! test mock).
//!
//! The driver owns: layer ordering, parallel-within-layer execution, session
//! threading across sequential layers, `trigger_rule` evaluation, loop
//! iteration + `until` signal detection, cancellation, and token-usage
//! aggregation into a [`RunReport`]. The runner owns: spawning processes /
//! agents, resolving `command` files, and reporting output + usage.

use std::collections::HashMap;
use std::sync::Arc;

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use futures::channel::mpsc::UnboundedSender;
use futures::future::join_all;
use serde::{Deserialize, Serialize};

use crate::error::DagError;
use crate::graph::topological_layers;
use crate::model::{
    ContextMode, LinearAction, Node, NodeKind, ScriptRuntime, TriggerRule, Workflow,
};
use crate::signal::detect_signal;
use crate::vars::{substitute, VarContext};

/// Token usage reported by a provider for one invocation.
///
/// Fields are `Option` because not every provider reports every counter (only
/// Anthropic reports cache reads/writes, for example). We store what we get and
/// render the rest as "n/a" rather than synthesizing zeros — see PLAN §10.1.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Usage {
    pub input: Option<u64>,
    pub output: Option<u64>,
    pub cache_read: Option<u64>,
    pub cache_write: Option<u64>,
}

impl Usage {
    /// Accumulate another usage, summing only the counters the other reports.
    pub fn add(&mut self, other: &Usage) {
        fn merge(a: &mut Option<u64>, b: Option<u64>) {
            if let Some(b) = b {
                *a = Some(a.unwrap_or(0) + b);
            }
        }
        merge(&mut self.input, other.input);
        merge(&mut self.output, other.output);
        merge(&mut self.cache_read, other.cache_read);
        merge(&mut self.cache_write, other.cache_write);
    }
}

/// The executable body handed to a [`NodeRunner`]. Inline text (`Prompt`,
/// `Bash`, `Script`) is already variable-substituted by the driver; `Command`
/// carries the raw name and the runner resolves + substitutes it via
/// [`NodeRequest::vars`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NodeBody {
    Prompt(String),
    Bash(String),
    Command(String),
    Script {
        script: String,
        runtime: ScriptRuntime,
        deps: Vec<String>,
    },
    /// A Linear operation on the run's issue. Deterministic, like `Bash`: the
    /// runner calls the server's run-scoped Linear endpoint rather than an AI
    /// provider. Vars are already substituted by the time it arrives.
    Linear(LinearAction),
}

/// One execution request for a single node body invocation.
pub struct NodeRequest<'a> {
    pub node_id: &'a str,
    pub provider: Option<&'a str>,
    pub model: Option<&'a str>,
    /// Reasoning-effort override (`low`..`max`), forwarded to the agent CLI.
    pub effort: Option<&'a str>,
    pub context: ContextMode,
    /// Session id to continue (shared context / loop threading), if any.
    pub session: Option<String>,
    /// 1-based iteration; `> 1` only for loop bodies.
    pub iteration: u32,
    pub body: NodeBody,
    pub timeout: Option<u64>,
    /// Variable context, for `Command`/`Script` resolution by the runner.
    pub vars: &'a VarContext,
    /// Optional JSON schema the agent's output should conform to (AI bodies
    /// only). The runner instructs the agent to emit matching JSON.
    pub output_format: Option<&'a serde_json::Value>,
    /// Provider-agnostic tool hooks translated per provider at dispatch.
    pub hooks: Option<&'a crate::model::NodeHooks>,
    /// External MCP servers the step asked for by name (e.g. `sentry`).
    pub mcp: &'a [crate::model::McpUse],
    /// Optional sink for live activity updates (AI bodies only; a streaming
    /// runner reports the node's latest line here). `None` = no live progress.
    pub progress: Option<ProgressSink>,
}

/// What a [`NodeRunner`] returns from one invocation.
#[derive(Debug, Clone, Default)]
pub struct NodeOutput {
    pub text: String,
    /// Session id to thread into the next shared/looping invocation.
    pub session: Option<String>,
    pub success: bool,
    pub usage: Usage,
}

/// Error from a runner invocation (process spawn failure, agent error, …).
#[derive(Debug, thiserror::Error)]
#[error("{0}")]
pub struct RunnerError(pub String);

/// What kind of live activity a [`NodeProgress`](RunEvent::NodeProgress) line
/// is, so the UI can render it as a distinct card instead of a flat log line.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ActivityKind {
    /// Assistant prose (a status line / what it's doing), or a progress marker.
    Text,
    /// A tool invocation: `text` is the tool name, `detail` its input summary.
    Tool,
    /// A tool's output: `detail` is a snippet of what the tool returned.
    ToolResult,
}

/// One live activity update from a running node — a tool call, a tool result,
/// or a line of assistant text. Replaces the old bare string so the UI can show
/// a structured feed (cards) rather than an undifferentiated scroll.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Activity {
    pub kind: ActivityKind,
    /// Primary line: assistant text, or a tool name for `Tool`.
    pub text: String,
    /// Secondary detail: a tool's input summary or a result snippet; `None` for
    /// plain text.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
    /// Tool-call correlation id, so the UI can pair a `Tool` with its
    /// `ToolResult` (and time it): the call's own id for `Tool`, the referenced
    /// call id for `ToolResult`. `None` for text and unidentified tools.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool_id: Option<String>,
    /// Whether a `ToolResult` reported failure (`is_error`), so the UI can mark
    /// it ✗ instead of ✓. Always `false` for text and tool calls.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub is_error: bool,
}

impl Activity {
    /// A plain text line (assistant prose or a progress marker).
    pub fn text(text: impl Into<String>) -> Self {
        Self {
            kind: ActivityKind::Text,
            text: text.into(),
            detail: None,
            tool_id: None,
            is_error: false,
        }
    }
    /// A tool call: `name` is the tool, `detail` a short input summary, `tool_id`
    /// the call id the matching result references.
    pub fn tool(name: impl Into<String>, detail: Option<String>, tool_id: Option<String>) -> Self {
        Self {
            kind: ActivityKind::Tool,
            text: name.into(),
            detail,
            tool_id,
            is_error: false,
        }
    }
    /// A tool result: `detail` is a snippet of the tool's output, `tool_id` the
    /// id of the call it answers, `is_error` whether the tool reported failure.
    pub fn tool_result(detail: impl Into<String>, tool_id: Option<String>, is_error: bool) -> Self {
        Self {
            kind: ActivityKind::ToolResult,
            text: String::new(),
            detail: Some(detail.into()),
            tool_id,
            is_error,
        }
    }
}

impl From<String> for Activity {
    fn from(s: String) -> Self {
        Activity::text(s)
    }
}
impl From<&str> for Activity {
    fn from(s: &str) -> Self {
        Activity::text(s)
    }
}

/// A per-node sink the driver hands to a streaming runner so it can report the
/// node's latest activity *while it runs* (rendered on the running node in the
/// graph). Cloning is cheap (an `Arc`); each call emits a live-only
/// [`RunEvent::NodeProgress`]. `None` when the run isn't streaming events.
#[derive(Clone)]
pub struct ProgressSink(pub Arc<dyn Fn(Activity) + Send + Sync>);

impl ProgressSink {
    /// Report one activity update for the current node.
    pub fn report(&self, activity: Activity) {
        (self.0)(activity);
    }
}

// `Arc<dyn Fn>` isn't `Debug`; provide one so `ProgressSink` can live on the
// `Debug`-deriving request structs that carry it.
impl std::fmt::Debug for ProgressSink {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("ProgressSink(..)")
    }
}

/// The seam between the DAG driver and the environment that executes node
/// bodies. Implementations: a local subprocess/worktree executor, a Kubernetes
/// executor, or a test mock.
#[async_trait]
pub trait NodeRunner: Send + Sync {
    /// Execute one node body invocation and return its output + usage.
    async fn execute(&self, req: NodeRequest<'_>) -> Result<NodeOutput, RunnerError>;
}

/// Terminal status of a single node.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NodeStatus {
    Success,
    Failed,
    Skipped,
    Cancelled,
}

/// Terminal status of a whole run.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RunStatus {
    Completed,
    Failed,
    Cancelled,
}

/// Per-node record in a [`RunReport`].
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NodeRun {
    pub id: String,
    pub status: NodeStatus,
    pub provider: Option<String>,
    pub model: Option<String>,
    pub output: String,
    pub usage: Usage,
    /// Iterations executed (1 for non-loop nodes).
    pub iterations: u32,
    /// For loop nodes: whether the `until` signal converged before the cap.
    pub converged: Option<bool>,
    /// Human-readable note (skip reason, error, cancel reason).
    pub note: Option<String>,
    /// Captured contents of the node's declared artifact file (filled by the
    /// server after the node runs; `None` if undeclared or not produced).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub artifact_content: Option<String>,
    /// When the node started executing (`None` for skipped/cancelled nodes that
    /// never ran). Drives the UI elapsed-time badge and the task-overview
    /// waterfall.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub started_at: Option<DateTime<Utc>>,
    /// When the node finished executing (`None` if it never ran).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ended_at: Option<DateTime<Utc>>,
}

/// The static shape of one DAG node: its id and the nodes it depends on. The
/// run's *topology* (graph edges) — separate from per-node execution results —
/// so the UI can render the actual workflow graph, live and historical.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NodeMeta {
    pub id: String,
    pub depends_on: Vec<String>,
    /// Optional category id for overview grouping/colouring. Defaulted for
    /// back-compat with graphs persisted before categories existed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub category: Option<String>,
    /// Optional artifact path this node produces (relative to the run's
    /// artifacts dir), for the step popup. Back-compat defaulted.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub artifact: Option<String>,
    /// This node's output is what a Linear-triggered run posts to its issue
    /// on completion. Kept with the run, not read back from the workflow, so
    /// an edit to the workflow mid-run can't change what gets posted.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub linear_comment: bool,
}

/// The result of driving a workflow to completion.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RunReport {
    pub workflow: String,
    pub status: RunStatus,
    pub nodes: Vec<NodeRun>,
    /// Static DAG topology (one entry per declared node), so consumers can draw
    /// the graph without re-parsing the workflow. Defaulted for back-compat.
    #[serde(default)]
    pub graph: Vec<NodeMeta>,
}

impl RunReport {
    /// Look up a node's record by id.
    pub fn node(&self, id: &str) -> Option<&NodeRun> {
        self.nodes.iter().find(|n| n.id == id)
    }
}

/// A live event emitted while a run executes (for WS streaming + the UI's live
/// graph overlay). Serialized with a `type` tag.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum RunEvent {
    /// The run has started; `total_nodes` is the DAG's node count and `nodes`
    /// carries the static topology so the UI can draw the graph immediately.
    RunStarted {
        workflow: String,
        total_nodes: usize,
        #[serde(default)]
        nodes: Vec<NodeMeta>,
    },
    /// A node began executing (it is now "running").
    NodeStarted {
        node_id: String,
        provider: Option<String>,
        model: Option<String>,
    },
    /// A node reached a terminal state (success/failed/skipped/cancelled).
    NodeFinished { node: NodeRun },
    /// Live: the running node's latest activity — a tool call, a tool result,
    /// or a line of assistant text. Broadcast to the UI for the live graph
    /// overlay and appended to the durable activity log for replay.
    NodeProgress { node_id: String, activity: Activity },
    /// The run reached a terminal state.
    RunFinished { status: RunStatus },
}

/// Sink for [`RunEvent`]s. `None` disables streaming.
type Events<'a> = Option<&'a UnboundedSender<RunEvent>>;

fn emit(events: Events<'_>, event: RunEvent) {
    if let Some(tx) = events {
        // Receiver dropped (client disconnected) is not fatal to the run.
        let _ = tx.unbounded_send(event);
    }
}

/// Internal per-node execution result, carrying the record plus driver-relevant
/// side outputs (session to thread forward, cancellation request).
struct NodeRunResult {
    run: NodeRun,
    session: Option<String>,
    cancel: Option<String>,
}

/// Drive `workflow` to completion using `runner` for node execution and `vars`
/// for template substitution. Returns a [`RunReport`]; the only hard error is a
/// structural one (cycle) from layer computation — node failures are recorded
/// in the report, not returned as `Err`.
pub async fn run_workflow<R: NodeRunner>(
    workflow: &Workflow,
    runner: &R,
    vars: &VarContext,
) -> Result<RunReport, DagError> {
    run_workflow_streaming(workflow, runner, vars, None).await
}

/// Like [`run_workflow`], but emits [`RunEvent`]s to `events` as the run
/// progresses (run + node start/finish). Pass `None` to disable streaming.
pub async fn run_workflow_streaming<R: NodeRunner>(
    workflow: &Workflow,
    runner: &R,
    vars: &VarContext,
    events: Events<'_>,
) -> Result<RunReport, DagError> {
    let layers = topological_layers(workflow)?;
    let graph: Vec<NodeMeta> = workflow
        .nodes
        .iter()
        .map(|n| NodeMeta {
            id: n.id.clone(),
            depends_on: n.depends_on.clone(),
            category: n.category.clone(),
            artifact: n.artifact.clone(),
            linear_comment: n.linear_comment,
        })
        .collect();
    emit(
        events,
        RunEvent::RunStarted {
            workflow: workflow.name.clone(),
            total_nodes: workflow.nodes.len(),
            nodes: graph.clone(),
        },
    );
    let wf_provider = workflow.provider.as_deref();
    let wf_model = workflow.model.as_deref();

    let mut statuses: HashMap<String, NodeStatus> = HashMap::new();
    let mut runs: HashMap<String, NodeRun> = HashMap::new();
    // Accumulated upstream outputs, exposed to downstream nodes as
    // `$id.output…`. Earlier topological layers are fully finalized before a
    // later layer is built, so a node always sees its dependencies' outputs.
    let mut outputs: HashMap<String, String> = HashMap::new();
    let mut last_session: Option<String> = None;
    let mut cancelled: Option<String> = None;

    for layer in &layers {
        // Snapshot the variables for this whole layer: base vars + every output
        // produced so far. Built once — siblings in a parallel layer cannot
        // depend on each other, so they all share the same upstream view.
        let layer_vars = with_outputs(vars, &outputs);

        // Decide run/skip for each node using already-finalized dep statuses.
        let mut to_run: Vec<usize> = Vec::new();
        for &idx in layer {
            let node = &workflow.nodes[idx];
            if let Some(reason) = &cancelled {
                let run = skipped_run(
                    node,
                    format!("run cancelled: {reason}"),
                    NodeStatus::Cancelled,
                )
                .run;
                emit(events, RunEvent::NodeFinished { node: run.clone() });
                finalize(&mut statuses, &mut runs, &mut outputs, run);
            } else if !trigger_satisfied(workflow, node, &statuses) {
                let run = skipped_run(
                    node,
                    "dependencies not satisfied (trigger_rule)".to_string(),
                    NodeStatus::Skipped,
                )
                .run;
                emit(events, RunEvent::NodeFinished { node: run.clone() });
                finalize(&mut statuses, &mut runs, &mut outputs, run);
            } else {
                // Trigger satisfied — now apply the optional `when:` gate.
                match when_allows(node, &layer_vars) {
                    Ok(true) => to_run.push(idx),
                    Ok(false) => {
                        let run = skipped_run(
                            node,
                            "`when` condition evaluated false".to_string(),
                            NodeStatus::Skipped,
                        )
                        .run;
                        emit(events, RunEvent::NodeFinished { node: run.clone() });
                        finalize(&mut statuses, &mut runs, &mut outputs, run);
                    }
                    Err(e) => {
                        let run = failed_run(
                            node,
                            node.provider.as_deref(),
                            node.model.as_deref(),
                            e.to_string(),
                        );
                        emit(events, RunEvent::NodeFinished { node: run.clone() });
                        finalize(&mut statuses, &mut runs, &mut outputs, run);
                    }
                }
            }
        }

        if to_run.is_empty() {
            continue;
        }

        let single = to_run.len() == 1;
        let futures = to_run.iter().map(|&idx| {
            let node = &workflow.nodes[idx];
            // Sessions thread only through single-node (sequential) layers; a
            // parallel layer always starts each node fresh.
            let incoming = if single && node.context == ContextMode::Shared {
                last_session.clone()
            } else {
                None
            };
            execute_node(
                runner,
                node,
                wf_provider,
                wf_model,
                &layer_vars,
                incoming,
                events,
            )
        });
        let results = join_all(futures).await;

        // After a single-node layer the produced session threads forward; a
        // parallel layer leaves no single thread to continue, so reset.
        last_session = if single {
            results.first().and_then(|r| r.session.clone())
        } else {
            None
        };

        for result in results {
            if result.cancel.is_some() {
                cancelled = result.cancel.clone();
            }
            finalize(&mut statuses, &mut runs, &mut outputs, result.run);
        }
    }

    // Emit node records in declaration order.
    let nodes: Vec<NodeRun> = workflow
        .nodes
        .iter()
        .filter_map(|n| runs.remove(&n.id))
        .collect();

    let status = if cancelled.is_some() {
        RunStatus::Cancelled
    } else if nodes.iter().any(|n| n.status == NodeStatus::Failed) {
        RunStatus::Failed
    } else {
        RunStatus::Completed
    };

    emit(events, RunEvent::RunFinished { status });

    Ok(RunReport {
        workflow: workflow.name.clone(),
        status,
        nodes,
        graph,
    })
}

fn finalize(
    statuses: &mut HashMap<String, NodeStatus>,
    runs: &mut HashMap<String, NodeRun>,
    outputs: &mut HashMap<String, String>,
    run: NodeRun,
) {
    statuses.insert(run.id.clone(), run.status);
    // Expose this node's output to `$id.output…` in downstream nodes. Skipped /
    // cancelled nodes contribute an empty string so refs resolve (not error).
    outputs.insert(run.id.clone(), run.output.clone());
    runs.insert(run.id.clone(), run);
}

/// Clone the base variable context and layer in the outputs produced so far,
/// so node bodies and `when:` conditions can reference `$id.output…`.
fn with_outputs(base: &VarContext, outputs: &HashMap<String, String>) -> VarContext {
    let mut ctx = base.clone();
    for (id, out) in outputs {
        ctx = ctx.set_node_output(id.clone(), out.clone());
    }
    ctx
}

/// Evaluate a node's optional `when:` gate. Nodes without `when:` always pass.
fn when_allows(node: &Node, vars: &VarContext) -> Result<bool, DagError> {
    match &node.when {
        Some(expr) => crate::cond::eval_when(expr, vars),
        None => Ok(true),
    }
}

fn skipped_run(node: &Node, note: String, status: NodeStatus) -> NodeRunResult {
    NodeRunResult {
        run: NodeRun {
            id: node.id.clone(),
            status,
            provider: node.provider.clone(),
            model: node.model.clone(),
            output: String::new(),
            usage: Usage::default(),
            iterations: 0,
            converged: None,
            note: Some(note),
            artifact_content: None,
            started_at: None,
            ended_at: None,
        },
        session: None,
        cancel: None,
    }
}

/// Evaluate a node's `trigger_rule` against its dependencies' finalized
/// statuses. Dependencies are guaranteed finalized because they sit in strictly
/// earlier topological layers.
fn trigger_satisfied(
    workflow: &Workflow,
    node: &Node,
    statuses: &HashMap<String, NodeStatus>,
) -> bool {
    if node.depends_on.is_empty() {
        return true;
    }
    let dep_statuses: Vec<NodeStatus> = node
        .depends_on
        .iter()
        .map(|d| statuses.get(d).copied().unwrap_or(NodeStatus::Skipped))
        .collect();
    let any_success = dep_statuses.contains(&NodeStatus::Success);
    let any_failed = dep_statuses.contains(&NodeStatus::Failed);
    match node.trigger_rule {
        TriggerRule::AllSuccess => dep_statuses.iter().all(|s| *s == NodeStatus::Success),
        TriggerRule::OneSuccess => any_success,
        TriggerRule::NoneFailedMinOneSuccess => !any_failed && any_success,
        TriggerRule::AllDone => true,
        TriggerRule::OneFailed => upstream(workflow, node)
            .iter()
            .any(|id| statuses.get(id.as_str()) == Some(&NodeStatus::Failed)),
    }
}

/// Every node `node` depends on, directly or through its dependencies.
fn upstream(workflow: &Workflow, node: &Node) -> Vec<String> {
    let mut seen: Vec<String> = Vec::new();
    let mut stack: Vec<&str> = node.depends_on.iter().map(String::as_str).collect();
    while let Some(id) = stack.pop() {
        if seen.iter().any(|s| s == id) {
            continue;
        }
        seen.push(id.to_string());
        if let Some(dep) = workflow.node(id) {
            stack.extend(dep.depends_on.iter().map(String::as_str));
        }
    }
    seen
}

/// Resolve and execute a single node. Loop nodes iterate internally; cancel and
/// approval nodes are handled without invoking the runner.
async fn execute_node<R: NodeRunner>(
    runner: &R,
    node: &Node,
    wf_provider: Option<&str>,
    wf_model: Option<&str>,
    vars: &VarContext,
    incoming_session: Option<String>,
    events: Events<'_>,
) -> NodeRunResult {
    // Only agent-dispatching nodes carry a provider/model. bash/script/cancel/
    // approval nodes run no agent, so they show none — they don't inherit the
    // workflow default just for display.
    let is_agent = node.kind.is_ai();
    let provider = if is_agent {
        node.provider.as_deref().or(wf_provider)
    } else {
        None
    };
    let model = if is_agent {
        node.model.as_deref().or(wf_model)
    } else {
        None
    };
    emit(
        events,
        RunEvent::NodeStarted {
            node_id: node.id.clone(),
            provider: provider.map(str::to_string),
            model: model.map(str::to_string),
        },
    );

    let started = Utc::now();
    // Live progress sink for streaming runners: each call emits a NodeProgress
    // for this node over the event channel already threaded into the driver.
    // `None` when the run isn't streaming events (e.g. the plain CLI path).
    let progress = events.map(|tx| {
        let tx = tx.clone();
        let id = node.id.clone();
        ProgressSink(Arc::new(move |activity| {
            let _ = tx.unbounded_send(RunEvent::NodeProgress {
                node_id: id.clone(),
                activity,
            });
        }))
    });
    // `retries` re-runs *this node only* on failure (same worktree/session),
    // up to a hard ceiling. `retries: 0` (default) → a single attempt. A cancel
    // signal is never retried (it's a deliberate stop, not a failure).
    const MAX_NODE_RETRIES: u32 = 10;
    let max_attempts = node.retries.min(MAX_NODE_RETRIES).saturating_add(1);
    let mut attempt: u32 = 0;
    let mut result = loop {
        attempt += 1;
        let r = dispatch_node_body(
            runner,
            node,
            provider,
            model,
            vars,
            incoming_session.clone(),
            progress.clone(),
        )
        .await;
        if r.run.status != NodeStatus::Failed || r.cancel.is_some() || attempt >= max_attempts {
            break r;
        }
        emit(
            events,
            RunEvent::NodeProgress {
                node_id: node.id.clone(),
                activity: Activity::text(format!(
                    "step failed — retrying (attempt {}/{})",
                    attempt + 1,
                    max_attempts
                )),
            },
        );
    };
    if attempt > 1 && result.run.note.is_none() {
        result.run.note = Some(if result.run.status == NodeStatus::Success {
            format!("succeeded on attempt {attempt}/{max_attempts}")
        } else {
            format!("failed after {attempt} attempt(s)")
        });
    }
    // Stamp execution timing once, here, for every body kind.
    result.run.started_at = Some(started);
    result.run.ended_at = Some(Utc::now());
    emit(
        events,
        RunEvent::NodeFinished {
            node: result.run.clone(),
        },
    );
    result
}

/// Run a node's body exactly once: dispatch on its [`NodeKind`] and map the
/// runner result into a [`NodeRunResult`]. [`execute_node`] wraps this in a
/// retry loop for nodes with `retries > 0`.
#[allow(clippy::too_many_arguments)]
async fn dispatch_node_body<R: NodeRunner>(
    runner: &R,
    node: &Node,
    provider: Option<&str>,
    model: Option<&str>,
    vars: &VarContext,
    session: Option<String>,
    progress: Option<ProgressSink>,
) -> NodeRunResult {
    match &node.kind {
        NodeKind::Cancel(reason) => {
            // Substitute vars (e.g. `$upstream.output.summary`) in the reason.
            let reason = substitute(reason, vars).unwrap_or_else(|_| reason.clone());
            NodeRunResult {
                run: NodeRun {
                    id: node.id.clone(),
                    status: NodeStatus::Success,
                    provider: provider.map(str::to_string),
                    model: model.map(str::to_string),
                    output: reason.clone(),
                    usage: Usage::default(),
                    iterations: 1,
                    converged: None,
                    note: Some(format!("cancel: {reason}")),
                    artifact_content: None,
                    started_at: None,
                    ended_at: None,
                },
                session: None,
                cancel: Some(reason),
            }
        }

        NodeKind::Approval(cfg) => NodeRunResult {
            // Human gates need an input channel the driver does not yet have.
            run: skipped_run_inline(
                node,
                provider,
                model,
                format!("approval gate not yet supported: {}", cfg.message),
            ),
            session: None,
            cancel: None,
        },

        NodeKind::Route(cfg) => {
            run_route(runner, node, provider, model, vars, session, progress, cfg).await
        }
        NodeKind::Prompt(text) => {
            run_single_body(
                runner,
                node,
                provider,
                model,
                vars,
                session,
                progress,
                NodeBody::Prompt,
                text,
            )
            .await
        }
        NodeKind::Bash(text) => {
            run_single_body(
                runner,
                node,
                provider,
                model,
                vars,
                session,
                progress,
                NodeBody::Bash,
                text,
            )
            .await
        }
        NodeKind::Script {
            script,
            runtime,
            deps,
        } => {
            let runtime = *runtime;
            let deps = deps.clone();
            run_single_body(
                runner,
                node,
                provider,
                model,
                vars,
                session,
                progress,
                move |t| NodeBody::Script {
                    script: t,
                    runtime,
                    deps,
                },
                script,
            )
            .await
        }
        NodeKind::Command(name) => {
            // Command text is resolved by the runner, so no pre-substitution.
            execute_body(
                runner,
                node,
                provider,
                model,
                vars,
                session,
                progress,
                1,
                NodeBody::Command(name.clone()),
            )
            .await
        }

        NodeKind::Linear(action) => {
            // No issue to act on: a prebuilt workflow ships with these steps and
            // has to stay usable in a project that never connected Linear, and
            // in a run started by hand rather than from an issue. `ISSUE_ID` is
            // empty in both cases.
            //
            // A **no-op success**, deliberately, not `Skipped`. The default
            // trigger rule is `AllSuccess`, which requires every dependency to
            // be `Success` — so a skipped first step would skip the whole
            // workflow behind it, which is the opposite of carrying these steps
            // harmlessly. The note is what says nothing happened; authors
            // should not have to reach for `trigger_rule` to tolerate a Linear
            // step they didn't opt out of.
            if vars.get("ISSUE_ID").is_none_or(str::is_empty) {
                let note = format!(
                    "no Linear issue for this run — did not {}",
                    action.summary()
                );
                return NodeRunResult {
                    run: NodeRun {
                        id: node.id.clone(),
                        status: NodeStatus::Success,
                        provider: provider.map(str::to_string),
                        model: model.map(str::to_string),
                        output: note.clone(),
                        usage: Usage::default(),
                        iterations: 1,
                        converged: None,
                        note: Some(note),
                        artifact_content: None,
                        started_at: None,
                        ended_at: None,
                    },
                    session: None,
                    cancel: None,
                };
            }
            // Substituted here rather than in the runner, so a comment can
            // carry `$TASK_TITLE` or an upstream node's output like any other
            // body text.
            let sub = |t: &String| substitute(t, vars).unwrap_or_else(|_| t.clone());
            let action = match action {
                LinearAction::MoveState(s) => LinearAction::MoveState(sub(s)),
                LinearAction::Comment(s) => LinearAction::Comment(sub(s)),
                LinearAction::AddLabels(l) => LinearAction::AddLabels(l.iter().map(sub).collect()),
                LinearAction::RemoveLabels(l) => {
                    LinearAction::RemoveLabels(l.iter().map(sub).collect())
                }
            };
            execute_body(
                runner,
                node,
                provider,
                model,
                vars,
                session,
                progress,
                1,
                NodeBody::Linear(action),
            )
            .await
        }

        NodeKind::Loop(cfg) => {
            run_loop(runner, node, provider, model, vars, session, progress, cfg).await
        }
    }
}

/// Substitute a node's inline text, then run it once through the runner.
#[allow(clippy::too_many_arguments)]
async fn run_single_body<R, F>(
    runner: &R,
    node: &Node,
    provider: Option<&str>,
    model: Option<&str>,
    vars: &VarContext,
    incoming_session: Option<String>,
    progress: Option<ProgressSink>,
    make_body: F,
    raw_text: &str,
) -> NodeRunResult
where
    R: NodeRunner,
    F: FnOnce(String) -> NodeBody,
{
    let rendered = match substitute(raw_text, vars) {
        Ok(r) => r,
        Err(e) => {
            return NodeRunResult {
                run: failed_run(node, provider, model, e.to_string()),
                session: None,
                cancel: None,
            }
        }
    };
    execute_body(
        runner,
        node,
        provider,
        model,
        vars,
        incoming_session,
        progress,
        1,
        make_body(rendered),
    )
    .await
}

/// A route node's output: which route, why, and what decided it.
fn route_output(route: &str, reason: &str, by: &str) -> String {
    serde_json::json!({ "route": route, "reason": reason, "by": by }).to_string()
}

/// Decide a route node: rules in order, then the model, then the default.
#[allow(clippy::too_many_arguments)]
async fn run_route<R: NodeRunner>(
    runner: &R,
    node: &Node,
    provider: Option<&str>,
    model: Option<&str>,
    vars: &VarContext,
    session: Option<String>,
    progress: Option<ProgressSink>,
    cfg: &crate::model::RouteConfig,
) -> NodeRunResult {
    let decided = |route: &str, reason: String, by: &str, run_model: bool| NodeRunResult {
        run: NodeRun {
            id: node.id.clone(),
            status: NodeStatus::Success,
            provider: run_model.then(|| provider.map(str::to_string)).flatten(),
            model: run_model.then(|| model.map(str::to_string)).flatten(),
            output: route_output(route, &reason, by),
            usage: Usage::default(),
            iterations: 1,
            converged: None,
            note: Some(format!("route: {route}")),
            artifact_content: None,
            started_at: None,
            ended_at: None,
        },
        session: None,
        cancel: None,
    };

    // 1. Rules, in order: certain, and no model call.
    for r in &cfg.routes {
        let Some(expr) = &r.when else { continue };
        match crate::cond::eval_when(expr, vars) {
            Ok(true) => return decided(&r.name, format!("`{expr}` held"), "rule", false),
            Ok(false) => {}
            Err(e) => {
                return NodeRunResult {
                    run: failed_run(node, provider, model, format!("route `{}`: {e}", r.name)),
                    session: None,
                    cancel: None,
                }
            }
        }
    }

    // 2. A model chooses from the descriptions.
    if let Some(prompt) = &cfg.prompt {
        // Only the author's prompt substitutes; the routes are appended after,
        // so a `$` in a description can't break it.
        let rendered = match substitute(prompt, vars) {
            Ok(r) => r,
            Err(e) => {
                return NodeRunResult {
                    run: failed_run(node, provider, model, e.to_string()),
                    session: None,
                    cancel: None,
                }
            }
        };
        let mut result = execute_body(
            runner,
            node,
            provider,
            model,
            vars,
            session,
            progress,
            1,
            NodeBody::Prompt(format!("{rendered}{}", cfg.choices())),
        )
        .await;
        if result.run.status != NodeStatus::Success {
            return result;
        }
        match cfg.read_choice(&result.run.output) {
            Some((route, reason)) => {
                result.run.output = route_output(&route, &reason, "model");
                result.run.note = Some(format!("route: {route}"));
            }
            None => match &cfg.default {
                Some(d) => {
                    result.run.output =
                        route_output(d, "the model's answer named no route", "default");
                    result.run.note = Some(format!("route: {d} (the model named none)"));
                }
                None => {
                    result.run.status = NodeStatus::Failed;
                    result.run.note = Some(format!(
                        "the model's answer named none of: {}",
                        cfg.names().join(", ")
                    ));
                }
            },
        }
        return result;
    }

    // 3. The default.
    match &cfg.default {
        Some(d) => decided(d, "no rule matched".into(), "default", false),
        None => NodeRunResult {
            run: failed_run(node, provider, model, "no route's rule matched".into()),
            session: None,
            cancel: None,
        },
    }
}

/// Invoke the runner once and map its result into a [`NodeRunResult`].
#[allow(clippy::too_many_arguments)]
async fn execute_body<R: NodeRunner>(
    runner: &R,
    node: &Node,
    provider: Option<&str>,
    model: Option<&str>,
    vars: &VarContext,
    incoming_session: Option<String>,
    progress: Option<ProgressSink>,
    iteration: u32,
    body: NodeBody,
) -> NodeRunResult {
    let (result, limit_note) = execute_through_limits(runner, &progress, || NodeRequest {
        node_id: &node.id,
        provider,
        model,
        effort: node.effort.as_deref(),
        context: node.context,
        session: incoming_session.clone(),
        iteration,
        body: body.clone(),
        timeout: node.timeout,
        vars,
        output_format: node.output_format.as_ref(),
        hooks: node.hooks.as_ref(),
        mcp: &node.mcp,
        progress: progress.clone(),
    })
    .await;
    let mut mapped = map_body_result(node, provider, model, iteration, result);
    mapped.run.note = with_limit_note(limit_note, mapped.run.note.take());
    mapped
}

/// Put a usage-limit note ahead of whatever note the step already has.
fn with_limit_note(limit: Option<String>, existing: Option<String>) -> Option<String> {
    match (limit, existing) {
        (Some(limit), Some(existing)) => Some(format!("{limit}: {existing}")),
        (limit, existing) => limit.or(existing),
    }
}

/// Run one invocation, and when it fails on a usage limit that resets soon
/// enough, wait for the reset and run it again. `make_req` builds the same
/// request each time. Returns the last result and, when a limit was involved,
/// a note saying what happened (waited, or why it did not).
async fn execute_through_limits<'a, R, F>(
    runner: &R,
    progress: &Option<ProgressSink>,
    make_req: F,
) -> (Result<NodeOutput, RunnerError>, Option<String>)
where
    R: NodeRunner,
    F: Fn() -> NodeRequest<'a>,
{
    let mut waited_until: Option<DateTime<Utc>> = None;
    let mut waits = 0u32;
    loop {
        let result = runner.execute(make_req()).await;
        let failure = match &result {
            Ok(out) if !out.success => Some(out.text.as_str()),
            Err(e) => Some(e.0.as_str()),
            Ok(_) => None,
        };
        let now = Utc::now();
        let Some(reset) = failure.and_then(|text| crate::usage_limit::reset_time(text, now)) else {
            let note = waited_until
                .map(|until| format!("waited for a usage limit to reset (until {})", clock(until)));
            return (result, note);
        };
        // Resets are approximate; starting a minute late beats hitting the
        // limit again and spending another wait.
        let wait = (reset - now).max(chrono::Duration::zero()) + chrono::Duration::minutes(1);
        let max_wait = max_usage_limit_wait();
        if wait > max_wait {
            let note = format!(
                "usage limit reached; it resets at {}, later than the {} this run waits",
                clock(reset),
                span(max_wait)
            );
            return (result, Some(note));
        }
        if waits >= MAX_USAGE_LIMIT_WAITS {
            let note = format!("usage limit reached again after {waits} waits; giving up");
            return (result, Some(note));
        }
        waits += 1;
        let resume = now + wait;
        if let Some(sink) = progress {
            sink.report(Activity::text(format!(
                "Usage limit reached. Waiting until {} ({}), then running this step again.",
                clock(resume),
                span(wait)
            )));
        }
        tokio::time::sleep(wait.to_std().unwrap_or_default()).await;
        if let Some(sink) = progress {
            sink.report(Activity::text(
                "Usage limit reset. Running this step again.",
            ));
        }
        waited_until = Some(resume);
    }
}

/// How many times one step invocation waits for a usage limit before it fails.
/// A limit that is still hit after several resets is not one waiting will fix.
const MAX_USAGE_LIMIT_WAITS: u32 = 3;

/// The longest a step waits for a usage limit to reset. Covers Claude's
/// 5-hour window; a weekly limit is longer and fails the step as before.
/// `HARNESS_USAGE_LIMIT_MAX_WAIT_MINS` overrides it (`0` disables waiting).
fn max_usage_limit_wait() -> chrono::Duration {
    const DEFAULT_MINS: i64 = 6 * 60;
    let mins = std::env::var("HARNESS_USAGE_LIMIT_MAX_WAIT_MINS")
        .ok()
        .and_then(|v| v.trim().parse::<i64>().ok())
        .filter(|m| *m >= 0)
        .unwrap_or(DEFAULT_MINS);
    chrono::Duration::minutes(mins)
}

/// `14:05 CET`, with the date when it is not today, in the same zone the
/// Linear comments use: `HARNESS_DISPLAY_TZ`, else Danish time (see
/// `display_tz` in harness-server's `linear_poller`).
fn clock(t: DateTime<Utc>) -> String {
    let tz = std::env::var("HARNESS_DISPLAY_TZ")
        .ok()
        .and_then(|name| name.parse().ok())
        .unwrap_or(chrono_tz::Europe::Copenhagen);
    clock_in(t, Utc::now(), tz)
}

fn clock_in(t: DateTime<Utc>, now: DateTime<Utc>, tz: chrono_tz::Tz) -> String {
    let local = t.with_timezone(&tz);
    if local.date_naive() == now.with_timezone(&tz).date_naive() {
        local.format("%H:%M %Z").to_string()
    } else {
        local.format("%Y-%m-%d %H:%M %Z").to_string()
    }
}

/// `2h 13m`, or `45m`.
fn span(d: chrono::Duration) -> String {
    let mins = (d.num_seconds() + 59) / 60;
    match (mins / 60, mins % 60) {
        (0, m) => format!("{m}m"),
        (h, 0) => format!("{h}h"),
        (h, m) => format!("{h}h {m}m"),
    }
}

/// Map one runner invocation's result into a [`NodeRunResult`].
fn map_body_result(
    node: &Node,
    provider: Option<&str>,
    model: Option<&str>,
    iteration: u32,
    result: Result<NodeOutput, RunnerError>,
) -> NodeRunResult {
    match result {
        Ok(out) => {
            let status = if out.success {
                NodeStatus::Success
            } else {
                NodeStatus::Failed
            };
            NodeRunResult {
                run: NodeRun {
                    id: node.id.clone(),
                    status,
                    provider: provider.map(str::to_string),
                    model: model.map(str::to_string),
                    output: out.text,
                    usage: out.usage,
                    iterations: iteration,
                    converged: None,
                    note: None,
                    artifact_content: None,
                    started_at: None,
                    ended_at: None,
                },
                session: out.session,
                cancel: None,
            }
        }
        Err(e) => NodeRunResult {
            run: failed_run(node, provider, model, e.to_string()),
            session: None,
            cancel: None,
        },
    }
}

/// Drive a loop node: re-run its prompt until the `until` signal converges or
/// `max_iterations` is reached.
#[allow(clippy::too_many_arguments)]
async fn run_loop<R: NodeRunner>(
    runner: &R,
    node: &Node,
    provider: Option<&str>,
    model: Option<&str>,
    vars: &VarContext,
    incoming_session: Option<String>,
    progress: Option<ProgressSink>,
    cfg: &crate::model::LoopConfig,
) -> NodeRunResult {
    if cfg.until.trim().is_empty() {
        return NodeRunResult {
            run: failed_run(
                node,
                provider,
                model,
                "loop `until` signal is empty".to_string(),
            ),
            session: None,
            cancel: None,
        };
    }

    // A loop block may declare its own provider/model; prefer those over the
    // node/workflow defaults for every iteration (and the recorded NodeRun).
    let provider = cfg.provider.as_deref().or(provider);
    let model = cfg.model.as_deref().or(model);

    let mut usage = Usage::default();
    let mut last_text = String::new();
    let mut session = incoming_session;
    let mut converged = false;
    let mut iterations = 0u32;
    // The latest usage-limit note from any iteration, kept for the node's note.
    let mut limit_note: Option<String> = None;

    for i in 1..=cfg.max_iterations {
        iterations = i;
        // Surface the loop position live (e.g. "🔁 2/5") so the UI can render an
        // iteration badge. `max_iterations` is a ceiling — the loop ends early
        // once `until` fires — so the UI frames the total as a max, not a target.
        if let Some(p) = &progress {
            p.report(Activity::text(format!("🔁 {i}/{}", cfg.max_iterations)));
        }
        // Expose the previous iteration's output to the prompt.
        let iter_vars = vars.clone().set("LOOP_PREV_OUTPUT", last_text.clone());
        let rendered = match substitute(&cfg.prompt, &iter_vars) {
            Ok(r) => r,
            Err(e) => {
                return NodeRunResult {
                    run: failed_run(node, provider, model, e.to_string()),
                    session: None,
                    cancel: None,
                }
            }
        };

        let iter_session = if cfg.fresh_context {
            None
        } else {
            session.clone()
        };
        let (result, iter_note) = execute_through_limits(runner, &progress, || NodeRequest {
            node_id: &node.id,
            provider,
            model,
            effort: node.effort.as_deref(),
            context: node.context,
            session: iter_session.clone(),
            iteration: i,
            body: NodeBody::Prompt(rendered.clone()),
            timeout: node.timeout,
            vars: &iter_vars,
            output_format: node.output_format.as_ref(),
            hooks: node.hooks.as_ref(),
            mcp: &node.mcp,
            progress: progress.clone(),
        })
        .await;
        if iter_note.is_some() {
            limit_note = iter_note;
        }

        match result {
            Ok(out) => {
                usage.add(&out.usage);
                last_text = out.text;
                session = out.session;
                if !out.success {
                    return NodeRunResult {
                        run: NodeRun {
                            id: node.id.clone(),
                            status: NodeStatus::Failed,
                            provider: provider.map(str::to_string),
                            model: model.map(str::to_string),
                            output: last_text,
                            usage,
                            iterations,
                            converged: Some(false),
                            note: with_limit_note(
                                limit_note,
                                Some("loop iteration failed".to_string()),
                            ),
                            artifact_content: None,
                            started_at: None,
                            ended_at: None,
                        },
                        session: None,
                        cancel: None,
                    };
                }
                if detect_signal(&last_text, &cfg.until) {
                    converged = true;
                    break;
                }
                // Optional secondary completion check: a shell command whose
                // exit 0 ends the loop. Run via the runner's Bash path so it
                // executes in the same environment (no extra trait method).
                if let Some(until_bash) = &cfg.until_bash {
                    let rendered = match substitute(until_bash, &iter_vars) {
                        Ok(r) => r,
                        Err(e) => {
                            return NodeRunResult {
                                run: failed_run(node, provider, model, e.to_string()),
                                session: None,
                                cancel: None,
                            }
                        }
                    };
                    let bash_req = NodeRequest {
                        node_id: &node.id,
                        provider,
                        model,
                        effort: None,
                        context: node.context,
                        session: None,
                        iteration: i,
                        body: NodeBody::Bash(rendered),
                        timeout: node.timeout,
                        vars: &iter_vars,
                        output_format: None,
                        hooks: None,
                        mcp: &[],
                        progress: None,
                    };
                    match runner.execute(bash_req).await {
                        Ok(check) => {
                            if check.success {
                                converged = true;
                                break;
                            }
                        }
                        Err(e) => {
                            return NodeRunResult {
                                run: failed_run(node, provider, model, e.to_string()),
                                session: None,
                                cancel: None,
                            }
                        }
                    }
                }
            }
            Err(e) => {
                let note = with_limit_note(limit_note, Some(e.to_string()));
                return NodeRunResult {
                    run: NodeRun {
                        note,
                        ..failed_run(node, provider, model, String::new())
                    },
                    session: None,
                    cancel: None,
                };
            }
        }
    }

    let note = if converged {
        None
    } else {
        Some(format!(
            "loop reached max_iterations ({}) without `{}`",
            cfg.max_iterations, cfg.until
        ))
    };
    let note = with_limit_note(limit_note, note);
    NodeRunResult {
        run: NodeRun {
            id: node.id.clone(),
            status: NodeStatus::Success,
            provider: provider.map(str::to_string),
            model: model.map(str::to_string),
            output: last_text,
            usage,
            iterations,
            converged: Some(converged),
            note,
            artifact_content: None,
            started_at: None,
            ended_at: None,
        },
        session,
        cancel: None,
    }
}

fn failed_run(node: &Node, provider: Option<&str>, model: Option<&str>, note: String) -> NodeRun {
    NodeRun {
        id: node.id.clone(),
        status: NodeStatus::Failed,
        provider: provider.map(str::to_string),
        model: model.map(str::to_string),
        output: String::new(),
        usage: Usage::default(),
        iterations: 1,
        converged: None,
        note: Some(note),
        artifact_content: None,
        started_at: None,
        ended_at: None,
    }
}

fn skipped_run_inline(
    node: &Node,
    provider: Option<&str>,
    model: Option<&str>,
    note: String,
) -> NodeRun {
    NodeRun {
        id: node.id.clone(),
        status: NodeStatus::Skipped,
        provider: provider.map(str::to_string),
        model: model.map(str::to_string),
        output: String::new(),
        usage: Usage::default(),
        iterations: 0,
        converged: None,
        note: Some(note),
        artifact_content: None,
        started_at: None,
        ended_at: None,
    }
}

#[cfg(test)]
mod tests;
