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

use async_trait::async_trait;
use futures::future::join_all;
use serde::{Deserialize, Serialize};

use crate::error::DagError;
use crate::graph::topological_layers;
use crate::model::{ContextMode, Node, NodeKind, ScriptRuntime, TriggerRule, Workflow};
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
}

/// One execution request for a single node body invocation.
pub struct NodeRequest<'a> {
    pub node_id: &'a str,
    pub provider: Option<&'a str>,
    pub model: Option<&'a str>,
    pub context: ContextMode,
    /// Session id to continue (shared context / loop threading), if any.
    pub session: Option<String>,
    /// 1-based iteration; `> 1` only for loop bodies.
    pub iteration: u32,
    pub body: NodeBody,
    pub timeout: Option<u64>,
    /// Variable context, for `Command`/`Script` resolution by the runner.
    pub vars: &'a VarContext,
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
}

/// The result of driving a workflow to completion.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RunReport {
    pub workflow: String,
    pub status: RunStatus,
    pub nodes: Vec<NodeRun>,
}

impl RunReport {
    /// Look up a node's record by id.
    pub fn node(&self, id: &str) -> Option<&NodeRun> {
        self.nodes.iter().find(|n| n.id == id)
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
    let layers = topological_layers(workflow)?;
    let wf_provider = workflow.provider.as_deref();
    let wf_model = workflow.model.as_deref();

    let mut statuses: HashMap<String, NodeStatus> = HashMap::new();
    let mut runs: HashMap<String, NodeRun> = HashMap::new();
    let mut last_session: Option<String> = None;
    let mut cancelled: Option<String> = None;

    for layer in &layers {
        // Decide run/skip for each node using already-finalized dep statuses.
        let mut to_run: Vec<usize> = Vec::new();
        for &idx in layer {
            let node = &workflow.nodes[idx];
            if let Some(reason) = &cancelled {
                finalize(
                    &mut statuses,
                    &mut runs,
                    skipped_run(
                        node,
                        format!("run cancelled: {reason}"),
                        NodeStatus::Cancelled,
                    )
                    .run,
                );
            } else if trigger_satisfied(node, &statuses) {
                to_run.push(idx);
            } else {
                finalize(
                    &mut statuses,
                    &mut runs,
                    skipped_run(
                        node,
                        "dependencies not satisfied (trigger_rule)".to_string(),
                        NodeStatus::Skipped,
                    )
                    .run,
                );
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
            execute_node(runner, node, wf_provider, wf_model, vars, incoming)
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
            finalize(&mut statuses, &mut runs, result.run);
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

    Ok(RunReport {
        workflow: workflow.name.clone(),
        status,
        nodes,
    })
}

fn finalize(
    statuses: &mut HashMap<String, NodeStatus>,
    runs: &mut HashMap<String, NodeRun>,
    run: NodeRun,
) {
    statuses.insert(run.id.clone(), run.status);
    runs.insert(run.id.clone(), run);
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
        },
        session: None,
        cancel: None,
    }
}

/// Evaluate a node's `trigger_rule` against its dependencies' finalized
/// statuses. Dependencies are guaranteed finalized because they sit in strictly
/// earlier topological layers.
fn trigger_satisfied(node: &Node, statuses: &HashMap<String, NodeStatus>) -> bool {
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
    }
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
) -> NodeRunResult {
    let provider = node.provider.as_deref().or(wf_provider);
    let model = node.model.as_deref().or(wf_model);

    match &node.kind {
        NodeKind::Cancel(reason) => NodeRunResult {
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
            },
            session: None,
            cancel: Some(reason.clone()),
        },

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

        NodeKind::Prompt(text) => {
            run_single_body(
                runner,
                node,
                provider,
                model,
                vars,
                incoming_session,
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
                incoming_session,
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
                incoming_session,
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
                incoming_session,
                1,
                NodeBody::Command(name.clone()),
            )
            .await
        }

        NodeKind::Loop(cfg) => {
            run_loop(runner, node, provider, model, vars, incoming_session, cfg).await
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
        1,
        make_body(rendered),
    )
    .await
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
    iteration: u32,
    body: NodeBody,
) -> NodeRunResult {
    let req = NodeRequest {
        node_id: &node.id,
        provider,
        model,
        context: node.context,
        session: incoming_session,
        iteration,
        body,
        timeout: node.timeout,
        vars,
    };
    match runner.execute(req).await {
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
async fn run_loop<R: NodeRunner>(
    runner: &R,
    node: &Node,
    provider: Option<&str>,
    model: Option<&str>,
    vars: &VarContext,
    incoming_session: Option<String>,
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

    let mut usage = Usage::default();
    let mut last_text = String::new();
    let mut session = incoming_session;
    let mut converged = false;
    let mut iterations = 0u32;

    for i in 1..=cfg.max_iterations {
        iterations = i;
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

        let req = NodeRequest {
            node_id: &node.id,
            provider,
            model,
            context: node.context,
            session: if cfg.fresh_context {
                None
            } else {
                session.clone()
            },
            iteration: i,
            body: NodeBody::Prompt(rendered),
            timeout: node.timeout,
            vars: &iter_vars,
        };

        match runner.execute(req).await {
            Ok(out) => {
                usage.add(&out.usage);
                last_text = out.text;
                session = out.session;
                if !out.success {
                    return NodeRunResult {
                        run: loop_run(
                            node,
                            provider,
                            model,
                            NodeStatus::Failed,
                            last_text,
                            usage,
                            iterations,
                            false,
                            Some("loop iteration failed".to_string()),
                        ),
                        session: None,
                        cancel: None,
                    };
                }
                if detect_signal(&last_text, &cfg.until) {
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

    let note = if converged {
        None
    } else {
        Some(format!(
            "loop reached max_iterations ({}) without `{}`",
            cfg.max_iterations, cfg.until
        ))
    };
    NodeRunResult {
        run: loop_run(
            node,
            provider,
            model,
            NodeStatus::Success,
            last_text,
            usage,
            iterations,
            converged,
            note,
        ),
        session,
        cancel: None,
    }
}

#[allow(clippy::too_many_arguments)]
fn loop_run(
    node: &Node,
    provider: Option<&str>,
    model: Option<&str>,
    status: NodeStatus,
    output: String,
    usage: Usage,
    iterations: u32,
    converged: bool,
    note: Option<String>,
) -> NodeRun {
    NodeRun {
        id: node.id.clone(),
        status,
        provider: provider.map(str::to_string),
        model: model.map(str::to_string),
        output,
        usage,
        iterations,
        converged: Some(converged),
        note,
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
    }
}

#[cfg(test)]
mod tests;
