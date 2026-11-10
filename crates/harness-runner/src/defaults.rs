//! Built-in default workflows and commands, compiled into the binary so a fresh
//! project gets the standard pipeline without copying any files.
//!
//! Resolution is **custom-first**: a global custom workflow
//! `.harness/workflows/<name>.yaml` (authored via the editor/MCP) or a
//! `.harness/commands/<name>.md` shadows the bundled default of the same name
//! (see [`resolve_workflow_source`] and [`crate::LocalRunner`]'s command
//! resolution). Workflows are global — there is no per-project storage.

use std::path::Path;

/// The harness's own improvement review (`internal: true`): started by the
/// server, never offered to a person.
pub const IMPROVEMENTS_WORKFLOW: &str = "improvements-analysis";

/// The harness's own fact learning (`internal: true`): started by the server
/// when a project's runs keep failing the same way, never offered to a person.
pub const FACTS_WORKFLOW: &str = "learn-facts";

/// Every workflow that is the harness's own machinery: hidden from run lists
/// and the dashboard, and started only by the server.
pub const INTERNAL_WORKFLOWS: &[&str] = &[IMPROVEMENTS_WORKFLOW, FACTS_WORKFLOW];

/// The workflow run when a request doesn't name one.
pub const DEFAULT_WORKFLOW: &str = "task";

/// Bundled workflows by name.
const WORKFLOWS: &[(&str, &str)] = &[
    (
        DEFAULT_WORKFLOW,
        include_str!("../defaults/workflows/task.yaml"),
    ),
    (
        "task-merge",
        include_str!("../defaults/workflows/task-merge.yaml"),
    ),
    (
        "revise-task",
        include_str!("../defaults/workflows/revise-task.yaml"),
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
    (
        IMPROVEMENTS_WORKFLOW,
        include_str!("../defaults/workflows/improvements-analysis.yaml"),
    ),
    (
        FACTS_WORKFLOW,
        include_str!("../defaults/workflows/learn-facts.yaml"),
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
        assert_eq!(
            default_workflow(DEFAULT_WORKFLOW).unwrap().lines().next(),
            Some("name: task")
        );
        for gone in ["idea-to-pr", "merge-pr", "revise-pr"] {
            assert!(
                default_workflow(gone).is_none(),
                "`{gone}` was replaced by task / task-merge / revise-task"
            );
        }
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
        let yaml = default_workflow("review-pr").expect("bundled");
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
            after("gpt-review-fix").contains(&"guard-scope".to_string()),
            "the reviewers must see the reverted tree, not the wandered one"
        );
    }

    /// A repository with no CI is merged, not parked; and a merge that did not
    /// happen ends the run cancelled. A *completed* run moves the issue to its
    /// ready column, and for a piece of an epic that is where the supervisor
    /// grades it and starts the next one: ERP-141 was declined for having no
    /// checks, still reached Done, and the epic advanced without it.
    #[test]
    fn task_merge_merges_without_ci_and_cancels_when_it_does_not_merge() {
        let yaml = default_workflow("task-merge").expect("registered");
        let wf = harness_dag::parse_workflow(yaml).expect("bundled workflow must parse");
        let node = |id: &str| wf.nodes.iter().find(|n| n.id == id).expect(id);

        let harness_dag::NodeKind::Prompt(merge) = &node("merge").kind else {
            panic!("merge is a prompt node");
        };
        assert!(
            !merge.contains("no checks exist at all: do NOT merge"),
            "a repository without CI must still be merged"
        );
        assert!(
            merge.contains("no checks reported"),
            "the no-checks case must be handled explicitly, with a re-check for slow CI"
        );

        let gate = node("not-merged");
        assert!(matches!(gate.kind, harness_dag::NodeKind::Cancel(_)));
        assert!(
            gate.when
                .as_deref()
                .is_some_and(|w| w.contains("$merge.output.merged != 'true'")),
            "a merge that did not happen must cancel: {:?}",
            gate.when
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
        for name in [
            "task",
            "task-merge",
            "revise-task",
            "review-pr",
            "architect",
        ] {
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
        for name in ["revise-task", "review-pr"] {
            let yaml = default_workflow(name).unwrap();
            let wf = harness_dag::parse_workflow(yaml).expect("bundled workflow must parse");
            let node = wf
                .nodes
                .iter()
                .find(|n| n.id == "gpt-review-fix")
                .unwrap_or_else(|| panic!("`{name}` has a gpt review node"));
            assert_eq!(node.provider.as_deref(), Some("pi"));
            assert_eq!(node.model.as_deref(), Some("openai-codex/gpt-6-astra"));
        }
    }

    /// Generated sources are gitignored, so a fresh worktree has none, and in a
    /// pnpm monorepo the script that writes them lives in a package, not the
    /// root. The step used to look for it only in the root and then run it with
    /// `pnpm -r`, which skips the root: it ran in neither case, silently, and
    /// two packages generating into one file ran in parallel. Every bundled copy
    /// of the step is the same script, so this pins all of them.
    #[test]
    fn install_deps_is_one_script_everywhere() {
        let bodies: Vec<(&str, String)> = ["task", "revise-task", "review-pr"]
            .into_iter()
            .map(|name| {
                let wf = harness_dag::parse_workflow(default_workflow(name).unwrap()).unwrap();
                let node = wf
                    .nodes
                    .iter()
                    .find(|n| n.id == "install-deps")
                    .unwrap_or_else(|| panic!("`{name}` has an install-deps step"));
                let harness_dag::NodeKind::Bash(body) = &node.kind else {
                    panic!("`{name}` install-deps is a bash step");
                };
                (name, body.replace("\r\n", "\n"))
            })
            .collect();
        for (name, body) in &bodies[1..] {
            assert_eq!(
                body, &bodies[0].1,
                "`{name}` install-deps has drifted from `task`'s"
            );
        }
        let body = &bodies[0].1;
        assert!(body.contains("git ls-files -- package.json '*/package.json'"));
        assert!(body.contains("pnpm -r --workspace-concurrency=1 --if-present run"));
        assert!(body.contains("no prebuild, codegen, generate or db:generate script"));
    }

    /// Runs the step's script against fake workspaces, with stand-in `pnpm` and
    /// `npm` that record how they were called. Skipped where there is no bash.
    #[test]
    fn install_deps_runs_codegen_where_the_script_lives() {
        use std::process::Command;
        // Windows may resolve `bash` to WSL's launcher with no distro behind it,
        // which starts but cannot run anything.
        let usable = Command::new("bash")
            .arg("-c")
            .arg("exit 0")
            .output()
            .is_ok_and(|o| o.status.success());
        if !usable {
            eprintln!("no usable bash on PATH: skipping");
            return;
        }
        let wf = harness_dag::parse_workflow(default_workflow("task").unwrap()).unwrap();
        let harness_dag::NodeKind::Bash(script) = &wf
            .nodes
            .iter()
            .find(|n| n.id == "install-deps")
            .unwrap()
            .kind
        else {
            panic!("install-deps is a bash step");
        };
        let script = script.replace("\r\n", "\n");

        // `lock` picks the package manager; each package is (dir, has prebuild).
        let run = |lock: &str, packages: &[(&str, bool)]| -> (String, String) {
            let dir = tempfile::tempdir().unwrap();
            let root = dir.path();
            let bin = root.join("bin");
            std::fs::create_dir_all(&bin).unwrap();
            std::fs::write(root.join(lock), "").unwrap();
            for pm in ["pnpm", "npm"] {
                std::fs::write(
                    bin.join(pm),
                    format!("#!/bin/sh\necho \"{pm} $* (in ${{PWD##*/}})\" >> \"$CALLS\"\n"),
                )
                .unwrap();
            }
            for (pkg, has) in packages {
                let p = root.join(pkg);
                std::fs::create_dir_all(&p).unwrap();
                let scripts = if *has {
                    r#"{"prebuild":"gen"}"#
                } else {
                    r#"{"build":"b"}"#
                };
                std::fs::write(
                    p.join("package.json"),
                    format!(r#"{{"name":"x","scripts":{scripts}}}"#),
                )
                .unwrap();
            }
            std::fs::write(root.join("install-deps.sh"), &script).unwrap();
            let sh = |cmd: &str| {
                Command::new("bash")
                    .arg("-c")
                    .arg(cmd)
                    .current_dir(root)
                    .env_remove("HARNESS_REPOS")
                    .output()
                    .unwrap()
            };
            sh("git init -q && git add -A");
            let out = sh(
                r#"chmod +x bin/*; export CALLS="$PWD/calls.log"; : > "$CALLS"; PATH="$PWD/bin:$PATH" bash install-deps.sh"#,
            );
            let calls = std::fs::read_to_string(root.join("calls.log")).unwrap_or_default();
            (String::from_utf8_lossy(&out.stdout).into_owned(), calls)
        };

        // The script lives only in a package (ECOM's case): it runs, serially.
        let (log, calls) = run(
            "pnpm-lock.yaml",
            &[(".", false), ("apps/web", true), ("apps/studio", true)],
        );
        assert!(
            log.contains("codegen: prebuild in apps/studio apps/web"),
            "{log}"
        );
        assert!(
            calls.contains("pnpm -r --workspace-concurrency=1 --if-present run prebuild"),
            "{calls}"
        );

        // Only the root, or the root and a package: the root's script runs, once.
        for packages in [
            &[(".", true), ("apps/web", false)][..],
            &[(".", true), ("apps/web", true)][..],
        ] {
            let (_, calls) = run("pnpm-lock.yaml", packages);
            let codegen: Vec<_> = calls.lines().filter(|l| l.contains(" run ")).collect();
            assert_eq!(codegen.len(), 1, "{calls}");
            assert!(codegen[0].starts_with("pnpm run prebuild"), "{calls}");
        }

        // Nobody defines one: it says so, rather than nothing.
        let (log, calls) = run("pnpm-lock.yaml", &[(".", false), ("apps/web", false)]);
        assert!(
            log.contains("no prebuild, codegen, generate or db:generate script"),
            "{log}"
        );
        assert!(!calls.contains(" run "), "{calls}");

        // npm has no `-r`: each package that defines it runs in turn.
        let (_, calls) = run(
            "package-lock.json",
            &[(".", false), ("apps/web", true), ("apps/api", true)],
        );
        assert!(calls.contains("npm run prebuild (in web)"), "{calls}");
        assert!(calls.contains("npm run prebuild (in api)"), "{calls}");
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
    fn revise_task_revalidates_after_review_fixes() {
        let yaml = default_workflow("revise-task").expect("revise-task bundled");
        let wf = harness_dag::parse_workflow(yaml).expect("revise-task must parse");
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
        // A revision starts in a fresh worktree: the PR branch is checked out by
        // a bash step (not an agent asked to remember), and its dependencies
        // installed, before anyone reads the code.
        assert!(matches!(
            node("checkout-pr").kind,
            harness_dag::NodeKind::Bash(_)
        ));
        assert_eq!(deps("checkout-pr"), vec!["start"]);
        assert_eq!(deps("install-deps"), vec!["checkout-pr"]);
        assert_eq!(deps("explore"), vec!["install-deps"]);

        assert_eq!(deps("final-validate"), vec!["anthropic-review-fix"]);
        assert!(
            matches!(&node("final-validate").kind, harness_dag::NodeKind::Command(name) if name == "validate")
        );
        assert!(node("final-validate").output_format.is_some());
        assert_eq!(
            deps("abort-final-invalid"),
            vec!["final-validate", "report-final-invalid"]
        );
        assert_eq!(
            node("abort-final-invalid").when.as_deref(),
            Some("$final-validate.output != '' && $final-validate.output.passed != 'true'")
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
    fn revise_task_aborts_when_neither_feedback_source_has_anything() {
        let yaml = default_workflow("revise-task").expect("revise-task bundled");
        let wf = harness_dag::parse_workflow(yaml).expect("revise-task must parse");
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
                "$gather-feedback.output != '' && \
                 $gather-feedback.output.has_github_feedback != 'true' && \
                 $gather-feedback.output.has_linear_feedback != 'true'"
            )
        );
        // The issue is told why nothing happened, and is not moved.
        let report = wf
            .nodes
            .iter()
            .find(|n| n.id == "report-no-feedback")
            .expect("report-no-feedback exists");
        assert!(matches!(report.kind, harness_dag::NodeKind::Linear(_)));
        assert!(
            wf.nodes
                .iter()
                .find(|n| n.id == "start")
                .expect("start exists")
                .when
                .as_deref()
                .is_some_and(|w| w.contains("== 'true'")),
            "the issue moves to In Progress only once there is real feedback"
        );

        // The prompt has to say that the text before the Linear heading is the
        // original report, or the same conflation is one paraphrase away.
        let harness_dag::NodeKind::Prompt(prompt) = &gather.kind else {
            panic!("gather-feedback is an inline prompt");
        };
        assert!(prompt.contains("If that heading is absent there is no Linear feedback"));
        assert!(prompt.contains("ORIGINAL bug report"));
        // The harness's own comments are recognised by the poller's mark, not
        // by an app name that differs per install.
        assert!(prompt.contains("[harness]"));
        assert!(!prompt.contains("AI Harness"));
    }

    #[test]
    fn resolve_falls_back_to_bundled_default() {
        let tmp = std::env::temp_dir();
        let (yaml, label) = resolve_workflow_source(DEFAULT_WORKFLOW, &tmp).unwrap();
        assert_eq!(label, DEFAULT_WORKFLOW);
        assert_eq!(yaml.lines().next(), Some("name: task"));
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
    fn learning_facts_is_internal_and_answers_in_the_shape_the_server_reads() {
        let wf =
            harness_dag::parse_workflow(default_workflow(FACTS_WORKFLOW).unwrap()).expect("parses");
        assert!(wf.internal, "hidden from every workflow list");
        assert!(INTERNAL_WORKFLOWS.contains(&FACTS_WORKFLOW));
        let evidence = wf.node("evidence").expect("fetches the failures first");
        assert!(
            matches!(&evidence.kind, harness_dag::NodeKind::Bash(b) if b.contains("/api/run/facts/evidence"))
        );
        // The server reads the `learn` step's `facts[].failures` labels.
        let learn = wf.node("learn").expect("learn step");
        let schema = learn.output_format.as_ref().expect("structured answer");
        assert!(schema["properties"]["facts"]["items"]["properties"]["failures"].is_object());
    }

    #[test]
    fn the_improvements_review_is_internal_and_reads_its_evidence() {
        let wf = harness_dag::parse_workflow(default_workflow(IMPROVEMENTS_WORKFLOW).unwrap())
            .expect("parses");
        assert!(wf.internal, "hidden from every workflow list");
        assert!(wf.improvements.is_none(), "it must not review itself");
        assert!(wf.node("evidence").is_some(), "fetches the runs first");
        let suggest = wf.node("suggest").expect("suggest step");
        assert!(suggest.output_format.is_some());
        assert_eq!(suggest.depends_on, vec!["evidence".to_string()]);
    }

    #[test]
    fn every_bundled_workflow_parses() {
        for (name, yaml) in WORKFLOWS {
            let wf = harness_dag::parse_workflow(yaml)
                .unwrap_or_else(|e| panic!("bundled workflow `{name}` failed to parse: {e}"));
            assert!(!wf.nodes.is_empty(), "`{name}` has no nodes");
        }
        // The default, and the Linear lifecycle around it, are all present.
        let names = list_default_workflows();
        assert!(names.contains(&DEFAULT_WORKFLOW));
        assert!(names.contains(&"architect"));
        assert!(names.contains(&"revise-task"));
        assert!(names.contains(&"task"));
        assert!(names.contains(&"task-merge"));
    }

    /// Plays `task-merge` against scripted outputs, as if from a Linear issue.
    struct Scripted {
        outputs: std::collections::HashMap<&'static str, &'static str>,
        calls: std::sync::Mutex<Vec<(String, String)>>,
    }

    impl Scripted {
        fn new(outputs: &[(&'static str, &'static str)]) -> Self {
            Self {
                outputs: outputs.iter().copied().collect(),
                calls: Default::default(),
            }
        }
        /// The Linear actions taken, in order.
        fn linear(&self) -> Vec<String> {
            self.calls
                .lock()
                .unwrap()
                .iter()
                .filter(|(_, body)| body.starts_with("linear:"))
                .map(|(_, body)| body.clone())
                .collect()
        }
        fn ran(&self, id: &str) -> bool {
            self.calls.lock().unwrap().iter().any(|(n, _)| n == id)
        }
    }

    #[async_trait::async_trait]
    impl harness_dag::NodeRunner for Scripted {
        async fn execute(
            &self,
            req: harness_dag::NodeRequest<'_>,
        ) -> Result<harness_dag::NodeOutput, harness_dag::RunnerError> {
            let body = match &req.body {
                harness_dag::NodeBody::Linear(a) => format!("linear:{}", a.summary()),
                other => format!("{other:?}"),
            };
            self.calls
                .lock()
                .unwrap()
                .push((req.node_id.to_string(), body));
            // `FAIL:` scripts a step that fails, with the rest as its output.
            let out = self.outputs.get(req.node_id).copied().unwrap_or("");
            let (success, text) = match out.strip_prefix("FAIL:") {
                Some(rest) => (false, rest),
                None => (true, out),
            };
            Ok(harness_dag::NodeOutput {
                text: text.to_string(),
                success,
                ..Default::default()
            })
        }
    }

    async fn run_task_merge(
        outputs: &[(&'static str, &'static str)],
    ) -> (harness_dag::RunReport, Scripted) {
        let wf = harness_dag::parse_workflow(default_workflow("task-merge").unwrap()).unwrap();
        let runner = Scripted::new(outputs);
        let vars = harness_dag::VarContext::new()
            .set("ARGUMENTS", "AIH-7")
            .set("ISSUE_ID", "issue-uuid");
        let report = harness_dag::run_workflow(&wf, &runner, &vars)
            .await
            .unwrap();
        (report, runner)
    }

    async fn run_bundled(
        name: &str,
        outputs: &[(&'static str, &'static str)],
    ) -> (harness_dag::RunReport, Scripted) {
        let wf = harness_dag::parse_workflow(default_workflow(name).unwrap()).unwrap();
        let runner = Scripted::new(outputs);
        let vars = harness_dag::VarContext::new()
            .set("ARGUMENTS", "NIL-10")
            .set("ISSUE_ID", "issue-uuid")
            .set("TASK_TITLE", "NIL-10")
            .set("ARTIFACTS_DIR", "/tmp/artifacts")
            .set("BASE_BRANCH", "main")
            .set("HARNESS_REPOS", "");
        let report = harness_dag::run_workflow(&wf, &runner, &vars)
            .await
            .unwrap();
        (report, runner)
    }

    /// A step that fails before `validate` ends the run *failed*, on that
    /// step. `task-1794075518503` instead ended cancelled with "validation did
    /// not return a pass": its first step failed, `validate` was skipped, and
    /// the `all_done` gate read the skipped step's empty output as a failed
    /// verdict — hiding the real cause.
    #[tokio::test]
    async fn a_failure_before_validation_is_not_reported_as_a_failed_validation() {
        {
            let name = "task";
            let wf = harness_dag::parse_workflow(default_workflow(name).unwrap()).unwrap();
            let roots: Vec<&'static str> = wf
                .nodes
                .iter()
                .filter(|n| n.depends_on.is_empty())
                .map(|n| &*Box::leak(n.id.clone().into_boxed_str()))
                .collect();
            let fails: Vec<(&'static str, &'static str)> =
                roots.iter().map(|id| (*id, "FAIL:boom")).collect();
            let (report, runner) = run_bundled(name, &fails).await;
            assert_eq!(report.status, harness_dag::RunStatus::Failed, "{name}");
            assert!(!runner.ran("validate"), "{name}");
            let gate = report.node("abort-on-invalid").expect("gate");
            assert_eq!(gate.status, harness_dag::NodeStatus::Skipped, "{name}");
        }
    }

    /// The gate still does its job once `validate` has run: a failed verdict
    /// cancels, and so does one that can't be read.
    #[tokio::test]
    async fn task_still_cancels_on_a_failed_or_unreadable_verdict() {
        for verdict in [r#"{"passed":false,"summary":"tests red"}"#, "no json here"] {
            let (report, runner) = run_bundled("task", &[("validate", verdict)]).await;
            assert_eq!(
                report.status,
                harness_dag::RunStatus::Cancelled,
                "{verdict}"
            );
            assert!(!runner.ran("finalize-pr"), "{verdict}");
        }
        let (report, runner) = run_bundled(
            "task",
            &[
                ("validate", r#"{"passed":true}"#),
                ("pi-review-fix-loop", "<promise>PI_REVIEW_CLEAN</promise>"),
                ("final-verify-loop", "<promise>FINAL_VERIFY_CLEAN</promise>"),
            ],
        )
        .await;
        assert_eq!(report.status, harness_dag::RunStatus::Completed);
        assert!(runner.ran("finalize-pr"));
    }

    // ── `task` runs epics itself ────────────────────────────────────────────

    const BUILT: [(&str, &str); 3] = [
        ("validate", r#"{"passed":true}"#),
        ("pi-review-fix-loop", "<promise>PI_REVIEW_CLEAN</promise>"),
        ("final-verify-loop", "<promise>FINAL_VERIFY_CLEAN</promise>"),
    ];
    const PIECE: &str =
        r#"{"mode":"piece","parent":"epic-uuid","identifier":"NIL-11","epic_ident":"NIL-10"}"#;

    async fn run_task(
        extra: &[(&'static str, &'static str)],
    ) -> (harness_dag::RunReport, Scripted) {
        let mut outputs: Vec<(&'static str, &'static str)> = BUILT.to_vec();
        outputs.extend_from_slice(extra);
        run_bundled("task", &outputs).await
    }

    fn moves(runner: &Scripted) -> Vec<String> {
        runner
            .linear()
            .into_iter()
            .filter_map(|l| {
                l.strip_prefix("linear:move the issue to ")
                    .map(str::to_string)
            })
            .collect()
    }

    #[test]
    fn task_says_it_runs_epics_itself() {
        let wf = harness_dag::parse_workflow(default_workflow("task").unwrap()).unwrap();
        assert!(
            wf.handles_epics,
            "or the server hands its epics to the supervisor"
        );
    }

    #[tokio::test]
    async fn an_ordinary_issue_is_built_and_sent_to_review_as_before() {
        let (report, runner) = run_task(&[("classify", r#"{"mode":"task"}"#)]).await;
        assert_eq!(report.status, harness_dag::RunStatus::Completed);
        assert!(runner.ran("finalize-pr"));
        assert_eq!(moves(&runner), ["In Progress", "In Review"]);
        for piece_only in ["grade", "merge-piece", "advance", "epic-start"] {
            assert!(!runner.ran(piece_only), "{piece_only}");
        }
    }

    #[tokio::test]
    async fn an_epic_is_started_not_built() {
        let (report, runner) = run_task(&[(
            "classify",
            r#"{"mode":"epic","first_id":"p1","first_ident":"NIL-11","epic_ident":"NIL-10","pieces":3}"#,
        )])
        .await;
        assert_eq!(report.status, harness_dag::RunStatus::Completed);
        assert!(runner.ran("epic-start"));
        for build in ["ack", "explore", "validate", "finalize-pr", "grade"] {
            assert!(!runner.ran(build), "{build}");
        }
        assert_eq!(
            moves(&runner),
            ["In Progress"],
            "the epic itself is under way"
        );
    }

    #[tokio::test]
    async fn an_epic_with_nothing_left_to_start_is_cancelled() {
        let (report, runner) = run_task(&[("classify", r#"{"mode":"skip"}"#)]).await;
        assert_eq!(report.status, harness_dag::RunStatus::Cancelled);
        assert!(!runner.ran("explore"));
    }

    #[tokio::test]
    async fn a_piece_that_passes_is_merged_marked_done_and_the_next_started() {
        let (report, runner) = run_task(&[
            ("classify", PIECE),
            ("grade", r#"{"passed":true,"summary":"ok"}"#),
        ])
        .await;
        assert_eq!(report.status, harness_dag::RunStatus::Completed);
        assert!(runner.ran("merge-piece") && runner.ran("advance"));
        assert!(!runner.ran("fix-shortfall"));
        assert_eq!(
            moves(&runner),
            ["In Progress", "Done"],
            "a piece's PR is into the epic branch: no In Review"
        );
    }

    #[tokio::test]
    async fn a_piece_that_falls_short_gets_one_round_of_fixes() {
        let (report, runner) = run_task(&[
            ("classify", PIECE),
            ("grade", r#"{"passed":false,"summary":"no"}"#),
            ("regrade", r#"{"passed":true,"summary":"ok now"}"#),
        ])
        .await;
        assert_eq!(report.status, harness_dag::RunStatus::Completed);
        assert!(runner.ran("fix-shortfall") && runner.ran("regrade"));
        assert!(runner.ran("merge-piece") && runner.ran("advance"));
    }

    #[tokio::test]
    async fn a_piece_that_fails_twice_stops_for_a_person_unmerged() {
        let (report, runner) = run_task(&[
            ("classify", PIECE),
            ("grade", r#"{"passed":false,"summary":"no"}"#),
            ("regrade", r#"{"passed":false,"summary":"still no"}"#),
        ])
        .await;
        assert_eq!(report.status, harness_dag::RunStatus::Completed);
        assert!(!runner.ran("merge-piece") && !runner.ran("advance"));
        assert!(runner.ran("report-stuck"));
        assert_eq!(moves(&runner), ["In Progress", "In Review"]);
    }

    #[tokio::test]
    async fn a_piece_github_will_not_merge_stops_for_a_person_and_says_why() {
        // NIL-73: graded and passed, but its PR was into a branch that
        // requires a review. The run failed and nothing on the board moved.
        let (report, runner) = run_task(&[
            ("classify", PIECE),
            ("grade", r#"{"passed":true,"summary":"ok"}"#),
            ("merge-piece", "FAIL:base branch policy prohibits the merge"),
        ])
        .await;
        assert_eq!(report.status, harness_dag::RunStatus::Failed);
        assert!(runner.ran("merge-refused") && runner.ran("report-merge-refused"));
        assert!(!runner.ran("piece-done") && !runner.ran("advance"));
        assert_eq!(moves(&runner), ["In Progress", "In Review"]);
    }

    #[tokio::test]
    async fn a_merged_piece_reports_no_refusal() {
        let (_, runner) = run_task(&[
            ("classify", PIECE),
            ("grade", r#"{"passed":true,"summary":"ok"}"#),
        ])
        .await;
        assert!(!runner.ran("merge-refused") && !runner.ran("report-merge-refused"));
    }

    /// The base a PR must target is the run's. `.pr-list` is written by the
    /// agent that opened the PR, so checking against it only confirms the
    /// agent's own choice, which is how a piece of NIL-72 went to
    /// `development` instead of `epic/NIL-72`.
    #[test]
    fn verify_pr_base_checks_against_the_runs_base_not_the_agents() {
        {
            let name = "task";
            let wf = harness_dag::parse_workflow(default_workflow(name).unwrap()).unwrap();
            let verify = wf
                .nodes
                .iter()
                .find(|n| n.id == "verify-pr-base")
                .and_then(|n| match &n.kind {
                    harness_dag::NodeKind::Bash(b) => Some(b.as_str()),
                    _ => None,
                })
                .expect("verify-pr-base is a bash node");
            assert!(
                !verify.contains(".base'"),
                "{name} reads the base from .pr-list"
            );
            assert!(verify.contains("intended_base"), "{name}");
        }
    }

    const OPEN_PR: &str = r#"{"pr":"42","url":"https://github.com/acme/shop/pull/42","base":"main","title":"t","state":"OPEN","mergeable":"MERGEABLE"}"#;

    /// In a multi-repo project the workspace root is not a git repo, so
    /// `gh pr view 42` fails there ("not a git repository") before it reaches
    /// GitHub. The review check must name the PR by its URL, which works from
    /// any folder.
    #[tokio::test]
    async fn task_merge_reads_the_review_by_pr_url() {
        let (_, runner) = run_task_merge(&[
            ("resolve-pr", OPEN_PR),
            ("review", r#"{"approved":true,"decision":"APPROVED"}"#),
            ("merge", r#"{"merged":true,"summary":"merged"}"#),
        ])
        .await;
        let calls = runner.calls.lock().unwrap();
        let review = calls
            .iter()
            .find(|(id, _)| id == "review")
            .map(|(_, body)| body.as_str())
            .expect("review ran");
        assert!(
            review.contains("gh pr view \\\"https://github.com/acme/shop/pull/42\\\""),
            "{review}"
        );
    }

    /// "Ready for merge" with no approved review is not a failed run: the issue
    /// goes back to In Review and says why, and nothing is merged.
    #[tokio::test]
    async fn task_merge_sends_an_unapproved_pr_back_to_review_without_failing() {
        let (report, runner) = run_task_merge(&[
            ("resolve-pr", OPEN_PR),
            (
                "review",
                r#"{"approved":false,"decision":"REVIEW_REQUIRED"}"#,
            ),
        ])
        .await;
        assert_eq!(report.status, harness_dag::RunStatus::Completed);
        assert!(!runner.ran("merge"), "an unapproved PR is never merged");
        let linear = runner.linear();
        assert_eq!(linear.len(), 2, "{linear:?}");
        assert!(linear[0].contains("In Review"), "{linear:?}");
        assert!(linear[1].starts_with("linear:comment"), "{linear:?}");
    }

    #[tokio::test]
    async fn task_merge_merges_an_approved_pr_and_moves_the_issue_to_done() {
        let (report, runner) = run_task_merge(&[
            ("resolve-pr", OPEN_PR),
            ("review", r#"{"approved":true,"decision":"APPROVED"}"#),
            ("merge", r#"{"merged":true,"summary":"merged"}"#),
        ])
        .await;
        assert_eq!(report.status, harness_dag::RunStatus::Completed);
        assert!(runner.ran("merge"));
        assert!(!runner.ran("resolve-conflicts"), "no conflicts to resolve");
        let linear = runner.linear();
        assert_eq!(linear.len(), 1, "{linear:?}");
        assert!(linear[0].contains("Done"), "{linear:?}");
    }

    /// Approved but refused (a red check): reported on the issue, and the run
    /// cancels so the issue stays where it is.
    #[tokio::test]
    async fn task_merge_reports_a_refused_merge_and_cancels() {
        let (report, runner) = run_task_merge(&[
            ("resolve-pr", OPEN_PR),
            ("review", r#"{"approved":true,"decision":"APPROVED"}"#),
            ("merge", r#"{"merged":false,"summary":"check `ci` failed"}"#),
        ])
        .await;
        assert_eq!(report.status, harness_dag::RunStatus::Cancelled);
        let linear = runner.linear();
        assert_eq!(linear.len(), 1, "{linear:?}");
        assert!(linear[0].starts_with("linear:comment"), "{linear:?}");
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
