//! Improvements: what a workflow's recent runs suggest changing.
//!
//! A workflow that opts in (`improvements: { every: N }`) is reviewed after
//! every N finished runs *in each project* — the fixes it leads to land in that
//! project's repo, so runs from different repos are never mixed. A review is an
//! internal run of `improvements-analysis`; what it suggests is kept here, each
//! suggestion new, applied (with the run that applied it) or dismissed.
//!
//! - `harness_improvement_reviews` — one row per review: which runs it read,
//!   the analysis run, and how it ended. At most one is `running` per
//!   (workflow, project), enforced by a partial unique index, so a burst of
//!   finishing runs can't start two.
//! - `harness_improvement_suggestions` — what a review found.

use chrono::{DateTime, Utc};
use serde::Serialize;
use serde_json::Value;
use sqlx::types::Json;
use sqlx::PgPool;

use crate::PersistError;

pub const REVIEW_RUNNING: &str = "running";
pub const REVIEW_DONE: &str = "done";
pub const REVIEW_FAILED: &str = "failed";

pub const SUGGESTION_NEW: &str = "new";
pub const SUGGESTION_APPLIED: &str = "applied";
pub const SUGGESTION_DISMISSED: &str = "dismissed";

const CREATE_REVIEWS: &str = "
CREATE TABLE IF NOT EXISTS harness_improvement_reviews (
    id          text PRIMARY KEY,
    workflow    text NOT NULL,
    project     text NOT NULL,
    run_ids     text[] NOT NULL DEFAULT '{}',
    run_id      text,
    status      text NOT NULL DEFAULT 'running',
    summary     text,
    error       text,
    created_at  timestamptz NOT NULL DEFAULT now(),
    finished_at timestamptz
)";

const INDEX_ONE_RUNNING: &str = "
CREATE UNIQUE INDEX IF NOT EXISTS harness_improvement_reviews_one_running
    ON harness_improvement_reviews (workflow, project) WHERE status = 'running'";

const INDEX_REVIEWS_RUN: &str = "
CREATE INDEX IF NOT EXISTS harness_improvement_reviews_run
    ON harness_improvement_reviews (run_id)";

const CREATE_SUGGESTIONS: &str = "
CREATE TABLE IF NOT EXISTS harness_improvement_suggestions (
    id             text PRIMARY KEY,
    review_id      text NOT NULL,
    workflow       text NOT NULL,
    project        text NOT NULL,
    target         text NOT NULL,
    category       text NOT NULL,
    node           text,
    title          text NOT NULL,
    problem        text NOT NULL,
    change         text NOT NULL,
    evidence       jsonb NOT NULL DEFAULT '[]'::jsonb,
    status         text NOT NULL DEFAULT 'new',
    applied_run_id text,
    created_at     timestamptz NOT NULL DEFAULT now(),
    updated_at     timestamptz NOT NULL DEFAULT now()
)";

const INDEX_SUGGESTIONS: &str = "
CREATE INDEX IF NOT EXISTS harness_improvement_suggestions_by_workflow
    ON harness_improvement_suggestions (workflow, project, created_at DESC)";

const REVIEW_COLUMNS: &str =
    "id, workflow, project, run_ids, run_id, status, summary, error, created_at, finished_at";

const SUGGESTION_COLUMNS: &str = "id, review_id, workflow, project, target, category, node, \
     title, problem, change, evidence, status, applied_run_id, created_at, updated_at";

/// One review of a workflow's runs in a project.
#[derive(Debug, Clone, Serialize, sqlx::FromRow)]
pub struct ImprovementReview {
    pub id: String,
    pub workflow: String,
    pub project: String,
    /// The runs it read, newest first.
    pub run_ids: Vec<String>,
    /// The `improvements-analysis` run doing the reading.
    pub run_id: Option<String>,
    /// [`REVIEW_RUNNING`], [`REVIEW_DONE`] or [`REVIEW_FAILED`].
    pub status: String,
    /// How the runs went overall, in the reviewer's words.
    pub summary: Option<String>,
    pub error: Option<String>,
    pub created_at: DateTime<Utc>,
    pub finished_at: Option<DateTime<Utc>>,
}

/// One thing a review suggests changing.
#[derive(Debug, Clone, Serialize, serde::Deserialize, sqlx::FromRow)]
pub struct ImprovementSuggestion {
    pub id: String,
    pub review_id: String,
    pub workflow: String,
    pub project: String,
    /// `repo` (the project's repo) or `workflow` (the workflow itself).
    pub target: String,
    /// `navigation`, `check`, `standard`, `skill`, `prune`, `access`, `workflow`.
    pub category: String,
    /// The workflow node it concerns, when there is one.
    pub node: Option<String>,
    pub title: String,
    pub problem: String,
    /// A self-contained task: what to change, where, and why.
    pub change: String,
    /// `[{run_id, node?, note}]` — the runs that show the problem.
    pub evidence: Json<Value>,
    /// [`SUGGESTION_NEW`], [`SUGGESTION_APPLIED`] or [`SUGGESTION_DISMISSED`].
    pub status: String,
    /// The run started to carry it out, when one was.
    pub applied_run_id: Option<String>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

/// A suggestion as a review produces it.
#[derive(Debug, Clone)]
pub struct NewSuggestion {
    pub target: String,
    pub category: String,
    pub node: Option<String>,
    pub title: String,
    pub problem: String,
    pub change: String,
    pub evidence: Value,
}

/// Which suggestions to list. `None` matches anything.
#[derive(Debug, Clone, Default)]
pub struct SuggestionFilter {
    pub workflow: Option<String>,
    pub project: Option<String>,
    pub status: Option<String>,
}

pub struct ImprovementStore {
    pool: PgPool,
}

impl ImprovementStore {
    pub async fn connect(database_url: &str) -> Result<Self, PersistError> {
        let pool = sqlx::postgres::PgPoolOptions::new()
            .max_connections(3)
            .connect(database_url)
            .await?;
        Self::from_pool(pool).await
    }

    pub async fn from_pool(pool: PgPool) -> Result<Self, PersistError> {
        sqlx::query(CREATE_REVIEWS).execute(&pool).await?;
        sqlx::query(INDEX_ONE_RUNNING).execute(&pool).await?;
        sqlx::query(INDEX_REVIEWS_RUN).execute(&pool).await?;
        sqlx::query(CREATE_SUGGESTIONS).execute(&pool).await?;
        sqlx::query(INDEX_SUGGESTIONS).execute(&pool).await?;
        Ok(Self { pool })
    }

    // ── Reviews ─────────────────────────────────────────────────────────────

    /// Start a review. `None` when one is already running for this workflow
    /// and project — the caller lost a race and should do nothing.
    pub async fn begin_review(
        &self,
        id: &str,
        workflow: &str,
        project: &str,
        run_ids: &[String],
    ) -> Result<Option<ImprovementReview>, PersistError> {
        let row = sqlx::query_as::<_, ImprovementReview>(&format!(
            "INSERT INTO harness_improvement_reviews (id, workflow, project, run_ids)
             VALUES ($1, $2, $3, $4)
             ON CONFLICT DO NOTHING
             RETURNING {REVIEW_COLUMNS}"
        ))
        .bind(id)
        .bind(workflow)
        .bind(project)
        .bind(run_ids)
        .fetch_optional(&self.pool)
        .await?;
        Ok(row)
    }

    /// Link a review to the analysis run reading its runs.
    pub async fn set_review_run(&self, id: &str, run_id: &str) -> Result<(), PersistError> {
        sqlx::query("UPDATE harness_improvement_reviews SET run_id = $2 WHERE id = $1")
            .bind(id)
            .bind(run_id)
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    /// End a review: done, with suggestions; or failed, with why. Only a
    /// running review ends, so a second finish is a no-op. `false` when there
    /// was nothing to end.
    pub async fn finish_review(
        &self,
        id: &str,
        status: &str,
        summary: Option<&str>,
        error: Option<&str>,
        suggestions: &[NewSuggestion],
    ) -> Result<bool, PersistError> {
        let mut tx = self.pool.begin().await?;
        let review = sqlx::query_as::<_, ImprovementReview>(&format!(
            "UPDATE harness_improvement_reviews
             SET status = $2, summary = $3, error = $4, finished_at = now()
             WHERE id = $1 AND status = 'running'
             RETURNING {REVIEW_COLUMNS}"
        ))
        .bind(id)
        .bind(status)
        .bind(summary)
        .bind(error)
        .fetch_optional(&mut *tx)
        .await?;
        let Some(review) = review else {
            return Ok(false);
        };
        for s in suggestions {
            sqlx::query(
                "INSERT INTO harness_improvement_suggestions
                    (id, review_id, workflow, project, target, category, node, title,
                     problem, change, evidence)
                 VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11)",
            )
            .bind(uuid::Uuid::new_v4().to_string())
            .bind(&review.id)
            .bind(&review.workflow)
            .bind(&review.project)
            .bind(&s.target)
            .bind(&s.category)
            .bind(&s.node)
            .bind(&s.title)
            .bind(&s.problem)
            .bind(&s.change)
            .bind(Json(&s.evidence))
            .execute(&mut *tx)
            .await?;
        }
        tx.commit().await?;
        Ok(true)
    }

    pub async fn get_review(&self, id: &str) -> Result<Option<ImprovementReview>, PersistError> {
        let row = sqlx::query_as::<_, ImprovementReview>(&format!(
            "SELECT {REVIEW_COLUMNS} FROM harness_improvement_reviews WHERE id = $1"
        ))
        .bind(id)
        .fetch_optional(&self.pool)
        .await?;
        Ok(row)
    }

    /// The review an analysis run belongs to.
    pub async fn review_for_run(
        &self,
        run_id: &str,
    ) -> Result<Option<ImprovementReview>, PersistError> {
        let row = sqlx::query_as::<_, ImprovementReview>(&format!(
            "SELECT {REVIEW_COLUMNS} FROM harness_improvement_reviews WHERE run_id = $1"
        ))
        .bind(run_id)
        .fetch_optional(&self.pool)
        .await?;
        Ok(row)
    }

    /// The most recent review of a workflow in a project, of any status.
    pub async fn latest_review(
        &self,
        workflow: &str,
        project: &str,
    ) -> Result<Option<ImprovementReview>, PersistError> {
        let row = sqlx::query_as::<_, ImprovementReview>(&format!(
            "SELECT {REVIEW_COLUMNS} FROM harness_improvement_reviews
             WHERE workflow = $1 AND project = $2
             ORDER BY created_at DESC LIMIT 1"
        ))
        .bind(workflow)
        .bind(project)
        .fetch_optional(&self.pool)
        .await?;
        Ok(row)
    }

    /// The most recent review that finished with an answer — where the next
    /// counting window starts. A failed review read nothing, so it doesn't
    /// move the window.
    pub async fn latest_review_done(
        &self,
        workflow: &str,
        project: &str,
    ) -> Result<Option<ImprovementReview>, PersistError> {
        let row = sqlx::query_as::<_, ImprovementReview>(&format!(
            "SELECT {REVIEW_COLUMNS} FROM harness_improvement_reviews
             WHERE workflow = $1 AND project = $2 AND status = 'done'
             ORDER BY created_at DESC LIMIT 1"
        ))
        .bind(workflow)
        .bind(project)
        .fetch_optional(&self.pool)
        .await?;
        Ok(row)
    }

    /// Every review still running, across workflows and projects.
    pub async fn running_reviews(&self) -> Result<Vec<ImprovementReview>, PersistError> {
        let rows = sqlx::query_as::<_, ImprovementReview>(&format!(
            "SELECT {REVIEW_COLUMNS} FROM harness_improvement_reviews WHERE status = 'running'"
        ))
        .fetch_all(&self.pool)
        .await?;
        Ok(rows)
    }

    /// The latest review per (workflow, project), newest first.
    pub async fn latest_reviews(&self) -> Result<Vec<ImprovementReview>, PersistError> {
        let rows = sqlx::query_as::<_, ImprovementReview>(&format!(
            "SELECT DISTINCT ON (workflow, project) {REVIEW_COLUMNS}
             FROM harness_improvement_reviews
             ORDER BY workflow, project, created_at DESC"
        ))
        .fetch_all(&self.pool)
        .await?;
        Ok(rows)
    }

    // ── Suggestions ─────────────────────────────────────────────────────────

    pub async fn list_suggestions(
        &self,
        filter: &SuggestionFilter,
    ) -> Result<Vec<ImprovementSuggestion>, PersistError> {
        let rows = sqlx::query_as::<_, ImprovementSuggestion>(&format!(
            "SELECT {SUGGESTION_COLUMNS} FROM harness_improvement_suggestions
             WHERE ($1::text IS NULL OR workflow = $1)
               AND ($2::text IS NULL OR project = $2)
               AND ($3::text IS NULL OR status = $3)
             ORDER BY created_at DESC, id"
        ))
        .bind(&filter.workflow)
        .bind(&filter.project)
        .bind(&filter.status)
        .fetch_all(&self.pool)
        .await?;
        Ok(rows)
    }

    pub async fn get_suggestion(
        &self,
        id: &str,
    ) -> Result<Option<ImprovementSuggestion>, PersistError> {
        let row = sqlx::query_as::<_, ImprovementSuggestion>(&format!(
            "SELECT {SUGGESTION_COLUMNS} FROM harness_improvement_suggestions WHERE id = $1"
        ))
        .bind(id)
        .fetch_optional(&self.pool)
        .await?;
        Ok(row)
    }

    /// Mark a suggestion applied, dismissed, or new again. `applied_run_id` is
    /// kept unless a new one is given, so reopening doesn't forget the PR.
    pub async fn set_suggestion_status(
        &self,
        id: &str,
        status: &str,
        applied_run_id: Option<&str>,
    ) -> Result<Option<ImprovementSuggestion>, PersistError> {
        let row = sqlx::query_as::<_, ImprovementSuggestion>(&format!(
            "UPDATE harness_improvement_suggestions
             SET status = $2, applied_run_id = COALESCE($3, applied_run_id), updated_at = now()
             WHERE id = $1
             RETURNING {SUGGESTION_COLUMNS}"
        ))
        .bind(id)
        .bind(status)
        .bind(applied_run_id)
        .fetch_optional(&self.pool)
        .await?;
        Ok(row)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn db_url() -> Option<String> {
        let url = std::env::var("HARNESS_DATABASE_URL").ok()?;
        crate::is_test_db(&url).then_some(url)
    }

    fn unique(prefix: &str) -> String {
        format!(
            "{prefix}-{}",
            chrono::Utc::now().timestamp_nanos_opt().unwrap()
        )
    }

    fn suggestion(title: &str) -> NewSuggestion {
        NewSuggestion {
            target: "repo".into(),
            category: "navigation".into(),
            node: None,
            title: title.into(),
            problem: "searched for the migrations folder in 4 of 10 runs".into(),
            change: "add a pointer to db/migrations in AGENTS.md".into(),
            evidence: serde_json::json!([{ "run_id": "r1", "note": "12 find calls" }]),
        }
    }

    #[tokio::test]
    #[serial_test::serial]
    async fn one_review_runs_at_a_time_per_workflow_and_project() {
        let Some(url) = db_url() else {
            eprintln!("skipping: HARNESS_DATABASE_URL not set");
            return;
        };
        let store = ImprovementStore::connect(&url).await.expect("connect");
        let wf = unique("wf");
        let runs = vec!["a".to_string(), "b".to_string()];
        let first = store
            .begin_review(&unique("rev"), &wf, "p", &runs)
            .await
            .unwrap();
        assert!(first.is_some());
        let second = store
            .begin_review(&unique("rev"), &wf, "p", &runs)
            .await
            .unwrap();
        assert!(second.is_none(), "a second review waits for the first");
        let other_project = store
            .begin_review(&unique("rev"), &wf, "q", &runs)
            .await
            .unwrap();
        assert!(other_project.is_some(), "projects are reviewed separately");

        let first = first.unwrap();
        store
            .finish_review(&first.id, REVIEW_FAILED, None, Some("x"), &[])
            .await
            .unwrap();
        let again = store
            .begin_review(&unique("rev"), &wf, "p", &runs)
            .await
            .unwrap();
        assert!(again.is_some(), "once it ends, the next may start");
    }

    #[tokio::test]
    #[serial_test::serial]
    async fn finishing_stores_suggestions_once() {
        let Some(url) = db_url() else {
            eprintln!("skipping: HARNESS_DATABASE_URL not set");
            return;
        };
        let store = ImprovementStore::connect(&url).await.expect("connect");
        let wf = unique("wf");
        let review = store
            .begin_review(&unique("rev"), &wf, "p", &["r1".to_string()])
            .await
            .unwrap()
            .unwrap();
        store.set_review_run(&review.id, "run-x").await.unwrap();
        assert_eq!(
            store.review_for_run("run-x").await.unwrap().map(|r| r.id),
            Some(review.id.clone())
        );

        let done = store
            .finish_review(
                &review.id,
                REVIEW_DONE,
                Some("ok"),
                None,
                &[suggestion("a"), suggestion("b")],
            )
            .await
            .unwrap();
        assert!(done);
        let again = store
            .finish_review(
                &review.id,
                REVIEW_DONE,
                Some("ok"),
                None,
                &[suggestion("c")],
            )
            .await
            .unwrap();
        assert!(!again, "a finished review doesn't finish twice");

        let filter = SuggestionFilter {
            workflow: Some(wf.clone()),
            ..Default::default()
        };
        let listed = store.list_suggestions(&filter).await.unwrap();
        assert_eq!(listed.len(), 2);

        let applied = store
            .set_suggestion_status(&listed[0].id, SUGGESTION_APPLIED, Some("run-apply"))
            .await
            .unwrap()
            .unwrap();
        assert_eq!(applied.applied_run_id.as_deref(), Some("run-apply"));
        let reopened = store
            .set_suggestion_status(&listed[0].id, SUGGESTION_NEW, None)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(
            reopened.applied_run_id.as_deref(),
            Some("run-apply"),
            "reopening keeps the link to what was tried"
        );
        let new_only = SuggestionFilter {
            workflow: Some(wf),
            status: Some(SUGGESTION_NEW.into()),
            ..Default::default()
        };
        assert_eq!(store.list_suggestions(&new_only).await.unwrap().len(), 2);
    }
}
