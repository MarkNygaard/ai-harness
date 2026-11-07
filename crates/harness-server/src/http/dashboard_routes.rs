//! The dashboard's metrics: how the harness has been doing, across projects
//! or for one.
//!
//! `GET /api/dashboard/metrics?days=7|30|90&project=<name>` answers with
//! per-day outcomes, run time and notional cost, the same per workflow, and
//! totals for the period and the one before it (for "vs previous 7 days").
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
use harness_persist::{DashboardRun, RunModelTokens};
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

/// The first UTC day of a `days`-long period ending today.
fn period_start(now: DateTime<Utc>, days: u32) -> NaiveDate {
    now.date_naive() - Duration::days(i64::from(days) - 1)
}

/// Metrics for the `days` up to and including today (UTC), from runs and
/// token rows covering at least twice that, so the previous period is known.
pub(crate) fn aggregate(
    runs: &[DashboardRun],
    tokens: &[RunModelTokens],
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
    let daily = by_day
        .into_iter()
        .map(|(day, (outcomes, secs, cost))| DayMetrics {
            day: day.format("%Y-%m-%d").to_string(),
            outcomes,
            median_secs: median(secs),
            cost_usd: cost,
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

    Metrics {
        days,
        daily,
        workflows,
        totals: totals(&current, &costs),
        previous: totals(&previous, &costs),
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
    Json(aggregate(&runs, &tokens, days, now)).into_response()
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
        let m = aggregate(&[run("a", "w", "t", "completed", 0, 60.0)], &[], 7, now());
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
        let m = aggregate(&runs, &[], 7, now());
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
        let m = aggregate(&runs, &[], 7, now());
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
        let m = aggregate(&runs, &t, 7, now());
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
        let m = aggregate(&runs, &[], 7, now());
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
        let m = aggregate(&runs, &[], 7, now());
        let names: Vec<&str> = m.workflows.iter().map(|w| w.workflow.as_str()).collect();
        assert_eq!(names, ["idea-to-pr", "merge-pr"]);
    }

    #[test]
    fn medians_of_odd_and_even_counts() {
        assert_eq!(median(vec![]), None);
        assert_eq!(median(vec![3.0, 1.0, 2.0]), Some(2.0));
        assert_eq!(median(vec![4.0, 1.0, 2.0, 3.0]), Some(2.5));
    }
}
