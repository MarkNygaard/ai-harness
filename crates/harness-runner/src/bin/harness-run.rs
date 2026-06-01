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
//!               [--args <text>]

use std::path::PathBuf;
use std::process::ExitCode;
use std::sync::Arc;

use harness_core::config::agents::SandboxMode;
use harness_core::config::HarnessConfig;
use harness_dag::{parse_workflow, run_workflow, NodeStatus, RunStatus, Usage, VarContext};
use harness_runner::{build_agent_registry, CodeAgentRunner, EchoAgent, LocalRunner, PromptAgent};

struct Args {
    workflow: PathBuf,
    workspace: PathBuf,
    base_branch: String,
    arguments: String,
    /// Use real agents (Claude/Codex) instead of the built-in echo agent.
    real: bool,
    /// Sandbox mode for real CLI agents.
    sandbox: SandboxMode,
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
            "-h" | "--help" => {
                return Err("usage: harness-run <workflow.yaml> [--workspace <dir>] \
                     [--base-branch <name>] [--args <text>] [--real] [--sandbox <mode>]"
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

    // Per-run artifacts dir under the workspace.
    let artifacts = args.workspace.join(".harness").join("artifacts");
    std::fs::create_dir_all(&artifacts)
        .map_err(|e| format!("failed to create artifacts dir: {e}"))?;
    let command_dirs = vec![args.workspace.join(".harness").join("commands")];

    let vars = VarContext::new()
        .set("WORKFLOW_ID", workflow.name.clone())
        .set("ARTIFACTS_DIR", artifacts.display().to_string())
        .set("BASE_BRANCH", args.base_branch.clone())
        .set("DOCS_DIR", "docs")
        .set("ARGUMENTS", args.arguments.clone())
        .set("USER_MESSAGE", args.arguments.clone());

    let agent: Arc<dyn PromptAgent> = if args.real {
        let config = HarnessConfig::default();
        let registry = Arc::new(build_agent_registry(&config, args.sandbox));
        Arc::new(CodeAgentRunner::new(registry))
    } else {
        Arc::new(EchoAgent)
    };
    let runner = LocalRunner::new(args.workspace.clone(), command_dirs, agent);

    println!(
        "▶ running workflow `{}` in {} ({} agent)\n",
        workflow.name,
        args.workspace.display(),
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
