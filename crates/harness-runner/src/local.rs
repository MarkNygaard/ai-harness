//! The local node-execution backend.
//!
//! [`LocalRunner`] implements [`harness_dag::NodeRunner`] by running each node
//! body on the local machine: `bash` bodies in a subprocess rooted at the run's
//! workspace, `command` bodies resolved from `.harness/commands/` and dispatched
//! as prompts, and `prompt`/loop bodies dispatched to a [`PromptAgent`].

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use harness_dag::{
    substitute, NodeBody, NodeOutput, NodeRequest, NodeRunner, RunnerError, ScriptRuntime, Usage,
};
use tokio::process::Command;

use crate::{PromptAgent, PromptRequest};

/// How to invoke a shell for `bash` bodies. Defaults to the platform shell
/// (`sh -c` on Unix, `cmd /C` on Windows); override for tests or custom setups.
#[derive(Debug, Clone)]
pub struct Shell {
    pub program: String,
    pub command_flag: String,
}

impl Shell {
    /// The platform default shell. On Unix we use **bash**, not `/bin/sh`: shell
    /// node bodies (and the bundled commands' scripts) use bash-isms like
    /// `set -o pipefail`, which dash — `/bin/sh` on Debian, our runtime image —
    /// rejects. The image ships bash; tests can override via [`Self`] + `with_shell`.
    pub fn platform_default() -> Self {
        if cfg!(windows) {
            Shell {
                program: "cmd".into(),
                command_flag: "/C".into(),
            }
        } else {
            Shell {
                program: "bash".into(),
                command_flag: "-c".into(),
            }
        }
    }
}

/// Executes workflow nodes on the local machine.
pub struct LocalRunner {
    workspace: PathBuf,
    command_dirs: Vec<PathBuf>,
    agent: Arc<dyn PromptAgent>,
    shell: Shell,
}

impl LocalRunner {
    /// Create a runner rooted at `workspace`, resolving `command` bodies from
    /// `command_dirs` (searched in order) and dispatching prompts to `agent`.
    pub fn new(
        workspace: impl Into<PathBuf>,
        command_dirs: Vec<PathBuf>,
        agent: Arc<dyn PromptAgent>,
    ) -> Self {
        Self {
            workspace: workspace.into(),
            command_dirs,
            agent,
            shell: Shell::platform_default(),
        }
    }

    /// Override the shell used for `bash` bodies.
    pub fn with_shell(mut self, shell: Shell) -> Self {
        self.shell = shell;
        self
    }

    /// Spawn `program` with `args` in the workspace and capture its output.
    /// `label` names the node kind for error/timeout messages.
    async fn run_process(
        &self,
        program: &str,
        args: &[&str],
        label: &str,
        timeout: Option<u64>,
    ) -> Result<NodeOutput, RunnerError> {
        let mut cmd = Command::new(program);
        cmd.args(args)
            .current_dir(&self.workspace)
            .kill_on_drop(true);

        let fut = cmd.output();
        let output = match timeout {
            Some(ms) => match tokio::time::timeout(Duration::from_millis(ms), fut).await {
                Ok(res) => res,
                Err(_) => {
                    // The dropped future kills the child (kill_on_drop).
                    return Err(RunnerError(format!("{label} timed out after {ms}ms")));
                }
            },
            None => fut.await,
        }
        .map_err(|e| RunnerError(format!("failed to spawn {label} (`{program}`): {e}")))?;

        let stdout = String::from_utf8_lossy(&output.stdout);
        let stderr = String::from_utf8_lossy(&output.stderr);
        let success = output.status.success();

        let mut text = stdout.trim_end().to_string();
        if !success && !stderr.trim().is_empty() {
            if !text.is_empty() {
                text.push('\n');
            }
            text.push_str("--- stderr ---\n");
            text.push_str(stderr.trim_end());
        }

        Ok(NodeOutput {
            text,
            session: None,
            success,
            usage: Usage::default(),
        })
    }

    /// Run a `bash` body through the platform shell in the workspace.
    async fn run_bash(
        &self,
        script: &str,
        timeout: Option<u64>,
    ) -> Result<NodeOutput, RunnerError> {
        self.run_process(
            self.shell.program.as_str(),
            &[self.shell.command_flag.as_str(), script],
            "bash node",
            timeout,
        )
        .await
    }

    /// Run a `script` body via its runtime (`bun` for TS/JS, `uv` for Python).
    /// The script text is written to a temp file in the workspace so relative
    /// paths resolve there. `uv` injects each dependency with `--with`; `bun`
    /// dependency auto-install is not yet supported (deps must be resolvable
    /// from the workspace).
    async fn run_script(
        &self,
        script: &str,
        runtime: ScriptRuntime,
        deps: &[String],
        timeout: Option<u64>,
    ) -> Result<NodeOutput, RunnerError> {
        use std::io::Write as _;

        let suffix = match runtime {
            ScriptRuntime::Bun => ".ts",
            ScriptRuntime::Uv => ".py",
        };
        let mut file = tempfile::Builder::new()
            .prefix("harness-script-")
            .suffix(suffix)
            .tempfile_in(&self.workspace)
            .map_err(|e| RunnerError(format!("failed to create temp script file: {e}")))?;
        file.write_all(script.as_bytes())
            .map_err(|e| RunnerError(format!("failed to write temp script: {e}")))?;
        let path = file.path().to_string_lossy().to_string();

        let result = match runtime {
            ScriptRuntime::Bun => {
                self.run_process("bun", &[path.as_str()], "bun script", timeout)
                    .await
            }
            ScriptRuntime::Uv => {
                let mut args: Vec<&str> = vec!["run"];
                for dep in deps {
                    args.push("--with");
                    args.push(dep.as_str());
                }
                args.push(path.as_str());
                self.run_process("uv", &args, "uv script", timeout).await
            }
        };
        // Keep the temp file alive until execution finishes, then clean up.
        drop(file);
        result
    }

    /// Resolve a `command` name to its prompt body. Names are validated to
    /// prevent path traversal; the first matching `<name>.md` across
    /// `command_dirs` wins. Claude-Code-style YAML frontmatter is stripped — it's
    /// metadata (description / argument-hint / allowed-tools), not prompt text,
    /// and its leading `---` would otherwise be mis-parsed by the agent CLIs as a
    /// command-line option.
    fn resolve_command(&self, name: &str) -> Result<String, RunnerError> {
        validate_command_name(name)?;
        let file = format!("{name}.md");
        for dir in &self.command_dirs {
            let path = dir.join(&file);
            if path.is_file() {
                return std::fs::read_to_string(&path)
                    .map(|s| strip_frontmatter(&s))
                    .map_err(|e| RunnerError(format!("failed to read command `{name}`: {e}")));
            }
        }
        // Fall back to a bundled default command (project dirs shadow these).
        if let Some(body) = crate::defaults::default_command(name) {
            return Ok(strip_frontmatter(body));
        }
        Err(RunnerError(format!(
            "command `{name}` not found in {} command dir(s) or bundled defaults",
            self.command_dirs.len()
        )))
    }

    /// Dispatch a prompt to the agent and map its result to a [`NodeOutput`].
    /// When the node declares an `output_format`, a directive instructing the
    /// agent to reply with conforming JSON is appended — so downstream
    /// `$node.output.field` access and `when:` conditions read a stable shape.
    async fn run_prompt(
        &self,
        prompt: String,
        req: &NodeRequest<'_>,
    ) -> Result<NodeOutput, RunnerError> {
        let prompt = match req.output_format {
            Some(schema) => format!("{prompt}{}", output_format_directive(schema)),
            None => prompt,
        };
        let result = self
            .agent
            .run(PromptRequest {
                provider: req.provider.map(str::to_string),
                model: req.model.map(str::to_string),
                prompt,
                cwd: self.workspace.clone(),
                session: req.session.clone(),
                iteration: req.iteration,
            })
            .await
            .map_err(|e| RunnerError(e.to_string()))?;

        Ok(NodeOutput {
            text: result.text,
            session: result.session,
            success: result.success,
            usage: result.usage,
        })
    }
}

#[async_trait]
impl NodeRunner for LocalRunner {
    async fn execute(&self, req: NodeRequest<'_>) -> Result<NodeOutput, RunnerError> {
        match &req.body {
            NodeBody::Bash(script) => self.run_bash(script, req.timeout).await,
            NodeBody::Prompt(text) => {
                // Inline prompt text is already substituted by the driver.
                self.run_prompt(text.clone(), &req).await
            }
            NodeBody::Command(name) => {
                let raw = self.resolve_command(name)?;
                let rendered = substitute(&raw, req.vars)
                    .map_err(|e| RunnerError(format!("command `{name}`: {e}")))?;
                self.run_prompt(rendered, &req).await
            }
            NodeBody::Script {
                script,
                runtime,
                deps,
            } => {
                // Script text is already variable-substituted by the driver.
                self.run_script(script, *runtime, deps, req.timeout).await
            }
        }
    }
}

/// Build the instruction appended to a prompt when a node declares an
/// `output_format`. Kept terse and unambiguous so any agent CLI honors it.
fn output_format_directive(schema: &serde_json::Value) -> String {
    let pretty = serde_json::to_string_pretty(schema).unwrap_or_else(|_| schema.to_string());
    format!(
        "\n\nIMPORTANT: Respond with ONLY a single JSON value that conforms to \
         this JSON schema. No prose, no markdown fences.\n\nSchema:\n{pretty}"
    )
}

/// Strip a leading YAML frontmatter block (`---` … `---`) from a command body,
/// returning the prompt text after it. Tolerates a UTF-8 BOM and CRLF. If there
/// is no well-formed leading frontmatter, the body is returned unchanged.
fn strip_frontmatter(body: &str) -> String {
    let s = body.strip_prefix('\u{feff}').unwrap_or(body);
    let mut lines = s.lines();
    // Frontmatter must open with `---` on the very first line.
    if lines.next().map(str::trim_end) != Some("---") {
        return body.to_string();
    }
    let mut rest: Vec<&str> = Vec::new();
    let mut closed = false;
    for line in lines {
        if !closed {
            if line.trim_end() == "---" {
                closed = true;
            }
            continue;
        }
        rest.push(line);
    }
    // No closing delimiter → it wasn't frontmatter; keep the original.
    if !closed {
        return body.to_string();
    }
    rest.join("\n").trim_start().to_string()
}

/// Reject command names that could escape the command directories.
fn validate_command_name(name: &str) -> Result<(), RunnerError> {
    let invalid = name.is_empty()
        || name.starts_with('.')
        || name.contains('/')
        || name.contains('\\')
        || name.contains("..");
    if invalid {
        return Err(RunnerError(format!("invalid command name: `{name}`")));
    }
    Ok(())
}

/// Visible to tests only: confirm a path stays within `root` (defensive helper
/// for future workspace-escape checks).
#[allow(dead_code)]
fn is_within(root: &Path, path: &Path) -> bool {
    match (root.canonicalize(), path.canonicalize()) {
        (Ok(r), Ok(p)) => p.starts_with(r),
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex;

    use harness_dag::{ContextMode, VarContext};
    use tempfile::TempDir;

    use super::*;

    /// Records the last prompt it received and returns a canned result.
    #[derive(Default)]
    struct MockAgent {
        last_prompt: Mutex<Option<String>>,
        reply: Mutex<PromptResultSpec>,
    }

    #[derive(Clone)]
    struct PromptResultSpec {
        text: String,
        session: Option<String>,
        success: bool,
    }

    impl Default for PromptResultSpec {
        fn default() -> Self {
            Self {
                text: "agent-reply".into(),
                session: Some("sess-1".into()),
                success: true,
            }
        }
    }

    use crate::{AgentError, PromptResult};

    #[async_trait]
    impl PromptAgent for MockAgent {
        async fn run(&self, req: PromptRequest) -> Result<PromptResult, AgentError> {
            *self.last_prompt.lock().unwrap() = Some(req.prompt.clone());
            let spec = self.reply.lock().unwrap().clone();
            Ok(PromptResult {
                text: spec.text,
                session: spec.session,
                usage: Usage {
                    input: Some(10),
                    output: Some(2),
                    ..Usage::default()
                },
                success: spec.success,
            })
        }
    }

    fn request<'a>(body: NodeBody, vars: &'a VarContext) -> NodeRequest<'a> {
        NodeRequest {
            node_id: "n",
            provider: Some("claude"),
            model: Some("sonnet"),
            context: ContextMode::Shared,
            session: None,
            iteration: 1,
            body,
            timeout: None,
            vars,
            output_format: None,
        }
    }

    fn runner_at(workspace: &Path, command_dirs: Vec<PathBuf>) -> (LocalRunner, Arc<MockAgent>) {
        let agent = Arc::new(MockAgent::default());
        let runner = LocalRunner::new(workspace.to_path_buf(), command_dirs, agent.clone());
        (runner, agent)
    }

    #[tokio::test]
    async fn bash_captures_stdout_and_succeeds() {
        let dir = TempDir::new().unwrap();
        let (runner, _agent) = runner_at(dir.path(), vec![]);
        let vars = VarContext::new();
        let out = runner
            .execute(request(NodeBody::Bash("echo hello".into()), &vars))
            .await
            .unwrap();
        assert!(out.success);
        assert!(out.text.contains("hello"), "got: {:?}", out.text);
    }

    #[tokio::test]
    async fn bash_nonzero_exit_reports_failure() {
        let dir = TempDir::new().unwrap();
        let (runner, _agent) = runner_at(dir.path(), vec![]);
        let vars = VarContext::new();
        let out = runner
            .execute(request(NodeBody::Bash("exit 3".into()), &vars))
            .await
            .unwrap();
        assert!(!out.success);
    }

    #[tokio::test]
    async fn prompt_is_dispatched_to_agent() {
        let dir = TempDir::new().unwrap();
        let (runner, agent) = runner_at(dir.path(), vec![]);
        let vars = VarContext::new();
        let out = runner
            .execute(request(NodeBody::Prompt("do the thing".into()), &vars))
            .await
            .unwrap();
        assert!(out.success);
        assert_eq!(out.text, "agent-reply");
        assert_eq!(out.session.as_deref(), Some("sess-1"));
        assert_eq!(out.usage.input, Some(10));
        assert_eq!(
            agent.last_prompt.lock().unwrap().as_deref(),
            Some("do the thing")
        );
    }

    #[tokio::test]
    async fn output_format_directive_is_appended_to_prompt() {
        let dir = TempDir::new().unwrap();
        let (runner, agent) = runner_at(dir.path(), vec![]);
        let vars = VarContext::new();
        let schema = serde_json::json!({ "type": "object" });
        let mut req = request(NodeBody::Prompt("classify this".into()), &vars);
        req.output_format = Some(&schema);

        runner.execute(req).await.unwrap();
        let sent = agent.last_prompt.lock().unwrap().clone().unwrap();
        assert!(sent.starts_with("classify this"));
        assert!(sent.contains("JSON schema"), "got: {sent:?}");
        assert!(sent.contains("\"type\""), "schema not embedded: {sent:?}");
    }

    #[tokio::test]
    async fn command_is_resolved_substituted_and_dispatched() {
        let dir = TempDir::new().unwrap();
        let cmd_dir = dir.path().join("commands");
        std::fs::create_dir_all(&cmd_dir).unwrap();
        std::fs::write(cmd_dir.join("plan.md"), "plan in $ARTIFACTS_DIR now").unwrap();

        let (runner, agent) = runner_at(dir.path(), vec![cmd_dir]);
        let vars = VarContext::new().set("ARTIFACTS_DIR", "/run/9");
        let out = runner
            .execute(request(NodeBody::Command("plan".into()), &vars))
            .await
            .unwrap();
        assert!(out.success);
        assert_eq!(
            agent.last_prompt.lock().unwrap().as_deref(),
            Some("plan in /run/9 now")
        );
    }

    #[tokio::test]
    async fn command_frontmatter_is_stripped_before_dispatch() {
        let dir = TempDir::new().unwrap();
        let cmd_dir = dir.path().join("commands");
        std::fs::create_dir_all(&cmd_dir).unwrap();
        // Claude-Code-style command: YAML frontmatter then the actual prompt.
        std::fs::write(
            cmd_dir.join("plan-setup.md"),
            "---\ndescription: Setup\nargument-hint: <path>\n---\n\n# Plan Setup\nDo $ARGUMENTS",
        )
        .unwrap();

        let (runner, agent) = runner_at(dir.path(), vec![cmd_dir]);
        let vars = VarContext::new().set("ARGUMENTS", "the thing");
        let out = runner
            .execute(request(NodeBody::Command("plan-setup".into()), &vars))
            .await
            .unwrap();
        assert!(out.success);
        // The dispatched prompt must NOT contain the frontmatter / its `---`.
        assert_eq!(
            agent.last_prompt.lock().unwrap().as_deref(),
            Some("# Plan Setup\nDo the thing")
        );
    }

    #[test]
    fn unix_default_shell_is_bash() {
        let s = Shell::platform_default();
        if cfg!(windows) {
            assert_eq!(s.program, "cmd");
        } else {
            // bash, not /bin/sh (dash) — scripts use `set -o pipefail` etc.
            assert_eq!(s.program, "bash");
        }
    }

    #[test]
    fn strip_frontmatter_handles_present_absent_and_unterminated() {
        assert_eq!(strip_frontmatter("---\na: 1\n---\nbody here"), "body here");
        // No frontmatter → unchanged.
        assert_eq!(strip_frontmatter("# Title\ntext"), "# Title\ntext");
        // Unterminated block → not treated as frontmatter (kept as-is).
        assert_eq!(strip_frontmatter("---\nstill going"), "---\nstill going");
    }

    #[tokio::test]
    async fn command_not_found_errors() {
        let dir = TempDir::new().unwrap();
        let (runner, _agent) = runner_at(dir.path(), vec![dir.path().join("commands")]);
        let vars = VarContext::new();
        let err = runner
            .execute(request(NodeBody::Command("missing".into()), &vars))
            .await
            .unwrap_err();
        assert!(err.0.contains("not found"), "got: {}", err.0);
    }

    #[tokio::test]
    async fn command_name_rejects_traversal() {
        let dir = TempDir::new().unwrap();
        let (runner, _agent) = runner_at(dir.path(), vec![dir.path().join("commands")]);
        let vars = VarContext::new();
        let err = runner
            .execute(request(NodeBody::Command("../secret".into()), &vars))
            .await
            .unwrap_err();
        assert!(err.0.contains("invalid command name"), "got: {}", err.0);
    }

    #[tokio::test]
    async fn drives_a_real_workflow_end_to_end() {
        // The DAG driver + LocalRunner composed: a bash node feeding a prompt node.
        let dir = TempDir::new().unwrap();
        let agent = Arc::new(MockAgent::default());
        let runner = LocalRunner::new(dir.path().to_path_buf(), vec![], agent.clone());

        let yaml = r#"
name: e2e
nodes:
  - id: build
    bash: "echo built"
  - id: summarize
    depends_on: [build]
    prompt: "summarize the build in $ARTIFACTS_DIR"
"#;
        let wf = harness_dag::parse_workflow(yaml).unwrap();
        let vars = VarContext::new().set("ARTIFACTS_DIR", "/run/1");
        let report = harness_dag::run_workflow(&wf, &runner, &vars)
            .await
            .unwrap();

        assert_eq!(report.status, harness_dag::RunStatus::Completed);
        assert!(report.node("build").unwrap().output.contains("built"));
        assert_eq!(report.node("summarize").unwrap().output, "agent-reply");
        // The prompt was rendered before dispatch.
        assert_eq!(
            agent.last_prompt.lock().unwrap().as_deref(),
            Some("summarize the build in /run/1")
        );
    }

    #[tokio::test]
    async fn script_runs_via_bun() {
        let dir = TempDir::new().unwrap();
        let (runner, _agent) = runner_at(dir.path(), vec![]);
        let vars = VarContext::new();
        let out = runner
            .execute(request(
                NodeBody::Script {
                    script: "console.log('hi from bun script')".into(),
                    runtime: ScriptRuntime::Bun,
                    deps: vec![],
                },
                &vars,
            ))
            .await
            .unwrap();
        assert!(out.success, "got: {:?}", out.text);
        assert!(
            out.text.contains("hi from bun script"),
            "got: {:?}",
            out.text
        );
    }

    #[tokio::test]
    async fn loop_until_bash_converges() {
        // The agent never emits the signal, but `until_bash: exit 0` ends the
        // loop on the first iteration.
        let dir = TempDir::new().unwrap();
        let (runner, _agent) = runner_at(dir.path(), vec![]);
        let yaml = r#"
name: ub
nodes:
  - id: poll
    loop:
      prompt: "keep polling"
      until: NEVER_EMITTED
      max_iterations: 5
      until_bash: "exit 0"
"#;
        let wf = harness_dag::parse_workflow(yaml).unwrap();
        let report = harness_dag::run_workflow(&wf, &runner, &VarContext::new())
            .await
            .unwrap();
        let poll = report.node("poll").unwrap();
        assert_eq!(poll.converged, Some(true));
        assert_eq!(poll.iterations, 1);
    }
}
