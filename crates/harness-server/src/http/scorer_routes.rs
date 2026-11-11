//! Scorers: workflow nodes that grade the run they are in.
//!
//! A node marked `scorer: { name, pass_at }` outputs JSON `{ score,
//! classification?, summary? }`. When it finishes, the run's event forwarder
//! hands it to [`record_grade`], which stores the grade with its cost and run
//! time. The routes serve a scorer's grades across runs: `GET /api/scorers`
//! for the list, `GET /api/scorers/{name}` for one scorer's trend and runs.

use std::sync::Arc;

use axum::extract::{Extension, Path, Query};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::Json;
use harness_dag::{NodeRun, NodeStatus, ScorerConfig};
use harness_persist::ScorerResultInput;
use serde::Deserialize;
use serde_json::{json, Value};

use super::runs_routes::RunsState;

fn err(status: StatusCode, msg: impl Into<String>) -> Response {
    (status, Json(json!({ "error": msg.into() }))).into_response()
}

/// What a scorer node said about the run.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Grade {
    pub score: f64,
    pub classification: Option<String>,
    pub summary: Option<String>,
}

/// The grade in a scorer node's output: the JSON object it printed, or the
/// outermost `{ … }` in prose around it. `None` when there is no object with
/// a `score` between 0 and 1: a grade that can't be read is not recorded as
/// a zero.
pub(crate) fn parse_grade(output: &str) -> Option<Grade> {
    let t = output.trim();
    let value: Value = serde_json::from_str(t).ok().or_else(|| {
        let start = t.find('{')?;
        let end = t.rfind('}')?;
        (start < end)
            .then(|| serde_json::from_str(&t[start..=end]).ok())
            .flatten()
    })?;
    let score = value.get("score")?.as_f64()?;
    if !(0.0..=1.0).contains(&score) {
        return None;
    }
    let text = |k: &str| {
        value
            .get(k)
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(str::to_string)
    };
    Some(Grade {
        score,
        classification: text("classification"),
        summary: text("summary"),
    })
}

/// What one run of a node cost, priced from its tokens and model.
fn node_cost(node: &NodeRun) -> Option<f64> {
    let model = node.model.as_deref()?;
    let u = &node.usage;
    if u.input.is_none() && u.output.is_none() {
        return None;
    }
    let rates = harness_runner::models::family_for(model).rates;
    Some(rates.cost_usd(
        u.input.unwrap_or(0),
        u.output.unwrap_or(0),
        u.cache_read.unwrap_or(0),
        u.cache_write.unwrap_or(0),
    ))
}

/// The run this grade belongs to, for its row.
pub(crate) struct GradedRun<'a> {
    pub run_id: &'a str,
    pub workflow: &'a str,
    pub project: Option<&'a str>,
    pub title: Option<&'a str>,
}

/// The row to store for a finished scorer node, or `None` when there is
/// nothing to record: the node didn't succeed, or its grade can't be read.
pub(crate) fn grade_row(
    run: &GradedRun<'_>,
    scorer: &ScorerConfig,
    node: &NodeRun,
    duration_ms: Option<i64>,
) -> Option<ScorerResultInput> {
    if node.status != NodeStatus::Success {
        return None;
    }
    let grade = parse_grade(&node.output)?;
    let duration_ms = duration_ms.or_else(|| {
        let (s, e) = (node.started_at?, node.ended_at?);
        Some((e - s).num_milliseconds())
    });
    Some(ScorerResultInput {
        run_id: run.run_id.to_string(),
        node_id: node.id.clone(),
        scorer: scorer.name.clone(),
        description: scorer.description.clone(),
        workflow: run.workflow.to_string(),
        project: run.project.map(str::to_string),
        run_title: run.title.map(str::to_string),
        score: grade.score,
        passed: grade.score >= scorer.pass_at,
        classification: grade.classification,
        summary: grade.summary,
        duration_ms,
        cost_usd: node_cost(node),
    })
}

/// Record a finished scorer node's grade. Best-effort: a scorer that can't
/// be read, or a store that is down, never touches the run itself.
pub(crate) async fn record_grade(
    state: &Arc<RunsState>,
    run: &GradedRun<'_>,
    scorer: &ScorerConfig,
    node: &NodeRun,
    duration_ms: Option<i64>,
) {
    let Some(row) = grade_row(run, scorer, node, duration_ms) else {
        if node.status == NodeStatus::Success {
            tracing::warn!(
                run_id = run.run_id,
                node = %node.id,
                scorer = %scorer.name,
                "scorer output has no readable grade (JSON with a `score` between 0 and 1); not recorded"
            );
        }
        return;
    };
    match state.scorer_store().await {
        Ok(store) => {
            if let Err(e) = store.record(&row).await {
                tracing::warn!(run_id = run.run_id, scorer = %scorer.name, "recording a scorer grade failed: {e}");
            }
        }
        Err(e) => tracing::warn!(scorer = %scorer.name, "no scorer store: {e}"),
    }
}

/// `GET /api/scorers` — every scorer that has graded something.
pub async fn list_scorers(Extension(state): Extension<Arc<RunsState>>) -> Response {
    let store = match state.scorer_store().await {
        Ok(s) => s,
        Err(e) => return err(StatusCode::SERVICE_UNAVAILABLE, e),
    };
    match store.summaries().await {
        Ok(rows) => Json(json!({ "scorers": rows })).into_response(),
        Err(e) => err(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()),
    }
}

#[derive(Debug, Deserialize)]
pub struct ScorerQuery {
    /// Days of history for the trend; default 30, at most 365.
    pub days: Option<i32>,
    /// How many recent runs to list; default 50, at most 500.
    pub limit: Option<i64>,
}

/// `GET /api/scorers/{name}` — one scorer: its summary, a grade per day over
/// `days`, and its latest runs.
pub async fn get_scorer(
    Extension(state): Extension<Arc<RunsState>>,
    Path(name): Path<String>,
    Query(q): Query<ScorerQuery>,
) -> Response {
    let days = q.days.unwrap_or(30).clamp(1, 365);
    let limit = q.limit.unwrap_or(50).clamp(1, 500);
    let store = match state.scorer_store().await {
        Ok(s) => s,
        Err(e) => return err(StatusCode::SERVICE_UNAVAILABLE, e),
    };
    let summary = match store.summary(&name).await {
        Ok(Some(s)) => s,
        Ok(None) => {
            return err(
                StatusCode::NOT_FOUND,
                format!("no grades from scorer `{name}` yet"),
            )
        }
        Err(e) => return err(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()),
    };
    let (daily, recent) = match (
        store.daily(&name, days).await,
        store.recent(&name, limit).await,
    ) {
        (Ok(d), Ok(r)) => (d, r),
        (Err(e), _) | (_, Err(e)) => return err(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()),
    };
    Json(json!({ "summary": summary, "days": days, "daily": daily, "runs": recent }))
        .into_response()
}

#[cfg(test)]
mod tests {
    use super::*;
    use harness_dag::Usage;

    #[test]
    fn a_grade_is_read_from_json_or_from_prose_around_it() {
        let g = parse_grade(
            r#"{"score": 0.5, "classification": "Insufficient evidence", "summary": " "}"#,
        )
        .unwrap();
        assert_eq!(g.score, 0.5);
        assert_eq!(g.classification.as_deref(), Some("Insufficient evidence"));
        assert_eq!(g.summary, None, "blank text is no summary");

        let wrapped = "Here is the verdict:\n```json\n{\"score\": 1, \"classification\": \"No surplus tests\"}\n```";
        assert_eq!(parse_grade(wrapped).unwrap().score, 1.0);
    }

    #[test]
    fn an_unreadable_grade_is_not_a_zero() {
        assert_eq!(parse_grade("I could not decide."), None);
        assert_eq!(
            parse_grade(r#"{"classification": "No surplus tests"}"#),
            None
        );
        assert_eq!(parse_grade(r#"{"score": 85}"#), None, "out of range");
        assert_eq!(parse_grade(r#"{"score": "high"}"#), None);
    }

    fn scorer() -> ScorerConfig {
        ScorerConfig {
            name: "redundant-tests".into(),
            pass_at: 0.5,
            description: Some("Surplus tests in the PR".into()),
        }
    }

    fn node(status: NodeStatus, output: &str) -> NodeRun {
        NodeRun {
            id: "score-tests".into(),
            status,
            provider: Some("claude".into()),
            model: Some("sonnet".into()),
            output: output.into(),
            usage: Usage {
                input: Some(10_000),
                output: Some(1_000),
                cache_read: None,
                cache_write: None,
            },
            iterations: 1,
            converged: None,
            note: None,
            artifact_content: None,
            started_at: None,
            ended_at: None,
        }
    }

    #[test]
    fn a_finished_scorer_becomes_a_row_with_its_pass_and_cost() {
        let run = GradedRun {
            run_id: "r1",
            workflow: "ecom-task",
            project: Some("dilling-ecom"),
            title: Some("ECOM-1 Fix the cart"),
        };
        let row = grade_row(
            &run,
            &scorer(),
            &node(
                NodeStatus::Success,
                r#"{"score": 0.5, "classification": "Some surplus"}"#,
            ),
            Some(42_000),
        )
        .unwrap();
        assert!(row.passed, "at pass_at passes");
        assert_eq!(row.scorer, "redundant-tests");
        assert_eq!(row.workflow, "ecom-task");
        assert_eq!(row.duration_ms, Some(42_000));
        assert!(row.cost_usd.unwrap() > 0.0, "priced from its tokens");

        let failing = grade_row(
            &run,
            &scorer(),
            &node(NodeStatus::Success, r#"{"score": 0.49}"#),
            None,
        );
        assert!(!failing.unwrap().passed);

        // A step that failed, or said nothing readable, records nothing.
        assert!(grade_row(
            &run,
            &scorer(),
            &node(NodeStatus::Failed, r#"{"score": 1}"#),
            None
        )
        .is_none());
        assert!(grade_row(&run, &scorer(), &node(NodeStatus::Success, "no idea"), None).is_none());
    }
}
