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
//! it concerns, once per person however many of their subscriptions match.
//! It then goes wherever those subscriptions also asked: an email, a Slack
//! direct message, a Slack channel (posted once, however many people route
//! the same thing there). A failure to reach one of those is kept on the
//! inbox entry. Nothing is sent to anybody who did not subscribe. All of it
//! runs in the background: telling people must never hold up, or fail, the
//! thing that happened.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

use axum::extract::{Extension, Path as AxumPath, Query};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::Json;
use harness_persist::{
    EventSubject, NewNotification, NotificationStore, NotificationSubscription, SubscriptionInput,
    CHANNEL_EMAIL, CHANNEL_SLACK_CHANNEL, CHANNEL_SLACK_DM, EVENT_RUN_CANCELLED,
    EVENT_RUN_COMPLETED, EVENT_RUN_FAILED, EVENT_TRIGGER_FAILED,
};
use serde::Deserialize;
use serde_json::json;
use sha2::{Digest, Sha256};

use super::accounts::{authenticated_user, Mode};
use super::runs_routes::RunsState;
use super::slack::{SlackClient, SlackMessage};

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
    let outgoing = Outgoing {
        event: EVENT_TRIGGER_FAILED,
        title: &title,
        body: &body,
        link: &link,
        run_id: None,
        trigger_id: failure.trigger_id.as_deref(),
        project: Some(&failure.project),
        dedupe_key: &dedupe_key,
    };
    send(state, store, &subscriptions, &outgoing, None).await
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
    let outgoing = Outgoing {
        event,
        title: &title,
        body: &body,
        link: &link,
        run_id: Some(run_id),
        trigger_id: trigger_id.as_deref(),
        project: detail.run.project.as_deref(),
        dedupe_key: &dedupe_key,
    };
    send(state, store, &subscriptions, &outgoing, actor).await
}

/// One thing to tell people.
struct Outgoing<'a> {
    event: &'a str,
    title: &'a str,
    body: &'a str,
    /// A path in the web UI, e.g. `/runs/<id>`.
    link: &'a str,
    run_id: Option<&'a str>,
    trigger_id: Option<&'a str>,
    project: Option<&'a str>,
    dedupe_key: &'a str,
}

/// Where one person's notification goes, merged across every subscription of
/// theirs that matched.
#[derive(Debug, Default, PartialEq)]
struct Route {
    /// The first matching subscription, recorded on the inbox entry.
    subscription_id: String,
    email: bool,
    slack_dm: bool,
    slack_channels: BTreeSet<String>,
}

/// Group matching subscriptions by person, leaving out `skip`.
fn routes(
    subscriptions: &[NotificationSubscription],
    skip: Option<&str>,
) -> BTreeMap<String, Route> {
    let mut out: BTreeMap<String, Route> = BTreeMap::new();
    for sub in subscriptions
        .iter()
        .filter(|s| Some(s.user_id.as_str()) != skip)
    {
        let route = out.entry(sub.user_id.clone()).or_insert_with(|| Route {
            subscription_id: sub.id.clone(),
            ..Route::default()
        });
        for channel in sub.channels.iter() {
            match channel.as_str() {
                CHANNEL_EMAIL => route.email = true,
                CHANNEL_SLACK_DM => route.slack_dm = true,
                CHANNEL_SLACK_CHANNEL => {
                    if let Some(c) = &sub.slack_channel {
                        route.slack_channels.insert(c.clone());
                    }
                }
                _ => {}
            }
        }
    }
    out
}

/// Write `outgoing` to each subscriber's inbox, then send it on to email and
/// Slack where they asked. A person who already has it is not sent it again.
async fn send(
    state: &Arc<RunsState>,
    store: &NotificationStore,
    subscriptions: &[NotificationSubscription],
    outgoing: &Outgoing<'_>,
    skip: Option<&str>,
) -> Result<(), String> {
    for (user_id, route) in routes(subscriptions, skip) {
        let written = store
            .notify(&NewNotification {
                user_id: &user_id,
                subscription_id: Some(&route.subscription_id),
                event: outgoing.event,
                title: outgoing.title,
                body: outgoing.body,
                link: Some(outgoing.link),
                run_id: outgoing.run_id,
                trigger_id: outgoing.trigger_id,
                project: outgoing.project,
                dedupe_key: outgoing.dedupe_key,
            })
            .await
            .map_err(|e| e.to_string())?;
        let Some(notification) = written else {
            continue;
        };
        if !route.email && !route.slack_dm && route.slack_channels.is_empty() {
            continue;
        }
        let errors = deliver(state, store, &user_id, &route, outgoing).await;
        if !errors.is_empty() {
            let joined = errors.join("; ");
            tracing::warn!(
                "notifications: {} for {user_id}: {joined}",
                outgoing.dedupe_key
            );
            let _ = store
                .set_delivery_error(notification.id, Some(&truncate(&joined, MAX_BODY)))
                .await;
        }
    }
    Ok(())
}

/// The harness's own address joined to `path`, when it has been set.
fn absolute(state: &Arc<RunsState>, path: &str) -> Option<String> {
    state.public_url().map(|base| format!("{base}{path}"))
}

/// Send one notification to email and Slack. Returns what failed, as a person
/// reads it; an empty list when everything arrived.
async fn deliver(
    state: &Arc<RunsState>,
    store: &NotificationStore,
    user_id: &str,
    route: &Route,
    outgoing: &Outgoing<'_>,
) -> Vec<String> {
    let mut errors = Vec::new();
    let email = match state.user_store().await {
        Ok(users) => users.get(user_id).await.ok().flatten().map(|u| u.email),
        Err(_) => None,
    };
    let url = absolute(state, outgoing.link);
    let settings_url = absolute(state, SETTINGS_PATH);

    if route.email {
        match &email {
            Some(to) => {
                let text = email_body(outgoing.body, url.as_deref(), settings_url.as_deref());
                if let Err(e) = super::mail::send(state, to, outgoing.title, &text).await {
                    errors.push(format!("Email: {e}"));
                }
            }
            None => errors.push("Email: no address on the account".into()),
        }
    }

    if route.slack_dm || !route.slack_channels.is_empty() {
        let Some(token) = super::slack::token(state).await else {
            errors.push("Slack: not connected (Settings → Integrations)".into());
            return errors;
        };
        let client = SlackClient::new(token);
        let message = SlackMessage {
            title: outgoing.title,
            body: outgoing.body,
            url: url.as_deref(),
            settings_url: settings_url.as_deref(),
        };
        if route.slack_dm {
            let result = match &email {
                Some(address) => match super::slack::user_id(&client, address).await {
                    Ok(slack_user) => client.post(&slack_user, &message).await,
                    Err(e) => Err(e),
                },
                None => Err("no address on the account".into()),
            };
            if let Err(e) = result {
                errors.push(format!("Slack DM: {e}"));
            }
        }
        for channel in &route.slack_channels {
            // Several people can route the same thing to one channel.
            match store.claim_channel_post(channel, outgoing.dedupe_key).await {
                Ok(true) => {
                    if let Err(e) = client.post(channel, &message).await {
                        errors.push(format!("Slack #{channel}: {e}"));
                    }
                }
                Ok(false) => {}
                Err(e) => errors.push(format!("Slack #{channel}: {e}")),
            }
        }
    }
    errors
}

/// Where a person changes what they hear about.
const SETTINGS_PATH: &str = "/settings/notifications";

/// A notification as plain-text mail.
fn email_body(body: &str, url: Option<&str>, settings_url: Option<&str>) -> String {
    let mut text = body.trim().to_string();
    if let Some(url) = url {
        text.push_str(&format!("\n\n{url}"));
    }
    text.push_str("\n\n—\nYou get this because you follow it in the harness.");
    if let Some(settings) = settings_url {
        text.push_str(&format!(" Change what you hear about: {settings}"));
    }
    text.push('\n');
    text
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
    // Refuse a channel that cannot deliver now, rather than accept it and fail
    // every time something happens.
    let available = channels(&state).await;
    if input.channels.iter().any(|c| c == CHANNEL_EMAIL) && !available.email {
        return err(
            StatusCode::BAD_REQUEST,
            "email isn't set up on this harness (Settings → Email)",
        );
    }
    let wants_slack = input
        .channels
        .iter()
        .any(|c| c == CHANNEL_SLACK_DM || c == CHANNEL_SLACK_CHANNEL);
    if wants_slack && !available.slack {
        return err(
            StatusCode::BAD_REQUEST,
            "Slack isn't connected to this harness (Settings → Integrations)",
        );
    }
    let store = match state.notification_store().await {
        Ok(s) => s,
        Err(e) => return err(StatusCode::SERVICE_UNAVAILABLE, e),
    };
    match store.subscribe(&user.id, &input).await {
        Ok(sub) => (StatusCode::CREATED, Json(sub)).into_response(),
        Err(e) => err(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()),
    }
}

/// Which channels besides the inbox can deliver on this harness.
#[derive(Debug, Clone, Copy, serde::Serialize)]
struct Available {
    email: bool,
    slack: bool,
}

async fn channels(state: &Arc<RunsState>) -> Available {
    let email = match state.cred_store().await {
        Ok(store) => super::mail::config(store).await.is_some(),
        Err(_) => false,
    };
    Available {
        email,
        slack: super::slack::token(state).await.is_some(),
    }
}

/// `GET /api/me/notification-channels` — where notifications can go here, and
/// the address email goes to.
pub async fn list_channels(
    Extension(state): Extension<Arc<RunsState>>,
    headers: HeaderMap,
) -> Response {
    let user = match me(&state, &headers).await {
        Ok(u) => u,
        Err(resp) => return resp,
    };
    let available = channels(&state).await;
    Json(json!({
        "email": { "available": available.email, "address": user.email },
        "slack": { "available": available.slack },
    }))
    .into_response()
}

#[derive(Debug, Deserialize)]
pub struct TestRequest {
    /// `email`, `slack_dm` or `slack_channel`.
    channel: String,
    #[serde(default)]
    slack_channel: Option<String>,
}

/// `POST /api/me/notifications/test` — send the caller a test notification on
/// one channel, and say what happened. Nothing is written to the inbox.
pub async fn send_test(
    Extension(state): Extension<Arc<RunsState>>,
    headers: HeaderMap,
    Json(req): Json<TestRequest>,
) -> Response {
    let user = match me(&state, &headers).await {
        Ok(u) => u,
        Err(resp) => return resp,
    };
    let store = match state.notification_store().await {
        Ok(s) => s,
        Err(e) => return err(StatusCode::SERVICE_UNAVAILABLE, e),
    };
    let mut route = Route::default();
    match req.channel.as_str() {
        CHANNEL_EMAIL => route.email = true,
        CHANNEL_SLACK_DM => route.slack_dm = true,
        CHANNEL_SLACK_CHANNEL => {
            let Some(channel) = req
                .slack_channel
                .as_deref()
                .map(|c| c.trim().trim_start_matches('#').to_string())
                .filter(|c| !c.is_empty())
            else {
                return err(StatusCode::BAD_REQUEST, "name the Slack channel to test");
            };
            route.slack_channels.insert(channel);
        }
        other => {
            return err(
                StatusCode::BAD_REQUEST,
                format!("unknown channel `{other}`"),
            )
        }
    }
    // A fresh key, so a test to a channel is never taken for a repeat.
    let dedupe_key = format!("test:{}", uuid::Uuid::new_v4());
    let outgoing = Outgoing {
        event: "test",
        title: "Test notification from the harness",
        body: "If you can read this, notifications reach you here.",
        link: SETTINGS_PATH,
        run_id: None,
        trigger_id: None,
        project: None,
        dedupe_key: &dedupe_key,
    };
    let errors = deliver(&state, store, &user.id, &route, &outgoing).await;
    if errors.is_empty() {
        Json(json!({ "sent": true })).into_response()
    } else {
        err(StatusCode::BAD_GATEWAY, errors.join("; "))
    }
}

/// `GET /api/integrations/slack/manifest` — the Slack app to create, as JSON.
pub async fn slack_manifest() -> Response {
    (
        [(axum::http::header::CONTENT_TYPE, "application/json")],
        super::slack::MANIFEST,
    )
        .into_response()
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

    fn sub(
        id: &str,
        user: &str,
        channels: &[&str],
        slack: Option<&str>,
    ) -> NotificationSubscription {
        serde_json::from_value(json!({
            "id": id, "user_id": user, "events": ["run_failed"], "scope_kind": "mine",
            "scope_value": "", "channels": channels, "slack_channel": slack,
            "created_at": "2026-11-26T00:00:00Z",
        }))
        .unwrap()
    }

    #[test]
    fn one_route_per_person_with_every_channel_they_asked_for() {
        let subs = [
            sub("s1", "ada", &["in_app", "email"], None),
            sub("s2", "ada", &["in_app", "slack_channel"], Some("builds")),
            sub("s3", "bob", &["in_app"], None),
            sub("s4", "cy", &["in_app", "slack_dm"], None),
        ];
        let all = routes(&subs, None);
        assert_eq!(all.len(), 3);
        let ada = &all["ada"];
        assert_eq!(ada.subscription_id, "s1", "the first match is recorded");
        assert!(ada.email && !ada.slack_dm);
        assert_eq!(ada.slack_channels.iter().collect::<Vec<_>>(), ["builds"]);
        assert_eq!(
            all["bob"],
            Route {
                subscription_id: "s3".into(),
                ..Route::default()
            }
        );
        assert!(all["cy"].slack_dm);

        // Whoever caused it is left out.
        assert!(!routes(&subs, Some("ada")).contains_key("ada"));
    }

    #[test]
    fn email_links_the_thing_and_the_settings() {
        let text = email_body(
            "`task` in `shop`",
            Some("https://h.example/runs/1"),
            Some("https://h.example/settings/notifications"),
        );
        assert!(text.starts_with("`task` in `shop`\n\nhttps://h.example/runs/1\n\n—\n"));
        assert!(
            text.contains("Change what you hear about: https://h.example/settings/notifications")
        );
        // Without a public address there is nothing to link.
        assert!(!email_body("x", None, None).contains("http"));
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
