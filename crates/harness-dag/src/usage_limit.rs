//! Recognise a subscription usage limit in a failed step, and when it resets.
//!
//! An agent that hits its plan's limit (Claude's 5-hour window, a gateway's
//! 429 cooldown) fails the step, but nothing is wrong with the step: run again
//! after the reset and it would succeed. The driver uses [`reset_time`] to tell
//! that failure apart from a real one, so it can wait for the reset instead of
//! failing the run (see `execute_body` in [`crate::exec`]).
//!
//! Failures reach the driver as text (a runner error or a failed output), so
//! this reads text. It returns a time only when the text both says a limit was
//! hit *and* says when it resets; a limit with no reset time stays a failure.

use std::sync::LazyLock;

use chrono::{DateTime, Duration, TimeZone, Utc};
use regex::Regex;

/// The phrase an agent adapter appends to its error when the agent reported
/// the reset time out of band, followed by unix seconds. Claude Code sends it
/// as a `rate_limit_event` stream line that never reaches the error text, so
/// `harness-agents` (which cannot depend on this crate) writes this phrase.
/// `harness_agents::claude_stream` builds it from its own copy, and a test
/// there fails if the two drift apart.
pub const RESETS_AT_UNIX: &str = "usage limit reached, resets at unix ";

/// Our own phrase: `usage limit reached, resets at unix 1762700000`.
static MARKER: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(&format!("{}(\\d{{9,13}})", regex::escape(RESETS_AT_UNIX))).unwrap()
});

/// Claude Code's older result text: `Claude AI usage limit reached|1762700000`.
static CLAUDE_PIPE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?i)usage limit reached\|(\d{9,13})").unwrap());

/// A gateway's cooldown in its 429 text: `(reset after 2h13m5s)`. API-key
/// sessions get no structured reset time, so this text is the only source.
static RESET_AFTER: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?i)reset after (?:(\d+)h ?)?(?:(\d+)m ?)?(?:(\d+)s)?").unwrap());

/// When the usage limit described in `text` resets, or `None` when `text` is
/// not a usage-limit failure or gives no reset time. `now` anchors relative
/// times ("reset after 2h").
pub fn reset_time(text: &str, now: DateTime<Utc>) -> Option<DateTime<Utc>> {
    if let Some(c) = MARKER.captures(text).or_else(|| CLAUDE_PIPE.captures(text)) {
        return from_unix(c[1].parse().ok()?);
    }
    if mentions_rate_limit(text) {
        if let Some(c) = RESET_AFTER.captures(text) {
            let part =
                |i: usize| -> i64 { c.get(i).and_then(|m| m.as_str().parse().ok()).unwrap_or(0) };
            let wait =
                Duration::hours(part(1)) + Duration::minutes(part(2)) + Duration::seconds(part(3));
            if wait > Duration::zero() {
                return Some(now + wait);
            }
        }
    }
    None
}

/// A "reset after" alone could be anything; only trust it next to a limit.
fn mentions_rate_limit(text: &str) -> bool {
    let lower = text.to_lowercase();
    ["429", "rate_limit", "rate limit", "usage limit"]
        .iter()
        .any(|needle| lower.contains(needle))
}

/// Unix seconds, or milliseconds when the number is too large to be seconds.
fn from_unix(n: i64) -> Option<DateTime<Utc>> {
    if n > 100_000_000_000 {
        Utc.timestamp_millis_opt(n).single()
    } else {
        Utc.timestamp_opt(n, 0).single()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn now() -> DateTime<Utc> {
        Utc.with_ymd_and_hms(2026, 11, 9, 12, 0, 0).unwrap()
    }

    #[test]
    fn reads_the_phrase_the_agent_adapter_appends() {
        let text = format!(
            "claude exited with exit status: 1: stdout_tail=[You've hit your limit] ({}1762700000)",
            RESETS_AT_UNIX
        );
        assert_eq!(
            reset_time(&text, now()),
            Some(Utc.timestamp_opt(1_762_700_000, 0).unwrap())
        );
    }

    #[test]
    fn reads_milliseconds_as_milliseconds() {
        let text = format!("{}1762700000000", RESETS_AT_UNIX);
        assert_eq!(
            reset_time(&text, now()),
            Some(Utc.timestamp_opt(1_762_700_000, 0).unwrap())
        );
    }

    #[test]
    fn reads_claude_codes_older_pipe_format() {
        let text = "claude exited with exit status: 1: stdout_tail=[Claude AI usage limit reached|1762700000]";
        assert_eq!(
            reset_time(text, now()),
            Some(Utc.timestamp_opt(1_762_700_000, 0).unwrap())
        );
    }

    #[test]
    fn reads_a_429_result_with_a_reset_after_text() {
        let text = r#"API Error: 429 {"type":"error","error":{"type":"rate_limit_error","message":"slow down"}} (reset after 1h2m3s)"#;
        assert_eq!(
            reset_time(text, now()),
            Some(now() + Duration::seconds(3600 + 120 + 3))
        );
    }

    #[test]
    fn reads_a_gateway_503_wrapping_a_429_cooldown() {
        let text = r#"API Error: 503 [gw] [429]: {"type":"error","error":{"type":"rate_limit_error"}} (reset after 45m)"#;
        assert_eq!(reset_time(text, now()), Some(now() + Duration::minutes(45)));
    }

    #[test]
    fn a_limit_without_a_reset_time_is_none() {
        assert_eq!(reset_time("rate limit exceeded", now()), None);
        assert_eq!(reset_time("You've hit your limit", now()), None);
    }

    #[test]
    fn reset_after_without_a_limit_is_none() {
        // A test failure that happens to print a retry hint is not a limit.
        assert_eq!(reset_time("cache: reset after 5m of idle", now()), None);
    }

    #[test]
    fn ordinary_failures_are_none() {
        assert_eq!(reset_time("error[E0308]: mismatched types", now()), None);
        assert_eq!(reset_time("", now()), None);
    }
}
