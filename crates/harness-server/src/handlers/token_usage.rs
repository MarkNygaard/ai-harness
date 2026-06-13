//! Model → billing-lane and per-MTok rate helpers, shared by `usage_routes` and
//! `billing_calibration`. (The legacy `/api/token-usage` handler was removed with
//! the task subsystem; only these pure pricing helpers remain.)

/// Per-MTok USD rates for a model family.
pub(crate) struct ModelRates {
    input: f64,
    output: f64,
    cache_read: f64,
    cache_write: f64,
}

impl ModelRates {
    /// Notional USD cost of a token breakdown at these per-MTok rates.
    pub(crate) fn cost_usd(
        &self,
        input: u64,
        output: u64,
        cache_read: u64,
        cache_write: u64,
    ) -> f64 {
        (input as f64 * self.input
            + output as f64 * self.output
            + cache_read as f64 * self.cache_read
            + cache_write as f64 * self.cache_write)
            / 1_000_000.0
    }
}

/// The billing **lane** a model belongs to — the coarse family that shares one
/// subscription / rate bucket (`claude`, `gpt`, `kimi`, `composer`), matched the
/// same way as [`rates_for_model`]. `other` for anything unrecognized.
pub(crate) fn lane_for_model(model: &str) -> &'static str {
    let m = model.to_ascii_lowercase();
    if m.contains("opus") || m.contains("haiku") || m.contains("fable") || m.contains("sonnet") {
        "claude"
    } else if m.contains("gpt-5") || m.contains("codex") || m.contains("openai") {
        "gpt"
    } else if m.contains("kimi") || m.contains("moonshot") {
        "kimi"
    } else if m.contains("composer") {
        "composer"
    } else {
        "other"
    }
}

/// Notional per-MTok price table, matched by substring on the (lowercased) model
/// id so id variants resolve to a family (`claude-opus-4-8`, `openai-codex/gpt-5.5`,
/// `kimi-for-coding`, …). Notional cost basis — comparable across subscription and
/// API-billed runs, NOT an invoice. Unknown models fall back to Sonnet-tier.
pub(crate) fn rates_for_model(model: &str) -> ModelRates {
    let m = model.to_ascii_lowercase();
    if m.contains("opus") {
        ModelRates {
            input: 5.0,
            output: 25.0,
            cache_read: 0.5,
            cache_write: 6.25,
        }
    } else if m.contains("haiku") {
        ModelRates {
            input: 1.0,
            output: 5.0,
            cache_read: 0.1,
            cache_write: 1.25,
        }
    } else if m.contains("fable") {
        ModelRates {
            input: 10.0,
            output: 50.0,
            cache_read: 1.0,
            cache_write: 12.5,
        }
    } else if m.contains("sonnet") {
        ModelRates {
            input: 3.0,
            output: 15.0,
            cache_read: 0.3,
            cache_write: 3.75,
        }
    } else if m.contains("gpt-5") || m.contains("codex") || m.contains("openai") {
        ModelRates {
            input: 5.0,
            output: 30.0,
            cache_read: 0.50,
            cache_write: 5.0,
        }
    } else if m.contains("kimi") || m.contains("moonshot") {
        ModelRates {
            input: 0.95,
            output: 4.0,
            cache_read: 0.16,
            cache_write: 0.95,
        }
    } else if m.contains("composer") {
        ModelRates {
            input: 0.50,
            output: 2.50,
            cache_read: 0.20,
            cache_write: 0.50,
        }
    } else {
        ModelRates {
            input: 3.0,
            output: 15.0,
            cache_read: 0.3,
            cache_write: 3.75,
        }
    }
}
