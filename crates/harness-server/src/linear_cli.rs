//! **What a workflow run may do to Linear**, exposed as `harness linear …`.
//!
//! The epic orchestrator has to file sub-issues, move them between columns and
//! keep a ledger comment on the epic — from inside a run, which is a DAG of
//! prompts and shell steps, not Rust.
//!
//! **A run never holds a credential.** It holds a *grant*: a token bound to one
//! run and one project, handed to it in `HARNESS_RUN_TOKEN`. This asks the
//! server to act, and the server — which does hold the credentials — decides
//! whether to. Two things follow, and both are the point:
//!
//!   * **the blast radius is a project.** A grant recovered from a log buys the
//!     five operations below on one project until it expires. The database URL
//!     and encryption key it replaced would have bought every credential the
//!     install holds — which is why `strip_control_plane_env` removes those at
//!     every spawn point, and why putting them back was the wrong fix;
//!   * **attribution.** The server writes as the `actor=app` install, so every
//!     sub-issue and ledger entry is authored by the application rather than by
//!     whoever's key was to hand.
//!
//! The project is **not** sent: the server reads it from the grant. A run is
//! never asked which project it is, so it cannot answer wrongly.

use serde::Serialize;

/// Where to ask, and with what.
#[derive(Debug)]
struct Grant {
    base: String,
    token: String,
}

/// Read the grant out of the environment.
fn grant() -> Result<Grant, String> {
    from_parts(
        std::env::var("HARNESS_RUN_URL").ok(),
        std::env::var("HARNESS_RUN_TOKEN").ok(),
    )
}

/// Build a grant from what the environment offered.
///
/// Split from the environment read so it is testable without mutating process
/// state. Both variables are set by the server when it starts a run; their
/// absence means this is not running inside one, which is worth saying plainly
/// because the command is otherwise indistinguishable from a mistyped one — and
/// the message is read in a run log by somebody who did not write the workflow.
fn from_parts(base: Option<String>, token: Option<String>) -> Result<Grant, String> {
    let base = base.map(|b| b.trim().to_string()).filter(|b| !b.is_empty()).ok_or(
        "HARNESS_RUN_URL is not set — `harness linear` runs inside a workflow run, and needs the harness's public URL (Settings -> General)",
    )?;
    let token = token
        .map(|t| t.trim().to_string())
        .filter(|t| !t.is_empty())
        .ok_or("HARNESS_RUN_TOKEN is not set — this is not a workflow run")?;
    Ok(Grant {
        // Trailing slash trimmed here rather than at every call site: the public
        // URL is typed by a person, and half of them end it with one.
        base: base.trim_end_matches('/').to_string(),
        token,
    })
}

/// POST one request, returning the body as text.
///
/// The server's error message is surfaced verbatim: it is written for whoever
/// is reading a failed run's log, and that is the only place it will be seen.
async fn ask<T: Serialize>(path: &str, body: &T) -> Result<String, String> {
    let g = grant()?;
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(30))
        .build()
        .map_err(|e| format!("http client: {e}"))?;
    let resp = client
        .post(format!("{}{path}", g.base))
        .bearer_auth(&g.token)
        .json(body)
        .send()
        .await
        .map_err(|e| format!("could not reach the harness at {}: {e}", g.base))?;

    let status = resp.status();
    let text = resp
        .text()
        .await
        .map_err(|e| format!("could not read the response: {e}"))?;
    if status.is_success() {
        return Ok(text);
    }
    let detail = serde_json::from_str::<serde_json::Value>(&text)
        .ok()
        .and_then(|v| v.get("error").and_then(|e| e.as_str()).map(str::to_string))
        .unwrap_or(text);
    Err(format!("{status}: {detail}"))
}

/// File a sub-issue under `parent`, in the parent's team.
pub async fn create_sub_issue(
    parent_id: &str,
    title: &str,
    description: &str,
    state_id: Option<&str>,
    label_ids: &[String],
) -> Result<String, String> {
    ask(
        "/api/run/linear/sub-issue",
        &serde_json::json!({
            "parent": parent_id,
            "title": title,
            "description": description,
            "state": state_id,
            "labels": label_ids,
        }),
    )
    .await
}

/// Move an issue to a workflow state — how a piece advances a column.
pub async fn move_state(issue_id: &str, state_id: &str) -> Result<String, String> {
    ask(
        "/api/run/linear/state",
        &serde_json::json!({ "issue": issue_id, "state": state_id }),
    )
    .await
}

/// Append to an issue — the epic ledger is comments on the epic.
pub async fn comment(issue_id: &str, body_md: &str) -> Result<String, String> {
    if body_md.trim().is_empty() {
        return Err("refusing to post an empty comment".to_string());
    }
    ask(
        "/api/run/linear/comment",
        &serde_json::json!({ "issue": issue_id, "body": body_md }),
    )
    .await
}

/// Where a finished epic goes once its pull request is open, from the binding
/// that started this run. `{"state": null}` when the binding says nothing,
/// which means leave the epic where it is.
pub async fn epic_review_state() -> Result<String, String> {
    ask("/api/run/linear/epic-review-state", &serde_json::json!({})).await
}

/// The column a piece must be moved to in order to be built.
///
/// Derived from the claims the poller already writes, so an epic needs no state
/// id pasted into a project environment variable. `issue` names the piece whose
/// build column to read; without it the answer comes from the asking run's own
/// claim.
pub async fn ready_state(issue_id: Option<&str>) -> Result<String, String> {
    ask(
        "/api/run/linear/ready-state",
        &serde_json::json!({ "issue": issue_id }),
    )
    .await
}

/// Give up an issue — clear its delegate so the poller stops offering it.
///
/// What a workflow calls when it is done with an issue it left where it found
/// it. Leaving the column is the poller's normal "finished" signal; a workflow
/// that deliberately does not move the issue has to say so some other way, or
/// it is handed the same issue every tick.
pub async fn release(issue_id: &str) -> Result<String, String> {
    ask(
        "/api/run/linear/release",
        &serde_json::json!({ "issue": issue_id }),
    )
    .await
}

/// One issue's context as JSON: identifier, team, state, parent, labels.
pub async fn issue(issue_id: &str) -> Result<String, String> {
    ask(
        "/api/run/linear/issue",
        &serde_json::json!({ "issue": issue_id }),
    )
    .await
}

/// An epic's sub-issues as JSON, in board order.
pub async fn children(issue_id: &str) -> Result<String, String> {
    ask(
        "/api/run/linear/children",
        &serde_json::json!({ "issue": issue_id }),
    )
    .await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn an_empty_ledger_entry_is_refused_before_anything_is_reached() {
        // Ahead of the network: a step that produced nothing must fail loudly
        // rather than post a blank comment on an epic, and it must not need a
        // server to say so.
        for blank in ["", "   ", "\n\t "] {
            let err = comment("issue", blank).await.unwrap_err();
            assert!(err.contains("empty"), "{err}");
        }
    }

    #[test]
    fn a_missing_grant_says_this_is_not_a_run() {
        let err = from_parts(None, None).unwrap_err();
        assert!(err.contains("HARNESS_RUN_URL"), "{err}");
        let err = from_parts(Some("https://h.test".into()), None).unwrap_err();
        assert!(err.contains("HARNESS_RUN_TOKEN"), "{err}");
    }

    #[test]
    fn an_empty_variable_is_the_same_as_an_absent_one() {
        // A container that sets these to "" is a configuration mistake, not a
        // grant, and must not produce a request to `https:///…`.
        assert!(from_parts(Some("  ".into()), Some("t".into())).is_err());
        assert!(from_parts(Some("https://h.test".into()), Some("".into())).is_err());
    }

    #[test]
    fn a_trailing_slash_does_not_double_up() {
        let g = from_parts(
            Some("https://harness.example/".into()),
            Some("hrn_run_abc".into()),
        )
        .unwrap();
        assert_eq!(g.base, "https://harness.example");
    }
}
