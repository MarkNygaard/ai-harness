use std::sync::Arc;
use tokio::sync::broadcast;

use super::rate_limit;

/// Core services: server config + resolved paths.
pub struct CoreServices {
    pub server: Arc<crate::server::HarnessServer>,
    pub project_root: std::path::PathBuf,
    /// Home directory captured at startup to avoid TOCTOU when validating
    /// project roots against `$HOME` in concurrent requests.
    pub home_dir: std::path::PathBuf,
}

/// Observability services: event store (shutdown flush) + auth rate limiting.
pub struct ObservabilityServices {
    pub events: Arc<harness_observe::event_store::EventStore>,
    pub password_reset_rate_limiter: Arc<rate_limit::PasswordResetRateLimiter>,
}

/// Notification services: graceful-shutdown signalling.
pub struct NotificationServices {
    /// Broadcast channel used to signal active connections to close gracefully.
    pub ws_shutdown_tx: broadcast::Sender<()>,
}

/// Minimal shared state for the HTTP server. The DAG run path runs off a
/// self-contained `RunsState` (built from config in `build_router`); this holds
/// only what the router construction, auth middleware, password-reset route, and
/// graceful shutdown still need.
pub struct AppState {
    pub core: CoreServices,
    pub observability: ObservabilityServices,
    pub notifications: NotificationServices,
    #[cfg(test)]
    pub _db_state_guard: Option<tokio::sync::OwnedMutexGuard<()>>,
}
