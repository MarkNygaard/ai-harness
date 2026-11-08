//! Improvements: what a workflow's recent runs suggest changing.
//!
//! A workflow opts in with `improvements: { every: N }`. After every N
//! finished runs of it *in a project*, the harness starts a review — an
//! internal run of `improvements-analysis` in that project's checkout — which
//! reads what those runs went through and suggests changes: mostly to the
//! project's repo (`CLAUDE.md` / `AGENTS.md`, skills, checks), sometimes to the
//! workflow. Runs from different projects are never mixed, because the fixes
//! land in one repo.
//!
//! The pieces:
//! - [`on_run_finished`] — called when any run ends. It counts towards a
//!   review, or, for a review's own run, stores what it found.
//! - `GET /api/run/improvements/evidence` — what the review reads, built here
//!   from the runs' steps and agent activity. Fetched by the review's first
//!   step with its run grant, so the size of ten runs' worth of evidence never
//!   passes through a command line.
//! - The HTTP routes and MCP tools for listing, reviewing now, applying and
//!   dismissing.
//!
//! The count is derived, not stored: the runs finished since the last review
//! began. A restart therefore can't lose or double it.

use std::collections::{BTreeMap, HashMap};
use std::sync::Arc;

use axum::extract::{Extension, Path as AxumPath, Query};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::Json;
use chrono::{DateTime, Utc};
use harness_persist::{
    ActivityEvent, ImprovementReview, ImprovementSuggestion, NewSuggestion, RunDetail,
    SuggestionFilter, REVIEW_DONE, REVIEW_FAILED, REVIEW_RUNNING, SUGGESTION_APPLIED,
    SUGGESTION_DISMISSED, SUGGESTION_NEW,
};
use harness_runner::IMPROVEMENTS_WORKFLOW;
use serde::Deserialize;
use serde_json::{json, Value};

use super::runs_routes::{start_run, CreateRunRequest, RunsState};

/// `trigger_source` on a review's run and on a run applying a suggestion. Also
/// what lets the server — and only the server — start the internal workflow.
pub(crate) const SOURCE_IMPROVEMENTS: &str = "improvements";

/// After a failed review, wait this long before an automatic retry, so a
/// standing problem (no public URL, say) isn't retried after every run.
const RETRY_AFTER_FAILURE_MINS: i64 = 60;

/// A review whose run never reported back is given up after this long — the
/// server restarted mid-review, or the run was lost — so the next can start.
const STALE_REVIEW_HOURS: i64 = 6;

/// Bounds on the evidence a review reads. Ten runs of a long workflow can hold
/// thousands of tool calls; the reviewer needs their shape, not every line.
const EVIDENCE_PER_RUN: usize = 12_000;
const EVIDENCE_TOTAL: usize = 160_000;
const WORKFLOW_YAML_LIMIT: usize = 14_000;
const ACTIVITY_PER_RUN: i64 = 6_000;
const OUTPUT_TAIL: usize = 600;
const FAILED_OUTPUT_TAIL: usize = 1_500;

/// Suggestions kept from one review. The reviewer is asked for at most five;
/// this only guards against a runaway answer.
const MAX_SUGGESTIONS: usize = 8;

const TARGETS: &[&str] = &["repo", "workflow"];
const CATEGORIES: &[&str] = &[
    "navigation",
    "check",
    "standard",
    "skill",
    "prune",
    "access",
    "workflow",
];

type Refusal = (StatusCode, String);

fn err(status: StatusCode, msg: impl Into<String>) -> Response {
    (status, Json(json!({ "error": msg.into() }))).into_response()
}

fn internal(e: impl std::fmt::Display) -> Refusal {
    (StatusCode::INTERNAL_SERVER_ERROR, e.to_string())
}

// ── When a run ends ─────────────────────────────────────────────────────────

/// Called when any run ends, in the background. Either stores what a review
/// found, or counts the run towards its workflow's next review.
///
/// Boxed, not `async fn`: a run ending can start a review, which starts a
/// run, which ends here — and an unboxed future would have to contain its own
/// type.
pub(crate) fn on_run_finished(
    state: Arc<RunsState>,
    run_id: String,
) -> futures::future::BoxFuture<'static, ()> {
    Box::pin(on_run_finished_inner(state, run_id))
}

async fn on_run_finished_inner(state: Arc<RunsState>, run_id: String) {
    let Ok(store) = state.store().await else {
        return;
    };
    let Ok(Some(detail)) = store.get_run(&run_id).await else {
        return;
    };
    super::pr_tracking::record_run(&state, &detail).await;
    super::facts_routes::on_run_finished(&state, &detail).await;
    if detail.run.workflow_name == IMPROVEMENTS_WORKFLOW {
        finish_review_run(&state, &detail).await;
        return;
    }
    let Some(project) = detail.run.project.clone() else {
        return;
    };
    let Some(every) = improvements_every(&state, &detail.run.workflow_name) else {
        return;
    };
    if let Err((_, e)) = review(&state, &detail.run.workflow_name, &project, every, false).await {
        tracing::warn!(
            "improvements: {}/{} — could not start a review: {e}",
            detail.run.workflow_name,
            project
        );
    }
}

/// `improvements.every` for a workflow, when it opts in.
fn improvements_every(state: &Arc<RunsState>, workflow: &str) -> Option<u32> {
    let (yaml, _) = harness_runner::resolve_workflow_source(workflow, &state.project_root).ok()?;
    let wf = harness_dag::parse_workflow(&yaml).ok()?;
    if wf.internal {
        return None;
    }
    wf.improvements.map(|c| c.every)
}

/// What `review` decided.
#[derive(Debug)]
pub(crate) enum Reviewed {
    Started(ImprovementReview),
    /// Fewer runs than a review needs.
    NotYet,
    AlreadyRunning(ImprovementReview),
    /// A recent review failed; automatic retries wait.
    Backoff,
}

/// Start a review of `workflow` in `project` if one is due — or, with
/// `force`, now, over the latest `every` runs whatever the count.
pub(crate) async fn review(
    state: &Arc<RunsState>,
    workflow: &str,
    project: &str,
    every: u32,
    force: bool,
) -> Result<Reviewed, Refusal> {
    let improvements = state
        .improvement_store()
        .await
        .map_err(|e| (StatusCode::SERVICE_UNAVAILABLE, e))?;
    let runs = state
        .store()
        .await
        .map_err(|e| (StatusCode::SERVICE_UNAVAILABLE, e))?;

    let latest = improvements
        .latest_review(workflow, project)
        .await
        .map_err(internal)?;
    if let Some(r) = &latest {
        if r.status == REVIEW_RUNNING {
            // Ended without reporting back (a restart mid-review)? Close it,
            // so it doesn't block every review after it.
            if !settle_stale(state, r).await {
                return Ok(Reviewed::AlreadyRunning(r.clone()));
            }
        } else if r.status == REVIEW_FAILED && !force {
            let recent = r.finished_at.is_some_and(|t| {
                Utc::now().signed_duration_since(t).num_minutes() < RETRY_AFTER_FAILURE_MINS
            });
            if recent {
                return Ok(Reviewed::Backoff);
            }
        }
    }

    // The window: runs finished since the last review that read anything.
    // A failed one didn't, so its runs still count.
    let since = window_start(
        &improvements
            .latest_review_done(workflow, project)
            .await
            .map_err(internal)?,
    );
    let ids = runs
        .improvement_candidates(
            workflow,
            project,
            if force { None } else { since },
            every as i64,
        )
        .await
        .map_err(internal)?;
    if ids.is_empty() || (!force && ids.len() < every as usize) {
        return Ok(Reviewed::NotYet);
    }

    let id = uuid::Uuid::new_v4().to_string();
    let Some(review) = improvements
        .begin_review(&id, workflow, project, &ids)
        .await
        .map_err(internal)?
    else {
        // Another finishing run got there first.
        let running = improvements
            .latest_review(workflow, project)
            .await
            .map_err(internal)?;
        return Ok(match running {
            Some(r) => Reviewed::AlreadyRunning(r),
            None => Reviewed::Backoff,
        });
    };

    let req = CreateRunRequest {
        triggered_by: None,
        trigger_source: Some(SOURCE_IMPROVEMENTS.to_string()),
        trigger_actor: Some("Improvements".to_string()),
        workflow: IMPROVEMENTS_WORKFLOW.to_string(),
        title: Some(format!("Improvements for {workflow} — {} runs", ids.len())),
        // Small on purpose: the evidence is fetched by the run, not passed in.
        description: format!(
            "Review {} recent runs of `{workflow}` in `{project}` and suggest improvements \
             (review {}).",
            ids.len(),
            review.id
        ),
        args: String::new(),
        issue_id: None,
        real: true,
        base_branch: None,
        project: Some(project.to_string()),
        swap_from: None,
        swap_to: None,
        ab_pair_id: None,
        ab_arm: None,
        ab_label: None,
    };
    match start_run(state, req).await {
        Ok(run_id) => {
            improvements
                .set_review_run(&review.id, &run_id)
                .await
                .map_err(internal)?;
            let mut review = review;
            review.run_id = Some(run_id);
            Ok(Reviewed::Started(review))
        }
        Err((status, message)) => {
            let _ = improvements
                .finish_review(&review.id, REVIEW_FAILED, None, Some(&message), &[])
                .await;
            Err((status, message))
        }
    }
}

/// Where the counting window starts: when the last completed review began.
fn window_start(last_done: &Option<ImprovementReview>) -> Option<DateTime<Utc>> {
    last_done.as_ref().map(|r| r.created_at)
}

/// Close a running review whose run is over (or was never started and is
/// long past starting). `true` when it was closed.
async fn settle_stale(state: &Arc<RunsState>, r: &ImprovementReview) -> bool {
    let Ok(store) = state.store().await else {
        return false;
    };
    match &r.run_id {
        Some(run_id) => match store.get_run(run_id).await {
            Ok(Some(detail)) if is_terminal(&detail.run.status) => {
                finish_review_run(state, &detail).await;
                true
            }
            // Still running, or not written yet.
            Ok(Some(_)) => false,
            Ok(None) | Err(_) => give_up_if_old(state, r).await,
        },
        None => give_up_if_old(state, r).await,
    }
}

async fn give_up_if_old(state: &Arc<RunsState>, r: &ImprovementReview) -> bool {
    if Utc::now().signed_duration_since(r.created_at).num_hours() < STALE_REVIEW_HOURS {
        return false;
    }
    let Ok(improvements) = state.improvement_store().await else {
        return false;
    };
    let _ = improvements
        .finish_review(
            &r.id,
            REVIEW_FAILED,
            None,
            Some("the review's run never reported back"),
            &[],
        )
        .await;
    true
}

fn is_terminal(status: &str) -> bool {
    matches!(status, "completed" | "failed" | "cancelled")
}

/// A review's run ended: store its suggestions, or why it failed.
async fn finish_review_run(state: &Arc<RunsState>, detail: &RunDetail) {
    let Ok(improvements) = state.improvement_store().await else {
        return;
    };
    let Ok(Some(review)) = improvements.review_for_run(&detail.run.id).await else {
        return;
    };
    if review.status != REVIEW_RUNNING {
        return;
    }
    let result = match detail.run.status.as_str() {
        "completed" => {
            let output = detail
                .nodes
                .iter()
                .find(|n| n.node_id == "suggest")
                .map(|n| n.output.as_str())
                .unwrap_or_default();
            let open: Vec<String> = improvements
                .list_suggestions(&SuggestionFilter {
                    workflow: Some(review.workflow.clone()),
                    project: Some(review.project.clone()),
                    status: Some(SUGGESTION_NEW.to_string()),
                })
                .await
                .unwrap_or_default()
                .into_iter()
                .map(|s| s.id)
                .collect();
            parse_suggestions(output, &review.run_ids, &open)
        }
        other => Err(format!("the review run {other}")),
    };
    let stored = match result {
        Ok((summary, suggestions)) => {
            improvements
                .finish_review(
                    &review.id,
                    REVIEW_DONE,
                    summary.as_deref(),
                    None,
                    &suggestions,
                )
                .await
        }
        Err(e) => {
            improvements
                .finish_review(&review.id, REVIEW_FAILED, None, Some(&e), &[])
                .await
        }
    };
    if let Err(e) = stored {
        tracing::warn!(
            "improvements: review {} — could not store the result: {e}",
            review.id
        );
    }
}

// ── Reading the review's answer ─────────────────────────────────────────────

#[derive(Debug, Deserialize)]
struct RawAnswer {
    #[serde(default)]
    summary: Option<String>,
    #[serde(default)]
    suggestions: Vec<RawSuggestion>,
}

#[derive(Debug, Deserialize)]
struct RawSuggestion {
    /// An open suggestion this is the same problem as.
    #[serde(default)]
    same_as: Option<String>,
    #[serde(default)]
    title: String,
    #[serde(default)]
    target: String,
    #[serde(default)]
    category: String,
    #[serde(default)]
    node: Option<String>,
    #[serde(default)]
    problem: String,
    #[serde(default)]
    change: String,
    #[serde(default)]
    evidence: Vec<RawEvidence>,
}

#[derive(Debug, Deserialize, serde::Serialize)]
struct RawEvidence {
    #[serde(default)]
    run_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    node: Option<String>,
    #[serde(default)]
    note: String,
}

/// The JSON object in an agent's answer: all of it, or the outermost `{…}` in
/// prose around it.
fn extract_json(text: &str) -> Option<Value> {
    let t = text.trim();
    let t = t
        .strip_prefix("```json")
        .or_else(|| t.strip_prefix("```"))
        .map(|s| s.trim_end().trim_end_matches("```"))
        .unwrap_or(t);
    if let Ok(v) = serde_json::from_str::<Value>(t.trim()) {
        return Some(v);
    }
    let start = t.find('{')?;
    let end = t.rfind('}')?;
    serde_json::from_str(&t[start..=end]).ok()
}

/// The review's suggestions, kept only when they can be traced: a suggestion
/// with no evidence from the runs it read is dropped, as the reviewer was told
/// it would be. Unknown targets and categories are normalised rather than
/// rejected — a slightly-off label shouldn't lose a good suggestion.
///
/// `same_as` is honoured only for one of `open` — a suggestion of this
/// workflow and project nobody has acted on yet. An answer pointing anywhere
/// else is read as a new suggestion, and needs a title and change like one.
pub(crate) fn parse_suggestions(
    output: &str,
    run_ids: &[String],
    open: &[String],
) -> Result<(Option<String>, Vec<NewSuggestion>), String> {
    let value = extract_json(output).ok_or("the review's answer held no JSON")?;
    let answer: RawAnswer =
        serde_json::from_value(value).map_err(|e| format!("the review's answer: {e}"))?;
    let mut out = Vec::new();
    for s in answer.suggestions {
        let evidence: Vec<RawEvidence> = s
            .evidence
            .into_iter()
            .filter(|e| run_ids.iter().any(|id| *id == e.run_id.trim()))
            .collect();
        let title = s.title.trim().to_string();
        let change = s.change.trim().to_string();
        let same_as = s
            .same_as
            .map(|id| id.trim().to_string())
            .filter(|id| open.contains(id));
        if evidence.is_empty() || (same_as.is_none() && (title.is_empty() || change.is_empty())) {
            continue;
        }
        let category = s.category.trim().to_ascii_lowercase();
        let category = if CATEGORIES.contains(&category.as_str()) {
            category
        } else if s.target.trim() == "workflow" {
            "workflow".to_string()
        } else {
            "standard".to_string()
        };
        // A workflow-category suggestion targets the workflow, whatever the
        // label said; everything else lands in the repo.
        let target = if category == "workflow" {
            "workflow".to_string()
        } else {
            "repo".to_string()
        };
        debug_assert!(TARGETS.contains(&target.as_str()));
        out.push(NewSuggestion {
            same_as,
            target,
            category,
            node: s
                .node
                .map(|n| n.trim().to_string())
                .filter(|n| !n.is_empty()),
            title,
            problem: s.problem.trim().to_string(),
            change,
            evidence: serde_json::to_value(evidence).unwrap_or_else(|_| json!([])),
        });
        if out.len() == MAX_SUGGESTIONS {
            break;
        }
    }
    Ok((
        answer
            .summary
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty()),
        out,
    ))
}

// ── The evidence a review reads ─────────────────────────────────────────────

fn tail(s: &str, max: usize) -> String {
    let s = s.trim();
    if s.len() <= max {
        return s.to_string();
    }
    let mut start = s.len() - max;
    while !s.is_char_boundary(start) {
        start += 1;
    }
    format!("…{}", &s[start..])
}

fn clip(s: &str, max: usize) -> String {
    let s = s.trim().replace('\n', " ");
    if s.len() <= max {
        return s;
    }
    let mut end = max;
    while !s.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}…", &s[..end])
}

/// One run, as the reviewer reads it: each step's shape (model, iterations,
/// tool calls, failures, repeats) and the end of what it said.
pub(crate) fn run_digest(detail: &RunDetail, activity: &[ActivityEvent]) -> String {
    let r = &detail.run;
    let mut out = format!(
        "## Run `{}` — {} ({})\n",
        r.id,
        r.title.as_deref().unwrap_or("untitled"),
        r.status
    );
    if let (Some(a), Some(b)) = (r.started_at, r.ended_at) {
        out.push_str(&format!(
            "Took {} min.\n",
            b.signed_duration_since(a).num_minutes()
        ));
    }

    let mut by_node: HashMap<&str, Vec<&ActivityEvent>> = HashMap::new();
    for a in activity {
        by_node.entry(a.node_id.as_str()).or_default().push(a);
    }

    for n in &detail.nodes {
        let events = by_node.get(n.node_id.as_str()).cloned().unwrap_or_default();
        let tools: Vec<&ActivityEvent> = events
            .iter()
            .copied()
            .filter(|a| a.kind == "tool")
            .collect();
        let errors: Vec<&ActivityEvent> = events.iter().copied().filter(|a| a.is_error).collect();
        let model = match (&n.provider, &n.model) {
            (Some(p), Some(m)) => format!("{p}/{m}"),
            (Some(p), None) => p.clone(),
            _ => "no model".into(),
        };
        out.push_str(&format!(
            "\n### `{}` — {} · {model} · {} iteration(s){} · {} tool call(s) · {} failed\n",
            n.node_id,
            n.status,
            n.iterations,
            match n.converged {
                Some(false) => " (did not converge)",
                _ => "",
            },
            tools.len(),
            errors.len(),
        ));

        // The same tool call over and over: re-reading, re-searching.
        let mut counts: BTreeMap<&str, usize> = BTreeMap::new();
        for t in &tools {
            if !t.text.trim().is_empty() {
                *counts.entry(t.text.trim()).or_default() += 1;
            }
        }
        let mut repeated: Vec<(&str, usize)> =
            counts.into_iter().filter(|(_, c)| *c >= 3).collect();
        repeated.sort_by_key(|r| std::cmp::Reverse(r.1));
        for (text, count) in repeated.iter().take(4) {
            out.push_str(&format!("- repeated ×{count}: {}\n", clip(text, 140)));
        }

        // Failures, de-duplicated: the same failure ten times is one line.
        let mut seen: Vec<String> = Vec::new();
        for e in &errors {
            let msg = clip(e.detail.as_deref().unwrap_or(&e.text), 220);
            if msg.is_empty() || seen.contains(&msg) {
                continue;
            }
            seen.push(msg);
            if seen.len() == 5 {
                break;
            }
        }
        for msg in &seen {
            out.push_str(&format!("- failed: {msg}\n"));
        }

        let limit = if n.status == "failed" {
            FAILED_OUTPUT_TAIL
        } else {
            OUTPUT_TAIL
        };
        let said = tail(&n.output, limit);
        if !said.is_empty() {
            out.push_str("Output (end):\n```\n");
            out.push_str(&said);
            out.push_str("\n```\n");
        }
        if let Some(note) = n.note.as_deref().filter(|s| !s.trim().is_empty()) {
            out.push_str(&format!("Note: {}\n", clip(note, 300)));
        }
    }

    if out.len() > EVIDENCE_PER_RUN {
        let mut end = EVIDENCE_PER_RUN;
        while !out.is_char_boundary(end) {
            end -= 1;
        }
        out.truncate(end);
        out.push_str("\n… (this run's digest was cut short)\n");
    }
    out
}

/// What was suggested before, by what became of it. Open ones carry their id:
/// a review that sees one again answers with `same_as` instead of writing it
/// up a second time.
pub(crate) fn earlier_suggestions(earlier: &[ImprovementSuggestion]) -> String {
    if earlier.is_empty() {
        return String::new();
    }
    let mut out = String::from("## Earlier suggestions\n\n");

    let open: Vec<_> = earlier
        .iter()
        .filter(|s| s.status == SUGGESTION_NEW)
        .take(30)
        .collect();
    if !open.is_empty() {
        out.push_str(
            "### Open — not acted on yet\n\nIf one of these still happens in the runs below, \
             don't write it up again: answer with its id in `same_as` and only the new \
             evidence.\n\n",
        );
        for s in open {
            out.push_str(&format!(
                "- `{}` — {} (seen in {} review{}) — {}\n",
                s.id,
                s.title,
                s.seen_count,
                if s.seen_count == 1 { "" } else { "s" },
                clip(&s.problem, 200)
            ));
        }
        out.push('\n');
    }
    let dismissed: Vec<_> = earlier
        .iter()
        .filter(|s| s.status == SUGGESTION_DISMISSED)
        .take(15)
        .collect();
    if !dismissed.is_empty() {
        out.push_str(
            "### Dismissed — a person decided against these; don't suggest them again\n\n",
        );
        for s in dismissed {
            out.push_str(&format!("- {} — {}\n", s.title, clip(&s.problem, 200)));
        }
        out.push('\n');
    }
    let applied: Vec<_> = earlier
        .iter()
        .filter(|s| s.status == SUGGESTION_APPLIED)
        .take(15)
        .collect();
    if !applied.is_empty() {
        out.push_str(
            "### Applied — a change was made for these; if the problem still shows, it \
             didn't work\n\n",
        );
        for s in applied {
            out.push_str(&format!("- {} — {}\n", s.title, clip(&s.problem, 200)));
        }
        out.push('\n');
    }
    out
}

/// Everything a review reads, as Markdown.
async fn evidence(state: &Arc<RunsState>, review: &ImprovementReview) -> Result<String, Refusal> {
    let runs = state
        .store()
        .await
        .map_err(|e| (StatusCode::SERVICE_UNAVAILABLE, e))?;
    let improvements = state
        .improvement_store()
        .await
        .map_err(|e| (StatusCode::SERVICE_UNAVAILABLE, e))?;

    let mut out = format!(
        "# Evidence: `{}` in `{}`\n\n{} run(s), newest first. Each lists its steps: status, \
         model, iterations, tool calls, failed tool calls (with their messages), tool calls \
         repeated three or more times, and the end of what the step said.\n\n",
        review.workflow,
        review.project,
        review.run_ids.len()
    );

    // The workflow itself, so a workflow-level suggestion can name a step.
    if let Ok(src) = harness_runner::authoring::get_workflow(&state.project_root, &review.workflow)
    {
        out.push_str("## The workflow\n\n```yaml\n");
        out.push_str(&tail(&src.yaml, WORKFLOW_YAML_LIMIT));
        out.push_str("\n```\n\n");
    }

    // What was suggested before, and what became of it.
    let earlier = improvements
        .list_suggestions(&SuggestionFilter {
            workflow: Some(review.workflow.clone()),
            project: Some(review.project.clone()),
            status: None,
        })
        .await
        .map_err(internal)?;
    out.push_str(&earlier_suggestions(&earlier));

    for id in &review.run_ids {
        if out.len() >= EVIDENCE_TOTAL {
            out.push_str("\n… (further runs left out to keep this readable)\n");
            break;
        }
        let Ok(Some(detail)) = runs.get_run(id).await else {
            continue;
        };
        let mut activity: Vec<ActivityEvent> = Vec::new();
        let mut after = 0;
        while (activity.len() as i64) < ACTIVITY_PER_RUN {
            let page = runs
                .activity_since(id, after, 1000)
                .await
                .map_err(internal)?;
            let Some(last) = page.last() else {
                break;
            };
            after = last.id;
            let full = page.len() == 1000;
            activity.extend(page);
            if !full {
                break;
            }
        }
        out.push_str(&run_digest(&detail, &activity));
        out.push('\n');
    }
    Ok(out)
}

/// `GET /api/run/improvements/evidence` — what a review's run reads. Authorized
/// by the run's own grant, which names the run: only the review that run
/// belongs to is served.
pub async fn get_evidence(
    Extension(state): Extension<Arc<RunsState>>,
    headers: HeaderMap,
) -> Response {
    let grant = match super::run_linear_routes::grant(&headers) {
        Ok(g) => g,
        Err((s, m)) => return err(s, m),
    };
    let Ok(improvements) = state.improvement_store().await else {
        return err(
            StatusCode::SERVICE_UNAVAILABLE,
            "improvements are unavailable",
        );
    };
    let review = match improvements.review_for_run(&grant.run_id).await {
        Ok(Some(r)) if r.project == grant.project => r,
        Ok(_) => {
            return err(
                StatusCode::NOT_FOUND,
                "this run is not an improvements review",
            )
        }
        Err(e) => return err(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()),
    };
    match evidence(&state, &review).await {
        Ok(text) => ([("content-type", "text/markdown; charset=utf-8")], text).into_response(),
        Err((s, m)) => err(s, m),
    }
}

// ── Applying ────────────────────────────────────────────────────────────────

/// The task a coding run is given to carry out a repo suggestion.
pub(crate) fn apply_task(s: &ImprovementSuggestion) -> String {
    let runs: Vec<String> = s
        .evidence
        .0
        .as_array()
        .map(|a| {
            a.iter()
                .filter_map(|e| e.get("run_id").and_then(Value::as_str))
                .map(|id| format!("`{id}`"))
                .collect()
        })
        .unwrap_or_default();
    format!(
        "Improve how coding agents work in this repository, as one focused pull request.\n\n\
         ## {title}\n\n{change}\n\n\
         ## Why\n\n{problem}\n\n\
         This came from reviewing runs of the `{workflow}` workflow{seen}.\n\n\
         Make only this change. If you add to `CLAUDE.md` or `AGENTS.md`, keep it short: \
         those files load into every agent session. A rule a tool can check belongs in a \
         lint rule, test or hook rather than in prose.",
        title = s.title,
        change = s.change,
        problem = s.problem,
        workflow = s.workflow,
        seen = if runs.is_empty() {
            String::new()
        } else {
            format!(" ({})", runs.join(", "))
        },
    )
}

/// Start a run carrying out a repo suggestion, and mark it applied. A workflow
/// suggestion is applied in the editor instead.
pub(crate) async fn apply(
    state: &Arc<RunsState>,
    id: &str,
    workflow: Option<String>,
    actor: &super::runs_routes::TriggerInfo,
) -> Result<(ImprovementSuggestion, String), Refusal> {
    let improvements = state
        .improvement_store()
        .await
        .map_err(|e| (StatusCode::SERVICE_UNAVAILABLE, e))?;
    let s = improvements
        .get_suggestion(id)
        .await
        .map_err(internal)?
        .ok_or_else(|| (StatusCode::NOT_FOUND, format!("no suggestion `{id}`")))?;
    if s.target != "repo" {
        return Err((
            StatusCode::BAD_REQUEST,
            "a suggestion about the workflow is applied by editing the workflow".into(),
        ));
    }
    // The project's own workflow for such work, unless the caller names one.
    let workflow = match workflow
        .map(|w| w.trim().to_string())
        .filter(|w| !w.is_empty())
    {
        Some(w) => w,
        None => {
            let projects = state
                .project_store()
                .await
                .map_err(|e| (StatusCode::SERVICE_UNAVAILABLE, e))?;
            projects
                .get(&s.project)
                .await
                .map_err(internal)?
                .and_then(|p| p.default_workflow)
                .filter(|w| !w.trim().is_empty())
                .unwrap_or_else(|| harness_runner::DEFAULT_WORKFLOW.to_string())
        }
    };
    let req = CreateRunRequest {
        triggered_by: actor.user_id.clone(),
        trigger_source: Some(SOURCE_IMPROVEMENTS.to_string()),
        trigger_actor: actor.actor.clone(),
        workflow,
        title: Some(format!("Improve agent setup: {}", s.title)),
        description: apply_task(&s),
        args: String::new(),
        issue_id: None,
        real: true,
        base_branch: None,
        project: Some(s.project.clone()),
        swap_from: None,
        swap_to: None,
        ab_pair_id: None,
        ab_arm: None,
        ab_label: None,
    };
    let run_id = start_run(state, req).await?;
    let s = improvements
        .set_suggestion_status(id, SUGGESTION_APPLIED, Some(&run_id))
        .await
        .map_err(internal)?
        .ok_or_else(|| (StatusCode::NOT_FOUND, format!("no suggestion `{id}`")))?;
    Ok((s, run_id))
}

pub(crate) async fn set_status(
    state: &Arc<RunsState>,
    id: &str,
    status: &str,
) -> Result<ImprovementSuggestion, Refusal> {
    if ![SUGGESTION_NEW, SUGGESTION_APPLIED, SUGGESTION_DISMISSED].contains(&status) {
        return Err((
            StatusCode::BAD_REQUEST,
            "`status` is `new`, `applied` or `dismissed`".into(),
        ));
    }
    let improvements = state
        .improvement_store()
        .await
        .map_err(|e| (StatusCode::SERVICE_UNAVAILABLE, e))?;
    improvements
        .set_suggestion_status(id, status, None)
        .await
        .map_err(internal)?
        .ok_or_else(|| (StatusCode::NOT_FOUND, format!("no suggestion `{id}`")))
}

// ── Overview ────────────────────────────────────────────────────────────────

/// Every workflow that opts in, per project: how many runs towards the next
/// review, and the latest review. Projects a workflow never ran in are left
/// out — there is nothing to say about them yet.
pub(crate) async fn overview(state: &Arc<RunsState>) -> Result<Vec<Value>, Refusal> {
    let improvements = state
        .improvement_store()
        .await
        .map_err(|e| (StatusCode::SERVICE_UNAVAILABLE, e))?;
    let runs = state
        .store()
        .await
        .map_err(|e| (StatusCode::SERVICE_UNAVAILABLE, e))?;
    let projects = state
        .project_store()
        .await
        .map_err(|e| (StatusCode::SERVICE_UNAVAILABLE, e))?
        .list()
        .await
        .map_err(internal)?;
    let latest = improvements.latest_reviews().await.map_err(internal)?;

    let mut out = Vec::new();
    for wf in harness_runner::authoring::list_workflows(&state.project_root) {
        let Some(cfg) = wf.improvements else {
            continue;
        };
        for p in &projects {
            let review = latest
                .iter()
                .find(|r| r.workflow == wf.name && r.project == p.name);
            let done = improvements
                .latest_review_done(&wf.name, &p.name)
                .await
                .map_err(internal)?;
            let since = runs
                .improvement_candidates(&wf.name, &p.name, window_start(&done), cfg.every as i64)
                .await
                .map_err(internal)?
                .len();
            let ever = review.is_some()
                || !runs
                    .improvement_candidates(&wf.name, &p.name, None, 1)
                    .await
                    .map_err(internal)?
                    .is_empty();
            if !ever {
                continue;
            }
            out.push(json!({
                "workflow": wf.name,
                "project": p.name,
                "every": cfg.every,
                "runs_since_review": since,
                "review": review,
            }));
        }
    }
    Ok(out)
}

// ── HTTP routes ─────────────────────────────────────────────────────────────

#[derive(Debug, Deserialize, Default)]
pub struct ListQuery {
    #[serde(default)]
    pub workflow: Option<String>,
    #[serde(default)]
    pub project: Option<String>,
    #[serde(default)]
    pub status: Option<String>,
}

fn non_empty(s: Option<String>) -> Option<String> {
    s.map(|v| v.trim().to_string()).filter(|v| !v.is_empty())
}

/// `GET /api/improvements` — suggestions (filtered), plus where each opted-in
/// workflow stands per project.
pub async fn list(
    Extension(state): Extension<Arc<RunsState>>,
    Query(q): Query<ListQuery>,
) -> Response {
    let improvements = match state.improvement_store().await {
        Ok(s) => s,
        Err(e) => return err(StatusCode::SERVICE_UNAVAILABLE, e),
    };
    let filter = SuggestionFilter {
        workflow: non_empty(q.workflow),
        project: non_empty(q.project),
        status: non_empty(q.status),
    };
    let suggestions = match improvements.list_suggestions(&filter).await {
        Ok(s) => s,
        Err(e) => return err(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()),
    };
    match overview(&state).await {
        Ok(workflows) => {
            Json(json!({ "suggestions": suggestions, "workflows": workflows })).into_response()
        }
        Err((s, m)) => err(s, m),
    }
}

#[derive(Debug, Deserialize)]
pub struct ReviewBody {
    pub workflow: String,
    pub project: String,
}

/// `POST /api/improvements/review` — review now, over the latest runs.
pub async fn review_now(
    Extension(state): Extension<Arc<RunsState>>,
    Json(body): Json<ReviewBody>,
) -> Response {
    match review_now_inner(&state, &body.workflow, &body.project).await {
        Ok(v) => Json(v).into_response(),
        Err((s, m)) => err(s, m),
    }
}

pub(crate) async fn review_now_inner(
    state: &Arc<RunsState>,
    workflow: &str,
    project: &str,
) -> Result<Value, Refusal> {
    let every = improvements_every(state, workflow).ok_or_else(|| {
        (
            StatusCode::BAD_REQUEST,
            format!("`{workflow}` doesn't have improvements switched on"),
        )
    })?;
    match review(state, workflow, project, every, true).await? {
        Reviewed::Started(r) => Ok(json!({ "started": r })),
        Reviewed::AlreadyRunning(r) => Ok(json!({ "running": r })),
        Reviewed::NotYet => Err((
            StatusCode::BAD_REQUEST,
            format!("`{workflow}` has no finished runs in `{project}` to review yet"),
        )),
        Reviewed::Backoff => Err((
            StatusCode::CONFLICT,
            "a review is already starting — try again in a moment".into(),
        )),
    }
}

#[derive(Debug, Deserialize)]
pub struct StatusBody {
    pub status: String,
}

/// `PUT /api/improvements/suggestions/{id}` — dismiss, reopen, or mark applied.
pub async fn update_suggestion(
    Extension(state): Extension<Arc<RunsState>>,
    AxumPath(id): AxumPath<String>,
    Json(body): Json<StatusBody>,
) -> Response {
    match set_status(&state, &id, &body.status).await {
        Ok(s) => Json(s).into_response(),
        Err((s, m)) => err(s, m),
    }
}

#[derive(Debug, Deserialize, Default)]
pub struct ApplyBody {
    /// The workflow to carry it out with; default the project's own.
    #[serde(default)]
    pub workflow: Option<String>,
}

/// `POST /api/improvements/suggestions/{id}/apply` — start a run that makes
/// the change, as a pull request in the project's repo.
pub async fn apply_suggestion(
    Extension(state): Extension<Arc<RunsState>>,
    headers: HeaderMap,
    AxumPath(id): AxumPath<String>,
    body: Option<Json<ApplyBody>>,
) -> Response {
    let actor = super::runs_routes::TriggerInfo::from_caller(
        &state,
        &headers,
        super::runs_routes::SOURCE_UI,
    )
    .await;
    let workflow = body.and_then(|Json(b)| b.workflow);
    match apply(&state, &id, workflow, &actor).await {
        Ok((s, run_id)) => Json(json!({ "suggestion": s, "run_id": run_id })).into_response(),
        Err((s, m)) => err(s, m),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ids() -> Vec<String> {
        vec!["run-a".into(), "run-b".into()]
    }

    #[test]
    fn suggestions_without_evidence_from_these_runs_are_dropped() {
        let answer = json!({
            "summary": "Mostly smooth.",
            "suggestions": [
                { "title": "Point at migrations", "target": "repo", "category": "navigation",
                  "problem": "searched", "change": "add a pointer",
                  "evidence": [{ "run_id": "run-a", "note": "14 finds" }] },
                { "title": "Generic advice", "target": "repo", "category": "standard",
                  "problem": "x", "change": "write better code", "evidence": [] },
                { "title": "Invented run", "target": "repo", "category": "check",
                  "problem": "x", "change": "y",
                  "evidence": [{ "run_id": "run-zzz", "note": "?" }] }
            ]
        });
        let (summary, s) = parse_suggestions(&answer.to_string(), &ids(), &[]).unwrap();
        assert_eq!(summary.as_deref(), Some("Mostly smooth."));
        assert_eq!(s.len(), 1);
        assert_eq!(s[0].title, "Point at migrations");
    }

    #[test]
    fn same_as_is_kept_only_for_an_open_suggestion() {
        let answer = json!({ "suggestions": [
            { "same_as": "open-1", "evidence": [{ "run_id": "run-a", "note": "again" }] },
            { "same_as": "someone-elses", "evidence": [{ "run_id": "run-a", "note": "n" }] },
            { "same_as": "gone", "title": "t", "target": "repo", "category": "check",
              "problem": "p", "change": "c", "evidence": [{ "run_id": "run-b", "note": "n" }] }
        ]});
        let open = vec!["open-1".to_string()];
        let (_, s) = parse_suggestions(&answer.to_string(), &ids(), &open).unwrap();
        assert_eq!(
            s.len(),
            2,
            "a bare same_as for an unknown id has nothing to store"
        );
        assert_eq!(s[0].same_as.as_deref(), Some("open-1"));
        assert_eq!(s[1].same_as, None, "an unknown id is read as new");
        assert_eq!(s[1].title, "t");
    }

    #[test]
    fn earlier_suggestions_show_open_ids_and_group_by_status() {
        let make = |id: &str, status: &str, seen: i32| -> ImprovementSuggestion {
            serde_json::from_value(json!({
                "id": id, "review_id": "r", "workflow": "w", "project": "p",
                "target": "repo", "category": "check", "node": null,
                "title": format!("title {id}"), "problem": "p", "change": "c",
                "evidence": [], "status": status, "applied_run_id": null,
                "seen_count": seen, "last_seen_at": "2026-11-07T00:00:00Z",
                "created_at": "2026-11-07T00:00:00Z", "updated_at": "2026-11-07T00:00:00Z"
            }))
            .unwrap()
        };
        let text = earlier_suggestions(&[
            make("s-open", "new", 3),
            make("s-no", "dismissed", 1),
            make("s-done", "applied", 1),
        ]);
        assert!(
            text.contains("`s-open` — title s-open (seen in 3 reviews)"),
            "{text}"
        );
        assert!(text.contains("same_as"));
        assert!(text.contains("### Dismissed") && text.contains("title s-no"));
        assert!(!text.contains("`s-no`"), "only open ones need an id");
        assert!(text.contains("### Applied") && text.contains("title s-done"));
        assert!(earlier_suggestions(&[]).is_empty());
    }

    #[test]
    fn answers_in_prose_or_fences_still_parse() {
        let body = r#"{"suggestions":[{"title":"t","target":"repo","category":"skill","problem":"p","change":"c","evidence":[{"run_id":"run-b","note":"n"}]}]}"#;
        for text in [
            format!("```json\n{body}\n```"),
            format!("Here you go:\n{body}\nThanks."),
        ] {
            let (_, s) = parse_suggestions(&text, &ids(), &[]).unwrap();
            assert_eq!(s.len(), 1, "{text}");
        }
        assert!(parse_suggestions("no json here", &ids(), &[]).is_err());
    }

    #[test]
    fn categories_decide_the_target() {
        let answer = json!({ "suggestions": [
            { "title": "a", "target": "repo", "category": "workflow", "node": " implement ",
              "problem": "p", "change": "c", "evidence": [{ "run_id": "run-a", "note": "n" }] },
            { "title": "b", "target": "workflow", "category": "nonsense",
              "problem": "p", "change": "c", "evidence": [{ "run_id": "run-a", "note": "n" }] },
            { "title": "c", "target": "workflow", "category": "check",
              "problem": "p", "change": "c", "evidence": [{ "run_id": "run-a", "note": "n" }] }
        ]});
        let (_, s) = parse_suggestions(&answer.to_string(), &ids(), &[]).unwrap();
        assert_eq!(
            (s[0].target.as_str(), s[0].node.as_deref()),
            ("workflow", Some("implement"))
        );
        assert_eq!(
            (s[1].target.as_str(), s[1].category.as_str()),
            ("workflow", "workflow")
        );
        assert_eq!(
            (s[2].target.as_str(), s[2].category.as_str()),
            ("repo", "check")
        );
    }

    fn node(id: &str, status: &str, output: &str) -> harness_persist::PersistedNode {
        serde_json::from_value(json!({
            "node_id": id, "ordinal": 0, "status": status, "provider": "claude",
            "model": "sonnet", "output": output, "iterations": 1, "converged": null,
            "note": null, "input_tokens": 10, "output_tokens": 5, "cache_read": null,
            "cache_write": null, "started_at": null, "ended_at": null,
            "artifact_content": null, "error_count": 0
        }))
        .expect("node")
    }

    fn event(node: &str, kind: &str, text: &str, err: bool) -> ActivityEvent {
        serde_json::from_value(json!({
            "id": 1, "node_id": node, "kind": kind, "text": text,
            "detail": if err { Some("ENOENT: no such file") } else { None },
            "tool_id": null, "is_error": err, "created_at": "2026-11-07T00:00:00Z"
        }))
        .expect("event")
    }

    #[test]
    fn a_run_digest_shows_repeats_failures_and_the_end_of_the_output() {
        let run: harness_persist::RunSummary = serde_json::from_value(json!({
            "id": "run-a", "workflow_name": "task", "title": "AIH-1 Fix", "description": null,
            "status": "failed", "project": "p", "node_count": 1,
            "recorded_at": "2026-11-07T00:00:00Z", "started_at": null, "ended_at": null,
            "ab_pair_id": null, "ab_arm": null, "ab_label": null, "triggered_by": null,
            "trigger_source": null, "trigger_actor": null
        }))
        .expect("run");
        let detail = RunDetail {
            run,
            nodes: vec![node("implement", "failed", &"x".repeat(5000))],
            graph: Vec::new(),
        };
        let mut activity = vec![event("implement", "tool", "Read src/db.rs", false); 4];
        activity.push(event("implement", "tool_result", "", true));
        activity.push(event("implement", "tool_result", "", true));
        let d = run_digest(&detail, &activity);
        assert!(d.contains("## Run `run-a` — AIH-1 Fix (failed)"), "{d}");
        assert!(d.contains("repeated ×4: Read src/db.rs"), "{d}");
        assert_eq!(
            d.matches("failed: ENOENT").count(),
            1,
            "the same failure is one line"
        );
        assert!(d.contains("…x"), "a long output is cut to its end");
        assert!(d.len() < FAILED_OUTPUT_TAIL + 1000);
    }

    #[test]
    fn the_apply_task_carries_the_change_and_its_reason() {
        let s: ImprovementSuggestion = serde_json::from_value(json!({
            "id": "s1", "review_id": "r1", "workflow": "task", "project": "p",
            "target": "repo", "category": "navigation", "node": null,
            "title": "Point at migrations", "problem": "4 of 10 runs searched for them",
            "change": "Add `db/migrations` to AGENTS.md under Layout.",
            "evidence": [{ "run_id": "run-a", "note": "n" }],
            "status": "new", "applied_run_id": null, "seen_count": 1,
            "last_seen_at": "2026-11-07T00:00:00Z",
            "created_at": "2026-11-07T00:00:00Z", "updated_at": "2026-11-07T00:00:00Z"
        }))
        .expect("suggestion");
        let task = apply_task(&s);
        assert!(task.contains("## Point at migrations"));
        assert!(task.contains("Add `db/migrations`"));
        assert!(task.contains("4 of 10 runs"));
        assert!(task.contains("`run-a`"));
    }
}

// ── MCP ─────────────────────────────────────────────────────────────────────

pub(crate) const IMPROVEMENT_TOOLS: &[&str] = &["improvements_list", "improvements_act"];

pub(crate) fn mcp_tool_defs() -> Vec<Value> {
    vec![
        json!({
            "name": "improvements_list",
            "description": "Improvement suggestions: what a workflow's recent runs in a project suggest changing — mostly in the project's repo (`CLAUDE.md` / `AGENTS.md`, skills, lint rules and hooks, a reviewers' standards file), sometimes in the workflow. Each has `id`, `target` (repo | workflow), `category`, `title`, `problem`, `change` (a self-contained task), `evidence` (the runs that show it) and `status` (new | applied | dismissed). Also returns, per workflow that has improvements switched on, how many runs each project has towards its next review. Read-only.",
            "inputSchema": {
                "type": "object",
                "additionalProperties": false,
                "properties": {
                    "workflow": { "type": "string" },
                    "project": { "type": "string" },
                    "status": { "type": "string", "enum": ["new", "applied", "dismissed"] }
                },
            }
        }),
        json!({
            "name": "improvements_act",
            "description": "Act on improvements. `review`: review a workflow's latest runs in a project now (needs `workflow` and `project`; the workflow must have `improvements:` switched on). `apply`: start a run that makes a repo suggestion's change as a pull request (needs `id`; optional `workflow` to carry it out with, default the project's own). `dismiss` / `reopen`: by `id`. `mark_applied`: by `id`, for a workflow suggestion you made in the editor.",
            "inputSchema": {
                "type": "object",
                "additionalProperties": false,
                "properties": {
                    "action": { "type": "string", "enum": ["review", "apply", "dismiss", "reopen", "mark_applied"] },
                    "id": { "type": "string" },
                    "workflow": { "type": "string" },
                    "project": { "type": "string" }
                },
                "required": ["action"],
            }
        }),
    ]
}

pub(crate) async fn mcp_call(
    state: &Arc<RunsState>,
    actor: &super::runs_routes::TriggerInfo,
    name: &str,
    args: &Value,
) -> Result<(String, Value), String> {
    let text = |k: &str| {
        args.get(k)
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(str::to_string)
    };
    match name {
        "improvements_list" => {
            let improvements = state.improvement_store().await?;
            let filter = SuggestionFilter {
                workflow: text("workflow"),
                project: text("project"),
                status: text("status"),
            };
            let suggestions = improvements
                .list_suggestions(&filter)
                .await
                .map_err(|e| e.to_string())?;
            let workflows = overview(state).await.map_err(|(_, m)| m)?;
            Ok((
                format!("{} suggestion(s)", suggestions.len()),
                json!({ "suggestions": suggestions, "workflows": workflows }),
            ))
        }
        "improvements_act" => {
            let action = text("action").unwrap_or_default();
            let need_id = || text("id").ok_or_else(|| format!("`{action}` needs `id`"));
            match action.as_str() {
                "review" => {
                    let (Some(w), Some(p)) = (text("workflow"), text("project")) else {
                        return Err("`review` needs `workflow` and `project`".into());
                    };
                    let v = review_now_inner(state, &w, &p).await.map_err(|(_, m)| m)?;
                    let summary = if v.get("started").is_some() {
                        format!("started a review of {w} in {p}")
                    } else {
                        format!("a review of {w} in {p} is already running")
                    };
                    Ok((summary, v))
                }
                "apply" => {
                    let id = need_id()?;
                    let (s, run_id) = apply(state, &id, text("workflow"), actor)
                        .await
                        .map_err(|(_, m)| m)?;
                    Ok((
                        format!("started run {run_id} to apply \"{}\"", s.title),
                        json!({ "suggestion": s, "run_id": run_id }),
                    ))
                }
                "dismiss" | "reopen" | "mark_applied" => {
                    let id = need_id()?;
                    let status = match action.as_str() {
                        "dismiss" => SUGGESTION_DISMISSED,
                        "reopen" => SUGGESTION_NEW,
                        _ => SUGGESTION_APPLIED,
                    };
                    let s = set_status(state, &id, status).await.map_err(|(_, m)| m)?;
                    Ok((format!("\"{}\" is now {status}", s.title), json!(s)))
                }
                other => Err(format!(
                    "`action` is review, apply, dismiss, reopen or mark_applied, not `{other}`"
                )),
            }
        }
        other => Err(format!("unknown improvements tool `{other}`")),
    }
}
