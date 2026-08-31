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

/// Prune stale worktree bookkeeping from `repo`: `git worktree prune` drops the
/// `.git/worktrees/<name>` admin entries for worktrees whose directories have
/// already been deleted (e.g. removed out-of-band by the orphan sweeper after a
/// hard process kill skipped the [`Worktree`] `Drop`). Best-effort.
pub fn prune_worktrees(repo: &Path) -> Result<(), WorktreeError> {
    run_git(repo, &["worktree", "prune"])?;
    Ok(())
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

/// The install directory of a mise-provisioned tool (`mise where <tool>`), e.g.
/// `$HOME/.local/share/mise/installs/dotnet/10.0.301`. Used to set `DOTNET_ROOT`
/// for the .NET runtime: a standalone .NET apphost (such as the `al` AL compiler
/// installed via `dotnet tool install`) resolves the runtime from `DOTNET_ROOT`
/// or the default `/usr/share/dotnet`, NOT from `PATH` — so a mise install, which
/// lives in neither, is invisible to it unless `DOTNET_ROOT` points here. Returns
/// `None` if the tool isn't provisioned or `mise` isn't on `PATH`.
pub fn mise_tool_path(tool: &str) -> Option<PathBuf> {
    let output = Command::new("mise").args(["where", tool]).output().ok()?;
    if !output.status.success() {
        return None;
    }
    let path = String::from_utf8(output.stdout).ok()?.trim().to_string();
    if path.is_empty() {
        return None;
    }
    Some(PathBuf::from(path))
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

/// Directory name for `git_url`'s mirror: host + path flattened into one safe
/// segment (`github.com-me-app.git`), so two repos that share a basename — or
/// the same repo written as `https://` and `git@` — can't collide or fork into
/// two mirrors. Scheme and userinfo are dropped for that reason.
fn mirror_name(git_url: &str) -> String {
    let s = git_url.trim().trim_end_matches('/');
    let s = s.strip_suffix(".git").unwrap_or(s);
    let s = s.rsplit("://").next().unwrap_or(s);
    let s = s.rsplit('@').next().unwrap_or(s);
    let mut out = String::with_capacity(s.len() + 4);
    for ch in s.chars() {
        if ch.is_ascii_alphanumeric() || ch == '.' || ch == '-' {
            out.push(ch);
        } else {
            out.push('-');
        }
    }
    // Never let the name start with a dot (hidden, and `.`/`..` are reserved),
    // and keep it inside the 255-byte filename limit with room for `.git`.
    let trimmed: String = out.trim_start_matches('.').chars().take(200).collect();
    if trimmed.is_empty() {
        return "repo.git".to_string();
    }
    format!("{trimmed}.git")
}

/// A persistent bare mirror of `git_url` under `mirror_root` — created on first
/// use, incrementally updated (`remote update --prune`) after. Returns the
/// mirror path.
///
/// The mirror is what makes a run's clone cheap: without it every run of a
/// multi-repo project transfers each repo's whole history again. Auth as in
/// [`clone_repo`].
pub fn ensure_mirror(
    git_url: &str,
    mirror_root: &Path,
    token: Option<&str>,
) -> Result<PathBuf, WorktreeError> {
    std::fs::create_dir_all(mirror_root)
        .map_err(|e| WorktreeError(format!("create mirror root: {e}")))?;
    let dir = mirror_root.join(mirror_name(git_url));
    if dir.join("HEAD").is_file() {
        let mut cmd = Command::new("git");
        cmd.arg("-C").arg(&dir);
        auth_args(&mut cmd, token);
        cmd.args(["remote", "update", "--prune"]);
        finish(cmd, "git remote update")?;
        return Ok(dir);
    }
    // Build into a private temp dir and rename into place, so the mirror path
    // only ever holds a finished mirror. Two runs of the same project starting
    // together is normal here (A/B pairs, an epic's children), and without this
    // they would clone over each other — or one would delete the other's
    // half-written mirror mid-transfer.
    let tmp = mirror_root.join(format!(
        "{}.tmp-{}-{}",
        dir.file_name().unwrap_or_default().to_string_lossy(),
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or_default(),
    ));
    let _ = std::fs::remove_dir_all(&tmp);
    let mut cmd = Command::new("git");
    auth_args(&mut cmd, token);
    cmd.arg("clone").arg("--mirror").arg(git_url).arg(&tmp);
    let cloned = finish(cmd, "git clone --mirror");
    if cloned.is_err() {
        let _ = std::fs::remove_dir_all(&tmp);
        cloned?;
    }
    if std::fs::rename(&tmp, &dir).is_err() {
        // Lost the race (or a stale directory is in the way). A mirror another
        // run just finished is as good as ours; otherwise clear the leftover and
        // let this run fall back to cloning from origin.
        let _ = std::fs::remove_dir_all(&tmp);
        if !dir.join("HEAD").is_file() {
            let _ = std::fs::remove_dir_all(&dir);
            return Err(WorktreeError(format!(
                "could not place mirror at {}",
                dir.display()
            )));
        }
    }
    Ok(dir)
}

/// [`clone_run_branch`], but sourcing objects from the persistent mirror under
/// `mirror_root` instead of the network. Falls back to a plain remote clone when
/// `mirror_root` is `None` or the mirror can't be brought up to date — a stale
/// mirror must never decide what a run builds, so the fallback is a fresh clone,
/// not the mirror we have.
pub fn clone_run_branch_cached(
    git_url: &str,
    dest: &Path,
    base_branch: &str,
    new_branch: &str,
    token: Option<&str>,
    mirror_root: Option<&Path>,
) -> Result<(), WorktreeError> {
    let Some(root) = mirror_root else {
        return clone_run_branch(git_url, dest, base_branch, new_branch, token);
    };
    let mirror = match ensure_mirror(git_url, root, token) {
        Ok(m) => m,
        Err(e) => {
            tracing::warn!("git mirror unusable for {git_url} ({e}); cloning from origin");
            return clone_run_branch(git_url, dest, base_branch, new_branch, token);
        }
    };
    // A local clone hardlinks the object store rather than refetching it: no
    // network, and near-zero extra disk. Hardlinks (not alternates) mean the
    // clone still stands if the mirror is pruned mid-run.
    let mut cmd = Command::new("git");
    cmd.arg("clone").arg(&mirror).arg(dest);
    finish(cmd, "git clone (from mirror)")?;
    // Point `origin` back at the real remote: the mirror is an implementation
    // detail, and the agent's push/PR must reach GitHub, not a local path.
    run_git(dest, &["remote", "set-url", "origin", git_url])?;
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

    #[test]
    fn mirror_name_is_stable_across_url_forms() {
        // https and ssh forms of the same repo share one mirror.
        assert_eq!(
            mirror_name("https://github.com/me/app.git"),
            "github.com-me-app.git"
        );
        assert_eq!(
            mirror_name("git@github.com:me/app.git"),
            "github.com-me-app.git"
        );
        assert_eq!(
            mirror_name("https://x-access-token@github.com/me/app/"),
            "github.com-me-app.git"
        );
        // Same basename, different owner — distinct mirrors.
        assert_ne!(
            mirror_name("https://github.com/me/app"),
            mirror_name("https://github.com/you/app")
        );
    }

    #[test]
    fn mirror_clone_reuses_objects_and_repoints_origin() {
        let origin = TempDir::new().unwrap();
        init_repo(origin.path());
        let mirrors = TempDir::new().unwrap();
        let dest = TempDir::new().unwrap();
        let dest = dest.path().join("checkout");
        let url = origin.path().to_string_lossy().to_string();

        // `git init` defaults to `master` or `main` depending on the install.
        let base = run_git(origin.path(), &["rev-parse", "--abbrev-ref", "HEAD"]).unwrap();
        clone_run_branch_cached(
            &url,
            &dest,
            base.trim(),
            "run/1",
            None,
            Some(mirrors.path()),
        )
        .unwrap();

        // The mirror is on disk, ready to serve the next run incrementally.
        assert!(mirrors
            .path()
            .join(mirror_name(&url))
            .join("HEAD")
            .is_file());
        // `origin` points at the real remote, not the mirror.
        let remote = run_git(&dest, &["remote", "get-url", "origin"]).unwrap();
        assert_eq!(remote.trim(), url.trim());
        // The run branch is checked out.
        let branch = run_git(&dest, &["rev-parse", "--abbrev-ref", "HEAD"]).unwrap();
        assert_eq!(branch.trim(), "run/1");
    }

    #[test]
    fn ensure_mirror_creates_then_updates_in_place() {
        let origin = TempDir::new().unwrap();
        init_repo(origin.path());
        let mirrors = TempDir::new().unwrap();
        let url = origin.path().to_string_lossy().to_string();

        let first = ensure_mirror(&url, mirrors.path(), None).unwrap();
        assert!(first.join("HEAD").is_file());
        // A second call fetches into the same mirror instead of rebuilding it,
        // and leaves no temp directory behind.
        let second = ensure_mirror(&url, mirrors.path(), None).unwrap();
        assert_eq!(first, second);
        let leftovers: Vec<_> = std::fs::read_dir(mirrors.path())
            .unwrap()
            .flatten()
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .filter(|n| n.contains(".tmp-"))
            .collect();
        assert!(leftovers.is_empty(), "temp dirs left: {leftovers:?}");
    }
}
