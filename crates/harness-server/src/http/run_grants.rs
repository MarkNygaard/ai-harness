//! **What a run is allowed to ask the server to do on its behalf.**
//!
//! A run must never hold the server's credentials. `strip_control_plane_env`
//! removes the database URL and the encryption key at every spawn point, on
//! purpose: an agent's environment ends up in its context, and "run `env` to see
//! what is available" is ordinary agent behaviour, so a secret placed there
//! reaches a model provider and a run log by accident rather than by attack.
//!
//! So a run gets a **capability** instead. This mints a token bound to one run
//! and one project; the server keeps the credentials and does the work. What a
//! leaked run token buys is the handful of Linear operations the epic workflows
//! need, on a single project, until it expires — against a leaked
//! `HARNESS_SECRET_KEY`, which buys every credential the install holds.
//!
//! **The project comes from the grant, never from the request.** That is the
//! whole security property: a run cannot name somebody else's project, because
//! it is not asked which project it is.
//!
//! In-process, like [`super::sso_flow`]'s pending map, and for the same reason:
//! the harness is one container, and the call lands on the instance that minted
//! the token. A restart mid-run costs that run its grant, which fails loudly.

use std::collections::HashMap;
use std::sync::LazyLock;

use sha2::{Digest, Sha256};

/// How long a grant outlives its minting.
///
/// Comfortably longer than a run — an epic piece can take an hour, and a
/// supervisor waits on nothing — and short enough that a token recovered from an
/// old log is useless.
const GRANT_TTL_MS: i64 = 12 * 60 * 60 * 1000;

/// What a token permits.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Grant {
    pub run_id: String,
    /// The only project this token can touch.
    pub project: String,
    created_at: i64,
}

impl Grant {
    fn live(&self, now: i64) -> bool {
        now - self.created_at < GRANT_TTL_MS
    }
}

/// Hashed, so the map never holds a usable token — the same reason sessions and
/// personal tokens are stored hashed.
static GRANTS: LazyLock<std::sync::Mutex<HashMap<String, Grant>>> =
    LazyLock::new(|| std::sync::Mutex::new(HashMap::new()));

fn hash(token: &str) -> String {
    format!("{:x}", Sha256::digest(token.as_bytes()))
}

fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

/// Mint a token for one run, pruning expired grants.
///
/// Prefixed so a token that escapes into a log or a repository is recognisable
/// as one — the same reason MCP keys carry `hrn_mcp_`.
pub(crate) fn mint(run_id: &str, project: &str) -> String {
    let token = format!(
        "hrn_run_{}{}",
        uuid::Uuid::new_v4().simple(),
        uuid::Uuid::new_v4().simple()
    );
    let now = now_ms();
    if let Ok(mut map) = GRANTS.lock() {
        map.retain(|_, g| g.live(now));
        map.insert(
            hash(&token),
            Grant {
                run_id: run_id.to_string(),
                project: project.to_string(),
                created_at: now,
            },
        );
    }
    token
}

/// What this token permits, if anything.
pub(crate) fn redeem(token: &str) -> Option<Grant> {
    let map = GRANTS.lock().ok()?;
    let grant = map.get(&hash(token))?;
    grant.live(now_ms()).then(|| grant.clone())
}

/// Give up a run's grant the moment it finishes.
///
/// The TTL is the backstop, not the mechanism: a token that outlives the run it
/// was minted for is a capability nobody is watching.
pub(crate) fn revoke_for_run(run_id: &str) {
    if let Ok(mut map) = GRANTS.lock() {
        map.retain(|_, g| g.run_id != run_id);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_token_names_its_run_and_project() {
        let token = mint("run-1", "ai-harness");
        let grant = redeem(&token).expect("live");
        assert_eq!(grant.run_id, "run-1");
        assert_eq!(grant.project, "ai-harness");
        revoke_for_run("run-1");
    }

    #[test]
    fn an_unknown_token_permits_nothing() {
        assert!(redeem("hrn_run_nope").is_none());
        assert!(redeem("").is_none());
    }

    #[test]
    fn revoking_a_run_ends_its_token_immediately() {
        // The run is over; the capability should not outlive it waiting for a
        // twelve-hour clock.
        let token = mint("run-2", "p");
        assert!(redeem(&token).is_some());
        revoke_for_run("run-2");
        assert!(redeem(&token).is_none());
    }

    #[test]
    fn one_runs_token_is_not_anothers() {
        let a = mint("run-3", "project-a");
        let b = mint("run-4", "project-b");
        assert_eq!(redeem(&a).unwrap().project, "project-a");
        assert_eq!(redeem(&b).unwrap().project, "project-b");
        // Revoking one leaves the other alone.
        revoke_for_run("run-3");
        assert!(redeem(&a).is_none());
        assert!(redeem(&b).is_some());
        revoke_for_run("run-4");
    }

    #[test]
    fn the_token_is_recognisable_and_not_stored_in_the_clear() {
        let token = mint("run-5", "p");
        assert!(token.starts_with("hrn_run_"), "{token}");
        let map = GRANTS.lock().unwrap();
        assert!(
            !map.contains_key(&token),
            "the map must key on the hash, never the token itself"
        );
        assert!(map.contains_key(&hash(&token)));
        drop(map);
        revoke_for_run("run-5");
    }
}
