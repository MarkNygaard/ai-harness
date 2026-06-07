//! Provider dispatch across heterogeneous [`PromptAgent`] backends.
//!
//! The DAG driver talks to a single [`PromptAgent`], but different providers are
//! backed by different machinery: `claude`/`codex`/`anthropic-api` go through
//! majiayu's session-less `CodeAgent` registry (via [`CodeAgentRunner`]), while
//! `pi` is a session-aware [`PiAgent`] that drives the `omp` CLI. [`DispatchAgent`]
//! routes by `provider` name so a workflow can mix all of them in one DAG.

use async_trait::async_trait;
use std::sync::Arc;

use crate::{AgentError, PromptAgent, PromptRequest, PromptResult};

/// Provider names routed to the Pi/`omp` backend.
const PI_PROVIDERS: &[&str] = &["pi", "omp", "kimi"];

/// Routes a [`PromptRequest`] to the Pi agent (for `provider: pi|omp|kimi`) or a
/// fallback agent (everything else, typically the [`CodeAgentRunner`]).
pub struct DispatchAgent {
    pi: Arc<dyn PromptAgent>,
    fallback: Arc<dyn PromptAgent>,
}

impl DispatchAgent {
    pub fn new(pi: Arc<dyn PromptAgent>, fallback: Arc<dyn PromptAgent>) -> Self {
        Self { pi, fallback }
    }

    fn routes_to_pi(provider: Option<&str>) -> bool {
        provider.is_some_and(|p| PI_PROVIDERS.contains(&p))
    }
}

#[async_trait]
impl PromptAgent for DispatchAgent {
    async fn run(&self, req: PromptRequest) -> Result<PromptResult, AgentError> {
        if Self::routes_to_pi(req.provider.as_deref()) {
            self.pi.run(req).await
        } else {
            self.fallback.run(req).await
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;
    use std::sync::Mutex;

    /// Records which provider name it last saw, so we can assert routing.
    struct Spy {
        label: &'static str,
        seen: Arc<Mutex<Vec<String>>>,
    }

    #[async_trait]
    impl PromptAgent for Spy {
        async fn run(&self, req: PromptRequest) -> Result<PromptResult, AgentError> {
            self.seen
                .lock()
                .unwrap()
                .push(req.provider.clone().unwrap_or_default());
            Ok(PromptResult {
                text: self.label.to_string(),
                ..Default::default()
            })
        }
    }

    fn req(provider: Option<&str>) -> PromptRequest {
        PromptRequest {
            provider: provider.map(str::to_string),
            model: None,
            prompt: "x".into(),
            cwd: PathBuf::from("."),
            session: None,
            iteration: 1,
            env_vars: Default::default(),
            hooks: None,
        }
    }

    #[tokio::test]
    async fn routes_pi_providers_to_pi_and_rest_to_fallback() {
        let pi_seen = Arc::new(Mutex::new(Vec::new()));
        let fb_seen = Arc::new(Mutex::new(Vec::new()));
        let dispatch = DispatchAgent::new(
            Arc::new(Spy {
                label: "pi",
                seen: pi_seen.clone(),
            }),
            Arc::new(Spy {
                label: "fallback",
                seen: fb_seen.clone(),
            }),
        );

        assert_eq!(dispatch.run(req(Some("pi"))).await.unwrap().text, "pi");
        assert_eq!(dispatch.run(req(Some("kimi"))).await.unwrap().text, "pi");
        assert_eq!(
            dispatch.run(req(Some("claude"))).await.unwrap().text,
            "fallback"
        );
        assert_eq!(dispatch.run(req(None)).await.unwrap().text, "fallback");

        assert_eq!(pi_seen.lock().unwrap().len(), 2);
        assert_eq!(fb_seen.lock().unwrap().as_slice(), ["claude", ""]);
    }
}
