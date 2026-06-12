use super::*;
use crate::{
    http::AppState, server::HarnessServer, test_helpers::make_test_state,
    thread_manager::ThreadManager,
};
use harness_agents::registry::AgentRegistry;
use harness_core::config::HarnessConfig;
use harness_protocol::{methods::Method, methods::RpcRequest, methods::VALIDATION_ERROR};
use std::sync::Arc;
use tokio::sync::RwLock;

#[tokio::test]
async fn initialized_returns_success() -> anyhow::Result<()> {
    if !crate::test_helpers::db_tests_enabled().await {
        return Ok(());
    }
    let dir = tempfile::tempdir()?;
    let state = make_test_state(dir.path()).await?;

    let req = RpcRequest {
        jsonrpc: "2.0".to_string(),
        // `initialized` is typically a notification, but we allow an `id` for
        // compatibility and return an empty success response in that case.
        id: Some(serde_json::json!(1)),
        method: Method::Initialized,
    };
    let resp = handle_request(&state, req)
        .await
        .expect("expected response for request with id");

    assert!(
        resp.error.is_none(),
        "initialized should succeed, got error: {:?}",
        resp.error
    );
    assert!(resp.result.is_some(), "initialized must return a result");
    Ok(())
}

#[tokio::test]
async fn initialize_then_initialized_succeeds() -> anyhow::Result<()> {
    if !crate::test_helpers::db_tests_enabled().await {
        return Ok(());
    }
    let dir = tempfile::tempdir()?;
    let mut state = make_test_state(dir.path()).await?;
    // Start uninitialised to test the full handshake.
    state.notifications.initialized = Arc::new(std::sync::atomic::AtomicBool::new(false));

    // Step 1: initialize
    let init_req = RpcRequest {
        jsonrpc: "2.0".to_string(),
        id: Some(serde_json::json!(1)),
        method: Method::Initialize,
    };
    let init_resp = handle_request(&state, init_req)
        .await
        .expect("expected response for initialize");
    assert!(
        init_resp.error.is_none(),
        "initialize should succeed: {:?}",
        init_resp.error
    );
    let result = init_resp
        .result
        .as_ref()
        .ok_or_else(|| anyhow::anyhow!("initialize must return result"))?;
    assert!(
        result["capabilities"].is_object(),
        "capabilities should be present"
    );

    // Step 2: initialized (notification — id is None)
    let ack_req = RpcRequest {
        jsonrpc: "2.0".to_string(),
        id: None,
        method: Method::Initialized,
    };
    let ack_resp = handle_request(&state, ack_req).await;
    assert!(
        ack_resp.is_none(),
        "expected no response for initialized notification, got: {ack_resp:?}"
    );
    Ok(())
}

#[tokio::test]
async fn rule_check_returns_warning_when_no_guards_loaded() -> anyhow::Result<()> {
    if !crate::test_helpers::db_tests_enabled().await {
        return Ok(());
    }
    let _lock = crate::test_helpers::HOME_LOCK.lock().await;
    let dir = tempfile::tempdir()?;
    let state = make_test_state(dir.path()).await?;
    let proj_dir = crate::test_helpers::tempdir_in_home("harness-test-")?;

    let req = RpcRequest {
        jsonrpc: "2.0".to_string(),
        id: Some(serde_json::json!(1)),
        method: Method::RuleCheck {
            project_root: proj_dir.path().to_path_buf(),
            files: None,
        },
    };
    let resp = handle_request(&state, req)
        .await
        .expect("expected response for request with id");

    let error = resp
        .error
        .as_ref()
        .ok_or_else(|| anyhow::anyhow!("expected warning error response"))?;
    assert_eq!(error.code, VALIDATION_ERROR);
    assert!(
        error
            .message
            .contains(harness_rules::engine::WARN_NO_GUARDS_REGISTERED),
        "expected explicit warning message, got: {}",
        error.message
    );
    assert!(
        resp.result.is_none(),
        "warning path should not return a success payload"
    );

    let events = state
        .observability
        .events
        .query(&harness_core::types::EventFilters {
            hook: Some("rule_scan".to_string()),
            ..Default::default()
        })
        .await?;
    assert!(
        events.is_empty(),
        "no scan event should be logged when scan request is rejected"
    );
    Ok(())
}

#[tokio::test]
async fn metrics_query_counts_rule_violations_from_events() -> anyhow::Result<()> {
    if !crate::test_helpers::db_tests_enabled().await {
        return Ok(());
    }
    let dir = tempfile::tempdir()?;
    let state = make_test_state(dir.path()).await?;

    let session_id = harness_core::types::SessionId::new();
    // The metrics_query handler scopes violation counts by the latest
    // rule_scan session, so we must log a rule_scan summary event first.
    let scan_event = harness_core::types::Event::new(
        session_id.clone(),
        "rule_scan",
        "RuleEngine",
        harness_core::types::Decision::Block,
    );
    state.observability.events.log(&scan_event).await?;
    for _ in 0..5 {
        let event = harness_core::types::Event::new(
            session_id.clone(),
            "rule_check",
            "SEC-01",
            harness_core::types::Decision::Block,
        );
        state.observability.events.log(&event).await?;
    }

    let req = RpcRequest {
        jsonrpc: "2.0".to_string(),
        id: Some(serde_json::json!(1)),
        method: Method::MetricsQuery {
            filters: harness_core::types::MetricFilters::default(),
        },
    };
    let resp = handle_request(&state, req)
        .await
        .expect("expected response for request with id");

    assert!(resp.error.is_none(), "expected success: {:?}", resp.error);
    let result = resp
        .result
        .ok_or_else(|| anyhow::anyhow!("missing result"))?;
    let coverage = result["dimensions"]["coverage"]
        .as_f64()
        .ok_or_else(|| anyhow::anyhow!("missing coverage"))?;
    assert!(
        coverage < 100.0,
        "coverage should degrade with violations, got {coverage}"
    );
    Ok(())
}

#[tokio::test]
async fn metrics_query_sees_violations_written_via_handler_entry() -> anyhow::Result<()> {
    if !crate::test_helpers::db_tests_enabled().await {
        return Ok(());
    }
    let dir = tempfile::tempdir()?;
    let state = make_test_state(dir.path()).await?;
    let project_root = dir.path().join("project");
    std::fs::create_dir_all(&project_root)?;
    let violations = vec![
        harness_core::types::Violation {
            rule_id: harness_core::types::RuleId::from_str("SEC-01"),
            file: std::path::PathBuf::from("src/lib.rs"),
            line: Some(7),
            message: "critical issue".to_string(),
            severity: harness_core::types::Severity::Critical,
        },
        harness_core::types::Violation {
            rule_id: harness_core::types::RuleId::from_str("U-01"),
            file: std::path::PathBuf::from("src/main.rs"),
            line: None,
            message: "style issue".to_string(),
            severity: harness_core::types::Severity::Low,
        },
    ];
    state
        .observability
        .events
        .persist_rule_scan(&project_root, &violations)
        .await;

    let req = RpcRequest {
        jsonrpc: "2.0".to_string(),
        id: Some(serde_json::json!(1)),
        method: Method::MetricsQuery {
            filters: harness_core::types::MetricFilters::default(),
        },
    };
    let resp = handle_request(&state, req)
        .await
        .expect("expected response for request with id");

    assert!(resp.error.is_none(), "expected success: {:?}", resp.error);
    let result = resp
        .result
        .ok_or_else(|| anyhow::anyhow!("missing result"))?;
    let coverage = result["dimensions"]["coverage"]
        .as_f64()
        .ok_or_else(|| anyhow::anyhow!("missing coverage"))?;
    assert!(
        coverage < 100.0,
        "coverage should degrade with persisted violations, got {coverage}"
    );

    let events = state
        .observability
        .events
        .query(&harness_core::types::EventFilters::default())
        .await?;
    let latest_scan = events
        .iter()
        .rev()
        .find(|event| event.hook == "rule_scan")
        .ok_or_else(|| anyhow::anyhow!("missing rule_scan anchor event"))?;
    let linked_checks = events
        .iter()
        .filter(|event| event.hook == "rule_check" && event.session_id == latest_scan.session_id)
        .count();
    assert_eq!(linked_checks, violations.len());

    Ok(())
}

#[tokio::test]
async fn thread_start_persists_to_db() -> anyhow::Result<()> {
    if !crate::test_helpers::db_tests_enabled().await {
        return Ok(());
    }
    let _lock = crate::test_helpers::HOME_LOCK.lock().await;
    let dir = tempfile::tempdir()?;
    let state = make_test_state(dir.path()).await?;
    let proj_dir = crate::test_helpers::tempdir_in_home("harness-test-")?;
    let canonical_proj = proj_dir.path().canonicalize()?;

    let req = RpcRequest {
        jsonrpc: "2.0".to_string(),
        id: Some(serde_json::json!(1)),
        method: Method::ThreadStart {
            cwd: proj_dir.path().to_path_buf(),
        },
    };
    let resp = handle_request(&state, req)
        .await
        .expect("expected response for request with id");

    assert!(
        resp.error.is_none(),
        "expected success, got error: {:?}",
        resp.error
    );
    let thread_id_str = resp.result.unwrap()["thread_id"]
        .as_str()
        .unwrap()
        .to_string();

    let db = state.core.thread_db.as_ref().unwrap();
    let thread = db
        .get(&thread_id_str)
        .await?
        .expect("thread should be in DB");
    assert_eq!(thread.project_root, canonical_proj);
    Ok(())
}

#[tokio::test]
async fn event_log_then_query_roundtrip() -> anyhow::Result<()> {
    if !crate::test_helpers::db_tests_enabled().await {
        return Ok(());
    }
    let dir = tempfile::tempdir()?;
    let state = make_test_state(dir.path()).await?;
    let session_id = harness_core::types::SessionId::new();
    let event = harness_core::types::Event::new(
        session_id.clone(),
        "pre_tool_use",
        "Edit",
        harness_core::types::Decision::Pass,
    );

    // Log the event via EventLog RPC
    let log_req = RpcRequest {
        jsonrpc: "2.0".to_string(),
        id: Some(serde_json::json!(1)),
        method: Method::EventLog {
            event: Box::new(event.clone()),
        },
    };
    let log_resp = handle_request(&state, log_req)
        .await
        .ok_or_else(|| anyhow::anyhow!("expected response for EventLog"))?;
    assert!(
        log_resp.error.is_none(),
        "EventLog should succeed: {:?}",
        log_resp.error
    );
    let result = log_resp
        .result
        .ok_or_else(|| anyhow::anyhow!("missing result"))?;
    assert_eq!(result["logged"], serde_json::json!(true));
    let event_id = result["event_id"]
        .as_str()
        .ok_or_else(|| anyhow::anyhow!("missing event_id"))?;
    assert_eq!(event_id, event.id.as_str());

    // Query the event via EventQuery RPC
    let query_req = RpcRequest {
        jsonrpc: "2.0".to_string(),
        id: Some(serde_json::json!(2)),
        method: Method::EventQuery {
            filters: harness_core::types::EventFilters {
                session_id: Some(session_id),
                ..Default::default()
            },
        },
    };
    let query_resp = handle_request(&state, query_req)
        .await
        .ok_or_else(|| anyhow::anyhow!("expected response for EventQuery"))?;
    assert!(
        query_resp.error.is_none(),
        "EventQuery should succeed: {:?}",
        query_resp.error
    );
    let events_val = query_resp
        .result
        .ok_or_else(|| anyhow::anyhow!("missing result"))?;
    let events_arr = events_val
        .as_array()
        .ok_or_else(|| anyhow::anyhow!("expected JSON array"))?;
    assert_eq!(events_arr.len(), 1, "expected exactly one event");
    assert_eq!(
        events_arr[0]["id"],
        serde_json::json!(event.id.as_str()),
        "returned event id should match logged event"
    );
    Ok(())
}

#[tokio::test]
async fn pre_init_request_rejected() -> anyhow::Result<()> {
    if !crate::test_helpers::db_tests_enabled().await {
        return Ok(());
    }
    let dir = tempfile::tempdir()?;
    let mut state = make_test_state(dir.path()).await?;
    state.notifications.initialized = Arc::new(std::sync::atomic::AtomicBool::new(false));

    let req = RpcRequest {
        jsonrpc: "2.0".to_string(),
        id: Some(serde_json::json!(1)),
        method: Method::ThreadList,
    };
    let resp = handle_request(&state, req)
        .await
        .expect("should return error response");
    assert!(resp.error.is_some(), "pre-init request should be rejected");
    assert_eq!(
        resp.error.unwrap().code,
        harness_protocol::methods::NOT_INITIALIZED
    );
    Ok(())
}

#[tokio::test]
async fn double_initialize_rejected() -> anyhow::Result<()> {
    if !crate::test_helpers::db_tests_enabled().await {
        return Ok(());
    }
    let dir = tempfile::tempdir()?;
    let state = make_test_state(dir.path()).await?;
    // state.notifications.initialized is already true from make_test_state

    let req = RpcRequest {
        jsonrpc: "2.0".to_string(),
        id: Some(serde_json::json!(1)),
        method: Method::Initialize,
    };
    let resp = handle_request(&state, req)
        .await
        .expect("should return error response");
    assert!(resp.error.is_some(), "double init should be rejected");
    Ok(())
}

#[tokio::test]
async fn initialized_without_initialize_rejected() -> anyhow::Result<()> {
    if !crate::test_helpers::db_tests_enabled().await {
        return Ok(());
    }
    let dir = tempfile::tempdir()?;
    let mut state = make_test_state(dir.path()).await?;
    // Simulate a fresh server where the handshake has not started.
    state.notifications.initializing = Arc::new(std::sync::atomic::AtomicBool::new(false));
    state.notifications.initialized = Arc::new(std::sync::atomic::AtomicBool::new(false));

    let req = RpcRequest {
        jsonrpc: "2.0".to_string(),
        id: Some(serde_json::json!(1)),
        method: Method::Initialized,
    };
    let resp = handle_request(&state, req)
        .await
        .expect("should return error response");
    assert!(
        resp.error.is_some(),
        "initialized without initialize should be rejected"
    );
    assert_eq!(
        resp.error.unwrap().code,
        harness_protocol::methods::INVALID_REQUEST
    );
    Ok(())
}

#[tokio::test]
async fn handshake_unlocks_methods() -> anyhow::Result<()> {
    if !crate::test_helpers::db_tests_enabled().await {
        return Ok(());
    }
    let dir = tempfile::tempdir()?;
    let mut state = make_test_state(dir.path()).await?;
    state.notifications.initialized = Arc::new(std::sync::atomic::AtomicBool::new(false));

    // Before handshake: ThreadList rejected
    let req = RpcRequest {
        jsonrpc: "2.0".to_string(),
        id: Some(serde_json::json!(1)),
        method: Method::ThreadList,
    };
    let resp = handle_request(&state, req).await.unwrap();
    assert!(resp.error.is_some());

    // Initialize
    let req = RpcRequest {
        jsonrpc: "2.0".to_string(),
        id: Some(serde_json::json!(2)),
        method: Method::Initialize,
    };
    let resp = handle_request(&state, req).await.unwrap();
    assert!(resp.error.is_none(), "initialize should succeed");

    // Initialized (complete handshake)
    let req = RpcRequest {
        jsonrpc: "2.0".to_string(),
        id: None,
        method: Method::Initialized,
    };
    handle_request(&state, req).await;

    // After handshake: ThreadList works
    let req = RpcRequest {
        jsonrpc: "2.0".to_string(),
        id: Some(serde_json::json!(3)),
        method: Method::ThreadList,
    };
    let resp = handle_request(&state, req).await.unwrap();
    assert!(resp.error.is_none(), "post-init request should work");
    Ok(())
}

// === Integration tests for previously unrouted methods ===

#[tokio::test]
async fn event_log_records_event() -> anyhow::Result<()> {
    if !crate::test_helpers::db_tests_enabled().await {
        return Ok(());
    }
    let dir = tempfile::tempdir()?;
    let state = make_test_state(dir.path()).await?;
    let session_id = harness_core::types::SessionId::new();
    let event = harness_core::types::Event::new(
        session_id,
        "test_hook",
        "TestTool",
        harness_core::types::Decision::Pass,
    );

    let req = RpcRequest {
        jsonrpc: "2.0".to_string(),
        id: Some(serde_json::json!(1)),
        method: Method::EventLog {
            event: Box::new(event),
        },
    };
    let resp = handle_request(&state, req)
        .await
        .expect("expected response");

    assert!(
        resp.error.is_none(),
        "event_log should succeed: {:?}",
        resp.error
    );
    let result = resp
        .result
        .ok_or_else(|| anyhow::anyhow!("missing result"))?;
    assert_eq!(result["logged"], serde_json::json!(true));
    assert!(result["event_id"].is_string(), "event_id should be present");
    Ok(())
}

#[tokio::test]
async fn event_query_returns_logged_events() -> anyhow::Result<()> {
    if !crate::test_helpers::db_tests_enabled().await {
        return Ok(());
    }
    let dir = tempfile::tempdir()?;
    let state = make_test_state(dir.path()).await?;
    let session_id = harness_core::types::SessionId::new();
    let event = harness_core::types::Event::new(
        session_id,
        "probe_hook",
        "ProbeTarget",
        harness_core::types::Decision::Pass,
    );
    state.observability.events.log(&event).await?;

    let req = RpcRequest {
        jsonrpc: "2.0".to_string(),
        id: Some(serde_json::json!(1)),
        method: Method::EventQuery {
            filters: harness_core::types::EventFilters {
                hook: Some("probe_hook".to_string()),
                ..Default::default()
            },
        },
    };
    let resp = handle_request(&state, req)
        .await
        .expect("expected response");

    assert!(
        resp.error.is_none(),
        "event_query should succeed: {:?}",
        resp.error
    );
    let result = resp
        .result
        .ok_or_else(|| anyhow::anyhow!("missing result"))?;
    let events = result
        .as_array()
        .ok_or_else(|| anyhow::anyhow!("expected array result"))?;
    assert_eq!(events.len(), 1, "should return exactly one matching event");
    Ok(())
}

#[tokio::test]
async fn thread_resume_errors_for_unknown_thread() -> anyhow::Result<()> {
    if !crate::test_helpers::db_tests_enabled().await {
        return Ok(());
    }
    let dir = tempfile::tempdir()?;
    let state = make_test_state(dir.path()).await?;

    let req = RpcRequest {
        jsonrpc: "2.0".to_string(),
        id: Some(serde_json::json!(1)),
        method: Method::ThreadResume {
            thread_id: harness_core::types::ThreadId::new(),
        },
    };
    let resp = handle_request(&state, req)
        .await
        .expect("expected response");
    assert!(
        resp.error.is_some(),
        "resume of unknown thread should return error"
    );
    Ok(())
}

#[tokio::test]
async fn thread_fork_errors_for_unknown_thread() -> anyhow::Result<()> {
    if !crate::test_helpers::db_tests_enabled().await {
        return Ok(());
    }
    let dir = tempfile::tempdir()?;
    let state = make_test_state(dir.path()).await?;

    let req = RpcRequest {
        jsonrpc: "2.0".to_string(),
        id: Some(serde_json::json!(1)),
        method: Method::ThreadFork {
            thread_id: harness_core::types::ThreadId::new(),
            from_turn: None,
        },
    };
    let resp = handle_request(&state, req)
        .await
        .expect("expected response");
    assert!(
        resp.error.is_some(),
        "fork of unknown thread should return error"
    );
    Ok(())
}

#[tokio::test]
async fn thread_compact_errors_for_unknown_thread() -> anyhow::Result<()> {
    if !crate::test_helpers::db_tests_enabled().await {
        return Ok(());
    }
    let dir = tempfile::tempdir()?;
    let state = make_test_state(dir.path()).await?;

    let req = RpcRequest {
        jsonrpc: "2.0".to_string(),
        id: Some(serde_json::json!(1)),
        method: Method::ThreadCompact {
            thread_id: harness_core::types::ThreadId::new(),
        },
    };
    let resp = handle_request(&state, req)
        .await
        .expect("expected response");
    assert!(
        resp.error.is_some(),
        "compact of unknown thread should return error"
    );
    Ok(())
}

#[tokio::test]
async fn turn_steer_returns_not_found_for_unknown_turn() -> anyhow::Result<()> {
    if !crate::test_helpers::db_tests_enabled().await {
        return Ok(());
    }
    let dir = tempfile::tempdir()?;
    let state = make_test_state(dir.path()).await?;

    let req = RpcRequest {
        jsonrpc: "2.0".to_string(),
        id: Some(serde_json::json!(1)),
        method: Method::TurnSteer {
            turn_id: harness_core::types::TurnId::new(),
            instruction: "redirect here".to_string(),
        },
    };
    let resp = handle_request(&state, req)
        .await
        .expect("expected response");
    assert!(
        resp.error.is_some(),
        "steer of unknown turn should return error"
    );
    assert_eq!(
        resp.error.unwrap().code,
        harness_protocol::methods::NOT_FOUND,
        "should return NOT_FOUND for unknown turn"
    );
    Ok(())
}

#[tokio::test]
async fn stats_query_returns_expected_shape() -> anyhow::Result<()> {
    if !crate::test_helpers::db_tests_enabled().await {
        return Ok(());
    }
    let dir = tempfile::tempdir()?;
    let state = make_test_state(dir.path()).await?;

    let req = RpcRequest {
        jsonrpc: "2.0".to_string(),
        id: Some(serde_json::json!(1)),
        method: Method::StatsQuery {
            since: None,
            until: None,
        },
    };
    let resp = handle_request(&state, req)
        .await
        .expect("expected response");
    assert!(
        resp.error.is_none(),
        "stats_query should succeed: {:?}",
        resp.error
    );
    let result = resp
        .result
        .ok_or_else(|| anyhow::anyhow!("missing result"))?;
    assert!(
        result["hook_stats"].is_array(),
        "result should contain hook_stats array"
    );
    assert!(
        result["rule_stats"].is_array(),
        "result should contain rule_stats array"
    );
    Ok(())
}

// --- ExecPlan persistence tests ---

async fn make_test_state_with_plan_db(dir: &std::path::Path) -> anyhow::Result<AppState> {
    let server = Arc::new(HarnessServer::new(
        HarnessConfig::default(),
        ThreadManager::new(),
        AgentRegistry::new("test"),
    ));
    let tasks = crate::task_runner::TaskStore::open(&harness_core::config::dirs::default_db_path(
        dir, "tasks",
    ))
    .await?;
    let events = Arc::new(harness_observe::event_store::EventStore::new(dir).await?);
    let thread_db = crate::thread_db::ThreadDb::open(&harness_core::config::dirs::default_db_path(
        dir, "threads",
    ))
    .await?;
    let plan_db =
        crate::plan_db::PlanDb::open(&harness_core::config::dirs::default_db_path(dir, "plans"))
            .await?;
    let (notification_tx, _) = tokio::sync::broadcast::channel(64);
    let _project_svc_tmp = crate::project_registry::ProjectRegistry::open(
        &harness_core::config::dirs::default_db_path(dir, "projects"),
    )
    .await?;
    let project_svc =
        crate::services::project::DefaultProjectService::new(_project_svc_tmp, dir.to_path_buf());
    let task_svc = crate::services::task::DefaultTaskService::new(tasks.clone());
    let execution_svc = crate::services::execution::DefaultExecutionService::new(
        tasks.clone(),
        server.agent_registry.clone(),
        Arc::new(server.config.clone()),
        events.clone(),
        vec![],
        None,
        Arc::new(crate::task_queue::TaskQueue::new(&Default::default())),
        Arc::new(crate::task_queue::TaskQueue::new(&Default::default())),
        None,
        None,
        None,
        None,
        vec![],
    );
    Ok(AppState {
        core: crate::http::CoreServices {
            server,
            project_root: dir.to_path_buf(),
            home_dir: std::env::var("HOME")
                .map(std::path::PathBuf::from)
                .unwrap_or_else(|_| dir.to_path_buf()),
            tasks,
            thread_db: Some(thread_db),
            plan_db: Some(plan_db),
            plan_cache: std::sync::Arc::new(dashmap::DashMap::new()),
            issue_workflow_store: None,
            project_workflow_store: None,
            workflow_runtime_store: None,
            project_registry: None,
            runtime_state_store: None,
            q_values: None,
            maintenance_active: std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false)),
        },
        engines: crate::http::EngineServices {
            rules: Arc::new(RwLock::new(harness_rules::engine::RuleEngine::new())),
        },
        observability: crate::http::ObservabilityServices {
            events,
            signal_rate_limiter: std::sync::Arc::new(
                crate::http::rate_limit::SignalRateLimiter::new(100),
            ),
            password_reset_rate_limiter: std::sync::Arc::new(
                crate::http::rate_limit::PasswordResetRateLimiter::new(5),
            ),
            review_store: None,
        },
        concurrency: crate::http::ConcurrencyServices {
            task_queue: Arc::new(crate::task_queue::TaskQueue::new(&Default::default())),
            review_task_queue: Arc::new(crate::task_queue::TaskQueue::new(&Default::default())),
            workspace_mgr: None,
        },
        #[cfg(test)]
        _db_state_guard: Some(crate::test_helpers::acquire_db_state_guard().await),
        runtime_hosts: Arc::new(crate::runtime_hosts::RuntimeHostManager::new()),
        runtime_project_cache: Arc::new(
            crate::runtime_project_cache::RuntimeProjectCacheManager::new(),
        ),
        runtime_state_persist_lock: tokio::sync::Mutex::new(()),
        runtime_state_dirty: std::sync::atomic::AtomicBool::new(false),
        notifications: crate::http::NotificationServices {
            notification_tx,
            notification_lagged_total: Arc::new(std::sync::atomic::AtomicU64::new(0)),
            notification_lag_log_every: 1,
            notify_tx: None,
            initializing: Arc::new(std::sync::atomic::AtomicBool::new(true)),
            initialized: Arc::new(std::sync::atomic::AtomicBool::new(true)),
            ws_shutdown_tx: tokio::sync::broadcast::channel(1).0,
        },
        interceptors: vec![],
        startup_statuses: vec![],
        degraded_subsystems: vec![],
        intake: crate::http::IntakeServices {
            feishu_intake: None,
            github_pollers: vec![],
            completion_callback: None,
        },
        project_svc,
        task_svc,
        execution_svc,
    })
}

#[tokio::test]
async fn exec_plan_init_persists_to_db() -> anyhow::Result<()> {
    if !crate::test_helpers::db_tests_enabled().await {
        return Ok(());
    }
    let _lock = crate::test_helpers::HOME_LOCK.lock().await;
    let dir = tempfile::tempdir()?;
    let proj_dir = crate::test_helpers::tempdir_in_home("harness-exec-test-")?;
    let state = make_test_state_with_plan_db(dir.path()).await?;

    let req = RpcRequest {
        jsonrpc: "2.0".to_string(),
        id: Some(serde_json::json!(1)),
        method: Method::ExecPlanInit {
            spec: "# Persist to DB\n\nTest plan.".to_string(),
            project_root: proj_dir.path().to_path_buf(),
        },
    };
    let resp = handle_request(&state, req)
        .await
        .expect("expected response");
    assert!(
        resp.error.is_none(),
        "init should succeed: {:?}",
        resp.error
    );

    let plan_id_str = resp
        .result
        .ok_or_else(|| anyhow::anyhow!("missing result"))?["plan_id"]
        .as_str()
        .ok_or_else(|| anyhow::anyhow!("plan_id not a string"))?
        .to_string();

    let plan_id = harness_core::types::ExecPlanId(plan_id_str);
    let db = state.core.plan_db.as_ref().unwrap();
    let stored = db
        .get(&plan_id)
        .await?
        .ok_or_else(|| anyhow::anyhow!("plan should be in DB"))?;
    assert_eq!(stored.purpose, "Persist to DB");
    Ok(())
}

#[tokio::test]
async fn exec_plan_status_reads_plan_from_memory() -> anyhow::Result<()> {
    if !crate::test_helpers::db_tests_enabled().await {
        return Ok(());
    }
    let _lock = crate::test_helpers::HOME_LOCK.lock().await;
    let dir = tempfile::tempdir()?;
    let proj_dir = crate::test_helpers::tempdir_in_home("harness-exec-test-")?;
    let state = make_test_state_with_plan_db(dir.path()).await?;

    let init_req = RpcRequest {
        jsonrpc: "2.0".to_string(),
        id: Some(serde_json::json!(1)),
        method: Method::ExecPlanInit {
            spec: "# Status Test".to_string(),
            project_root: proj_dir.path().to_path_buf(),
        },
    };
    let init_resp = handle_request(&state, init_req)
        .await
        .ok_or_else(|| anyhow::anyhow!("expected response"))?;
    assert!(
        init_resp.error.is_none(),
        "init should succeed: {:?}",
        init_resp.error
    );
    let plan_id_str = init_resp
        .result
        .ok_or_else(|| anyhow::anyhow!("missing result"))?["plan_id"]
        .as_str()
        .ok_or_else(|| anyhow::anyhow!("plan_id not a string"))?
        .to_string();

    let status_req = RpcRequest {
        jsonrpc: "2.0".to_string(),
        id: Some(serde_json::json!(2)),
        method: Method::ExecPlanStatus {
            plan_id: harness_core::types::ExecPlanId(plan_id_str),
        },
    };
    let status_resp = handle_request(&state, status_req)
        .await
        .ok_or_else(|| anyhow::anyhow!("expected response"))?;
    assert!(
        status_resp.error.is_none(),
        "status should succeed: {:?}",
        status_resp.error
    );
    let result = status_resp
        .result
        .ok_or_else(|| anyhow::anyhow!("missing result"))?;
    assert_eq!(result["purpose"], "Status Test");
    assert_eq!(result["status"], "draft");
    Ok(())
}

#[tokio::test]
async fn exec_plan_update_persists_status_change() -> anyhow::Result<()> {
    if !crate::test_helpers::db_tests_enabled().await {
        return Ok(());
    }
    let _lock = crate::test_helpers::HOME_LOCK.lock().await;
    let dir = tempfile::tempdir()?;
    let proj_dir = crate::test_helpers::tempdir_in_home("harness-exec-test-")?;
    let state = make_test_state_with_plan_db(dir.path()).await?;

    let init_req = RpcRequest {
        jsonrpc: "2.0".to_string(),
        id: Some(serde_json::json!(1)),
        method: Method::ExecPlanInit {
            spec: "# Update Test".to_string(),
            project_root: proj_dir.path().to_path_buf(),
        },
    };
    let init_resp = handle_request(&state, init_req)
        .await
        .ok_or_else(|| anyhow::anyhow!("expected response"))?;
    assert!(
        init_resp.error.is_none(),
        "init should succeed: {:?}",
        init_resp.error
    );
    let plan_id_str = init_resp
        .result
        .ok_or_else(|| anyhow::anyhow!("missing result"))?["plan_id"]
        .as_str()
        .ok_or_else(|| anyhow::anyhow!("plan_id not a string"))?
        .to_string();
    let plan_id = harness_core::types::ExecPlanId(plan_id_str);

    let update_req = RpcRequest {
        jsonrpc: "2.0".to_string(),
        id: Some(serde_json::json!(2)),
        method: Method::ExecPlanUpdate {
            plan_id: plan_id.clone(),
            updates: serde_json::json!({ "action": "activate" }),
        },
    };
    let update_resp = handle_request(&state, update_req)
        .await
        .ok_or_else(|| anyhow::anyhow!("expected response"))?;
    assert!(
        update_resp.error.is_none(),
        "update should succeed: {:?}",
        update_resp.error
    );

    let db = state
        .core
        .plan_db
        .as_ref()
        .ok_or_else(|| anyhow::anyhow!("plan_db should be set"))?;
    let stored = db
        .get(&plan_id)
        .await?
        .ok_or_else(|| anyhow::anyhow!("plan should be in DB"))?;
    assert_eq!(stored.status, harness_core::types::ExecPlanStatus::Active);
    Ok(())
}

#[tokio::test]
async fn exec_plan_survives_simulated_restart() -> anyhow::Result<()> {
    if !crate::test_helpers::db_tests_enabled().await {
        return Ok(());
    }
    let _lock = crate::test_helpers::HOME_LOCK.lock().await;
    let data_dir = tempfile::tempdir()?;
    let proj_dir = crate::test_helpers::tempdir_in_home("harness-exec-test-")?;
    let plan_db_path = harness_core::config::dirs::default_db_path(data_dir.path(), "plans");
    let plan_id_str: String;

    // Session 1: create and activate a plan.
    {
        let state = make_test_state_with_plan_db(data_dir.path()).await?;
        let init_req = RpcRequest {
            jsonrpc: "2.0".to_string(),
            id: Some(serde_json::json!(1)),
            method: Method::ExecPlanInit {
                spec: "# Restart Test".to_string(),
                project_root: proj_dir.path().to_path_buf(),
            },
        };
        let init_resp = handle_request(&state, init_req)
            .await
            .ok_or_else(|| anyhow::anyhow!("expected response"))?;
        assert!(
            init_resp.error.is_none(),
            "init should succeed: {:?}",
            init_resp.error
        );
        plan_id_str = init_resp
            .result
            .ok_or_else(|| anyhow::anyhow!("missing result"))?["plan_id"]
            .as_str()
            .ok_or_else(|| anyhow::anyhow!("plan_id not a string"))?
            .to_string();

        let update_req = RpcRequest {
            jsonrpc: "2.0".to_string(),
            id: Some(serde_json::json!(2)),
            method: Method::ExecPlanUpdate {
                plan_id: harness_core::types::ExecPlanId(plan_id_str.clone()),
                updates: serde_json::json!({ "action": "activate" }),
            },
        };
        let update_resp = handle_request(&state, update_req)
            .await
            .ok_or_else(|| anyhow::anyhow!("expected response"))?;
        assert!(
            update_resp.error.is_none(),
            "activate should succeed: {:?}",
            update_resp.error
        );
    } // state is dropped here — simulates server shutdown

    // Session 2: fresh DB open — simulates restart.
    {
        let plan_db = crate::plan_db::PlanDb::open(&plan_db_path).await?;
        let persisted = plan_db.list().await?;
        let mut map = std::collections::HashMap::new();
        for plan in persisted {
            map.insert(plan.id.clone(), plan);
        }

        let pid = harness_core::types::ExecPlanId(plan_id_str.clone());
        let recovered = map
            .get(&pid)
            .ok_or_else(|| anyhow::anyhow!("plan should survive restart"))?;
        assert_eq!(recovered.purpose, "Restart Test");
        assert_eq!(
            recovered.status,
            harness_core::types::ExecPlanStatus::Active
        );
    }
    Ok(())
}

#[tokio::test]
async fn exec_plan_status_fallback_to_db_when_not_in_memory() -> anyhow::Result<()> {
    if !crate::test_helpers::db_tests_enabled().await {
        return Ok(());
    }
    let _lock = crate::test_helpers::HOME_LOCK.lock().await;
    let data_dir = tempfile::tempdir()?;
    let proj_dir = crate::test_helpers::tempdir_in_home("harness-exec-test-")?;
    let plan_db_path = harness_core::config::dirs::default_db_path(data_dir.path(), "plans");

    // Insert a plan directly into the DB without going through the in-memory HashMap.
    let plan = harness_exec::plan::ExecPlan::from_spec("# Direct DB Insert", proj_dir.path())?;
    let plan_id = plan.id.clone();
    {
        let db = crate::plan_db::PlanDb::open(&plan_db_path).await?;
        db.upsert(&plan).await?;
    }

    // Create state with the DB but empty HashMap (simulates post-restart before warmup).
    let state = make_test_state_with_plan_db(data_dir.path()).await?;

    // ExecPlanStatus must fall back to DB when plan is absent from HashMap.
    let status_req = RpcRequest {
        jsonrpc: "2.0".to_string(),
        id: Some(serde_json::json!(1)),
        method: Method::ExecPlanStatus { plan_id },
    };
    let resp = handle_request(&state, status_req)
        .await
        .ok_or_else(|| anyhow::anyhow!("expected response"))?;
    assert!(
        resp.error.is_none(),
        "status should fall back to DB: {:?}",
        resp.error
    );
    let result = resp
        .result
        .ok_or_else(|| anyhow::anyhow!("missing result"))?;
    assert_eq!(result["purpose"], "Direct DB Insert");
    Ok(())
}
