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
use super::runs_routes::{start_run, CreateRunRequest, RunsState, SOURCE_SCHEDULE};

/// How often the scheduler looks for due schedules. A schedule fires within
/// this much of its time.
const TICK: std::time::Duration = std::time::Duration::from_secs(30);

/// How many upcoming times the preview returns.
const PREVIEW_COUNT: usize = 5;

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
        },
        next,
    ))
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
    }
    Ok(run_id)
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
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

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
            id: "s".into(),
            name: "Weekly INP".into(),
            project: "p".into(),
            workflow: "w".into(),
            cron: "0 0 * * 1".into(),
            timezone: "Europe/Copenhagen".into(),
            task: String::new(),
            enabled: true,
            next_run_at: None,
            last_run_at: None,
            last_run_id: None,
            created_by: None,
            created_at: Utc::now(),
            updated_at: Utc::now(),
        };
        // 23:30 UTC Sunday is already Monday in Copenhagen.
        let at = Utc.with_ymd_and_hms(2026, 10, 25, 23, 30, 0).unwrap();
        assert_eq!(run_title(&s, at), "Weekly INP — 2026-10-26");
    }
}
