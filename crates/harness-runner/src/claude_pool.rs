//! **Several Claude accounts behind one run.**
//!
//! When a harness has more than one Claude subscription connected, each run
//! gets a [`ClaudeAccountPool`]. The pool asks an [`AccountRanker`] (the
//! server, which knows every account's usage and plan) for the account with the
//! most headroom, and every Claude step of the run uses it: Claude Code keeps a
//! session in its account's config folder, so a run that resumes sessions has
//! to stay where they are.
//!
//! When a step hits that account's usage limit, the pool marks the account
//! spent until it resets and moves the run to the next best one. The step that
//! hit the limit starts again from scratch there, since its conversation lives
//! in the other account. Only when every account is spent does the limit reach
//! the engine, which then waits for a reset as it always has.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use async_trait::async_trait;

/// One account a step can run on.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AccountSlot {
    pub id: String,
    /// What a person calls it, for the run's activity feed.
    pub label: String,
    /// The account's own Claude Code config folder, given to the CLI as
    /// `CLAUDE_CONFIG_DIR`. `None` for the default account, which uses
    /// `~/.claude` like a single-account harness always has.
    pub config_dir: Option<PathBuf>,
}

/// The server's side: which accounts to use, best first.
#[async_trait]
pub trait AccountRanker: Send + Sync {
    /// Every usable account, best first, leaving out those in `spent`.
    async fn rank(&self, spent: &[String]) -> Vec<AccountSlot>;
    /// A run started using `id`.
    fn acquire(&self, id: &str);
    /// A run stopped using `id`.
    fn release(&self, id: &str);
}

#[derive(Default)]
struct PoolState {
    current: Option<AccountSlot>,
    /// Accounts this run found at their limit.
    spent: Vec<String>,
    /// Which account each Claude session was started on.
    sessions: HashMap<String, String>,
}

/// One run's view of the Claude accounts.
pub struct ClaudeAccountPool {
    ranker: Arc<dyn AccountRanker>,
    state: Mutex<PoolState>,
}

impl ClaudeAccountPool {
    pub fn new(ranker: Arc<dyn AccountRanker>) -> Self {
        Self {
            ranker,
            state: Mutex::new(PoolState::default()),
        }
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, PoolState> {
        self.state.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// The account a step runs on, and the session it may resume.
    ///
    /// The run's current account, chosen on first use. A step resuming a
    /// session goes to that session's account when it is still usable; if not,
    /// the session is dropped and the step starts fresh on the current account.
    pub async fn account_for(
        &self,
        session: Option<&str>,
    ) -> Option<(AccountSlot, Option<String>)> {
        let (current, spent, session_account) = {
            let state = self.lock();
            (
                state.current.clone(),
                state.spent.clone(),
                session.and_then(|s| state.sessions.get(s).cloned()),
            )
        };
        let current = match current {
            Some(c) => c,
            None => {
                let best = self.ranker.rank(&spent).await.into_iter().next()?;
                let mut state = self.lock();
                // Another step of this run may have picked one meanwhile.
                match &state.current {
                    Some(c) => c.clone(),
                    None => {
                        self.ranker.acquire(&best.id);
                        state.current = Some(best.clone());
                        best
                    }
                }
            }
        };
        let session = session.map(str::to_string);
        match session_account {
            // A session from an account that is now spent cannot be resumed
            // anywhere else.
            Some(acct) if acct != current.id && spent.contains(&acct) => Some((current, None)),
            Some(acct) if acct != current.id => {
                // Still usable: resume it where it lives.
                let slot = self
                    .ranker
                    .rank(&spent)
                    .await
                    .into_iter()
                    .find(|s| s.id == acct);
                match slot {
                    Some(slot) => Some((slot, session)),
                    None => Some((current, None)),
                }
            }
            _ => Some((current, session)),
        }
    }

    /// `id` hit its usage limit: move the run to the next best account.
    /// `None` when there is no other account left.
    pub async fn fail_over(&self, id: &str) -> Option<AccountSlot> {
        let spent = {
            let mut state = self.lock();
            if !state.spent.iter().any(|s| s == id) {
                state.spent.push(id.to_string());
            }
            state.spent.clone()
        };
        let next = self.ranker.rank(&spent).await.into_iter().next()?;
        let mut state = self.lock();
        if let Some(old) = state.current.replace(next.clone()) {
            self.ranker.release(&old.id);
        }
        self.ranker.acquire(&next.id);
        Some(next)
    }

    /// Remember which account a session was started on.
    pub fn note_session(&self, session: &str, account: &str) {
        self.lock()
            .sessions
            .insert(session.to_string(), account.to_string());
    }
}

impl Drop for ClaudeAccountPool {
    fn drop(&mut self) {
        if let Some(current) = self.lock().current.take() {
            self.ranker.release(&current.id);
        }
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    /// Ranks a fixed list, recording acquire/release.
    pub(crate) struct FixedRanker {
        pub accounts: Vec<AccountSlot>,
        pub held: Mutex<HashMap<String, i32>>,
        pub ranks: AtomicUsize,
    }

    impl FixedRanker {
        pub(crate) fn new(ids: &[&str]) -> Arc<Self> {
            Arc::new(Self {
                accounts: ids
                    .iter()
                    .map(|id| AccountSlot {
                        id: id.to_string(),
                        label: format!("Account {id}"),
                        config_dir: Some(PathBuf::from(format!("/accounts/{id}"))),
                    })
                    .collect(),
                held: Mutex::new(HashMap::new()),
                ranks: AtomicUsize::new(0),
            })
        }
        pub(crate) fn held(&self, id: &str) -> i32 {
            *self.held.lock().unwrap().get(id).unwrap_or(&0)
        }
    }

    #[async_trait]
    impl AccountRanker for FixedRanker {
        async fn rank(&self, spent: &[String]) -> Vec<AccountSlot> {
            self.ranks.fetch_add(1, Ordering::SeqCst);
            self.accounts
                .iter()
                .filter(|a| !spent.contains(&a.id))
                .cloned()
                .collect()
        }
        fn acquire(&self, id: &str) {
            *self.held.lock().unwrap().entry(id.into()).or_default() += 1;
        }
        fn release(&self, id: &str) {
            *self.held.lock().unwrap().entry(id.into()).or_default() -= 1;
        }
    }

    #[tokio::test]
    async fn a_run_stays_on_its_account_and_moves_when_it_is_spent() {
        let ranker = FixedRanker::new(&["a", "b"]);
        let pool = ClaudeAccountPool::new(ranker.clone());

        let (first, _) = pool.account_for(None).await.unwrap();
        assert_eq!(first.id, "a");
        let (again, _) = pool.account_for(None).await.unwrap();
        assert_eq!(again.id, "a", "pinned for the run");
        assert_eq!(ranker.held("a"), 1);

        let next = pool.fail_over("a").await.unwrap();
        assert_eq!(next.id, "b");
        assert_eq!((ranker.held("a"), ranker.held("b")), (0, 1));
        assert!(pool.fail_over("b").await.is_none(), "nothing left");

        drop(pool);
        assert_eq!(ranker.held("b"), 0, "released when the run ends");
    }

    #[tokio::test]
    async fn a_session_resumes_on_its_own_account_or_starts_fresh() {
        let ranker = FixedRanker::new(&["a", "b"]);
        let pool = ClaudeAccountPool::new(ranker.clone());
        let (a, _) = pool.account_for(None).await.unwrap();
        pool.note_session("s-on-a", &a.id);

        // Same account: the session is resumed.
        let (slot, session) = pool.account_for(Some("s-on-a")).await.unwrap();
        assert_eq!(
            (slot.id.as_str(), session.as_deref()),
            ("a", Some("s-on-a"))
        );

        // `a` is spent: its session can't follow the run to `b`.
        pool.fail_over("a").await.unwrap();
        let (slot, session) = pool.account_for(Some("s-on-a")).await.unwrap();
        assert_eq!((slot.id.as_str(), session), ("b", None));

        // A session nobody recorded runs where the run is.
        let (slot, session) = pool.account_for(Some("unknown")).await.unwrap();
        assert_eq!(
            (slot.id.as_str(), session.as_deref()),
            ("b", Some("unknown"))
        );
    }

    #[tokio::test]
    async fn no_accounts_means_no_pool_choice() {
        let pool = ClaudeAccountPool::new(FixedRanker::new(&[]));
        assert!(pool.account_for(None).await.is_none());
    }
}
