//! Tests for the DAG execution driver, using an in-memory mock runner.

use std::collections::{HashMap, VecDeque};
use std::sync::Mutex;

use async_trait::async_trait;

use super::*;
use crate::parse::parse_workflow;
use crate::vars::VarContext;

/// A recorded invocation, for asserting how the driver called the runner.
#[derive(Debug, Clone)]
struct Recorded {
    node_id: String,
    session: Option<String>,
    iteration: u32,
    body: String,
}

/// In-memory runner. Per-node it pops scripted outputs in order; when a node's
/// queue is empty it returns a default success with empty text (no signal).
#[derive(Default)]
struct MockRunner {
    responses: Mutex<HashMap<String, VecDeque<NodeOutput>>>,
    calls: Mutex<Vec<Recorded>>,
}

impl MockRunner {
    fn new() -> Self {
        Self::default()
    }

    /// Queue an output for `node_id` (FIFO across that node's invocations).
    fn respond(self, node_id: &str, out: NodeOutput) -> Self {
        self.responses
            .lock()
            .unwrap()
            .entry(node_id.to_string())
            .or_default()
            .push_back(out);
        self
    }

    fn calls_for(&self, node_id: &str) -> Vec<Recorded> {
        self.calls
            .lock()
            .unwrap()
            .iter()
            .filter(|c| c.node_id == node_id)
            .cloned()
            .collect()
    }
}

fn ok(text: &str) -> NodeOutput {
    NodeOutput {
        text: text.to_string(),
        session: None,
        success: true,
        usage: Usage::default(),
    }
}

fn ok_session(text: &str, session: &str) -> NodeOutput {
    NodeOutput {
        session: Some(session.to_string()),
        ..ok(text)
    }
}

fn fail(text: &str) -> NodeOutput {
    NodeOutput {
        success: false,
        ..ok(text)
    }
}

#[async_trait]
impl NodeRunner for MockRunner {
    async fn execute(&self, req: NodeRequest<'_>) -> Result<NodeOutput, RunnerError> {
        let body = match &req.body {
            NodeBody::Prompt(t) => format!("prompt:{t}"),
            NodeBody::Bash(t) => format!("bash:{t}"),
            NodeBody::Command(n) => format!("command:{n}"),
            NodeBody::Script { script, .. } => format!("script:{script}"),
        };
        self.calls.lock().unwrap().push(Recorded {
            node_id: req.node_id.to_string(),
            session: req.session.clone(),
            iteration: req.iteration,
            body,
        });
        let out = self
            .responses
            .lock()
            .unwrap()
            .get_mut(req.node_id)
            .and_then(|q| q.pop_front())
            .unwrap_or_else(|| ok(""));
        Ok(out)
    }
}

fn empty_vars() -> VarContext {
    VarContext::new()
}

#[tokio::test]
async fn runs_linear_chain_in_order() {
    let yaml = r#"
name: chain
nodes:
  - id: a
    bash: "echo a"
  - id: b
    depends_on: [a]
    prompt: "do b"
"#;
    let wf = parse_workflow(yaml).unwrap();
    let runner = MockRunner::new();
    let report = run_workflow(&wf, &runner, &empty_vars()).await.unwrap();

    assert_eq!(report.status, RunStatus::Completed);
    assert_eq!(report.nodes.len(), 2);
    assert_eq!(report.nodes[0].id, "a");
    assert_eq!(report.nodes[1].id, "b");
    assert!(report.nodes.iter().all(|n| n.status == NodeStatus::Success));
    assert_eq!(report.node("b").unwrap().iterations, 1);
}

#[tokio::test]
async fn threads_session_through_sequential_layers() {
    let yaml = r#"
name: thread
nodes:
  - id: a
    prompt: "first"
  - id: b
    depends_on: [a]
    prompt: "second"
"#;
    let wf = parse_workflow(yaml).unwrap();
    let runner = MockRunner::new().respond("a", ok_session("hi", "sess-a"));
    let report = run_workflow(&wf, &runner, &empty_vars()).await.unwrap();
    assert_eq!(report.status, RunStatus::Completed);

    // `a` started with no session; `b` (shared, sequential) inherited a's.
    assert_eq!(runner.calls_for("a")[0].session, None);
    assert_eq!(runner.calls_for("b")[0].session.as_deref(), Some("sess-a"));
}

#[tokio::test]
async fn parallel_layer_does_not_thread_sessions() {
    let yaml = r#"
name: diamond
nodes:
  - id: root
    prompt: "r"
  - id: left
    depends_on: [root]
    prompt: "l"
  - id: right
    depends_on: [root]
    prompt: "rt"
  - id: join
    depends_on: [left, right]
    prompt: "j"
"#;
    let wf = parse_workflow(yaml).unwrap();
    let runner = MockRunner::new().respond("root", ok_session("r", "sess-root"));
    let report = run_workflow(&wf, &runner, &empty_vars()).await.unwrap();
    assert_eq!(report.status, RunStatus::Completed);

    // Parallel layer: neither left nor right inherits root's session.
    assert_eq!(runner.calls_for("left")[0].session, None);
    assert_eq!(runner.calls_for("right")[0].session, None);
    // And the join after a parallel layer also starts fresh.
    assert_eq!(runner.calls_for("join")[0].session, None);
}

#[tokio::test]
async fn failure_skips_dependents_and_fails_run() {
    let yaml = r#"
name: failchain
nodes:
  - id: a
    bash: "boom"
  - id: b
    depends_on: [a]
    prompt: "after"
"#;
    let wf = parse_workflow(yaml).unwrap();
    let runner = MockRunner::new().respond("a", fail("error"));
    let report = run_workflow(&wf, &runner, &empty_vars()).await.unwrap();

    assert_eq!(report.status, RunStatus::Failed);
    assert_eq!(report.node("a").unwrap().status, NodeStatus::Failed);
    assert_eq!(report.node("b").unwrap().status, NodeStatus::Skipped);
    // `b` never reached the runner.
    assert!(runner.calls_for("b").is_empty());
}

#[tokio::test]
async fn loop_converges_on_signal() {
    let yaml = r#"
name: looped
nodes:
  - id: review
    loop:
      prompt: "review pass"
      until: REVIEW_CLEAN
      max_iterations: 5
"#;
    let wf = parse_workflow(yaml).unwrap();
    let runner = MockRunner::new()
        .respond("review", ok("still working"))
        .respond("review", ok("more work"))
        .respond("review", ok("all good <promise>REVIEW_CLEAN</promise>"));
    let report = run_workflow(&wf, &runner, &empty_vars()).await.unwrap();

    let review = report.node("review").unwrap();
    assert_eq!(review.status, NodeStatus::Success);
    assert_eq!(review.iterations, 3);
    assert_eq!(review.converged, Some(true));
    assert_eq!(runner.calls_for("review").len(), 3);
    // Iterations are numbered.
    assert_eq!(runner.calls_for("review")[2].iteration, 3);
}

#[tokio::test]
async fn loop_stops_at_max_without_signal() {
    let yaml = r#"
name: looped
nodes:
  - id: review
    loop:
      prompt: "review"
      until: REVIEW_CLEAN
      max_iterations: 3
"#;
    let wf = parse_workflow(yaml).unwrap();
    // No responses queued → default empty output, never signals.
    let runner = MockRunner::new();
    let report = run_workflow(&wf, &runner, &empty_vars()).await.unwrap();

    let review = report.node("review").unwrap();
    assert_eq!(review.status, NodeStatus::Success);
    assert_eq!(review.iterations, 3);
    assert_eq!(review.converged, Some(false));
    assert!(review.note.as_deref().unwrap().contains("max_iterations"));
}

#[tokio::test]
async fn loop_accumulates_usage_across_iterations() {
    let yaml = r#"
name: looped
nodes:
  - id: review
    loop:
      prompt: "review"
      until: DONE
      max_iterations: 5
"#;
    let wf = parse_workflow(yaml).unwrap();
    let usage = |input: u64, output: u64| NodeOutput {
        usage: Usage {
            input: Some(input),
            output: Some(output),
            ..Usage::default()
        },
        ..ok("working")
    };
    let runner = MockRunner::new()
        .respond("review", usage(100, 10))
        .respond("review", usage(200, 20))
        .respond(
            "review",
            NodeOutput {
                usage: Usage {
                    input: Some(50),
                    output: Some(5),
                    ..Usage::default()
                },
                ..ok("DONE")
            },
        );
    let report = run_workflow(&wf, &runner, &empty_vars()).await.unwrap();
    let review = report.node("review").unwrap();
    assert_eq!(review.converged, Some(true));
    assert_eq!(review.usage.input, Some(350));
    assert_eq!(review.usage.output, Some(35));
}

#[tokio::test]
async fn cancel_node_cancels_run_and_skips_downstream() {
    let yaml = r#"
name: cancelflow
nodes:
  - id: start
    bash: "go"
  - id: stop
    depends_on: [start]
    cancel: "manual stop"
  - id: after
    depends_on: [stop]
    bash: "never"
"#;
    let wf = parse_workflow(yaml).unwrap();
    let runner = MockRunner::new();
    let report = run_workflow(&wf, &runner, &empty_vars()).await.unwrap();

    assert_eq!(report.status, RunStatus::Cancelled);
    assert_eq!(report.node("start").unwrap().status, NodeStatus::Success);
    assert_eq!(report.node("stop").unwrap().status, NodeStatus::Success);
    assert_eq!(report.node("after").unwrap().status, NodeStatus::Cancelled);
    assert!(runner.calls_for("after").is_empty());
}

#[tokio::test]
async fn renders_variables_before_dispatch() {
    let yaml = r#"
name: vars
nodes:
  - id: a
    prompt: "write to $ARTIFACTS_DIR"
"#;
    let wf = parse_workflow(yaml).unwrap();
    let runner = MockRunner::new();
    let vars = VarContext::new().set("ARTIFACTS_DIR", "/run/7");
    let report = run_workflow(&wf, &runner, &vars).await.unwrap();
    assert_eq!(report.status, RunStatus::Completed);
    assert_eq!(runner.calls_for("a")[0].body, "prompt:write to /run/7");
}
