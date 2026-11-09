//! The **MCP gateway**: how a workflow step reaches a registered MCP server
//! without ever holding its credentials.
//!
//! - `POST   /api/run/mcp/session`  — a step opens a session (run token)
//! - `DELETE /api/run/mcp/session`  — and closes it when it ends (session token)
//! - `POST   /api/run/mcp/{server}` — the MCP endpoint the agent talks to
//!
//! Why a gateway rather than handing the agent the server's config: an agent
//! has a shell. Whatever reaches its environment or a file it can read — a
//! token in an env var, a header in a config — it can use directly, and then
//! the tool list a step was limited to means nothing. So the server keeps the
//! credentials, starts or calls the real server itself, and answers the agent
//! as an MCP server that lists only the step's tools and refuses any other
//! call. What the agent holds is a session token that buys exactly those
//! tools, for this step, until the step ends.
//!
//! In-process like [`super::run_grants`]: the harness is one container, and
//! a step's calls land on the instance that opened its session. A stdio server
//! is started on first use and stopped when the session closes.

use std::collections::{BTreeMap, HashMap};
use std::sync::{Arc, LazyLock};

use axum::extract::{Extension, Path};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::Json;
use harness_dag::{Activity, McpUse};
use serde::Deserialize;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};

use super::mcp_client::{McpClient, Upstream, PROTOCOL_VERSION};
use super::runs_routes::RunsState;

/// Longer than any step should take; a session whose step crashed without
/// closing it stops working on its own.
const SESSION_TTL_MS: i64 = 12 * 60 * 60 * 1000;

/// One server inside a session.
struct Gated {
    upstream: Upstream,
    /// The only tools this step may list or call.
    allowed: Vec<String>,
    /// Started on first use, then reused for the rest of the step.
    client: tokio::sync::OnceCell<Arc<McpClient>>,
}

struct Session {
    run_id: String,
    node: String,
    servers: HashMap<String, Gated>,
    created_ms: i64,
}

/// Hashed token → session. Hashed so the map never holds a usable token.
static SESSIONS: LazyLock<std::sync::Mutex<HashMap<String, Arc<Session>>>> =
    LazyLock::new(Default::default);

fn hash(token: &str) -> String {
    format!("{:x}", Sha256::digest(token.as_bytes()))
}

fn now_ms() -> i64 {
    chrono::Utc::now().timestamp_millis()
}

fn bearer(headers: &HeaderMap) -> Option<&str> {
    headers
        .get(axum::http::header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "))
        .map(str::trim)
        .filter(|t| !t.is_empty())
}

fn insert(session: Session) -> String {
    let token = format!(
        "hrn_mcp_step_{}{}",
        uuid::Uuid::new_v4().simple(),
        uuid::Uuid::new_v4().simple()
    );
    if let Ok(mut map) = SESSIONS.lock() {
        let now = now_ms();
        map.retain(|_, s| now - s.created_ms < SESSION_TTL_MS);
        map.insert(hash(&token), Arc::new(session));
    }
    token
}

fn find(token: &str) -> Option<Arc<Session>> {
    let s = SESSIONS.lock().ok()?.get(&hash(token)).cloned()?;
    (now_ms() - s.created_ms < SESSION_TTL_MS).then_some(s)
}

/// The tools a step gets for a server: what it asked for, inside what the
/// admin enabled. Checked again here, not only at save time, because the admin
/// may have switched a tool off since the workflow was saved.
fn allowed_for(asked: Option<&[String]>, enabled: &[String]) -> Vec<String> {
    match asked {
        None => enabled.to_vec(),
        Some(asked) => asked
            .iter()
            .filter(|t| enabled.contains(t))
            .cloned()
            .collect(),
    }
}

#[derive(Debug, Deserialize)]
pub struct OpenRequest {
    node: String,
    uses: Vec<McpUse>,
}

/// `POST /api/run/mcp/session` — authenticated by the run's own grant, so a
/// session is only ever for that run's project.
pub async fn open_session(
    Extension(state): Extension<Arc<RunsState>>,
    headers: HeaderMap,
    Json(req): Json<OpenRequest>,
) -> Response {
    let grant = match super::run_linear_routes::grant(&headers) {
        Ok(g) => g,
        Err((s, m)) => return (s, Json(json!({ "error": m }))).into_response(),
    };
    let mut servers = HashMap::new();
    let mut missing = Vec::new();
    for u in &req.uses {
        match gated(&state, &grant.project, u).await {
            Ok(g) if g.allowed.is_empty() => {
                tracing::info!(run_id = %grant.run_id, node = %req.node, server = %u.server,
                    "mcp gateway: no enabled tools left for this step");
                missing.push(u.server.clone());
            }
            Ok(g) => {
                servers.insert(u.server.clone(), g);
            }
            Err(e) => {
                tracing::info!(run_id = %grant.run_id, node = %req.node, server = %u.server,
                    "mcp gateway: unavailable: {e}");
                missing.push(u.server.clone());
            }
        }
    }
    let base = state
        .public_url()
        .unwrap_or_default()
        .trim_end_matches('/')
        .to_string();
    let urls: BTreeMap<String, String> = servers
        .keys()
        .map(|name| (name.clone(), format!("{base}/api/run/mcp/{name}")))
        .collect();
    let token = insert(Session {
        run_id: grant.run_id.clone(),
        node: req.node.clone(),
        servers,
        created_ms: now_ms(),
    });
    Json(json!({ "token": token, "servers": urls, "missing": missing })).into_response()
}

/// Resolve one `mcp:` entry to a server this session can reach.
async fn gated(state: &Arc<RunsState>, project: &str, u: &McpUse) -> Result<Gated, String> {
    let (upstream, enabled) = if harness_runner::mcp_servers::is_builtin(&u.server) {
        let up = super::mcp_registry::builtin_upstream(state, &u.server, project).await?;
        let tools = harness_runner::mcp_servers::tool_names(&u.server)
            .iter()
            .map(|t| t.to_string())
            .collect();
        (up, tools)
    } else {
        let store = state.mcp_server_store().await?;
        let server = store
            .get(&u.server)
            .await
            .map_err(|e| e.to_string())?
            .ok_or_else(|| format!("no MCP server `{}` is set up", u.server))?;
        let up = super::mcp_registry::registry_upstream(state, &server).await?;
        (up, server.enabled_tools.0)
    };
    Ok(Gated {
        upstream,
        allowed: allowed_for(u.tools.as_deref(), &enabled),
        client: tokio::sync::OnceCell::new(),
    })
}

/// `DELETE /api/run/mcp/session` — the step is over. Dropping the session
/// stops any server it started.
pub async fn close_session(headers: HeaderMap) -> Response {
    if let Some(token) = bearer(&headers) {
        if let Ok(mut map) = SESSIONS.lock() {
            map.remove(&hash(token));
        }
    }
    StatusCode::NO_CONTENT.into_response()
}

/// `POST /api/run/mcp/{server}` — one JSON-RPC message (or a batch) from the
/// agent. Answers as an MCP server offering only the session's tools.
pub async fn handle(
    Extension(state): Extension<Arc<RunsState>>,
    Path(server): Path<String>,
    headers: HeaderMap,
    Json(body): Json<Value>,
) -> Response {
    let Some(session) = bearer(&headers).and_then(find) else {
        return (
            StatusCode::UNAUTHORIZED,
            Json(json!({ "error": "this MCP session is unknown or has ended" })),
        )
            .into_response();
    };
    if !session.servers.contains_key(&server) {
        return (
            StatusCode::NOT_FOUND,
            Json(json!({ "error": format!("this step has no MCP server `{server}`") })),
        )
            .into_response();
    }
    let answer = match body {
        Value::Array(msgs) => {
            let mut out = Vec::new();
            for m in msgs {
                if let Some(a) = one_message(&state, &session, &server, m).await {
                    out.push(a);
                }
            }
            (!out.is_empty()).then_some(Value::Array(out))
        }
        m => one_message(&state, &session, &server, m).await,
    };
    match answer {
        Some(a) => Json(a).into_response(),
        // Only notifications: nothing to answer.
        None => StatusCode::ACCEPTED.into_response(),
    }
}

fn reply(id: &Value, result: Value) -> Value {
    json!({ "jsonrpc": "2.0", "id": id, "result": result })
}

fn rpc_error(id: &Value, code: i64, message: impl Into<String>) -> Value {
    json!({ "jsonrpc": "2.0", "id": id, "error": { "code": code, "message": message.into() } })
}

/// Answer one message; `None` for a notification.
async fn one_message(
    state: &Arc<RunsState>,
    session: &Session,
    server: &str,
    msg: Value,
) -> Option<Value> {
    let id = msg.get("id").cloned()?;
    let method = msg
        .get("method")
        .and_then(Value::as_str)
        .unwrap_or_default();
    let params = msg.get("params").cloned().unwrap_or(Value::Null);
    let gated = &session.servers[server];
    Some(match method {
        "initialize" => {
            let version = params
                .get("protocolVersion")
                .and_then(Value::as_str)
                .unwrap_or(PROTOCOL_VERSION);
            reply(
                &id,
                json!({
                    "protocolVersion": version,
                    "capabilities": { "tools": { "listChanged": false } },
                    "serverInfo": { "name": format!("harness-{server}"), "version": env!("CARGO_PKG_VERSION") },
                }),
            )
        }
        "ping" => reply(&id, json!({})),
        "tools/list" => match client(gated).await {
            Ok(c) => match c.list_tools().await {
                Ok(tools) => reply(&id, json!({ "tools": filter_tools(tools, &gated.allowed) })),
                Err(e) => rpc_error(&id, -32603, format!("{server}: {e}")),
            },
            Err(e) => rpc_error(&id, -32603, format!("{server}: {e}")),
        },
        "tools/call" => {
            let tool = params
                .get("name")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string();
            if !gated.allowed.contains(&tool) {
                tracing::warn!(run_id = %session.run_id, node = %session.node, server, tool,
                    "mcp gateway: refused a tool this step may not use");
                record(
                    state,
                    session,
                    Activity::text(format!(
                        "MCP: refused `{server}.{tool}` — not one of this step's tools"
                    )),
                )
                .await;
                return Some(rpc_error(
                    &id,
                    -32602,
                    format!("`{tool}` is not available to this step"),
                ));
            }
            tracing::info!(run_id = %session.run_id, node = %session.node, server, tool,
                "mcp gateway: tool call");
            match client(gated).await {
                Ok(c) => match c.request("tools/call", params).await {
                    Ok(Ok(result)) => reply(&id, result),
                    Ok(Err(error)) => json!({ "jsonrpc": "2.0", "id": id, "error": error }),
                    Err(e) => rpc_error(&id, -32603, format!("{server}: {e}")),
                },
                Err(e) => rpc_error(&id, -32603, format!("{server}: {e}")),
            }
        }
        // The gateway offers tools only; an agent that asks for the rest gets
        // an empty list rather than an error it might retry.
        "resources/list" => reply(&id, json!({ "resources": [] })),
        "resources/templates/list" => reply(&id, json!({ "resourceTemplates": [] })),
        "prompts/list" => reply(&id, json!({ "prompts": [] })),
        other => rpc_error(&id, -32601, format!("method `{other}` is not supported")),
    })
}

async fn client(gated: &Gated) -> Result<Arc<McpClient>, String> {
    gated
        .client
        .get_or_try_init(|| async {
            McpClient::connect(&gated.upstream)
                .await
                .map(Arc::new)
                .map_err(|e| e.to_string())
        })
        .await
        .cloned()
}

fn filter_tools(tools: Vec<Value>, allowed: &[String]) -> Vec<Value> {
    tools
        .into_iter()
        .filter(|t| {
            t.get("name")
                .and_then(Value::as_str)
                .is_some_and(|n| allowed.iter().any(|a| a == n))
        })
        .collect()
}

/// A line in the step's activity feed, so a refusal is visible on the run.
async fn record(state: &Arc<RunsState>, session: &Session, act: Activity) {
    if let Ok(store) = state.store().await {
        let _ = store
            .record_activity(&session.run_id, &session.node, &act)
            .await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn s(v: &[&str]) -> Vec<String> {
        v.iter().map(|x| x.to_string()).collect()
    }

    #[test]
    fn a_step_gets_what_it_asked_for_inside_what_is_enabled() {
        let enabled = s(&["a", "b"]);
        assert_eq!(allowed_for(None, &enabled), s(&["a", "b"]));
        assert_eq!(allowed_for(Some(&s(&["b"])), &enabled), s(&["b"]));
        // Switched off since the workflow was saved: dropped, not granted.
        assert_eq!(allowed_for(Some(&s(&["b", "c"])), &enabled), s(&["b"]));
    }

    #[test]
    fn only_allowed_tools_are_listed() {
        let tools = vec![
            json!({ "name": "search" }),
            json!({ "name": "send" }),
            json!({ "description": "nameless" }),
        ];
        let kept = filter_tools(tools, &s(&["search"]));
        assert_eq!(kept, vec![json!({ "name": "search" })]);
    }

    #[test]
    fn a_session_token_is_hashed_at_rest_and_closes() {
        let token = insert(Session {
            run_id: "r".into(),
            node: "n".into(),
            servers: HashMap::new(),
            created_ms: now_ms(),
        });
        assert!(token.starts_with("hrn_mcp_step_"));
        assert!(SESSIONS.lock().unwrap().get(&token).is_none());
        assert_eq!(find(&token).unwrap().node, "n");
        SESSIONS.lock().unwrap().remove(&hash(&token));
        assert!(find(&token).is_none());
    }

    #[tokio::test]
    async fn a_call_outside_the_list_is_refused_without_reaching_the_server() {
        let state = Arc::new(RunsState::new(
            None,
            Arc::new(harness_agents::registry::AgentRegistry::new("claude")),
            std::path::PathBuf::from("."),
            None,
            None,
        ));
        let session = Session {
            run_id: "r".into(),
            node: "n".into(),
            servers: HashMap::from([(
                "mail".to_string(),
                Gated {
                    // Would fail to start if reached — the refusal must come first.
                    upstream: Upstream::Stdio {
                        command: "definitely-not-a-real-command".into(),
                        args: vec![],
                        env: BTreeMap::new(),
                    },
                    allowed: s(&["search"]),
                    client: tokio::sync::OnceCell::new(),
                },
            )]),
            created_ms: now_ms(),
        };
        let msg = json!({
            "jsonrpc": "2.0", "id": 3, "method": "tools/call",
            "params": { "name": "send", "arguments": {} }
        });
        let a = one_message(&state, &session, "mail", msg).await.unwrap();
        assert_eq!(a["error"]["code"], -32602);
        assert!(a["error"]["message"].as_str().unwrap().contains("send"));

        let init = json!({ "jsonrpc": "2.0", "id": 1, "method": "initialize",
            "params": { "protocolVersion": "2025-03-26" } });
        let a = one_message(&state, &session, "mail", init).await.unwrap();
        assert_eq!(a["result"]["protocolVersion"], "2025-03-26");
        assert!(a["result"]["capabilities"]["tools"].is_object());

        let note = json!({ "jsonrpc": "2.0", "method": "notifications/initialized" });
        assert!(one_message(&state, &session, "mail", note).await.is_none());
    }
}
