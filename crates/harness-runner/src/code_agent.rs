//! A [`PromptAgent`] backed by majiayu's real agent adapters.
//!
//! [`CodeAgentRunner`] resolves a node's `provider` to a registered
//! [`CodeAgent`] (Claude CLI, Codex CLI, Anthropic API) and runs the prompt
//! through `CodeAgent::execute`, mapping the response back to a [`PromptResult`].
//!
//! ## Session-threading gap (known limitation)
//!
//! `CodeAgent::execute(AgentRequest) -> AgentResponse` carries **no session id**
//! in or out, so it cannot resume a prior agent session. The DAG driver threads
//! a session for `context: shared` / loops, but this runner returns
//! `session: None` and ignores any incoming session — every prompt is a fresh
//! agent invocation. Conversational continuity across `shared` nodes therefore
//! does not work through this path yet. Closing it requires extending the agent
//! trait (or using a session-aware adapter); tracked in `docs/PHASES.md`.

use std::sync::Arc;

use async_trait::async_trait;
use harness_agents::registry::AgentRegistry;
use harness_core::agent::AgentRequest;
use harness_dag::Usage;

use crate::{AgentError, PromptAgent, PromptRequest, PromptResult};

/// A [`PromptAgent`] that dispatches to registered [`CodeAgent`]s by provider.
pub struct CodeAgentRunner {
    registry: Arc<AgentRegistry>,
}

impl CodeAgentRunner {
    /// Build a runner over a populated agent registry.
    pub fn new(registry: Arc<AgentRegistry>) -> Self {
        Self { registry }
    }
}

#[async_trait]
impl PromptAgent for CodeAgentRunner {
    async fn run(&self, req: PromptRequest) -> Result<PromptResult, AgentError> {
        // Resolve the provider to a CodeAgent. A named-but-unregistered provider
        // is a hard error (don't silently fall back and hide misconfiguration);
        // an unset provider uses the registry default.
        let agent = match req.provider.as_deref() {
            Some(name) => self.registry.get(name).ok_or_else(|| {
                AgentError(format!(
                    "no agent registered for provider `{name}` (have: {:?})",
                    self.registry.list()
                ))
            })?,
            None => self
                .registry
                .default_agent()
                .ok_or_else(|| AgentError("no default agent registered".to_string()))?,
        };

        let agent_req = AgentRequest {
            prompt: req.prompt,
            project_root: req.cwd,
            model: req.model,
            ..AgentRequest::default()
        };

        let resp = agent
            .execute(agent_req)
            .await
            .map_err(|e| AgentError(e.to_string()))?;

        Ok(PromptResult {
            text: resp.output,
            // See the module-level note: no session resume through CodeAgent.
            session: None,
            usage: Usage {
                input: Some(resp.token_usage.input_tokens),
                output: Some(resp.token_usage.output_tokens),
                cache_read: None,
                cache_write: None,
            },
            // Treat a missing exit code (API adapter) as success; a non-zero
            // CLI exit is a failure.
            success: resp.exit_code.map(|c| c == 0).unwrap_or(true),
        })
    }
}

#[cfg(test)]
mod tests {
    use harness_core::agent::{AgentResponse, CodeAgent, StreamItem};
    use harness_core::types::{Capability, TokenUsage};

    use super::*;

    /// Minimal CodeAgent that echoes a canned response and records the request.
    struct StubAgent {
        name: &'static str,
        output: &'static str,
        exit_code: Option<i32>,
    }

    #[async_trait]
    impl CodeAgent for StubAgent {
        fn name(&self) -> &str {
            self.name
        }
        fn capabilities(&self) -> Vec<Capability> {
            vec![]
        }
        async fn execute(&self, req: AgentRequest) -> harness_core::error::Result<AgentResponse> {
            Ok(AgentResponse {
                output: format!("{}: {}", self.output, req.prompt),
                stderr: String::new(),
                items: vec![],
                token_usage: TokenUsage {
                    input_tokens: 12,
                    output_tokens: 3,
                    total_tokens: 15,
                    cost_usd: 0.0,
                },
                model: req.model.unwrap_or_else(|| self.name.to_string()),
                exit_code: self.exit_code,
            })
        }
        async fn execute_stream(
            &self,
            _req: AgentRequest,
            _tx: tokio::sync::mpsc::Sender<StreamItem>,
        ) -> harness_core::error::Result<()> {
            Ok(())
        }
    }

    fn registry_with(name: &'static str) -> Arc<AgentRegistry> {
        let mut reg = AgentRegistry::new(name);
        reg.register(
            name,
            Arc::new(StubAgent {
                name,
                output: "stub",
                exit_code: Some(0),
            }),
        );
        Arc::new(reg)
    }

    fn request(provider: Option<&str>) -> PromptRequest {
        PromptRequest {
            provider: provider.map(str::to_string),
            model: Some("sonnet".into()),
            prompt: "do it".into(),
            cwd: std::path::PathBuf::from("."),
            session: None,
            iteration: 1,
        }
    }

    #[tokio::test]
    async fn dispatches_to_named_provider_and_maps_response() {
        let runner = CodeAgentRunner::new(registry_with("claude"));
        let out = runner.run(request(Some("claude"))).await.unwrap();
        assert_eq!(out.text, "stub: do it");
        assert!(out.success);
        assert_eq!(out.usage.input, Some(12));
        assert_eq!(out.usage.output, Some(3));
        assert_eq!(out.usage.cache_read, None);
        assert_eq!(out.session, None); // session gap
    }

    #[tokio::test]
    async fn unset_provider_uses_default() {
        let runner = CodeAgentRunner::new(registry_with("claude"));
        let out = runner.run(request(None)).await.unwrap();
        assert!(out.success);
    }

    #[tokio::test]
    async fn unknown_provider_errors() {
        let runner = CodeAgentRunner::new(registry_with("claude"));
        let err = runner.run(request(Some("ghost"))).await.unwrap_err();
        assert!(err.0.contains("no agent registered for provider `ghost`"));
    }

    #[tokio::test]
    async fn nonzero_exit_is_failure() {
        let mut reg = AgentRegistry::new("codex");
        reg.register(
            "codex",
            Arc::new(StubAgent {
                name: "codex",
                output: "stub",
                exit_code: Some(2),
            }),
        );
        let runner = CodeAgentRunner::new(Arc::new(reg));
        let out = runner.run(request(Some("codex"))).await.unwrap();
        assert!(!out.success);
    }
}
