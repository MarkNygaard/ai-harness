use axum::{
    extract::{Extension, State},
    http::StatusCode,
    Json,
};
use serde_json::json;
use std::sync::Arc;

use super::state::AppState;

/// GET /health — liveness + runtime-log state.
pub(crate) async fn health_check(State(state): State<Arc<AppState>>) -> Json<serde_json::Value> {
    let runtime_logs = &state.core.server.runtime_logs;
    Json(json!({
        "status": "ok",
        "runtime_logs": {
            "state": runtime_logs.state.as_str(),
            "path_hint": runtime_logs.path_hint.clone(),
            "retention_days": runtime_logs.retention_days,
        }
    }))
}

#[derive(serde::Deserialize)]
pub(crate) struct PasswordResetRequest {
    pub(crate) email: String,
}

pub(crate) fn prepare_password_reset_request(
    rate_limiter: &crate::http::rate_limit::PasswordResetRateLimiter,
    limit: u32,
    email: &str,
) -> Result<String, (StatusCode, serde_json::Value)> {
    let email = email.trim().to_lowercase();
    if email.is_empty() {
        return Err((
            StatusCode::BAD_REQUEST,
            json!({"error": "email is required"}),
        ));
    }

    if !rate_limiter.check_and_increment(&email) {
        return Err((
            StatusCode::TOO_MANY_REQUESTS,
            json!({
                "error": format!(
                    "rate limit exceeded: max {} password reset requests per hour",
                    limit
                )
            }),
        ));
    }

    Ok(email)
}

/// POST /auth/reset-password — ask for a link to set a new password.
///
/// **The answer never says whether the address exists.** An unauthenticated
/// endpoint that distinguishes a known address from an unknown one is a way to
/// enumerate who has an account here, so both take the same path and get the
/// same words.
///
/// Rate limiting stays where it was: this route sends mail on an unauthenticated
/// request, which is a spam vector as much as a guessing one.
pub(crate) async fn password_reset(
    State(state): State<Arc<AppState>>,
    Extension(runs): Extension<Arc<crate::http::runs_routes::RunsState>>,
    Json(req): Json<PasswordResetRequest>,
) -> (StatusCode, Json<serde_json::Value>) {
    let limit = state
        .core
        .server
        .config
        .server
        .password_reset_rate_limit_per_hour;
    let email = match prepare_password_reset_request(
        &state.observability.password_reset_rate_limiter,
        limit,
        &req.email,
    ) {
        Ok(email) => email,
        Err((status, body)) => return (status, Json(body)),
    };

    // Best-effort and silent: whether an account exists, whether mail is
    // configured, and whether the send succeeded are all invisible from here.
    crate::http::invites_routes::send_reset_link(&runs, &email).await;

    (
        StatusCode::OK,
        Json(json!({
            "ok": true,
            "message": "If that address has an account, a reset link is on its way.",
        })),
    )
}
