//! Best-effort filesystem **write confinement** via Linux Landlock.
//!
//! Apply [`restrict_self_writes`] inside a child process's `pre_exec` hook to
//! confine that process — and every descendant it spawns — to writing only
//! under an allowlist of paths. Everything else on the filesystem stays
//! readable and executable but **not writable**, so an agent tool can't
//! overwrite a shared toolchain or system binary (the `zig`-wrapper fork-bomb
//! class of incident).
//!
//! Landlock is an unprivileged Linux LSM (kernel ≥ 5.13) — it needs no
//! namespaces or helper binaries, so it works inside a locked-down container.
//! It's **best-effort**: on a kernel without Landlock the ruleset simply isn't
//! enforced (no error), and on non-Linux platforms this is a no-op.

use std::io;
use std::path::PathBuf;

/// Confine the current thread/process (and its children) to writing only under
/// `allowed`. Intended for a `Command::pre_exec` hook (returns `io::Result`).
///
/// Grants read+execute on the whole filesystem and read+write only under each
/// existing path in `allowed`. Best-effort: degrades to no enforcement on a
/// kernel without Landlock rather than failing the spawn.
#[cfg(target_os = "linux")]
pub fn restrict_self_writes(allowed: &[PathBuf]) -> io::Result<()> {
    use landlock::{
        Access, AccessFs, PathBeneath, PathFd, Ruleset, RulesetAttr, RulesetCreatedAttr, ABI,
    };

    let abi = ABI::V1;
    let map_err = |e: landlock::RulesetError| io::Error::other(e.to_string());

    let mut ruleset = Ruleset::default()
        .handle_access(AccessFs::from_all(abi))
        .map_err(map_err)?
        .create()
        .map_err(map_err)?;

    // Read + execute everywhere: toolchains stay runnable, just not writable.
    if let Ok(root) = PathFd::new("/") {
        ruleset = ruleset
            .add_rule(PathBeneath::new(root, AccessFs::from_read(abi)))
            .map_err(map_err)?;
    }
    // Full (incl. write) access under each allowed path that actually exists.
    for path in allowed {
        if let Ok(fd) = PathFd::new(path) {
            ruleset = ruleset
                .add_rule(PathBeneath::new(fd, AccessFs::from_all(abi)))
                .map_err(map_err)?;
        }
    }
    // `restrict_self` applies the ruleset; with the default best-effort
    // compatibility it returns (un-enforced) rather than erroring on an old
    // kernel, so a missing-Landlock host degrades gracefully.
    ruleset.restrict_self().map_err(map_err)?;
    Ok(())
}

#[cfg(not(target_os = "linux"))]
pub fn restrict_self_writes(_allowed: &[PathBuf]) -> io::Result<()> {
    Ok(())
}
