//! Grades recorded by scorer nodes.
//!
//! A *scorer* is a workflow node marked `scorer: { name, pass_at }` that
//! grades the run it is in (e.g. "does this PR add redundant tests?"). Each
//! time one finishes, the server records its grade here, keyed by run and
//! node, so a scorer's grades can be followed across every run and workflow
//! that uses it: pass rate and cost over time, and the runs behind them.

use chrono::{DateTime, NaiveDate, Utc};
use serde::Serialize;
use sqlx::PgPool;

use crate::PersistError;

const CREATE_SCORER_RESULTS: &str = "
CREATE TABLE IF NOT EXISTS harness_scorer_results (
    run_id         text NOT NULL,
    node_id        text NOT NULL,
    scorer         text NOT NULL,
    description    text,
    workflow       text NOT NULL,
    project        text,
    run_title      text,
    score          double precision NOT NULL,
    passed         boolean NOT NULL,
    classification text,
    summary        text,
    duration_ms    bigint,
    cost_usd       double precision,
    created_at     timestamptz NOT NULL DEFAULT now(),
    PRIMARY KEY (run_id, node_id)
)";

const CREATE_SCORER_INDEX: &str = "
CREATE INDEX IF NOT EXISTS harness_scorer_results_scorer_time
    ON harness_scorer_results (scorer, created_at DESC)";

/// One grade, as a scorer node gave it.
#[derive(Debug, Clone, Serialize, sqlx::FromRow)]
pub struct ScorerResult {
    pub run_id: String,
    pub node_id: String,
    pub scorer: String,
    pub description: Option<String>,
    pub workflow: String,
    pub project: Option<String>,
    pub run_title: Option<String>,
    pub score: f64,
    pub passed: bool,
    pub classification: Option<String>,
    pub summary: Option<String>,
    pub duration_ms: Option<i64>,
    pub cost_usd: Option<f64>,
    pub created_at: DateTime<Utc>,
}

/// What a finished scorer node reports.
#[derive(Debug, Clone, Default)]
pub struct ScorerResultInput {
    pub run_id: String,
    pub node_id: String,
    pub scorer: String,
    pub description: Option<String>,
    pub workflow: String,
    pub project: Option<String>,
    pub run_title: Option<String>,
    pub score: f64,
    pub passed: bool,
    pub classification: Option<String>,
    pub summary: Option<String>,
    pub duration_ms: Option<i64>,
    pub cost_usd: Option<f64>,
}

/// A scorer at a glance: how often it ran, how often it passed, what it cost.
#[derive(Debug, Clone, Serialize, sqlx::FromRow)]
pub struct ScorerSummary {
    pub scorer: String,
    /// From its most recent grade.
    pub description: Option<String>,
    pub runs: i64,
    pub passed: i64,
    pub total_cost_usd: f64,
    pub last_at: DateTime<Utc>,
    /// The workflows that run it.
    pub workflows: Vec<String>,
}

/// One day of a scorer's grades.
#[derive(Debug, Clone, Serialize, sqlx::FromRow)]
pub struct ScorerDay {
    pub day: NaiveDate,
    pub runs: i64,
    pub passed: i64,
    pub cost_usd: f64,
}

/// Postgres-backed store for scorer grades.
pub struct ScorerResultStore {
    pool: PgPool,
}

impl ScorerResultStore {
    /// Connect to `database_url` and ensure the schema exists.
    pub async fn connect(database_url: &str) -> Result<Self, PersistError> {
        let pool = sqlx::postgres::PgPoolOptions::new()
            .max_connections(3)
            .connect(database_url)
            .await?;
        Self::from_pool(pool).await
    }

    /// Wrap an existing pool; ensures the table exists.
    pub async fn from_pool(pool: PgPool) -> Result<Self, PersistError> {
        sqlx::query(CREATE_SCORER_RESULTS).execute(&pool).await?;
        sqlx::query(CREATE_SCORER_INDEX).execute(&pool).await?;
        Ok(Self { pool })
    }

    /// Record a grade. A node that runs again in the same run (a retry)
    /// replaces its earlier grade.
    pub async fn record(&self, r: &ScorerResultInput) -> Result<(), PersistError> {
        sqlx::query(
            "INSERT INTO harness_scorer_results (
                run_id, node_id, scorer, description, workflow, project, run_title,
                score, passed, classification, summary, duration_ms, cost_usd
            ) VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13)
            ON CONFLICT (run_id, node_id) DO UPDATE SET
                scorer = excluded.scorer,
                description = excluded.description,
                workflow = excluded.workflow,
                project = excluded.project,
                run_title = excluded.run_title,
                score = excluded.score,
                passed = excluded.passed,
                classification = excluded.classification,
                summary = excluded.summary,
                duration_ms = excluded.duration_ms,
                cost_usd = excluded.cost_usd,
                created_at = now()",
        )
        .bind(&r.run_id)
        .bind(&r.node_id)
        .bind(&r.scorer)
        .bind(r.description.as_deref())
        .bind(&r.workflow)
        .bind(r.project.as_deref())
        .bind(r.run_title.as_deref())
        .bind(r.score)
        .bind(r.passed)
        .bind(r.classification.as_deref())
        .bind(r.summary.as_deref())
        .bind(r.duration_ms)
        .bind(r.cost_usd)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    /// Every scorer that has graded something, most recently active first.
    pub async fn summaries(&self) -> Result<Vec<ScorerSummary>, PersistError> {
        let rows = sqlx::query_as::<_, ScorerSummary>(
            "SELECT scorer,
                    (ARRAY_AGG(description ORDER BY created_at DESC))[1] AS description,
                    COUNT(*) AS runs,
                    COUNT(*) FILTER (WHERE passed) AS passed,
                    COALESCE(SUM(cost_usd), 0) AS total_cost_usd,
                    MAX(created_at) AS last_at,
                    ARRAY_AGG(DISTINCT workflow ORDER BY workflow) AS workflows
             FROM harness_scorer_results
             GROUP BY scorer
             ORDER BY MAX(created_at) DESC",
        )
        .fetch_all(&self.pool)
        .await?;
        Ok(rows)
    }

    /// One scorer's summary, or `None` when it has graded nothing.
    pub async fn summary(&self, scorer: &str) -> Result<Option<ScorerSummary>, PersistError> {
        Ok(self
            .summaries()
            .await?
            .into_iter()
            .find(|s| s.scorer == scorer))
    }

    /// A scorer's grades per day over the last `days` days (UTC), oldest
    /// first. Days without a grade are absent.
    pub async fn daily(&self, scorer: &str, days: i32) -> Result<Vec<ScorerDay>, PersistError> {
        let rows = sqlx::query_as::<_, ScorerDay>(
            "SELECT (created_at AT TIME ZONE 'UTC')::date AS day,
                    COUNT(*) AS runs,
                    COUNT(*) FILTER (WHERE passed) AS passed,
                    COALESCE(SUM(cost_usd), 0) AS cost_usd
             FROM harness_scorer_results
             WHERE scorer = $1 AND created_at >= now() - make_interval(days => $2)
             GROUP BY 1
             ORDER BY 1",
        )
        .bind(scorer)
        .bind(days)
        .fetch_all(&self.pool)
        .await?;
        Ok(rows)
    }

    /// A scorer's most recent grades, newest first.
    pub async fn recent(
        &self,
        scorer: &str,
        limit: i64,
    ) -> Result<Vec<ScorerResult>, PersistError> {
        let rows = sqlx::query_as::<_, ScorerResult>(
            "SELECT run_id, node_id, scorer, description, workflow, project, run_title,
                    score, passed, classification, summary, duration_ms, cost_usd, created_at
             FROM harness_scorer_results
             WHERE scorer = $1
             ORDER BY created_at DESC
             LIMIT $2",
        )
        .bind(scorer)
        .bind(limit)
        .fetch_all(&self.pool)
        .await?;
        Ok(rows)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn db_url() -> Option<String> {
        let url = std::env::var("HARNESS_DATABASE_URL").ok()?;
        crate::is_test_db(&url).then_some(url)
    }

    #[tokio::test]
    #[serial_test::serial]
    async fn grades_add_up_per_scorer_and_per_day() {
        let Some(url) = db_url() else {
            eprintln!("skipping: HARNESS_DATABASE_URL not set");
            return;
        };
        let store = ScorerResultStore::connect(&url).await.expect("connect");
        let scorer = format!(
            "test-scorer-{}",
            chrono::Utc::now().timestamp_nanos_opt().unwrap()
        );
        let grade = |run: &str, workflow: &str, score: f64, cost: f64| ScorerResultInput {
            run_id: run.into(),
            node_id: "score-tests".into(),
            scorer: scorer.clone(),
            description: Some("Surplus tests".into()),
            workflow: workflow.into(),
            score,
            passed: score >= 0.5,
            classification: Some(
                if score >= 0.5 {
                    "No surplus tests"
                } else {
                    "Some surplus"
                }
                .into(),
            ),
            cost_usd: Some(cost),
            duration_ms: Some(1000),
            ..Default::default()
        };
        store
            .record(&grade(&format!("{scorer}-a"), "ecom-task", 1.0, 0.25))
            .await
            .unwrap();
        store
            .record(&grade(&format!("{scorer}-b"), "revise-task", 0.2, 0.5))
            .await
            .unwrap();
        // A retry of the same node replaces its grade rather than adding one.
        store
            .record(&grade(&format!("{scorer}-b"), "revise-task", 0.8, 0.5))
            .await
            .unwrap();

        let s = store.summary(&scorer).await.unwrap().expect("summary");
        assert_eq!(s.runs, 2);
        assert_eq!(s.passed, 2);
        assert!((s.total_cost_usd - 0.75).abs() < 1e-9);
        assert_eq!(
            s.workflows,
            vec!["ecom-task".to_string(), "revise-task".to_string()]
        );

        let days = store.daily(&scorer, 30).await.unwrap();
        assert_eq!(days.iter().map(|d| d.runs).sum::<i64>(), 2);
        let recent = store.recent(&scorer, 10).await.unwrap();
        assert_eq!(recent.len(), 2);
        assert_eq!(recent[0].run_id, format!("{scorer}-b"));
        assert!(store
            .summary("no-such-scorer-ever")
            .await
            .unwrap()
            .is_none());
    }
}
