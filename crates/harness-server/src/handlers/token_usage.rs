//! Model → billing-lane and per-MTok rate helpers, shared by `usage_routes` and
//! `billing_calibration`.
//!
//! Both are now one lookup into [`harness_runner::models`], which is the table
//! the editor's model list also reads. It used to be a substring matcher here
//! and the same matcher again in TypeScript, with three comments saying
//! "mirrors token_usage.rs" — a comment doing a compiler's job, and nothing
//! that failed when the two drifted.

pub(crate) use harness_runner::models::Rates as ModelRates;

/// The billing **lane** a model belongs to — the coarse family sharing one
/// subscription bucket (`claude`, `gpt`, `kimi`, `composer`), `other` for
/// anything unrecognised.
pub(crate) fn lane_for_model(model: &str) -> &'static str {
    harness_runner::models::family_for(model).lane
}

/// Notional per-MTok rates for a model.
///
/// Exact for a listed model, and by the id's shape for one nobody listed —
/// a workflow can pin any string, so this always answers with something
/// defensible rather than zero.
pub(crate) fn rates_for_model(model: &str) -> ModelRates {
    harness_runner::models::family_for(model).rates
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The ids that actually appear in bundled workflows and run rows, pinned
    /// here because this is what the dashboard's cost column is computed from.
    #[test]
    fn the_ids_runs_actually_carry_land_in_the_right_lane() {
        assert_eq!(lane_for_model("opus"), "claude");
        assert_eq!(lane_for_model("claude-sonnet-5"), "claude");
        assert_eq!(lane_for_model("openai-codex/gpt-6-astra"), "gpt");
        assert_eq!(lane_for_model("gpt-5.6-sol"), "gpt");
        assert_eq!(lane_for_model("kimi-code/kimi-for-coding"), "kimi");
        assert_eq!(lane_for_model("composer-2.5"), "composer");
    }

    /// A cost is a number somebody reads as money, so the arithmetic is worth
    /// pinning rather than trusting to the rate table alone.
    #[test]
    fn a_cost_is_the_sum_of_its_four_rates_per_million() {
        let rates = rates_for_model("opus");
        // 1M input at $5, 1M output at $25 — the two that dominate.
        assert!((rates.cost_usd(1_000_000, 1_000_000, 0, 0) - 30.0).abs() < 1e-9);
        // Nothing spent is nothing owed, rather than a minimum.
        assert_eq!(rates.cost_usd(0, 0, 0, 0), 0.0);
    }
}
