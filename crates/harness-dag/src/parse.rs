//! YAML loading and validation for workflow DAGs.
//!
//! Deserialization is two-step: YAML is parsed into a lenient [`RawNode`] shape
//! (every body field optional), then converted into the validated [`Node`]
//! model. This gives precise error messages — "exactly one body", "unknown
//! dependency" — instead of the opaque failures an untagged enum produces.

use std::collections::HashSet;

use serde::Deserialize;

use crate::error::DagError;
use crate::model::{
    ApprovalConfig, ContextMode, LoopConfig, Node, NodeKind, ScriptRuntime, TriggerRule, Workflow,
};

#[derive(Debug, Deserialize)]
struct RawWorkflow {
    name: String,
    #[serde(default)]
    description: Option<String>,
    #[serde(default)]
    provider: Option<String>,
    #[serde(default)]
    model: Option<String>,
    #[serde(default)]
    nodes: Vec<RawNode>,
}

#[derive(Debug, Deserialize)]
struct RawNode {
    id: String,
    #[serde(default)]
    depends_on: Vec<String>,
    #[serde(default)]
    when: Option<String>,
    #[serde(default)]
    trigger_rule: TriggerRule,
    #[serde(default)]
    provider: Option<String>,
    #[serde(default)]
    model: Option<String>,
    #[serde(default)]
    context: ContextMode,
    #[serde(default)]
    timeout: Option<u64>,

    // Mutually exclusive body discriminators.
    #[serde(default)]
    prompt: Option<String>,
    #[serde(default)]
    bash: Option<String>,
    #[serde(default)]
    command: Option<String>,
    #[serde(default)]
    script: Option<String>,
    #[serde(default)]
    runtime: Option<ScriptRuntime>,
    #[serde(default)]
    deps: Vec<String>,
    #[serde(default, rename = "loop")]
    loop_: Option<LoopConfig>,
    #[serde(default)]
    approval: Option<ApprovalConfig>,
    #[serde(default)]
    cancel: Option<String>,
}

impl RawNode {
    /// Resolve the single executable body, erroring if zero or more than one
    /// discriminator is present.
    fn into_kind(self) -> Result<(String, NodeKindParts), DagError> {
        let RawNode {
            id,
            prompt,
            bash,
            command,
            script,
            runtime,
            deps,
            loop_,
            approval,
            cancel,
            depends_on,
            when,
            trigger_rule,
            provider,
            model,
            context,
            timeout,
        } = self;

        let mut found: Vec<&'static str> = Vec::new();
        if prompt.is_some() {
            found.push("prompt");
        }
        if bash.is_some() {
            found.push("bash");
        }
        if command.is_some() {
            found.push("command");
        }
        if script.is_some() {
            found.push("script");
        }
        if loop_.is_some() {
            found.push("loop");
        }
        if approval.is_some() {
            found.push("approval");
        }
        if cancel.is_some() {
            found.push("cancel");
        }

        let kind = match found.as_slice() {
            [] => return Err(DagError::NoNodeKind(id)),
            [_one] => {
                if let Some(p) = prompt {
                    NodeKind::Prompt(p)
                } else if let Some(b) = bash {
                    NodeKind::Bash(b)
                } else if let Some(c) = command {
                    NodeKind::Command(c)
                } else if let Some(s) = script {
                    let runtime =
                        runtime.ok_or_else(|| DagError::ScriptMissingRuntime(id.clone()))?;
                    NodeKind::Script {
                        script: s,
                        runtime,
                        deps,
                    }
                } else if let Some(l) = loop_ {
                    NodeKind::Loop(l)
                } else if let Some(a) = approval {
                    NodeKind::Approval(a)
                } else if let Some(c) = cancel {
                    NodeKind::Cancel(c)
                } else {
                    unreachable!("found exactly one discriminator")
                }
            }
            _ => return Err(DagError::MultipleNodeKinds { node: id, found }),
        };

        Ok((
            id,
            NodeKindParts {
                kind,
                depends_on,
                when,
                trigger_rule,
                provider,
                model,
                context,
                timeout,
            },
        ))
    }
}

/// Common (non-body) node fields, separated so `into_kind` can move the body
/// out while returning the rest.
struct NodeKindParts {
    kind: NodeKind,
    depends_on: Vec<String>,
    when: Option<String>,
    trigger_rule: TriggerRule,
    provider: Option<String>,
    model: Option<String>,
    context: ContextMode,
    timeout: Option<u64>,
}

/// Parse a workflow from YAML, validating node uniqueness, body exclusivity,
/// and dependency references. Does not check for cycles — use
/// [`crate::graph::topological_layers`] for that (it needs the parsed nodes).
pub fn parse_workflow(yaml: &str) -> Result<Workflow, DagError> {
    let raw: RawWorkflow = serde_yaml::from_str(yaml)?;

    let mut nodes = Vec::with_capacity(raw.nodes.len());
    let mut ids: HashSet<String> = HashSet::with_capacity(raw.nodes.len());

    for raw_node in raw.nodes {
        let (id, parts) = raw_node.into_kind()?;
        if !ids.insert(id.clone()) {
            return Err(DagError::DuplicateNodeId(id));
        }
        nodes.push(Node {
            id,
            depends_on: parts.depends_on,
            when: parts.when,
            trigger_rule: parts.trigger_rule,
            provider: parts.provider,
            model: parts.model,
            context: parts.context,
            timeout: parts.timeout,
            kind: parts.kind,
        });
    }

    // All dependency references must resolve to a declared node.
    for node in &nodes {
        for dep in &node.depends_on {
            if !ids.contains(dep) {
                return Err(DagError::UnknownDependency {
                    node: node.id.clone(),
                    dep: dep.clone(),
                });
            }
        }
    }

    Ok(Workflow {
        name: raw.name,
        description: raw.description,
        provider: raw.provider,
        model: raw.model,
        nodes,
    })
}
