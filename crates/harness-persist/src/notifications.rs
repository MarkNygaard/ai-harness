//! Notifications a person asked for, and the inbox they land in.
//!
//! **Nothing is on by default.** A notification exists only because somebody
//! subscribed to it: a subscription names which events (a run failed, a
//! trigger could not start a run, …), what they are about (one trigger, a
//! workflow, a project, or the runs the person started) and where they go.
//! The harness matches every event against the subscriptions and writes one
//! inbox row per person it concerns.
//!
//! **One notification per person per thing.** A run can be seen ending more
//! than once (the executor records it, a cancel races it), and a person can
//! hold two subscriptions that both match. `dedupe_key` is unique per person,
//! so the second write is a no-op rather than a second ping.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sqlx::types::Json;
use sqlx::PgPool;

use crate::PersistError;

/// A run ended `failed`.
pub const EVENT_RUN_FAILED: &str = "run_failed";
/// A run ended `cancelled`: by a person, or by the server reaping a run whose
/// executor went away.
pub const EVENT_RUN_CANCELLED: &str = "run_cancelled";
/// A run ended `completed`.
pub const EVENT_RUN_COMPLETED: &str = "run_completed";
/// A trigger or schedule fired but could not start a run.
pub const EVENT_TRIGGER_FAILED: &str = "trigger_failed";
pub const EVENTS: &[&str] = &[
    EVENT_RUN_FAILED,
    EVENT_RUN_CANCELLED,
    EVENT_RUN_COMPLETED,
    EVENT_TRIGGER_FAILED,
];

/// One trigger or schedule, by id.
pub const SCOPE_TRIGGER: &str = "trigger";
/// Every run of a workflow, in any project, and every trigger that starts it.
pub const SCOPE_WORKFLOW: &str = "workflow";
/// Everything in a project.
pub const SCOPE_PROJECT: &str = "project";
/// The runs this person started, and the triggers they created.
pub const SCOPE_MINE: &str = "mine";
pub const SCOPES: &[&str] = &[SCOPE_TRIGGER, SCOPE_WORKFLOW, SCOPE_PROJECT, SCOPE_MINE];

/// The inbox in the web UI. Every subscription writes there.
pub const CHANNEL_IN_APP: &str = "in_app";
/// An email to the person's account address.
pub const CHANNEL_EMAIL: &str = "email";
/// A Slack direct message, to the Slack account with the person's address.
pub const CHANNEL_SLACK_DM: &str = "slack_dm";
/// A post in a Slack channel, named on the subscription.
pub const CHANNEL_SLACK_CHANNEL: &str = "slack_channel";
pub const CHANNELS: &[&str] = &[
    CHANNEL_IN_APP,
    CHANNEL_EMAIL,
    CHANNEL_SLACK_DM,
    CHANNEL_SLACK_CHANNEL,
];

const CREATE_SUBSCRIPTIONS: &str = "
CREATE TABLE IF NOT EXISTS harness_notification_subscriptions (
    id          text PRIMARY KEY,
    user_id     text NOT NULL REFERENCES harness_users(id) ON DELETE CASCADE,
    -- Which events: a JSON array of EVENTS.
    events      jsonb NOT NULL,
    -- What they are about: one of SCOPES, and its value (a trigger id, a
    -- workflow or project name; empty for 'mine').
    scope_kind  text NOT NULL,
    scope_value text NOT NULL DEFAULT '',
    -- Where they go: a JSON array of CHANNELS.
    channels    jsonb NOT NULL,
    created_at  timestamptz NOT NULL DEFAULT now()
)";

const CREATE_SUBSCRIPTIONS_USER_IDX: &str = "CREATE INDEX IF NOT EXISTS \
    harness_notification_subscriptions_user_idx ON harness_notification_subscriptions (user_id)";

const CREATE_NOTIFICATIONS: &str = "
CREATE TABLE IF NOT EXISTS harness_notifications (
    id              bigserial PRIMARY KEY,
    user_id         text NOT NULL REFERENCES harness_users(id) ON DELETE CASCADE,
    -- The subscription that matched. Kept when it is deleted, as history.
    subscription_id text,
    event           text NOT NULL,
    title           text NOT NULL,
    body            text NOT NULL DEFAULT '',
    -- Where in the web UI it points: '/runs/<id>', '/triggers/<id>', …
    link            text,
    run_id          text,
    trigger_id      text,
    project         text,
    -- The same thing is never announced to the same person twice.
    dedupe_key      text NOT NULL,
    created_at      timestamptz NOT NULL DEFAULT now(),
    read_at         timestamptz
)";

const CREATE_NOTIFICATIONS_DEDUPE_IDX: &str = "CREATE UNIQUE INDEX IF NOT EXISTS \
    harness_notifications_dedupe_idx ON harness_notifications (user_id, dedupe_key)";

const CREATE_NOTIFICATIONS_INBOX_IDX: &str = "CREATE INDEX IF NOT EXISTS \
    harness_notifications_inbox_idx ON harness_notifications (user_id, created_at DESC)";

/// The Slack channel a `slack_channel` subscription posts to.
const ADD_SUBSCRIPTION_SLACK_CHANNEL: &str = "ALTER TABLE harness_notification_subscriptions \
    ADD COLUMN IF NOT EXISTS slack_channel text";

/// Why a notification did not reach somewhere it should have (email, Slack).
const ADD_NOTIFICATION_DELIVERY_ERROR: &str =
    "ALTER TABLE harness_notifications ADD COLUMN IF NOT EXISTS delivery_error text";

/// A post to a shared channel, once per thing however many people route it
/// there.
const CREATE_CHANNEL_POSTS: &str = "
CREATE TABLE IF NOT EXISTS harness_notification_channel_posts (
    channel    text NOT NULL,
    dedupe_key text NOT NULL,
    created_at timestamptz NOT NULL DEFAULT now(),
    PRIMARY KEY (channel, dedupe_key)
)";

/// A standing request to be told about something.
#[derive(Debug, Clone, Serialize, Deserialize, sqlx::FromRow)]
pub struct NotificationSubscription {
    pub id: String,
    pub user_id: String,
    pub events: Json<Vec<String>>,
    pub scope_kind: String,
    pub scope_value: String,
    pub channels: Json<Vec<String>>,
    /// Where a `slack_channel` subscription posts: a channel name or id.
    pub slack_channel: Option<String>,
    pub created_at: DateTime<Utc>,
}

/// What a person submits to subscribe.
#[derive(Debug, Clone, Deserialize)]
pub struct SubscriptionInput {
    pub events: Vec<String>,
    pub scope_kind: String,
    #[serde(default)]
    pub scope_value: String,
    #[serde(default)]
    pub channels: Vec<String>,
    /// Required with the `slack_channel` channel; ignored otherwise.
    #[serde(default)]
    pub slack_channel: Option<String>,
}

impl SubscriptionInput {
    /// Check and normalise: known events, a known scope with the value it
    /// needs, known channels. The in-app inbox is always one of them.
    pub fn validated(mut self) -> Result<Self, String> {
        self.events.retain(|e| !e.trim().is_empty());
        self.events.sort();
        self.events.dedup();
        if self.events.is_empty() {
            return Err("choose at least one event".into());
        }
        if let Some(bad) = self.events.iter().find(|e| !EVENTS.contains(&e.as_str())) {
            return Err(format!("unknown event `{bad}`"));
        }
        if !SCOPES.contains(&self.scope_kind.as_str()) {
            return Err(format!("unknown scope `{}`", self.scope_kind));
        }
        self.scope_value = self.scope_value.trim().to_string();
        if self.scope_kind == SCOPE_MINE {
            self.scope_value.clear();
        } else if self.scope_value.is_empty() {
            return Err(format!(
                "a `{}` subscription needs a value",
                self.scope_kind
            ));
        }
        if !self.channels.iter().any(|c| c == CHANNEL_IN_APP) {
            self.channels.push(CHANNEL_IN_APP.to_string());
        }
        self.channels.sort();
        self.channels.dedup();
        if let Some(bad) = self
            .channels
            .iter()
            .find(|c| !CHANNELS.contains(&c.as_str()))
        {
            return Err(format!("unknown channel `{bad}`"));
        }
        // `#builds` and `builds` are the same channel; keep one spelling.
        self.slack_channel = self
            .slack_channel
            .as_deref()
            .map(|c| c.trim().trim_start_matches('#').trim().to_string())
            .filter(|c| !c.is_empty());
        if self.channels.iter().any(|c| c == CHANNEL_SLACK_CHANNEL) {
            match &self.slack_channel {
                None => return Err("name the Slack channel to post to".into()),
                Some(c) if c.chars().any(char::is_whitespace) => {
                    return Err(format!("`{c}` is not a Slack channel name"))
                }
                Some(_) => {}
            }
        } else {
            self.slack_channel = None;
        }
        Ok(self)
    }
}

/// What an event is about, for matching against subscriptions. A field that
/// does not apply is `None` and matches nothing.
#[derive(Debug, Clone, Copy, Default)]
pub struct EventSubject<'a> {
    pub trigger_id: Option<&'a str>,
    pub workflow: Option<&'a str>,
    pub project: Option<&'a str>,
    /// The person the event belongs to under the `mine` scope: who started
    /// the run, or who created the trigger.
    pub owner: Option<&'a str>,
}

/// One entry in a person's inbox.
#[derive(Debug, Clone, Serialize, sqlx::FromRow)]
pub struct Notification {
    pub id: i64,
    pub user_id: String,
    pub subscription_id: Option<String>,
    pub event: String,
    pub title: String,
    pub body: String,
    pub link: Option<String>,
    pub run_id: Option<String>,
    pub trigger_id: Option<String>,
    pub project: Option<String>,
    pub created_at: DateTime<Utc>,
    pub read_at: Option<DateTime<Utc>>,
    /// Why it did not reach email or Slack, when it didn't.
    pub delivery_error: Option<String>,
}

/// A notification to write.
#[derive(Debug, Clone, Default)]
pub struct NewNotification<'a> {
    pub user_id: &'a str,
    pub subscription_id: Option<&'a str>,
    pub event: &'a str,
    pub title: &'a str,
    pub body: &'a str,
    pub link: Option<&'a str>,
    pub run_id: Option<&'a str>,
    pub trigger_id: Option<&'a str>,
    pub project: Option<&'a str>,
    pub dedupe_key: &'a str,
}

const SUBSCRIPTION_COLUMNS: &str = "s.id, s.user_id, s.events, s.scope_kind, s.scope_value, \
    s.channels, s.slack_channel, s.created_at";
const NOTIFICATION_COLUMNS: &str = "id, user_id, subscription_id, event, title, body, link, \
    run_id, trigger_id, project, created_at, read_at, delivery_error";

/// Postgres-backed subscriptions and inbox.
pub struct NotificationStore {
    pool: PgPool,
}

impl NotificationStore {
    /// Connect to `database_url` and ensure the schema exists.
    pub async fn connect(database_url: &str) -> Result<Self, PersistError> {
        let pool = sqlx::postgres::PgPoolOptions::new()
            .max_connections(3)
            .connect(database_url)
            .await?;
        Self::from_pool(pool).await
    }

    /// Wrap an existing pool; ensures the tables exist. The accounts table
    /// comes first, because both tables have a foreign key into it.
    pub async fn from_pool(pool: PgPool) -> Result<Self, PersistError> {
        crate::users::ensure_schema(&pool).await?;
        for stmt in [
            CREATE_SUBSCRIPTIONS,
            CREATE_SUBSCRIPTIONS_USER_IDX,
            CREATE_NOTIFICATIONS,
            CREATE_NOTIFICATIONS_DEDUPE_IDX,
            CREATE_NOTIFICATIONS_INBOX_IDX,
            ADD_SUBSCRIPTION_SLACK_CHANNEL,
            ADD_NOTIFICATION_DELIVERY_ERROR,
            CREATE_CHANNEL_POSTS,
        ] {
            sqlx::query(stmt).execute(&pool).await?;
        }
        Ok(Self { pool })
    }

    /// Subscribe `user_id`. The input must already be [`SubscriptionInput::validated`].
    pub async fn subscribe(
        &self,
        user_id: &str,
        input: &SubscriptionInput,
    ) -> Result<NotificationSubscription, PersistError> {
        let sql = format!(
            "INSERT INTO harness_notification_subscriptions AS s
                 (id, user_id, events, scope_kind, scope_value, channels, slack_channel)
             VALUES ($1, $2, $3, $4, $5, $6, $7) RETURNING {SUBSCRIPTION_COLUMNS}"
        );
        Ok(sqlx::query_as::<_, NotificationSubscription>(&sql)
            .bind(uuid::Uuid::new_v4().to_string())
            .bind(user_id)
            .bind(Json(&input.events))
            .bind(&input.scope_kind)
            .bind(&input.scope_value)
            .bind(Json(&input.channels))
            .bind(&input.slack_channel)
            .fetch_one(&self.pool)
            .await?)
    }

    /// A person's subscriptions, newest first.
    pub async fn subscriptions(
        &self,
        user_id: &str,
    ) -> Result<Vec<NotificationSubscription>, PersistError> {
        let sql = format!(
            "SELECT {SUBSCRIPTION_COLUMNS} FROM harness_notification_subscriptions s
             WHERE s.user_id = $1 ORDER BY s.created_at DESC"
        );
        Ok(sqlx::query_as::<_, NotificationSubscription>(&sql)
            .bind(user_id)
            .fetch_all(&self.pool)
            .await?)
    }

    /// Remove a subscription, but only the caller's own.
    pub async fn unsubscribe(&self, id: &str, user_id: &str) -> Result<bool, PersistError> {
        let result = sqlx::query(
            "DELETE FROM harness_notification_subscriptions WHERE id = $1 AND user_id = $2",
        )
        .bind(id)
        .bind(user_id)
        .execute(&self.pool)
        .await?;
        Ok(result.rows_affected() > 0)
    }

    /// Every subscription that wants `event` about `subject`, from people
    /// whose account is not disabled.
    pub async fn matching(
        &self,
        event: &str,
        subject: &EventSubject<'_>,
    ) -> Result<Vec<NotificationSubscription>, PersistError> {
        let sql = format!(
            "SELECT {SUBSCRIPTION_COLUMNS}
             FROM harness_notification_subscriptions s
             JOIN harness_users u ON u.id = s.user_id AND u.disabled_at IS NULL
             WHERE s.events ? $1
               AND ((s.scope_kind = 'trigger'  AND s.scope_value = $2)
                 OR (s.scope_kind = 'workflow' AND s.scope_value = $3)
                 OR (s.scope_kind = 'project'  AND s.scope_value = $4)
                 OR (s.scope_kind = 'mine'     AND s.user_id = $5))
             ORDER BY s.created_at"
        );
        Ok(sqlx::query_as::<_, NotificationSubscription>(&sql)
            .bind(event)
            .bind(subject.trigger_id)
            .bind(subject.workflow)
            .bind(subject.project)
            .bind(subject.owner)
            .fetch_all(&self.pool)
            .await?)
    }

    /// Write a notification. `None` when this person already has one with the
    /// same `dedupe_key`.
    pub async fn notify(
        &self,
        n: &NewNotification<'_>,
    ) -> Result<Option<Notification>, PersistError> {
        let sql = format!(
            "INSERT INTO harness_notifications
                 (user_id, subscription_id, event, title, body, link, run_id, trigger_id, project, dedupe_key)
             VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10)
             ON CONFLICT (user_id, dedupe_key) DO NOTHING
             RETURNING {NOTIFICATION_COLUMNS}"
        );
        Ok(sqlx::query_as::<_, Notification>(&sql)
            .bind(n.user_id)
            .bind(n.subscription_id)
            .bind(n.event)
            .bind(n.title)
            .bind(n.body)
            .bind(n.link)
            .bind(n.run_id)
            .bind(n.trigger_id)
            .bind(n.project)
            .bind(n.dedupe_key)
            .fetch_optional(&self.pool)
            .await?)
    }

    /// A person's inbox, newest first.
    pub async fn inbox(
        &self,
        user_id: &str,
        unread_only: bool,
        limit: i64,
    ) -> Result<Vec<Notification>, PersistError> {
        let sql = format!(
            "SELECT {NOTIFICATION_COLUMNS} FROM harness_notifications
             WHERE user_id = $1 AND ($2 = false OR read_at IS NULL)
             ORDER BY created_at DESC, id DESC LIMIT $3"
        );
        Ok(sqlx::query_as::<_, Notification>(&sql)
            .bind(user_id)
            .bind(unread_only)
            .bind(limit.clamp(1, 200))
            .fetch_all(&self.pool)
            .await?)
    }

    /// How many unread notifications a person has.
    pub async fn unread_count(&self, user_id: &str) -> Result<i64, PersistError> {
        let (n,): (i64,) = sqlx::query_as(
            "SELECT count(*) FROM harness_notifications WHERE user_id = $1 AND read_at IS NULL",
        )
        .bind(user_id)
        .fetch_one(&self.pool)
        .await?;
        Ok(n)
    }

    /// Mark notifications read: the given ids, or all of them when `ids` is
    /// `None`. Only ever the caller's own. Returns how many changed.
    pub async fn mark_read(&self, user_id: &str, ids: Option<&[i64]>) -> Result<u64, PersistError> {
        let result = match ids {
            Some(ids) => {
                sqlx::query(
                    "UPDATE harness_notifications SET read_at = now()
                     WHERE user_id = $1 AND read_at IS NULL AND id = ANY($2)",
                )
                .bind(user_id)
                .bind(ids)
                .execute(&self.pool)
                .await?
            }
            None => {
                sqlx::query(
                    "UPDATE harness_notifications SET read_at = now()
                     WHERE user_id = $1 AND read_at IS NULL",
                )
                .bind(user_id)
                .execute(&self.pool)
                .await?
            }
        };
        Ok(result.rows_affected())
    }

    /// Record why a notification did not reach email or Slack, or clear it.
    pub async fn set_delivery_error(
        &self,
        id: i64,
        error: Option<&str>,
    ) -> Result<(), PersistError> {
        sqlx::query("UPDATE harness_notifications SET delivery_error = $2 WHERE id = $1")
            .bind(id)
            .bind(error)
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    /// Claim the one post `dedupe_key` gets in a shared `channel`. `false`
    /// when somebody else's subscription already posted it there.
    pub async fn claim_channel_post(
        &self,
        channel: &str,
        dedupe_key: &str,
    ) -> Result<bool, PersistError> {
        let result = sqlx::query(
            "INSERT INTO harness_notification_channel_posts (channel, dedupe_key)
             VALUES ($1, $2) ON CONFLICT DO NOTHING",
        )
        .bind(channel)
        .bind(dedupe_key)
        .execute(&self.pool)
        .await?;
        Ok(result.rows_affected() > 0)
    }

    /// Delete notifications older than `days`, read or not.
    pub async fn prune(&self, days: i64) -> Result<u64, PersistError> {
        let result = sqlx::query(
            "DELETE FROM harness_notifications WHERE created_at < now() - ($1 * interval '1 day')",
        )
        .bind(days as f64)
        .execute(&self.pool)
        .await?;
        sqlx::query(
            "DELETE FROM harness_notification_channel_posts
             WHERE created_at < now() - ($1 * interval '1 day')",
        )
        .bind(days as f64)
        .execute(&self.pool)
        .await?;
        Ok(result.rows_affected())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{NewUser, UserStore};

    fn input(events: &[&str], scope: &str, value: &str) -> SubscriptionInput {
        SubscriptionInput {
            events: events.iter().map(|e| e.to_string()).collect(),
            scope_kind: scope.into(),
            scope_value: value.into(),
            channels: vec![],
            slack_channel: None,
        }
    }

    #[test]
    fn validation_normalises_and_refuses() {
        let ok = input(
            &["run_failed", "run_failed", "trigger_failed"],
            "trigger",
            " t1 ",
        )
        .validated()
        .unwrap();
        assert_eq!(ok.events, ["run_failed", "trigger_failed"]);
        assert_eq!(ok.scope_value, "t1");
        assert_eq!(
            ok.channels,
            [CHANNEL_IN_APP],
            "the inbox is always a channel"
        );

        let mine = input(&["run_failed"], "mine", "ignored")
            .validated()
            .unwrap();
        assert_eq!(mine.scope_value, "");

        assert!(input(&[], "mine", "").validated().is_err());
        assert!(input(&["run_exploded"], "mine", "").validated().is_err());
        assert!(input(&["run_failed"], "galaxy", "x").validated().is_err());
        assert!(input(&["run_failed"], "project", "  ").validated().is_err());
        let mut bad_channel = input(&["run_failed"], "mine", "");
        bad_channel.channels = vec!["carrier-pigeon".into()];
        assert!(bad_channel.validated().is_err());
    }

    #[test]
    fn a_slack_channel_needs_a_name_and_only_with_that_channel() {
        let mut to_channel = input(&["run_failed"], "mine", "");
        to_channel.channels = vec!["slack_channel".into()];
        assert!(to_channel.clone().validated().is_err(), "no channel named");

        to_channel.slack_channel = Some("  #builds ".into());
        let ok = to_channel.clone().validated().unwrap();
        assert_eq!(ok.slack_channel.as_deref(), Some("builds"));
        assert_eq!(ok.channels, [CHANNEL_IN_APP, CHANNEL_SLACK_CHANNEL]);

        to_channel.slack_channel = Some("two words".into());
        assert!(to_channel.validated().is_err());

        // A channel name without the channel that uses it is dropped.
        let mut dm_only = input(&["run_failed"], "mine", "");
        dm_only.channels = vec!["slack_dm".into(), "email".into()];
        dm_only.slack_channel = Some("builds".into());
        let ok = dm_only.validated().unwrap();
        assert_eq!(ok.slack_channel, None);
        assert_eq!(
            ok.channels,
            [CHANNEL_EMAIL, CHANNEL_IN_APP, CHANNEL_SLACK_DM]
        );
    }

    fn db_url() -> Option<String> {
        let url = std::env::var("HARNESS_DATABASE_URL").ok()?;
        crate::is_test_db(&url).then_some(url)
    }

    async fn a_user(url: &str, tag: &str) -> String {
        let users = UserStore::connect(url).await.expect("users");
        users
            .create(&NewUser {
                email: format!(
                    "{tag}-{}@example.test",
                    chrono::Utc::now().timestamp_nanos_opt().unwrap()
                ),
                name: tag.into(),
                role: "member".into(),
                password_hash: None,
            })
            .await
            .expect("create user")
            .id
    }

    /// Subscriptions match on their scope only, and a person is told about a
    /// thing once however many of their subscriptions match it.
    #[tokio::test]
    async fn matching_and_dedupe() {
        let Some(url) = db_url() else { return };
        let store = NotificationStore::connect(&url).await.unwrap();
        let alice = a_user(&url, "notify-alice").await;
        let bob = a_user(&url, "notify-bob").await;
        let trigger = format!("t-{}", uuid::Uuid::new_v4());

        let one_trigger = input(&["trigger_failed", "run_failed"], "trigger", &trigger)
            .validated()
            .unwrap();
        store.subscribe(&alice, &one_trigger).await.unwrap();
        let mine = input(&["run_failed"], "mine", "").validated().unwrap();
        store.subscribe(&alice, &mine).await.unwrap();
        let project = input(&["run_completed"], "project", "proj-x")
            .validated()
            .unwrap();
        store.subscribe(&bob, &project).await.unwrap();

        // A failure of that trigger reaches Alice through the trigger scope.
        let subject = EventSubject {
            trigger_id: Some(&trigger),
            workflow: Some("task"),
            project: Some("proj-x"),
            owner: Some(&alice),
        };
        let failed = store
            .matching(EVENT_TRIGGER_FAILED, &subject)
            .await
            .unwrap();
        assert_eq!(failed.len(), 1);
        assert_eq!(failed[0].user_id, alice);

        // A failed run of hers matches both her subscriptions…
        let run_failed = store.matching(EVENT_RUN_FAILED, &subject).await.unwrap();
        assert_eq!(run_failed.iter().filter(|s| s.user_id == alice).count(), 2);
        // …and Bob, who only asked about completed runs, hears nothing.
        assert!(run_failed.iter().all(|s| s.user_id != bob));

        // Both matches write one notification between them.
        let key = format!("run:{}:run_failed", uuid::Uuid::new_v4());
        let n = NewNotification {
            user_id: &alice,
            event: EVENT_RUN_FAILED,
            title: "Run failed",
            dedupe_key: &key,
            ..Default::default()
        };
        assert!(store.notify(&n).await.unwrap().is_some());
        assert!(store.notify(&n).await.unwrap().is_none(), "deduped");
        assert_eq!(store.unread_count(&alice).await.unwrap(), 1);

        // Read state and ownership.
        let inbox = store.inbox(&alice, true, 50).await.unwrap();
        assert_eq!(inbox.len(), 1);
        assert_eq!(
            store.mark_read(&bob, Some(&[inbox[0].id])).await.unwrap(),
            0
        );
        assert_eq!(
            store.mark_read(&alice, Some(&[inbox[0].id])).await.unwrap(),
            1
        );
        assert_eq!(store.unread_count(&alice).await.unwrap(), 0);
        assert_eq!(store.inbox(&alice, false, 50).await.unwrap().len(), 1);

        // A delivery failure is kept on the notification.
        store
            .set_delivery_error(inbox[0].id, Some("Slack: channel_not_found"))
            .await
            .unwrap();
        let again = store.inbox(&alice, false, 50).await.unwrap();
        assert_eq!(
            again[0].delivery_error.as_deref(),
            Some("Slack: channel_not_found")
        );

        // A shared channel gets one post per thing.
        let channel = format!("builds-{}", uuid::Uuid::new_v4());
        assert!(store.claim_channel_post(&channel, &key).await.unwrap());
        assert!(!store.claim_channel_post(&channel, &key).await.unwrap());

        // Only the owner can unsubscribe.
        let subs = store.subscriptions(&alice).await.unwrap();
        assert_eq!(subs.len(), 2);
        assert!(!store.unsubscribe(&subs[0].id, &bob).await.unwrap());
        assert!(store.unsubscribe(&subs[0].id, &alice).await.unwrap());
        assert_eq!(store.subscriptions(&alice).await.unwrap().len(), 1);
    }
}
