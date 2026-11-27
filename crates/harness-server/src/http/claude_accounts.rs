//! **Several Claude subscriptions**, and which one a run uses.
//!
//! The first account is the `claude` credential, in `~/.claude`, as a harness
//! with one subscription has always had it. Each further one is a `claude:<id>`
//! credential with its own Claude Code config folder,
//! `~/.claude-accounts/<id>`, which a step reaches through `CLAUDE_CONFIG_DIR`.
//! Folders keep accounts apart: credentials, settings and sessions never mix.
//!
//! **Choosing.** Each account's usage comes from the same probe as its
//! dashboard card: the share used of its 5-hour and weekly windows. Those are
//! relative to that account's own plan, so they are weighted by the plan's
//! size, read from the credential (`rateLimitTier`: Pro, Max 5x, Max 20x) unless
//! an administrator set one. An account's headroom is what is left of its
//! tighter window times that size, discounted for the runs already using it,
//! whose usage the gauge has not caught up with yet. A run takes the account
//! with the most; see [`ServerRanker`] and `harness_runner::ClaudeAccountPool`.

use std::collections::{BTreeMap, HashMap};
use std::path::PathBuf;
use std::sync::{Arc, LazyLock, Mutex};

use async_trait::async_trait;
use axum::extract::{Extension, Path as AxumPath};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::Json;
use harness_persist::CredentialStore;
use harness_runner::{AccountRanker, AccountSlot};
use serde::{Deserialize, Serialize};
use serde_json::json;

use super::accounts::AdminOnly;
use super::runs_routes::RunsState;

/// The first account's id; its credential row is plain `claude`.
pub(crate) const DEFAULT_ID: &str = "default";
const KEY: &str = "claude";
const KEY_PREFIX: &str = "claude:";
/// An account this far through any window is left for when nothing else has
/// room: a step started on it would likely stop at the limit.
const NEARLY_SPENT_PCT: f64 = 95.0;
/// Each run already on an account takes this share off its headroom, until
/// the gauge shows what they used.
const PER_RUNNING_RUN: f64 = 0.85;

/// One connected Claude subscription.
#[derive(Debug, Clone, Serialize)]
pub(crate) struct ClaudeAccount {
    pub id: String,
    pub label: String,
    /// The plan read from the credential, e.g. `default_claude_max_5x`.
    pub tier: Option<String>,
    /// The plan's size relative to Pro: 1, 5, 20.
    pub weight: f64,
    /// Where the weight came from: `override`, `credential` or `default`.
    pub weight_source: &'static str,
    /// Whether a credential has been pasted.
    pub connected: bool,
    #[serde(skip)]
    pub key: String,
}

fn home_dir() -> PathBuf {
    std::env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("/home/harness"))
}

/// The credential row an account id lives under.
pub(crate) fn key_for(id: &str) -> String {
    if id == DEFAULT_ID {
        KEY.to_string()
    } else {
        format!("{KEY_PREFIX}{id}")
    }
}

/// The account id a credential row belongs to, if it is a Claude account.
pub(crate) fn id_for_key(key: &str) -> Option<String> {
    if key == KEY {
        return Some(DEFAULT_ID.to_string());
    }
    key.strip_prefix(KEY_PREFIX)
        .filter(|id| valid_id(id))
        .map(str::to_string)
}

/// Ids name folders: lower-case letters, digits and dashes only.
fn valid_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= 40
        && id != DEFAULT_ID
        && id
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
}

/// An account's Claude Code config folder.
pub(crate) fn config_dir(id: &str) -> PathBuf {
    if id == DEFAULT_ID {
        home_dir().join(".claude")
    } else {
        home_dir().join(".claude-accounts").join(id)
    }
}

/// The credential file Claude Code reads for an account.
pub(crate) fn credentials_path(id: &str) -> PathBuf {
    config_dir(id).join(".credentials.json")
}

/// The plan size a `rateLimitTier` stands for: Max 5x is 5, Max 20x is 20,
/// anything else (Pro, Team standard) 1.
fn tier_weight(tier: &str) -> f64 {
    let t = tier.to_ascii_lowercase();
    if let Some(i) = t.find("max_") {
        let digits: String = t[i + 4..]
            .chars()
            .take_while(char::is_ascii_digit)
            .collect();
        if let Ok(n) = digits.parse::<f64>() {
            if n > 0.0 {
                return n;
            }
        }
    }
    1.0
}

fn tier_of(credentials_json: &str) -> Option<String> {
    let v: serde_json::Value = serde_json::from_str(credentials_json).ok()?;
    v.get("claudeAiOauth")?
        .get("rateLimitTier")?
        .as_str()
        .map(str::to_string)
}

/// An account as its credential row describes it.
fn account_from(id: String, fields: &BTreeMap<String, String>) -> ClaudeAccount {
    let creds = std::fs::read_to_string(credentials_path(&id))
        .ok()
        .filter(|s| !s.trim().is_empty())
        .or_else(|| {
            fields
                .get("credentials_json")
                .cloned()
                .filter(|s| !s.is_empty())
        });
    let tier = creds.as_deref().and_then(tier_of);
    let manual = fields
        .get("plan_weight")
        .and_then(|w| w.trim().parse::<f64>().ok())
        .filter(|w| *w > 0.0);
    let (weight, weight_source) = match (manual, &tier) {
        (Some(w), _) => (w, "override"),
        (None, Some(t)) => (tier_weight(t), "credential"),
        (None, None) => (1.0, "default"),
    };
    let label = fields
        .get("label")
        .map(|l| l.trim().to_string())
        .filter(|l| !l.is_empty())
        .unwrap_or_else(|| {
            if id == DEFAULT_ID {
                "Claude".to_string()
            } else {
                format!("Claude {id}")
            }
        });
    ClaudeAccount {
        key: key_for(&id),
        id,
        label,
        tier,
        weight,
        weight_source,
        connected: creds.is_some(),
    }
}

/// Every Claude account, the default first.
pub(crate) async fn list(store: &CredentialStore) -> Vec<ClaudeAccount> {
    let mut out = Vec::new();
    let keys = store.list_configured().await.unwrap_or_default();
    // The default account counts when its file is there even without a row,
    // as a single-account harness always allowed.
    let default_fields = store.get(KEY).await.ok().flatten().unwrap_or_default();
    let default = account_from(DEFAULT_ID.to_string(), &default_fields);
    if default.connected || keys.iter().any(|k| k == KEY) {
        out.push(default);
    }
    for key in keys {
        let Some(id) = id_for_key(&key).filter(|id| id != DEFAULT_ID) else {
            continue;
        };
        let fields = store.get(&key).await.ok().flatten().unwrap_or_default();
        out.push(account_from(id, &fields));
    }
    out
}

// ── Choosing an account ─────────────────────────────────────────────────────

/// Runs currently using each account, across this process.
static RUNNING: LazyLock<Mutex<HashMap<String, usize>>> = LazyLock::new(Default::default);

pub(crate) fn running(id: &str) -> usize {
    RUNNING
        .lock()
        .map(|m| m.get(id).copied().unwrap_or(0))
        .unwrap_or(0)
}

/// How much of the account's tighter window is used, 0–100, when known.
type UsedPct = Option<f64>;

/// The order to use accounts in, best first. Pure, for testing.
pub(crate) fn rank_accounts(
    accounts: &[ClaudeAccount],
    used: &HashMap<String, UsedPct>,
    running: &dyn Fn(&str) -> usize,
    spent: &[String],
) -> Vec<ClaudeAccount> {
    let candidates: Vec<&ClaudeAccount> = accounts
        .iter()
        .filter(|a| a.connected && !spent.contains(&a.id))
        .collect();
    let headroom = |a: &ClaudeAccount| {
        // An account whose usage could not be read counts as half full:
        // usable, behind one known to have room.
        let used = used.get(&a.id).copied().flatten().unwrap_or(50.0);
        let left = (1.0 - used / 100.0).max(0.0);
        left * a.weight * PER_RUNNING_RUN.powi(running(&a.id) as i32)
    };
    let nearly_spent = |a: &ClaudeAccount| {
        used.get(&a.id)
            .copied()
            .flatten()
            .is_some_and(|u| u >= NEARLY_SPENT_PCT)
    };
    let mut roomy: Vec<&ClaudeAccount> = candidates
        .iter()
        .copied()
        .filter(|a| !nearly_spent(a))
        .collect();
    if roomy.is_empty() {
        // Nothing has room: still run, on the one with the most left, and let
        // the engine wait for its reset if it comes to that.
        roomy = candidates;
    }
    roomy.sort_by(|a, b| {
        headroom(b)
            .partial_cmp(&headroom(a))
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| a.id.cmp(&b.id))
    });
    roomy.into_iter().cloned().collect()
}

/// The server's answer to "which account": live usage, plans and the runs in
/// flight.
pub(crate) struct ServerRanker {
    pub state: Arc<RunsState>,
}

#[async_trait]
impl AccountRanker for ServerRanker {
    async fn rank(&self, spent: &[String]) -> Vec<AccountSlot> {
        let Ok(store) = self.state.cred_store().await else {
            return Vec::new();
        };
        let accounts = list(store).await;
        let used = super::usage_routes::claude_used_by_account(&self.state).await;
        rank_accounts(&accounts, &used, &running, spent)
            .into_iter()
            .map(|a| AccountSlot {
                config_dir: (a.id != DEFAULT_ID).then(|| config_dir(&a.id)),
                id: a.id,
                label: a.label,
            })
            .collect()
    }

    fn acquire(&self, id: &str) {
        if let Ok(mut m) = RUNNING.lock() {
            *m.entry(id.to_string()).or_default() += 1;
        }
    }

    fn release(&self, id: &str) {
        if let Ok(mut m) = RUNNING.lock() {
            if let Some(n) = m.get_mut(id) {
                *n = n.saturating_sub(1);
            }
        }
    }
}

/// A pool for a run, when the harness has more than one connected account.
/// `None` keeps a single-account harness exactly as it was.
pub(crate) async fn pool_for_run(
    state: &Arc<RunsState>,
) -> Option<Arc<harness_runner::ClaudeAccountPool>> {
    let store = state.cred_store().await.ok()?;
    let connected = list(store)
        .await
        .into_iter()
        .filter(|a| a.connected)
        .count();
    (connected > 1).then(|| {
        Arc::new(harness_runner::ClaudeAccountPool::new(Arc::new(
            ServerRanker {
                state: state.clone(),
            },
        )))
    })
}

// ── API ─────────────────────────────────────────────────────────────────────

fn err(status: StatusCode, msg: impl Into<String>) -> Response {
    (status, Json(json!({ "error": msg.into() }))).into_response()
}

/// `GET /api/claude-accounts` — every account with its plan, and how many runs
/// are on it now.
pub async fn list_accounts(_: AdminOnly, Extension(state): Extension<Arc<RunsState>>) -> Response {
    let store = match state.cred_store().await {
        Ok(s) => s,
        Err(e) => return err(StatusCode::SERVICE_UNAVAILABLE, e),
    };
    let accounts: Vec<_> = list(store)
        .await
        .into_iter()
        .map(|a| {
            let running = running(&a.id);
            json!({ "account": a, "running": running })
        })
        .collect();
    Json(json!({ "accounts": accounts })).into_response()
}

#[derive(Debug, Deserialize)]
pub struct NewAccount {
    pub label: String,
}

/// `POST /api/claude-accounts` — add an account. Its credential is pasted
/// afterwards, like the first one's, under `claude:<id>`.
pub async fn create_account(
    _: AdminOnly,
    Extension(state): Extension<Arc<RunsState>>,
    Json(req): Json<NewAccount>,
) -> Response {
    let label = req.label.trim();
    if label.is_empty() {
        return err(StatusCode::BAD_REQUEST, "an account needs a name");
    }
    let store = match state.cred_store().await {
        Ok(s) => s,
        Err(e) => return err(StatusCode::SERVICE_UNAVAILABLE, e),
    };
    let id = uuid::Uuid::new_v4().simple().to_string()[..8].to_string();
    let fields = BTreeMap::from([("label".to_string(), label.to_string())]);
    match store.set(&key_for(&id), &fields).await {
        Ok(()) => (
            StatusCode::CREATED,
            Json(json!({ "id": id, "provider": key_for(&id) })),
        )
            .into_response(),
        Err(e) => err(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()),
    }
}

#[derive(Debug, Deserialize)]
pub struct AccountUpdate {
    #[serde(default)]
    pub label: Option<String>,
    /// The plan's size relative to Pro; `0` or empty goes back to reading it
    /// from the credential.
    #[serde(default)]
    pub plan_weight: Option<f64>,
}

/// `PATCH /api/claude-accounts/{id}` — rename it, or set its plan size.
pub async fn update_account(
    _: AdminOnly,
    Extension(state): Extension<Arc<RunsState>>,
    AxumPath(id): AxumPath<String>,
    Json(req): Json<AccountUpdate>,
) -> Response {
    if id != DEFAULT_ID && !valid_id(&id) {
        return err(StatusCode::NOT_FOUND, "no such account");
    }
    let store = match state.cred_store().await {
        Ok(s) => s,
        Err(e) => return err(StatusCode::SERVICE_UNAVAILABLE, e),
    };
    let mut fields = BTreeMap::new();
    if let Some(label) = req
        .label
        .as_deref()
        .map(str::trim)
        .filter(|l| !l.is_empty())
    {
        fields.insert("label".to_string(), label.to_string());
    }
    if let Some(w) = req.plan_weight {
        if !(0.0..=1000.0).contains(&w) {
            return err(
                StatusCode::BAD_REQUEST,
                "`plan_weight` is between 0 and 1000",
            );
        }
        // Empty clears the override (`set` merges, so an empty value is how
        // a field is removed here).
        let value = if w > 0.0 {
            w.to_string()
        } else {
            String::new()
        };
        fields.insert("plan_weight".to_string(), value);
    }
    if fields.is_empty() {
        return err(StatusCode::BAD_REQUEST, "nothing to change");
    }
    match store.set(&key_for(&id), &fields).await {
        Ok(()) => Json(json!({ "saved": true })).into_response(),
        Err(e) => err(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()),
    }
}

/// `DELETE /api/claude-accounts/{id}` — remove an added account and its config
/// folder. The first account is cleared from its credential card instead.
pub async fn delete_account(
    _: AdminOnly,
    Extension(state): Extension<Arc<RunsState>>,
    AxumPath(id): AxumPath<String>,
) -> Response {
    if !valid_id(&id) {
        return err(
            StatusCode::BAD_REQUEST,
            "the first Claude account is cleared from its card, not removed",
        );
    }
    let store = match state.cred_store().await {
        Ok(s) => s,
        Err(e) => return err(StatusCode::SERVICE_UNAVAILABLE, e),
    };
    if let Err(e) = store.delete(&key_for(&id)).await {
        return err(StatusCode::INTERNAL_SERVER_ERROR, e.to_string());
    }
    let _ = std::fs::remove_dir_all(config_dir(&id));
    super::usage_routes::invalidate_cache().await;
    StatusCode::NO_CONTENT.into_response()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn acct(id: &str, weight: f64) -> ClaudeAccount {
        ClaudeAccount {
            id: id.into(),
            label: id.into(),
            tier: None,
            weight,
            weight_source: "credential",
            connected: true,
            key: key_for(id),
        }
    }

    #[test]
    fn plan_size_comes_from_the_tier() {
        assert_eq!(tier_weight("default_claude_max_5x"), 5.0);
        assert_eq!(tier_weight("default_claude_max_20x"), 20.0);
        assert_eq!(tier_weight("default_claude_pro"), 1.0);
        assert_eq!(tier_weight("default_claude_ai"), 1.0);
        let creds = r#"{"claudeAiOauth":{"rateLimitTier":"default_claude_max_20x"}}"#;
        assert_eq!(tier_of(creds).as_deref(), Some("default_claude_max_20x"));
    }

    #[test]
    fn keys_ids_and_folders() {
        assert_eq!(key_for(DEFAULT_ID), "claude");
        assert_eq!(key_for("ab12"), "claude:ab12");
        assert_eq!(id_for_key("claude").as_deref(), Some(DEFAULT_ID));
        assert_eq!(id_for_key("claude:ab12").as_deref(), Some("ab12"));
        assert_eq!(id_for_key("claude:../etc"), None, "ids name folders");
        assert_eq!(id_for_key("codex"), None);
        assert!(config_dir("ab12").ends_with(".claude-accounts/ab12"));
    }

    #[test]
    fn the_most_headroom_wins_weighted_by_plan() {
        let max20 = acct("max20", 20.0);
        let max5 = acct("max5", 5.0);
        let pro = acct("pro", 1.0);
        let accounts = [pro.clone(), max5.clone(), max20.clone()];
        let none = |_: &str| 0;
        let ids = |r: Vec<ClaudeAccount>| r.into_iter().map(|a| a.id).collect::<Vec<_>>();

        // Half used on Max 20x still beats an empty Pro.
        let used = HashMap::from([
            ("max20".into(), Some(50.0)),
            ("max5".into(), Some(80.0)),
            ("pro".into(), Some(0.0)),
        ]);
        assert_eq!(
            ids(rank_accounts(&accounts, &used, &none, &[])),
            ["max20", "pro", "max5"]
        );

        // Nearly spent accounts drop out while anything has room.
        let used = HashMap::from([
            ("max20".into(), Some(97.0)),
            ("max5".into(), Some(10.0)),
            ("pro".into(), Some(10.0)),
        ]);
        assert_eq!(
            ids(rank_accounts(&accounts, &used, &none, &[])),
            ["max5", "pro"]
        );

        // What this run found spent is skipped.
        assert_eq!(
            ids(rank_accounts(&accounts, &used, &none, &["max5".into()])),
            ["pro"]
        );

        // When everything is nearly spent, the one with the most left still
        // runs: 4% of Max 5x is more than 0.5% of Max 20x.
        let all_full = HashMap::from([
            ("max20".into(), Some(99.5)),
            ("max5".into(), Some(96.0)),
            ("pro".into(), Some(98.0)),
        ]);
        assert_eq!(
            ids(rank_accounts(&accounts, &all_full, &none, &[]))[0],
            "max5"
        );
    }

    #[test]
    fn runs_already_on_an_account_count_against_it() {
        let a = acct("a", 5.0);
        let b = acct("b", 5.0);
        let used = HashMap::from([("a".into(), Some(10.0)), ("b".into(), Some(20.0))]);
        let ids = |r: Vec<ClaudeAccount>| r.into_iter().map(|a| a.id).collect::<Vec<_>>();
        let idle = |_: &str| 0;
        assert_eq!(
            ids(rank_accounts(&[a.clone(), b.clone()], &used, &idle, &[]))[0],
            "a"
        );
        // Two runs already on `a`: `b` has more room in practice.
        let busy_a = |id: &str| if id == "a" { 2 } else { 0 };
        assert_eq!(ids(rank_accounts(&[a, b], &used, &busy_a, &[]))[0], "b");
    }

    #[test]
    fn unknown_usage_is_usable_but_behind_known_room() {
        let a = acct("a", 5.0);
        let b = acct("b", 5.0);
        let used = HashMap::from([("a".into(), None), ("b".into(), Some(10.0))]);
        let ranked = rank_accounts(&[a, b], &used, &|_| 0, &[]);
        assert_eq!(
            ranked.iter().map(|a| a.id.as_str()).collect::<Vec<_>>(),
            ["b", "a"]
        );
    }
}
