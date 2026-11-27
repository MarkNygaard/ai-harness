//! Filesystem **write confinement** for every step's process, via Linux
//! Landlock.
//!
//! A step — an agent, or a `bash`/`script` body — and every process it starts
//! may write only under an allowlist: its own workspace, the git directories
//! behind it, temp and the per-user caches and agent state under `$HOME`.
//! Everything else stays readable and executable but **not writable**, so a
//! tool cannot overwrite a shared toolchain or system binary (the `zig`
//! wrapper fork bomb that first prompted this).
//!
//! **On by default.** `HARNESS_FS_SANDBOX=off` (or `0`, `false`, `no`) turns it
//! off. It needs Landlock ABI v2 (Linux ≥ 5.19): v1 forbids moving a file into
//! another directory, which package managers do all the time, so on an older
//! kernel, or one without Landlock, steps run unconfined, as they always did.
//!
//! **Safe to use from a multi-threaded process.** The ruleset is built in the
//! parent, before `fork`; the child's `pre_exec` hook only makes two syscalls
//! (`prctl(PR_SET_NO_NEW_PRIVS)` and `landlock_restrict_self`), and allocates
//! nothing. Building the ruleset inside the hook, as the first version did,
//! can deadlock a child forked while another thread holds the allocator's
//! lock.

use std::ffi::OsString;
use std::io;
use std::path::{Path, PathBuf};

/// The switch, read from the server's environment.
pub const ENV_SWITCH: &str = "HARNESS_FS_SANDBOX";

/// Whether steps are confined: yes unless [`ENV_SWITCH`] says otherwise.
pub fn enabled() -> bool {
    switch_allows(std::env::var(ENV_SWITCH).ok().as_deref())
}

fn switch_allows(value: Option<&str>) -> bool {
    !matches!(
        value.map(|v| v.trim().to_ascii_lowercase()).as_deref(),
        Some("off" | "0" | "false" | "no")
    )
}

/// Directories under `$HOME` a step may write: caches, package stores and the
/// agent CLIs' own state. Toolchains (`~/.local/share/mise`, `~/.nvm`,
/// `/usr/local`) are deliberately absent: they are installed by the harness,
/// and overwriting one is the failure this guards against.
const HOME_WRITABLE: &[&str] = &[
    // Agent CLIs' state and config. `.claude-accounts` holds the config
    // folders of further Claude subscriptions.
    ".claude",
    ".claude-accounts",
    ".omp",
    ".cursor",
    ".config",
    // Caches and package stores.
    ".cache",
    ".cargo",
    ".rustup",
    ".npm",
    ".bun",
    ".pnpm-store",
    ".local/share/pnpm",
    ".local/state",
    ".yarn",
    ".nuget",
    ".dotnet",
    ".templateengine",
    "go",
    ".gradle",
    ".m2",
];

/// Files directly in `$HOME` a step may write. A file rule covers the file
/// itself only; a CLI that replaces one by writing a temporary sibling and
/// renaming it needs the directory, and none of these do.
const HOME_WRITABLE_FILES: &[&str] = &[".claude.json", ".claude.json.backup", ".gitconfig"];

/// Environment variables that name a cache or scratch directory a step may
/// write, read from the step's own environment first and the server's second.
///
/// Includes every variable the server's shared dependency caches set
/// (`harness-server` `caches::DEPS_ENV`); a test there fails if one is added
/// without being listed here.
pub const WRITABLE_ENV: &[&str] = &[
    "ARTIFACTS_DIR",
    "HARNESS_ARTIFACTS_DIR",
    // The project's own cache on the persistent volume, for workflows.
    "HARNESS_CACHE_DIR",
    "TMPDIR",
    // Rust
    "CARGO_HOME",
    "CARGO_TARGET_DIR",
    "RUSTUP_HOME",
    // XDG
    "XDG_CACHE_HOME",
    "XDG_STATE_HOME",
    "XDG_CONFIG_HOME",
    // Node
    "npm_config_store_dir",
    "pnpm_config_store_dir",
    "npm_config_cache_dir",
    "pnpm_config_cache_dir",
    "npm_config_cache",
    "PNPM_HOME",
    "PNPM_STORE_DIR",
    "BUN_INSTALL_CACHE_DIR",
    "YARN_CACHE_FOLDER",
    "YARN_GLOBAL_FOLDER",
    // .NET
    "NUGET_PACKAGES",
    "NUGET_HTTP_CACHE_PATH",
    "DOTNET_CLI_HOME",
    // Go
    "GOPATH",
    "GOMODCACHE",
    "GOCACHE",
    // Python, PHP
    "UV_CACHE_DIR",
    "PIP_CACHE_DIR",
    "POETRY_CACHE_DIR",
    "COMPOSER_CACHE_DIR",
    // Browsers
    "PLAYWRIGHT_BROWSERS_PATH",
];

/// Where a step running in `cwd` may write.
///
/// `env` looks a variable up in the step's own environment; the server's is
/// the fallback. Paths that do not exist are kept (Landlock skips them), so the
/// list reads the same on every host.
pub fn step_write_allowlist(cwd: &Path, env: &dyn Fn(&str) -> Option<OsString>) -> Vec<PathBuf> {
    let lookup = |key: &str| env(key).or_else(|| std::env::var_os(key));
    let mut paths = vec![
        cwd.to_path_buf(),
        PathBuf::from("/tmp"),
        PathBuf::from("/var/tmp"),
        // /dev/null, /dev/tty, and /dev/shm, which Chromium needs.
        PathBuf::from("/dev"),
    ];
    // A worktree's commits, index and refs live in the clone it came from,
    // not under the worktree: without these, `git commit` fails.
    paths.extend(git_dirs(cwd));
    if let Ok(children) = std::fs::read_dir(cwd) {
        // A multi-repo workspace holds one worktree per repo folder.
        for child in children.flatten() {
            if child.file_type().map(|t| t.is_dir()).unwrap_or(false) {
                paths.extend(git_dirs(&child.path()));
            }
        }
    }
    if let Some(home) = lookup("HOME") {
        let home = PathBuf::from(home);
        paths.extend(HOME_WRITABLE.iter().map(|sub| home.join(sub)));
        paths.extend(HOME_WRITABLE_FILES.iter().map(|f| home.join(f)));
    }
    paths.extend(
        WRITABLE_ENV
            .iter()
            .filter_map(|key| lookup(key))
            .filter(|v| !v.is_empty())
            .map(PathBuf::from),
    );
    paths.sort();
    paths.dedup();
    paths
}

/// The git directories a checkout at `dir` writes to: for a worktree, its own
/// admin directory and the shared one of the clone it belongs to. Nothing for
/// a plain clone, whose `.git` is already under `dir`.
fn git_dirs(dir: &Path) -> Vec<PathBuf> {
    let Ok(text) = std::fs::read_to_string(dir.join(".git")) else {
        return Vec::new();
    };
    let Some(gitdir) = text.lines().find_map(|l| l.strip_prefix("gitdir:")) else {
        return Vec::new();
    };
    let gitdir = PathBuf::from(gitdir.trim());
    let gitdir = if gitdir.is_absolute() {
        gitdir
    } else {
        dir.join(gitdir)
    };
    let mut out = vec![gitdir.clone()];
    // `<clone>/.git/worktrees/<name>` → `<clone>/.git`, where objects and
    // refs live. `commondir` says so when present; the layout says so too.
    let common = std::fs::read_to_string(gitdir.join("commondir"))
        .ok()
        .map(|c| {
            let c = PathBuf::from(c.trim());
            if c.is_absolute() {
                c
            } else {
                gitdir.join(c)
            }
        })
        .or_else(|| {
            gitdir
                .parent()
                .and_then(Path::parent)
                .map(Path::to_path_buf)
        });
    out.extend(common);
    out
}

/// Whether, and why not, a confinement could be prepared.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Support {
    /// The kernel enforces it.
    Enforced,
    /// Landlock is missing or disabled.
    Unsupported,
    /// Landlock is there, but older than v2: it would block cross-directory
    /// renames, which installs depend on.
    TooOld(i32),
}

/// The kernel's Landlock ABI version, or 0 without Landlock.
#[cfg(target_os = "linux")]
fn kernel_abi() -> i32 {
    const LANDLOCK_CREATE_RULESET_VERSION: libc::c_uint = 1;
    // SAFETY: a query with a null attribute and size 0, as the kernel
    // documents for asking the version; no memory is passed.
    let v = unsafe {
        libc::syscall(
            libc::SYS_landlock_create_ruleset,
            std::ptr::null::<libc::c_void>(),
            0usize,
            LANDLOCK_CREATE_RULESET_VERSION,
        )
    };
    if v < 0 {
        0
    } else {
        v as i32
    }
}

/// A Landlock ruleset built ahead of `fork`, ready to apply in a child.
#[cfg(target_os = "linux")]
pub struct WriteConfinement {
    fd: std::os::fd::OwnedFd,
}

#[cfg(target_os = "linux")]
impl WriteConfinement {
    /// Build a ruleset that allows reading and executing everywhere and
    /// writing only under `allowed`. `Err` carries [`Support`] when the kernel
    /// cannot enforce it, so the caller can run unconfined.
    pub fn prepare(allowed: &[PathBuf]) -> Result<Self, Support> {
        use landlock::{
            Access, AccessFs, PathBeneath, PathFd, Ruleset, RulesetAttr, RulesetCreatedAttr, ABI,
        };
        let abi = kernel_abi();
        if abi <= 0 {
            return Err(Support::Unsupported);
        }
        if abi < 2 {
            return Err(Support::TooOld(abi));
        }
        // v3 adds truncation. Later versions also handle device ioctls, which
        // would need rules for every terminal a tool opens; not worth it.
        let handled = ABI::V3;
        let build = || -> Result<Option<std::os::fd::OwnedFd>, landlock::RulesetError> {
            let mut ruleset = Ruleset::default()
                .handle_access(AccessFs::from_all(handled))?
                .create()?;
            if let Ok(root) = PathFd::new("/") {
                ruleset = ruleset.add_rule(PathBeneath::new(root, AccessFs::from_read(handled)))?;
            }
            for path in allowed {
                if let Ok(fd) = PathFd::new(path) {
                    ruleset =
                        ruleset.add_rule(PathBeneath::new(fd, AccessFs::from_all(handled)))?;
                }
            }
            Ok(ruleset.into())
        };
        match build() {
            Ok(Some(fd)) => Ok(Self { fd }),
            _ => Err(Support::Unsupported),
        }
    }

    /// Apply it to `cmd`: the child, and everything it starts, is confined
    /// from `exec` on.
    pub fn apply_to(self, cmd: &mut std::process::Command) {
        use std::os::fd::AsRawFd;
        use std::os::unix::process::CommandExt;
        let fd = self.fd;
        // SAFETY: runs in the child between fork and exec. It only reads an
        // integer and makes two syscalls; it allocates nothing and takes no
        // locks, so it is async-signal-safe.
        unsafe {
            cmd.pre_exec(move || {
                if libc::prctl(libc::PR_SET_NO_NEW_PRIVS, 1, 0, 0, 0) != 0 {
                    return Err(io::Error::last_os_error());
                }
                if libc::syscall(libc::SYS_landlock_restrict_self, fd.as_raw_fd(), 0) != 0 {
                    return Err(io::Error::last_os_error());
                }
                Ok(())
            });
        }
    }
}

/// Confine `cmd`, a step's process that will run in `cwd`, to writing only
/// where [`step_write_allowlist`] says. Returns whether it is confined; `false`
/// when switched off or when the kernel cannot enforce it, in which case it
/// runs as before.
#[cfg(target_os = "linux")]
pub fn confine_step(
    cmd: &mut std::process::Command,
    cwd: &Path,
    env: &dyn Fn(&str) -> Option<OsString>,
) -> Result<bool, Support> {
    if !enabled() {
        return Ok(false);
    }
    let confinement = WriteConfinement::prepare(&step_write_allowlist(cwd, env))?;
    confinement.apply_to(cmd);
    Ok(true)
}

#[cfg(not(target_os = "linux"))]
pub fn confine_step(
    _cmd: &mut std::process::Command,
    _cwd: &Path,
    _env: &dyn Fn(&str) -> Option<OsString>,
) -> Result<bool, Support> {
    Ok(false)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn on_unless_switched_off() {
        assert!(switch_allows(None));
        assert!(switch_allows(Some("1")));
        assert!(switch_allows(Some("on")));
        for off in ["off", "0", "false", "no", " OFF "] {
            assert!(!switch_allows(Some(off)), "{off}");
        }
    }

    #[test]
    fn a_worktree_can_write_to_the_clone_it_came_from() {
        let dir = tempfile::tempdir().unwrap();
        let clone_git = dir.path().join("clone/.git");
        let admin = clone_git.join("worktrees/run-1");
        std::fs::create_dir_all(&admin).unwrap();
        std::fs::write(admin.join("commondir"), "../..\n").unwrap();
        let ws = dir.path().join("ws");
        let repo = ws.join("frontend");
        std::fs::create_dir_all(&repo).unwrap();
        std::fs::write(repo.join(".git"), format!("gitdir: {}\n", admin.display())).unwrap();

        let allowed = step_write_allowlist(&ws, &|_| None);
        assert!(allowed.contains(&ws));
        assert!(allowed.contains(&admin), "the worktree's admin dir");
        assert!(
            allowed.iter().any(|p| p == &admin.join("../..")),
            "the clone's shared .git, through commondir: {allowed:?}"
        );
    }

    #[test]
    fn the_step_environment_names_more_places() {
        let dir = tempfile::tempdir().unwrap();
        let allowed = step_write_allowlist(dir.path(), &|k| match k {
            "HOME" => Some("/home/h".into()),
            "ARTIFACTS_DIR" => Some("/runs/a".into()),
            "CARGO_TARGET_DIR" => Some("".into()),
            _ => None,
        });
        assert!(allowed.contains(&PathBuf::from("/runs/a")));
        assert!(allowed.contains(&PathBuf::from("/home/h/.claude")));
        assert!(allowed.contains(&PathBuf::from("/home/h/.claude.json")));
        assert!(!allowed.contains(&PathBuf::from("/home/h/.local/share/mise")));
        assert!(
            !allowed.contains(&PathBuf::from("")),
            "an empty value is not a path"
        );
    }

    /// End to end on a kernel with Landlock: a confined child writes where it
    /// may and nowhere else. Skips itself elsewhere.
    #[cfg(target_os = "linux")]
    #[test]
    fn a_confined_child_writes_only_where_allowed() {
        let dir = tempfile::tempdir().unwrap();
        let inside = dir.path().join("inside");
        let outside = dir.path().join("outside");
        std::fs::create_dir_all(&inside).unwrap();
        std::fs::create_dir_all(&outside).unwrap();
        let Ok(confinement) = WriteConfinement::prepare(std::slice::from_ref(&inside)) else {
            eprintln!("no Landlock v2 here; skipping");
            return;
        };
        let mut cmd = std::process::Command::new("sh");
        cmd.arg("-c").arg(format!(
            "echo ok > {in}/a && mkdir {in}/d && mv {in}/a {in}/d/a && \
             ! echo no > {out}/b 2>/dev/null && cat {out}/../inside/d/a",
            in = inside.display(),
            out = outside.display(),
        ));
        confinement.apply_to(&mut cmd);
        let out = cmd.output().unwrap();
        assert!(
            out.status.success(),
            "{:?}",
            String::from_utf8_lossy(&out.stderr)
        );
        assert_eq!(String::from_utf8_lossy(&out.stdout).trim(), "ok");
        assert!(!outside.join("b").exists(), "the write outside was refused");
    }
}
