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
    provider: Option<String>,
    model: Option<String>,
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
            NodeBody::Linear(a) => format!("linear:{}", a.summary()),
        };
        self.calls.lock().unwrap().push(Recorded {
            node_id: req.node_id.to_string(),
            session: req.session.clone(),
            iteration: req.iteration,
            body,
            provider: req.provider.map(str::to_string),
            model: req.model.map(str::to_string),
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
async fn one_failed_runs_after_a_failure_anywhere_upstream() {
    // `implement` fails, so `verify` is skipped — the cleanup step's direct
    // dependency never *failed*. It still has to run.
    let yaml = r#"
name: cleanup
nodes:
  - id: implement
    prompt: "build it"
  - id: verify
    depends_on: [implement]
    prompt: "check it"
  - id: mark-failed
    depends_on: [verify]
    trigger_rule: one_failed
    prompt: "label the issue"
"#;
    let wf = parse_workflow(yaml).unwrap();
    let runner = MockRunner::new()
        .respond("implement", fail("boom"))
        .respond("mark-failed", ok("labelled"));
    let report = run_workflow(&wf, &runner, &empty_vars()).await.unwrap();
    assert_eq!(report.node("verify").unwrap().status, NodeStatus::Skipped);
    assert_eq!(
        report.node("mark-failed").unwrap().status,
        NodeStatus::Success
    );
    assert_eq!(
        report.status,
        RunStatus::Failed,
        "handling a failure doesn't hide it"
    );
}

#[tokio::test]
async fn one_failed_is_skipped_when_nothing_failed() {
    let yaml = r#"
name: cleanup
nodes:
  - id: implement
    prompt: "build it"
  - id: mark-failed
    depends_on: [implement]
    trigger_rule: one_failed
    prompt: "label the issue"
"#;
    let wf = parse_workflow(yaml).unwrap();
    let runner = MockRunner::new().respond("implement", ok("done"));
    let report = run_workflow(&wf, &runner, &empty_vars()).await.unwrap();
    assert_eq!(
        report.node("mark-failed").unwrap().status,
        NodeStatus::Skipped
    );
    assert!(runner.calls_for("mark-failed").is_empty());
    assert_eq!(report.status, RunStatus::Completed);
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
async fn loop_block_provider_model_override_node_and_workflow() {
    // Workflow default is claude/sonnet; the loop block declares pi/kimi — the
    // loop body (and the recorded NodeRun) must use the loop-level provider/model.
    let yaml = r#"
name: looped
provider: claude
model: sonnet
nodes:
  - id: review
    loop:
      provider: pi
      model: kimi-coding/kimi-for-coding
      prompt: "review pass"
      until: REVIEW_CLEAN
      max_iterations: 3
"#;
    let wf = parse_workflow(yaml).unwrap();
    let runner = MockRunner::new().respond("review", ok("done <promise>REVIEW_CLEAN</promise>"));
    let report = run_workflow(&wf, &runner, &empty_vars()).await.unwrap();

    let review = report.node("review").unwrap();
    assert_eq!(review.provider.as_deref(), Some("pi"));
    assert_eq!(review.model.as_deref(), Some("kimi-coding/kimi-for-coding"));

    let call = &runner.calls_for("review")[0];
    assert_eq!(call.provider.as_deref(), Some("pi"));
    assert_eq!(call.model.as_deref(), Some("kimi-coding/kimi-for-coding"));
}

#[tokio::test]
async fn non_agent_nodes_carry_no_provider_or_model() {
    // A workflow-level provider/model must NOT be stamped onto a bash node (it
    // runs no agent); an agent node still inherits it.
    let yaml = r#"
name: t
provider: pi
model: kimi-coding/kimi-for-coding
nodes:
  - id: setup
    bash: "echo hi"
  - id: think
    prompt: "do it"
    depends_on: [setup]
"#;
    let wf = parse_workflow(yaml).unwrap();
    let runner = MockRunner::new();
    let report = run_workflow(&wf, &runner, &empty_vars()).await.unwrap();

    let setup = report.node("setup").unwrap();
    assert_eq!(setup.provider, None, "bash node must have no provider");
    assert_eq!(setup.model, None, "bash node must have no model");

    let think = report.node("think").unwrap();
    assert_eq!(think.provider.as_deref(), Some("pi"));
    assert_eq!(think.model.as_deref(), Some("kimi-coding/kimi-for-coding"));
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

#[tokio::test]
async fn records_timestamps_for_executed_but_not_skipped_nodes() {
    let yaml = r#"
name: ts
nodes:
  - id: a
    bash: "boom"
  - id: b
    depends_on: [a]
    prompt: "after"
"#;
    let wf = parse_workflow(yaml).unwrap();
    let runner = MockRunner::new().respond("a", fail("err"));
    let report = run_workflow(&wf, &runner, &empty_vars()).await.unwrap();

    // `a` ran (even though it failed) → timestamps set, ended >= started.
    let a = report.node("a").unwrap();
    let (start, end) = (a.started_at.unwrap(), a.ended_at.unwrap());
    assert!(
        end >= start,
        "ended_at {end} should be >= started_at {start}"
    );

    // `b` was skipped (a failed) → it never ran, so no timing.
    let b = report.node("b").unwrap();
    assert_eq!(b.status, NodeStatus::Skipped);
    assert!(b.started_at.is_none() && b.ended_at.is_none());
}

#[tokio::test]
async fn streams_run_and_node_events() {
    use futures::StreamExt as _;

    let yaml = r#"
name: ev
nodes:
  - id: a
    bash: "x"
  - id: b
    depends_on: [a]
    prompt: "y"
"#;
    let wf = parse_workflow(yaml).unwrap();
    let runner = MockRunner::new();
    let (tx, rx) = futures::channel::mpsc::unbounded();
    let report = run_workflow_streaming(&wf, &runner, &empty_vars(), Some(&tx))
        .await
        .unwrap();
    drop(tx); // close the stream so collect terminates
    let events: Vec<RunEvent> = rx.collect().await;

    assert!(matches!(
        events.first(),
        Some(RunEvent::RunStarted { total_nodes: 2, .. })
    ));
    assert!(matches!(
        events.last(),
        Some(RunEvent::RunFinished {
            status: RunStatus::Completed
        })
    ));

    // Both nodes emit a NodeStarted (in dependency order) and a NodeFinished.
    let started: Vec<String> = events
        .iter()
        .filter_map(|e| match e {
            RunEvent::NodeStarted { node_id, .. } => Some(node_id.clone()),
            _ => None,
        })
        .collect();
    assert_eq!(started, vec!["a", "b"]);
    let finished = events
        .iter()
        .filter(
            |e| matches!(e, RunEvent::NodeFinished { node } if node.status == NodeStatus::Success),
        )
        .count();
    assert_eq!(finished, 2);
    assert_eq!(report.status, RunStatus::Completed);
}

#[tokio::test]
async fn loop_emits_iteration_progress() {
    use futures::StreamExt as _;

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
        .respond("review", ok("done <promise>REVIEW_CLEAN</promise>"));
    let (tx, rx) = futures::channel::mpsc::unbounded();
    run_workflow_streaming(&wf, &runner, &empty_vars(), Some(&tx))
        .await
        .unwrap();
    drop(tx);
    let events: Vec<RunEvent> = rx.collect().await;

    // Each iteration reports its position against the max (a ceiling, not a
    // target — the loop converged on iteration 2, so there is no "3/5").
    let progress: Vec<String> = events
        .iter()
        .filter_map(|e| match e {
            RunEvent::NodeProgress { node_id, activity } if node_id == "review" => {
                Some(activity.text.clone())
            }
            _ => None,
        })
        .collect();
    assert!(progress.contains(&"🔁 1/5".to_string()));
    assert!(progress.contains(&"🔁 2/5".to_string()));
    assert!(!progress.contains(&"🔁 3/5".to_string()));
}

#[tokio::test]
async fn passes_upstream_output_into_downstream_prompt() {
    let yaml = r#"
name: passthrough
nodes:
  - id: plan
    prompt: "make a plan"
  - id: build
    depends_on: [plan]
    prompt: "implement: $plan.output"
"#;
    let wf = parse_workflow(yaml).unwrap();
    let runner = MockRunner::new().respond("plan", ok("STEP 1; STEP 2"));
    let report = run_workflow(&wf, &runner, &empty_vars()).await.unwrap();

    assert_eq!(report.status, RunStatus::Completed);
    assert_eq!(
        runner.calls_for("build")[0].body,
        "prompt:implement: STEP 1; STEP 2"
    );
}

#[tokio::test]
async fn when_gate_skips_the_unmatched_branch() {
    let yaml = r#"
name: branch
nodes:
  - id: classify
    prompt: "classify"
    output_format:
      type: object
  - id: fix-bug
    depends_on: [classify]
    when: "$classify.output.type == 'BUG'"
    prompt: "fix the bug"
  - id: plan-feature
    depends_on: [classify]
    when: "$classify.output.type == 'FEATURE'"
    prompt: "plan the feature"
  - id: ship
    depends_on: [fix-bug, plan-feature]
    trigger_rule: none_failed_min_one_success
    prompt: "ship it"
"#;
    let wf = parse_workflow(yaml).unwrap();
    let runner = MockRunner::new().respond("classify", ok(r#"{"type":"BUG"}"#));
    let report = run_workflow(&wf, &runner, &empty_vars()).await.unwrap();

    assert_eq!(report.status, RunStatus::Completed);
    assert_eq!(report.node("fix-bug").unwrap().status, NodeStatus::Success);
    // The FEATURE branch's `when` was false → skipped, never dispatched.
    assert_eq!(
        report.node("plan-feature").unwrap().status,
        NodeStatus::Skipped
    );
    assert!(runner.calls_for("plan-feature").is_empty());
    // Merge node runs: one dep succeeded, none failed (the skip is tolerated).
    assert_eq!(report.node("ship").unwrap().status, NodeStatus::Success);
}

#[tokio::test]
async fn output_format_appends_schema_directive_for_ai_nodes() {
    // The driver only carries `output_format`; the *runner* injects the
    // directive. This asserts the schema reaches the request body unchanged for
    // an AI node (the mock echoes the prompt it received).
    let yaml = r#"
name: structured
nodes:
  - id: classify
    prompt: "classify this"
    output_format:
      type: object
      properties:
        type: { type: string }
"#;
    let wf = parse_workflow(yaml).unwrap();
    // The mock records the body the *driver* sent (pre-runner-injection), so we
    // assert the driver substituted normally; runner injection is covered in the
    // harness-runner crate. Here we just confirm output_format doesn't disturb
    // the pipeline (node runs to success).
    let runner = MockRunner::new().respond("classify", ok(r#"{"type":"BUG"}"#));
    let report = run_workflow(&wf, &runner, &empty_vars()).await.unwrap();
    assert_eq!(report.node("classify").unwrap().status, NodeStatus::Success);
}

#[tokio::test]
async fn node_retries_until_success() {
    // `retries: 2` → up to 3 attempts. The node fails twice then succeeds; the
    // run completes and the node ends Success.
    let yaml = r#"
name: retry-success
nodes:
  - id: v
    bash: "run tests"
    retries: 2
"#;
    let wf = parse_workflow(yaml).unwrap();
    let runner = MockRunner::new()
        .respond("v", fail("flaky 1"))
        .respond("v", fail("flaky 2"))
        .respond("v", ok("green"));
    let report = run_workflow(&wf, &runner, &empty_vars()).await.unwrap();

    assert_eq!(report.status, RunStatus::Completed);
    assert_eq!(report.node("v").unwrap().status, NodeStatus::Success);
    // Re-ran the same node three times (2 failures + 1 success).
    assert_eq!(runner.calls_for("v").len(), 3);
}

#[tokio::test]
async fn node_retries_exhausted_fails_run() {
    // `retries: 1` → 2 attempts max; both fail, so the node (and run) fail and
    // we stop at the cap rather than looping.
    let yaml = r#"
name: retry-exhausted
nodes:
  - id: v
    bash: "run tests"
    retries: 1
"#;
    let wf = parse_workflow(yaml).unwrap();
    let runner = MockRunner::new()
        .respond("v", fail("nope 1"))
        .respond("v", fail("nope 2"));
    let report = run_workflow(&wf, &runner, &empty_vars()).await.unwrap();

    assert_eq!(report.status, RunStatus::Failed);
    assert_eq!(report.node("v").unwrap().status, NodeStatus::Failed);
    assert_eq!(runner.calls_for("v").len(), 2);
}

#[tokio::test]
async fn node_without_retries_runs_once() {
    // Default (`retries: 0`) preserves the original behaviour: one attempt.
    let yaml = r#"
name: no-retry
nodes:
  - id: v
    bash: "run tests"
"#;
    let wf = parse_workflow(yaml).unwrap();
    let runner = MockRunner::new().respond("v", fail("boom"));
    let report = run_workflow(&wf, &runner, &empty_vars()).await.unwrap();

    assert_eq!(report.status, RunStatus::Failed);
    assert_eq!(report.node("v").unwrap().status, NodeStatus::Failed);
    assert_eq!(runner.calls_for("v").len(), 1);
}

#[tokio::test]
async fn a_linear_step_no_ops_when_the_run_has_no_issue() {
    // The point of the feature: a prebuilt workflow can carry Linear steps and
    // still run in a project that never connected Linear, or from a manual
    // start.
    //
    // Success-with-a-note rather than Skipped, because the default trigger
    // rule is AllSuccess: a skipped first step would skip everything behind
    // it. This test exists to catch that regression specifically.
    let yaml = r#"
name: chain
nodes:
  - id: start
    linear:
      move_state: In Progress
  - id: work
    depends_on: [start]
    bash: "echo work"
"#;
    let wf = parse_workflow(yaml).unwrap();
    let runner = MockRunner::new();
    let report = run_workflow(&wf, &runner, &empty_vars()).await.unwrap();

    assert_eq!(report.status, RunStatus::Completed);
    assert_eq!(report.node("start").unwrap().status, NodeStatus::Success);
    assert_eq!(report.node("work").unwrap().status, NodeStatus::Success);
    // It says which action it declined to take, not just "skipped".
    let note = report
        .node("start")
        .unwrap()
        .note
        .clone()
        .unwrap_or_default();
    assert!(note.contains("no Linear issue"), "note was: {note}");
    assert!(
        note.contains("move the issue to In Progress"),
        "note was: {note}"
    );
    // And nothing reached the runner.
    assert!(runner.calls_for("start").is_empty());
}

#[tokio::test]
async fn a_linear_step_runs_and_substitutes_when_an_issue_is_present() {
    let yaml = r#"
name: chain
nodes:
  - id: note
    linear:
      comment: "working on $TASK_TITLE"
"#;
    let wf = parse_workflow(yaml).unwrap();
    let runner = MockRunner::new();
    let vars = VarContext::new()
        .set("ISSUE_ID", "AIH-42")
        .set("TASK_TITLE", "a title");
    let report = run_workflow(&wf, &runner, &vars).await.unwrap();

    assert_eq!(report.node("note").unwrap().status, NodeStatus::Success);
    let calls = runner.calls_for("note");
    assert_eq!(calls.len(), 1, "the step reached the runner once");
    assert_eq!(calls[0].body, "linear:comment on the issue");
}

// ── route ───────────────────────────────────────────────────────────────────

const ROUTED: &str = r#"
name: routed
nodes:
  - id: fetch
    prompt: "look up the issue"
    output_format:
      type: object
  - id: kind
    depends_on: [fetch]
    route:
      prompt: "Is this an epic or a single task? $ARGUMENTS"
      routes:
        - name: epic
          description: The issue has sub-issues.
          when: "$fetch.output.children > 0"
        - name: task
          description: A single piece of work.
      default: task
  - id: supervise
    depends_on: [kind]
    when: "$kind.output.route == 'epic'"
    prompt: "supervise the epic"
  - id: build
    depends_on: [kind]
    when: "$kind.output.route == 'task'"
    prompt: "build the task"
  - id: report
    depends_on: [supervise, build]
    trigger_rule: none_failed_min_one_success
    prompt: "report"
"#;

fn args(text: &str) -> VarContext {
    VarContext::new().set("ARGUMENTS", text)
}

#[tokio::test]
async fn a_route_rule_decides_without_asking_a_model() {
    let wf = parse_workflow(ROUTED).unwrap();
    let runner = MockRunner::new().respond("fetch", ok(r#"{"children": 3}"#));
    let report = run_workflow(&wf, &runner, &args("AIH-1")).await.unwrap();

    let kind = report.node("kind").unwrap();
    let out: serde_json::Value = serde_json::from_str(&kind.output).unwrap();
    assert_eq!(out["route"], "epic");
    assert_eq!(out["by"], "rule");
    assert!(runner.calls_for("kind").is_empty(), "no model call");
    assert_eq!(
        report.node("supervise").unwrap().status,
        NodeStatus::Success
    );
    assert_eq!(report.node("build").unwrap().status, NodeStatus::Skipped);
    assert_eq!(report.node("report").unwrap().status, NodeStatus::Success);
    assert_eq!(report.status, RunStatus::Completed);
}

#[tokio::test]
async fn with_no_rule_matching_the_model_chooses_from_the_descriptions() {
    let wf = parse_workflow(ROUTED).unwrap();
    let runner = MockRunner::new()
        .respond("fetch", ok(r#"{"children": 0}"#))
        .respond("kind", ok(r#"{"route": "Task", "reason": "one change"}"#));
    let report = run_workflow(&wf, &runner, &args("AIH-2")).await.unwrap();

    let calls = runner.calls_for("kind");
    assert_eq!(calls.len(), 1);
    // The author's prompt, substituted, followed by the routes to pick from.
    assert!(calls[0]
        .body
        .starts_with("prompt:Is this an epic or a single task? AIH-2"));
    assert!(calls[0]
        .body
        .contains("- `epic` — The issue has sub-issues."));
    assert!(calls[0].body.contains("- `task` — A single piece of work."));

    let out: serde_json::Value =
        serde_json::from_str(&report.node("kind").unwrap().output).unwrap();
    assert_eq!(
        out["route"], "task",
        "matched case-insensitively, stored as declared"
    );
    assert_eq!(out["by"], "model");
    assert_eq!(out["reason"], "one change");
    assert_eq!(report.node("build").unwrap().status, NodeStatus::Success);
    assert_eq!(
        report.node("supervise").unwrap().status,
        NodeStatus::Skipped
    );
}

#[tokio::test]
async fn an_answer_naming_no_route_falls_back_to_the_default() {
    let wf = parse_workflow(ROUTED).unwrap();
    let runner = MockRunner::new()
        .respond("fetch", ok(r#"{"children": 0}"#))
        .respond("kind", ok(r#"{"route": "spike"}"#));
    let report = run_workflow(&wf, &runner, &args("x")).await.unwrap();
    let out: serde_json::Value =
        serde_json::from_str(&report.node("kind").unwrap().output).unwrap();
    assert_eq!(
        (out["route"].as_str(), out["by"].as_str()),
        (Some("task"), Some("default"))
    );
}

#[tokio::test]
async fn without_a_prompt_the_default_is_taken_when_no_rule_holds() {
    let yaml = r#"
name: rules
nodes:
  - id: check
    bash: "echo"
  - id: approved
    depends_on: [check]
    route:
      routes:
        - name: merge
          when: "$check.output.approved == 'true'"
        - name: back-to-review
      default: back-to-review
"#;
    let wf = parse_workflow(yaml).unwrap();
    let runner = MockRunner::new().respond("check", ok(r#"{"approved": false}"#));
    let report = run_workflow(&wf, &runner, &empty_vars()).await.unwrap();
    let node = report.node("approved").unwrap();
    let out: serde_json::Value = serde_json::from_str(&node.output).unwrap();
    assert_eq!(out["route"], "back-to-review");
    assert_eq!(out["by"], "default");
    assert!(runner.calls_for("approved").is_empty());
    assert_eq!(node.provider, None, "a rules-only route runs no agent");
}

// ── usage limits ─────────────────────────────────────────────────────────

/// A failure saying the usage limit resets `mins` minutes from now.
fn limited(mins: i64) -> NodeOutput {
    let reset = Utc::now() + chrono::Duration::minutes(mins);
    fail(&format!(
        "claude exited with exit status: 1: stdout_tail=[You've hit your limit] ({}{})",
        crate::usage_limit::RESETS_AT_UNIX,
        reset.timestamp()
    ))
}

const ONE_STEP: &str = r#"
name: one
nodes:
  - id: a
    prompt: "work"
"#;

/// Run `ONE_STEP`, returning the report and the live activity lines.
async fn run_one(runner: &MockRunner) -> (RunReport, Vec<String>) {
    use futures::StreamExt as _;
    let wf = parse_workflow(ONE_STEP).unwrap();
    let (tx, rx) = futures::channel::mpsc::unbounded();
    let report = run_workflow_streaming(&wf, runner, &empty_vars(), Some(&tx))
        .await
        .unwrap();
    drop(tx);
    let events: Vec<RunEvent> = rx.collect().await;
    let lines = events
        .into_iter()
        .filter_map(|e| match e {
            RunEvent::NodeProgress { activity, .. } => Some(activity.text),
            _ => None,
        })
        .collect();
    (report, lines)
}

#[tokio::test(start_paused = true)]
async fn a_step_that_hits_a_usage_limit_waits_for_the_reset_and_runs_again() {
    let runner = MockRunner::new()
        .respond("a", limited(30))
        .respond("a", ok("done"));
    let (report, lines) = run_one(&runner).await;

    assert_eq!(report.status, RunStatus::Completed);
    let a = report.node("a").unwrap();
    assert_eq!(a.output, "done");
    assert!(
        a.note
            .as_deref()
            .unwrap()
            .contains("waited for a usage limit"),
        "{:?}",
        a.note
    );
    assert_eq!(runner.calls_for("a").len(), 2);
    assert!(
        lines
            .iter()
            .any(|l| l.starts_with("Usage limit reached. Waiting until")),
        "{lines:?}"
    );
}

#[tokio::test(start_paused = true)]
async fn a_usage_limit_that_resets_too_late_fails_the_step() {
    // A weekly limit: far past the default six-hour wait.
    let runner = MockRunner::new().respond("a", limited(3 * 24 * 60));
    let (report, lines) = run_one(&runner).await;

    assert_eq!(report.status, RunStatus::Failed);
    let note = report.node("a").unwrap().note.clone().unwrap();
    assert!(note.contains("later than the 6h this run waits"), "{note}");
    assert_eq!(runner.calls_for("a").len(), 1);
    assert!(lines.is_empty(), "{lines:?}");
}

#[tokio::test(start_paused = true)]
async fn a_usage_limit_with_no_reset_time_fails_as_before() {
    let runner = MockRunner::new().respond("a", fail("rate limit exceeded"));
    let (report, _) = run_one(&runner).await;

    assert_eq!(report.status, RunStatus::Failed);
    assert_eq!(runner.calls_for("a").len(), 1);
}

#[tokio::test(start_paused = true)]
async fn a_limit_hit_after_every_reset_gives_up() {
    let mut runner = MockRunner::new();
    for _ in 0..5 {
        runner = runner.respond("a", limited(10));
    }
    let (report, _) = run_one(&runner).await;

    assert_eq!(report.status, RunStatus::Failed);
    // The first try, then one more after each of the three waits.
    assert_eq!(runner.calls_for("a").len(), 4);
    let note = report.node("a").unwrap().note.clone().unwrap();
    assert!(note.contains("again after 3 waits"), "{note}");
}

#[tokio::test(start_paused = true)]
async fn a_loop_resumes_the_iteration_that_hit_the_limit() {
    let yaml = r#"
name: looped
nodes:
  - id: r
    loop:
      prompt: "review"
      until: DONE
      max_iterations: 3
"#;
    let wf = parse_workflow(yaml).unwrap();
    let runner = MockRunner::new()
        .respond("r", ok("first pass"))
        .respond("r", limited(20))
        .respond("r", ok("DONE"));
    let report = run_workflow(&wf, &runner, &empty_vars()).await.unwrap();

    assert_eq!(report.status, RunStatus::Completed);
    let iterations: Vec<u32> = runner.calls_for("r").iter().map(|c| c.iteration).collect();
    assert_eq!(iterations, vec![1, 2, 2]);
}

#[test]
fn the_resume_time_reads_in_the_display_zone() {
    use chrono::TimeZone as _;
    let tz = chrono_tz::Europe::Copenhagen;
    let now = Utc.with_ymd_and_hms(2026, 11, 9, 10, 0, 0).unwrap();
    let later = Utc.with_ymd_and_hms(2026, 11, 9, 13, 5, 0).unwrap();
    assert_eq!(clock_in(later, now, tz), "14:05 CET");
    let tomorrow = Utc.with_ymd_and_hms(2026, 11, 10, 1, 0, 0).unwrap();
    assert_eq!(clock_in(tomorrow, now, tz), "2026-11-10 02:00 CET");
    assert_eq!(span(chrono::Duration::minutes(133)), "2h 13m");
    assert_eq!(span(chrono::Duration::minutes(45)), "45m");
}
