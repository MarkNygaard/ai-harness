//! **What a workflow run may do to Linear**, exposed as `harness linear …`.
//!
//! The epic orchestrator has to file sub-issues, move them between columns and
//! keep a ledger comment on the epic — from inside a run, which is a DAG of
//! prompts and shell steps, not Rust.
//!
//! It goes through the harness rather than around it. A run never holds a
//! Linear credential: it names a **project**, and this resolves that project to
//! its connection and uses the same `actor=app` token the poller does. Two
//! things follow from that, and both are the point:
//!
//!   * **attribution** — every sub-issue and every ledger entry is authored by
//!     the application, not by whoever's key happened to be lying around. An
//!     orchestrator that writes for days must not read as a colleague;
//!   * **one trusted path** — token refresh, connection resolution and the
//!     app-actor rule are decided in exactly one place, which is already
//!     written and already tested.
//!
//! Reachable because runs execute in this container, where `/usr/local/bin/harness`
//! is on `PATH` and the database is the one the server is using. That is why
//! this is a subcommand rather than an HTTP endpoint: an endpoint would need a
//! run-scoped credential invented, scoped, expired and revoked, to reach a
//! process that is already inside the trust boundary.
//!
//! Deliberately narrow — file, move, comment, read. There is no delete, and no
//! way to reach an issue outside the project's own workspace.

use harness_persist::{CredentialStore, ProjectStore};
use harness_sources::linear::{LinearClient, SubIssue};

use crate::http::linear_connections::resolve_with_stores;
use crate::http::linear_oauth::client_for_store;

/// Everything each command needs to reach Linear as the right workspace.
struct Resolved {
    client: LinearClient,
}

/// Connect the stores, resolve the project's workspace, build a client.
///
/// Errors are written for whoever is reading a failed run's log, since that is
/// the only place they will be seen.
async fn resolve(
    database_url: &str,
    secret_key_b64: &str,
    project: &str,
) -> Result<Resolved, String> {
    let key = CredentialStore::key_from_base64(secret_key_b64)
        .map_err(|e| format!("HARNESS_SECRET_KEY: {e}"))?;
    let creds = CredentialStore::connect(database_url, key)
        .await
        .map_err(|e| format!("could not reach the credential store: {e}"))?;
    let projects = ProjectStore::connect(database_url)
        .await
        .map_err(|e| format!("could not reach the database: {e}"))?;
    let conn = resolve_with_stores(&creds, &projects, project).await?;
    let client = client_for_store(&creds, &conn).await?;
    Ok(Resolved { client })
}

/// File a sub-issue under `parent`, in the parent's team.
///
/// The team is read from the parent rather than passed in: a sub-issue of an
/// epic belongs to that epic's team by definition, and letting a run name a
/// team would let a typo scatter an epic across a workspace.
pub async fn create_sub_issue(
    database_url: &str,
    secret_key_b64: &str,
    project: &str,
    parent_id: &str,
    title: &str,
    description: &str,
    state_id: Option<&str>,
    label_ids: &[String],
) -> Result<String, String> {
    let r = resolve(database_url, secret_key_b64, project).await?;
    let team_id = r
        .client
        .issue_context(parent_id)
        .await
        .map_err(|e| format!("could not read the parent issue: {e}"))?
        .team_id
        .ok_or("that parent issue does not resolve — check the id")?;
    let issue = r
        .client
        .create_issue(
            &team_id,
            title,
            description,
            state_id,
            label_ids,
            Some(parent_id),
        )
        .await
        .map_err(|e| format!("could not file the sub-issue: {e}"))?;
    Ok(format!("{} {}", issue.identifier, issue.url))
}

/// Move an issue to a workflow state — how a piece advances a column.
pub async fn move_state(
    database_url: &str,
    secret_key_b64: &str,
    project: &str,
    issue_id: &str,
    state_id: &str,
) -> Result<String, String> {
    let r = resolve(database_url, secret_key_b64, project).await?;
    r.client
        .set_issue_state(issue_id, state_id)
        .await
        .map_err(|e| format!("could not move the issue: {e}"))?;
    Ok(format!("moved {issue_id}"))
}

/// Append to an issue — the epic ledger is comments on the epic.
pub async fn comment(
    database_url: &str,
    secret_key_b64: &str,
    project: &str,
    issue_id: &str,
    body_md: &str,
) -> Result<String, String> {
    if body_md.trim().is_empty() {
        return Err("refusing to post an empty comment".to_string());
    }
    let r = resolve(database_url, secret_key_b64, project).await?;
    r.client
        .add_comment(issue_id, body_md)
        .await
        .map_err(|e| format!("could not comment: {e}"))?;
    Ok(format!("commented on {issue_id}"))
}

/// An epic's sub-issues as JSON on stdout, in board order.
///
/// JSON because the caller is a workflow step that has to branch on it — which
/// piece is next, which are done — and a `jq` expression against a stable shape
/// beats parsing prose.
pub async fn children(
    database_url: &str,
    secret_key_b64: &str,
    project: &str,
    issue_id: &str,
) -> Result<String, String> {
    let r = resolve(database_url, secret_key_b64, project).await?;
    let kids: Vec<SubIssue> = r
        .client
        .list_children(issue_id)
        .await
        .map_err(|e| format!("could not read the epic's sub-issues: {e}"))?;
    serde_json::to_string_pretty(&kids).map_err(|e| format!("could not encode the result: {e}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn an_empty_ledger_entry_is_refused_before_anything_is_reached() {
        // The guard is deliberately ahead of `resolve`: a workflow step that
        // produced nothing must fail loudly rather than post a blank comment on
        // an epic, and it must not need a database to say so.
        for blank in ["", "   ", "\n\t "] {
            let err = comment("postgres://unused", "unused", "p", "issue", blank)
                .await
                .unwrap_err();
            assert!(err.contains("empty"), "{err}");
        }
    }
}
