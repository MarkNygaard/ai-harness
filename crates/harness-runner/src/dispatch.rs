//! Provider dispatch across heterogeneous [`PromptAgent`] backends.
//!
//! The DAG driver talks to a single [`PromptAgent`], but different providers are
//! backed by different machinery: `claude`/`anthropic-api` go through
//! majiayu's session-less `CodeAgent` registry (via [`CodeAgentRunner`]), `pi` is
//! a session-aware [`PiAgent`] that drives the `omp` CLI, and `cursor` is a
//! [`CursorAgent`] driving the `cursor-agent` CLI. [`DispatchAgent`] routes by
//! `provider` name so a workflow can mix all of them in one DAG.

use async_trait::async_trait;
use std::collections::HashMap;
use std::sync::Arc;

use crate::claude_pool::ClaudeAccountPool;
use crate::{AgentError, PromptAgent, PromptRequest, PromptResult};
use harness_dag::Activity;

/// Provider names routed to the Pi/`omp` backend.
const PI_PROVIDERS: &[&str] = &["pi", "omp", "kimi"];
/// Provider names routed to the Cursor (`cursor-agent`) backend.
const CURSOR_PROVIDERS: &[&str] = &["cursor"];

/// Where Claude Code keeps an account's credentials, settings and sessions.
const CLAUDE_CONFIG_DIR: &str = "CLAUDE_CONFIG_DIR";

/// Routes a [`PromptRequest`] by `provider`: the Pi agent (`pi|omp|kimi`), the
/// Cursor agent (`cursor`), or a fallback (everything else, typically the
/// [`CodeAgentRunner`] for `claude`/`anthropic-api`).
pub struct DispatchAgent {
    pi: Arc<dyn PromptAgent>,
    cursor: Arc<dyn PromptAgent>,
    fallback: Arc<dyn PromptAgent>,
    /// Credentials per backend (`pi`, `cursor`, or the fallback's provider
    /// name), added to a step's environment only when it runs on that backend.
    provider_env: HashMap<String, HashMap<String, String>>,
    /// The run's Claude accounts, when the harness has more than one.
    claude_pool: Option<Arc<ClaudeAccountPool>>,
}

/// Steps Claude Code runs: no provider, or `claude`.
fn is_claude(provider: Option<&str>) -> bool {
    provider.is_none_or(|p| p == "claude")
}

impl DispatchAgent {
    pub fn new(
        pi: Arc<dyn PromptAgent>,
        cursor: Arc<dyn PromptAgent>,
        fallback: Arc<dyn PromptAgent>,
    ) -> Self {
        Self {
            pi,
            cursor,
            fallback,
            provider_env: HashMap::new(),
            claude_pool: None,
        }
    }

    /// Spread this run's Claude steps over several accounts (see
    /// [`ClaudeAccountPool`]).
    pub fn with_claude_pool(mut self, pool: Arc<ClaudeAccountPool>) -> Self {
        self.claude_pool = Some(pool);
        self
    }

    /// Run a Claude step on the pool's account, and when it hits that
    /// account's usage limit, again on the next one that has room.
    async fn run_on_pool(
        &self,
        pool: &ClaudeAccountPool,
        mut req: PromptRequest,
    ) -> Result<PromptResult, AgentError> {
        let mut announced: Option<String> = None;
        loop {
            let Some((slot, session)) = pool.account_for(req.session.as_deref()).await else {
                // No account usable at all: run as a single-account harness does.
                return self.fallback.run(req).await;
            };
            req.session = session;
            match &slot.config_dir {
                Some(dir) => {
                    req.env_vars
                        .insert(CLAUDE_CONFIG_DIR.into(), dir.display().to_string());
                }
                None => {
                    req.env_vars.remove(CLAUDE_CONFIG_DIR);
                }
            }
            if announced.as_deref() != Some(slot.id.as_str()) {
                if let Some(sink) = &req.progress {
                    sink.report(Activity::text(format!("Claude account: {}", slot.label)));
                }
                announced = Some(slot.id.clone());
            }
            let result = self.fallback.run(req.clone()).await;
            let failure = match &result {
                Err(e) => Some(e.0.as_str()),
                Ok(r) if !r.success => Some(r.text.as_str()),
                Ok(_) => None,
            };
            let at_limit = failure.is_some_and(|text| {
                harness_dag::usage_limit::reset_time(text, chrono::Utc::now()).is_some()
            });
            if at_limit {
                if let Some(next) = pool.fail_over(&slot.id).await {
                    if let Some(sink) = &req.progress {
                        sink.report(Activity::text(format!(
                            "Claude account “{}” reached its usage limit. Running this step again on “{}”.",
                            slot.label, next.label
                        )));
                    }
                    // The conversation stays with the account it started on.
                    req.session = None;
                    continue;
                }
            }
            if let Ok(r) = &result {
                if let Some(s) = &r.session {
                    pool.note_session(s, &slot.id);
                }
            }
            return result;
        }
    }

    /// Credentials each backend's agent needs, keyed `pi`, `cursor`,
    /// `claude`. A step gets only its own backend's entry: the run's shared
    /// environment carries no provider key, so an omp step cannot read the
    /// Cursor key and a bash step reads none.
    pub fn with_provider_env(mut self, env: HashMap<String, HashMap<String, String>>) -> Self {
        self.provider_env = env;
        self
    }

    fn routes_to(set: &[&str], provider: Option<&str>) -> bool {
        provider.is_some_and(|p| set.contains(&p))
    }

    fn add_env(&self, backend: &str, req: &mut PromptRequest) {
        if let Some(env) = self.provider_env.get(backend) {
            req.env_vars
                .extend(env.iter().map(|(k, v)| (k.clone(), v.clone())));
        }
    }
}

#[async_trait]
impl PromptAgent for DispatchAgent {
    async fn run(&self, mut req: PromptRequest) -> Result<PromptResult, AgentError> {
        let provider = req.provider.clone();
        let provider = provider.as_deref();
        if Self::routes_to(PI_PROVIDERS, provider) {
            self.add_env("pi", &mut req);
            self.pi.run(req).await
        } else if Self::routes_to(CURSOR_PROVIDERS, provider) {
            self.add_env("cursor", &mut req);
            self.cursor.run(req).await
        } else {
            if let Some(p) = provider {
                self.add_env(p, &mut req);
            }
            match &self.claude_pool {
                Some(pool) if is_claude(provider) => self.run_on_pool(pool, req).await,
                _ => self.fallback.run(req).await,
            }
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
            effort: None,
            prompt: "x".into(),
            cwd: PathBuf::from("."),
            session: None,
            iteration: 1,
            env_vars: Default::default(),
            hooks: None,
            mcp_config: None,
            progress: None,
        }
    }

    #[tokio::test]
    async fn routes_each_provider_to_its_backend() {
        let pi_seen = Arc::new(Mutex::new(Vec::new()));
        let cursor_seen = Arc::new(Mutex::new(Vec::new()));
        let fb_seen = Arc::new(Mutex::new(Vec::new()));
        let dispatch = DispatchAgent::new(
            Arc::new(Spy {
                label: "pi",
                seen: pi_seen.clone(),
            }),
            Arc::new(Spy {
                label: "cursor",
                seen: cursor_seen.clone(),
            }),
            Arc::new(Spy {
                label: "fallback",
                seen: fb_seen.clone(),
            }),
        );

        assert_eq!(dispatch.run(req(Some("pi"))).await.unwrap().text, "pi");
        assert_eq!(dispatch.run(req(Some("kimi"))).await.unwrap().text, "pi");
        assert_eq!(
            dispatch.run(req(Some("cursor"))).await.unwrap().text,
            "cursor"
        );
        assert_eq!(
            dispatch.run(req(Some("claude"))).await.unwrap().text,
            "fallback"
        );
        assert_eq!(dispatch.run(req(None)).await.unwrap().text, "fallback");

        assert_eq!(pi_seen.lock().unwrap().len(), 2);
        assert_eq!(cursor_seen.lock().unwrap().as_slice(), ["cursor"]);
        assert_eq!(fb_seen.lock().unwrap().as_slice(), ["claude", ""]);
    }
}

#[cfg(test)]
mod provider_env_tests {
    use super::*;
    use std::path::PathBuf;
    use std::sync::Mutex;

    /// Records the environment each request arrived with.
    struct EnvSpy(Arc<Mutex<Vec<HashMap<String, String>>>>);

    #[async_trait]
    impl PromptAgent for EnvSpy {
        async fn run(&self, req: PromptRequest) -> Result<PromptResult, AgentError> {
            self.0.lock().unwrap().push(req.env_vars.clone());
            Ok(PromptResult::default())
        }
    }

    fn req(provider: Option<&str>) -> PromptRequest {
        PromptRequest {
            provider: provider.map(str::to_string),
            model: None,
            effort: None,
            prompt: "x".into(),
            cwd: PathBuf::from("."),
            session: None,
            iteration: 1,
            env_vars: HashMap::from([("SHARED".to_string(), "1".to_string())]),
            hooks: None,
            mcp_config: None,
            progress: None,
        }
    }

    #[tokio::test]
    async fn each_backend_gets_only_its_own_credentials() {
        let seen = Arc::new(Mutex::new(Vec::new()));
        let spy = || Arc::new(EnvSpy(seen.clone())) as Arc<dyn PromptAgent>;
        let dispatch = DispatchAgent::new(spy(), spy(), spy()).with_provider_env(HashMap::from([
            (
                "pi".to_string(),
                HashMap::from([("MOONSHOT_API_KEY".to_string(), "moon".to_string())]),
            ),
            (
                "cursor".to_string(),
                HashMap::from([("CURSOR_API_KEY".to_string(), "cur".to_string())]),
            ),
        ]));

        for provider in [Some("kimi"), Some("cursor"), Some("claude"), None] {
            dispatch.run(req(provider)).await.unwrap();
        }
        let seen = seen.lock().unwrap();
        let keys = |i: usize| {
            let mut k: Vec<&str> = seen[i].keys().map(String::as_str).collect();
            k.sort();
            k
        };
        // omp (via an alias) gets the Moonshot key and not the Cursor one.
        assert_eq!(keys(0), ["MOONSHOT_API_KEY", "SHARED"]);
        assert_eq!(keys(1), ["CURSOR_API_KEY", "SHARED"]);
        // Claude, named or by default, gets neither.
        assert_eq!(keys(2), ["SHARED"]);
        assert_eq!(keys(3), ["SHARED"]);
    }

    /// Each call's config folder and session, in order.
    type Calls = Arc<Mutex<Vec<(Option<String>, Option<String>)>>>;

    /// Hits the usage limit on account `a`, succeeds anywhere else, and
    /// records which config folder and session each call got.
    struct LimitedOnA(Calls);

    #[async_trait]
    impl PromptAgent for LimitedOnA {
        async fn run(&self, req: PromptRequest) -> Result<PromptResult, AgentError> {
            let dir = req.env_vars.get(CLAUDE_CONFIG_DIR).cloned();
            self.0
                .lock()
                .unwrap()
                .push((dir.clone(), req.session.clone()));
            if dir.as_deref() == Some("/accounts/a") {
                return Err(AgentError(
                    "usage limit reached, resets at unix 4102444800".into(),
                ));
            }
            Ok(PromptResult {
                text: "done".into(),
                session: Some("new-session".into()),
                success: true,
                ..Default::default()
            })
        }
    }

    #[tokio::test]
    async fn a_claude_step_moves_to_another_account_at_the_limit() {
        let calls = Arc::new(Mutex::new(Vec::new()));
        let claude = Arc::new(LimitedOnA(calls.clone()));
        let ranker = crate::claude_pool::tests::FixedRanker::new(&["a", "b"]);
        let pool = Arc::new(ClaudeAccountPool::new(ranker.clone()));
        let dispatch =
            DispatchAgent::new(spy_unused(), spy_unused(), claude).with_claude_pool(pool.clone());

        let mut first = req(None);
        first.session = Some("old-session".into());
        let out = dispatch.run(first).await.unwrap();
        assert_eq!(out.text, "done");
        let calls = calls.lock().unwrap().clone();
        assert_eq!(
            calls,
            [
                (Some("/accounts/a".into()), Some("old-session".into())),
                // Started again on `b`, without the conversation from `a`.
                (Some("/accounts/b".into()), None),
            ]
        );
        assert_eq!((ranker.held("a"), ranker.held("b")), (0, 1));

        // An omp step is not Claude's business.
        let omp = dispatch.run(req(Some("pi"))).await.unwrap();
        assert_eq!(omp.text, "unused");
    }

    /// A backend this test does not exercise; answers `unused`.
    struct Unused;

    #[async_trait]
    impl PromptAgent for Unused {
        async fn run(&self, _req: PromptRequest) -> Result<PromptResult, AgentError> {
            Ok(PromptResult {
                text: "unused".into(),
                success: true,
                ..Default::default()
            })
        }
    }

    fn spy_unused() -> Arc<dyn PromptAgent> {
        Arc::new(Unused)
    }
}
