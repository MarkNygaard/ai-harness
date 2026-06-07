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

/// Provider-agnostic per-node tool hooks (Archon-shaped). Translated by the
/// runner into Claude Code settings.json hooks or the omp hook extension.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct NodeHooks {
    /// Fired before a tool runs; a `deny` decision blocks the call.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub pre_tool_use: Vec<HookRule>,
    /// Fired after a tool runs; used for non-blocking steering / context.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub post_tool_use: Vec<HookRule>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HookRule {
    /// Regex over the tool name (e.g. "Write|Edit", "Bash", "Read").
    /// Absent/empty = match every tool.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub matcher: Option<String>,
    /// Decision for matching tools (pre_tool_use only). `None` = observe/inject.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub decision: Option<HookDecision>,
    /// Reason surfaced to the agent when denied.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    /// Extra context injected for the agent (non-blocking).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub additional_context: Option<String>,
    /// System-level message surfaced to the user/agent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub system_message: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HookDecision {
    Allow,
    Deny,
    Ask,
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
    /// Optional category id (e.g. `planning`, `implementation`, `validation`)
    /// grouping this step for the run overview's time-by-category breakdown and
    /// bar colouring. Free-form; resolved to a colour via the categories
    /// registry at render time. `None` → the node falls back to status colour.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub category: Option<String>,
    /// Optional artifact this node produces — a path relative to the run's
    /// artifacts dir (e.g. `exploration.md`). Surfaced in the step popup.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub artifact: Option<String>,
    /// Timeout in milliseconds (for `bash`/`script` bodies).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub timeout: Option<u64>,
    /// Optional JSON schema the AI body's output should conform to. When set on a
    /// `prompt`/`command` node the runner instructs the agent to emit JSON
    /// matching it, so downstream `$node.output.field` access and `when:`
    /// conditions read a stable shape. Ignored for deterministic bodies.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub output_format: Option<serde_json::Value>,
    /// Provider-agnostic tool hooks translated per provider at dispatch.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hooks: Option<NodeHooks>,
    /// The executable body.
    pub kind: NodeKind,
}

impl Node {
    /// Every inline text this node contributes to variable substitution: its
    /// body text plus its `when:` expression. Used to collect `$id.output`
    /// references for validation.
    pub fn substitutable_text(&self) -> Vec<&str> {
        let mut texts: Vec<&str> = Vec::new();
        if let Some(w) = &self.when {
            texts.push(w);
        }
        match &self.kind {
            NodeKind::Prompt(t) | NodeKind::Bash(t) | NodeKind::Cancel(t) => texts.push(t),
            NodeKind::Script { script, .. } => texts.push(script),
            NodeKind::Loop(cfg) => {
                texts.push(&cfg.prompt);
                if let Some(b) = &cfg.until_bash {
                    texts.push(b);
                }
            }
            // `command` is a file reference (resolved by the runner); `approval`
            // carries a human message, not substitutable node refs.
            NodeKind::Command(_) | NodeKind::Approval(_) => {}
        }
        texts
    }
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
