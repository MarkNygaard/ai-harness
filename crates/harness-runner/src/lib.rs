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

use std::collections::HashMap;
use std::path::PathBuf;

use async_trait::async_trait;
use harness_dag::Usage;

pub mod authoring;
mod code_agent;
mod cursor;
pub mod defaults;
mod dispatch;
mod hooks;
mod local;
mod pi;
mod registry;
mod run;
mod worktree;
pub use code_agent::CodeAgentRunner;
pub use cursor::{parse_cursor_output, CursorAgent, ParsedCursor};
pub use defaults::{default_command, default_workflow, resolve_workflow_source, DEFAULT_WORKFLOW};
pub use dispatch::DispatchAgent;
pub use local::{LocalRunner, Shell};
pub use pi::{parse_omp_stream, ParsedOmp, PiAgent};
pub use registry::build_agent_registry;
pub use run::{execute_run, parse_sandbox, print_report, RunOptions};
pub use worktree::{
    clone_repo, default_branch, fetch_repo, mise_shims_dir, provision_toolchains,
    sanitize_branch_component, Worktree, WorktreeError,
};

/// A request to run an AI prompt against a working directory.
#[derive(Debug, Clone)]
pub struct PromptRequest {
    pub provider: Option<String>,
    pub model: Option<String>,
    /// Reasoning-effort override (`low`..`max`), forwarded to the agent CLI.
    pub effort: Option<String>,
    pub prompt: String,
    pub cwd: PathBuf,
    /// Session to resume (for `context: shared` / loop threading), if any.
    pub session: Option<String>,
    /// 1-based iteration (`> 1` only inside loops).
    pub iteration: u32,
    /// Additional environment variables to pass to the agent subprocess.
    pub env_vars: HashMap<String, String>,
    /// Provider-agnostic tool hooks translated per provider at dispatch.
    pub hooks: Option<harness_dag::NodeHooks>,
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

/// A trivial built-in [`PromptAgent`] for local development and demos.
///
/// It echoes the prompt back as output and threads a synthetic session id — no
/// real model is invoked, and token usage is left unknown (`None`) rather than
/// faked. Useful for exercising the DAG driver + [`LocalRunner`] end-to-end
/// before real agent adapters are wired in.
pub struct EchoAgent;

#[async_trait]
impl PromptAgent for EchoAgent {
    async fn run(&self, req: PromptRequest) -> Result<PromptResult, AgentError> {
        let session = req
            .session
            .or_else(|| Some(format!("echo-session-{}", req.iteration)));
        Ok(PromptResult {
            text: format!(
                "[echo:{}] {}",
                req.provider.as_deref().unwrap_or("default"),
                req.prompt
            ),
            session,
            usage: Usage::default(),
            success: true,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn echo_agent_echoes_and_threads_session() {
        let out = EchoAgent
            .run(PromptRequest {
                provider: Some("claude".into()),
                model: None,
                effort: None,
                prompt: "hello".into(),
                cwd: PathBuf::from("."),
                session: None,
                iteration: 1,
                env_vars: Default::default(),
                hooks: None,
            })
            .await
            .unwrap();
        assert_eq!(out.text, "[echo:claude] hello");
        assert!(out.success);
        assert_eq!(out.session.as_deref(), Some("echo-session-1"));
    }
}
