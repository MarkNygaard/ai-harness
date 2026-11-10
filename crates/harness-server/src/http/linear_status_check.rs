//! **Does the issue's team have every status the workflow moves it to?**
//!
//! A `linear:` step names its status (`move_state: In Review`), and the name is
//! resolved against the issue's team only when that step runs. A team without
//! that status fails the step then, which for `In Review` or `Done` is at the
//! end of the run: the pull request is open, the tokens are spent, and the
//! issue is left in In Progress.
//!
//! So the same comparison is made up front, in three places that share this
//! module: before a run starts ([`for_issue`], which refuses the run), on the
//! Triggers page ([`for_team`], a warning on each affected trigger), and in the
//! wiring check that `linear_check` and the project's Linear tab read.
//!
//! Each check fails open. A Linear outage, a missing connection or an
//! unreadable workflow says nothing here: the step itself still reports a
//! missing status when it runs, so this only ever brings that failure forward.

use std::sync::Arc;

use harness_dag::Workflow;
use harness_sources::linear::{LinearClient, Team};

use super::run_linear_routes::matches;
use super::runs_routes::RunsState;

/// The statuses in `targets` that `team` does not have, matched the way the
/// move step matches them: by id, or by name ignoring case.
pub(crate) fn missing_statuses<'a>(targets: &[&'a str], team: &Team) -> Vec<&'a str> {
    targets
        .iter()
        .copied()
        .filter(|t| !team.states.iter().any(|s| matches(t, &s.id, &[&s.name])))
        .collect()
}

/// What to tell a person, or `None` when the team has everything.
pub(crate) fn problem(workflow: &Workflow, team: &Team) -> Option<String> {
    let missing = missing_statuses(&workflow.linear_target_states(), team);
    if missing.is_empty() {
        return None;
    }
    let names = |list: &[&str]| -> String {
        let quoted: Vec<String> = list.iter().map(|s| format!("`{s}`")).collect();
        match quoted.as_slice() {
            [] => String::new(),
            [one] => one.clone(),
            [rest @ .., last] => format!("{} and {last}", rest.join(", ")),
        }
    };
    let has: Vec<&str> = team.states.iter().map(|s| s.name.as_str()).collect();
    let (it, them) = if missing.len() == 1 {
        ("that status", "it")
    } else {
        ("those statuses", "them")
    };
    Some(format!(
        "`{wf}` moves issues to {missing}, which team {key} ({team}) does not have. \
         Its statuses are {has}. Rename {it} in the workflow, or add {them} to the team.",
        wf = workflow.name,
        missing = names(&missing),
        key = team.key,
        team = team.name,
        has = has.join(", "),
    ))
}

/// A Linear client for the project's connection, or `None` with the reason
/// logged. Fails open, like everything here.
async fn client(state: &Arc<RunsState>, project: &str) -> Option<LinearClient> {
    let conn = match super::linear_connections::resolve_for_project(state, project).await {
        Ok(c) => c,
        Err(e) => {
            tracing::debug!("linear status check: no connection for {project}: {e}");
            return None;
        }
    };
    match super::linear_oauth::linear_client(state, &conn).await {
        Ok(c) => Some(c),
        Err(e) => {
            tracing::debug!("linear status check: no client for {project}: {e}");
            None
        }
    }
}

/// Before a run on `issue` starts: the reason to refuse it, if its team lacks
/// a status the workflow moves to. One issue read and one discovery read, and
/// none at all for a workflow with no `move_state` step.
pub(crate) async fn for_issue(
    state: &Arc<RunsState>,
    project: &str,
    issue: &str,
    workflow: &Workflow,
) -> Option<String> {
    if workflow.linear_target_states().is_empty() {
        return None;
    }
    let c = client(state, project).await?;
    let team_id = match c.issue_labels(issue).await {
        Ok(i) => i.team_id?,
        Err(e) => {
            tracing::warn!("linear status check: could not read {issue}: {}", e.0);
            return None;
        }
    };
    let discovery = match c.discover().await {
        Ok(d) => d,
        Err(e) => {
            tracing::warn!("linear status check: could not read the teams: {}", e.0);
            return None;
        }
    };
    let team = discovery.teams.iter().find(|t| t.id == team_id)?;
    problem(workflow, team)
}

/// The teams a project's Linear connection can see, for checking many
/// workflows against one read. `None` when Linear cannot be reached.
pub(crate) async fn teams(state: &Arc<RunsState>, project: &str) -> Option<Vec<Team>> {
    let c = client(state, project).await?;
    match c.discover().await {
        Ok(d) => Some(d.teams),
        Err(e) => {
            tracing::warn!("linear status check: could not read the teams: {}", e.0);
            None
        }
    }
}

/// For a binding or Linear trigger: `workflow_name`, run on issues of `team_id`.
/// Resolves the workflow the way a run would (a custom one shadows a bundled one).
pub(crate) fn for_team(
    state: &RunsState,
    teams: &[Team],
    team_id: &str,
    workflow_name: &str,
) -> Option<String> {
    let (yaml, _) =
        harness_runner::resolve_workflow_source(workflow_name, &state.project_root).ok()?;
    let workflow = harness_dag::parse_workflow(&yaml).ok()?;
    let team = teams.iter().find(|t| t.id == team_id)?;
    problem(&workflow, team)
}

#[cfg(test)]
mod tests {
    use super::*;
    use harness_sources::linear::WorkflowState;

    fn team(statuses: &[&str]) -> Team {
        Team {
            id: "t1".into(),
            name: "eCommerce".into(),
            key: "ECOM".into(),
            states: statuses
                .iter()
                .enumerate()
                .map(|(i, n)| WorkflowState {
                    id: format!("s{i}"),
                    name: (*n).into(),
                    kind: "started".into(),
                    position: i as f64,
                })
                .collect(),
            labels: vec![],
        }
    }

    fn workflow(moves: &[&str]) -> Workflow {
        let mut yaml = String::from("name: task\nnodes:\n");
        for (i, m) in moves.iter().enumerate() {
            yaml.push_str(&format!(
                "  - id: m{i}\n    linear: {{ move_state: \"{m}\" }}\n"
            ));
        }
        harness_dag::parse_workflow(&yaml).expect("parses")
    }

    /// The board this was written for: eCommerce has Code Review, not In Review.
    #[test]
    fn a_missing_status_is_named_with_what_the_team_has() {
        let t = team(&["Todo", "In Progress", "Code Review", "Done"]);
        let msg = problem(&workflow(&["In Progress", "In Review"]), &t).expect("a problem");
        assert_eq!(
            msg,
            "`task` moves issues to `In Review`, which team ECOM (eCommerce) does not \
             have. Its statuses are Todo, In Progress, Code Review, Done. Rename that \
             status in the workflow, or add it to the team."
        );
    }

    #[test]
    fn several_missing_statuses_are_listed_together() {
        let t = team(&["Todo", "In Progress"]);
        let msg = problem(&workflow(&["In Review", "Done", "Released"]), &t).unwrap();
        assert!(
            msg.contains("to `In Review`, `Done` and `Released`, which"),
            "{msg}"
        );
        assert!(msg.contains("Rename those statuses in the workflow, or add them"));
    }

    /// The same rule as the move step: case does not matter, and an id works.
    #[test]
    fn matching_follows_the_move_step() {
        let t = team(&["In Review"]);
        assert!(problem(&workflow(&["in review", "s0"]), &t).is_none());
    }

    #[test]
    fn a_workflow_with_no_moves_has_nothing_to_check() {
        assert!(problem(&workflow(&[]), &team(&[])).is_none());
    }
}
