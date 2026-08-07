//! System maintenance endpoints: report and update the bundled agent CLIs.
//!
//! Today this covers the **Claude Code** CLI. The image bakes a copy via
//! `npm install -g` as root, but that global dir isn't writable by the non-root
//! `harness` user, so an in-place update can't touch it (and wouldn't survive a
//! redeploy anyway). Instead we install/update into `$HOME/.local`, whose `bin`
//! is first on `PATH` (so it shadows the image copy) and which is expected to be
//! a persistent volume (so the update sticks across restarts). A best-effort
//! [`bootstrap_claude_code`] seeds that location on startup when it's empty.

use axum::{extract::State, Json};
use serde::Serialize;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use super::state::AppState;

const PKG: &str = "@anthropic-ai/claude-code";
const REGISTRY_LATEST: &str = "https://registry.npmjs.org/@anthropic-ai/claude-code/latest";

#[derive(Serialize)]
pub(crate) struct ClaudeVersionInfo {
    /// Version reported by the on-PATH `claude` binary, e.g. `"2.1.223"`.
    installed: Option<String>,
    /// Latest version published to npm (`null` if the registry was unreachable).
    latest: Option<String>,
    /// True when `latest` is strictly newer than `installed`.
    update_available: bool,
    /// Populated when the latest-version lookup failed (offline, etc.).
    error: Option<String>,
}

/// GET /api/system/claude-version — installed vs latest Claude Code CLI.
pub(crate) async fn claude_version(State(_state): State<Arc<AppState>>) -> Json<ClaudeVersionInfo> {
    let installed = installed_version().await;
    let (latest, error) = match latest_version().await {
        Ok(v) => (Some(v), None),
        Err(e) => (None, Some(e)),
    };
    let update_available = match (installed.as_deref(), latest.as_deref()) {
        (Some(i), Some(l)) => is_newer(l, i),
        _ => false,
    };
    Json(ClaudeVersionInfo {
        installed,
        latest,
        update_available,
        error,
    })
}

#[derive(Serialize)]
pub(crate) struct ClaudeUpdateResult {
    ok: bool,
    installed: Option<String>,
    latest: Option<String>,
    update_available: bool,
    /// Human-readable install log (success) or error detail (failure).
    message: String,
}

/// POST /api/system/claude-update — install the latest Claude Code into
/// `$HOME/.local` (user-writable, PATH-priority, volume-persistent).
pub(crate) async fn claude_update(State(state): State<Arc<AppState>>) -> Json<ClaudeUpdateResult> {
    let prefix = state.core.home_dir.join(".local");
    match run_npm_install_latest(&prefix).await {
        Ok(log) => {
            let installed = installed_version().await;
            let latest = latest_version().await.ok();
            let update_available = match (installed.as_deref(), latest.as_deref()) {
                (Some(i), Some(l)) => is_newer(l, i),
                _ => false,
            };
            Json(ClaudeUpdateResult {
                ok: true,
                installed,
                latest,
                update_available,
                message: log,
            })
        }
        Err(e) => Json(ClaudeUpdateResult {
            ok: false,
            installed: installed_version().await,
            latest: latest_version().await.ok(),
            update_available: false,
            message: e,
        }),
    }
}

/// Best-effort startup seed: if there's no user-local `claude` in `$HOME/.local`,
/// install the latest there so the in-app updater has a writable target and the
/// on-PATH binary is the updatable one. No-op when it already exists (preserves
/// whatever version the user updated to). The image's root-owned copy remains as
/// a fallback while this runs.
pub(crate) async fn bootstrap_claude_code(home_dir: PathBuf) {
    let prefix = home_dir.join(".local");
    let claude_bin = prefix.join("bin").join("claude");
    if claude_bin.exists() {
        return;
    }
    tracing::info!(
        target = %claude_bin.display(),
        "claude-code: no user-local install — bootstrapping latest into $HOME/.local"
    );
    match run_npm_install_latest(&prefix).await {
        Ok(_) => tracing::info!("claude-code: bootstrap install complete"),
        Err(e) => {
            tracing::warn!("claude-code: bootstrap install failed (using image copy): {e}")
        }
    }
}

/// Version reported by the on-PATH `claude` binary. `claude --version` prints
/// e.g. `"2.1.223 (Claude Code)"`; we take the leading version token.
async fn installed_version() -> Option<String> {
    let out = tokio::time::timeout(
        Duration::from_secs(15),
        tokio::process::Command::new("claude")
            .arg("--version")
            .output(),
    )
    .await
    .ok()?
    .ok()?;
    if !out.status.success() {
        return None;
    }
    String::from_utf8_lossy(&out.stdout)
        .split_whitespace()
        .next()
        .map(str::to_string)
}

/// Latest published version from the npm registry. Parsed via `text()` +
/// `serde_json` so we don't depend on reqwest's `json` feature.
async fn latest_version() -> Result<String, String> {
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(10))
        .build()
        .map_err(|e| format!("http client: {e}"))?;
    let resp = client
        .get(REGISTRY_LATEST)
        .send()
        .await
        .map_err(|e| format!("reach npm registry: {e}"))?;
    if !resp.status().is_success() {
        return Err(format!("npm registry returned {}", resp.status()));
    }
    let body = resp
        .text()
        .await
        .map_err(|e| format!("read npm response: {e}"))?;
    let json: serde_json::Value =
        serde_json::from_str(&body).map_err(|e| format!("parse npm response: {e}"))?;
    json.get("version")
        .and_then(|v| v.as_str())
        .map(str::to_string)
        .ok_or_else(|| "npm response missing `version`".to_string())
}

/// `npm install -g --prefix <prefix> @anthropic-ai/claude-code@latest`.
/// Targets a user-writable prefix whose `bin` is first on PATH.
async fn run_npm_install_latest(prefix: &Path) -> Result<String, String> {
    let spec = format!("{PKG}@latest");
    let out = tokio::time::timeout(
        Duration::from_secs(180),
        tokio::process::Command::new("npm")
            .arg("install")
            .arg("-g")
            .arg("--prefix")
            .arg(prefix)
            .arg(&spec)
            .output(),
    )
    .await
    .map_err(|_| "npm install timed out after 180s".to_string())?
    .map_err(|e| format!("spawn npm: {e} (is npm on PATH?)"))?;
    let stderr = String::from_utf8_lossy(&out.stderr);
    if out.status.success() {
        let stdout = String::from_utf8_lossy(&out.stdout);
        let combined = format!("{}\n{}", stdout.trim(), stderr.trim());
        Ok(combined.trim().to_string())
    } else {
        Err(format!(
            "npm install exited {}: {}",
            out.status,
            stderr.trim()
        ))
    }
}

/// Parse `"a.b.c"` (ignoring a `v` prefix or `-pre`/`+build` suffix) into a tuple.
fn parse_semver(v: &str) -> Option<(u64, u64, u64)> {
    let core = v.trim().trim_start_matches('v');
    let core = core.split(['-', '+', ' ']).next().unwrap_or(core);
    let mut it = core.split('.');
    let major = it.next()?.parse().ok()?;
    let minor = it.next()?.parse().ok()?;
    let patch = it.next().unwrap_or("0").parse().ok()?;
    Some((major, minor, patch))
}

/// True if `latest` is strictly newer than `installed`. Falls back to string
/// inequality when either side can't be parsed as semver.
fn is_newer(latest: &str, installed: &str) -> bool {
    match (parse_semver(latest), parse_semver(installed)) {
        (Some(l), Some(i)) => l > i,
        _ => latest != installed,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn semver_parses_version_and_ignores_suffix() {
        assert_eq!(parse_semver("2.1.223"), Some((2, 1, 223)));
        assert_eq!(parse_semver("v2.1.0"), Some((2, 1, 0)));
        assert_eq!(parse_semver("2.1.223 (Claude Code)"), Some((2, 1, 223)));
        assert_eq!(parse_semver("3.0.0-beta.1"), Some((3, 0, 0)));
        assert_eq!(parse_semver("2.1"), Some((2, 1, 0)));
        assert_eq!(parse_semver("not-a-version"), None);
    }

    #[test]
    fn is_newer_compares_numerically_not_lexically() {
        // 2.1.187 vs 2.1.223 — lexical string compare would get this wrong.
        assert!(is_newer("2.1.223", "2.1.187"));
        assert!(is_newer("2.2.0", "2.1.223"));
        assert!(is_newer("3.0.0", "2.9.9"));
        assert!(!is_newer("2.1.187", "2.1.187"));
        assert!(!is_newer("2.1.100", "2.1.187"));
        // 2.1.9 vs 2.1.10 — the classic lexical trap.
        assert!(is_newer("2.1.10", "2.1.9"));
    }

    #[test]
    fn is_newer_falls_back_to_string_inequality_when_unparseable() {
        assert!(is_newer("weird-a", "weird-b"));
        assert!(!is_newer("weird", "weird"));
    }
}
