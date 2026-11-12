pub mod anthropic_api;
pub mod claude;
pub mod claude_adapter;
mod claude_stream;
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

/// What a Claude Code session exports to the processes it starts. A `claude`
/// spawned with these believes it is part of that session (its id, messaging
/// socket and token) and traps. Not every `CLAUDE*` variable: the OAuth token
/// and `CLAUDE_CONFIG_DIR` are credentials and configuration, not session
/// markers. The list T3 Code strips for the same reason.
pub const CLAUDE_SESSION_ENV: &[&str] = &[
    "CLAUDECODE",
    "CLAUDE_CODE_ENTRYPOINT",
    "CLAUDE_CODE_SESSION_ID",
    "CLAUDE_CODE_HOST_SESSION_ID",
    "CLAUDE_CODE_CHILD_SESSION",
    "CLAUDE_CODE_SESSION_ATTENDED",
    "CLAUDE_CODE_MESSAGING_SOCKET",
    "CLAUDE_CODE_MESSAGING_TOKEN",
    "CLAUDE_CODE_EXECPATH",
    "CLAUDE_CODE_DESKTOP_APP_VERSION",
    "CLAUDE_EFFORT",
    "CLAUDE_PID",
    "CLAUDE_AGENT_SDK_VERSION",
];

/// Remove [`CLAUDE_SESSION_ENV`] from this process's own environment, so
/// nothing it spawns inherits them: not only the `claude` adapters (which
/// already strip them) but bash nodes, omp, cursor-agent, version probes and
/// MCP servers, any of which can start `claude` in turn.
///
/// Call it first thing in `main`, before a runtime starts other threads:
/// changing the environment while another thread reads it is unsound.
pub fn scrub_claude_session_env() {
    for key in CLAUDE_SESSION_ENV {
        std::env::remove_var(key);
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

/// Model-provider credentials a process can read from its environment. Each
/// belongs to one agent: a spawn removes all of them from what it inherits,
/// and the dispatcher adds back only the key of the agent that step runs on.
/// Without that, an omp step could read the Claude token and a bash step every
/// key there is. (Operator-set ones count too: a key in the container's
/// environment reaches only its own agent.)
pub const PROVIDER_KEY_ENV: &[&str] = &[
    "CLAUDE_CODE_OAUTH_TOKEN",
    "ANTHROPIC_API_KEY",
    "OPENAI_API_KEY",
    "MOONSHOT_API_KEY",
    "CURSOR_API_KEY",
];

/// Strip [`CONTROL_PLANE_ENV`] and [`PROVIDER_KEY_ENV`] from a command before
/// spawning it, so an agent or `bash`/`script` node (and everything it spawns)
/// can't reach the server's secrets, its production database, or another
/// agent's credentials. Call at every task-facing spawn point, before adding
/// the environment the step is meant to have.
pub fn strip_inherited_secrets(cmd: &mut tokio::process::Command) {
    for key in CONTROL_PLANE_ENV.iter().chain(PROVIDER_KEY_ENV) {
        cmd.env_remove(key);
    }
}

/// Give a spawn back its own agent's keys from the server's environment, after
/// [`strip_inherited_secrets`] removed them: an operator who set
/// `ANTHROPIC_API_KEY` on the container still reaches Claude, and only Claude.
/// Call between stripping and adding the step's own environment, which wins.
pub fn restore_own_keys(cmd: &mut tokio::process::Command, keys: &[&str]) {
    for key in keys {
        if let Some(value) = std::env::var_os(key).filter(|v| !v.is_empty()) {
            cmd.env(key, value);
        }
    }
}

/// The key Claude Code may take from the environment. Not
/// `CLAUDE_CODE_OAUTH_TOKEN`: the workflow spawns strip every `CLAUDE*`
/// variable and sign in with `~/.claude/.credentials.json`, which the server
/// keeps fresh.
pub const CLAUDE_KEYS: &[&str] = &["ANTHROPIC_API_KEY"];

/// Place the child process into its own process group.
///
/// Uses the stable `CommandExt::process_group(0)` API (Rust 1.64+).
/// When the child is later killed, we can send `SIGKILL` to the entire
/// process group to also terminate grandchild processes like `cargo test`
/// binaries.
#[cfg(unix)]
pub fn set_process_group(cmd: &mut tokio::process::Command) {
    cmd.process_group(0);
}

/// Kills a spawned agent's whole process group when dropped.
///
/// `kill_on_drop` kills the agent's own process only. The tools it started
/// (`cargo test`, a shell, `gh`) are its children and keep running, and can
/// keep editing the worktree after the run shows as cancelled. A cancel aborts
/// the run's task, which drops the future holding the agent, so a guard held
/// next to the `Child` is enough to take the whole tree down with it.
///
/// Create it right after spawning a command that [`set_process_group`] put in
/// its own group. Dropped on every exit path, it also clears away anything
/// the agent left running after it finished: a step's processes do not
/// outlive the step. A no-op on Windows, which has no process groups.
pub struct ProcessGroupGuard {
    #[cfg(unix)]
    pgid: Option<i32>,
}

impl ProcessGroupGuard {
    pub fn new(child: &tokio::process::Child) -> Self {
        #[cfg(unix)]
        {
            Self {
                pgid: child.id().map(|pid| pid as i32),
            }
        }
        #[cfg(not(unix))]
        {
            let _ = child;
            Self {}
        }
    }
}

impl Drop for ProcessGroupGuard {
    fn drop(&mut self) {
        #[cfg(unix)]
        if let Some(pgid) = self.pgid {
            // ESRCH (the group is already gone) is the normal case after a
            // clean exit, so the result is not worth a log line.
            // SAFETY: standard POSIX signal, no memory unsafety.
            unsafe {
                nix_kill(-pgid, 9);
            }
        }
    }
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

#[cfg(test)]
mod session_env_tests {
    use super::CLAUDE_SESSION_ENV;

    /// Scrubbed at startup, so the list must hold session markers only:
    /// dropping the OAuth token or config dir would sign Claude out.
    #[test]
    fn the_session_scrub_leaves_credentials_and_config_alone() {
        for kept in [
            "CLAUDE_CODE_OAUTH_TOKEN",
            "CLAUDE_CONFIG_DIR",
            "ANTHROPIC_API_KEY",
        ] {
            assert!(
                !CLAUDE_SESSION_ENV.contains(&kept),
                "{kept} would be scrubbed"
            );
        }
        assert!(CLAUDE_SESSION_ENV.contains(&"CLAUDECODE"));
        assert!(CLAUDE_SESSION_ENV.contains(&"CLAUDE_CODE_ENTRYPOINT"));
    }
}

#[cfg(test)]
mod inherited_secrets_tests {
    use super::*;

    /// What a command will do with each variable: `Some(None)` = removed.
    fn plan(cmd: &tokio::process::Command) -> Vec<(String, Option<String>)> {
        cmd.as_std()
            .get_envs()
            .map(|(k, v)| {
                (
                    k.to_string_lossy().into_owned(),
                    v.map(|v| v.to_string_lossy().into_owned()),
                )
            })
            .collect()
    }

    #[test]
    fn every_provider_key_and_control_plane_secret_is_removed() {
        let mut cmd = tokio::process::Command::new("true");
        strip_inherited_secrets(&mut cmd);
        let plan = plan(&cmd);
        for key in PROVIDER_KEY_ENV.iter().chain(CONTROL_PLANE_ENV) {
            assert!(
                plan.iter().any(|(k, v)| k == key && v.is_none()),
                "{key} is not removed"
            );
        }
    }

    #[test]
    fn the_step_environment_added_after_stripping_wins() {
        // The order every spawn uses: strip, then the step's own variables,
        // which may legitimately include a key (a project's build env).
        let mut cmd = tokio::process::Command::new("true");
        strip_inherited_secrets(&mut cmd);
        cmd.env("CURSOR_API_KEY", "from-the-step");
        let plan = plan(&cmd);
        assert!(plan
            .iter()
            .any(|(k, v)| k == "CURSOR_API_KEY" && v.as_deref() == Some("from-the-step")));
    }

    #[test]
    fn claude_does_not_take_the_oauth_token_from_the_environment() {
        assert!(!CLAUDE_KEYS.contains(&"CLAUDE_CODE_OAUTH_TOKEN"));
        assert!(PROVIDER_KEY_ENV.contains(&"CLAUDE_CODE_OAUTH_TOKEN"));
    }
}

#[cfg(all(test, target_os = "linux"))]
mod process_group_tests {
    use super::*;
    use std::time::Duration;

    /// Whether `pid` is a live process, not merely a zombie awaiting reaping.
    fn running(pid: u32) -> bool {
        std::fs::read_to_string(format!("/proc/{pid}/stat"))
            .map(|stat| {
                stat.rsplit_once(") ")
                    .is_some_and(|(_, rest)| !rest.starts_with('Z'))
            })
            .unwrap_or(false)
    }

    #[tokio::test]
    async fn dropping_the_guard_stops_what_the_child_started() {
        let dir = tempfile::tempdir().unwrap();
        let pidfile = dir.path().join("tool.pid");
        let mut cmd = tokio::process::Command::new("sh");
        cmd.arg("-c")
            .arg(format!(
                "sleep 60 & echo $! > {}; sleep 60",
                pidfile.display()
            ))
            .kill_on_drop(true);
        set_process_group(&mut cmd);
        let child = cmd.spawn().unwrap();
        let guard = ProcessGroupGuard::new(&child);

        let mut tool = None;
        for _ in 0..250 {
            tool = std::fs::read_to_string(&pidfile)
                .ok()
                .and_then(|s| s.trim().parse::<u32>().ok());
            if tool.is_some() {
                break;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        let tool = tool.expect("the child started its tool");
        assert!(running(tool));

        // What a cancel does: drop the future that holds both.
        drop(guard);
        drop(child);
        let mut stopped = false;
        for _ in 0..250 {
            if !running(tool) {
                stopped = true;
                break;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        assert!(stopped, "the child's tool outlived the guard");
    }
}
