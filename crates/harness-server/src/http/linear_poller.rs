//! **Linear poller (Phase 8, Slice 3b).**
//!
//! Every tick it does two things:
//!
//! 1. **Status-sync** — for each in-flight claim, drive the issue through the
//!    lifecycle based on its run's progress:
//!      - a `delivery`-category node succeeded (PR opened) → **In Review**,
//!      - run completed → **ready** (Functional testing),
//!      - run failed/cancelled → **back to its original state**.
//! 2. **Claim/fire** — for each *enabled* binding, on its `poll_interval`:
//!      - `live`  → claim one eligible issue (move to In Progress, fire the bound
//!        workflow, record the claim) — **one at a time per binding**,
//!      - dry-run → just log what it *would* do.
//!
//! Per-project Linear key (project-scoped, else global). The `live` flag defaults
//! to false, so a binding is dry-run until explicitly switched on.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::time::Duration;

use harness_persist::LinearSource;
use harness_sources::linear::{Issue, LinearClient};
use tokio::time::{interval, Instant, MissedTickBehavior};

use super::runs_routes::{start_run, CreateRunRequest, RunsState};

/// Base loop cadence. A binding is only *claimed* once its own
/// `poll_interval_secs` has elapsed; status-sync runs every tick.
const POLLER_TICK_SECS: u64 = 30;

/// Spawn the Linear poller. Best-effort; a no-op outside a Tokio runtime.
pub(crate) fn spawn_poller(state: Arc<RunsState>) {
    if tokio::runtime::Handle::try_current().is_err() {
        return;
    }
    tokio::spawn(async move {
        let mut last: HashMap<(String, String), Instant> = HashMap::new();
        let mut tick = interval(Duration::from_secs(POLLER_TICK_SECS));
        tick.set_missed_tick_behavior(MissedTickBehavior::Skip);
        loop {
            tick.tick().await;
            poll_once(&state, &mut last).await;
        }
    });
}

/// The Linear API key for `project` (project-scoped credential first, else global).
async fn linear_key_for_project(state: &Arc<RunsState>, project: &str) -> Option<String> {
    let store = state.cred_store().await.ok()?;
    let fields = store.get_for_project(project, "linear").await.ok()??;
    fields.get("api_key").filter(|k| !k.is_empty()).cloned()
}

async fn poll_once(state: &Arc<RunsState>, last: &mut HashMap<(String, String), Instant>) {
    // 1. Drive transitions for in-flight claims (independent of bindings).
    sync_active_claims(state).await;

    // 2. Claim / dry-run per enabled binding, honoring per-binding intervals.
    let source_store = match state.linear_source_store().await {
        Ok(s) => s,
        Err(_) => return, // no DB configured
    };
    let bindings = match source_store.list_enabled().await {
        Ok(b) => b,
        Err(e) => {
            tracing::warn!("linear poller: list_enabled failed: {e}");
            return;
        }
    };

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

        let Some(api_key) = linear_key_for_project(state, &b.project).await else {
            tracing::debug!(
                "linear poller: {}/{} — no Linear credential for project",
                b.project,
                b.workflow
            );
            continue;
        };
        let client = LinearClient::new(api_key);

        if b.live {
            claim_and_fire(state, &client, &b).await;
        } else {
            dry_run_log(&client, &b).await;
        }
    }
}

/// Log what a binding *would* claim, without mutating anything.
async fn dry_run_log(client: &LinearClient, b: &LinearSource) {
    match client
        .preview_issues(&b.team_id, &b.source_state_id, b.label.as_deref())
        .await
    {
        Ok(issues) if issues.is_empty() => {}
        Ok(issues) => {
            let ids: Vec<&str> = issues.iter().map(|i| i.identifier.as_str()).collect();
            let pick = ids.first().copied().unwrap_or("?");
            tracing::info!(
                "linear poller [dry-run]: {}/{} — {} eligible [{}]; would claim {} and fire `{}` (set live=true to act)",
                b.project, b.workflow, ids.len(), ids.join(", "), pick, b.workflow
            );
        }
        Err(e) => tracing::warn!(
            "linear poller: preview failed for {}/{}: {}",
            b.project,
            b.workflow,
            e.0
        ),
    }
}

/// Claim one eligible issue for a live binding and fire its workflow — one at a
/// time per binding.
async fn claim_and_fire(state: &Arc<RunsState>, client: &LinearClient, b: &LinearSource) {
    let claim_store = match state.linear_claim_store().await {
        Ok(s) => s,
        Err(_) => return,
    };
    // One-at-a-time: skip if this binding already has an active claim.
    match claim_store.has_active(&b.project, &b.workflow).await {
        Ok(true) => return,
        Ok(false) => {}
        Err(e) => {
            tracing::warn!(
                "linear poller: has_active failed for {}/{}: {e}",
                b.project,
                b.workflow
            );
            return;
        }
    }

    let issues = match client
        .preview_issues(&b.team_id, &b.source_state_id, b.label.as_deref())
        .await
    {
        Ok(i) => i,
        Err(e) => {
            tracing::warn!(
                "linear poller: preview failed for {}/{}: {}",
                b.project,
                b.workflow,
                e.0
            );
            return;
        }
    };
    let Some(issue) = issues.into_iter().next() else {
        return; // nothing eligible
    };

    // Move to In Progress first — this is also the claim signal (it leaves the
    // source column, so the next poll won't re-pick it).
    if let Some(in_progress) = &b.in_progress_state_id {
        if let Err(e) = client.set_issue_state(&issue.id, in_progress).await {
            tracing::warn!(
                "linear poller: {}/{} — failed to move {} to In Progress: {}",
                b.project,
                b.workflow,
                issue.identifier,
                e.0
            );
            return;
        }
    }

    let req = CreateRunRequest {
        workflow: b.workflow.clone(),
        title: Some(format!("{} {}", issue.identifier, issue.title)),
        description: task_for_issue(&issue),
        args: String::new(),
        real: true,
        base_branch: b.base_branch.clone(),
        project: Some(b.project.clone()),
    };
    match start_run(state, req).await {
        Ok(run_id) => {
            if let Err(e) = claim_store
                .record(
                    &run_id,
                    &b.project,
                    &b.workflow,
                    &issue.id,
                    &issue.identifier,
                    &b.source_state_id,
                )
                .await
            {
                tracing::warn!("linear poller: failed to record claim for {}: {e}", run_id);
            }
            let _ = client
                .add_comment(
                    &issue.id,
                    &format!("🤖 ai-harness started `{}` (run `{}`).", b.workflow, run_id),
                )
                .await;
            tracing::info!(
                "linear poller: {}/{} — claimed {} → fired run {}",
                b.project,
                b.workflow,
                issue.identifier,
                run_id
            );
        }
        Err((_, e)) => {
            tracing::warn!(
                "linear poller: {}/{} — start_run failed for {}: {} (rolling back state)",
                b.project,
                b.workflow,
                issue.identifier,
                e
            );
            // Undo the In Progress move so the issue can be retried next poll.
            let _ = client.set_issue_state(&issue.id, &b.source_state_id).await;
        }
    }
}

/// The task spec handed to the fired run: the issue identifier/title/url + body.
fn task_for_issue(issue: &Issue) -> String {
    format!(
        "Linear issue {} — {}\n{}\n\n{}",
        issue.identifier,
        issue.title,
        issue.url,
        issue.body.as_deref().unwrap_or("").trim()
    )
}

/// Walk in-flight claims and transition their issues based on run progress.
async fn sync_active_claims(state: &Arc<RunsState>) {
    let claim_store = match state.linear_claim_store().await {
        Ok(s) => s,
        Err(_) => return,
    };
    let claims = match claim_store.list_active().await {
        Ok(c) => c,
        Err(e) => {
            tracing::warn!("linear poller: list_active claims failed: {e}");
            return;
        }
    };
    if claims.is_empty() {
        return;
    }
    let run_store = match state.store().await {
        Ok(s) => s,
        Err(_) => return,
    };
    let source_store = state.linear_source_store().await.ok();

    for c in claims {
        let detail = match run_store.get_run(&c.run_id).await {
            Ok(Some(d)) => d,
            Ok(None) => {
                // Run row gone — stop tracking this claim.
                let _ = claim_store.set_phase(&c.run_id, "done").await;
                continue;
            }
            Err(_) => continue,
        };
        let Some(api_key) = linear_key_for_project(state, &c.project).await else {
            continue;
        };
        let client = LinearClient::new(api_key);
        let binding = match &source_store {
            Some(s) => s.get(&c.project, &c.workflow).await.ok().flatten(),
            None => None,
        };

        match detail.run.status.as_str() {
            "completed" => {
                if let Some(ready) = binding.as_ref().and_then(|b| b.ready_state_id.as_deref()) {
                    let _ = client.set_issue_state(&c.issue_id, ready).await;
                }
                let _ = client
                    .add_comment(
                        &c.issue_id,
                        &format!("✅ ai-harness run `{}` completed.", c.run_id),
                    )
                    .await;
                let _ = claim_store.set_phase(&c.run_id, "done").await;
                tracing::info!("linear poller: {} — run completed → ready", c.identifier);
            }
            "failed" | "cancelled" => {
                // Roll back to the state the issue was claimed from.
                let _ = client
                    .set_issue_state(&c.issue_id, &c.original_state_id)
                    .await;
                let _ = client
                    .add_comment(
                        &c.issue_id,
                        &format!(
                            "⚠️ ai-harness run `{}` did not complete ({}); returned to its previous state.",
                            c.run_id, detail.run.status
                        ),
                    )
                    .await;
                let _ = claim_store.set_phase(&c.run_id, "done").await;
                tracing::info!(
                    "linear poller: {} — run {} → rolled back",
                    c.identifier,
                    detail.run.status
                );
            }
            _ => {
                // Still running: move to Review once the PR exists (a delivery
                // node has succeeded), and only once.
                if c.phase == "claimed" && delivery_succeeded(&detail) {
                    if let Some(review) =
                        binding.as_ref().and_then(|b| b.review_state_id.as_deref())
                    {
                        let _ = client.set_issue_state(&c.issue_id, review).await;
                        let _ = claim_store.set_phase(&c.run_id, "in_review").await;
                        tracing::info!("linear poller: {} — PR opened → In Review", c.identifier);
                    }
                }
            }
        }
    }
}

/// True once any `delivery`-category node in the run has succeeded (the PR step).
fn delivery_succeeded(detail: &harness_persist::RunDetail) -> bool {
    let delivery: HashSet<&str> = detail
        .graph
        .iter()
        .filter(|m| m.category.as_deref() == Some("delivery"))
        .map(|m| m.id.as_str())
        .collect();
    detail
        .nodes
        .iter()
        .any(|n| n.status == "success" && delivery.contains(n.node_id.as_str()))
}
