//! Built-in chat conversations and their messages.
//!
//! A conversation belongs to the person who started it (`user_id`; `NULL` on an
//! install without accounts, where there is nobody to scope by) and is pinned to
//! the agent that started it — a Claude session id means nothing to `omp`, so
//! changing the chat setting affects new conversations only.
//!
//! Messages are `user`, `assistant`, `action` or `error`. An `action` is a tool
//! call the agent asked for that must not run on its say-so (starting a run,
//! deleting a workflow): it waits in `action` with `status: pending` until the
//! person confirms or cancels it in the panel.

use chrono::{DateTime, Utc};
use serde::Serialize;
use serde_json::Value;
use sqlx::types::Json;
use sqlx::PgPool;

use crate::PersistError;

const CREATE_CONVERSATIONS: &str = "
CREATE TABLE IF NOT EXISTS harness_chat_conversations (
    id          text PRIMARY KEY,
    user_id     text,
    title       text NOT NULL DEFAULT '',
    provider    text NOT NULL,
    model       text,
    session_id  text,
    context     jsonb NOT NULL DEFAULT '{}'::jsonb,
    created_at  timestamptz NOT NULL DEFAULT now(),
    updated_at  timestamptz NOT NULL DEFAULT now()
)";

const INDEX_CONVERSATIONS_USER: &str = "
CREATE INDEX IF NOT EXISTS harness_chat_conversations_user
    ON harness_chat_conversations (user_id, updated_at DESC)";

const CREATE_MESSAGES: &str = "
CREATE TABLE IF NOT EXISTS harness_chat_messages (
    id               bigserial PRIMARY KEY,
    conversation_id  text NOT NULL REFERENCES harness_chat_conversations(id) ON DELETE CASCADE,
    role             text NOT NULL,
    body             text NOT NULL DEFAULT '',
    activity         jsonb NOT NULL DEFAULT '[]'::jsonb,
    action           jsonb,
    created_at       timestamptz NOT NULL DEFAULT now()
)";

const INDEX_MESSAGES_CONVERSATION: &str = "
CREATE INDEX IF NOT EXISTS harness_chat_messages_conversation
    ON harness_chat_messages (conversation_id, id)";

/// A conversation (matches `harness_chat_conversations`).
#[derive(Debug, Clone, Serialize, sqlx::FromRow)]
pub struct Conversation {
    pub id: String,
    pub user_id: Option<String>,
    /// The first message, shortened. Empty until there is one.
    pub title: String,
    /// The agent this conversation runs on, fixed at creation.
    pub provider: String,
    pub model: Option<String>,
    /// The agent session to resume; set after the first answer.
    #[serde(skip)]
    pub session_id: Option<String>,
    /// Where it was opened from (`{ run_id?, workflow?, project? }`).
    pub context: Json<Value>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

/// A message (matches `harness_chat_messages`).
#[derive(Debug, Clone, Serialize, sqlx::FromRow)]
pub struct ChatMessage {
    pub id: i64,
    pub conversation_id: String,
    /// `user` | `assistant` | `action` | `error`.
    pub role: String,
    pub body: String,
    /// What the agent did while answering (tool calls), for an assistant reply.
    pub activity: Json<Value>,
    /// For an `action`: `{ tool, args, status, result?, reported? }`.
    pub action: Option<Json<Value>>,
    pub created_at: DateTime<Utc>,
}

/// Postgres-backed store for chat conversations.
pub struct ChatStore {
    pool: PgPool,
}

const CONVERSATION_COLUMNS: &str =
    "id, user_id, title, provider, model, session_id, context, created_at, updated_at";
const MESSAGE_COLUMNS: &str = "id, conversation_id, role, body, activity, action, created_at";

impl ChatStore {
    /// Connect to `database_url` and ensure the schema exists.
    pub async fn connect(database_url: &str) -> Result<Self, PersistError> {
        let pool = sqlx::postgres::PgPoolOptions::new()
            .max_connections(3)
            .connect(database_url)
            .await?;
        Self::from_pool(pool).await
    }

    /// Wrap an existing pool; ensures the tables exist.
    pub async fn from_pool(pool: PgPool) -> Result<Self, PersistError> {
        sqlx::query(CREATE_CONVERSATIONS).execute(&pool).await?;
        sqlx::query(INDEX_CONVERSATIONS_USER).execute(&pool).await?;
        sqlx::query(CREATE_MESSAGES).execute(&pool).await?;
        sqlx::query(INDEX_MESSAGES_CONVERSATION)
            .execute(&pool)
            .await?;
        Ok(Self { pool })
    }

    /// Start a conversation.
    pub async fn create(
        &self,
        id: &str,
        user_id: Option<&str>,
        provider: &str,
        model: Option<&str>,
        context: &Value,
    ) -> Result<Conversation, PersistError> {
        let row = sqlx::query_as::<_, Conversation>(&format!(
            "INSERT INTO harness_chat_conversations (id, user_id, provider, model, context)
             VALUES ($1, $2, $3, $4, $5)
             RETURNING {CONVERSATION_COLUMNS}"
        ))
        .bind(id)
        .bind(user_id)
        .bind(provider)
        .bind(model)
        .bind(Json(context))
        .fetch_one(&self.pool)
        .await?;
        Ok(row)
    }

    /// One person's conversations, most recently active first. `None` lists
    /// the unowned ones, which is every conversation on an install without
    /// accounts.
    pub async fn list(
        &self,
        user_id: Option<&str>,
        limit: i64,
    ) -> Result<Vec<Conversation>, PersistError> {
        let rows = sqlx::query_as::<_, Conversation>(&format!(
            "SELECT {CONVERSATION_COLUMNS} FROM harness_chat_conversations
             WHERE user_id IS NOT DISTINCT FROM $1
             ORDER BY updated_at DESC
             LIMIT $2"
        ))
        .bind(user_id)
        .bind(limit)
        .fetch_all(&self.pool)
        .await?;
        Ok(rows)
    }

    pub async fn get(&self, id: &str) -> Result<Option<Conversation>, PersistError> {
        let row = sqlx::query_as::<_, Conversation>(&format!(
            "SELECT {CONVERSATION_COLUMNS} FROM harness_chat_conversations WHERE id = $1"
        ))
        .bind(id)
        .fetch_optional(&self.pool)
        .await?;
        Ok(row)
    }

    /// Delete a conversation and its messages. Returns whether it existed.
    pub async fn delete(&self, id: &str) -> Result<bool, PersistError> {
        let res = sqlx::query("DELETE FROM harness_chat_conversations WHERE id = $1")
            .bind(id)
            .execute(&self.pool)
            .await?;
        Ok(res.rows_affected() > 0)
    }

    /// A conversation's messages, oldest first.
    pub async fn messages(&self, conversation_id: &str) -> Result<Vec<ChatMessage>, PersistError> {
        let rows = sqlx::query_as::<_, ChatMessage>(&format!(
            "SELECT {MESSAGE_COLUMNS} FROM harness_chat_messages
             WHERE conversation_id = $1 ORDER BY id"
        ))
        .bind(conversation_id)
        .fetch_all(&self.pool)
        .await?;
        Ok(rows)
    }

    /// One message, by id.
    pub async fn message(&self, id: i64) -> Result<Option<ChatMessage>, PersistError> {
        let row = sqlx::query_as::<_, ChatMessage>(&format!(
            "SELECT {MESSAGE_COLUMNS} FROM harness_chat_messages WHERE id = $1"
        ))
        .bind(id)
        .fetch_optional(&self.pool)
        .await?;
        Ok(row)
    }

    /// Append a message and mark the conversation active.
    pub async fn add_message(
        &self,
        conversation_id: &str,
        role: &str,
        body: &str,
        activity: &Value,
        action: Option<&Value>,
    ) -> Result<ChatMessage, PersistError> {
        let mut tx = self.pool.begin().await?;
        let row = sqlx::query_as::<_, ChatMessage>(&format!(
            "INSERT INTO harness_chat_messages (conversation_id, role, body, activity, action)
             VALUES ($1, $2, $3, $4, $5)
             RETURNING {MESSAGE_COLUMNS}"
        ))
        .bind(conversation_id)
        .bind(role)
        .bind(body)
        .bind(Json(activity))
        .bind(action.map(Json))
        .fetch_one(&mut *tx)
        .await?;
        sqlx::query("UPDATE harness_chat_conversations SET updated_at = now() WHERE id = $1")
            .bind(conversation_id)
            .execute(&mut *tx)
            .await?;
        tx.commit().await?;
        Ok(row)
    }

    /// Replace an action message's `action`.
    pub async fn set_action(&self, message_id: i64, action: &Value) -> Result<(), PersistError> {
        sqlx::query("UPDATE harness_chat_messages SET action = $2 WHERE id = $1")
            .bind(message_id)
            .bind(Json(action))
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    /// Record the agent session to resume next turn.
    pub async fn set_session(&self, id: &str, session_id: &str) -> Result<(), PersistError> {
        sqlx::query("UPDATE harness_chat_conversations SET session_id = $2 WHERE id = $1")
            .bind(id)
            .bind(session_id)
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    /// Replace a conversation's context (e.g. to pin the commits it reads code at).
    pub async fn set_context(&self, id: &str, context: &Value) -> Result<(), PersistError> {
        sqlx::query("UPDATE harness_chat_conversations SET context = $2 WHERE id = $1")
            .bind(id)
            .bind(Json(context))
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    /// Name a conversation after its first message, once.
    pub async fn set_title_if_empty(&self, id: &str, title: &str) -> Result<(), PersistError> {
        sqlx::query(
            "UPDATE harness_chat_conversations SET title = $2 WHERE id = $1 AND title = ''",
        )
        .bind(id)
        .bind(title)
        .execute(&self.pool)
        .await?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn db_url() -> Option<String> {
        let url = std::env::var("HARNESS_DATABASE_URL").ok()?;
        crate::is_test_db(&url).then_some(url)
    }

    /// The lifecycle the chat routes rely on: create, talk, hold an action,
    /// resolve it, list per owner, and delete with its messages.
    #[tokio::test]
    #[serial_test::serial]
    async fn a_conversation_round_trips() {
        let Some(url) = db_url() else {
            eprintln!("skipping: HARNESS_DATABASE_URL not set");
            return;
        };
        let store = ChatStore::connect(&url).await.expect("connect");
        let ts = chrono::Utc::now().timestamp_nanos_opt().unwrap();
        let (id, user) = (format!("chat-{ts}"), format!("user-{ts}"));

        let conv = store
            .create(
                &id,
                Some(&user),
                "claude",
                Some("sonnet"),
                &json!({"run_id": "r1"}),
            )
            .await
            .unwrap();
        assert_eq!(conv.title, "");
        assert_eq!(conv.context.0["run_id"], "r1");

        store
            .add_message(&id, "user", "which PR?", &json!([]), None)
            .await
            .unwrap();
        let action = store
            .add_message(
                &id,
                "action",
                "",
                &json!([]),
                Some(&json!({"tool": "run_trigger", "status": "pending"})),
            )
            .await
            .unwrap();
        store
            .set_action(
                action.id,
                &json!({"tool": "run_trigger", "status": "confirmed"}),
            )
            .await
            .unwrap();
        store.set_session(&id, "sess-1").await.unwrap();
        store.set_title_if_empty(&id, "which PR?").await.unwrap();
        store
            .set_title_if_empty(&id, "something later")
            .await
            .unwrap();

        let got = store.get(&id).await.unwrap().unwrap();
        assert_eq!(got.title, "which PR?");
        assert_eq!(got.session_id.as_deref(), Some("sess-1"));
        let msgs = store.messages(&id).await.unwrap();
        assert_eq!(msgs.len(), 2);
        assert_eq!(msgs[1].action.as_ref().unwrap().0["status"], "confirmed");

        // Listed for its owner, not for somebody else.
        assert!(store
            .list(Some(&user), 50)
            .await
            .unwrap()
            .iter()
            .any(|c| c.id == id));
        assert!(!store
            .list(Some("someone-else"), 50)
            .await
            .unwrap()
            .iter()
            .any(|c| c.id == id));

        assert!(store.delete(&id).await.unwrap());
        assert!(store.messages(&id).await.unwrap().is_empty());
        assert!(store.message(action.id).await.unwrap().is_none());
    }
}
