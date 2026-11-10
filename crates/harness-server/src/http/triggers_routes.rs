//! Triggers — the model behind the Triggers page.
//!
//! A trigger is *when* (a Linear status, a webhook) plus *what* (a project and a
//! workflow). It deliberately has no status map: where the issue goes once the
//! run starts is the workflow's business, through its `linear:` steps. That is
//! the difference from a classic Linear binding, which also moved the issue
//! through In Progress / Review / Ready itself.
//!
//! Which of the two starts Linear runs is one installation-wide switch,
//! [`SETTING_TRIGGER_SYSTEM`]. Classic (the default) keeps every project's
//! bindings in charge, exactly as before; switching to triggers hands Linear to
//! the triggers here and leaves the bindings idle. Webhook triggers have no
//! classic counterpart, so they work under either.
//!
//! Inside the poller and the delegation handler a Linear trigger is turned into
//! a binding with an empty status map ([`as_binding`]), so issue selection,
//! claims, progress reporting and completion are the same code for both.

use std::collections::BTreeMap;
use std::collections::HashMap;
use std::sync::Arc;

use axum::body::Bytes;
use axum::extract::{Extension, Path as AxumPath, Query};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::Json;
use harness_persist::{LinearSource, RunTrigger, TriggerInput, KIND_LINEAR, KIND_WEBHOOK};
use hmac::{Hmac, Mac};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sha2::Sha256;
use subtle::ConstantTimeEq;

use super::accounts::{AdminOnly, ROLE_ADMIN};
use super::runs_routes::{start_run, CreateRunRequest, RunsState};

/// Settings key: which system starts Linear runs.
pub(crate) const SETTING_TRIGGER_SYSTEM: &str = "trigger_system";
/// Project bindings start Linear runs (the default).
pub(crate) const SYSTEM_CLASSIC: &str = "classic";
/// Triggers start Linear runs; bindings are idle.
pub(crate) const SYSTEM_TRIGGERS: &str = "triggers";

/// `trigger_source` on a run a webhook started.
pub(crate) const SOURCE_WEBHOOK: &str = "webhook";

/// The credential-store key holding a webhook trigger's secret. Never in the
/// trigger row itself, which is plain JSON in the database.
fn secret_key(id: &str) -> String {
    format!("webhook-trigger:{id}")
}

/// Largest payload rendered into a run's task when no template is set.
const PAYLOAD_IN_TASK_LIMIT: usize = 20_000;

pub(crate) type Refusal = (StatusCode, String);

fn err(status: StatusCode, msg: impl Into<String>) -> Response {
    (status, Json(json!({ "error": msg.into() }))).into_response()
}

// ── The switch ──────────────────────────────────────────────────────────────

/// Which system starts Linear runs: [`SYSTEM_CLASSIC`] or [`SYSTEM_TRIGGERS`].
pub(crate) async fn system(state: &Arc<RunsState>) -> &'static str {
    let stored = match state.settings_store().await {
        Ok(s) => s.get(SETTING_TRIGGER_SYSTEM).await.ok().flatten(),
        Err(_) => None,
    };
    if stored.as_deref() == Some(SYSTEM_TRIGGERS) {
        SYSTEM_TRIGGERS
    } else {
        SYSTEM_CLASSIC
    }
}

/// Whether Linear runs come from triggers rather than project bindings.
pub(crate) async fn uses_triggers(state: &Arc<RunsState>) -> bool {
    system(state).await == SYSTEM_TRIGGERS
}

/// `GET /api/triggers/system` — readable by anyone signed in, since it decides
/// what the Triggers page and every project's Linear tab show.
pub async fn get_system(Extension(state): Extension<Arc<RunsState>>) -> Response {
    Json(json!({ "system": system(&state).await })).into_response()
}

#[derive(Debug, Deserialize)]
pub struct SystemBody {
    pub system: String,
}

/// `PUT /api/triggers/system` — flip the switch. Takes effect on the poller's
/// next tick and the next delegation; nothing is migrated or deleted.
pub async fn put_system(
    _: AdminOnly,
    Extension(state): Extension<Arc<RunsState>>,
    Json(body): Json<SystemBody>,
) -> Response {
    let store = match state.settings_store().await {
        Ok(s) => s,
        Err(e) => return err(StatusCode::SERVICE_UNAVAILABLE, e),
    };
    let result = match body.system.as_str() {
        SYSTEM_TRIGGERS => store.set(SETTING_TRIGGER_SYSTEM, SYSTEM_TRIGGERS).await,
        SYSTEM_CLASSIC => store.delete(SETTING_TRIGGER_SYSTEM).await.map(|_| ()),
        other => {
            return err(
                StatusCode::BAD_REQUEST,
                format!(
                    "`system` must be `{SYSTEM_CLASSIC}` or `{SYSTEM_TRIGGERS}`, not `{other}`"
                ),
            )
        }
    };
    match result {
        Ok(()) => Json(json!({ "system": system(&state).await })).into_response(),
        Err(e) => err(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()),
    }
}

// ── Kind configs ────────────────────────────────────────────────────────────

fn one() -> i32 {
    1
}

/// A Linear trigger: an issue in `state_id` of `team_id`, delegated to the
/// harness — or, with `label`, carrying that label — starts the workflow.
///
/// Names are kept beside ids so the list reads without asking Linear.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct LinearConfig {
    pub team_id: String,
    pub team_name: String,
    pub state_id: String,
    pub state_name: String,
    /// Only issues in this Linear project of the team.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub linear_project_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub linear_project_name: Option<String>,
    /// Also start on issues with this label that nobody delegated.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub base_branch: Option<String>,
    #[serde(default = "one")]
    pub max_concurrent_runs: i32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub(crate) enum WebhookAuth {
    /// HMAC-SHA256 of the raw body with the secret, GitHub-style
    /// (`X-Hub-Signature-256: sha256=<hex>`).
    #[default]
    Signature,
    /// The secret itself, as `Authorization: Bearer`, `X-Harness-Token`, or
    /// `?token=` for senders that can set nothing else.
    Token,
}

/// One condition a delivery must meet: the value at `path` is one of `equals`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct WebhookFilter {
    /// `body.<a>.<b>` into the JSON body, or `headers.<name>`.
    pub path: String,
    pub equals: Vec<String>,
}

/// A webhook trigger: a signed request to `/api/hooks/<id>` starts the
/// workflow, with the task rendered from the payload.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[serde(deny_unknown_fields)]
pub(crate) struct WebhookConfig {
    #[serde(default)]
    pub auth: WebhookAuth,
    /// Every filter must match, or the delivery is acknowledged and ignored.
    #[serde(default)]
    pub filters: Vec<WebhookFilter>,
    /// The run's task, with `{{body.x.y}}` / `{{headers.name}}` filled in.
    /// Empty: the task is the payload itself.
    #[serde(default)]
    pub task_template: String,
    /// Header naming a delivery, so a sender's retry starts nothing new.
    /// Empty: the common ones (`X-GitHub-Delivery`, `X-Request-Id`, …).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub delivery_header: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub base_branch: Option<String>,
}

/// What a create or update asks for.
#[derive(Debug, Clone, Deserialize)]
pub struct TriggerRequest {
    pub name: String,
    #[serde(default)]
    pub kind: String,
    pub project: String,
    pub workflow: String,
    #[serde(default)]
    pub enabled: bool,
    #[serde(default)]
    pub config: Value,
}

fn trimmed(s: Option<String>) -> Option<String> {
    s.map(|v| v.trim().to_string()).filter(|v| !v.is_empty())
}

/// Check a request and normalise its config to the kind's shape. Pure, so the
/// rules are testable without a database; existence of the project is checked
/// by the caller.
pub(crate) fn validate(req: TriggerRequest) -> Result<TriggerInput, String> {
    let name = req.name.trim().to_string();
    if name.is_empty() {
        return Err("a trigger needs a `name`".into());
    }
    if name.chars().count() > 120 {
        return Err("`name` is longer than 120 characters".into());
    }
    let project = req.project.trim().to_string();
    let workflow = req.workflow.trim().to_string();
    if project.is_empty() || workflow.is_empty() {
        return Err("a trigger needs a `project` and a `workflow`".into());
    }
    let config = match req.kind.as_str() {
        KIND_LINEAR => {
            let mut c: LinearConfig = serde_json::from_value(req.config)
                .map_err(|e| format!("Linear trigger config: {e}"))?;
            for (field, v) in [
                ("team_id", &c.team_id),
                ("team_name", &c.team_name),
                ("state_id", &c.state_id),
                ("state_name", &c.state_name),
            ] {
                if v.trim().is_empty() {
                    return Err(format!("a Linear trigger needs `{field}`"));
                }
            }
            if !(1..=20).contains(&c.max_concurrent_runs) {
                return Err("`max_concurrent_runs` must be between 1 and 20".into());
            }
            c.label = trimmed(c.label);
            c.base_branch = trimmed(c.base_branch);
            c.linear_project_id = trimmed(c.linear_project_id);
            // A name only means something beside an id.
            c.linear_project_name = c
                .linear_project_id
                .as_ref()
                .and(trimmed(c.linear_project_name));
            serde_json::to_value(c).expect("serializable")
        }
        KIND_WEBHOOK => {
            let mut c: WebhookConfig = if req.config.is_null() {
                WebhookConfig::default()
            } else {
                serde_json::from_value(req.config)
                    .map_err(|e| format!("webhook trigger config: {e}"))?
            };
            for f in &c.filters {
                let p = f.path.trim();
                let rooted = p.starts_with("body.") || p.starts_with("headers.");
                if !rooted || p.ends_with('.') {
                    return Err(format!(
                        "filter path `{p}` must start with `body.` or `headers.`"
                    ));
                }
                if f.equals.is_empty() {
                    return Err(format!("filter on `{p}` needs at least one value"));
                }
            }
            if c.task_template.len() > 20_000 {
                return Err("`task_template` is longer than 20000 characters".into());
            }
            c.delivery_header = trimmed(c.delivery_header);
            c.base_branch = trimmed(c.base_branch);
            serde_json::to_value(c).expect("serializable")
        }
        other => {
            return Err(format!(
                "`kind` must be `{KIND_LINEAR}` or `{KIND_WEBHOOK}`, not `{other}`"
            ))
        }
    };
    Ok(TriggerInput {
        name,
        kind: req.kind,
        project,
        workflow,
        enabled: req.enabled,
        config,
    })
}

// ── One source, one trigger ─────────────────────────────────────────────────

/// A Linear trigger's config, when it is one and it parses.
fn linear_config(t: &RunTrigger) -> Option<LinearConfig> {
    (t.kind == KIND_LINEAR)
        .then(|| serde_json::from_value(t.config.0.clone()).ok())
        .flatten()
}

/// Whether one issue can match both triggers: the same team and status, and
/// Linear-project scopes that meet (either is the whole team, or both name
/// the same project). Two triggers like that both start a run on the same
/// issue, and whichever workflow moves the issue first decides what the other
/// one finds.
///
/// The label doesn't keep them apart. It only *adds* issues nobody
/// delegated; an issue delegated to the harness matches both regardless. Nor
/// does the harness project: a Linear trigger matches issues by their place
/// in Linear, not by which repository the run will work in.
pub(crate) fn same_source(a: &LinearConfig, b: &LinearConfig) -> bool {
    a.team_id == b.team_id
        && a.state_id == b.state_id
        && match (&a.linear_project_id, &b.linear_project_id) {
            (Some(x), Some(y)) => x == y,
            _ => true,
        }
}

/// The trigger among `existing` (other than `own_id`) whose source a Linear
/// trigger with `config` would share.
pub(crate) fn clash<'a>(
    config: &LinearConfig,
    own_id: Option<&str>,
    existing: &'a [RunTrigger],
) -> Option<&'a RunTrigger> {
    existing.iter().find(|t| {
        Some(t.id.as_str()) != own_id && linear_config(t).is_some_and(|c| same_source(config, &c))
    })
}

/// Why a second trigger on a source is refused, and what to do instead.
fn clash_message(config: &LinearConfig, other: &RunTrigger) -> String {
    let scope = config
        .linear_project_name
        .as_deref()
        .map(|p| format!(" ({p})"))
        .unwrap_or_default();
    format!(
        "\"{}\" already starts `{}` in {} when an issue reaches {} in {}{scope}. \
         One status starts one workflow, or both would run on the same issue. \
         To do different things for different issues, edit that trigger's \
         workflow and let a route node choose.",
        other.name, other.workflow, other.project, config.state_name, config.team_name
    )
}

/// Enabled Linear triggers that share a source with an *older* enabled one,
/// mapped to it. The older one fires and these don't: a pair saved before
/// overlaps were refused shouldn't keep fighting over issues.
pub(crate) fn shadowed(triggers: &[RunTrigger]) -> HashMap<String, (String, String)> {
    let mut live: Vec<(&RunTrigger, LinearConfig)> = triggers
        .iter()
        .filter(|t| t.enabled)
        .filter_map(|t| linear_config(t).map(|c| (t, c)))
        .collect();
    live.sort_by(|(a, _), (b, _)| a.created_at.cmp(&b.created_at).then(a.id.cmp(&b.id)));
    let mut out = HashMap::new();
    for (i, (t, c)) in live.iter().enumerate() {
        if let Some((older, _)) = live[..i]
            .iter()
            .find(|(o, oc)| !out.contains_key(&o.id) && same_source(c, oc))
        {
            out.insert(t.id.clone(), (older.id.clone(), older.name.clone()));
        }
    }
    out
}

/// Refuse a Linear trigger whose source another trigger already has.
async fn refuse_clash(
    state: &Arc<RunsState>,
    input: &TriggerInput,
    own_id: Option<&str>,
) -> Result<(), Refusal> {
    if input.kind != KIND_LINEAR {
        return Ok(());
    }
    let config: LinearConfig = serde_json::from_value(input.config.clone())
        .map_err(|e| (StatusCode::BAD_REQUEST, e.to_string()))?;
    let existing = list(state).await?;
    match clash(&config, own_id, &existing) {
        Some(other) => Err((StatusCode::CONFLICT, clash_message(&config, other))),
        None => Ok(()),
    }
}

/// A Linear trigger as the poller and delegation see it: a binding with no
/// status map, one attempt, always live. `None` when the stored config doesn't
/// parse (a row written by a newer server, say).
pub(crate) fn as_binding(t: &RunTrigger) -> Option<LinearSource> {
    if t.kind != KIND_LINEAR {
        return None;
    }
    let c: LinearConfig = serde_json::from_value(t.config.0.clone()).ok()?;
    Some(LinearSource {
        project: t.project.clone(),
        workflow: t.workflow.clone(),
        team_id: c.team_id,
        team_name: c.team_name,
        source_state_id: c.state_id,
        failed_label: None,
        in_progress_state_id: None,
        review_state_id: None,
        ready_state_id: None,
        piece_ready_state_id: None,
        epic_review_state_id: None,
        base_branch: c.base_branch,
        poll_interval_secs: 60,
        max_concurrent_runs: c.max_concurrent_runs,
        max_attempts: 1,
        enabled: t.enabled,
        live: true,
        linear_project_id: c.linear_project_id,
        linear_project_name: c.linear_project_name,
        trigger_label: c.label,
        created_at: t.created_at,
        updated_at: t.updated_at,
    })
}

/// Every Linear trigger, as bindings, paired with its id — less any that
/// share a source with an older one (see [`shadowed`]).
pub(crate) async fn linear_bindings(state: &Arc<RunsState>) -> Vec<(String, LinearSource)> {
    let Ok(store) = state.trigger_store().await else {
        return Vec::new();
    };
    let rows = store.list().await.unwrap_or_default();
    let shadowed = shadowed(&rows);
    rows.iter()
        .filter(|t| match shadowed.get(&t.id) {
            Some((_, older)) => {
                tracing::warn!(
                    "trigger \"{}\" shares its Linear status with \"{older}\", which fires instead",
                    t.name
                );
                false
            }
            None => true,
        })
        .filter_map(|t| as_binding(t).map(|b| (t.id.clone(), b)))
        .collect()
}

/// The trigger a binding produced by [`as_binding`] came from.
pub(crate) fn trigger_id_for(pairs: &[(String, LinearSource)], b: &LinearSource) -> Option<String> {
    pairs
        .iter()
        .find(|(_, t)| {
            t.project == b.project
                && t.workflow == b.workflow
                && t.team_id == b.team_id
                && t.source_state_id == b.source_state_id
                && t.linear_project_id == b.linear_project_id
                && t.trigger_label == b.trigger_label
        })
        .map(|(id, _)| id.clone())
}

// ── CRUD, shared by HTTP and MCP ────────────────────────────────────────────

/// A trigger as the API shows it: the row, plus where a webhook is reached.
pub(crate) fn view(state: &Arc<RunsState>, t: &RunTrigger) -> Value {
    let mut v = serde_json::to_value(t).unwrap_or_else(|_| json!({}));
    if t.kind == KIND_WEBHOOK {
        v["url"] = json!(state
            .public_url()
            .map(|base| format!("{base}/api/hooks/{}", t.id)));
    }
    v
}

pub(crate) async fn list(state: &Arc<RunsState>) -> Result<Vec<RunTrigger>, Refusal> {
    let store = state
        .trigger_store()
        .await
        .map_err(|e| (StatusCode::SERVICE_UNAVAILABLE, e))?;
    store
        .list()
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))
}

pub(crate) async fn get(state: &Arc<RunsState>, id: &str) -> Result<RunTrigger, Refusal> {
    let store = state
        .trigger_store()
        .await
        .map_err(|e| (StatusCode::SERVICE_UNAVAILABLE, e))?;
    store
        .get(id)
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?
        .ok_or_else(|| (StatusCode::NOT_FOUND, format!("no trigger `{id}`")))
}

/// Webhook triggers open an endpoint anyone on the internet can call, so only
/// an administrator sets them up. A Linear trigger is ordinary configuration,
/// as a binding always was.
fn may_change(kind: &str, admin: bool) -> Result<(), Refusal> {
    if kind == KIND_WEBHOOK && !admin {
        return Err((
            StatusCode::FORBIDDEN,
            "only an administrator can create or change a webhook trigger".into(),
        ));
    }
    Ok(())
}

fn mint_secret() -> String {
    format!(
        "whsec_{}{}",
        uuid::Uuid::new_v4().simple(),
        uuid::Uuid::new_v4().simple()
    )
}

async fn store_secret(state: &Arc<RunsState>, id: &str) -> Result<String, Refusal> {
    let creds = state.cred_store().await.map_err(|e| {
        (
            StatusCode::SERVICE_UNAVAILABLE,
            format!("webhook secrets need the encrypted credential store: {e}"),
        )
    })?;
    let secret = mint_secret();
    let fields = BTreeMap::from([("secret".to_string(), secret.clone())]);
    creds
        .set(&secret_key(id), &fields)
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    Ok(secret)
}

/// Create a trigger. A webhook's secret is returned here and on rotation only.
pub(crate) async fn create(
    state: &Arc<RunsState>,
    req: TriggerRequest,
    admin: bool,
    created_by: Option<&str>,
) -> Result<(RunTrigger, Option<String>), Refusal> {
    let input = validate(req).map_err(|e| (StatusCode::BAD_REQUEST, e))?;
    may_change(&input.kind, admin)?;
    super::linear_source_routes::project_exists(state, &input.project).await?;
    refuse_clash(state, &input, None).await?;
    let store = state
        .trigger_store()
        .await
        .map_err(|e| (StatusCode::SERVICE_UNAVAILABLE, e))?;
    let id = uuid::Uuid::new_v4().to_string();
    // The secret first: a webhook row with no secret would refuse every call.
    let secret = if input.kind == KIND_WEBHOOK {
        Some(store_secret(state, &id).await?)
    } else {
        None
    };
    let row = store
        .create(&id, &input, created_by)
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    Ok((row, secret))
}

/// Replace a trigger's settings. Its kind is fixed at creation.
pub(crate) async fn update(
    state: &Arc<RunsState>,
    id: &str,
    mut req: TriggerRequest,
    admin: bool,
    updated_by: Option<&str>,
) -> Result<RunTrigger, Refusal> {
    let current = get(state, id).await?;
    req.kind = current.kind.clone();
    let input = validate(req).map_err(|e| (StatusCode::BAD_REQUEST, e))?;
    may_change(&input.kind, admin)?;
    super::linear_source_routes::project_exists(state, &input.project).await?;
    refuse_clash(state, &input, Some(id)).await?;
    let store = state
        .trigger_store()
        .await
        .map_err(|e| (StatusCode::SERVICE_UNAVAILABLE, e))?;
    store
        .update(id, &input, updated_by)
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?
        .ok_or_else(|| (StatusCode::NOT_FOUND, format!("no trigger `{id}`")))
}

pub(crate) async fn delete(state: &Arc<RunsState>, id: &str, admin: bool) -> Result<(), Refusal> {
    let current = get(state, id).await?;
    may_change(&current.kind, admin)?;
    let store = state
        .trigger_store()
        .await
        .map_err(|e| (StatusCode::SERVICE_UNAVAILABLE, e))?;
    store
        .delete(id)
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    if current.kind == KIND_WEBHOOK {
        if let Ok(creds) = state.cred_store().await {
            let _ = creds.delete(&secret_key(id)).await;
        }
    }
    Ok(())
}

/// Replace a webhook trigger's secret; the old one stops working at once.
pub(crate) async fn rotate_secret(
    state: &Arc<RunsState>,
    id: &str,
    admin: bool,
) -> Result<String, Refusal> {
    let current = get(state, id).await?;
    if current.kind != KIND_WEBHOOK {
        return Err((
            StatusCode::BAD_REQUEST,
            "only a webhook trigger has a secret".into(),
        ));
    }
    may_change(&current.kind, admin)?;
    store_secret(state, id).await
}

/// Whether the HTTP caller is an administrator — or, before the install has
/// accounts, the operator by definition (see [`AdminOnly`]).
async fn caller_is_admin(state: &Arc<RunsState>, headers: &HeaderMap) -> bool {
    use super::accounts::{authenticated_user, mode, Mode};
    if mode(state).await != Mode::Accounts {
        return true;
    }
    matches!(authenticated_user(state, headers).await, Some(u) if u.role == ROLE_ADMIN)
}

// ── HTTP routes ─────────────────────────────────────────────────────────────

/// `GET /api/triggers`
pub async fn list_triggers(Extension(state): Extension<Arc<RunsState>>) -> Response {
    match list(&state).await {
        Ok(rows) => {
            let shadowed = shadowed(&rows);
            let people = super::accounts::actor_labels(&state).await;
            let views: Vec<Value> = rows
                .iter()
                .map(|t| {
                    super::accounts::with_actors(
                        with_shadow(view(&state, t), &shadowed, &t.id),
                        &people,
                    )
                })
                .collect();
            Json(json!({ "triggers": views })).into_response()
        }
        Err((s, m)) => err(s, m),
    }
}

/// Add `shadowed_by: {id, name}` to a trigger that an older one on the same
/// source keeps from firing.
pub(crate) fn with_shadow(
    mut v: Value,
    shadowed: &HashMap<String, (String, String)>,
    id: &str,
) -> Value {
    if let Some((older_id, older_name)) = shadowed.get(id) {
        v["shadowed_by"] = json!({ "id": older_id, "name": older_name });
    }
    v
}

/// `GET /api/triggers/{id}`
pub async fn get_trigger(
    Extension(state): Extension<Arc<RunsState>>,
    AxumPath(id): AxumPath<String>,
) -> Response {
    match get(&state, &id).await {
        Ok(t) => {
            let all = list(&state).await.unwrap_or_default();
            let people = super::accounts::actor_labels(&state).await;
            Json(super::accounts::with_actors(
                with_shadow(view(&state, &t), &shadowed(&all), &t.id),
                &people,
            ))
            .into_response()
        }
        Err((s, m)) => err(s, m),
    }
}

/// `POST /api/triggers`
pub async fn create_trigger(
    Extension(state): Extension<Arc<RunsState>>,
    headers: HeaderMap,
    Json(req): Json<TriggerRequest>,
) -> Response {
    let admin = caller_is_admin(&state, &headers).await;
    let (user_id, _) = super::accounts::caller_trigger(&state, &headers).await;
    match create(&state, req, admin, user_id.as_deref()).await {
        Ok((t, secret)) => {
            let people = super::accounts::actor_labels(&state).await;
            let trigger = super::accounts::with_actors(view(&state, &t), &people);
            (
                StatusCode::CREATED,
                Json(json!({ "trigger": trigger, "secret": secret })),
            )
                .into_response()
        }
        Err((s, m)) => err(s, m),
    }
}

/// `PUT /api/triggers/{id}`
pub async fn update_trigger(
    Extension(state): Extension<Arc<RunsState>>,
    headers: HeaderMap,
    AxumPath(id): AxumPath<String>,
    Json(req): Json<TriggerRequest>,
) -> Response {
    let admin = caller_is_admin(&state, &headers).await;
    let (user_id, _) = super::accounts::caller_trigger(&state, &headers).await;
    match update(&state, &id, req, admin, user_id.as_deref()).await {
        Ok(t) => {
            let people = super::accounts::actor_labels(&state).await;
            Json(super::accounts::with_actors(view(&state, &t), &people)).into_response()
        }
        Err((s, m)) => err(s, m),
    }
}

/// `DELETE /api/triggers/{id}`
pub async fn delete_trigger(
    Extension(state): Extension<Arc<RunsState>>,
    headers: HeaderMap,
    AxumPath(id): AxumPath<String>,
) -> Response {
    let admin = caller_is_admin(&state, &headers).await;
    match delete(&state, &id, admin).await {
        Ok(()) => StatusCode::NO_CONTENT.into_response(),
        Err((s, m)) => err(s, m),
    }
}

/// `POST /api/triggers/{id}/secret` — a new webhook secret, shown once.
pub async fn rotate_trigger_secret(
    Extension(state): Extension<Arc<RunsState>>,
    headers: HeaderMap,
    AxumPath(id): AxumPath<String>,
) -> Response {
    let admin = caller_is_admin(&state, &headers).await;
    match rotate_secret(&state, &id, admin).await {
        Ok(secret) => Json(json!({ "secret": secret })).into_response(),
        Err((s, m)) => err(s, m),
    }
}

// ── The webhook receiver ────────────────────────────────────────────────────

/// Headers that commonly name a delivery, tried when the trigger names none.
const DELIVERY_HEADERS: &[&str] = &[
    "x-github-delivery",
    "x-gitlab-event-uuid",
    "x-request-id",
    "x-delivery-id",
    "idempotency-key",
];

/// Headers carrying an HMAC signature, tried in order.
const SIGNATURE_HEADERS: &[&str] = &[
    "x-hub-signature-256",
    "x-harness-signature",
    "x-signature-256",
    "x-signature",
];

/// The request as filters and templates see it: `headers.*` (lowercased names)
/// and `body.*`.
pub(crate) fn delivery_context(headers: &HeaderMap, body: &Value) -> Value {
    let mut h = serde_json::Map::new();
    for (name, value) in headers {
        // Credentials stay out of anything that can end up in a run's task.
        let n = name.as_str().to_ascii_lowercase();
        if matches!(n.as_str(), "authorization" | "cookie" | "x-harness-token")
            || SIGNATURE_HEADERS.contains(&n.as_str())
        {
            continue;
        }
        if let Ok(v) = value.to_str() {
            h.insert(n, json!(v));
        }
    }
    json!({ "headers": h, "body": body })
}

/// The value at a dotted path (`body.pull_request.number`, `headers.x-github-event`),
/// as text. Array elements are addressed by index.
pub(crate) fn lookup(ctx: &Value, path: &str) -> Option<String> {
    let mut cur = ctx;
    // Header names contain dots never and hyphens often; split only on dots.
    for part in path.trim().split('.') {
        cur = match cur {
            Value::Object(m) => m.get(part)?,
            Value::Array(a) => a.get(part.parse::<usize>().ok()?)?,
            _ => return None,
        };
    }
    match cur {
        Value::Null => None,
        Value::String(s) => Some(s.clone()),
        other => Some(other.to_string()),
    }
}

/// `Ok` when every filter matches; otherwise why the delivery is ignored.
pub(crate) fn filters_match(filters: &[WebhookFilter], ctx: &Value) -> Result<(), String> {
    for f in filters {
        match lookup(ctx, &f.path) {
            Some(v) if f.equals.contains(&v) => {}
            Some(v) => {
                return Err(format!(
                    "`{}` is `{v}`, not {}",
                    f.path,
                    f.equals.join(" or ")
                ))
            }
            None => return Err(format!("`{}` is missing", f.path)),
        }
    }
    Ok(())
}

/// Fill `{{ path }}` placeholders from the delivery. A path with no value
/// renders empty rather than failing the delivery.
pub(crate) fn render(template: &str, ctx: &Value) -> String {
    let mut out = String::with_capacity(template.len());
    let mut rest = template;
    while let Some(start) = rest.find("{{") {
        out.push_str(&rest[..start]);
        let after = &rest[start + 2..];
        match after.find("}}") {
            Some(end) => {
                out.push_str(&lookup(ctx, after[..end].trim()).unwrap_or_default());
                rest = &after[end + 2..];
            }
            None => {
                out.push_str(&rest[start..]);
                rest = "";
            }
        }
    }
    out.push_str(rest);
    out
}

/// The run's task when the trigger sets no template: the payload, bounded.
fn payload_task(name: &str, body: &Value) -> String {
    let mut pretty = serde_json::to_string_pretty(body).unwrap_or_default();
    if pretty.len() > PAYLOAD_IN_TASK_LIMIT {
        let mut cut = PAYLOAD_IN_TASK_LIMIT;
        while !pretty.is_char_boundary(cut) {
            cut -= 1;
        }
        pretty.truncate(cut);
        pretty.push_str("\n… (truncated)");
    }
    format!("The webhook trigger `{name}` received this payload:\n\n```json\n{pretty}\n```")
}

fn hmac_hex(secret: &str, body: &[u8]) -> String {
    let mut mac = Hmac::<Sha256>::new_from_slice(secret.as_bytes()).expect("any key length");
    mac.update(body);
    mac.finalize()
        .into_bytes()
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

fn same(a: &str, b: &str) -> bool {
    a.len() == b.len() && bool::from(a.as_bytes().ct_eq(b.as_bytes()))
}

/// Whether the delivery proves it knows the secret.
pub(crate) fn authentic(
    auth: WebhookAuth,
    secret: &str,
    headers: &HeaderMap,
    query_token: Option<&str>,
    body: &[u8],
) -> bool {
    let header = |name: &str| headers.get(name).and_then(|v| v.to_str().ok());
    match auth {
        WebhookAuth::Signature => {
            let expected = hmac_hex(secret, body);
            SIGNATURE_HEADERS
                .iter()
                .filter_map(|h| header(h))
                .any(|got| {
                    let got = got.trim();
                    let hex = got.strip_prefix("sha256=").unwrap_or(got);
                    same(&hex.to_ascii_lowercase(), &expected)
                })
        }
        WebhookAuth::Token => {
            let bearer = header("authorization").and_then(|v| v.strip_prefix("Bearer "));
            [bearer, header("x-harness-token"), query_token]
                .into_iter()
                .flatten()
                .any(|got| same(got.trim(), secret))
        }
    }
}

#[derive(Debug, Deserialize, Default)]
pub struct HookQuery {
    #[serde(default)]
    pub token: Option<String>,
}

/// `POST /api/hooks/{id}` — a delivery for a webhook trigger.
///
/// Exempt from the session middleware: the sender is a third party. Each
/// trigger authenticates its own deliveries with its own secret, and an unknown,
/// disabled or non-webhook id gets the same 404 so ids can't be probed.
pub async fn receive_hook(
    Extension(state): Extension<Arc<RunsState>>,
    AxumPath(id): AxumPath<String>,
    Query(q): Query<HookQuery>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    let not_found = || err(StatusCode::NOT_FOUND, "no such hook");
    let Ok(store) = state.trigger_store().await else {
        return err(StatusCode::SERVICE_UNAVAILABLE, "triggers are unavailable");
    };
    let trigger = match store.get(&id).await {
        Ok(Some(t)) if t.kind == KIND_WEBHOOK && t.enabled => t,
        Ok(_) => return not_found(),
        Err(e) => return err(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()),
    };
    let cfg: WebhookConfig = serde_json::from_value(trigger.config.0.clone()).unwrap_or_default();

    let secret = match state.cred_store().await {
        Ok(creds) => creds
            .get(&secret_key(&id))
            .await
            .ok()
            .flatten()
            .and_then(|f| f.get("secret").cloned()),
        Err(_) => None,
    };
    let Some(secret) = secret else {
        return err(
            StatusCode::SERVICE_UNAVAILABLE,
            "this hook has no secret — rotate it on the trigger's page",
        );
    };
    if !authentic(cfg.auth, &secret, &headers, q.token.as_deref(), &body) {
        return err(
            StatusCode::UNAUTHORIZED,
            "the delivery is not signed with this hook's secret",
        );
    }

    let delivery = match cfg.delivery_header.as_deref() {
        Some(h) => headers
            .get(h.to_ascii_lowercase())
            .and_then(|v| v.to_str().ok()),
        None => DELIVERY_HEADERS
            .iter()
            .find_map(|h| headers.get(*h).and_then(|v| v.to_str().ok())),
    };
    if let Some(delivery) = delivery {
        match store.first_delivery(&id, delivery).await {
            Ok(true) => {}
            Ok(false) => return Json(json!({ "duplicate": delivery })).into_response(),
            Err(e) => return err(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()),
        }
    }

    let payload: Value = if body.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&body)
            .unwrap_or_else(|_| Value::String(String::from_utf8_lossy(&body).into_owned()))
    };
    let ctx = delivery_context(&headers, &payload);
    if let Err(reason) = filters_match(&cfg.filters, &ctx) {
        return Json(json!({ "ignored": reason })).into_response();
    }

    let description = if cfg.task_template.trim().is_empty() {
        payload_task(&trigger.name, &payload)
    } else {
        render(&cfg.task_template, &ctx)
    };
    let req = CreateRunRequest {
        triggered_by: None,
        trigger_source: Some(SOURCE_WEBHOOK.to_string()),
        trigger_actor: Some(format!("webhook “{}”", trigger.name)),
        workflow: trigger.workflow.clone(),
        title: Some(format!(
            "{} — {}",
            trigger.name,
            chrono::Utc::now().format("%Y-%m-%d %H:%M")
        )),
        issue_id: None,
        description,
        args: String::new(),
        real: true,
        base_branch: cfg.base_branch.clone(),
        project: Some(trigger.project.clone()),
        swap_from: None,
        swap_to: None,
        ab_pair_id: None,
        ab_arm: None,
        ab_label: None,
    };
    match start_run(&state, req).await {
        Ok(run_id) => {
            let _ = store.record_fired(&id, &run_id).await;
            (StatusCode::ACCEPTED, Json(json!({ "run_id": run_id }))).into_response()
        }
        Err((status, message)) => {
            let _ = store.record_error(&id, &message).await;
            err(status, message)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::http::HeaderValue;

    fn req(kind: &str, config: Value) -> TriggerRequest {
        TriggerRequest {
            name: "  Todo → task ".into(),
            kind: kind.into(),
            project: "ai-harness".into(),
            workflow: "task".into(),
            enabled: true,
            config,
        }
    }

    fn linear_row(id: &str, age_mins: i64, enabled: bool, config: Value) -> RunTrigger {
        let at = chrono::Utc::now() - chrono::Duration::minutes(age_mins);
        RunTrigger {
            id: id.into(),
            name: format!("trigger {id}"),
            kind: KIND_LINEAR.into(),
            project: "niles".into(),
            workflow: "task".into(),
            enabled,
            config: sqlx::types::Json(config),
            last_fired_at: None,
            last_run_id: None,
            last_error: None,
            created_by: None,
            created_at: at,
            updated_at: at,
            updated_by: None,
        }
    }

    fn todo(project: Option<&str>) -> LinearConfig {
        let mut c: LinearConfig = serde_json::from_value(linear_config()).unwrap();
        c.linear_project_id = project.map(str::to_string);
        c.label = None;
        c
    }

    #[test]
    fn one_status_has_one_trigger_unless_their_linear_projects_differ() {
        let existing = [linear_row(
            "a",
            10,
            true,
            serde_json::to_value(todo(None)).unwrap(),
        )];
        // Same team and status: refused, whatever the harness project or label.
        let mut labelled = todo(None);
        labelled.label = Some("harness".into());
        assert_eq!(
            clash(&labelled, None, &existing).map(|t| t.id.as_str()),
            Some("a")
        );
        // A narrower scope still meets an unscoped one.
        assert!(clash(&todo(Some("p1")), None, &existing).is_some());
        // Editing the trigger itself isn't a clash with itself.
        assert!(clash(&todo(None), Some("a"), &existing).is_none());
        // Another status is another source.
        let mut review = todo(None);
        review.state_id = "s2".into();
        assert!(clash(&review, None, &existing).is_none());
        // Two different Linear projects of one team never see the same issue.
        let scoped = [linear_row(
            "b",
            10,
            true,
            serde_json::to_value(todo(Some("p1"))).unwrap(),
        )];
        assert!(clash(&todo(Some("p2")), None, &scoped).is_none());
        assert!(clash(&todo(Some("p1")), None, &scoped).is_some());
    }

    #[test]
    fn of_two_triggers_already_sharing_a_source_the_older_enabled_one_fires() {
        let cfg = serde_json::to_value(todo(None)).unwrap();
        let rows = [
            linear_row("new", 1, true, cfg.clone()),
            linear_row("old", 60, true, cfg.clone()),
            linear_row("off", 120, false, cfg.clone()),
        ];
        let s = shadowed(&rows);
        assert_eq!(s.len(), 1);
        assert_eq!(
            s["new"].0, "old",
            "a disabled one doesn't hold anything back"
        );
    }

    fn linear_config() -> Value {
        json!({
            "team_id": "t1", "team_name": "AIH",
            "state_id": "s1", "state_name": "Todo",
            "label": "  ", "linear_project_name": "orphan"
        })
    }

    #[test]
    fn a_linear_trigger_is_normalised() {
        let input = validate(req(KIND_LINEAR, linear_config())).unwrap();
        assert_eq!(input.name, "Todo → task");
        let c: LinearConfig = serde_json::from_value(input.config).unwrap();
        assert_eq!(c.label, None, "a blank label is no label");
        assert_eq!(c.linear_project_name, None, "a project name needs its id");
        assert_eq!(c.max_concurrent_runs, 1);
    }

    #[test]
    fn a_linear_trigger_needs_its_team_and_status() {
        let mut c = linear_config();
        c["state_id"] = json!("");
        let e = validate(req(KIND_LINEAR, c)).unwrap_err();
        assert!(e.contains("state_id"), "{e}");
        // A status map is not part of a trigger; asking for one is refused,
        // not silently dropped.
        let mut c = linear_config();
        c["in_progress_state_id"] = json!("s2");
        assert!(validate(req(KIND_LINEAR, c)).is_err());
    }

    #[test]
    fn unknown_kinds_and_bad_filters_are_refused() {
        assert!(validate(req("cron", json!({}))).is_err());
        let bad = json!({ "filters": [{ "path": "action", "equals": ["opened"] }] });
        assert!(validate(req(KIND_WEBHOOK, bad)).is_err());
        let empty = json!({ "filters": [{ "path": "body.action", "equals": [] }] });
        assert!(validate(req(KIND_WEBHOOK, empty)).is_err());
        assert!(
            validate(req(KIND_WEBHOOK, Value::Null)).is_ok(),
            "all defaults"
        );
    }

    #[test]
    fn a_linear_trigger_becomes_a_binding_without_a_status_map() {
        let input = validate(req(KIND_LINEAR, linear_config())).unwrap();
        let now = chrono::Utc::now();
        let t = RunTrigger {
            id: "id1".into(),
            name: input.name,
            kind: input.kind,
            project: input.project,
            workflow: input.workflow,
            enabled: true,
            config: sqlx::types::Json(input.config),
            last_fired_at: None,
            last_run_id: None,
            last_error: None,
            created_by: None,
            created_at: now,
            updated_at: now,
            updated_by: None,
        };
        let b = as_binding(&t).unwrap();
        assert_eq!(b.source_state_id, "s1");
        assert!(b.in_progress_state_id.is_none() && b.ready_state_id.is_none());
        assert!(b.failed_label.is_none() && b.live);
        let pairs = vec![("id1".to_string(), b.clone())];
        assert_eq!(trigger_id_for(&pairs, &b).as_deref(), Some("id1"));
    }

    fn ctx() -> Value {
        let mut h = HeaderMap::new();
        h.insert("X-GitHub-Event", HeaderValue::from_static("pull_request"));
        h.insert("Authorization", HeaderValue::from_static("Bearer s3cret"));
        delivery_context(
            &h,
            &json!({ "action": "opened", "number": 7, "pull_request": { "html_url": "https://x/7" }, "labels": [{"name": "bug"}] }),
        )
    }

    #[test]
    fn paths_reach_headers_body_and_array_items() {
        let c = ctx();
        assert_eq!(
            lookup(&c, "headers.x-github-event").as_deref(),
            Some("pull_request")
        );
        assert_eq!(lookup(&c, "body.number").as_deref(), Some("7"));
        assert_eq!(lookup(&c, "body.labels.0.name").as_deref(), Some("bug"));
        assert_eq!(lookup(&c, "body.nope"), None);
        assert_eq!(
            lookup(&c, "headers.authorization"),
            None,
            "credentials never reach a task"
        );
    }

    #[test]
    fn filters_say_why_a_delivery_is_ignored() {
        let c = ctx();
        let opened = WebhookFilter {
            path: "body.action".into(),
            equals: vec!["opened".into(), "reopened".into()],
        };
        assert!(filters_match(&[opened], &c).is_ok());
        let closed = WebhookFilter {
            path: "body.action".into(),
            equals: vec!["closed".into()],
        };
        let why = filters_match(&[closed], &c).unwrap_err();
        assert!(why.contains("opened") && why.contains("closed"), "{why}");
    }

    #[test]
    fn templates_fill_known_paths_and_blank_unknown_ones() {
        let c = ctx();
        assert_eq!(
            render(
                "Review {{ body.pull_request.html_url }} ({{headers.x-github-event}}){{body.none}}",
                &c
            ),
            "Review https://x/7 (pull_request)"
        );
        assert_eq!(
            render("unclosed {{ body.number", &c),
            "unclosed {{ body.number"
        );
    }

    #[test]
    fn a_signature_or_token_must_match_the_secret() {
        let body = br#"{"a":1}"#;
        let mut h = HeaderMap::new();
        let sig = format!("sha256={}", hmac_hex("k", body));
        h.insert("X-Hub-Signature-256", HeaderValue::from_str(&sig).unwrap());
        assert!(authentic(WebhookAuth::Signature, "k", &h, None, body));
        assert!(!authentic(WebhookAuth::Signature, "other", &h, None, body));
        assert!(!authentic(
            WebhookAuth::Signature,
            "k",
            &h,
            None,
            b"tampered"
        ));

        let mut t = HeaderMap::new();
        t.insert("Authorization", HeaderValue::from_static("Bearer k"));
        assert!(authentic(WebhookAuth::Token, "k", &t, None, body));
        assert!(authentic(
            WebhookAuth::Token,
            "k",
            &HeaderMap::new(),
            Some("k"),
            body
        ));
        assert!(!authentic(
            WebhookAuth::Token,
            "k",
            &HeaderMap::new(),
            None,
            body
        ));
        // A signature doesn't satisfy token mode, nor the other way round.
        assert!(!authentic(WebhookAuth::Token, "k", &h, None, body));
    }

    #[test]
    fn the_default_task_is_the_payload_bounded() {
        let big = json!({ "blob": "x".repeat(PAYLOAD_IN_TASK_LIMIT * 2) });
        let task = payload_task("deploys", &big);
        assert!(task.contains("(truncated)"));
        assert!(task.len() < PAYLOAD_IN_TASK_LIMIT + 500);
    }
}
