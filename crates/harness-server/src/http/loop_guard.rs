//! The loop guard: step in when a running step keeps hitting the same failure.
//!
//! Every tool result a node produces already streams through the server on its
//! way to the activity log. [`LoopGuard::observe`] watches them per (run, node)
//! and, once one failure (by [`harness_persist::error_fingerprint`]) has come
//! back [`REPEATS`] times, holds a redirect for that node: a short note saying
//! what keeps failing and to try another way.
//!
//! The agent picks it up through a hook at its next tool call, which asks
//! `GET /api/run/guard?node=<id>` with the run's grant: Claude Code through a
//! PostToolUse hook (`harness guard --format claude`), omp through the
//! harness-hooks extension. A node gets one redirect. If the same failure
//! comes back [`REPEATS`] more times after it, the guard writes a line into
//! the node's activity saying the step still looks stuck; it never stops a
//! step itself (the node's own timeout and iteration caps do that).
//!
//! State is in memory: a restart forgets it, which only costs a redirect.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use axum::extract::{Extension, Query};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::Json;
use harness_dag::{Activity, ActivityKind};
use serde::Deserialize;
use serde_json::json;

use super::runs_routes::RunsState;

/// Times one failure comes back in a step before the guard steps in, and
/// again after its redirect before it says the step still looks stuck.
pub(crate) const REPEATS: u32 = 3;

/// How much of the failing message the note quotes.
const SAMPLE_CHARS: usize = 400;

#[derive(Debug, Default)]
struct Watch {
    /// fingerprint → times seen in this step.
    counts: HashMap<String, u32>,
    /// The redirect waiting for the agent's next hook call.
    pending: Option<String>,
    /// The failure the redirect was about, once one was made.
    redirected: Option<String>,
    /// Times that failure came back after the redirect.
    after: u32,
    /// The still-stuck line was written.
    flagged: bool,
}

/// What [`LoopGuard::observe`] noticed.
#[derive(Debug, PartialEq)]
pub(crate) enum Noticed {
    /// A redirect is now waiting for the step.
    Redirect,
    /// Still the same failure after the redirect: the line to add to the
    /// step's activity.
    StillStuck(String),
}

#[derive(Debug, Default)]
pub(crate) struct LoopGuard {
    nodes: Mutex<HashMap<(String, String), Watch>>,
}

fn clip(s: &str, max: usize) -> String {
    let s = s.trim();
    if s.chars().count() <= max {
        s.to_string()
    } else {
        format!("{}…", s.chars().take(max).collect::<String>())
    }
}

/// The note a stuck step is given.
pub(crate) fn redirect_note(sample: &str, times: u32) -> String {
    format!(
        "Loop guard (from the harness): this step has now hit the same failure {times} times:\n\n\
         ```\n{}\n```\n\n\
         Repeating the same approach won't get past it. Stop and work out why it fails: read \
         the whole error, check how this repository does it (its scripts, README, AGENTS.md or \
         CLAUDE.md), and try a different approach. If it can't be fixed from here, say so in \
         your answer instead of retrying.",
        clip(sample, SAMPLE_CHARS)
    )
}

impl LoopGuard {
    /// Look at one activity line of a running step.
    pub(crate) fn observe(&self, run: &str, node: &str, activity: &Activity) -> Option<Noticed> {
        if activity.kind != ActivityKind::ToolResult || !activity.is_error {
            return None;
        }
        let message = activity
            .detail
            .as_deref()
            .filter(|d| !d.trim().is_empty())
            .unwrap_or(activity.text.as_str());
        if message.trim().is_empty() {
            return None;
        }
        let fp = harness_persist::error_fingerprint(message);
        let mut nodes = self.nodes.lock().ok()?;
        let w = nodes
            .entry((run.to_string(), node.to_string()))
            .or_default();
        let n = {
            let c = w.counts.entry(fp.clone()).or_default();
            *c += 1;
            *c
        };
        match &w.redirected {
            None if n >= REPEATS => {
                w.pending = Some(redirect_note(message, n));
                w.redirected = Some(fp);
                Some(Noticed::Redirect)
            }
            Some(r) if *r == fp && !w.flagged => {
                w.after += 1;
                if w.after >= REPEATS {
                    w.flagged = true;
                    Some(Noticed::StillStuck(format!(
                        "Loop guard: this step still hits the same failure after its redirect \
                         ({n} times in all). It is left to its own limits; consider stopping it."
                    )))
                } else {
                    None
                }
            }
            _ => None,
        }
    }

    /// The redirect waiting for a step, handed over once.
    pub(crate) fn take(&self, run: &str, node: &str) -> Option<String> {
        let mut nodes = self.nodes.lock().ok()?;
        nodes
            .get_mut(&(run.to_string(), node.to_string()))?
            .pending
            .take()
    }

    /// A step ended: its watch goes. (A loop node's iterations are one step.)
    pub(crate) fn forget_node(&self, run: &str, node: &str) {
        if let Ok(mut nodes) = self.nodes.lock() {
            nodes.remove(&(run.to_string(), node.to_string()));
        }
    }

    /// A run ended: every watch of it goes.
    pub(crate) fn forget_run(&self, run: &str) {
        if let Ok(mut nodes) = self.nodes.lock() {
            nodes.retain(|(r, _), _| r != run);
        }
    }
}

#[derive(Debug, Deserialize)]
pub struct GuardQuery {
    pub node: String,
}

/// `GET /api/run/guard?node=<id>` — the redirect waiting for a step of the
/// caller's run, if any: `{"note": "…"}` or `{}`. Authorized by the run's
/// grant, so a run can only ever read its own.
pub async fn get_guard(
    Extension(state): Extension<Arc<RunsState>>,
    headers: HeaderMap,
    Query(q): Query<GuardQuery>,
) -> Response {
    let grant = match super::run_linear_routes::grant(&headers) {
        Ok(g) => g,
        Err((s, m)) => return (s, Json(json!({ "error": m }))).into_response(),
    };
    match state.loop_guard.take(&grant.run_id, &q.node) {
        Some(note) => Json(json!({ "note": note })).into_response(),
        None => (StatusCode::OK, Json(json!({}))).into_response(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fail(msg: &str) -> Activity {
        Activity::tool_result(msg, None, true)
    }

    #[test]
    fn a_step_gets_one_redirect_after_the_same_failure_three_times() {
        let g = LoopGuard::default();
        let missing = |n: u32| fail(&format!("error: script \"test\" not found (took {n}ms)"));
        assert_eq!(g.observe("r", "build", &missing(10)), None);
        assert_eq!(g.observe("r", "build", &missing(20)), None);
        assert!(g.take("r", "build").is_none(), "nothing before the third");
        assert_eq!(
            g.observe("r", "build", &missing(30)),
            Some(Noticed::Redirect),
            "numbers don't make it a different failure"
        );
        let note = g.take("r", "build").expect("a redirect waits");
        assert!(note.contains("same failure 3 times") && note.contains("script \"test\""));
        assert!(g.take("r", "build").is_none(), "handed over once");
    }

    #[test]
    fn successes_other_steps_and_other_failures_dont_count() {
        let g = LoopGuard::default();
        for _ in 0..5 {
            assert_eq!(
                g.observe("r", "build", &Activity::tool_result("ok", None, false)),
                None
            );
            assert_eq!(g.observe("r", "build", &Activity::text("thinking")), None);
        }
        g.observe("r", "build", &fail("a: not found"));
        g.observe("r", "build", &fail("b: permission denied"));
        g.observe("r", "other", &fail("a: not found"));
        g.observe("r2", "build", &fail("a: not found"));
        assert!(g.take("r", "build").is_none());
    }

    #[test]
    fn still_failing_after_the_redirect_is_said_once() {
        let g = LoopGuard::default();
        for _ in 0..3 {
            g.observe("r", "fix", &fail("cargo: linker failed"));
        }
        g.take("r", "fix");
        assert_eq!(g.observe("r", "fix", &fail("cargo: linker failed")), None);
        assert_eq!(g.observe("r", "fix", &fail("cargo: linker failed")), None);
        assert!(matches!(
            g.observe("r", "fix", &fail("cargo: linker failed")),
            Some(Noticed::StillStuck(_))
        ));
        assert_eq!(g.observe("r", "fix", &fail("cargo: linker failed")), None);
        assert!(g.take("r", "fix").is_none(), "no second redirect");
    }

    #[test]
    fn a_finished_run_is_forgotten() {
        let g = LoopGuard::default();
        for _ in 0..3 {
            g.observe("r", "a", &fail("x failed"));
            g.observe("r", "b", &fail("x failed"));
        }
        g.forget_node("r", "a");
        assert!(g.take("r", "a").is_none());
        g.forget_run("r");
        assert!(g.take("r", "b").is_none());
    }
}
