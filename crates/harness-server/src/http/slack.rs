//! **Slack**, for notifications: a direct message to a person, or a post in a
//! channel, sent as the workspace's own Slack app.
//!
//! An administrator creates the app from [`MANIFEST`] and stores its bot token
//! under the `slack` credential (Settings → Integrations). Nothing else is
//! configured: a person's Slack account is found by their harness email
//! address, so nobody links anything.
//!
//! Only three Web API methods are used — `auth.test`, `users.lookupByEmail`
//! and `chat.postMessage` — over plain `reqwest`; a Slack SDK would be far
//! more than this needs.

use std::collections::HashMap;
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

use serde_json::{json, Value};

use super::runs_routes::RunsState;

/// Credential provider the bot token lives under.
pub(crate) const PROVIDER: &str = "slack";
/// The field holding the bot token (`xoxb-…`).
const TOKEN_FIELD: &str = "bot_token";
const API: &str = "https://slack.com/api";
/// How long a Slack user id found by email is reused.
const LOOKUP_TTL: Duration = Duration::from_secs(60 * 60);
/// The longest a single Slack call may take.
const TIMEOUT: Duration = Duration::from_secs(15);

/// The app the administrator creates in Slack. The scopes are what posting to
/// channels and people, and finding people by address, need.
pub(crate) const MANIFEST: &str = r##"{
  "display_information": {
    "name": "Harness",
    "description": "Notifications about runs and triggers you follow.",
    "background_color": "#1f2937"
  },
  "features": {
    "bot_user": { "display_name": "Harness", "always_online": false }
  },
  "oauth_config": {
    "scopes": {
      "bot": ["chat:write", "chat:write.public", "im:write", "users:read", "users:read.email"]
    }
  },
  "settings": {
    "org_deploy_enabled": false,
    "socket_mode_enabled": false,
    "token_rotation_enabled": false
  }
}"##;

/// A message: a title, a line or two of detail, and where to look.
#[derive(Debug, Clone)]
pub(crate) struct SlackMessage<'a> {
    pub title: &'a str,
    pub body: &'a str,
    /// Absolute URL of the thing it is about, when the harness knows its own
    /// address.
    pub url: Option<&'a str>,
    /// Absolute URL of the person's notification settings.
    pub settings_url: Option<&'a str>,
}

impl SlackMessage<'_> {
    /// Slack's mrkdwn treats `&`, `<` and `>` as markup.
    fn escape(text: &str) -> String {
        text.replace('&', "&amp;")
            .replace('<', "&lt;")
            .replace('>', "&gt;")
    }

    /// The `chat.postMessage` body for `channel`: blocks for Slack, and the
    /// title as `text` for notifications and screen readers.
    fn payload(&self, channel: &str) -> Value {
        let title = Self::escape(self.title);
        let heading = match self.url {
            Some(url) => format!("*<{url}|{title}>*"),
            None => format!("*{title}*"),
        };
        let mut text = heading;
        if !self.body.trim().is_empty() {
            text.push('\n');
            text.push_str(&Self::escape(self.body));
        }
        let mut blocks = vec![json!({
            "type": "section",
            "text": { "type": "mrkdwn", "text": text },
        })];
        if let Some(settings) = self.settings_url {
            blocks.push(json!({
                "type": "context",
                "elements": [{
                    "type": "mrkdwn",
                    "text": format!("You follow this in the harness · <{settings}|Change what you hear about>"),
                }],
            }));
        }
        json!({
            "channel": channel,
            "text": self.title,
            "blocks": blocks,
            "unfurl_links": false,
            "unfurl_media": false,
        })
    }
}

/// A Slack Web API client for one bot token.
pub(crate) struct SlackClient {
    http: reqwest::Client,
    base: String,
    token: String,
}

impl SlackClient {
    pub(crate) fn new(token: impl Into<String>) -> Self {
        Self::with_base(API, token)
    }

    /// Against another address: a test's mock server.
    pub(crate) fn with_base(base: impl Into<String>, token: impl Into<String>) -> Self {
        Self {
            http: reqwest::Client::builder()
                .timeout(TIMEOUT)
                .build()
                .unwrap_or_default(),
            base: base.into().trim_end_matches('/').to_string(),
            token: token.into(),
        }
    }

    /// Call `method`. Slack answers HTTP 200 with `{"ok": false, "error": …}`
    /// for most failures, so both layers are checked.
    async fn call(&self, method: &str, request: reqwest::RequestBuilder) -> Result<Value, String> {
        let resp = request
            .bearer_auth(&self.token)
            .send()
            .await
            .map_err(|e| format!("Slack {method}: {e}"))?;
        let status = resp.status();
        if status == reqwest::StatusCode::TOO_MANY_REQUESTS {
            return Err(format!("Slack {method}: rate limited"));
        }
        let body: Value = resp
            .json()
            .await
            .map_err(|e| format!("Slack {method}: HTTP {status}, unreadable answer: {e}"))?;
        if body.get("ok").and_then(Value::as_bool) == Some(true) {
            Ok(body)
        } else {
            let error = body
                .get("error")
                .and_then(Value::as_str)
                .unwrap_or("unknown error");
            Err(format!("Slack {method}: {}", explain(error)))
        }
    }

    /// The Slack user id of the person with this address.
    pub(crate) async fn user_id_for_email(&self, email: &str) -> Result<String, String> {
        let req = self
            .http
            .get(format!("{}/users.lookupByEmail", self.base))
            .query(&[("email", email)]);
        let body = self.call("users.lookupByEmail", req).await?;
        body.pointer("/user/id")
            .and_then(Value::as_str)
            .map(str::to_string)
            .ok_or_else(|| "Slack users.lookupByEmail: no user id in the answer".into())
    }

    /// Post `message` to `channel`: a channel id or name, or a user id for a DM.
    pub(crate) async fn post(
        &self,
        channel: &str,
        message: &SlackMessage<'_>,
    ) -> Result<(), String> {
        let req = self
            .http
            .post(format!("{}/chat.postMessage", self.base))
            .json(&message.payload(channel));
        self.call("chat.postMessage", req).await.map(|_| ())
    }
}

/// Slack's error codes, said the way an operator can act on them.
fn explain(code: &str) -> String {
    match code {
        "users_not_found" => "no Slack account uses this email address".into(),
        "channel_not_found" => {
            "no such channel; a private channel needs the app invited to it".into()
        }
        "not_in_channel" => "the app is not in that channel; invite it".into(),
        "is_archived" => "the channel is archived".into(),
        "invalid_auth" | "not_authed" | "token_revoked" | "account_inactive" => {
            "the bot token is not valid any more; replace it under Settings → Integrations".into()
        }
        "missing_scope" => {
            "the Slack app is missing a permission; recreate it from the manifest".into()
        }
        other => other.to_string(),
    }
}

/// The stored bot token, if Slack is connected.
pub(crate) async fn token(state: &Arc<RunsState>) -> Option<String> {
    let store = state.cred_store().await.ok()?;
    store
        .get(PROVIDER)
        .await
        .ok()
        .flatten()?
        .get(TOKEN_FIELD)
        .map(|t| t.trim().to_string())
        .filter(|t| !t.is_empty())
}

/// Slack user ids found by email: (token, lower-cased address) → (id, when).
type LookupCache = Mutex<HashMap<(String, String), (String, Instant)>>;

/// Recently found Slack user ids, so a burst of notifications does not look
/// the same person up again and again.
fn lookups() -> &'static LookupCache {
    static CACHE: OnceLock<LookupCache> = OnceLock::new();
    CACHE.get_or_init(|| Mutex::new(HashMap::new()))
}

/// The Slack user id for `email`, from the cache or Slack.
pub(crate) async fn user_id(client: &SlackClient, email: &str) -> Result<String, String> {
    let key = (client.token.clone(), email.trim().to_ascii_lowercase());
    if let Ok(cache) = lookups().lock() {
        if let Some((id, at)) = cache.get(&key) {
            if at.elapsed() < LOOKUP_TTL {
                return Ok(id.clone());
            }
        }
    }
    let id = client.user_id_for_email(email.trim()).await?;
    if let Ok(mut cache) = lookups().lock() {
        cache.insert(key, (id.clone(), Instant::now()));
    }
    Ok(id)
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::extract::Query;
    use axum::routing::{get, post};
    use axum::{Json, Router};

    /// A Slack stand-in: one known person, one known channel, and a record of
    /// what was posted.
    async fn mock() -> (String, Arc<Mutex<Vec<Value>>>) {
        let posted = Arc::new(Mutex::new(Vec::new()));
        let seen = posted.clone();
        let app = Router::new()
            .route(
                "/users.lookupByEmail",
                get(|Query(q): Query<HashMap<String, String>>| async move {
                    Json(match q.get("email").map(String::as_str) {
                        Some("ada@example.test") => json!({ "ok": true, "user": { "id": "U123" } }),
                        _ => json!({ "ok": false, "error": "users_not_found" }),
                    })
                }),
            )
            .route(
                "/chat.postMessage",
                post(move |Json(body): Json<Value>| {
                    let seen = seen.clone();
                    async move {
                        let ok = matches!(body["channel"].as_str(), Some("U123" | "builds"));
                        seen.lock().unwrap().push(body);
                        Json(if ok {
                            json!({ "ok": true })
                        } else {
                            json!({ "ok": false, "error": "channel_not_found" })
                        })
                    }
                }),
            );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        (format!("http://{addr}"), posted)
    }

    fn message() -> SlackMessage<'static> {
        SlackMessage {
            title: "Run failed: ECOM-1 <fix>",
            body: "`task` in `shop`, at step `test`",
            url: Some("https://harness.example/runs/task-1"),
            settings_url: Some("https://harness.example/settings/notifications"),
        }
    }

    #[tokio::test]
    async fn finds_people_by_address_and_posts() {
        let (base, posted) = mock().await;
        let client = SlackClient::with_base(base, "xoxb-test");

        assert_eq!(user_id(&client, "ada@example.test").await.unwrap(), "U123");
        // The same person in another case is the cached answer, not a second
        // lookup (which this mock would refuse).
        assert_eq!(
            user_id(&client, " ADA@Example.test ").await.unwrap(),
            "U123"
        );
        let missing = client
            .user_id_for_email("bob@example.test")
            .await
            .unwrap_err();
        assert!(
            missing.contains("no Slack account uses this email address"),
            "{missing}"
        );

        client.post("U123", &message()).await.unwrap();
        client.post("builds", &message()).await.unwrap();
        let err = client.post("nowhere", &message()).await.unwrap_err();
        assert!(
            err.contains("a private channel needs the app invited"),
            "{err}"
        );

        let posted = posted.lock().unwrap();
        assert_eq!(posted.len(), 3);
        assert_eq!(posted[0]["text"], "Run failed: ECOM-1 <fix>");
    }

    #[test]
    fn the_message_links_its_title_and_escapes_markup() {
        let p = message().payload("C1");
        assert_eq!(p["channel"], "C1");
        let text = p["blocks"][0]["text"]["text"].as_str().unwrap();
        assert!(text
            .starts_with("*<https://harness.example/runs/task-1|Run failed: ECOM-1 &lt;fix&gt;>*"));
        assert!(text.ends_with("`task` in `shop`, at step `test`"));
        assert!(p["blocks"][1]["elements"][0]["text"]
            .as_str()
            .unwrap()
            .contains("/settings/notifications|"));

        let bare = SlackMessage {
            url: None,
            settings_url: None,
            ..message()
        }
        .payload("C1");
        assert_eq!(bare["blocks"].as_array().unwrap().len(), 1);
        assert!(bare["blocks"][0]["text"]["text"]
            .as_str()
            .unwrap()
            .starts_with("*Run failed"));
    }

    #[test]
    fn the_manifest_is_valid_json_with_the_scopes_used() {
        let m: Value = serde_json::from_str(MANIFEST).unwrap();
        let scopes = m["oauth_config"]["scopes"]["bot"].as_array().unwrap();
        for needed in ["chat:write", "users:read.email"] {
            assert!(scopes.iter().any(|s| s == needed), "{needed}");
        }
    }
}
