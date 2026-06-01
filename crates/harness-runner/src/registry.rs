//! Build a populated [`AgentRegistry`] from [`HarnessConfig`].
//!
//! Mirrors the registry construction in `harness-cli`'s `exec` command: register
//! the Claude CLI, Codex CLI, and (when `ANTHROPIC_API_KEY` is set) the Anthropic
//! API agent, so a [`crate::CodeAgentRunner`] can dispatch real workflow prompts.
//! This is config-only — no Postgres, no server — so it works for the standalone
//! `harness-run` binary.

use std::sync::Arc;

use harness_agents::registry::AgentRegistry;
use harness_core::config::agents::SandboxMode;
use harness_core::config::HarnessConfig;

/// Construct an agent registry with Claude, Codex, and (if `ANTHROPIC_API_KEY`
/// is present) the Anthropic API agent, using `sandbox_mode` for the CLI agents.
pub fn build_agent_registry(config: &HarnessConfig, sandbox_mode: SandboxMode) -> AgentRegistry {
    let mut registry = AgentRegistry::new(&config.agents.default_agent);
    registry.set_complexity_preferences(config.agents.complexity_preferred_agents.clone());

    let mut claude = harness_agents::claude::ClaudeCodeAgent::new(
        config.agents.claude.cli_path.clone(),
        config.agents.claude.default_model.clone(),
        sandbox_mode,
    )
    .with_no_session_persistence_probe()
    .with_stream_timeout(config.agents.stream_timeout_secs);
    if let Some(budget) = config.agents.claude.reasoning_budget.clone() {
        claude = claude.with_reasoning_budget(budget);
    }
    registry.register("claude", Arc::new(claude));

    registry.register(
        "codex",
        Arc::new(
            harness_agents::codex::CodexAgent::from_config(
                config.agents.codex.clone(),
                sandbox_mode,
            )
            .with_stream_timeout(config.agents.stream_timeout_secs),
        ),
    );

    if let Ok(api_key) = std::env::var("ANTHROPIC_API_KEY") {
        registry.register(
            "anthropic-api",
            Arc::new(
                harness_agents::anthropic_api::AnthropicApiAgent::from_config(
                    api_key,
                    &config.agents.anthropic_api,
                ),
            ),
        );
    }

    registry
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builds_registry_with_claude_and_codex() {
        let config = HarnessConfig::default();
        let registry = build_agent_registry(&config, SandboxMode::ReadOnly);
        let agents = registry.list();
        assert!(agents.contains(&"claude"), "agents: {agents:?}");
        assert!(agents.contains(&"codex"), "agents: {agents:?}");
        // anthropic-api is conditional on ANTHROPIC_API_KEY, so not asserted.
    }
}
