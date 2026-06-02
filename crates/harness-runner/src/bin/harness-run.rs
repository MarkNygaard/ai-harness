//! `harness-run` — a minimal local workflow runner for development.
//!
//! Thin wrapper around [`harness_runner::execute_run`] (shared with the
//! `harness run` CLI subcommand): parses args, runs the workflow, prints a
//! per-node summary, and exits with a status-derived code.
//!
//! Usage:
//!   harness-run <workflow.yaml> [--workspace <dir>] [--base-branch <name>]
//!               [--args <text>] [--real] [--sandbox <mode>] [--config <file>]
//!               [--worktree] [--database-url <url>]

use std::path::PathBuf;
use std::process::ExitCode;

use harness_dag::RunStatus;
use harness_runner::{execute_run, parse_sandbox, RunOptions, DEFAULT_WORKFLOW};

fn parse_args() -> Result<RunOptions, String> {
    let mut workflow: Option<PathBuf> = None;
    let mut workspace: Option<PathBuf> = None;
    let mut base_branch = "main".to_string();
    let mut arguments = String::new();
    let mut real = false;
    let mut sandbox = harness_runner::parse_sandbox("danger-full-access")?;
    let mut config: Option<PathBuf> = None;
    let mut worktree = false;
    let mut database_url: Option<String> = None;

    let mut it = std::env::args().skip(1);
    while let Some(arg) = it.next() {
        match arg.as_str() {
            "--workspace" => {
                workspace = Some(PathBuf::from(it.next().ok_or("--workspace needs a value")?));
            }
            "--base-branch" => base_branch = it.next().ok_or("--base-branch needs a value")?,
            "--args" => arguments = it.next().ok_or("--args needs a value")?,
            "--real" => real = true,
            "--sandbox" => sandbox = parse_sandbox(&it.next().ok_or("--sandbox needs a value")?)?,
            "--config" => config = Some(PathBuf::from(it.next().ok_or("--config needs a value")?)),
            "--worktree" => worktree = true,
            "--database-url" => {
                database_url = Some(it.next().ok_or("--database-url needs a value")?)
            }
            "-h" | "--help" => {
                return Err("usage: harness-run <workflow.yaml> [--workspace <dir>] \
                     [--base-branch <name>] [--args <text>] [--real] [--sandbox <mode>] \
                     [--config <file>] [--worktree] [--database-url <url>]"
                    .into());
            }
            other if other.starts_with('-') => return Err(format!("unknown flag: {other}")),
            other => {
                if workflow.is_some() {
                    return Err(format!("unexpected extra argument: {other}"));
                }
                workflow = Some(PathBuf::from(other));
            }
        }
    }

    // Omitting the workflow runs the bundled default pipeline (resolved by name).
    let workflow = workflow.unwrap_or_else(|| PathBuf::from(DEFAULT_WORKFLOW));
    let workspace = workspace
        .or_else(|| std::env::current_dir().ok())
        .ok_or("could not determine workspace directory")?;
    Ok(RunOptions {
        workflow,
        workspace,
        base_branch,
        arguments,
        real,
        sandbox,
        config,
        worktree,
        database_url,
    })
}

#[tokio::main]
async fn main() -> ExitCode {
    let opts = match parse_args() {
        Ok(opts) => opts,
        Err(e) => {
            eprintln!("harness-run: {e}");
            return ExitCode::FAILURE;
        }
    };
    match execute_run(opts).await {
        Ok(report) => match report.status {
            RunStatus::Completed => ExitCode::SUCCESS,
            RunStatus::Failed => ExitCode::from(1),
            RunStatus::Cancelled => ExitCode::from(2),
        },
        Err(e) => {
            eprintln!("harness-run: {e}");
            ExitCode::FAILURE
        }
    }
}
