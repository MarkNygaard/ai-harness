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
use serde_json::Value;
use tokio::process::Command;

use crate::{AgentError, PromptAgent, PromptRequest, PromptResult};

/// Default model when a `pi` node declares no `model` (or only a bare name).
/// `kimi-for-coding` is the model *id* (its display name is "Kimi-k2.6").
const DEFAULT_MODEL: &str = "kimi-code/kimi-for-coding";
/// Model-namespace prefix for `omp --model provider/model` (bare model names are
/// prefixed with this). Note: the *auth* provider is `kimi-coding`, but models
/// are addressed under `kimi-code/`.
const KIMI_PREFIX: &str = "kimi-code/";
/// Hard cap on a single `omp` invocation, overridable via `OMP_TIMEOUT_SECS`.
/// 30 min: a Kimi node that runs the project's verify chain can trigger a cold
/// Rust compile, which the previous 15-min cap killed mid-build.
const DEFAULT_TIMEOUT_SECS: u64 = 1800;

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

        // A clean run is a zero exit with either the `agent_end` marker OR at
        // least an assistant message. We do NOT hard-require `agent_end`: omp's
        // terminal-event schema drifts by version, and requiring it previously
        // failed runs that actually completed (and produced the full output).
        let saw_end = parsed.saw_end;
        let success = output.status.success() && (saw_end || !parsed.text.is_empty());
        if !success {
            let stderr = String::from_utf8_lossy(&output.stderr);
            let s = stderr.trim();
            let skip = s.chars().count().saturating_sub(500);
            let tail: String = s.chars().skip(skip).collect();
            return Err(AgentError(format!(
                "omp run did not complete (exit={:?}, saw_end={saw_end}, text={}B): {tail}",
                output.status.code(),
                parsed.text.len()
            )));
        }

        Ok(PromptResult {
            text: parsed.text,
            session: parsed.session.or(req.session),
            usage: parsed.usage,
            success: true,
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
        // Parse leniently as a generic Value: a single field-shape quirk inside a
        // message (tool_use/thinking content, or `content` as a bare string) must
        // never drop the whole event. Strict struct deserialization previously
        // failed `agent_end` that way — losing both the completion signal and the
        // token usage that rides on it.
        let Ok(v) = serde_json::from_str::<Value>(line) else {
            continue;
        };
        match v.get("type").and_then(Value::as_str) {
            Some("session") => {
                if let Some(id) = v.get("id").and_then(Value::as_str) {
                    if !id.is_empty() {
                        out.session = Some(id.to_string());
                    }
                }
            }
            Some("message_end") => {
                if let Some(text) = assistant_text(v.get("message")) {
                    out.text = text; // last assistant message wins
                }
            }
            Some("agent_end") => {
                out.saw_end = true;
                // Prefer the final assistant text from the completed conversation.
                if let Some(text) = v
                    .get("messages")
                    .and_then(Value::as_array)
                    .and_then(|msgs| msgs.iter().rev().find_map(|m| assistant_text(Some(m))))
                {
                    out.text = text;
                }
                if let Some(usage) = v.get("telemetry").and_then(|t| t.get("usage")) {
                    out.usage = usage_from_value(usage);
                }
                if let Some(cost) = v
                    .get("telemetry")
                    .and_then(|t| t.get("cost"))
                    .and_then(|c| c.get("estimatedUsd").or_else(|| c.get("estimated_usd")))
                    .and_then(Value::as_f64)
                {
                    out.cost_usd = Some(cost);
                }
            }
            _ => {}
        }
    }
    out
}

/// Assistant text from a message `Value`. `content` may be a plain string or an
/// array of `{type:"text", text}` parts; non-assistant roles yield `None`.
fn assistant_text(msg: Option<&serde_json::Value>) -> Option<String> {
    if let Some(role) = msg.get("role").and_then(Value::as_str) {
        if role != "assistant" {
            return None;
        }
    }
    let content = msg.get("content")?;
    if let Some(s) = content.as_str() {
        return (!s.is_empty()).then(|| s.to_string());
    }
    let text: String = content
        .as_array()?
        .iter()
        .filter_map(|c| {
            if c.get("type").and_then(Value::as_str) == Some("text") {
                c.get("text").and_then(Value::as_str)
            } else {
                None
            }
        })
        .fold(String::new(), |mut acc, s| {
            acc.push_str(s);
            acc
        });
    (!text.is_empty()).then_some(text)
}

/// Token usage from a telemetry `usage` object, tolerating camelCase or
/// snake_case key spellings across omp versions.
fn usage_from_value(u: &serde_json::Value) -> Usage {
    let pick = |keys: &[&str]| {
        keys.iter()
            .find_map(|k| u.get(*k).and_then(serde_json::Value::as_u64))
    };
    Usage {
        input: pick(&[
            "inputTokens",
            "input_tokens",
            "input",
            "promptTokens",
            "prompt_tokens",
        ]),
        output: pick(&[
            "outputTokens",
            "output_tokens",
            "output",
            "completionTokens",
            "completion_tokens",
        ]),
        cache_read: pick(&[
            "cachedInputTokens",
            "cached_input_tokens",
            "cacheRead",
            "cache_read",
        ]),
        cache_write: pick(&[
            "cacheWriteTokens",
            "cache_write_tokens",
            "cacheWrite",
            "cache_write",
        ]),
    }
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
    fn agent_end_with_tool_and_string_content_still_yields_usage() {
        // Regression: a real agent_end carries the whole conversation in
        // `messages[]` — tool_use/tool_result/thinking parts and sometimes a
        // string `content`. Strict struct parsing dropped the event (losing
        // saw_end + usage). The Value-based parser must tolerate it.
        let stream = concat!(
            "{\"type\":\"agent_end\",\"messages\":[",
            "{\"role\":\"user\",\"content\":\"do the thing\"},",
            "{\"role\":\"assistant\",\"content\":[{\"type\":\"tool_use\",\"name\":\"bash\",\"input\":{}}]},",
            "{\"role\":\"tool\",\"content\":[{\"type\":\"tool_result\",\"output\":\"ok\"}]},",
            "{\"role\":\"assistant\",\"content\":[{\"type\":\"text\",\"text\":\"done\"}]}",
            "],\"telemetry\":{\"usage\":{\"input_tokens\":900,\"output_tokens\":120,\"cache_read\":40}}}\n"
        );
        let parsed = parse_omp_stream(stream);
        assert!(
            parsed.saw_end,
            "agent_end must register despite tool/string content"
        );
        assert_eq!(parsed.text, "done");
        // snake_case usage keys are tolerated.
        assert_eq!(parsed.usage.input, Some(900));
        assert_eq!(parsed.usage.output, Some(120));
        assert_eq!(parsed.usage.cache_read, Some(40));
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
            agent.resolve_model(Some("kimi-code/kimi-for-coding")),
            "kimi-code/kimi-for-coding"
        );
        // Bare name → prefixed with the Kimi model namespace.
        assert_eq!(
            agent.resolve_model(Some("kimi-for-coding")),
            "kimi-code/kimi-for-coding"
        );
        assert_eq!(agent.resolve_model(None), DEFAULT_MODEL);
    }

    #[test]
    fn build_args_adds_resume_when_session_present() {
        let agent = PiAgent::from_env();
        assert_eq!(
            base,
            vec![
                "-p",
                "do it",
                "--mode",
                "json",
                "--model",
                "kimi-code/kimi-for-coding"
            ]
        );
        let resumed = agent.build_args("again", "kimi-code/kimi-for-coding", Some("sess-9"));
        assert!(resumed.windows(2).any(|w| w == ["--resume", "sess-9"]));
    }
}
