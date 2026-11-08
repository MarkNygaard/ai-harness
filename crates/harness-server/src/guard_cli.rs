//! `harness guard`: what an agent's tool hook runs to pick up a loop-guard
//! redirect for its step (see `http::loop_guard`).
//!
//! It asks `GET /api/run/guard?node=$HARNESS_NODE_ID` with the run's grant and
//! prints the answer in the shape the hook expects, or nothing. It runs after
//! tool calls, inside a step that is doing real work, so it never fails
//! loudly: no grant, no node id, a slow or unreachable server all mean "no
//! note", and the step carries on as if the guard weren't there.

use std::time::Duration;

/// How long a hook waits for the server. Short: it runs after tool calls.
const TIMEOUT: Duration = Duration::from_secs(3);

/// The hook's output for `format`, if a redirect is waiting.
pub async fn answer(format: &str) -> Option<String> {
    let node = std::env::var("HARNESS_NODE_ID")
        .ok()
        .filter(|n| !n.trim().is_empty())?;
    let note = fetch(&node).await?;
    Some(render(format, &note))
}

async fn fetch(node: &str) -> Option<String> {
    let g = crate::linear_cli::grant().ok()?;
    let client = reqwest::Client::builder().timeout(TIMEOUT).build().ok()?;
    let resp = client
        .get(format!("{}/api/run/guard", g.base))
        .query(&[("node", node)])
        .bearer_auth(&g.token)
        .send()
        .await
        .ok()?;
    if !resp.status().is_success() {
        return None;
    }
    let body: serde_json::Value = resp.json().await.ok()?;
    body.get("note")
        .and_then(|n| n.as_str())
        .filter(|n| !n.trim().is_empty())
        .map(str::to_string)
}

/// A note in the shape a hook prints it.
pub(crate) fn render(format: &str, note: &str) -> String {
    match format {
        // Claude Code reads a PostToolUse hook's stdout as JSON and adds
        // `additionalContext` to what the model sees next.
        "claude" => serde_json::json!({
            "hookSpecificOutput": {
                "hookEventName": "PostToolUse",
                "additionalContext": note,
            }
        })
        .to_string(),
        _ => note.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn claude_gets_a_post_tool_use_answer() {
        let out: serde_json::Value =
            serde_json::from_str(&render("claude", "try another way")).unwrap();
        assert_eq!(out["hookSpecificOutput"]["hookEventName"], "PostToolUse");
        assert_eq!(
            out["hookSpecificOutput"]["additionalContext"],
            "try another way"
        );
        assert_eq!(render("plain", "try another way"), "try another way");
    }
}
