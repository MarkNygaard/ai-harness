//! `GET /api/usage` — subscription usage (rolling + weekly windows) per connected
//! CLI, for the dashboard's "Subscriptions" cards.
//!
//! Each subscription's usage is read from the **same access path the harness
//! actually uses it through**, so the limits are the real ones:
//! - **Claude** → Claude Code's own `api/oauth/usage` endpoint with the
//!   subscription OAuth token (first-party Pro/Max windows — NOT omp's lower API
//!   tier).
//! - **ChatGPT/Codex** and **Kimi** → the omp auth-broker `/v1/usage` (those
//!   genuinely run through omp, so its report reflects the limits in force).
//!
//! Both upstreams rate-limit `/usage` aggressively per source IP, so the whole
//! response is cached process-wide for [`CACHE_TTL`]; the dashboard polling can't
//! trip a 429.

use std::path::PathBuf;
use std::sync::Arc;
use std::sync::LazyLock;
use std::time::{Duration, Instant};

use axum::extract::Extension;
use axum::response::{IntoResponse, Response};
use axum::Json;
use chrono::{DateTime, Datelike, TimeZone, Utc};
use serde::Serialize;
use tokio::sync::Mutex;

use super::runs_routes::RunsState;
use crate::handlers::token_usage::{lane_for_model, rates_for_model};

/// Cache floor — at/above Claude's ~3-minute polling floor and omp's 5-minute
/// report TTL, so the dashboard never hammers the upstream `/usage` endpoints.
const CACHE_TTL: Duration = Duration::from_secs(180);

/// A single rate-limit window for a subscription (e.g. the 5-hour or weekly cap).
#[derive(Clone, Serialize)]
struct UsageWindow {
    label: String,
    /// Percent of the window consumed (0–100). Ignored by the UI when `amount`
    /// is set (an absolute figure is shown instead of a percent bar).
    #[serde(rename = "usedPct")]
    used_pct: f64,
    /// Absolute reset time (RFC3339), when known.
    #[serde(rename = "resetsAt")]
    resets_at: Option<String>,
    /// Preformatted absolute figure (e.g. `"$1.86"`) shown in place of the
    /// percent bar — used where a percentage would be misleading (Cursor, whose
    /// real quota we can't read, so we only have a notional cost estimate).
    #[serde(skip_serializing_if = "Option::is_none")]
    amount: Option<String>,
    /// Short qualifier under the amount (e.g. `"notional · API list rates"`).
    #[serde(skip_serializing_if = "Option::is_none")]
    caption: Option<String>,
}

/// One subscription card.
#[derive(Clone, Serialize)]
struct SubscriptionUsage {
    /// Stable key for the UI (`claude` | `codex` | `kimi`).
    cli: &'static str,
    label: &'static str,
    /// True when we got a usage report; false when the source was unreachable.
    available: bool,
    error: Option<String>,
    windows: Vec<UsageWindow>,
}

#[derive(Clone, Serialize)]
pub(crate) struct UsageResponse {
    subscriptions: Vec<SubscriptionUsage>,
}

static CACHE: LazyLock<Mutex<Option<(Instant, UsageResponse)>>> =
    LazyLock::new(|| Mutex::new(None));

/// Drop the cached usage report so the next `GET /api/usage` rebuilds from
/// scratch. Called when a credential changes (e.g. a usage-card visibility
/// toggle) so the dashboard reflects it on the next poll, not after the TTL.
pub(crate) async fn invalidate_cache() {
    *CACHE.lock().await = None;
}

fn home_dir() -> PathBuf {
    std::env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("/home/harness"))
}

/// `GET /api/usage` — per-subscription usage for connected CLIs (cached).
pub async fn get_usage(Extension(state): Extension<Arc<RunsState>>) -> Response {
    Json(cached_usage(&state).await).into_response()
}

/// Cached per-subscription usage report (the body of [`get_usage`], reusable by
/// internal callers like the billing calibrator).
pub(crate) async fn cached_usage(state: &RunsState) -> UsageResponse {
    if let Some((at, resp)) = CACHE.lock().await.as_ref() {
        if at.elapsed() < CACHE_TTL {
            return resp.clone();
        }
    }

    let creds = super::credentials_routes::connected_clis().await;
    // Per-credential opt-out: each card can be hidden from the dashboard via the
    // `show_usage_card` field on its credential (default: shown). Hidden cards
    // are skipped before fetching, so e.g. hiding Claude also stops its probe.
    let (show_claude, show_codex, show_kimi, show_cursor) = match state.cred_store().await {
        Ok(s) => (
            super::credentials_routes::usage_card_visible(&s, "claude").await,
            super::credentials_routes::usage_card_visible(&s, "codex").await,
            super::credentials_routes::usage_card_visible(&s, "pi").await,
            super::credentials_routes::usage_card_visible(&s, "cursor").await,
        ),
        Err(_) => (true, true, true, true),
    };
    let mut subscriptions = Vec::new();

    if creds.claude && show_claude {
        subscriptions.push(fetch_claude_usage(state).await);
    }
    // ChatGPT/Codex and Kimi both come from one omp broker call.
    if (creds.codex && show_codex) || (creds.kimi && show_kimi) {
        let broker = fetch_broker_reports().await;
        if creds.codex && show_codex {
            subscriptions.push(broker_subscription(
                "codex",
                "ChatGPT (Codex)",
                "openai-codex",
                &broker,
            ));
        }
        if creds.kimi && show_kimi {
            subscriptions.push(broker_subscription(
                "kimi",
                "Kimi-for-Coding",
                "kimi-code",
                &broker,
            ));
        }
    }
    // Cursor has no usage API for individual plans, so we self-track and show
    // this month's Cursor (composer-lane) spend at list rates as a notional
    // dollar figure (its dashboard percentage isn't readable). Shown whenever a
    // Cursor credential is added.
    if show_cursor {
        if let Some(sub) = fetch_cursor_usage(state).await {
            subscriptions.push(sub);
        }
    }

    let resp = UsageResponse { subscriptions };
    *CACHE.lock().await = Some((Instant::now(), resp.clone()));
    resp
}

/// The weekly window (consumed %, reset time) for a subscription `cli`
/// (`claude` | `codex` | `kimi`), if available. The calibrator pairs this with
/// the tokens the harness spent in the window to estimate the plan's capacity.
pub(crate) async fn weekly_window_for(
    state: &RunsState,
    cli: &str,
) -> Option<(f64, Option<chrono::DateTime<chrono::Utc>>)> {
    let report = cached_usage(state).await;
    let sub = report
        .subscriptions
        .iter()
        .find(|s| s.cli == cli && s.available)?;
    let window = sub.windows.iter().find(|w| w.label == "Weekly")?;
    let resets_at = window
        .resets_at
        .as_deref()
        .and_then(|s| chrono::DateTime::parse_from_rfc3339(s).ok())
        .map(|dt| dt.with_timezone(&chrono::Utc));
    Some((window.used_pct, resets_at))
}

fn unavailable(
    cli: &'static str,
    label: &'static str,
    err: impl Into<String>,
) -> SubscriptionUsage {
    SubscriptionUsage {
        cli,
        label,
        available: false,
        error: Some(err.into()),
        windows: Vec::new(),
    }
}

// ── Cursor (self-tracked: no usage API for individual plans) ─────────────────

/// First instant of `now`'s calendar month (UTC); falls back to `now`.
fn month_start_utc(now: DateTime<Utc>) -> DateTime<Utc> {
    Utc.with_ymd_and_hms(now.year(), now.month(), 1, 0, 0, 0)
        .single()
        .unwrap_or(now)
}

/// First instant of the month after `now` (UTC) — the budget's reset point.
fn next_month_start_utc(now: DateTime<Utc>) -> DateTime<Utc> {
    let (year, month) = if now.month() == 12 {
        (now.year() + 1, 1)
    } else {
        (now.year(), now.month() + 1)
    };
    Utc.with_ymd_and_hms(year, month, 1, 0, 0, 0)
        .single()
        .unwrap_or(now)
}

/// Cursor usage card. Cursor exposes no usage API for individual plans, and its
/// dashboard percentage is measured against an undisclosed Auto+Composer
/// allowance at discounted rates — neither of which we can read. So we can't
/// mirror that percentage; we self-track instead, summing this calendar month's
/// Cursor (`composer`-lane) token spend at API **list rates** and showing it as
/// a notional dollar figure (NOT a percent — a `% of $20` would misrepresent
/// both the denominator and the rate). Shown whenever a Cursor credential exists.
async fn fetch_cursor_usage(state: &RunsState) -> Option<SubscriptionUsage> {
    // Only when a Cursor credential has been added (an api_key is stored).
    let store = state.cred_store().await.ok()?;
    let has_cursor_cred = store
        .get("cursor")
        .await
        .ok()
        .flatten()
        .and_then(|f| f.get("api_key").map(|k| !k.is_empty()))
        .unwrap_or(false);
    if !has_cursor_cred {
        return None;
    }

    let now = Utc::now();
    let sums = match state.store().await {
        Ok(store) => match store.token_sums_by_model_since(month_start_utc(now)).await {
            Ok(s) => s,
            Err(e) => return Some(unavailable("cursor", "Cursor", e.to_string())),
        },
        Err(e) => return Some(unavailable("cursor", "Cursor", e)),
    };
    let spend_usd: f64 = sums
        .iter()
        .filter(|s| lane_for_model(&s.model) == "composer")
        .map(|s| {
            rates_for_model(&s.model).cost_usd(
                s.input_tokens.max(0) as u64,
                s.output_tokens.max(0) as u64,
                s.cache_read.max(0) as u64,
                s.cache_write.max(0) as u64,
            )
        })
        .sum();

    Some(SubscriptionUsage {
        cli: "cursor",
        label: "Cursor",
        available: true,
        error: None,
        windows: vec![UsageWindow {
            label: "This month".to_string(),
            used_pct: 0.0, // ignored — `amount` is set, so no bar is shown
            resets_at: Some(next_month_start_utc(now).to_rfc3339()),
            amount: Some(format!("${spend_usd:.2}")),
            caption: Some("notional · API list rates".to_string()),
        }],
    })
}

// ── Claude (first-party Claude Code usage) ───────────────────────────────────

/// Claude subscription usage. The dedicated `oauth/usage` endpoint needs a
/// `user:profile` scope that subscription tokens never carry (so it 403s/429s);
/// instead we make the same trivial `/v1/messages` call the CLI does — which the
/// token's `user:inference` scope *does* permit — and read usage off the
/// `anthropic-ratelimit-unified-*` response headers. Costs one 1-token Haiku
/// request per cache refresh (≈ every 3 min). Approach borrowed from Clawdmeter.
const CLAUDE_MESSAGES_URL: &str = "https://api.anthropic.com/v1/messages";

async fn fetch_claude_usage(state: &RunsState) -> SubscriptionUsage {
    const CLI: &str = "claude";
    const LABEL: &str = "Claude Code";
    let Some(token) = claude_token(state).await else {
        return unavailable(CLI, LABEL, "no Claude credential connected");
    };

    let client = reqwest::Client::new();
    let resp = client
        .post(CLAUDE_MESSAGES_URL)
        .header("Authorization", format!("Bearer {token}"))
        .header("anthropic-version", "2023-06-01")
        // The OAuth subscription token path is gated behind this beta flag and
        // the Claude Code CLI User-Agent.
        .header("anthropic-beta", "oauth-2025-04-20")
        .header("User-Agent", "claude-code/2.1.5")
        .json(&serde_json::json!({
            "model": "claude-haiku-4-5-20251001",
            "max_tokens": 1,
            "messages": [{ "role": "user", "content": "hi" }],
        }))
        .send()
        .await;
    let resp = match resp {
        Ok(r) if r.status().is_success() => r,
        Ok(r) => return unavailable(CLI, LABEL, format!("Claude usage HTTP {}", r.status())),
        Err(e) => return unavailable(CLI, LABEL, format!("Claude usage request failed: {e}")),
    };

    // Usage rides on the unified rate-limit headers: utilization is a 0..1
    // fraction of the window consumed; reset is a unix timestamp (seconds).
    let headers = resp.headers();
    let util = |name: &str| -> Option<f64> { headers.get(name)?.to_str().ok()?.parse().ok() };
    let reset_rfc3339 = |name: &str| -> Option<String> {
        let secs: i64 = headers.get(name)?.to_str().ok()?.parse().ok()?;
        chrono::DateTime::from_timestamp(secs, 0).map(|dt| dt.to_rfc3339())
    };

    let mut windows = Vec::new();
    for (util_hdr, reset_hdr, label) in [
        (
            "anthropic-ratelimit-unified-5h-utilization",
            "anthropic-ratelimit-unified-5h-reset",
            "5-hour",
        ),
        (
            "anthropic-ratelimit-unified-7d-utilization",
            "anthropic-ratelimit-unified-7d-reset",
            "Weekly",
        ),
    ] {
        if let Some(frac) = util(util_hdr) {
            windows.push(UsageWindow {
                label: label.to_string(),
                used_pct: frac * 100.0,
                resets_at: reset_rfc3339(reset_hdr),
                amount: None,
                caption: None,
            });
        }
    }
    if windows.is_empty() {
        return unavailable(CLI, LABEL, "Claude usage rate-limit headers missing");
    }
    SubscriptionUsage {
        cli: CLI,
        label: LABEL,
        available: true,
        error: None,
        windows,
    }
}

/// The Claude Code subscription OAuth access token. Prefer the self-refreshing
/// on-disk credential (claude refreshes it on every run); fall back to the
/// encrypted store.
async fn claude_token(state: &RunsState) -> Option<String> {
    let disk = std::fs::read_to_string(home_dir().join(".claude").join(".credentials.json"))
        .ok()
        .and_then(|j| parse_claude_access(&j));
    if disk.is_some() {
        return disk;
    }
    let store = state.cred_store().await.ok()?;
    let claude = store.get("claude").await.ok().flatten()?;
    if let Some(j) = claude.get("credentials_json").filter(|v| !v.is_empty()) {
        if let Some(t) = parse_claude_access(j) {
            return Some(t);
        }
    }
    claude.get("oauth_token").filter(|v| !v.is_empty()).cloned()
}

/// Extract the access token from a `~/.claude/.credentials.json` body.
fn parse_claude_access(json: &str) -> Option<String> {
    let v: serde_json::Value = serde_json::from_str(json).ok()?;
    v.get("claudeAiOauth")?
        .get("accessToken")?
        .as_str()
        .filter(|s| !s.is_empty())
        .map(str::to_string)
}

// ── omp auth-broker (Codex + Kimi) ───────────────────────────────────────────

/// Loopback address the server self-hosts `omp auth-broker serve` on, so the
/// Codex/Kimi usage cards work off the container's local omp creds with no
/// external broker or env config.
const LOCAL_BROKER_BIND: &str = "127.0.0.1:8765";
const LOCAL_BROKER_URL: &str = "http://127.0.0.1:8765";

/// Self-host `omp auth-broker serve` on loopback so the usage cards can read the
/// local omp credentials (the ones the "Connect" flow writes to `agent.db`) — no
/// external broker or env wiring needed. Skipped when an external
/// `OMP_AUTH_BROKER_URL` is set, or when disabled via
/// `HARNESS_SELF_HOST_OMP_BROKER=0`. Supervised: respawns with backoff if it
/// exits. **Usage-only** — runs still authenticate from the local `agent.db`
/// directly (we never set the global broker env), so this doesn't change how
/// agents run.
pub fn spawn_local_broker() {
    if std::env::var("OMP_AUTH_BROKER_URL")
        .map(|s| !s.is_empty())
        .unwrap_or(false)
    {
        tracing::info!("OMP_AUTH_BROKER_URL set — using the external auth-broker for usage cards");
        return;
    }
    if std::env::var("HARNESS_SELF_HOST_OMP_BROKER")
        .map(|v| v == "0" || v.eq_ignore_ascii_case("false"))
        .unwrap_or(false)
    {
        return;
    }
    tokio::spawn(async move {
        let omp = std::env::var_os("OMP_CLI")
            .or_else(|| std::env::var_os("OMP_PATH"))
            .unwrap_or_else(|| std::ffi::OsString::from("omp"));
        let mut backoff = Duration::from_secs(2);
        loop {
            let mut cmd = tokio::process::Command::new(&omp);
            cmd.arg("auth-broker")
                .arg("serve")
                .arg("--bind")
                .arg(LOCAL_BROKER_BIND);
            // Agent-session vars make spawned CLIs SIGTRAP — never propagate them.
            cmd.env_remove("CLAUDECODE");
            cmd.env_remove("CLAUDE_CODE_ENTRYPOINT");
            match cmd.spawn() {
                Ok(mut child) => {
                    backoff = Duration::from_secs(2);
                    tracing::info!(
                        "self-hosting omp auth-broker on {LOCAL_BROKER_BIND} for usage cards"
                    );
                    let _ = child.wait().await;
                    tracing::warn!("local omp auth-broker exited; restarting shortly");
                }
                Err(e) => {
                    tracing::warn!("could not start omp auth-broker (usage cards unavailable): {e}")
                }
            }
            tokio::time::sleep(backoff).await;
            backoff = (backoff * 2).min(Duration::from_secs(60));
        }
    });
}

/// Fetch `GET {broker}/v1/usage` and return its `reports` array. `Err` carries a
/// reason (broker unreachable / not ready) used as the card's error.
async fn fetch_broker_reports() -> Result<Vec<serde_json::Value>, String> {
    // Prefer an external broker; otherwise the loopback one we self-host.
    let base = std::env::var("OMP_AUTH_BROKER_URL")
        .ok()
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| LOCAL_BROKER_URL.to_string());
    let token = broker_token()
        .ok_or("auth-broker not ready yet — reconnect the subscription, then refresh")?;

    let client = reqwest::Client::new();
    let resp = client
        .get(format!("{}/v1/usage", base.trim_end_matches('/')))
        .header("Authorization", format!("Bearer {token}"))
        .header("Accept", "application/json")
        .send()
        .await
        .map_err(|e| format!("broker request failed: {e}"))?;
    if !resp.status().is_success() {
        return Err(format!("broker /v1/usage HTTP {}", resp.status()));
    }
    let body: serde_json::Value = resp
        .json()
        .await
        .map_err(|e| format!("broker parse failed: {e}"))?;
    Ok(body
        .get("reports")
        .and_then(|r| r.as_array())
        .cloned()
        .unwrap_or_default())
}

fn broker_token() -> Option<String> {
    if let Ok(t) = std::env::var("OMP_AUTH_BROKER_TOKEN") {
        if !t.is_empty() {
            return Some(t);
        }
    }
    std::fs::read_to_string(home_dir().join(".omp").join("auth-broker.token"))
        .ok()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
}

/// Build a subscription card from the broker reports for a given omp provider id.
fn broker_subscription(
    cli: &'static str,
    label: &'static str,
    provider: &str,
    reports: &Result<Vec<serde_json::Value>, String>,
) -> SubscriptionUsage {
    let reports = match reports {
        Ok(r) => r,
        Err(e) => return unavailable(cli, label, e.clone()),
    };
    let Some(report) = reports
        .iter()
        .find(|r| r.get("provider").and_then(|p| p.as_str()) == Some(provider))
    else {
        return unavailable(cli, label, "no usage reported for this subscription yet");
    };

    let mut windows = Vec::new();
    if let Some(limits) = report.get("limits").and_then(|l| l.as_array()) {
        for limit in limits {
            let window = limit.get("window");
            let label = window
                .and_then(|w| w.get("label"))
                .and_then(|v| v.as_str())
                .or_else(|| {
                    limit
                        .get("scope")
                        .and_then(|s| s.get("windowId"))
                        .and_then(|v| v.as_str())
                })
                .or_else(|| limit.get("label").and_then(|v| v.as_str()))
                .unwrap_or("Usage")
                .to_string();
            let amount = limit.get("amount");
            // `used` is already 0–100 for percent-unit providers (codex); else
            // derive from the 0..1 fraction (kimi).
            let used_pct = amount
                .and_then(|a| {
                    let unit = a.get("unit").and_then(|u| u.as_str());
                    if unit == Some("percent") {
                        a.get("used").and_then(|v| v.as_f64())
                    } else {
                        a.get("usedFraction")
                            .and_then(|v| v.as_f64())
                            .map(|f| f * 100.0)
                    }
                })
                .unwrap_or(0.0);
            let resets_at = window
                .and_then(|w| w.get("resetsAt"))
                .and_then(|v| v.as_f64())
                .and_then(|ms| chrono::DateTime::from_timestamp_millis(ms as i64))
                .map(|dt| dt.to_rfc3339_opts(chrono::SecondsFormat::Secs, true));
            windows.push(UsageWindow {
                label,
                used_pct,
                resets_at,
                amount: None,
                caption: None,
            });
        }
    }
    if windows.is_empty() {
        return unavailable(cli, label, "no usage windows reported yet");
    }
    SubscriptionUsage {
        cli,
        label,
        available: true,
        error: None,
        windows,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn month_boundaries_are_first_of_month_utc() {
        let now = Utc.with_ymd_and_hms(2026, 6, 17, 9, 30, 0).unwrap();
        assert_eq!(
            month_start_utc(now),
            Utc.with_ymd_and_hms(2026, 6, 1, 0, 0, 0).unwrap()
        );
        assert_eq!(
            next_month_start_utc(now),
            Utc.with_ymd_and_hms(2026, 7, 1, 0, 0, 0).unwrap()
        );
        // December rolls the year.
        let dec = Utc.with_ymd_and_hms(2026, 12, 5, 0, 0, 0).unwrap();
        assert_eq!(
            next_month_start_utc(dec),
            Utc.with_ymd_and_hms(2027, 1, 1, 0, 0, 0).unwrap()
        );
    }

    #[test]
    fn parses_claude_access_token() {
        let body = r#"{"claudeAiOauth":{"accessToken":"sk-ant-oat-abc","refreshToken":"r"}}"#;
        assert_eq!(parse_claude_access(body).as_deref(), Some("sk-ant-oat-abc"));
        assert_eq!(parse_claude_access("{}"), None);
        assert_eq!(parse_claude_access("not json"), None);
    }

    #[test]
    fn broker_subscription_maps_codex_percent_windows() {
        // Mirrors omp's openai-codex report: primary (5h) + secondary (7d),
        // percent-unit amounts, absolute ms reset.
        let reports = Ok(vec![json!({
            "provider": "openai-codex",
            "limits": [
                {
                    "id": "openai-codex:primary",
                    "label": "5 Hours",
                    "scope": { "provider": "openai-codex", "windowId": "5h" },
                    "window": { "id": "5h", "label": "5 Hour", "resetsAt": 1_700_000_000_000.0 },
                    "amount": { "used": 45.0, "limit": 100.0, "usedFraction": 0.45, "unit": "percent" }
                },
                {
                    "id": "openai-codex:secondary",
                    "label": "7 Days",
                    "scope": { "provider": "openai-codex", "windowId": "7d" },
                    "window": { "id": "7d", "label": "7 Day", "resetsAt": 1_700_600_000_000.0 },
                    "amount": { "used": 62.0, "unit": "percent" }
                }
            ]
        })]);
        let sub = broker_subscription("codex", "ChatGPT (Codex)", "openai-codex", &reports);
        assert!(sub.available);
        assert_eq!(sub.windows.len(), 2);
        assert_eq!(sub.windows[0].label, "5 Hour");
        assert_eq!(sub.windows[0].used_pct, 45.0);
        assert!(sub.windows[0].resets_at.is_some());
        assert_eq!(sub.windows[1].used_pct, 62.0);
    }

    #[test]
    fn broker_subscription_derives_percent_from_fraction() {
        // Kimi-style: unit "unknown" with raw counts → percent from usedFraction.
        let reports = Ok(vec![json!({
            "provider": "kimi-code",
            "limits": [{
                "id": "kimi-code:0",
                "label": "Quota",
                "scope": { "provider": "kimi-code", "windowId": "weekly" },
                "window": { "id": "weekly", "label": "Weekly" },
                "amount": { "used": 30.0, "limit": 120.0, "usedFraction": 0.25, "unit": "unknown" }
            }]
        })]);
        let sub = broker_subscription("kimi", "Kimi-for-Coding", "kimi-code", &reports);
        assert!(sub.available);
        assert_eq!(sub.windows[0].used_pct, 25.0);
        assert_eq!(sub.windows[0].resets_at, None);
    }

    #[test]
    fn broker_subscription_handles_missing_and_errored() {
        let empty: Result<Vec<serde_json::Value>, String> = Ok(vec![]);
        let missing = broker_subscription("kimi", "Kimi-for-Coding", "kimi-code", &empty);
        assert!(!missing.available);
        assert!(missing.error.is_some());

        let errored: Result<Vec<serde_json::Value>, String> = Err("broker down".into());
        let down = broker_subscription("codex", "ChatGPT (Codex)", "openai-codex", &errored);
        assert!(!down.available);
        assert_eq!(down.error.as_deref(), Some("broker down"));
    }
}
