//! Shared "execute one workflow run" logic, used by both the standalone
//! `harness-run` binary and the `harness run` CLI subcommand.
//!
//! [`execute_run`] does the full lifecycle: optional git-worktree isolation,
//! `VarContext` construction, agent selection (echo vs real registry), DAG
//! execution, and optional Postgres persistence. It returns the [`RunReport`];
//! callers render it with [`print_report`] and map the status to an exit code.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use harness_core::config::agents::SandboxMode;
use harness_core::config::HarnessConfig;
use harness_dag::{
    parse_workflow, run_workflow, NodeStatus, RunReport, RunStatus, Usage, VarContext,
};
use harness_persist::RunStore;

use crate::{
    build_agent_registry, sanitize_branch_component, CodeAgentRunner, DispatchAgent, EchoAgent,
    LocalRunner, PiAgent, PromptAgent, Worktree,
};

/// Options for executing a single workflow run.
pub struct RunOptions {
    pub workflow: PathBuf,
    pub workspace: PathBuf,
    pub base_branch: String,
    pub arguments: String,
    /// Use the real agent registry (Claude/Codex) instead of the echo agent.
    pub real: bool,
    pub sandbox: SandboxMode,
    /// Optional harness config TOML for the real registry.
    pub config: Option<PathBuf>,
    /// Run inside an isolated git worktree of the workspace.
    pub worktree: bool,
    /// Postgres URL to persist to (else `$HARNESS_DATABASE_URL`).
    pub database_url: Option<String>,
}

/// Parse a `--sandbox` value into a [`SandboxMode`].
pub fn parse_sandbox(value: &str) -> Result<SandboxMode, String> {
    match value {
        "read-only" => Ok(SandboxMode::ReadOnly),
        "read-only-with-network" => Ok(SandboxMode::ReadOnlyWithNetwork),
        "workspace-write" => Ok(SandboxMode::WorkspaceWrite),
        "danger-full-access" => Ok(SandboxMode::DangerFullAccess),
        other => Err(format!(
            "unknown sandbox `{other}` (expected read-only, read-only-with-network, workspace-write, danger-full-access)"
        )),
    }
}

fn now_millis() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis())
        .unwrap_or(0)
}

fn load_config_file(path: &Path) -> Result<HarnessConfig, String> {
    let content = std::fs::read_to_string(path)
        .map_err(|e| format!("failed to read config {}: {e}", path.display()))?;
    let mut config: HarnessConfig =
        toml::from_str(&content).map_err(|e| format!("invalid config {}: {e}", path.display()))?;
    if let Some(dir) = path.parent() {
        config.rebase_relative_paths(dir);
    }
    Ok(config)
}

/// Execute a workflow run end-to-end and return its report.
pub async fn execute_run(opts: RunOptions) -> Result<RunReport, String> {
    // The workflow may be a file path or a bare name (project `.harness/workflows`
    // then a bundled default); an empty value uses the default pipeline.
    let workflow_ref = opts.workflow.to_string_lossy();
    let (yaml, _label) = crate::defaults::resolve_workflow_source(&workflow_ref, &opts.workspace)?;
    let workflow = parse_workflow(&yaml).map_err(|e| format!("invalid workflow: {e}"))?;

    // Optional isolation in a fresh git worktree (removed when the guard drops).
    let mut _worktree_guard: Option<Worktree> = None;
    let workspace = if opts.worktree {
        let slug = format!(
            "{}-{}",
            sanitize_branch_component(&workflow.name),
            std::process::id()
        );
        let dest = std::env::temp_dir().join(format!("harness-run-{slug}"));
        let wt = Worktree::create(
            &opts.workspace,
            "HEAD",
            &format!("harness-run/{slug}"),
            &dest,
        )
        .map_err(|e| format!("worktree isolation failed: {e}"))?;
        let path = wt.path.clone();
        _worktree_guard = Some(wt);
        path
    } else {
        opts.workspace.clone()
    };

    let artifacts = workspace.join(".harness").join("artifacts");
    std::fs::create_dir_all(&artifacts)
        .map_err(|e| format!("failed to create artifacts dir: {e}"))?;
    let command_dirs = vec![workspace.join(".harness").join("commands")];

    let vars = VarContext::new()
        .set("WORKFLOW_ID", workflow.name.clone())
        .set("ARTIFACTS_DIR", artifacts.display().to_string())
        .set("BASE_BRANCH", opts.base_branch.clone())
        .set("DOCS_DIR", "docs")
        .set("ARGUMENTS", opts.arguments.clone())
        .set("USER_MESSAGE", opts.arguments.clone());

    let agent: Arc<dyn PromptAgent> = if opts.real {
        let config = match &opts.config {
            Some(path) => load_config_file(path)?,
            None => HarnessConfig::default(),
        };
        let registry = Arc::new(build_agent_registry(&config, opts.sandbox));
        // Route `provider: pi` to the omp-backed session-aware agent; everything
        // else goes through the CodeAgent registry (claude/codex/anthropic-api).
        let code = Arc::new(CodeAgentRunner::new(registry));
        Arc::new(DispatchAgent::new(Arc::new(PiAgent::from_env()), code))
    } else {
        Arc::new(EchoAgent)
    };
    let runner = LocalRunner::new(workspace.clone(), command_dirs, agent);

    println!(
        "▶ running workflow `{}` in {}{} ({} agent)\n",
        workflow.name,
        workspace.display(),
        if opts.worktree { " [worktree]" } else { "" },
        if opts.real { "real" } else { "echo" }
    );

    let report = run_workflow(&workflow, &runner, &vars)
        .await
        .map_err(|e| format!("run failed: {e}"))?;

    print_report(&report);

    // Optional persistence.
    let db_url = opts
        .database_url
        .clone()
        .or_else(|| std::env::var("HARNESS_DATABASE_URL").ok());
    if let Some(url) = db_url {
        let run_id = format!(
            "{}-{}",
            sanitize_branch_component(&workflow.name),
            now_millis()
        );
        match RunStore::connect(&url).await {
            Ok(store) => match store.record_run(&run_id, None, &report).await {
                Ok(()) => println!("\n✔ persisted run `{run_id}`"),
                Err(e) => eprintln!("\n⚠ persist failed: {e}"),
            },
            Err(e) => eprintln!("\n⚠ persist connect failed: {e}"),
        }
    }

    Ok(report)
}

/// Render a per-node summary of a [`RunReport`] to stdout.
pub fn print_report(report: &RunReport) {
    for node in &report.nodes {
        let mark = match node.status {
            NodeStatus::Success => "✓",
            NodeStatus::Failed => "✗",
            NodeStatus::Skipped => "–",
            NodeStatus::Cancelled => "⊘",
        };
        let model = match (&node.provider, &node.model) {
            (Some(p), Some(m)) => format!("{p}/{m}"),
            (Some(p), None) => p.clone(),
            _ => "-".to_string(),
        };
        let iters = if node.iterations > 1 {
            format!(" ×{}", node.iterations)
        } else {
            String::new()
        };
        println!(
            "  {mark} {:<16} {:<18} {}{}",
            node.id,
            model,
            run_node_status(node.status),
            iters
        );
        if let Some(note) = &node.note {
            println!("      note: {note}");
        }
        if has_usage(&node.usage) {
            println!("      tokens: {}", fmt_usage(&node.usage));
        }
    }
    println!("\n■ run status: {}", run_status(report.status));
}

fn run_node_status(s: NodeStatus) -> &'static str {
    match s {
        NodeStatus::Success => "success",
        NodeStatus::Failed => "failed",
        NodeStatus::Skipped => "skipped",
        NodeStatus::Cancelled => "cancelled",
    }
}

fn run_status(s: RunStatus) -> &'static str {
    match s {
        RunStatus::Completed => "completed",
        RunStatus::Failed => "failed",
        RunStatus::Cancelled => "cancelled",
    }
}

fn has_usage(u: &Usage) -> bool {
    u.input.is_some() || u.output.is_some() || u.cache_read.is_some() || u.cache_write.is_some()
}

fn fmt_usage(u: &Usage) -> String {
    let f = |v: Option<u64>| v.map(|n| n.to_string()).unwrap_or_else(|| "n/a".into());
    format!(
        "in {} / out {} / cache_read {} / cache_write {}",
        f(u.input),
        f(u.output),
        f(u.cache_read),
        f(u.cache_write)
    )
}
