//! Pi / Kimi-for-Coding provider — a session-aware [`PromptAgent`] that shells
//! out to the **`omp`** CLI (the `oh-my-pi` fork) in headless JSON mode.
//!
//! Invocation: `omp -p "<prompt>" --mode json --model <provider/model>` (+
//! `--resume <id>` to continue a session). `omp` emits **newline-delimited JSON**
//! events (camelCase keys, discriminated on `type`): a `session` header line
//! (its `id` is what we thread for `context: shared`), `message_end` events whose
//! `message.content[]` carries the assistant text, and a final `agent_end` whose
//! `telemetry.usage` gives full token fidelity incl. cache (PLAN §7.3/§10.1).
//!
//! Models are namespaced `kimi-code/*` (e.g. `kimi-code/kimi-k2.6`); auth for the
//! Kimi-for-Coding subscription is the `kimi-coding` provider — `KIMI_API_KEY` or a
//! `~/.pi/agent/auth.json` from `omp /login`. `MOONSHOT_API_KEY` covers the
//! per-token Moonshot API (`moonshotai/*`). The server materializes these from the
//! credential store before a run; we don't manage them here.

use std::path::PathBuf;
use std::process::Stdio;
use std::time::Duration;

use async_trait::async_trait;
use harness_dag::Usage;
use serde::Deserialize;
use tokio::process::Command;

use crate::{AgentError, PromptAgent, PromptRequest, PromptResult};

/// Default model when a `pi` node declares no `model` (or only a bare name).
const DEFAULT_MODEL: &str = "kimi-code/kimi-k2.6";
/// Model-namespace prefix for `omp --model provider/model` (bare model names are
/// prefixed with this). Note: the *auth* provider is `kimi-coding`, but models
/// are addressed under `kimi-code/`.
const KIMI_PREFIX: &str = "kimi-code/";
/// Hard cap on a single `omp` invocation, overridable via `OMP_TIMEOUT_SECS`.
const DEFAULT_TIMEOUT_SECS: u64 = 900;

/// A [`PromptAgent`] backed by the `omp` CLI. Selected for `provider: pi`.
pub struct PiAgent {
    cli_path: PathBuf,
    default_model: String,
    timeout: Duration,
}

impl Default for PiAgent {
    fn default() -> Self {
        Self::from_env()
    }
}

impl PiAgent {
    /// Build from the environment: `OMP_CLI`/`OMP_PATH` overrides the binary
    /// (default `omp`); `OMP_TIMEOUT_SECS` overrides the per-call timeout.
    pub fn from_env() -> Self {
        let cli_path = std::env::var_os("OMP_CLI")
            .or_else(|| std::env::var_os("OMP_PATH"))
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from("omp"));
        let timeout = std::env::var("OMP_TIMEOUT_SECS")
            .ok()
            .and_then(|v| v.parse::<u64>().ok())
            .map(Duration::from_secs)
            .unwrap_or_else(|| Duration::from_secs(DEFAULT_TIMEOUT_SECS));
        Self {
            cli_path,
            default_model: DEFAULT_MODEL.to_string(),
            timeout,
        }
    }

    /// Resolve a node's `model` to an `omp --model provider/model` value:
    /// pass through anything already provider-qualified, prefix a bare Kimi
    /// model name, and fall back to the default.
    fn resolve_model(&self, model: Option<&str>) -> String {
        match model {
            Some(m) if m.contains('/') => m.to_string(),
            Some(m) => format!("{KIMI_PREFIX}{m}"),
            None => self.default_model.clone(),
        }
    }

    fn build_args(&self, prompt: &str, model: &str, session: Option<&str>) -> Vec<String> {
        let mut args = vec![
            "-p".to_string(),
            prompt.to_string(),
            "--mode".to_string(),
            "json".to_string(),
            "--model".to_string(),
            model.to_string(),
        ];
        if let Some(id) = session {
            args.push("--resume".to_string());
            args.push(id.to_string());
        }
        args
    }
}

#[async_trait]
impl PromptAgent for PiAgent {
    async fn run(&self, req: PromptRequest) -> Result<PromptResult, AgentError> {
        let model = self.resolve_model(req.model.as_deref());
        let args = self.build_args(&req.prompt, &model, req.session.as_deref());

        let mut cmd = Command::new(&self.cli_path);
        cmd.args(&args)
            .current_dir(&req.cwd)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true);

        let fut = cmd.output();
        let output = match tokio::time::timeout(self.timeout, fut).await {
            Ok(Ok(out)) => out,
            Ok(Err(e)) => {
                return Err(AgentError(format!(
                    "failed to spawn `{}` (is the omp CLI installed / on PATH?): {e}",
                    self.cli_path.display()
                )))
            }
            Err(_) => {
                return Err(AgentError(format!(
                    "omp timed out after {}s",
                    self.timeout.as_secs()
                )))
            }
        };

        let stdout = String::from_utf8_lossy(&output.stdout);
        let parsed = parse_omp_stream(&stdout);

        // A clean run reaches `agent_end` with a zero exit; otherwise surface the
        // stderr tail so a misconfiguration (bad model, auth) is diagnosable.
        let success = output.status.success() && parsed.saw_end;
        if !success && parsed.text.is_empty() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            let tail: String = stderr
                .trim()
                .chars()
                .rev()
                .take(500)
                .collect::<Vec<_>>()
                .into_iter()
                .rev()
                .collect();
            return Err(AgentError(format!(
                "omp run did not complete (exit {:?}): {tail}",
                output.status.code()
            )));
        }

        Ok(PromptResult {
            text: parsed.text,
            session: parsed.session.or(req.session),
            usage: parsed.usage,
            success,
        })
    }
}

/// The distilled result of an `omp --mode json` stream.
#[derive(Debug, Default, PartialEq)]
pub struct ParsedOmp {
    pub text: String,
    pub session: Option<String>,
    pub usage: Usage,
    pub cost_usd: Option<f64>,
    /// Whether an `agent_end` event was seen (the run completed).
    pub saw_end: bool,
}

/// Parse `omp`'s newline-delimited JSON event stream into a [`ParsedOmp`].
/// Tolerant by design: unknown event types and malformed lines are skipped, so
/// a new `omp` event variant never breaks a run. Pure — unit-tested.
pub fn parse_omp_stream(stdout: &str) -> ParsedOmp {
    let mut out = ParsedOmp::default();
    for line in stdout.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let Ok(event) = serde_json::from_str::<OmpEvent>(line) else {
            continue;
        };
        match event {
            OmpEvent::Session { id } => {
                if id.is_some() {
                    out.session = id;
                }
            }
            OmpEvent::MessageEnd { message } => {
                if let Some(text) = message.text() {
                    out.text = text; // last assistant message wins
                }
            }
            OmpEvent::AgentEnd {
                telemetry,
                messages,
            } => {
                out.saw_end = true;
                // Prefer the final assistant text from the completed conversation.
                if let Some(text) = messages.iter().rev().find_map(OmpMessage::text) {
                    out.text = text;
                }
                if let Some(tel) = telemetry {
                    if let Some(usage) = tel.usage {
                        out.usage = usage.into();
                    }
                    if let Some(cost) = tel.cost {
                        out.cost_usd = cost.estimated_usd;
                    }
                }
            }
            OmpEvent::Other => {}
        }
    }
    out
}

// ── Lenient deserialization of the `omp` event stream ────────────────────────

#[derive(Deserialize)]
#[serde(tag = "type")]
enum OmpEvent {
    #[serde(rename = "session")]
    Session { id: Option<String> },
    #[serde(rename = "message_end")]
    MessageEnd { message: OmpMessage },
    #[serde(rename = "agent_end")]
    AgentEnd {
        #[serde(default)]
        telemetry: Option<OmpTelemetry>,
        #[serde(default)]
        messages: Vec<OmpMessage>,
    },
    #[serde(other)]
    Other,
}

#[derive(Deserialize)]
struct OmpMessage {
    #[serde(default)]
    role: Option<String>,
    #[serde(default)]
    content: Vec<OmpContent>,
}

impl OmpMessage {
    /// Concatenated text content of an assistant message, if any.
    fn text(&self) -> Option<String> {
        if self.role.as_deref().is_some_and(|r| r != "assistant") {
            return None;
        }
        let text: String = self
            .content
            .iter()
            .filter_map(|c| match c {
                OmpContent::Text { text } => Some(text.as_str()),
                OmpContent::Other => None,
            })
            .collect::<Vec<_>>()
            .join("");
        (!text.is_empty()).then_some(text)
    }
}

#[derive(Deserialize)]
#[serde(tag = "type")]
enum OmpContent {
    #[serde(rename = "text")]
    Text { text: String },
    #[serde(other)]
    Other,
}

#[derive(Deserialize)]
struct OmpTelemetry {
    #[serde(default)]
    usage: Option<OmpUsage>,
    #[serde(default)]
    cost: Option<OmpCost>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct OmpUsage {
    #[serde(default)]
    input_tokens: Option<u64>,
    #[serde(default)]
    output_tokens: Option<u64>,
    #[serde(default)]
    cached_input_tokens: Option<u64>,
    #[serde(default)]
    cache_write_tokens: Option<u64>,
}

impl From<OmpUsage> for Usage {
    fn from(u: OmpUsage) -> Self {
        Usage {
            input: u.input_tokens,
            output: u.output_tokens,
            cache_read: u.cached_input_tokens,
            cache_write: u.cache_write_tokens,
        }
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct OmpCost {
    #[serde(default)]
    estimated_usd: Option<f64>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_session_text_and_usage() {
        let stream = r#"
{"type":"session","id":"sess-123","cwd":"/tmp"}
{"type":"agent_start"}
{"type":"message_end","message":{"role":"assistant","content":[{"type":"text","text":"hello world"}]}}
{"type":"agent_end","messages":[{"role":"assistant","content":[{"type":"text","text":"final answer"}]}],"telemetry":{"usage":{"inputTokens":120,"outputTokens":45,"cachedInputTokens":10,"cacheWriteTokens":3,"totalTokens":178},"cost":{"estimatedUsd":0.0021}}}
"#;
        let parsed = parse_omp_stream(stream);
        assert_eq!(parsed.session.as_deref(), Some("sess-123"));
        assert_eq!(parsed.text, "final answer");
        assert!(parsed.saw_end);
        assert_eq!(parsed.usage.input, Some(120));
        assert_eq!(parsed.usage.output, Some(45));
        assert_eq!(parsed.usage.cache_read, Some(10));
        assert_eq!(parsed.usage.cache_write, Some(3));
        assert_eq!(parsed.cost_usd, Some(0.0021));
    }

    #[test]
    fn falls_back_to_message_end_when_agent_end_has_no_messages() {
        let stream = concat!(
            "{\"type\":\"message_end\",\"message\":{\"role\":\"assistant\",\"content\":[{\"type\":\"text\",\"text\":\"only message\"}]}}\n",
            "{\"type\":\"agent_end\",\"telemetry\":{\"usage\":{\"inputTokens\":5,\"outputTokens\":2}}}\n"
        );
        let parsed = parse_omp_stream(stream);
        assert_eq!(parsed.text, "only message");
        assert!(parsed.saw_end);
        assert_eq!(parsed.usage.input, Some(5));
        assert_eq!(parsed.usage.cache_read, None);
    }

    #[test]
    fn skips_malformed_and_unknown_events() {
        let stream = concat!(
            "not json at all\n",
            "{\"type\":\"some_future_event\",\"foo\":1}\n",
            "{\"type\":\"tool_execution_start\",\"toolName\":\"bash\"}\n",
            "{\"type\":\"message_end\",\"message\":{\"role\":\"assistant\",\"content\":[{\"type\":\"text\",\"text\":\"ok\"}]}}\n"
        );
        let parsed = parse_omp_stream(stream);
        assert_eq!(parsed.text, "ok");
        assert!(!parsed.saw_end);
    }

    #[test]
    fn ignores_text_from_non_assistant_roles() {
        let stream = "{\"type\":\"message_end\",\"message\":{\"role\":\"user\",\"content\":[{\"type\":\"text\",\"text\":\"the prompt\"}]}}\n";
        let parsed = parse_omp_stream(stream);
        assert_eq!(parsed.text, "");
    }

    #[test]
    fn empty_stream_is_empty_result() {
        let parsed = parse_omp_stream("\n\n  \n");
        assert_eq!(parsed, ParsedOmp::default());
    }

    #[test]
    fn resolve_model_qualifies_bare_names() {
        let agent = PiAgent::from_env();
        // Already provider-qualified → passed through unchanged.
        assert_eq!(
            agent.resolve_model(Some("kimi-code/kimi-k2.6")),
            "kimi-code/kimi-k2.6"
        );
        // Bare name → prefixed with the Kimi model namespace.
        assert_eq!(
            agent.resolve_model(Some("kimi-k2.5")),
            "kimi-code/kimi-k2.5"
        );
        assert_eq!(agent.resolve_model(None), DEFAULT_MODEL);
    }

    #[test]
    fn build_args_adds_resume_when_session_present() {
        let agent = PiAgent::from_env();
        let base = agent.build_args("do it", "kimi-code/kimi-k2.5", None);
        assert_eq!(
            base,
            vec![
                "-p",
                "do it",
                "--mode",
                "json",
                "--model",
                "kimi-code/kimi-k2.5"
            ]
        );
        let resumed = agent.build_args("again", "kimi-code/kimi-k2.5", Some("sess-9"));
        assert!(resumed.windows(2).any(|w| w == ["--resume", "sess-9"]));
    }
}
