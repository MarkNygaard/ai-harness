pub mod anthropic_api;
pub mod claude;
pub mod claude_adapter;
mod claude_stream;
mod cloud_setup;
pub mod codex;
pub mod codex_adapter;
pub mod registry;
mod streaming;

/// Remove all `CLAUDE`-prefixed environment variables from a command to prevent
/// nested Claude Code detection (SIGTRAP).
pub(crate) fn strip_claude_env(cmd: &mut tokio::process::Command) {
    let claude_keys: Vec<String> = std::env::vars()
        .filter(|(k, _)| k.starts_with("CLAUDE"))
        .map(|(k, _)| k)
        .collect();
    for key in &claude_keys {
        cmd.env_remove(key);
    }
}

/// Control-plane secrets that must never reach a spawned task/agent process.
/// A run executes untrusted, AI-generated code (and full test suites) inside the
/// server's process tree — it has no business reading the control plane's
/// database URL, credential-encryption key, or API tokens. Leaking
/// `HARNESS_DATABASE_URL` in particular made task `cargo test` runs connect to
/// the **production** database (creating stray `test-*` runs and, before the
/// run-lease fix, cancelling live runs).
const CONTROL_PLANE_ENV: &[&str] = &[
    "HARNESS_DATABASE_URL",
    "DATABASE_URL",
    "HARNESS_SECRET_KEY",
    "HARNESS_API_TOKEN",
    "HARNESS_TOKEN",
    "HARNESS_REMOTE_URL",
];

/// Strip [`CONTROL_PLANE_ENV`] from a command before spawning it, so an agent or
/// `bash`/`script` node (and everything it spawns) can't reach the server's
/// secrets or its production database. Call at every task-facing spawn point.
pub fn strip_control_plane_env(cmd: &mut tokio::process::Command) {
    for key in CONTROL_PLANE_ENV {
        cmd.env_remove(key);
    }
}

/// Place the child process into its own process group.
///
/// Uses the stable `CommandExt::process_group(0)` API (Rust 1.64+).
/// When the child is later killed, we can send `SIGKILL` to the entire
/// process group to also terminate grandchild processes like `cargo test`
/// binaries.
#[cfg(unix)]
pub(crate) fn set_process_group(cmd: &mut tokio::process::Command) {
    cmd.process_group(0);
}

/// Kill the entire process group rooted at `child`.
///
/// Sends `SIGKILL` to `-pid` (the process group) so that all descendants
/// (cargo test binaries, shell subprocesses, etc.) are terminated together.
#[cfg(unix)]
pub(crate) fn kill_process_group(child: &tokio::process::Child) {
    if let Some(pid) = child.id() {
        // kill(-pgid, SIGKILL) kills the entire process group.
        // SAFETY: standard POSIX signal, no memory unsafety.
        let ret = unsafe { nix_kill(-(pid as i32), 9) };
        if ret == 0 {
            tracing::debug!(pgid = pid, "killed process group");
        } else {
            tracing::warn!(pgid = pid, "failed to kill process group");
        }
    }
}

/// Raw kill(2) syscall without libc dependency.
#[cfg(unix)]
unsafe fn nix_kill(pid: i32, sig: i32) -> i32 {
    extern "C" {
        fn kill(pid: i32, sig: i32) -> i32;
    }
    kill(pid, sig)
}
