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
//!
//! An install replaces a package tree that live agents are executing out of, so
//! it never happens under a run: [`cli_update`] queues one while anything is in
//! flight, [`spawn_cli_update_watcher`] applies the queue at the next idle
//! moment, and a lease held for the length of the install holds new runs back
//! (the other half of that interlock is in
//! [`super::runs_routes::start_run`]).

use axum::{
    extract::{Path as AxumPath, State},
    Extension, Json,
};
use serde::Serialize;
use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use super::accounts::AdminOnly;
use super::runs_routes::RunsState;
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
    /// Nothing was installed: runs are in flight, so this is queued for the
    /// next idle moment instead. `ok` is still true — the request succeeded.
    queued: bool,
    /// Human-readable install log (success), error detail (failure), or what
    /// the queue will do (queued).
    message: String,
}

/// POST /api/system/cli-update/{provider} — install that provider's CLI at
/// latest into `$HOME/.local` (user-writable, PATH-priority, volume-persistent),
/// or queue it when a run is in flight.
///
/// Queueing rather than refusing is the point: an `npm install` deletes and
/// re-extracts the package tree, so a CLI spawned during it can fail to exec at
/// all, and one already running can lose a file it lazily reaches for (Claude
/// Code spawns its bundled `rg` per search). Both fail a run for a reason that
/// looks like nothing. An administrator should not have to wait by the page for
/// the cluster to go quiet, so a busy install is remembered and applied by
/// [`spawn_cli_update_watcher`] the moment the last run finishes.
///
/// `POST /api/system/claude-update` stays as an alias: a browser holding the
/// previous bundle keeps working through the window where the server has
/// deployed and the UI has not.
pub(crate) async fn cli_update(
    _: AdminOnly,
    Extension(runs): Extension<Arc<RunsState>>,
    State(state): State<Arc<AppState>>,
    AxumPath(provider): AxumPath<String>,
) -> Json<ClaudeUpdateResult> {
    update_provider(&state, &runs, &provider).await
}

/// POST /api/system/claude-update — the original route, now one case of
/// [`cli_update`].
pub(crate) async fn claude_update(
    _: AdminOnly,
    Extension(runs): Extension<Arc<RunsState>>,
    State(state): State<Arc<AppState>>,
) -> Json<ClaudeUpdateResult> {
    update_provider(&state, &runs, "claude").await
}

/// DELETE /api/system/cli-update/{provider} — drop a queued update.
///
/// A queued action you cannot take back is worse than no queue: whoever
/// changed their mind would otherwise have to wait for it to happen.
pub(crate) async fn cli_update_cancel(
    _: AdminOnly,
    Extension(runs): Extension<Arc<RunsState>>,
    AxumPath(provider): AxumPath<String>,
) -> Json<CliUpdateStatus> {
    if let Ok(store) = runs.settings_store().await {
        let mut pending = read_pending(store).await;
        if pending.remove(&provider) {
            write_pending(store, &pending).await;
        }
    }
    cli_update_status_for(&runs).await
}

async fn update_provider(
    state: &Arc<AppState>,
    runs: &Arc<RunsState>,
    provider: &str,
) -> Json<ClaudeUpdateResult> {
    // An unknown provider, or one installed by something other than npm, is
    // refused here rather than silently reinstalling Claude Code.
    let Some(cli) = agent_cli(provider) else {
        return Json(refused(format!("no agent CLI named `{provider}`")));
    };
    let Some(pkg) = cli.package else {
        let mut r = refused(format!(
            "`{}` is not installed from npm, so it cannot be updated here",
            cli.binary
        ));
        r.installed = cli_version(cli.binary).await;
        return Json(r);
    };
    let Ok(settings) = runs.settings_store().await else {
        // No settings store means no idle interlock, and installing without one
        // is exactly the run-killing behaviour this route exists to avoid.
        return Json(refused(
            "no database configured, so an update cannot be interlocked with \
             running runs"
                .to_string(),
        ));
    };
    // Busy, or another replica is already installing: remember the intent and
    // let the watcher apply it. Checked before the lease so the common busy
    // case never disturbs it.
    let busy = match active_run_count(runs).await {
        Ok(n) => n > 0,
        Err(e) => return Json(refused(format!("could not check for running runs: {e}"))),
    };
    if busy || !claim_install_lease(runs).await {
        let mut pending = read_pending(settings).await;
        pending.insert(cli.provider.to_string());
        write_pending(settings, &pending).await;
        return Json(ClaudeUpdateResult {
            ok: true,
            installed: cli_version(cli.binary).await,
            latest: latest_version(pkg).await.ok(),
            update_available: true,
            queued: true,
            message: format!(
                "runs are in flight — `{}` will be updated as soon as the last one finishes",
                cli.binary
            ),
        });
    }
    // Lease held: no run can start (see the interlock in `start_run`) and none
    // was running, so nothing is spawning the tree we are about to replace.
    let result = install_provider(&state.core.home_dir, cli, pkg).await;
    release_install_lease(runs).await;
    let mut pending = read_pending(settings).await;
    if pending.remove(cli.provider) {
        write_pending(settings, &pending).await;
    }
    record_completed(settings, cli.provider, &result).await;
    Json(result)
}

/// A result that installed nothing and is not going to.
fn refused(message: String) -> ClaudeUpdateResult {
    ClaudeUpdateResult {
        ok: false,
        installed: None,
        latest: None,
        update_available: false,
        queued: false,
        message,
    }
}

/// Install one CLI at latest and report what came out. Assumes the caller holds
/// [`INSTALL_LEASE_KEY`] — the interlock, not the install, is what keeps a run
/// from being spawned into a half-extracted package tree.
async fn install_provider(home_dir: &Path, cli: &AgentCli, pkg: &str) -> ClaudeUpdateResult {
    let prefix = home_dir.join(".local");
    match run_npm_install_latest(&prefix, pkg).await {
        Ok(log) => {
            let installed = cli_version(cli.binary).await;
            let latest = latest_version(pkg).await.ok();
            let update_available = match (installed.as_deref(), latest.as_deref()) {
                (Some(i), Some(l)) => is_newer(l, i),
                _ => false,
            };
            ClaudeUpdateResult {
                ok: true,
                installed,
                latest,
                update_available,
                queued: false,
                message: log,
            }
        }
        Err(e) => ClaudeUpdateResult {
            ok: false,
            installed: cli_version(cli.binary).await,
            latest: latest_version(pkg).await.ok(),
            update_available: false,
            queued: false,
            message: e,
        },
    }
}

// ── The idle interlock ──────────────────────────────────────────────────────
//
// Three settings rows, because all three have to be visible to every replica
// and to survive a restart: a queued update that evaporated on a redeploy would
// silently never happen.

/// Providers waiting for an idle moment, comma-separated.
const PENDING_KEY: &str = "agent_cli_update_pending";
/// Held for the duration of an install. Its presence is what stops a run from
/// starting; see [`update_install_lease_holder`].
pub(crate) const INSTALL_LEASE_KEY: &str = "agent_cli_update_installing";
/// What the queue last did, as JSON, so the page can say so afterwards.
const COMPLETED_KEY: &str = "agent_cli_update_completed";

/// Lease lifetime, derived from the install timeout rather than picked: a lease
/// that expired under a slow-but-working install would stop holding runs back
/// at the worst possible moment, so it must outlast the longest install that
/// can still be in progress. The margin keeps it short enough that a pod killed
/// mid-install stops blocking runs within a few minutes.
pub(crate) const INSTALL_LEASE_SECS: f64 = INSTALL_TIMEOUT.as_secs() as f64 + 120.0;

/// How long a completed update stays worth mentioning on the Agents page.
const COMPLETED_TTL: chrono::Duration = chrono::Duration::hours(24);

/// One finished queued update, kept only to be shown.
#[derive(Serialize, serde::Deserialize, Clone)]
pub(crate) struct CompletedUpdate {
    provider: String,
    ok: bool,
    /// Version now installed, when it could be read back.
    version: Option<String>,
    /// Failure detail. Success needs no explanation, and the install log is
    /// long, so it is dropped rather than stored.
    message: Option<String>,
    at: String,
}

/// GET /api/system/cli-update — what the queue is doing.
///
/// Separate from [`providers`] because it changes on a different clock: a CLI's
/// version only moves when someone installs one, while the run count moves
/// constantly, and the page polls this without re-spawning four processes and
/// four npm lookups to learn it.
#[derive(Serialize)]
pub(crate) struct CliUpdateStatus {
    /// Runs in flight anywhere (this process and every other replica).
    active_runs: usize,
    /// An install is happening right now, here or on another replica.
    installing: bool,
    /// Providers queued for the next idle moment.
    pending: Vec<String>,
    /// What the queue did recently, newest first.
    completed: Vec<CompletedUpdate>,
    /// Set when the queue's own state could not be read (no database).
    error: Option<String>,
}

pub(crate) async fn cli_update_status(
    _: AdminOnly,
    Extension(runs): Extension<Arc<RunsState>>,
) -> Json<CliUpdateStatus> {
    cli_update_status_for(&runs).await
}

async fn cli_update_status_for(runs: &Arc<RunsState>) -> Json<CliUpdateStatus> {
    let Ok(settings) = runs.settings_store().await else {
        return Json(CliUpdateStatus {
            active_runs: 0,
            installing: false,
            pending: Vec::new(),
            completed: Vec::new(),
            error: Some("no database configured".to_string()),
        });
    };
    let (active, installing) = match active_run_count(runs).await {
        Ok(n) => (n, update_install_lease_holder(runs).await.is_some()),
        Err(e) => {
            return Json(CliUpdateStatus {
                active_runs: 0,
                installing: false,
                pending: read_pending(settings).await.into_iter().collect(),
                completed: read_completed(settings).await,
                error: Some(e),
            })
        }
    };
    Json(CliUpdateStatus {
        active_runs: active,
        installing,
        pending: read_pending(settings).await.into_iter().collect(),
        completed: read_completed(settings).await,
        error: None,
    })
}

/// Runs in flight, counted across replicas.
///
/// The union of two views, because neither alone is enough: the database is the
/// only one that sees other replicas, and this process's live map is the only
/// one that sees a run it started moments ago — the `running` row is written by
/// the run task, a beat after [`super::runs_routes::start_run`] returns.
async fn active_run_count(runs: &Arc<RunsState>) -> Result<usize, String> {
    let store = runs.store().await?;
    let mut ids = store
        .running_run_ids()
        .await
        .map_err(|e| format!("read running runs: {e}"))?;
    ids.extend(runs.live_run_ids().await);
    Ok(ids.len())
}

/// Whoever is installing right now, if anyone. Read by `start_run` before it
/// spawns anything.
pub(crate) async fn update_install_lease_holder(runs: &Arc<RunsState>) -> Option<String> {
    runs.settings_store()
        .await
        .ok()?
        .lease_holder(INSTALL_LEASE_KEY, INSTALL_LEASE_SECS)
        .await
        .ok()
        .flatten()
}

/// Take the install lease, if it is free.
async fn claim_install_lease(runs: &Arc<RunsState>) -> bool {
    let Ok(store) = runs.settings_store().await else {
        return false;
    };
    store
        .acquire_lease(INSTALL_LEASE_KEY, runs.instance_id(), INSTALL_LEASE_SECS)
        .await
        .unwrap_or(false)
}

async fn release_install_lease(runs: &Arc<RunsState>) {
    if let Ok(store) = runs.settings_store().await {
        if let Err(e) = store.delete(INSTALL_LEASE_KEY).await {
            // Not fatal: the lease expires on its own, it just blocks runs for
            // longer than it should. Worth a line, since that is confusing.
            tracing::warn!("agent CLI update: could not release the install lease: {e}");
        }
    }
}

async fn read_pending(store: &harness_persist::SettingsStore) -> BTreeSet<String> {
    store
        .get(PENDING_KEY)
        .await
        .ok()
        .flatten()
        .map(|v| parse_pending(&v))
        .unwrap_or_default()
}

/// A comma list, filtered to providers this build can actually install — a
/// queue entry written by an older build, or for a provider since removed from
/// [`AGENT_CLIS`], must not stall the queue forever.
fn parse_pending(raw: &str) -> BTreeSet<String> {
    raw.split(',')
        .map(str::trim)
        .filter(|p| agent_cli(p).is_some_and(|c| c.package.is_some()))
        .map(str::to_string)
        .collect()
}

async fn write_pending(store: &harness_persist::SettingsStore, pending: &BTreeSet<String>) {
    let joined = pending.iter().cloned().collect::<Vec<_>>().join(",");
    let write = if joined.is_empty() {
        store.delete(PENDING_KEY).await.map(|_| ())
    } else {
        store.set(PENDING_KEY, &joined).await
    };
    if let Err(e) = write {
        tracing::warn!("agent CLI update: could not save the queue: {e}");
    }
}

async fn read_completed(store: &harness_persist::SettingsStore) -> Vec<CompletedUpdate> {
    let Some(raw) = store.get(COMPLETED_KEY).await.ok().flatten() else {
        return Vec::new();
    };
    let all: Vec<CompletedUpdate> = serde_json::from_str(&raw).unwrap_or_default();
    let cutoff = chrono::Utc::now() - COMPLETED_TTL;
    all.into_iter().filter(|c| c.is_after(cutoff)).collect()
}

impl CompletedUpdate {
    /// Whether this record is newer than `cutoff`. An unparseable stamp counts
    /// as expired: better to forget a notice than to pin a stale one forever.
    fn is_after(&self, cutoff: chrono::DateTime<chrono::Utc>) -> bool {
        chrono::DateTime::parse_from_rfc3339(&self.at)
            .is_ok_and(|at| at.with_timezone(&chrono::Utc) > cutoff)
    }
}

/// Remember what an install did, so the Agents page can say what happened while
/// nobody was watching. One record per provider, newest kept.
async fn record_completed(
    store: &harness_persist::SettingsStore,
    provider: &str,
    result: &ClaudeUpdateResult,
) {
    let mut kept: Vec<CompletedUpdate> = read_completed(store)
        .await
        .into_iter()
        .filter(|c| c.provider != provider)
        .collect();
    kept.insert(
        0,
        CompletedUpdate {
            provider: provider.to_string(),
            ok: result.ok,
            version: result.installed.clone(),
            message: (!result.ok).then(|| result.message.clone()),
            at: chrono::Utc::now().to_rfc3339(),
        },
    );
    match serde_json::to_string(&kept) {
        Ok(json) => {
            if let Err(e) = store.set(COMPLETED_KEY, &json).await {
                tracing::warn!("agent CLI update: could not record the result: {e}");
            }
        }
        Err(e) => tracing::warn!("agent CLI update: could not encode the result: {e}"),
    }
}

/// How often the watcher looks for an idle moment. Slow on purpose: a queued
/// update is not urgent, and an idle instance should be doing nothing.
const WATCHER_INTERVAL: Duration = Duration::from_secs(30);

/// Apply queued CLI updates as soon as nothing is running.
///
/// The counterpart to the queue in [`cli_update`]: an administrator asked for
/// an update on a busy cluster, and this is what eventually installs it.
/// Every replica runs one; the install lease decides which one acts.
pub(crate) fn spawn_cli_update_watcher(runs: Arc<RunsState>, home_dir: PathBuf) {
    tokio::spawn(async move {
        let mut tick = tokio::time::interval(WATCHER_INTERVAL);
        loop {
            tick.tick().await;
            drain_queued_updates(&runs, &home_dir).await;
        }
    });
}

/// One pass: install everything queued, if and only if the cluster is idle.
async fn drain_queued_updates(runs: &Arc<RunsState>, home_dir: &Path) {
    let Ok(settings) = runs.settings_store().await else {
        return;
    };
    // The cheap check first — an empty queue is the normal state, and this is
    // one indexed lookup every 30 seconds.
    let pending = read_pending(settings).await;
    if pending.is_empty() {
        return;
    }
    match active_run_count(runs).await {
        Ok(0) => {}
        Ok(_) => return,
        Err(e) => {
            tracing::warn!("agent CLI update: cannot tell whether runs are active: {e}");
            return;
        }
    }
    if !claim_install_lease(runs).await {
        return; // another replica got there first
    }
    // Re-check now that runs are interlocked: a run could have started between
    // the count above and the lease below, and it is cheaper to wait 30 seconds
    // than to install underneath it.
    if !matches!(active_run_count(runs).await, Ok(0)) {
        release_install_lease(runs).await;
        return;
    }
    // Sequential: two `npm install --prefix` runs against one prefix contend
    // over the same `lib/node_modules` tree.
    for provider in &pending {
        let Some(cli) = agent_cli(provider) else {
            continue;
        };
        let Some(pkg) = cli.package else { continue };
        tracing::info!("agent CLI update: cluster is idle — installing queued {pkg}@latest");
        let result = install_provider(home_dir, cli, pkg).await;
        if result.ok {
            tracing::info!(
                "agent CLI update: {} is now {}",
                cli.binary,
                result.installed.as_deref().unwrap_or("installed")
            );
        } else {
            tracing::warn!(
                "agent CLI update: {} failed: {}",
                cli.binary,
                result.message
            );
        }
        record_completed(settings, cli.provider, &result).await;
    }
    // Drop what this pass handled, rather than emptying the queue: an update
    // asked for while these were installing is still waiting to happen.
    // Dropped whether or not each install worked — a failing update retried
    // every 30 seconds forever would be worse than one that has to be asked
    // for again, and the page shows why it failed.
    let mut left = read_pending(settings).await;
    left.retain(|p| !pending.contains(p));
    write_pending(settings, &left).await;
    release_install_lease(runs).await;
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

/// How long one `npm install` is given. Also what [`INSTALL_LEASE_SECS`] is
/// derived from: the lease has to outlast the longest install that can still be
/// running, or it stops holding runs back while npm is still writing.
const INSTALL_TIMEOUT: Duration = Duration::from_secs(180);

/// `npm install -g --prefix <prefix> <pkg>@latest`.
/// Targets a user-writable prefix whose `bin` is first on PATH.
async fn run_npm_install_latest(prefix: &Path, pkg: &str) -> Result<String, String> {
    let spec = format!("{pkg}@latest");
    let out = tokio::time::timeout(
        INSTALL_TIMEOUT,
        tokio::process::Command::new("npm")
            .arg("install")
            .arg("-g")
            .arg("--prefix")
            .arg(prefix)
            .arg(&spec)
            .output(),
    )
    .await
    .map_err(|_| format!("npm install timed out after {}s", INSTALL_TIMEOUT.as_secs()))?
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

    /// The queue is a comma list in one settings row, so it outlives the build
    /// that wrote it. An entry naming a provider this build cannot install
    /// would be picked up by the watcher, matched against nothing, and left in
    /// place forever -- with the page showing an update permanently "queued".
    #[test]
    fn the_queue_forgets_providers_this_build_cannot_install() {
        assert_eq!(
            parse_pending("claude,codex"),
            ["claude", "codex"]
                .into_iter()
                .map(str::to_string)
                .collect::<BTreeSet<_>>()
        );
        // Whitespace and empties, as a hand-edited row or an emptied list.
        assert_eq!(
            parse_pending(" claude , "),
            ["claude"].into_iter().map(str::to_string).collect()
        );
        assert!(parse_pending("").is_empty());
        // Known providers, but not installable from here.
        assert!(parse_pending("pi,cursor").is_empty());
        // A provider from another build entirely.
        assert!(parse_pending("gpt").is_empty());
    }

    /// A completed notice is shown for a day and then stops being interesting.
    /// An unparseable stamp has to read as expired: the alternative is a notice
    /// pinned to the page with no way to clear it.
    #[test]
    fn a_completed_notice_expires_and_a_broken_stamp_counts_as_expired() {
        let now = chrono::Utc::now();
        let at = |stamp: &str| CompletedUpdate {
            provider: "claude".to_string(),
            ok: true,
            version: Some("2.1.223".to_string()),
            message: None,
            at: stamp.to_string(),
        };
        let cutoff = now - COMPLETED_TTL;

        assert!(at(&now.to_rfc3339()).is_after(cutoff));
        assert!(at(&(now - chrono::Duration::hours(23)).to_rfc3339()).is_after(cutoff));
        assert!(!at(&(now - chrono::Duration::hours(25)).to_rfc3339()).is_after(cutoff));
        assert!(!at("whenever").is_after(cutoff));
        assert!(!at("").is_after(cutoff));
    }
}
