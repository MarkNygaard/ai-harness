//! # harness-dag
//!
//! The workflow DAG model, YAML loader, topological scheduler, and
//! template-variable substitution for the Harness orchestration layer.
//!
//! The format is Archon-inspired (see `docs/PLAN.md` §6) but typed and
//! validated in Rust. This crate is execution-agnostic: it turns YAML into a
//! validated [`Workflow`], computes execution [`layers`](graph::topological_layers),
//! and renders templates ([`vars::substitute`]). Running the nodes (agent
//! adapters, worktrees, persistence) lives in the executor crates.
//!
//! ```
//! use harness_dag::{parse_workflow, topological_layers};
//!
//! let yaml = r#"
//! name: demo
//! nodes:
//!   - id: a
//!     bash: "echo hi"
//!   - id: b
//!     depends_on: [a]
//!     prompt: "summarize $ARTIFACTS_DIR"
//! "#;
//! let wf = parse_workflow(yaml).unwrap();
//! let layers = topological_layers(&wf).unwrap();
//! assert_eq!(layers.len(), 2);
//! ```

pub mod error;
pub mod exec;
pub mod graph;
pub mod model;
pub mod parse;
pub mod signal;
pub mod vars;

pub use error::DagError;
pub use exec::{
    run_workflow, run_workflow_streaming, NodeBody, NodeOutput, NodeRequest, NodeRun, NodeRunner,
    NodeStatus, RunEvent, RunReport, RunStatus, RunnerError, Usage,
};
pub use graph::topological_layers;
pub use model::{
    ApprovalConfig, ContextMode, LoopConfig, Node, NodeKind, ScriptRuntime, TriggerRule, Workflow,
};
pub use parse::parse_workflow;
pub use signal::{detect_signal, validate_signal};
pub use vars::{substitute, VarContext, RECOGNIZED_VARS};

#[cfg(test)]
mod tests;
