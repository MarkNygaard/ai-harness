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
struct SessionGuard(String);

impl SessionGuard {
    /// `None` when another task is already handling this session.
    fn acquire(session_id: &str) -> Option<Self> {
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
    let (project, workflow, base_branch, source_state_id) = match resolve_target(state, &event)
        .await
    {
        Target::Ready {
            project,
            workflow,
            base_branch,
            source_state_id,
        } => (project, workflow, base_branch, source_state_id),
        Target::NoBinding => {
            let _ = client
                .create_agent_activity(
                    &session,
                    &AgentActivity::Error {
                        body: "No harness project is set up for this issue's team. Add a Linear \
                               trigger binding for the project on the Projects page, then \
                               delegate again."
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
    let req = CreateRunRequest {
        workflow: workflow.clone(),
        title,
        description,
        args: String::new(),
        real: true,
        base_branch: base_branch.clone(),
        project: Some(project.clone()),
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
                        &project,
                        &workflow,
                        event.issue_id.as_deref().unwrap_or_default(),
                        event.issue_identifier.as_deref().unwrap_or("(delegated)"),
                        &source_state_id,
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
                        parameter: workflow.clone(),
                        result: run_link.clone(),
                    },
                )
                .await;
            // Attach the run link to the issue as well, matching the poller.
            if let (Some(issue_id), Some(url)) = (event.issue_id.as_deref(), run_link.as_deref()) {
                let _ = client.add_attachment(issue_id, url, "ai-harness run").await;
            }
            tracing::info!(
                "linear webhook: session {session} → run {run_id} (`{workflow}` in `{project}`)"
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
                        body: format!("Could not start the `{workflow}` workflow: {message}"),
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
async fn handle_prompted(state: &Arc<RunsState>, event: AgentSessionEvent) {
    let Ok(client) = linear_client(state).await else {
        return;
    };
    let body = "Noted — but I can't change course mid-run yet. Let this run finish (or cancel it \
                in the harness), then delegate again with the updated ask."
        .to_string();
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
    Ready {
        project: String,
        workflow: String,
        base_branch: Option<String>,
        source_state_id: String,
    },
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
fn choose_target(bindings: &[LinearSource], context: &IssueContext) -> Target {
    // Candidates: every binding for the issue's team. With an unknown team, a
    // lone binding is assumed to be the intended one.
    let candidates: Vec<&LinearSource> = match context.team_id.as_deref() {
        Some(team) => bindings.iter().filter(|b| b.team_id == team).collect(),
        None => Vec::new(),
    };
    let candidates = if !candidates.is_empty() {
        candidates
    } else if bindings.len() == 1 {
        vec![&bindings[0]]
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

    Target::Ready {
        project: chosen.project.clone(),
        // A binding names the workflow it fires; fall back to the conventional
        // default only if the row somehow carries none.
        workflow: if chosen.workflow.is_empty() {
            DEFAULT_WORKFLOW.to_string()
        } else {
            chosen.workflow.clone()
        },
        base_branch: chosen.base_branch.clone(),
        source_state_id: chosen.source_state_id.clone(),
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

    fn binding(team: &str, workflow: &str, source_state: &str) -> LinearSource {
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
            // Deliberately off: `enabled`/`live` gate only the column poller, so
            // delegation must still resolve through a disabled binding.
            enabled: false,
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
            Target::Ready {
                workflow,
                source_state_id,
                ..
            } => {
                assert_eq!(source_state_id, state_id);
                workflow
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

    #[test]
    fn two_bindings_sharing_a_source_status_resolve_deterministically() {
        // Ambiguous configuration: first in `list_all` order wins, rather than
        // varying run to run.
        let bindings = vec![
            binding("ecom", "aaa-first", "todo"),
            binding("ecom", "zzz-second", "todo"),
        ];
        match choose_target(&bindings, &context("ecom", "todo", "Todo")) {
            Target::Ready { workflow, .. } => assert_eq!(workflow, "aaa-first"),
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

    #[test]
    fn task_text_survives_a_bare_session() {
        let json = br#"{"type":"AgentSessionEvent","action":"created",
            "agentSession":{"id":"s"}}"#;
        let ev = parse_agent_session_event(json).unwrap().unwrap();
        // Nothing to say, but it must not panic or invent content.
        assert_eq!(task_text(&ev), "");
    }
}
