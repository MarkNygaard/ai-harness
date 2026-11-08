//! Learned project facts: what earlier runs in a project kept tripping over,
//! written down as a short fact every later run there is told up front.
//!
//! The evidence is failing tool calls the harness already records
//! (`harness_run_activity`), grouped by [`crate::error_fingerprint`]. A fact
//! names the fingerprints it answers. Whether it reaches prompts is decided by
//! counting runs, never by a model:
//!
//! - **forming** until its failures have shown up in [`ACTIVE_AFTER_RUNS`]
//!   distinct runs, then **active**: injected into every run of the project.
//! - **retired** when its failures keep showing up in that many runs *after*
//!   it went live (it isn't helping), or when a person removes it. A retired
//!   fact keeps its fingerprints, so the same failures are never learned again.
//!
//! A person can also add a fact by hand; it is active at once and has no
//! fingerprints.
//!
//! - `harness_project_facts` — the facts.
//! - `harness_fact_reviews` — one row per `learn-facts` run: which failures it
//!   was shown, and how it ended. At most one is `running` per project.

use chrono::{DateTime, Utc};
use serde::Serialize;
use sqlx::PgPool;

use crate::PersistError;

pub const FACT_FORMING: &str = "forming";
pub const FACT_ACTIVE: &str = "active";
pub const FACT_RETIRED: &str = "retired";

pub const FACT_LEARNED: &str = "learned";
pub const FACT_MANUAL: &str = "manual";

/// Distinct runs a fact's failures must appear in before it goes live, and
/// after it went live before it is judged not to help.
pub const ACTIVE_AFTER_RUNS: usize = 3;

pub const FACT_REVIEW_RUNNING: &str = "running";
pub const FACT_REVIEW_DONE: &str = "done";
pub const FACT_REVIEW_FAILED: &str = "failed";

const CREATE_FACTS: &str = "
CREATE TABLE IF NOT EXISTS harness_project_facts (
    id            text PRIMARY KEY,
    project       text NOT NULL,
    text          text NOT NULL,
    status        text NOT NULL,
    source        text NOT NULL,
    fingerprints  text[] NOT NULL DEFAULT '{}',
    run_ids       text[] NOT NULL DEFAULT '{}',
    retire_reason text,
    created_at    timestamptz NOT NULL DEFAULT now(),
    updated_at    timestamptz NOT NULL DEFAULT now(),
    activated_at  timestamptz,
    retired_at    timestamptz
)";

const INDEX_FACTS: &str = "
CREATE INDEX IF NOT EXISTS harness_project_facts_by_project
    ON harness_project_facts (project, status)";

const CREATE_REVIEWS: &str = "
CREATE TABLE IF NOT EXISTS harness_fact_reviews (
    id           text PRIMARY KEY,
    project      text NOT NULL,
    run_id       text,
    fingerprints text[] NOT NULL DEFAULT '{}',
    status       text NOT NULL DEFAULT 'running',
    error        text,
    created_at   timestamptz NOT NULL DEFAULT now(),
    finished_at  timestamptz
)";

const INDEX_ONE_RUNNING: &str = "
CREATE UNIQUE INDEX IF NOT EXISTS harness_fact_reviews_one_running
    ON harness_fact_reviews (project) WHERE status = 'running'";

const INDEX_REVIEWS_RUN: &str = "
CREATE INDEX IF NOT EXISTS harness_fact_reviews_run ON harness_fact_reviews (run_id)";

const FACT_COLUMNS: &str = "id, project, text, status, source, fingerprints, run_ids, \
     retire_reason, created_at, updated_at, activated_at, retired_at";

const REVIEW_COLUMNS: &str =
    "id, project, run_id, fingerprints, status, error, created_at, finished_at";

/// One fact about a project.
#[derive(Debug, Clone, Serialize, sqlx::FromRow)]
pub struct ProjectFact {
    pub id: String,
    pub project: String,
    /// The fact, as every run is told it.
    pub text: String,
    /// [`FACT_FORMING`], [`FACT_ACTIVE`] or [`FACT_RETIRED`].
    pub status: String,
    /// [`FACT_LEARNED`] or [`FACT_MANUAL`].
    pub source: String,
    /// The failure fingerprints it answers. Empty for a fact added by hand.
    pub fingerprints: Vec<String>,
    /// Runs whose failures it was learned from (and confirmed by), newest
    /// last, capped.
    pub run_ids: Vec<String>,
    pub retire_reason: Option<String>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub activated_at: Option<DateTime<Utc>>,
    pub retired_at: Option<DateTime<Utc>>,
}

/// One `learn-facts` run.
#[derive(Debug, Clone, Serialize, sqlx::FromRow)]
pub struct FactReview {
    pub id: String,
    pub project: String,
    pub run_id: Option<String>,
    /// The failures it was shown. A later review isn't started for these
    /// again while this one is recent, so failures the reviewer judged not
    /// worth a fact don't start a review after every run.
    pub fingerprints: Vec<String>,
    pub status: String,
    pub error: Option<String>,
    pub created_at: DateTime<Utc>,
    pub finished_at: Option<DateTime<Utc>>,
}

/// How many run ids a fact keeps. Enough to open a few examples.
const RUN_IDS_KEPT: usize = 10;

pub struct FactStore {
    pool: PgPool,
}

impl FactStore {
    pub async fn connect(database_url: &str) -> Result<Self, PersistError> {
        let pool = sqlx::postgres::PgPoolOptions::new()
            .max_connections(3)
            .connect(database_url)
            .await?;
        Self::from_pool(pool).await
    }

    pub async fn from_pool(pool: PgPool) -> Result<Self, PersistError> {
        sqlx::query(CREATE_FACTS).execute(&pool).await?;
        sqlx::query(INDEX_FACTS).execute(&pool).await?;
        sqlx::query(CREATE_REVIEWS).execute(&pool).await?;
        sqlx::query(INDEX_ONE_RUNNING).execute(&pool).await?;
        sqlx::query(INDEX_REVIEWS_RUN).execute(&pool).await?;
        Ok(Self { pool })
    }

    // ── Facts ───────────────────────────────────────────────────────────────

    /// Every fact of a project, live ones first, then forming, then retired.
    pub async fn list(&self, project: &str) -> Result<Vec<ProjectFact>, PersistError> {
        Ok(sqlx::query_as::<_, ProjectFact>(&format!(
            "SELECT {FACT_COLUMNS} FROM harness_project_facts WHERE project = $1
             ORDER BY CASE status WHEN 'active' THEN 0 WHEN 'forming' THEN 1 ELSE 2 END,
                      created_at"
        ))
        .bind(project)
        .fetch_all(&self.pool)
        .await?)
    }

    /// The facts a run of `project` is told, oldest first.
    pub async fn active(&self, project: &str) -> Result<Vec<ProjectFact>, PersistError> {
        Ok(sqlx::query_as::<_, ProjectFact>(&format!(
            "SELECT {FACT_COLUMNS} FROM harness_project_facts
             WHERE project = $1 AND status = 'active' ORDER BY activated_at, created_at"
        ))
        .bind(project)
        .fetch_all(&self.pool)
        .await?)
    }

    pub async fn get(&self, id: &str) -> Result<Option<ProjectFact>, PersistError> {
        Ok(sqlx::query_as::<_, ProjectFact>(&format!(
            "SELECT {FACT_COLUMNS} FROM harness_project_facts WHERE id = $1"
        ))
        .bind(id)
        .fetch_optional(&self.pool)
        .await?)
    }

    /// A fact a person wrote: live at once.
    pub async fn add_manual(&self, project: &str, text: &str) -> Result<ProjectFact, PersistError> {
        Ok(sqlx::query_as::<_, ProjectFact>(&format!(
            "INSERT INTO harness_project_facts (id, project, text, status, source, activated_at)
             VALUES ($1, $2, $3, 'active', 'manual', now())
             RETURNING {FACT_COLUMNS}"
        ))
        .bind(uuid::Uuid::new_v4().to_string())
        .bind(project)
        .bind(text)
        .fetch_one(&self.pool)
        .await?)
    }

    /// A fact a review proposed, forming until [`Self::activate`].
    pub async fn add_learned(
        &self,
        project: &str,
        text: &str,
        fingerprints: &[String],
        run_ids: &[String],
    ) -> Result<ProjectFact, PersistError> {
        Ok(sqlx::query_as::<_, ProjectFact>(&format!(
            "INSERT INTO harness_project_facts (id, project, text, status, source, fingerprints, run_ids)
             VALUES ($1, $2, $3, 'forming', 'learned', $4, $5)
             RETURNING {FACT_COLUMNS}"
        ))
        .bind(uuid::Uuid::new_v4().to_string())
        .bind(project)
        .bind(text)
        .bind(fingerprints)
        .bind(keep_last(run_ids))
        .fetch_one(&self.pool)
        .await?)
    }

    /// More failures and runs a fact answers, from a later review.
    pub async fn extend(
        &self,
        id: &str,
        fingerprints: &[String],
        run_ids: &[String],
    ) -> Result<(), PersistError> {
        let Some(fact) = self.get(id).await? else {
            return Ok(());
        };
        let mut fps = fact.fingerprints;
        for f in fingerprints {
            if !fps.contains(f) {
                fps.push(f.clone());
            }
        }
        let mut runs = fact.run_ids;
        for r in run_ids {
            if !runs.contains(r) {
                runs.push(r.clone());
            }
        }
        sqlx::query(
            "UPDATE harness_project_facts
             SET fingerprints = $2, run_ids = $3, updated_at = now() WHERE id = $1",
        )
        .bind(id)
        .bind(&fps)
        .bind(keep_last(&runs))
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    /// Forming → active.
    pub async fn activate(&self, id: &str) -> Result<(), PersistError> {
        sqlx::query(
            "UPDATE harness_project_facts
             SET status = 'active', activated_at = now(), updated_at = now()
             WHERE id = $1 AND status = 'forming'",
        )
        .bind(id)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    /// Out of prompts for good. Its fingerprints stay, so its failures aren't
    /// learned again.
    pub async fn retire(&self, id: &str, reason: &str) -> Result<(), PersistError> {
        sqlx::query(
            "UPDATE harness_project_facts
             SET status = 'retired', retire_reason = $2, retired_at = now(), updated_at = now()
             WHERE id = $1 AND status <> 'retired'",
        )
        .bind(id)
        .bind(reason)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    /// A person's rewording. It stays what it was otherwise.
    pub async fn set_text(&self, id: &str, text: &str) -> Result<(), PersistError> {
        sqlx::query("UPDATE harness_project_facts SET text = $2, updated_at = now() WHERE id = $1")
            .bind(id)
            .bind(text)
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    // ── Reviews ─────────────────────────────────────────────────────────────

    /// Start a review. `None` when one is already running for the project.
    pub async fn begin_review(
        &self,
        project: &str,
        fingerprints: &[String],
    ) -> Result<Option<FactReview>, PersistError> {
        Ok(sqlx::query_as::<_, FactReview>(&format!(
            "INSERT INTO harness_fact_reviews (id, project, fingerprints) VALUES ($1, $2, $3)
             ON CONFLICT DO NOTHING
             RETURNING {REVIEW_COLUMNS}"
        ))
        .bind(uuid::Uuid::new_v4().to_string())
        .bind(project)
        .bind(fingerprints)
        .fetch_optional(&self.pool)
        .await?)
    }

    pub async fn set_review_run(&self, id: &str, run_id: &str) -> Result<(), PersistError> {
        sqlx::query("UPDATE harness_fact_reviews SET run_id = $2 WHERE id = $1")
            .bind(id)
            .bind(run_id)
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    pub async fn finish_review(
        &self,
        id: &str,
        status: &str,
        error: Option<&str>,
    ) -> Result<(), PersistError> {
        sqlx::query(
            "UPDATE harness_fact_reviews SET status = $2, error = $3, finished_at = now()
             WHERE id = $1 AND status = 'running'",
        )
        .bind(id)
        .bind(status)
        .bind(error)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    pub async fn review_for_run(&self, run_id: &str) -> Result<Option<FactReview>, PersistError> {
        Ok(sqlx::query_as::<_, FactReview>(&format!(
            "SELECT {REVIEW_COLUMNS} FROM harness_fact_reviews WHERE run_id = $1"
        ))
        .bind(run_id)
        .fetch_optional(&self.pool)
        .await?)
    }

    /// Reviews of a project started within `days`, newest first.
    pub async fn recent_reviews(
        &self,
        project: &str,
        days: i32,
    ) -> Result<Vec<FactReview>, PersistError> {
        Ok(sqlx::query_as::<_, FactReview>(&format!(
            "SELECT {REVIEW_COLUMNS} FROM harness_fact_reviews
             WHERE project = $1 AND created_at >= now() - make_interval(days => $2)
             ORDER BY created_at DESC"
        ))
        .bind(project)
        .bind(days.max(1))
        .fetch_all(&self.pool)
        .await?)
    }
}

fn keep_last(run_ids: &[String]) -> Vec<String> {
    let skip = run_ids.len().saturating_sub(RUN_IDS_KEPT);
    run_ids[skip..].to_vec()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn db_url() -> Option<String> {
        let url = std::env::var("HARNESS_DATABASE_URL").ok()?;
        // Only ever touch an obvious test database, never production.
        crate::is_test_db(&url).then_some(url)
    }

    #[tokio::test]
    #[serial_test::serial]
    async fn a_learned_fact_forms_goes_live_and_retires() {
        let Some(url) = db_url() else {
            eprintln!("skipping: HARNESS_DATABASE_URL not set");
            return;
        };
        let store = FactStore::connect(&url).await.expect("connect");
        let project = format!("facts-test-{}", Utc::now().timestamp_nanos_opt().unwrap());

        let f = store
            .add_learned(
                &project,
                "Use `bun run test`.",
                &["fp-a".into()],
                &["r1".into()],
            )
            .await
            .unwrap();
        assert_eq!(f.status, FACT_FORMING);
        assert!(store.active(&project).await.unwrap().is_empty());

        store
            .extend(
                &f.id,
                &["fp-a".into(), "fp-b".into()],
                &["r1".into(), "r2".into()],
            )
            .await
            .unwrap();
        store.activate(&f.id).await.unwrap();
        let live = store.active(&project).await.unwrap();
        assert_eq!(live.len(), 1);
        assert_eq!(live[0].fingerprints, ["fp-a", "fp-b"]);
        assert_eq!(live[0].run_ids, ["r1", "r2"]);
        assert!(live[0].activated_at.is_some());

        store.set_text(&f.id, "Run `bun run test`.").await.unwrap();
        store.retire(&f.id, "removed by a person").await.unwrap();
        let all = store.list(&project).await.unwrap();
        assert_eq!(all[0].status, FACT_RETIRED);
        assert_eq!(all[0].text, "Run `bun run test`.");
        assert_eq!(all[0].retire_reason.as_deref(), Some("removed by a person"));
        assert!(store.active(&project).await.unwrap().is_empty());

        let manual = store
            .add_manual(&project, "Builds need `make gen`.")
            .await
            .unwrap();
        assert_eq!(
            (manual.status.as_str(), manual.source.as_str()),
            (FACT_ACTIVE, FACT_MANUAL)
        );
    }

    #[tokio::test]
    #[serial_test::serial]
    async fn one_review_runs_per_project_at_a_time() {
        let Some(url) = db_url() else {
            eprintln!("skipping: HARNESS_DATABASE_URL not set");
            return;
        };
        let store = FactStore::connect(&url).await.expect("connect");
        let project = format!("facts-review-{}", Utc::now().timestamp_nanos_opt().unwrap());
        let first = store
            .begin_review(&project, &["fp-a".into()])
            .await
            .unwrap()
            .expect("started");
        assert!(store.begin_review(&project, &[]).await.unwrap().is_none());
        store.set_review_run(&first.id, "run-1").await.unwrap();
        let found = store.review_for_run("run-1").await.unwrap().unwrap();
        assert_eq!(found.fingerprints, ["fp-a"]);
        store
            .finish_review(&first.id, FACT_REVIEW_DONE, None)
            .await
            .unwrap();
        assert!(store.begin_review(&project, &[]).await.unwrap().is_some());
        let recent = store.recent_reviews(&project, 14).await.unwrap();
        assert_eq!(recent.len(), 2);
    }

    #[test]
    fn a_fact_keeps_its_latest_runs() {
        let ids: Vec<String> = (0..14).map(|i| format!("r{i}")).collect();
        let kept = keep_last(&ids);
        assert_eq!(kept.len(), RUN_IDS_KEPT);
        assert_eq!(kept.first().unwrap(), "r4");
        assert_eq!(kept.last().unwrap(), "r13");
    }
}
