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
    ApprovalConfig, ContextMode, LinearAction, LoopConfig, Node, NodeHooks, NodeKind,
    ScriptRuntime, TriggerRule, Workflow,
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
    ui: Option<crate::model::WorkflowUi>,
    #[serde(default)]
    improvements: Option<crate::model::ImprovementsConfig>,
    #[serde(default)]
    internal: bool,
    #[serde(default)]
    handles_epics: bool,
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
    effort: Option<String>,
    #[serde(default)]
    context: ContextMode,
    #[serde(default)]
    category: Option<String>,
    #[serde(default)]
    artifact: Option<String>,
    #[serde(default)]
    timeout: Option<u64>,
    #[serde(default)]
    retries: u32,
    #[serde(default)]
    output_format: Option<serde_json::Value>,
    #[serde(default)]
    hooks: Option<NodeHooks>,
    #[serde(default)]
    mcp: Vec<crate::model::McpUse>,
    #[serde(default)]
    linear_comment: bool,

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
    #[serde(default)]
    linear: Option<RawLinear>,
    #[serde(default)]
    route: Option<crate::model::RouteConfig>,
}

/// The `linear:` body's own mutually exclusive keys.
///
/// A struct of optionals rather than deserializing [`LinearAction`] directly:
/// serde's externally-tagged enum wants `!move_state` YAML-tag syntax, which is
/// not how the rest of this file reads. This mirrors how `RawNode` picks one
/// body from several discriminators, and gives the same kind of error.
#[derive(Debug, Deserialize)]
struct RawLinear {
    #[serde(default)]
    move_state: Option<String>,
    #[serde(default)]
    comment: Option<String>,
    #[serde(default)]
    add_labels: Option<Vec<String>>,
    #[serde(default)]
    remove_labels: Option<Vec<String>>,
}

impl RawLinear {
    fn into_action(self, node: &str) -> Result<LinearAction, DagError> {
        let mut found: Vec<&'static str> = Vec::new();
        if self.move_state.is_some() {
            found.push("linear.move_state");
        }
        if self.comment.is_some() {
            found.push("linear.comment");
        }
        if self.add_labels.is_some() {
            found.push("linear.add_labels");
        }
        if self.remove_labels.is_some() {
            found.push("linear.remove_labels");
        }
        match found.as_slice() {
            [] => Err(DagError::NoNodeKind(node.to_string())),
            [_one] => {
                if let Some(s) = self.move_state {
                    Ok(LinearAction::MoveState(s))
                } else if let Some(c) = self.comment {
                    Ok(LinearAction::Comment(c))
                } else if let Some(l) = self.add_labels {
                    Ok(LinearAction::AddLabels(l))
                } else if let Some(l) = self.remove_labels {
                    Ok(LinearAction::RemoveLabels(l))
                } else {
                    unreachable!("exactly one linear key matched")
                }
            }
            _ => Err(DagError::MultipleNodeKinds {
                node: node.to_string(),
                found,
            }),
        }
    }
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
            linear,
            route,
            depends_on,
            when,
            trigger_rule,
            provider,
            model,
            effort,
            context,
            category,
            artifact,
            timeout,
            retries,
            output_format,
            hooks,
            mcp,
            linear_comment,
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
        if linear.is_some() {
            found.push("linear");
        }
        if route.is_some() {
            found.push("route");
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
                } else if let Some(l) = linear {
                    NodeKind::Linear(l.into_action(&id)?)
                } else if let Some(r) = route {
                    validate_route(&id, &r)?;
                    NodeKind::Route(r)
                } else {
                    unreachable!("found exactly one discriminator")
                }
            }
            _ => return Err(DagError::MultipleNodeKinds { node: id, found }),
        };

        // A route that asks a model needs its answer as JSON naming a route;
        // spelled out here from the routes, so the author never has to.
        let output_format = match (&kind, output_format) {
            (NodeKind::Route(r), None) if r.prompt.is_some() => Some(r.output_schema()),
            (_, of) => of,
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
                effort,
                context,
                category,
                artifact,
                timeout,
                retries,
                output_format,
                hooks,
                mcp,
                linear_comment,
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
    effort: Option<String>,
    context: ContextMode,
    category: Option<String>,
    artifact: Option<String>,
    timeout: Option<u64>,
    retries: u32,
    output_format: Option<serde_json::Value>,
    hooks: Option<NodeHooks>,
    mcp: Vec<crate::model::McpUse>,
    linear_comment: bool,
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
        // Which servers exist is the install's registry, checked at save time;
        // parsing only refuses what could never name one.
        if let Some(u) = parts
            .mcp
            .iter()
            .find(|u| !crate::model::is_valid_mcp_server_name(&u.server))
        {
            return Err(DagError::InvalidMcpServerName {
                node: id,
                name: u.server.clone(),
            });
        }
        if let Some(effort) = &parts.effort {
            if !crate::model::EFFORT_LEVELS.contains(&effort.as_str()) {
                return Err(DagError::InvalidEffort {
                    node: id,
                    value: effort.clone(),
                });
            }
        }
        nodes.push(Node {
            id,
            depends_on: parts.depends_on,
            when: parts.when,
            trigger_rule: parts.trigger_rule,
            provider: parts.provider,
            model: parts.model,
            effort: parts.effort,
            context: parts.context,
            category: parts.category,
            artifact: parts.artifact,
            timeout: parts.timeout,
            retries: parts.retries,
            output_format: parts.output_format,
            hooks: parts.hooks,
            mcp: parts.mcp,
            linear_comment: parts.linear_comment,
            kind: parts.kind,
        });
    }

    // One summary per issue: two nodes both posting would leave the issue with
    // two "results" and no way to tell which one is the answer.
    let posting: Vec<String> = nodes
        .iter()
        .filter(|n| n.linear_comment)
        .map(|n| n.id.clone())
        .collect();
    if posting.len() > 1 {
        return Err(DagError::MultipleLinearComments(posting));
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

    // Fail loud on dangling references and malformed conditions, so authoring
    // mistakes surface at load time rather than silently no-op'ing at run time.
    for node in &nodes {
        // Every `$id.output` reference (in any inline body or the `when:`
        // expression) must point to a declared node.
        for text in node.substitutable_text() {
            for referenced in crate::vars::referenced_node_ids(text) {
                if !ids.contains(&referenced) {
                    return Err(DagError::UnknownNodeReference {
                        node: node.id.clone(),
                        referenced,
                    });
                }
            }
        }
        // The `when:` expression must be structurally valid.
        if let Some(expr) = &node.when {
            crate::cond::validate_syntax(expr)?;
        }
        // So must every route's rule.
        if let NodeKind::Route(cfg) = &node.kind {
            for expr in cfg.routes.iter().filter_map(|r| r.when.as_deref()) {
                crate::cond::validate_syntax(expr)?;
            }
        }
    }

    // A report's `verdict_node`, when named, must resolve to a declared node.
    if let Some(node_id) = raw
        .ui
        .as_ref()
        .and_then(|ui| ui.report.as_ref())
        .and_then(|r| r.verdict_node.as_ref())
    {
        if !ids.contains(node_id) {
            return Err(DagError::ReportVerdictNodeUnknown(node_id.clone()));
        }
    }

    if let Some(cfg) = &raw.improvements {
        if !crate::model::IMPROVEMENTS_EVERY.contains(&cfg.every) {
            return Err(DagError::ImprovementsEvery(cfg.every));
        }
    }

    Ok(Workflow {
        name: raw.name,
        description: raw.description,
        provider: raw.provider,
        model: raw.model,
        ui: raw.ui,
        improvements: raw.improvements,
        internal: raw.internal,
        handles_epics: raw.handles_epics,
        nodes,
    })
}

/// A route node must always end on a route, and name its routes so a `when:`
/// can compare against them.
fn validate_route(node: &str, cfg: &crate::model::RouteConfig) -> Result<(), DagError> {
    let bad = |reason: String| DagError::InvalidRoute {
        node: node.to_string(),
        reason,
    };
    if cfg.routes.len() < 2 {
        return Err(bad("it needs at least two routes to choose between".into()));
    }
    let mut seen: Vec<&str> = Vec::new();
    for r in &cfg.routes {
        let name = r.name.as_str();
        let ok = !name.is_empty()
            && name
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-');
        if !ok {
            return Err(bad(format!(
                "route name `{name}` must be letters, digits, `-` or `_`"
            )));
        }
        if seen.contains(&name) {
            return Err(bad(format!("route `{name}` is named twice")));
        }
        seen.push(name);
    }
    if let Some(d) = &cfg.default {
        if !seen.contains(&d.as_str()) {
            return Err(bad(format!("its default `{d}` isn't one of its routes")));
        }
    }
    // Rules alone can all be false. Without a model to ask or a default to
    // fall back on, a run could reach this step and have nowhere to go.
    if cfg.prompt.is_none() && cfg.default.is_none() {
        return Err(bad(
            "it needs a `prompt` (a model chooses) or a `default`, so every run ends on a route"
                .into(),
        ));
    }
    Ok(())
}
