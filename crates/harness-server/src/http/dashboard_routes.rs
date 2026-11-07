//! The dashboard's metrics: how the harness has been doing, across projects
//! or for one.
//!
//! `GET /api/dashboard/metrics?days=7|30|90&project=<name>` answers with
//! per-day outcomes, run time and notional cost, the same per workflow, and
//! totals for the period and the one before it (for "vs previous 7 days").
//! The pull requests runs opened (see `pr_tracking`) add PRs opened and
//! merged per day, and how long tasks took from kickoff to PR and PR to merge.
//!
//! Aggregated here from two plain queries rather than in SQL, so the
//! arithmetic — medians, windows, what counts as a task — is tested without a
//! database. A window is a few thousand runs at most.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::sync::Arc;

use axum::extract::{Extension, Query};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::Json;
use chrono::{DateTime, Duration, NaiveDate, Utc};
use harness_persist::{DashboardRun, RunModelTokens, RunPr};
use serde::{Deserialize, Serialize};
use serde_json::json;

use super::runs_routes::RunsState;

/// The periods the dashboard offers. Anything else is clamped into range.
const MAX_DAYS: u32 = 90;

#[derive(Debug, Clone, Default, PartialEq, Serialize)]
pub(crate) struct Outcomes {
    pub completed: u32,
    pub failed: u32,
    pub cancelled: u32,
}

impl Outcomes {
    fn add(&mut self, status: &str) {
        match status {
            "completed" => self.completed += 1,
            "failed" => self.failed += 1,
            _ => self.cancelled += 1,
        }
    }

    fn total(&self) -> u32 {
        self.completed + self.failed + self.cancelled
    }
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub(crate) struct DayMetrics {
    /// UTC day, `YYYY-MM-DD`.
    pub day: String,
    #[serde(flatten)]
    pub outcomes: Outcomes,
    /// Median wall-clock of the runs that finished that day; `None` if none.
    pub median_secs: Option<f64>,
    /// API-equivalent cost of their tokens, in USD.
    pub cost_usd: f64,
    /// Pull requests opened that day, and merged that day.
    pub prs_opened: u32,
    pub prs_merged: u32,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub(crate) struct WorkflowMetrics {
    pub workflow: String,
    #[serde(flatten)]
    pub outcomes: Outcomes,
    pub median_secs: Option<f64>,
    pub cost_usd: f64,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize)]
pub(crate) struct Totals {
    pub runs: u32,
    #[serde(flatten)]
    pub outcomes: Outcomes,
    /// Distinct tasks with at least one completed run: runs sharing a title
    /// are one task (a Linear issue built, then reviewed, then merged).
    pub tasks: u32,
    pub median_secs: Option<f64>,
    pub cost_usd: f64,
    #[serde(flatten)]
    pub prs: PrTotals,
}

/// Pull requests over a period.
#[derive(Debug, Clone, Default, PartialEq, Serialize)]
pub(crate) struct PrTotals {
    pub prs_opened: u32,
    pub prs_merged: u32,
    /// The cycle, as medians. Kickoff is a run starting, so the kickoff
    /// times only count PRs the run itself opened.
    ///
    /// Kickoff to merge, over the PRs merged in the period.
    pub median_kickoff_to_merge_secs: Option<f64>,
    /// Kickoff to the PR opening, over the PRs opened in the period.
    pub median_to_pr_secs: Option<f64>,
    /// The PR opening to someone other than its author first reviewing it,
    /// over the PRs first reviewed in the period.
    pub median_to_review_secs: Option<f64>,
    /// That first review to the merge, over the PRs merged in the period.
    pub median_review_to_merge_secs: Option<f64>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub(crate) struct Metrics {
    pub days: u32,
    /// Oldest first, one entry per day of the period, zero-filled.
    pub daily: Vec<DayMetrics>,
    /// Busiest first.
    pub workflows: Vec<WorkflowMetrics>,
    pub totals: Totals,
    /// The same period immediately before, for deltas.
    pub previous: Totals,
}

/// The middle value, or the mean of the middle two.
fn median(mut xs: Vec<f64>) -> Option<f64> {
    if xs.is_empty() {
        return None;
    }
    xs.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    let n = xs.len();
    Some(if n % 2 == 1 {
        xs[n / 2]
    } else {
        (xs[n / 2 - 1] + xs[n / 2]) / 2.0
    })
}

/// A run's duration when it is meaningful: it ran steps and finished on its
/// own. A cancelled run's length says when someone stopped it, not how long
/// the work takes.
fn duration(r: &DashboardRun) -> Option<f64> {
    (r.status != "cancelled")
        .then_some(r.secs)
        .flatten()
        .filter(|s| *s > 0.0)
}

/// Each run's notional cost, from its per-model token usage.
fn run_costs(tokens: &[RunModelTokens]) -> HashMap<&str, f64> {
    let mut out: HashMap<&str, f64> = HashMap::new();
    for t in tokens {
        let rates = harness_runner::models::family_for(&t.model).rates;
        let cost = rates.cost_usd(
            t.input_tokens.max(0) as u64,
            t.output_tokens.max(0) as u64,
            t.cache_read.max(0) as u64,
            t.cache_write.max(0) as u64,
        );
        *out.entry(t.run_id.as_str()).or_default() += cost;
    }
    out
}

fn totals(runs: &[&DashboardRun], costs: &HashMap<&str, f64>) -> Totals {
    let mut t = Totals::default();
    let mut tasks: HashSet<&str> = HashSet::new();
    let mut secs = Vec::new();
    for r in runs {
        t.runs += 1;
        t.outcomes.add(&r.status);
        if r.status == "completed" {
            tasks.insert(
                r.title
                    .as_deref()
                    .filter(|s| !s.trim().is_empty())
                    .unwrap_or(&r.id),
            );
        }
        secs.extend(duration(r));
        t.cost_usd += costs.get(r.id.as_str()).copied().unwrap_or(0.0);
    }
    t.tasks = tasks.len() as u32;
    t.median_secs = median(secs);
    t
}

/// Pull requests opened and merged in `[from, to)` (UTC days), with the
/// cycle medians. A PR opened before its run started was opened by someone
/// else and only worked on by the run, so it has no kickoff times.
fn pr_totals(prs: &[RunPr], from: NaiveDate, to: NaiveDate) -> PrTotals {
    let within = |t: &Option<DateTime<Utc>>| {
        t.map(|t| t.date_naive())
            .is_some_and(|d| d >= from && d < to)
    };
    let secs = |a: DateTime<Utc>, b: DateTime<Utc>| (b - a).num_seconds().max(0) as f64;
    let mut t = PrTotals::default();
    let (mut to_merge, mut to_pr, mut to_review, mut review_to_merge) =
        (Vec::new(), Vec::new(), Vec::new(), Vec::new());
    for pr in prs {
        let kickoff = pr
            .opened_at
            .filter(|opened| *opened >= pr.run_started_at)
            .map(|_| pr.run_started_at);
        if within(&pr.opened_at) {
            t.prs_opened += 1;
            if let (Some(k), Some(opened)) = (kickoff, pr.opened_at) {
                to_pr.push(secs(k, opened));
            }
        }
        if within(&pr.first_review_at) {
            if let (Some(opened), Some(review)) = (pr.opened_at, pr.first_review_at) {
                to_review.push(secs(opened, review));
            }
        }
        if within(&pr.merged_at) {
            t.prs_merged += 1;
            let merged = pr.merged_at.expect("within");
            if let Some(k) = kickoff {
                to_merge.push(secs(k, merged));
            }
            if let Some(review) = pr.first_review_at {
                review_to_merge.push(secs(review, merged));
            }
        }
    }
    t.median_kickoff_to_merge_secs = median(to_merge);
    t.median_to_pr_secs = median(to_pr);
    t.median_to_review_secs = median(to_review);
    t.median_review_to_merge_secs = median(review_to_merge);
    t
}

/// The first UTC day of a `days`-long period ending today.
fn period_start(now: DateTime<Utc>, days: u32) -> NaiveDate {
    now.date_naive() - Duration::days(i64::from(days) - 1)
}

/// Metrics for the `days` up to and including today (UTC), from runs and
/// token rows covering at least twice that, so the previous period is known.
pub(crate) fn aggregate(
    runs: &[DashboardRun],
    tokens: &[RunModelTokens],
    prs: &[RunPr],
    days: u32,
    now: DateTime<Utc>,
) -> Metrics {
    let start = period_start(now, days);
    let prev_start = start - Duration::days(i64::from(days));
    let costs = run_costs(tokens);

    let current: Vec<&DashboardRun> = runs
        .iter()
        .filter(|r| r.recorded_at.date_naive() >= start)
        .collect();
    let previous: Vec<&DashboardRun> = runs
        .iter()
        .filter(|r| {
            let d = r.recorded_at.date_naive();
            d >= prev_start && d < start
        })
        .collect();

    // Per day, zero-filled so the axis is continuous.
    let mut by_day: BTreeMap<NaiveDate, (Outcomes, Vec<f64>, f64)> = BTreeMap::new();
    for i in 0..days {
        by_day.insert(start + Duration::days(i64::from(i)), Default::default());
    }
    for r in &current {
        if let Some(entry) = by_day.get_mut(&r.recorded_at.date_naive()) {
            entry.0.add(&r.status);
            entry.1.extend(duration(r));
            entry.2 += costs.get(r.id.as_str()).copied().unwrap_or(0.0);
        }
    }
    let mut prs_by_day: HashMap<NaiveDate, (u32, u32)> = HashMap::new();
    for pr in prs {
        if let Some(d) = pr.opened_at {
            prs_by_day.entry(d.date_naive()).or_default().0 += 1;
        }
        if let Some(d) = pr.merged_at {
            prs_by_day.entry(d.date_naive()).or_default().1 += 1;
        }
    }
    let daily = by_day
        .into_iter()
        .map(|(day, (outcomes, secs, cost))| {
            let (opened, merged) = prs_by_day.get(&day).copied().unwrap_or_default();
            DayMetrics {
                day: day.format("%Y-%m-%d").to_string(),
                outcomes,
                median_secs: median(secs),
                cost_usd: cost,
                prs_opened: opened,
                prs_merged: merged,
            }
        })
        .collect();

    // Per workflow, busiest first.
    let mut by_workflow: HashMap<&str, (Outcomes, Vec<f64>, f64)> = HashMap::new();
    for r in &current {
        let entry = by_workflow.entry(r.workflow_name.as_str()).or_default();
        entry.0.add(&r.status);
        entry.1.extend(duration(r));
        entry.2 += costs.get(r.id.as_str()).copied().unwrap_or(0.0);
    }
    let mut workflows: Vec<WorkflowMetrics> = by_workflow
        .into_iter()
        .map(|(workflow, (outcomes, secs, cost))| WorkflowMetrics {
            workflow: workflow.to_string(),
            outcomes,
            median_secs: median(secs),
            cost_usd: cost,
        })
        .collect();
    workflows.sort_by(|a, b| {
        b.outcomes
            .total()
            .cmp(&a.outcomes.total())
            .then_with(|| a.workflow.cmp(&b.workflow))
    });

    let end = now.date_naive() + Duration::days(1);
    Metrics {
        days,
        daily,
        workflows,
        totals: Totals {
            prs: pr_totals(prs, start, end),
            ..totals(&current, &costs)
        },
        previous: Totals {
            prs: pr_totals(prs, prev_start, start),
            ..totals(&previous, &costs)
        },
    }
}

#[derive(Debug, Deserialize)]
pub struct MetricsQuery {
    #[serde(default)]
    pub days: Option<u32>,
    #[serde(default)]
    pub project: Option<String>,
}

/// `GET /api/dashboard/metrics?days=&project=`
pub async fn metrics(
    Extension(state): Extension<Arc<RunsState>>,
    Query(q): Query<MetricsQuery>,
) -> Response {
    let days = q.days.unwrap_or(7).clamp(1, MAX_DAYS);
    let project = q
        .project
        .as_deref()
        .map(str::trim)
        .filter(|p| !p.is_empty());
    let store = match state.store().await {
        Ok(s) => s,
        Err(e) => {
            return (StatusCode::SERVICE_UNAVAILABLE, Json(json!({ "error": e }))).into_response()
        }
    };
    let now = Utc::now();
    // Both periods in one read: this one and the one before it.
    let since = (period_start(now, days) - Duration::days(i64::from(days)))
        .and_hms_opt(0, 0, 0)
        .expect("midnight")
        .and_utc();
    // The harness's own reviews are machinery, not work done for anyone.
    let excluded = vec![harness_runner::IMPROVEMENTS_WORKFLOW.to_string()];
    let fail = |e: harness_persist::PersistError| {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({ "error": e.to_string() })),
        )
            .into_response()
    };
    let runs = match store.dashboard_runs(since, project, &excluded).await {
        Ok(r) => r,
        Err(e) => return fail(e),
    };
    let tokens = match store.dashboard_tokens(since, project, &excluded).await {
        Ok(t) => t,
        Err(e) => return fail(e),
    };
    // PRs are an addition to the picture, not a condition for it: with the
    // store unreachable the runs still show, and the PR numbers read zero.
    let prs = match state.pr_store().await {
        Ok(s) => s.dashboard_prs(since, project).await.unwrap_or_else(|e| {
            tracing::warn!("dashboard: could not read pull requests: {e}");
            Vec::new()
        }),
        Err(_) => Vec::new(),
    };
    Json(aggregate(&runs, &tokens, &prs, days, now)).into_response()
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    fn now() -> DateTime<Utc> {
        Utc.with_ymd_and_hms(2026, 11, 7, 12, 0, 0).unwrap()
    }

    fn run(
        id: &str,
        workflow: &str,
        title: &str,
        status: &str,
        days_ago: i64,
        secs: f64,
    ) -> DashboardRun {
        DashboardRun {
            id: id.into(),
            workflow_name: workflow.into(),
            title: (!title.is_empty()).then(|| title.to_string()),
            status: status.into(),
            recorded_at: now() - Duration::days(days_ago),
            secs: Some(secs),
        }
    }

    fn tokens(run_id: &str, input: i64, output: i64) -> RunModelTokens {
        RunModelTokens {
            run_id: run_id.into(),
            model: "opus".into(),
            input_tokens: input,
            output_tokens: output,
            cache_read: 0,
            cache_write: 0,
        }
    }

    #[test]
    fn the_period_is_zero_filled_oldest_first_and_ends_today() {
        let m = aggregate(
            &[run("a", "w", "t", "completed", 0, 60.0)],
            &[],
            &[],
            7,
            now(),
        );
        assert_eq!(m.daily.len(), 7);
        assert_eq!(m.daily.first().unwrap().day, "2026-11-01");
        assert_eq!(m.daily.last().unwrap().day, "2026-11-07");
        assert_eq!(m.daily.last().unwrap().outcomes.completed, 1);
        assert_eq!(m.daily[0].outcomes.total(), 0);
        assert_eq!(m.daily[0].median_secs, None);
    }

    #[test]
    fn runs_sharing_a_title_are_one_task_and_only_completed_ones_count() {
        let runs = [
            run("a", "idea-to-pr", "AIH-1 Fix", "completed", 1, 100.0),
            run("b", "merge-pr", "AIH-1 Fix", "completed", 1, 50.0),
            run("c", "idea-to-pr", "AIH-2 Add", "failed", 2, 300.0),
            run("d", "idea-to-pr", "", "completed", 2, 10.0),
        ];
        let m = aggregate(&runs, &[], &[], 7, now());
        assert_eq!(m.totals.runs, 4);
        assert_eq!(
            m.totals.tasks, 2,
            "AIH-1 once; the untitled run by its id; AIH-2 failed"
        );
        assert_eq!(m.totals.outcomes.failed, 1);
    }

    #[test]
    fn medians_leave_out_cancelled_runs() {
        let runs = [
            run("a", "w", "1", "completed", 0, 100.0),
            run("b", "w", "2", "failed", 0, 300.0),
            run("c", "w", "3", "cancelled", 0, 5.0),
        ];
        let m = aggregate(&runs, &[], &[], 7, now());
        assert_eq!(m.totals.median_secs, Some(200.0));
        assert_eq!(m.daily.last().unwrap().median_secs, Some(200.0));
        assert_eq!(m.totals.outcomes.cancelled, 1);
    }

    #[test]
    fn cost_is_priced_per_model_and_attributed_to_day_and_workflow() {
        let runs = [
            run("a", "idea-to-pr", "1", "completed", 0, 60.0),
            run("b", "merge-pr", "2", "completed", 0, 60.0),
        ];
        // Opus: 1M input at $5 + 1M output at $25.
        let t = [tokens("a", 1_000_000, 1_000_000)];
        let m = aggregate(&runs, &t, &[], 7, now());
        assert!((m.totals.cost_usd - 30.0).abs() < 1e-9);
        assert!((m.daily.last().unwrap().cost_usd - 30.0).abs() < 1e-9);
        let idea = m
            .workflows
            .iter()
            .find(|w| w.workflow == "idea-to-pr")
            .unwrap();
        assert!((idea.cost_usd - 30.0).abs() < 1e-9);
    }

    #[test]
    fn the_previous_period_is_the_same_length_just_before() {
        let runs = [
            run("now", "w", "1", "completed", 0, 60.0),
            run("edge", "w", "2", "completed", 6, 60.0),
            run("prev", "w", "3", "failed", 7, 60.0),
            run("prev2", "w", "4", "completed", 13, 60.0),
            run("too-old", "w", "5", "completed", 14, 60.0),
        ];
        let m = aggregate(&runs, &[], &[], 7, now());
        assert_eq!(m.totals.runs, 2);
        assert_eq!(m.previous.runs, 2);
        assert_eq!(m.previous.outcomes.failed, 1);
    }

    #[test]
    fn workflows_are_busiest_first() {
        let runs = [
            run("a", "merge-pr", "1", "completed", 0, 1.0),
            run("b", "idea-to-pr", "2", "completed", 0, 1.0),
            run("c", "idea-to-pr", "3", "failed", 0, 1.0),
        ];
        let m = aggregate(&runs, &[], &[], 7, now());
        let names: Vec<&str> = m.workflows.iter().map(|w| w.workflow.as_str()).collect();
        assert_eq!(names, ["idea-to-pr", "merge-pr"]);
    }

    fn pr(
        run_hours_ago: i64,
        opened_hours_ago: Option<i64>,
        merged_hours_ago: Option<i64>,
    ) -> RunPr {
        reviewed_pr(run_hours_ago, opened_hours_ago, None, merged_hours_ago)
    }

    fn reviewed_pr(
        run_hours_ago: i64,
        opened_hours_ago: Option<i64>,
        review_hours_ago: Option<i64>,
        merged_hours_ago: Option<i64>,
    ) -> RunPr {
        let at = |h: i64| now() - Duration::hours(h);
        RunPr {
            url: format!("https://github.com/o/r/pull/{run_hours_ago}"),
            run_id: "r".into(),
            project: None,
            workflow: "task".into(),
            run_started_at: at(run_hours_ago),
            state: if merged_hours_ago.is_some() {
                "merged"
            } else {
                "open"
            }
            .into(),
            opened_at: opened_hours_ago.map(at),
            merged_at: merged_hours_ago.map(at),
            first_review_at: review_hours_ago.map(at),
            checked_at: None,
        }
    }

    #[test]
    fn prs_are_counted_on_the_day_they_open_and_the_day_they_merge() {
        let prs = [
            // Kicked off 3h ago, PR 2h ago (1h), merged 1h ago (1h).
            pr(3, Some(2), Some(1)),
            // Kicked off 50h ago, PR 47h ago (3h), still open.
            pr(50, Some(47), None),
            // Opened by a person before the run touched it: no kickoff time.
            pr(10, Some(30), None),
            // Opened in the previous period, merged in this one.
            pr(24 * 9, Some(24 * 8), Some(24 * 2)),
        ];
        let m = aggregate(&[], &[], &prs, 7, now());
        assert_eq!(m.totals.prs.prs_opened, 3);
        assert_eq!(m.totals.prs.prs_merged, 2);
        assert_eq!(
            m.totals.prs.median_to_pr_secs,
            Some(7200.0),
            "1h and 3h; the person's PR is left out"
        );
        assert_eq!(
            m.totals.prs.median_kickoff_to_merge_secs,
            Some((2.0 * 3600.0 + 7.0 * 86400.0) / 2.0),
            "3h-ago kickoff merged 1h ago (2h), 9-days-ago kickoff merged 2 days ago (7d)"
        );
        assert_eq!(m.previous.prs.prs_opened, 1);
        assert_eq!(m.previous.prs.prs_merged, 0);
        let today = m.daily.last().unwrap();
        assert_eq!((today.prs_opened, today.prs_merged), (1, 1));
    }

    #[test]
    fn the_review_splits_the_wait_between_pr_and_merge() {
        // Kickoff 10h ago, PR 8h ago, first review 5h ago, merged 1h ago.
        let prs = [reviewed_pr(10, Some(8), Some(5), Some(1))];
        let m = aggregate(&[], &[], &prs, 7, now());
        let t = &m.totals.prs;
        assert_eq!(t.median_to_pr_secs, Some(2.0 * 3600.0));
        assert_eq!(t.median_to_review_secs, Some(3.0 * 3600.0));
        assert_eq!(t.median_review_to_merge_secs, Some(4.0 * 3600.0));
        assert_eq!(t.median_kickoff_to_merge_secs, Some(9.0 * 3600.0));
    }

    #[test]
    fn medians_of_odd_and_even_counts() {
        assert_eq!(median(vec![]), None);
        assert_eq!(median(vec![3.0, 1.0, 2.0]), Some(2.0));
        assert_eq!(median(vec![4.0, 1.0, 2.0, 3.0]), Some(2.5));
    }
}
