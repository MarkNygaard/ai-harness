//! Caches that survive a run: package-manager downloads and git mirrors.
//!
//! Every run works in a throwaway clone or worktree, so anything a build writes
//! *inside* the tree (`node_modules`, `obj/`, `bin/`, `.next/`) is cold every
//! time. What can be reused is the layer underneath — the package managers' own
//! download caches (pnpm's content-addressed store, NuGet's global packages
//! folder, Go's module cache) and git's object store. Those default to somewhere
//! under `$HOME`, which is warm only by accident of how the container happens to
//! be mounted; pointing them at directories the harness owns makes the reuse
//! deliberate, measurable (`GET /api/projects/{name}/cache-size`) and bounded.
//!
//! The dependency cache is **shared by every project** on purpose: pnpm's store
//! and NuGet's packages folder are content-addressed, so two projects on the same
//! dependency version store it once. Per-project copies would multiply the
//! largest thing on the volume and buy nothing. It also sits next to the
//! worktrees on the same filesystem, which is what lets pnpm hardlink
//! `node_modules` into a fresh tree instead of copying it.
//!
//! **Deliberately not cached: anything that decides whether to re-run a step** —
//! turbo/nx task caches, `.next/cache`. Codegen (Sanity types, GraphQL) reads a
//! remote schema that no hash of the tree can see, so a task cache will happily
//! replay a stale `typecheck` against types that changed upstream. Reusing a
//! *download* is safe; reusing a *conclusion* is not.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use super::runs_routes::dir_size;

/// Default cap for the shared dependency cache, in GiB. Override with
/// `HARNESS_DEPS_CACHE_CAP_GB`; `0` disables sweeping entirely.
pub(crate) const DEPS_CAP_GB_DEFAULT: u64 = 20;

/// Default cap for one project's workflow cache, in GiB. Override with
/// `HARNESS_PROJECT_CACHE_CAP_GB`; `0` disables sweeping it.
///
/// Smaller than the dependency cap on purpose: this holds whatever a workflow
/// decided to keep (BC symbol packages, a downloaded SDK), keyed by the author.
/// A key that changes every run would otherwise grow without bound.
pub(crate) const PROJECT_CAP_GB_DEFAULT: u64 = 5;

/// How long an unused git mirror is kept before the sweeper drops it. Override
/// with `HARNESS_GIT_MIRROR_TTL_DAYS`; `0` keeps mirrors forever.
pub(crate) const GIT_MIRROR_TTL_DAYS_DEFAULT: u64 = 30;

/// The shared dependency cache root (`<projects_dir>/.deps-cache`).
pub(crate) fn deps_root(projects_dir: &Path) -> PathBuf {
    projects_dir.join(".deps-cache")
}

/// Root of every project's workflow cache (`<projects_dir>/.project-cache`).
pub(crate) fn project_cache_all(projects_dir: &Path) -> PathBuf {
    projects_dir.join(".project-cache")
}

/// One project's workflow cache — handed to runs as `HARNESS_CACHE_DIR`.
///
/// For what only the workflow author knows is cacheable: BC symbol packages
/// keyed by `app.json`, a downloaded SDK, a generated fixture that costs more to
/// rebuild than to keep. Per project, because the contents are a project's
/// business, and swept by whole immediate subdirectory — so a workflow should
/// put its cache key in the *top-level* name (`alpackages-<hash>/`) rather than
/// nesting keys, or eviction takes every key at once.
pub(crate) fn project_cache(projects_dir: &Path, project: &str) -> PathBuf {
    project_cache_all(projects_dir).join(project)
}

/// Effective per-project workflow-cache cap in GiB.
pub(crate) fn project_cap_gb() -> u64 {
    parse_env_u64("HARNESS_PROJECT_CACHE_CAP_GB").unwrap_or(PROJECT_CAP_GB_DEFAULT)
}

/// The shared git mirror root (`<projects_dir>/.git-cache`).
pub(crate) fn git_mirror_root(projects_dir: &Path) -> PathBuf {
    projects_dir.join(".git-cache")
}

/// Environment variable → cache subdirectory, one row per ecosystem the
/// bundled `install-deps` step knows how to install.
///
/// Two names for pnpm on purpose: pnpm ≤10 reads npm-style `npm_config_*`, pnpm
/// 11 dropped that in favour of `pnpm_config_*`. Setting both keeps a project
/// cached across a pnpm major bump instead of silently going cold.
///
/// `CARGO_HOME` is *not* here: it holds the toolchain's own binaries as well as
/// the registry cache, and moving it out from under a mise-provisioned Rust
/// breaks the shims. Rust reuse is the per-project `CARGO_TARGET_DIR` instead.
const DEPS_ENV: &[(&str, &str)] = &[
    // Node — pnpm's store is what makes a fresh `node_modules` a hardlink pass
    // rather than a download.
    ("npm_config_store_dir", "pnpm-store"),
    ("pnpm_config_store_dir", "pnpm-store"),
    ("npm_config_cache_dir", "pnpm-metadata"),
    ("pnpm_config_cache_dir", "pnpm-metadata"),
    ("npm_config_cache", "npm"),
    ("BUN_INSTALL_CACHE_DIR", "bun"),
    ("YARN_CACHE_FOLDER", "yarn"),
    ("YARN_GLOBAL_FOLDER", "yarn-berry"),
    // .NET
    ("NUGET_PACKAGES", "nuget"),
    ("NUGET_HTTP_CACHE_PATH", "nuget-http"),
    // Go
    ("GOMODCACHE", "go-mod"),
    ("GOCACHE", "go-build"),
    // Python
    ("UV_CACHE_DIR", "uv"),
    ("PIP_CACHE_DIR", "pip"),
    ("POETRY_CACHE_DIR", "poetry"),
    // PHP
    ("COMPOSER_CACHE_DIR", "composer"),
];

/// Environment for a run: every package manager pointed at the shared cache.
/// Creates the directories (a package manager handed a missing cache dir is
/// usually fine, but `GOCACHE` and NuGet both prefer it to exist) and skips any
/// it can't create, so a read-only volume degrades to "uncached" rather than
/// failing the run.
pub(crate) fn deps_env(root: &Path) -> HashMap<String, String> {
    let mut env = HashMap::new();
    for (var, sub) in DEPS_ENV {
        let dir = root.join(sub);
        if std::fs::create_dir_all(&dir).is_err() {
            continue;
        }
        env.insert((*var).to_string(), dir.display().to_string());
    }
    env
}

/// Effective dependency-cache cap in GiB (`HARNESS_DEPS_CACHE_CAP_GB` → default).
pub(crate) fn deps_cap_gb() -> u64 {
    parse_env_u64("HARNESS_DEPS_CACHE_CAP_GB").unwrap_or(DEPS_CAP_GB_DEFAULT)
}

/// Effective git-mirror TTL in days (`HARNESS_GIT_MIRROR_TTL_DAYS` → default).
pub(crate) fn git_mirror_ttl_days() -> u64 {
    parse_env_u64("HARNESS_GIT_MIRROR_TTL_DAYS").unwrap_or(GIT_MIRROR_TTL_DAYS_DEFAULT)
}

fn parse_env_u64(key: &str) -> Option<u64> {
    std::env::var(key).ok()?.trim().parse::<u64>().ok()
}

/// Bound a cache directory: when it exceeds `cap` bytes, drop whole immediate
/// subdirectories (least recently written first) until it is under `target`.
///
/// Serves both the shared dependency cache (subdirectory per ecosystem) and a
/// project's workflow cache (subdirectory per cache key).
///
/// Whole subdirectories, not individual files — unlike a cargo target dir, these
/// caches are indexed. Deleting one file out of pnpm's content-addressed store
/// leaves an index entry pointing at nothing, which fails an install; deleting
/// the store leaves a cold cache the next install simply refills. A subdirectory
/// written within `floor_secs` is left alone so a concurrent install (possibly on
/// another replica) can't have the store pulled out from under it.
///
/// Returns `(before, after)` bytes when it acted, else `None`.
pub(crate) fn sweep_whole_subdirs(
    root: &Path,
    cap: u64,
    target: u64,
    floor_secs: u64,
) -> Option<(u64, u64)> {
    if cap == 0 || !root.is_dir() {
        return None;
    }
    let mut dirs: Vec<(PathBuf, u64, std::time::SystemTime)> = Vec::new();
    let mut total = 0u64;
    for entry in std::fs::read_dir(root).ok()?.flatten() {
        if !entry.file_type().map(|t| t.is_dir()).unwrap_or(false) {
            continue;
        }
        let path = entry.path();
        let size = dir_size(&path);
        total = total.saturating_add(size);
        dirs.push((path.clone(), size, newest_mtime(&path)));
    }
    if total <= cap {
        return None;
    }
    dirs.sort_by_key(|(_, _, mtime)| *mtime); // least recently written first
    let now = std::time::SystemTime::now();
    let floor = std::time::Duration::from_secs(floor_secs);
    let mut remaining = total;
    for (path, size, mtime) in dirs {
        if remaining <= target {
            break;
        }
        if now
            .duration_since(mtime)
            .map(|age| age < floor)
            .unwrap_or(false)
        {
            continue;
        }
        if std::fs::remove_dir_all(&path).is_ok() {
            remaining = remaining.saturating_sub(size);
        }
    }
    Some((total, remaining))
}

/// Drop git mirrors nothing has fetched for `ttl_days`. A mirror is pure cache:
/// the next run that needs it re-mirrors, and existing run clones hardlink their
/// objects, so removing one can't break a checkout that is already on disk.
/// Returns the number of mirrors removed.
pub(crate) fn sweep_git_mirrors(root: &Path, ttl_days: u64) -> usize {
    if ttl_days == 0 || !root.is_dir() {
        return 0;
    }
    let ttl = std::time::Duration::from_secs(ttl_days.saturating_mul(24 * 60 * 60));
    let now = std::time::SystemTime::now();
    let mut removed = 0;
    let Ok(rd) = std::fs::read_dir(root) else {
        return 0;
    };
    for entry in rd.flatten() {
        if !entry.file_type().map(|t| t.is_dir()).unwrap_or(false) {
            continue;
        }
        let path = entry.path();
        let age = now
            .duration_since(newest_mtime(&path))
            .unwrap_or(std::time::Duration::ZERO);
        if age >= ttl && std::fs::remove_dir_all(&path).is_ok() {
            removed += 1;
        }
    }
    removed
}

/// The most recent mtime of any file under `dir` — "when was this cache last
/// written", which is what makes it evictable. A directory's own mtime only
/// tracks its immediate entries, so it stays old while a nested cache is busy.
fn newest_mtime(dir: &Path) -> std::time::SystemTime {
    let mut newest = std::time::UNIX_EPOCH;
    let mut stack = vec![dir.to_path_buf()];
    while let Some(d) = stack.pop() {
        let Ok(rd) = std::fs::read_dir(&d) else {
            continue;
        };
        for entry in rd.flatten() {
            let Ok(ft) = entry.file_type() else { continue };
            if ft.is_symlink() {
                continue;
            }
            if ft.is_dir() {
                stack.push(entry.path());
            } else if let Ok(meta) = entry.metadata() {
                if let Ok(mtime) = meta.modified() {
                    if mtime > newest {
                        newest = mtime;
                    }
                }
            }
        }
    }
    newest
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write(path: &Path, bytes: usize) {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, vec![b'x'; bytes]).unwrap();
    }

    #[test]
    fn deps_env_points_every_manager_into_the_cache() {
        let dir = tempfile::TempDir::new().unwrap();
        let env = deps_env(dir.path());
        // pnpm's two spellings must agree, or a pnpm major bump goes cold.
        assert_eq!(env["npm_config_store_dir"], env["pnpm_config_store_dir"]);
        assert!(env["npm_config_store_dir"].ends_with("pnpm-store"));
        assert!(env["NUGET_PACKAGES"].ends_with("nuget"));
        assert!(dir.path().join("pnpm-store").is_dir());
        // Task caches stay out: they decide whether to re-run codegen.
        assert!(!env.contains_key("TURBO_CACHE_DIR"));
        assert!(!env.contains_key("CARGO_HOME"));
    }

    #[test]
    fn sweep_whole_subdirs_noop_under_cap() {
        let dir = tempfile::TempDir::new().unwrap();
        write(&dir.path().join("pnpm-store/a"), 1024);
        assert_eq!(
            sweep_whole_subdirs(dir.path(), 1024 * 1024, 512 * 1024, 0),
            None
        );
        assert!(dir.path().join("pnpm-store").is_dir());
    }

    #[test]
    fn sweep_whole_subdirs_drops_whole_subdirs_oldest_first() {
        let dir = tempfile::TempDir::new().unwrap();
        write(&dir.path().join("nuget/a"), 400 * 1024);
        // Give the second cache a newer mtime than the first.
        std::thread::sleep(std::time::Duration::from_millis(20));
        write(&dir.path().join("pnpm-store/b"), 400 * 1024);
        let (before, after) = sweep_whole_subdirs(dir.path(), 512 * 1024, 410 * 1024, 0).unwrap();
        assert!(before > after, "{before} -> {after}");
        assert!(!dir.path().join("nuget").exists(), "oldest goes first");
        assert!(dir.path().join("pnpm-store").is_dir(), "newest survives");
    }

    #[test]
    fn sweep_whole_subdirs_safety_floor_protects_a_live_install() {
        let dir = tempfile::TempDir::new().unwrap();
        write(&dir.path().join("pnpm-store/a"), 600 * 1024);
        assert_eq!(
            sweep_whole_subdirs(dir.path(), 512 * 1024, 400 * 1024, 3600),
            Some((600 * 1024, 600 * 1024))
        );
        assert!(dir.path().join("pnpm-store").is_dir());
    }

    #[test]
    fn project_cache_is_per_project_and_capped() {
        let root = Path::new("/srv/projects");
        let a = project_cache(root, "bc-customizations");
        let b = project_cache(root, "dilling-ecom");
        assert_ne!(a, b, "one project's cache is not another's");
        assert!(a.starts_with(project_cache_all(root)));
        assert_eq!(project_cap_gb(), PROJECT_CAP_GB_DEFAULT);
    }

    #[test]
    fn a_project_cache_evicts_one_key_not_all_of_them() {
        // Why a workflow puts its key in the top-level name: eviction is by
        // immediate subdirectory, so `alpackages-<old>` can go while
        // `alpackages-<new>` stays.
        let dir = tempfile::TempDir::new().unwrap();
        write(&dir.path().join("alpackages-old/Base.app"), 400 * 1024);
        std::thread::sleep(std::time::Duration::from_millis(20));
        write(&dir.path().join("alpackages-new/Base.app"), 400 * 1024);
        sweep_whole_subdirs(dir.path(), 512 * 1024, 410 * 1024, 0).unwrap();
        assert!(!dir.path().join("alpackages-old").exists());
        assert!(dir.path().join("alpackages-new").is_dir());
    }
    #[test]
    fn sweep_git_mirrors_keeps_fresh_and_is_disabled_by_zero() {
        let dir = tempfile::TempDir::new().unwrap();
        write(&dir.path().join("github.com-me-app.git/HEAD"), 32);
        assert_eq!(sweep_git_mirrors(dir.path(), 30), 0);
        assert_eq!(sweep_git_mirrors(dir.path(), 0), 0);
        assert!(dir.path().join("github.com-me-app.git").is_dir());
    }
}
