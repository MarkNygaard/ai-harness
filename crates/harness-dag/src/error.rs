//! Error types for DAG parsing, validation, scheduling, and variable
//! substitution.

use thiserror::Error;

/// Errors produced while loading, validating, scheduling, or rendering a
/// workflow DAG.
#[derive(Debug, Error)]
pub enum DagError {
    /// The YAML could not be deserialized into the raw workflow shape.
    #[error("failed to parse workflow YAML: {0}")]
    Yaml(#[from] serde_yaml::Error),

    /// Two nodes share the same `id`.
    #[error("duplicate node id: `{0}`")]
    DuplicateNodeId(String),

    /// A node's `depends_on` references an id that is not defined.
    #[error("node `{node}` depends on unknown node `{dep}`")]
    UnknownDependency { node: String, dep: String },

    /// A node body or `when:` expression references `$id.output` for an id that
    /// is not a declared node.
    #[error("node `{node}` references output of unknown node `{referenced}`")]
    UnknownNodeReference { node: String, referenced: String },

    /// A `when:` expression is structurally invalid (e.g. a dangling operator).
    #[error("invalid `when` condition `{expr}`: {reason}")]
    InvalidCondition { expr: String, reason: String },

    /// A node declares no executable body (no `prompt`/`bash`/`command`/…).
    #[error("node `{0}` has no body: expected exactly one of prompt, bash, command, script, loop, approval, cancel, linear, route")]
    NoNodeKind(String),

    /// A node declares more than one mutually exclusive body.
    #[error("node `{node}` has conflicting bodies: {found:?} (exactly one allowed)")]
    MultipleNodeKinds {
        node: String,
        found: Vec<&'static str>,
    },

    /// More than one node asks for its output to be posted to Linear.
    #[error("only one node may set `linear_comment: true`; found: {0:?}")]
    MultipleLinearComments(Vec<String>),

    /// A node's `mcp:` entry is not a usable server name.
    #[error(
        "node `{node}` asks for MCP server `{name}`, which is not a valid server name \
         (lowercase letters, digits, `-` and `_`)"
    )]
    InvalidMcpServerName { node: String, name: String },

    /// A node's `effort` is not one of [`crate::model::EFFORT_LEVELS`]. Checked
    /// here because the CLIs take it as given: a typo reached
    /// `claude --effort` as it was written.
    #[error("node `{node}` has effort `{value}`; expected one of: low, medium, high, xhigh, max")]
    InvalidEffort { node: String, value: String },

    /// A `script` node is missing its required `runtime`.
    #[error("script node `{0}` is missing `runtime` (expected `bun` or `uv`)")]
    ScriptMissingRuntime(String),

    /// The dependency graph contains a cycle; the listed nodes could not be
    /// scheduled.
    #[error("workflow has a dependency cycle involving: {0:?}")]
    Cycle(Vec<String>),

    /// A template referenced a recognized variable that has no value in the
    /// current context.
    #[error("template references variable `${0}` which is not available in this context")]
    MissingVariable(String),

    /// A loop's `until` signal is empty.
    #[error("loop completion signal is empty")]
    EmptySignal,

    /// A workflow's `ui.report.verdict_node` names a node that doesn't exist.
    #[error("workflow `ui.report.verdict_node` references unknown node `{0}`")]
    ReportVerdictNodeUnknown(String),

    /// A route node that can't always choose a route, or names its routes
    /// badly.
    #[error("route node `{node}`: {reason}")]
    InvalidRoute { node: String, reason: String },

    /// `improvements.every` outside [`crate::model::IMPROVEMENTS_EVERY`].
    #[error("`improvements.every` is {0}; it must be between 3 and 100 runs")]
    ImprovementsEvery(u32),
}
