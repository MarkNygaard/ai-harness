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
use harness_sources::linear::{Comment, Issue, LinearClient};
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
pub(crate) async fn linear_key_for_project(
    state: &Arc<RunsState>,
    project: &str,
) -> Option<String> {
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
    // Pick the first eligible issue that hasn't already exhausted its attempt
    // budget. This is the hard loop-guard: a single binding never claims an issue
    // more than `max_attempts` times (so even a misconfigured binding — e.g. In
    // Progress == source — can't loop). The cap is per (issue, workflow), so an
    // issue that legitimately moves through several bindings across pipeline
    // stages (idea-to-pr → merge-pr) isn't exhausted by an earlier binding.
    let max_attempts = b.max_attempts.max(1) as i64;
    let mut chosen = None;
    for issue in issues {
        match claim_store.attempts_for_issue(&issue.id, &b.workflow).await {
            Ok(n) if n >= max_attempts => {
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
        description: task_for_issue(&issue, &comments),
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
                // The binding's attempt budget (default 1); fall back to 1 if the
                // binding has gone away. On a lookup error, assume exhausted so we
                // give up rather than risk a loop.
                let max_attempts = binding
                    .as_ref()
                    .map(|b| b.max_attempts.max(1) as i64)
                    .unwrap_or(1);
                // How many times this issue has been attempted for this workflow
                // (incl. this run).
                let attempts = claim_store
                    .attempts_for_issue(&c.issue_id, &c.workflow)
                    .await
                    .unwrap_or(max_attempts);
                if attempts >= max_attempts {
                    // Fail-safe: stop the loop. Do NOT return it to the source
                    // column (which would re-claim it forever). Leave it where the
                    // run left it and flag it for a human.
                    let _ = client
                        .add_comment(
                            &c.issue_id,
                            &format!(
                                "❌ ai-harness run `{}` failed ({}) — gave up after {} attempt(s). Not retrying; needs a human. Move it back to the source column to re-arm.",
                                c.run_id, detail.run.status, attempts
                            ),
                        )
                        .await;
                    tracing::warn!(
                        "linear poller: {} — {} attempt(s) failed; giving up (no rollback)",
                        c.identifier,
                        attempts
                    );
                } else {
                    // Roll back to the claimed-from state so it's retried.
                    let _ = client
                        .set_issue_state(&c.issue_id, &c.original_state_id)
                        .await;
                    let _ = client
                        .add_comment(
                            &c.issue_id,
                            &format!(
                                "⚠️ ai-harness run `{}` did not complete ({}); returning for retry (attempt {}/{}).",
                                c.run_id, detail.run.status, attempts, max_attempts
                            ),
                        )
                        .await;
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
