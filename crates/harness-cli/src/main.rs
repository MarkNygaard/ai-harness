#![allow(clippy::manual_map, clippy::needless_return, clippy::ptr_arg)]

use clap::Parser;

mod cmd;
mod commands;

fn main() -> anyhow::Result<()> {
    // Started from inside a Claude Code session, the harness would hand that
    // session's markers to every process it starts, and a `claude` among them
    // would trap. Before the runtime exists, while this is the only thread.
    harness_agents::scrub_claude_session_env();
    let cli = commands::Cli::parse();
    tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()?
        .block_on(commands::run(cli))
}
