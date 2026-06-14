use std::collections::{HashMap, VecDeque};
use std::sync::Mutex;
use std::time::{Duration, Instant};

/// Read-only snapshot of [`PasswordResetRateLimiter`] state.
pub struct PasswordResetLimiterSnapshot {
    pub tracked_identifiers: usize,
    pub limit_per_hour: usize,
}

/// Per-identifier rate limiter for `POST /auth/reset-password`.
///
/// Uses a 1-hour rolling window per email address to prevent brute-force
/// and enumeration attacks on the password reset flow.
///
/// Memory is bounded: at most `max_tracked_keys` identifiers are tracked at
/// once; expired timestamps are evicted lazily on each access.
pub struct PasswordResetRateLimiter {
    timestamps: Mutex<HashMap<String, VecDeque<Instant>>>,
    max_per_hour: usize,
    max_tracked_keys: usize,
}

impl PasswordResetRateLimiter {
    const WINDOW: Duration = Duration::from_secs(3600);

    fn prune_expired_timestamps(map: &mut HashMap<String, VecDeque<Instant>>, now: Instant) {
        map.retain(|_, timestamps| {
            while let Some(&front) = timestamps.front() {
                if now.duration_since(front) >= Self::WINDOW {
                    timestamps.pop_front();
                } else {
                    break;
                }
            }
            !timestamps.is_empty()
        });
    }

    fn prune_identifier_entry(
        map: &mut HashMap<String, VecDeque<Instant>>,
        identifier: &str,
        now: Instant,
    ) {
        if let Some(timestamps) = map.get_mut(identifier) {
            while let Some(&front) = timestamps.front() {
                if now.duration_since(front) >= Self::WINDOW {
                    timestamps.pop_front();
                } else {
                    break;
                }
            }
            if timestamps.is_empty() {
                map.remove(identifier);
            }
        }
    }

    fn active_identifier_count(map: &HashMap<String, VecDeque<Instant>>, now: Instant) -> usize {
        map.values()
            .filter(|timestamps| {
                timestamps
                    .back()
                    .is_some_and(|last| now.duration_since(*last) < Self::WINDOW)
            })
            .count()
    }

    #[cfg(test)]
    fn insert_for_test(&self, identifier: &str, timestamps: VecDeque<Instant>) {
        let mut map = self.timestamps.lock().unwrap_or_else(|p| p.into_inner());
        map.insert(identifier.to_string(), timestamps);
    }

    #[cfg(test)]
    fn window() -> Duration {
        Self::WINDOW
    }

    #[cfg(test)]
    fn len_for_test(&self) -> usize {
        self.timestamps
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .len()
    }

    #[cfg(test)]
    fn contains_for_test(&self, identifier: &str) -> bool {
        self.timestamps
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .contains_key(identifier)
    }

    pub fn new(max_per_hour: u32) -> Self {
        Self {
            timestamps: Mutex::new(HashMap::new()),
            max_per_hour: max_per_hour as usize,
            max_tracked_keys: 100_000,
        }
    }

    #[cfg(test)]
    fn new_with_cap(max_per_hour: u32, max_tracked_keys: usize) -> Self {
        Self {
            timestamps: Mutex::new(HashMap::new()),
            max_per_hour: max_per_hour as usize,
            max_tracked_keys,
        }
    }

    /// Return a read-only snapshot of the current limiter state.
    pub fn snapshot(&self) -> PasswordResetLimiterSnapshot {
        let map = self.timestamps.lock().unwrap_or_else(|p| p.into_inner());
        PasswordResetLimiterSnapshot {
            tracked_identifiers: Self::active_identifier_count(&map, Instant::now()),
            limit_per_hour: self.max_per_hour,
        }
    }

    /// Returns `true` if the request is within the rate limit and increments the counter.
    pub fn check_and_increment(&self, identifier: &str) -> bool {
        let mut map = self.timestamps.lock().unwrap_or_else(|p| p.into_inner());
        let now = Instant::now();

        Self::prune_identifier_entry(&mut map, identifier, now);

        // Reject new identifiers when map is at capacity (memory-DoS guard).
        if !map.contains_key(identifier) && map.len() >= self.max_tracked_keys {
            Self::prune_expired_timestamps(&mut map, now);
        }

        if !map.contains_key(identifier) && map.len() >= self.max_tracked_keys {
            return false;
        }

        let entry = map.entry(identifier.to_string()).or_default();
        if entry.len() < self.max_per_hour {
            entry.push_back(now);
            true
        } else {
            false
        }
    }
}

#[cfg(test)]
mod tests {
    use super::PasswordResetRateLimiter;
    use std::collections::VecDeque;
    use std::time::{Duration, Instant};

    #[test]
    fn allows_requests_within_limit() {
        let limiter = PasswordResetRateLimiter::new(3);
        assert!(limiter.check_and_increment("user@example.com"));
        assert!(limiter.check_and_increment("user@example.com"));
        assert!(limiter.check_and_increment("user@example.com"));
    }

    #[test]
    fn blocks_after_limit_exceeded() {
        let limiter = PasswordResetRateLimiter::new(2);
        assert!(limiter.check_and_increment("a@example.com"));
        assert!(limiter.check_and_increment("a@example.com"));
        assert!(!limiter.check_and_increment("a@example.com"));
    }

    #[test]
    fn limits_are_per_identifier() {
        let limiter = PasswordResetRateLimiter::new(1);
        assert!(limiter.check_and_increment("alice@example.com"));
        assert!(!limiter.check_and_increment("alice@example.com"));
        assert!(limiter.check_and_increment("bob@example.com"));
    }

    #[test]
    fn rejects_new_identifiers_when_key_cap_reached() {
        let limiter = PasswordResetRateLimiter::new_with_cap(10, 2);
        assert!(limiter.check_and_increment("a@example.com"));
        assert!(limiter.check_and_increment("b@example.com"));
        assert!(!limiter.check_and_increment("c@example.com"));
        assert!(limiter.check_and_increment("a@example.com"));
    }

    #[test]
    fn password_reset_snapshot_excludes_expired_identifiers() {
        let limiter = PasswordResetRateLimiter::new(5);
        let now = Instant::now();
        limiter.insert_for_test(
            "active@example.com",
            VecDeque::from([now - Duration::from_secs(30)]),
        );
        limiter.insert_for_test(
            "expired@example.com",
            VecDeque::from([now - (PasswordResetRateLimiter::window() + Duration::from_secs(1))]),
        );

        let snap = limiter.snapshot();

        assert_eq!(snap.tracked_identifiers, 1);
    }

    #[test]
    fn password_reset_prunes_expired_identifiers_before_capacity_check() {
        let limiter = PasswordResetRateLimiter::new_with_cap(10, 1);
        let expired =
            Instant::now() - (PasswordResetRateLimiter::window() + Duration::from_secs(1));
        limiter.insert_for_test("expired@example.com", VecDeque::from([expired]));

        assert!(limiter.check_and_increment("fresh@example.com"));
        assert_eq!(limiter.len_for_test(), 1);
        assert!(limiter.contains_for_test("fresh@example.com"));
    }

    #[test]
    fn password_reset_hot_path_only_prunes_requested_identifier() {
        let limiter = PasswordResetRateLimiter::new_with_cap(10, 10);
        let now = Instant::now();
        let expired = now - (PasswordResetRateLimiter::window() + Duration::from_secs(1));

        limiter.insert_for_test(
            "active@example.com",
            VecDeque::from([now - Duration::from_secs(5)]),
        );
        limiter.insert_for_test("expired@example.com", VecDeque::from([expired]));

        assert!(limiter.check_and_increment("active@example.com"));
        assert_eq!(limiter.len_for_test(), 2);
        assert!(limiter.contains_for_test("expired@example.com"));
    }
}
