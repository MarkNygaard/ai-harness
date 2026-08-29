use axum::{
    middleware,
    routing::{delete, get, post, put},
    Extension, Router,
};
use std::sync::Arc;

use super::{
    auth, auth_routes, billing_routes, categories_routes, credentials_routes, finding_routes,
    health_check, invites_routes, linear_agent, linear_connections, linear_oauth, linear_routes,
    linear_source_routes, mcp_routes, password_reset, runs_routes, settings_routes,
    state::AppState, system_routes, tokens_routes, users_routes, workflows_routes,
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
        Arc::new(
            runs_routes::RunsState::new(
                db_url,
                agent_registry,
                state.core.project_root.clone(),
                secret_key,
                config.server.public_url.clone(),
            )
            .with_api_token(auth::resolve_api_token(&config.server)),
        )
    };
    // Periodically reap runs whose lease has gone stale (crashed/orphaned), so a
    // lost run doesn't linger as `running`. Live runs heartbeat and are skipped.
    runs_routes::spawn_reaper(runs_state.clone());
    // Reclaim worktrees left behind by hard kills (normal completion cleans up via
    // RAII); keeps `.worktrees/` from accumulating junk on the server.
    runs_routes::spawn_worktree_sweeper(runs_state.clone());
    // Bound the shared cargo build cache (size-gated) so it can't fill the disk.
    runs_routes::spawn_cache_sweeper(runs_state.clone());
    // Self-host a loopback omp auth-broker so the dashboard's subscription-usage
    // cards work off the local omp creds (skipped if OMP_AUTH_BROKER_URL is set).
    super::usage_routes::spawn_local_broker();
    // Measure subscription effective cost: pair the weekly usage gauge with the
    // tokens spent on each dedicated lane (kimi, codex) to keep their billing
    // profiles' estimated monthly value calibrated.
    crate::billing_calibration::spawn_billing_calibrator(runs_state.clone());
    // Linear poller (Slice 3a, dry-run): logs which eligible issues each enabled
    // binding WOULD fire — no claim, no transition, no run triggered.
    super::linear_poller::spawn_poller(runs_state.clone());
    // Generate the MCP key now, so `/mcp` is guarded from the first request
    // rather than from whenever someone first opens its settings page.
    super::mcp_key::spawn_ensure(runs_state.clone());
    // A stored public URL overrides the environment, so a typo is a form field
    // rather than a redeploy.
    super::settings_routes::spawn_load_public_url(runs_state.clone());
    // Announce a one-time setup token while nobody has claimed this install.
    super::accounts::spawn_setup_token(runs_state.clone());
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
        .route("/auth/reset-password", post(password_reset))
        // ── Accounts: claiming the install, and signing in ──────────────────
        // Auth-exempt because they are how you get past the middleware; each
        // authenticates itself (setup token, password, or nothing to protect).
        .route("/api/auth/status", get(auth_routes::status))
        .route("/api/auth/setup", post(auth_routes::setup))
        .route("/api/auth/login", post(auth_routes::login))
        .route("/api/auth/logout", post(auth_routes::logout))
        // ── Who has an account here. Administrator-only, at the route. ──────
        // Your own tokens: what a program authenticates with once there is a
        // login in front of the UI. Scoped to whoever is signed in.
        .route(
            "/api/tokens",
            get(tokens_routes::list_tokens).post(tokens_routes::create_token),
        )
        .route("/api/tokens/{id}", delete(tokens_routes::revoke_token))
        // ── How the harness itself is configured. Administrator-only. ──────
        .route(
            "/api/settings/general",
            get(settings_routes::general).put(settings_routes::set_general),
        )
        .route(
            "/api/settings/mail",
            get(settings_routes::mail_settings).put(settings_routes::set_mail),
        )
        .route("/api/settings/mail/test", post(settings_routes::test_mail))
        // ── Invitations, and the links that redeem them ────────────────────
        .route(
            "/api/invites",
            get(invites_routes::list_invites).post(invites_routes::create_invite),
        )
        .route("/api/invites/{id}", delete(invites_routes::revoke_invite))
        // Redeeming is public: whoever holds the link has not signed in yet.
        .route(
            "/api/invites/token/{token}",
            get(invites_routes::describe_invite).post(invites_routes::accept_invite),
        )
        .route("/api/users", get(users_routes::list_users))
        .route("/api/users/{id}/role", put(users_routes::set_role))
        .route("/api/users/{id}/disabled", put(users_routes::set_disabled))
        .route("/api/users/{id}", delete(users_routes::delete_user))
        // ── System: agent-CLI version + in-app update ───────────────────────
        .route(
            "/api/system/claude-version",
            get(system_routes::claude_version),
        )
        .route(
            "/api/system/claude-update",
            post(system_routes::claude_update),
        )
        // ── Runs API (harness-dag execution model) ──────────────────────────
        // Under /api so the SPA can own `/runs/{id}` as a client route.
        .route(
            "/api/runs",
            post(runs_routes::create_run).get(runs_routes::list_runs),
        )
        .route("/api/runs/summary", get(runs_routes::runs_daily_summary))
        // A/B pairing: start both arms, and list a workflow's model pairs for the
        // swap picker. Static segments — matchit prefers these over `{id}` below.
        .route("/api/runs/pair", post(runs_routes::create_run_pair))
        .route("/api/runs/pair/{pair_id}", get(runs_routes::get_run_pair))
        .route(
            "/api/runs/pair/{pair_id}/judge",
            post(runs_routes::judge_run_pair),
        )
        .route(
            "/api/runs/workflow-models",
            get(runs_routes::list_workflow_models),
        )
        .route(
            "/api/runs/{id}",
            get(runs_routes::get_run).delete(runs_routes::delete_run),
        )
        .route("/api/runs/{id}/cancel", post(runs_routes::cancel_run))
        .route("/api/runs/{id}/rerun", post(runs_routes::rerun_run))
        .route("/api/runs/{id}/stream", get(runs_routes::stream_run))
        .route(
            "/api/runs/{id}/activity",
            get(runs_routes::get_run_activity),
        )
        // ── Report per-finding triage state (built / issued / ignored) ───────
        // One unified store, served at the generic `/findings` for every report
        // (GEO, review, and any `ui.report` workflow).
        .route(
            "/api/runs/{id}/findings",
            get(finding_routes::list_findings)
                .put(finding_routes::set_finding)
                .delete(finding_routes::clear_finding),
        )
        // ── Cluster-hosted MCP endpoint (JSON-RPC over HTTP; no local binary) ─
        // Authoring + run control for editors via `{ "type": "http", "url":
        // ".../mcp" }`. Behind the global bearer-token middleware.
        .route("/mcp", post(mcp_routes::handle_mcp))
        // How an editor is told to connect: the endpoint, the key, and the
        // snippet to paste. Behind the normal middleware, unlike `/mcp` itself.
        .route(
            "/api/mcp/connection",
            get(mcp_routes::connection).post(mcp_routes::regenerate_connection),
        )
        // ── Linear read-only discovery (Phase 8, Slice 1) ───────────────────
        .route(
            "/api/projects/{project}/linear/discovery",
            get(linear_routes::discovery),
        )
        .route(
            "/api/projects/{project}/linear/preview",
            get(linear_routes::preview),
        )
        // ── Linear connections — one per connected workspace. A project
        // resolves to one; with a single connection nothing needs configuring.
        .route(
            "/api/linear/connections",
            get(linear_connections::list_connections).post(linear_connections::create_connection),
        )
        .route(
            "/api/linear/connections/{id}",
            delete(linear_connections::delete_connection),
        )
        .route(
            "/api/projects/{project}/linear-connection",
            put(linear_connections::set_project_connection),
        )
        // ── Linear OAuth (`actor=app`) — writes attributed to the app, not to
        // the person whose personal API key was pasted. One install per
        // connected workspace, managed on the Credentials page; the routes take
        // `?connection=<id>`. `start` returns the authorization URL as JSON (a
        // bearer-authenticated fetch); the callback is a browser redirect and is
        // auth-exempt, guarded by its `state` nonce.
        .route("/api/linear/oauth/start", get(linear_oauth::start))
        .route("/api/linear/oauth/status", get(linear_oauth::status))
        .route(
            "/api/linear/oauth/disconnect",
            post(linear_oauth::disconnect),
        )
        .route(linear_oauth::CALLBACK_PATH, get(linear_oauth::callback))
        // Agent sessions: delegating an issue to the app (or @-mentioning it)
        // posts here. Auth-exempt, verified by its `Linear-Signature` HMAC.
        .route(linear_oauth::WEBHOOK_PATH, post(linear_agent::webhook))
        // ── Step categories (global registry for overview grouping/colour) ──
        .route("/api/categories", get(categories_routes::list_categories))
        .route(
            "/api/categories/{id}",
            axum::routing::put(categories_routes::save_category)
                .delete(categories_routes::delete_category),
        )
        // ── Billing profiles (effective vs notional cost per model lane) ────
        .route(
            "/api/billing-profiles",
            get(billing_routes::list_billing_profiles),
        )
        .route(
            "/api/billing-profiles/{lane}",
            axum::routing::put(billing_routes::save_billing_profile)
                .delete(billing_routes::delete_billing_profile),
        )
        // ── Workflow authoring API (visual editor + MCP) ────────────────────
        .route("/api/authoring/catalog", get(workflows_routes::get_catalog))
        .route(
            "/api/authoring/workflows",
            get(workflows_routes::list_workflows).post(workflows_routes::save_workflow),
        )
        .route(
            "/api/authoring/workflows/{name}",
            get(workflows_routes::get_workflow).delete(workflows_routes::delete_workflow),
        )
        .route(
            "/api/authoring/validate",
            post(workflows_routes::validate_workflow),
        )
        .route(
            "/api/authoring/create",
            post(workflows_routes::create_workflow),
        )
        .route("/api/authoring/set-node", post(workflows_routes::set_node))
        .route("/api/authoring/set-ui", post(workflows_routes::set_ui))
        .route(
            "/api/authoring/remove-node",
            post(workflows_routes::remove_node),
        )
        .route(
            "/api/authoring/connect",
            post(workflows_routes::connect_nodes),
        )
        // ── Linear trigger binding (per project+workflow; persist only) ─────
        .route(
            "/api/projects/{project}/linear-source",
            get(linear_source_routes::get_source)
                .put(linear_source_routes::put_source)
                .delete(linear_source_routes::delete_source),
        )
        .route(
            "/api/projects/{project}/linear-sources",
            get(linear_source_routes::list_sources),
        )
        .route(
            "/api/projects/{project}/linear-issues",
            post(linear_source_routes::create_issue),
        )
        // ── Provider credentials (UI-managed, encrypted at rest) ────────────
        .route(
            "/api/credentials",
            get(credentials_routes::list_credentials),
        )
        // Subscription usage (weekly + rolling windows) for the dashboard.
        .route("/api/usage", get(super::usage_routes::get_usage))
        .route(
            "/api/credentials/{provider}",
            axum::routing::put(credentials_routes::set_credential)
                .delete(credentials_routes::delete_credential),
        )
        // Per-project credential overrides (linear / github only).
        .route(
            "/api/projects/{project}/credentials",
            get(credentials_routes::list_project_credentials),
        )
        .route(
            "/api/projects/{project}/credentials/{provider}",
            axum::routing::put(credentials_routes::set_project_credential)
                .delete(credentials_routes::delete_project_credential),
        )
        // Per-project build environment variables (injected into runs + .env.local).
        .route(
            "/api/projects/{project}/env",
            get(credentials_routes::list_project_env)
                .put(credentials_routes::set_project_env)
                .delete(credentials_routes::delete_project_env),
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
        .route(
            "/api/projects/{name}/cache-size",
            get(super::projects_routes::get_cache_size),
        )
        .route(
            "/api/projects/{name}/cache-cap",
            post(super::projects_routes::set_cache_cap).put(super::projects_routes::set_cache_cap),
        )
        .route(
            "/api/projects/{name}/cache/clear",
            post(super::projects_routes::clear_cache),
        )
        .route(
            "/api/projects/{name}/cache/sweep",
            post(super::projects_routes::sweep_cache),
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
