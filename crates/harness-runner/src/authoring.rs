//! Workflow **authoring** core — the shared logic behind the Phase 4.5 visual
//! editor and the Phase 4.6 MCP server, so both front-ends behave identically.
//!
//! It does four things, all over the same [`harness_dag`] model the executor
//! uses: **list** workflows (bundled + project), **get** one's source, **validate**
//! a candidate YAML (via [`harness_dag::parse_workflow`], surfacing cycle/dep/body
//! errors), **save** it to the project, and expose a **catalog** of building
//! blocks (node kinds, provider/model hints, available commands) so an editor or
//! an AI knows what it may use.
//!
//! Resolution is project-first, matching [`crate::defaults`]: a project's
//! `.harness/workflows/<name>.yaml` shadows a bundled default of the same name.

use std::path::Path;

use harness_dag::{parse_workflow, NodeKind, Workflow};
use serde::{Deserialize, Serialize};

use crate::defaults;

/// Where a workflow or command came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Source {
    /// Compiled into the binary.
    Bundled,
    /// A file under the project's `.harness/`.
    Project,
}

/// A workflow as it appears in a listing.
#[derive(Debug, Clone, Serialize)]
pub struct WorkflowSummary {
    pub name: String,
    pub source: Source,
    pub description: Option<String>,
    pub node_count: usize,
}

/// A workflow's editable source.
#[derive(Debug, Clone, Serialize)]
pub struct WorkflowSource {
    pub name: String,
    pub source: Source,
    pub yaml: String,
}

/// A node, distilled for quick feedback in the editor (id + kind + edges).
#[derive(Debug, Clone, Serialize)]
pub struct NodeSummary {
    pub id: String,
    pub kind: String,
    pub depends_on: Vec<String>,
}

/// The result of validating a candidate workflow YAML.
#[derive(Debug, Clone, Serialize)]
pub struct ValidationResult {
    pub valid: bool,
    /// The first structural error (cycle, unknown dep, bad/duplicate body, …).
    pub error: Option<String>,
    /// Node summaries when valid (empty otherwise) — drives the canvas preview.
    pub nodes: Vec<NodeSummary>,
}

/// A request to validate or save a workflow.
#[derive(Debug, Clone, Deserialize)]
pub struct WorkflowYaml {
    pub yaml: String,
}

/// A request to save a named workflow.
#[derive(Debug, Clone, Deserialize)]
pub struct SaveWorkflow {
    pub name: String,
    pub yaml: String,
}

/// One palette building block.
#[derive(Debug, Clone, Serialize)]
pub struct NodeKindInfo {
    /// The YAML body key (`prompt`/`bash`/`command`/`loop`/`script`).
    pub kind: &'static str,
    /// Friendly palette label.
    pub label: &'static str,
    pub description: &'static str,
    /// Whether the body is executed by an AI provider (vs deterministic).
    pub ai: bool,
}

/// Provider + suggested models (hints; any model string is accepted).
#[derive(Debug, Clone, Serialize)]
pub struct ProviderInfo {
    pub id: &'static str,
    pub label: &'static str,
    pub models: Vec<&'static str>,
}

/// A command available to `command:` nodes.
#[derive(Debug, Clone, Serialize)]
pub struct CommandInfo {
    pub name: String,
    pub source: Source,
}

/// The building-blocks catalog an editor/AI uses to author a workflow.
#[derive(Debug, Clone, Serialize)]
pub struct Catalog {
    pub node_kinds: Vec<NodeKindInfo>,
    pub providers: Vec<ProviderInfo>,
    pub commands: Vec<CommandInfo>,
    pub context_modes: Vec<&'static str>,
    pub trigger_rules: Vec<&'static str>,
}

/// The YAML body key for a node kind (mirrors the parser's discriminators).
pub fn kind_label(kind: &NodeKind) -> &'static str {
    match kind {
        NodeKind::Prompt(_) => "prompt",
        NodeKind::Bash(_) => "bash",
        NodeKind::Command(_) => "command",
        NodeKind::Script { .. } => "script",
        NodeKind::Loop(_) => "loop",
        NodeKind::Approval(_) => "approval",
        NodeKind::Cancel(_) => "cancel",
    }
}

fn summarize(wf: &Workflow) -> Vec<NodeSummary> {
    wf.nodes
        .iter()
        .map(|n| NodeSummary {
            id: n.id.clone(),
            kind: kind_label(&n.kind).to_string(),
            depends_on: n.depends_on.clone(),
        })
        .collect()
}

/// Parse + validate a candidate workflow YAML, returning structural errors
/// (cycle, unknown dependency, zero/multiple bodies, duplicate id) rather than
/// throwing — the build→validate→fix loop for the editor and MCP server.
pub fn validate_workflow(yaml: &str) -> ValidationResult {
    match parse_workflow(yaml) {
        Ok(wf) => {
            // parse_workflow checks bodies + dependency references; also run the
            // cycle check so the editor catches a bad edge before save/run.
            if let Err(e) = harness_dag::topological_layers(&wf) {
                return ValidationResult {
                    valid: false,
                    error: Some(e.to_string()),
                    nodes: Vec::new(),
                };
            }
            ValidationResult {
                valid: true,
                error: None,
                nodes: summarize(&wf),
            }
        }
        Err(e) => ValidationResult {
            valid: false,
            error: Some(e.to_string()),
            nodes: Vec::new(),
        },
    }
}

/// List workflows available to a project: bundled defaults plus any under
/// `<project_root>/.harness/workflows/*.yaml`. A project workflow shadows a
/// bundled one of the same name (reported as `Project`).
pub fn list_workflows(project_root: &Path) -> Vec<WorkflowSummary> {
    let mut out: Vec<WorkflowSummary> = Vec::new();
    let mut seen: std::collections::HashSet<String> = std::collections::HashSet::new();

    let dir = project_root.join(".harness").join("workflows");
    if let Ok(entries) = std::fs::read_dir(&dir) {
        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension().and_then(|e| e.to_str()) != Some("yaml") {
                continue;
            }
            let Some(stem) = path.file_stem().and_then(|s| s.to_str()) else {
                continue;
            };
            if let Ok(yaml) = std::fs::read_to_string(&path) {
                if let Ok(wf) = parse_workflow(&yaml) {
                    seen.insert(stem.to_string());
                    out.push(WorkflowSummary {
                        name: stem.to_string(),
                        source: Source::Project,
                        description: wf.description.clone(),
                        node_count: wf.nodes.len(),
                    });
                }
            }
        }
    }

    for name in defaults::list_default_workflows() {
        if seen.contains(name) {
            continue; // shadowed by a project workflow
        }
        if let Some(yaml) = defaults::default_workflow(name) {
            if let Ok(wf) = parse_workflow(yaml) {
                out.push(WorkflowSummary {
                    name: name.to_string(),
                    source: Source::Bundled,
                    description: wf.description.clone(),
                    node_count: wf.nodes.len(),
                });
            }
        }
    }

    out.sort_by(|a, b| a.name.cmp(&b.name));
    out
}

/// Fetch a workflow's editable source by name (project shadows bundled).
pub fn get_workflow(project_root: &Path, name: &str) -> Result<WorkflowSource, String> {
    let project_file = project_root
        .join(".harness")
        .join("workflows")
        .join(format!("{name}.yaml"));
    if project_file.is_file() {
        let yaml = std::fs::read_to_string(&project_file)
            .map_err(|e| format!("failed to read workflow {name}: {e}"))?;
        return Ok(WorkflowSource {
            name: name.to_string(),
            source: Source::Project,
            yaml,
        });
    }
    if let Some(yaml) = defaults::default_workflow(name) {
        return Ok(WorkflowSource {
            name: name.to_string(),
            source: Source::Bundled,
            yaml: yaml.to_string(),
        });
    }
    Err(format!("workflow `{name}` not found"))
}

/// A workflow/command name safe to use as a file stem (no traversal).
fn is_safe_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 128
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'))
        && name != "."
        && name != ".."
}

/// Validate then save a workflow to `<project_root>/.harness/workflows/<name>.yaml`.
/// Rejects an invalid workflow (so the editor can never persist a broken DAG) and
/// unsafe names.
pub fn save_workflow(project_root: &Path, name: &str, yaml: &str) -> Result<(), String> {
    if !is_safe_name(name) {
        return Err(format!(
            "invalid workflow name `{name}` (use letters, digits, `-`, `_`, `.`)"
        ));
    }
    let result = validate_workflow(yaml);
    if !result.valid {
        return Err(format!(
            "refusing to save invalid workflow: {}",
            result.error.unwrap_or_else(|| "unknown error".into())
        ));
    }
    let dir = project_root.join(".harness").join("workflows");
    std::fs::create_dir_all(&dir)
        .map_err(|e| format!("failed to create {}: {e}", dir.display()))?;
    let path = dir.join(format!("{name}.yaml"));
    std::fs::write(&path, yaml).map_err(|e| format!("failed to write {}: {e}", path.display()))?;
    Ok(())
}

/// List commands available to `command:` nodes: bundled defaults plus project
/// `.harness/commands/*.md` (project shadows bundled).
pub fn list_commands(project_root: &Path) -> Vec<CommandInfo> {
    let mut out: Vec<CommandInfo> = Vec::new();
    let mut seen: std::collections::HashSet<String> = std::collections::HashSet::new();

    let dir = project_root.join(".harness").join("commands");
    if let Ok(entries) = std::fs::read_dir(&dir) {
        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension().and_then(|e| e.to_str()) != Some("md") {
                continue;
            }
            if let Some(stem) = path.file_stem().and_then(|s| s.to_str()) {
                seen.insert(stem.to_string());
                out.push(CommandInfo {
                    name: stem.to_string(),
                    source: Source::Project,
                });
            }
        }
    }
    for name in defaults::default_command_names() {
        if !seen.contains(name) {
            out.push(CommandInfo {
                name: name.to_string(),
                source: Source::Bundled,
            });
        }
    }
    out.sort_by(|a, b| a.name.cmp(&b.name));
    out
}

/// The building-blocks catalog: the editor palette + the drawer's option lists.
pub fn catalog(project_root: &Path) -> Catalog {
    Catalog {
        node_kinds: vec![
            NodeKindInfo {
                kind: "prompt",
                label: "Agent step",
                description: "An inline AI prompt run by the node's provider/model.",
                ai: true,
            },
            NodeKindInfo {
                kind: "command",
                label: "Command",
                description: "Run a bundled or project command (.harness/commands/<name>.md).",
                ai: true,
            },
            NodeKindInfo {
                kind: "bash",
                label: "Shell",
                description: "A deterministic bash script (supports a timeout).",
                ai: false,
            },
            NodeKindInfo {
                kind: "loop",
                label: "Loop",
                description: "Re-run a prompt until a convergence signal or max iterations.",
                ai: true,
            },
            NodeKindInfo {
                kind: "script",
                label: "Script",
                description: "An inline script run via bun (TS/JS) or uv (Python).",
                ai: false,
            },
            NodeKindInfo {
                kind: "approval",
                label: "Approval",
                description: "Pause for human approval (requires interactive delivery).",
                ai: false,
            },
            NodeKindInfo {
                kind: "cancel",
                label: "Cancel",
                description: "Terminate the run with a reason (usually gated with when:).",
                ai: false,
            },
        ],
        providers: vec![
            ProviderInfo {
                id: "claude",
                label: "Claude (subscription)",
                models: vec!["sonnet", "opus", "sonnet[1m]", "opus[1m]", "haiku"],
            },
            ProviderInfo {
                id: "codex",
                label: "Codex",
                models: vec!["gpt-5.3-codex", "gpt-5-codex"],
            },
            ProviderInfo {
                id: "pi",
                label: "Pi / Kimi",
                models: vec!["kimi-code/kimi-for-coding", "kimi-code/kimi-k2.6"],
            },
            ProviderInfo {
                id: "anthropic-api",
                label: "Anthropic API",
                models: vec!["sonnet", "opus"],
            },
        ],
        commands: list_commands(project_root),
        context_modes: vec!["fresh", "shared"],
        trigger_rules: vec![
            "all_success",
            "one_success",
            "none_failed_min_one_success",
            "all_done",
        ],
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn validates_a_good_workflow_and_summarizes_nodes() {
        let yaml = r#"
name: demo
nodes:
  - id: a
    bash: "echo hi"
  - id: b
    depends_on: [a]
    prompt: "do it"
"#;
        let r = validate_workflow(yaml);
        assert!(r.valid, "{:?}", r.error);
        assert_eq!(r.nodes.len(), 2);
        assert_eq!(r.nodes[0].kind, "bash");
        assert_eq!(r.nodes[1].kind, "prompt");
        assert_eq!(r.nodes[1].depends_on, vec!["a".to_string()]);
    }

    #[test]
    fn flags_unknown_dependency() {
        let yaml = r#"
name: demo
nodes:
  - id: a
    depends_on: [ghost]
    bash: "echo hi"
"#;
        let r = validate_workflow(yaml);
        assert!(!r.valid);
        assert!(r.error.unwrap().to_lowercase().contains("ghost"));
    }

    #[test]
    fn flags_cycle() {
        let yaml = r#"
name: demo
nodes:
  - id: a
    depends_on: [b]
    bash: "echo a"
  - id: b
    depends_on: [a]
    bash: "echo b"
"#;
        let r = validate_workflow(yaml);
        assert!(!r.valid);
    }

    #[test]
    fn flags_missing_body() {
        let yaml = r#"
name: demo
nodes:
  - id: a
"#;
        let r = validate_workflow(yaml);
        assert!(!r.valid);
    }

    #[test]
    fn save_round_trips_and_rejects_invalid_and_unsafe() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        let good = "name: t\nnodes:\n  - id: a\n    bash: \"echo hi\"\n";
        save_workflow(root, "my-flow", good).expect("save good");
        let got = get_workflow(root, "my-flow").expect("get");
        assert_eq!(got.name, "my-flow");
        assert!(matches!(got.source, Source::Project));
        assert!(got.yaml.contains("echo hi"));

        // Invalid YAML is refused.
        assert!(save_workflow(root, "bad", "name: t\nnodes:\n  - id: x\n").is_err());
        // Unsafe names are refused.
        assert!(save_workflow(root, "../escape", good).is_err());
        assert!(save_workflow(root, "a/b", good).is_err());
    }

    #[test]
    fn lists_bundled_and_project_with_project_shadowing() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        // Bundled default is present.
        let listed = list_workflows(root);
        assert!(listed
            .iter()
            .any(|w| w.name == defaults::DEFAULT_WORKFLOW && matches!(w.source, Source::Bundled)));

        // A project workflow with the same name shadows the bundled one.
        save_workflow(
            root,
            defaults::DEFAULT_WORKFLOW,
            "name: x\nnodes:\n  - id: a\n    bash: \"echo hi\"\n",
        )
        .unwrap();
        let listed = list_workflows(root);
        let entry = listed
            .iter()
            .find(|w| w.name == defaults::DEFAULT_WORKFLOW)
            .unwrap();
        assert!(matches!(entry.source, Source::Project));
        assert_eq!(
            listed
                .iter()
                .filter(|w| w.name == defaults::DEFAULT_WORKFLOW)
                .count(),
            1
        );
    }

    #[test]
    fn catalog_exposes_kinds_providers_and_bundled_commands() {
        let tmp = tempfile::tempdir().unwrap();
        let cat = catalog(tmp.path());
        assert_eq!(cat.node_kinds.len(), 7);
        assert!(cat.node_kinds.iter().any(|k| k.kind == "loop" && k.ai));
        assert!(cat.node_kinds.iter().any(|k| k.kind == "cancel"));
        assert!(cat.node_kinds.iter().any(|k| k.kind == "approval"));
        assert!(cat.providers.iter().any(|p| p.id == "pi"));
        // Bundled commands surface in the catalog.
        assert!(cat.commands.iter().any(|c| c.name == "implement-tasks"));
    }
}
