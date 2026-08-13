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
//!      - `live`  → claim one **delegated** issue from the binding's source
//!        status (move to In Progress, fire the bound workflow, record the
//!        claim) — **one at a time per binding**,
//!      - dry-run → just log what it *would* do.
//!
//! Both triggers apply the same two gates, so they agree on what is startable:
//! the issue must be **delegated to the harness's app user** (what replaced the
//! old "AI Eligible" label) *and* sitting in the binding's **source status**.
//! [`super::linear_agent`] is the fast path — Linear pushes an
//! `AgentSessionEvent` the moment someone delegates, and the run starts in
//! seconds. This poller is the **reconciliation** path: if the harness was down
//! or a webhook delivery failed, the delegated issue is still picked up on a
//! later tick. Without a known app user id the gate cannot be evaluated, so the
//! poller claims **nothing** rather than everything.
//!
//! The Linear credential is global, resolved by [`linear_client_or_none`] — an
//! `actor=app` OAuth token once the workspace is connected, so the comments and
//! transitions below are authored by the app rather than by whoever's personal
//! API key was pasted. The `live` flag defaults to false, so a binding is dry-run
//! until explicitly switched on.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::time::Duration;

use harness_persist::{LinearClaim, LinearSource};
use harness_sources::linear::{AgentActivity, Comment, Issue, LinearClient};
use tokio::time::{interval, Instant, MissedTickBehavior};

use super::linear_oauth::{app_user_id, linear_client_or_none};
use super::runs_routes::{start_run, CreateRunRequest, RunsState};

/// Base loop cadence. A binding is only *claimed* once its own
/// `poll_interval_secs` has elapsed; status-sync runs every tick.
const POLLER_TICK_SECS: u64 = 30;

/// Runaway backstop for bindings with a failed-label configured: once an issue
/// has been (re-)claimed this many times, stop claiming it even if the label is
/// absent. The failed label is normally the pickup gate (and removing it re-arms
/// the issue for one more try), so this only fires if labeling ever fails to
/// apply — preventing an unlabeled issue from looping forever. Without a
/// failed-label, the binding's `max_attempts` is the cap instead.
const RUNAWAY_BACKSTOP: i64 = 10;

/// Spawn the Linear poller. Best-effort; a no-op outside a Tokio runtime.
pub(crate) fn spawn_poller(state: Arc<RunsState>) {
    if tokio::runtime::Handle::try_current().is_err() {
        return;
    }
    tokio::spawn(async move {
        let mut last: HashMap<(String, String), Instant> = HashMap::new();
        // Sweeping downloaded attachments is a directory walk, so it runs on its
        // own slow cadence rather than every tick.
        let mut last_sweep: Option<Instant> = None;
        let mut tick = interval(Duration::from_secs(POLLER_TICK_SECS));
        tick.set_missed_tick_behavior(MissedTickBehavior::Skip);
        loop {
            tick.tick().await;
            sweep_attachments_if_due(&state, &mut last_sweep);
            poll_once(&state, &mut last).await;
        }
    });
}

/// How often expired Linear attachment directories are swept.
const ATTACHMENT_SWEEP_INTERVAL_SECS: u64 = 3600;

/// Sweep expired attachment directories, at most once per
/// [`ATTACHMENT_SWEEP_INTERVAL_SECS`]. Runs on the first tick so a restart also
/// clears anything left behind by a crash.
fn sweep_attachments_if_due(state: &Arc<RunsState>, last_sweep: &mut Option<Instant>) {
    let due = last_sweep
        .map(|t| {
            Instant::now().duration_since(t) >= Duration::from_secs(ATTACHMENT_SWEEP_INTERVAL_SECS)
        })
        .unwrap_or(true);
    if !due {
        return;
    }
    *last_sweep = Some(Instant::now());
    let root = super::linear_attachments::attachments_root(&state.projects_dir);
    super::linear_attachments::sweep(
        &root,
        std::time::SystemTime::now(),
        super::linear_attachments::ttl(),
    );
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

        let Some(client) = linear_client_or_none(state).await else {
            tracing::debug!(
                "linear poller: {}/{} — Linear is not connected",
                b.project,
                b.workflow
            );
            continue;
        };
        // Delegation is the eligibility gate, so without knowing our own app user
        // id there is no gate — claim nothing rather than everything in the
        // column. Reconnecting the workspace records the id.
        let Some(delegate_id) = app_user_id(state).await else {
            tracing::warn!(
                "linear poller: {}/{} — the harness's Linear app user id is unknown, so \
                 delegated issues cannot be identified; skipping (reconnect the workspace \
                 on the Credentials page)",
                b.project,
                b.workflow
            );
            continue;
        };

        if b.live {
            claim_and_fire(state, &client, &b, &delegate_id).await;
        } else {
            dry_run_log(&client, &b, &delegate_id).await;
        }
    }
}

/// Log what a binding *would* claim, without mutating anything.
async fn dry_run_log(client: &LinearClient, b: &LinearSource, delegate_id: &str) {
    match client
        .preview_issues(&b.team_id, &b.source_state_id, delegate_id)
        .await
    {
        Ok(issues) => {
            // Mirror the live path: failed-labeled issues are gated off.
            let issues: Vec<Issue> = match b.failed_label.as_deref() {
                Some(fl) => issues
                    .into_iter()
                    .filter(|i| !i.labels.iter().any(|l| l == fl))
                    .collect(),
                None => issues,
            };
            if issues.is_empty() {
                return;
            }
            let ids: Vec<&str> = issues.iter().map(|i| i.identifier.as_str()).collect();
            let pick = ids.first().copied().unwrap_or("?");
            tracing::info!(
                "linear poller [dry-run]: {}/{} — {} delegated in the source status [{}]; would claim {} and fire `{}` (set live=true to act)",
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

/// Resolve a label *name* to its Linear id within `team_id` (case-insensitive),
/// via discovery. `None` if discovery fails or the label isn't on the team.
pub(crate) async fn resolve_label_id(
    client: &LinearClient,
    team_id: &str,
    name: &str,
) -> Option<String> {
    let discovery = client.discover().await.ok()?;
    discovery
        .teams
        .iter()
        .find(|t| t.id == team_id)
        .and_then(|t| t.labels.iter().find(|l| l.name.eq_ignore_ascii_case(name)))
        .map(|l| l.id.clone())
}

/// Re-arm the Linear issue behind `original_run_id` for a Rerun, linking it to
/// `new_run_id`. No-op if the original run wasn't Linear-triggered.
///
/// Unlike removing the failed label by hand (which grants one more try via the
/// poller), this is a **full reset**: it clears the prior claims (resetting the
/// attempt counter), removes the binding's failed-label, moves the issue to In
/// Progress, and records a fresh claim for the new run — so the manually-fired
/// rerun drives the issue's status just like a poller-claimed run, and the
/// poller won't double-claim it (it's out of the source column and has an active
/// claim). Best-effort: a Linear hiccup never fails the rerun itself.
pub(crate) async fn rearm_linear_claim(
    state: &Arc<RunsState>,
    original_run_id: &str,
    new_run_id: &str,
) {
    let Ok(claim_store) = state.linear_claim_store().await else {
        return;
    };
    let claim = match claim_store.claim_for_run(original_run_id).await {
        Ok(Some(c)) => c,
        _ => return, // not a Linear-triggered run — nothing to re-arm
    };
    let binding = match state.linear_source_store().await {
        Ok(s) => s.get(&claim.project, &claim.workflow).await.ok().flatten(),
        Err(_) => None,
    };
    if let Some(client) = linear_client_or_none(state).await {
        if let Some(b) = &binding {
            // Clear the failed-label so the issue no longer reads as failed.
            if let Some(name) = b.failed_label.as_deref() {
                if let Some(id) = resolve_label_id(&client, &b.team_id, name).await {
                    let _ = client.remove_label(&claim.issue_id, &id).await;
                }
            }
            // Move it to In Progress so the poller won't re-claim it from source.
            if let Some(ip) = b.in_progress_state_id.as_deref() {
                let _ = client.set_issue_state(&claim.issue_id, ip).await;
            }
        }
    }
    // Reset the attempt counter, then link the new run so status-sync drives it.
    let _ = claim_store
        .clear_claims(&claim.issue_id, &claim.workflow)
        .await;
    if let Err(e) = claim_store
        .record(
            new_run_id,
            &claim.project,
            &claim.workflow,
            &claim.issue_id,
            &claim.identifier,
            &claim.original_state_id,
            // Carry the delegating session across a rerun, so progress keeps
            // reporting into the same Linear thread.
            claim.agent_session_id.as_deref(),
        )
        .await
    {
        tracing::warn!("rerun: failed to re-link Linear claim for {new_run_id}: {e}");
    }
}

/// Claim one eligible issue for a live binding and fire its workflow — one at a
/// time per binding.
async fn claim_and_fire(
    state: &Arc<RunsState>,
    client: &LinearClient,
    b: &LinearSource,
    delegate_id: &str,
) {
    let claim_store = match state.linear_claim_store().await {
        Ok(s) => s,
        Err(_) => return,
    };
    // Concurrency gate: skip if this binding is already at its in-flight cap.
    // `max_concurrent_runs` defaults to 1 (the original one-at-a-time behaviour);
    // a binding can raise it to run several issues in parallel. The poller still
    // claims at most one issue per tick, so it ramps up to the cap over ticks.
    let cap = b.max_concurrent_runs.max(1) as i64;
    match claim_store.count_active(&b.project, &b.workflow).await {
        Ok(active) if active >= cap => return,
        Ok(_) => {}
        Err(e) => {
            tracing::warn!(
                "linear poller: count_active failed for {}/{}: {e}",
                b.project,
                b.workflow
            );
            return;
        }
    }

    let issues = match client
        .preview_issues(&b.team_id, &b.source_state_id, delegate_id)
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
    // Issues already carrying the binding's failed-label are gated off: they've
    // been given up on, and stay excluded until a human removes the label.
    let issues: Vec<Issue> = match b.failed_label.as_deref() {
        Some(fl) => issues
            .into_iter()
            .filter(|i| !i.labels.iter().any(|l| l == fl))
            .collect(),
        None => issues,
    };
    // Pick the first eligible issue that hasn't already exhausted its attempt
    // cap (per (issue, workflow), so an issue that legitimately flows through
    // several bindings across pipeline stages isn't exhausted by an earlier one).
    // With a failed-label configured, that label is the real pickup gate (above)
    // and removing it re-arms the issue, so here we only enforce a generous
    // runaway backstop; without it, the per-binding `max_attempts` is the cap.
    let cap = if b.failed_label.is_some() {
        RUNAWAY_BACKSTOP
    } else {
        b.max_attempts.max(1) as i64
    };
    let mut chosen = None;
    for issue in issues {
        match claim_store
            .failed_attempts_for_issue(&issue.id, &b.workflow)
            .await
        {
            Ok(n) if n >= cap => {
                tracing::debug!(
                    "linear poller: {}/{} — skipping {} (hit retry cap {})",
                    b.project,
                    b.workflow,
                    issue.identifier,
                    n
                );
            }
            Ok(_) => {
                chosen = Some(issue);
                break;
            }
            Err(e) => tracing::warn!("linear poller: attempts lookup failed: {e}"),
        }
    }
    let Some(issue) = chosen else {
        return; // nothing eligible (or all eligible issues exhausted retries)
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

    let comments = client.list_comments(&issue.id).await.unwrap_or_else(|e| {
        tracing::warn!(
            "linear poller: {}/{} — list_comments failed for {}: {} (proceeding without)",
            b.project,
            b.workflow,
            issue.identifier,
            e.0
        );
        Vec::new()
    });
    let req = CreateRunRequest {
        workflow: b.workflow.clone(),
        title: Some(format!("{} {}", issue.identifier, issue.title)),
        // Images pasted into the issue or its comments are downloaded and the
        // links rewritten to local paths, so the agent can actually see them.
        description: super::linear_attachments::localize(
            client,
            &super::linear_attachments::attachments_root(&state.projects_dir),
            &issue.identifier,
            &task_for_issue(&issue, &comments),
        )
        .await,
        args: String::new(),
        real: true,
        base_branch: b.base_branch.clone(),
        project: Some(b.project.clone()),
        swap_from: None,
        swap_to: None,
        ab_pair_id: None,
        ab_arm: None,
        ab_label: None,
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
                    // Poller-claimed: no delegating session.
                    None,
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
            if let Some(base) = &state.public_url {
                let run_url = format!("{base}/runs/{run_id}");
                if let Err(e) = client
                    .add_attachment(&issue.id, &run_url, "ai-harness run")
                    .await
                {
                    tracing::warn!(
                        "linear poller: {}/{} — failed to attach run link for {}: {}",
                        b.project,
                        b.workflow,
                        issue.identifier,
                        e.0
                    );
                }
            }
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
///
/// The identifier is stated up front *and* as an explicit PR-title directive so
/// the `verify-pr-title` node carries it into the final title — that's what lets
/// Linear link the PR back to this issue (and lets `merge-pr` find the PR via
/// `gh pr list --search`). We deliberately avoid close keywords ("Fixes"/
/// "Closes") so Linear's GitHub integration links without auto-closing the
/// issue, which would bypass the poller's own status transitions.
///
/// Injected Linear comments give reviewers a feedback channel for `revise-pr`
/// and a place to drop clarifications for `idea-to-pr`; the poller's own
/// status comments are filtered out so they don't drown human feedback.
fn task_for_issue(issue: &Issue, comments: &[Comment]) -> String {
    let id = &issue.identifier;
    let mut task = format!(
        "Linear issue {id} — {title}\n{url}\n\n\
         When you open the PR, make sure its title ends with \" ({id})\" so \
         Linear links the PR to this issue. Do not use close keywords like \
         \"Fixes\" or \"Closes\".\n\n\
         {body}",
        title = issue.title,
        url = issue.url,
        body = issue.body.as_deref().unwrap_or("").trim()
    );
    let human: Vec<&Comment> = comments
        .iter()
        .filter(|c| !is_bot_status(&c.body))
        .collect();
    if !human.is_empty() {
        task.push_str("\n\n## Reviewer comments (Linear)\n\n");
        for (idx, c) in human.iter().enumerate() {
            task.push_str(&format!(
                "**{}** ({}):\n{}\n",
                c.author, c.created_at, c.body
            ));
            if idx + 1 < human.len() {
                task.push_str("\n---\n\n");
            }
        }
    }
    task
}
/// True for the poller's own status comments. The heuristic requires both an
/// emoji prefix (🤖 / ✅ / ⚠️ / ❌) AND the string "ai-harness" in the body,
/// so a human comment beginning with ✅ (e.g. "✅ LGTM") is not filtered out.
fn is_bot_status(body: &str) -> bool {
    let t = body.trim_start();
    let has_prefix = ["🤖", "✅", "⚠️", "❌"].iter().any(|p| t.starts_with(p));
    has_prefix && body.contains("ai-harness")
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
        let Some(client) = linear_client_or_none(state).await else {
            continue;
        };
        let binding = match &source_store {
            Some(s) => s.get(&c.project, &c.workflow).await.ok().flatten(),
            None => None,
        };

        match detail.run.status.as_str() {
            "completed" => {
                if let Some(ready) = binding.as_ref().and_then(|b| b.ready_state_id.as_deref()) {
                    let _ = client.set_issue_state(&c.issue_id, ready).await;
                }
                // A `response` activity is what marks a delegated session
                // complete; a poller-claimed issue gets the comment instead.
                let msg = format!("✅ ai-harness run `{}` completed.", c.run_id);
                if !report_to_session(&client, &c, AgentActivity::Response { body: msg.clone() })
                    .await
                {
                    let _ = client.add_comment(&c.issue_id, &msg).await;
                }
                let _ = claim_store.set_phase(&c.run_id, "done").await;
                tracing::info!("linear poller: {} — run completed → ready", c.identifier);
            }
            "failed" | "cancelled" => {
                // The binding's attempt budget (default 1); fall back to 1 if the
                // binding has gone away. On a lookup error, assume exhausted so we
                // give up rather than risk a loop.
                let max_attempts = binding
                    .as_ref()
                    .map(|b| b.max_attempts.max(1) as i64)
                    .unwrap_or(1);
                // How many failed/cancelled attempts this issue has had for this
                // workflow (incl. this run, whose row already carries its
                // terminal status). Successful prior runs don't count.
                let attempts = claim_store
                    .failed_attempts_for_issue(&c.issue_id, &c.workflow)
                    .await
                    .unwrap_or(max_attempts);
                if attempts >= max_attempts {
                    // Attempt budget spent. If the binding has a failed-label,
                    // mark the issue failed and return it to the source column:
                    // the label suppresses pickup (so it can't loop), and removing
                    // it re-arms the issue for one more try. Without a failed-label,
                    // fall back to a comment and leave the issue where it is.
                    let failed_label = binding.as_ref().and_then(|b| b.failed_label.as_deref());
                    let team_id = binding.as_ref().map(|b| b.team_id.as_str());
                    let labelled = match (failed_label, team_id) {
                        (Some(name), Some(team)) => {
                            match resolve_label_id(&client, team, name).await {
                                Some(id) if client.add_label(&c.issue_id, &id).await.is_ok() => {
                                    // Return to source so removing the label re-arms it in place.
                                    let _ = client
                                        .set_issue_state(&c.issue_id, &c.original_state_id)
                                        .await;
                                    true
                                }
                                _ => {
                                    tracing::warn!(
                                        "linear poller: {} — could not apply failed-label `{}`; leaving unlabeled",
                                        c.identifier,
                                        name
                                    );
                                    false
                                }
                            }
                        }
                        _ => false,
                    };
                    let msg = if labelled {
                        format!(
                            "❌ ai-harness run `{}` failed ({}) after {} attempt(s) — labeled `{}` and returned to the source column. Remove the label (or hit Rerun) to re-arm.",
                            c.run_id,
                            detail.run.status,
                            attempts,
                            failed_label.unwrap_or("failed")
                        )
                    } else {
                        format!(
                            "❌ ai-harness run `{}` failed ({}) — gave up after {} attempt(s). Not retrying; needs a human. Move it back to the source column to re-arm.",
                            c.run_id, detail.run.status, attempts
                        )
                    };
                    if !report_to_session(&client, &c, AgentActivity::Error { body: msg.clone() })
                        .await
                    {
                        let _ = client.add_comment(&c.issue_id, &msg).await;
                    }
                    tracing::warn!(
                        "linear poller: {} — {} attempt(s) failed; giving up (labeled={})",
                        c.identifier,
                        attempts,
                        labelled
                    );
                } else {
                    // Roll back to the claimed-from state so it's retried.
                    let _ = client
                        .set_issue_state(&c.issue_id, &c.original_state_id)
                        .await;
                    let msg = format!(
                        "⚠️ ai-harness run `{}` did not complete ({}); returning for retry (attempt {}/{}).",
                        c.run_id, detail.run.status, attempts, max_attempts
                    );
                    // A retry is not terminal, so it stays a `thought` — an
                    // `error` would close the session before the next attempt.
                    if !report_to_session(&client, &c, AgentActivity::Thought { body: msg.clone() })
                        .await
                    {
                        let _ = client.add_comment(&c.issue_id, &msg).await;
                    }
                    tracing::info!(
                        "linear poller: {} — run {} → rolled back (attempt {}/{})",
                        c.identifier,
                        detail.run.status,
                        attempts,
                        max_attempts
                    );
                }
                let _ = claim_store.set_phase(&c.run_id, "done").await;
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
                        // Mid-run progress for a delegated session: an `action`,
                        // not a `response` — the run is still going.
                        report_to_session(
                            &client,
                            &c,
                            AgentActivity::Action {
                                action: "Opened pull request".into(),
                                parameter: c.workflow.clone(),
                                result: Some("moved to In Review".into()),
                            },
                        )
                        .await;
                    }
                }
            }
        }
    }
}

/// Report a run's progress into the agent session that delegated it.
///
/// Returns whether the claim *was* delegated, which is also the caller's signal
/// to skip the plain issue comment: for a delegated run the session thread is the
/// conversation, and posting both duplicates every update.
async fn report_to_session(
    client: &LinearClient,
    claim: &LinearClaim,
    activity: AgentActivity,
) -> bool {
    let Some(session) = claim.agent_session_id.as_deref() else {
        return false;
    };
    if let Err(e) = client.create_agent_activity(session, &activity).await {
        tracing::warn!(
            "linear poller: {} — failed to report into session {session}: {}",
            claim.identifier,
            e.0
        );
    }
    true
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
#[cfg(test)]
mod tests {
    use super::task_for_issue;
    use harness_sources::linear::{Comment, Issue};
    fn sample_issue() -> Issue {
        Issue {
            id: "issue-123".into(),
            identifier: "COR-12".into(),
            title: "Add dark mode".into(),
            url: "https://linear.app/issue/COR-12".into(),
            body: Some("Support a system-level dark theme.".into()),
            labels: vec![],
        }
    }
    fn sample_comment(body: &str, author: &str) -> Comment {
        Comment {
            body: body.into(),
            author: author.into(),
            created_at: "2026-06-01T10:00:00Z".into(),
        }
    }
    #[test]
    fn task_for_issue_without_comments_omits_section() {
        let task = task_for_issue(&sample_issue(), &[]);
        assert!(!task.contains("Reviewer comments (Linear)"));
        assert!(task.contains("COR-12"));
        assert!(task.contains("title ends with \" (COR-12)\""));
    }
    #[test]
    fn task_for_issue_appends_reviewer_comments() {
        let comments = vec![sample_comment("Please add a toggle.", "Alice")];
        let task = task_for_issue(&sample_issue(), &comments);
        assert!(task.contains("## Reviewer comments (Linear)"));
        assert!(task.contains("**Alice** (2026-06-01T10:00:00Z):"));
        assert!(task.contains("Please add a toggle."));
    }
    #[test]
    fn task_for_issue_filters_bot_status_comments() {
        let comments = vec![
            sample_comment("🤖 ai-harness started `revise-pr` (run `r1`).", "bot"),
            sample_comment("Please add a toggle.", "Alice"),
        ];
        let task = task_for_issue(&sample_issue(), &comments);
        assert!(task.contains("## Reviewer comments (Linear)"));
        assert!(!task.contains("🤖 ai-harness started"));
        assert!(task.contains("Please add a toggle."));
    }
    #[test]
    fn task_for_issue_omits_section_when_all_comments_are_bot_status() {
        let comments = vec![sample_comment(
            "✅ ai-harness completed `idea-to-pr` (run `r2`).",
            "bot",
        )];
        let task = task_for_issue(&sample_issue(), &comments);
        assert!(!task.contains("Reviewer comments (Linear)"));
    }
    #[test]
    fn task_for_issue_does_not_filter_human_emoji_comments() {
        // A human reviewer starting with ✅ should NOT be filtered (no "ai-harness").
        let comments = vec![sample_comment("✅ LGTM, just one nit.", "Alice")];
        let task = task_for_issue(&sample_issue(), &comments);
        assert!(task.contains("## Reviewer comments (Linear)"));
        assert!(task.contains("✅ LGTM, just one nit."));
    }
}
