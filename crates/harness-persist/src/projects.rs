//! Project registry.
//!
//! A **project** scopes runs to a git repo. Each project is registered once (its
//! repo is cloned onto the control plane's persistent volume) and then runs are
//! triggered *within* it — the run operates on that project's checkout, and
//! (later) its Linear sources feed it. Provider credentials stay global; only the
//! repo + its GitHub access are project-scoped.
//!
//! This module just persists the registry rows; cloning/fetching the working
//! copy is the server's responsibility (it owns the filesystem layout).

use chrono::{DateTime, Utc};
use serde::Serialize;
use sqlx::postgres::{PgPool, PgPoolOptions};

use crate::PersistError;

const CREATE_PROJECTS: &str = "
CREATE TABLE IF NOT EXISTS harness_projects (
    name             text PRIMARY KEY,
    git_url          text NOT NULL,
    base_branch      text NOT NULL DEFAULT 'main',
    default_workflow text,
    toolchains       jsonb NOT NULL DEFAULT '[]'::jsonb,
    created_at       timestamptz NOT NULL DEFAULT now(),
    updated_at       timestamptz NOT NULL DEFAULT now()
)";

/// Bring older `harness_projects` tables up to date. Idempotent.
const ALTER_PROJECTS_TOOLCHAINS: &str =
    "ALTER TABLE harness_projects ADD COLUMN IF NOT EXISTS toolchains jsonb NOT NULL DEFAULT '[]'::jsonb";
/// Per-project cargo build-cache size cap, in GiB. NULL = use the env
/// default (`HARNESS_CARGO_TARGET_CAP_GB`, else 50 GiB). Idempotent.
const ALTER_PROJECTS_CAP: &str =
    "ALTER TABLE harness_projects ADD COLUMN IF NOT EXISTS cargo_target_cap_gb integer";

/// A registered project (matches `harness_projects`).
#[derive(Debug, Clone, Serialize, sqlx::FromRow)]
pub struct Project {
    /// Slug + display name; also the on-disk checkout directory name.
    pub name: String,
    /// Clone source (https or ssh). Private repos use the global GitHub token.
    pub git_url: String,
    /// Branch runs are based off of (worktree HEAD).
    pub base_branch: String,
    /// Workflow used when a run for this project names none.
    pub default_workflow: Option<String>,
    /// `mise` tool specs provisioned before a run (e.g. `rust`, `node@22`,
    /// `pnpm`). Installed on demand onto the persistent volume — no image rebuild.
    #[sqlx(json)]
    pub toolchains: Vec<String>,
    /// Per-project build-cache cap in GiB; `None` falls back to the env default.
    pub cargo_target_cap_gb: Option<i32>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

/// Fields accepted when registering / updating a project.
#[derive(Debug, Clone)]
pub struct ProjectInput {
    pub git_url: String,
    pub base_branch: String,
    pub default_workflow: Option<String>,
    pub toolchains: Vec<String>,
    pub cargo_target_cap_gb: Option<i32>,
}

/// Postgres-backed registry of projects.
pub struct ProjectStore {
    pool: PgPool,
}

impl ProjectStore {
    /// Connect to `database_url` and ensure the schema exists.
    pub async fn connect(database_url: &str) -> Result<Self, PersistError> {
        let pool = PgPoolOptions::new()
            .max_connections(3)
            .connect(database_url)
            .await?;
        Self::from_pool(pool).await
    }

    /// Wrap an existing pool; ensures the table exists + is up to date.
    pub async fn from_pool(pool: PgPool) -> Result<Self, PersistError> {
        let store = Self { pool };
        sqlx::query(CREATE_PROJECTS).execute(&store.pool).await?;
        sqlx::query(ALTER_PROJECTS_TOOLCHAINS)
            .execute(&store.pool)
            .await?;
        sqlx::query(ALTER_PROJECTS_CAP).execute(&store.pool).await?;
        Ok(store)
    }

    /// All projects, alphabetical.
    pub async fn list(&self) -> Result<Vec<Project>, PersistError> {
        let rows = sqlx::query_as::<_, Project>(
            "SELECT name, git_url, base_branch, default_workflow, toolchains, cargo_target_cap_gb, created_at, updated_at
             FROM harness_projects ORDER BY name",
        )
        .fetch_all(&self.pool)
        .await?;
        Ok(rows)
    }

    /// One project by name, if present.
    pub async fn get(&self, name: &str) -> Result<Option<Project>, PersistError> {
        let row = sqlx::query_as::<_, Project>(
            "SELECT name, git_url, base_branch, default_workflow, toolchains, cargo_target_cap_gb, created_at, updated_at
             FROM harness_projects WHERE name = $1",
        )
        .bind(name)
        .fetch_optional(&self.pool)
        .await?;
        Ok(row)
    }

    /// Register or update a project (upsert on `name`). `created_at` is preserved
    /// on update; `updated_at` always advances.
    pub async fn upsert(&self, name: &str, input: &ProjectInput) -> Result<Project, PersistError> {
        let row = sqlx::query_as::<_, Project>(
            "INSERT INTO harness_projects (name, git_url, base_branch, default_workflow, toolchains, cargo_target_cap_gb, created_at, updated_at)
             VALUES ($1, $2, $3, $4, $5, $6, now(), now())
             ON CONFLICT (name) DO UPDATE SET
                git_url               = excluded.git_url,
                base_branch           = excluded.base_branch,
                default_workflow      = excluded.default_workflow,
                toolchains            = excluded.toolchains,
                cargo_target_cap_gb   = excluded.cargo_target_cap_gb,
                updated_at            = now()
             RETURNING name, git_url, base_branch, default_workflow, toolchains, cargo_target_cap_gb, created_at, updated_at",
        )
        .bind(name)
        .bind(&input.git_url)
        .bind(&input.base_branch)
        .bind(input.default_workflow.as_deref())
        .bind(sqlx::types::Json(&input.toolchains))
        .bind(input.cargo_target_cap_gb)
        .fetch_one(&self.pool)
        .await?;
        Ok(row)
    }
    /// Set or clear (`None`) a project's build-cache cap. Returns the updated
    /// row, or `None` if the project doesn't exist.
    pub async fn set_cargo_target_cap(
        &self,
        name: &str,
        cap_gb: Option<i32>,
    ) -> Result<Option<Project>, PersistError> {
        let row = sqlx::query_as::<_, Project>(
            "UPDATE harness_projects SET cargo_target_cap_gb = $2, updated_at = now()
             WHERE name = $1
             RETURNING name, git_url, base_branch, default_workflow, toolchains,
                       cargo_target_cap_gb, created_at, updated_at",
        )
        .bind(name)
        .bind(cap_gb)
        .fetch_optional(&self.pool)
        .await?;
        Ok(row)
    }

    /// Remove a project from the registry (does not touch its checkout on disk).
    pub async fn delete(&self, name: &str) -> Result<(), PersistError> {
        sqlx::query("DELETE FROM harness_projects WHERE name = $1")
            .bind(name)
            .execute(&self.pool)
            .await?;
        Ok(())
    }
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
    async fn project_upsert_get_list_delete_round_trip() {
        let Some(url) = db_url() else {
            eprintln!("skipping: HARNESS_DATABASE_URL not set");
            return;
        };
        let store = ProjectStore::connect(&url).await.expect("connect");
        let name = format!(
            "test-proj-{}",
            chrono::Utc::now().timestamp_nanos_opt().unwrap()
        );

        let created = store
            .upsert(
                &name,
                &ProjectInput {
                    git_url: "https://github.com/me/ticket0.git".into(),
                    base_branch: "main".into(),
                    default_workflow: None,
                    toolchains: vec![],
                    cargo_target_cap_gb: None,
                },
            )
            .await
            .expect("upsert");
        assert_eq!(created.git_url, "https://github.com/me/ticket0.git");
        assert_eq!(created.base_branch, "main");
        assert!(created.toolchains.is_empty());
        assert_eq!(created.cargo_target_cap_gb, None);

        // Update changes fields + advances updated_at, preserves created_at.
        let updated = store
            .upsert(
                &name,
                &ProjectInput {
                    git_url: "https://github.com/me/ticket0.git".into(),
                    base_branch: "develop".into(),
                    default_workflow: Some("idea-to-pr".into()),
                    toolchains: vec!["rust".into(), "pnpm".into()],
                    cargo_target_cap_gb: None,
                },
            )
            .await
            .expect("update");
        assert_eq!(updated.base_branch, "develop");
        assert_eq!(updated.created_at, created.created_at);
        assert_eq!(updated.default_workflow.as_deref(), Some("idea-to-pr"));
        assert_eq!(updated.toolchains, vec!["rust", "pnpm"]);
        assert_eq!(updated.cargo_target_cap_gb, None);

        let with_cap = store
            .set_cargo_target_cap(&name, Some(20))
            .await
            .expect("set cap")
            .expect("project exists");
        assert_eq!(with_cap.cargo_target_cap_gb, Some(20));
        assert_eq!(
            store.get(&name).await.unwrap().unwrap().cargo_target_cap_gb,
            Some(20)
        );

        let cleared = store
            .set_cargo_target_cap(&name, None)
            .await
            .expect("clear cap")
            .expect("project exists");
        assert_eq!(cleared.cargo_target_cap_gb, None);

        assert!(store
            .list()
            .await
            .unwrap()
            .iter()
            .any(|p| p.name == name && p.cargo_target_cap_gb.is_none()));
        assert!(store.get(&name).await.unwrap().is_some());

        store.delete(&name).await.unwrap();
        assert!(store.get(&name).await.unwrap().is_none());
    }
}
