//! Per-run git worktree isolation.
//!
//! A [`Worktree`] checks out an isolated copy of a repo on its own branch so a
//! workflow run's edits never touch the user's working tree, and parallel runs
//! don't collide. It is removed on `Drop` (best-effort).
//!
//! NOTE: this is the **one** place harness code shells out to `git`. Worktree
//! management is run *infrastructure* — the same deliberate exception majiayu's
//! `WorkspaceManager` makes (`harness-server/src/workspace.rs`). The "no
//! `Command::new(git)` in harness crates" rule (AGENTS.md) is about *agent*-
//! delegated git (commits, PRs); the agent still does all of that itself.

use std::path::{Path, PathBuf};
use std::process::Command;

/// Error from a git worktree operation.
#[derive(Debug, thiserror::Error)]
#[error("worktree error: {0}")]
pub struct WorktreeError(pub String);

/// An isolated git worktree, removed on drop.
pub struct Worktree {
    repo: PathBuf,
    /// The checked-out worktree directory (use this as the run's workspace).
    pub path: PathBuf,
    /// The branch created for this worktree.
    pub branch: String,
}

impl Worktree {
    /// Create a worktree of `repo` at `base_ref` on a new branch `branch`,
    /// checked out at `dest` (which must not already exist).
    pub fn create(
        repo: &Path,
        base_ref: &str,
        branch: &str,
        dest: &Path,
    ) -> Result<Self, WorktreeError> {
        run_git(
            repo,
            &[
                "worktree",
                "add",
                "-b",
                branch,
                &dest.to_string_lossy(),
                base_ref,
            ],
        )?;
        Ok(Self {
            repo: repo.to_path_buf(),
            path: dest.to_path_buf(),
            branch: branch.to_string(),
        })
    }

    /// Remove the worktree (force, discarding any uncommitted changes) and
    /// delete the branch it created. Idempotent-ish: errors are returned, not
    /// panicked.
    pub fn remove(&self) -> Result<(), WorktreeError> {
        run_git(
            &self.repo,
            &[
                "worktree",
                "remove",
                "--force",
                &self.path.to_string_lossy(),
            ],
        )?;
        // `git worktree remove` leaves the branch behind; delete it too so
        // repeated runs don't accumulate `harness-run/*` branches. Best-effort.
        let _ = run_git(&self.repo, &["branch", "-D", &self.branch]);
        Ok(())
    }
}

impl Drop for Worktree {
    fn drop(&mut self) {
        // Best-effort cleanup; ignore errors (e.g. already removed).
        let _ = self.remove();
    }
}

/// Clone `git_url` into `dest` (which must not already exist). When `token` is
/// set, authenticates over HTTPS via a transient credential helper — the token
/// is passed through the child environment and **never** written to the cloned
/// repo's config or remote URL.
pub fn clone_repo(git_url: &str, dest: &Path, token: Option<&str>) -> Result<(), WorktreeError> {
    let mut cmd = Command::new("git");
    auth_args(&mut cmd, token);
    cmd.arg("clone").arg(git_url).arg(dest);
    finish(cmd, "git clone")
}

/// Clone `git_url` into `dest` and cut a fresh `new_branch` off
/// `origin/<base_branch>` — the multi-repo analog of [`Worktree::create`] (which
/// worktrees an *existing* local clone). Used to lay out each repo of a
/// multi-repo project into its own folder in the run workspace. Auth as in
/// [`clone_repo`].
pub fn clone_run_branch(
    git_url: &str,
    dest: &Path,
    base_branch: &str,
    new_branch: &str,
    token: Option<&str>,
) -> Result<(), WorktreeError> {
    clone_repo(git_url, dest, token)?;
    run_git(
        dest,
        &[
            "checkout",
            "-b",
            new_branch,
            &format!("origin/{base_branch}"),
        ],
    )?;
    Ok(())
}

/// Fetch + prune `repo` from its origin (so a project's checkout has the latest
/// `base_branch` before a run cuts a worktree off it). Auth as in [`clone_repo`].
pub fn fetch_repo(repo: &Path, token: Option<&str>) -> Result<(), WorktreeError> {
    let mut cmd = Command::new("git");
    cmd.arg("-C").arg(repo);
    auth_args(&mut cmd, token);
    cmd.args(["fetch", "--prune", "origin"]);
    finish(cmd, "git fetch")
}

/// The repo's default branch, detected from `origin/HEAD` (set by `git clone`).
/// Returns the bare branch name (e.g. `main`, `develop`), or `None` if it can't
/// be determined — caller should fall back to a sane default.
pub fn default_branch(repo: &Path) -> Option<String> {
    // `git symbolic-ref refs/remotes/origin/HEAD` → "refs/remotes/origin/<branch>".
    let out = run_git(repo, &["symbolic-ref", "refs/remotes/origin/HEAD"]).ok()?;
    let branch = out.trim().rsplit('/').next()?.trim();
    if branch.is_empty() {
        None
    } else {
        Some(branch.to_string())
    }
}

/// Provision `mise` tool specs globally (e.g. `rust`, `node@22`, `pnpm`) so they
/// resolve for a run. Installs on demand into mise's data dir — which lives under
/// `$HOME` (the persistent volume), so installs are cached across runs and need
/// no image rebuild. No-op for an empty list.
pub fn provision_toolchains(specs: &[String]) -> Result<(), WorktreeError> {
    if specs.is_empty() {
        return Ok(());
    }
    let mut cmd = Command::new("mise");
    cmd.args(["use", "--global", "--yes"]);
    for s in specs {
        cmd.arg(s);
    }
    finish(cmd, "mise use")
}

/// mise's shims directory (`$HOME/.local/share/mise/shims`) — prepend to `PATH`
/// so tools provisioned by [`provision_toolchains`] resolve in node subprocesses.
pub fn mise_shims_dir() -> Option<PathBuf> {
    let home = std::env::var_os("HOME")?;
    Some(
        PathBuf::from(home)
            .join(".local")
            .join("share")
            .join("mise")
            .join("shims"),
    )
}

/// Inject a transient GitHub HTTPS credential helper that reads the token from
/// the child env (`HARNESS_GIT_TOKEN`) — clears inherited helpers first.
fn auth_args(cmd: &mut Command, token: Option<&str>) {
    if let Some(tok) = token {
        cmd.env("HARNESS_GIT_TOKEN", tok);
        cmd.args([
            "-c",
            "credential.helper=",
            "-c",
            "credential.helper=!f() { echo username=x-access-token; echo password=$HARNESS_GIT_TOKEN; }; f",
        ]);
    }
}

fn finish(mut cmd: Command, what: &str) -> Result<(), WorktreeError> {
    let output = cmd
        .output()
        .map_err(|e| WorktreeError(format!("failed to spawn {what}: {e}")))?;
    if !output.status.success() {
        return Err(WorktreeError(format!(
            "{what} failed: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        )));
    }
    Ok(())
}

fn run_git(repo: &Path, args: &[&str]) -> Result<String, WorktreeError> {
    let output = Command::new("git")
        .arg("-C")
        .arg(repo)
        .args(args)
        .output()
        .map_err(|e| WorktreeError(format!("failed to spawn git: {e}")))?;
    if !output.status.success() {
        return Err(WorktreeError(format!(
            "git {} failed: {}",
            args.join(" "),
            String::from_utf8_lossy(&output.stderr).trim()
        )));
    }
    Ok(String::from_utf8_lossy(&output.stdout).to_string())
}

/// Replace characters that are awkward in branch names / paths.
pub fn sanitize_branch_component(s: &str) -> String {
    s.chars()
        .map(|c| {
            if c.is_alphanumeric() || c == '-' || c == '_' {
                c
            } else {
                '-'
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn init_repo(dir: &Path) {
        // Minimal repo with one commit so HEAD is valid.
        let run = |args: &[&str]| {
            let ok = Command::new("git")
                .arg("-C")
                .arg(dir)
                .args(args)
                .output()
                .expect("git")
                .status
                .success();
            assert!(ok, "git {args:?} failed");
        };
        run(&["init", "-q"]);
        run(&["config", "user.email", "t@example.com"]);
        run(&["config", "user.name", "t"]);
        std::fs::write(dir.join("file.txt"), "hello").unwrap();
        run(&["add", "."]);
        run(&["commit", "-q", "-m", "init"]);
    }

    #[test]
    fn creates_and_removes_worktree() {
        let repo = TempDir::new().unwrap();
        init_repo(repo.path());
        let dest = repo.path().join("wt-a");

        {
            let wt = Worktree::create(repo.path(), "HEAD", "harness-run/test", &dest).unwrap();
            assert!(wt.path.exists(), "worktree dir should exist");
            assert!(
                wt.path.join("file.txt").exists(),
                "checked-out content present"
            );
        } // dropped here → removed

        assert!(!dest.exists(), "worktree dir should be removed on drop");
    }

    #[test]
    fn explicit_remove_then_drop_is_safe() {
        let repo = TempDir::new().unwrap();
        init_repo(repo.path());
        let dest = repo.path().join("wt-b");
        let wt = Worktree::create(repo.path(), "HEAD", "harness-run/test2", &dest).unwrap();
        wt.remove().unwrap();
        assert!(!dest.exists());
        // The branch should be deleted too (no accumulation across runs).
        let branches = String::from_utf8(
            Command::new("git")
                .arg("-C")
                .arg(repo.path())
                .args(["branch", "--list", "harness-run/test2"])
                .output()
                .unwrap()
                .stdout,
        )
        .unwrap();
        assert!(
            branches.trim().is_empty(),
            "branch should be gone: {branches:?}"
        );
        // Drop will best-effort remove again; must not panic.
        drop(wt);
    }

    #[test]
    fn sanitizes_branch_components() {
        assert_eq!(sanitize_branch_component("idea-to/pr v2"), "idea-to-pr-v2");
    }
}
