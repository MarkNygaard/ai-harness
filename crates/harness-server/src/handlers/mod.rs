pub mod dashboard;
pub mod operator_snapshot;
pub mod overview;
pub mod projects;
pub mod reconcile;
pub mod runtime_hosts;
pub mod runtime_project_cache;
pub mod token_usage;
pub mod worktrees;

#[cfg(test)]
mod runtime_project_cache_api_tests;

#[cfg(test)]
mod runtime_hosts_api_tests;

#[cfg(test)]
mod runtime_hosts_workflow_api_tests;

/// Validate that a project root is an existing directory within the given home
/// directory. `home` should be captured once at server startup to avoid TOCTOU
/// races from reading `$HOME` per-request.
/// Returns the canonicalized path on success.
pub(crate) fn validate_project_root(
    path: &std::path::Path,
    home: &std::path::Path,
) -> Result<std::path::PathBuf, String> {
    let canonical = path
        .canonicalize()
        .map_err(|e| format!("invalid project root '{}': {e}", path.display()))?;
    if !canonical.is_dir() {
        return Err(format!(
            "project root is not a directory: {}",
            canonical.display()
        ));
    }
    if !canonical.starts_with(home) {
        return Err(format!(
            "project root must be within HOME: {}",
            canonical.display()
        ));
    }
    Ok(canonical)
}
