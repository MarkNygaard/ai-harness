//! Unit tests for the DAG model, parser, scheduler, and variable substitution.

use crate::error::DagError;
use crate::graph::topological_layers;
use crate::model::{ContextMode, NodeKind, ScriptRuntime};
use crate::parse::parse_workflow;
use crate::vars::{substitute, VarContext};

// ── parsing ──────────────────────────────────────────────────────────────

#[test]
fn parses_node_kinds_and_defaults() {
    let yaml = r#"
name: demo
description: a demo
provider: claude
model: sonnet
nodes:
  - id: explore
    bash: "echo hi"
    timeout: 5000
  - id: plan
    depends_on: [explore]
    provider: claude
    model: opus
    context: fresh
    prompt: "plan it"
  - id: setup
    depends_on: [plan]
    command: harness-plan-setup
  - id: gen
    depends_on: [setup]
    script: "console.log(1)"
    runtime: bun
    deps: ["left-pad"]
  - id: review
    depends_on: [gen]
    loop:
      prompt: "review"
      until: REVIEW_CLEAN
      max_iterations: 5
"#;
    let wf = parse_workflow(yaml).expect("parse");
    assert_eq!(wf.name, "demo");
    assert_eq!(wf.provider.as_deref(), Some("claude"));
    assert_eq!(wf.nodes.len(), 5);

    let explore = wf.node("explore").unwrap();
    assert!(matches!(explore.kind, NodeKind::Bash(_)));
    assert_eq!(explore.timeout, Some(5000));
    // Default context is Shared.
    assert_eq!(explore.context, ContextMode::Shared);

    let plan = wf.node("plan").unwrap();
    assert_eq!(plan.context, ContextMode::Fresh);
    assert_eq!(plan.model.as_deref(), Some("opus"));
    assert!(plan.kind.is_ai());

    let gen = wf.node("gen").unwrap();
    match &gen.kind {
        NodeKind::Script { runtime, deps, .. } => {
            assert_eq!(*runtime, ScriptRuntime::Bun);
            assert_eq!(deps, &["left-pad"]);
        }
        other => panic!("expected script, got {other:?}"),
    }

    let review = wf.node("review").unwrap();
    match &review.kind {
        NodeKind::Loop(cfg) => {
            assert_eq!(cfg.until, "REVIEW_CLEAN");
            assert_eq!(cfg.max_iterations, 5);
            assert!(!cfg.fresh_context);
        }
        other => panic!("expected loop, got {other:?}"),
    }
}

#[test]
fn rejects_duplicate_node_ids() {
    let yaml = r#"
name: dup
nodes:
  - id: a
    bash: "x"
  - id: a
    bash: "y"
"#;
    assert!(matches!(
        parse_workflow(yaml),
        Err(DagError::DuplicateNodeId(id)) if id == "a"
    ));
}

#[test]
fn rejects_unknown_dependency() {
    let yaml = r#"
name: baddep
nodes:
  - id: a
    depends_on: [ghost]
    bash: "x"
"#;
    match parse_workflow(yaml) {
        Err(DagError::UnknownDependency { node, dep }) => {
            assert_eq!(node, "a");
            assert_eq!(dep, "ghost");
        }
        other => panic!("expected UnknownDependency, got {other:?}"),
    }
}

#[test]
fn rejects_node_without_body() {
    let yaml = r#"
name: nobody
nodes:
  - id: a
    depends_on: []
"#;
    assert!(matches!(
        parse_workflow(yaml),
        Err(DagError::NoNodeKind(id)) if id == "a"
    ));
}

#[test]
fn rejects_node_with_multiple_bodies() {
    let yaml = r#"
name: multi
nodes:
  - id: a
    bash: "x"
    prompt: "y"
"#;
    match parse_workflow(yaml) {
        Err(DagError::MultipleNodeKinds { node, found }) => {
            assert_eq!(node, "a");
            assert!(found.contains(&"bash") && found.contains(&"prompt"));
        }
        other => panic!("expected MultipleNodeKinds, got {other:?}"),
    }
}

#[test]
fn rejects_script_without_runtime() {
    let yaml = r#"
name: norun
nodes:
  - id: a
    script: "print(1)"
"#;
    assert!(matches!(
        parse_workflow(yaml),
        Err(DagError::ScriptMissingRuntime(id)) if id == "a"
    ));
}

// ── scheduling ─────────────────────────────────────────────────────────────

fn ids<'a>(wf: &'a crate::model::Workflow, layer: &[usize]) -> Vec<&'a str> {
    layer.iter().map(|&i| wf.nodes[i].id.as_str()).collect()
}

#[test]
fn layers_linear_chain() {
    let yaml = r#"
name: chain
nodes:
  - id: a
    bash: "x"
  - id: b
    depends_on: [a]
    bash: "x"
  - id: c
    depends_on: [b]
    bash: "x"
"#;
    let wf = parse_workflow(yaml).unwrap();
    let layers = topological_layers(&wf).unwrap();
    assert_eq!(layers.len(), 3);
    assert_eq!(ids(&wf, &layers[0]), ["a"]);
    assert_eq!(ids(&wf, &layers[1]), ["b"]);
    assert_eq!(ids(&wf, &layers[2]), ["c"]);
}

#[test]
fn layers_diamond_groups_parallel_nodes() {
    let yaml = r#"
name: diamond
nodes:
  - id: root
    bash: "x"
  - id: left
    depends_on: [root]
    bash: "x"
  - id: right
    depends_on: [root]
    bash: "x"
  - id: join
    depends_on: [left, right]
    bash: "x"
"#;
    let wf = parse_workflow(yaml).unwrap();
    let layers = topological_layers(&wf).unwrap();
    assert_eq!(layers.len(), 3);
    assert_eq!(ids(&wf, &layers[0]), ["root"]);
    assert_eq!(ids(&wf, &layers[1]), ["left", "right"]);
    assert_eq!(ids(&wf, &layers[2]), ["join"]);
}

#[test]
fn detects_cycle() {
    // a -> b -> a is not expressible as a self-loop here; build a 2-cycle.
    let yaml = r#"
name: cyclic
nodes:
  - id: a
    depends_on: [b]
    bash: "x"
  - id: b
    depends_on: [a]
    bash: "x"
"#;
    let wf = parse_workflow(yaml).unwrap();
    match topological_layers(&wf) {
        Err(DagError::Cycle(stuck)) => {
            assert!(stuck.contains(&"a".to_string()));
            assert!(stuck.contains(&"b".to_string()));
        }
        other => panic!("expected Cycle, got {other:?}"),
    }
}

// ── variable substitution ──────────────────────────────────────────────────

#[test]
fn substitutes_named_and_braced_vars() {
    let ctx = VarContext::new()
        .set("ARTIFACTS_DIR", "/run/42")
        .set("BASE_BRANCH", "main");
    let out = substitute("write to $ARTIFACTS_DIR on ${BASE_BRANCH}", &ctx).unwrap();
    assert_eq!(out, "write to /run/42 on main");
}

#[test]
fn substitutes_positional_args() {
    let ctx = VarContext::new().with_positional(vec!["one".into(), "two".into()]);
    let out = substitute("first=$1 second=$2", &ctx).unwrap();
    assert_eq!(out, "first=one second=two");
}

#[test]
fn passes_through_unrecognized_shell_vars() {
    let ctx = VarContext::new().set("BASE_BRANCH", "main");
    // $HOME and ${results} are not harness vars and must survive untouched.
    let out = substitute("cd $HOME && echo ${results} on $BASE_BRANCH", &ctx).unwrap();
    assert_eq!(out, "cd $HOME && echo ${results} on main");
}

#[test]
fn errors_on_recognized_but_missing_var() {
    let ctx = VarContext::new();
    match substitute("at $ARTIFACTS_DIR", &ctx) {
        Err(DagError::MissingVariable(name)) => assert_eq!(name, "ARTIFACTS_DIR"),
        other => panic!("expected MissingVariable, got {other:?}"),
    }
}

#[test]
fn workflow_round_trips_through_json() {
    let yaml = r#"
name: rt
nodes:
  - id: a
    prompt: "hi"
"#;
    let wf = parse_workflow(yaml).unwrap();
    let json = serde_json::to_string(&wf).unwrap();
    let back: crate::model::Workflow = serde_json::from_str(&json).unwrap();
    assert_eq!(wf, back);
}
