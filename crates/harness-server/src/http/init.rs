use crate::server::HarnessServer;
use std::path::PathBuf;
use std::sync::Arc;
use tokio::sync::broadcast;

use super::{
    rate_limit,
    state::{AppState, CoreServices, NotificationServices, ObservabilityServices},
};

fn resolve_project_root(configured_root: &std::path::Path) -> anyhow::Result<std::path::PathBuf> {
    let project_root = configured_root.canonicalize().map_err(|e| {
        anyhow::anyhow!(
            "invalid server.project_root '{}': {e}",
            configured_root.display()
        )
    })?;
    if !project_root.is_dir() {
        anyhow::bail!(
            "server.project_root is not a directory: {}",
            project_root.display()
        );
    }
    Ok(project_root)
}

/// Expand a leading `~/` or standalone `~` to the value of `$HOME`.
/// Returns the path unchanged when `~` is not present or `HOME` is unset.
pub(super) fn expand_tilde(path: &std::path::Path) -> std::path::PathBuf {
    if let Some(s) = path.to_str() {
        if let Some(rest) = s.strip_prefix("~/") {
            if let Ok(home) = std::env::var("HOME") {
                return std::path::PathBuf::from(home).join(rest);
            }
        } else if s == "~" {
            if let Ok(home) = std::env::var("HOME") {
                return std::path::PathBuf::from(home);
            }
        }
    }
    path.to_path_buf()
}

/// Build the minimal [`AppState`] the HTTP server still needs after the legacy
/// task/RPC/runtime subsystems were removed. The DAG run path runs off a
/// self-contained `RunsState` constructed in `build_router`; this holds only the
/// config/paths, the event store (shutdown flush), the password-reset rate
/// limiter, and the graceful-shutdown channel.
pub async fn build_app_state(server: Arc<HarnessServer>) -> anyhow::Result<AppState> {
    let dir = expand_tilde(&server.config.server.data_dir);
    let project_root = resolve_project_root(&server.config.server.project_root)?;
    let home_dir = std::env::var("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|_| project_root.clone());

    let database_url =
        harness_core::db::resolve_database_url(server.config.server.database_url.as_deref())?;
    let events = Arc::new(
        harness_observe::event_store::EventStore::new_with_database_url(&dir, Some(&database_url))
            .await?,
    );

    let password_reset_rate_limit = server.config.server.password_reset_rate_limit_per_hour;
    let (ws_shutdown_tx, _) = broadcast::channel(1);

    Ok(AppState {
        core: CoreServices {
            server,
            project_root,
            home_dir,
        },
        observability: ObservabilityServices {
            events,
            password_reset_rate_limiter: Arc::new(rate_limit::PasswordResetRateLimiter::new(
                password_reset_rate_limit,
            )),
        },
        notifications: NotificationServices { ws_shutdown_tx },
        #[cfg(test)]
        _db_state_guard: None,
    })
}
