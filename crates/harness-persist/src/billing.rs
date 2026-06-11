//! Per-lane **billing profiles** — how a model's usage maps to real cost.
//!
//! The notional cost we compute (`tokens × API list rate`) equals real marginal
//! cost only for usage-based providers (e.g. Cursor's dollar pool). For a flat
//! **subscription** (Claude Max, a Kimi plan, a ChatGPT/Codex plan) the true
//! cost is the monthly fee amortized over actual usage — far below list price
//! when you saturate the plan's rate-limit window. A profile records, per
//! **model lane** (the same buckets the rate table matches: `claude`, `gpt`,
//! `kimi`, `composer`, …), the billing mode and — for subscriptions — the
//! monthly price and an estimate of the list-dollar value the plan yields per
//! month, from which an *effective* cost multiplier is derived.
//!
//! A lane keys on the model, not the provider, on purpose: one provider (e.g.
//! omp/`pi`) can front several subscriptions — `kimi-for-coding` (a Kimi plan)
//! and `openai-codex/gpt-5.5` (a ChatGPT plan) — that bill completely
//! differently.

use chrono::{DateTime, Utc};
use serde::Serialize;
use sqlx::postgres::{PgPool, PgPoolOptions};

use crate::PersistError;

const CREATE_BILLING_PROFILES: &str = "
CREATE TABLE IF NOT EXISTS harness_billing_profiles (
    lane                  text PRIMARY KEY,
    billing_mode          text NOT NULL,
    monthly_price_usd     double precision NOT NULL DEFAULT 0,
    est_monthly_value_usd double precision,
    updated_at            timestamptz NOT NULL DEFAULT now()
)";

/// A model lane's billing profile (matches `harness_billing_profiles`).
#[derive(Debug, Clone, Serialize, sqlx::FromRow)]
pub struct BillingProfile {
    /// Model-lane key, matching the rate table's buckets (e.g. `claude`,
    /// `gpt`, `kimi`, `composer`).
    pub lane: String,
    /// `"usage_based"` (pay-per-token; effective == notional) or
    /// `"subscription"` (flat fee; effective is amortized).
    pub billing_mode: String,
    /// Monthly subscription price in USD (0 for usage-based / unset).
    pub monthly_price_usd: f64,
    /// Estimated list-dollar value the plan yields per month when saturated.
    /// `None` until known; without it the effective cost falls back to notional.
    pub est_monthly_value_usd: Option<f64>,
    pub updated_at: DateTime<Utc>,
}

impl BillingProfile {
    /// Multiplier applied to a lane's notional cost to get effective cost.
    /// `1.0` for usage-based or until a subscription is fully configured
    /// (price > 0 and a positive estimated monthly value).
    pub fn effective_multiplier(&self) -> f64 {
        if self.billing_mode != "subscription" {
            return 1.0;
        }
        match self.est_monthly_value_usd {
            Some(value) if value > 0.0 && self.monthly_price_usd > 0.0 => {
                self.monthly_price_usd / value
            }
            _ => 1.0,
        }
    }
}

/// Fields accepted when creating / updating a billing profile.
#[derive(Debug, Clone)]
pub struct BillingProfileInput {
    pub billing_mode: String,
    pub monthly_price_usd: f64,
    pub est_monthly_value_usd: Option<f64>,
}

/// Postgres-backed registry of per-lane billing profiles.
pub struct BillingProfileStore {
    pool: PgPool,
}

impl BillingProfileStore {
    /// Connect to `database_url` and ensure the schema exists.
    pub async fn connect(database_url: &str) -> Result<Self, PersistError> {
        let pool = PgPoolOptions::new()
            .max_connections(3)
            .connect(database_url)
            .await?;
        Self::from_pool(pool).await
    }

    /// Wrap an existing pool; ensures the table exists.
    pub async fn from_pool(pool: PgPool) -> Result<Self, PersistError> {
        let store = Self { pool };
        sqlx::query(CREATE_BILLING_PROFILES)
            .execute(&store.pool)
            .await?;
        Ok(store)
    }

    /// All configured profiles, ordered by lane.
    pub async fn list(&self) -> Result<Vec<BillingProfile>, PersistError> {
        let rows = sqlx::query_as::<_, BillingProfile>(
            "SELECT lane, billing_mode, monthly_price_usd, est_monthly_value_usd, updated_at
             FROM harness_billing_profiles ORDER BY lane",
        )
        .fetch_all(&self.pool)
        .await?;
        Ok(rows)
    }

    /// Insert or update a lane's profile.
    pub async fn upsert(
        &self,
        lane: &str,
        input: &BillingProfileInput,
    ) -> Result<BillingProfile, PersistError> {
        let row = sqlx::query_as::<_, BillingProfile>(
            "INSERT INTO harness_billing_profiles
                 (lane, billing_mode, monthly_price_usd, est_monthly_value_usd, updated_at)
             VALUES ($1, $2, $3, $4, now())
             ON CONFLICT (lane) DO UPDATE SET
                 billing_mode = excluded.billing_mode,
                 monthly_price_usd = excluded.monthly_price_usd,
                 est_monthly_value_usd = excluded.est_monthly_value_usd,
                 updated_at = now()
             RETURNING lane, billing_mode, monthly_price_usd, est_monthly_value_usd, updated_at",
        )
        .bind(lane)
        .bind(&input.billing_mode)
        .bind(input.monthly_price_usd)
        .bind(input.est_monthly_value_usd)
        .fetch_one(&self.pool)
        .await?;
        Ok(row)
    }

    /// Remove a lane's profile.
    pub async fn delete(&self, lane: &str) -> Result<(), PersistError> {
        sqlx::query("DELETE FROM harness_billing_profiles WHERE lane = $1")
            .bind(lane)
            .execute(&self.pool)
            .await?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn multiplier_is_one_for_usage_based() {
        let p = BillingProfile {
            lane: "composer".into(),
            billing_mode: "usage_based".into(),
            monthly_price_usd: 20.0,
            est_monthly_value_usd: None,
            updated_at: Utc::now(),
        };
        assert_eq!(p.effective_multiplier(), 1.0);
    }

    #[test]
    fn multiplier_amortizes_a_configured_subscription() {
        // $39 Kimi plan that yields ~$400 of list-priced usage/month → ~0.0975×.
        let p = BillingProfile {
            lane: "kimi".into(),
            billing_mode: "subscription".into(),
            monthly_price_usd: 39.0,
            est_monthly_value_usd: Some(400.0),
            updated_at: Utc::now(),
        };
        assert!((p.effective_multiplier() - 39.0 / 400.0).abs() < 1e-9);
    }

    #[test]
    fn multiplier_falls_back_to_notional_until_value_known() {
        let p = BillingProfile {
            lane: "claude".into(),
            billing_mode: "subscription".into(),
            monthly_price_usd: 100.0,
            est_monthly_value_usd: None,
            updated_at: Utc::now(),
        };
        assert_eq!(p.effective_multiplier(), 1.0);
    }
}
