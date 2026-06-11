//! Per-lane **billing profile** API (global). Records how a model lane's usage
//! maps to real cost: usage-based (effective == notional) vs subscription (a
//! flat fee amortized over actual usage). Lanes key on the model bucket the rate
//! table already matches (`claude`, `gpt`, `kimi`, `composer`, …) because one
//! provider can front several subscriptions.
//!
//! - `GET    /api/billing-profiles`        — list profiles
//! - `PUT    /api/billing-profiles/{lane}` — create / update a profile
//! - `DELETE /api/billing-profiles/{lane}` — remove a profile

use std::sync::Arc;

use axum::extract::{Extension, Path as AxumPath};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::Json;
use harness_persist::BillingProfileInput;
use serde::Deserialize;

use super::runs_routes::RunsState;

fn err(status: StatusCode, msg: impl Into<String>) -> Response {
    (status, Json(serde_json::json!({ "error": msg.into() }))).into_response()
}

/// A lane is a short slug (matches a rate-table bucket).
fn valid_lane(lane: &str) -> bool {
    !lane.is_empty()
        && lane.len() <= 48
        && lane
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
}

/// `GET /api/billing-profiles` — list all configured billing profiles.
pub async fn list_billing_profiles(Extension(state): Extension<Arc<RunsState>>) -> Response {
    let store = match state.billing_store().await {
        Ok(s) => s,
        Err(e) => return err(StatusCode::SERVICE_UNAVAILABLE, e),
    };
    match store.list().await {
        Ok(profiles) => Json(profiles).into_response(),
        Err(e) => err(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()),
    }
}

#[derive(Debug, Deserialize)]
pub struct SaveBillingProfileRequest {
    /// `"usage_based"` or `"subscription"`.
    pub billing_mode: String,
    #[serde(default)]
    pub monthly_price_usd: f64,
    #[serde(default)]
    pub est_monthly_value_usd: Option<f64>,
}

/// `PUT /api/billing-profiles/{lane}` — create or update a lane's profile.
pub async fn save_billing_profile(
    Extension(state): Extension<Arc<RunsState>>,
    AxumPath(lane): AxumPath<String>,
    Json(req): Json<SaveBillingProfileRequest>,
) -> Response {
    if !valid_lane(&lane) {
        return err(
            StatusCode::BAD_REQUEST,
            "lane must be 1–48 chars of [A-Za-z0-9_-]",
        );
    }
    if req.billing_mode != "usage_based" && req.billing_mode != "subscription" {
        return err(
            StatusCode::BAD_REQUEST,
            "billing_mode must be 'usage_based' or 'subscription'",
        );
    }
    let store = match state.billing_store().await {
        Ok(s) => s,
        Err(e) => return err(StatusCode::SERVICE_UNAVAILABLE, e),
    };
    let input = BillingProfileInput {
        billing_mode: req.billing_mode,
        monthly_price_usd: req.monthly_price_usd,
        est_monthly_value_usd: req.est_monthly_value_usd,
    };
    match store.upsert(&lane, &input).await {
        Ok(p) => Json(p).into_response(),
        Err(e) => err(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()),
    }
}

/// `DELETE /api/billing-profiles/{lane}` — remove a lane's profile (its usage
/// then falls back to notional cost).
pub async fn delete_billing_profile(
    Extension(state): Extension<Arc<RunsState>>,
    AxumPath(lane): AxumPath<String>,
) -> Response {
    let store = match state.billing_store().await {
        Ok(s) => s,
        Err(e) => return err(StatusCode::SERVICE_UNAVAILABLE, e),
    };
    match store.delete(&lane).await {
        Ok(()) => Json(serde_json::json!({ "deleted": true, "lane": lane })).into_response(),
        Err(e) => err(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()),
    }
}
