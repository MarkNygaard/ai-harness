//! The bin for deleted settings rows: projects and Linear bindings.
//!
//! A delete **moves** the row here, whole, as JSON, and a restore puts it back.
//! Nothing else changes: the live tables keep only live rows, so no query
//! anywhere has to remember to skip deleted ones. A `deleted_at` column would
//! need exactly that in every reader, and the poller missing it once would
//! leave a deleted binding claiming issues.
//!
//! The row goes back with `jsonb_populate_record`, which maps the JSON onto
//! the table's own columns. A column added since the row was deleted comes
//! back `NULL`, and one dropped is ignored.
//!
//! Secrets never come here. Credentials, tokens and connections are deleted
//! for real: a person deleting one usually means "make this stop working",
//! and a recoverable copy would defeat that.
//!
//! Entries are cleared out after [`TRASH_RETENTION_DAYS`], whenever the bin is
//! listed.

use chrono::{DateTime, Utc};
use serde::Serialize;
use sqlx::postgres::{PgPool, PgPoolOptions};

use crate::PersistError;

/// How long a deleted row stays restorable.
pub const TRASH_RETENTION_DAYS: i64 = 14;

const CREATE_TRASH: &str = "
CREATE TABLE IF NOT EXISTS harness_trash (
    id            text PRIMARY KEY DEFAULT gen_random_uuid()::text,
    -- Which table the row came from; see `TrashKind`.
    kind          text NOT NULL,
    -- What a person recognises it by: the project name, or `project / workflow`.
    label         text NOT NULL,
    -- The project the row belongs to, for filtering a project's bin.
    project       text NOT NULL,
    payload       jsonb NOT NULL,
    deleted_at    timestamptz NOT NULL DEFAULT now(),
    deleted_actor text
)";

/// What kind of row a bin entry holds. The table name comes from here and
/// nowhere else, so nothing a caller sends can name a table.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TrashKind {
    Project,
    LinearBinding,
}

impl TrashKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Project => "project",
            Self::LinearBinding => "linear_binding",
        }
    }

    fn parse(s: &str) -> Option<Self> {
        match s {
            "project" => Some(Self::Project),
            "linear_binding" => Some(Self::LinearBinding),
            _ => None,
        }
    }

    fn table(self) -> &'static str {
        match self {
            Self::Project => "harness_projects",
            Self::LinearBinding => "harness_linear_sources",
        }
    }
}

/// A deleted row waiting in the bin.
#[derive(Debug, Clone, Serialize)]
pub struct TrashEntry {
    pub id: String,
    /// `project` or `linear_binding`.
    pub kind: String,
    pub label: String,
    pub project: String,
    /// For a binding, the workflow it ran; `None` for a project.
    pub workflow: Option<String>,
    pub deleted_at: DateTime<Utc>,
    pub expires_at: DateTime<Utc>,
    /// Who deleted it, as a reader sees them; `None` if unknown.
    pub deleted_actor: Option<String>,
}

#[derive(sqlx::FromRow)]
struct TrashRow {
    id: String,
    kind: String,
    label: String,
    project: String,
    workflow: Option<String>,
    deleted_at: DateTime<Utc>,
    deleted_actor: Option<String>,
}

impl From<TrashRow> for TrashEntry {
    fn from(r: TrashRow) -> Self {
        Self {
            expires_at: r.deleted_at + chrono::Duration::days(TRASH_RETENTION_DAYS),
            id: r.id,
            kind: r.kind,
            label: r.label,
            project: r.project,
            workflow: r.workflow,
            deleted_at: r.deleted_at,
            deleted_actor: r.deleted_actor,
        }
    }
}

/// Why a restore did not happen.
#[derive(Debug)]
pub enum RestoreError {
    /// No such entry: never there, already restored, or cleared out.
    NotInBin,
    /// A row with the same key exists again — a project of that name, or a
    /// binding for that project and workflow. Restoring would overwrite it.
    Taken(String),
    Db(PersistError),
}

impl std::fmt::Display for RestoreError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NotInBin => write!(
                f,
                "not in the bin — it may have been restored already, or cleared \
                 out after {TRASH_RETENTION_DAYS} days"
            ),
            Self::Taken(label) => write!(
                f,
                "`{label}` exists again; delete or rename it first, then restore this one"
            ),
            Self::Db(e) => write!(f, "{e}"),
        }
    }
}

impl From<sqlx::Error> for RestoreError {
    fn from(e: sqlx::Error) -> Self {
        Self::Db(e.into())
    }
}

const SELECT_ENTRY: &str = "SELECT id, kind, label, project, payload->>'workflow' AS workflow,
        deleted_at, deleted_actor FROM harness_trash";

pub struct TrashStore {
    pool: PgPool,
}

impl TrashStore {
    pub async fn connect(database_url: &str) -> Result<Self, PersistError> {
        let pool = PgPoolOptions::new()
            .max_connections(2)
            .connect(database_url)
            .await?;
        Self::from_pool(pool).await
    }

    pub async fn from_pool(pool: PgPool) -> Result<Self, PersistError> {
        sqlx::query(CREATE_TRASH).execute(&pool).await?;
        Ok(Self { pool })
    }

    /// Move a project's row to the bin. Returns the entry, or `None` when there
    /// was no such project. One statement, so the row is never in both places
    /// or neither.
    pub async fn trash_project(
        &self,
        name: &str,
        actor: Option<&str>,
    ) -> Result<Option<TrashEntry>, PersistError> {
        let row = sqlx::query_as::<_, TrashRow>(&format!(
            "WITH moved AS (DELETE FROM harness_projects WHERE name = $1 RETURNING *),
                  ins AS (
                    INSERT INTO harness_trash (kind, label, project, payload, deleted_actor)
                    SELECT 'project', name, name, to_jsonb(moved), $2 FROM moved
                    RETURNING *)
             {}",
            SELECT_ENTRY.replace("FROM harness_trash", "FROM ins")
        ))
        .bind(name)
        .bind(actor)
        .fetch_optional(&self.pool)
        .await?;
        Ok(row.map(Into::into))
    }

    /// Move a Linear binding's row to the bin; `None` when there was none.
    pub async fn trash_linear_binding(
        &self,
        project: &str,
        workflow: &str,
        actor: Option<&str>,
    ) -> Result<Option<TrashEntry>, PersistError> {
        let row = sqlx::query_as::<_, TrashRow>(&format!(
            "WITH moved AS (DELETE FROM harness_linear_sources
                            WHERE project = $1 AND workflow = $2 RETURNING *),
                  ins AS (
                    INSERT INTO harness_trash (kind, label, project, payload, deleted_actor)
                    SELECT 'linear_binding', project || ' / ' || workflow, project,
                           to_jsonb(moved), $3 FROM moved
                    RETURNING *)
             {}",
            SELECT_ENTRY.replace("FROM harness_trash", "FROM ins")
        ))
        .bind(project)
        .bind(workflow)
        .bind(actor)
        .fetch_optional(&self.pool)
        .await?;
        Ok(row.map(Into::into))
    }

    /// What is in the bin, newest first, after clearing out anything past its
    /// retention — so what is listed is exactly what can still be restored.
    pub async fn list(&self) -> Result<Vec<TrashEntry>, PersistError> {
        self.purge().await?;
        let rows =
            sqlx::query_as::<_, TrashRow>(&format!("{SELECT_ENTRY} ORDER BY deleted_at DESC"))
                .fetch_all(&self.pool)
                .await?;
        Ok(rows.into_iter().map(Into::into).collect())
    }

    /// Put a row back where it came from, and return what was restored.
    /// Refuses, leaving the entry in the bin, when its key is in use again.
    pub async fn restore(&self, id: &str) -> Result<TrashEntry, RestoreError> {
        let mut tx = self.pool.begin().await?;
        let row =
            sqlx::query_as::<_, TrashRow>(&format!("{SELECT_ENTRY} WHERE id = $1 FOR UPDATE"))
                .bind(id)
                .fetch_optional(&mut *tx)
                .await?
                .ok_or(RestoreError::NotInBin)?;
        let kind = TrashKind::parse(&row.kind).ok_or(RestoreError::NotInBin)?;
        let table = kind.table();
        let inserted = sqlx::query(&format!(
            "INSERT INTO {table}
             SELECT (jsonb_populate_record(NULL::{table}, payload)).*
             FROM harness_trash WHERE id = $1
             ON CONFLICT DO NOTHING"
        ))
        .bind(id)
        .execute(&mut *tx)
        .await?
        .rows_affected();
        if inserted == 0 {
            return Err(RestoreError::Taken(row.label));
        }
        sqlx::query("DELETE FROM harness_trash WHERE id = $1")
            .bind(id)
            .execute(&mut *tx)
            .await?;
        tx.commit().await?;
        Ok(row.into())
    }

    /// Clear out entries past their retention; returns how many went.
    pub async fn purge(&self) -> Result<u64, PersistError> {
        let done = sqlx::query(
            "DELETE FROM harness_trash WHERE deleted_at < now() - make_interval(days => $1)",
        )
        .bind(TRASH_RETENTION_DAYS as i32)
        .execute(&self.pool)
        .await?;
        Ok(done.rows_affected())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{LinearSourceInput, LinearSourceStore, ProjectInput, ProjectStore};

    fn db_url() -> Option<String> {
        let url = std::env::var("HARNESS_DATABASE_URL").ok()?;
        crate::is_test_db(&url).then_some(url)
    }

    fn project_input() -> ProjectInput {
        ProjectInput {
            git_url: "https://example.com/r.git".into(),
            base_branch: "develop".into(),
            default_workflow: Some("idea-to-pr".into()),
            external_url: None,
            toolchains: vec!["node@22".into()],
            repos: vec![],
            cargo_target_cap_gb: None,
        }
    }

    fn binding_input() -> LinearSourceInput {
        LinearSourceInput {
            team_id: "team".into(),
            team_name: "Team".into(),
            source_state_id: "todo".into(),
            failed_label: None,
            in_progress_state_id: None,
            review_state_id: None,
            ready_state_id: Some("ready".into()),
            piece_ready_state_id: Some("merge".into()),
            epic_review_state_id: None,
            base_branch: None,
            poll_interval_secs: 60,
            max_concurrent_runs: 2,
            max_attempts: 1,
            enabled: true,
            live: true,
        }
    }

    #[tokio::test]
    #[serial_test::serial]
    async fn a_deleted_project_comes_back_whole() {
        let Some(url) = db_url() else {
            eprintln!("skipping: HARNESS_DATABASE_URL not set");
            return;
        };
        let projects = ProjectStore::connect(&url).await.unwrap();
        let trash = TrashStore::connect(&url).await.unwrap();
        let name = format!("p-{}", chrono::Utc::now().timestamp_nanos_opt().unwrap());
        let before = projects.upsert(&name, &project_input()).await.unwrap();

        let entry = trash
            .trash_project(&name, Some("Ann <a@x>"))
            .await
            .unwrap()
            .unwrap();
        assert_eq!(entry.kind, "project");
        assert_eq!(entry.deleted_actor.as_deref(), Some("Ann <a@x>"));
        assert!(
            projects.get(&name).await.unwrap().is_none(),
            "gone from the live table"
        );
        assert!(trash.list().await.unwrap().iter().any(|e| e.id == entry.id));

        trash.restore(&entry.id).await.unwrap();
        let after = projects.get(&name).await.unwrap().expect("restored");
        assert_eq!(after.base_branch, "develop");
        assert_eq!(after.toolchains, before.toolchains);
        assert_eq!(
            after.created_at, before.created_at,
            "the original row, not a new one"
        );
        assert!(matches!(
            trash.restore(&entry.id).await,
            Err(RestoreError::NotInBin)
        ));

        // Deleting what is not there puts nothing in the bin.
        projects.delete(&name).await.unwrap();
        assert!(trash.trash_project(&name, None).await.unwrap().is_none());
    }

    #[tokio::test]
    #[serial_test::serial]
    async fn restoring_a_binding_never_overwrites_one_made_since() {
        let Some(url) = db_url() else {
            eprintln!("skipping: HARNESS_DATABASE_URL not set");
            return;
        };
        let projects = ProjectStore::connect(&url).await.unwrap();
        let sources = LinearSourceStore::connect(&url).await.unwrap();
        let trash = TrashStore::connect(&url).await.unwrap();
        let name = format!("p-{}", chrono::Utc::now().timestamp_nanos_opt().unwrap());
        projects.upsert(&name, &project_input()).await.unwrap();
        sources
            .upsert(&name, "idea-to-pr", &binding_input())
            .await
            .unwrap();

        let entry = trash
            .trash_linear_binding(&name, "idea-to-pr", None)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(entry.workflow.as_deref(), Some("idea-to-pr"));
        assert!(sources.get(&name, "idea-to-pr").await.unwrap().is_none());

        // A new binding for the same workflow, made in the meantime.
        let mut newer = binding_input();
        newer.source_state_id = "backlog".into();
        sources.upsert(&name, "idea-to-pr", &newer).await.unwrap();
        assert!(matches!(
            trash.restore(&entry.id).await,
            Err(RestoreError::Taken(_))
        ));
        let live = sources.get(&name, "idea-to-pr").await.unwrap().unwrap();
        assert_eq!(live.source_state_id, "backlog", "left alone");

        // Once the name is free again, the original comes back with its columns.
        sources.delete(&name, "idea-to-pr").await.unwrap();
        trash.restore(&entry.id).await.unwrap();
        let back = sources.get(&name, "idea-to-pr").await.unwrap().unwrap();
        assert_eq!(back.source_state_id, "todo");
        assert_eq!(back.piece_ready_state_id.as_deref(), Some("merge"));
        assert_eq!(back.max_concurrent_runs, 2);

        sources.delete(&name, "idea-to-pr").await.unwrap();
        projects.delete(&name).await.unwrap();
    }
}
