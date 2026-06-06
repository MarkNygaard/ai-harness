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
use std::sync::LazyLock;

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
/// A curated, ready-to-drop node template for the editor palette. `node` is a
/// complete node spec (the same flat shape `set_node` accepts): a default `id`,
/// exactly one body, plus provider/model/category/output_format where relevant.
#[derive(Debug, Clone, Serialize)]
pub struct PrebuiltStep {
    /// Default node id (the palette key; reassigned to a fresh id on insert).
    pub id: &'static str,
    /// Friendly palette label.
    pub label: &'static str,
    pub description: &'static str,
    /// The full node spec (flat `EditorNode`/`RawNode` shape) to drop onto the canvas.
    pub node: serde_json::Value,
}

/// The building-blocks catalog an editor/AI uses to author a workflow.
#[derive(Debug, Clone, Serialize)]
pub struct Catalog {
    pub node_kinds: Vec<NodeKindInfo>,
    pub providers: Vec<ProviderInfo>,
    pub commands: Vec<CommandInfo>,
    pub context_modes: Vec<&'static str>,
    pub trigger_rules: Vec<&'static str>,
    pub prebuilt_steps: Vec<PrebuiltStep>,
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

// ── Structured (node-level) authoring ────────────────────────────────────────
//
// These let an MCP client build a workflow incrementally — one targeted change
// per call — instead of authoring raw YAML. Each operation loads the workflow,
// applies one mutation, then routes through `save_workflow` (which validates and
// rejects a broken DAG), so a bad call fails atomically without corrupting the
// file. Mutations happen on the YAML document (`serde_yaml::Value`) so the flat
// node shape the parser expects is preserved exactly.

/// Serialize a YAML document and save it (validates first via `save_workflow`).
fn save_doc(project_root: &Path, name: &str, doc: &serde_yaml::Value) -> Result<(), String> {
    let yaml = serde_yaml::to_string(doc).map_err(|e| format!("serialize workflow: {e}"))?;
    save_workflow(project_root, name, &yaml)
}

/// Parse a workflow's current YAML into a mutable document. Bundled defaults are
/// editable too — the save lands in the project, shadowing the default.
fn load_doc(project_root: &Path, name: &str) -> Result<serde_yaml::Value, String> {
    let src = get_workflow(project_root, name)?;
    serde_yaml::from_str(&src.yaml).map_err(|e| format!("parse workflow `{name}`: {e}"))
}

/// Borrow the document's `nodes:` sequence, creating it if absent.
fn nodes_mut(doc: &mut serde_yaml::Value) -> Result<&mut Vec<serde_yaml::Value>, String> {
    let map = doc
        .as_mapping_mut()
        .ok_or_else(|| "workflow is not a mapping".to_string())?;
    let key = serde_yaml::Value::from("nodes");
    if !map.contains_key(&key) {
        map.insert(key.clone(), serde_yaml::Value::Sequence(Vec::new()));
    }
    map.get_mut(&key)
        .and_then(|v| v.as_sequence_mut())
        .ok_or_else(|| "`nodes` is not a sequence".to_string())
}

fn node_id(node: &serde_yaml::Value) -> Option<&str> {
    node.get("id").and_then(|v| v.as_str())
}

/// Create a new, empty workflow in the project. Errors if one already exists
/// there (so it never clobbers — edit existing ones with the node ops).
pub fn create_workflow(
    project_root: &Path,
    name: &str,
    description: Option<&str>,
    provider: Option<&str>,
    model: Option<&str>,
) -> Result<(), String> {
    if !is_safe_name(name) {
        return Err(format!("invalid workflow name `{name}`"));
    }
    let project_file = project_root
        .join(".harness")
        .join("workflows")
        .join(format!("{name}.yaml"));
    if project_file.is_file() {
        return Err(format!("workflow `{name}` already exists in this project"));
    }
    let mut map = serde_yaml::Mapping::new();
    map.insert("name".into(), name.into());
    for (k, v) in [
        ("description", description),
        ("provider", provider),
        ("model", model),
    ] {
        if let Some(val) = v.filter(|s| !s.is_empty()) {
            map.insert(k.into(), val.into());
        }
    }
    map.insert("nodes".into(), serde_yaml::Value::Sequence(Vec::new()));
    save_doc(project_root, name, &serde_yaml::Value::Mapping(map))
}

/// Add or replace a node (matched by `id`) from a JSON object describing it —
/// the same fields the YAML node accepts (`prompt`/`bash`/`command`/`script`/
/// `loop`/`approval`/`cancel` body, plus `depends_on`/`when`/`category`/…). The
/// save validates exactly-one-body, resolvable refs, and no cycles.
pub fn set_node(project_root: &Path, name: &str, node: serde_json::Value) -> Result<(), String> {
    let id = node
        .get("id")
        .and_then(|v| v.as_str())
        .filter(|s| !s.is_empty())
        .ok_or_else(|| "node must have a non-empty `id`".to_string())?
        .to_string();
    let node_yaml: serde_yaml::Value =
        serde_yaml::to_value(&node).map_err(|e| format!("convert node: {e}"))?;

    let mut doc = load_doc(project_root, name)?;
    let nodes = nodes_mut(&mut doc)?;
    match nodes.iter_mut().find(|n| node_id(n) == Some(id.as_str())) {
        Some(existing) => *existing = node_yaml,
        None => nodes.push(node_yaml),
    }
    save_doc(project_root, name, &doc)
}

/// Remove a node by id and strip it from every other node's `depends_on`.
pub fn remove_node(project_root: &Path, name: &str, id: &str) -> Result<(), String> {
    let mut doc = load_doc(project_root, name)?;
    let nodes = nodes_mut(&mut doc)?;
    let before = nodes.len();
    nodes.retain(|n| node_id(n) != Some(id));
    if nodes.len() == before {
        return Err(format!("no node `{id}` in workflow `{name}`"));
    }
    for n in nodes.iter_mut() {
        if let Some(deps) = n.get_mut("depends_on").and_then(|d| d.as_sequence_mut()) {
            deps.retain(|d| d.as_str() != Some(id));
        }
    }
    save_doc(project_root, name, &doc)
}

/// Add a dependency edge: `to` now `depends_on` `from` (deduped). Unknown ids
/// are caught by the validating save.
pub fn connect_nodes(project_root: &Path, name: &str, from: &str, to: &str) -> Result<(), String> {
    let mut doc = load_doc(project_root, name)?;
    let nodes = nodes_mut(&mut doc)?;
    let target = nodes
        .iter_mut()
        .find(|n| node_id(n) == Some(to))
        .ok_or_else(|| format!("no node `{to}` in workflow `{name}`"))?;
    let map = target
        .as_mapping_mut()
        .ok_or_else(|| "node is not a mapping".to_string())?;
    let deps = map
        .entry("depends_on".into())
        .or_insert_with(|| serde_yaml::Value::Sequence(Vec::new()))
        .as_sequence_mut()
        .ok_or_else(|| "`depends_on` is not a sequence".to_string())?;
    if !deps.iter().any(|d| d.as_str() == Some(from)) {
        deps.push(from.into());
    }
    save_doc(project_root, name, &doc)
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

/// Curated node templates for the editor's "Prebuilt steps" palette section,
/// distilled from the bundled `idea-to-pr` pipeline so the prompts/configs stay
/// a single source of truth.
static PREBUILT_CURATED: &[(&str, &str, &str)] = &[
    ("explore", "Explore codebase",
     "Read-only Sonnet pass that maps the code a task touches and writes exploration notes."),
    ("create-plan", "Create plan",
     "Opus planner that turns the task + exploration into a concrete implementation plan."),
    ("install-deps", "Install dependencies",
     "Auto-detects the project's package manager and installs locked dependencies."),
    ("implement-tasks", "Implement tasks",
     "Runs the bundled implement-tasks command to write the code for the plan."),
    ("validate", "Validate",
     "Runs the validate command and emits a {passed, summary} verdict downstream nodes gate on."),
    ("pi-review-fix-loop", "Review & fix loop",
     "Self-review-and-fix loop (up to 5 passes) that commits fixes until the PR is clean."),
    ("finalize-pr", "Open PR",
     "Runs the finalize-pr command to push the branch and open the pull request."),
    ("final-verify-loop", "Final verify gate",
     "Final build gate (up to 3 passes) that re-runs the full verify chain before merge."),
];

static PREBUILT_STEPS_CACHE: LazyLock<Vec<PrebuiltStep>> = LazyLock::new(|| {
    let Some(yaml) = defaults::default_workflow(defaults::DEFAULT_WORKFLOW) else {
        tracing::warn!("bundled default workflow not found; prebuilt steps unavailable");
        return Vec::new();
    };
    let doc: serde_yaml::Value = match serde_yaml::from_str(yaml) {
        Ok(v) => v,
        Err(e) => {
            tracing::warn!("failed to parse bundled idea-to-pr.yaml: {e}");
            return Vec::new();
        }
    };
    let empty = Vec::new();
    let nodes = doc
        .get("nodes")
        .and_then(|n| n.as_sequence())
        .unwrap_or(&empty);

    PREBUILT_CURATED
        .iter()
        .filter_map(|&(id, label, description)| {
            let node_yaml = nodes.iter().find(|n| node_id(n) == Some(id))?;
            // Convert the (already flat) YAML node to JSON, then drop the
            // pipeline-only wiring so a freshly dropped node is unconnected.
            let mut node = serde_json::to_value(node_yaml).ok()?;
            if let Some(obj) = node.as_object_mut() {
                obj.remove("depends_on");
                obj.remove("when");
            }
            Some(PrebuiltStep {
                id,
                label,
                description,
                node,
            })
        })
        .collect()
});

fn prebuilt_steps() -> Vec<PrebuiltStep> {
    PREBUILT_STEPS_CACHE.clone()
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
                // ChatGPT-account auth exposes general models (gpt-5.5/5.4) but
                // not the `-codex` variants; an account's actual list is shown by
                // `codex` → `/model`. gpt-5.3-codex needs API-key / non-ChatGPT auth.
                models: vec!["gpt-5.5", "gpt-5-codex", "gpt-5.3-codex"],
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
        prebuilt_steps: prebuilt_steps(),
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
    #[test]
    fn catalog_exposes_valid_prebuilt_steps() {
        let tmp = tempfile::tempdir().unwrap();
        let cat = catalog(tmp.path());

        // Every curated step must be present (if one is renamed in the bundled
        // YAML and the list isn't updated, this fails loud and clear).
        assert_eq!(cat.prebuilt_steps.len(), PREBUILT_CURATED.len());
        for want in [
            "explore",
            "create-plan",
            "implement-tasks",
            "validate",
            "finalize-pr",
        ] {
            assert!(
                cat.prebuilt_steps.iter().any(|s| s.id == want),
                "missing prebuilt step `{want}`"
            );
        }

        // Pipeline-only wiring is stripped from every template.
        for step in &cat.prebuilt_steps {
            let obj = step.node.as_object().expect("node is an object");
            assert!(
                !obj.contains_key("depends_on"),
                "{} kept depends_on",
                step.id
            );
            assert!(!obj.contains_key("when"), "{} kept when", step.id);
        }

        // Each template, dropped as a single-node workflow, passes validation.
        for step in &cat.prebuilt_steps {
            let doc = serde_json::json!({
                "name": "t",
                "provider": "claude",
                "model": "sonnet",
                "nodes": [step.node],
            });
            let yaml = serde_yaml::to_string(&doc).unwrap();
            let r = validate_workflow(&yaml);
            assert!(r.valid, "prebuilt `{}` invalid: {:?}", step.id, r.error);
            assert_eq!(r.nodes.len(), 1);
        }
    }

    #[test]
    fn structured_authoring_create_set_connect_remove() {
        use serde_json::json;
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();

        create_workflow(root, "built", Some("via tools"), Some("pi"), None).unwrap();
        // Can't create twice.
        assert!(create_workflow(root, "built", None, None, None).is_err());

        // Add two nodes, then connect them.
        set_node(root, "built", json!({ "id": "explore", "prompt": "look" })).unwrap();
        set_node(
            root,
            "built",
            json!({ "id": "plan", "prompt": "plan it", "category": "planning" }),
        )
        .unwrap();
        connect_nodes(root, "built", "explore", "plan").unwrap();

        let wf = parse_workflow(&get_workflow(root, "built").unwrap().yaml).unwrap();
        assert_eq!(wf.nodes.len(), 2);
        assert_eq!(
            wf.node("plan").unwrap().depends_on,
            vec!["explore".to_string()]
        );

        // set_node replaces by id (here: swap plan's body to a command).
        set_node(
            root,
            "built",
            json!({ "id": "plan", "command": "create-plan", "depends_on": ["explore"] }),
        )
        .unwrap();
        let wf = parse_workflow(&get_workflow(root, "built").unwrap().yaml).unwrap();
        assert!(matches!(
            wf.node("plan").unwrap().kind,
            NodeKind::Command(_)
        ));

        // A node whose body is invalid (two bodies) is rejected atomically.
        assert!(set_node(
            root,
            "built",
            json!({ "id": "bad", "prompt": "x", "bash": "y" }),
        )
        .is_err());

        // Removing a node also strips it from dependents' depends_on.
        remove_node(root, "built", "explore").unwrap();
        let wf = parse_workflow(&get_workflow(root, "built").unwrap().yaml).unwrap();
        assert_eq!(wf.nodes.len(), 1);
        assert!(wf.node("plan").unwrap().depends_on.is_empty());
        assert!(remove_node(root, "built", "explore").is_err());
    }
}
