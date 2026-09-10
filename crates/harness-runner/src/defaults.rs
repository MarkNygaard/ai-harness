//! Built-in default workflows and commands, compiled into the binary so a fresh
//! project gets the standard pipeline without copying any files.
//!
//! Resolution is **custom-first**: a global custom workflow
//! `.harness/workflows/<name>.yaml` (authored via the editor/MCP) or a
//! `.harness/commands/<name>.md` shadows the bundled default of the same name
//! (see [`resolve_workflow_source`] and [`crate::LocalRunner`]'s command
//! resolution). Workflows are global — there is no per-project storage.

use std::path::Path;

/// The workflow run when a request doesn't name one.
pub const DEFAULT_WORKFLOW: &str = "idea-to-pr";

/// Bundled workflows by name.
const WORKFLOWS: &[(&str, &str)] = &[
    (
        DEFAULT_WORKFLOW,
        include_str!("../defaults/workflows/idea-to-pr.yaml"),
    ),
    (
        "merge-pr",
        include_str!("../defaults/workflows/merge-pr.yaml"),
    ),
    (
        "revise-pr",
        include_str!("../defaults/workflows/revise-pr.yaml"),
    ),
    (
        "architect",
        include_str!("../defaults/workflows/architect.yaml"),
    ),
    (
        "judge-ab",
        include_str!("../defaults/workflows/judge-ab.yaml"),
    ),
    (
        "review-pr",
        include_str!("../defaults/workflows/review-pr.yaml"),
    ),
    (
        "linear-epic-supervise",
        include_str!("../defaults/workflows/linear-epic-supervise.yaml"),
    ),
];

/// Bundled command bodies by (de-prefixed) name.
const COMMANDS: &[(&str, &str)] = &[
    (
        "plan-setup",
        include_str!("../defaults/commands/plan-setup.md"),
    ),
    (
        "confirm-plan",
        include_str!("../defaults/commands/confirm-plan.md"),
    ),
    (
        "implement-tasks",
        include_str!("../defaults/commands/implement-tasks.md"),
    ),
    ("validate", include_str!("../defaults/commands/validate.md")),
    (
        "finalize-pr",
        include_str!("../defaults/commands/finalize-pr.md"),
    ),
];

/// A bundled workflow's YAML, by name.
pub fn default_workflow(name: &str) -> Option<&'static str> {
    WORKFLOWS.iter().find(|(n, _)| *n == name).map(|(_, c)| *c)
}

/// A bundled command's markdown body, by name.
pub fn default_command(name: &str) -> Option<&'static str> {
    COMMANDS.iter().find(|(n, _)| *n == name).map(|(_, c)| *c)
}

/// Names of all bundled workflows (for listings / error messages).
pub fn list_default_workflows() -> Vec<&'static str> {
    WORKFLOWS.iter().map(|(n, _)| *n).collect()
}

/// Names of all bundled commands (for the authoring catalog).
pub fn default_command_names() -> Vec<&'static str> {
    COMMANDS.iter().map(|(n, _)| *n).collect()
}

/// Resolve a workflow reference (a filesystem path **or** a bare name) to its
/// YAML source, custom-first.
///
/// 1. An existing file path is read directly.
/// 2. Otherwise the name resolves to a global custom workflow at
///    `<root>/.harness/workflows/<name>.yaml`.
/// 3. Otherwise a bundled [`default_workflow`].
/// 4. Otherwise an error listing what's available.
///
/// Returns `(yaml, label)` where `label` is the resolved name/path for messages.
pub fn resolve_workflow_source(
    input: &str,
    project_root: &Path,
) -> Result<(String, String), String> {
    let trimmed = input.trim();
    if trimmed.is_empty() {
        return resolve_workflow_source(DEFAULT_WORKFLOW, project_root);
    }

    // 1. Explicit, existing path.
    let as_path = Path::new(trimmed);
    if as_path.is_file() {
        let yaml = std::fs::read_to_string(as_path)
            .map_err(|e| format!("failed to read workflow {trimmed}: {e}"))?;
        return Ok((yaml, trimmed.to_string()));
    }

    // 2. Global custom workflow by name.
    let project_file = project_root
        .join(".harness")
        .join("workflows")
        .join(format!("{trimmed}.yaml"));
    if project_file.is_file() {
        let yaml = std::fs::read_to_string(&project_file)
            .map_err(|e| format!("failed to read workflow {trimmed}: {e}"))?;
        return Ok((yaml, trimmed.to_string()));
    }

    // 3. Bundled default by name.
    if let Some(yaml) = default_workflow(trimmed) {
        return Ok((yaml.to_string(), trimmed.to_string()));
    }

    // 4. Not found.
    Err(format!(
        "workflow `{trimmed}` not found (not a file, no custom .harness/workflows/{trimmed}.yaml, \
         and not a bundled default; bundled: {:?})",
        list_default_workflows()
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bundled_workflow_and_commands_are_present() {
        assert!(default_workflow(DEFAULT_WORKFLOW)
            .unwrap()
            .contains("idea-to-pr"));
        for name in [
            "plan-setup",
            "confirm-plan",
            "implement-tasks",
            "validate",
            "finalize-pr",
        ] {
            assert!(
                default_command(name).is_some(),
                "missing bundled command {name}"
            );
        }
        assert!(default_command("nope").is_none());
    }

    /// Both places that start a piece derive the column, and `advance` names the
    /// piece it just reviewed.
    ///
    /// The distinction is the whole correctness of the derivation. `start-epic`
    /// has no piece to ask about yet, so it falls back to the epic's own claim.
    /// `advance` must pass the piece — the supervise run's own claim was made
    /// from the column a *merged* piece rests in, and using that would move the
    /// next piece straight to Done. An earlier edit had these two swapped.
    /// The finished epic's pull request lists its pieces in the order they were
    /// built, which is the order they *completed*.
    ///
    /// Not `sortOrder`: Linear reassigns that when an issue moves between
    /// columns, so by the time an epic finishes it describes where the cards
    /// ended up on the board rather than what happened first. The first real
    /// epic listed its three pieces in exactly reverse order because of it.
    #[test]
    fn the_epic_pull_request_lists_pieces_by_when_they_finished() {
        let yaml = super::WORKFLOWS
            .iter()
            .find(|(name, _)| *name == "linear-epic-supervise")
            .expect("bundled")
            .1;
        let wf = harness_dag::parse_workflow(yaml).expect("parses");
        let advance = wf
            .nodes
            .iter()
            .find(|n| n.id == "advance")
            .and_then(|n| match &n.kind {
                harness_dag::NodeKind::Bash(b) => Some(b.as_str()),
                _ => None,
            })
            .expect("advance is a bash node");

        assert!(
            advance.contains("sort_by(.completed_at"),
            "the epic PR must list pieces by completion"
        );
        assert!(
            !advance.contains("sort_by(.sortOrder)"),
            "sortOrder describes the board at the end, not the build order"
        );
    }

    /// Scope is computed once, deterministically, and consumed — not re-derived
    /// per node. Two runs had an agent "simplify" 79 files of a repo the run
    /// never touched, because a diff against the wrong base (or a `git` call at
    /// the multi-repo root) answers with something plausible instead of nothing.
    #[test]
    fn the_changed_file_set_is_computed_once_and_read_not_rederived() {
        let yaml = super::WORKFLOWS
            .iter()
            .find(|(name, _)| *name == "idea-to-pr")
            .expect("bundled")
            .1;
        let wf = harness_dag::parse_workflow(yaml).expect("parses");
        let body = |id: &str| {
            wf.nodes
                .iter()
                .find(|n| n.id == id)
                .map(|n| match &n.kind {
                    harness_dag::NodeKind::Bash(b) => b.clone(),
                    harness_dag::NodeKind::Prompt(p) => p.clone(),
                    harness_dag::NodeKind::Loop(l) => l.prompt.clone(),
                    _ => String::new(),
                })
                .unwrap_or_else(|| panic!("no node `{id}`"))
        };

        // The producer diffs each repo against *its own* base, not the run's.
        let scope = body("scope");
        assert!(
            scope.contains(".base_branch") && scope.contains(r#"origin/$base...HEAD"#),
            "scope must diff each repo against its own configured base"
        );
        assert!(
            scope.contains("scope.json") && scope.contains("head:"),
            "scope must record the file set and each repo's head"
        );
        assert!(
            scope.contains("ls-files --others --exclude-standard")
                && scope.contains("diff --name-only HEAD"),
            "scope must count UNCOMMITTED work: `implement-tasks` does not commit, \
             so a committed-only diff calls the repo the run just edited untouched \
             — and the guard then deletes it"
        );

        // The consumer reads it and is told not to run its own diff.
        let simplify = body("pi-simplify");
        assert!(
            simplify.contains("scope.json"),
            "pi-simplify must read the computed scope"
        );
        assert!(
            !simplify.contains("git diff --name-only"),
            "pi-simplify must not compute its own changed-file set any more"
        );

        // And the guard makes it enforceable rather than merely instructed.
        let guard = body("guard-scope");
        assert!(
            guard.contains("files | length == 0") && guard.contains("reset --hard"),
            "guard-scope must revert repos that were not in scope"
        );
        assert!(
            guard.contains("clean -fd") && !guard.contains("clean -fdx"),
            "the guard must not clean ignored files — that is the warm build state"
        );
        assert!(
            guard.contains(r#"[ "$empty" -eq "$total" ]"#),
            "the guard must refuse to revert when the scope says every repo is \
             empty — a run always changes something, so that is a broken scope, \
             and reverting on it destroys the work instead of protecting it"
        );
        // Ordering: nothing validates or ships before the guard has run.
        let after = |id: &str| {
            wf.nodes
                .iter()
                .find(|n| n.id == id)
                .map(|n| n.depends_on.clone())
                .unwrap_or_default()
        };
        assert!(
            after("guard-scope").contains(&"pi-simplify".to_string()),
            "the guard runs after the step that wanders"
        );
        assert!(
            after("validate").contains(&"guard-scope".to_string()),
            "validate must see the reverted tree, not the wandered one"
        );
    }

    /// A prose "the title MUST end with the identifier" was applied to the first
    /// PR of a run and skipped on the second, so Linear never linked it.
    #[test]
    fn every_pr_title_is_gated_not_merely_requested() {
        let yaml = super::WORKFLOWS
            .iter()
            .find(|(name, _)| *name == "idea-to-pr")
            .expect("bundled")
            .1;
        let wf = harness_dag::parse_workflow(yaml).expect("parses");
        let gate = wf
            .nodes
            .iter()
            .find(|n| n.id == "gate-pr-titles")
            .expect("gate exists");
        let script = match &gate.kind {
            harness_dag::NodeKind::Bash(b) => b.clone(),
            _ => panic!("the gate must be bash — an agent cannot gate itself"),
        };
        assert!(
            script.contains(".pr-list"),
            "the gate must check every PR the run opened, not one"
        );
        assert!(
            script.contains("harness linear issue --issue"),
            "the identifier must come from Linear, not from matching the task text"
        );
        assert!(
            script.contains("feat|fix|chore|ci|docs|refactor|perf|test"),
            "the gate must check the conventional-commits format"
        );
        assert!(
            script.contains("gh api -X PATCH"),
            "appending a missing identifier is mechanical, so the gate fixes it \
             (and `gh pr edit` needs a token scope this one lacks)"
        );
        assert!(
            script.contains("exit 1"),
            "a title needing a type/scope judgement must fail the node"
        );
        assert!(
            gate.depends_on.contains(&"verify-pr-title".to_string()),
            "the gate runs after the step it checks"
        );
        assert!(
            wf.nodes
                .iter()
                .find(|n| n.id == "pi-review-fix-loop")
                .expect("review loop")
                .depends_on
                .contains(&"gate-pr-titles".to_string()),
            "nothing proceeds past a wrong title"
        );
    }

    #[test]
    fn the_supervisor_derives_the_build_column_in_both_places() {
        let yaml = super::WORKFLOWS
            .iter()
            .find(|(name, _)| *name == "linear-epic-supervise")
            .expect("bundled")
            .1;
        let wf = harness_dag::parse_workflow(yaml).expect("parses");
        let script = |id: &str| {
            wf.nodes
                .iter()
                .find(|n| n.id == id)
                .and_then(|n| match &n.kind {
                    harness_dag::NodeKind::Bash(b) => Some(b.clone()),
                    _ => None,
                })
                .unwrap_or_else(|| panic!("{id} is a bash node"))
        };

        let start = script("start-epic");
        assert!(
            start.contains("harness linear ready-state"),
            "start-epic must derive the column rather than require EPIC_READY_STATE"
        );
        assert!(
            !start.contains("ready-state --issue"),
            "start-epic has no piece to ask about; it uses the epic's own claim"
        );

        let advance = script("advance");
        assert!(
            advance.contains(r#"harness linear ready-state --issue "$ISSUE_ID""#),
            "advance must ask about the piece it reviewed, not this run's claim"
        );

        // The override still works, in both.
        for (id, body) in [("start-epic", &start), ("advance", &advance)] {
            assert!(
                body.contains("${EPIC_READY_STATE:-}"),
                "{id} must still honour an explicit EPIC_READY_STATE"
            );
        }
    }

    #[test]
    fn the_epic_supervisor_parses_and_branches_on_the_verdict() {
        let yaml = default_workflow("linear-epic-supervise").expect("registered");
        let wf = harness_dag::parse_workflow(yaml).expect("bundled workflow must parse");

        let node = |id: &str| wf.nodes.iter().find(|n| n.id == id).expect(id);

        // Nothing writes to Linear until the review has spoken, and the two
        // write paths are gated on opposite verdicts — so a run can advance the
        // epic or file a corrective, never both.
        let advance = node("advance");
        let correct = node("correct");
        assert_eq!(advance.depends_on, ["review"]);
        assert_eq!(correct.depends_on, ["review"]);
        assert_eq!(
            advance.when.as_deref(),
            Some("$review.output.passed == 'true'")
        );
        assert_eq!(
            correct.when.as_deref(),
            Some("$review.output.passed != 'true'")
        );

        // An issue that is neither a piece nor an epic is cancelled, not
        // failed: the likely cause is a column that also holds ordinary work,
        // and that should not read as a broken run.
        assert!(matches!(
            node("not-a-piece").kind,
            harness_dag::NodeKind::Cancel(_)
        ));

        // The three cases are mutually exclusive, so exactly one path runs:
        // grade a merged piece, start an epic, or cancel.
        let gate = |id: &str| node(id).when.clone().expect(id);
        assert_eq!(gate("review"), "$context.output.mode == 'piece'");
        assert_eq!(gate("start-epic"), "$context.output.mode == 'epic'");
        assert_eq!(gate("not-a-piece"), "$context.output.mode == 'neither'");

        // The last piece turns the epic branch into one pull request and hands
        // the epic to a human. Asserted on the source because the branch only
        // runs at the end of a whole epic: nothing else would catch it going
        // missing in an edit.
        let advance = &node("advance").kind;
        let harness_dag::NodeKind::Bash(script) = advance else {
            panic!("advance should be a shell step");
        };
        assert!(
            script.contains("gh pr create"),
            "the finished epic opens a PR"
        );
        assert!(
            script.contains("gh pr list"),
            "a re-run must find the existing PR rather than open a second"
        );
        assert!(
            script.contains("EPIC_REVIEW_STATE"),
            "the finished epic is handed to a human"
        );
        assert!(
            !script.contains("gh pr merge"),
            "the whole feature is never merged automatically"
        );

        // The reviewer is the expensive one on purpose; everything else is shell.
        let review = node("review");
        assert_eq!(review.model.as_deref(), Some("opus"));
    }

    #[test]
    fn bundled_workflow_parses_and_uses_loop_providers() {
        // The bundled pipeline must actually parse with our DAG model, including
        // the loop blocks that set provider/model inside `loop:`.
        let yaml = default_workflow(DEFAULT_WORKFLOW).unwrap();
        let wf = harness_dag::parse_workflow(yaml).expect("bundled workflow must parse");
        assert_eq!(wf.name, DEFAULT_WORKFLOW);
        assert!(wf.nodes.iter().any(|n| n.id == "pi-review-fix-loop"));
    }

    /// Bundled workflows ship to every project, so an agent prompt must never
    /// name a concrete verify chain. `final-verify-loop` used to branch on paths
    /// starting with `web/` or `crates/` and otherwise fall back to "run the
    /// Rust+web gate" — in a pnpm monorepo (`apps/web/...`) no branch matched, so
    /// the gate was told to run `cargo` in a repo with no `Cargo.toml`. Project
    /// commands belong in the project's `CLAUDE.md`, which every agent node reads.
    #[test]
    fn bundled_workflows_name_no_concrete_verify_chain() {
        for name in ["idea-to-pr", "architect", "revise-pr", "merge-pr"] {
            let yaml = default_workflow(name).unwrap_or_else(|| panic!("bundled {name}"));
            // `cargo clippy` is deliberately absent from this list: architect's
            // metrics node runs it behind its own `HAS_RUST` stack detection,
            // which is guarded, not assumed.
            for needle in ["bunx", "cargo nextest", "RUSTFLAGS", "pnpm --filter"] {
                assert!(
                    !yaml.contains(needle),
                    "`{name}` hardcodes `{needle}` — read the chain from the \
                     project's CLAUDE.md instead"
                );
            }
        }
    }

    /// The final gate skips re-running what `validate` already proved by comparing
    /// HEAD against the sha recorded right after it. That recording must land
    /// before `finalize-pr`, which may itself commit.
    #[test]
    fn the_verified_head_is_recorded_before_finalize_pr() {
        let yaml = default_workflow(DEFAULT_WORKFLOW).unwrap();
        let wf = harness_dag::parse_workflow(yaml).expect("must parse");
        let node = |id: &str| {
            wf.nodes
                .iter()
                .find(|n| n.id == id)
                .unwrap_or_else(|| panic!("missing node `{id}`"))
        };
        assert_eq!(node("record-verified-head").depends_on, vec!["validate"]);
        assert_eq!(
            node("finalize-pr").depends_on,
            vec!["record-verified-head"],
            "finalize-pr must run after the sha is recorded, not beside it"
        );
    }

    #[test]
    fn bundled_gpt_review_uses_subscription_codex_namespace() {
        let yaml = default_workflow(DEFAULT_WORKFLOW).unwrap();
        let wf = harness_dag::parse_workflow(yaml).expect("bundled workflow must parse");
        let node = wf
            .nodes
            .iter()
            .find(|n| n.id == "gpt-review-fix")
            .expect("gpt review node exists");
        assert_eq!(node.provider.as_deref(), Some("pi"));
        assert_eq!(node.model.as_deref(), Some("openai-codex/gpt-6-astra"));
    }

    /// `review-pr` reviews code that was never planned here, so it must judge
    /// the diff against git-computed scope and the PR's own stated intent — not
    /// against a plan. A future edit that "helpfully" adds a planning node
    /// would hand two reviewers a guess at the author's intent and let them
    /// enforce it as a contract, which is the one failure mode this workflow is
    /// shaped to avoid.
    #[test]
    fn review_pr_reviews_against_the_diff_not_a_plan() {
        let yaml = default_workflow("review-pr").expect("review-pr bundled");
        assert!(
            !yaml.contains("plan.md"),
            "review-pr must not review against a plan — the PR was not planned here"
        );
        let wf = harness_dag::parse_workflow(yaml).expect("review-pr must parse");
        let node = |id: &str| {
            wf.nodes
                .iter()
                .find(|n| n.id == id)
                .unwrap_or_else(|| panic!("review-pr has a `{id}` node"))
        };
        let body = |id: &str| match &node(id).kind {
            harness_dag::NodeKind::Prompt(p) => p.clone(),
            harness_dag::NodeKind::Bash(b) => b.clone(),
            other => panic!("`{id}` has an unexpected body: {other:?}"),
        };

        // What replaces the plan: the git-computed file set and the recorded
        // intent. Both review passes must actually read both.
        for review in ["gpt-review-fix", "anthropic-review-fix"] {
            let prompt = body(review);
            assert!(
                prompt.contains("scope.json"),
                "`{review}` must take its scope from scope.json, not its own git diff"
            );
            assert!(
                prompt.contains("pr-intent.md"),
                "`{review}` must read the recorded PR intent"
            );
        }

        // Model diversity is the point of running two passes.
        assert_eq!(node("gpt-review-fix").provider.as_deref(), Some("pi"));
        assert_eq!(
            node("gpt-review-fix").model.as_deref(),
            Some("openai-codex/gpt-6-astra")
        );
        assert_eq!(
            node("anthropic-review-fix").provider.as_deref(),
            Some("claude")
        );
        assert_eq!(node("anthropic-review-fix").model.as_deref(), Some("opus"));

        // The checkout must be deterministic: every node downstream reads the
        // working tree, so an agent that forgets `gh pr checkout` would have
        // them review the base branch and report it clean.
        assert!(
            matches!(node("checkout-pr").kind, harness_dag::NodeKind::Bash(_)),
            "checkout-pr must be a bash node, not an agent asked to remember"
        );

        // Review fixes land after the baseline, so the final gate re-verifies.
        assert_eq!(
            node("final-validate").depends_on,
            vec!["anthropic-review-fix"]
        );
    }

    #[test]
    fn revise_pr_revalidates_after_review_fixes() {
        let yaml = default_workflow("revise-pr").expect("revise-pr bundled");
        let wf = harness_dag::parse_workflow(yaml).expect("revise-pr must parse");
        let node = |id: &str| {
            wf.nodes
                .iter()
                .find(|n| n.id == id)
                .unwrap_or_else(|| panic!("missing node `{id}`"))
        };
        let deps = |id: &str| {
            node(id)
                .depends_on
                .iter()
                .map(String::as_str)
                .collect::<Vec<_>>()
        };

        assert_eq!(
            node("explore").when.as_deref(),
            Some(
                "$gather-feedback.output.has_github_feedback == 'true' || \
                 $gather-feedback.output.has_linear_feedback == 'true'"
            )
        );
        assert_eq!(deps("final-validate"), vec!["anthropic-review-fix"]);
        assert!(
            matches!(&node("final-validate").kind, harness_dag::NodeKind::Command(name) if name == "validate")
        );
        assert!(node("final-validate").output_format.is_some());
        assert_eq!(deps("abort-final-invalid"), vec!["final-validate"]);
        assert_eq!(
            node("abort-final-invalid").when.as_deref(),
            Some("$final-validate.output.passed != 'true'")
        );
        assert_eq!(deps("summary"), vec!["final-validate"]);
        assert_eq!(
            node("summary").when.as_deref(),
            Some("$final-validate.output.passed == 'true'")
        );
    }

    /// A card moved to "Changes requested" by mistake must cancel, not revise.
    ///
    /// It didn't, once: `gather-feedback` reported a single `has_feedback`, and the
    /// issue's own bug report — which `task_for_issue` puts in `$ARGUMENTS` on every
    /// run — passed as a tester saying the fix had failed. So the run planned and
    /// pushed a second speculative fix to a PR nobody had complained about. The two
    /// booleans exist so the model must attribute feedback to a source, and the
    /// Linear one is a question about a string's presence rather than a judgement.
    #[test]
    fn revise_pr_aborts_when_neither_feedback_source_has_anything() {
        let yaml = default_workflow("revise-pr").expect("revise-pr bundled");
        let wf = harness_dag::parse_workflow(yaml).expect("revise-pr must parse");
        let gather = wf
            .nodes
            .iter()
            .find(|n| n.id == "gather-feedback")
            .expect("gather-feedback exists");
        let schema = gather
            .output_format
            .as_ref()
            .expect("gather-feedback is structured")
            .to_string();
        for field in ["has_github_feedback", "has_linear_feedback"] {
            assert!(schema.contains(field), "{field} missing from {schema}");
        }
        assert!(
            !schema.contains("\"has_feedback\""),
            "the single conflated boolean is back: {schema}"
        );

        let abort = wf
            .nodes
            .iter()
            .find(|n| n.id == "abort-no-feedback")
            .expect("abort-no-feedback exists");
        assert_eq!(
            abort.when.as_deref(),
            Some(
                "$gather-feedback.output.has_github_feedback != 'true' && \
                 $gather-feedback.output.has_linear_feedback != 'true'"
            )
        );

        // The prompt has to say that the text before the Linear heading is the
        // original report, or the same conflation is one paraphrase away.
        let harness_dag::NodeKind::Prompt(prompt) = &gather.kind else {
            panic!("gather-feedback is an inline prompt");
        };
        assert!(prompt.contains("If that heading is absent there is no Linear feedback"));
        assert!(prompt.contains("ORIGINAL bug report"));
    }

    #[test]
    fn resolve_falls_back_to_bundled_default() {
        let tmp = std::env::temp_dir();
        let (yaml, label) = resolve_workflow_source(DEFAULT_WORKFLOW, &tmp).unwrap();
        assert_eq!(label, DEFAULT_WORKFLOW);
        assert!(yaml.contains("idea-to-pr"));
    }

    #[test]
    fn resolve_empty_uses_default_workflow() {
        let (_, label) = resolve_workflow_source("   ", &std::env::temp_dir()).unwrap();
        assert_eq!(label, DEFAULT_WORKFLOW);
    }

    #[test]
    fn resolve_unknown_name_errors() {
        let err = resolve_workflow_source("ghost-workflow", &std::env::temp_dir()).unwrap_err();
        assert!(err.contains("ghost-workflow"));
    }

    #[test]
    fn every_bundled_workflow_parses() {
        for (name, yaml) in WORKFLOWS {
            let wf = harness_dag::parse_workflow(yaml)
                .unwrap_or_else(|e| panic!("bundled workflow `{name}` failed to parse: {e}"));
            assert!(!wf.nodes.is_empty(), "`{name}` has no nodes");
        }
        // The default, merge-pr, and revise-pr workflows are all present.
        let names = list_default_workflows();
        assert!(names.contains(&DEFAULT_WORKFLOW));
        assert!(names.contains(&"merge-pr"));
        assert!(names.contains(&"architect"));
        assert!(names.contains(&"revise-pr"));
    }

    /// A node that derives its scope from `origin/$BASE_BRANCH` must also say
    /// WHICH repo it is standing in. In a multi-repo project the workspace root
    /// is not a git repo at all, so a bare `git diff` there fails — and an agent
    /// that reads that failure as "look harder" walks into a sibling repo and
    /// rewrites another ticket's work. The anchor is `$HARNESS_REPOS` (before
    /// any PR exists) or `.pr-list` (after `finalize-pr` writes it).
    #[test]
    fn base_diff_nodes_in_multi_repo_workflows_name_their_repo() {
        for (name, yaml) in WORKFLOWS {
            // Only workflows written for multi-repo projects: a single-repo one
            // stands in the single repo at the root by construction.
            if !yaml.contains("HARNESS_REPOS") {
                continue;
            }
            let wf = harness_dag::parse_workflow(yaml).expect("bundled workflow parses");
            for node in &wf.nodes {
                let body = match &node.kind {
                    harness_dag::NodeKind::Prompt(p) => p.as_str(),
                    harness_dag::NodeKind::Bash(b) => b.as_str(),
                    harness_dag::NodeKind::Loop(l) => l.prompt.as_str(),
                    _ => continue,
                };
                if !body.contains("origin/$BASE_BRANCH") {
                    continue;
                }
                assert!(
                    body.contains("HARNESS_REPOS") || body.contains("pr-list"),
                    "`{name}` node `{}` diffs against origin/$BASE_BRANCH without \
                     naming the repo it runs in — at a multi-repo root that git \
                     call fails and the agent goes hunting in a sibling repo",
                    node.id
                );
            }
        }
    }

    #[test]
    fn architect_workflow_parses_and_enforces_readonly() {
        let yaml = default_workflow("architect").expect("architect bundled");
        let wf = harness_dag::parse_workflow(yaml).expect("architect must parse");
        assert_eq!(wf.name, "architect");
        let node = |id: &str| {
            wf.nodes
                .iter()
                .find(|n| n.id == id)
                .unwrap_or_else(|| panic!("missing node `{id}`"))
        };
        // analyze + plan are read-only: a pre_tool_use deny rule must be present.
        for id in ["analyze", "plan"] {
            let hooks = node(id).hooks.as_ref().expect("read-only node has hooks");
            let denies = hooks
                .pre_tool_use
                .iter()
                .any(|r| r.decision == Some(harness_dag::HookDecision::Deny));
            assert!(denies, "{id} must deny code-mutating tools");
        }
        // simplify steers per-edit verification via a post_tool_use rule.
        let hooks = node("simplify").hooks.as_ref().expect("simplify has hooks");
        assert!(hooks
            .post_tool_use
            .iter()
            .any(|r| r.additional_context.is_some()));
        // validate exposes the {passed} verdict downstream nodes gate on.
        assert!(node("validate").output_format.is_some());
    }

    #[test]
    fn judge_ab_workflow_emits_verdict_on_claude_default() {
        let yaml = default_workflow("judge-ab").expect("judge-ab bundled");
        let wf = harness_dag::parse_workflow(yaml).expect("judge-ab must parse");
        assert_eq!(wf.name, "judge-ab");
        // The judge model is the workflow default, held constant across a
        // comparison unless the trigger overrides it.
        assert_eq!(wf.provider.as_deref(), Some("claude"));
        assert_eq!(wf.model.as_deref(), Some("opus"));
        // Single judge node that emits the structured verdict.
        let judge = wf
            .nodes
            .iter()
            .find(|n| n.id == "judge")
            .expect("judge node exists");
        assert!(
            judge.output_format.is_some(),
            "judge must emit a structured verdict"
        );
    }
}
