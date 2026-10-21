//! **The built-in chat** — ask the harness about runs, PRs and workflows, or have
//! it author one, from a panel in the web UI.
//!
//! - `GET/PUT  /api/chat/settings`                    — which agent backs it (PUT: administrators)
//! - `GET/POST /api/chat/conversations`               — yours / start one
//! - `GET/DELETE /api/chat/conversations/{id}`        — one, with its messages
//! - `POST     /api/chat/conversations/{id}/messages` — send a message (answers asynchronously)
//! - `POST     /api/chat/actions/{id}`                — confirm or cancel a held tool call
//!
//! **What the agent can do is exactly what `/mcp` offers**, nothing more: each
//! turn spawns the chosen CLI with the harness attached as its only MCP server
//! and every built-in tool denied at the CLI boundary (see
//! [`harness_runner::chat`]). It reaches `/mcp` over loopback with a token
//! minted for that one turn, which names the conversation and the person — so a
//! workflow it edits is recorded as theirs, "via chat".
//!
//! **Side effects wait for the person.** A tool in
//! [`super::mcp_routes::NEEDS_CONFIRMATION`] called with a chat token is not run:
//! it becomes an `action` message the panel shows as a card, and runs only when
//! the person presses Confirm. That is enforced here, server-side, so it holds
//! whichever model is behind the chat.

use std::collections::HashMap;
use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::{Arc, LazyLock, Mutex, OnceLock};

use axum::extract::{Extension, Path as AxumPath};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::Json;
use harness_dag::{Activity, ProgressSink};
use harness_persist::ChatMessage;
use harness_runner::authoring::ProviderInfo;
use harness_runner::chat::{self, ChatMcp, ChatTurn};
use serde::Deserialize;
use serde_json::{json, Value};

use super::accounts::{caller_trigger, AdminOnly};
use super::runs_routes::{RunsState, TriggerInfo, SOURCE_CHAT};

const SETTING_PROVIDER: &str = "chat_provider";
const SETTING_MODEL: &str = "chat_model";
const SETTING_EFFORT: &str = "chat_effort";

/// Marks a chat token in logs, and is how `/mcp` tells one from an MCP key.
const TOKEN_PREFIX: &str = "hrn_chat_";

/// How many earlier messages a fresh session is given when the stored one
/// cannot be resumed.
const TRANSCRIPT_MESSAGES: usize = 20;

/// Conversations listed in the panel.
const LIST_LIMIT: i64 = 50;

fn err(status: StatusCode, msg: impl Into<String>) -> Response {
    (status, Json(json!({ "error": msg.into() }))).into_response()
}

// ── Where the agent reaches the harness ────────────────────────────────────

static LISTEN_ADDR: OnceLock<SocketAddr> = OnceLock::new();

/// Record the address the server is listening on, so a chat agent can be
/// pointed at `/mcp` over loopback.
pub(crate) fn set_listen_addr(addr: SocketAddr) {
    let _ = LISTEN_ADDR.set(addr);
}

/// `/mcp` on this server, over loopback. A wildcard bind is reachable on
/// 127.0.0.1; a specific address is used as given.
fn mcp_url() -> String {
    let addr = LISTEN_ADDR
        .get()
        .copied()
        .unwrap_or_else(|| SocketAddr::from(([127, 0, 0, 1], 9800)));
    let host = if addr.ip().is_unspecified() {
        "127.0.0.1".to_string()
    } else if addr.is_ipv6() {
        format!("[{}]", addr.ip())
    } else {
        addr.ip().to_string()
    };
    format!("http://{host}:{}/mcp", addr.port())
}

// ── Turn tokens ────────────────────────────────────────────────────────────

/// Who a chat agent's MCP call is on behalf of.
#[derive(Debug, Clone)]
pub(crate) struct ChatCaller {
    pub conversation_id: String,
    pub user_id: Option<String>,
    pub actor: Option<String>,
}

impl ChatCaller {
    pub(crate) fn trigger(&self) -> TriggerInfo {
        TriggerInfo {
            source: Some(SOURCE_CHAT.to_string()),
            user_id: self.user_id.clone(),
            actor: self.actor.clone(),
        }
    }
}

/// Live turn tokens. In memory on purpose: a token lives exactly as long as the
/// CLI process it was minted for, which never outlives this server.
static TOKENS: LazyLock<Mutex<HashMap<String, ChatCaller>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

/// A minted token; revoked when dropped, so every exit path from a turn —
/// answer, error, panic — takes it back.
struct TurnToken(String);

impl TurnToken {
    fn mint(caller: ChatCaller) -> Self {
        let token = format!(
            "{TOKEN_PREFIX}{}{}",
            uuid::Uuid::new_v4().simple(),
            uuid::Uuid::new_v4().simple()
        );
        TOKENS
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .insert(token.clone(), caller);
        Self(token)
    }
}

impl Drop for TurnToken {
    fn drop(&mut self) {
        TOKENS
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .remove(&self.0);
    }
}

/// The chat behind a request to `/mcp`, if it carries a live turn token.
pub(crate) fn caller_for(headers: &HeaderMap) -> Option<ChatCaller> {
    let token = headers
        .get(axum::http::header::AUTHORIZATION)?
        .to_str()
        .ok()?
        .strip_prefix("Bearer ")?
        .trim();
    if !token.starts_with(TOKEN_PREFIX) {
        return None;
    }
    TOKENS
        .lock()
        .unwrap_or_else(|p| p.into_inner())
        .get(token)
        .cloned()
}

// ── Turns in progress ──────────────────────────────────────────────────────

/// Conversation id → what the agent has done so far this turn. Its presence is
/// also the "a turn is running" flag that refuses a second message meanwhile.
static RUNNING: LazyLock<Mutex<HashMap<String, Vec<Activity>>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

fn running_activity(id: &str) -> Option<Vec<Activity>> {
    RUNNING
        .lock()
        .unwrap_or_else(|p| p.into_inner())
        .get(id)
        .cloned()
}

// ── Settings ───────────────────────────────────────────────────────────────

/// The agents that can back the chat right now: connected, and able to be
/// held to the harness's tools (see [`chat::CHAT_PROVIDERS`]).
async fn chat_options() -> Vec<ProviderInfo> {
    let creds = super::credentials_routes::connected_clis().await;
    harness_runner::authoring::providers(creds)
        .into_iter()
        .filter(|p| chat::supports_chat(p.id))
        .collect()
}

/// What new conversations run on.
#[derive(Debug, Clone, PartialEq)]
struct Effective {
    provider: Option<String>,
    model: Option<String>,
    effort: Option<String>,
}

/// The stored choice when it can run, else the first agent that can — Claude
/// Code before Pi, by the order `providers` lists them. A stored model belongs
/// to the stored provider, so it is dropped when that provider is not the one
/// in force.
fn effective(
    options: &[ProviderInfo],
    stored_provider: Option<&str>,
    stored_model: Option<&str>,
    stored_effort: Option<&str>,
) -> Effective {
    let chosen = stored_provider
        .and_then(|p| options.iter().find(|o| o.id == p))
        .or_else(|| options.first());
    let Some(chosen) = chosen else {
        return Effective {
            provider: None,
            model: None,
            effort: None,
        };
    };
    let model = stored_model
        .filter(|m| !m.is_empty() && stored_provider == Some(chosen.id))
        .map(str::to_string)
        .or_else(|| chosen.models.first().map(|m| m.to_string()));
    Effective {
        provider: Some(chosen.id.to_string()),
        model,
        effort: stored_effort.filter(|e| !e.is_empty()).map(str::to_string),
    }
}

async fn load_settings(state: &Arc<RunsState>) -> (Vec<ProviderInfo>, Effective, Value) {
    let options = chat_options().await;
    let (p, m, e) = match state.settings_store().await {
        Ok(s) => (
            s.get(SETTING_PROVIDER).await.ok().flatten(),
            s.get(SETTING_MODEL).await.ok().flatten(),
            s.get(SETTING_EFFORT).await.ok().flatten(),
        ),
        Err(_) => (None, None, None),
    };
    let eff = effective(&options, p.as_deref(), m.as_deref(), e.as_deref());
    let stored = json!({ "provider": p, "model": m, "effort": e });
    (options, eff, stored)
}

/// `GET /api/chat/settings` — what the chat runs on, and what it could.
pub async fn get_settings(Extension(state): Extension<Arc<RunsState>>) -> Response {
    let (options, eff, stored) = load_settings(&state).await;
    Json(json!({
        "provider": eff.provider,
        "model": eff.model,
        "effort": eff.effort,
        "stored": stored,
        "options": options,
    }))
    .into_response()
}

#[derive(Debug, Deserialize)]
pub struct SettingsRequest {
    #[serde(default)]
    pub provider: Option<String>,
    #[serde(default)]
    pub model: Option<String>,
    #[serde(default)]
    pub effort: Option<String>,
}

/// `PUT /api/chat/settings` — choose the agent. An empty or absent field clears
/// that choice back to the default.
pub async fn put_settings(
    _: AdminOnly,
    Extension(state): Extension<Arc<RunsState>>,
    Json(req): Json<SettingsRequest>,
) -> Response {
    let clean = |v: Option<String>| v.map(|s| s.trim().to_string()).filter(|s| !s.is_empty());
    let (provider, model, effort) = (clean(req.provider), clean(req.model), clean(req.effort));
    if let Some(p) = provider.as_deref() {
        if !chat::supports_chat(p) {
            return err(
                StatusCode::BAD_REQUEST,
                format!(
                    "`{p}` cannot back the chat — it needs an agent that can be limited to the \
                     harness's tools ({})",
                    chat::CHAT_PROVIDERS.join(", ")
                ),
            );
        }
    }
    if let Some(e) = effort.as_deref() {
        if !["low", "medium", "high", "xhigh", "max"].contains(&e) {
            return err(StatusCode::BAD_REQUEST, format!("unknown effort `{e}`"));
        }
    }
    let store = match state.settings_store().await {
        Ok(s) => s,
        Err(e) => return err(StatusCode::SERVICE_UNAVAILABLE, e),
    };
    for (key, value) in [
        (SETTING_PROVIDER, &provider),
        (SETTING_MODEL, &model),
        (SETTING_EFFORT, &effort),
    ] {
        let res = match value {
            Some(v) => store.set(key, v).await,
            None => store.delete(key).await.map(|_| ()),
        };
        if let Err(e) = res {
            return err(StatusCode::INTERNAL_SERVER_ERROR, e.to_string());
        }
    }
    get_settings(Extension(state)).await
}

// ── Conversations ──────────────────────────────────────────────────────────

/// The conversation, if it exists and belongs to the caller. Someone else's is
/// reported as not found rather than forbidden: its existence is not theirs to
/// learn either.
// The `Err` is the finished HTTP response, handed straight back to axum —
// boxing it would add an allocation to avoid a move axum makes anyway (as in
// `linear_routes::connection`).
#[allow(clippy::result_large_err)]
async fn owned(
    state: &Arc<RunsState>,
    headers: &HeaderMap,
    id: &str,
) -> Result<
    (
        harness_persist::Conversation,
        Option<String>,
        Option<String>,
    ),
    Response,
> {
    let (user, actor) = caller_trigger(state, headers).await;
    let store = state
        .chat_store()
        .await
        .map_err(|e| err(StatusCode::SERVICE_UNAVAILABLE, e))?;
    match store.get(id).await {
        Ok(Some(c)) if c.user_id == user => Ok((c, user, actor)),
        Ok(_) => Err(err(StatusCode::NOT_FOUND, "no such conversation")),
        Err(e) => Err(err(StatusCode::INTERNAL_SERVER_ERROR, e.to_string())),
    }
}

/// `GET /api/chat/conversations` — yours, most recent first.
pub async fn list_conversations(
    Extension(state): Extension<Arc<RunsState>>,
    headers: HeaderMap,
) -> Response {
    let (user, _) = caller_trigger(&state, &headers).await;
    let store = match state.chat_store().await {
        Ok(s) => s,
        Err(e) => return err(StatusCode::SERVICE_UNAVAILABLE, e),
    };
    match store.list(user.as_deref(), LIST_LIMIT).await {
        Ok(rows) => Json(json!({ "conversations": rows })).into_response(),
        Err(e) => err(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()),
    }
}

#[derive(Debug, Default, Deserialize)]
pub struct CreateConversationRequest {
    /// Where it was opened from: `{ run_id?, workflow?, project? }`.
    #[serde(default)]
    pub context: Option<Value>,
}

/// Keep only the context keys the prompt knows how to describe, as short strings.
fn clean_context(ctx: Option<&Value>) -> Value {
    let mut out = serde_json::Map::new();
    if let Some(obj) = ctx.and_then(Value::as_object) {
        for key in ["run_id", "workflow", "project"] {
            if let Some(v) = obj.get(key).and_then(Value::as_str) {
                let v = v.trim();
                if !v.is_empty() && v.len() <= 200 {
                    out.insert(key.to_string(), Value::String(v.to_string()));
                }
            }
        }
    }
    Value::Object(out)
}

/// `POST /api/chat/conversations` — start one on the agent the setting names.
pub async fn create_conversation(
    Extension(state): Extension<Arc<RunsState>>,
    headers: HeaderMap,
    body: Option<Json<CreateConversationRequest>>,
) -> Response {
    let req = body.map(|Json(b)| b).unwrap_or_default();
    let (user, _) = caller_trigger(&state, &headers).await;
    let (_, eff, _) = load_settings(&state).await;
    let Some(provider) = eff.provider else {
        return err(
            StatusCode::CONFLICT,
            "no agent can run the chat yet — connect Claude Code or Pi under Settings → Agents",
        );
    };
    let store = match state.chat_store().await {
        Ok(s) => s,
        Err(e) => return err(StatusCode::SERVICE_UNAVAILABLE, e),
    };
    let id = uuid::Uuid::new_v4().to_string();
    match store
        .create(
            &id,
            user.as_deref(),
            &provider,
            eff.model.as_deref(),
            &clean_context(req.context.as_ref()),
        )
        .await
    {
        Ok(conv) => (StatusCode::CREATED, Json(conv)).into_response(),
        Err(e) => err(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()),
    }
}

/// `GET /api/chat/conversations/{id}` — the conversation, its messages, and
/// what the agent is doing right now if a turn is running.
pub async fn get_conversation(
    Extension(state): Extension<Arc<RunsState>>,
    headers: HeaderMap,
    AxumPath(id): AxumPath<String>,
) -> Response {
    let (conv, _, _) = match owned(&state, &headers, &id).await {
        Ok(c) => c,
        Err(resp) => return resp,
    };
    let store = match state.chat_store().await {
        Ok(s) => s,
        Err(e) => return err(StatusCode::SERVICE_UNAVAILABLE, e),
    };
    match store.messages(&id).await {
        Ok(messages) => Json(json!({
            "conversation": conv,
            "messages": messages,
            "running": running_activity(&id).map(|activity| json!({ "activity": activity })),
        }))
        .into_response(),
        Err(e) => err(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()),
    }
}

/// `DELETE /api/chat/conversations/{id}`.
pub async fn delete_conversation(
    Extension(state): Extension<Arc<RunsState>>,
    headers: HeaderMap,
    AxumPath(id): AxumPath<String>,
) -> Response {
    if let Err(resp) = owned(&state, &headers, &id).await {
        return resp;
    }
    if running_activity(&id).is_some() {
        return err(StatusCode::CONFLICT, "wait for the answer before deleting");
    }
    let store = match state.chat_store().await {
        Ok(s) => s,
        Err(e) => return err(StatusCode::SERVICE_UNAVAILABLE, e),
    };
    match store.delete(&id).await {
        Ok(_) => {
            let _ = std::fs::remove_dir_all(chat_dir(&id));
            StatusCode::NO_CONTENT.into_response()
        }
        Err(e) => err(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()),
    }
}

#[derive(Debug, Deserialize)]
pub struct MessageRequest {
    pub text: String,
    /// The page the person is on now, which may differ from where the
    /// conversation started.
    #[serde(default)]
    pub context: Option<Value>,
}

/// `POST /api/chat/conversations/{id}/messages` — send a message. Returns 202
/// at once; the answer is a later `GET` of the conversation.
pub async fn post_message(
    Extension(state): Extension<Arc<RunsState>>,
    headers: HeaderMap,
    AxumPath(id): AxumPath<String>,
    Json(req): Json<MessageRequest>,
) -> Response {
    let text = req.text.trim().to_string();
    if text.is_empty() {
        return err(StatusCode::BAD_REQUEST, "say something");
    }
    let (conv, user, actor) = match owned(&state, &headers, &id).await {
        Ok(c) => c,
        Err(resp) => return resp,
    };
    // Claim the conversation before anything slow, so a double-send is refused
    // rather than starting two agents on one session.
    {
        let mut running = RUNNING.lock().unwrap_or_else(|p| p.into_inner());
        if running.contains_key(&id) {
            return err(StatusCode::CONFLICT, "still answering the last message");
        }
        running.insert(id.clone(), Vec::new());
    }
    let store = match state.chat_store().await {
        Ok(s) => s,
        Err(e) => {
            release(&id);
            return err(StatusCode::SERVICE_UNAVAILABLE, e);
        }
    };
    if let Err(e) = store
        .add_message(&id, "user", &text, &json!([]), None)
        .await
    {
        release(&id);
        return err(StatusCode::INTERNAL_SERVER_ERROR, e.to_string());
    }
    let _ = store.set_title_if_empty(&id, &title_for(&text)).await;
    let caller = ChatCaller {
        conversation_id: id.clone(),
        user_id: user,
        actor,
    };
    let context = req
        .context
        .as_ref()
        .map(|c| clean_context(Some(c)))
        .unwrap_or_else(|| conv.context.0.clone());
    tokio::spawn(run_turn_task(state.clone(), conv, caller, text, context));
    (StatusCode::ACCEPTED, Json(json!({ "accepted": true }))).into_response()
}

fn release(id: &str) {
    RUNNING.lock().unwrap_or_else(|p| p.into_inner()).remove(id);
}

/// The first line of the first message, shortened.
fn title_for(text: &str) -> String {
    let line = text.lines().next().unwrap_or("").trim();
    if line.chars().count() <= 80 {
        line.to_string()
    } else {
        format!("{}…", line.chars().take(79).collect::<String>())
    }
}

/// The conversation's working directory. Stable across turns because Claude
/// stores sessions per directory — resuming from anywhere else finds nothing.
/// Under `$HOME`, which is the persistent volume the CLIs keep their own state
/// on, so a session survives a restart along with the directory naming it.
fn chat_dir(id: &str) -> PathBuf {
    std::env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("/home/harness"))
        .join(".harness")
        .join("chat")
        .join(id)
}

/// One turn, start to finish, in the background.
async fn run_turn_task(
    state: Arc<RunsState>,
    conv: harness_persist::Conversation,
    caller: ChatCaller,
    text: String,
    context: Value,
) {
    let id = conv.id.clone();
    // Released however this ends — a panic included — or the conversation
    // would refuse every later message as "still answering".
    struct Release(String);
    impl Drop for Release {
        fn drop(&mut self) {
            release(&self.0);
        }
    }
    let _release = Release(id.clone());
    let outcome = answer(&state, &conv, &caller, &text, &context).await;
    let activity = running_activity(&id).unwrap_or_default();
    if let Ok(store) = state.chat_store().await {
        let saved = match outcome {
            Ok(reply) => {
                if let Some(session) = reply.session.as_deref() {
                    let _ = store.set_session(&id, session).await;
                }
                store
                    .add_message(&id, "assistant", reply.text.trim(), &json!(activity), None)
                    .await
            }
            Err(e) => {
                tracing::warn!("chat {id}: {e}");
                store
                    .add_message(&id, "error", &e, &json!(activity), None)
                    .await
            }
        };
        if let Err(e) = saved {
            tracing::warn!("chat {id}: could not save the answer: {e}");
        }
    }
}

async fn answer(
    state: &Arc<RunsState>,
    conv: &harness_persist::Conversation,
    caller: &ChatCaller,
    text: &str,
    context: &Value,
) -> Result<harness_runner::chat::ChatReply, String> {
    // Fresh agent credentials, as a run gets them.
    match state.cred_store().await {
        Ok(store) => super::credentials_routes::materialize(store).await,
        Err(e) => tracing::info!("chat: credentials not materialized: {e}"),
    }
    let store = state.chat_store().await?;
    // Code questions: Claude only, and only when the conversation names a
    // project. omp's file tools can't be confined to a directory.
    let code = match conv.context.0.get("project").and_then(Value::as_str) {
        Some(project) if conv.provider == "claude" => {
            Some(code_checkout(state, conv, project).await?)
        }
        _ => None,
    };
    let messages = store.messages(&conv.id).await.map_err(|e| e.to_string())?;
    let outcomes = take_action_outcomes(store, &messages).await;
    // Everything before the message being answered, which is the last one.
    let earlier = &messages[..messages.len().saturating_sub(1)];

    let token = TurnToken::mint(caller.clone());
    let progress_id = conv.id.clone();
    let progress = ProgressSink(Arc::new(move |act: Activity| {
        if let Some(list) = RUNNING
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .get_mut(&progress_id)
        {
            list.push(act);
        }
    }));
    let turn = |session: Option<String>, prompt: String| ChatTurn {
        provider: conv.provider.clone(),
        model: conv.model.clone(),
        effort: None,
        prompt,
        cwd: chat_dir(&conv.id),
        code_dir: code.as_ref().map(|c| c.dir.clone()),
        session,
        mcp: ChatMcp {
            url: mcp_url(),
            token: token.0.clone(),
            tools: super::mcp_routes::tool_names(),
        },
        progress: Some(progress.clone()),
    };
    let effort = load_settings(state).await.1.effort;

    if let Some(session) = conv.session_id.clone() {
        let prompt = build_prompt(None, None, context, &outcomes, text);
        let mut t = turn(Some(session), prompt);
        t.effort = effort.clone();
        match chat::run_turn(t).await {
            Ok(reply) => return Ok(reply),
            // A session can be lost (a wiped volume, a CLI upgrade that moved
            // its store). Start a new one that is told what was said, rather
            // than failing every message from here on.
            Err(e) => tracing::warn!(
                "chat {}: resume failed ({}), starting a new session",
                conv.id,
                e.0
            ),
        }
    }
    let note = code.as_ref().map(|c| c.note.as_str());
    let prompt = build_prompt(Some(earlier), note, context, &outcomes, text);
    let mut t = turn(None, prompt);
    t.effort = effort;
    chat::run_turn(t).await.map_err(|e| e.0)
}

// ── Code checkouts ─────────────────────────────────────────────────────────

/// Where conversations' read-only checkouts live: `<root>/<project>/<key>`.
fn checkouts_root(state: &RunsState) -> PathBuf {
    state.projects_dir.join(".chat-checkouts")
}

/// A checkout is removed after a day nobody has read it.
const CHECKOUT_IDLE: std::time::Duration = std::time::Duration::from_secs(24 * 3600);

/// One checkout at a time: two conversations starting together must not both
/// build the same one.
static CHECKOUT_LOCK: LazyLock<tokio::sync::Mutex<()>> =
    LazyLock::new(|| tokio::sync::Mutex::new(()));

struct CodeCheckout {
    dir: PathBuf,
    /// For the prompt: which project, at which commits.
    note: String,
}

/// The commits a conversation is pinned to, from its context.
fn pinned_commits(context: &Value) -> Option<Vec<harness_runner::SnapshotRepo>> {
    let list = context.get("checkout")?.as_array()?;
    let repos: Vec<_> = list
        .iter()
        .filter_map(|r| {
            Some(harness_runner::SnapshotRepo {
                folder: r.get("folder").and_then(Value::as_str).map(str::to_string),
                sha: r.get("sha")?.as_str()?.to_string(),
            })
        })
        .collect();
    (!repos.is_empty()).then_some(repos)
}

/// The read-only checkout a conversation reads `project`'s code from.
///
/// The first turn checks out the tip of each repo's base branch and **pins**
/// those commits in the conversation's context, so later turns read the same
/// code (and run in the same directory, which Claude's resume needs) however
/// far the branch has moved. A pin that no longer resolves — a force-push that
/// dropped the commit — is replaced by a fresh one.
async fn code_checkout(
    state: &Arc<RunsState>,
    conv: &harness_persist::Conversation,
    project: &str,
) -> Result<CodeCheckout, String> {
    let p = state
        .project_store()
        .await?
        .get(project)
        .await
        .map_err(|e| e.to_string())?
        .ok_or_else(|| format!("project `{project}` is not registered"))?;
    // (folder, url, base branch). One repo sits at the root of the checkout.
    let layout: Vec<(Option<String>, String, String)> = if p.repos.is_empty() {
        vec![(None, p.git_url.clone(), p.base_branch.clone())]
    } else {
        super::runs_routes::plan_layout(&p.git_url, &p.base_branch, &p.repos)
            .into_iter()
            .map(|r| (Some(r.folder), r.url, r.base_branch))
            .collect()
    };
    if let Some(bad) = layout
        .iter()
        .filter_map(|(f, _, _)| f.as_deref())
        .find(|f| !super::runs_routes::valid_folder(f))
    {
        return Err(format!(
            "invalid repo folder `{bad}` in project `{project}`"
        ));
    }
    let token = state.github_token_for_project(project).await;
    let pinned = pinned_commits(&conv.context.0);

    let _one_at_a_time = CHECKOUT_LOCK.lock().await;
    let make = |pin: Option<Vec<harness_runner::SnapshotRepo>>| {
        let layout = layout.clone();
        let token = token.clone();
        let projects_dir = state.projects_dir.clone();
        let root = checkouts_root(state).join(project);
        let project = project.to_string();
        tokio::task::spawn_blocking(move || {
            let mut sources = Vec::new();
            for (folder, url, base) in layout {
                let (repo, tip) = match &folder {
                    // A single-repo project already has a clone the runs use.
                    None => {
                        let clone = projects_dir.join(&project);
                        if pin.is_none() {
                            let _ = harness_runner::fetch_repo(&clone, token.as_deref());
                        }
                        (clone, format!("origin/{base}"))
                    }
                    // Linked repos come from the persistent bare mirrors.
                    Some(_) => {
                        let mirrors = super::caches::git_mirror_root(&projects_dir);
                        let mirror =
                            harness_runner::ensure_mirror(&url, &mirrors, token.as_deref())
                                .map_err(|e| e.0)?;
                        (mirror, format!("refs/heads/{base}"))
                    }
                };
                let rev = pin
                    .as_ref()
                    .and_then(|p| p.iter().find(|r| r.folder == folder))
                    .map(|r| r.sha.clone())
                    .unwrap_or(tip);
                sources.push(harness_runner::SnapshotSource { repo, rev, folder });
            }
            harness_runner::chat_snapshot(&sources, &root).map_err(|e| e.0)
        })
    };
    let made = match make(pinned.clone()).await {
        Ok(Ok(made)) => made,
        Ok(Err(e)) if pinned.is_some() => {
            tracing::warn!(
                "chat {}: pinned checkout unavailable ({e}); re-pinning",
                conv.id
            );
            make(None)
                .await
                .map_err(|e| format!("checkout task failed: {e}"))??
        }
        Ok(Err(e)) => return Err(format!("could not check out `{project}`: {e}")),
        Err(e) => return Err(format!("checkout task failed: {e}")),
    };
    let (dir, repos) = made;

    if pinned.as_deref() != Some(repos.as_slice()) {
        let mut ctx = conv.context.0.clone();
        ctx["checkout"] = json!(repos
            .iter()
            .map(|r| json!({ "folder": r.folder, "sha": r.sha }))
            .collect::<Vec<_>>());
        if let Ok(store) = state.chat_store().await {
            let _ = store.set_context(&conv.id, &ctx).await;
        }
    }
    let at = repos
        .iter()
        .map(|r| {
            let sha = &r.sha[..r.sha.len().min(8)];
            match &r.folder {
                Some(f) => format!("`{f}/` at {sha}"),
                None => format!("{} at {sha}", p.base_branch),
            }
        })
        .collect::<Vec<_>>()
        .join(", ");
    Ok(CodeCheckout {
        dir,
        note: format!(
            "You can also read the code of project `{project}` ({at}): your working directory \
             is a read-only checkout of it, and you have Read, Grep and Glob. Cite file paths. \
             You cannot change files or run anything; to have code changed, the person starts a \
             run."
        ),
    })
}

/// Remove checkouts nobody has read for a day, hourly. No-op outside a Tokio
/// runtime (a synchronous router-build test), like the worktree sweeper.
pub(crate) fn spawn_checkout_sweeper(state: Arc<RunsState>) {
    if tokio::runtime::Handle::try_current().is_err() {
        return;
    }
    tokio::spawn(async move {
        let mut tick = tokio::time::interval(std::time::Duration::from_secs(3600));
        loop {
            tick.tick().await;
            let root = checkouts_root(&state);
            let _guard = CHECKOUT_LOCK.lock().await;
            let removed = tokio::task::spawn_blocking(move || {
                harness_runner::sweep_chat_snapshots(&root, CHECKOUT_IDLE)
            })
            .await
            .unwrap_or(0);
            if removed > 0 {
                tracing::info!("chat: removed {removed} idle code checkout(s)");
            }
        }
    });
}

// ── Prompts ────────────────────────────────────────────────────────────────

/// What the agent is told at the start of a session.
const PREAMBLE: &str = "You are the assistant built into ai-harness, a system that runs AI coding \
workflows (DAGs of agent steps) against a team's projects and opens pull requests. You are \
talking to someone in the harness's web UI. They may be an engineer, or a product or project \
manager who does not read code — answer in plain language and lead with the answer.

Your only tools are the harness's own (the `harness` MCP server). Use them; never guess run \
ids, statuses, PR numbers or workflow contents. Typical questions and where the answers are:
- Why did a run fail, or what is it doing: run_status to find the step, then \
run_node_output for that step's output; run_activity_errors, run_list.
- Which pull request to test for a Linear issue (e.g. ECOM-44), or whether a PR is merged: \
issue_prs, pr_status. Recommend one, say why in a sentence, and give its preview link.
- Why a Linear issue did not start: linear_check, linear_bindings, linear_states.
- Creating or changing a workflow: workflow_catalog and workflow_get first, workflow_validate \
before workflow_save. Saving keeps the previous version, so edits can be undone.

Starting runs and deleting things need the person's confirmation. When you call such a tool \
it is not run: a card asking them to confirm appears in the chat instead. Say in one or two \
sentences what it will do, then stop — do not call it again. You will be told the outcome \
with their next message.

Keep answers short. Use Markdown for lists and links.";

/// The prompt for one turn. `transcript` is `Some` when this is a new session
/// — the first message, or a resume that failed — and carries what was said
/// so far; a resumed session already has it.
fn build_prompt(
    transcript: Option<&[ChatMessage]>,
    code_note: Option<&str>,
    context: &Value,
    outcomes: &[String],
    text: &str,
) -> String {
    let mut out = String::new();
    if let Some(earlier) = transcript {
        out.push_str(PREAMBLE);
        out.push_str("\n\n");
        if let Some(note) = code_note {
            out.push_str(note);
            out.push_str("\n\n");
        }
        let said: Vec<&ChatMessage> = earlier
            .iter()
            .filter(|m| m.role == "user" || m.role == "assistant")
            .collect();
        if !said.is_empty() {
            out.push_str("## The conversation so far\n\n");
            let skip = said.len().saturating_sub(TRANSCRIPT_MESSAGES);
            for m in &said[skip..] {
                let who = if m.role == "user" { "Them" } else { "You" };
                out.push_str(&format!("{who}: {}\n\n", m.body.trim()));
            }
        }
    }
    if let Some(line) = context_line(context) {
        out.push_str(&line);
        out.push_str("\n\n");
    }
    for o in outcomes {
        out.push_str(o);
        out.push_str("\n\n");
    }
    if transcript.is_some() {
        out.push_str("## Their message\n\n");
    }
    out.push_str(text);
    out
}

/// What the person has open, so "this run" and "this workflow" resolve.
fn context_line(ctx: &Value) -> Option<String> {
    let s = |k: &str| ctx.get(k).and_then(Value::as_str);
    let mut parts = Vec::new();
    if let Some(run) = s("run_id") {
        parts.push(format!("run `{run}`"));
    }
    if let Some(wf) = s("workflow") {
        parts.push(format!("the workflow `{wf}` in the editor"));
    }
    if let Some(p) = s("project") {
        parts.push(format!("project `{p}`"));
    }
    (!parts.is_empty()).then(|| format!("(They have {} open.)", parts.join(" and ")))
}

/// Held actions that were confirmed or cancelled since the agent last spoke,
/// worded for its next prompt — and marked as told, so each is said once.
async fn take_action_outcomes(
    store: &harness_persist::ChatStore,
    messages: &[ChatMessage],
) -> Vec<String> {
    let mut out = Vec::new();
    for m in messages.iter().filter(|m| m.role == "action") {
        let Some(action) = m.action.as_ref().map(|a| a.0.clone()) else {
            continue;
        };
        if let Some(line) = outcome_line(&action) {
            out.push(line);
            let mut told = action;
            told["reported"] = json!(true);
            let _ = store.set_action(m.id, &told).await;
        }
    }
    out
}

/// One resolved action as a sentence; `None` while pending or once reported.
fn outcome_line(action: &Value) -> Option<String> {
    if action["reported"].as_bool() == Some(true) {
        return None;
    }
    let tool = action["tool"].as_str().unwrap_or("the tool");
    let result = action["result"].as_str().unwrap_or("").trim();
    match action["status"].as_str()? {
        "confirmed" => Some(format!(
            "(They confirmed `{tool}`. Result: {})",
            if result.is_empty() { "done" } else { result }
        )),
        "failed" => Some(format!(
            "(They confirmed `{tool}`, but it failed: {result})"
        )),
        "cancelled" => Some(format!("(They cancelled `{tool}`; it was not run.)")),
        _ => None,
    }
}

// ── Held actions ───────────────────────────────────────────────────────────

/// Called by `/mcp` instead of running a tool that needs confirmation: record
/// it as an action card, and tell the agent so.
pub(crate) async fn hold_for_confirmation(
    state: &Arc<RunsState>,
    caller: &ChatCaller,
    name: &str,
    args: &Value,
) -> Value {
    let store = match state.chat_store().await {
        Ok(s) => s,
        Err(e) => return super::mcp_routes::tool_error(e),
    };
    let action = json!({ "tool": name, "args": args, "status": "pending" });
    match store
        .add_message(
            &caller.conversation_id,
            "action",
            "",
            &json!([]),
            Some(&action),
        )
        .await
    {
        Ok(msg) => super::mcp_routes::to_result(
            format!(
                "Not run yet: `{name}` needs the person's confirmation, and a card asking them \
                 to confirm it is now showing in the chat. Tell them in one or two sentences \
                 what it will do, then stop. Do not call it again."
            ),
            &json!({ "status": "pending_confirmation", "action_id": msg.id }),
        ),
        Err(e) => super::mcp_routes::tool_error(e.to_string()),
    }
}

#[derive(Debug, Deserialize)]
pub struct ResolveRequest {
    pub confirm: bool,
}

/// `POST /api/chat/actions/{id}` — run a held tool call, or drop it.
pub async fn resolve_action(
    Extension(state): Extension<Arc<RunsState>>,
    headers: HeaderMap,
    AxumPath(message_id): AxumPath<i64>,
    Json(req): Json<ResolveRequest>,
) -> Response {
    let store = match state.chat_store().await {
        Ok(s) => s,
        Err(e) => return err(StatusCode::SERVICE_UNAVAILABLE, e),
    };
    let msg = match store.message(message_id).await {
        Ok(Some(m)) if m.role == "action" => m,
        Ok(_) => return err(StatusCode::NOT_FOUND, "no such action"),
        Err(e) => return err(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()),
    };
    let (_, user, actor) = match owned(&state, &headers, &msg.conversation_id).await {
        Ok(c) => c,
        Err(_) => return err(StatusCode::NOT_FOUND, "no such action"),
    };
    let mut action = msg.action.map(|a| a.0).unwrap_or_else(|| json!({}));
    if action["status"] != "pending" {
        return err(StatusCode::CONFLICT, "already answered");
    }
    if req.confirm {
        let tool = action["tool"].as_str().unwrap_or_default().to_string();
        let args = action["args"].clone();
        let trigger = ChatCaller {
            conversation_id: msg.conversation_id.clone(),
            user_id: user,
            actor,
        }
        .trigger();
        let result = super::mcp_routes::call_tool(&state, &trigger, &tool, &args).await;
        let failed = result["isError"].as_bool().unwrap_or(false);
        action["status"] = json!(if failed { "failed" } else { "confirmed" });
        action["result"] = result["content"][0]["text"].clone();
        if let Some(structured) = result.get("structuredContent") {
            action["structured"] = structured.clone();
        }
    } else {
        action["status"] = json!("cancelled");
    }
    if let Err(e) = store.set_action(message_id, &action).await {
        return err(StatusCode::INTERNAL_SERVER_ERROR, e.to_string());
    }
    Json(json!({ "id": message_id, "action": action })).into_response()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn opt(id: &'static str, models: &[&'static str]) -> ProviderInfo {
        ProviderInfo {
            id,
            label: id,
            models: models.to_vec(),
        }
    }

    #[test]
    fn the_stored_agent_wins_only_while_it_can_run() {
        let both = [
            opt("claude", &["opus", "sonnet"]),
            opt("pi", &["kimi-code/k2"]),
        ];
        let e = effective(&both, Some("pi"), Some("kimi-code/k2"), Some("high"));
        assert_eq!(e.provider.as_deref(), Some("pi"));
        assert_eq!(e.model.as_deref(), Some("kimi-code/k2"));
        assert_eq!(e.effort.as_deref(), Some("high"));

        // Pi disconnected: fall back to Claude, and not with Pi's model.
        let claude_only = [opt("claude", &["opus", "sonnet"])];
        let e = effective(&claude_only, Some("pi"), Some("kimi-code/k2"), None);
        assert_eq!(e.provider.as_deref(), Some("claude"));
        assert_eq!(e.model.as_deref(), Some("opus"));

        // Nothing connected: nothing to run on.
        assert_eq!(effective(&[], Some("claude"), None, None).provider, None);
    }

    #[test]
    fn a_turn_token_is_revoked_when_the_turn_ends() {
        let caller = ChatCaller {
            conversation_id: "c1".into(),
            user_id: Some("u1".into()),
            actor: Some("Jane".into()),
        };
        let token = TurnToken::mint(caller);
        let mut headers = HeaderMap::new();
        headers.insert(
            axum::http::header::AUTHORIZATION,
            format!("Bearer {}", token.0).parse().unwrap(),
        );
        let found = caller_for(&headers).expect("live token");
        assert_eq!(found.conversation_id, "c1");
        assert_eq!(found.trigger().source.as_deref(), Some(SOURCE_CHAT));
        drop(token);
        assert!(caller_for(&headers).is_none());

        // An MCP key or any other bearer is not a chat caller.
        headers.insert(
            axum::http::header::AUTHORIZATION,
            "Bearer hrn_mcp_abc".parse().unwrap(),
        );
        assert!(caller_for(&headers).is_none());
    }

    #[test]
    fn a_wildcard_bind_is_reached_over_loopback() {
        // The process-wide address may already be set by another test; this
        // only checks the shape.
        assert!(mcp_url().starts_with("http://"));
        assert!(mcp_url().ends_with("/mcp"));
    }

    fn msg(role: &str, body: &str) -> ChatMessage {
        ChatMessage {
            id: 0,
            conversation_id: "c".into(),
            role: role.into(),
            body: body.into(),
            activity: sqlx::types::Json(json!([])),
            action: None,
            created_at: chrono::Utc::now(),
        }
    }

    #[test]
    fn a_new_session_is_told_who_it_is_and_what_was_said() {
        let earlier = [
            msg("user", "which PR for ECOM-44?"),
            msg("assistant", "Test #12."),
            msg("error", "boom"),
        ];
        let p = build_prompt(
            Some(&earlier),
            Some("You can also read the code of project `shop`."),
            &json!({ "run_id": "r-9" }),
            &[],
            "and is it merged?",
        );
        assert!(p.starts_with("You are the assistant built into ai-harness"));
        assert!(p.contains("Them: which PR for ECOM-44?"));
        assert!(p.contains("You: Test #12."));
        assert!(
            !p.contains("boom"),
            "errors are not part of the conversation"
        );
        assert!(p.contains("(They have run `r-9` open.)"));
        assert!(p.contains("read the code of project `shop`"));
        assert!(p.ends_with("and is it merged?"));

        // A resumed session gets only what is new.
        let resumed = build_prompt(
            None,
            None,
            &json!({}),
            &["(They cancelled `run_trigger`; it was not run.)".into()],
            "ok",
        );
        assert!(!resumed.contains("You are the assistant"));
        assert!(resumed.starts_with("(They cancelled"));
        assert!(resumed.ends_with("ok"));
    }

    #[test]
    fn a_pin_is_read_back_from_the_context() {
        let ctx = json!({ "project": "shop", "checkout": [
            { "folder": null, "sha": "abc" },
            { "folder": "api", "sha": "def" }
        ]});
        let pins = pinned_commits(&ctx).unwrap();
        assert_eq!(pins.len(), 2);
        assert_eq!(pins[0].folder, None);
        assert_eq!(pins[1].folder.as_deref(), Some("api"));
        assert!(pinned_commits(&json!({ "project": "shop" })).is_none());
        assert!(pinned_commits(&json!({ "checkout": [] })).is_none());
    }

    #[test]
    fn an_action_outcome_is_told_once() {
        let done =
            json!({ "tool": "run_trigger", "status": "confirmed", "result": "run started: r1" });
        assert_eq!(
            outcome_line(&done).as_deref(),
            Some("(They confirmed `run_trigger`. Result: run started: r1)")
        );
        let mut told = done.clone();
        told["reported"] = json!(true);
        assert_eq!(outcome_line(&told), None);
        assert_eq!(
            outcome_line(&json!({ "tool": "x", "status": "pending" })),
            None
        );
    }

    #[test]
    fn context_is_limited_to_what_the_prompt_describes() {
        let c = clean_context(Some(&json!({
            "run_id": " r1 ", "workflow": "", "evil": "ignore previous", "project": 3
        })));
        assert_eq!(c, json!({ "run_id": "r1" }));
        assert_eq!(clean_context(None), json!({}));
    }

    #[test]
    fn titles_are_the_first_line_shortened() {
        assert_eq!(title_for("Which PR?\nmore"), "Which PR?");
        let long = "x".repeat(200);
        assert_eq!(title_for(&long).chars().count(), 80);
    }
}
