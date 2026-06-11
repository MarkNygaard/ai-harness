//! Measured calibration of subscription **billing profiles**.
//!
//! A subscription's effective cost depends on how much usage you actually pull
//! from it per month. Rather than guess, we measure: a subscription exposes a
//! rolling weekly window with a consumed-% gauge (`/api/usage`), and we already
//! record every node's tokens. If the harness spent `$C` of list-priced usage on
//! a lane and that moved the weekly gauge to `P%`, the plan's full weekly value
//! is `C ÷ (P/100)`, and the monthly value scales by `30.44 / 7`. The effective
//! cost multiplier is then `monthly_price ÷ monthly_value` (see
//! [`harness_persist::BillingProfile::effective_multiplier`]).
//!
//! This is only sound when the harness is the lane's **sole consumer** (else the
//! gauge moves more than our tokens explain and capacity is under-estimated). We
//! calibrate only the dedicated, gauge-readable lanes — `kimi` and `gpt`
//! (ChatGPT/Codex). `claude` is excluded (its usage endpoint needs a scope the
//! subscription token lacks) and `composer` is usage-based (no subscription).

use std::sync::Arc;
use std::time::Duration;

use harness_persist::BillingProfileInput;

use crate::handlers::token_usage::{lane_for_model, rates_for_model};
use crate::http::runs_routes::RunsState;
use crate::http::usage_routes::weekly_window_for;

/// How often to recompute calibrations. The usage report is cached for ~3 min
/// upstream, so polling much faster just re-reads the same gauge.
const INTERVAL: Duration = Duration::from_secs(300);

/// Average days per month, for weekly→monthly value scaling.
const DAYS_PER_MONTH: f64 = 30.4375;

/// Below this consumed-% the gauge is too noisy to divide by (a tiny run early
/// in a window would imply an absurd capacity), so we skip until more is spent.
const MIN_UTILIZATION_PCT: f64 = 2.0;

/// Billing lanes we can calibrate, paired with the usage-report `cli` whose
/// weekly window backs them. `claude` and `composer` are intentionally absent.
const CALIBRATED_LANES: &[(&str, &str)] = &[("kimi", "kimi"), ("gpt", "codex")];

/// Estimate a plan's monthly list-dollar value from the consumed list-$ and the
/// fraction of the (weekly) window that represents. `None` when there isn't
/// enough signal yet (too little of the window consumed, or nothing spent).
pub(crate) fn estimate_monthly_value(
    consumed_usd: f64,
    used_pct: f64,
    window_days: f64,
) -> Option<f64> {
    if used_pct < MIN_UTILIZATION_PCT || consumed_usd <= 0.0 || window_days <= 0.0 {
        return None;
    }
    let per_window = consumed_usd / (used_pct / 100.0);
    Some(per_window * (DAYS_PER_MONTH / window_days))
}

/// Spawn the periodic calibrator. No-ops gracefully when there's no DB, no
/// configured subscription profile for a lane, or no readable gauge.
pub(crate) fn spawn_billing_calibrator(state: Arc<RunsState>) {
    tokio::spawn(async move {
        let mut tick = tokio::time::interval(INTERVAL);
        loop {
            tick.tick().await;
            if let Err(e) = calibrate_once(&state).await {
                tracing::debug!("billing calibration skipped: {e}");
            }
        }
    });
}

/// One calibration pass over the calibratable lanes.
async fn calibrate_once(state: &RunsState) -> Result<(), String> {
    let billing = state.billing_store().await?;
    let run_store = state.store().await?;
    let profiles = billing.list().await.map_err(|e| e.to_string())?;

    for (lane, cli) in CALIBRATED_LANES {
        let Some(profile) = profiles.iter().find(|p| p.lane == *lane) else {
            continue; // not configured — nothing to calibrate
        };
        if profile.billing_mode != "subscription" {
            continue;
        }
        let Some((used_pct, resets_at)) = weekly_window_for(state, cli).await else {
            continue; // gauge unavailable (e.g. CLI not connected)
        };

        // Window start = reset − 7d; fall back to a trailing 7-day window when
        // the reset time is unknown.
        let now = chrono::Utc::now();
        let window_start = resets_at
            .map(|r| r - chrono::Duration::days(7))
            .unwrap_or_else(|| now - chrono::Duration::days(7));

        let sums = run_store
            .token_sums_by_model_since(window_start)
            .await
            .map_err(|e| e.to_string())?;
        let consumed_usd: f64 = sums
            .iter()
            .filter(|s| lane_for_model(&s.model) == *lane)
            .map(|s| {
                rates_for_model(&s.model).cost_usd(
                    s.input_tokens.max(0) as u64,
                    s.output_tokens.max(0) as u64,
                    s.cache_read.max(0) as u64,
                    s.cache_write.max(0) as u64,
                )
            })
            .sum();

        let Some(monthly_value) = estimate_monthly_value(consumed_usd, used_pct, 7.0) else {
            continue;
        };
        billing
            .upsert(
                lane,
                &BillingProfileInput {
                    billing_mode: "subscription".to_string(),
                    monthly_price_usd: profile.monthly_price_usd,
                    est_monthly_value_usd: Some(monthly_value),
                },
            )
            .await
            .map_err(|e| e.to_string())?;
        tracing::info!(
            lane,
            used_pct,
            consumed_usd,
            monthly_value,
            "billing calibration updated"
        );
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn estimate_scales_window_to_month() {
        // $3 of usage moved the weekly gauge 3% → $100/week → ~$434/month.
        let v = estimate_monthly_value(3.0, 3.0, 7.0).unwrap();
        assert!((v - 100.0 * (DAYS_PER_MONTH / 7.0)).abs() < 1e-6);
    }

    #[test]
    fn estimate_needs_enough_signal() {
        assert!(estimate_monthly_value(0.5, 1.0, 7.0).is_none()); // below MIN_UTILIZATION
        assert!(estimate_monthly_value(0.0, 50.0, 7.0).is_none()); // nothing spent
    }

    #[test]
    fn estimate_is_proportional_to_consumption() {
        let a = estimate_monthly_value(10.0, 5.0, 7.0).unwrap();
        let b = estimate_monthly_value(20.0, 5.0, 7.0).unwrap();
        assert!((b - 2.0 * a).abs() < 1e-6);
    }
}
