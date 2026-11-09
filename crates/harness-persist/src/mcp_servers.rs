//! The **MCP server registry**: servers an admin added under Settings → MCP
//! servers, for workflow steps to ask for with `mcp:`.
//!
//! A row holds what is safe to show: how the server is reached, which tools it
//! offers (as last listed), which of them steps may use, and how its last
//! connection went. Anything secret — a token in an environment variable or a
//! header, OAuth client and tokens — lives in the encrypted
//! [`crate::CredentialStore`] under [`credential_key`], and only its key names
//! are listed here.
//!
//! Each server is connected **once for the whole install**, with a shared
//! service account, never per person: most runs have nobody behind them (the
//! Linear poller, schedules), so there is no person whose connection a run
//! could borrow.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sqlx::types::Json;
use sqlx::PgPool;

use crate::PersistError;

const CREATE_MCP_SERVERS: &str = "
CREATE TABLE IF NOT EXISTS harness_mcp_servers (
    name               text PRIMARY KEY,
    label              text NOT NULL DEFAULT '',
    transport          text NOT NULL,
    command            text NOT NULL DEFAULT '',
    args               jsonb NOT NULL DEFAULT '[]',
    url                text NOT NULL DEFAULT '',
    auth               text NOT NULL DEFAULT 'none',
    secret_keys        jsonb NOT NULL DEFAULT '[]',
    tools              jsonb NOT NULL DEFAULT '[]',
    enabled_tools      jsonb NOT NULL DEFAULT '[]',
    tools_refreshed_at timestamptz,
    last_error         text NOT NULL DEFAULT '',
    created_at         timestamptz NOT NULL DEFAULT now(),
    updated_at         timestamptz NOT NULL DEFAULT now()
)";

/// The credential-store provider key holding `name`'s secrets.
pub fn credential_key(name: &str) -> String {
    format!("mcp:{name}")
}

/// A tool a server offers, as its `tools/list` last reported it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct McpTool {
    pub name: String,
    #[serde(default)]
    pub description: String,
    /// The server's own `readOnlyHint`. A tool that doesn't say is treated as
    /// one that writes — new servers start with only read-only tools enabled.
    #[serde(default)]
    pub read_only: bool,
}

/// One registered server (matches `harness_mcp_servers`).
#[derive(Debug, Clone, Serialize, Deserialize, sqlx::FromRow)]
pub struct McpServer {
    /// The name steps use (`mcp: [name]`), and the prefix of the agent's tool
    /// names. Never changes once created.
    pub name: String,
    pub label: String,
    /// `stdio` (a command the harness starts) or `http` (a remote URL).
    pub transport: String,
    /// For `stdio`.
    pub command: String,
    pub args: Json<Vec<String>>,
    /// For `http`.
    pub url: String,
    /// `none`, `secret` (values in [`Self::secret_keys`]) or `oauth`.
    pub auth: String,
    /// The names of the secret values: environment variables for `stdio`,
    /// request headers for `http`. The values are in the credential store.
    pub secret_keys: Json<Vec<String>>,
    pub tools: Json<Vec<McpTool>>,
    /// The tools steps may use. A step can narrow this list, never widen it.
    pub enabled_tools: Json<Vec<String>>,
    pub tools_refreshed_at: Option<DateTime<Utc>>,
    /// Why the last connection failed, `""` when it worked.
    pub last_error: String,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

const COLUMNS: &str = "name, label, transport, command, args, url, auth, secret_keys, tools, \
                       enabled_tools, tools_refreshed_at, last_error, created_at, updated_at";

/// Postgres-backed MCP server registry.
pub struct McpServerStore {
    pool: PgPool,
}

impl McpServerStore {
    pub async fn connect(database_url: &str) -> Result<Self, PersistError> {
        let pool = sqlx::postgres::PgPoolOptions::new()
            .max_connections(2)
            .connect(database_url)
            .await?;
        Self::from_pool(pool).await
    }

    pub async fn from_pool(pool: PgPool) -> Result<Self, PersistError> {
        sqlx::query(CREATE_MCP_SERVERS).execute(&pool).await?;
        Ok(Self { pool })
    }

    pub async fn list(&self) -> Result<Vec<McpServer>, PersistError> {
        Ok(sqlx::query_as::<_, McpServer>(&format!(
            "SELECT {COLUMNS} FROM harness_mcp_servers ORDER BY name"
        ))
        .fetch_all(&self.pool)
        .await?)
    }

    pub async fn get(&self, name: &str) -> Result<Option<McpServer>, PersistError> {
        Ok(sqlx::query_as::<_, McpServer>(&format!(
            "SELECT {COLUMNS} FROM harness_mcp_servers WHERE name = $1"
        ))
        .bind(name)
        .fetch_optional(&self.pool)
        .await?)
    }

    /// Insert or replace `server` by name. `created_at` is kept on replace.
    pub async fn save(&self, server: &McpServer) -> Result<McpServer, PersistError> {
        Ok(sqlx::query_as::<_, McpServer>(&format!(
            "INSERT INTO harness_mcp_servers
                (name, label, transport, command, args, url, auth, secret_keys, tools,
                 enabled_tools, tools_refreshed_at, last_error)
             VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12)
             ON CONFLICT (name) DO UPDATE SET
                label = EXCLUDED.label,
                transport = EXCLUDED.transport,
                command = EXCLUDED.command,
                args = EXCLUDED.args,
                url = EXCLUDED.url,
                auth = EXCLUDED.auth,
                secret_keys = EXCLUDED.secret_keys,
                tools = EXCLUDED.tools,
                enabled_tools = EXCLUDED.enabled_tools,
                tools_refreshed_at = EXCLUDED.tools_refreshed_at,
                last_error = EXCLUDED.last_error,
                updated_at = now()
             RETURNING {COLUMNS}"
        ))
        .bind(&server.name)
        .bind(&server.label)
        .bind(&server.transport)
        .bind(&server.command)
        .bind(&server.args)
        .bind(&server.url)
        .bind(&server.auth)
        .bind(&server.secret_keys)
        .bind(&server.tools)
        .bind(&server.enabled_tools)
        .bind(server.tools_refreshed_at)
        .bind(&server.last_error)
        .fetch_one(&self.pool)
        .await?)
    }

    /// Record how the last connection went, without touching anything else.
    pub async fn set_error(&self, name: &str, error: &str) -> Result<(), PersistError> {
        sqlx::query(
            "UPDATE harness_mcp_servers SET last_error = $2, updated_at = now() WHERE name = $1",
        )
        .bind(name)
        .bind(error)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    pub async fn delete(&self, name: &str) -> Result<bool, PersistError> {
        let done = sqlx::query("DELETE FROM harness_mcp_servers WHERE name = $1")
            .bind(name)
            .execute(&self.pool)
            .await?;
        Ok(done.rows_affected() > 0)
    }
}

impl McpServer {
    /// A new, unconnected server: nothing listed, nothing enabled.
    pub fn new(name: &str, transport: &str) -> Self {
        let now = Utc::now();
        Self {
            name: name.to_string(),
            label: String::new(),
            transport: transport.to_string(),
            command: String::new(),
            args: Json(Vec::new()),
            url: String::new(),
            auth: "none".to_string(),
            secret_keys: Json(Vec::new()),
            tools: Json(Vec::new()),
            enabled_tools: Json(Vec::new()),
            tools_refreshed_at: None,
            last_error: String::new(),
            created_at: now,
            updated_at: now,
        }
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
    async fn a_server_is_saved_updated_listed_and_deleted() {
        let Some(url) = db_url() else {
            eprintln!("skipping: HARNESS_DATABASE_URL not set");
            return;
        };
        let store = McpServerStore::connect(&url).await.expect("connect");
        let name = format!("srv-{}", Utc::now().timestamp_nanos_opt().unwrap());
        let mut server = McpServer::new(&name, "http");
        server.url = "https://mcp.example.com/mcp".into();
        server.auth = "oauth".into();
        let saved = store.save(&server).await.unwrap();
        assert_eq!(saved.url, server.url);
        assert!(saved.tools.0.is_empty());

        server.tools = Json(vec![McpTool {
            name: "search".into(),
            description: "Search".into(),
            read_only: true,
        }]);
        server.enabled_tools = Json(vec!["search".into()]);
        let updated = store.save(&server).await.unwrap();
        assert_eq!(updated.created_at, saved.created_at);
        assert_eq!(updated.enabled_tools.0, vec!["search".to_string()]);

        store.set_error(&name, "401 Unauthorized").await.unwrap();
        let got = store.get(&name).await.unwrap().unwrap();
        assert_eq!(got.last_error, "401 Unauthorized");
        assert_eq!(got.tools.0.len(), 1, "set_error leaves the rest alone");
        assert!(store.list().await.unwrap().iter().any(|s| s.name == name));

        assert!(store.delete(&name).await.unwrap());
        assert!(store.get(&name).await.unwrap().is_none());
        assert!(!store.delete(&name).await.unwrap());
    }
}
