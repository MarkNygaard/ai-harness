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
use harness_dag::{substitute, NodeBody, NodeOutput, NodeRequest, NodeRunner, RunnerError, Usage};
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
    /// The platform default shell.
    pub fn platform_default() -> Self {
        if cfg!(windows) {
            Shell {
                program: "cmd".into(),
                command_flag: "/C".into(),
            }
        } else {
            Shell {
                program: "sh".into(),
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

    /// Run a shell script in the workspace, returning its captured output.
    async fn run_bash(
        &self,
        script: &str,
        timeout: Option<u64>,
    ) -> Result<NodeOutput, RunnerError> {
        let mut cmd = Command::new(&self.shell.program);
        cmd.arg(&self.shell.command_flag)
            .arg(script)
            .current_dir(&self.workspace)
            .kill_on_drop(true);

        let fut = cmd.output();
        let output = match timeout {
            Some(ms) => match tokio::time::timeout(Duration::from_millis(ms), fut).await {
                Ok(res) => res,
                Err(_) => {
                    // The dropped future kills the child (kill_on_drop).
                    return Err(RunnerError(format!("bash node timed out after {ms}ms")));
                }
            },
            None => fut.await,
        }
        .map_err(|e| RunnerError(format!("failed to spawn shell: {e}")))?;

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

    /// Resolve a `command` name to its markdown body. Names are validated to
    /// prevent path traversal; the first matching `<name>.md` across
    /// `command_dirs` wins.
    fn resolve_command(&self, name: &str) -> Result<String, RunnerError> {
        validate_command_name(name)?;
        let file = format!("{name}.md");
        for dir in &self.command_dirs {
            let path = dir.join(&file);
            if path.is_file() {
                return std::fs::read_to_string(&path)
                    .map_err(|e| RunnerError(format!("failed to read command `{name}`: {e}")));
            }
        }
        Err(RunnerError(format!(
            "command `{name}` not found in {} command dir(s)",
            self.command_dirs.len()
        )))
    }

    /// Dispatch a prompt to the agent and map its result to a [`NodeOutput`].
    async fn run_prompt(
        &self,
        prompt: String,
        req: &NodeRequest<'_>,
    ) -> Result<NodeOutput, RunnerError> {
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
            NodeBody::Script { .. } => Err(RunnerError(
                "script nodes are not yet supported by LocalRunner".into(),
            )),
        }
    }
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
    async fn script_node_is_unsupported_for_now() {
        let dir = TempDir::new().unwrap();
        let (runner, _agent) = runner_at(dir.path(), vec![]);
        let vars = VarContext::new();
        let err = runner
            .execute(request(
                NodeBody::Script {
                    script: "print(1)".into(),
                    runtime: harness_dag::ScriptRuntime::Uv,
                    deps: vec![],
                },
                &vars,
            ))
            .await
            .unwrap_err();
        assert!(err.0.contains("not yet supported"));
    }
}
