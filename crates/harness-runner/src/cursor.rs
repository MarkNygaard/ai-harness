//! Cursor provider — a [`PromptAgent`] that shells out to the **`cursor-agent`**
//! CLI in headless mode. Selected for `provider: cursor`.
//!
//! Invocation: `cursor-agent -p "<prompt>" --output-format json --model <model>
//! --force --trust` (+ `--resume <id>` to continue a session). With
//! `--output-format json` the CLI prints a single JSON object on completion:
//! `{ type:"result", is_error, result, session_id, usage:{ inputTokens,
//! outputTokens, cacheReadTokens, cacheWriteTokens } }`. `-p` grants full
//! tool access (write + shell); `--force`/`--trust` keep it non-interactive.
//!
//! Models are bare Cursor ids (e.g. `composer`, `sonnet-4`, `gpt-5`); the node's
//! `model` is passed through verbatim, defaulting to [`DEFAULT_MODEL`]. Auth is
//! the `CURSOR_API_KEY` env var (a Cursor dashboard API key), materialized into
//! the run environment by the server like the other providers — we don't manage
//! it here. The idle watchdog mirrors [`crate::pi`]: a call is killed only if it
//! goes silent for `idle_timeout`, never while it is actively producing output.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::{Arc, Mutex, OnceLock, Weak};
use std::time::Duration;

use async_trait::async_trait;
use harness_dag::Usage;
use tokio::io::{AsyncBufReadExt, AsyncReadExt, BufReader};
use tokio::process::Command;

use crate::{AgentError, PromptAgent, PromptRequest, PromptResult};

/// Default model when a `cursor` node declares no `model` — Cursor's own model.
const DEFAULT_MODEL: &str = "composer";

/// Idle (no-output) watchdog: kill a call that emits no stdout for this long
/// (a silently dropped connection), never one that is actively working.
/// Overridable via `CURSOR_IDLE_TIMEOUT_SECS`.
const DEFAULT_IDLE_TIMEOUT_SECS: u64 = 900;
/// Embedded Cursor hook script, materialized to a temp dir at runtime and
/// registered via `<cwd>/.cursor/hooks.json`.
const CURSOR_HOOK_SCRIPT: &str = include_str!("../extensions/harness-cursor-hooks/hook.js");

/// Outcome of one `cursor-agent` invocation that didn't error outright.
enum Attempt {
    Done {
        stdout: String,
        stderr: String,
        status: std::process::ExitStatus,
    },
    /// No output for the idle window — killed as stalled (retryable once).
    Stalled,
}

/// A [`PromptAgent`] backed by the `cursor-agent` CLI. Selected for
/// `provider: cursor`.
pub struct CursorAgent {
    cli_path: PathBuf,
    default_model: String,
    /// Optional wall-clock hard ceiling (none by default — only the idle
    /// watchdog applies). Set via `CURSOR_TIMEOUT_SECS`.
    timeout: Option<Duration>,
    idle_timeout: Duration,
}

impl Default for CursorAgent {
    fn default() -> Self {
        Self::from_env()
    }
}

impl CursorAgent {
    /// Build from the environment: `CURSOR_AGENT_CLI`/`CURSOR_CLI` overrides the
    /// binary (default `cursor-agent`). `CURSOR_IDLE_TIMEOUT_SECS` tunes the idle
    /// watchdog; `CURSOR_TIMEOUT_SECS`, if set, adds a wall-clock hard ceiling.
    pub fn from_env() -> Self {
        let cli_path = std::env::var_os("CURSOR_AGENT_CLI")
            .or_else(|| std::env::var_os("CURSOR_CLI"))
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from("cursor-agent"));
        let secs = |key: &str| {
            std::env::var(key)
                .ok()
                .and_then(|v| v.parse::<u64>().ok())
                .map(Duration::from_secs)
        };
        Self {
            cli_path,
            default_model: DEFAULT_MODEL.to_string(),
            timeout: secs("CURSOR_TIMEOUT_SECS"),
            idle_timeout: secs("CURSOR_IDLE_TIMEOUT_SECS")
                .unwrap_or(Duration::from_secs(DEFAULT_IDLE_TIMEOUT_SECS)),
        }
    }

    fn resolve_model(&self, model: Option<&str>) -> String {
        match model {
            Some(m) if !m.trim().is_empty() => m.to_string(),
            _ => self.default_model.clone(),
        }
    }

    fn build_args(&self, prompt: &str, model: &str, session: Option<&str>) -> Vec<String> {
        let mut args = vec![
            "-p".to_string(),
            prompt.to_string(),
            "--output-format".to_string(),
            "json".to_string(),
            "--model".to_string(),
            model.to_string(),
            // Full, non-interactive tool access: write + shell, trusting the
            // run's worktree without a prompt.
            "--force".to_string(),
            "--trust".to_string(),
        ];
        if let Some(id) = session {
            args.push("--resume".to_string());
            args.push(id.to_string());
        }

        args
    }
}

/// Guards the materialized `<cwd>/.cursor/hooks.json`.  If the file already
/// existed we back up its contents and restore them on drop; otherwise we
/// delete the file. The `_lock` serializes writers per worktree so concurrent
/// Cursor runs cannot snapshot or restore each other's transient hook config.
/// The bundled hook script lives in `_dir`, a `TempDir` that cleans itself up.
struct CursorHooksGuard {
    hooks_json: std::path::PathBuf,
    _dir: tempfile::TempDir,
    _lock: Option<tokio::sync::OwnedMutexGuard<()>>,
    /// Original contents of `hooks_json` if it pre-existed; restored on drop.
    original: Option<Vec<u8>>,
}
impl Drop for CursorHooksGuard {
    fn drop(&mut self) {
        if let Some(ref contents) = self.original {
            let _ = std::fs::write(&self.hooks_json, contents);
        } else {
            let _ = std::fs::remove_file(&self.hooks_json);
        }
    }
}

fn cursor_hooks_mutex(cwd: &Path) -> Arc<tokio::sync::Mutex<()>> {
    static LOCKS: OnceLock<Mutex<HashMap<PathBuf, Weak<tokio::sync::Mutex<()>>>>> = OnceLock::new();
    let key = std::fs::canonicalize(cwd).unwrap_or_else(|_| cwd.to_path_buf());
    let locks = LOCKS.get_or_init(|| Mutex::new(HashMap::new()));
    let mut locks = locks.lock().expect("cursor hook lock map poisoned");
    if let Some(lock) = locks.get(&key).and_then(Weak::upgrade) {
        return lock;
    }
    let lock = Arc::new(tokio::sync::Mutex::new(()));
    locks.insert(key, Arc::downgrade(&lock));
    lock
}

fn ensure_cursor_hooks_path(cursor_dir: &Path, hooks_json: &Path) -> Result<(), AgentError> {
    if let Ok(metadata) = std::fs::symlink_metadata(cursor_dir) {
        if metadata.file_type().is_symlink() {
            return Err(AgentError(format!(
                "refusing to write Cursor hooks through symlinked directory `{}`",
                cursor_dir.display()
            )));
        }
        if !metadata.is_dir() {
            return Err(AgentError(format!(
                "refusing to write Cursor hooks because `{}` is not a directory",
                cursor_dir.display()
            )));
        }
    }
    if let Ok(metadata) = std::fs::symlink_metadata(hooks_json) {
        if metadata.file_type().is_symlink() {
            return Err(AgentError(format!(
                "refusing to overwrite symlinked Cursor hooks file `{}`",
                hooks_json.display()
            )));
        }
        if !metadata.is_file() {
            return Err(AgentError(format!(
                "refusing to overwrite Cursor hooks path `{}` because it is not a file",
                hooks_json.display()
            )));
        }
    }
    Ok(())
}

#[async_trait]
impl PromptAgent for CursorAgent {
    async fn run(&self, req: PromptRequest) -> Result<PromptResult, AgentError> {
        let model = self.resolve_model(req.model.as_deref());
        let args = self.build_args(&req.prompt, &model, req.session.as_deref());

        let mut env_vars = req.env_vars.clone();
        let _hooks_guard = if let Some(hooks) = req.hooks.as_ref() {
            let hooks_lock = cursor_hooks_mutex(&req.cwd).lock_owned().await;
            let dir = tempfile::Builder::new()
                .prefix("harness-cursor-hooks-")
                .tempdir()
                .map_err(|e| AgentError(e.to_string()))?;
            let script_path = dir.path().join("hook.js");
            std::fs::write(&script_path, CURSOR_HOOK_SCRIPT)
                .map_err(|e| AgentError(e.to_string()))?;

            let cursor_dir = req.cwd.join(".cursor");
            let hooks_json = cursor_dir.join("hooks.json");
            ensure_cursor_hooks_path(&cursor_dir, &hooks_json)?;
            std::fs::create_dir_all(&cursor_dir).map_err(|e| AgentError(e.to_string()))?;
            let original = std::fs::read(&hooks_json).ok();
            let value = crate::hooks::cursor_hooks_json(hooks, &script_path);
            let value = crate::hooks::merge_cursor_hooks_json(original.as_deref(), value);
            std::fs::write(&hooks_json, serde_json::to_string_pretty(&value).unwrap())
                .map_err(|e| AgentError(e.to_string()))?;
            env_vars.insert("HARNESS_HOOKS".into(), crate::hooks::omp_hooks_env(hooks));
            Some(CursorHooksGuard {
                hooks_json,
                _dir: dir,
                _lock: Some(hooks_lock),
                original,
            })
        } else {
            None
        };

        // One automatic retry, but ONLY on a stall (a transient dropped
        // connection). A clean non-zero exit is deterministic — never retried.
        let mut attempt = 0u32;
        let (stdout, stderr, status) = loop {
            match self.run_attempt(&args, &req.cwd, &env_vars).await? {
                Attempt::Done {
                    stdout,
                    stderr,
                    status,
                } => break (stdout, stderr, status),
                Attempt::Stalled => {
                    if attempt == 0 {
                        attempt += 1;
                        tracing::warn!(
                            "cursor-agent produced no output for {}s — stalled; retrying once",
                            self.idle_timeout.as_secs()
                        );
                        continue;
                    }
                    return Err(AgentError(format!(
                        "cursor-agent stalled (no output for {}s) and again after one retry",
                        self.idle_timeout.as_secs()
                    )));
                }
            }
        };

        let parsed = parse_cursor_output(&stdout);
        // Success = clean exit AND the CLI reported a non-error result.
        if !status.success() || parsed.is_error || parsed.text.is_empty() {
            let tail: String = stderr.trim().chars().rev().take(500).collect::<String>();
            let tail: String = tail.chars().rev().collect();
            return Err(AgentError(format!(
                "cursor-agent did not complete (exit={:?}, is_error={}, text={}B): {tail}",
                status.code(),
                parsed.is_error,
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

impl CursorAgent {
    /// Run `cursor-agent` once, streaming stdout so an idle watchdog can kill a
    /// stalled call without ever stopping an actively-producing step.
    async fn run_attempt(
        &self,
        args: &[String],
        cwd: &Path,
        env_vars: &HashMap<String, String>,
    ) -> Result<Attempt, AgentError> {
        let mut cmd = Command::new(&self.cli_path);
        cmd.args(args)
            .current_dir(cwd)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true);
        // Never let the agent (or tools it spawns) inherit the control-plane DB
        // URL / secrets.
        harness_agents::strip_control_plane_env(&mut cmd);
        cmd.envs(env_vars);

        let mut child = cmd.spawn().map_err(|e| {
            AgentError(format!(
                "failed to spawn `{}` (is the cursor-agent CLI installed / on PATH?): {e}",
                self.cli_path.display()
            ))
        })?;

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
                    }
                    Ok(None) => break,
                    Err(e) => {
                        let _ = child.start_kill();
                        return Err(AgentError(format!("cursor-agent stdout read error: {e}")));
                    }
                },
                _ = &mut overall => {
                    let _ = child.start_kill();
                    let _ = child.wait().await;
                    let secs = self.timeout.map(|d| d.as_secs()).unwrap_or(0);
                    return Err(AgentError(format!(
                        "cursor-agent exceeded the wall-clock cap ({secs}s, CURSOR_TIMEOUT_SECS)"
                    )));
                }
                _ = idle => {
                    let _ = child.start_kill();
                    let _ = child.wait().await;
                    return Ok(Attempt::Stalled);
                }
            }
        }

        let status = child
            .wait()
            .await
            .map_err(|e| AgentError(format!("cursor-agent wait failed: {e}")))?;
        let stderr = stderr_task.await.unwrap_or_default();
        Ok(Attempt::Done {
            stdout: acc,
            stderr,
            status,
        })
    }
}

/// The distilled result of a `cursor-agent --output-format json` run.
#[derive(Debug, Default, PartialEq)]
pub struct ParsedCursor {
    pub text: String,
    pub session: Option<String>,
    pub usage: Usage,
    pub is_error: bool,
}

/// Parse `cursor-agent`'s output into a [`ParsedCursor`]. With
/// `--output-format json` the completion is a single JSON object with
/// `type:"result"`; we scan lines for it (tolerating any leading noise) and
/// fall back to an empty result if none is found.
pub fn parse_cursor_output(stdout: &str) -> ParsedCursor {
    use serde_json::Value;
    let mut out = ParsedCursor::default();
    for line in stdout.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let Ok(v) = serde_json::from_str::<Value>(line) else {
            continue;
        };
        // The terminal event is `type:"result"`; ignore any stream events.
        if v.get("type").and_then(Value::as_str) != Some("result") {
            continue;
        }
        if let Some(text) = v.get("result").and_then(Value::as_str) {
            out.text = text.to_string();
        }
        if let Some(id) = v.get("session_id").and_then(Value::as_str) {
            if !id.is_empty() {
                out.session = Some(id.to_string());
            }
        }
        out.is_error = v.get("is_error").and_then(Value::as_bool).unwrap_or(false)
            || v.get("subtype").and_then(Value::as_str) == Some("error");
        if let Some(u) = v.get("usage") {
            out.usage = usage_from_value(u);
        }
    }
    out
}

/// Token usage from cursor's `usage` object (camelCase `*Tokens` keys).
fn usage_from_value(u: &serde_json::Value) -> Usage {
    let pick = |keys: &[&str]| {
        keys.iter()
            .find_map(|k| u.get(*k).and_then(serde_json::Value::as_u64))
    };
    Usage {
        input: pick(&["inputTokens", "input_tokens", "input"]),
        output: pick(&["outputTokens", "output_tokens", "output"]),
        cache_read: pick(&["cacheReadTokens", "cache_read_tokens", "cacheRead"]),
        cache_write: pick(&["cacheWriteTokens", "cache_write_tokens", "cacheWrite"]),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_result_text_session_and_usage() {
        // The exact shape emitted by `cursor-agent -p --output-format json`.
        let stdout = r#"{"type":"result","subtype":"success","is_error":false,"duration_ms":7329,"result":"pong","session_id":"sess-42","request_id":"req-1","usage":{"inputTokens":5648,"outputTokens":56,"cacheReadTokens":5382,"cacheWriteTokens":0}}"#;
        let parsed = parse_cursor_output(stdout);
        assert_eq!(parsed.text, "pong");
        assert_eq!(parsed.session.as_deref(), Some("sess-42"));
        assert!(!parsed.is_error);
        assert_eq!(parsed.usage.input, Some(5648));
        assert_eq!(parsed.usage.output, Some(56));
        assert_eq!(parsed.usage.cache_read, Some(5382));
        assert_eq!(parsed.usage.cache_write, Some(0));
    }

    #[test]
    fn flags_error_result() {
        let stdout = r#"{"type":"result","subtype":"error","is_error":true,"result":"boom"}"#;
        let parsed = parse_cursor_output(stdout);
        assert!(parsed.is_error);
    }

    #[test]
    fn ignores_non_result_lines_and_garbage() {
        let stdout = concat!(
            "not json\n",
            "{\"type\":\"assistant\",\"message\":\"thinking...\"}\n",
            "{\"type\":\"result\",\"is_error\":false,\"result\":\"done\",\"session_id\":\"s1\"}\n"
        );
        let parsed = parse_cursor_output(stdout);
        assert_eq!(parsed.text, "done");
        assert_eq!(parsed.session.as_deref(), Some("s1"));
    }

    #[test]
    fn empty_output_is_default() {
        assert_eq!(parse_cursor_output("\n  \n"), ParsedCursor::default());
    }

    #[test]
    fn resolve_model_defaults_and_passes_through() {
        let agent = CursorAgent::from_env();
        assert_eq!(agent.resolve_model(None), DEFAULT_MODEL);
        assert_eq!(agent.resolve_model(Some("")), DEFAULT_MODEL);
        assert_eq!(agent.resolve_model(Some("sonnet-4")), "sonnet-4");
        assert_eq!(agent.resolve_model(Some("composer-2.5")), "composer-2.5");
    }

    #[test]
    fn build_args_shape_and_resume() {
        let agent = CursorAgent::from_env();
        let base = agent.build_args("do it", "composer", None);
        assert_eq!(
            base,
            vec![
                "-p",
                "do it",
                "--output-format",
                "json",
                "--model",
                "composer",
                "--force",
                "--trust",
            ]
        );
        let resumed = agent.build_args("again", "composer", Some("sess-9"));
        assert!(resumed.windows(2).any(|w| w == ["--resume", "sess-9"]));
    }
    #[test]
    fn cursor_hooks_guard_restores_pre_existing_file() {
        let dir = tempfile::tempdir().unwrap();
        let cursor_dir = dir.path().join(".cursor");
        std::fs::create_dir_all(&cursor_dir).unwrap();
        let hooks_json = cursor_dir.join("hooks.json");
        std::fs::write(&hooks_json, b"original").unwrap();
        {
            let script_dir = tempfile::tempdir().unwrap();
            let guard = CursorHooksGuard {
                hooks_json: hooks_json.clone(),
                _dir: script_dir,
                _lock: None,
                original: Some(b"original".to_vec()),
            };
            std::fs::write(&hooks_json, b"overwritten").unwrap();
            drop(guard);
        }
        assert_eq!(std::fs::read(&hooks_json).unwrap(), b"original");
    }
    #[test]
    fn cursor_hooks_guard_deletes_file_when_no_original() {
        let dir = tempfile::tempdir().unwrap();
        let cursor_dir = dir.path().join(".cursor");
        std::fs::create_dir_all(&cursor_dir).unwrap();
        let hooks_json = cursor_dir.join("hooks.json");
        std::fs::write(&hooks_json, b"temp").unwrap();
        {
            let script_dir = tempfile::tempdir().unwrap();
            let guard = CursorHooksGuard {
                hooks_json: hooks_json.clone(),
                _dir: script_dir,
                _lock: None,
                original: None,
            };
            drop(guard);
        }
        assert!(!hooks_json.exists());
    }
}
