//! System maintenance endpoints: report and update the bundled agent CLIs.
//!
//! [`providers`] reports what each agent provider's CLI actually is in this
//! image -- present or missing, and at what version -- which the credentials
//! page needs in order to say anything truthful about whether a provider can
//! run. Updating covers the CLIs installed **from npm** -- Claude Code and
//! Codex -- because they share one mechanism. `omp` comes from bun and
//! `cursor-agent` from a vendor installer, so neither can use this path and
//! both report version only. The image bakes a copy via
//! `npm install -g` as root, but that global dir isn't writable by the non-root
//! `harness` user, so an in-place update can't touch it (and wouldn't survive a
//! redeploy anyway). Instead we install/update into `$HOME/.local`, whose `bin`
//! is first on `PATH` (so it shadows the image copy) and which is expected to be
//! a persistent volume (so the update sticks across restarts). A best-effort
//! [`bootstrap_agent_clis`] seeds that location on startup when it's empty.

use axum::{
    extract::{Path as AxumPath, State},
    Json,
};
use serde::Serialize;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use super::accounts::AdminOnly;
use super::state::AppState;

/// The agent providers that run through a CLI: the executable each needs, and
/// the npm package it comes from when it has one.
///
/// Mirrors the dispatch table in `harness-runner`: a provider whose binary is
/// missing from the image cannot run a node, and that is worth saying on the
/// credentials page rather than at the first failed run.
///
/// The package is what makes a CLI updatable here. `None` is not "we haven't
/// got round to it" -- `omp` is installed by bun into `/opt/bun` and
/// `cursor-agent` by a vendor script, so `npm install --prefix` cannot manage
/// either, and offering the button would be a lie.
const AGENT_CLIS: &[AgentCli] = &[
    AgentCli {
        provider: "claude",
        binary: "claude",
        package: Some("@anthropic-ai/claude-code"),
    },
    AgentCli {
        provider: "codex",
        binary: "codex",
        package: Some("@openai/codex"),
    },
    AgentCli {
        provider: "pi",
        binary: "omp",
        package: None,
    },
    AgentCli {
        provider: "cursor",
        binary: "cursor-agent",
        package: None,
    },
];

/// One provider's CLI as this image ships it.
struct AgentCli {
    provider: &'static str,
    binary: &'static str,
    /// npm package, when that is how it is installed.
    package: Option<&'static str>,
}

/// The CLI a provider key names, if the harness knows one.
fn agent_cli(provider: &str) -> Option<&'static AgentCli> {
    AGENT_CLIS.iter().find(|c| c.provider == provider)
}

/// What we can tell about one provider's CLI without running a job.
#[derive(Serialize)]
pub(crate) struct ProviderHealth {
    /// Credential-store key: `claude`, `codex`, `pi`, `cursor`.
    provider: String,
    /// Executable the harness spawns for it.
    binary: String,
    /// Whether that executable resolves on `PATH`.
    on_path: bool,
    /// Version it reports, when it is there to be asked.
    version: Option<String>,
    /// Newer version available. Only for a CLI this can install, so the UI
    /// never offers a button that has nothing behind it.
    latest: Option<String>,
    update_available: bool,
    /// Why the update check came back empty (offline, registry down).
    error: Option<String>,
}

/// GET /api/system/providers - per-provider CLI presence and version.
///
/// Probes run concurrently because each one spawns a process: serially this is
/// most of a second of page load per provider, and they have nothing to say to
/// each other. Admin-only, matching the credentials page it feeds.
pub(crate) async fn providers(
    _: AdminOnly,
    State(_state): State<Arc<AppState>>,
) -> Json<Vec<ProviderHealth>> {
    // Each probe spawns a process and each registry lookup is a round trip;
    // they have nothing to say to each other, so the whole page's worth runs at
    // once rather than one provider at a time.
    let probes = futures::future::join_all(AGENT_CLIS.iter().map(|cli| async move {
        let on_path = which(cli.binary);
        let version = if on_path {
            cli_version(cli.binary).await
        } else {
            None
        };
        // A CLI with no package cannot be updated from here, so it is not worth
        // a request to ask what the newest one would be.
        let latest = match cli.package {
            Some(pkg) => Some(latest_version(pkg).await),
            None => None,
        };
        (cli, on_path, version, latest)
    }))
    .await;

    Json(
        probes
            .into_iter()
            .map(|(cli, on_path, version, latest)| {
                let (latest, error) = match latest {
                    Some(Ok(v)) => (Some(v), None),
                    Some(Err(e)) => (None, Some(e)),
                    // No package: not an error, just nothing to say.
                    None => (None, None),
                };
                ProviderHealth {
                    provider: cli.provider.to_string(),
                    binary: cli.binary.to_string(),
                    on_path,
                    update_available: matches!(
                        (version.as_deref(), latest.as_deref()),
                        (Some(i), Some(l)) if is_newer(l, i)
                    ),
                    latest,
                    error,
                    version,
                }
            })
            .collect(),
    )
}

/// Whether `binary` resolves on `PATH`, without spawning it.
///
/// Deliberately not the `which` crate: this is a `PATH` split and a metadata
/// call, and the answer only ever feeds a status line.
fn which(binary: &str) -> bool {
    let Some(path) = std::env::var_os("PATH") else {
        return false;
    };
    // Windows needs an extension appended; everywhere else the bare name is it.
    let candidates: Vec<String> = if cfg!(windows) {
        std::env::var("PATHEXT")
            .unwrap_or_else(|_| ".EXE;.CMD;.BAT".to_string())
            .split(';')
            .map(|ext| format!("{binary}{ext}"))
            .chain(std::iter::once(binary.to_string()))
            .collect()
    } else {
        vec![binary.to_string()]
    };
    std::env::split_paths(&path).any(|dir| candidates.iter().any(|name| dir.join(name).is_file()))
}

/// Version reported by `<binary> --version`.
///
/// Each of these CLIs prints its version somewhere in a line of prose
/// (`"2.1.223 (Claude Code)"`, `"codex-cli 0.5.0"`), so take the first token
/// that looks like a version rather than assuming a position.
async fn cli_version(binary: &str) -> Option<String> {
    let out = tokio::time::timeout(
        Duration::from_secs(15),
        tokio::process::Command::new(binary)
            .arg("--version")
            .output(),
    )
    .await
    .ok()?
    .ok()?;
    if !out.status.success() {
        return None;
    }
    pick_version(&String::from_utf8_lossy(&out.stdout))
}

/// The version token in a `--version` line.
///
/// Split out from the spawn so the parsing is testable: these CLIs each print
/// their version in a different place in a different sentence, and picking the
/// wrong token shows a confidently wrong number on the page.
fn pick_version(stdout: &str) -> Option<String> {
    stdout
        .split_whitespace()
        .map(|tok| tok.trim_start_matches('v'))
        // A dotted token, because `parse_semver` accepts a bare number and a
        // build counter would otherwise be shown as the version.
        .find(|tok| tok.contains('.') && parse_semver(tok).is_some())
        .map(str::to_string)
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

/// POST /api/system/cli-update/{provider} — install that provider's CLI at
/// latest into `$HOME/.local` (user-writable, PATH-priority, volume-persistent).
///
/// `POST /api/system/claude-update` stays as an alias: a browser holding the
/// previous bundle keeps working through the window where the server has
/// deployed and the UI has not.
pub(crate) async fn cli_update(
    State(state): State<Arc<AppState>>,
    AxumPath(provider): AxumPath<String>,
) -> Json<ClaudeUpdateResult> {
    update_provider(&state, &provider).await
}

/// POST /api/system/claude-update — the original route, now one case of
/// [`cli_update`].
pub(crate) async fn claude_update(State(state): State<Arc<AppState>>) -> Json<ClaudeUpdateResult> {
    update_provider(&state, "claude").await
}

async fn update_provider(state: &Arc<AppState>, provider: &str) -> Json<ClaudeUpdateResult> {
    // An unknown provider, or one installed by something other than npm, is
    // refused here rather than silently reinstalling Claude Code.
    let Some(cli) = agent_cli(provider) else {
        return Json(ClaudeUpdateResult {
            ok: false,
            installed: None,
            latest: None,
            update_available: false,
            message: format!("no agent CLI named `{provider}`"),
        });
    };
    let Some(pkg) = cli.package else {
        return Json(ClaudeUpdateResult {
            ok: false,
            installed: cli_version(cli.binary).await,
            latest: None,
            update_available: false,
            message: format!(
                "`{}` is not installed from npm, so it cannot be updated here",
                cli.binary
            ),
        });
    };
    let prefix = state.core.home_dir.join(".local");
    match run_npm_install_latest(&prefix, pkg).await {
        Ok(log) => {
            let installed = cli_version(cli.binary).await;
            let latest = latest_version(pkg).await.ok();
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
            installed: cli_version(cli.binary).await,
            latest: latest_version(pkg).await.ok(),
            update_available: false,
            message: e,
        }),
    }
}

/// Best-effort startup seed for every npm-installed CLI: if there is no
/// user-local copy in `$HOME/.local`, install the latest there so the in-app
/// updater has a writable target and the on-PATH binary is the updatable one.
///
/// No-op per CLI when it already exists, which preserves whatever version the
/// user updated to. The image's root-owned copies remain as a fallback while
/// this runs. Sequential on purpose: two concurrent `npm install --prefix` runs
/// against one prefix contend over the same `lib/node_modules` tree, and this
/// is startup work nobody is waiting on.
pub(crate) async fn bootstrap_agent_clis(home_dir: PathBuf) {
    let prefix = home_dir.join(".local");
    for cli in AGENT_CLIS {
        let Some(pkg) = cli.package else { continue };
        let bin = prefix.join("bin").join(cli.binary);
        if bin.exists() {
            continue;
        }
        tracing::info!(
            target = %bin.display(),
            "{pkg}: no user-local install — bootstrapping latest into $HOME/.local"
        );
        match run_npm_install_latest(&prefix, pkg).await {
            Ok(_) => tracing::info!("{pkg}: bootstrap install complete"),
            Err(e) => tracing::warn!("{pkg}: bootstrap install failed (using image copy): {e}"),
        }
    }
}

/// Latest published version of `pkg` from the npm registry. Parsed via `text()`
/// + `serde_json` so we don't depend on reqwest's `json` feature.
async fn latest_version(pkg: &str) -> Result<String, String> {
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(10))
        .build()
        .map_err(|e| format!("http client: {e}"))?;
    let resp = client
        .get(format!("https://registry.npmjs.org/{pkg}/latest"))
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

/// `npm install -g --prefix <prefix> <pkg>@latest`.
/// Targets a user-writable prefix whose `bin` is first on PATH.
async fn run_npm_install_latest(prefix: &Path, pkg: &str) -> Result<String, String> {
    let spec = format!("{pkg}@latest");
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

    /// Which CLIs can be updated is decided by one column of the table, and
    /// the UI shows a button wherever the server reports an update. So a
    /// package added here without an installer to match -- or removed while a
    /// button still expects it -- is a button that does nothing.
    #[test]
    fn only_the_npm_installed_clis_offer_an_update() {
        let updatable: Vec<&str> = AGENT_CLIS
            .iter()
            .filter(|c| c.package.is_some())
            .map(|c| c.provider)
            .collect();
        assert_eq!(updatable, vec!["claude", "codex"]);

        // omp comes from bun and cursor-agent from a vendor script; neither can
        // be reached by `npm install --prefix`.
        for provider in ["pi", "cursor"] {
            assert!(
                agent_cli(provider).expect(provider).package.is_none(),
                "{provider} is not installed from npm"
            );
        }
    }

    #[test]
    fn an_unknown_provider_names_no_cli() {
        assert!(agent_cli("gpt").is_none());
        assert!(agent_cli("").is_none());
        // The binary is not the key: the credential store keys on the provider.
        assert!(agent_cli("cursor-agent").is_none());
        assert_eq!(agent_cli("cursor").unwrap().binary, "cursor-agent");
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

    #[test]
    fn picks_the_version_wherever_each_cli_puts_it() {
        // Real shapes: leading (Claude Code), trailing after a name (Codex),
        // and a `v` prefix.
        assert_eq!(
            pick_version("2.1.223 (Claude Code)"),
            Some("2.1.223".to_string())
        );
        assert_eq!(pick_version("codex-cli 0.5.0"), Some("0.5.0".to_string()));
        assert_eq!(
            pick_version("cursor-agent version v1.4.2"),
            Some("1.4.2".to_string())
        );
    }

    #[test]
    fn ignores_words_that_only_look_numeric() {
        // A bare number is not a version -- taking one would put "2" on the page.
        assert_eq!(pick_version("omp 2 (build 7)"), None);
        assert_eq!(pick_version(""), None);
        assert_eq!(pick_version("command not found"), None);
    }

    #[test]
    fn a_binary_that_cannot_exist_is_not_on_path() {
        assert!(!which("harness-no-such-binary-84e1c0"));
    }
}
