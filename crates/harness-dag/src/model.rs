//! The workflow DAG model.
//!
//! A [`Workflow`] is a list of [`Node`]s connected by `depends_on` edges. Each
//! node carries optional AI options (provider/model/context) and exactly one
//! executable *body* ([`NodeKind`]): a prompt, a bash script, a referenced
//! command, an inline script, a convergence loop, a human approval gate, or a
//! cancel.
//!
//! The format is intentionally close to Archon's YAML so existing workflows
//! port with minimal edits, but it is typed and validated in Rust. Parsing and
//! validation live in [`crate::parse`]; this module is the data model only.

use serde::{Deserialize, Serialize};

/// Session-handling mode for an AI node.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ContextMode {
    /// Always run with a brand-new agent session (parallelizable).
    Fresh,
    /// Inherit the session from the previous sequential node (default).
    #[default]
    Shared,
}

/// How a node reacts to the terminal states of its upstream dependencies.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TriggerRule {
    /// Run only if every dependency succeeded (default).
    #[default]
    AllSuccess,
    /// Run if at least one dependency succeeded.
    OneSuccess,
    /// Run if no dependency failed and at least one succeeded.
    NoneFailedMinOneSuccess,
    /// Run once every dependency reached a terminal state, regardless of result.
    AllDone,
}

/// Runtime for an inline [`NodeKind::Script`] body.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ScriptRuntime {
    /// TypeScript/JavaScript via `bun`.
    Bun,
    /// Python via `uv`.
    Uv,
}

/// Configuration for a convergence [`NodeKind::Loop`].
///
/// The loop re-runs `prompt` until the agent emits the `until` signal (see
/// [`crate::vars`] consumers / the executor for detection) or `max_iterations`
/// is reached.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LoopConfig {
    /// Prompt executed each iteration.
    pub prompt: String,
    /// Completion signal string; emitting it (e.g. wrapped in a tag) ends the loop.
    pub until: String,
    /// Hard cap on iterations.
    pub max_iterations: u32,
    /// Provider for the loop body, overriding the node/workflow provider when set.
    /// Archon-style workflows declare provider/model *inside* the `loop:` block.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider: Option<String>,
    /// Model for the loop body, overriding the node/workflow model when set.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    /// Start each iteration with a fresh session instead of reusing the loop's.
    #[serde(default)]
    pub fresh_context: bool,
    /// Optional shell command run after each iteration; exit 0 also ends the loop.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub until_bash: Option<String>,
    /// Pause for user input between iterations.
    #[serde(default)]
    pub interactive: bool,
    /// Message shown when paused (used with `interactive`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub gate_message: Option<String>,
}

/// Configuration for a human [`NodeKind::Approval`] gate.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ApprovalConfig {
    /// Message presented to the human approver.
    pub message: String,
    /// Whether the approver's free-text response is captured for downstream use.
    #[serde(default)]
    pub capture_response: bool,
    /// Node id to route to if the human rejects (optional).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub on_reject: Option<String>,
}

/// The executable body of a node. Exactly one variant per node.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NodeKind {
    /// Inline AI prompt.
    Prompt(String),
    /// Shell script body.
    Bash(String),
    /// Reference to a markdown command resolved from `.harness/commands/`.
    Command(String),
    /// Inline script with an explicit runtime and optional dependencies.
    Script {
        script: String,
        runtime: ScriptRuntime,
        deps: Vec<String>,
    },
    /// Convergence loop.
    Loop(LoopConfig),
    /// Human approval gate.
    Approval(ApprovalConfig),
    /// Terminate the run with a reason.
    Cancel(String),
}

impl NodeKind {
    /// Whether this body is executed by an AI provider (vs. deterministic).
    pub fn is_ai(&self) -> bool {
        matches!(
            self,
            NodeKind::Prompt(_) | NodeKind::Command(_) | NodeKind::Loop(_)
        )
    }
}

/// A single node in the workflow DAG.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Node {
    /// Unique node identifier within the workflow.
    pub id: String,
    /// Ids of nodes that must reach a terminal state before this one runs.
    pub depends_on: Vec<String>,
    /// Optional conditional-execution expression (evaluated by the executor).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub when: Option<String>,
    /// How this node reacts to its dependencies' terminal states.
    pub trigger_rule: TriggerRule,
    /// Provider override (falls back to the workflow default, then config).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub provider: Option<String>,
    /// Model override.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    /// Session-handling mode for AI bodies.
    pub context: ContextMode,
    /// Timeout in milliseconds (for `bash`/`script` bodies).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub timeout: Option<u64>,
    /// The executable body.
    pub kind: NodeKind,
}

/// A parsed, validated workflow DAG.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Workflow {
    /// Workflow name.
    pub name: String,
    /// Optional human description.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    /// Workflow-level default provider (overridden per node).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub provider: Option<String>,
    /// Workflow-level default model.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    /// Nodes in declaration order.
    pub nodes: Vec<Node>,
}

impl Workflow {
    /// Look up a node by id.
    pub fn node(&self, id: &str) -> Option<&Node> {
        self.nodes.iter().find(|n| n.id == id)
    }
}
