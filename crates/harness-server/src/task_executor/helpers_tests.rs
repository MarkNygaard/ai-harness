use super::*;
use async_trait::async_trait;
use harness_core::agent::{AgentRequest, AgentResponse, CodeAgent, StreamItem};
use harness_core::error::HarnessError;
use harness_core::interceptor::{
    InterceptResult, PostExecuteResult, PostToolUseResult, ToolUseEvent, TurnInterceptor,
};
use harness_core::types::{Decision, TokenUsage, TurnFailureKind};
use std::sync::{
    atomic::{AtomicBool, AtomicU32, Ordering},
    Arc,
};

// ── Mock helpers ──────────────────────────────────────────────────────────────

fn make_req() -> AgentRequest {
    AgentRequest {
        prompt: "test prompt".to_string(),
        project_root: std::path::PathBuf::from("/tmp"),
        ..Default::default()
    }
}

fn make_resp() -> AgentResponse {
    AgentResponse {
        output: "done".to_string(),
        stderr: String::new(),
        items: vec![],
        token_usage: TokenUsage::default(),
        model: "mock".to_string(),
        exit_code: Some(0),
    }
}

// ── Mock interceptors ─────────────────────────────────────────────────────────

struct PassInterceptor;

#[async_trait]
impl TurnInterceptor for PassInterceptor {
    fn name(&self) -> &str {
        "pass"
    }
    async fn pre_execute(&self, _req: &AgentRequest) -> InterceptResult {
        InterceptResult::pass()
    }
}

struct BlockInterceptor {
    reason: String,
}

impl BlockInterceptor {
    fn new(reason: impl Into<String>) -> Self {
        Self {
            reason: reason.into(),
        }
    }
}

#[async_trait]
impl TurnInterceptor for BlockInterceptor {
    fn name(&self) -> &str {
        "block"
    }
    async fn pre_execute(&self, _req: &AgentRequest) -> InterceptResult {
        InterceptResult::block(self.reason.clone())
    }
}

struct WarnInterceptor;

#[async_trait]
impl TurnInterceptor for WarnInterceptor {
    fn name(&self) -> &str {
        "warn"
    }
    async fn pre_execute(&self, _req: &AgentRequest) -> InterceptResult {
        InterceptResult::warn("non-fatal warning")
    }
}

struct ModifyingInterceptor;

#[async_trait]
impl TurnInterceptor for ModifyingInterceptor {
    fn name(&self) -> &str {
        "modifying"
    }
    async fn pre_execute(&self, req: &AgentRequest) -> InterceptResult {
        let mut modified = req.clone();
        modified.prompt = format!("MODIFIED: {}", req.prompt);
        InterceptResult {
            decision: Decision::Pass,
            reason: None,
            request: Some(modified),
        }
    }
}

struct FailingPostInterceptor;

#[async_trait]
impl TurnInterceptor for FailingPostInterceptor {
    fn name(&self) -> &str {
        "failing_post"
    }
    async fn pre_execute(&self, _req: &AgentRequest) -> InterceptResult {
        InterceptResult::pass()
    }
    async fn post_execute(&self, _req: &AgentRequest, _resp: &AgentResponse) -> PostExecuteResult {
        PostExecuteResult::fail("validation failed")
    }
}

struct CountingErrorInterceptor {
    count: Arc<AtomicU32>,
}

#[async_trait]
impl TurnInterceptor for CountingErrorInterceptor {
    fn name(&self) -> &str {
        "counting_error"
    }
    async fn pre_execute(&self, _req: &AgentRequest) -> InterceptResult {
        InterceptResult::pass()
    }
    async fn on_error(&self, _req: &AgentRequest, _error: &str) {
        self.count.fetch_add(1, Ordering::SeqCst);
    }
}

struct ViolatingToolInterceptor;

#[async_trait]
impl TurnInterceptor for ViolatingToolInterceptor {
    fn name(&self) -> &str {
        "violating_tool"
    }
    async fn pre_execute(&self, _req: &AgentRequest) -> InterceptResult {
        InterceptResult::pass()
    }
    async fn post_tool_use(
        &self,
        _event: &ToolUseEvent,
        _root: &std::path::Path,
    ) -> PostToolUseResult {
        PostToolUseResult::with_violations("found a violation")
    }
}

struct CallTrackedInterceptor {
    called: Arc<AtomicBool>,
}

#[async_trait]
impl TurnInterceptor for CallTrackedInterceptor {
    fn name(&self) -> &str {
        "call_tracked"
    }
    async fn pre_execute(&self, _req: &AgentRequest) -> InterceptResult {
        self.called.store(true, Ordering::SeqCst);
        InterceptResult::pass()
    }
}

struct NamedFailingPostInterceptor {
    name_str: &'static str,
    error_msg: &'static str,
}

#[async_trait]
impl TurnInterceptor for NamedFailingPostInterceptor {
    fn name(&self) -> &str {
        self.name_str
    }
    async fn pre_execute(&self, _req: &AgentRequest) -> InterceptResult {
        InterceptResult::pass()
    }
    async fn post_execute(&self, _req: &AgentRequest, _resp: &AgentResponse) -> PostExecuteResult {
        PostExecuteResult::fail(self.error_msg)
    }
}

fn wrap<T: TurnInterceptor + 'static>(t: T) -> Arc<dyn TurnInterceptor> {
    Arc::new(t)
}

enum StreamScenario {
    StreamingSuccess,
    NonStreamingSuccess,
    ArtifactSuccess,
    UpstreamFailure,
}

struct TestStreamingAgent {
    scenario: StreamScenario,
}

#[async_trait]
impl CodeAgent for TestStreamingAgent {
    fn name(&self) -> &str {
        "test-agent"
    }

    fn capabilities(&self) -> Vec<harness_core::types::Capability> {
        vec![]
    }

    async fn execute(&self, _req: AgentRequest) -> harness_core::error::Result<AgentResponse> {
        Ok(make_resp())
    }

    async fn execute_stream(
        &self,
        _req: AgentRequest,
        tx: tokio::sync::mpsc::Sender<StreamItem>,
    ) -> harness_core::error::Result<()> {
        match self.scenario {
            StreamScenario::StreamingSuccess => {
                tx.send(StreamItem::MessageDelta {
                    text: "hello".to_string(),
                })
                .await
                .map_err(|e| HarnessError::AgentExecution(format!("stream closed: {e}")))?;
                tx.send(StreamItem::Done)
                    .await
                    .map_err(|e| HarnessError::AgentExecution(format!("stream closed: {e}")))?;
                Ok(())
            }
            StreamScenario::NonStreamingSuccess => {
                tx.send(StreamItem::ItemCompleted {
                    item: harness_core::types::Item::AgentReasoning {
                        content: "final response".to_string(),
                    },
                })
                .await
                .map_err(|e| HarnessError::AgentExecution(format!("stream closed: {e}")))?;
                tx.send(StreamItem::Done)
                    .await
                    .map_err(|e| HarnessError::AgentExecution(format!("stream closed: {e}")))?;
                Ok(())
            }
            StreamScenario::ArtifactSuccess => {
                tx.send(StreamItem::ItemCompleted {
                    item: harness_core::types::Item::ToolCall {
                        name: "test-tool".to_string(),
                        input: serde_json::json!({}),
                        output: Some(serde_json::json!("ok")),
                    },
                })
                .await
                .map_err(|e| HarnessError::AgentExecution(format!("stream closed: {e}")))?;
                tx.send(StreamItem::Done)
                    .await
                    .map_err(|e| HarnessError::AgentExecution(format!("stream closed: {e}")))?;
                Ok(())
            }
            StreamScenario::UpstreamFailure => Err(HarnessError::AgentExecution(
                "API returned 500: upstream exploded".to_string(),
            )),
        }
    }
}

// ── run_pre_execute ───────────────────────────────────────────────────────────

#[tokio::test]
async fn run_pre_execute_passes_with_pass_interceptor() {
    let interceptors = vec![wrap(PassInterceptor)];
    let result = run_pre_execute(&interceptors, make_req()).await;
    assert!(result.is_ok());
}

#[tokio::test]
async fn run_pre_execute_fails_with_blocking_interceptor() {
    let interceptors = vec![wrap(BlockInterceptor::new("not allowed"))];
    let result = run_pre_execute(&interceptors, make_req()).await;
    assert!(result.is_err());
    let msg = result.unwrap_err().to_string();
    assert!(msg.contains("Blocked by interceptor"));
    assert!(msg.contains("not allowed"));
}

#[tokio::test]
async fn run_pre_execute_warn_does_not_block() {
    let interceptors = vec![wrap(WarnInterceptor)];
    let result = run_pre_execute(&interceptors, make_req()).await;
    assert!(result.is_ok(), "warn should not block execution");
}

#[tokio::test]
async fn run_pre_execute_returns_modified_request() {
    let interceptors = vec![wrap(ModifyingInterceptor)];
    let req = make_req();
    let result = run_pre_execute(&interceptors, req).await.unwrap();
    assert!(
        result.prompt.starts_with("MODIFIED:"),
        "interceptor should have modified the prompt"
    );
}

#[tokio::test]
async fn run_pre_execute_empty_interceptors_returns_original() {
    let interceptors: Vec<Arc<dyn TurnInterceptor>> = vec![];
    let req = make_req();
    let result = run_pre_execute(&interceptors, req.clone()).await.unwrap();
    assert_eq!(result.prompt, req.prompt);
}

#[tokio::test]
async fn run_pre_execute_stops_chain_at_first_block() {
    let second_called = Arc::new(AtomicBool::new(false));
    let interceptors: Vec<Arc<dyn TurnInterceptor>> = vec![
        Arc::new(BlockInterceptor::new("early block")),
        Arc::new(CallTrackedInterceptor {
            called: second_called.clone(),
        }),
    ];
    let result = run_pre_execute(&interceptors, make_req()).await;
    assert!(result.is_err(), "should fail due to block");
    assert!(
        !second_called.load(Ordering::SeqCst),
        "interceptor after block must not be called"
    );
}

// ── run_post_execute ──────────────────────────────────────────────────────────

#[tokio::test]
async fn run_post_execute_returns_none_when_all_pass() {
    let interceptors = vec![wrap(PassInterceptor)];
    let result = run_post_execute(&interceptors, &make_req(), &make_resp()).await;
    assert!(result.is_none());
}

#[tokio::test]
async fn run_post_execute_returns_error_when_interceptor_fails() {
    let interceptors = vec![wrap(FailingPostInterceptor)];
    let result = run_post_execute(&interceptors, &make_req(), &make_resp()).await;
    assert!(result.is_some());
    let err = result.unwrap();
    assert!(
        err.contains("failing_post"),
        "error should name the interceptor"
    );
    assert!(err.contains("validation failed"));
}

#[tokio::test]
async fn run_post_execute_returns_first_failure_only() {
    let interceptors: Vec<Arc<dyn TurnInterceptor>> = vec![
        Arc::new(NamedFailingPostInterceptor {
            name_str: "first_fail",
            error_msg: "first error",
        }),
        Arc::new(NamedFailingPostInterceptor {
            name_str: "second_fail",
            error_msg: "second error",
        }),
    ];
    let result = run_post_execute(&interceptors, &make_req(), &make_resp()).await;
    let error = result.expect("should have an error");
    assert!(
        error.contains("first_fail"),
        "should name the first interceptor"
    );
    assert!(
        error.contains("first error"),
        "should contain the first error message"
    );
    assert!(
        !error.contains("second_fail"),
        "second interceptor must not run"
    );
    assert!(
        !error.contains("second error"),
        "second interceptor must not run"
    );
}

#[tokio::test]
async fn run_post_execute_empty_interceptors_returns_none() {
    let interceptors: Vec<Arc<dyn TurnInterceptor>> = vec![];
    let result = run_post_execute(&interceptors, &make_req(), &make_resp()).await;
    assert!(result.is_none());
}

// ── run_on_error ──────────────────────────────────────────────────────────────

#[tokio::test]
async fn run_on_error_calls_all_interceptors() {
    let count = Arc::new(AtomicU32::new(0));
    let interceptors: Vec<Arc<dyn TurnInterceptor>> = vec![
        Arc::new(CountingErrorInterceptor {
            count: count.clone(),
        }),
        Arc::new(CountingErrorInterceptor {
            count: count.clone(),
        }),
    ];
    run_on_error(&interceptors, &make_req(), "some error").await;
    assert_eq!(
        count.load(Ordering::SeqCst),
        2,
        "both interceptors should have been called"
    );
}

#[tokio::test]
async fn run_on_error_empty_interceptors_is_noop() {
    let interceptors: Vec<Arc<dyn TurnInterceptor>> = vec![];
    run_on_error(&interceptors, &make_req(), "error").await;
}

// ── run_post_tool_use ─────────────────────────────────────────────────────────

#[tokio::test]
async fn run_post_tool_use_returns_none_when_no_violations() {
    let interceptors = vec![wrap(PassInterceptor)];
    let event = ToolUseEvent {
        tool_name: "write_file".to_string(),
        affected_files: vec![],
        session_id: None,
    };
    let result = run_post_tool_use(&interceptors, &event, std::path::Path::new("/tmp")).await;
    assert!(result.is_none());
}

#[tokio::test]
async fn run_post_tool_use_returns_violation_feedback() {
    let interceptors = vec![wrap(ViolatingToolInterceptor)];
    let event = ToolUseEvent {
        tool_name: "write_file".to_string(),
        affected_files: vec![std::path::PathBuf::from("foo.rs")],
        session_id: None,
    };
    let result = run_post_tool_use(&interceptors, &event, std::path::Path::new("/tmp")).await;
    assert!(result.is_some());
    let feedback = result.unwrap();
    assert!(
        feedback.contains("violating_tool"),
        "feedback should name the interceptor"
    );
    assert!(feedback.contains("found a violation"));
}

#[tokio::test]
async fn run_post_tool_use_empty_interceptors_returns_none() {
    let interceptors: Vec<Arc<dyn TurnInterceptor>> = vec![];
    let event = ToolUseEvent {
        tool_name: "read_file".to_string(),
        affected_files: vec![],
        session_id: None,
    };
    let result = run_post_tool_use(&interceptors, &event, std::path::Path::new("/tmp")).await;
    assert!(result.is_none());
}

// ── run_agent_streaming ──────────────────────────────────────────────────────

#[tokio::test]
async fn run_agent_streaming_records_first_token_latency_for_streaming_output() {
    if !crate::test_helpers::db_tests_enabled().await {
        return;
    }
    let dir = tempfile::tempdir().expect("tempdir");
    let store = crate::task_runner::TaskStore::open(&dir.path().join("tasks.db"))
        .await
        .expect("task store");
    let task_id = crate::task_runner::TaskId::new();
    let task = crate::task_runner::TaskState::new(task_id.clone());
    store.insert(&task).await;

    let result = run_agent_streaming(
        &TestStreamingAgent {
            scenario: StreamScenario::StreamingSuccess,
        },
        make_req(),
        &task_id,
        &store,
        1,
        chrono::Utc::now(),
        chrono::Utc::now(),
    )
    .await
    .expect("streaming success");

    assert_eq!(result.response.output, "hello");
    assert!(
        result.telemetry.first_token_latency_ms.is_some(),
        "streaming output should record first token latency"
    );
    assert!(result.telemetry.first_output_at.is_some());
}

#[tokio::test]
async fn run_agent_streaming_leaves_first_token_latency_empty_for_non_streaming_output() {
    if !crate::test_helpers::db_tests_enabled().await {
        return;
    }
    let dir = tempfile::tempdir().expect("tempdir");
    let store = crate::task_runner::TaskStore::open(&dir.path().join("tasks.db"))
        .await
        .expect("task store");
    let task_id = crate::task_runner::TaskId::new();
    let task = crate::task_runner::TaskState::new(task_id.clone());
    store.insert(&task).await;

    let result = run_agent_streaming(
        &TestStreamingAgent {
            scenario: StreamScenario::NonStreamingSuccess,
        },
        make_req(),
        &task_id,
        &store,
        1,
        chrono::Utc::now(),
        chrono::Utc::now(),
    )
    .await
    .expect("non-streaming success");

    assert_eq!(result.response.output, "final response");
    assert_eq!(result.telemetry.first_token_latency_ms, None);
    assert!(result.telemetry.first_output_at.is_some());
}

#[test]
fn telemetry_for_timeout_omits_first_output() {
    let now = chrono::Utc::now();
    let telemetry = telemetry_for_timeout(now, now, now, Some(2));
    assert_eq!(telemetry.first_output_at, None);
    assert_eq!(telemetry.first_token_latency_ms, None);
    assert_eq!(telemetry.retry_count, Some(2));
}

#[tokio::test]
async fn run_agent_streaming_classifies_upstream_failure() {
    if !crate::test_helpers::db_tests_enabled().await {
        return;
    }
    let dir = tempfile::tempdir().expect("tempdir");
    let store = crate::task_runner::TaskStore::open(&dir.path().join("tasks.db"))
        .await
        .expect("task store");
    let task_id = crate::task_runner::TaskId::new();
    let task = crate::task_runner::TaskState::new(task_id.clone());
    store.insert(&task).await;

    let failure = run_agent_streaming(
        &TestStreamingAgent {
            scenario: StreamScenario::UpstreamFailure,
        },
        make_req(),
        &task_id,
        &store,
        1,
        chrono::Utc::now(),
        chrono::Utc::now(),
    )
    .await
    .expect_err("upstream failure expected");

    assert_eq!(failure.failure.kind, TurnFailureKind::Upstream);
    assert_eq!(failure.failure.upstream_status, Some(500));
}

#[tokio::test]
async fn run_agent_streaming_can_skip_artifact_persistence() {
    if !crate::test_helpers::db_tests_enabled().await {
        return;
    }
    let dir = tempfile::tempdir().expect("tempdir");
    let store = crate::task_runner::TaskStore::open(&dir.path().join("tasks.db"))
        .await
        .expect("task store");
    let task_id = crate::task_runner::TaskId::new();
    let task = crate::task_runner::TaskState::new(task_id.clone());
    store.insert(&task).await;

    run_agent_streaming_with_options(
        &TestStreamingAgent {
            scenario: StreamScenario::ArtifactSuccess,
        },
        make_req(),
        &task_id,
        &store,
        1,
        chrono::Utc::now(),
        chrono::Utc::now(),
        RunAgentStreamingOptions {
            persist_artifacts: false,
            backfill_auto_fix_issue: false,
        },
    )
    .await
    .expect("artifact streaming success");

    let artifacts = store.list_artifacts(&task_id).await.expect("artifacts");
    assert!(
        artifacts.is_empty(),
        "artifact persistence should be disabled when requested"
    );
}

#[test]
fn inject_project_context_appends_agents_and_claude_instructions() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("AGENTS.md"), "Always run cargo check").unwrap();
    std::fs::write(dir.path().join("CLAUDE.md"), "Use English only").unwrap();

    let result = inject_project_context_into_prompt(dir.path(), "Base task".to_string());

    assert!(result.contains("Base task"));
    assert!(result.contains("## Project Instructions"));
    assert!(result.contains("Always run cargo check"));
    assert!(result.contains("Use English only"));
}

#[test]
fn inject_project_context_noops_without_instruction_files() {
    let dir = tempfile::tempdir().unwrap();
    let result = inject_project_context_into_prompt(dir.path(), "Base task".to_string());
    assert_eq!(result, "Base task");
}

// ── truncate_validation_error ─────────────────────────────────────────────────

#[test]
fn truncate_short_error_passes_through() {
    assert_eq!(truncate_validation_error("short", 100), "short");
}

#[test]
fn truncate_long_error_includes_summary() {
    let input = "x".repeat(200);
    let result = truncate_validation_error(&input, 50);
    assert!(result.starts_with(&"x".repeat(50)));
    assert!(result.contains("(output truncated, 200 chars total)"));
}

// ── process_stream_item: ApprovalRequest ─────────────────────────────────────

#[tokio::test]
async fn process_stream_item_approval_request_appends_item_and_emits_notification() {
    use crate::server::HarnessServer;
    use crate::thread_manager::ThreadManager;
    use harness_agents::registry::AgentRegistry;
    use harness_core::agent::StreamItem;
    use harness_core::config::HarnessConfig;
    use harness_core::types::AgentId;
    use harness_protocol::notifications::RpcNotification;
    use std::path::PathBuf;
    use std::sync::Arc;
    use tokio::sync::broadcast;

    let server = Arc::new(HarnessServer::new(
        HarnessConfig::default(),
        ThreadManager::new(),
        AgentRegistry::new(""),
    ));
    let thread_id = server.thread_manager.start_thread(PathBuf::from("/tmp"));
    let turn_id = server
        .thread_manager
        .start_turn(&thread_id, "task".to_string(), AgentId::new())
        .unwrap();

    let (notification_tx, mut notification_rx) = broadcast::channel::<RpcNotification>(16);

    process_stream_item(
        &server,
        &None,
        &None,
        &notification_tx,
        &thread_id,
        &turn_id,
        StreamItem::ApprovalRequest {
            id: "req-42".to_string(),
            command: "rm -rf /tmp/test".to_string(),
        },
    )
    .await;

    // Check item was appended.
    let turn = server
        .thread_manager
        .get_turn(&thread_id, &turn_id)
        .expect("turn must exist");
    let has_approval = turn.items.iter().any(|item| {
        matches!(
            item,
            harness_core::types::Item::ApprovalRequest {
                action,
                approved: None,
                ..
            } if action == "rm -rf /tmp/test"
        )
    });
    assert!(
        has_approval,
        "ApprovalRequest item must be appended to turn"
    );

    // Check notification was emitted.
    let notif = notification_rx
        .try_recv()
        .expect("notification must be emitted");
    match notif.notification {
        harness_protocol::notifications::Notification::ApprovalRequest {
            turn_id: notif_turn_id,
            request_id,
            command,
        } => {
            assert_eq!(notif_turn_id, turn_id);
            assert_eq!(request_id, "req-42");
            assert_eq!(command, "rm -rf /tmp/test");
        }
        other => panic!("expected ApprovalRequest notification, got {other:?}"),
    }
}

#[tokio::test]
async fn process_stream_item_warning_emits_notification() {
    use crate::server::HarnessServer;
    use crate::thread_manager::ThreadManager;
    use harness_agents::registry::AgentRegistry;
    use harness_core::agent::StreamItem;
    use harness_core::config::HarnessConfig;
    use harness_core::types::AgentId;
    use harness_protocol::notifications::RpcNotification;
    use std::path::PathBuf;
    use std::sync::Arc;
    use tokio::sync::broadcast;

    let server = Arc::new(HarnessServer::new(
        HarnessConfig::default(),
        ThreadManager::new(),
        AgentRegistry::new(""),
    ));
    let thread_id = server.thread_manager.start_thread(PathBuf::from("/tmp"));
    let turn_id = server
        .thread_manager
        .start_turn(&thread_id, "task".to_string(), AgentId::new())
        .unwrap();
    let (notification_tx, mut notification_rx) = broadcast::channel::<RpcNotification>(16);

    process_stream_item(
        &server,
        &None,
        &None,
        &notification_tx,
        &thread_id,
        &turn_id,
        StreamItem::Warning {
            message: "careful".to_string(),
        },
    )
    .await;

    let notif = notification_rx
        .try_recv()
        .expect("warning notification must be emitted");
    match notif.notification {
        harness_protocol::notifications::Notification::Warning {
            turn_id: notif_turn_id,
            message,
        } => {
            assert_eq!(notif_turn_id, turn_id);
            assert_eq!(message, "careful");
        }
        other => panic!("expected Warning notification, got {other:?}"),
    }
}

#[tokio::test]
async fn process_stream_item_tool_output_delta_emits_notification() {
    use crate::server::HarnessServer;
    use crate::thread_manager::ThreadManager;
    use harness_agents::registry::AgentRegistry;
    use harness_core::agent::StreamItem;
    use harness_core::config::HarnessConfig;
    use harness_core::types::AgentId;
    use harness_protocol::notifications::RpcNotification;
    use std::path::PathBuf;
    use std::sync::Arc;
    use tokio::sync::broadcast;

    let server = Arc::new(HarnessServer::new(
        HarnessConfig::default(),
        ThreadManager::new(),
        AgentRegistry::new(""),
    ));
    let thread_id = server.thread_manager.start_thread(PathBuf::from("/tmp"));
    let turn_id = server
        .thread_manager
        .start_turn(&thread_id, "task".to_string(), AgentId::new())
        .unwrap();
    let (notification_tx, mut notification_rx) = broadcast::channel::<RpcNotification>(16);

    process_stream_item(
        &server,
        &None,
        &None,
        &notification_tx,
        &thread_id,
        &turn_id,
        StreamItem::ToolOutputDelta {
            item_id: "item-1".to_string(),
            text: "cargo check\n".to_string(),
        },
    )
    .await;

    let notif = notification_rx
        .try_recv()
        .expect("tool output notification must be emitted");
    match notif.notification {
        harness_protocol::notifications::Notification::ToolOutputDelta {
            turn_id: notif_turn_id,
            item_id,
            text,
        } => {
            assert_eq!(notif_turn_id, turn_id);
            assert_eq!(item_id, "item-1");
            assert_eq!(text, "cargo check\n");
        }
        other => panic!("expected ToolOutputDelta notification, got {other:?}"),
    }
}
