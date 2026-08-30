//! **Does this project's Linear wiring actually connect?**
//!
//! A binding is a relay: an issue is claimed from one column, and on completion
//! moved to another — where, if the setup is right, a *different* binding picks
//! it up. `Todo → idea-to-pr → Ready for merge → merge-pr → Done → supervise`.
//!
//! Nothing enforces that the relay joins up. Point a ready state at a column no
//! binding polls and the issue lands there and stops, with no error anywhere:
//! the run succeeded, the move succeeded, and the work is simply parked. For an
//! epic that means a feature quietly stops advancing overnight, and the only
//! symptom is absence.
//!
//! So this reads the wiring and says what breaks, in the same terms the board
//! uses — column names, not the state ids the rows actually store.

use std::collections::HashMap;

use harness_persist::LinearSource;

use super::linear_agent::EPIC_SUPERVISOR;

/// How much a finding matters.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Level {
    /// Work will be dropped or never start.
    Error,
    /// Works, but probably not as intended.
    Warn,
    /// Worth stating so the report reads as a whole picture.
    Ok,
}

/// One thing worth saying about the wiring.
#[derive(Debug, Clone, serde::Serialize)]
pub struct Finding {
    pub level: Level,
    /// The binding this is about, as `workflow` — absent for project-wide notes.
    pub workflow: Option<String>,
    pub message: String,
}

/// A team's columns, by state id.
pub type States = HashMap<String, String>;

fn name<'a>(states: &'a States, id: &'a str) -> &'a str {
    states.get(id).map(String::as_str).unwrap_or(id)
}

/// Whether any enabled binding on the same team claims from `state`.
fn polled_by<'a>(
    bindings: &'a [LinearSource],
    team: &str,
    state: &str,
) -> Option<&'a LinearSource> {
    bindings
        .iter()
        .find(|b| b.enabled && b.team_id == team && b.source_state_id == state)
}

/// Read a project's bindings and report what will not work.
///
/// `states` maps state id → column name for every team involved; ids it does not
/// know are reported as unknown, which is itself the finding — a column renamed
/// in Linear keeps its id, but one *deleted* and recreated does not, and the
/// binding then points at nothing.
pub fn diagnose(bindings: &[LinearSource], states: &States) -> Vec<Finding> {
    let mut out = Vec::new();

    if bindings.is_empty() {
        out.push(Finding {
            level: Level::Error,
            workflow: None,
            message: "No Linear bindings for this project — nothing will ever be picked up."
                .to_string(),
        });
        return out;
    }

    for b in bindings {
        let wf = Some(b.workflow.clone());

        if !b.enabled {
            out.push(Finding {
                level: Level::Warn,
                workflow: wf.clone(),
                message: format!(
                    "Disabled, so `{}` is never triggered — by the poller or by delegation.",
                    b.workflow
                ),
            });
            continue;
        }
        if !b.live {
            out.push(Finding {
                level: Level::Warn,
                workflow: wf.clone(),
                message: format!(
                    "Enabled but not live: the poller only logs what it *would* claim from {}. \
                     Delegation still works.",
                    name(states, &b.source_state_id)
                ),
            });
        }
        if !states.contains_key(&b.source_state_id) {
            out.push(Finding {
                level: Level::Error,
                workflow: wf.clone(),
                message: format!(
                    "Claims from a status that no longer exists on {} (id {}). Nothing will ever \
                     be picked up — re-pick the column.",
                    b.team_name, b.source_state_id
                ),
            });
        }

        // Where a completed run leaves the issue, and who takes it from there.
        // A target nobody polls is the silent stall this whole check exists for.
        for (slot, target) in [
            ("Ready", b.ready_state_id.as_deref()),
            ("Ready (epic piece)", b.piece_ready_state_id.as_deref()),
        ] {
            let Some(target) = target else { continue };
            if !states.contains_key(target) {
                out.push(Finding {
                    level: Level::Error,
                    workflow: wf.clone(),
                    message: format!(
                        "`{slot}` points at a status that no longer exists on {} (id {target}).",
                        b.team_name
                    ),
                });
                continue;
            }
            match polled_by(bindings, &b.team_id, target) {
                Some(next) => out.push(Finding {
                    level: Level::Ok,
                    workflow: wf.clone(),
                    message: format!(
                        "`{slot}` → {} → `{}` picks it up.",
                        name(states, target),
                        next.workflow
                    ),
                }),
                None => out.push(Finding {
                    level: Level::Warn,
                    workflow: wf.clone(),
                    message: format!(
                        "`{slot}` → {}, which no enabled binding claims from. Work stops there \
                         with no error — fine if a person takes over at that column, a silent \
                         stall if not.",
                        name(states, target)
                    ),
                }),
            }
        }
    }

    out.extend(epic_findings(bindings, states));
    out
}

/// The epic relay specifically: a piece has to be able to get from built, to
/// merged, to reviewed, to the next piece starting.
fn epic_findings(bindings: &[LinearSource], states: &States) -> Vec<Finding> {
    let mut out = Vec::new();
    let supervisors: Vec<&LinearSource> = bindings
        .iter()
        .filter(|b| b.enabled && b.workflow == EPIC_SUPERVISOR)
        .collect();

    if supervisors.is_empty() {
        // Not a failure: a project need not use epics. Said once, so the report
        // does not read as though epics were configured and broken.
        out.push(Finding {
            level: Level::Ok,
            workflow: None,
            message: format!(
                "No `{EPIC_SUPERVISOR}` binding, so epics are off. An issue with sub-issues is \
                 built like any other."
            ),
        });
        return out;
    }

    for sup in supervisors {
        // The supervisor reviews a piece once its PR has merged, so something
        // has to deliver a piece into the column it watches. Unreachable, the
        // epic builds its first piece and then never advances.
        let feeds: Vec<&str> = bindings
            .iter()
            .filter(|b| {
                b.enabled
                    && b.team_id == sup.team_id
                    && b.workflow != EPIC_SUPERVISOR
                    && (b.ready_state_id.as_deref() == Some(&sup.source_state_id)
                        || b.piece_ready_state_id.as_deref() == Some(&sup.source_state_id))
            })
            .map(|b| b.workflow.as_str())
            .collect();

        if feeds.is_empty() {
            out.push(Finding {
                level: Level::Error,
                workflow: Some(sup.workflow.clone()),
                message: format!(
                    "Nothing moves an issue into {}, so a piece is never reviewed and the epic \
                     stops after its first piece. Point some binding's `Ready` (or `Ready (epic \
                     piece)`) at that column.",
                    name(states, &sup.source_state_id)
                ),
            });
        } else {
            out.push(Finding {
                level: Level::Ok,
                workflow: Some(sup.workflow.clone()),
                message: format!(
                    "Reviews pieces arriving in {} from `{}`.",
                    name(states, &sup.source_state_id),
                    feeds.join("`, `")
                ),
            });
        }

        // A piece that stops at a human gate is the thing `piece_ready_state_id`
        // exists to avoid, and it is invisible until an epic sits there for a
        // day. Only worth saying when the gate is genuinely a dead end.
        for b in bindings
            .iter()
            .filter(|b| b.enabled && b.team_id == sup.team_id && b.workflow != EPIC_SUPERVISOR)
        {
            let Some(ready) = b.ready_state_id.as_deref() else {
                continue;
            };
            if b.piece_ready_state_id.is_some() {
                continue;
            }
            if polled_by(bindings, &b.team_id, ready).is_none() {
                out.push(Finding {
                    level: Level::Warn,
                    workflow: Some(b.workflow.clone()),
                    message: format!(
                        "Epics are on, and a finished piece stops at {} waiting for a person. Set \
                         `Ready (epic piece)` to the column that merges, so pieces relay onward \
                         while standalone issues still wait for you.",
                        name(states, ready)
                    ),
                });
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Utc;

    fn states() -> States {
        [
            ("todo", "Todo"),
            ("rfm", "Ready for merge"),
            ("done", "Done"),
            ("ft", "Functional testing"),
        ]
        .into_iter()
        .map(|(a, b)| (a.to_string(), b.to_string()))
        .collect()
    }

    fn binding(workflow: &str, source: &str, ready: Option<&str>) -> LinearSource {
        LinearSource {
            project: "p".into(),
            workflow: workflow.into(),
            team_id: "t".into(),
            team_name: "Team".into(),
            source_state_id: source.into(),
            failed_label: None,
            in_progress_state_id: None,
            review_state_id: None,
            ready_state_id: ready.map(str::to_string),
            piece_ready_state_id: None,
            base_branch: None,
            poll_interval_secs: 60,
            max_concurrent_runs: 1,
            max_attempts: 1,
            enabled: true,
            live: true,
            created_at: Utc::now(),
            updated_at: Utc::now(),
        }
    }

    fn messages(f: &[Finding], level: Level) -> Vec<&str> {
        f.iter()
            .filter(|x| x.level == level)
            .map(|x| x.message.as_str())
            .collect()
    }

    /// The whole relay joined up: nothing to report but the chain itself.
    #[test]
    fn a_complete_epic_relay_reports_no_problems() {
        let mut build = binding("idea-to-pr", "todo", Some("rfm"));
        build.piece_ready_state_id = Some("rfm".into());
        let bindings = vec![
            build,
            binding("merge-pr", "rfm", Some("done")),
            binding(EPIC_SUPERVISOR, "done", None),
        ];
        let f = diagnose(&bindings, &states());
        assert!(messages(&f, Level::Error).is_empty(), "{f:#?}");
        assert!(messages(&f, Level::Warn).is_empty(), "{f:#?}");
    }

    /// The silent stall this exists to catch: a ready state nobody polls. The
    /// run succeeds, the move succeeds, and the work is parked forever.
    #[test]
    fn a_ready_state_no_binding_polls_is_reported() {
        let bindings = vec![binding("idea-to-pr", "todo", Some("ft"))];
        let f = diagnose(&bindings, &states());
        let warns = messages(&f, Level::Warn);
        assert!(
            warns.iter().any(|m| m.contains("Functional testing")),
            "{warns:#?}"
        );
    }

    /// An epic that builds its first piece and then stops forever, because
    /// nothing delivers a merged piece to the supervisor.
    #[test]
    fn a_supervisor_nothing_feeds_is_an_error() {
        let bindings = vec![
            binding("idea-to-pr", "todo", Some("ft")),
            binding(EPIC_SUPERVISOR, "done", None),
        ];
        let f = diagnose(&bindings, &states());
        let errors = messages(&f, Level::Error);
        assert!(
            errors.iter().any(|m| m.contains("never reviewed")),
            "{errors:#?}"
        );
    }

    /// Epics on, pieces stopping at a human gate: works, but it is the loop the
    /// epic exists to remove, and nothing else would ever say so.
    #[test]
    fn epic_pieces_waiting_at_a_human_gate_are_pointed_out() {
        let bindings = vec![
            binding("idea-to-pr", "todo", Some("ft")),
            binding("merge-pr", "rfm", Some("done")),
            binding(EPIC_SUPERVISOR, "done", None),
        ];
        let f = diagnose(&bindings, &states());
        assert!(
            messages(&f, Level::Warn)
                .iter()
                .any(|m| m.contains("Ready (epic piece)")),
            "{f:#?}"
        );
    }

    /// A column deleted in Linear takes its id with it, and the binding then
    /// points at nothing — indistinguishable from working, until nothing runs.
    #[test]
    fn a_status_that_no_longer_exists_is_an_error() {
        let bindings = vec![binding("idea-to-pr", "gone", Some("rfm"))];
        let f = diagnose(&bindings, &states());
        assert!(
            messages(&f, Level::Error)
                .iter()
                .any(|m| m.contains("no longer exists")),
            "{f:#?}"
        );
    }

    #[test]
    fn no_bindings_at_all_is_the_only_thing_worth_saying() {
        let f = diagnose(&[], &states());
        assert_eq!(f.len(), 1);
        assert_eq!(f[0].level, Level::Error);
    }

    /// A project need not use epics, and a report that stayed silent about it
    /// would read as though they were configured and broken.
    #[test]
    fn a_project_without_a_supervisor_is_told_so_plainly() {
        let bindings = vec![binding("idea-to-pr", "todo", Some("rfm"))];
        let f = diagnose(&bindings, &states());
        assert!(
            messages(&f, Level::Ok)
                .iter()
                .any(|m| m.contains("epics are off")),
            "{f:#?}"
        );
    }
}
