//! Built-in default workflows and commands, compiled into the binary so a fresh
//! project gets the standard pipeline without copying any files — the way Archon
//! ships `.archon/{workflows,commands}/defaults`.
//!
//! Resolution order is **project-first**: a project's
//! `.harness/workflows/<name>.yaml` or `.harness/commands/<name>.md` shadows the
//! bundled default of the same name (see [`resolve_workflow_source`] and
//! [`crate::LocalRunner`]'s command resolution).

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
/// YAML source, project-first.
///
/// 1. An existing file path is read directly.
/// 2. Otherwise the name resolves to `<project_root>/.harness/workflows/<name>.yaml`.
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

    // 2. Project-local workflow by name.
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
        "workflow `{trimmed}` not found (not a file, no project .harness/workflows/{trimmed}.yaml, \
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
        assert_eq!(node.model.as_deref(), Some("openai-codex/gpt-5.5"));
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
            Some("$gather-feedback.output.has_feedback == 'true'")
        );
        assert_eq!(deps("final-validate"), vec!["sonnet-final-review"]);
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
}
