//! **Linear poller — Slice 3a (dry-run).**
//!
//! Periodically walks every *enabled* `linear_sources` binding and, on each
//! binding's `poll_interval`, asks Linear which issues match (team + source
//! status + eligibility label). It then **logs what it *would* do** — claim one
//! issue and fire the bound workflow — but performs **no** mutation: nothing is
//! claimed, no status is changed, no run is triggered.
//!
//! This is the safe scaffold for the live poller (Slice 3b): the loop, the
//! per-binding interval, the eligible-issue query and the one-at-a-time pick are
//! all here; Slice 3b just swaps the dry-run log for an actual claim + `start_run`
//! behind a per-binding `live` flag.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use harness_sources::linear::LinearClient;
use tokio::time::{interval, Instant, MissedTickBehavior};

use super::runs_routes::RunsState;

/// Base loop cadence. Each binding is only polled once its own
/// `poll_interval_secs` has elapsed; this is just how often we check due-ness.
const POLLER_TICK_SECS: u64 = 30;

/// Spawn the dry-run Linear poller. Best-effort; a no-op when called outside a
/// Tokio runtime (e.g. a synchronous router-build test).
pub(crate) fn spawn_poller(state: Arc<RunsState>) {
    if tokio::runtime::Handle::try_current().is_err() {
        return;
    }
    tokio::spawn(async move {
        // Last poll time per (project, workflow) binding, to honor per-binding
        // intervals across the shared base tick.
        let mut last: HashMap<(String, String), Instant> = HashMap::new();
        let mut tick = interval(Duration::from_secs(POLLER_TICK_SECS));
        tick.set_missed_tick_behavior(MissedTickBehavior::Skip);
        loop {
            tick.tick().await;
            poll_once(&state, &mut last).await;
        }
    });
}

/// The Linear API key from the credential store (provider `linear`), if set.
async fn linear_api_key(state: &Arc<RunsState>) -> Option<String> {
    let store = state.cred_store().await.ok()?;
    let fields = store.get("linear").await.ok()??;
    fields.get("api_key").filter(|k| !k.is_empty()).cloned()
}

async fn poll_once(state: &Arc<RunsState>, last: &mut HashMap<(String, String), Instant>) {
    let store = match state.linear_source_store().await {
        Ok(s) => s,
        Err(_) => return, // no DB configured
    };
    let bindings = match store.list_enabled().await {
        Ok(b) => b,
        Err(e) => {
            tracing::warn!("linear poller: list_enabled failed: {e}");
            return;
        }
    };
    if bindings.is_empty() {
        return;
    }
    let Some(api_key) = linear_api_key(state).await else {
        // Enabled bindings exist but no Linear credential — nothing to poll.
        tracing::debug!(
            "linear poller: {} enabled binding(s) but no `linear` credential",
            bindings.len()
        );
        return;
    };
    let client = LinearClient::new(api_key);

    let now = Instant::now();
    for b in bindings {
        let key = (b.project.clone(), b.workflow.clone());
        let due = last
            .get(&key)
            .map(|t| {
                now.duration_since(*t) >= Duration::from_secs(b.poll_interval_secs.max(0) as u64)
            })
            .unwrap_or(true);
        if !due {
            continue;
        }
        last.insert(key, now);

        match client
            .preview_issues(&b.team_id, &b.source_state_id, b.label.as_deref())
            .await
        {
            Ok(issues) if issues.is_empty() => {
                tracing::debug!(
                    "linear poller [dry-run]: {}/{} — no eligible issues",
                    b.project,
                    b.workflow
                );
            }
            Ok(issues) => {
                let ids: Vec<&str> = issues.iter().map(|i| i.identifier.as_str()).collect();
                let pick = ids.first().copied().unwrap_or("?");
                tracing::info!(
                    "linear poller [dry-run]: {}/{} — {} eligible issue(s) [{}]; would claim {} and fire `{}` (DRY-RUN — not claiming or firing)",
                    b.project,
                    b.workflow,
                    ids.len(),
                    ids.join(", "),
                    pick,
                    b.workflow
                );
            }
            Err(e) => {
                tracing::warn!(
                    "linear poller: preview failed for {}/{}: {}",
                    b.project,
                    b.workflow,
                    e.0
                );
            }
        }
    }
}
