//! # harness-runner
//!
//! Concrete [`harness_dag::NodeRunner`] backends — the environment that actually
//! executes node bodies for the DAG driver. This crate provides [`LocalRunner`]
//! (subprocess + a pluggable agent seam); a Kubernetes runner lands in Phase 6
//! behind the same trait.
//!
//! Prompt/command (AI) bodies are delegated to a [`PromptAgent`] — a small seam
//! that maps `(provider, model, prompt, cwd, session)` to text + session +
//! token usage. It carries a session id (which majiayu's `CodeAgent` trait does
//! not), so the driver's `context: shared` threading works; a real adapter that
//! wraps the agent CLIs implements this trait.

use std::path::PathBuf;

use async_trait::async_trait;
use harness_dag::Usage;

mod local;

pub use local::{LocalRunner, Shell};

/// A request to run an AI prompt against a working directory.
#[derive(Debug, Clone)]
pub struct PromptRequest {
    pub provider: Option<String>,
    pub model: Option<String>,
    pub prompt: String,
    pub cwd: PathBuf,
    /// Session to resume (for `context: shared` / loop threading), if any.
    pub session: Option<String>,
    /// 1-based iteration (`> 1` only inside loops).
    pub iteration: u32,
}

/// The result of an AI prompt invocation.
#[derive(Debug, Clone, Default)]
pub struct PromptResult {
    pub text: String,
    /// Session id to thread into the next shared/looping invocation.
    pub session: Option<String>,
    pub usage: Usage,
    pub success: bool,
}

/// Error from a [`PromptAgent`] invocation.
#[derive(Debug, thiserror::Error)]
#[error("agent error: {0}")]
pub struct AgentError(pub String);

/// The seam between [`LocalRunner`] and a concrete agent backend (Claude/Codex/
/// Pi). Kept minimal and session-aware so the DAG driver's session threading
/// works regardless of which backend is wired in.
#[async_trait]
pub trait PromptAgent: Send + Sync {
    async fn run(&self, req: PromptRequest) -> Result<PromptResult, AgentError>;
}
