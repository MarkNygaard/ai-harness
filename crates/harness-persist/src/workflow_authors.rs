//! Who wrote a custom workflow, and who touched it last.
//!
//! A workflow is a **file** — `.harness/workflows/<name>.yaml` — so there is no
//! row to hang provenance on, and the YAML is the wrong place for it: people
//! author that by hand, it is what the editor shows, and a hand-edit would
//! silently rewrite whatever we wrote there. This table is the provenance
//! instead, keyed by the same name the file is.
//!
//! **The file is the source of truth for existence; this is only a note beside
//! it.** They can drift — a workflow dropped into the directory by hand has no
//! row, and one deleted outside the app leaves one behind. Both are harmless as
//! long as a missing row reads as "unknown" rather than as an error, which is
//! the same way a run from before attribution shows nobody. Bundled workflows
//! never have a row at all: they ship inside the binary and nobody here wrote
//! them.

use chrono::{DateTime, Utc};
use serde::Serialize;
use sqlx::postgres::{PgPool, PgPoolOptions};

use crate::PersistError;

const CREATE_WORKFLOW_AUTHORS: &str = "
CREATE TABLE IF NOT EXISTS harness_workflow_authors (
    -- The workflow's file stem, which is what names it everywhere else.
    name            text PRIMARY KEY,
    -- Harness account id, when the editor has one; the label is what a reader
    -- sees and is filled whether or not there is an account behind it.
    created_by      text,
    created_actor   text,
    created_source  text,
    created_at      timestamptz NOT NULL DEFAULT now(),
    updated_by      text,
    updated_actor   text,
    updated_source  text,
    updated_at      timestamptz NOT NULL DEFAULT now()
)";

/// Who created a workflow and who last changed it.
#[derive(Debug, Clone, Serialize, sqlx::FromRow)]
pub struct WorkflowAuthorship {
    pub name: String,
    /// Harness account id of whoever created it; `None` if unidentified.
    pub created_by: Option<String>,
    /// Creator as a reader sees them (`Name <email>`).
    pub created_actor: Option<String>,
    /// Which door the creation came through: `ui` or `mcp`.
    pub created_source: Option<String>,
    pub created_at: DateTime<Utc>,
    pub updated_by: Option<String>,
    pub updated_actor: Option<String>,
    pub updated_source: Option<String>,
    pub updated_at: DateTime<Utc>,
}

/// Who made one edit, borrowed for binding.
#[derive(Debug, Clone, Copy, Default)]
pub struct Editor<'a> {
    pub user_id: Option<&'a str>,
    pub actor: Option<&'a str>,
    pub source: Option<&'a str>,
}

/// Reads and writes [`WorkflowAuthorship`].
#[derive(Clone)]
pub struct WorkflowAuthorStore {
    pool: PgPool,
}

impl WorkflowAuthorStore {
    pub async fn connect(database_url: &str) -> Result<Self, PersistError> {
        let pool = PgPoolOptions::new()
            .max_connections(3)
            .connect(database_url)
            .await?;
        Self::from_pool(pool).await
    }

    pub async fn from_pool(pool: PgPool) -> Result<Self, PersistError> {
        let store = Self { pool };
        sqlx::query(CREATE_WORKFLOW_AUTHORS)
            .execute(&store.pool)
            .await?;
        Ok(store)
    }

    /// Record an edit, creating the row on the first one.
    ///
    /// **The creator is written once and never again.** Every save routes here —
    /// a node moved, an edge drawn, the YAML replaced wholesale — and if the
    /// creation fields were rewritten each time, the first person to open
    /// somebody else's workflow and nudge a node would become its author.
    /// `COALESCE` keeps the original, so the pair reads as "who started this"
    /// and "who touched it last".
    ///
    /// A first edit to a workflow that already existed on disk — one added by
    /// hand, or created before this table did — records that editor as the
    /// creator. There is no better answer available; nobody wrote down who
    /// actually made it.
    pub async fn record_edit(&self, name: &str, editor: &Editor<'_>) -> Result<(), PersistError> {
        sqlx::query(
            "INSERT INTO harness_workflow_authors
                 (name, created_by, created_actor, created_source,
                  updated_by, updated_actor, updated_source, updated_at)
             VALUES ($1, $2, $3, $4, $2, $3, $4, now())
             ON CONFLICT (name) DO UPDATE SET
                created_by     = COALESCE(harness_workflow_authors.created_by, excluded.created_by),
                created_actor  = COALESCE(harness_workflow_authors.created_actor, excluded.created_actor),
                created_source = COALESCE(harness_workflow_authors.created_source, excluded.created_source),
                updated_by     = excluded.updated_by,
                updated_actor  = excluded.updated_actor,
                updated_source = excluded.updated_source,
                updated_at     = now()",
        )
        .bind(name)
        .bind(editor.user_id)
        .bind(editor.actor)
        .bind(editor.source)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    /// Forget a workflow's provenance, for when the workflow itself is deleted.
    /// Leaving the row would re-attach an old author to a new workflow that
    /// happened to reuse the name.
    pub async fn forget(&self, name: &str) -> Result<(), PersistError> {
        sqlx::query("DELETE FROM harness_workflow_authors WHERE name = $1")
            .bind(name)
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    /// Authorship for every workflow that has any, for joining onto a listing.
    pub async fn all(&self) -> Result<Vec<WorkflowAuthorship>, PersistError> {
        Ok(sqlx::query_as::<_, WorkflowAuthorship>(
            "SELECT name, created_by, created_actor, created_source, created_at,
                    updated_by, updated_actor, updated_source, updated_at
             FROM harness_workflow_authors",
        )
        .fetch_all(&self.pool)
        .await?)
    }

    /// One workflow's authorship, or `None` when nothing is recorded.
    pub async fn get(&self, name: &str) -> Result<Option<WorkflowAuthorship>, PersistError> {
        Ok(sqlx::query_as::<_, WorkflowAuthorship>(
            "SELECT name, created_by, created_actor, created_source, created_at,
                    updated_by, updated_actor, updated_source, updated_at
             FROM harness_workflow_authors WHERE name = $1",
        )
        .bind(name)
        .fetch_optional(&self.pool)
        .await?)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn db_url() -> Option<String> {
        let url = std::env::var("HARNESS_DATABASE_URL").ok()?;
        crate::is_test_db(&url).then_some(url)
    }

    /// The invariant the whole table turns on: every authoring change routes
    /// through `record_edit`, so if it rewrote the creation fields, the first
    /// person to nudge a node on somebody else's workflow would become its
    /// author.
    #[tokio::test]
    #[serial_test::serial]
    async fn the_creator_survives_everyone_elses_edits() {
        let Some(url) = db_url() else {
            eprintln!("skipping: HARNESS_DATABASE_URL not set");
            return;
        };
        let store = WorkflowAuthorStore::connect(&url).await.expect("connect");
        let name = format!(
            "wf-authors-{}",
            chrono::Utc::now().timestamp_nanos_opt().unwrap()
        );

        store
            .record_edit(
                &name,
                &Editor {
                    user_id: Some("u-andrius"),
                    actor: Some("Andrius <a@x.com>"),
                    source: Some("ui"),
                },
            )
            .await
            .unwrap();
        store
            .record_edit(
                &name,
                &Editor {
                    user_id: Some("u-mark"),
                    actor: Some("Mark <m@x.com>"),
                    source: Some("mcp"),
                },
            )
            .await
            .unwrap();

        let got = store.get(&name).await.unwrap().expect("row");
        assert_eq!(got.created_by.as_deref(), Some("u-andrius"));
        assert_eq!(got.created_actor.as_deref(), Some("Andrius <a@x.com>"));
        assert_eq!(got.created_source.as_deref(), Some("ui"));
        assert_eq!(got.updated_by.as_deref(), Some("u-mark"));
        assert_eq!(got.updated_actor.as_deref(), Some("Mark <m@x.com>"));
        assert_eq!(got.updated_source.as_deref(), Some("mcp"));
        assert!(got.updated_at >= got.created_at);

        // An unidentified edit must not blank out a known creator, or an
        // install losing its session would erase what it already knew.
        store.record_edit(&name, &Editor::default()).await.unwrap();
        let after = store.get(&name).await.unwrap().expect("row");
        assert_eq!(after.created_by.as_deref(), Some("u-andrius"));

        assert!(store.all().await.unwrap().iter().any(|a| a.name == name));

        // Deleting the workflow forgets it, so a later workflow reusing the
        // name does not inherit an author who never saw it.
        store.forget(&name).await.unwrap();
        assert!(store.get(&name).await.unwrap().is_none());
    }

    #[tokio::test]
    #[serial_test::serial]
    async fn a_workflow_nobody_recorded_reads_as_unknown() {
        let Some(url) = db_url() else {
            eprintln!("skipping: HARNESS_DATABASE_URL not set");
            return;
        };
        let store = WorkflowAuthorStore::connect(&url).await.expect("connect");
        // A bundled workflow, or one dropped into the directory by hand: no
        // row, and that has to be an ordinary answer rather than an error.
        assert!(store
            .get("never-recorded-workflow")
            .await
            .unwrap()
            .is_none());
    }
}
