//! MCP tools for **triggers**: everything that starts a run by itself, as one
//! kind of thing.
//!
//! A trigger is a source plus a target (a project and a workflow). Two sources
//! exist today, each with its own table and rules — Linear bindings and
//! schedules — and these tools put one interface over both, matching the
//! Triggers page. Writes go through the same functions as the HTTP routes, so a
//! trigger saved here is held to exactly the rules the page's form is.
//!
//! The older per-kind tools (`schedule_*`, `linear_bindings`) stay: clients
//! already call them, and they remain correct.

use std::collections::HashMap;
use std::sync::Arc;

use harness_persist::LinearSource;
use harness_sources::linear::Team;
use serde_json::{json, Value};

use super::linear_source_routes::{save_binding, trash_binding, PutSourceBody};
use super::runs_routes::{RunsState, TriggerInfo};
use super::schedules_routes as sched;

pub(crate) const TRIGGER_TOOLS: &[&str] = &["trigger_list", "trigger_save", "trigger_delete"];

/// The kinds a trigger can be, as `kind` accepts them.
const KINDS: &[&str] = &["linear", "schedule"];

/// Status slots of a Linear binding that `trigger_save` takes by name, with
/// the field each one fills.
const STATUS_SLOTS: &[&str] = &[
    "in_progress",
    "review",
    "ready",
    "piece_ready",
    "epic_review",
];

pub(crate) fn tool_defs() -> Vec<Value> {
    let kind = json!({
        "type": "string",
        "enum": KINDS,
        "description": "`linear`: an issue in a team's status, delegated to the harness (or labelled), starts the run. `schedule`: a cron time starts it."
    });
    vec![
        json!({
            "name": "trigger_list",
            "description": "Every trigger — each thing that starts a run by itself — of every kind, as one list: Linear triggers and schedules. Each row has `kind`, `project`, `workflow`, `state` (on / dry-run / off), a `when` sentence, and its kind's own fields, with Linear statuses named rather than given as ids. Read-only. Use before trigger_save to see what exists.",
            "inputSchema": {
                "type": "object",
                "additionalProperties": false,
                "properties": {
                    "kind": kind,
                    "project": { "type": "string" },
                    "workflow": { "type": "string" }
                },
            }
        }),
        json!({
            "name": "trigger_save",
            "description": "Create or change a trigger. Only the fields given change; on a new trigger the rest take their defaults.\n\n`kind: linear` — keyed by `project` + `workflow` (one Linear trigger per workflow per project). A new one needs `team` and `claims_from`, and starts disabled and dry-run unless `enabled`/`live` say otherwise. Teams and statuses are taken by name (as linear_states and trigger_list show them) or by id. The status map (`in_progress`, `review`, `ready`, `piece_ready`, `epic_review`) is optional — a workflow can move the issue itself with `linear:` steps instead; pass \"\" to clear one.\n\n`kind: schedule` — with `id` it changes that schedule; without, it creates one and needs `name`, `project`, `workflow` and `cron`. Administrators only.",
            "inputSchema": {
                "type": "object",
                "additionalProperties": false,
                "properties": {
                    "kind": kind,
                    "project": { "type": "string" },
                    "workflow": { "type": "string" },
                    "enabled": { "type": "boolean" },
                    "team": { "type": "string", "description": "linear: team name, key (e.g. AIH) or id." },
                    "claims_from": { "type": "string", "description": "linear: the status an issue sits in to be picked up." },
                    "trigger_label": { "type": "string", "description": "linear: also start on issues carrying this label that nobody delegated. \"\" = delegation only." },
                    "live": { "type": "boolean", "description": "linear: false = dry-run, which logs what it would claim and starts nothing." },
                    "in_progress": { "type": "string" },
                    "review": { "type": "string" },
                    "ready": { "type": "string" },
                    "piece_ready": { "type": "string" },
                    "epic_review": { "type": "string" },
                    "failed_label": { "type": "string", "description": "linear: label applied when the attempt budget is spent. \"\" = none." },
                    "base_branch": { "type": "string", "description": "linear: \"\" = the project's own." },
                    "poll_interval_secs": { "type": "integer", "minimum": 1, "maximum": 86400 },
                    "max_concurrent_runs": { "type": "integer", "minimum": 1, "maximum": 20 },
                    "max_attempts": { "type": "integer", "minimum": 1, "maximum": 10 },
                    "id": { "type": "string", "description": "schedule: which one to change, from trigger_list." },
                    "name": { "type": "string", "description": "schedule: also titles each run." },
                    "cron": { "type": "string", "description": "schedule: five fields, read in `timezone`. Check it with schedule_preview." },
                    "timezone": { "type": "string", "description": "schedule: IANA zone; default UTC." },
                    "task": { "type": "string", "description": "schedule: what each run is asked to do ($ARGUMENTS)." }
                },
                "required": ["kind"],
            }
        }),
        json!({
            "name": "trigger_delete",
            "description": "Delete a trigger. `kind: linear` takes `project` + `workflow`, and the trigger goes to the bin for 14 days (restorable from Recently deleted on the Projects page). `kind: schedule` takes `id` and is permanent; runs it already started are kept. Deleting a schedule is for administrators.",
            "inputSchema": {
                "type": "object",
                "additionalProperties": false,
                "properties": {
                    "kind": kind,
                    "project": { "type": "string" },
                    "workflow": { "type": "string" },
                    "id": { "type": "string" }
                },
                "required": ["kind"],
            }
        }),
    ]
}

/// Run one of [`TRIGGER_TOOLS`]: the summary line and the structured body, or
/// why not.
pub(crate) async fn call(
    state: &Arc<RunsState>,
    actor: &TriggerInfo,
    name: &str,
    args: &Value,
) -> Result<(String, Value), String> {
    match name {
        "trigger_list" => list(state, args).await,
        "trigger_save" => match kind_of(args)? {
            "linear" => save_linear(state, args).await,
            _ => save_schedule(state, actor, args).await,
        },
        "trigger_delete" => {
            match kind_of(args)? {
                "linear" => {
                    let (project, workflow) = (text(args, "project"), text(args, "workflow"));
                    if project.is_empty() || workflow.is_empty() {
                        return Err("a Linear trigger is named by `project` and `workflow`".into());
                    }
                    match trash_binding(state, &project, &workflow, actor.actor.as_deref()).await {
                    Ok(Some(trash_id)) => Ok((
                        format!("deleted the {workflow} trigger in {project}; restorable for 14 days"),
                        json!({ "deleted": true, "trash_id": trash_id }),
                    )),
                    Ok(None) => Err(format!("no Linear trigger for `{workflow}` in `{project}`")),
                    Err((_, msg)) => Err(msg),
                }
                }
                _ => {
                    super::mcp_routes::may_change_schedules(state, actor).await?;
                    let id = text(args, "id");
                    if id.is_empty() {
                        return Err("a schedule is named by `id`".into());
                    }
                    sched::delete(state, &id)
                        .await
                        .map(|()| ("deleted".to_string(), json!({ "deleted": id })))
                        .map_err(|(_, msg)| msg)
                }
            }
        }
        other => Err(format!("unknown trigger tool `{other}`")),
    }
}

fn text(args: &Value, key: &str) -> String {
    args.get(key)
        .and_then(Value::as_str)
        .unwrap_or_default()
        .trim()
        .to_string()
}

fn kind_of(args: &Value) -> Result<&'static str, String> {
    let k = text(args, "kind");
    KINDS
        .iter()
        .copied()
        .find(|known| *known == k)
        .ok_or_else(|| format!("`kind` must be one of: {}", KINDS.join(", ")))
}

// ── Listing ─────────────────────────────────────────────────────────────────

async fn list(state: &Arc<RunsState>, args: &Value) -> Result<(String, Value), String> {
    let kind = text(args, "kind");
    let project = text(args, "project");
    let workflow = text(args, "workflow");
    if !kind.is_empty() {
        kind_of(args)?;
    }
    let keep = |k: &str, p: &str, w: &str| {
        (kind.is_empty() || kind == k)
            && (project.is_empty() || project == p)
            && (workflow.is_empty() || workflow == w)
    };

    let mut rows: Vec<Value> = Vec::new();

    let store = state
        .linear_source_store()
        .await
        .map_err(|e| e.to_string())?;
    let bindings: Vec<LinearSource> = store
        .list_all()
        .await
        .map_err(|e| e.to_string())?
        .into_iter()
        .filter(|b| keep("linear", &b.project, &b.workflow))
        .collect();
    // One discovery per project, best-effort: a list is still useful with ids
    // when Linear is unreachable. Discovery is cached server-side, so asking
    // again for each project's rows costs nothing extra.
    let mut names: HashMap<String, super::linear_diagnose::States> = HashMap::new();
    for b in &bindings {
        if !names.contains_key(&b.project) {
            let n = super::linear_diagnose::state_names(state, &b.project)
                .await
                .unwrap_or_default();
            names.insert(b.project.clone(), n);
        }
    }
    for b in &bindings {
        rows.push(linear_row(b, &names[&b.project]));
    }

    let schedules = sched::list(state).await.map_err(|(_, msg)| msg)?;
    for s in schedules
        .iter()
        .filter(|s| keep("schedule", &s.project, &s.workflow))
    {
        rows.push(json!({
            "kind": "schedule",
            "id": s.id,
            "name": s.name,
            "project": s.project,
            "workflow": s.workflow,
            "state": if s.enabled { "on" } else { "off" },
            "when": format!("{} · {}", s.cron, s.timezone),
            "cron": s.cron,
            "timezone": s.timezone,
            "task": s.task,
            "enabled": s.enabled,
            "next_run_at": s.next_run_at,
            "last_run_at": s.last_run_at,
            "last_run_id": s.last_run_id,
        }));
    }

    // Not grouped by project: by what the trigger starts, like the page.
    rows.sort_by(|a, b| {
        let key = |v: &Value| {
            (
                v["workflow"].as_str().unwrap_or_default().to_string(),
                v["project"].as_str().unwrap_or_default().to_string(),
                v["kind"].as_str().unwrap_or_default().to_string(),
            )
        };
        key(a).cmp(&key(b))
    });
    Ok((
        format!("{} trigger(s)", rows.len()),
        json!({ "triggers": rows }),
    ))
}

fn linear_row(b: &LinearSource, names: &super::linear_diagnose::States) -> Value {
    let name = |id: Option<&str>| -> Value {
        match id {
            Some(id) => json!(names.get(id).cloned().unwrap_or_else(|| id.to_string())),
            None => Value::Null,
        }
    };
    let claims = names
        .get(&b.source_state_id)
        .cloned()
        .unwrap_or_else(|| b.source_state_id.clone());
    let mut when = format!("{} · {claims}", b.team_name);
    if let Some(p) = b
        .linear_project_name
        .as_deref()
        .or(b.linear_project_id.as_deref())
    {
        when.push_str(&format!(" · only {p}"));
    }
    match &b.trigger_label {
        Some(l) => when.push_str(&format!(" · delegated or labelled {l}")),
        None => when.push_str(" · delegated"),
    }
    json!({
        "kind": "linear",
        "project": b.project,
        "workflow": b.workflow,
        "state": if !b.enabled { "off" } else if b.live { "on" } else { "dry-run" },
        "when": when,
        "team": b.team_name,
        "claims_from": claims,
        "trigger_label": b.trigger_label,
        "linear_project": b.linear_project_name.as_deref().or(b.linear_project_id.as_deref()),
        "in_progress": name(b.in_progress_state_id.as_deref()),
        "review": name(b.review_state_id.as_deref()),
        "ready": name(b.ready_state_id.as_deref()),
        "piece_ready": name(b.piece_ready_state_id.as_deref()),
        "epic_review": name(b.epic_review_state_id.as_deref()),
        "failed_label": b.failed_label,
        "base_branch": b.base_branch,
        "poll_interval_secs": b.poll_interval_secs,
        "max_concurrent_runs": b.max_concurrent_runs,
        "max_attempts": b.max_attempts,
        "enabled": b.enabled,
        "live": b.live,
    })
}

// ── Saving ──────────────────────────────────────────────────────────────────

async fn save_linear(state: &Arc<RunsState>, args: &Value) -> Result<(String, Value), String> {
    let project = text(args, "project");
    let workflow = text(args, "workflow");
    if project.is_empty() || workflow.is_empty() {
        return Err("a Linear trigger needs `project` and `workflow`".into());
    }
    let store = state
        .linear_source_store()
        .await
        .map_err(|e| e.to_string())?;
    let existing = store
        .get(&project, &workflow)
        .await
        .map_err(|e| e.to_string())?;
    let created = existing.is_none();
    let mut body = existing
        .as_ref()
        .map(PutSourceBody::from_binding)
        .unwrap_or_else(|| PutSourceBody::new(&workflow));

    // Linear is only asked when a name has to be turned into an id: a call
    // that flips `live` needs nothing from it.
    let needs_teams = ["team", "claims_from"]
        .iter()
        .chain(STATUS_SLOTS)
        .any(|k| args.get(*k).is_some());
    let teams = if needs_teams {
        let conn = super::linear_connections::resolve_for_project(state, &project).await?;
        let client = super::linear_oauth::linear_client(state, &conn).await?;
        Some(client.discover().await.map_err(|e| e.0)?.teams)
    } else {
        None
    };
    apply_linear_args(&mut body, args, teams.as_deref(), created)?;

    let saved = save_binding(state, &project, body)
        .await
        .map_err(|(_, msg)| msg)?;
    let verb = if created { "created" } else { "updated" };
    let state_word = if !saved.enabled {
        "off"
    } else if saved.live {
        "on"
    } else {
        "dry-run"
    };
    Ok((
        format!("{verb} the {workflow} trigger in {project} ({state_word})"),
        json!(saved),
    ))
}

/// The team `wanted` names: by id, key or name, case-insensitively.
fn find_team<'a>(teams: &'a [Team], wanted: &str) -> Result<&'a Team, String> {
    teams
        .iter()
        .find(|t| {
            t.id == wanted
                || t.key.eq_ignore_ascii_case(wanted)
                || t.name.eq_ignore_ascii_case(wanted)
        })
        .ok_or_else(|| {
            let known: Vec<String> = teams
                .iter()
                .map(|t| format!("{} ({})", t.name, t.key))
                .collect();
            format!(
                "no team `{wanted}`; this project's Linear has: {}",
                known.join(", ")
            )
        })
}

/// The id of the status `wanted` names in `team`: by id or name.
fn find_state(team: &Team, wanted: &str) -> Result<String, String> {
    team.states
        .iter()
        .find(|s| s.id == wanted || s.name.eq_ignore_ascii_case(wanted))
        .map(|s| s.id.clone())
        .ok_or_else(|| {
            let known: Vec<&str> = team.states.iter().map(|s| s.name.as_str()).collect();
            format!(
                "no status `{wanted}` in {}; it has: {}",
                team.name,
                known.join(", ")
            )
        })
}

/// Lay the call's fields over `body`. Absent fields keep their value; `""`
/// clears an optional one. Names are resolved against `teams`, which the
/// caller fetched only if a name was given.
fn apply_linear_args(
    body: &mut PutSourceBody,
    args: &Value,
    teams: Option<&[Team]>,
    created: bool,
) -> Result<(), String> {
    let given = |k: &str| args.get(k).and_then(Value::as_str).map(str::trim);
    let optional = |v: &str| (!v.is_empty()).then(|| v.to_string());

    if created && (given("team").is_none() || given("claims_from").is_none()) {
        return Err("a new Linear trigger needs `team` and `claims_from`".into());
    }

    let teams = teams.unwrap_or_default();
    let old_team = body.team_id.clone();
    if let Some(t) = given("team") {
        let team = find_team(teams, t)?;
        body.team_id = team.id.clone();
        body.team_name = team.name.clone();
    }
    let team_changed = body.team_id != old_team;
    // A status id belongs to one team. Keeping the old ones after a move would
    // save a binding that waits on a column its new team doesn't have.
    if team_changed && !created {
        if given("claims_from").is_none() {
            return Err("moving a trigger to another team needs `claims_from` in that team".into());
        }
        for slot in STATUS_SLOTS {
            if given(slot).is_none() {
                *status_slot(body, slot) = None;
            }
        }
    }

    // Statuses only resolve against the trigger's own team.
    let team_id = body.team_id.clone();
    let team = || -> Result<&Team, String> { find_team(teams, &team_id) };
    if let Some(s) = given("claims_from") {
        body.source_state_id = find_state(team()?, s)?;
    }
    for slot in STATUS_SLOTS {
        if let Some(s) = given(slot) {
            let id = if s.is_empty() {
                None
            } else {
                Some(find_state(team()?, s)?)
            };
            *status_slot(body, slot) = id;
        }
    }

    if let Some(v) = given("trigger_label") {
        body.trigger_label = optional(v);
    }
    if let Some(v) = given("failed_label") {
        body.failed_label = optional(v);
    }
    if let Some(v) = given("base_branch") {
        body.base_branch = optional(v);
    }
    let int = |k: &str| args.get(k).and_then(Value::as_i64).map(|n| n as i32);
    if let Some(n) = int("poll_interval_secs") {
        body.poll_interval_secs = n;
    }
    if let Some(n) = int("max_concurrent_runs") {
        body.max_concurrent_runs = n;
    }
    if let Some(n) = int("max_attempts") {
        body.max_attempts = n;
    }
    if let Some(b) = args.get("enabled").and_then(Value::as_bool) {
        body.enabled = b;
    }
    if let Some(b) = args.get("live").and_then(Value::as_bool) {
        body.live = b;
    }
    Ok(())
}

fn status_slot<'a>(body: &'a mut PutSourceBody, slot: &str) -> &'a mut Option<String> {
    match slot {
        "in_progress" => &mut body.in_progress_state_id,
        "review" => &mut body.review_state_id,
        "ready" => &mut body.ready_state_id,
        "piece_ready" => &mut body.piece_ready_state_id,
        _ => &mut body.epic_review_state_id,
    }
}

async fn save_schedule(
    state: &Arc<RunsState>,
    actor: &TriggerInfo,
    args: &Value,
) -> Result<(String, Value), String> {
    super::mcp_routes::may_change_schedules(state, actor).await?;
    let id = text(args, "id");
    let saved = if id.is_empty() {
        let base = sched::ScheduleRequest {
            name: String::new(),
            project: String::new(),
            workflow: String::new(),
            cron: String::new(),
            timezone: "UTC".into(),
            task: String::new(),
            enabled: true,
        };
        sched::create(
            state,
            super::mcp_routes::schedule_request(base, args),
            actor.user_id.as_deref(),
        )
        .await
    } else {
        let current = sched::get(state, &id).await.map_err(|(_, msg)| msg)?;
        let req = super::mcp_routes::schedule_request(
            sched::ScheduleRequest::from_schedule(&current),
            args,
        );
        sched::update(state, &id, req).await
    }
    .map_err(|(_, msg)| msg)?;
    let verb = if id.is_empty() { "created" } else { "updated" };
    Ok((
        format!(
            "{verb} the schedule `{}`, next run {:?}",
            saved.name, saved.next_run_at
        ),
        json!(saved),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use harness_sources::linear::WorkflowState;

    fn teams() -> Vec<Team> {
        let st = |id: &str, name: &str| WorkflowState {
            id: id.into(),
            name: name.into(),
            kind: "started".into(),
            position: 0.0,
        };
        vec![
            Team {
                id: "t-aih".into(),
                name: "ai-harness".into(),
                key: "AIH".into(),
                states: vec![
                    st("a-todo", "Todo"),
                    st("a-prog", "In Progress"),
                    st("a-rev", "In Review"),
                ],
                labels: vec![],
            },
            Team {
                id: "t-nil".into(),
                name: "niles".into(),
                key: "NIL".into(),
                states: vec![st("n-todo", "Todo"), st("n-prog", "In Progress")],
                labels: vec![],
            },
        ]
    }

    #[test]
    fn a_new_trigger_resolves_names_and_starts_switched_off() {
        let mut body = PutSourceBody::new("task");
        apply_linear_args(
            &mut body,
            &json!({ "team": "aih", "claims_from": "todo", "review": "In Review" }),
            Some(&teams()),
            true,
        )
        .unwrap();
        assert_eq!(body.team_id, "t-aih");
        assert_eq!(body.team_name, "ai-harness");
        assert_eq!(body.source_state_id, "a-todo");
        assert_eq!(body.review_state_id.as_deref(), Some("a-rev"));
        assert!(
            !body.enabled && !body.live,
            "nothing starts until it is turned on"
        );
    }

    #[test]
    fn a_new_trigger_without_team_or_status_is_refused() {
        let mut body = PutSourceBody::new("task");
        let e = apply_linear_args(&mut body, &json!({ "team": "AIH" }), Some(&teams()), true)
            .unwrap_err();
        assert!(e.contains("claims_from"), "{e}");
    }

    #[test]
    fn an_update_changes_only_what_it_names_and_needs_no_linear() {
        let mut body = PutSourceBody::new("task");
        apply_linear_args(
            &mut body,
            &json!({ "team": "AIH", "claims_from": "Todo", "in_progress": "In Progress" }),
            Some(&teams()),
            true,
        )
        .unwrap();
        // Flipping `live` must not need discovery at all.
        apply_linear_args(
            &mut body,
            &json!({ "live": true, "enabled": true }),
            None,
            false,
        )
        .unwrap();
        assert!(body.live && body.enabled);
        assert_eq!(body.in_progress_state_id.as_deref(), Some("a-prog"));
    }

    #[test]
    fn an_empty_string_clears_an_optional_field() {
        let mut body = PutSourceBody::new("task");
        apply_linear_args(
            &mut body,
            &json!({ "team": "AIH", "claims_from": "Todo", "ready": "In Review", "trigger_label": "go" }),
            Some(&teams()),
            true,
        )
        .unwrap();
        apply_linear_args(
            &mut body,
            &json!({ "ready": "", "trigger_label": "" }),
            Some(&teams()),
            false,
        )
        .unwrap();
        assert_eq!(body.ready_state_id, None);
        assert_eq!(body.trigger_label, None);
    }

    #[test]
    fn moving_teams_drops_the_old_teams_statuses() {
        let mut body = PutSourceBody::new("task");
        apply_linear_args(
            &mut body,
            &json!({ "team": "AIH", "claims_from": "Todo", "review": "In Review" }),
            Some(&teams()),
            true,
        )
        .unwrap();
        let refused = apply_linear_args(
            &mut body.clone(),
            &json!({ "team": "NIL" }),
            Some(&teams()),
            false,
        )
        .unwrap_err();
        assert!(refused.contains("claims_from"), "{refused}");

        apply_linear_args(
            &mut body,
            &json!({ "team": "NIL", "claims_from": "Todo" }),
            Some(&teams()),
            false,
        )
        .unwrap();
        assert_eq!(body.source_state_id, "n-todo");
        assert_eq!(
            body.review_state_id, None,
            "an AIH status can't follow it to NIL"
        );
    }

    #[test]
    fn an_unknown_name_says_what_exists() {
        let mut body = PutSourceBody::new("task");
        let e = apply_linear_args(
            &mut body,
            &json!({ "team": "AIH", "claims_from": "Doing" }),
            Some(&teams()),
            true,
        )
        .unwrap_err();
        assert!(e.contains("Todo") && e.contains("In Review"), "{e}");
        let e = find_team(&teams(), "ops").unwrap_err();
        assert!(e.contains("ai-harness (AIH)"), "{e}");
    }

    #[test]
    fn kind_is_required_and_checked() {
        assert!(kind_of(&json!({})).is_err());
        assert!(kind_of(&json!({ "kind": "webhook" })).is_err());
        assert_eq!(kind_of(&json!({ "kind": "schedule" })).unwrap(), "schedule");
    }
}
