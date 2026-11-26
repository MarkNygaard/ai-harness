//! **Notifications**: telling people about runs and triggers they asked to
//! hear about, and the `/api/me/notifications*` API behind the inbox.
//!
//! Two events feed it:
//!
//! - [`run_ended`], from every place a run reaches a final status: the
//!   executor finishing it, a run that could not be set up or launched, a
//!   person cancelling it, and the reaper cancelling a run whose executor went
//!   away.
//! - [`trigger_failed`], from every place a trigger or schedule fires but
//!   cannot start a run.
//!
//! Each is matched against the subscriptions in
//! [`harness_persist::NotificationStore`] and written to the inbox of everyone
//! it concerns. Nothing is sent to anybody who did not subscribe. Both run in
//! the background: telling people must never hold up, or fail, the thing that
//! happened.

use std::sync::Arc;

use axum::extract::{Extension, Path as AxumPath, Query};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::Json;
use harness_persist::{
    EventSubject, NewNotification, SubscriptionInput, EVENT_RUN_CANCELLED, EVENT_RUN_COMPLETED,
    EVENT_RUN_FAILED, EVENT_TRIGGER_FAILED,
};
use serde::Deserialize;
use serde_json::json;
use sha2::{Digest, Sha256};

use super::accounts::{authenticated_user, Mode};
use super::runs_routes::RunsState;

/// How long the inbox keeps a notification, read or not.
const RETENTION_DAYS: i64 = 90;
/// How often old notifications are cleared out.
const PRUNE_EVERY_SECS: u64 = 6 * 60 * 60;
/// The longest error text a notification carries.
const MAX_BODY: usize = 600;

/// Which kind of thing failed to start a run, for the notification's wording
/// and its link.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum TriggerKind {
    Webhook,
    Linear,
    Schedule,
    /// A classic Linear binding, which has no id of its own.
    Binding,
}

/// A trigger or schedule that fired but could not start a run.
#[derive(Debug, Clone)]
pub(crate) struct TriggerFailure {
    pub kind: TriggerKind,
    /// `None` for a classic binding.
    pub trigger_id: Option<String>,
    pub name: String,
    pub project: String,
    pub workflow: String,
    /// Who created the trigger, for the `mine` scope.
    pub owner: Option<String>,
    pub error: String,
}

impl TriggerFailure {
    fn link(&self) -> String {
        match (self.kind, self.trigger_id.as_deref()) {
            (TriggerKind::Schedule, Some(id)) => format!("/triggers/schedule/{id}"),
            (TriggerKind::Webhook | TriggerKind::Linear, Some(id)) => format!("/triggers/{id}"),
            _ => "/triggers".to_string(),
        }
    }

    /// One notification per trigger, per distinct error, per hour. A webhook
    /// whose sender keeps retrying with a bad signature is one problem, not a
    /// notification per delivery.
    fn dedupe_key(&self, now: chrono::DateTime<chrono::Utc>) -> String {
        let which = self
            .trigger_id
            .clone()
            .unwrap_or_else(|| format!("{}/{}", self.project, self.workflow));
        let error = format!("{:x}", Sha256::digest(self.error.as_bytes()));
        format!(
            "trigger:{which}:{}:{}",
            &error[..16],
            now.format("%Y-%m-%dT%H")
        )
    }

    fn noun(&self) -> &'static str {
        match self.kind {
            TriggerKind::Webhook => "Webhook",
            TriggerKind::Linear => "Linear trigger",
            TriggerKind::Schedule => "Schedule",
            TriggerKind::Binding => "Linear binding",
        }
    }
}

/// A Linear trigger, or a classic binding when `trigger_id` is `None`, that
/// could not start a run. Looks up the trigger's name and owner.
pub(crate) async fn linear_trigger_failed(
    state: &Arc<RunsState>,
    trigger_id: Option<&str>,
    project: &str,
    workflow: &str,
    error: &str,
) {
    let mut failure = TriggerFailure {
        kind: TriggerKind::Binding,
        trigger_id: None,
        name: format!("{project} · {workflow}"),
        project: project.to_string(),
        workflow: workflow.to_string(),
        owner: None,
        error: error.to_string(),
    };
    if let Some(id) = trigger_id {
        failure.kind = TriggerKind::Linear;
        failure.trigger_id = Some(id.to_string());
        if let Ok(store) = state.trigger_store().await {
            if let Ok(Some(t)) = store.get(id).await {
                failure.name = t.name;
                failure.owner = t.created_by;
            }
        }
    }
    trigger_failed(state, failure);
}

/// Tell subscribers that a trigger could not start a run. Returns at once.
pub(crate) fn trigger_failed(state: &Arc<RunsState>, failure: TriggerFailure) {
    let state = state.clone();
    tokio::spawn(async move {
        if let Err(e) = trigger_failed_inner(&state, &failure).await {
            tracing::warn!("notifications: trigger `{}` — {e}", failure.name);
        }
    });
}

async fn trigger_failed_inner(
    state: &Arc<RunsState>,
    failure: &TriggerFailure,
) -> Result<(), String> {
    let store = state.notification_store().await?;
    let subject = EventSubject {
        trigger_id: failure.trigger_id.as_deref(),
        workflow: Some(&failure.workflow),
        project: Some(&failure.project),
        owner: failure.owner.as_deref(),
    };
    let subscriptions = store
        .matching(EVENT_TRIGGER_FAILED, &subject)
        .await
        .map_err(|e| e.to_string())?;
    if subscriptions.is_empty() {
        return Ok(());
    }
    let title = format!("{} “{}” couldn't start a run", failure.noun(), failure.name);
    let body = truncate(&failure.error, MAX_BODY);
    let link = failure.link();
    let dedupe_key = failure.dedupe_key(chrono::Utc::now());
    for sub in &subscriptions {
        store
            .notify(&NewNotification {
                user_id: &sub.user_id,
                subscription_id: Some(&sub.id),
                event: EVENT_TRIGGER_FAILED,
                title: &title,
                body: &body,
                link: Some(&link),
                run_id: None,
                trigger_id: failure.trigger_id.as_deref(),
                project: Some(&failure.project),
                dedupe_key: &dedupe_key,
            })
            .await
            .map_err(|e| e.to_string())?;
    }
    Ok(())
}

/// Tell subscribers that a run reached a final status. Returns at once.
///
/// `actor` is the person who caused the ending, when one did (cancelling a
/// run): they know already, so they are not told.
pub(crate) fn run_ended(state: &Arc<RunsState>, run_id: &str, actor: Option<String>) {
    let state = state.clone();
    let run_id = run_id.to_string();
    tokio::spawn(async move {
        if let Err(e) = run_ended_inner(&state, &run_id, actor.as_deref()).await {
            tracing::warn!("notifications: run {run_id} — {e}");
        }
    });
}

/// The event a final run status raises, if any.
fn run_event(status: &str) -> Option<&'static str> {
    match status {
        "failed" => Some(EVENT_RUN_FAILED),
        "cancelled" => Some(EVENT_RUN_CANCELLED),
        "completed" => Some(EVENT_RUN_COMPLETED),
        _ => None,
    }
}

/// What a run notification says: its title and a line about how it ended.
fn run_message(detail: &harness_persist::RunDetail, event: &str) -> (String, String) {
    let run = &detail.run;
    let name = run
        .title
        .as_deref()
        .filter(|t| !t.trim().is_empty())
        .unwrap_or(&run.workflow_name);
    let verb = match event {
        EVENT_RUN_FAILED => "failed",
        EVENT_RUN_CANCELLED => "was cancelled",
        _ => "finished",
    };
    let title = format!("Run {verb}: {name}");
    let mut body = format!(
        "`{}` in `{}`",
        run.workflow_name,
        run.project.as_deref().unwrap_or("?")
    );
    let ended_on = match event {
        EVENT_RUN_FAILED => Some("failed"),
        EVENT_RUN_CANCELLED => Some("cancelled"),
        _ => None,
    };
    if let Some(status) = ended_on {
        if let Some(node) = detail.nodes.iter().find(|n| n.status == status) {
            body.push_str(&format!(", at step `{}`", node.node_id));
            if let Some(note) = node.note.as_deref().filter(|n| !n.trim().is_empty()) {
                body.push_str(&format!(": {}", truncate(note, MAX_BODY)));
            }
        }
    }
    (title, body)
}

async fn run_ended_inner(
    state: &Arc<RunsState>,
    run_id: &str,
    actor: Option<&str>,
) -> Result<(), String> {
    let runs = state.store().await?;
    let Some(detail) = runs.get_run(run_id).await.map_err(|e| e.to_string())? else {
        return Ok(());
    };
    let Some(event) = run_event(&detail.run.status) else {
        return Ok(());
    };
    let trigger_id = runs
        .run_trigger_id(run_id)
        .await
        .map_err(|e| e.to_string())?;
    let store = state.notification_store().await?;
    let subject = EventSubject {
        trigger_id: trigger_id.as_deref(),
        workflow: Some(&detail.run.workflow_name),
        project: detail.run.project.as_deref(),
        owner: detail.run.triggered_by.as_deref(),
    };
    let subscriptions = store
        .matching(event, &subject)
        .await
        .map_err(|e| e.to_string())?;
    if subscriptions.is_empty() {
        return Ok(());
    }
    let (title, body) = run_message(&detail, event);
    let link = format!("/runs/{run_id}");
    let dedupe_key = format!("run:{run_id}:{event}");
    for sub in subscriptions
        .iter()
        .filter(|s| Some(s.user_id.as_str()) != actor)
    {
        store
            .notify(&NewNotification {
                user_id: &sub.user_id,
                subscription_id: Some(&sub.id),
                event,
                title: &title,
                body: &body,
                link: Some(&link),
                run_id: Some(run_id),
                trigger_id: trigger_id.as_deref(),
                project: detail.run.project.as_deref(),
                dedupe_key: &dedupe_key,
            })
            .await
            .map_err(|e| e.to_string())?;
    }
    Ok(())
}

/// Clear out old notifications now and then.
pub(crate) fn spawn_pruner(state: Arc<RunsState>) {
    if tokio::runtime::Handle::try_current().is_err() {
        return;
    }
    tokio::spawn(async move {
        let mut tick = tokio::time::interval(std::time::Duration::from_secs(PRUNE_EVERY_SECS));
        tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        loop {
            tick.tick().await;
            let Ok(store) = state.notification_store().await else {
                continue;
            };
            match store.prune(RETENTION_DAYS).await {
                Ok(n) if n > 0 => tracing::info!("notifications: pruned {n} old notification(s)"),
                Ok(_) => {}
                Err(e) => tracing::warn!("notifications: prune failed: {e}"),
            }
        }
    });
}

fn truncate(text: &str, max: usize) -> String {
    let text = text.trim();
    if text.chars().count() <= max {
        return text.to_string();
    }
    let cut: String = text.chars().take(max).collect();
    format!("{}…", cut.trim_end())
}

// ── API ──────────────────────────────────────────────────────────────────────

fn err(status: StatusCode, msg: impl Into<String>) -> Response {
    (status, Json(json!({ "error": msg.into() }))).into_response()
}

/// The signed-in person. Notifications belong to someone, so a harness
/// without accounts has none.
// The `Err` here IS the response, handed straight back to axum.
#[allow(clippy::result_large_err)]
async fn me(
    state: &Arc<RunsState>,
    headers: &HeaderMap,
) -> Result<harness_persist::User, Response> {
    if super::accounts::mode(state).await != Mode::Accounts {
        return Err(err(
            StatusCode::CONFLICT,
            "notifications need accounts, and this harness has no sign-in yet: claim it at /setup",
        ));
    }
    authenticated_user(state, headers).await.ok_or_else(|| {
        err(
            StatusCode::UNAUTHORIZED,
            "sign in to see your notifications",
        )
    })
}

#[derive(Debug, Deserialize)]
pub struct InboxQuery {
    #[serde(default)]
    unread: bool,
    #[serde(default)]
    limit: Option<i64>,
}

/// `GET /api/me/notifications` — the inbox, newest first, and how many are
/// unread.
pub async fn inbox(
    Extension(state): Extension<Arc<RunsState>>,
    headers: HeaderMap,
    Query(q): Query<InboxQuery>,
) -> Response {
    let user = match me(&state, &headers).await {
        Ok(u) => u,
        Err(resp) => return resp,
    };
    let store = match state.notification_store().await {
        Ok(s) => s,
        Err(e) => return err(StatusCode::SERVICE_UNAVAILABLE, e),
    };
    let list = store.inbox(&user.id, q.unread, q.limit.unwrap_or(50)).await;
    let unread = store.unread_count(&user.id).await;
    match (list, unread) {
        (Ok(notifications), Ok(unread)) => {
            Json(json!({ "notifications": notifications, "unread": unread })).into_response()
        }
        (Err(e), _) | (_, Err(e)) => err(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()),
    }
}

#[derive(Debug, Deserialize)]
pub struct MarkRead {
    /// The notifications to mark read; all of them when absent.
    #[serde(default)]
    ids: Option<Vec<i64>>,
}

/// `POST /api/me/notifications/read` — mark some, or all, read.
pub async fn mark_read(
    Extension(state): Extension<Arc<RunsState>>,
    headers: HeaderMap,
    Json(body): Json<MarkRead>,
) -> Response {
    let user = match me(&state, &headers).await {
        Ok(u) => u,
        Err(resp) => return resp,
    };
    let store = match state.notification_store().await {
        Ok(s) => s,
        Err(e) => return err(StatusCode::SERVICE_UNAVAILABLE, e),
    };
    match store.mark_read(&user.id, body.ids.as_deref()).await {
        Ok(marked) => Json(json!({ "marked": marked })).into_response(),
        Err(e) => err(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()),
    }
}

/// `GET /api/me/notification-subscriptions` — what the caller asked to hear about.
pub async fn list_subscriptions(
    Extension(state): Extension<Arc<RunsState>>,
    headers: HeaderMap,
) -> Response {
    let user = match me(&state, &headers).await {
        Ok(u) => u,
        Err(resp) => return resp,
    };
    let store = match state.notification_store().await {
        Ok(s) => s,
        Err(e) => return err(StatusCode::SERVICE_UNAVAILABLE, e),
    };
    match store.subscriptions(&user.id).await {
        Ok(subscriptions) => Json(json!({ "subscriptions": subscriptions })).into_response(),
        Err(e) => err(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()),
    }
}

/// `POST /api/me/notification-subscriptions` — subscribe the caller.
pub async fn subscribe(
    Extension(state): Extension<Arc<RunsState>>,
    headers: HeaderMap,
    Json(input): Json<SubscriptionInput>,
) -> Response {
    let user = match me(&state, &headers).await {
        Ok(u) => u,
        Err(resp) => return resp,
    };
    let input = match input.validated() {
        Ok(i) => i,
        Err(e) => return err(StatusCode::BAD_REQUEST, e),
    };
    let store = match state.notification_store().await {
        Ok(s) => s,
        Err(e) => return err(StatusCode::SERVICE_UNAVAILABLE, e),
    };
    match store.subscribe(&user.id, &input).await {
        Ok(sub) => (StatusCode::CREATED, Json(sub)).into_response(),
        Err(e) => err(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()),
    }
}

/// `DELETE /api/me/notification-subscriptions/{id}` — unsubscribe.
pub async fn unsubscribe(
    Extension(state): Extension<Arc<RunsState>>,
    headers: HeaderMap,
    AxumPath(id): AxumPath<String>,
) -> Response {
    let user = match me(&state, &headers).await {
        Ok(u) => u,
        Err(resp) => return resp,
    };
    let store = match state.notification_store().await {
        Ok(s) => s,
        Err(e) => return err(StatusCode::SERVICE_UNAVAILABLE, e),
    };
    match store.unsubscribe(&id, &user.id).await {
        Ok(true) => StatusCode::NO_CONTENT.into_response(),
        Ok(false) => err(StatusCode::NOT_FOUND, "no such subscription"),
        Err(e) => err(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use harness_persist::{PersistedNode, RunDetail, RunSummary};

    fn failure(kind: TriggerKind, id: Option<&str>, error: &str) -> TriggerFailure {
        TriggerFailure {
            kind,
            trigger_id: id.map(str::to_string),
            name: "nightly".into(),
            project: "p".into(),
            workflow: "w".into(),
            owner: None,
            error: error.into(),
        }
    }

    #[test]
    fn trigger_failures_link_to_their_page() {
        assert_eq!(
            failure(TriggerKind::Webhook, Some("t1"), "x").link(),
            "/triggers/t1"
        );
        assert_eq!(
            failure(TriggerKind::Schedule, Some("s1"), "x").link(),
            "/triggers/schedule/s1"
        );
        assert_eq!(failure(TriggerKind::Binding, None, "x").link(), "/triggers");
    }

    #[test]
    fn a_repeated_failure_is_one_notification_an_hour() {
        let at = |h: u32| {
            chrono::NaiveDate::from_ymd_opt(2026, 11, 26)
                .unwrap()
                .and_hms_opt(h, 10, 0)
                .unwrap()
                .and_utc()
        };
        let bad_sig = failure(TriggerKind::Webhook, Some("t1"), "bad signature");
        assert_eq!(bad_sig.dedupe_key(at(3)), bad_sig.dedupe_key(at(3)));
        assert_ne!(
            bad_sig.dedupe_key(at(3)),
            bad_sig.dedupe_key(at(4)),
            "next hour"
        );
        let other = failure(TriggerKind::Webhook, Some("t1"), "no secret");
        assert_ne!(
            bad_sig.dedupe_key(at(3)),
            other.dedupe_key(at(3)),
            "other error"
        );
        // A binding is keyed by what it binds.
        assert!(failure(TriggerKind::Binding, None, "x")
            .dedupe_key(at(3))
            .starts_with("trigger:p/w:"));
    }

    #[test]
    fn only_final_statuses_raise_an_event() {
        assert_eq!(run_event("failed"), Some(EVENT_RUN_FAILED));
        assert_eq!(run_event("cancelled"), Some(EVENT_RUN_CANCELLED));
        assert_eq!(run_event("completed"), Some(EVENT_RUN_COMPLETED));
        assert_eq!(run_event("running"), None);
        assert_eq!(run_event("queued"), None);
    }

    fn detail(status: &str, nodes: &[(&str, &str, Option<&str>)]) -> RunDetail {
        let run: RunSummary = serde_json::from_value(json!({
            "id": "task-1", "workflow_name": "task", "title": "ECOM-1 Fix it",
            "description": null, "status": status, "project": "shop", "node_count": 2,
            "recorded_at": "2026-11-26T00:00:00Z", "started_at": null, "ended_at": null,
            "ab_pair_id": null, "ab_arm": null, "ab_label": null, "triggered_by": null,
            "trigger_source": null, "trigger_actor": null,
        }))
        .unwrap();
        let nodes = nodes
            .iter()
            .map(|(id, status, note)| {
                serde_json::from_value::<PersistedNode>(json!({
                    "node_id": id, "ordinal": 0, "status": status, "provider": null,
                    "model": null, "output": "", "iterations": 0, "converged": null,
                    "note": note, "input_tokens": null, "output_tokens": null,
                    "cache_read": null, "cache_write": null, "started_at": null,
                    "ended_at": null, "artifact_content": null, "error_count": 0,
                }))
                .unwrap()
            })
            .collect();
        RunDetail {
            run,
            nodes,
            graph: vec![],
        }
    }

    #[test]
    fn a_failed_run_names_its_step() {
        let d = detail(
            "failed",
            &[
                ("plan", "success", None),
                ("test", "failed", Some("3 tests failed")),
            ],
        );
        let (title, body) = run_message(&d, EVENT_RUN_FAILED);
        assert_eq!(title, "Run failed: ECOM-1 Fix it");
        assert_eq!(body, "`task` in `shop`, at step `test`: 3 tests failed");

        let (title, body) = run_message(&detail("completed", &[]), EVENT_RUN_COMPLETED);
        assert_eq!(title, "Run finished: ECOM-1 Fix it");
        assert_eq!(body, "`task` in `shop`");
    }

    #[test]
    fn truncation_keeps_whole_characters() {
        assert_eq!(truncate("  short  ", 10), "short");
        assert_eq!(truncate("æøå æøå", 4), "æøå…");
    }
}
