use crate::server::HarnessServer;
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

pub(crate) mod auth;
pub(crate) mod billing_routes;
pub(crate) mod categories_routes;
pub(crate) mod codex_routes;
pub(crate) mod credentials_routes;
pub(crate) mod finding_routes;
pub(crate) mod http_router;
pub(crate) mod init;
pub(crate) mod kimi_routes;
pub(crate) mod linear_poller;
pub(crate) mod linear_routes;
pub(crate) mod linear_source_routes;
pub(crate) mod mcp_routes;
pub(crate) mod misc_routes;
pub(crate) mod projects_routes;
pub(crate) mod rate_limit;
pub(crate) mod runs_routes;
pub(crate) mod state;
pub(crate) mod usage_routes;
pub(crate) mod workflows_routes;

#[cfg(test)]
mod shutdown_test;
#[cfg(test)]
mod tests_password_reset;

// Re-export all public symbols so callers using `crate::http::*` paths continue to work.
pub use init::build_app_state;
pub use state::{AppState, CoreServices, NotificationServices, ObservabilityServices};

// Handler re-exports kept accessible via `crate::http::`.
pub(crate) use misc_routes::{health_check, password_reset};

pub async fn serve(server: Arc<HarnessServer>, addr: SocketAddr) -> anyhow::Result<()> {
    tracing::info!("harness: HTTP server listening on {addr}");
    // Record true server start time before accepting any connections.
    crate::handlers::dashboard::SERVER_START.get_or_init(std::time::Instant::now);

    let state = Arc::new(build_app_state(server.clone()).await?);

    // Startup summary — one clean line instead of scattered logs.
    tracing::info!(
        project = %state.core.project_root.display(),
        "harness: ready"
    );

    let app = http_router::build_router(state.clone());

    let listener = tokio::net::TcpListener::bind(addr).await?;
    let ws_shutdown_tx = state.notifications.ws_shutdown_tx.clone();
    let shutdown_cfg = state.core.server.config.server.shutdown.clone();

    // Fan the first SIGINT/SIGTERM out to two consumers:
    //   (a) the with_graceful_shutdown future, so Axum stops accepting new
    //       connections the instant the signal lands (P1 fix);
    //   (b) the force-watcher task, so the hard-deadline timer starts at
    //       signal time, not at server startup (P0 fix).
    // A watch channel lets a single signal handler notify both consumers
    // without racing on which side runs first.
    let (first_signal_tx, first_signal_rx_serve) = tokio::sync::watch::channel(false);
    let first_signal_rx_force = first_signal_tx.subscribe();
    tokio::spawn(async move {
        wait_first_termination_signal().await;
        let _ = first_signal_tx.send(true);
    });

    let serve_future = axum::serve(listener, app).with_graceful_shutdown(
        axum_graceful_shutdown_signal(first_signal_rx_serve, shutdown_cfg.clone()),
    );

    let force_watcher_cfg = shutdown_cfg.clone();
    let force_watcher_events = state.observability.events.clone();
    let force_watcher_ws_tx = ws_shutdown_tx.clone();
    let force_watcher = tokio::spawn(async move {
        let Some(reason) = wait_for_first_signal_then_drain_or_force(
            first_signal_rx_force,
            force_watcher_cfg.clone(),
            wait_second_termination_signal(),
        )
        .await
        else {
            return;
        };
        tracing::info!(
            ?reason,
            "shutdown: drain phase ended, force-closing long-lived connections"
        );
        force_watcher_ws_tx.send(()).ok();

        // Phase 3: hard deadline. If the serve future has not resolved within
        // the force-grace window, exit forcefully.
        tokio::time::sleep(Duration::from_secs(force_watcher_cfg.force_grace_secs)).await;
        tracing::error!(
            force_grace_secs = force_watcher_cfg.force_grace_secs,
            "shutdown: force grace exceeded — process::exit(1)"
        );
        force_watcher_events.shutdown().await;
        std::process::exit(1);
    });

    let serve_result = serve_future.await;
    tracing::info!("server shutting down");
    ws_shutdown_tx.send(()).ok();
    state.observability.events.shutdown().await;
    force_watcher.abort();
    serve_result?;
    Ok(())
}

async fn await_first_shutdown_signal(mut signal_rx: tokio::sync::watch::Receiver<bool>) -> bool {
    loop {
        if *signal_rx.borrow_and_update() {
            return true;
        }
        if signal_rx.changed().await.is_err() {
            return false;
        }
    }
}

async fn axum_graceful_shutdown_signal(
    signal_rx: tokio::sync::watch::Receiver<bool>,
    shutdown_cfg: harness_core::config::shutdown::ShutdownConfig,
) {
    if !await_first_shutdown_signal(signal_rx).await {
        return;
    }
    tracing::info!(
        drain_deadline_secs = shutdown_cfg.drain_timeout_secs,
        progress_log_secs = shutdown_cfg.progress_log_secs,
        "shutdown: draining; press Ctrl+C again to force"
    );
}

async fn wait_for_first_signal_then_drain_or_force<F>(
    signal_rx: tokio::sync::watch::Receiver<bool>,
    shutdown_cfg: harness_core::config::shutdown::ShutdownConfig,
    user_force: F,
) -> Option<ShutdownReason>
where
    F: std::future::Future<Output = ()>,
{
    if !await_first_shutdown_signal(signal_rx).await {
        return None;
    }
    Some(
        drain_or_force(
            shutdown_cfg.drain_timeout_secs,
            shutdown_cfg.progress_log_secs,
            user_force,
        )
        .await,
    )
}

/// Reason a graceful shutdown ended. Surfaced for observability and tests.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ShutdownReason {
    /// Drain deadline expired before all in-flight work finished.
    DrainTimeout,
    /// User pressed Ctrl+C (or sent SIGTERM) a second time during drain.
    UserForced,
}

/// Wait for the first SIGINT/SIGTERM. Errors installing the handlers are
/// logged but do not abort the drain — we still want orderly shutdown when
/// only one signal source is available.
async fn wait_first_termination_signal() {
    let ctrl_c = async {
        if let Err(e) = tokio::signal::ctrl_c().await {
            tracing::error!("failed to install Ctrl+C handler: {e}");
        }
    };

    #[cfg(unix)]
    let terminate = async {
        match tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate()) {
            Ok(mut s) => {
                s.recv().await;
            }
            Err(e) => tracing::error!("failed to install SIGTERM handler: {e}"),
        }
    };

    #[cfg(not(unix))]
    let terminate = std::future::pending::<()>();

    tokio::select! {
        _ = ctrl_c => tracing::info!("received Ctrl+C"),
        _ = terminate => tracing::info!("received SIGTERM"),
    }
}

/// Wait for a second SIGINT/SIGTERM after the first one has already been
/// consumed. Re-installs fresh handlers so a follow-up Ctrl+C is observed.
async fn wait_second_termination_signal() {
    let ctrl_c = async {
        if let Err(e) = tokio::signal::ctrl_c().await {
            tracing::error!("failed to re-install Ctrl+C handler: {e}");
            std::future::pending::<()>().await;
        }
    };

    #[cfg(unix)]
    let terminate = async {
        match tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate()) {
            Ok(mut s) => {
                s.recv().await;
            }
            Err(e) => {
                tracing::error!("failed to re-install SIGTERM handler: {e}");
                std::future::pending::<()>().await;
            }
        }
    };

    #[cfg(not(unix))]
    let terminate = std::future::pending::<()>();

    tokio::select! {
        _ = ctrl_c => tracing::warn!("shutdown: second Ctrl+C — forcing"),
        _ = terminate => tracing::warn!("shutdown: second SIGTERM — forcing"),
    }
}

/// Run the drain phase: emit progress logs every `progress_log_secs`,
/// stop when either the drain deadline expires or the user-force future
/// resolves.
async fn drain_or_force<F>(
    drain_timeout_secs: u64,
    progress_log_secs: u64,
    user_force: F,
) -> ShutdownReason
where
    F: std::future::Future<Output = ()>,
{
    let drain_deadline = tokio::time::sleep(Duration::from_secs(drain_timeout_secs));
    let progress_interval = Duration::from_secs(progress_log_secs.max(1)); // never spin at 0s
    let mut progress = tokio::time::interval(progress_interval);
    // Skip the immediate first tick so the user does not see a duplicate
    // log right after the "draining" line above.
    progress.tick().await;

    tokio::pin!(drain_deadline);
    tokio::pin!(user_force);

    loop {
        tokio::select! {
            biased;
            _ = &mut user_force => return ShutdownReason::UserForced,
            _ = &mut drain_deadline => return ShutdownReason::DrainTimeout,
            _ = progress.tick() => {
                tracing::info!(
                    drain_timeout_secs,
                    "shutdown: still draining"
                );
            }
        }
    }
}
