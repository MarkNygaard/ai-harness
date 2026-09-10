//! Which library workflows this harness has installed.
//!
//! An installed workflow is a **copy**: the YAML is written to
//! `.harness/workflows/<name>.yaml` like any other, and nothing ever changes it
//! underneath a running install. That is the whole point of the design, and it
//! is also why this table has to exist — once the file is on disk it is
//! indistinguishable from one somebody hand-wrote, so the fact that it came from
//! the library, which version it was, and what it is called upstream all live
//! here or nowhere.
//!
//! Without it there is no Update button (nothing to compare against), no way to
//! separate installed workflows from authored ones in the editor, and no way to
//! tell the registry that a workflow was removed.

use chrono::{DateTime, Utc};
use serde::Serialize;
use sqlx::postgres::{PgPool, PgPoolOptions};

use crate::PersistError;

const CREATE_INSTALLED: &str = "
CREATE TABLE IF NOT EXISTS harness_installed_workflows (
    -- The local file stem, which is what the editor and every run name. The
    -- primary key because that is what must be unique on disk: two library
    -- workflows cannot occupy one file.
    name         text PRIMARY KEY,
    -- The registry's own identifier, which is NOT the local name: a collision
    -- with an existing workflow is resolved at install time by choosing a
    -- different file name, and the link back to the library has to survive that.
    slug         text NOT NULL,
    version      integer NOT NULL,
    -- Who published it, for display beside the workflow without asking the
    -- registry again — and so it still reads correctly when the registry is
    -- unreachable or the entry has since been unlisted.
    publisher    text,
    title        text,
    installed_at timestamptz NOT NULL DEFAULT now(),
    updated_at   timestamptz NOT NULL DEFAULT now()
)";

/// A library workflow this harness has installed.
#[derive(Debug, Clone, Serialize, sqlx::FromRow)]
pub struct InstalledWorkflow {
    /// Local file stem under `.harness/workflows/`.
    pub name: String,
    /// The registry's identifier for it.
    pub slug: String,
    /// The version currently on disk. Compared against the registry's latest to
    /// decide whether an update is offered.
    pub version: i32,
    pub publisher: Option<String>,
    pub title: Option<String>,
    pub installed_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

/// What an install or update records.
#[derive(Debug, Clone, Copy)]
pub struct InstallRecord<'a> {
    pub name: &'a str,
    pub slug: &'a str,
    pub version: i32,
    pub publisher: Option<&'a str>,
    pub title: Option<&'a str>,
}

/// Reads and writes [`InstalledWorkflow`].
#[derive(Clone)]
pub struct InstalledWorkflowStore {
    pool: PgPool,
}

impl InstalledWorkflowStore {
    pub async fn connect(database_url: &str) -> Result<Self, PersistError> {
        let pool = PgPoolOptions::new()
            .max_connections(3)
            .connect(database_url)
            .await?;
        Self::from_pool(pool).await
    }

    pub async fn from_pool(pool: PgPool) -> Result<Self, PersistError> {
        let store = Self { pool };
        sqlx::query(CREATE_INSTALLED).execute(&store.pool).await?;
        Ok(store)
    }

    /// Record an install, or move an existing one to a new version.
    ///
    /// `installed_at` is preserved across updates — it answers "since when has
    /// this been here", which an update does not change.
    pub async fn record(&self, install: &InstallRecord<'_>) -> Result<(), PersistError> {
        sqlx::query(
            "INSERT INTO harness_installed_workflows
                 (name, slug, version, publisher, title)
             VALUES ($1, $2, $3, $4, $5)
             ON CONFLICT (name) DO UPDATE SET
                slug       = excluded.slug,
                version    = excluded.version,
                publisher  = excluded.publisher,
                title      = excluded.title,
                updated_at = now()",
        )
        .bind(install.name)
        .bind(install.slug)
        .bind(install.version)
        .bind(install.publisher)
        .bind(install.title)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    /// Forget one, for when the workflow is uninstalled or deleted.
    ///
    /// Returns whether a row was there. The caller needs to know: a workflow
    /// deleted through the editor rather than the library leaves this row
    /// behind, and reporting the uninstall to the registry should only happen
    /// when there was genuinely something installed.
    pub async fn forget(&self, name: &str) -> Result<bool, PersistError> {
        let done = sqlx::query("DELETE FROM harness_installed_workflows WHERE name = $1")
            .bind(name)
            .execute(&self.pool)
            .await?;
        Ok(done.rows_affected() > 0)
    }

    /// Everything installed, for annotating a library listing and for grouping
    /// the editor's workflow list.
    pub async fn all(&self) -> Result<Vec<InstalledWorkflow>, PersistError> {
        Ok(sqlx::query_as::<_, InstalledWorkflow>(
            "SELECT name, slug, version, publisher, title, installed_at, updated_at
             FROM harness_installed_workflows ORDER BY name",
        )
        .fetch_all(&self.pool)
        .await?)
    }

    /// One by local name, or `None` when that workflow was not installed from
    /// the library.
    pub async fn get(&self, name: &str) -> Result<Option<InstalledWorkflow>, PersistError> {
        Ok(sqlx::query_as::<_, InstalledWorkflow>(
            "SELECT name, slug, version, publisher, title, installed_at, updated_at
             FROM harness_installed_workflows WHERE name = $1",
        )
        .bind(name)
        .fetch_optional(&self.pool)
        .await?)
    }

    /// One by registry slug — the direction the Library dialog asks in, since it
    /// is showing registry entries and needs to know which are already here.
    pub async fn by_slug(&self, slug: &str) -> Result<Option<InstalledWorkflow>, PersistError> {
        Ok(sqlx::query_as::<_, InstalledWorkflow>(
            "SELECT name, slug, version, publisher, title, installed_at, updated_at
             FROM harness_installed_workflows WHERE slug = $1",
        )
        .bind(slug)
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

    #[tokio::test]
    #[serial_test::serial]
    async fn an_update_moves_the_version_and_keeps_the_install_date() {
        let Some(url) = db_url() else {
            eprintln!("skipping: HARNESS_DATABASE_URL not set");
            return;
        };
        let store = InstalledWorkflowStore::connect(&url)
            .await
            .expect("connect");
        let name = format!(
            "geo-audit-{}",
            chrono::Utc::now().timestamp_nanos_opt().unwrap()
        );

        store
            .record(&InstallRecord {
                name: &name,
                slug: "geo-audit-ecommerce",
                version: 1,
                publisher: Some("marknygaard"),
                title: Some("GEO Audit — Ecommerce"),
            })
            .await
            .unwrap();
        let first = store.get(&name).await.unwrap().expect("installed");
        assert_eq!(first.version, 1);

        store
            .record(&InstallRecord {
                name: &name,
                slug: "geo-audit-ecommerce",
                version: 4,
                publisher: Some("marknygaard"),
                title: Some("GEO Audit — Ecommerce"),
            })
            .await
            .unwrap();
        let updated = store.get(&name).await.unwrap().expect("installed");
        assert_eq!(updated.version, 4);
        // "Since when has this been here" is not changed by an update.
        assert_eq!(updated.installed_at, first.installed_at);
        assert!(updated.updated_at >= first.updated_at);

        // Found by slug too — the direction the library listing asks in.
        assert_eq!(
            store
                .by_slug("geo-audit-ecommerce")
                .await
                .unwrap()
                .map(|w| w.name),
            Some(name.clone())
        );

        assert!(store.forget(&name).await.unwrap());
        // A second uninstall found nothing, and says so rather than claiming a
        // deletion — the caller uses this to decide whether to tell the registry.
        assert!(!store.forget(&name).await.unwrap());
        assert!(store.get(&name).await.unwrap().is_none());
    }
}
