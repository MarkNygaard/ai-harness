//! Learned project facts: what a project's runs keep failing on, written down
//! once and told to every later run there.
//!
//! The pieces:
//! - [`on_run_finished`] — after every run of a project: settle its facts
//!   (forming → active once their failures were seen in enough runs; active →
//!   retired when those failures keep happening anyway), then start a
//!   `learn-facts` run if failures recur that no fact covers yet. For a
//!   `learn-facts` run itself, store what it proposed.
//! - `GET /api/run/facts/evidence` — what that run reads, fetched with its run
//!   grant.
//! - [`facts_env`] — the live facts as the env var every run of the project
//!   carries; the runner puts them at the top of each prompt.
//! - The HTTP routes behind the project page's "Learned facts" card.
//!
//! Whether a fact reaches prompts is decided here by counting runs, never by
//! the model that proposed it.

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::sync::Arc;

use axum::extract::{Extension, Path as AxumPath};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::Json;
use chrono::{DateTime, Utc};
use harness_persist::{
    error_fingerprint, ErrorLine, FactReview, ProjectFact, RunDetail, ACTIVE_AFTER_RUNS,
    FACT_ACTIVE, FACT_FORMING, FACT_LEARNED, FACT_RETIRED, FACT_REVIEW_DONE, FACT_REVIEW_FAILED,
    FACT_REVIEW_RUNNING,
};
use harness_runner::FACTS_WORKFLOW;
use serde::Deserialize;
use serde_json::{json, Value};

use super::runs_routes::{start_run, CreateRunRequest, RunsState};

/// `trigger_source` on a `learn-facts` run. Also what lets the server, and
/// only the server, start it.
pub(crate) const SOURCE_FACTS: &str = "facts";

/// How far back failures count.
const WINDOW_DAYS: i32 = 30;
/// Failing tool calls read per check, newest first.
const SCAN_LIMIT: i64 = 5_000;
/// A failure is worth a review once this many runs hit it.
const REVIEW_AFTER_RUNS: usize = 2;
/// Failures shown to one review.
const FAILURES_PER_REVIEW: usize = 12;
/// A failure a review was shown isn't shown again for this long: the
/// reviewer may have judged it not worth a fact.
const REVIEW_AGAIN_DAYS: i32 = 14;
/// After a failed review, wait this long before the next.
const RETRY_AFTER_FAILURE_MINS: i64 = 60;
/// A review whose run never reported back is given up after this long.
const STALE_REVIEW_HOURS: i64 = 6;
/// Facts kept from one review, and the length of each.
const FACTS_PER_REVIEW: usize = 5;
const FACT_MAX_CHARS: usize = 300;
/// "What the agent did next", per sample run of a failure.
const NEXT_LINES: usize = 8;
const SAMPLE_RUNS: usize = 2;

type Refusal = (StatusCode, String);

fn err(status: StatusCode, msg: impl Into<String>) -> Response {
    (status, Json(json!({ "error": msg.into() }))).into_response()
}

// ── The failures, indexed ───────────────────────────────────────────────────

/// One recurring failure: every run that hit it, and when it first did.
#[derive(Debug, Default)]
pub(crate) struct Failure {
    /// run id → the first time that run hit it.
    pub runs: BTreeMap<String, DateTime<Utc>>,
    /// node id → times.
    pub nodes: HashMap<String, usize>,
    /// The newest message, verbatim.
    pub sample: String,
    /// One line per run (the first), newest run first: where to read what
    /// happened next.
    pub lines: Vec<ErrorLine>,
}

/// A project's failing tool calls, by fingerprint.
pub(crate) fn index(lines: &[ErrorLine]) -> BTreeMap<String, Failure> {
    let mut out: BTreeMap<String, Failure> = BTreeMap::new();
    // Lines arrive newest first.
    for line in lines {
        let f = out.entry(error_fingerprint(&line.message)).or_default();
        if f.sample.is_empty() {
            f.sample = line.message.clone();
        }
        *f.nodes.entry(line.node_id.clone()).or_default() += 1;
        match f.runs.get_mut(&line.run_id) {
            Some(first) => {
                if line.created_at < *first {
                    *first = line.created_at;
                }
            }
            None => {
                f.runs.insert(line.run_id.clone(), line.created_at);
                f.lines.push(line.clone());
            }
        }
    }
    out
}

/// The runs that hit any of `fingerprints`, optionally only those that first
/// hit them after `after`.
fn runs_hitting(
    fingerprints: &[String],
    failures: &BTreeMap<String, Failure>,
    after: Option<DateTime<Utc>>,
) -> BTreeSet<String> {
    fingerprints
        .iter()
        .filter_map(|fp| failures.get(fp))
        .flat_map(|f| f.runs.iter())
        .filter(|(_, at)| after.is_none_or(|t| **at > t))
        .map(|(run, _)| run.clone())
        .collect()
}

/// What to do with a fact, given the failures.
#[derive(Debug, PartialEq)]
pub(crate) enum Verdict {
    Activate,
    Retire(String),
    Keep,
}

/// A learned fact goes live once its failures were seen in
/// [`ACTIVE_AFTER_RUNS`] runs, and is retired when they show up in that many
/// runs after it went live: it is in every prompt and they happen anyway.
/// A fact a person wrote is theirs to remove.
pub(crate) fn judge(fact: &ProjectFact, failures: &BTreeMap<String, Failure>) -> Verdict {
    if fact.source != FACT_LEARNED || fact.fingerprints.is_empty() {
        return Verdict::Keep;
    }
    match fact.status.as_str() {
        FACT_FORMING => {
            if runs_hitting(&fact.fingerprints, failures, None).len() >= ACTIVE_AFTER_RUNS {
                Verdict::Activate
            } else {
                Verdict::Keep
            }
        }
        FACT_ACTIVE => {
            let since = fact.activated_at.unwrap_or(fact.created_at);
            let after = runs_hitting(&fact.fingerprints, failures, Some(since)).len();
            if after >= ACTIVE_AFTER_RUNS {
                Verdict::Retire(format!(
                    "its failures happened in {after} runs after it went live, so it isn't helping"
                ))
            } else {
                Verdict::Keep
            }
        }
        _ => Verdict::Keep,
    }
}

/// Failures recurring in enough runs that no fact covers, in any status (a
/// retired fact's failures are not learned again), most runs first.
pub(crate) fn uncovered(
    failures: &BTreeMap<String, Failure>,
    facts: &[ProjectFact],
) -> Vec<String> {
    let covered: BTreeSet<&String> = facts.iter().flat_map(|f| f.fingerprints.iter()).collect();
    let mut out: Vec<(&String, usize)> = failures
        .iter()
        .filter(|(fp, f)| f.runs.len() >= REVIEW_AFTER_RUNS && !covered.contains(fp))
        .map(|(fp, f)| (fp, f.runs.len()))
        .collect();
    out.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(b.0)));
    out.into_iter().map(|(fp, _)| fp.clone()).collect()
}

/// The failures a new review should be shown, if one is due: none while one
/// is running (unless it is stale) or a recent one failed, and none it would
/// see again too soon.
pub(crate) fn due(uncovered: &[String], recent: &[FactReview], now: DateTime<Utc>) -> Vec<String> {
    for r in recent {
        let age = now.signed_duration_since(r.created_at);
        if r.status == FACT_REVIEW_RUNNING && age.num_hours() < STALE_REVIEW_HOURS {
            return Vec::new();
        }
        if r.status == FACT_REVIEW_FAILED
            && r.finished_at.unwrap_or(r.created_at)
                > now - chrono::Duration::minutes(RETRY_AFTER_FAILURE_MINS)
        {
            return Vec::new();
        }
    }
    let seen: BTreeSet<&String> = recent
        .iter()
        .filter(|r| r.status == FACT_REVIEW_DONE)
        .flat_map(|r| r.fingerprints.iter())
        .collect();
    uncovered
        .iter()
        .filter(|fp| !seen.contains(fp))
        .take(FAILURES_PER_REVIEW)
        .cloned()
        .collect()
}

// ── When a run ends ─────────────────────────────────────────────────────────

/// Called when any run ends (from the improvements hook, which every run
/// passes through).
pub(crate) async fn on_run_finished(state: &Arc<RunsState>, detail: &RunDetail) {
    if detail.run.workflow_name == FACTS_WORKFLOW {
        finish_review_run(state, detail).await;
        return;
    }
    if harness_runner::INTERNAL_WORKFLOWS.contains(&detail.run.workflow_name.as_str()) {
        return;
    }
    let Some(project) = detail.run.project.clone() else {
        return;
    };
    if let Err(e) = settle_and_learn(state, &project).await {
        tracing::warn!("facts: {project} — {e}");
    }
}

/// Settle the project's facts against its failures, then start a review if
/// one is due.
async fn settle_and_learn(state: &Arc<RunsState>, project: &str) -> Result<(), String> {
    let facts_store = state.fact_store().await?;
    let runs = state.store().await?;
    let lines = runs
        .project_error_lines(project, WINDOW_DAYS, SCAN_LIMIT)
        .await
        .map_err(|e| e.to_string())?;
    let failures = index(&lines);
    let facts = facts_store.list(project).await.map_err(|e| e.to_string())?;
    for fact in &facts {
        match judge(fact, &failures) {
            Verdict::Activate => facts_store.activate(&fact.id).await,
            Verdict::Retire(why) => facts_store.retire(&fact.id, &why).await,
            Verdict::Keep => Ok(()),
        }
        .map_err(|e| e.to_string())?;
    }

    let recent = facts_store
        .recent_reviews(project, REVIEW_AGAIN_DAYS)
        .await
        .map_err(|e| e.to_string())?;
    // A review that never reported back is closed, so it stops blocking.
    for r in &recent {
        if r.status == FACT_REVIEW_RUNNING
            && Utc::now().signed_duration_since(r.created_at).num_hours() >= STALE_REVIEW_HOURS
        {
            let _ = facts_store
                .finish_review(
                    &r.id,
                    FACT_REVIEW_FAILED,
                    Some("the run never reported back"),
                )
                .await;
        }
    }
    let show = due(&uncovered(&failures, &facts), &recent, Utc::now());
    if show.is_empty() {
        return Ok(());
    }
    let Some(review) = facts_store
        .begin_review(project, &show)
        .await
        .map_err(|e| e.to_string())?
    else {
        return Ok(()); // another finishing run got there first
    };
    let req = CreateRunRequest {
        triggered_by: None,
        trigger_source: Some(SOURCE_FACTS.to_string()),
        trigger_actor: Some("Learned facts".to_string()),
        workflow: FACTS_WORKFLOW.to_string(),
        title: Some(format!("Learn facts — {} recurring failures", show.len())),
        description: format!(
            "Turn {} recurring failures in `{project}` into facts (review {}).",
            show.len(),
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
        Ok(run_id) => facts_store
            .set_review_run(&review.id, &run_id)
            .await
            .map_err(|e| e.to_string()),
        Err((_, message)) => {
            let _ = facts_store
                .finish_review(&review.id, FACT_REVIEW_FAILED, Some(&message))
                .await;
            Err(format!("could not start a review: {message}"))
        }
    }
}

/// A `learn-facts` run ended: store what it proposed.
async fn finish_review_run(state: &Arc<RunsState>, detail: &RunDetail) {
    let Ok(facts_store) = state.fact_store().await else {
        return;
    };
    let Ok(Some(review)) = facts_store.review_for_run(&detail.run.id).await else {
        return;
    };
    if review.status != FACT_REVIEW_RUNNING {
        return;
    }
    let outcome = match detail.run.status.as_str() {
        "completed" => {
            let output = detail
                .nodes
                .iter()
                .find(|n| n.node_id == "learn")
                .map(|n| n.output.as_str())
                .unwrap_or_default();
            match parse_answer(output, &review.fingerprints) {
                Ok(proposed) => store_proposed(state, &review, proposed).await,
                Err(e) => Err(e),
            }
        }
        other => Err(format!("the run {other}")),
    };
    let stored = match outcome {
        Ok(()) => {
            facts_store
                .finish_review(&review.id, FACT_REVIEW_DONE, None)
                .await
        }
        Err(e) => {
            facts_store
                .finish_review(&review.id, FACT_REVIEW_FAILED, Some(&e))
                .await
        }
    };
    if let Err(e) = stored {
        tracing::warn!(
            "facts: review {} — could not store the result: {e}",
            review.id
        );
    }
}

async fn store_proposed(
    state: &Arc<RunsState>,
    review: &FactReview,
    proposed: Vec<Proposed>,
) -> Result<(), String> {
    let facts_store = state.fact_store().await?;
    let runs = state.store().await?;
    let failures = index(
        &runs
            .project_error_lines(&review.project, WINDOW_DAYS, SCAN_LIMIT)
            .await
            .map_err(|e| e.to_string())?,
    );
    let known = facts_store
        .list(&review.project)
        .await
        .map_err(|e| e.to_string())?;
    for p in proposed {
        let run_ids: Vec<String> = runs_hitting(&p.fingerprints, &failures, None)
            .into_iter()
            .collect();
        let same = p.same_as.as_deref().and_then(|id| {
            known
                .iter()
                .find(|f| f.id == id && f.status != FACT_RETIRED)
        });
        let fact_id = match same {
            Some(f) => {
                facts_store
                    .extend(&f.id, &p.fingerprints, &run_ids)
                    .await
                    .map_err(|e| e.to_string())?;
                f.id.clone()
            }
            None if !p.text.is_empty() => {
                facts_store
                    .add_learned(&review.project, &p.text, &p.fingerprints, &run_ids)
                    .await
                    .map_err(|e| e.to_string())?
                    .id
            }
            None => continue,
        };
        // The evidence may already be enough: live from the next run.
        if let Ok(Some(fact)) = facts_store.get(&fact_id).await {
            if judge(&fact, &failures) == Verdict::Activate {
                let _ = facts_store.activate(&fact.id).await;
            }
        }
    }
    Ok(())
}

// ── Reading the run's answer ────────────────────────────────────────────────

#[derive(Debug, PartialEq)]
pub(crate) struct Proposed {
    pub text: String,
    pub fingerprints: Vec<String>,
    pub same_as: Option<String>,
}

#[derive(Debug, Deserialize)]
struct RawAnswer {
    #[serde(default)]
    facts: Vec<RawFact>,
}

#[derive(Debug, Deserialize)]
struct RawFact {
    #[serde(default)]
    text: String,
    #[serde(default)]
    failures: Vec<String>,
    #[serde(default)]
    same_as: Option<String>,
}

/// The facts in a `learn-facts` answer. Labels (`F1`, …) are turned back into
/// the fingerprints the review was shown, in order; a fact naming none of
/// them is dropped, since nothing could ever confirm it.
pub(crate) fn parse_answer(text: &str, shown: &[String]) -> Result<Vec<Proposed>, String> {
    let value = extract_json(text).ok_or("the answer held no JSON")?;
    let raw: RawAnswer =
        serde_json::from_value(value).map_err(|e| format!("the answer's JSON: {e}"))?;
    let label = |l: &str| -> Option<String> {
        let n: usize = l.trim().trim_start_matches(['F', 'f']).parse().ok()?;
        shown.get(n.checked_sub(1)?).cloned()
    };
    Ok(raw
        .facts
        .into_iter()
        .filter_map(|f| {
            let mut fps: Vec<String> = f.failures.iter().filter_map(|l| label(l)).collect();
            fps.dedup();
            if fps.is_empty() {
                return None;
            }
            let text = clip_fact(&f.text);
            let same_as = f.same_as.filter(|s| !s.trim().is_empty());
            (!text.is_empty() || same_as.is_some()).then_some(Proposed {
                text,
                fingerprints: fps,
                same_as,
            })
        })
        .take(FACTS_PER_REVIEW)
        .collect())
}

fn clip_fact(s: &str) -> String {
    let s = s.split_whitespace().collect::<Vec<_>>().join(" ");
    if s.chars().count() <= FACT_MAX_CHARS {
        s
    } else {
        s.chars().take(FACT_MAX_CHARS).collect()
    }
}

/// The JSON object in an agent's answer: all of it, or the outermost `{…}`.
fn extract_json(text: &str) -> Option<Value> {
    let t = text.trim();
    if let Ok(v) = serde_json::from_str::<Value>(t) {
        return Some(v);
    }
    let start = t.find('{')?;
    let end = t.rfind('}')?;
    serde_json::from_str(&t[start..=end]).ok()
}

// ── The evidence a review reads ─────────────────────────────────────────────

fn clip(s: &str, max: usize) -> String {
    let s = s.split_whitespace().collect::<Vec<_>>().join(" ");
    if s.chars().count() <= max {
        s
    } else {
        format!("{}…", s.chars().take(max).collect::<String>())
    }
}

/// The lines after a failure, in its own step: what the agent tried next.
pub(crate) fn next_lines(
    failure: &ErrorLine,
    after: &[harness_persist::ActivityEvent],
) -> Vec<String> {
    after
        .iter()
        .filter(|a| a.node_id == failure.node_id && a.id > failure.id && a.kind != "text")
        .take(NEXT_LINES)
        .map(|a| {
            let mark = match (a.kind.as_str(), a.is_error) {
                ("tool_result", true) => "✗ ",
                ("tool_result", false) => "✓ ",
                _ => "→ ",
            };
            let body = match a.detail.as_deref().filter(|d| !d.trim().is_empty()) {
                Some(d) if a.text.trim().is_empty() => clip(d, 220),
                Some(d) => format!("{} — {}", clip(&a.text, 60), clip(d, 200)),
                None => clip(&a.text, 220),
            };
            format!("{mark}{body}")
        })
        .collect()
}

async fn evidence(state: &Arc<RunsState>, review: &FactReview) -> Result<String, Refusal> {
    let unavailable = |e: String| (StatusCode::SERVICE_UNAVAILABLE, e);
    let internal =
        |e: harness_persist::PersistError| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string());
    let runs = state.store().await.map_err(unavailable)?;
    let facts_store = state.fact_store().await.map_err(unavailable)?;
    let failures = index(
        &runs
            .project_error_lines(&review.project, WINDOW_DAYS, SCAN_LIMIT)
            .await
            .map_err(internal)?,
    );

    let mut out = format!(
        "# Recurring failures in `{}`\n\nFrom the last {WINDOW_DAYS} days of runs. Each is a \
         failing tool call that happened in more than one run, with what the agent did next \
         in a couple of them (→ a call, ✓ it worked, ✗ it failed).\n\n",
        review.project
    );
    for (i, fp) in review.fingerprints.iter().enumerate() {
        let Some(f) = failures.get(fp) else {
            continue;
        };
        let mut nodes: Vec<(&String, &usize)> = f.nodes.iter().collect();
        nodes.sort_by(|a, b| b.1.cmp(a.1).then_with(|| a.0.cmp(b.0)));
        let nodes: Vec<&str> = nodes.iter().take(5).map(|(n, _)| n.as_str()).collect();
        out.push_str(&format!(
            "## F{}: in {} runs, in {}\n\n```\n{}\n```\n\n",
            i + 1,
            f.runs.len(),
            nodes.join(", "),
            clip(&f.sample, 600)
        ));
        for line in f.lines.iter().take(SAMPLE_RUNS) {
            let after = runs
                .activity_since(&line.run_id, line.id, 80)
                .await
                .map_err(internal)?;
            let next = next_lines(line, &after);
            if next.is_empty() {
                continue;
            }
            out.push_str(&format!("What came next in run `{}`:\n\n", line.run_id));
            for l in next {
                out.push_str(&format!("- {l}\n"));
            }
            out.push('\n');
        }
    }

    let known = facts_store.list(&review.project).await.map_err(internal)?;
    out.push_str("# Facts already known\n\n");
    if known.is_empty() {
        out.push_str("None yet.\n");
    }
    for f in &known {
        let why = match (f.status.as_str(), f.retire_reason.as_deref()) {
            (FACT_RETIRED, Some(r)) => format!("retired: {r}"),
            (s, _) => s.to_string(),
        };
        out.push_str(&format!("- `{}` ({why}): {}\n", f.id, f.text));
    }
    Ok(out)
}

/// `GET /api/run/facts/evidence` — what a `learn-facts` run reads. Authorized
/// by the run's own grant: only the review that run belongs to is served.
pub async fn get_evidence(
    Extension(state): Extension<Arc<RunsState>>,
    headers: HeaderMap,
) -> Response {
    let grant = match super::run_linear_routes::grant(&headers) {
        Ok(g) => g,
        Err((s, m)) => return err(s, m),
    };
    let Ok(facts_store) = state.fact_store().await else {
        return err(
            StatusCode::SERVICE_UNAVAILABLE,
            "learned facts are unavailable",
        );
    };
    let review = match facts_store.review_for_run(&grant.run_id).await {
        Ok(Some(r)) if r.project == grant.project => r,
        Ok(_) => return err(StatusCode::NOT_FOUND, "this run is not a learn-facts run"),
        Err(e) => return err(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()),
    };
    match evidence(&state, &review).await {
        Ok(text) => ([("content-type", "text/markdown; charset=utf-8")], text).into_response(),
        Err((s, m)) => err(s, m),
    }
}

// ── Reaching the runs ───────────────────────────────────────────────────────

/// The live facts of `project`, as the value of
/// [`harness_runner::PROJECT_FACTS_ENV`]. `None` when there are none (or the
/// store is down: a run without facts is a run as before).
pub(crate) async fn facts_env(state: &Arc<RunsState>, project: &str) -> Option<String> {
    let facts = state.fact_store().await.ok()?.active(project).await.ok()?;
    if facts.is_empty() {
        return None;
    }
    let texts: Vec<&str> = facts.iter().map(|f| f.text.as_str()).collect();
    serde_json::to_string(&texts).ok()
}

// ── The project page ────────────────────────────────────────────────────────

/// A fact as the project page shows it: with how many runs back it up.
fn view(f: &ProjectFact, failures: &BTreeMap<String, Failure>) -> Value {
    let runs = runs_hitting(&f.fingerprints, failures, None).len();
    let mut v = serde_json::to_value(f).unwrap_or_default();
    v["runs"] = json!(runs);
    v["needed"] = json!(ACTIVE_AFTER_RUNS);
    v
}

async fn project_known(state: &Arc<RunsState>, name: &str) -> Result<(), Refusal> {
    let store = state
        .project_store()
        .await
        .map_err(|e| (StatusCode::SERVICE_UNAVAILABLE, e))?;
    match store.get(name).await {
        Ok(Some(_)) => Ok(()),
        Ok(None) => Err((StatusCode::NOT_FOUND, format!("project `{name}` not found"))),
        Err(e) => Err((StatusCode::INTERNAL_SERVER_ERROR, e.to_string())),
    }
}

/// `GET /api/projects/{name}/facts` — every fact, with its evidence count.
pub async fn list(
    Extension(state): Extension<Arc<RunsState>>,
    AxumPath(name): AxumPath<String>,
) -> Response {
    if let Err((s, m)) = project_known(&state, &name).await {
        return err(s, m);
    }
    let facts_store = match state.fact_store().await {
        Ok(s) => s,
        Err(e) => return err(StatusCode::SERVICE_UNAVAILABLE, e),
    };
    let facts = match facts_store.list(&name).await {
        Ok(f) => f,
        Err(e) => return err(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()),
    };
    let failures = match state.store().await {
        Ok(runs) => index(
            &runs
                .project_error_lines(&name, WINDOW_DAYS, SCAN_LIMIT)
                .await
                .unwrap_or_default(),
        ),
        Err(_) => BTreeMap::new(),
    };
    Json(json!({
        "facts": facts.iter().map(|f| view(f, &failures)).collect::<Vec<_>>(),
    }))
    .into_response()
}

#[derive(Debug, Deserialize)]
pub struct FactBody {
    pub text: String,
}

fn fact_text(body: &FactBody) -> Result<String, Refusal> {
    let text = clip_fact(&body.text);
    if text.is_empty() {
        return Err((StatusCode::BAD_REQUEST, "a fact needs some text".into()));
    }
    Ok(text)
}

/// `POST /api/projects/{name}/facts` — a fact a person wrote, live at once.
pub async fn add(
    Extension(state): Extension<Arc<RunsState>>,
    AxumPath(name): AxumPath<String>,
    Json(body): Json<FactBody>,
) -> Response {
    if let Err((s, m)) = project_known(&state, &name).await {
        return err(s, m);
    }
    let text = match fact_text(&body) {
        Ok(t) => t,
        Err((s, m)) => return err(s, m),
    };
    match state.fact_store().await {
        Ok(s) => match s.add_manual(&name, &text).await {
            Ok(f) => (StatusCode::CREATED, Json(f)).into_response(),
            Err(e) => err(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()),
        },
        Err(e) => err(StatusCode::SERVICE_UNAVAILABLE, e),
    }
}

/// The fact `id` of project `name`, or the response saying why not.
async fn own_fact(state: &Arc<RunsState>, name: &str, id: &str) -> Result<ProjectFact, Refusal> {
    let s = state
        .fact_store()
        .await
        .map_err(|e| (StatusCode::SERVICE_UNAVAILABLE, e))?;
    match s.get(id).await {
        Ok(Some(f)) if f.project == name => Ok(f),
        Ok(_) => Err((StatusCode::NOT_FOUND, "no such fact in this project".into())),
        Err(e) => Err((StatusCode::INTERNAL_SERVER_ERROR, e.to_string())),
    }
}

/// `PUT /api/projects/{name}/facts/{id}` — reword a fact.
pub async fn update(
    Extension(state): Extension<Arc<RunsState>>,
    AxumPath((name, id)): AxumPath<(String, String)>,
    Json(body): Json<FactBody>,
) -> Response {
    let fact = match own_fact(&state, &name, &id).await {
        Ok(f) => f,
        Err((s, m)) => return err(s, m),
    };
    let text = match fact_text(&body) {
        Ok(t) => t,
        Err((s, m)) => return err(s, m),
    };
    match state.fact_store().await {
        Ok(s) => match s.set_text(&fact.id, &text).await {
            Ok(()) => StatusCode::NO_CONTENT.into_response(),
            Err(e) => err(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()),
        },
        Err(e) => err(StatusCode::SERVICE_UNAVAILABLE, e),
    }
}

/// `DELETE /api/projects/{name}/facts/{id}` — remove a fact. It is retired,
/// not deleted, so its failures aren't learned again.
pub async fn remove(
    Extension(state): Extension<Arc<RunsState>>,
    AxumPath((name, id)): AxumPath<(String, String)>,
) -> Response {
    let fact = match own_fact(&state, &name, &id).await {
        Ok(f) => f,
        Err((s, m)) => return err(s, m),
    };
    match state.fact_store().await {
        Ok(s) => match s.retire(&fact.id, "removed by a person").await {
            Ok(()) => StatusCode::NO_CONTENT.into_response(),
            Err(e) => err(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()),
        },
        Err(e) => err(StatusCode::SERVICE_UNAVAILABLE, e),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    fn at(min: i64) -> DateTime<Utc> {
        Utc.with_ymd_and_hms(2026, 11, 1, 12, 0, 0).unwrap() + chrono::Duration::minutes(min)
    }

    fn line(id: i64, run: &str, msg: &str, min: i64) -> ErrorLine {
        ErrorLine {
            id,
            run_id: run.into(),
            node_id: "implement".into(),
            message: msg.into(),
            created_at: at(min),
        }
    }

    fn fact(status: &str, fps: &[&str], activated: Option<i64>) -> ProjectFact {
        ProjectFact {
            id: "f1".into(),
            project: "niles".into(),
            text: "Use `bun run test`.".into(),
            status: status.into(),
            source: FACT_LEARNED.into(),
            fingerprints: fps.iter().map(|s| s.to_string()).collect(),
            run_ids: vec![],
            retire_reason: None,
            created_at: at(0),
            updated_at: at(0),
            activated_at: activated.map(at),
            retired_at: None,
        }
    }

    const MISSING: &str = "bun test: no tests found";

    #[test]
    fn the_same_failure_in_one_run_counts_once() {
        let failures = index(&[
            line(3, "r1", MISSING, 3),
            line(2, "r1", MISSING, 2),
            line(1, "r2", MISSING, 1),
        ]);
        let f = failures.get(&error_fingerprint(MISSING)).unwrap();
        assert_eq!(f.runs.len(), 2);
        assert_eq!(
            f.runs["r1"],
            at(2),
            "a run is dated by when it first hit it"
        );
        assert_eq!(f.lines.len(), 2, "one sample line per run");
    }

    #[test]
    fn a_fact_goes_live_once_three_runs_back_it() {
        let fp = error_fingerprint(MISSING);
        let two = index(&[line(2, "r2", MISSING, 2), line(1, "r1", MISSING, 1)]);
        assert_eq!(
            judge(&fact(FACT_FORMING, &[&fp], None), &two),
            Verdict::Keep
        );
        let three = index(&[
            line(3, "r3", MISSING, 3),
            line(2, "r2", MISSING, 2),
            line(1, "r1", MISSING, 1),
        ]);
        assert_eq!(
            judge(&fact(FACT_FORMING, &[&fp], None), &three),
            Verdict::Activate
        );
    }

    #[test]
    fn a_live_fact_is_retired_when_its_failure_keeps_happening() {
        let fp = error_fingerprint(MISSING);
        // Live from minute 10. Three runs before it don't count against it.
        let before = index(&[
            line(3, "r3", MISSING, 3),
            line(2, "r2", MISSING, 2),
            line(1, "r1", MISSING, 1),
            line(4, "r4", MISSING, 20),
        ]);
        assert_eq!(
            judge(&fact(FACT_ACTIVE, &[&fp], Some(10)), &before),
            Verdict::Keep
        );
        let after = index(&[
            line(4, "r4", MISSING, 20),
            line(5, "r5", MISSING, 30),
            line(6, "r6", MISSING, 40),
        ]);
        assert!(matches!(
            judge(&fact(FACT_ACTIVE, &[&fp], Some(10)), &after),
            Verdict::Retire(_)
        ));
    }

    #[test]
    fn a_fact_a_person_wrote_is_left_alone() {
        let mut f = fact(FACT_ACTIVE, &[], Some(0));
        f.source = harness_persist::FACT_MANUAL.into();
        assert_eq!(judge(&f, &BTreeMap::new()), Verdict::Keep);
    }

    #[test]
    fn only_recurring_failures_no_fact_covers_are_learned() {
        let covered = "cargo: command not found";
        let once = "permission denied: /etc/x";
        let failures = index(&[
            line(1, "r1", MISSING, 1),
            line(2, "r2", MISSING, 2),
            line(3, "r1", covered, 3),
            line(4, "r2", covered, 4),
            line(5, "r1", once, 5),
        ]);
        // Retired facts still cover: their failures aren't learned again.
        let mut retired = fact(FACT_RETIRED, &[&error_fingerprint(covered)], None);
        retired.retire_reason = Some("removed by a person".into());
        assert_eq!(
            uncovered(&failures, &[retired]),
            vec![error_fingerprint(MISSING)]
        );
    }

    fn review(status: &str, fps: &[&str], min_ago: i64, now: DateTime<Utc>) -> FactReview {
        FactReview {
            id: "rv".into(),
            project: "niles".into(),
            run_id: None,
            fingerprints: fps.iter().map(|s| s.to_string()).collect(),
            status: status.into(),
            error: None,
            created_at: now - chrono::Duration::minutes(min_ago),
            finished_at: Some(now - chrono::Duration::minutes(min_ago)),
        }
    }

    #[test]
    fn a_review_is_due_only_for_failures_not_shown_recently() {
        let now = at(1000);
        let fps = vec!["a".to_string(), "b".to_string()];
        assert_eq!(due(&fps, &[], now), fps);
        let shown = review(FACT_REVIEW_DONE, &["a"], 600, now);
        assert_eq!(due(&fps, &[shown], now), vec!["b".to_string()]);
        let running = review(FACT_REVIEW_RUNNING, &[], 10, now);
        assert!(due(&fps, &[running], now).is_empty(), "one at a time");
        let failed = review(FACT_REVIEW_FAILED, &[], 10, now);
        assert!(
            due(&fps, &[failed], now).is_empty(),
            "backs off after a failure"
        );
        let old_failure = review(FACT_REVIEW_FAILED, &[], 120, now);
        assert_eq!(due(&fps, &[old_failure], now), fps);
    }

    #[test]
    fn an_answer_names_failures_by_label() {
        let shown = vec!["fp-a".to_string(), "fp-b".to_string()];
        let answer = r#"Here you go:
        {"facts":[
          {"text":"Run tests with `bun run test`.\n It needs the db.","failures":["F2","F2"]},
          {"text":"","failures":["F1"],"same_as":"fact-9"},
          {"text":"Unanchored","failures":["F7"]},
          {"text":"","failures":["F1"]}
        ]}"#;
        assert_eq!(
            parse_answer(answer, &shown).unwrap(),
            vec![
                Proposed {
                    text: "Run tests with `bun run test`. It needs the db.".into(),
                    fingerprints: vec!["fp-b".into()],
                    same_as: None,
                },
                Proposed {
                    text: String::new(),
                    fingerprints: vec!["fp-a".into()],
                    same_as: Some("fact-9".into()),
                },
            ]
        );
        assert!(parse_answer("no json here", &shown).is_err());
    }

    #[test]
    fn what_came_next_is_read_from_the_failures_own_step() {
        let failure = line(10, "r1", MISSING, 1);
        let ev = |id: i64, node: &str, kind: &str, text: &str, detail: Option<&str>, e: bool| {
            harness_persist::ActivityEvent {
                id,
                node_id: node.into(),
                kind: kind.into(),
                text: text.into(),
                detail: detail.map(str::to_string),
                tool_id: None,
                is_error: e,
                created_at: at(2),
            }
        };
        let after = vec![
            ev(
                11,
                "implement",
                "text",
                "Let me try another way",
                None,
                false,
            ),
            ev(12, "other-step", "tool", "Bash", Some("ls"), false),
            ev(13, "implement", "tool", "Bash", Some("bun run test"), false),
            ev(14, "implement", "tool_result", "", Some("12 passed"), false),
        ];
        assert_eq!(
            next_lines(&failure, &after),
            vec!["→ Bash — bun run test", "✓ 12 passed"]
        );
    }
}
