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
        "geo-audit",
        include_str!("../defaults/workflows/geo-audit.yaml"),
    ),
    (
        "bc-idea-to-pr",
        include_str!("../defaults/workflows/bc-idea-to-pr.yaml"),
    ),
    (
        "review-area",
        include_str!("../defaults/workflows/review-area.yaml"),
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
        assert_eq!(node.model.as_deref(), Some("openai-codex/gpt-5.6-sol"));
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
        assert_eq!(deps("final-validate"), vec!["sonnet-review-fix"]);
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

    #[test]
    fn geo_audit_workflow_discovers_then_scores() {
        let yaml = default_workflow("geo-audit").expect("geo-audit bundled");
        let wf = harness_dag::parse_workflow(yaml).expect("geo-audit must parse");
        assert_eq!(wf.name, "geo-audit");
        // A deterministic fetch step the analysis depends on.
        let discover = wf
            .nodes
            .iter()
            .find(|n| n.id == "discover")
            .expect("discover node exists");
        assert!(matches!(discover.kind, harness_dag::NodeKind::Bash(_)));
        // A store's homepage carries no Product schema, no price and no reviews,
        // so the audit samples a real PDP and PLP before scoring anything: the
        // picker reads the sitemap, then a deterministic node fetches its choices.
        let pick = wf
            .nodes
            .iter()
            .find(|n| n.id == "pick-pages")
            .expect("pick-pages node exists");
        assert_eq!(pick.depends_on, vec!["discover".to_string()]);
        assert!(pick.output_format.is_some(), "pick-pages returns two URLs");
        // Source order matters: llms.txt is curated and carries descriptions, so
        // it identifies a category without guessing at URL shapes; the sitemap is
        // where individual products actually live; a product link off the category
        // page is the fallback for a store whose PDPs aren't in the sitemap.
        let pick_prompt = match &pick.kind {
            harness_dag::NodeKind::Prompt(p) => p.as_str(),
            other => panic!("pick-pages is not an inline prompt: {other:?}"),
        };
        let order = [
            "llms.txt",
            "sitemap-urls.txt",
            "page.html",
            "One targeted fetch",
        ];
        let mut at = 0usize;
        for source in order {
            let found = pick_prompt[at..]
                .find(source)
                .unwrap_or_else(|| panic!("pick-pages lost source `{source}` (or its order)"));
            at += found + source.len();
        }
        let fetch = wf
            .nodes
            .iter()
            .find(|n| n.id == "fetch-pages")
            .expect("fetch-pages node exists");
        assert_eq!(fetch.depends_on, vec!["pick-pages".to_string()]);
        assert!(matches!(fetch.kind, harness_dag::NodeKind::Bash(_)));

        // Five dimension analyses fan out from the sampled pages (parallel), each
        // scoring one dimension; they share an output schema (a YAML anchor).
        let dims = ["technical", "crawlers", "schema", "content", "entity"];
        for dim in dims {
            let n = wf
                .nodes
                .iter()
                .find(|n| n.id == dim)
                .unwrap_or_else(|| panic!("dimension node `{dim}` exists"));
            assert_eq!(n.depends_on, vec!["fetch-pages".to_string()]);
            assert!(
                n.output_format.is_some(),
                "{dim} must emit a structured score"
            );
        }
        // Synthesis joins all five and emits the composite verdict.
        let synth = wf
            .nodes
            .iter()
            .find(|n| n.id == "synthesize")
            .expect("synthesize node exists");
        assert_eq!(synth.depends_on, dims);
        assert!(
            synth.output_format.is_some(),
            "synthesize must emit the structured GEO verdict"
        );
    }

    /// The GEO audit is scored for ecommerce, and two of its prompts carry
    /// corrections that a well-meaning reword would quietly undo.
    #[test]
    fn geo_audit_is_ecommerce_scored_and_grades_crawlers_by_purpose() {
        let yaml = default_workflow("geo-audit").expect("geo-audit bundled");
        let wf = harness_dag::parse_workflow(yaml).expect("geo-audit must parse");
        let prompt = |id: &str| match &wf
            .nodes
            .iter()
            .find(|n| n.id == id)
            .unwrap_or_else(|| panic!("node `{id}`"))
            .kind
        {
            harness_dag::NodeKind::Prompt(p) => p.clone(),
            other => panic!("node `{id}` is not an inline prompt: {other:?}"),
        };

        // Blocking a training crawler is a business decision, not a defect. The
        // audit used to call any AI-bot Disallow critical, which turned a
        // deliberate opt-out into a finding somebody would "fix".
        let crawlers = prompt("crawlers");
        for needle in [
            "Citation crawlers",
            "Training / grounding opt-outs",
            "User-triggered fetchers",
            "ignore robots.txt by design",
            "never propose \"fixing\" it",
        ] {
            assert!(crawlers.contains(needle), "crawlers prompt lost: {needle}");
        }
        // llms.txt stays a priority here by choice — but the report has to say why
        // honestly, since Google Search documents that it ignores the file.
        assert!(crawlers.contains("priority quick win"));
        assert!(crawlers.contains("Google Search **ignores** these files"));
        assert!(crawlers.contains("Do not claim a measured citation effect"));

        // The commerce signals that only exist on a product page.
        let schema = prompt("schema");
        for needle in [
            "`name`, `image`, `offers`",
            "AggregateOffer",
            "ProductGroup",
        ] {
            assert!(schema.contains(needle), "schema prompt lost: {needle}");
        }
        // Citability anchors, and the guard against turning them into a word count.
        let content = prompt("content");
        assert!(content.contains("134–167 words"));
        assert!(content.contains("never \"make this 150 words\""));

        // Weights must cover the five dimensions and sum to 1.0. Asserted per
        // token, because the prompt is a YAML block scalar and any reflow moves
        // the line breaks.
        let synth = prompt("synthesize");
        let weights = [
            ("technical", 0.20),
            ("crawlers", 0.20),
            ("schema", 0.25),
            ("content", 0.20),
            ("entity", 0.15),
        ];
        for (dim, w) in weights {
            let token = format!("{dim}×{w:.2}");
            assert!(synth.contains(&token), "synthesize prompt lost: {token}");
        }
        let total: f64 = weights.iter().map(|(_, w)| w).sum();
        assert!(
            (total - 1.0).abs() < 1e-9,
            "weights sum to {total}, not 1.0"
        );
        // Per-platform readiness, because one composite hides which surface fails.
        for surface in ["AI Overviews", "AI Mode", "ChatGPT", "Perplexity"] {
            assert!(synth.contains(surface), "synthesize prompt lost: {surface}");
        }
    }

    /// Two shell traps the GEO audit's measured signals died on once. Both are
    /// silent — they yield a plausible number rather than an error, so the audit
    /// would keep scoring confidently off a figure capped at 1 or a preference
    /// that never applied.
    #[test]
    fn geo_audit_measurements_survive_minified_html() {
        let yaml = default_workflow("geo-audit").expect("geo-audit bundled");
        let wf = harness_dag::parse_workflow(yaml).expect("geo-audit must parse");
        let bash: String = wf
            .nodes
            .iter()
            .filter_map(|n| match &n.kind {
                harness_dag::NodeKind::Bash(b) => Some(b.as_str()),
                _ => None,
            })
            .collect::<Vec<_>>()
            .join("\n");

        // `grep -c` counts matching LINES. Minified store HTML and this
        // workflow's own extracted text are each a single line, so every count
        // would cap at 1 — silently turning "12 images, 3 with alt" into "1, 1".
        for banned in ["grep -Eoc", "grep -ci", "grep -oc", "grep -co"] {
            assert!(
                !bash.contains(banned),
                "`{banned}` counts lines, not matches — pipe `grep -Eo` into `wc -l`"
            );
        }
        assert!(bash.contains("| wc -l"), "counts must come from wc -l");

        // The sitemap-index preference ranks product children first. The word
        // "sitemap" contains the substring "item", so an `item` alternative
        // matches every child and the ranking quietly becomes a no-op.
        let ranking = bash
            .lines()
            .find(|l| l.contains("produkt|product"))
            .expect("sitemap child ranking exists");
        assert!(
            !ranking.contains("item"),
            "`item` matches every sitemap URL: {ranking}"
        );
    }
}
