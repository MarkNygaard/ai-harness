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
    #[error("node `{0}` has no body: expected exactly one of prompt, bash, command, script, loop, approval, cancel")]
    NoNodeKind(String),

    /// A node declares more than one mutually exclusive body.
    #[error("node `{node}` has conflicting bodies: {found:?} (exactly one allowed)")]
    MultipleNodeKinds {
        node: String,
        found: Vec<&'static str>,
    },

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
}
