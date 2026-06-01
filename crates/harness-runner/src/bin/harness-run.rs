//! `harness-run` — a minimal local workflow runner for development.
//!
//! Parses a workflow YAML, builds a [`VarContext`], and drives it with a
//! [`LocalRunner`] + [`EchoAgent`] (no real model). Prints a per-node summary
//! of the resulting `RunReport`. Real agent backends and a `harness run`
//! subcommand inside the main CLI land later; this is the locally-verifiable
//! end-to-end demo.
//!
//! Usage:
//!   harness-run <workflow.yaml> [--workspace <dir>] [--base-branch <name>]
//!               [--args <text>] [--real] [--sandbox <mode>] [--config <file>]
//!               [--worktree]

use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::sync::Arc;

use harness_core::config::agents::SandboxMode;
use harness_core::config::HarnessConfig;
use harness_dag::{parse_workflow, run_workflow, NodeStatus, RunStatus, Usage, VarContext};
use harness_runner::{
    build_agent_registry, sanitize_branch_component, CodeAgentRunner, EchoAgent, LocalRunner,
    PromptAgent, Worktree,
};

struct Args {
    workflow: PathBuf,
    workspace: PathBuf,
    base_branch: String,
    arguments: String,
    /// Use real agents (Claude/Codex) instead of the built-in echo agent.
    real: bool,
    /// Sandbox mode for real CLI agents.
    sandbox: SandboxMode,
    /// Optional path to a harness config TOML (for the real agent registry).
    config: Option<PathBuf>,
    /// Run inside an isolated git worktree of the workspace (removed after).
    worktree: bool,
}

/// Load a [`HarnessConfig`] from a TOML file, rebasing relative paths to its dir.
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

fn parse_sandbox(value: &str) -> Result<SandboxMode, String> {
    match value {
        "read-only" => Ok(SandboxMode::ReadOnly),
        "read-only-with-network" => Ok(SandboxMode::ReadOnlyWithNetwork),
        "workspace-write" => Ok(SandboxMode::WorkspaceWrite),
        "danger-full-access" => Ok(SandboxMode::DangerFullAccess),
        other => Err(format!(
            "unknown --sandbox `{other}` (expected read-only, read-only-with-network, workspace-write, danger-full-access)"
        )),
    }
}

fn parse_args() -> Result<Args, String> {
    let mut workflow: Option<PathBuf> = None;
    let mut workspace: Option<PathBuf> = None;
    let mut base_branch = "main".to_string();
    let mut arguments = String::new();
    let mut real = false;
    let mut sandbox = SandboxMode::DangerFullAccess;
    let mut config: Option<PathBuf> = None;
    let mut worktree = false;

    let mut it = std::env::args().skip(1);
    while let Some(arg) = it.next() {
        match arg.as_str() {
            "--workspace" => {
                workspace = Some(PathBuf::from(it.next().ok_or("--workspace needs a value")?));
            }
            "--base-branch" => {
                base_branch = it.next().ok_or("--base-branch needs a value")?;
            }
            "--args" => {
                arguments = it.next().ok_or("--args needs a value")?;
            }
            "--real" => {
                real = true;
            }
            "--sandbox" => {
                sandbox = parse_sandbox(&it.next().ok_or("--sandbox needs a value")?)?;
            }
            "--config" => {
                config = Some(PathBuf::from(it.next().ok_or("--config needs a value")?));
            }
            "--worktree" => {
                worktree = true;
            }
            "-h" | "--help" => {
                return Err("usage: harness-run <workflow.yaml> [--workspace <dir>] \
                     [--base-branch <name>] [--args <text>] [--real] [--sandbox <mode>] \
                     [--config <file>] [--worktree]"
                    .into());
            }
            other if other.starts_with('-') => {
                return Err(format!("unknown flag: {other}"));
            }
            other => {
                if workflow.is_some() {
                    return Err(format!("unexpected extra argument: {other}"));
                }
                workflow = Some(PathBuf::from(other));
            }
        }
    }

    let workflow = workflow.ok_or("missing <workflow.yaml> argument")?;
    let workspace = workspace
        .or_else(|| std::env::current_dir().ok())
        .ok_or("could not determine workspace directory")?;
    Ok(Args {
        workflow,
        workspace,
        base_branch,
        arguments,
        real,
        sandbox,
        config,
        worktree,
    })
}

#[tokio::main]
async fn main() -> ExitCode {
    match run().await {
        Ok(code) => code,
        Err(e) => {
            eprintln!("harness-run: {e}");
            ExitCode::FAILURE
        }
    }
}

async fn run() -> Result<ExitCode, String> {
    let args = parse_args()?;

    let yaml = std::fs::read_to_string(&args.workflow)
        .map_err(|e| format!("failed to read {}: {e}", args.workflow.display()))?;
    let workflow = parse_workflow(&yaml).map_err(|e| format!("invalid workflow: {e}"))?;

    // Optionally isolate the run in a fresh git worktree of the workspace.
    // The guard lives to the end of `run()`; its Drop removes the worktree.
    let mut _worktree_guard: Option<Worktree> = None;
    let workspace = if args.worktree {
        let slug = format!(
            "{}-{}",
            sanitize_branch_component(&workflow.name),
            std::process::id()
        );
        let dest = std::env::temp_dir().join(format!("harness-run-{slug}"));
        let wt = Worktree::create(
            &args.workspace,
            "HEAD",
            &format!("harness-run/{slug}"),
            &dest,
        )
        .map_err(|e| format!("worktree isolation failed: {e}"))?;
        let path = wt.path.clone();
        _worktree_guard = Some(wt);
        path
    } else {
        args.workspace.clone()
    };

    // Per-run artifacts dir under the (effective) workspace.
    let artifacts = workspace.join(".harness").join("artifacts");
    std::fs::create_dir_all(&artifacts)
        .map_err(|e| format!("failed to create artifacts dir: {e}"))?;
    let command_dirs = vec![workspace.join(".harness").join("commands")];

    let vars = VarContext::new()
        .set("WORKFLOW_ID", workflow.name.clone())
        .set("ARTIFACTS_DIR", artifacts.display().to_string())
        .set("BASE_BRANCH", args.base_branch.clone())
        .set("DOCS_DIR", "docs")
        .set("ARGUMENTS", args.arguments.clone())
        .set("USER_MESSAGE", args.arguments.clone());

    let agent: Arc<dyn PromptAgent> = if args.real {
        let config = match &args.config {
            Some(path) => load_config_file(path)?,
            None => HarnessConfig::default(),
        };
        let registry = Arc::new(build_agent_registry(&config, args.sandbox));
        Arc::new(CodeAgentRunner::new(registry))
    } else {
        Arc::new(EchoAgent)
    };
    let runner = LocalRunner::new(workspace.clone(), command_dirs, agent);

    println!(
        "▶ running workflow `{}` in {}{} ({} agent)\n",
        workflow.name,
        workspace.display(),
        if args.worktree { " [worktree]" } else { "" },
        if args.real { "real" } else { "echo" }
    );

    let report = run_workflow(&workflow, &runner, &vars)
        .await
        .map_err(|e| format!("run failed: {e}"))?;

    print_report(&report);

    Ok(match report.status {
        RunStatus::Completed => ExitCode::SUCCESS,
        RunStatus::Failed => ExitCode::from(1),
        RunStatus::Cancelled => ExitCode::from(2),
    })
}

fn print_report(report: &harness_dag::RunReport) {
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
            fmt_status(node.status),
            iters
        );
        if let Some(note) = &node.note {
            println!("      note: {note}");
        }
        if has_usage(&node.usage) {
            println!("      tokens: {}", fmt_usage(&node.usage));
        }
    }
    println!("\n■ run status: {}", fmt_run_status(report.status));
}

fn fmt_status(s: NodeStatus) -> &'static str {
    match s {
        NodeStatus::Success => "success",
        NodeStatus::Failed => "failed",
        NodeStatus::Skipped => "skipped",
        NodeStatus::Cancelled => "cancelled",
    }
}

fn fmt_run_status(s: RunStatus) -> &'static str {
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
