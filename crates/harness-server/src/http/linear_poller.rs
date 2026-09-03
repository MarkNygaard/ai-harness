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
//! The Linear credential is per connection, resolved from each binding's project
//! by [`resolve_for_project`] and then by [`linear_client_or_none`] — an
//! `actor=app` OAuth token once the workspace is connected, so the comments and
//! transitions below are authored by the app rather than by whoever's personal
//! API key was pasted. The `live` flag defaults to false, so a binding is dry-run
//! until explicitly switched on.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::time::Duration;

use chrono::Utc;
use harness_persist::{LinearClaim, LinearSource};
use harness_sources::linear::{AgentActivity, Comment, Issue, LinearClient};
use tokio::time::{interval, Instant, MissedTickBehavior};

use super::linear_connections::resolve_for_project;
use super::linear_oauth::{app_user_id, linear_client_or_none};
use super::runs_routes::{start_run, CreateRunRequest, RunsState};

/// Base loop cadence. A binding is only *claimed* once its own
/// `poll_interval_secs` has elapsed; status-sync runs every tick.
const POLLER_TICK_SECS: u64 = 30;

/// Hard ceiling on one tick's work.
///
/// The loop awaits [`poll_once`] inline, so anything that never returns stops the
/// poller for the lifetime of the process: no claim swept, no progress reported,
/// no status transitioned, and — because the loop simply stops reaching its next
/// statement — nothing logged to say so. That happened: an untimed Linear request
/// stalled and a delegated run went 50 minutes without a single session activity
/// while the run itself completed normally. The per-request timeout is the actual
/// fix; this is the backstop for the next await that forgets one.
const TICK_BUDGET_SECS: u64 = 20 * 60;
// The budget has to leave room for a tick's real work — several Linear round trips
// per active claim — while still being far shorter than a person's patience.
const _: () = assert!(TICK_BUDGET_SECS > POLLER_TICK_SECS * 4 && TICK_BUDGET_SECS <= 60 * 60);

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
            // Bounded, and unwind-guarded: a tick that hangs or panics costs one
            // tick, not the poller. Without this the loop is a single point of
            // failure for every Linear status transition the harness makes.
            let work = std::panic::AssertUnwindSafe(poll_once(&state, &mut last));
            match tokio::time::timeout(
                Duration::from_secs(TICK_BUDGET_SECS),
                futures::FutureExt::catch_unwind(work),
            )
            .await
            {
                Ok(Ok(())) => {}
                Ok(Err(_)) => tracing::error!(
                    "linear poller: tick panicked; continuing with the next tick"
                ),
                Err(_) => tracing::error!(
                    "linear poller: tick exceeded {TICK_BUDGET_SECS}s and was abandoned;                      continuing with the next tick"
                ),
            }
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

        // Which Linear account this binding's project belongs to. A
        // single-account install resolves every project to the one connection.
        let conn = match resolve_for_project(state, &b.project).await {
            Ok(c) => c,
            Err(e) => {
                tracing::warn!(
                    "linear poller: {}/{} — {e}; skipping",
                    b.project,
                    b.workflow
                );
                continue;
            }
        };
        let Some(client) = linear_client_or_none(state, &conn).await else {
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
        let Some(delegate_id) = app_user_id(state, &conn).await else {
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
    // Skip the Linear side rather than guessing an account: re-arming against the
    // wrong workspace would move somebody else's issue.
    let client = match resolve_for_project(state, &claim.project).await {
        Ok(conn) => linear_client_or_none(state, &conn).await,
        Err(e) => {
            tracing::warn!(
                "linear poller: {} — {e}; not re-arming the issue in Linear",
                claim.identifier
            );
            None
        }
    };
    if let Some(client) = client {
        if let Some(b) = &binding {
            // Clear the failed-label so the issue no longer reads as failed.
            if let Some(name) = b.failed_label.as_deref() {
                if let Some(id) = resolve_label_id(&client, &b.team_id, name).await {
                    let _ = client.remove_label(&claim.issue_id, &id).await;
                }
            }
            // Move it to In Progress so the poller won't re-claim it from source.
            if let Some(ip) = b.in_progress_state_id.as_deref() {
                transition(
                    &client,
                    &claim.identifier,
                    &claim.issue_id,
                    ip,
                    "in-progress",
                )
                .await;
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
    match claim_store.count_active(&b.project, &b.workflow).await {
        Ok(active) if super::linear_agent::at_capacity(active, b.max_concurrent_runs) => return,
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
                // Delegation reads the same column this tick did, so an issue
                // sitting here may already be on its way to a run. Take the
                // guard as part of choosing: held for the rest of this claim, it
                // is what makes the active-claim check below stay true.
                let Some(guard) = super::linear_agent::IssueGuard::acquire(&b.workflow, &issue.id)
                else {
                    tracing::info!(
                        "linear poller: {}/{} — {} is already being started elsewhere; skipping",
                        b.project,
                        b.workflow,
                        issue.identifier
                    );
                    continue;
                };
                if super::linear_agent::issue_already_claimed(state, &b.workflow, &issue.id).await {
                    tracing::info!(
                        "linear poller: {}/{} — {} already has a run in flight; skipping",
                        b.project,
                        b.workflow,
                        issue.identifier
                    );
                    continue;
                }
                chosen = Some((issue, guard));
                break;
            }
            Err(e) => tracing::warn!("linear poller: attempts lookup failed: {e}"),
        }
    }
    // `_issue_guard` lives as long as this claim: dropping it at the end of the
    // function is the point, so delegation cannot slip in behind us.
    let Some((issue, _issue_guard)) = chosen else {
        return; // nothing eligible (retries exhausted, or already being claimed)
    };

    let route = super::linear_agent::route_issue(state, client, b, &issue.id).await;

    // Move to In Progress first — this is also the claim signal (it leaves the
    // source column, so the next poll won't re-pick it).
    //
    // An epic needs this as much as a piece does, and for the same reason: it
    // stays delegated and stays in the trigger column otherwise, and the poller
    // picks it up again every tick. What an epic must *not* inherit is the rest
    // of the binding's lifecycle — ready, done — which is handled at completion.
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
    // An epic's pieces build on the epic's branch, not on the binding's base:
    // the feature accumulates in one place and `main` is left alone.
    let (workflow, base_branch) = match &route {
        super::linear_agent::Route::Supervise => (
            super::linear_agent::EPIC_SUPERVISOR.to_string(),
            b.base_branch.clone(),
        ),
        super::linear_agent::Route::BuildOnEpic(branch) => {
            (b.workflow.clone(), Some(branch.clone()))
        }
        super::linear_agent::Route::Build => (b.workflow.clone(), b.base_branch.clone()),
    };
    let req = CreateRunRequest {
        triggered_by: Some("linear".to_string()),
        workflow,
        title: Some(format!("{} {}", issue.identifier, issue.title)),
        issue_id: Some(issue.id.clone()),
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
        base_branch,
        project: Some(b.project.clone()),
        swap_from: None,
        swap_to: None,
        ab_pair_id: None,
        ab_arm: None,
        ab_label: None,
    };
    // Open a session of our own so this run reports progress into a thread like a
    // delegated one, instead of leaving detached comments. Delegation supplies a
    // session; a claim the poller made has none — and that is the retry-after-
    // failure path, which is exactly when the visibility is wanted.
    //
    // The guard is held until the claim carries the session id: if Linear echoes a
    // `created` webhook for a session we opened ourselves, the delegation handler
    // must bail before acknowledging it. After that the claim's session id makes
    // `already_handled` reject the echo on its own.
    let session = client
        .create_agent_session(&issue.id)
        .await
        .map_err(|e| {
            // Non-fatal: fall back to the plain comment rather than lose the run.
            tracing::warn!(
                "linear poller: {} — could not open an agent session, falling back to \
                 comments: {}",
                issue.identifier,
                e.0
            );
        })
        .ok();
    let _session_guard = session
        .as_deref()
        .and_then(super::linear_agent::SessionGuard::acquire);

    match start_run(state, req).await {
        Ok(run_id) => {
            match claim_store
                .record(
                    &run_id,
                    &b.project,
                    &b.workflow,
                    &issue.id,
                    &issue.identifier,
                    &b.source_state_id,
                    session.as_deref(),
                )
                .await
            {
                // Refused by the one-active-claim index despite the guard and
                // the check above, so the duplicate came from another process.
                // Loud: the run is live, and a person has to pick a PR.
                Ok(false) => tracing::error!(
                    "linear poller: claim for {run_id} refused — {} already has an active \
                     claim for {}. Two runs are now in flight for one issue.",
                    issue.identifier,
                    b.workflow
                ),
                Ok(true) => {}
                Err(e) => {
                    tracing::warn!("linear poller: failed to record claim for {}: {e}", run_id)
                }
            }
            let run_url = state
                .public_url()
                .map(|base| format!("{base}/runs/{run_id}"));
            match session.as_deref() {
                // With a session, progress belongs in the thread — a comment as
                // well would just duplicate it.
                Some(s) => {
                    let _ = client
                        .create_agent_activity(
                            s,
                            // `action` + `parameter` render concatenated, so the
                            // workflow goes in `parameter` — putting it in both
                            // read as "Started workflow revise-pr ECOM-16". The
                            // session is already on the issue, so naming it again
                            // added nothing. Matches the delegation path's wording.
                            &AgentActivity::Action {
                                action: "Started workflow".into(),
                                parameter: b.workflow.clone(),
                                result: run_url.clone(),
                            },
                        )
                        .await;
                }
                None => {
                    let _ = client
                        .add_comment(
                            &issue.id,
                            &format!("🤖 ai-harness started `{}` (run `{}`).", b.workflow, run_id),
                        )
                        .await;
                }
            }
            if let Some(base) = &state.public_url() {
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
            // `client` is already a `&LinearClient` here (unlike the owned binding
            // inside the other call sites), so it is passed through as-is.
            transition(
                client,
                &issue.identifier,
                &issue.id,
                &b.source_state_id,
                "source",
            )
            .await;
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
        .filter(|c| !is_bot_status(&c.body) && !is_agent_session_preamble(&c.body))
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

/// True for the comment **Linear itself** posts to open an agent-session thread
/// ("This thread is for an agent session with <app>."). It carries no reviewer
/// intent, so feeding it to the agent as feedback is pure noise — and unlike our
/// own status comments it has no emoji prefix, so [`is_bot_status`] misses it.
///
/// Matched on Linear's wording rather than the app name, which varies per install.
fn is_agent_session_preamble(body: &str) -> bool {
    body.contains("is for an agent session with")
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
                // Not necessarily gone — possibly not written yet. `start_run`
                // returns before its spawned task records the run, so a fresh claim
                // can point at a row that is still seconds away. Only give up once
                // the row has had time to appear; closing on the first miss retires
                // the claim for good and the run reports nothing for its entire life.
                if Utc::now().signed_duration_since(c.created_at).num_minutes()
                    >= MISSING_RUN_GRACE_MINS
                {
                    tracing::warn!(
                        "linear poller: {} — no run row for {} after {}m; dropping the claim",
                        c.identifier,
                        c.run_id,
                        MISSING_RUN_GRACE_MINS
                    );
                    let _ = claim_store.set_phase(&c.run_id, "done").await;
                }
                continue;
            }
            Err(_) => continue,
        };
        let conn = match resolve_for_project(state, &c.project).await {
            Ok(conn) => conn,
            Err(e) => {
                tracing::warn!("linear poller: {} — {e}; not syncing status", c.identifier);
                continue;
            }
        };
        let Some(client) = linear_client_or_none(state, &conn).await else {
            continue;
        };
        let binding = match &source_store {
            Some(s) => s.get(&c.project, &c.workflow).await.ok().flatten(),
            None => None,
        };

        match detail.run.status.as_str() {
            "completed" => {
                // A supervise run does not move the issue it ran for: an epic's
                // column is the supervisor's own business, and a piece it
                // reviewed is already where it belongs.
                let supervising = detail.run.workflow_name == super::linear_agent::EPIC_SUPERVISOR;
                // A piece of an epic and a standalone issue are finished in
                // different senses, and a binding can now say so. A standalone
                // issue is heading for the default branch, so it stops at
                // whatever gate the team keeps -- functional testing, review. A
                // piece is heading for the epic's own branch, where the
                // supervisor grades it and the whole feature is reviewed once,
                // as a single pull request; stopping every piece at that gate
                // reviews the same feature N times and puts a human back in the
                // loop the epic exists to remove.
                //
                // Unset (the default) means the two are the same, so a binding
                // that never mentions it behaves exactly as before.
                let piece_ready = match binding.as_ref() {
                    Some(b) if b.piece_ready_state_id.is_some() && !supervising => {
                        is_epic_piece(state, &client, b, &c.issue_id).await
                    }
                    _ => false,
                };
                let ready = binding
                    .as_ref()
                    .and_then(|b| ready_state_for(b, piece_ready))
                    .filter(|_| !supervising);
                let moved = match ready {
                    Some(ready) => {
                        transition(&client, &c.identifier, &c.issue_id, ready, "ready").await
                    }
                    // No ready state configured: nothing to move to, so the claim
                    // is done as soon as the run is.
                    None => true,
                };
                // A `response` activity is what marks a delegated session
                // complete; a poller-claimed issue gets the comment instead.
                let msg = format!("✅ ai-harness run `{}` completed.", c.run_id);
                if !report_to_session(&client, &c, AgentActivity::Response { body: msg.clone() })
                    .await
                {
                    let _ = client.add_comment(&c.issue_id, &msg).await;
                }
                // Closing the claim is what stops the retry, so it is gated on the
                // move having landed. Left open, the next tick tries again — the
                // alternative is an issue stranded in the wrong column with the
                // claim marked done and nothing left to notice.
                let stale = Utc::now().signed_duration_since(c.created_at).num_hours()
                    >= TRANSITION_RETRY_HOURS;
                if moved {
                    let _ = claim_store.set_phase(&c.run_id, "done").await;
                    tracing::info!("linear poller: {} — run completed → ready", c.identifier);
                } else if stale {
                    let note = format!(
                        "⚠️ ai-harness run `{}` completed, but this issue could not be moved                          to its ready state after {}h of retries — move it by hand and check                          the binding's ready state.",
                        c.run_id, TRANSITION_RETRY_HOURS
                    );
                    let _ = client.add_comment(&c.issue_id, &note).await;
                    let _ = claim_store.set_phase(&c.run_id, "done").await;
                    tracing::error!(
                        "linear poller: {} — giving up on the ready-state move after {}h",
                        c.identifier,
                        TRANSITION_RETRY_HOURS
                    );
                } else {
                    tracing::warn!(
                        "linear poller: {} — run completed but the ready-state move failed;                          retrying on the next tick",
                        c.identifier
                    );
                }
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
                // Still running: stream node progress into the session and keep it
                // from going stale. Without this a delegated session shows
                // "stopped responding" for most of a long run while the harness
                // works fine.
                report_progress(&client, claim_store, &c, &detail).await;

                // Move to Review once the PR exists (a delivery node has
                // succeeded), and only once.
                if c.phase == "claimed" && delivery_succeeded(&detail) {
                    if let Some(review) =
                        binding.as_ref().and_then(|b| b.review_state_id.as_deref())
                    {
                        transition(&client, &c.identifier, &c.issue_id, review, "review").await;
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

/// Which status a completed run's issue moves to.
///
/// A binding that never sets a piece state behaves exactly as it always has --
/// that is what keeps this change invisible to every existing install -- and a
/// piece falls back to the ordinary ready state rather than going nowhere.
fn ready_state_for(binding: &LinearSource, is_piece: bool) -> Option<&str> {
    if is_piece {
        if let Some(piece) = binding.piece_ready_state_id.as_deref() {
            return Some(piece);
        }
    }
    binding.ready_state_id.as_deref()
}

/// Whether this issue was built as a piece of an epic.
///
/// Asks the same question the claim asked, rather than guessing from the branch
/// name or from the issue merely having a parent: `route_issue` also requires
/// the supervisor to be enabled for the team, so a sub-issue in a project with
/// no epic workflow is an ordinary issue and must go to the ordinary gate.
///
/// Only called when a binding actually sets a piece state, so a project not
/// using epics pays nothing.
async fn is_epic_piece(
    state: &Arc<RunsState>,
    client: &LinearClient,
    binding: &LinearSource,
    issue_id: &str,
) -> bool {
    matches!(
        super::linear_agent::route_issue(state, client, binding, issue_id).await,
        super::linear_agent::Route::BuildOnEpic(_)
    )
}

/// Report a run's progress into the agent session that delegated it.
///
/// Returns whether the claim *was* delegated, which is also the caller's signal
/// to skip the plain issue comment: for a delegated run the session thread is the
/// conversation, and posting both duplicates every update.
/// How long a claim may reference a run row that does not exist yet.
///
/// The run row is written by the spawned run task, not by `start_run` — which
/// returns as soon as it has an id — so a claim recorded the instant `start_run`
/// returns legitimately precedes its own run by seconds (6.3s on the run that
/// exposed this). Treating that gap as "run gone" and closing the claim ends all
/// Linear reporting for the whole run, permanently, from a single unlucky tick.
/// Long enough to cover worktree setup and a slow first write; short enough that a
/// genuinely deleted run stops being swept the same day.
const MISSING_RUN_GRACE_MINS: i64 = 10;

/// How long to keep retrying a Linear state transition that keeps failing.
///
/// A rejected move is usually transient (a hiccup, a timeout), so the claim stays
/// open and the next tick tries again. A permanently invalid state id would
/// otherwise retry forever, so past this age the poller says so on the issue and
/// closes the claim rather than looping in the background.
const TRANSITION_RETRY_HOURS: i64 = 6;

/// Move an issue to `state_id`, reporting a failure instead of discarding it.
///
/// Every transition used to be `let _ = client.set_issue_state(…)`, so a rejected
/// move left no trace whatsoever — and in the completed branch the `info!` line
/// underneath announced the move as done regardless. An issue could sit in the
/// wrong column indefinitely while the logs claimed otherwise, which is exactly
/// what happened to a delegated run that finished but never reached its ready
/// state.
async fn transition(
    client: &LinearClient,
    identifier: &str,
    issue_id: &str,
    state_id: &str,
    what: &str,
) -> bool {
    match client.set_issue_state(issue_id, state_id).await {
        Ok(()) => true,
        Err(e) => {
            tracing::warn!(
                "linear poller: {identifier} — could not move to {what} ({state_id}): {}",
                e.0
            );
            false
        }
    }
}

/// Post one activity into the claim's agent session.
///
/// Returns whether it was **delivered** — callers use that to fall back to a plain
/// issue comment. It used to return `true` whenever a session id existed, even
/// when the post had just failed, so a rejected activity produced neither a
/// session entry nor the comment that was supposed to replace it: the thread simply
/// went quiet, and only a `warn` line said otherwise.
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
        return false;
    }
    true
}

/// How long Linear tolerates a session with no activity before marking it `stale`
/// (and showing "stopped responding" on the issue).
const LINEAR_STALE_MINS: i64 = 30;

/// Minutes of silence before the session gets a keep-alive `thought`.
///
/// This exists only to stay inside [`LINEAR_STALE_MINS`] — it is not a progress
/// channel.
/// Starts and finishes are both reported now, so the only silence left is a single
/// step in flight, and anything shorter than this never needs a heartbeat at all.
///
/// It was 10, sized when finishes were the only signal and a long step therefore
/// meant genuine silence. That fired for every step over ten minutes — four times
/// in one 16-step run, none of them needed, since the longest step there ran 18
/// minutes. Each one also cost thread structure: Linear bundles consecutive
/// `action` activities under one "Used N tools" accordion, and a `thought` closes
/// the bundle, so every heartbeat split the step list into another group.
const SESSION_HEARTBEAT_MINS: i64 = 20;

// Bounded from both sides, because either direction is a real failure.
//
// Too late and the session goes stale: it must stay far enough under the window that
// a missed tick, a slow Linear call or a brief restart cannot cross it. Stated as the
// slack itself rather than as a ratio, because the ratio is what went stale in the
// reasoning last time.
//
// Too early and it fires for ordinary long steps, fragmenting the thread for nothing
// — 18m 7s was the longest step in the run that prompted this, and it must stay
// silent for one.
const _: () =
    assert!(SESSION_HEARTBEAT_MINS > 18 && SESSION_HEARTBEAT_MINS + 10 <= LINEAR_STALE_MINS);

/// Nodes that have finished but not yet been reported into the session, in graph
/// order, as `(node_id, status)`.
fn unreported_finished_nodes(
    claim: &LinearClaim,
    detail: &harness_persist::RunDetail,
) -> Vec<(String, String)> {
    detail
        .nodes
        .iter()
        .filter(|n| matches!(n.status.as_str(), "success" | "failed" | "cancelled"))
        .filter(|n| !claim.has_reported(&n.node_id))
        .map(|n| (n.node_id.clone(), n.status.clone()))
        .collect()
}

/// The reported-set key for "we've said this node started".
///
/// Sharing one column with the finished-node keys avoids a migration; the prefix
/// keeps the two apart. A node id containing this prefix would collide, which no
/// bundled workflow does and a `:` in an id would be unusual.
fn running_key(node_id: &str) -> String {
    format!("running:{node_id}")
}

/// Nodes in flight that haven't been announced yet.
///
/// A layer can run several nodes at once, so this returns all of them rather than
/// the first — announcing one of a parallel pair would misreport the run.
fn unannounced_running_nodes(
    claim: &LinearClaim,
    detail: &harness_persist::RunDetail,
) -> Vec<String> {
    detail
        .nodes
        .iter()
        .filter(|n| n.status == "running")
        .map(|n| n.node_id.clone())
        .filter(|id| !claim.has_reported(&running_key(id)))
        .collect()
}

/// Where a node sits in the workflow as authored: `(position, total)`, 1-based.
///
/// Read from the run's stored graph — one entry per *declared* node, in
/// declaration order — rather than from the node's execution `ordinal`. Someone
/// asking "which step is it on" means the workflow's own numbering, the one the
/// DAG view shows; the two diverge as soon as a layer runs in parallel or a
/// `when:` skips something, because `ordinal` is the order nodes actually ran in.
///
/// The total counts declared nodes, so it includes any a `when:` will skip. That
/// is the honest denominator: how many steps the workflow *has* is knowable, how
/// many will run is not.
///
/// `None` when the graph is empty (runs recorded before graphs were stored) or the
/// node isn't in it — the counter is then dropped rather than invented.
fn step_position(node_id: &str, detail: &harness_persist::RunDetail) -> Option<(usize, usize)> {
    let total = detail.graph.len();
    if total == 0 {
        return None;
    }
    let at = detail.graph.iter().position(|m| m.id == node_id)?;
    Some((at + 1, total))
}

/// Timezone for the absolute times shown in Linear.
///
/// Danish time, because that is the wall clock the people reading these threads
/// are looking at. `Europe/Copenhagen` rather than a fixed +01:00 "CET" offset so
/// the CET/CEST switch is handled — a hardcoded offset would be an hour wrong from
/// late March to late October. `HARNESS_DISPLAY_TZ` takes any IANA name for a
/// deployment in another country.
///
/// Deliberately not `chrono::Local`: the image sets no `TZ`, so the host decides,
/// and a container coming up without it would silently render UTC.
fn display_tz() -> chrono_tz::Tz {
    std::env::var("HARNESS_DISPLAY_TZ")
        .ok()
        .and_then(|name| name.parse().ok())
        .unwrap_or(chrono_tz::Europe::Copenhagen)
}

/// A UTC instant as a wall-clock time in `tz`, with the zone named — "13:14 CEST".
///
/// The abbreviation comes from the zone rather than being hardcoded, so it reads
/// `CET` in winter without anyone remembering to change it. Minutes, not seconds:
/// this is for orienting yourself in a thread, and the duration on the matching
/// "finished" line is where precision lives.
///
/// Takes `tz` explicitly so it is testable without touching process environment.
fn format_local(at: chrono::DateTime<Utc>, tz: chrono_tz::Tz) -> String {
    at.with_timezone(&tz).format("%H:%M %Z").to_string()
}

/// When a node started, on the reader's wall clock. `None` if no start is recorded.
fn step_started_at(node_id: &str, detail: &harness_persist::RunDetail) -> Option<String> {
    let node = detail.nodes.iter().find(|n| n.node_id == node_id)?;
    Some(format_local(node.started_at?, display_tz()))
}

/// Wall time a node took, formatted for a human — `None` unless both timestamps
/// are recorded and the interval is sane.
///
/// This is elapsed wall time, which for a loop node covers every iteration. It is
/// not agent time: a node that queues behind the concurrency cap or waits on a
/// dependency has that waiting counted in.
fn step_duration(node_id: &str, detail: &harness_persist::RunDetail) -> Option<String> {
    let node = detail.nodes.iter().find(|n| n.node_id == node_id)?;
    let secs = node
        .ended_at?
        .signed_duration_since(node.started_at?)
        .num_seconds();
    // A skipped node has neither timestamp, and clock skew between a restart and
    // the row's write can invert the pair. Report nothing rather than "-3s".
    (secs >= 0).then(|| human_duration(secs))
}

/// `12s` / `5m` / `1m 46s` / `2h` / `1h 4m`.
///
/// Coarsens as it grows: seconds stop mattering once a step has run for an hour,
/// and a step that took exactly five minutes should not read "5m 0s".
fn human_duration(secs: i64) -> String {
    let (h, m, s) = (secs / 3600, (secs % 3600) / 60, secs % 60);
    match (h, m, s) {
        (0, 0, s) => format!("{s}s"),
        (0, m, 0) => format!("{m}m"),
        (0, m, s) => format!("{m}m {s}s"),
        (h, 0, _) => format!("{h}h"),
        (h, m, _) => format!("{h}h {m}m"),
    }
}

/// The parenthetical after a step's name: where it sits, and one timing phrase —
/// `started 13:14 CEST` while running, `took 1m 46s` once finished. The phrase
/// arrives complete so this stays the single place the brackets are assembled.
///
/// Either half can be absent, so this yields the four sensible shapes rather than
/// ever emitting an empty `()`.
fn step_note(position: Option<(usize, usize)>, timing: Option<&str>) -> String {
    match (position, timing) {
        (Some((n, total)), Some(t)) => format!(" ({n} of {total}, {t})"),
        (Some((n, total)), None) => format!(" ({n} of {total})"),
        (None, Some(t)) => format!(" ({t})"),
        (None, None) => String::new(),
    }
}

/// How a step reads in the session thread.
///
/// Linear concatenates an action activity's `action` and `parameter` in the UI, and
/// documents `parameter` as "the parameters for the action" — so the workflow name
/// goes there and the sentence is built to end with it. The previous wording put
/// the bare node id in `action` and the workflow in `parameter`, which rendered as
/// "Finished explore idea-to-pr": three names in a row with nothing saying which
/// was a step and which a workflow.
///
/// A failed or cancelled node is not "finished" — saying so was actively wrong.
fn step_action(
    node_id: &str,
    status: &str,
    position: Option<(usize, usize)>,
    timing: Option<&str>,
) -> String {
    let at = step_note(position, timing);
    match status {
        "running" => format!("Running the {node_id} step{at} of workflow"),
        "failed" => format!("The {node_id} step{at} failed in workflow"),
        "cancelled" => format!("The {node_id} step{at} was cancelled in workflow"),
        _ => format!("Finished the {node_id} step{at} of workflow"),
    }
}

/// Report progress into a delegated run's session and keep it alive.
///
/// Two jobs, because neither covers the other:
///
/// * **Per-node activities.** A multi-node workflow finishes something every few
///   minutes, which is both genuinely informative and enough to keep the session
///   warm.
/// * **A heartbeat.** A single node can run far longer than Linear's 30-minute
///   window — `implement-tasks` routinely takes 15 minutes and the review loops
///   can take much more — so per-node reporting alone would let the session go
///   stale *mid-node*, which is exactly when a human is most likely to look.
///
/// Only ever posts for claims that carry a session (delegated runs); a
/// poller-claimed run has nowhere to post and is left alone.
async fn report_progress(
    client: &LinearClient,
    claim_store: &harness_persist::LinearClaimStore,
    claim: &LinearClaim,
    detail: &harness_persist::RunDetail,
) {
    if claim.agent_session_id.is_none() {
        return;
    }

    // Everything announced this tick, in the order it is posted: finishes first,
    // then whatever that unblocked. The reported-set is keyed so a start and a
    // finish for the same node are distinct entries.
    let mut announced: Vec<String> = Vec::new();

    for (node_id, status) in unreported_finished_nodes(claim, detail) {
        report_to_session(
            client,
            claim,
            AgentActivity::Action {
                action: step_action(
                    &node_id,
                    &status,
                    step_position(&node_id, detail),
                    // How long it took. The matching "Running" line already gave
                    // the clock time it began, so repeating that here would be
                    // noise — between the two you can read both.
                    step_duration(&node_id, detail)
                        .map(|d| format!("took {d}"))
                        .as_deref(),
                ),
                parameter: claim.workflow.clone(),
                // The action text now names the outcome, so repeating the raw
                // status behind a disclosure arrow adds nothing.
                result: None,
            },
        )
        .await;
        announced.push(node_id);
    }

    // Say what has *started*, not only what ended. A step can run for many
    // minutes — `create-plan` and `implement-tasks` routinely do — and a thread
    // whose last line is "Finished explore" reads as stalled for all of it. This
    // is also what lets the heartbeat below be rare: with a start line carrying
    // the clock time it began, "still going" is readable without one.
    for node_id in unannounced_running_nodes(claim, detail) {
        report_to_session(
            client,
            claim,
            AgentActivity::Action {
                action: step_action(
                    &node_id,
                    "running",
                    step_position(&node_id, detail),
                    // The clock time it began. No elapsed time here — it just
                    // started; the heartbeat covers a long-running one, and the
                    // matching "Finished" line reports the total.
                    step_started_at(&node_id, detail)
                        .map(|t| format!("started {t}"))
                        .as_deref(),
                ),
                parameter: claim.workflow.clone(),
                result: None,
            },
        )
        .await;
        announced.push(running_key(&node_id));
    }

    if !announced.is_empty() {
        let _ = claim_store
            .set_session_progress(&claim.run_id, &claim.with_reported(&announced))
            .await;
        return;
    }

    // Nothing changed this tick. Keep the session alive if we've gone quiet,
    // naming the node in flight and how long it has been going — that is the most
    // the claim can honestly say, since it sees node status rather than the
    // agent's internal progress.
    let quiet_for = Utc::now().signed_duration_since(claim.last_activity_at);
    if quiet_for.num_minutes() < SESSION_HEARTBEAT_MINS {
        return;
    }
    let running = detail
        .nodes
        .iter()
        .find(|n| n.status == "running")
        .map(|n| n.node_id.clone());
    let body = match &running {
        Some(node) => format!(
            "Still working — the `{node}` step{} has been running for {} minutes.",
            match step_position(node, detail) {
                Some((n, total)) => format!(" ({n} of {total})"),
                None => String::new(),
            },
            quiet_for.num_minutes()
        ),
        // No node in flight but the run is live: between nodes, or starting one.
        None => format!(
            "Still working on `{}` — {} minutes since the last update.",
            claim.workflow,
            quiet_for.num_minutes()
        ),
    };
    if report_to_session(client, claim, AgentActivity::Thought { body }).await {
        // Reset the clock even though no node was reported.
        let _ = claim_store
            .set_session_progress(&claim.run_id, &claim.reported_nodes)
            .await;
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
#[cfg(test)]
mod tests {
    use super::{
        format_local, human_duration, is_agent_session_preamble, ready_state_for, step_action,
        step_duration, step_position, task_for_issue, unannounced_running_nodes,
        unreported_finished_nodes, LinearSource,
    };
    use chrono::Utc;
    use harness_sources::linear::{Comment, Issue};

    fn node(node_id: &str, ordinal: i32, status: &str) -> harness_persist::PersistedNode {
        harness_persist::PersistedNode {
            node_id: node_id.into(),
            ordinal,
            status: status.into(),
            provider: None,
            model: None,
            output: String::new(),
            iterations: 1,
            converged: None,
            note: None,
            input_tokens: None,
            output_tokens: None,
            cache_read: None,
            cache_write: None,
            started_at: None,
            ended_at: None,
            artifact_content: None,
        }
    }

    fn meta(id: &str) -> harness_dag::NodeMeta {
        harness_dag::NodeMeta {
            id: id.into(),
            depends_on: vec![],
            category: None,
            artifact: None,
        }
    }

    fn detail_with(nodes: Vec<harness_persist::PersistedNode>) -> harness_persist::RunDetail {
        harness_persist::RunDetail {
            run: harness_persist::RunSummary {
                triggered_by: None,
                id: "r1".into(),
                workflow_name: "idea-to-pr".into(),
                title: None,
                description: Some(String::new()),
                status: "running".into(),
                node_count: nodes.len() as i32,
                started_at: Some(chrono::Utc::now()),
                ended_at: None,
                recorded_at: chrono::Utc::now(),
                project: Some("p".into()),
                ab_pair_id: None,
                ab_arm: None,
                ab_label: None,
            },
            nodes,
            graph: vec![],
        }
    }

    fn claim(reported: &str, quiet_minutes: i64) -> harness_persist::LinearClaim {
        harness_persist::LinearClaim {
            run_id: "r1".into(),
            project: "p".into(),
            workflow: "idea-to-pr".into(),
            issue_id: "i".into(),
            identifier: "ECOM-16".into(),
            original_state_id: "todo".into(),
            phase: "claimed".into(),
            agent_session_id: Some("sess-1".into()),
            reported_nodes: reported.into(),
            last_activity_at: chrono::Utc::now() - chrono::Duration::minutes(quiet_minutes),
            created_at: chrono::Utc::now(),
        }
    }

    #[test]
    fn only_finished_and_unreported_nodes_are_selected() {
        let detail = detail_with(vec![
            node("explore", 0, "success"),
            node("create-plan", 1, "success"),
            node("implement-tasks", 2, "running"),
            node("validate", 3, "pending"),
        ]);
        // `explore` already reported → only `create-plan` is new. A running or
        // pending node is not progress to report.
        let got = unreported_finished_nodes(&claim("explore", 0), &detail);
        assert_eq!(
            got,
            vec![("create-plan".to_string(), "success".to_string())]
        );

        // Nothing reported yet → both finished nodes, in graph order.
        let got = unreported_finished_nodes(&claim("", 0), &detail);
        assert_eq!(
            got.iter().map(|(id, _)| id.as_str()).collect::<Vec<_>>(),
            vec!["explore", "create-plan"]
        );

        // All reported → nothing, so a repeated tick posts nothing.
        assert!(unreported_finished_nodes(&claim("explore,create-plan", 0), &detail).is_empty());
    }

    /// The gap this closes: the thread reported finishes and, after 10 minutes of
    /// silence, a heartbeat — but never a start. So after "Finished explore" it
    /// read as stalled for the whole of `create-plan`, which routinely runs longer
    /// than the heartbeat interval.
    fn ready_binding(ready: Option<&str>, piece: Option<&str>) -> LinearSource {
        LinearSource {
            project: "p".into(),
            workflow: "idea-to-pr".into(),
            team_id: "t".into(),
            team_name: "T".into(),
            source_state_id: "todo".into(),
            failed_label: None,
            in_progress_state_id: None,
            review_state_id: None,
            ready_state_id: ready.map(str::to_string),
            piece_ready_state_id: piece.map(str::to_string),
            epic_review_state_id: None,
            base_branch: None,
            poll_interval_secs: 60,
            max_concurrent_runs: 1,
            max_attempts: 1,
            enabled: true,
            live: true,
            created_at: Utc::now(),
            updated_at: Utc::now(),
        }
    }

    /// A standalone issue is finished work heading for the default branch, so it
    /// stops at whatever gate the team keeps. A piece of an epic is heading for
    /// the epic's own branch, where the supervisor grades it and the feature is
    /// reviewed once as a single pull request — stopping every piece at the same
    /// gate reviews the same work N times.
    #[test]
    fn a_piece_and_a_standalone_issue_stop_in_different_columns() {
        let b = ready_binding(Some("functional-testing"), Some("ready-for-merge"));
        assert_eq!(ready_state_for(&b, false), Some("functional-testing"));
        assert_eq!(ready_state_for(&b, true), Some("ready-for-merge"));
    }

    /// The invariant that makes this safe to ship: a binding that never mentions
    /// the piece state behaves exactly as it did before it existed.
    #[test]
    fn a_binding_that_says_nothing_about_pieces_is_unchanged() {
        let b = ready_binding(Some("ready"), None);
        assert_eq!(ready_state_for(&b, false), Some("ready"));
        assert_eq!(ready_state_for(&b, true), Some("ready"));
    }

    #[test]
    fn no_ready_state_still_means_no_move() {
        let b = ready_binding(None, None);
        assert_eq!(ready_state_for(&b, true), None);
        // A piece state without a ready state is a half-configured binding, but
        // it is unambiguous about the piece, so honour it.
        let half = ready_binding(None, Some("ready-for-merge"));
        assert_eq!(ready_state_for(&half, true), Some("ready-for-merge"));
        assert_eq!(ready_state_for(&half, false), None);
    }

    #[test]
    fn a_running_node_is_announced_once() {
        let detail = detail_with(vec![
            node("explore", 0, "success"),
            node("create-plan", 1, "running"),
            node("install-deps", 2, "pending"),
        ]);
        // Finishing `explore` is reported and `create-plan` starting is too, in the
        // same tick — the thread shows both the end and what it unblocked.
        assert_eq!(
            unannounced_running_nodes(&claim("explore", 0), &detail),
            vec!["create-plan".to_string()]
        );
        // Announced already → silent on later ticks, so a 30s poll can't spam it.
        assert!(
            unannounced_running_nodes(&claim("explore,running:create-plan", 0), &detail).is_empty()
        );
        // The start key is distinct from the finish key: having announced the start
        // must not suppress the finish, nor the reverse.
        assert_eq!(
            unannounced_running_nodes(&claim("running:create-plan", 0), &detail),
            Vec::<String>::new()
        );
        assert_eq!(
            unreported_finished_nodes(&claim("running:explore", 0), &detail),
            vec![("explore".to_string(), "success".to_string())]
        );
    }

    /// A parallel layer runs several nodes at once; announcing only the first would
    /// misreport what the run is doing.
    #[test]
    fn every_node_of_a_parallel_layer_is_announced() {
        let detail = detail_with(vec![
            node("lint", 0, "running"),
            node("test", 1, "running"),
            node("deploy", 2, "pending"),
        ]);
        assert_eq!(
            unannounced_running_nodes(&claim("", 0), &detail),
            vec!["lint".to_string(), "test".to_string()]
        );
        // One announced, the other still pending announcement.
        assert_eq!(
            unannounced_running_nodes(&claim("running:lint", 0), &detail),
            vec!["test".to_string()]
        );
    }

    /// Linear concatenates `action` and `parameter`, so these read as one sentence
    /// ending in the workflow name. The old wording produced "Finished explore
    /// idea-to-pr" — three names in a row, nothing saying which was which.
    #[test]
    fn step_wording_names_the_step_its_position_and_the_workflow() {
        let at = Some((6, 15));
        let rendered = |node: &str, status: &str| {
            format!(
                "{} idea-to-pr",
                step_action(node, status, at, Some("took 1m 46s"))
            )
        };
        assert_eq!(
            rendered("explore", "success"),
            "Finished the explore step (6 of 15, took 1m 46s) of workflow idea-to-pr"
        );
        // A failed step did not "finish" — the old wording said it did — and how
        // long it ran before failing is the useful part.
        assert_eq!(
            rendered("validate", "failed"),
            "The validate step (6 of 15, took 1m 46s) failed in workflow idea-to-pr"
        );
        assert_eq!(
            rendered("finalize-pr", "cancelled"),
            "The finalize-pr step (6 of 15, took 1m 46s) was cancelled in workflow idea-to-pr"
        );
        // A running step reports the clock time it began rather than a duration:
        // across the two lines a reader gets both, with neither repeated.
        assert_eq!(
            format!(
                "{} idea-to-pr",
                step_action("create-plan", "running", at, Some("started 13:14 CEST"))
            ),
            "Running the create-plan step (6 of 15, started 13:14 CEST) of workflow idea-to-pr"
        );
    }

    /// Denmark observes summer time, so a fixed +01:00 "CET" offset would be an
    /// hour wrong from late March to late October. `Europe/Copenhagen` switches on
    /// its own, and the abbreviation is read off the zone rather than hardcoded, so
    /// the label can never disagree with the number beside it.
    #[test]
    fn danish_times_follow_summer_time() {
        let cph = chrono_tz::Europe::Copenhagen;
        let at = |m, d, h| chrono::TimeZone::with_ymd_and_hms(&Utc, 2026, m, d, h, 14, 0).unwrap();

        // Winter: UTC+1, labelled CET.
        assert_eq!(format_local(at(1, 15, 12), cph), "13:14 CET");
        // Summer: UTC+2, labelled CEST — the run this was built for.
        assert_eq!(format_local(at(7, 15, 11), cph), "13:14 CEST");

        // The transitions themselves, so a boundary bug can't hide between the
        // two cases above. 2026: forward 29 Mar, back 25 Oct, both at 01:00 UTC.
        assert_eq!(format_local(at(3, 29, 0), cph), "01:14 CET");
        assert_eq!(format_local(at(3, 29, 1), cph), "03:14 CEST");
        assert_eq!(format_local(at(10, 25, 0), cph), "02:14 CEST");
        assert_eq!(format_local(at(10, 25, 1), cph), "02:14 CET");
    }

    /// The zone is a setting with a Danish default, not a hardcoded country.
    #[test]
    fn another_zone_renders_its_own_wall_clock() {
        let at = chrono::TimeZone::with_ymd_and_hms(&Utc, 2026, 7, 15, 11, 14, 0).unwrap();
        assert_eq!(format_local(at, chrono_tz::UTC), "11:14 UTC");
        assert_eq!(
            format_local(at, "America/New_York".parse().unwrap()),
            "07:14 EDT"
        );
    }

    /// Either half of the parenthetical can be missing — a run recorded before
    /// graphs were stored has no position, a skipped-then-resumed node no
    /// timestamps. Neither may produce a stray "()".
    #[test]
    fn wording_omits_whichever_half_is_unknown() {
        assert_eq!(
            step_action("explore", "success", None, None),
            "Finished the explore step of workflow"
        );
        assert_eq!(
            step_action("explore", "success", None, Some("took 12s")),
            "Finished the explore step (took 12s) of workflow"
        );
        assert_eq!(
            step_action("explore", "success", Some((1, 4)), None),
            "Finished the explore step (1 of 4) of workflow"
        );
    }

    /// Coarsens as it grows: seconds stop mattering at hour scale, and an exact
    /// five minutes should not read "5m 0s".
    #[test]
    fn durations_read_the_way_a_human_would_say_them() {
        assert_eq!(human_duration(0), "0s");
        assert_eq!(human_duration(9), "9s");
        assert_eq!(human_duration(59), "59s");
        assert_eq!(human_duration(60), "1m");
        assert_eq!(human_duration(106), "1m 46s");
        assert_eq!(human_duration(300), "5m");
        assert_eq!(human_duration(3600), "1h");
        assert_eq!(human_duration(3660), "1h 1m");
        // Seconds are dropped past an hour rather than padding the string.
        assert_eq!(human_duration(3859), "1h 4m");
        assert_eq!(human_duration(7200), "2h");
    }

    /// Duration comes from the node's own timestamps, and anything unusable —
    /// a missing timestamp, or an inverted pair from clock skew — reports nothing
    /// rather than a negative or invented number.
    #[test]
    fn step_duration_needs_two_sane_timestamps() {
        let start = chrono::Utc::now();
        let mut finished = node("explore", 0, "success");
        finished.started_at = Some(start);
        finished.ended_at = Some(start + chrono::Duration::seconds(106));

        let mut no_end = node("create-plan", 1, "running");
        no_end.started_at = Some(start);

        let mut inverted = node("validate", 2, "success");
        inverted.started_at = Some(start);
        inverted.ended_at = Some(start - chrono::Duration::seconds(3));

        let detail = detail_with(vec![
            finished,
            no_end,
            inverted,
            node("skipped", 3, "skipped"),
        ]);
        assert_eq!(step_duration("explore", &detail).as_deref(), Some("1m 46s"));
        // Still running: no end time yet.
        assert_eq!(step_duration("create-plan", &detail), None);
        // Clock skew must not yield "-3s".
        assert_eq!(step_duration("validate", &detail), None);
        // A skipped node has neither timestamp.
        assert_eq!(step_duration("skipped", &detail), None);
        // A node absent from the run entirely.
        assert_eq!(step_duration("nope", &detail), None);
    }

    /// The position is the workflow's own numbering — what the DAG view shows —
    /// not the order nodes happened to execute in.
    #[test]
    fn step_position_counts_declared_nodes_in_declaration_order() {
        let mut detail = detail_with(vec![node("validate", 0, "running")]);
        detail.graph = ["explore", "create-plan", "install-deps", "validate"]
            .into_iter()
            .map(meta)
            .collect();
        // 4th declared node, 1-based — a human counting steps starts at one.
        assert_eq!(step_position("validate", &detail), Some((4, 4)));
        assert_eq!(step_position("explore", &detail), Some((1, 4)));
        // The total counts every declared node, including any a `when:` skips:
        // how many steps the workflow has is knowable, how many will run is not.
        assert_eq!(step_position("install-deps", &detail), Some((3, 4)));
        // A node not in the graph, and a run stored without one, both degrade.
        assert_eq!(step_position("nope", &detail), None);
        assert_eq!(
            step_position(
                "validate",
                &detail_with(vec![node("validate", 0, "running")])
            ),
            None
        );
    }

    /// `ordinal` is execution order, which diverges from the authored order as soon
    /// as a layer runs in parallel — so the counter must not be derived from it.
    #[test]
    fn step_position_ignores_execution_order() {
        let mut detail = detail_with(vec![
            // `test` ran first despite being declared second.
            node("test", 0, "success"),
            node("lint", 1, "running"),
        ]);
        detail.graph = ["lint", "test"].into_iter().map(meta).collect();
        assert_eq!(step_position("lint", &detail), Some((1, 2)));
        assert_eq!(step_position("test", &detail), Some((2, 2)));
    }

    #[test]
    fn failed_and_cancelled_nodes_are_reported_too() {
        // A failure is progress a human wants to see in the thread, and it also
        // resets the staleness clock.
        let detail = detail_with(vec![
            node("validate", 0, "failed"),
            node("finalize-pr", 1, "cancelled"),
        ]);
        let got = unreported_finished_nodes(&claim("", 0), &detail);
        assert_eq!(
            got,
            vec![
                ("validate".to_string(), "failed".to_string()),
                ("finalize-pr".to_string(), "cancelled".to_string()),
            ]
        );
    }

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

    /// Delegating an issue makes Linear post its own thread-opening comment. It
    /// has no emoji prefix, so the bot-status heuristic misses it, and it reached
    /// the agent as if a reviewer had written it.
    #[test]
    fn task_for_issue_filters_linears_agent_session_preamble() {
        let comments = vec![
            sample_comment(
                "This thread is for an agent session with aiharness.",
                "unknown",
            ),
            sample_comment("Please add a toggle.", "Alice"),
        ];
        let task = task_for_issue(&sample_issue(), &comments);
        assert!(!task.contains("agent session with"));
        assert!(task.contains("Please add a toggle."));

        // Matched on Linear's wording, so any app name is covered…
        assert!(is_agent_session_preamble(
            "This thread is for an agent session with some-other-bot."
        ));
        // …but a human discussing agent sessions is not silenced.
        assert!(!is_agent_session_preamble(
            "Should we use an agent session for this?"
        ));
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
