//! MCP tools for **triggers**: everything that starts a run by itself, as the
//! Triggers page shows it — Linear triggers, webhooks and schedules.
//!
//! Linear triggers and webhooks are the Triggers page's own model
//! (`triggers_routes`); schedules keep their table and rules. Writes go through
//! the same functions as the HTTP routes, so a trigger saved here is held to
//! exactly the rules the page's form is.
//!
//! Classic project bindings are not triggers in this sense. They stay readable
//! through `linear_bindings`; whether they or the Linear triggers start runs is
//! the installation's `system`, which `trigger_list` reports.

use std::sync::Arc;

use harness_persist::{RunTrigger, KIND_LINEAR};
use harness_sources::linear::Team;
use serde_json::{json, Value};

use super::runs_routes::{RunsState, TriggerInfo};
use super::schedules_routes as sched;
use super::triggers_routes::{self as triggers, LinearConfig, TriggerRequest, WebhookConfig};

pub(crate) const TRIGGER_TOOLS: &[&str] = &["trigger_list", "trigger_save", "trigger_delete"];

/// The kinds `kind` accepts.
const KINDS: &[&str] = &["linear", "webhook", "schedule"];

pub(crate) fn tool_defs() -> Vec<Value> {
    let kind = json!({
        "type": "string",
        "enum": KINDS,
        "description": "`linear`: an issue in a team's status, delegated to the harness (or labelled), starts the run. `webhook`: a signed HTTP request does. `schedule`: a cron time does."
    });
    vec![
        json!({
            "name": "trigger_list",
            "description": "Every trigger — each thing that starts a run by itself — as the Triggers page lists them: Linear triggers, webhooks and schedules. Each row has `id`, `kind`, `name`, `project`, `workflow`, `enabled`, a `when` sentence and its kind's settings; a webhook also has its `url`. Also returns `system`: `triggers` when Linear triggers start Linear runs, `classic` when each project's Linear bindings still do (then Linear triggers here are saved but idle). Read-only.",
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
            "description": "Create a trigger, or change one by `id`. Only the fields given change.\n\nA trigger only says when to start: it has no status map. Where the issue goes after that is the workflow's job, with `linear:` steps.\n\n`kind: linear` — needs `project`, `workflow`, `team` and `status` (names as linear_states shows them, or ids). Optional `label`: also start on issues carrying it that nobody delegated. Fires once each time an issue comes to match.\n\n`kind: webhook` — needs `project` and `workflow`. Creating one returns its `url` and `secret` (the secret is shown only then). Deliveries are verified by HMAC-SHA256 signature (`auth: signature`, GitHub-compatible) or by the secret as a bearer token (`auth: token`). `filters` must all match; `task_template` fills `{{body.x}}` / `{{headers.name}}`. Administrators only.\n\n`kind: schedule` — needs `name`, `project`, `workflow` and `cron` to create. Administrators only.\n\nNew triggers are created disabled unless `enabled: true`.",
            "inputSchema": {
                "type": "object",
                "additionalProperties": false,
                "properties": {
                    "kind": kind,
                    "id": { "type": "string", "description": "Which trigger to change, from trigger_list. Omit to create." },
                    "name": { "type": "string" },
                    "project": { "type": "string" },
                    "workflow": { "type": "string" },
                    "enabled": { "type": "boolean" },
                    "team": { "type": "string", "description": "linear: team name, key (e.g. AIH) or id." },
                    "status": { "type": "string", "description": "linear: the status an issue enters to start the run." },
                    "label": { "type": "string", "description": "linear: also start on issues with this label that nobody delegated. \"\" = delegation only." },
                    "base_branch": { "type": "string", "description": "linear/webhook: \"\" = the project's own." },
                    "max_concurrent_runs": { "type": "integer", "minimum": 1, "maximum": 20, "description": "linear" },
                    "auth": { "type": "string", "enum": ["signature", "token"], "description": "webhook" },
                    "filters": {
                        "type": "array",
                        "description": "webhook: conditions that must all hold, e.g. {\"path\": \"headers.x-github-event\", \"equals\": [\"pull_request\"]}.",
                        "items": {
                            "type": "object",
                            "properties": {
                                "path": { "type": "string" },
                                "equals": { "type": "array", "items": { "type": "string" } }
                            },
                            "required": ["path", "equals"]
                        }
                    },
                    "task_template": { "type": "string", "description": "webhook: the run's task. Empty = the payload itself." },
                    "delivery_header": { "type": "string", "description": "webhook: header naming a delivery, for de-duplicating retries." },
                    "cron": { "type": "string", "description": "schedule: five fields, read in `timezone`. Check with schedule_preview." },
                    "timezone": { "type": "string", "description": "schedule: IANA zone; default UTC." },
                    "task": { "type": "string", "description": "schedule: what each run is asked to do." }
                },
                "required": ["kind"],
            }
        }),
        json!({
            "name": "trigger_delete",
            "description": "Delete a trigger by `kind` and `id`. Runs it already started are kept. A webhook's URL stops working at once. Deleting a webhook or a schedule is for administrators.",
            "inputSchema": {
                "type": "object",
                "additionalProperties": false,
                "properties": { "kind": kind, "id": { "type": "string" } },
                "required": ["kind", "id"],
            }
        }),
    ]
}

/// Run one of [`TRIGGER_TOOLS`]: the summary line and the structured body.
pub(crate) async fn call(
    state: &Arc<RunsState>,
    actor: &TriggerInfo,
    name: &str,
    args: &Value,
) -> Result<(String, Value), String> {
    match name {
        "trigger_list" => list(state, args).await,
        "trigger_save" => match kind_of(args)? {
            "schedule" => save_schedule(state, actor, args).await,
            kind => save(state, actor, kind, args).await,
        },
        "trigger_delete" => {
            let id = text(args, "id");
            if id.is_empty() {
                return Err("`id` is required".into());
            }
            match kind_of(args)? {
                "schedule" => {
                    super::mcp_routes::may_change_schedules(state, actor).await?;
                    sched::delete(state, &id).await.map_err(|(_, m)| m)?;
                }
                _ => {
                    let admin = is_admin(state, actor).await;
                    triggers::delete(state, &id, admin)
                        .await
                        .map_err(|(_, m)| m)?;
                }
            }
            Ok(("deleted".into(), json!({ "deleted": id })))
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

/// The MCP caller as `triggers_routes` judges them: an administrator, or
/// anyone before the install has accounts. The shared MCP key belongs to no one
/// in particular; an administrator issued it, so it counts as one — the same
/// rule as for schedules.
async fn is_admin(state: &Arc<RunsState>, actor: &TriggerInfo) -> bool {
    super::mcp_routes::may_change_schedules(state, actor)
        .await
        .is_ok()
}

// ── Listing ─────────────────────────────────────────────────────────────────

/// What sets a trigger off, in words.
fn when(t: &RunTrigger) -> String {
    match t.kind.as_str() {
        KIND_LINEAR => match serde_json::from_value::<LinearConfig>(t.config.0.clone()) {
            Ok(c) => {
                let mut s = format!("{} · {}", c.team_name, c.state_name);
                if let Some(p) = c
                    .linear_project_name
                    .as_deref()
                    .or(c.linear_project_id.as_deref())
                {
                    s.push_str(&format!(" · only {p}"));
                }
                match c.label {
                    Some(l) => s.push_str(&format!(" · delegated or labelled {l}")),
                    None => s.push_str(" · delegated"),
                }
                s
            }
            Err(_) => "an unreadable Linear trigger".into(),
        },
        _ => match serde_json::from_value::<WebhookConfig>(t.config.0.clone()) {
            Ok(c) if c.filters.is_empty() => "any signed delivery".into(),
            Ok(c) => {
                let parts: Vec<String> = c
                    .filters
                    .iter()
                    .map(|f| format!("{} = {}", f.path, f.equals.join("|")))
                    .collect();
                format!("a delivery where {}", parts.join(" and "))
            }
            Err(_) => "an unreadable webhook".into(),
        },
    }
}

async fn list(state: &Arc<RunsState>, args: &Value) -> Result<(String, Value), String> {
    let kind = text(args, "kind");
    if !kind.is_empty() {
        kind_of(args)?;
    }
    let project = text(args, "project");
    let workflow = text(args, "workflow");
    let keep = |k: &str, p: &str, w: &str| {
        (kind.is_empty() || kind == k)
            && (project.is_empty() || project == p)
            && (workflow.is_empty() || workflow == w)
    };

    let mut rows: Vec<Value> = Vec::new();
    let all = triggers::list(state).await.map_err(|(_, m)| m)?;
    let shadowed = triggers::shadowed(&all);
    for t in &all {
        if !keep(&t.kind, &t.project, &t.workflow) {
            continue;
        }
        let mut v = triggers::with_shadow(triggers::view(state, t), &shadowed, &t.id);
        v["when"] = json!(when(t));
        rows.push(v);
    }
    for s in sched::list(state).await.map_err(|(_, m)| m)? {
        if !keep("schedule", &s.project, &s.workflow) {
            continue;
        }
        rows.push(json!({
            "kind": "schedule",
            "id": s.id,
            "name": s.name,
            "project": s.project,
            "workflow": s.workflow,
            "enabled": s.enabled,
            "when": format!("{} · {}", s.cron, s.timezone),
            "cron": s.cron,
            "timezone": s.timezone,
            "task": s.task,
            "next_run_at": s.next_run_at,
            "last_run_at": s.last_run_at,
            "last_run_id": s.last_run_id,
        }));
    }
    rows.sort_by(|a, b| {
        let key = |v: &Value| {
            [
                v["workflow"].as_str().unwrap_or_default().to_string(),
                v["project"].as_str().unwrap_or_default().to_string(),
                v["name"].as_str().unwrap_or_default().to_string(),
            ]
        };
        key(a).cmp(&key(b))
    });
    let system = triggers::system(state).await;
    Ok((
        format!("{} trigger(s); Linear runs come from {system}", rows.len()),
        json!({ "system": system, "triggers": rows }),
    ))
}

// ── Saving ──────────────────────────────────────────────────────────────────

async fn save(
    state: &Arc<RunsState>,
    actor: &TriggerInfo,
    kind: &str,
    args: &Value,
) -> Result<(String, Value), String> {
    let id = text(args, "id");
    let current = if id.is_empty() {
        None
    } else {
        let t = triggers::get(state, &id).await.map_err(|(_, m)| m)?;
        if t.kind != kind {
            return Err(format!(
                "trigger `{id}` is a {} trigger, not {kind}",
                t.kind
            ));
        }
        Some(t)
    };

    let given = |k: &str| args.get(k).and_then(Value::as_str).map(str::trim);
    let project = given("project")
        .map(str::to_string)
        .or_else(|| current.as_ref().map(|t| t.project.clone()))
        .unwrap_or_default();
    let workflow = given("workflow")
        .map(str::to_string)
        .or_else(|| current.as_ref().map(|t| t.workflow.clone()))
        .unwrap_or_default();
    let enabled = args
        .get("enabled")
        .and_then(Value::as_bool)
        .or(current.as_ref().map(|t| t.enabled))
        .unwrap_or(false);

    let config = match kind {
        KIND_LINEAR => {
            let base: Option<LinearConfig> = current
                .as_ref()
                .and_then(|t| serde_json::from_value(t.config.0.clone()).ok());
            // Linear is only asked when a name has to become an id.
            let teams = if given("team").is_some() || given("status").is_some() {
                if project.is_empty() {
                    return Err("a Linear trigger needs `project` to find its team".into());
                }
                let conn = super::linear_connections::resolve_for_project(state, &project).await?;
                let client = super::linear_oauth::linear_client(state, &conn).await?;
                Some(client.discover().await.map_err(|e| e.0)?.teams)
            } else {
                None
            };
            let c = apply_linear(base, args, teams.as_deref())?;
            serde_json::to_value(c).expect("serializable")
        }
        _ => {
            let mut c: WebhookConfig = current
                .as_ref()
                .and_then(|t| serde_json::from_value(t.config.0.clone()).ok())
                .unwrap_or_default();
            apply_webhook(&mut c, args)?;
            serde_json::to_value(c).expect("serializable")
        }
    };

    let name = given("name")
        .map(str::to_string)
        .or_else(|| current.as_ref().map(|t| t.name.clone()))
        .unwrap_or_else(|| default_name(kind, &config, &workflow));
    let req = TriggerRequest {
        name,
        kind: kind.to_string(),
        project,
        workflow,
        enabled,
        config,
    };
    let admin = is_admin(state, actor).await;
    match current {
        Some(_) => {
            let t = triggers::update(state, &id, req, admin, actor.user_id.as_deref())
                .await
                .map_err(|(_, m)| m)?;
            Ok((
                format!("updated the trigger “{}”", t.name),
                triggers::view(state, &t),
            ))
        }
        None => {
            let (t, secret) = triggers::create(state, req, admin, actor.user_id.as_deref())
                .await
                .map_err(|(_, m)| m)?;
            let mut body = triggers::view(state, &t);
            let mut summary = format!(
                "created the {kind} trigger “{}” ({})",
                t.name,
                if t.enabled { "enabled" } else { "disabled" }
            );
            if let Some(secret) = secret {
                body["secret"] = json!(secret);
                summary.push_str(". Its secret is shown only now — store it with the sender");
            }
            Ok((summary, body))
        }
    }
}

fn default_name(kind: &str, config: &Value, workflow: &str) -> String {
    if kind == KIND_LINEAR {
        if let Ok(c) = serde_json::from_value::<LinearConfig>(config.clone()) {
            return format!("{} · {}", c.team_name, c.state_name);
        }
    }
    format!("{workflow} webhook")
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

/// The status `wanted` names in `team`: `(id, name)`.
fn find_state(team: &Team, wanted: &str) -> Result<(String, String), String> {
    team.states
        .iter()
        .find(|s| s.id == wanted || s.name.eq_ignore_ascii_case(wanted))
        .map(|s| (s.id.clone(), s.name.clone()))
        .ok_or_else(|| {
            let known: Vec<&str> = team.states.iter().map(|s| s.name.as_str()).collect();
            format!(
                "no status `{wanted}` in {}; it has: {}",
                team.name,
                known.join(", ")
            )
        })
}

/// Lay the call's fields over a Linear trigger's config (`base` when
/// updating). Names resolve against `teams`, fetched only when one was given.
fn apply_linear(
    base: Option<LinearConfig>,
    args: &Value,
    teams: Option<&[Team]>,
) -> Result<LinearConfig, String> {
    let given = |k: &str| args.get(k).and_then(Value::as_str).map(str::trim);
    let optional = |v: &str| (!v.is_empty()).then(|| v.to_string());
    let creating = base.is_none();
    if creating && (given("team").is_none() || given("status").is_none()) {
        return Err("a new Linear trigger needs `team` and `status`".into());
    }
    let teams = teams.unwrap_or_default();
    let mut c = base.unwrap_or(LinearConfig {
        team_id: String::new(),
        team_name: String::new(),
        state_id: String::new(),
        state_name: String::new(),
        linear_project_id: None,
        linear_project_name: None,
        label: None,
        base_branch: None,
        max_concurrent_runs: 1,
    });

    if let Some(t) = given("team") {
        let team = find_team(teams, t)?;
        if team.id != c.team_id && !creating && given("status").is_none() {
            // A status id belongs to one team; keeping the old one would wait
            // on a column the new team doesn't have.
            return Err("moving a trigger to another team needs a `status` in that team".into());
        }
        if team.id != c.team_id {
            // A Linear project belongs to its team too.
            c.linear_project_id = None;
            c.linear_project_name = None;
        }
        c.team_id = team.id.clone();
        c.team_name = team.name.clone();
    }
    if let Some(s) = given("status") {
        let team = find_team(teams, &c.team_id)?;
        let (id, name) = find_state(team, s)?;
        c.state_id = id;
        c.state_name = name;
    }
    if let Some(v) = given("label") {
        c.label = optional(v);
    }
    if let Some(v) = given("base_branch") {
        c.base_branch = optional(v);
    }
    if let Some(n) = args.get("max_concurrent_runs").and_then(Value::as_i64) {
        c.max_concurrent_runs = n as i32;
    }
    Ok(c)
}

fn apply_webhook(c: &mut WebhookConfig, args: &Value) -> Result<(), String> {
    let given = |k: &str| args.get(k).and_then(Value::as_str).map(str::trim);
    let optional = |v: &str| (!v.is_empty()).then(|| v.to_string());
    if let Some(a) = args.get("auth") {
        c.auth = serde_json::from_value(a.clone())
            .map_err(|_| "`auth` is `signature` or `token`".to_string())?;
    }
    if let Some(f) = args.get("filters") {
        c.filters = serde_json::from_value(f.clone())
            .map_err(|e| format!("`filters` must be [{{path, equals: [..]}}]: {e}"))?;
    }
    if let Some(v) = given("task_template") {
        c.task_template = v.to_string();
    }
    if let Some(v) = given("delivery_header") {
        c.delivery_header = optional(v);
    }
    if let Some(v) = given("base_branch") {
        c.base_branch = optional(v);
    }
    Ok(())
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
        sched::update(state, &id, req, actor.user_id.as_deref()).await
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
                states: vec![st("a-todo", "Todo"), st("a-rev", "In Review")],
                labels: vec![],
            },
            Team {
                id: "t-nil".into(),
                name: "niles".into(),
                key: "NIL".into(),
                states: vec![st("n-todo", "Todo")],
                labels: vec![],
            },
        ]
    }

    #[test]
    fn a_new_linear_trigger_resolves_names_and_keeps_them() {
        let c = apply_linear(
            None,
            &json!({ "team": "aih", "status": "todo", "label": "go" }),
            Some(&teams()),
        )
        .unwrap();
        assert_eq!(
            (c.team_id.as_str(), c.team_name.as_str()),
            ("t-aih", "ai-harness")
        );
        assert_eq!(
            (c.state_id.as_str(), c.state_name.as_str()),
            ("a-todo", "Todo")
        );
        assert_eq!(c.label.as_deref(), Some("go"));
    }

    #[test]
    fn a_new_linear_trigger_needs_team_and_status() {
        let e = apply_linear(None, &json!({ "team": "AIH" }), Some(&teams())).unwrap_err();
        assert!(e.contains("status"), "{e}");
    }

    #[test]
    fn an_update_needs_no_linear_and_clears_with_empty_strings() {
        let c = apply_linear(
            None,
            &json!({ "team": "AIH", "status": "Todo", "label": "go" }),
            Some(&teams()),
        )
        .unwrap();
        let c = apply_linear(
            Some(c),
            &json!({ "label": "", "max_concurrent_runs": 3 }),
            None,
        )
        .unwrap();
        assert_eq!(c.label, None);
        assert_eq!(c.max_concurrent_runs, 3);
        assert_eq!(c.state_name, "Todo");
    }

    #[test]
    fn moving_teams_needs_a_status_in_the_new_team() {
        let c = apply_linear(
            None,
            &json!({ "team": "AIH", "status": "Todo" }),
            Some(&teams()),
        )
        .unwrap();
        let e =
            apply_linear(Some(c.clone()), &json!({ "team": "NIL" }), Some(&teams())).unwrap_err();
        assert!(e.contains("status"), "{e}");
        let moved = apply_linear(
            Some(c),
            &json!({ "team": "NIL", "status": "Todo" }),
            Some(&teams()),
        )
        .unwrap();
        assert_eq!(moved.state_id, "n-todo");
    }

    #[test]
    fn an_unknown_name_says_what_exists() {
        let e = apply_linear(
            None,
            &json!({ "team": "AIH", "status": "Doing" }),
            Some(&teams()),
        )
        .unwrap_err();
        assert!(e.contains("Todo") && e.contains("In Review"), "{e}");
    }

    #[test]
    fn webhook_fields_apply_and_bad_auth_is_refused() {
        let mut c = WebhookConfig::default();
        apply_webhook(
            &mut c,
            &json!({
                "auth": "token",
                "filters": [{ "path": "body.action", "equals": ["opened"] }],
                "task_template": "Review {{body.pull_request.html_url}}"
            }),
        )
        .unwrap();
        assert_eq!(c.auth, triggers::WebhookAuth::Token);
        assert_eq!(c.filters.len(), 1);
        assert!(apply_webhook(&mut c, &json!({ "auth": "password" })).is_err());
    }

    #[test]
    fn kind_is_required_and_checked() {
        assert!(kind_of(&json!({})).is_err());
        assert!(kind_of(&json!({ "kind": "cron" })).is_err());
        assert_eq!(kind_of(&json!({ "kind": "webhook" })).unwrap(), "webhook");
    }
}
