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
//! Models are namespaced `kimi-code/*` (e.g. `kimi-code/kimi-k2.6`) or
//! `openai-codex/*` (e.g. `openai-codex/gpt-5.5` for the
//! ChatGPT-subscription path); auth for the Kimi-for-Coding subscription is the
//! `kimi-coding` provider — `KIMI_API_KEY` or a `~/.pi/agent/auth.json` from
//! `omp /login`. `MOONSHOT_API_KEY` covers the per-token Moonshot API
//! (`moonshotai/*`).
//! The server materializes these from the credential store before a run; we
//! don't manage them here.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::{Duration, Instant};

use async_trait::async_trait;
use harness_dag::{ProgressSink, Usage};
use tokio::io::{AsyncBufReadExt, AsyncReadExt, BufReader};
use tokio::process::Command;

use crate::{AgentError, PromptAgent, PromptRequest, PromptResult};

/// Embedded omp hook extension source, materialized to a temp dir at runtime.
const OMP_HOOK_EXTENSION_TS: &str = include_str!("../extensions/harness-hooks/index.ts");
const OMP_HOOK_EXTENSION_PKG: &str = include_str!("../extensions/harness-hooks/package.json");

/// Default model when a `pi` node declares no `model` (or only a bare name).
const DEFAULT_MODEL: &str = "kimi-code/kimi-for-coding";

/// Model-namespace prefix for `omp --model provider/model` (bare model names are
/// prefixed with this). Note: the *auth* provider is `kimi-coding`, but models
/// are addressed under `kimi-code/`.
const KIMI_PREFIX: &str = "kimi-code/";
/// Idle (no-output) watchdog: if `omp` emits no stdout line for this long, the
/// call is treated as stalled (e.g. an LLM connection died with no read timeout
/// on omp's side) and killed. Overridable via `OMP_IDLE_TIMEOUT_SECS`.
///
/// This is **activity-based**, not wall-clock: a step that keeps producing
/// output is never stopped, however long it runs — so big tasks are safe. The
/// default (15 min) sits comfortably above a silent in-tool gap (e.g. a long
/// `cargo build`) while still bounding a true silent hang. There is **no**
/// wall-clock cap by default; set `OMP_TIMEOUT_SECS` to add a hard ceiling.
const DEFAULT_IDLE_TIMEOUT_SECS: u64 = 900;

/// Outcome of one `omp` invocation that didn't error outright.
enum Attempt {
    /// The process closed stdout and exited; carries streamed stdout + stderr.
    Done {
        stdout: String,
        stderr: String,
        status: std::process::ExitStatus,
    },
    /// No output for the idle window — killed as stalled (retryable once).
    Stalled,
}

/// A [`PromptAgent`] backed by the `omp` CLI. Selected for `provider: pi`.
pub struct PiAgent {
    cli_path: PathBuf,
    default_model: String,
    /// Optional wall-clock hard ceiling on a single call. `None` (default) means
    /// no wall-clock cap — only the activity-based idle watchdog applies, so a
    /// long *active* step is never stopped. Set via `OMP_TIMEOUT_SECS`.
    timeout: Option<Duration>,
    idle_timeout: Duration,
    /// Optional `omp` plugin dirs (e.g. pi-web-access), loaded via `--plugin-dir`.
    plugin_dirs: Vec<String>,
}

impl Default for PiAgent {
    fn default() -> Self {
        Self::from_env()
    }
}

impl PiAgent {
    /// Build from the environment: `OMP_CLI`/`OMP_PATH` overrides the binary
    /// (default `omp`). `OMP_IDLE_TIMEOUT_SECS` tunes the idle (stall) watchdog;
    /// `OMP_TIMEOUT_SECS`, if set, adds an optional wall-clock hard ceiling
    /// (default: none — long active steps run uncapped).
    pub fn from_env() -> Self {
        let cli_path = std::env::var_os("OMP_CLI")
            .or_else(|| std::env::var_os("OMP_PATH"))
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from("omp"));
        let secs = |key: &str| {
            std::env::var(key)
                .ok()
                .and_then(|v| v.parse::<u64>().ok())
                .map(Duration::from_secs)
        };
        let timeout = secs("OMP_TIMEOUT_SECS"); // None = no wall-clock cap
        let idle_timeout =
            secs("OMP_IDLE_TIMEOUT_SECS").unwrap_or(Duration::from_secs(DEFAULT_IDLE_TIMEOUT_SECS));
        Self {
            cli_path,
            default_model: DEFAULT_MODEL.to_string(),
            timeout,
            idle_timeout,
            plugin_dirs: std::env::var("OMP_PLUGIN_DIRS")
                .ok()
                .map(|d| {
                    d.split(':')
                        .map(str::trim)
                        .filter(|s| !s.is_empty())
                        .map(String::from)
                        .collect()
                })
                .unwrap_or_default(),
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
        for dir in &self.plugin_dirs {
            args.push("--plugin-dir".to_string());
            args.push(dir.clone());
        }
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
        let mut args = self.build_args(&req.prompt, &model, req.session.as_deref());

        let _hooks_dir = if let Some(_hooks) = req.hooks.as_ref() {
            let dir = tempfile::Builder::new()
                .prefix("harness-omp-hooks-")
                .tempdir()
                .map_err(|e| AgentError(e.to_string()))?;
            std::fs::write(dir.path().join("index.ts"), OMP_HOOK_EXTENSION_TS)
                .map_err(|e| AgentError(e.to_string()))?;
            std::fs::write(dir.path().join("package.json"), OMP_HOOK_EXTENSION_PKG)
                .map_err(|e| AgentError(e.to_string()))?;
            args.push("--plugin-dir".to_string());
            args.push(dir.path().to_string_lossy().into_owned());
            Some(dir)
        } else {
            None
        };

        let env_vars = if let Some(hooks) = req.hooks.as_ref() {
            let mut cloned = req.env_vars.clone();
            cloned.insert("HARNESS_HOOKS".into(), crate::hooks::omp_hooks_env(hooks));
            cloned
        } else {
            req.env_vars.clone()
        };

        // One automatic retry, but ONLY on a stall (a transient dropped LLM
        // connection that omp doesn't time out). A clean non-zero exit (e.g.
        // tests failed) is deterministic — never retried. Capped at 1 → no loop.
        let mut attempt = 0u32;
        let output = loop {
            match self
                .run_attempt(&args, &req.cwd, &env_vars, req.progress.as_ref())
                .await?
            {
                Attempt::Done {
                    stdout,
                    stderr,
                    status,
                } => break (stdout, stderr, status),
                Attempt::Stalled => {
                    if attempt == 0 {
                        attempt += 1;
                        tracing::warn!(
                            "omp produced no output for {}s — stalled; retrying once",
                            self.idle_timeout.as_secs()
                        );
                        continue;
                    }
                    return Err(AgentError(format!(
                        "omp stalled (no output for {}s) and again after one retry",
                        self.idle_timeout.as_secs()
                    )));
                }
            }
        };
        let (stdout, stderr, status) = output;
        let parsed = parse_omp_stream(&stdout);

        // A clean run is a zero exit with either the `agent_end` marker OR at
        // least an assistant message. We do NOT hard-require `agent_end`: omp's
        // terminal-event schema drifts by version, and requiring it previously
        // failed runs that actually completed (and produced the full output).
        let saw_end = parsed.saw_end;
        let success = status.success() && (saw_end || !parsed.text.is_empty());
        if !success {
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
                "omp run did not complete (exit={:?}, saw_end={saw_end}, text={}B): {tail}",
                status.code(),
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

impl PiAgent {
    /// Run `omp` once, streaming stdout so an **idle watchdog** can kill a
    /// *stalled* call (no output for `idle_timeout`) without ever stopping a
    /// long but actively-producing step. Spawn failures / the optional
    /// wall-clock cap / read errors are returned as `Err` (not retried); a stall
    /// returns `Attempt::Stalled` (retried once by `run`).
    async fn run_attempt(
        &self,
        args: &[String],
        cwd: &Path,
        env_vars: &HashMap<String, String>,
        progress: Option<&ProgressSink>,
    ) -> Result<Attempt, AgentError> {
        let mut cmd = Command::new(&self.cli_path);
        cmd.args(args)
            .current_dir(cwd)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true);
        // Never let the agent (or any tool it spawns, e.g. `cargo test`) inherit
        // the control plane's DB URL / secrets.
        harness_agents::strip_control_plane_env(&mut cmd);
        cmd.envs(env_vars);

        // Opt-in filesystem sandbox (`HARNESS_FS_SANDBOX`): confine the agent and
        // every tool it spawns to writing only under the worktree + build caches,
        // so it can't overwrite shared toolchains/system binaries. Best-effort
        // Landlock (Linux only); see `harness_sandbox::restrict_self_writes`.
        #[cfg(unix)]
        if std::env::var_os("HARNESS_FS_SANDBOX").is_some() {
            let allowed = sandbox_allowlist(cwd);
            // SAFETY: the hook only performs Landlock syscalls + path opens, all
            // safe to run in the single-threaded child between fork and exec.
            unsafe {
                cmd.pre_exec(move || harness_sandbox::restrict_self_writes(&allowed));
            }
        }

        let mut child = cmd.spawn().map_err(|e| {
            AgentError(format!(
                "failed to spawn `{}` (is the omp CLI installed / on PATH?): {e}",
                self.cli_path.display()
            ))
        })?;

        // Drain stderr concurrently so a full stderr pipe can't deadlock stdout.
        let stderr_pipe = child.stderr.take();
        let stderr_task = tokio::spawn(async move {
            let mut buf = String::new();
            if let Some(pipe) = stderr_pipe {
                let _ = BufReader::new(pipe).read_to_string(&mut buf).await;
            }
            buf
        });

        let stdout = child.stdout.take().expect("stdout piped");
        let mut lines = BufReader::new(stdout).lines();
        let mut acc = String::new();
        // Live activity overlay: emit the node's latest line to the progress
        // sink, throttled so the SSE/UI never floods. `pending` holds the most
        // recent line seen since the last emit; it's flushed after the loop so
        // the final activity is never dropped. No-op when `progress` is None.
        let mut last_emit: Option<Instant> = None;
        let mut pending: Option<String> = None;
        const ACTIVITY_THROTTLE: Duration = Duration::from_millis(750);

        // Optional wall-clock ceiling. When unset, this future is `pending` —
        // it never fires, so only the idle watchdog can stop the call.
        let cap = self.timeout;
        let overall = async move {
            match cap {
                Some(d) => tokio::time::sleep(d).await,
                None => std::future::pending::<()>().await,
            }
        };
        tokio::pin!(overall);
        loop {
            let idle = tokio::time::sleep(self.idle_timeout);
            tokio::select! {
                read = lines.next_line() => match read {
                    Ok(Some(line)) => {
                        acc.push_str(&line);
                        acc.push('\n');
                        if let Some(sink) = progress {
                            if let Some(activity) = activity_from_line(&line) {
                                let due = last_emit
                                    .map(|t| t.elapsed() >= ACTIVITY_THROTTLE)
                                    .unwrap_or(true);
                                if due {
                                    sink.report(activity);
                                    last_emit = Some(Instant::now());
                                    pending = None;
                                } else {
                                    pending = Some(activity);
                                }
                            }
                        }
                    }
                    Ok(None) => break, // stdout closed → process is finishing
                    Err(e) => {
                        let _ = child.start_kill();
                        return Err(AgentError(format!("omp stdout read error: {e}")));
                    }
                },
                _ = &mut overall => {
                    let _ = child.start_kill();
                    let _ = child.wait().await;
                    let secs = self.timeout.map(|d| d.as_secs()).unwrap_or(0);
                    return Err(AgentError(format!(
                        "omp exceeded the wall-clock cap ({secs}s, OMP_TIMEOUT_SECS)"
                    )));
                }
                _ = idle => {
                    let _ = child.start_kill();
                    let _ = child.wait().await;
                    return Ok(Attempt::Stalled);
                }
            }
        }

        // Flush the final pending activity so the last line isn't lost to the throttle.
        if let (Some(sink), Some(activity)) = (progress, pending.take()) {
            sink.report(activity);
        }

        let status = child
            .wait()
            .await
            .map_err(|e| AgentError(format!("omp wait failed: {e}")))?;
        let stderr = stderr_task.await.unwrap_or_default();
        Ok(Attempt::Done {
            stdout: acc,
            stderr,
            status,
        })
    }
}

/// Paths the agent (and its build tools) may legitimately write to under the
/// opt-in filesystem sandbox: the run's worktree plus the package/build caches
/// and omp's own state. Everything else (toolchains in `$HOME/.local`, `/usr`,
/// `/usr/local`, …) stays read+execute only. Missing paths are simply skipped.
#[cfg(unix)]
fn sandbox_allowlist(cwd: &Path) -> Vec<PathBuf> {
    let mut paths = vec![
        cwd.to_path_buf(),
        PathBuf::from("/tmp"),
        PathBuf::from("/var/tmp"),
    ];
    if let Some(home) = std::env::var_os("HOME") {
        let home = PathBuf::from(home);
        for sub in [
            ".cargo", ".rustup", ".cache", ".bun", ".npm", ".config", ".omp",
        ] {
            paths.push(home.join(sub));
        }
    }
    for key in ["CARGO_TARGET_DIR", "XDG_CACHE_HOME"] {
        if let Some(v) = std::env::var_os(key) {
            paths.push(PathBuf::from(v));
        }
    }
    paths
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
    use serde_json::Value;
    let mut out = ParsedOmp::default();
    // omp reports tokens per assistant `message_end` (`message.usage`, fields
    // input/output/cacheRead/cacheWrite). The headless `agent_end` has NO
    // telemetry, so accumulate the per-message usage; if a future omp version
    // *does* emit an `agent_end` telemetry summary, that overrides the sum.
    let mut msg_usage = Usage::default();
    let mut summary: Option<Usage> = None;
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
                if let Some(u) = v.get("message").and_then(|m| m.get("usage")) {
                    add_usage(&mut msg_usage, &usage_from_value(u));
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
                    summary = Some(usage_from_value(usage));
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
    // A summary total (if any) is authoritative; otherwise use the per-message sum.
    out.usage = summary.unwrap_or(msg_usage);
    out
}

/// Add token counts from `b` into `a` (summing per-message usage across turns).
fn add_usage(a: &mut Usage, b: &Usage) {
    fn add(x: &mut Option<u64>, y: Option<u64>) {
        if let Some(v) = y {
            *x = Some(x.unwrap_or(0) + v);
        }
    }
    add(&mut a.input, b.input);
    add(&mut a.output, b.output);
    add(&mut a.cache_read, b.cache_read);
    add(&mut a.cache_write, b.cache_write);
}

/// Assistant text from a message `Value`. `content` may be a plain string or an
/// array of `{type:"text", text}` parts; non-assistant roles yield `None`.
fn assistant_text(msg: Option<&serde_json::Value>) -> Option<String> {
    use serde_json::Value;
    let msg = msg?;
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
        .collect::<Vec<_>>()
        .join("");
    (!text.is_empty()).then_some(text)
}

/// Extract a short "current activity" line from one omp stream event (one line
/// of its newline-delimited JSON), for the live node-progress overlay. Prefers a
/// tool action (`⚙ <tool>`), else the last non-empty line of assistant text.
/// Returns `None` for events with nothing useful to show (non-JSON, telemetry,
/// empty messages) so the caller emits nothing.
fn activity_from_line(line: &str) -> Option<String> {
    use serde_json::Value;
    let v: Value = serde_json::from_str(line.trim()).ok()?;
    let msg = v.get("message");
    // A tool call: surface the tool name (e.g. "⚙ bash").
    let content = msg.and_then(|m| m.get("content")).and_then(Value::as_array);
    for part in content.into_iter().flatten() {
        if part.get("type").and_then(Value::as_str) != Some("tool_use") {
            continue;
        }
        if let Some(name) = part.get("name").and_then(Value::as_str) {
            return Some(truncate_activity(&format!("⚙ {name}")));
        }
    }
    // Otherwise the last non-empty line of the assistant's text.
    let text = assistant_text(msg)?;
    let last = text.lines().map(str::trim).rfind(|l| !l.is_empty())?;
    Some(truncate_activity(last))
}

/// Trim and cap an activity string to a sensible single-line length, on a char
/// boundary, appending an ellipsis when truncated.
fn truncate_activity(s: &str) -> String {
    const MAX: usize = 120;
    let s = s.trim();
    if s.chars().count() <= MAX {
        return s.to_string();
    }
    let truncated: String = s.chars().take(MAX).collect();
    format!("{truncated}…")
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
    fn activity_from_line_extracts_text_tool_and_skips_noise() {
        // Assistant text → last non-empty line.
        let line = "{\"type\":\"message_end\",\"message\":{\"role\":\"assistant\",\"content\":[{\"type\":\"text\",\"text\":\"first\\n\\nsecond line\"}]}}";
        assert_eq!(activity_from_line(line).as_deref(), Some("second line"));

        // Tool use → "⚙ <name>" (preferred over any text in the same message).
        let tool = "{\"message\":{\"role\":\"assistant\",\"content\":[{\"type\":\"tool_use\",\"name\":\"bash\",\"input\":{}}]}}";
        assert_eq!(activity_from_line(tool).as_deref(), Some("⚙ bash"));

        // Non-JSON / telemetry / empty → None.
        assert_eq!(activity_from_line("not json"), None);
        assert_eq!(
            activity_from_line("{\"type\":\"agent_end\",\"telemetry\":{}}"),
            None
        );
    }

    #[test]
    fn truncate_activity_caps_on_char_boundary() {
        let short = "edit Source/Foo.al";
        assert_eq!(truncate_activity(short), short);
        let long = "x".repeat(200);
        let out = truncate_activity(&long);
        assert!(out.ends_with('…'));
        assert_eq!(out.chars().count(), 121); // 120 + ellipsis
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
    fn sums_per_message_usage_when_agent_end_has_no_telemetry() {
        // Real omp --mode json shape: usage is on each message_end's message
        // (input/output/cacheRead/cacheWrite), and agent_end has NO telemetry.
        // Sum the per-message usage; don't double-count agent_end.messages.
        let stream = concat!(
            "{\"type\":\"message_end\",\"message\":{\"role\":\"assistant\",\"content\":[{\"type\":\"text\",\"text\":\"a\"}],\"usage\":{\"input\":22893,\"output\":10,\"cacheRead\":100,\"cacheWrite\":0}}}\n",
            "{\"type\":\"message_end\",\"message\":{\"role\":\"assistant\",\"content\":[{\"type\":\"text\",\"text\":\"hi\"}],\"usage\":{\"input\":5485,\"output\":26,\"cacheRead\":17408,\"cacheWrite\":0}}}\n",
            "{\"type\":\"agent_end\",\"messages\":[{\"role\":\"assistant\",\"content\":[{\"type\":\"text\",\"text\":\"hi\"}],\"usage\":{\"input\":5485,\"output\":26}}]}\n"
        );
        let parsed = parse_omp_stream(stream);
        assert!(parsed.saw_end);
        assert_eq!(parsed.text, "hi");
        assert_eq!(parsed.usage.input, Some(22893 + 5485));
        assert_eq!(parsed.usage.output, Some(10 + 26));
        assert_eq!(parsed.usage.cache_read, Some(100 + 17408));
        assert_eq!(parsed.usage.cache_write, Some(0));
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
            agent.resolve_model(Some("kimi-k2.5")),
            "kimi-code/kimi-k2.5"
        );
        assert_eq!(agent.resolve_model(None), DEFAULT_MODEL);
    }

    #[test]
    fn resolve_model_passes_through_openai_codex_namespace() {
        let agent = PiAgent::from_env();
        // Namespace-qualified OpenAI Codex model → passed through unchanged
        // (ChatGPT subscription via omp). This is why the workflow uses
        // `openai-codex/gpt-5.5`, not a bare `gpt-5.5` (which would be
        // mis-prefixed to kimi-code/).
        assert_eq!(
            agent.resolve_model(Some("openai-codex/gpt-5.5")),
            "openai-codex/gpt-5.5"
        );
        assert_eq!(agent.resolve_model(Some("gpt-5.5")), "kimi-code/gpt-5.5");
    }

    #[test]
    fn build_args_adds_resume_when_session_present() {
        let agent = PiAgent {
            cli_path: PathBuf::from("omp"),
            default_model: DEFAULT_MODEL.to_string(),
            timeout: None,
            idle_timeout: Duration::from_secs(DEFAULT_IDLE_TIMEOUT_SECS),
            plugin_dirs: vec![],
        };
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

    // The idle watchdog is exercised against a real `sh` process (unix only; CI
    // is Linux). It must NOT depend on a wall-clock cap — only on output silence.
    #[cfg(unix)]
    fn agent_with(cli: &str, idle: Duration) -> PiAgent {
        PiAgent {
            cli_path: PathBuf::from(cli),
            default_model: DEFAULT_MODEL.to_string(),
            timeout: None, // no wall-clock cap — prove the idle watchdog alone works
            idle_timeout: idle,
            plugin_dirs: vec![],
        }
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn idle_watchdog_kills_a_stalled_process() {
        let agent = agent_with("sh", Duration::from_millis(300));
        // Prints one line, then goes silent for 30s — the watchdog must fire fast.
        let args = vec!["-c".to_string(), "printf 'a\\n'; sleep 30".to_string()];
        let started = std::time::Instant::now();
        let out = agent
            .run_attempt(&args, Path::new("."), &Default::default(), None)
            .await
            .unwrap();
        assert!(matches!(out, Attempt::Stalled));
        assert!(
            started.elapsed() < Duration::from_secs(5),
            "killed promptly"
        );
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn active_process_completes_without_being_killed() {
        // Emits a line every ~100ms for ~1s — never silent past the 300ms idle
        // window, so it must run to completion (proving active work isn't killed).
        let agent = agent_with("sh", Duration::from_millis(300));
        let args = vec![
            "-c".to_string(),
            "for i in 1 2 3 4 5 6 7 8 9 10; do printf 'line %s\\n' \"$i\"; sleep 0.1; done"
                .to_string(),
        ];
        let out = agent
            .run_attempt(&args, Path::new("."), &Default::default(), None)
            .await
            .unwrap();
        match out {
            Attempt::Done { stdout, status, .. } => {
                assert!(status.success());
                assert!(stdout.contains("line 10"), "ran to completion: {stdout:?}");
            }
            Attempt::Stalled => panic!("active process was wrongly killed as stalled"),
        }
    }
}
