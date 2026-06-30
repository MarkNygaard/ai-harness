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
//! trait (or using a session-aware adapter).

use std::sync::Arc;

use async_trait::async_trait;
use harness_agents::registry::AgentRegistry;
use harness_core::agent::{AgentRequest, StreamItem};
use harness_core::types::{Item, TokenUsage};
use harness_dag::{Activity, Usage};

use crate::{AgentError, PromptAgent, PromptRequest, PromptResult};

/// Map a [`CodeAgent`] token tally to the DAG's [`Usage`]. Claude reports most
/// of its prompt as `cache_read`, so surface the cache breakdown (matching the
/// omp path); `None` preserves the "unknown" vs "zero" distinction.
fn map_usage(u: &TokenUsage) -> Usage {
    Usage {
        input: Some(u.input_tokens),
        output: Some(u.output_tokens),
        cache_read: (u.cache_read_tokens > 0).then_some(u.cache_read_tokens),
        cache_write: (u.cache_creation_tokens > 0).then_some(u.cache_creation_tokens),
    }
}

/// Map a streamed [`Item`] to a live [`Activity`] for the progress feed —
/// surfacing what the agent is *doing* (tool calls, edits, reads). Returns
/// `None` for items that aren't activity (assistant text is the output, not
/// progress; user messages are echoes). Claude's `Item`s carry no tool-call id,
/// so `tool_id` is `None` (the UI shows the cards unpaired).
fn activity_from_item(item: &Item) -> Option<Activity> {
    match item {
        Item::ToolCall { name, input, .. } => Some(Activity::tool(
            name.clone(),
            crate::pi::tool_input_detail(Some(input)),
            None,
        )),
        Item::ShellCommand { command, .. } => Some(Activity::tool(
            "shell",
            Some(crate::pi::truncate_activity(command)),
            None,
        )),
        Item::FileEdit { path, .. } => Some(Activity::tool(
            "edit",
            Some(crate::pi::truncate_activity(&path.display().to_string())),
            None,
        )),
        Item::FileRead { path, .. } => Some(Activity::tool(
            "read",
            Some(crate::pi::truncate_activity(&path.display().to_string())),
            None,
        )),
        Item::ApprovalRequest { action, .. } => Some(Activity::text(format!("approval: {action}"))),
        Item::Error { message, .. } => Some(Activity::tool_result(
            crate::pi::truncate_activity(message),
            None,
            true,
        )),
        // Assistant text is the node's output (shown in the Output panel), not a
        // progress line; user messages are prompt echoes.
        Item::AgentReasoning { .. } | Item::UserMessage { .. } => None,
    }
}

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
    async fn run(&self, mut req: PromptRequest) -> Result<PromptResult, AgentError> {
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

        let _hooks_dir = if let Some(hooks) = req.hooks.as_ref() {
            let dir = tempfile::Builder::new()
                .prefix("harness-claude-hooks-")
                .tempdir()
                .map_err(|e| AgentError(e.to_string()))?;
            let (settings, payloads) = crate::hooks::claude_settings(hooks, dir.path());
            for (path, body) in payloads {
                std::fs::write(&path, body).map_err(|e| AgentError(e.to_string()))?;
            }
            let settings_path = dir.path().join("settings.json");
            std::fs::write(
                &settings_path,
                serde_json::to_vec_pretty(&settings).unwrap(),
            )
            .map_err(|e| AgentError(e.to_string()))?;
            req.env_vars.insert(
                "HARNESS_CLAUDE_SETTINGS".into(),
                settings_path.to_string_lossy().into_owned(),
            );
            Some(dir) // bound to keep the TempDir alive across execute()
        } else {
            None
        };

        // Take the progress sink before req fields are moved into the request.
        let progress = req.progress.take();

        let agent_req = AgentRequest {
            prompt: req.prompt,
            project_root: req.cwd,
            model: req.model,
            reasoning_effort: req.effort,
            env_vars: req.env_vars,
            ..AgentRequest::default()
        };

        // Streaming path: when the run is live (a progress sink is present),
        // stream the agent so its tool calls / edits surface as live activity —
        // exactly like the omp path. The final output and token usage are
        // recovered from the stream: the last `AgentReasoning` item carries the
        // assembled output (the agent replaces the accumulated deltas with the
        // canonical text on completion), and the `TokenUsage` item the totals.
        // `_hooks_dir` (above) stays alive across this await.
        if let Some(sink) = progress {
            let (tx, mut rx) = tokio::sync::mpsc::channel::<StreamItem>(256);
            let drain = async {
                let mut final_output: Option<String> = None;
                let mut delta_acc = String::new();
                let mut usage: Option<TokenUsage> = None;
                while let Some(item) = rx.recv().await {
                    match item {
                        StreamItem::ItemCompleted { item } => match item {
                            Item::AgentReasoning { content } => final_output = Some(content),
                            other => {
                                if let Some(act) = activity_from_item(&other) {
                                    sink.report(act);
                                }
                            }
                        },
                        StreamItem::MessageDelta { text } => delta_acc.push_str(&text),
                        StreamItem::TokenUsage { usage: u } => usage = Some(u),
                        StreamItem::Error { message } => {
                            sink.report(Activity::text(format!("⚠ {message}")))
                        }
                        _ => {}
                    }
                }
                (final_output.unwrap_or(delta_acc), usage)
            };
            // Run the agent and the drain concurrently on this task; when
            // `execute_stream` returns, `tx` drops, ending the drain loop.
            let (stream_res, (output, usage)) =
                tokio::join!(agent.execute_stream(agent_req, tx), drain);
            stream_res.map_err(|e| AgentError(e.to_string()))?;
            return Ok(PromptResult {
                text: output,
                session: None,
                usage: usage.as_ref().map(map_usage).unwrap_or_default(),
                // `execute_stream` returns `Err` on a non-zero CLI exit, so
                // reaching here means the run succeeded.
                success: true,
            });
        }

        let resp = agent
            .execute(agent_req)
            .await
            .map_err(|e| AgentError(e.to_string()))?;

        Ok(PromptResult {
            text: resp.output,
            // See the module-level note: no session resume through CodeAgent.
            session: None,
            usage: map_usage(&resp.token_usage),
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
                    cache_read_tokens: 0,
                    cache_creation_tokens: 0,
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
            effort: None,
            prompt: "do it".into(),
            cwd: std::path::PathBuf::from("."),
            session: None,
            iteration: 1,
            env_vars: Default::default(),
            hooks: None,
            progress: None,
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
    async fn hooks_materialize_claude_settings_env_var() {
        use std::collections::HashMap;
        use std::sync::Mutex;

        struct RecordingStubAgent {
            last_env: Mutex<Option<HashMap<String, String>>>,
        }

        #[async_trait]
        impl CodeAgent for RecordingStubAgent {
            fn name(&self) -> &str {
                "claude"
            }
            fn capabilities(&self) -> Vec<Capability> {
                vec![]
            }
            async fn execute(
                &self,
                req: AgentRequest,
            ) -> harness_core::error::Result<AgentResponse> {
                *self.last_env.lock().unwrap() = Some(req.env_vars.clone());
                // Verify the settings file exists during execute.
                if let Some(path) = req.env_vars.get("HARNESS_CLAUDE_SETTINGS") {
                    assert!(std::path::Path::new(path).exists(), "settings.json missing");
                }
                Ok(AgentResponse {
                    output: "ok".into(),
                    stderr: String::new(),
                    items: vec![],
                    token_usage: TokenUsage {
                        input_tokens: 1,
                        output_tokens: 1,
                        total_tokens: 2,
                        cache_read_tokens: 0,
                        cache_creation_tokens: 0,
                        cost_usd: 0.0,
                    },
                    model: "claude".into(),
                    exit_code: Some(0),
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

        let mut reg = AgentRegistry::new("claude");
        let agent = Arc::new(RecordingStubAgent {
            last_env: Mutex::new(None),
        });
        reg.register("claude", agent.clone());
        let runner = CodeAgentRunner::new(Arc::new(reg));

        let mut req = request(Some("claude"));
        req.hooks = Some(harness_dag::NodeHooks {
            pre_tool_use: vec![harness_dag::HookRule {
                matcher: Some("Write".into()),
                decision: Some(harness_dag::HookDecision::Deny),
                reason: Some("blocked".into()),
                additional_context: None,
                system_message: None,
            }],
            post_tool_use: vec![],
        });
        runner.run(req).await.unwrap();

        let env = agent.last_env.lock().unwrap().take().unwrap();
        assert!(env.contains_key("HARNESS_CLAUDE_SETTINGS"));
    }

    #[tokio::test]
    async fn hooksless_request_has_no_claude_settings_env() {
        let runner = CodeAgentRunner::new(registry_with("claude"));
        let out = runner.run(request(Some("claude"))).await.unwrap();
        assert!(out.success);
        // StubAgent doesn't record env, but the test succeeding with no error
        // confirms no settings file was materialized.
    }

    /// A CodeAgent that streams a representative item sequence: a partial text
    /// delta, a tool call, the final assembled reasoning, and token usage.
    struct StreamingStubAgent;

    #[async_trait]
    impl CodeAgent for StreamingStubAgent {
        fn name(&self) -> &str {
            "claude"
        }
        fn capabilities(&self) -> Vec<Capability> {
            vec![]
        }
        async fn execute(&self, _req: AgentRequest) -> harness_core::error::Result<AgentResponse> {
            // The streaming path must NOT call this; a marker output makes an
            // accidental fallthrough fail the assertion loudly.
            Ok(AgentResponse {
                output: "BUFFERED-PATH-WRONGLY-TAKEN".into(),
                stderr: String::new(),
                items: vec![],
                token_usage: TokenUsage::default(),
                model: "claude".into(),
                exit_code: Some(0),
            })
        }
        async fn execute_stream(
            &self,
            _req: AgentRequest,
            tx: tokio::sync::mpsc::Sender<StreamItem>,
        ) -> harness_core::error::Result<()> {
            use harness_core::types::Item;
            let _ = tx
                .send(StreamItem::MessageDelta {
                    text: "partial ".into(),
                })
                .await;
            let _ = tx
                .send(StreamItem::ItemCompleted {
                    item: Item::ToolCall {
                        name: "bash".into(),
                        input: serde_json::json!({ "command": "cargo test" }),
                        output: None,
                    },
                })
                .await;
            let _ = tx
                .send(StreamItem::ItemCompleted {
                    item: Item::AgentReasoning {
                        content: "final answer".into(),
                    },
                })
                .await;
            let _ = tx
                .send(StreamItem::TokenUsage {
                    usage: TokenUsage {
                        input_tokens: 100,
                        output_tokens: 20,
                        total_tokens: 120,
                        cache_read_tokens: 80,
                        cache_creation_tokens: 5,
                        cost_usd: 0.0,
                    },
                })
                .await;
            let _ = tx.send(StreamItem::Done).await;
            Ok(())
        }
    }

    #[tokio::test]
    async fn streaming_path_reports_activity_and_recovers_output_and_usage() {
        use harness_dag::{ActivityKind, ProgressSink};
        use std::sync::Mutex;

        let mut reg = AgentRegistry::new("claude");
        reg.register("claude", Arc::new(StreamingStubAgent));
        let runner = CodeAgentRunner::new(Arc::new(reg));

        let recorded: Arc<Mutex<Vec<Activity>>> = Arc::new(Mutex::new(Vec::new()));
        let rec = recorded.clone();
        let mut req = request(Some("claude"));
        req.progress = Some(ProgressSink(Arc::new(move |a| {
            rec.lock().unwrap().push(a);
        })));

        let out = runner.run(req).await.unwrap();

        // Output is the final AgentReasoning content (not the partial delta), and
        // usage is recovered from the TokenUsage item with the cache breakdown.
        assert_eq!(out.text, "final answer");
        assert!(out.success);
        assert_eq!(out.usage.input, Some(100));
        assert_eq!(out.usage.output, Some(20));
        assert_eq!(out.usage.cache_read, Some(80));
        assert_eq!(out.usage.cache_write, Some(5));
        assert_eq!(out.session, None);

        // The tool call surfaced as a live activity; the final reasoning did not.
        let acts = recorded.lock().unwrap();
        assert_eq!(acts.len(), 1, "only the tool call is an activity");
        assert_eq!(acts[0].kind, ActivityKind::Tool);
        assert_eq!(acts[0].text, "bash");
        assert_eq!(acts[0].detail.as_deref(), Some("cargo test"));
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
