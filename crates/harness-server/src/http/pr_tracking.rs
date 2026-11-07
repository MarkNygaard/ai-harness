//! Following the pull requests runs open, for the dashboard.
//!
//! - [`record_run`] runs when any run ends. When its output links a PR, the PR
//!   is recorded against the run and checked once straight away, so it has
//!   its opened time before the next poll.
//! - [`spawn_poller`] asks GitHub about the open ones every few minutes until
//!   each has merged or closed.
//!
//! The harness reads GitHub's REST API here. That isn't git work, which stays
//! in agent prompts; it's the same read the PR status MCP tool does.

use std::sync::Arc;
use std::time::Duration;

use chrono::{DateTime, Utc};
use harness_persist::{RunDetail, RunPr};
use harness_sources::github::{parse_pr_url, GithubClient, PullFields};

use super::runs_routes::RunsState;

/// How often the open PRs are checked.
const POLL_EVERY: Duration = Duration::from_secs(10 * 60);
/// Open PRs checked per pass, least recently checked first. Well inside
/// GitHub's hourly limit at this rate, and every PR comes round in time.
const PER_PASS: i64 = 30;

/// Record the PR a finished run links to, if any, and read it once.
pub(crate) async fn record_run(state: &Arc<RunsState>, detail: &RunDetail) {
    let Some(url) = super::runs_routes::extract_pr_url(detail) else {
        return;
    };
    if parse_pr_url(&url).is_none() {
        return;
    }
    let Ok(store) = state.pr_store().await else {
        return;
    };
    let run = &detail.run;
    let kickoff = run.started_at.unwrap_or(run.recorded_at);
    match store
        .record(
            &url,
            &run.id,
            run.project.as_deref(),
            &run.workflow_name,
            kickoff,
        )
        .await
    {
        Ok(true) => {
            let pr = RunPr {
                url,
                run_id: run.id.clone(),
                project: run.project.clone(),
                workflow: run.workflow_name.clone(),
                run_started_at: kickoff,
                state: harness_persist::PR_OPEN.to_string(),
                opened_at: None,
                merged_at: None,
                checked_at: None,
            };
            check(state, &pr).await;
        }
        Ok(false) => {}
        Err(e) => tracing::warn!("pr tracking: could not record {url}: {e}"),
    }
}

/// Check the open PRs every [`POLL_EVERY`]. No database: nothing to do.
pub(crate) fn spawn_poller(state: Arc<RunsState>) {
    tokio::spawn(async move {
        let mut tick = tokio::time::interval(POLL_EVERY);
        loop {
            tick.tick().await;
            let Ok(store) = state.pr_store().await else {
                continue;
            };
            let open = match store.open_prs(PER_PASS).await {
                Ok(open) => open,
                Err(e) => {
                    tracing::debug!("pr tracking: could not list open PRs: {e}");
                    continue;
                }
            };
            for pr in &open {
                check(&state, pr).await;
            }
        }
    });
}

/// Ask GitHub about one PR and store the answer.
async fn check(state: &Arc<RunsState>, pr: &RunPr) {
    let Ok(store) = state.pr_store().await else {
        return;
    };
    let Some(r) = parse_pr_url(&pr.url) else {
        let _ = store.touch(&pr.url).await;
        return;
    };
    let token = match pr.project.as_deref() {
        Some(p) => state.github_token_for_project(p).await,
        None => state.github_token().await,
    };
    match GithubClient::new(token).pull(&r).await {
        Ok(fields) => {
            let (status, opened, merged) = outcome(&fields);
            if let Err(e) = store.update(&pr.url, status, opened, merged).await {
                tracing::warn!("pr tracking: could not update {}: {e}", pr.url);
            }
        }
        Err(e) => {
            tracing::debug!("pr tracking: {}: {e}", pr.url);
            let _ = store.touch(&pr.url).await;
        }
    }
}

/// What a PR's GitHub fields mean for tracking: its state, opened and merged.
fn outcome(f: &PullFields) -> (&'static str, Option<DateTime<Utc>>, Option<DateTime<Utc>>) {
    let at = |s: &Option<String>| {
        s.as_deref()
            .and_then(|s| DateTime::parse_from_rfc3339(s).ok())
            .map(|d| d.with_timezone(&Utc))
    };
    let state = match f.state.as_str() {
        "merged" => harness_persist::PR_MERGED,
        "closed" => harness_persist::PR_CLOSED,
        _ => harness_persist::PR_OPEN,
    };
    (state, at(&f.created_at), at(&f.merged_at))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fields(state: &str, created: Option<&str>, merged: Option<&str>) -> PullFields {
        PullFields {
            url: String::new(),
            title: String::new(),
            state: state.into(),
            draft: false,
            head_branch: String::new(),
            head_sha: String::new(),
            created_at: created.map(str::to_string),
            updated_at: None,
            merged_at: merged.map(str::to_string),
        }
    }

    #[test]
    fn a_merged_pr_carries_both_times() {
        let (state, opened, merged) = outcome(&fields(
            "merged",
            Some("2026-11-01T09:00:00Z"),
            Some("2026-11-02T10:30:00Z"),
        ));
        assert_eq!(state, harness_persist::PR_MERGED);
        assert_eq!(opened.unwrap().to_rfc3339(), "2026-11-01T09:00:00+00:00");
        assert_eq!(merged.unwrap().to_rfc3339(), "2026-11-02T10:30:00+00:00");
    }

    #[test]
    fn open_and_closed_are_told_apart_and_bad_times_are_dropped() {
        assert_eq!(
            outcome(&fields("open", None, None)).0,
            harness_persist::PR_OPEN
        );
        assert_eq!(
            outcome(&fields("closed", None, None)).0,
            harness_persist::PR_CLOSED
        );
        let (_, opened, _) = outcome(&fields("open", Some("yesterday"), None));
        assert!(opened.is_none());
    }
}
