//! **Chat turns** — one message in a conversation with an agent whose only
//! tools are the harness's own MCP tools.
//!
//! Separate from [`crate::PromptAgent`] on purpose. A workflow node and a chat
//! turn want different things from the same CLIs: a node runs with the full tool
//! profile in a worktree and starts fresh, while a chat turn must resume the
//! conversation it belongs to, reach the harness over MCP, and be denied every
//! other tool **at the CLI boundary** (a prompt asking it not to use a shell is
//! a request, not a control). Bending the node path to fit would put all three
//! on code every workflow run goes through.
//!
//! Two backends, chosen in Settings → Chat:
//!
//! - **`claude`** — `--mcp-config` + `--strict-mcp-config` attach only the
//!   harness; `--tools ""` removes every built-in; `--allowedTools
//!   mcp__harness__*` pre-approves the harness tools; `--permission-mode
//!   dontAsk` denies anything else instead of waiting on a prompt nobody sees.
//! - **`pi`** (`omp`) — an `.mcp.json` in the conversation's directory attaches
//!   the harness; `--tools` is an exact allowlist naming each harness tool, so
//!   the built-ins (bash, edit, read, …) are not active at all.
//!
//! The bearer token never appears in argv or on disk: both config files say
//! `${HARNESS_CHAT_TOKEN}`, which the CLI expands from the child's environment.

use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::Duration;

use harness_dag::{Activity, ProgressSink, Usage};
use serde_json::{json, Value};
use tokio::io::{AsyncBufReadExt, AsyncReadExt, BufReader};

use crate::AgentError;

/// Providers that can back a chat. Each needs a way to attach an MCP server and
/// to deny every other tool; a provider that can't do both is not offered.
pub const CHAT_PROVIDERS: &[&str] = &["claude", "pi"];

/// The MCP server name the harness is attached under. Fixes the tool names the
/// agent sees (`mcp__harness__run_list` for Claude, `mcp__harness_run_list` for
/// omp).
const SERVER_NAME: &str = "harness";

/// Environment variable the MCP config reads the bearer token from.
pub const TOKEN_ENV: &str = "HARNESS_CHAT_TOKEN";

/// Claude's MCP config file, in the conversation directory. Not `.mcp.json`:
/// that name is a *project* config Claude would also pick up on its own, which
/// `--strict-mcp-config` makes moot but there is no reason to rely on.
const CLAUDE_MCP_FILE: &str = ".harness-chat-mcp.json";
/// omp discovers `mcp.json` / `.mcp.json` in its working directory.
const OMP_MCP_FILE: &str = ".mcp.json";

/// The omp extension every Pi chat turn runs with: a tool allowlist, and the
/// confined `code_*` tools. See its header for why it exists.
const OMP_GUARD_TS: &str = include_str!("../extensions/harness-chat-guard/index.ts");
/// Where the guard is written in the conversation directory.
const OMP_GUARD_FILE: &str = ".harness-chat-guard.ts";
/// Tells the guard which checkout its code tools may read.
const OMP_CODE_ROOT_ENV: &str = "HARNESS_CHAT_CODE_ROOT";

/// A turn that takes longer than this is killed. Generous — a turn that reads a
/// dozen runs is several tool calls — but finite, so a wedged CLI cannot hold a
/// conversation "thinking" forever.
const TURN_TIMEOUT: Duration = Duration::from_secs(10 * 60);

/// omp's default when the setting names no model (mirrors [`crate::PiAgent`]).
const OMP_DEFAULT_MODEL: &str = "kimi-code/kimi-for-coding";

/// Whether `provider` can back a chat.
pub fn supports_chat(provider: &str) -> bool {
    CHAT_PROVIDERS.contains(&provider)
}

/// How the agent reaches the harness.
#[derive(Debug, Clone)]
pub struct ChatMcp {
    /// The `/mcp` endpoint, e.g. `http://127.0.0.1:9800/mcp`.
    pub url: String,
    /// A short-lived token that identifies the conversation and its owner.
    pub token: String,
    /// The harness tool names to allow (unprefixed, e.g. `run_list`).
    pub tools: Vec<String>,
}

/// One message to send.
#[derive(Debug)]
pub struct ChatTurn {
    /// `claude` or `pi`.
    pub provider: String,
    pub model: Option<String>,
    /// Reasoning effort (`low`..`max`); omp calls it thinking.
    pub effort: Option<String>,
    pub prompt: String,
    /// The conversation's own directory. Must be the same on every turn: Claude
    /// keys its stored sessions by working directory, so a resume from anywhere
    /// else finds nothing.
    pub cwd: PathBuf,
    /// A read-only checkout of a project to answer code questions from. Claude
    /// runs *in* it with `Read`/`Grep`/`Glob`, confined by its permission
    /// rules; omp gets the chat guard's `code_*` tools over it. The MCP config
    /// and the conversation's own state stay in `cwd`. Must also be the same
    /// every turn, for the same reason.
    pub code_dir: Option<PathBuf>,
    /// The session to resume; `None` on the first turn.
    pub session: Option<String>,
    pub mcp: ChatMcp,
    /// Live tool activity, for the "looking up runs…" lines while it works.
    pub progress: Option<ProgressSink>,
}

/// What came back.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct ChatReply {
    pub text: String,
    /// The session to resume next turn.
    pub session: Option<String>,
    pub usage: Usage,
}

/// Run one turn to completion.
pub async fn run_turn(turn: ChatTurn) -> Result<ChatReply, AgentError> {
    std::fs::create_dir_all(&turn.cwd)
        .map_err(|e| AgentError(format!("chat directory {}: {e}", turn.cwd.display())))?;
    let config = mcp_config(&turn.mcp.url);
    match turn.provider.as_str() {
        "claude" => {
            let file = turn.cwd.join(CLAUDE_MCP_FILE);
            write_config(&file, &config)?;
            let args = claude_args(&turn, &file);
            let out = spawn(&cli("HARNESS_CHAT_CLAUDE_CLI", "claude"), &args, &turn).await?;
            let reply = parse_claude_stream(&out.stdout);
            finish(reply, out, turn.session)
        }
        "pi" => {
            write_config(&turn.cwd.join(OMP_MCP_FILE), &config)?;
            // Absolute: `--trusted-extension` refuses a relative path.
            let guard = std::path::absolute(turn.cwd.join(OMP_GUARD_FILE))
                .map_err(|e| AgentError(format!("chat guard path: {e}")))?;
            std::fs::write(&guard, OMP_GUARD_TS)
                .map_err(|e| AgentError(format!("write {}: {e}", guard.display())))?;
            let args = omp_args(&turn, &guard);
            let out = spawn(&cli("OMP_CLI", "omp"), &args, &turn).await?;
            let parsed = crate::pi::parse_omp_stream(&out.stdout);
            let reply = ChatReply {
                text: parsed.text,
                session: parsed.session,
                usage: parsed.usage,
            };
            finish(reply, out, turn.session)
        }
        other => Err(AgentError(format!(
            "`{other}` cannot back the chat (supported: {})",
            CHAT_PROVIDERS.join(", ")
        ))),
    }
}

/// An empty answer from a clean exit is still a failure to the person waiting
/// on it; say what the CLI said instead.
fn finish(
    mut reply: ChatReply,
    out: Output,
    prior_session: Option<String>,
) -> Result<ChatReply, AgentError> {
    if reply.text.trim().is_empty() {
        let why = out.stderr.trim();
        return Err(AgentError(if why.is_empty() {
            format!("the agent exited ({}) without an answer", out.status)
        } else {
            format!("the agent exited ({}) without an answer: {why}", out.status)
        }));
    }
    // A CLI that didn't report a session is still in the one it resumed.
    reply.session = reply.session.or(prior_session);
    Ok(reply)
}

fn cli(env: &str, default: &str) -> PathBuf {
    std::env::var_os(env)
        .filter(|v| !v.is_empty())
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(default))
}

/// The MCP config both CLIs read. The token is a reference, not a value.
pub fn mcp_config(url: &str) -> Value {
    json!({
        "mcpServers": {
            SERVER_NAME: {
                "type": "http",
                "url": url,
                "headers": { "Authorization": format!("Bearer ${{{TOKEN_ENV}}}") }
            }
        }
    })
}

fn write_config(path: &Path, config: &Value) -> Result<(), AgentError> {
    std::fs::write(path, serde_json::to_vec_pretty(config).unwrap_or_default())
        .map_err(|e| AgentError(format!("write {}: {e}", path.display())))
}

/// Claude's flags. The prompt is the value of `-p`, never a trailing positional.
pub fn claude_args(turn: &ChatTurn, mcp_file: &Path) -> Vec<String> {
    let mut args = vec![
        "-p".into(),
        turn.prompt.clone(),
        "--output-format".into(),
        "stream-json".into(),
        "--verbose".into(),
        "--mcp-config".into(),
        mcp_file.to_string_lossy().into_owned(),
        "--strict-mcp-config".into(),
        // No shell, no writes, no web. Reading files only in code mode.
        "--tools".into(),
        if turn.code_dir.is_some() {
            "Read,Grep,Glob".into()
        } else {
            String::new()
        },
        "--allowedTools".into(),
        format!("mcp__{SERVER_NAME}__*"),
        "--permission-mode".into(),
        "dontAsk".into(),
        // Only the harness's own user settings. A project's
        // `.claude/settings.json` can define hooks — shell commands — and the
        // checkout is the team's repo; Claude loads those from its working
        // directory unless told not to.
        "--setting-sources".into(),
        "user".into(),
    ];
    if turn.code_dir.is_some() {
        // `dontAsk` already refuses a read outside the working directory; this
        // makes the file tools refuse it themselves, Grep and Glob included.
        args.push("--settings".into());
        args.push(r#"{"permissions":{"blockReadsOutsideWorkingDirectories":true}}"#.into());
    }
    if let Some(model) = turn.model.as_deref().filter(|m| !m.is_empty()) {
        args.push("--model".into());
        args.push(model.into());
    }
    if let Some(effort) = turn.effort.as_deref().filter(|e| !e.is_empty()) {
        args.push("--effort".into());
        args.push(effort.into());
    }
    if let Some(session) = turn.session.as_deref() {
        args.push("--resume".into());
        args.push(session.into());
    }
    args
}

/// omp's name for a harness tool: `mcp__<server>_<tool>`, lowercased with
/// anything outside `[a-z_]` folded to `_` (omp's `createMCPToolName`).
pub fn omp_tool_name(tool: &str) -> String {
    let mut sanitized = String::with_capacity(tool.len());
    for c in tool.chars() {
        let c = c.to_ascii_lowercase();
        let c = if c.is_ascii_lowercase() || c == '_' {
            c
        } else {
            '_'
        };
        if !(c == '_' && sanitized.ends_with('_')) {
            sanitized.push(c);
        }
    }
    let sanitized = sanitized.trim_matches('_');
    let tool = sanitized
        .strip_prefix(&format!("{SERVER_NAME}_"))
        .unwrap_or(sanitized);
    format!("mcp__{SERVER_NAME}_{tool}")
}

/// The guard's own code tools, which stand in for omp's unconfinable
/// read/grep/glob when a turn has a checkout.
const OMP_CODE_TOOLS: &[&str] = &["code_read", "code_grep", "code_glob"];

/// omp's flags. `--tools` is the *whole* active set, so naming only harness
/// tools (and, with a checkout, the guard's `code_*` tools) leaves omp's own
/// bash/edit/read/write switched off. `--trusted-extension` loads the guard,
/// disables every other extension, and fails the turn if the guard won't load
/// — so the `code_*` tools never exist without their confinement.
pub fn omp_args(turn: &ChatTurn, guard: &Path) -> Vec<String> {
    let model = match turn.model.as_deref().filter(|m| !m.is_empty()) {
        Some(m) if m.contains('/') => m.to_string(),
        Some(m) => format!("kimi-code/{m}"),
        None => OMP_DEFAULT_MODEL.to_string(),
    };
    let mut tools: Vec<String> = turn.mcp.tools.iter().map(|t| omp_tool_name(t)).collect();
    if turn.code_dir.is_some() {
        tools.extend(OMP_CODE_TOOLS.iter().map(|t| t.to_string()));
    }
    let mut args = vec![
        "-p".into(),
        turn.prompt.clone(),
        "--mode".into(),
        "json".into(),
        "--model".into(),
        model,
        "--tools".into(),
        tools.join(","),
        "--trusted-extension".into(),
        guard.to_string_lossy().into_owned(),
    ];
    if let Some(effort) = turn.effort.as_deref().filter(|e| !e.is_empty()) {
        args.push("--thinking".into());
        args.push(effort.into());
    }
    if let Some(session) = turn.session.as_deref() {
        args.push("--resume".into());
        args.push(session.into());
    }
    args
}

struct Output {
    stdout: String,
    stderr: String,
    status: std::process::ExitStatus,
}

/// Spawn the CLI, forward live activity, and collect its output.
async fn spawn(program: &Path, args: &[String], turn: &ChatTurn) -> Result<Output, AgentError> {
    let mut cmd = tokio::process::Command::new(program);
    let claude = turn.provider == "claude";
    // Claude runs *in* the checkout, confined by its own permission rules. omp
    // stays in the conversation directory (where its `.mcp.json` is found)
    // and is told the checkout by environment, for the guard's code tools.
    let cwd = match (&turn.code_dir, claude) {
        (Some(code), true) => code,
        _ => &turn.cwd,
    };
    if let (Some(code), false) = (&turn.code_dir, claude) {
        cmd.env(OMP_CODE_ROOT_ENV, code);
    }
    cmd.args(args)
        .current_dir(cwd)
        .env(TOKEN_ENV, &turn.mcp.token)
        // Spawned from inside a Claude Code session (a developer running the
        // server locally), these make the child think it is nested and trap.
        .env_remove("CLAUDECODE")
        .env_remove("CLAUDE_CODE_ENTRYPOINT")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    let mut child = cmd
        .spawn()
        .map_err(|e| AgentError(format!("could not start {}: {e}", program.display())))?;
    let stdout = child.stdout.take().expect("piped stdout");
    let mut stderr = child.stderr.take().expect("piped stderr");
    let claude = turn.provider == "claude";
    let progress = turn.progress.clone();

    let read = async move {
        let mut lines = BufReader::new(stdout).lines();
        let mut all = String::new();
        while let Ok(Some(line)) = lines.next_line().await {
            if let Some(sink) = &progress {
                let acts = if claude {
                    claude_activities(&line)
                } else {
                    crate::pi::activities_from_line(&line)
                };
                for act in acts {
                    sink.report(act);
                }
            }
            all.push_str(&line);
            all.push('\n');
        }
        let mut err = String::new();
        let _ = stderr.read_to_string(&mut err).await;
        (all, err)
    };
    let ((stdout, stderr), status) = tokio::time::timeout(TURN_TIMEOUT, async {
        let out = read.await;
        (out, child.wait().await)
    })
    .await
    .map_err(|_| {
        AgentError(format!(
            "the agent did not answer within {} minutes",
            TURN_TIMEOUT.as_secs() / 60
        ))
    })?;
    let status = status.map_err(|e| AgentError(e.to_string()))?;
    Ok(Output {
        stdout,
        stderr,
        status,
    })
}

/// Strip the server prefix so a feed line reads `run_list`, not
/// `mcp__harness__run_list`.
fn short_tool_name(name: &str) -> &str {
    name.strip_prefix(&format!("mcp__{SERVER_NAME}__"))
        .or_else(|| name.strip_prefix(&format!("mcp__{SERVER_NAME}_")))
        .unwrap_or(name)
}

/// Live activity from one line of Claude's `stream-json`: a tool call per
/// `tool_use` block, a result per `tool_result`.
pub fn claude_activities(line: &str) -> Vec<Activity> {
    let Ok(v) = serde_json::from_str::<Value>(line.trim()) else {
        return Vec::new();
    };
    let blocks = v
        .get("message")
        .and_then(|m| m.get("content"))
        .and_then(Value::as_array);
    let Some(blocks) = blocks else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for b in blocks {
        match b.get("type").and_then(Value::as_str) {
            Some("tool_use") => {
                let name = b.get("name").and_then(Value::as_str).unwrap_or("tool");
                out.push(Activity::tool(
                    short_tool_name(name),
                    crate::pi::tool_input_detail(b.get("input")),
                    b.get("id").and_then(Value::as_str).map(str::to_string),
                ));
            }
            Some("tool_result") => {
                let text = match b.get("content") {
                    Some(Value::String(s)) => s.clone(),
                    Some(Value::Array(parts)) => parts
                        .iter()
                        .filter_map(|p| p.get("text").and_then(Value::as_str))
                        .collect::<Vec<_>>()
                        .join("\n"),
                    _ => String::new(),
                };
                let first = text.lines().map(str::trim).find(|l| !l.is_empty());
                if let Some(first) = first {
                    out.push(Activity::tool_result(
                        crate::pi::truncate_activity(first),
                        b.get("tool_use_id")
                            .and_then(Value::as_str)
                            .map(str::to_string),
                        b.get("is_error").and_then(Value::as_bool).unwrap_or(false),
                    ));
                }
            }
            _ => {}
        }
    }
    out
}

/// The answer, session and usage from Claude's `stream-json`. The `result`
/// event is authoritative; without one (a killed CLI) the last assistant text
/// stands in.
pub fn parse_claude_stream(stdout: &str) -> ChatReply {
    let mut reply = ChatReply::default();
    let mut last_text = String::new();
    let mut result: Option<String> = None;
    for line in stdout.lines() {
        let Ok(v) = serde_json::from_str::<Value>(line.trim()) else {
            continue;
        };
        if let Some(id) = v.get("session_id").and_then(Value::as_str) {
            if !id.is_empty() {
                reply.session = Some(id.to_string());
            }
        }
        match v.get("type").and_then(Value::as_str) {
            Some("assistant") => {
                let text: Vec<&str> = v
                    .get("message")
                    .and_then(|m| m.get("content"))
                    .and_then(Value::as_array)
                    .map(|blocks| {
                        blocks
                            .iter()
                            .filter(|b| b.get("type").and_then(Value::as_str) == Some("text"))
                            .filter_map(|b| b.get("text").and_then(Value::as_str))
                            .collect()
                    })
                    .unwrap_or_default();
                if !text.is_empty() {
                    last_text = text.join("");
                }
            }
            Some("result") => {
                // An error result's text is the error, not an answer.
                let is_error = v.get("is_error").and_then(Value::as_bool).unwrap_or(false);
                if !is_error {
                    result = v.get("result").and_then(Value::as_str).map(str::to_string);
                }
                if let Some(u) = v.get("usage") {
                    let n = |k: &str| u.get(k).and_then(Value::as_u64);
                    reply.usage = Usage {
                        input: n("input_tokens"),
                        output: n("output_tokens"),
                        cache_read: n("cache_read_input_tokens"),
                        cache_write: n("cache_creation_input_tokens"),
                    };
                }
            }
            _ => {}
        }
    }
    reply.text = result.filter(|r| !r.trim().is_empty()).unwrap_or(last_text);
    reply
}

#[cfg(test)]
mod tests {
    use super::*;
    use harness_dag::ActivityKind;

    fn turn(provider: &str, session: Option<&str>) -> ChatTurn {
        ChatTurn {
            provider: provider.into(),
            model: Some("sonnet".into()),
            effort: None,
            prompt: "which PR do I test?".into(),
            cwd: PathBuf::from("/data/chat/c1"),
            code_dir: None,
            session: session.map(str::to_string),
            mcp: ChatMcp {
                url: "http://127.0.0.1:9800/mcp".into(),
                token: "hrn_chat_secret".into(),
                tools: vec!["run_list".into(), "issue_prs".into()],
            },
            progress: None,
        }
    }

    #[test]
    fn the_config_references_the_token_instead_of_holding_it() {
        let cfg = mcp_config("http://127.0.0.1:9800/mcp");
        let server = &cfg["mcpServers"]["harness"];
        assert_eq!(server["type"], "http");
        assert_eq!(server["url"], "http://127.0.0.1:9800/mcp");
        assert_eq!(
            server["headers"]["Authorization"],
            "Bearer ${HARNESS_CHAT_TOKEN}"
        );
    }

    #[test]
    fn code_mode_adds_read_only_file_tools_confined_to_the_checkout() {
        let mut t = turn("claude", None);
        t.code_dir = Some(PathBuf::from("/data/chat-checkouts/shop/abc"));
        let args = claude_args(&t, Path::new("/data/chat/c1/mcp.json"));
        let after = |flag: &str| args.windows(2).find(|w| w[0] == flag).map(|w| w[1].clone());
        assert_eq!(after("--tools").as_deref(), Some("Read,Grep,Glob"));
        assert_eq!(
            after("--settings").as_deref(),
            Some(r#"{"permissions":{"blockReadsOutsideWorkingDirectories":true}}"#)
        );
        // A repo's own .claude/settings.json (hooks!) is never loaded.
        assert_eq!(after("--setting-sources").as_deref(), Some("user"));
        assert!(!args.iter().any(|a| a == "--add-dir"));
    }

    #[test]
    fn omp_always_loads_the_guard_and_reads_code_only_through_it() {
        let guard = Path::new("/data/chat/c1/.harness-chat-guard.ts");
        let after = |args: &[String], flag: &str| {
            args.windows(2).find(|w| w[0] == flag).map(|w| w[1].clone())
        };
        let plain = omp_args(&turn("pi", None), guard);
        assert_eq!(
            after(&plain, "--trusted-extension").as_deref(),
            Some("/data/chat/c1/.harness-chat-guard.ts")
        );
        assert!(!after(&plain, "--tools").unwrap().contains("code_"));

        let mut t = turn("pi", None);
        t.code_dir = Some(PathBuf::from("/data/chat-checkouts/shop/abc"));
        let code = omp_args(&t, guard);
        let tools = after(&code, "--tools").unwrap();
        assert!(tools.ends_with(",code_read,code_grep,code_glob"), "{tools}");
        // omp's own file tools never appear, whatever the mode.
        for builtin in ["read", "grep", "glob", "bash"] {
            assert!(!tools.split(',').any(|t| t == builtin), "{tools}");
        }
    }

    /// The guard's own tests (path confinement, the tool allowlist) are
    /// TypeScript; run them with bun when it is installed, as it is in CI.
    #[test]
    fn the_guard_extensions_own_tests_pass() {
        let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("extensions/harness-chat-guard");
        let out = match std::process::Command::new("bun")
            .arg("test")
            .current_dir(&dir)
            .output()
        {
            Ok(out) => out,
            Err(_) => {
                eprintln!("skipping: bun is not installed");
                return;
            }
        };
        assert!(
            out.status.success(),
            "bun test failed:\n{}\n{}",
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr)
        );
    }

    #[test]
    fn claude_gets_no_built_in_tools_and_only_the_harness_server() {
        let args = claude_args(&turn("claude", Some("s-1")), Path::new("/x/mcp.json"));
        // The prompt is the value of -p.
        assert_eq!(args[0], "-p");
        assert_eq!(args[1], "which PR do I test?");
        let after = |flag: &str| args.windows(2).find(|w| w[0] == flag).map(|w| w[1].clone());
        assert_eq!(after("--tools").as_deref(), Some(""));
        assert_eq!(after("--allowedTools").as_deref(), Some("mcp__harness__*"));
        assert_eq!(after("--permission-mode").as_deref(), Some("dontAsk"));
        assert_eq!(after("--resume").as_deref(), Some("s-1"));
        assert_eq!(after("--setting-sources").as_deref(), Some("user"));
        assert!(args.iter().any(|a| a == "--strict-mcp-config"));
        // Never the full profile, and the token is not on the command line.
        assert!(!args.iter().any(|a| a.contains("dangerously")));
        assert!(!args.iter().any(|a| a.contains("hrn_chat_secret")));
        // A first turn has nothing to resume.
        let first = claude_args(&turn("claude", None), Path::new("/x/mcp.json"));
        assert!(!first.iter().any(|a| a == "--resume"));
    }

    #[test]
    fn omp_gets_an_exact_allowlist_of_harness_tools() {
        let args = omp_args(&turn("pi", None), Path::new("/g.ts"));
        let tools = args
            .windows(2)
            .find(|w| w[0] == "--tools")
            .map(|w| w[1].clone())
            .unwrap();
        assert_eq!(tools, "mcp__harness_run_list,mcp__harness_issue_prs");
        // A bare Kimi model name is qualified, as for workflow nodes.
        assert!(args
            .windows(2)
            .any(|w| w == ["--model", "kimi-code/sonnet"]));
        assert!(!args.iter().any(|a| a.contains("hrn_chat_secret")));
    }

    #[test]
    fn omp_tool_names_follow_omps_own_rule() {
        assert_eq!(omp_tool_name("run_list"), "mcp__harness_run_list");
        assert_eq!(omp_tool_name("Run-List"), "mcp__harness_run_list");
        // A redundant server prefix is dropped, as omp does.
        assert_eq!(omp_tool_name("harness_x"), "mcp__harness_x");
    }

    #[test]
    fn claude_stream_yields_answer_session_usage_and_activity() {
        let stream = [
            r#"{"type":"system","subtype":"init","session_id":"sess-1"}"#,
            r#"{"type":"assistant","message":{"content":[{"type":"tool_use","id":"t1","name":"mcp__harness__issue_prs","input":{"issue":"ECOM-44"}}]},"session_id":"sess-1"}"#,
            r#"{"type":"user","message":{"content":[{"type":"tool_result","tool_use_id":"t1","content":[{"type":"text","text":"ECOM-44: 2 run(s), 2 PR(s)\n{...}"}]}]}}"#,
            r#"{"type":"assistant","message":{"content":[{"type":"text","text":"Test #12."}]},"session_id":"sess-1"}"#,
            r#"{"type":"result","subtype":"success","is_error":false,"result":"Test #12 — its run passed.","session_id":"sess-1","usage":{"input_tokens":10,"output_tokens":5,"cache_read_input_tokens":100}}"#,
        ]
        .join("\n");
        let reply = parse_claude_stream(&stream);
        assert_eq!(reply.text, "Test #12 — its run passed.");
        assert_eq!(reply.session.as_deref(), Some("sess-1"));
        assert_eq!(reply.usage.input, Some(10));
        assert_eq!(reply.usage.cache_read, Some(100));

        let call = claude_activities(stream.lines().nth(1).unwrap());
        assert_eq!(call.len(), 1);
        assert_eq!(call[0].kind, ActivityKind::Tool);
        assert_eq!(call[0].text, "issue_prs");
        let res = claude_activities(stream.lines().nth(2).unwrap());
        assert_eq!(res[0].kind, ActivityKind::ToolResult);
        assert_eq!(res[0].detail.as_deref(), Some("ECOM-44: 2 run(s), 2 PR(s)"));
    }

    #[test]
    fn an_error_result_is_not_an_answer() {
        let stream = [
            r#"{"type":"assistant","message":{"content":[{"type":"text","text":"partial"}]}}"#,
            r#"{"type":"result","is_error":true,"result":"API Error: overloaded","session_id":"s"}"#,
        ]
        .join("\n");
        assert_eq!(parse_claude_stream(&stream).text, "partial");
    }

    #[test]
    fn an_empty_answer_is_an_error_that_carries_stderr() {
        #[cfg(unix)]
        let status = {
            use std::os::unix::process::ExitStatusExt;
            std::process::ExitStatus::from_raw(256)
        };
        #[cfg(windows)]
        let status = {
            use std::os::windows::process::ExitStatusExt;
            std::process::ExitStatus::from_raw(1)
        };
        let out = Output {
            stdout: String::new(),
            stderr: "Unknown tool in --tools: mcp__harness_x".into(),
            status,
        };
        let err = finish(ChatReply::default(), out, None).unwrap_err();
        assert!(err.0.contains("Unknown tool"), "{}", err.0);
    }
}
