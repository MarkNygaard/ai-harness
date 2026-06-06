use axum::{
    extract::DefaultBodyLimit,
    middleware,
    routing::{get, post},
    Extension, Router,
};
use std::sync::Arc;

use super::{
    auth, categories_routes, credentials_routes, get_issue_workflow_by_issue,
    get_issue_workflow_by_pr, get_project_workflow_by_project, get_task, get_task_artifacts,
    get_task_prompts, get_task_proof, get_workflow_runtime_tree, github_webhook, handle_rpc,
    health_check, ingest_signal, intake_status, list_tasks, password_reset, project_queue_stats,
    project_authoring_routes, runs_routes, state::AppState, stream_task_sse,
    task_mutation_routes, task_routes, workflows_routes,
};

pub(super) fn build_router(state: Arc<AppState>) -> Router {
    // Self-contained state for the runs API (attached as an Extension so it
    // doesn't entangle AppState). Echo runs need no real agents; real runs use
    // a registry built from config. Store connects lazily on first use.
    let runs_state = {
        let config = &state.core.server.config;
        let db_url = config.server.database_url.clone();
        let agent_registry = Arc::new(harness_runner::build_agent_registry(
            config,
            harness_core::config::agents::SandboxMode::DangerFullAccess,
        ));
        // Credential-encryption key for UI-entered provider creds. Absent =>
        // the credentials API is disabled (503) and nothing is materialized.
        let secret_key = std::env::var("HARNESS_SECRET_KEY").ok().and_then(|b64| {
            match harness_persist::CredentialStore::key_from_base64(&b64) {
                Ok(k) => Some(k),
                Err(e) => {
                    tracing::warn!("ignoring invalid HARNESS_SECRET_KEY: {e}");
                    None
                }
            }
        });
        Arc::new(runs_routes::RunsState::new(
            db_url,
            agent_registry,
            state.core.project_root.clone(),
            secret_key,
        ))
    };
    Router::new()
        .route("/", get(crate::dashboard::index))
        .route("/overview", get(crate::overview::index))
        .route("/worktrees", get(crate::dashboard::index))
        .route(
            "/assets/{filename}",
            axum::routing::get(crate::assets::serve),
        )
        .route("/favicon.ico", get(crate::dashboard::favicon))
        .route("/health", get(health_check))
        .route("/rpc", post(handle_rpc))
        .route("/ws", get(crate::websocket::ws_handler))
        .route("/tasks", post(task_routes::create_task))
        .route("/tasks", get(list_tasks))
        .route("/tasks/batch", post(task_routes::create_tasks_batch))
        .route("/tasks/{id}", get(get_task))
        .route("/tasks/{id}/cancel", post(task_routes::cancel_task))
        .route("/tasks/{id}/merge", post(task_mutation_routes::merge_task))
        .route("/tasks/{id}/artifacts", get(get_task_artifacts))
        .route("/tasks/{id}/prompts", get(get_task_prompts))
        .route("/tasks/{id}/proof", get(get_task_proof))
        .route("/tasks/{id}/stream", get(stream_task_sse))
        .route(
            "/projects",
            post(crate::handlers::projects::register_project)
                .get(crate::handlers::projects::list_projects),
        )
        .route(
            "/projects/{id}",
            get(crate::handlers::projects::get_project)
                .delete(crate::handlers::projects::delete_project),
        )
        .route("/projects/queue-stats", get(project_queue_stats))
        .route("/api/dashboard", get(crate::handlers::dashboard::dashboard))
        .route("/api/overview", get(crate::handlers::overview::overview))
        .route("/api/worktrees", get(crate::handlers::worktrees::worktrees))
        .route(
            "/api/operator-snapshot",
            get(crate::handlers::operator_snapshot::operator_snapshot),
        )
        .route("/api/intake", get(intake_status))
        .route(
            "/api/workflows/issues/by-issue",
            get(get_issue_workflow_by_issue),
        )
        .route("/api/workflows/issues/by-pr", get(get_issue_workflow_by_pr))
        .route(
            "/api/workflows/projects/by-project",
            get(get_project_workflow_by_project),
        )
        .route(
            "/api/workflows/runtime/tree",
            get(get_workflow_runtime_tree),
        )
        .route(
            "/api/workflows/runtime/merge",
            post(task_mutation_routes::merge_workflow_runtime),
        )
        .route(
            "/api/workflows/runtime/cancel",
            post(task_mutation_routes::cancel_workflow_runtime),
        )
        .route(
            "/api/runtime-hosts",
            get(crate::handlers::runtime_hosts::list_runtime_hosts),
        )
        .route(
            "/api/runtime-hosts/register",
            post(crate::handlers::runtime_hosts::register_runtime_host),
        )
        .route(
            "/api/runtime-hosts/{host_id}/heartbeat",
            post(crate::handlers::runtime_hosts::heartbeat_runtime_host),
        )
        .route(
            "/api/runtime-hosts/{host_id}/deregister",
            post(crate::handlers::runtime_hosts::deregister_runtime_host),
        )
        .route(
            "/api/runtime-hosts/{host_id}/tasks/claim",
            post(crate::handlers::runtime_hosts::claim_task_for_runtime_host),
        )
        .route(
            "/api/runtime-hosts/{host_id}/runtime-jobs/claim",
            post(crate::handlers::runtime_hosts::claim_runtime_job_for_runtime_host),
        )
        .route(
            "/api/runtime-hosts/{host_id}/runtime-jobs/{runtime_job_id}/complete",
            post(crate::handlers::runtime_hosts::complete_runtime_job_for_runtime_host),
        )
        .route(
            "/api/runtime-hosts/{host_id}/projects",
            get(crate::handlers::runtime_project_cache::list_runtime_host_projects),
        )
        .route(
            "/api/runtime-hosts/{host_id}/projects/sync",
            post(crate::handlers::runtime_project_cache::sync_runtime_host_projects),
        )
        .route(
            "/api/token-usage",
            get(crate::handlers::token_usage::token_usage),
        )
        .route(
            "/webhook",
            post(github_webhook).layer(DefaultBodyLimit::max(
                state.core.server.config.server.max_webhook_body_bytes,
            )),
        )
        .route(
            "/webhook/feishu",
            post(crate::intake::feishu::feishu_webhook).layer(DefaultBodyLimit::max(
                state.core.server.config.server.max_webhook_body_bytes,
            )),
        )
        .route(
            "/signals",
            post(ingest_signal).layer(DefaultBodyLimit::max(
                state.core.server.config.server.max_webhook_body_bytes,
            )),
        )
        .route("/auth/reset-password", post(password_reset))
        .route("/reconcile", post(crate::handlers::reconcile::handle))
        // ── Runs API (harness-dag execution model) ──────────────────────────
        // Under /api so the SPA can own `/runs/{id}` as a client route.
        .route(
            "/api/runs",
            post(runs_routes::create_run).get(runs_routes::list_runs),
        )
        .route(
            "/api/runs/{id}",
            get(runs_routes::get_run).delete(runs_routes::delete_run),
        )
        .route("/api/runs/{id}/cancel", post(runs_routes::cancel_run))
        .route("/api/runs/{id}/stream", get(runs_routes::stream_run))
        // ── Step categories (global registry for overview grouping/colour) ──
        .route("/api/categories", get(categories_routes::list_categories))
        .route(
            "/api/categories/{id}",
            axum::routing::put(categories_routes::save_category)
                .delete(categories_routes::delete_category),
        )
        // ── Workflow authoring API (visual editor + MCP) ────────────────────
        .route("/api/authoring/catalog", get(workflows_routes::get_catalog))
        .route(
            "/api/authoring/workflows",
            get(workflows_routes::list_workflows).post(workflows_routes::save_workflow),
        )
        .route(
            "/api/authoring/workflows/{name}",
            get(workflows_routes::get_workflow),
        )
        .route(
            "/api/authoring/validate",
            post(workflows_routes::validate_workflow),
        )
        // ── Project-scoped authoring (per registered project; remote MCP) ───
        .route(
            "/api/projects/{project}/authoring/catalog",
            get(project_authoring_routes::get_catalog),
        )
        .route(
            "/api/projects/{project}/authoring/workflows",
            get(project_authoring_routes::list_workflows)
                .post(project_authoring_routes::save_workflow),
        )
        .route(
            "/api/projects/{project}/authoring/workflows/{name}",
            get(project_authoring_routes::get_workflow),
        )
        .route(
            "/api/projects/{project}/authoring/validate",
            post(project_authoring_routes::validate_workflow),
        )
        .route(
            "/api/projects/{project}/authoring/create",
            post(project_authoring_routes::create_workflow),
        )
        .route(
            "/api/projects/{project}/authoring/set-node",
            post(project_authoring_routes::set_node),
        )
        .route(
            "/api/projects/{project}/authoring/remove-node",
            post(project_authoring_routes::remove_node),
        )
        .route(
            "/api/projects/{project}/authoring/connect",
            post(project_authoring_routes::connect_nodes),
        )
        // ── Provider credentials (UI-managed, encrypted at rest) ────────────
        .route(
            "/api/credentials",
            get(credentials_routes::list_credentials),
        )
        .route(
            "/api/credentials/{provider}",
            axum::routing::put(credentials_routes::set_credential)
                .delete(credentials_routes::delete_credential),
        )
        // ── Connect Kimi (OAuth device flow, server-side) ───────────────────
        .route(
            "/api/credentials/kimi/connect/start",
            post(super::kimi_routes::connect_start),
        )
        .route(
            "/api/credentials/kimi/connect/poll",
            post(super::kimi_routes::connect_poll),
        )
        // ── Connect Codex (ChatGPT OAuth device flow, server-side) ──────────
        .route(
            "/api/credentials/codex/connect/start",
            post(super::codex_routes::connect_start),
        )
        .route(
            "/api/credentials/codex/connect/complete",
            post(super::codex_routes::connect_complete),
        )
        // ── Project registry (scopes runs to a git repo) ────────────────────
        .route(
            "/api/projects",
            get(super::projects_routes::list_projects)
                .post(super::projects_routes::register_project),
        )
        .route(
            "/api/projects/{name}",
            get(super::projects_routes::get_project).delete(super::projects_routes::delete_project),
        )
        // SPA fallback: serve the React shell for client-side routes
        // (`/runs/{id}`, `/editor`, …) so deep links / refreshes work.
        .fallback(crate::dashboard::spa_fallback)
        .layer(Extension(runs_state))
        .layer(middleware::from_fn_with_state(
            state.clone(),
            auth::api_auth_middleware,
        ))
        .with_state(state)
}
