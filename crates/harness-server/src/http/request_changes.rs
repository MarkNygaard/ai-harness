//! "Request these changes": how the chat hands agreed changes to an issue's
//! existing pull request over to the team's revise path.
//!
//! When someone agrees a change with the agent in a Linear thread, the work
//! is a revision of the PR the issue already has — not a fresh build. The team
//! wired that path on its board: a comment, then the issue moved to the column
//! a trigger revises from (e.g. "Changes requested" → `revise-task`). This
//! does exactly that, after the person confirms: it posts the agreed changes
//! as a comment credited to them, makes sure the issue is delegated (the
//! trigger selects delegated issues), and moves it. The trigger then starts
//! the revision with its normal Linear lifecycle.
//!
//! The comment is written by the harness's account, so it carries
//! [`REQUESTED_BY_MARKER`]: the poller then hands it to the revision as the
//! person's feedback rather than as one of the harness's own notes (see
//! `linear_poller::task_for_issue`).

use std::sync::Arc;

use harness_persist::RunTrigger;
use harness_sources::linear::Team;
use serde_json::Value;

use super::runs_routes::RunsState;

/// Opens the hidden line naming who asked: `<!-- harness:requested-by: Name -->`.
pub(crate) const REQUESTED_BY_MARKER: &str = "<!-- harness:requested-by:";

/// The person's display name from an actor label (`Name <email>`): the email
/// stays out of the comment.
pub(crate) fn display_name(actor: &str) -> String {
    let name = actor.split(" <").next().unwrap_or_default().trim();
    if name.is_empty() {
        "Someone".to_string()
    } else {
        name.to_string()
    }
}

/// The comment that carries the agreed changes to the revision.
pub(crate) fn comment_body(requester: &str, changes: &str) -> String {
    // A name can't close the HTML comment early.
    let safe = requester.replace("--", "-").replace('>', "");
    format!(
        "**{requester} asked for these changes**, agreed with the AI Harness agent in this thread:\n\n\
         {changes}\n\n{REQUESTED_BY_MARKER} {safe} -->",
        changes = changes.trim()
    )
}

/// Who a harness comment speaks for, when it carries the marker: the name,
/// and the body without the marker line.
pub(crate) fn requested_by(body: &str) -> Option<(String, String)> {
    let start = body.find(REQUESTED_BY_MARKER)?;
    let rest = &body[start + REQUESTED_BY_MARKER.len()..];
    let end = rest.find("-->")?;
    let name = rest[..end].trim();
    if name.is_empty() {
        return None;
    }
    let mut cleaned = String::new();
    cleaned.push_str(body[..start].trim_end());
    cleaned.push_str(rest[end + 3..].trim_end());
    Some((name.to_string(), cleaned))
}

/// The enabled Linear trigger in `project` that fires when an issue of `team`
/// enters `state_id`: what makes moving the issue there start the revision.
pub(crate) fn watching_trigger<'a>(
    triggers: &'a [RunTrigger],
    project: &str,
    team_id: &str,
    state_id: &str,
) -> Option<&'a RunTrigger> {
    triggers.iter().find(|t| {
        let c = &t.config.0;
        t.enabled
            && t.kind == harness_persist::KIND_LINEAR
            && t.project == project
            && c.get("team_id").and_then(Value::as_str) == Some(team_id)
            && c.get("state_id").and_then(Value::as_str) == Some(state_id)
    })
}

/// The status a name (or id) names in the team, ignoring case.
fn find_state<'a>(
    team: &'a Team,
    wanted: &str,
) -> Option<&'a harness_sources::linear::WorkflowState> {
    let w = wanted.trim();
    team.states
        .iter()
        .find(|s| s.id == w || s.name.eq_ignore_ascii_case(w))
}

/// Post the agreed changes on `issue` as `actor`'s request and move it to
/// `state`, where a trigger starts the revision. Returns a one-line summary.
pub(crate) async fn request_changes(
    state: &Arc<RunsState>,
    actor: &str,
    project: &str,
    issue: &str,
    target: &str,
    changes: &str,
) -> Result<String, String> {
    if changes.trim().is_empty() {
        return Err("`changes` is empty: say what should change".into());
    }
    let conn = super::linear_connections::resolve_for_project(state, project).await?;
    let client = super::linear_oauth::linear_client(state, &conn).await?;
    let found = client.issue_labels(issue).await.map_err(|e| e.0)?;
    let discovery = client.discover().await.map_err(|e| e.0)?;
    let team = found
        .team_id
        .as_deref()
        .and_then(|id| discovery.teams.iter().find(|t| t.id == id))
        .ok_or_else(|| format!("could not read {}'s team", found.identifier))?;
    let Some(to) = find_state(team, target) else {
        let known: Vec<&str> = team.states.iter().map(|s| s.name.as_str()).collect();
        return Err(format!(
            "no status `{target}` in team {}; statuses: {}",
            team.key,
            known.join(", ")
        ));
    };

    // Refuse a column nothing watches: the issue would be parked there with
    // nobody coming for it.
    let triggers = state
        .trigger_store()
        .await?
        .list()
        .await
        .map_err(|e| e.to_string())?;
    let Some(trigger) = watching_trigger(&triggers, project, &team.id, &to.id) else {
        let watched: Vec<String> = triggers
            .iter()
            .filter(|t| {
                t.enabled
                    && t.kind == harness_persist::KIND_LINEAR
                    && t.project == project
                    && t.config.0.get("team_id").and_then(Value::as_str) == Some(team.id.as_str())
            })
            .map(|t| {
                let name = t
                    .config
                    .0
                    .get("state_name")
                    .and_then(Value::as_str)
                    .unwrap_or("?");
                format!("{name} → {}", t.workflow)
            })
            .collect();
        return Err(format!(
            "no enabled trigger in {project} starts a run when a {} issue enters `{}`, so \
             moving {} there would start nothing. Triggers for this team: {}",
            team.key,
            to.name,
            found.identifier,
            if watched.is_empty() {
                "none".into()
            } else {
                watched.join("; ")
            }
        ));
    };

    let requester = display_name(actor);
    client
        .add_comment(&found.id, &comment_body(&requester, changes))
        .await
        .map_err(|e| format!("could not comment on {}: {}", found.identifier, e.0))?;
    // The trigger selects issues delegated to the harness.
    if let Some(app) = super::linear_oauth::app_user_id(state, &conn).await {
        if let Err(e) = client.set_delegate(&found.id, &app).await {
            tracing::warn!(
                "request changes: could not delegate {}: {}",
                found.identifier,
                e.0
            );
        }
    }
    client
        .set_issue_state(&found.id, &to.id)
        .await
        .map_err(|e| {
            format!(
                "commented, but could not move {}: {}",
                found.identifier, e.0
            )
        })?;
    Ok(format!(
        "Posted {requester}'s changes on {} and moved it to {}; the `{}` trigger starts {} \
         from there.",
        found.identifier, to.name, trigger.name, trigger.workflow
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn the_comment_names_the_person_and_hands_back_as_theirs() {
        let name = display_name("Ada Lindqvist <ada@example.com>");
        assert_eq!(name, "Ada Lindqvist", "no email in a comment");
        assert_eq!(display_name(""), "Someone");

        let body = comment_body(&name, "Rewrite the Campaign orders bullet.\n");
        assert!(body.starts_with("**Ada Lindqvist asked for these changes**"));
        assert!(!body.contains("ada@"));

        let (who, text) = requested_by(&body).unwrap();
        assert_eq!(who, "Ada Lindqvist");
        assert!(text.contains("Rewrite the Campaign orders bullet."));
        assert!(!text.contains("harness:requested-by"), "{text}");

        // An ordinary harness note speaks for nobody.
        assert_eq!(requested_by("Working on the requested changes."), None);
        // A name can't end the hidden line early.
        let tricky = comment_body("A --> B", "x");
        assert_eq!(requested_by(&tricky).unwrap().0, "A - B");
    }

    fn trigger(project: &str, team: &str, state: &str, enabled: bool) -> RunTrigger {
        RunTrigger {
            id: format!("{team}-{state}"),
            name: format!("{team} · {state}"),
            kind: harness_persist::KIND_LINEAR.into(),
            project: project.into(),
            workflow: "revise-task".into(),
            enabled,
            config: sqlx::types::Json(
                json!({ "team_id": team, "state_id": state, "state_name": state }),
            ),
            last_fired_at: None,
            last_run_id: None,
            last_error: None,
            created_by: None,
            created_at: chrono::Utc::now(),
            updated_at: chrono::Utc::now(),
            updated_by: None,
        }
    }

    #[test]
    fn only_an_enabled_trigger_on_that_team_and_column_counts() {
        let ts = vec![
            trigger("dilling-ecom", "ecom", "changes", false),
            trigger("other", "ecom", "changes", true),
            trigger("dilling-ecom", "erp", "changes", true),
            trigger("dilling-ecom", "ecom", "changes", true),
        ];
        let hit = watching_trigger(&ts, "dilling-ecom", "ecom", "changes").unwrap();
        assert!(hit.enabled && hit.project == "dilling-ecom");
        assert!(watching_trigger(&ts, "dilling-ecom", "ecom", "todo").is_none());
    }
}
