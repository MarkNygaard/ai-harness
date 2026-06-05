//! Workflow step **category** registry.
//!
//! A category (e.g. `planning`, `implementation`, `validation`) groups workflow
//! steps for the run overview's time-by-category breakdown and bar colouring. A
//! node references a category by `id`; the colour is resolved from this registry
//! at render time, so recolouring a category updates every overview at once.
//!
//! Global (not project-scoped) to start. Seeded with three sensible defaults in
//! muted tones matching the overview's tokens-by-type palette; users add more.

use chrono::{DateTime, Utc};
use serde::Serialize;
use sqlx::postgres::{PgPool, PgPoolOptions};

use crate::PersistError;

const CREATE_CATEGORIES: &str = "
CREATE TABLE IF NOT EXISTS harness_categories (
    id          text PRIMARY KEY,
    label       text NOT NULL,
    color       text NOT NULL,
    ordinal     integer NOT NULL DEFAULT 0,
    created_at  timestamptz NOT NULL DEFAULT now(),
    updated_at  timestamptz NOT NULL DEFAULT now()
)";

/// Built-in categories seeded on first connect — the phases of the default
/// `idea-to-pr` pipeline. Muted, well-separated hues (Factory-style). Ordered to
/// read as the pipeline flows; users add/recolour/reorder freely in the UI.
const SEED: &[(&str, &str, &str, i32)] = &[
    ("planning", "Planning", "oklch(0.64 0.07 200)", 0), // teal
    ("setup", "Setup", "oklch(0.70 0.03 250)", 1),       // slate
    (
        "implementation",
        "Implementation",
        "oklch(0.75 0.09 130)", // olive-green
        2,
    ),
    ("validation", "Validation", "oklch(0.76 0.09 70)", 3), // amber
    ("review", "Review", "oklch(0.68 0.10 300)", 4),        // violet
    ("delivery", "Delivery", "oklch(0.70 0.10 20)", 5),     // rose
];

/// A category (matches `harness_categories`).
#[derive(Debug, Clone, Serialize, sqlx::FromRow)]
pub struct Category {
    /// Slug referenced by a node's `category` field.
    pub id: String,
    pub label: String,
    /// CSS colour (e.g. an `oklch(...)` or hex string).
    pub color: String,
    /// Sort order in lists / the legend.
    pub ordinal: i32,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

/// Fields accepted when creating / updating a category.
#[derive(Debug, Clone)]
pub struct CategoryInput {
    pub label: String,
    pub color: String,
    pub ordinal: i32,
}

/// Postgres-backed registry of step categories.
pub struct CategoryStore {
    pool: PgPool,
}

impl CategoryStore {
    /// Connect to `database_url`, ensure the schema exists, and seed defaults.
    pub async fn connect(database_url: &str) -> Result<Self, PersistError> {
        let pool = PgPoolOptions::new()
            .max_connections(3)
            .connect(database_url)
            .await?;
        Self::from_pool(pool).await
    }

    /// Wrap an existing pool; ensures the table exists and the defaults are
    /// present (seed is idempotent — it never overwrites an edited category).
    pub async fn from_pool(pool: PgPool) -> Result<Self, PersistError> {
        let store = Self { pool };
        sqlx::query(CREATE_CATEGORIES).execute(&store.pool).await?;
        for (id, label, color, ordinal) in SEED {
            sqlx::query(
                "INSERT INTO harness_categories (id, label, color, ordinal)
                 VALUES ($1, $2, $3, $4) ON CONFLICT (id) DO NOTHING",
            )
            .bind(id)
            .bind(label)
            .bind(color)
            .bind(ordinal)
            .execute(&store.pool)
            .await?;
        }
        Ok(store)
    }

    /// All categories, by ordinal then id.
    pub async fn list(&self) -> Result<Vec<Category>, PersistError> {
        let rows = sqlx::query_as::<_, Category>(
            "SELECT id, label, color, ordinal, created_at, updated_at
             FROM harness_categories ORDER BY ordinal, id",
        )
        .fetch_all(&self.pool)
        .await?;
        Ok(rows)
    }

    /// Create or update a category (upsert on `id`). `created_at` is preserved.
    pub async fn upsert(&self, id: &str, input: &CategoryInput) -> Result<Category, PersistError> {
        let row = sqlx::query_as::<_, Category>(
            "INSERT INTO harness_categories (id, label, color, ordinal, created_at, updated_at)
             VALUES ($1, $2, $3, $4, now(), now())
             ON CONFLICT (id) DO UPDATE SET
                label   = excluded.label,
                color   = excluded.color,
                ordinal = excluded.ordinal,
                updated_at = now()
             RETURNING id, label, color, ordinal, created_at, updated_at",
        )
        .bind(id)
        .bind(&input.label)
        .bind(&input.color)
        .bind(input.ordinal)
        .fetch_one(&self.pool)
        .await?;
        Ok(row)
    }

    /// Remove a category. Nodes still referencing it fall back to status colour.
    pub async fn delete(&self, id: &str) -> Result<(), PersistError> {
        sqlx::query("DELETE FROM harness_categories WHERE id = $1")
            .bind(id)
            .execute(&self.pool)
            .await?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn db_url() -> Option<String> {
        std::env::var("HARNESS_DATABASE_URL").ok()
    }

    #[tokio::test]
    #[serial_test::serial]
    async fn seeds_defaults_and_round_trips_custom() {
        let Some(url) = db_url() else {
            eprintln!("skipping: HARNESS_DATABASE_URL not set");
            return;
        };
        let store = CategoryStore::connect(&url).await.expect("connect");

        // Defaults are present after connect.
        let listed = store.list().await.unwrap();
        for id in ["planning", "implementation", "validation"] {
            assert!(listed.iter().any(|c| c.id == id), "missing default {id}");
        }

        // Create + update a custom category.
        let id = format!(
            "test-cat-{}",
            chrono::Utc::now().timestamp_nanos_opt().unwrap()
        );
        let created = store
            .upsert(
                &id,
                &CategoryInput {
                    label: "Research".into(),
                    color: "oklch(0.7 0.05 300)".into(),
                    ordinal: 9,
                },
            )
            .await
            .unwrap();
        assert_eq!(created.label, "Research");

        let updated = store
            .upsert(
                &id,
                &CategoryInput {
                    label: "Research+".into(),
                    color: "oklch(0.7 0.05 300)".into(),
                    ordinal: 9,
                },
            )
            .await
            .unwrap();
        assert_eq!(updated.label, "Research+");
        assert_eq!(updated.created_at, created.created_at);

        store.delete(&id).await.unwrap();
        assert!(!store.list().await.unwrap().iter().any(|c| c.id == id));
    }
}
