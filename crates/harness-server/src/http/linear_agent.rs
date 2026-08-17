//! **Linear agent sessions** — delegation and @-mentions as a run trigger.
//!
//! Delegating an issue to the app (or @-mentioning it) makes Linear open an
//! *agent session* and POST an `AgentSessionEvent` here. That replaces the
//! column-and-label poll as the way work is handed to the harness: a human
//! chooses, explicitly, in the Linear UI.
//!
//! - `POST /api/linear/webhook` — `AgentSessionEvent` (auth-exempt; see below)
//!
//! **Authentication.** Linear cannot send our API bearer token, so this route is
//! exempt from that middleware and authenticated instead by the
//! `Linear-Signature` header: a hex HMAC-SHA256 of the **raw** body under the
//! OAuth app's webhook signing secret, compared in constant time. Unsigned or
//! mis-signed requests are rejected before anything is parsed. `webhookTimestamp`
//! is additionally required to be within [`TIMESTAMP_TOLERANCE_MS`] to blunt
//! replays. With no secret stored, the endpoint refuses everything rather than
//! trusting unverified input.
//!
//! **The 10-second rule.** Linear marks a session unresponsive unless an activity
//! arrives within 10 seconds of `created`. So the handler emits the acknowledging
//! `thought` **inline, before** any slower work (workflow lookup, worktree, run
//! start), and only then spawns the run in the background. The HTTP response is
//! returned immediately either way — Linear needs a fast 200, not our run.

use std::collections::HashSet;
use std::sync::{Arc, LazyLock};

use axum::body::Bytes;
use axum::extract::Extension;
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::Json;
use harness_persist::LinearSource;
use harness_sources::linear::{
    parse_agent_session_event, AgentActivity, AgentSessionAction, AgentSessionEvent, IssueContext,
    LinearClient,
};
use hmac::{Hmac, Mac};
use sha2::Sha256;
use subtle::ConstantTimeEq;

use super::linear_oauth::{linear_client, webhook_secret};
use super::runs_routes::{start_run, CreateRunRequest, RunsState};

/// Header carrying the hex HMAC-SHA256 of the raw body.
const SIGNATURE_HEADER: &str = "linear-signature";

/// How far `webhookTimestamp` may be from now. Linear suggests about a minute.
const TIMESTAMP_TOLERANCE_MS: i64 = 60 * 1000;

/// Workflow fired for a delegated issue when the project has no binding to name
/// a better one. Mirrors the default used when filing an issue from a finding.
const DEFAULT_WORKFLOW: &str = "idea-to-pr";

fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

/// Constant-time compare of the received signature against one computed over
/// `body`. Rejects a malformed header rather than falling back to equality.
fn signature_matches(secret: &str, body: &[u8], received: &str) -> bool {
    let Ok(expected) = compute_signature(secret, body) else {
        return false;
    };
    // Compare the hex strings in constant time: same length for any valid input,
    // so length alone leaks nothing useful.
    let a = expected.as_bytes();
    let b = received.trim().as_bytes();
    a.len() == b.len() && a.ct_eq(b).into()
}

fn compute_signature(secret: &str, body: &[u8]) -> Result<String, String> {
    let mut mac = Hmac::<Sha256>::new_from_slice(secret.as_bytes())
        .map_err(|e| format!("bad webhook secret: {e}"))?;
    mac.update(body);
    Ok(mac
        .finalize()
        .into_bytes()
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect())
}

/// Reject a payload whose `webhookTimestamp` is outside the tolerance. Absent
/// timestamp is accepted — not every Linear event carries one, and the signature
/// is the primary control.
fn timestamp_fresh(body: &[u8], now: i64) -> bool {
    let Ok(v) = serde_json::from_slice::<serde_json::Value>(body) else {
        // Unparseable bodies are caught later by the event parser.
        return true;
    };
    match v.get("webhookTimestamp").and_then(|t| t.as_i64()) {
        Some(ts) => (now - ts).abs() <= TIMESTAMP_TOLERANCE_MS,
        None => true,
    }
}

/// `POST /api/linear/webhook` — an agent session was created or prompted.
///
/// Always answers 200 once the signature checks out, including for event types
/// we ignore: a non-2xx makes Linear retry and eventually disable the webhook.
pub async fn webhook(
    Extension(state): Extension<Arc<RunsState>>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    let store = match state.cred_store().await {
        Ok(s) => s,
        Err(e) => return err(StatusCode::SERVICE_UNAVAILABLE, e),
    };
    // No secret stored → we cannot verify anything, so we trust nothing.
    let Some(secret) = webhook_secret(store).await else {
        tracing::warn!(
            "linear webhook: rejected — no webhook signing secret stored (save it on the \
             Credentials page)"
        );
        return err(
            StatusCode::PRECONDITION_FAILED,
            "no Linear webhook signing secret configured",
        );
    };
    let Some(received) = headers
        .get(SIGNATURE_HEADER)
        .and_then(|v| v.to_str().ok())
        .filter(|s| !s.is_empty())
    else {
        return err(StatusCode::UNAUTHORIZED, "missing Linear-Signature header");
    };
    if !signature_matches(&secret, &body, received) {
        tracing::warn!("linear webhook: rejected — signature mismatch");
        return err(StatusCode::UNAUTHORIZED, "invalid signature");
    }
    if !timestamp_fresh(&body, now_ms()) {
        tracing::warn!("linear webhook: rejected — stale webhookTimestamp (replay?)");
        return err(StatusCode::UNAUTHORIZED, "stale webhook timestamp");
    }

    // Signature is good. From here on every outcome is a 200 — Linear retries
    // non-2xx and disables a webhook that keeps failing.
    let event = match parse_agent_session_event(&body) {
        Ok(Some(e)) => e,
        // Some other event type (Issue, Comment, OAuthApp revoked…). Ignored.
        Ok(None) => return ack(),
        Err(e) => {
            tracing::warn!(
                "linear webhook: could not parse agent session event: {}",
                e.0
            );
            return ack();
        }
    };

    // Handle off the response path. Linear resends any delivery that takes more
    // than 5 seconds, so the 200 must not wait on a token refresh, a GraphQL
    // round trip or a run start — all of which this does.
    match event.action {
        AgentSessionAction::Created => {
            let state = state.clone();
            tokio::spawn(async move { handle_created(&state, event).await });
        }
        AgentSessionAction::Prompted => {
            let state = state.clone();
            tokio::spawn(async move { handle_prompted(&state, event).await });
        }
        AgentSessionAction::Other(ref a) => {
            tracing::debug!("linear webhook: ignoring agent session action `{a}`");
        }
    }
    ack()
}

// ── Duplicate-delivery guard ─────────────────────────────────────────────────
//
// Two layers, because Linear's retries span very different timescales:
//   * this in-process set catches a *concurrent* or near-immediate resend, before
//     any claim row exists to check against;
//   * `claim_exists_for_session` catches the 1-minute / 1-hour / 6-hour retries,
//     by which time the first delivery has recorded its claim.
// In-process is sufficient for the first layer because the harness runs as a
// single container; a restart mid-flight falls through to the database check.

static SESSIONS_IN_FLIGHT: LazyLock<std::sync::Mutex<HashSet<String>>> =
    LazyLock::new(|| std::sync::Mutex::new(HashSet::new()));

/// Claims exclusive handling of a session, releasing it on drop so an early
/// return or a panic can't wedge the session permanently.
///
/// Also held by the **poller** across opening a session of its own: if Linear
/// echoes an `AgentSessionEvent created` back for a session we just created
/// ourselves, [`handle_created`] must return before posting anything rather than
/// acknowledging and then refusing in the very thread we are about to stream into.
pub(crate) struct SessionGuard(String);

impl SessionGuard {
    /// `None` when another task is already handling this session.
    pub(crate) fn acquire(session_id: &str) -> Option<Self> {
        let mut set = SESSIONS_IN_FLIGHT.lock().ok()?;
        set.insert(session_id.to_string())
            .then(|| SessionGuard(session_id.to_string()))
    }
}

impl Drop for SessionGuard {
    fn drop(&mut self) {
        if let Ok(mut set) = SESSIONS_IN_FLIGHT.lock() {
            set.remove(&self.0);
        }
    }
}

/// Whether this `created` event is a redelivery of one already handled.
async fn already_handled(state: &Arc<RunsState>, session_id: &str) -> bool {
    match state.linear_claim_store().await {
        Ok(store) => store
            .claim_exists_for_session(session_id)
            .await
            .unwrap_or(false),
        // No database to check against — proceed rather than drop the work.
        Err(_) => false,
    }
}

fn ack() -> Response {
    (StatusCode::OK, Json(serde_json::json!({ "ok": true }))).into_response()
}

fn err(status: StatusCode, msg: impl Into<String>) -> Response {
    (status, Json(serde_json::json!({ "error": msg.into() }))).into_response()
}

/// A session was opened: acknowledge inside 10s, then start the run.
///
/// Runs off the response path (see [`webhook`]), so it may take as long as it
/// needs — but the acknowledgement still comes first, because Linear marks a
/// session unresponsive without one inside 10 seconds.
async fn handle_created(state: &Arc<RunsState>, event: AgentSessionEvent) {
    // Deduplicate before doing anything observable: a redelivery must not post a
    // second acknowledgement, let alone start a second run.
    let Some(_guard) = SessionGuard::acquire(&event.session_id) else {
        tracing::info!(
            "linear webhook: session {} is already being handled; ignoring redelivery",
            event.session_id
        );
        return;
    };
    if already_handled(state, &event.session_id).await {
        tracing::info!(
            "linear webhook: session {} already has a run; ignoring redelivery",
            event.session_id
        );
        return;
    }

    let client = match linear_client(state).await {
        Ok(c) => c,
        Err(e) => {
            tracing::warn!("linear webhook: no usable Linear credential: {e}");
            return;
        }
    };

    // ── The 10-second acknowledgement. Nothing slower may precede this. ──
    let ack_body = match &event.issue_identifier {
        Some(id) => format!("Picking up {id} — working out which workflow to run."),
        None => "Picking this up — working out which workflow to run.".to_string(),
    };
    if let Err(e) = client
        .create_agent_activity(
            &event.session_id,
            &AgentActivity::Thought { body: ack_body },
        )
        .await
    {
        // Non-fatal: the session may read as unresponsive, but the run is still
        // worth starting.
        tracing::warn!(
            "linear webhook: failed to acknowledge session {}: {}",
            event.session_id,
            e.0
        );
    }

    // Awaited, not spawned: `_guard` must outlive the run start, otherwise the
    // reservation would be released before the claim row exists and a concurrent
    // redelivery could slip between the two checks.
    start_delegated_run(state, event, client).await;
}

/// Resolve project + workflow for a delegated issue, fire the run, and report
/// the outcome into the session.
async fn start_delegated_run(
    state: &Arc<RunsState>,
    event: AgentSessionEvent,
    client: LinearClient,
) {
    let session = event.session_id.clone();
    let binding = match resolve_target(state, &event).await {
        Target::Ready { binding } => binding,
        Target::NoBinding => {
            let _ = client
                .create_agent_activity(
                    &session,
                    &AgentActivity::Error {
                        body: "No enabled Linear trigger covers this issue's team. Add one for \
                               the project on the Projects page — or enable the existing one — \
                               then delegate again."
                            .into(),
                    },
                )
                .await;
            return;
        }
        // The configured source statuses are still the gate — say so plainly, and
        // name the statuses that would have worked.
        Target::WrongStatus {
            current,
            team_id,
            triggers,
        } => {
            let body = wrong_status_message(&client, &current, team_id.as_deref(), &triggers).await;
            let _ = client
                .create_agent_activity(&session, &AgentActivity::Error { body })
                .await;
            return;
        }
    };

    // Honour **Max simultaneous tasks** before touching anything. Checked here,
    // ahead of the status move and the run start, so a refused delegation leaves the
    // issue exactly where it was: still delegated, still in the source status — which
    // is precisely what a live binding's poller looks for, so it starts by itself
    // once a slot frees.
    if let Ok(claims) = state.linear_claim_store().await {
        match claims
            .count_active(&binding.project, &binding.workflow)
            .await
        {
            Ok(active) if at_capacity(active, binding.max_concurrent_runs) => {
                tracing::info!(
                    "linear webhook: {}/{} at capacity ({active} active) — leaving {} for the \
                     poller",
                    binding.project,
                    binding.workflow,
                    event.issue_identifier.as_deref().unwrap_or("(delegated)")
                );
                // A `response`, not an `error`: nothing failed, and it closes this
                // session cleanly rather than leaving it open to go stale. The run
                // that eventually starts opens a session of its own.
                let _ = client
                    .create_agent_activity(
                        &session,
                        &AgentActivity::Response {
                            body: at_capacity_message(&binding),
                        },
                    )
                    .await;
                return;
            }
            Ok(_) => {}
            Err(e) => {
                // Can't count — proceed rather than refuse work over a database
                // hiccup. The poller's own cap still applies to its claims.
                tracing::warn!("linear webhook: count_active failed, proceeding: {e}");
            }
        }
    }

    let title = match (&event.issue_identifier, &event.issue_title) {
        (Some(id), Some(t)) => Some(format!("{id} {t}")),
        (Some(id), None) => Some(id.clone()),
        (None, Some(t)) => Some(t.clone()),
        (None, None) => None,
    };
    // Images pasted into the issue are downloaded and their links rewritten to
    // local paths, so the agent can see them rather than getting a URL it has no
    // credential for. Keyed by issue identifier, falling back to the session id.
    let description = super::linear_attachments::localize(
        &client,
        &super::linear_attachments::attachments_root(&state.projects_dir),
        event
            .issue_identifier
            .as_deref()
            .unwrap_or(&event.session_id),
        &task_text(&event),
    )
    .await;
    // Move the issue out of the source status before firing, exactly as the
    // poller does. This is not only cosmetic: leaving the source column is the
    // *claim signal*. An issue that stays there is still delegated and still in
    // the source status, so the poller remains eligible to claim it — today only
    // `max_concurrent_runs` stops a second run, which stops nothing once a
    // binding raises that above 1.
    if let (Some(issue_id), Some(in_progress)) = (
        event.issue_id.as_deref(),
        binding.in_progress_state_id.as_deref(),
    ) {
        if let Err(e) = client.set_issue_state(issue_id, in_progress).await {
            // Non-fatal: better to run the work than to refuse over a status move.
            tracing::warn!(
                "linear webhook: could not move {} to in-progress: {}",
                event.issue_identifier.as_deref().unwrap_or(issue_id),
                e.0
            );
        }
    }

    let req = CreateRunRequest {
        workflow: binding.workflow.clone(),
        title,
        description,
        args: String::new(),
        real: true,
        base_branch: binding.base_branch.clone(),
        project: Some(binding.project.clone()),
        swap_from: None,
        swap_to: None,
        ab_pair_id: None,
        ab_arm: None,
        ab_label: None,
    };

    match start_run(state, req).await {
        Ok(run_id) => {
            // Link the run to the session so status-sync reports progress back
            // into this thread (see `linear_poller::sync_active_claims`).
            if let Ok(claims) = state.linear_claim_store().await {
                if let Err(e) = claims
                    .record(
                        &run_id,
                        &binding.project,
                        &binding.workflow,
                        event.issue_id.as_deref().unwrap_or_default(),
                        event.issue_identifier.as_deref().unwrap_or("(delegated)"),
                        &binding.source_state_id,
                        Some(&session),
                    )
                    .await
                {
                    tracing::warn!("linear webhook: failed to record claim for {run_id}: {e}");
                }
            }
            let run_link = state
                .public_url
                .as_deref()
                .map(|b| format!("{b}/runs/{run_id}"));
            let _ = client
                .create_agent_activity(
                    &session,
                    &AgentActivity::Action {
                        action: "Started workflow".into(),
                        parameter: binding.workflow.clone(),
                        result: run_link.clone(),
                    },
                )
                .await;
            // Attach the run link to the issue as well, matching the poller.
            if let (Some(issue_id), Some(url)) = (event.issue_id.as_deref(), run_link.as_deref()) {
                let _ = client.add_attachment(issue_id, url, "ai-harness run").await;
            }
            tracing::info!(
                "linear webhook: session {session} → run {run_id} (`{}` in `{}`)",
                binding.workflow,
                binding.project
            );
        }
        // `start_run` reports failures as (status, message).
        Err((status, message)) => {
            tracing::warn!(
                "linear webhook: failed to start run for session {session}: {} {message}",
                status.as_u16()
            );
            let _ = client
                .create_agent_activity(
                    &session,
                    &AgentActivity::Error {
                        body: format!(
                            "Could not start the `{}` workflow: {message}",
                            binding.workflow
                        ),
                    },
                )
                .await;
        }
    }
}

/// A follow-up message in an existing session.
///
/// The harness has no mid-run steering channel yet, so this is acknowledged
/// honestly rather than silently dropped.
/// What to say when there is no run to talk about — an @-mention on an issue nobody
/// delegated, or a session whose claim has been cleared by a Rerun.
const NO_RUN_REPLY: &str = "I don't have a run for this issue, so there's nothing for me to \
                            report on. Delegate the issue to me while it's in a trigger's source \
                            status and I'll pick it up.";

/// What to say when a run exists but the message asks for a change of course.
const CANNOT_STEER_REPLY: &str = "Noted — but I can't change course mid-run yet. Let this run \
                                  finish (or cancel it in the harness), then delegate again with \
                                  the updated ask.";

/// How much of one artifact to inline when answering. `plan.md` runs to several
/// thousand words; the head of it carries the summary and file list, which is what
/// questions are usually about.
const ARTIFACT_EXCERPT_CHARS: usize = 6000;

/// A follow-up message on a session.
///
/// Answering "what are you doing?" needs no access to the repo and must not touch
/// the run — everything a question about progress can want is already in Postgres
/// (node statuses, timings, and the exploration/plan artifacts). So this builds the
/// whole context inline and asks a model to answer from it, rather than starting a
/// run or opening a worktree.
///
/// Steering is a separate problem and still refused, honestly: nothing here edits
/// the DAG in flight.
async fn handle_prompted(state: &Arc<RunsState>, event: AgentSessionEvent) {
    let Ok(client) = linear_client(state).await else {
        return;
    };
    let question = event.prompt_body.clone().unwrap_or_default();
    let body = match answer_for_session(state, &event.session_id, &question).await {
        Some(answer) => answer,
        None => NO_RUN_REPLY.to_string(),
    };
    // A `thought`, never a `response`: Linear treats a response as the agent's final
    // word and marks the session complete, which would end the thread while the run
    // is still going.
    if let Err(e) = client
        .create_agent_activity(&event.session_id, &AgentActivity::Thought { body })
        .await
    {
        tracing::warn!(
            "linear webhook: failed to answer prompt on session {}: {}",
            event.session_id,
            e.0
        );
    }
}

/// Answer a question about the run behind `session`. `None` when there is no run.
///
/// Every failure below the "is there a run" check degrades to the honest refusal
/// rather than silence: a database hiccup or an agent that won't start is not a
/// reason to leave someone's question unanswered in a thread.
async fn answer_for_session(
    state: &Arc<RunsState>,
    session: &str,
    question: &str,
) -> Option<String> {
    let claims = state.linear_claim_store().await.ok()?;
    let claim = claims.claim_for_session(session).await.ok()??;
    let store = state.store().await.ok()?;
    let detail = store.get_run(&claim.run_id).await.ok()??;

    let agent = state.agent_registry().get("claude").or_else(|| {
        tracing::warn!("linear webhook: no `claude` agent registered to answer a session prompt");
        None
    })?;
    let req = harness_core::agent::AgentRequest {
        prompt: session_answer_prompt(question, &detail),
        // Nothing is read from disk — the prompt says so and carries the context —
        // but the CLI still needs a working directory to start in.
        project_root: state.projects_dir.clone(),
        ..Default::default()
    };
    match agent.execute(req).await {
        Ok(res) if !res.output.trim().is_empty() => Some(res.output.trim().to_string()),
        Ok(_) => {
            tracing::warn!("linear webhook: empty answer for session {session}");
            Some(CANNOT_STEER_REPLY.to_string())
        }
        Err(e) => {
            tracing::warn!("linear webhook: could not answer session {session}: {e}");
            Some(CANNOT_STEER_REPLY.to_string())
        }
    }
}

/// The prompt for answering a follow-up, with the run's state inlined.
///
/// Pure so the context assembly is unit-tested without an agent, matching how the
/// rest of this module is tested.
fn session_answer_prompt(question: &str, detail: &harness_persist::RunDetail) -> String {
    let mut out = String::new();
    out.push_str(
        "You are the ai-harness agent, answering a question in a Linear thread about a run \
         you are executing. Everything you know is below — do NOT read files, run commands, \
         or use tools, and do NOT try to change the run.\n\n",
    );
    out.push_str("## The message\n\n");
    out.push_str(if question.trim().is_empty() {
        "(no text — treat it as \"what is happening?\")"
    } else {
        question.trim()
    });
    out.push_str("\n\n## The run\n\n");
    out.push_str(&format!(
        "- workflow: `{}`\n- status: {}\n- steps declared: {}\n",
        detail.run.workflow_name,
        detail.run.status,
        detail.graph.len()
    ));
    out.push_str("\n### Steps so far\n\n");
    for node in &detail.nodes {
        out.push_str(&format!("- `{}` — {}\n", node.node_id, node.status));
    }
    for node in &detail.nodes {
        let Some(artifact) = node.artifact_content.as_deref() else {
            continue;
        };
        if artifact.trim().is_empty() {
            continue;
        }
        out.push_str(&format!("\n### Artifact from `{}`\n\n", node.node_id));
        out.push_str(&excerpt(artifact, ARTIFACT_EXCERPT_CHARS));
        out.push('\n');
    }
    out.push_str(
        "\n## How to reply\n\nA short paragraph of plain prose. No headings, no bullet \
         lists, no preamble — this goes straight into an issue thread.\n\nIf the message is a \
         question, answer it from the context above and say plainly when the context doesn't \
         cover it. If it is an instruction to do something differently, reply with exactly \
         this and nothing else:\n\n",
    );
    out.push_str(CANNOT_STEER_REPLY);
    out.push('\n');
    out
}

/// First `max` characters, on a character boundary, with a marker when cut.
fn excerpt(text: &str, max: usize) -> String {
    if text.chars().count() <= max {
        return text.to_string();
    }
    let head: String = text.chars().take(max).collect();
    format!("{head}\n\n…(truncated)")
}

/// The task text handed to the workflow, with the follow-up prompt appended when
/// one came with the event.
fn task_text(event: &AgentSessionEvent) -> String {
    let mut out = String::new();
    if let Some(id) = &event.issue_identifier {
        out.push_str(&format!("# {id}"));
        if let Some(t) = &event.issue_title {
            out.push_str(&format!(" {t}"));
        }
        out.push_str("\n\n");
    }
    if let Some(body) = event.task_text() {
        out.push_str(body);
        out.push('\n');
    }
    if let Some(g) = &event.guidance {
        if !g.trim().is_empty() {
            out.push_str(&format!("\n## Workspace guidance\n\n{g}\n"));
        }
    }
    if let Some(p) = &event.prompt_body {
        if !p.trim().is_empty() {
            out.push_str(&format!("\n## Additional instruction\n\n{p}\n"));
        }
    }
    out
}

/// What a delegated issue resolves to, or why it doesn't.
enum Target {
    /// Startable: matched a binding, and the issue is in its source status.
    ///
    /// Carries the whole binding rather than a copy of the fields it needs. Every
    /// time delegation turned out to be missing something the poller does —
    /// `enabled`, the in-progress move, the concurrency cap — the fix was another
    /// field here. Handing over the row removes that as a recurring edit and makes
    /// the two paths read from the same object.
    ///
    /// `workflow` is normalized before this is built, so callers need not repeat
    /// the empty-name fallback.
    Ready { binding: Box<LinearSource> },
    /// No Linear trigger binding covers the issue's team.
    NoBinding,
    /// The team has bindings, but none of them triggers from the status this
    /// issue is in.
    WrongStatus {
        /// The status the issue is actually in, named where Linear told us.
        current: String,
        /// Team whose bindings were considered, for naming their statuses.
        team_id: Option<String>,
        /// The `(source_state_id, workflow)` pairs that *would* have started —
        /// what to tell the user instead.
        triggers: Vec<(String, String)>,
    },
}

/// Which project/workflow a delegated issue belongs to, and whether it is
/// startable.
///
/// Delegation carries no binding of its own, so the issue's **team** is matched
/// against the Linear trigger bindings — the same rows that configure the status
/// map — and that binding supplies the project, workflow and base branch.
///
/// The binding's **source status is still the gate**: delegating an issue that
/// is sitting in Backlog does not start it. Delegation says *who* should do the
/// work; the status says *when* it is ready.
async fn resolve_target(state: &Arc<RunsState>, event: &AgentSessionEvent) -> Target {
    let Ok(sources) = state.linear_source_store().await else {
        return Target::NoBinding;
    };
    // `list_all`, not `list_enabled`: `enabled`/`live` govern only the column
    // poller, and delegation-only is the expected setup (poller off, bindings
    // kept as the team → project/workflow/status-map mapping).
    let Ok(bindings) = sources.list_all().await else {
        return Target::NoBinding;
    };
    choose_target(&bindings, &issue_context(state, event).await)
}

/// The binding-selection and status-gate decision, split out so it is testable
/// without a database.
///
/// **The status selects the binding.** A team normally has several — `Todo →
/// idea-to-pr`, `Ready for merge → merge-pr`, `Changes requested → revise-pr` —
/// so the issue's current status is what picks the workflow, exactly as it does
/// for the column poller. (Checking the status against one arbitrarily-chosen
/// binding of the team would let only that one workflow ever start.)
///
/// **Disabled bindings are skipped.** `enabled` means "this binding is active",
/// for delegation as much as for the poller: unchecking it must stop work
/// arriving by any route. It previously governed only the poller, so a disabled
/// binding still won delegation — and when two bindings shared a source status,
/// the disabled one could shadow the one that was meant to run. `live` remains
/// poller-only (claim vs. dry-run), so a delegation-only setup is `enabled` on,
/// `live` off.
fn choose_target(bindings: &[LinearSource], context: &IssueContext) -> Target {
    let active: Vec<&LinearSource> = bindings.iter().filter(|b| b.enabled).collect();
    // Candidates: every active binding for the issue's team. With an unknown
    // team, a lone active binding is assumed to be the intended one.
    let candidates: Vec<&LinearSource> = match context.team_id.as_deref() {
        Some(team) => active
            .iter()
            .copied()
            .filter(|b| b.team_id == team)
            .collect(),
        None => Vec::new(),
    };
    let candidates = if !candidates.is_empty() {
        candidates
    } else if active.len() == 1 {
        vec![active[0]]
    } else {
        return Target::NoBinding;
    };

    // Pick the binding whose source status the issue is actually in. Two bindings
    // sharing a source status would be ambiguous; the first in `list_all` order
    // (project, then workflow) wins deterministically.
    let chosen = context
        .state_id
        .as_deref()
        .and_then(|state| candidates.iter().find(|b| b.source_state_id == state));

    let Some(chosen) = chosen else {
        // In a status none of the team's bindings trigger from — including the
        // case where Linear didn't tell us the status at all. Refusing is safer
        // than starting work from a status the operator never nominated.
        return Target::WrongStatus {
            current: context
                .state_name
                .clone()
                .or_else(|| context.state_id.clone())
                .unwrap_or_else(|| "an unknown status".to_string()),
            team_id: context.team_id.clone(),
            triggers: candidates
                .iter()
                .map(|b| {
                    let workflow = if b.workflow.is_empty() {
                        DEFAULT_WORKFLOW.to_string()
                    } else {
                        b.workflow.clone()
                    };
                    (b.source_state_id.clone(), workflow)
                })
                .collect(),
        };
    };

    let mut binding = (**chosen).clone();
    // A binding names the workflow it fires; fall back to the conventional default
    // only if the row somehow carries none. Normalized here so every caller sees a
    // usable name.
    if binding.workflow.is_empty() {
        binding.workflow = DEFAULT_WORKFLOW.to_string();
    }
    Target::Ready {
        binding: Box::new(binding),
    }
}

/// Whether a binding is already running as many claims as it allows.
///
/// The poller checks the same thing before claiming; delegation did not, so
/// handing three issues to the harness at once started three runs regardless of
/// **Max simultaneous tasks**.
pub(crate) fn at_capacity(active: i64, max_concurrent_runs: i32) -> bool {
    active >= max_concurrent_runs.max(1) as i64
}

/// What to say when a delegation arrives while the binding is already at capacity.
///
/// Deliberately not phrased as an error — nothing is wrong, the work is simply
/// waiting. What is *true* about the wait depends on `live`: only a live binding
/// has a poller that will come back for the issue. Promising a pickup that will
/// never happen would be worse than saying nothing.
fn at_capacity_message(binding: &LinearSource) -> String {
    let limit = binding.max_concurrent_runs.max(1);
    let running = if limit == 1 {
        "Another task is already running".to_string()
    } else {
        format!("{limit} tasks are already running")
    };
    if binding.live {
        format!(
            "{running} for `{}`, which is its limit. I've left this issue where it is \
             and will start it automatically once a slot frees.",
            binding.workflow
        )
    } else {
        format!(
            "{running} for `{}`, which is its limit. I've left this issue where it is \
             — delegate it again once the current work finishes.",
            binding.workflow
        )
    }
}

/// Explain a refusal by naming the statuses that *do* trigger something.
///
/// Best-effort: status names come from discovery, falling back to bare workflow
/// names if that call fails — a refusal message is never worth failing over.
async fn wrong_status_message(
    client: &LinearClient,
    current: &str,
    team_id: Option<&str>,
    triggers: &[(String, String)],
) -> String {
    if triggers.is_empty() {
        return format!(
            "This issue is in {current}, and no Linear trigger for its team says which status to \
             start from. Configure one on the Projects page."
        );
    }
    // state id → name, for this team only.
    let names: Vec<(Option<String>, &String)> = match (team_id, client.discover().await) {
        (Some(team), Ok(discovery)) => {
            let states = discovery
                .teams
                .into_iter()
                .find(|t| t.id == team)
                .map(|t| t.states)
                .unwrap_or_default();
            triggers
                .iter()
                .map(|(state_id, workflow)| {
                    let name = states
                        .iter()
                        .find(|s| &s.id == state_id)
                        .map(|s| s.name.clone());
                    (name, workflow)
                })
                .collect()
        }
        _ => triggers.iter().map(|(_, w)| (None, w)).collect(),
    };
    let list = names
        .iter()
        .map(|(name, workflow)| match name {
            Some(n) => format!("{n} → `{workflow}`"),
            None => format!("`{workflow}`"),
        })
        .collect::<Vec<_>>()
        .join(", ");
    format!(
        "This issue is in {current}, which isn't a status I start from. Move it to one of these \
         and delegate again: {list}."
    )
}

/// Where a delegated issue sits — team, status, delegate. Empty when the event
/// carried no issue id or the lookup failed.
async fn issue_context(state: &Arc<RunsState>, event: &AgentSessionEvent) -> IssueContext {
    let Some(issue_id) = event.issue_id.as_deref() else {
        return IssueContext::default();
    };
    let Ok(client) = linear_client(state).await else {
        return IssueContext::default();
    };
    client
        .issue_context(issue_id)
        .await
        .unwrap_or_else(|_| IssueContext::default())
}

#[cfg(test)]
mod tests {
    use super::*;
    use harness_sources::linear::parse_agent_session_event;

    /// Known-answer vectors (independently computed with `openssl dgst -sha256
    /// -hmac`). These pin the algorithm *and* the hex encoding — a self-consistent
    /// round-trip would pass even if we signed the wrong thing.
    #[test]
    fn signature_is_hex_hmac_sha256_of_the_raw_body() {
        assert_eq!(
            compute_signature("secret", b"body").unwrap(),
            "dc46983557fea127b43af721467eb9b3fde2338fe3e14f51952aa8478c13d355"
        );
        assert_eq!(
            compute_signature("k", b"v").unwrap(),
            "c5d4be1992d50d3b41f9a21292fc67a28a1486fc64a0517d37f9af847e0732de"
        );
    }

    #[test]
    fn signature_matches_is_exact_and_rejects_tampering() {
        let secret = "whsec_abc";
        let body = br#"{"type":"AgentSessionEvent","action":"created"}"#;
        let good = compute_signature(secret, body).unwrap();
        assert!(signature_matches(secret, body, &good));
        // Surrounding whitespace in the header is tolerated.
        assert!(signature_matches(secret, body, &format!("  {good}  ")));
        // A different secret, a changed body, or a truncated/garbage signature all fail.
        assert!(!signature_matches("other", body, &good));
        assert!(!signature_matches(secret, b"tampered", &good));
        assert!(!signature_matches(secret, body, &good[..good.len() - 2]));
        assert!(!signature_matches(secret, body, "not-hex"));
        assert!(!signature_matches(secret, body, ""));
    }

    #[test]
    fn signature_is_lowercase_hex_of_expected_length() {
        let sig = compute_signature("k", b"v").unwrap();
        // SHA-256 → 32 bytes → 64 hex chars, lowercase.
        assert_eq!(sig.len(), 64);
        assert!(sig.chars().all(|c| c.is_ascii_hexdigit()));
        assert_eq!(sig, sig.to_lowercase());
    }

    #[test]
    fn timestamp_within_tolerance_is_fresh() {
        let now = 1_700_000_000_000;
        let at = |ts: i64| format!(r#"{{"webhookTimestamp":{ts}}}"#).into_bytes();
        assert!(timestamp_fresh(&at(now), now));
        assert!(timestamp_fresh(&at(now - TIMESTAMP_TOLERANCE_MS), now));
        assert!(timestamp_fresh(&at(now + TIMESTAMP_TOLERANCE_MS), now));
        // Outside the window — a replayed capture.
        assert!(!timestamp_fresh(&at(now - TIMESTAMP_TOLERANCE_MS - 1), now));
        assert!(!timestamp_fresh(&at(now + TIMESTAMP_TOLERANCE_MS + 1), now));
        // Absent or unparseable → not rejected here (signature is the control).
        assert!(timestamp_fresh(br#"{"type":"Issue"}"#, now));
        assert!(timestamp_fresh(b"not json", now));
    }

    #[test]
    fn task_text_leads_with_the_issue_and_appends_guidance_and_prompt() {
        let json = br#"{
            "type":"AgentSessionEvent","action":"created",
            "guidance":"Always open a PR",
            "agentSession":{"id":"s","issue":{
                "identifier":"COR-7","title":"Fix login","description":"It 500s."}}}"#;
        let ev = parse_agent_session_event(json).unwrap().unwrap();
        let text = task_text(&ev);
        assert!(text.starts_with("# COR-7 Fix login"), "{text}");
        assert!(text.contains("It 500s."), "{text}");
        assert!(text.contains("## Workspace guidance"), "{text}");
        assert!(text.contains("Always open a PR"), "{text}");
        // No prompt on a `created` event.
        assert!(!text.contains("## Additional instruction"), "{text}");
    }

    /// An **enabled** binding — the normal case. `live` stays off, since it is
    /// poller-only and delegation must work without it.
    fn binding(team: &str, workflow: &str, source_state: &str) -> LinearSource {
        LinearSource {
            enabled: true,
            ..disabled_binding(team, workflow, source_state)
        }
    }

    /// A binding with a full status map, as the UI produces.
    fn binding_with_status_map(team: &str, workflow: &str, source_state: &str) -> LinearSource {
        LinearSource {
            in_progress_state_id: Some("in-progress".into()),
            review_state_id: Some("in-review".into()),
            ready_state_id: Some("ready".into()),
            ..binding(team, workflow, source_state)
        }
    }

    fn disabled_binding(team: &str, workflow: &str, source_state: &str) -> LinearSource {
        let now = chrono::Utc::now();
        LinearSource {
            project: format!("{team}-project"),
            workflow: workflow.into(),
            team_id: team.into(),
            team_name: team.into(),
            source_state_id: source_state.into(),
            failed_label: None,
            in_progress_state_id: None,
            review_state_id: None,
            ready_state_id: None,
            base_branch: Some("main".into()),
            poll_interval_secs: 60,
            max_concurrent_runs: 1,
            max_attempts: 1,
            enabled: false,
            // `live` is poller-only (claim vs. dry-run) and stays off throughout:
            // delegation must resolve without it.
            live: false,
            created_at: now,
            updated_at: now,
        }
    }

    fn context(team: &str, state_id: &str, state_name: &str) -> IssueContext {
        IssueContext {
            team_id: Some(team.into()),
            state_id: Some(state_id.into()),
            state_name: Some(state_name.into()),
            delegate_id: Some("app-user-1".into()),
        }
    }

    /// The real-world shape: one team, several bindings, one per pipeline stage.
    /// The issue's status must select the workflow — matching the team alone would
    /// let only the first binding ever fire.
    #[test]
    fn status_selects_the_workflow_among_several_bindings_on_one_team() {
        // Same order `list_all` returns (project, then workflow).
        let bindings = vec![
            binding("ecom", "idea-to-pr", "todo"),
            binding("ecom", "merge-pr", "ready-for-merge"),
            binding("ecom", "revise-pr", "changes-requested"),
        ];
        let started = |state_id: &str, name: &str| match choose_target(
            &bindings,
            &context("ecom", state_id, name),
        ) {
            Target::Ready { binding } => {
                assert_eq!(binding.source_state_id, state_id);
                binding.workflow
            }
            Target::WrongStatus { current, .. } => {
                panic!("{name} should start something, got WrongStatus (current: {current})")
            }
            Target::NoBinding => panic!("{name} should match a binding"),
        };
        assert_eq!(started("todo", "Todo"), "idea-to-pr");
        assert_eq!(started("ready-for-merge", "Ready for merge"), "merge-pr");
        assert_eq!(
            started("changes-requested", "Changes requested"),
            "revise-pr"
        );
    }

    #[test]
    fn a_status_no_binding_triggers_from_is_refused_with_the_alternatives() {
        let bindings = vec![
            binding("ecom", "idea-to-pr", "todo"),
            binding("ecom", "merge-pr", "ready-for-merge"),
        ];
        match choose_target(&bindings, &context("ecom", "backlog", "Backlog")) {
            Target::WrongStatus {
                current,
                team_id,
                triggers,
            } => {
                assert_eq!(current, "Backlog");
                assert_eq!(team_id.as_deref(), Some("ecom"));
                // Every startable status is offered, not just the first.
                assert_eq!(
                    triggers,
                    vec![
                        ("todo".to_string(), "idea-to-pr".to_string()),
                        ("ready-for-merge".to_string(), "merge-pr".to_string()),
                    ]
                );
            }
            _ => panic!("expected WrongStatus"),
        }
    }

    #[test]
    fn unknown_status_or_team_is_refused_not_guessed() {
        let bindings = vec![
            binding("team-a", "idea-to-pr", "todo"),
            binding("team-b", "review-area", "queued"),
        ];
        // Startable, and the binding being disabled doesn't matter for delegation.
        assert!(matches!(
            choose_target(&bindings, &context("team-a", "todo", "To Do")),
            Target::Ready { .. }
        ));
        // A binding's status doesn't unlock a different team.
        assert!(matches!(
            choose_target(&bindings, &context("team-a", "queued", "Queued")),
            Target::WrongStatus { .. }
        ));
        // No binding covers this team.
        assert!(matches!(
            choose_target(&bindings, &context("team-z", "todo", "To Do")),
            Target::NoBinding
        ));
        // Linear didn't tell us the status → refuse rather than assume it's ready.
        let ctx = IssueContext {
            team_id: Some("team-a".into()),
            state_id: None,
            state_name: None,
            delegate_id: Some("app-user-1".into()),
        };
        match choose_target(&bindings, &ctx) {
            Target::WrongStatus { current, .. } => assert_eq!(current, "an unknown status"),
            _ => panic!("expected WrongStatus for an unknown status"),
        }
        // Nothing configured at all.
        assert!(matches!(
            choose_target(&[], &context("team-a", "todo", "To Do")),
            Target::NoBinding
        ));
    }

    /// Regression: a delegated run must be able to move the issue out of the
    /// source status. The binding's `in_progress_state_id` previously never left
    /// `choose_target`, so the delegation path had nothing to move the issue with
    /// and it sat in the source column — where the poller still considered it
    /// claimable, with only `max_concurrent_runs` preventing a second run.
    #[test]
    fn ready_carries_the_bindings_in_progress_status() {
        let bindings = vec![binding_with_status_map("ecom", "idea-to-pr", "todo")];
        match choose_target(&bindings, &context("ecom", "todo", "Todo")) {
            Target::Ready { binding } => {
                assert_eq!(binding.in_progress_state_id.as_deref(), Some("in-progress"));
                // The source status is still reported, so a failure can roll back.
                assert_eq!(binding.source_state_id, "todo");
            }
            other => panic!("expected Ready, got {}", target_name(&other)),
        }

        // A binding with no in-progress status maps to None — the move is then
        // skipped rather than guessed at.
        match choose_target(&bindings_without_map(), &context("ecom", "todo", "Todo")) {
            Target::Ready { binding } => assert_eq!(binding.in_progress_state_id, None),
            other => panic!("expected Ready, got {}", target_name(&other)),
        }
    }

    fn bindings_without_map() -> Vec<LinearSource> {
        vec![binding("ecom", "idea-to-pr", "todo")]
    }

    /// Regression: unchecking `enabled` must stop delegation routing to a
    /// binding. It previously governed only the poller, so a disabled binding
    /// still won — and with two bindings on one status the disabled one shadowed
    /// the one that was meant to run, because it sorted first.
    #[test]
    fn a_disabled_binding_is_skipped_and_cannot_shadow_an_enabled_one() {
        // `list_all` order: `idea-to-pr` sorts before `image-vision-test`.
        let bindings = vec![
            disabled_binding("ecom", "idea-to-pr", "todo"),
            binding("ecom", "image-vision-test", "todo"),
        ];
        match choose_target(&bindings, &context("ecom", "todo", "Todo")) {
            Target::Ready { binding } => assert_eq!(
                binding.workflow, "image-vision-test",
                "the enabled binding must win over a disabled one that sorts first"
            ),
            other => panic!("expected Ready, got {}", target_name(&other)),
        }

        // Disabled on its own → nothing to route to.
        assert!(matches!(
            choose_target(
                &[disabled_binding("ecom", "idea-to-pr", "todo")],
                &context("ecom", "todo", "Todo")
            ),
            Target::NoBinding
        ));
    }

    #[test]
    fn a_lone_binding_fallback_only_considers_enabled_ones() {
        // Unknown team + exactly one *enabled* binding → still used.
        let unknown_team = IssueContext {
            team_id: None,
            state_id: Some("todo".into()),
            state_name: Some("Todo".into()),
            delegate_id: None,
        };
        let bindings = vec![
            disabled_binding("ecom", "idea-to-pr", "todo"),
            binding("other", "only-live-one", "todo"),
        ];
        match choose_target(&bindings, &unknown_team) {
            Target::Ready { binding } => assert_eq!(binding.workflow, "only-live-one"),
            other => panic!("expected Ready, got {}", target_name(&other)),
        }
        // …but a lone *disabled* binding is not a fallback.
        assert!(matches!(
            choose_target(
                &[disabled_binding("ecom", "idea-to-pr", "todo")],
                &unknown_team
            ),
            Target::NoBinding
        ));
    }

    fn target_name(t: &Target) -> &'static str {
        match t {
            Target::Ready { .. } => "Ready",
            Target::NoBinding => "NoBinding",
            Target::WrongStatus { .. } => "WrongStatus",
        }
    }

    #[test]
    fn two_bindings_sharing_a_source_status_resolve_deterministically() {
        // Ambiguous configuration: first in `list_all` order wins, rather than
        // varying run to run.
        let bindings = vec![
            binding("ecom", "aaa-first", "todo"),
            binding("ecom", "zzz-second", "todo"),
        ];
        match choose_target(&bindings, &context("ecom", "todo", "Todo")) {
            Target::Ready { binding } => assert_eq!(binding.workflow, "aaa-first"),
            _ => panic!("expected Ready"),
        }
    }

    #[test]
    fn a_sole_binding_covers_an_unknown_team_but_still_gates_on_status() {
        let bindings = vec![binding("team-a", "idea-to-pr", "todo")];
        let unknown_team = IssueContext {
            team_id: None,
            state_id: Some("todo".into()),
            state_name: Some("To Do".into()),
            delegate_id: None,
        };
        assert!(matches!(
            choose_target(&bindings, &unknown_team),
            Target::Ready { .. }
        ));
        // …but the status gate still applies to the fallback.
        let wrong = IssueContext {
            team_id: None,
            state_id: Some("done".into()),
            state_name: Some("Done".into()),
            delegate_id: None,
        };
        assert!(matches!(
            choose_target(&bindings, &wrong),
            Target::WrongStatus { .. }
        ));
    }

    #[test]
    fn session_guard_admits_one_holder_and_releases_on_drop() {
        let session = "sess-guard-1";
        let first = SessionGuard::acquire(session).expect("first acquire succeeds");
        // A redelivery arriving while the first is still working is turned away —
        // this is what stops one delegation becoming two runs.
        assert!(
            SessionGuard::acquire(session).is_none(),
            "a concurrent redelivery must not be admitted"
        );
        // A different session is unaffected.
        let other = SessionGuard::acquire("sess-guard-2").expect("unrelated session admitted");
        drop(other);
        drop(first);
        // Released, so a genuine later delegation of the same session can proceed
        // (the database check is what stops a *retry* at that point).
        assert!(SessionGuard::acquire(session).is_some());
    }

    #[test]
    fn session_guard_releases_even_when_the_holder_panics() {
        let session = "sess-guard-panic";
        let result = std::panic::catch_unwind(|| {
            let _g = SessionGuard::acquire(session).expect("acquired");
            panic!("handler blew up");
        });
        assert!(result.is_err());
        assert!(
            SessionGuard::acquire(session).is_some(),
            "a panicking handler must not wedge the session forever"
        );
    }

    /// The cap is a limit, not a threshold: at exactly `max_concurrent_runs`
    /// there is no room left. Off-by-one here would let a 1-task binding run two.
    #[test]
    fn capacity_is_reached_at_the_limit_not_past_it() {
        assert!(!at_capacity(0, 1));
        assert!(at_capacity(1, 1));
        assert!(at_capacity(2, 1));
        assert!(!at_capacity(2, 3));
        assert!(at_capacity(3, 3));
    }

    /// A binding stored with 0 (or a negative, from a hand-edited row) still allows
    /// one run — the same `.max(1)` floor the poller applies, so neither path can
    /// silently stop working.
    #[test]
    fn a_zero_limit_still_allows_one_run() {
        assert!(!at_capacity(0, 0));
        assert!(at_capacity(1, 0));
        assert!(!at_capacity(0, -5));
        assert!(at_capacity(1, -5));
    }

    /// Only a live binding has a poller that comes back for the issue, so only a
    /// live binding may promise an automatic start.
    #[test]
    fn a_live_binding_promises_pickup_and_a_dry_one_does_not() {
        let mut b = binding("ecom", "idea-to-pr", "todo");
        b.live = true;
        let live = at_capacity_message(&b);
        assert!(live.contains("Another task is already running"), "{live}");
        assert!(live.contains("automatically"), "{live}");
        assert!(!live.contains("delegate it again"), "{live}");

        b.live = false;
        let dry = at_capacity_message(&b);
        assert!(dry.contains("delegate it again"), "{dry}");
        assert!(
            !dry.contains("automatically"),
            "a dry binding has no poller to keep that promise: {dry}"
        );
    }

    /// Both wordings name the workflow and read as information, not failure.
    #[test]
    fn the_capacity_message_never_reads_as_an_error() {
        let mut b = binding("ecom", "idea-to-pr", "todo");
        b.max_concurrent_runs = 3;
        for live in [true, false] {
            b.live = live;
            let msg = at_capacity_message(&b);
            assert!(msg.contains("3 tasks are already running"), "{msg}");
            assert!(msg.contains("idea-to-pr"), "{msg}");
            for word in ["error", "fail", "cannot", "Unable"] {
                assert!(
                    !msg.to_lowercase().contains(&word.to_lowercase()),
                    "capacity is not a failure, but the message says {word:?}: {msg}"
                );
            }
        }
    }

    fn run_detail_for_answering() -> harness_persist::RunDetail {
        let node =
            |id: &str, status: &str, artifact: Option<&str>| harness_persist::PersistedNode {
                node_id: id.into(),
                ordinal: 0,
                status: status.into(),
                provider: None,
                model: None,
                output: String::new(),
                iterations: 1,
                converged: None,
                note: None,
                input_tokens: None,
                output_tokens: None,
                cache_read: None,
                cache_write: None,
                started_at: None,
                ended_at: None,
                artifact_content: artifact.map(str::to_string),
            };
        let meta = |id: &str| harness_dag::NodeMeta {
            id: id.into(),
            depends_on: vec![],
            category: None,
            artifact: None,
        };
        harness_persist::RunDetail {
            run: harness_persist::RunSummary {
                id: "r1".into(),
                workflow_name: "idea-to-pr".into(),
                title: None,
                description: Some(String::new()),
                status: "running".into(),
                node_count: 3,
                started_at: None,
                ended_at: None,
                recorded_at: chrono::Utc::now(),
                project: Some("dilling-ecom".into()),
                ab_pair_id: None,
                ab_arm: None,
                ab_label: None,
            },
            nodes: vec![
                node(
                    "explore",
                    "success",
                    Some("# Exploration\n\nRoot cause is in RangeFacet."),
                ),
                node("create-plan", "running", None),
            ],
            graph: ["explore", "create-plan", "install-deps"]
                .into_iter()
                .map(meta)
                .collect(),
        }
    }

    /// The whole point of answering from Postgres: the prompt must carry the run's
    /// state and artifacts, because the agent is told not to read anything.
    #[test]
    fn the_answer_prompt_carries_the_run_state_and_artifacts() {
        let p = session_answer_prompt("which file is the bug in?", &run_detail_for_answering());
        assert!(p.contains("which file is the bug in?"), "{p}");
        // Workflow, status and the declared-step count come from the run.
        assert!(p.contains("`idea-to-pr`"), "{p}");
        assert!(p.contains("steps declared: 3"), "{p}");
        // Per-step status, so "what are you doing" is answerable.
        assert!(p.contains("`explore` — success"), "{p}");
        assert!(p.contains("`create-plan` — running"), "{p}");
        // The artifact body, which is what a question about the work needs.
        assert!(p.contains("Root cause is in RangeFacet."), "{p}");
        // And the standing instructions that keep it read-only and on-format.
        assert!(p.contains("do NOT read files"), "{p}");
        assert!(p.contains(CANNOT_STEER_REPLY), "{p}");
    }

    /// An empty prompt body is the "@mention with no text" case; it must still ask
    /// something answerable rather than sending the model a blank question.
    #[test]
    fn an_empty_message_is_read_as_asking_what_is_happening() {
        let p = session_answer_prompt("   ", &run_detail_for_answering());
        assert!(p.contains("what is happening?"), "{p}");
    }

    /// `plan.md` runs to thousands of words; the prompt has to stay bounded, and a
    /// cut must be visible rather than silently truncating mid-sentence.
    #[test]
    fn a_long_artifact_is_excerpted_and_says_so() {
        assert_eq!(excerpt("short", 10), "short");
        // Exactly at the limit is not truncated.
        assert_eq!(excerpt("0123456789", 10), "0123456789");
        let long = "x".repeat(ARTIFACT_EXCERPT_CHARS + 500);
        let cut = excerpt(&long, ARTIFACT_EXCERPT_CHARS);
        assert!(cut.ends_with("…(truncated)"), "no marker: {}", &cut[..40]);
        assert_eq!(
            cut.chars().count(),
            ARTIFACT_EXCERPT_CHARS + "\n\n…(truncated)".chars().count()
        );
        // Multi-byte input must not panic on a byte boundary.
        let danish = "æøå".repeat(20);
        assert!(excerpt(&danish, 5).starts_with("æøå"));
    }

    #[test]
    fn task_text_survives_a_bare_session() {
        let json = br#"{"type":"AgentSessionEvent","action":"created",
            "agentSession":{"id":"s"}}"#;
        let ev = parse_agent_session_event(json).unwrap().unwrap();
        // Nothing to say, but it must not panic or invent content.
        assert_eq!(task_text(&ev), "");
    }
}
