//! **Scheduled runs** — a workflow started on a cron schedule, with no Linear
//! issue behind it ("every Monday 08:00, report on INP from Grafana").
//!
//! - `GET    /api/schedules`           — every schedule (any signed-in user)
//! - `POST   /api/schedules`           — create (administrators)
//! - `GET    /api/schedules/{id}`      — one
//! - `PUT    /api/schedules/{id}`      — change (administrators)
//! - `DELETE /api/schedules/{id}`      — remove (administrators)
//! - `POST   /api/schedules/{id}/run`  — start it now, off-schedule (administrators)
//! - `POST   /api/schedules/preview`   — the next few times an expression fires
//!
//! A schedule is five-field cron read in its own IANA timezone, so "08:00
//! Europe/Copenhagen" stays 08:00 across daylight-saving changes. The next
//! occurrence is stored in UTC, and [`spawn_scheduler`] fires due ones by
//! claiming them with a compare-and-swap — a second replica, or this one
//! after a restart, can't start the same occurrence twice. After downtime a
//! schedule catches up with **one** run, never one per missed slot.
//!
//! A schedule may also name a Linear team. Each run it starts is then tracked,
//! and once the run succeeds its result is filed there as a new issue
//! ([`file_results`]) — the output of the step marked `linear_comment: true`,
//! linking back to the run.

use std::str::FromStr;
use std::sync::Arc;

use axum::extract::{Extension, Path as AxumPath};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::Json;
use chrono::{DateTime, Utc};
use chrono_tz::Tz;
use croner::parser::{CronParser, Seconds, Year};
use croner::Cron;
use harness_persist::{Schedule, ScheduleInput};
use serde::Deserialize;
use serde_json::json;

use super::accounts::{caller_trigger, AdminOnly};
use super::linear_connections::resolve_for_project;
use super::linear_oauth::linear_client;
use super::linear_poller::{flagged_summary, MISSING_RUN_GRACE_MINS};
use super::runs_routes::{start_run, CreateRunRequest, RunsState, SOURCE_SCHEDULE};

/// How often the scheduler looks for due schedules. A schedule fires within
/// this much of its time.
const TICK: std::time::Duration = std::time::Duration::from_secs(30);

/// How many upcoming times the preview returns.
const PREVIEW_COUNT: usize = 5;

/// How many times to try filing a result before giving up on it. Spread over
/// ticks, this rides out a Linear outage of a few minutes.
const MAX_FILING_ATTEMPTS: i32 = 5;

fn err(status: StatusCode, msg: impl Into<String>) -> Response {
    (status, Json(json!({ "error": msg.into() }))).into_response()
}

// ── Cron ───────────────────────────────────────────────────────────────────

/// Parse a five-field cron expression (minute hour day-of-month month
/// day-of-week). Seconds and years are refused: they only make a schedule
/// harder to read, and nothing here runs more often than a minute.
pub(crate) fn parse_cron(expr: &str) -> Result<Cron, String> {
    let fields = expr.split_whitespace().count();
    if fields != 5 {
        return Err(format!(
            "`{expr}` has {fields} fields; use five: minute hour day-of-month month day-of-week \
             (e.g. `0 8 * * 1` for Mondays at 08:00)"
        ));
    }
    CronParser::builder()
        .seconds(Seconds::Disallowed)
        .year(Year::Disallowed)
        .build()
        .parse(expr)
        .map_err(|e| format!("`{expr}` is not a valid cron expression: {e}"))
}

pub(crate) fn parse_timezone(tz: &str) -> Result<Tz, String> {
    Tz::from_str(tz.trim())
        .map_err(|_| format!("`{tz}` is not a timezone (e.g. Europe/Copenhagen)"))
}

/// The first occurrence strictly after `after`, read in `tz`, in UTC.
pub(crate) fn next_after(cron: &Cron, tz: Tz, after: DateTime<Utc>) -> Option<DateTime<Utc>> {
    cron.find_next_occurrence(&after.with_timezone(&tz), false)
        .ok()
        .map(|t| t.with_timezone(&Utc))
}

/// The next `n` occurrences after `after`.
pub(crate) fn upcoming(cron: &Cron, tz: Tz, after: DateTime<Utc>, n: usize) -> Vec<DateTime<Utc>> {
    let mut out = Vec::with_capacity(n);
    let mut t = after;
    while out.len() < n {
        match next_after(cron, tz, t) {
            Some(next) => {
                out.push(next);
                t = next;
            }
            None => break,
        }
    }
    out
}

// ── Requests ───────────────────────────────────────────────────────────────

#[derive(Debug, Deserialize)]
pub struct ScheduleRequest {
    pub name: String,
    pub project: String,
    pub workflow: String,
    pub cron: String,
    pub timezone: String,
    #[serde(default)]
    pub task: String,
    #[serde(default = "enabled_by_default")]
    pub enabled: bool,
    /// File each successful run's result as an issue in this Linear team (an
    /// id from the project's Linear discovery). Absent or blank: don't.
    #[serde(default)]
    pub linear_team_id: Option<String>,
}

fn enabled_by_default() -> bool {
    true
}

/// Check a request against what exists, and work out its next occurrence.
// The error is the HTTP response itself, returned as-is by every caller.
#[allow(clippy::result_large_err)]
async fn validate(
    state: &Arc<RunsState>,
    req: ScheduleRequest,
) -> Result<(ScheduleInput, Option<DateTime<Utc>>), Response> {
    let bad = |m: String| err(StatusCode::BAD_REQUEST, m);
    let name = req.name.trim().to_string();
    if name.is_empty() {
        return Err(bad("give the schedule a name".into()));
    }
    let cron = parse_cron(req.cron.trim()).map_err(bad)?;
    let tz = parse_timezone(&req.timezone).map_err(bad)?;
    let project = req.project.trim().to_string();
    match state.project_store().await {
        Ok(store) => match store.get(&project).await {
            Ok(Some(_)) => {}
            Ok(None) => return Err(bad(format!("no project `{project}`"))),
            Err(e) => return Err(err(StatusCode::INTERNAL_SERVER_ERROR, e.to_string())),
        },
        Err(e) => return Err(err(StatusCode::SERVICE_UNAVAILABLE, e)),
    }
    let workflow = req.workflow.trim().to_string();
    if let Err(e) = harness_runner::authoring::get_workflow(&state.project_root, &workflow) {
        return Err(bad(format!("no workflow `{workflow}`: {e}")));
    }
    let linear_team_id = req
        .linear_team_id
        .map(|t| t.trim().to_string())
        .filter(|t| !t.is_empty());
    if let Some(team) = &linear_team_id {
        check_linear_team(state, &project, team).await?;
    }
    let next = if req.enabled {
        let next = next_after(&cron, tz, Utc::now());
        if next.is_none() {
            return Err(bad(format!("`{}` never fires", req.cron.trim())));
        }
        next
    } else {
        None
    };
    Ok((
        ScheduleInput {
            name,
            project,
            workflow,
            cron: req.cron.trim().to_string(),
            timezone: tz.name().to_string(),
            task: req.task.trim().to_string(),
            enabled: req.enabled,
            linear_team_id,
        },
        next,
    ))
}

/// The team must be one the project's Linear connection can see — caught here
/// rather than the first time a report has nowhere to go.
#[allow(clippy::result_large_err)]
async fn check_linear_team(
    state: &Arc<RunsState>,
    project: &str,
    team: &str,
) -> Result<(), Response> {
    let bad = |m: String| err(StatusCode::BAD_REQUEST, m);
    let conn = resolve_for_project(state, project)
        .await
        .map_err(|e| bad(format!("can't file in Linear for `{project}`: {e}")))?;
    let client = linear_client(state, &conn)
        .await
        .map_err(|e| bad(format!("can't file in Linear for `{project}`: {e}")))?;
    let teams = client
        .discover()
        .await
        .map_err(|e| err(StatusCode::BAD_GATEWAY, format!("Linear: {}", e.0)))?
        .teams;
    if teams.iter().any(|t| t.id == team) {
        Ok(())
    } else {
        Err(bad(format!(
            "`{project}`'s Linear workspace has no team `{team}`"
        )))
    }
}

// ── Routes ─────────────────────────────────────────────────────────────────

/// `GET /api/schedules`.
pub async fn list_schedules(Extension(state): Extension<Arc<RunsState>>) -> Response {
    let store = match state.schedule_store().await {
        Ok(s) => s,
        Err(e) => return err(StatusCode::SERVICE_UNAVAILABLE, e),
    };
    match store.list().await {
        Ok(rows) => Json(json!({ "schedules": rows })).into_response(),
        Err(e) => err(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()),
    }
}

/// `GET /api/schedules/{id}`.
pub async fn get_schedule(
    Extension(state): Extension<Arc<RunsState>>,
    AxumPath(id): AxumPath<String>,
) -> Response {
    let store = match state.schedule_store().await {
        Ok(s) => s,
        Err(e) => return err(StatusCode::SERVICE_UNAVAILABLE, e),
    };
    match store.get(&id).await {
        Ok(Some(s)) => Json(s).into_response(),
        Ok(None) => err(StatusCode::NOT_FOUND, "no such schedule"),
        Err(e) => err(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()),
    }
}

/// `POST /api/schedules`.
pub async fn create_schedule(
    _: AdminOnly,
    Extension(state): Extension<Arc<RunsState>>,
    headers: HeaderMap,
    Json(req): Json<ScheduleRequest>,
) -> Response {
    let (input, next) = match validate(&state, req).await {
        Ok(v) => v,
        Err(resp) => return resp,
    };
    let (user, _) = caller_trigger(&state, &headers).await;
    let store = match state.schedule_store().await {
        Ok(s) => s,
        Err(e) => return err(StatusCode::SERVICE_UNAVAILABLE, e),
    };
    let id = uuid::Uuid::new_v4().to_string();
    match store.create(&id, &input, next, user.as_deref()).await {
        Ok(s) => (StatusCode::CREATED, Json(s)).into_response(),
        Err(e) => err(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()),
    }
}

/// `PUT /api/schedules/{id}`. Recomputes the next occurrence from now, so a
/// changed time takes effect at once.
pub async fn update_schedule(
    _: AdminOnly,
    Extension(state): Extension<Arc<RunsState>>,
    AxumPath(id): AxumPath<String>,
    Json(req): Json<ScheduleRequest>,
) -> Response {
    let (input, next) = match validate(&state, req).await {
        Ok(v) => v,
        Err(resp) => return resp,
    };
    let store = match state.schedule_store().await {
        Ok(s) => s,
        Err(e) => return err(StatusCode::SERVICE_UNAVAILABLE, e),
    };
    match store.update(&id, &input, next).await {
        Ok(Some(s)) => Json(s).into_response(),
        Ok(None) => err(StatusCode::NOT_FOUND, "no such schedule"),
        Err(e) => err(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()),
    }
}

/// `DELETE /api/schedules/{id}`.
pub async fn delete_schedule(
    _: AdminOnly,
    Extension(state): Extension<Arc<RunsState>>,
    AxumPath(id): AxumPath<String>,
) -> Response {
    let store = match state.schedule_store().await {
        Ok(s) => s,
        Err(e) => return err(StatusCode::SERVICE_UNAVAILABLE, e),
    };
    match store.delete(&id).await {
        Ok(true) => StatusCode::NO_CONTENT.into_response(),
        Ok(false) => err(StatusCode::NOT_FOUND, "no such schedule"),
        Err(e) => err(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()),
    }
}

/// `POST /api/schedules/{id}/run` — start it now. The schedule's own next
/// occurrence is left alone.
pub async fn run_schedule_now(
    _: AdminOnly,
    Extension(state): Extension<Arc<RunsState>>,
    AxumPath(id): AxumPath<String>,
) -> Response {
    let store = match state.schedule_store().await {
        Ok(s) => s,
        Err(e) => return err(StatusCode::SERVICE_UNAVAILABLE, e),
    };
    let schedule = match store.get(&id).await {
        Ok(Some(s)) => s,
        Ok(None) => return err(StatusCode::NOT_FOUND, "no such schedule"),
        Err(e) => return err(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()),
    };
    match fire(&state, &schedule).await {
        Ok(run_id) => (StatusCode::ACCEPTED, Json(json!({ "run_id": run_id }))).into_response(),
        Err((status, msg)) => err(status, msg),
    }
}

#[derive(Debug, Deserialize)]
pub struct PreviewRequest {
    pub cron: String,
    pub timezone: String,
}

/// `POST /api/schedules/preview` — the next few times, or why it's invalid.
pub async fn preview_schedule(Json(req): Json<PreviewRequest>) -> Response {
    let cron = match parse_cron(req.cron.trim()) {
        Ok(c) => c,
        Err(e) => return err(StatusCode::BAD_REQUEST, e),
    };
    let tz = match parse_timezone(&req.timezone) {
        Ok(t) => t,
        Err(e) => return err(StatusCode::BAD_REQUEST, e),
    };
    let next = upcoming(&cron, tz, Utc::now(), PREVIEW_COUNT);
    if next.is_empty() {
        return err(
            StatusCode::BAD_REQUEST,
            format!("`{}` never fires", req.cron.trim()),
        );
    }
    Json(json!({ "next": next })).into_response()
}

// ── Firing ─────────────────────────────────────────────────────────────────

/// The title a scheduled run is recorded under: the schedule's name and the
/// date in its own timezone, so a week of reports reads as a list.
fn run_title(schedule: &Schedule, at: DateTime<Utc>) -> String {
    let date = match parse_timezone(&schedule.timezone) {
        Ok(tz) => at.with_timezone(&tz).format("%Y-%m-%d").to_string(),
        Err(_) => at.format("%Y-%m-%d").to_string(),
    };
    format!("{} — {date}", schedule.name)
}

/// Start one run of `schedule`, and record it as the schedule's last run.
async fn fire(state: &Arc<RunsState>, schedule: &Schedule) -> Result<String, (StatusCode, String)> {
    let req = CreateRunRequest {
        triggered_by: None,
        trigger_source: Some(SOURCE_SCHEDULE.to_string()),
        // Who a reader should see behind it: the schedule, by name.
        trigger_actor: Some(format!("Schedule “{}”", schedule.name)),
        workflow: schedule.workflow.clone(),
        title: Some(run_title(schedule, Utc::now())),
        description: schedule.task.clone(),
        args: String::new(),
        issue_id: None,
        real: true,
        base_branch: None,
        project: Some(schedule.project.clone()),
        swap_from: None,
        swap_to: None,
        ab_pair_id: None,
        ab_arm: None,
        ab_label: None,
    };
    let run_id = start_run(state, req).await?;
    if let Ok(store) = state.schedule_store().await {
        if let Err(e) = store.set_last_run(&schedule.id, &run_id).await {
            tracing::warn!(
                "schedule {}: could not record run {run_id}: {e}",
                schedule.id
            );
        }
        if let Some(team) = &schedule.linear_team_id {
            if let Err(e) = store.track_filing(&run_id, &schedule.id, team).await {
                tracing::warn!(
                    "schedule {}: run {run_id} won't be filed in Linear: {e}",
                    schedule.id
                );
            }
        }
    }
    Ok(run_id)
}

/// The issue a successful run is filed as: the flagged step's summary, or a
/// pointer to the report when the workflow flags none, then where it came from.
fn issue_description(
    detail: &harness_persist::RunDetail,
    schedule: &Schedule,
    run_link: Option<String>,
) -> String {
    let body = flagged_summary(detail).unwrap_or_else(|| {
        "The scheduled run completed. Its report is on the run page.".to_string()
    });
    let run = match run_link {
        Some(url) => format!("[run `{}`]({url})", detail.run.id),
        None => format!("run `{}`", detail.run.id),
    };
    format!(
        "{body}\n\n---\nFiled by the schedule “{}” from ai-harness {run}.",
        schedule.name
    )
}

/// One pass: file every finished run whose schedule asked for it. A run that
/// failed or was cancelled files nothing; one still going waits for a later
/// tick.
async fn file_results(state: &Arc<RunsState>) {
    let Ok(store) = state.schedule_store().await else {
        return;
    };
    let open = match store.open_filings().await {
        Ok(rows) => rows,
        Err(e) => {
            tracing::warn!("scheduler: could not read runs to file: {e}");
            return;
        }
    };
    if open.is_empty() {
        return;
    }
    let Ok(run_store) = state.store().await else {
        return;
    };
    for filing in open {
        let detail = match run_store.get_run(&filing.run_id).await {
            Ok(Some(d)) => d,
            // `start_run` returns before the run row is written; give it time.
            Ok(None) => {
                let age = Utc::now().signed_duration_since(filing.created_at);
                if age.num_minutes() >= MISSING_RUN_GRACE_MINS {
                    let _ = store.skip_filing(&filing.run_id).await;
                }
                continue;
            }
            Err(_) => continue,
        };
        match detail.run.status.as_str() {
            "completed" => {}
            "failed" | "cancelled" => {
                let _ = store.skip_filing(&filing.run_id).await;
                continue;
            }
            _ => continue,
        }
        let schedule = match store.get(&filing.schedule_id).await {
            Ok(Some(s)) => s,
            // Deleted since: nobody to file it for.
            Ok(None) => {
                let _ = store.skip_filing(&filing.run_id).await;
                continue;
            }
            Err(_) => continue,
        };
        if !matches!(store.hold_filing(&filing.run_id).await, Ok(true)) {
            continue;
        }
        let run_link = state
            .public_url()
            .map(|base| format!("{base}/runs/{}", detail.run.id));
        let title = detail
            .run
            .title
            .clone()
            .unwrap_or_else(|| run_title(&schedule, detail.run.recorded_at));
        let description = issue_description(&detail, &schedule, run_link);
        let created = async {
            let conn = resolve_for_project(state, &schedule.project).await?;
            let client = linear_client(state, &conn).await?;
            client
                .create_issue(&filing.team_id, &title, &description, None, &[], None)
                .await
                .map_err(|e| e.0)
        }
        .await;
        match created {
            Ok(issue) => {
                tracing::info!(
                    "scheduler: `{}` run {} filed as {}",
                    schedule.name,
                    filing.run_id,
                    issue.identifier
                );
                if let Err(e) = store
                    .filed(&filing.run_id, &issue.identifier, &issue.url)
                    .await
                {
                    tracing::warn!("scheduler: could not record {}: {e}", issue.identifier);
                }
            }
            Err(e) => {
                let give_up = filing.attempts + 1 >= MAX_FILING_ATTEMPTS;
                tracing::warn!(
                    "scheduler: could not file `{}` run {} in Linear{}: {e}",
                    schedule.name,
                    filing.run_id,
                    if give_up { " (giving up)" } else { "" }
                );
                let _ = store.release_filing(&filing.run_id, give_up).await;
            }
        }
    }
}

/// One pass: fire every due schedule exactly once.
async fn tick(state: &Arc<RunsState>) {
    let Ok(store) = state.schedule_store().await else {
        return;
    };
    let now = Utc::now();
    let due = match store.due(now).await {
        Ok(d) => d,
        Err(e) => {
            tracing::warn!("scheduler: could not read due schedules: {e}");
            return;
        }
    };
    for schedule in due {
        let Some(expected) = schedule.next_run_at else {
            continue;
        };
        // Next occurrence after *now*, not after the one being fired: after
        // downtime this is one catch-up run, not one per missed slot.
        let next = match (
            parse_cron(&schedule.cron),
            parse_timezone(&schedule.timezone),
        ) {
            (Ok(cron), Ok(tz)) => next_after(&cron, tz, now),
            _ => None,
        };
        match store.claim(&schedule.id, expected, next).await {
            Ok(true) => match fire(state, &schedule).await {
                Ok(run_id) => tracing::info!("scheduler: `{}` started run {run_id}", schedule.name),
                Err((_, e)) => {
                    tracing::warn!("scheduler: `{}` could not start: {e}", schedule.name)
                }
            },
            // Another replica took it, or it was edited meanwhile.
            Ok(false) => {}
            Err(e) => tracing::warn!("scheduler: could not claim `{}`: {e}", schedule.name),
        }
    }
}

/// Fire due schedules, every [`TICK`]. No-op outside a Tokio runtime (a
/// synchronous router-build test), like the other sweepers.
pub(crate) fn spawn_scheduler(state: Arc<RunsState>) {
    if tokio::runtime::Handle::try_current().is_err() {
        return;
    }
    tokio::spawn(async move {
        let mut interval = tokio::time::interval(TICK);
        interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        loop {
            interval.tick().await;
            tick(&state).await;
            file_results(&state).await;
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    fn schedule() -> Schedule {
        Schedule {
            id: "s1".into(),
            name: "Weekly INP report".into(),
            project: "p".into(),
            workflow: "grafana-inp-report".into(),
            cron: "0 8 * * 1".into(),
            timezone: "Europe/Copenhagen".into(),
            task: String::new(),
            enabled: true,
            next_run_at: None,
            last_run_at: None,
            last_run_id: None,
            linear_team_id: Some("team-1".into()),
            last_issue_identifier: None,
            last_issue_url: None,
            created_by: None,
            created_at: Utc::now(),
            updated_at: Utc::now(),
        }
    }

    fn completed_run(summary: Option<&str>) -> harness_persist::RunDetail {
        let node = |id: &str, output: &str| harness_persist::PersistedNode {
            node_id: id.into(),
            ordinal: 0,
            status: "success".into(),
            provider: None,
            model: None,
            output: output.into(),
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
            error_count: 0,
        };
        let meta = |id: &str, flagged: bool| harness_dag::NodeMeta {
            id: id.into(),
            depends_on: vec![],
            category: None,
            artifact: None,
            linear_comment: flagged,
        };
        harness_persist::RunDetail {
            run: harness_persist::RunSummary {
                triggered_by: None,
                trigger_source: Some(SOURCE_SCHEDULE.into()),
                trigger_actor: None,
                id: "r1".into(),
                workflow_name: "grafana-inp-report".into(),
                title: Some("Weekly INP report — 2026-10-26".into()),
                description: None,
                status: "completed".into(),
                node_count: 2,
                started_at: None,
                ended_at: None,
                recorded_at: Utc::now(),
                project: Some("p".into()),
                ab_pair_id: None,
                ab_arm: None,
                ab_label: None,
            },
            nodes: vec![
                node("report", "the long report"),
                node("summary", summary.unwrap_or("")),
            ],
            graph: vec![meta("report", false), meta("summary", summary.is_some())],
        }
    }

    #[test]
    fn a_filed_issue_carries_the_flagged_summary_and_links_the_run() {
        let text = issue_description(
            &completed_run(Some("INP p75 rose to 240 ms on PDPs.")),
            &schedule(),
            Some("https://harness.example/runs/r1".into()),
        );
        assert!(
            text.starts_with("INP p75 rose to 240 ms on PDPs."),
            "{text}"
        );
        assert!(!text.contains("the long report"));
        assert!(text.contains("“Weekly INP report”"));
        assert!(
            text.ends_with("[run `r1`](https://harness.example/runs/r1)."),
            "{text}"
        );
    }

    #[test]
    fn without_a_flagged_step_the_issue_points_at_the_report() {
        let text = issue_description(&completed_run(None), &schedule(), None);
        assert!(text.starts_with("The scheduled run completed."), "{text}");
        assert!(text.ends_with("run `r1`."), "{text}");
    }

    #[test]
    fn five_fields_only() {
        assert!(parse_cron("0 8 * * 1").is_ok());
        assert!(parse_cron("*/15 * * * *").is_ok());
        let six = parse_cron("0 0 8 * * 1").unwrap_err();
        assert!(six.contains("6 fields"), "{six}");
        assert!(parse_cron("0 25 * * *").is_err(), "hour 25");
        assert!(parse_cron("every monday").is_err());
    }

    #[test]
    fn mondays_at_eight_stay_at_eight_across_dst() {
        let cron = parse_cron("0 8 * * 1").unwrap();
        let tz = parse_timezone("Europe/Copenhagen").unwrap();
        // From a Wednesday in late October 2026; DST ends Sunday 25 Oct.
        let after = Utc.with_ymd_and_hms(2026, 10, 21, 12, 0, 0).unwrap();
        let next = upcoming(&cron, tz, after, 2);
        // 26 Oct is CET (UTC+1) → 07:00 UTC; the Monday before DST ended
        // would have been 06:00 UTC.
        assert_eq!(
            next[0],
            Utc.with_ymd_and_hms(2026, 10, 26, 7, 0, 0).unwrap()
        );
        assert_eq!(next[1], Utc.with_ymd_and_hms(2026, 11, 2, 7, 0, 0).unwrap());
        for t in &next {
            assert_eq!(
                t.with_timezone(&tz).format("%a %H:%M").to_string(),
                "Mon 08:00"
            );
        }
    }

    #[test]
    fn a_bad_timezone_is_named() {
        assert!(parse_timezone("Europe/Copenhagen").is_ok());
        assert!(parse_timezone("Mars/Base")
            .unwrap_err()
            .contains("Mars/Base"));
    }

    #[test]
    fn the_run_title_uses_the_schedules_own_date() {
        let s = Schedule {
            name: "Weekly INP".into(),
            ..schedule()
        };
        // 23:30 UTC Sunday is already Monday in Copenhagen.
        let at = Utc.with_ymd_and_hms(2026, 10, 25, 23, 30, 0).unwrap();
        assert_eq!(run_title(&s, at), "Weekly INP — 2026-10-26");
    }
}
