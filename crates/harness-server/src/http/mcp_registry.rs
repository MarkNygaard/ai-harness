//! **Settings → MCP servers**: the servers workflow steps can ask for with
//! `mcp:`, and which of their tools steps may use.
//!
//! - `GET    /api/mcp-servers`                     — every server, built-ins included
//! - `POST   /api/mcp-servers`                     — add one (admin)
//! - `PUT    /api/mcp-servers/{name}`              — change how it is reached (admin)
//! - `DELETE /api/mcp-servers/{name}`              — remove it (admin)
//! - `PUT    /api/mcp-servers/{name}/tools`        — which tools steps may use (admin)
//! - `POST   /api/mcp-servers/{name}/refresh`      — list its tools again (admin)
//! - `POST   /api/mcp-servers/{name}/oauth/start`  — `{ url }` to sign in at (admin)
//! - `GET    /api/mcp-servers/oauth/callback`      — the provider's redirect (auth-exempt)
//! - `POST   /api/mcp-servers/{name}/oauth/disconnect` — forget the tokens (admin)
//!
//! **One connection per server, for the whole install.** Runs mostly have
//! nobody behind them — the Linear poller, schedules — so the account behind a
//! server is a shared service account, never the person who triggered the run.
//! That makes the tool lists the security boundary: anyone who can put text in
//! front of an agent can try to use that account. A new server therefore starts
//! with only the tools that declare themselves read-only enabled, and a step can
//! narrow a server's enabled tools but never widen them (checked at save time —
//! see [`harness_runner::mcp_servers::check_uses`] — and again by the gateway).
//!
//! Secrets — values of environment variables or headers, OAuth client and
//! tokens — are in the encrypted credential store under
//! [`harness_persist::mcp_credential_key`]; the registry row lists only their
//! names. Nothing secret is ever sent back to the browser.

use std::collections::{BTreeMap, HashMap};
use std::sync::{Arc, LazyLock};

use axum::extract::{Extension, Path, Query};
use axum::http::{header, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::Json;
use base64::Engine as _;
use harness_persist::{mcp_credential_key, CredentialStore, McpServer, McpTool};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use sqlx::types::Json as SqlJson;

use super::accounts::AdminOnly;
use super::mcp_client::{registry_tool, ClientError, McpClient, Upstream};
use super::runs_routes::RunsState;

/// Where the OAuth provider sends the browser back. Exempt from the bearer
/// middleware (a redirect can't carry a header); the single-use `state` nonce
/// authenticates it.
pub(crate) const CALLBACK_PATH: &str = "/api/mcp-servers/oauth/callback";

/// The Settings page the callback returns to.
const UI_PATH: &str = "/settings/mcp-servers";

/// Names a registry server may not take besides the built-ins: the gateway's
/// own `session` and `oauth` routes, which share the `/api/run/mcp/` prefix,
/// and the browser every Claude step is given, which is never listed.
const RESERVED_NAMES: &[&str] = &[
    "session",
    "oauth",
    harness_runner::mcp_servers::BROWSER_SERVER,
];

/// A token this close to expiry is refreshed before a step uses it.
const REFRESH_MARGIN_MS: i64 = 60_000;

const SECRET_PREFIX: &str = "secret:";

fn err(status: StatusCode, msg: impl Into<String>) -> Response {
    (status, Json(json!({ "error": msg.into() }))).into_response()
}

fn now_ms() -> i64 {
    chrono::Utc::now().timestamp_millis()
}

// ── What the browser sees ────────────────────────────────────────────────────

/// One server as Settings and the step editor show it. Never carries a secret.
#[derive(Debug, Serialize)]
pub(crate) struct ServerView {
    name: String,
    label: String,
    /// `builtin`, `stdio` or `http`.
    transport: String,
    command: String,
    args: Vec<String>,
    url: String,
    auth: String,
    secret_keys: Vec<String>,
    tools: Vec<McpTool>,
    enabled_tools: Vec<String>,
    tools_refreshed_at: Option<chrono::DateTime<chrono::Utc>>,
    /// `ok`, `not_connected` (OAuth not signed in yet), `reconnect` (the
    /// sign-in expired or was revoked), or `error` (see `last_error`).
    status: &'static str,
    last_error: String,
    builtin: bool,
}

fn status_of(server: &McpServer, creds: &BTreeMap<String, String>) -> &'static str {
    let has = |k: &str| creds.get(k).is_some_and(|v| !v.is_empty());
    if server.auth == "oauth" && !has("access_token") {
        return "not_connected";
    }
    if has("refresh_error") {
        return "reconnect";
    }
    if server.last_error.is_empty() {
        "ok"
    } else {
        "error"
    }
}

fn view(server: McpServer, creds: &BTreeMap<String, String>) -> ServerView {
    let status = status_of(&server, creds);
    ServerView {
        status,
        name: server.name,
        label: server.label,
        transport: server.transport,
        command: server.command,
        args: server.args.0,
        url: server.url,
        auth: server.auth,
        secret_keys: server.secret_keys.0,
        tools: server.tools.0,
        enabled_tools: server.enabled_tools.0,
        tools_refreshed_at: server.tools_refreshed_at,
        last_error: server.last_error,
        builtin: false,
    }
}

/// A built-in server as a read-only row: configured under Integrations, its
/// tools fixed and read-only, all of them enabled.
async fn builtin_view(state: &Arc<RunsState>, name: &str) -> ServerView {
    let connected = match name {
        "sentry" => !state.sentry_env(None).await.is_empty(),
        "grafana" => !state.grafana_env().await.is_empty(),
        _ => false,
    };
    let tools: Vec<McpTool> = harness_runner::mcp_servers::tool_names(name)
        .iter()
        .map(|t| McpTool {
            name: t.to_string(),
            description: String::new(),
            read_only: true,
        })
        .collect();
    ServerView {
        name: name.to_string(),
        label: match name {
            "sentry" => "Sentry",
            "grafana" => "Grafana",
            other => other,
        }
        .to_string(),
        transport: "builtin".to_string(),
        command: String::new(),
        args: Vec::new(),
        url: String::new(),
        auth: "builtin".to_string(),
        secret_keys: Vec::new(),
        enabled_tools: tools.iter().map(|t| t.name.clone()).collect(),
        tools,
        tools_refreshed_at: None,
        status: if connected { "ok" } else { "not_connected" },
        last_error: String::new(),
        builtin: true,
    }
}

async fn creds_of(store: &CredentialStore, name: &str) -> BTreeMap<String, String> {
    store
        .get(&mcp_credential_key(name))
        .await
        .ok()
        .flatten()
        .unwrap_or_default()
}

// ── The save-time snapshot ───────────────────────────────────────────────────

/// Publish the registry to save-time validation (see
/// [`harness_runner::mcp_servers::set_catalog`]). Called at start-up and after
/// every change; on a database error the snapshot is left as it was.
pub(crate) async fn sync_catalog(state: &Arc<RunsState>) {
    let Ok(store) = state.mcp_server_store().await else {
        return;
    };
    match store.list().await {
        Ok(list) => harness_runner::mcp_servers::set_catalog(
            list.into_iter()
                .map(|s| (s.name, s.enabled_tools.0))
                .collect::<HashMap<_, _>>(),
        ),
        Err(e) => tracing::warn!("mcp registry: could not load servers: {e}"),
    }
}

/// The servers a step can ask for, with the tools an admin enabled on each —
/// the only ones a step may name. For the workflow catalog, which is what an
/// authoring agent reads before writing `mcp:`.
pub(crate) async fn catalog_entries(state: &Arc<RunsState>) -> Value {
    let mut out: Vec<Value> = harness_runner::mcp_servers::BUILTIN_SERVERS
        .iter()
        .map(|name| {
            json!({
                "name": name,
                "builtin": true,
                "enabled_tools": harness_runner::mcp_servers::tool_names(name),
            })
        })
        .collect();
    if let Ok(store) = state.mcp_server_store().await {
        for s in store.list().await.unwrap_or_default() {
            out.push(json!({
                "name": s.name,
                "label": s.label,
                "builtin": false,
                "enabled_tools": s.enabled_tools.0,
            }));
        }
    }
    Value::Array(out)
}

// ── Routes ───────────────────────────────────────────────────────────────────

/// `GET /api/mcp-servers` — open to every signed-in person: the step editor
/// needs the names and enabled tools to offer them.
pub async fn list(Extension(state): Extension<Arc<RunsState>>) -> Response {
    let mut out = Vec::new();
    for name in harness_runner::mcp_servers::BUILTIN_SERVERS {
        out.push(builtin_view(&state, name).await);
    }
    // No database: the built-ins are still worth showing.
    if let (Ok(store), Ok(creds)) = (state.mcp_server_store().await, state.cred_store().await) {
        match store.list().await {
            Ok(list) => {
                for s in list {
                    let c = creds_of(creds, &s.name).await;
                    out.push(view(s, &c));
                }
            }
            Err(e) => return err(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()),
        }
    }
    Json(json!({ "servers": out })).into_response()
}

/// What Settings sends to add or change a server.
#[derive(Debug, Deserialize)]
pub struct ServerInput {
    #[serde(default)]
    name: String,
    #[serde(default)]
    label: String,
    transport: String,
    #[serde(default)]
    command: String,
    #[serde(default)]
    args: Vec<String>,
    #[serde(default)]
    url: String,
    #[serde(default = "default_auth")]
    auth: String,
    /// The secret names to keep. A name left out is deleted with its value.
    #[serde(default)]
    secret_keys: Vec<String>,
    /// New values, by name: added to the kept names. Write-only — the values
    /// are never sent back.
    #[serde(default)]
    secrets: BTreeMap<String, String>,
}

fn default_auth() -> String {
    "none".to_string()
}

/// Check an input and apply it to `server` (new or existing).
fn apply(server: &mut McpServer, input: &ServerInput) -> Result<(), String> {
    match input.transport.as_str() {
        "stdio" => {
            if input.command.trim().is_empty() {
                return Err("a local server needs the command that starts it".into());
            }
        }
        "http" => {
            let url = reqwest::Url::parse(input.url.trim())
                .map_err(|_| format!("`{}` is not a URL", input.url.trim()))?;
            if !matches!(url.scheme(), "http" | "https") {
                return Err("a remote server's URL must start with https:// or http://".into());
            }
        }
        other => return Err(format!("unknown transport `{other}` (use stdio or http)")),
    }
    match input.auth.as_str() {
        "none" | "secret" => {}
        "oauth" if input.transport == "http" => {}
        "oauth" => return Err("OAuth sign-in is only for remote (http) servers".into()),
        other => {
            return Err(format!(
                "unknown auth `{other}` (use none, secret or oauth)"
            ))
        }
    }
    let mut keys: Vec<String> = input.secret_keys.clone();
    keys.extend(input.secrets.keys().cloned());
    keys.sort();
    keys.dedup();
    for k in &keys {
        let ok = !k.is_empty()
            && k.len() <= 100
            && k.chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-');
        if !ok {
            return Err(format!(
                "`{k}` is not a usable {} name (letters, digits, `_`, `-`)",
                if input.transport == "http" {
                    "header"
                } else {
                    "variable"
                }
            ));
        }
    }
    server.label = input.label.trim().to_string();
    server.transport = input.transport.clone();
    server.command = input.command.trim().to_string();
    server.args = SqlJson(input.args.clone());
    server.url = input.url.trim().to_string();
    server.auth = input.auth.clone();
    server.secret_keys = SqlJson(keys);
    if server.transport == "stdio" {
        server.url.clear();
    } else {
        server.command.clear();
        server.args = SqlJson(Vec::new());
    }
    Ok(())
}

/// Store the input's secrets: kept names keep their values, new values are
/// written, and everything else that was a secret is deleted. OAuth fields are
/// left alone unless the server stopped using OAuth.
async fn store_secrets(
    store: &CredentialStore,
    server: &McpServer,
    input: &ServerInput,
) -> Result<(), String> {
    let key = mcp_credential_key(&server.name);
    let old = creds_of(store, &server.name).await;
    let mut fields: BTreeMap<String, String> = BTreeMap::new();
    for (k, v) in &old {
        match k.strip_prefix(SECRET_PREFIX) {
            Some(name) if server.secret_keys.0.iter().any(|s| s == name) => {
                fields.insert(k.clone(), v.clone());
            }
            Some(_) => {}
            None if server.auth == "oauth" => {
                fields.insert(k.clone(), v.clone());
            }
            None => {}
        }
    }
    for (k, v) in &input.secrets {
        fields.insert(format!("{SECRET_PREFIX}{k}"), v.clone());
    }
    store.delete(&key).await.map_err(|e| e.to_string())?;
    if !fields.is_empty() {
        store.set(&key, &fields).await.map_err(|e| e.to_string())?;
    }
    Ok(())
}

/// `POST /api/mcp-servers`
pub async fn create(
    _: AdminOnly,
    Extension(state): Extension<Arc<RunsState>>,
    Json(input): Json<ServerInput>,
) -> Response {
    let name = input.name.trim().to_string();
    if !harness_dag::is_valid_mcp_server_name(&name) {
        return err(
            StatusCode::BAD_REQUEST,
            "use a short name of lowercase letters, digits, `-` and `_` — steps refer to it",
        );
    }
    if harness_runner::mcp_servers::is_builtin(&name) || RESERVED_NAMES.contains(&name.as_str()) {
        return err(
            StatusCode::BAD_REQUEST,
            format!("`{name}` is taken by the harness; choose another name"),
        );
    }
    let (store, creds) = match (state.mcp_server_store().await, state.cred_store().await) {
        (Ok(s), Ok(c)) => (s, c),
        (Err(e), _) | (_, Err(e)) => return err(StatusCode::SERVICE_UNAVAILABLE, e),
    };
    match store.get(&name).await {
        Ok(Some(_)) => {
            return err(
                StatusCode::CONFLICT,
                format!("a server called `{name}` exists"),
            )
        }
        Ok(None) => {}
        Err(e) => return err(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()),
    }
    let mut server = McpServer::new(&name, &input.transport);
    if let Err(e) = apply(&mut server, &input) {
        return err(StatusCode::BAD_REQUEST, e);
    }
    if let Err(e) = store.save(&server).await {
        return err(StatusCode::INTERNAL_SERVER_ERROR, e.to_string());
    }
    if let Err(e) = store_secrets(creds, &server, &input).await {
        return err(StatusCode::INTERNAL_SERVER_ERROR, e);
    }
    finish_change(&state, &name).await
}

/// `PUT /api/mcp-servers/{name}`
pub async fn update(
    _: AdminOnly,
    Extension(state): Extension<Arc<RunsState>>,
    Path(name): Path<String>,
    Json(input): Json<ServerInput>,
) -> Response {
    let (store, creds) = match (state.mcp_server_store().await, state.cred_store().await) {
        (Ok(s), Ok(c)) => (s, c),
        (Err(e), _) | (_, Err(e)) => return err(StatusCode::SERVICE_UNAVAILABLE, e),
    };
    let mut server = match store.get(&name).await {
        Ok(Some(s)) => s,
        Ok(None) => return err(StatusCode::NOT_FOUND, format!("no server `{name}`")),
        Err(e) => return err(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()),
    };
    if let Err(e) = apply(&mut server, &input) {
        return err(StatusCode::BAD_REQUEST, e);
    }
    server.last_error.clear();
    if let Err(e) = store.save(&server).await {
        return err(StatusCode::INTERNAL_SERVER_ERROR, e.to_string());
    }
    if let Err(e) = store_secrets(creds, &server, &input).await {
        return err(StatusCode::INTERNAL_SERVER_ERROR, e);
    }
    finish_change(&state, &name).await
}

/// After a server is added or changed: list its tools if it can be reached
/// yet, publish the registry, and answer with the server as it now stands.
async fn finish_change(state: &Arc<RunsState>, name: &str) -> Response {
    if let Err(e) = refresh_tools(state, name).await {
        tracing::info!("mcp registry: `{name}` saved but not reached: {e}");
    }
    sync_catalog(state).await;
    one(state, name).await
}

async fn one(state: &Arc<RunsState>, name: &str) -> Response {
    let (store, creds) = match (state.mcp_server_store().await, state.cred_store().await) {
        (Ok(s), Ok(c)) => (s, c),
        (Err(e), _) | (_, Err(e)) => return err(StatusCode::SERVICE_UNAVAILABLE, e),
    };
    match store.get(name).await {
        Ok(Some(s)) => {
            let c = creds_of(creds, name).await;
            Json(view(s, &c)).into_response()
        }
        Ok(None) => err(StatusCode::NOT_FOUND, format!("no server `{name}`")),
        Err(e) => err(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()),
    }
}

/// `DELETE /api/mcp-servers/{name}`. Workflows that name it keep running,
/// without it — the step's prompt says the server is unavailable.
pub async fn remove(
    _: AdminOnly,
    Extension(state): Extension<Arc<RunsState>>,
    Path(name): Path<String>,
) -> Response {
    let (store, creds) = match (state.mcp_server_store().await, state.cred_store().await) {
        (Ok(s), Ok(c)) => (s, c),
        (Err(e), _) | (_, Err(e)) => return err(StatusCode::SERVICE_UNAVAILABLE, e),
    };
    match store.delete(&name).await {
        Ok(true) => {}
        Ok(false) => return err(StatusCode::NOT_FOUND, format!("no server `{name}`")),
        Err(e) => return err(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()),
    }
    let _ = creds.delete(&mcp_credential_key(&name)).await;
    sync_catalog(&state).await;
    StatusCode::NO_CONTENT.into_response()
}

#[derive(Debug, Deserialize)]
pub struct ToolsInput {
    enabled: Vec<String>,
}

/// `PUT /api/mcp-servers/{name}/tools` — which listed tools steps may use.
pub async fn set_tools(
    _: AdminOnly,
    Extension(state): Extension<Arc<RunsState>>,
    Path(name): Path<String>,
    Json(input): Json<ToolsInput>,
) -> Response {
    let store = match state.mcp_server_store().await {
        Ok(s) => s,
        Err(e) => return err(StatusCode::SERVICE_UNAVAILABLE, e),
    };
    let mut server = match store.get(&name).await {
        Ok(Some(s)) => s,
        Ok(None) => return err(StatusCode::NOT_FOUND, format!("no server `{name}`")),
        Err(e) => return err(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()),
    };
    let unknown: Vec<&str> = input
        .enabled
        .iter()
        .filter(|t| !server.tools.0.iter().any(|x| &x.name == *t))
        .map(String::as_str)
        .collect();
    if !unknown.is_empty() {
        return err(
            StatusCode::BAD_REQUEST,
            format!(
                "`{name}` does not offer {} — refresh its tools first",
                unknown.join(", ")
            ),
        );
    }
    let mut enabled = input.enabled;
    enabled.sort();
    enabled.dedup();
    server.enabled_tools = SqlJson(enabled);
    if let Err(e) = store.save(&server).await {
        return err(StatusCode::INTERNAL_SERVER_ERROR, e.to_string());
    }
    sync_catalog(&state).await;
    one(&state, &name).await
}

/// `POST /api/mcp-servers/{name}/refresh`
pub async fn refresh(
    _: AdminOnly,
    Extension(state): Extension<Arc<RunsState>>,
    Path(name): Path<String>,
) -> Response {
    if let Err(e) = refresh_tools(&state, &name).await {
        return err(StatusCode::BAD_GATEWAY, e);
    }
    sync_catalog(&state).await;
    one(&state, &name).await
}

/// Connect to `name`, list its tools, and save them.
///
/// The first listing enables the tools that say they are read-only; later
/// listings keep what was enabled and leave any new tool off — a server that
/// grows a "delete everything" tool doesn't hand it to every step.
pub(crate) async fn refresh_tools(state: &Arc<RunsState>, name: &str) -> Result<(), String> {
    let store = state.mcp_server_store().await?;
    let mut server = store
        .get(name)
        .await
        .map_err(|e| e.to_string())?
        .ok_or_else(|| format!("no server `{name}`"))?;
    let listed = async {
        let upstream = registry_upstream(state, &server).await?;
        let client = McpClient::connect(&upstream)
            .await
            .map_err(|e| describe(&server, e))?;
        client.list_tools().await.map_err(|e| describe(&server, e))
    }
    .await;
    let tools = match listed {
        Ok(t) => t,
        Err(e) => {
            let _ = store.set_error(name, &e).await;
            return Err(e);
        }
    };
    let tools: Vec<McpTool> = tools.iter().filter_map(registry_tool).collect();
    let enabled: Vec<String> = if server.tools_refreshed_at.is_none() {
        tools
            .iter()
            .filter(|t| t.read_only)
            .map(|t| t.name.clone())
            .collect()
    } else {
        server
            .enabled_tools
            .0
            .iter()
            .filter(|e| tools.iter().any(|t| &t.name == *e))
            .cloned()
            .collect()
    };
    server.tools = SqlJson(tools);
    server.enabled_tools = SqlJson(enabled);
    server.tools_refreshed_at = Some(chrono::Utc::now());
    server.last_error.clear();
    store.save(&server).await.map_err(|e| e.to_string())?;
    Ok(())
}

fn describe(server: &McpServer, e: ClientError) -> String {
    match e {
        ClientError::Unauthorized(m) if server.auth == "oauth" => {
            format!("the sign-in was refused ({m}) — reconnect")
        }
        ClientError::Unauthorized(m) => format!("the credentials were refused ({m})"),
        ClientError::Other(m) => m,
    }
}

// ── Reaching a server ────────────────────────────────────────────────────────

/// How to reach a registry server, secrets filled in. For OAuth, the access
/// token is refreshed first when it is about to expire.
pub(crate) async fn registry_upstream(
    state: &Arc<RunsState>,
    server: &McpServer,
) -> Result<Upstream, String> {
    let creds_store = state.cred_store().await?;
    let mut creds = creds_of(creds_store, &server.name).await;
    let secrets: BTreeMap<String, String> = creds
        .iter()
        .filter_map(|(k, v)| {
            k.strip_prefix(SECRET_PREFIX)
                .map(|n| (n.to_string(), v.clone()))
        })
        .collect();
    match server.transport.as_str() {
        "stdio" => Ok(Upstream::Stdio {
            command: server.command.clone(),
            args: server.args.0.clone(),
            env: secrets,
        }),
        "http" => {
            let mut headers = secrets;
            if server.auth == "oauth" {
                let token = fresh_access_token(creds_store, &server.name, &mut creds).await?;
                headers.insert("Authorization".to_string(), format!("Bearer {token}"));
            }
            Ok(Upstream::Http {
                url: server.url.clone(),
                headers,
            })
        }
        other => Err(format!("unknown transport `{other}`")),
    }
}

/// How to reach a built-in server for `project`, when a step narrows its tools
/// and so goes through the gateway. The same command and arguments the agent
/// would start itself, with the `${VAR}` placeholders filled in here.
pub(crate) async fn builtin_upstream(
    state: &Arc<RunsState>,
    name: &str,
    project: &str,
) -> Result<Upstream, String> {
    let env: HashMap<String, String> = match name {
        "sentry" => state.sentry_env(Some(project)).await,
        "grafana" => state.grafana_env().await,
        _ => Vec::new(),
    }
    .into_iter()
    .collect();
    let (doc, _) = harness_runner::mcp_servers::resolve(&[name.to_string()], &env);
    let cfg = doc
        .as_ref()
        .and_then(|d| d["mcpServers"].get(name))
        .ok_or_else(|| format!("{name} is not connected"))?;
    let fill = |s: &str| expand_placeholders(s, &env);
    Ok(Upstream::Stdio {
        command: cfg["command"].as_str().unwrap_or_default().to_string(),
        args: cfg["args"]
            .as_array()
            .map(|a| a.iter().filter_map(Value::as_str).map(fill).collect())
            .unwrap_or_default(),
        env: cfg["env"]
            .as_object()
            .map(|m| {
                m.iter()
                    .map(|(k, v)| (k.clone(), fill(v.as_str().unwrap_or_default())))
                    .collect()
            })
            .unwrap_or_default(),
    })
}

/// Replace each `${NAME}` with its value from `env` (empty when absent).
fn expand_placeholders(s: &str, env: &HashMap<String, String>) -> String {
    let mut out = String::with_capacity(s.len());
    let mut rest = s;
    while let Some(start) = rest.find("${") {
        out.push_str(&rest[..start]);
        match rest[start + 2..].find('}') {
            Some(end) => {
                let key = &rest[start + 2..start + 2 + end];
                out.push_str(env.get(key).map(String::as_str).unwrap_or_default());
                rest = &rest[start + 2 + end + 1..];
            }
            None => {
                out.push_str(&rest[start..]);
                rest = "";
            }
        }
    }
    out.push_str(rest);
    out
}

// ── OAuth ────────────────────────────────────────────────────────────────────
//
// The MCP authorization spec: the server names its authorization server in
// protected-resource metadata; the harness registers itself as a client there
// (dynamic client registration) unless an admin pasted a client ID, then runs
// the authorization-code flow with PKCE. The tokens belong to the shared
// service account an admin signs in as.

/// The endpoints the sign-in uses.
#[derive(Debug, Clone, PartialEq, Eq)]
struct AuthServer {
    authorization_endpoint: String,
    token_endpoint: String,
    registration_endpoint: Option<String>,
    scopes: Vec<String>,
}

/// An authorization in flight, keyed by its `state` nonce.
#[derive(Debug, Clone)]
struct Pending {
    server: String,
    verifier: String,
    token_endpoint: String,
    client_id: String,
    client_secret: Option<String>,
    redirect_uri: String,
    resource: String,
    created_ms: i64,
}

static PENDING: LazyLock<std::sync::Mutex<HashMap<String, Pending>>> =
    LazyLock::new(Default::default);

const PENDING_TTL_MS: i64 = 15 * 60 * 1000;

/// `<origin>/.well-known/<suffix>` with the URL's path appended, then without
/// it — the order the specs (RFC 9728, RFC 8414) ask clients to try.
fn well_known(url: &str, suffix: &str) -> Vec<String> {
    let Ok(u) = reqwest::Url::parse(url) else {
        return Vec::new();
    };
    let origin = u.origin().ascii_serialization();
    let path = u.path().trim_end_matches('/');
    let mut out = Vec::new();
    if !path.is_empty() {
        out.push(format!("{origin}/.well-known/{suffix}{path}"));
    }
    out.push(format!("{origin}/.well-known/{suffix}"));
    out
}

/// `resource_metadata="…"` from a `WWW-Authenticate` header.
fn resource_metadata_hint(www_authenticate: &str) -> Option<String> {
    let at = www_authenticate.find("resource_metadata=")?;
    let rest = &www_authenticate[at + "resource_metadata=".len()..];
    let rest = rest.strip_prefix('"').unwrap_or(rest);
    let end = rest.find(['"', ',']).unwrap_or(rest.len());
    Some(rest[..end].trim().to_string()).filter(|s| !s.is_empty())
}

async fn get_json(http: &reqwest::Client, url: &str) -> Option<Value> {
    let resp = http
        .get(url)
        .header(header::ACCEPT, "application/json")
        .timeout(std::time::Duration::from_secs(15))
        .send()
        .await
        .ok()?;
    if !resp.status().is_success() {
        return None;
    }
    resp.json::<Value>().await.ok()
}

/// Find the authorization server for the MCP server at `server_url`.
async fn discover(http: &reqwest::Client, server_url: &str) -> Result<AuthServer, String> {
    // An unauthenticated request is answered 401 with a pointer to the
    // protected-resource metadata.
    let mut prm_urls = Vec::new();
    if let Ok(resp) = http
        .post(server_url)
        .header(header::ACCEPT, "application/json, text/event-stream")
        .json(&json!({
            "jsonrpc": "2.0", "id": 0, "method": "initialize",
            "params": {
                "protocolVersion": super::mcp_client::PROTOCOL_VERSION,
                "capabilities": {},
                "clientInfo": { "name": "ai-harness", "version": env!("CARGO_PKG_VERSION") }
            }
        }))
        .timeout(std::time::Duration::from_secs(15))
        .send()
        .await
    {
        if let Some(hint) = resp
            .headers()
            .get(header::WWW_AUTHENTICATE)
            .and_then(|v| v.to_str().ok())
            .and_then(resource_metadata_hint)
        {
            prm_urls.push(hint);
        }
    }
    prm_urls.extend(well_known(server_url, "oauth-protected-resource"));
    let mut prm = None;
    for u in &prm_urls {
        if let Some(v) = get_json(http, u).await {
            prm = Some(v);
            break;
        }
    }
    let origin = reqwest::Url::parse(server_url)
        .map_err(|e| e.to_string())?
        .origin()
        .ascii_serialization();
    let issuer = prm
        .as_ref()
        .and_then(|p| p["authorization_servers"].get(0))
        .and_then(Value::as_str)
        .map(str::to_string)
        .unwrap_or_else(|| origin.clone());
    let scopes: Vec<String> = prm
        .as_ref()
        .and_then(|p| p["scopes_supported"].as_array())
        .map(|a| {
            a.iter()
                .filter_map(Value::as_str)
                .map(str::to_string)
                .collect()
        })
        .unwrap_or_default();

    let mut as_urls = well_known(&issuer, "oauth-authorization-server");
    as_urls.extend(well_known(&issuer, "openid-configuration"));
    as_urls.push(format!(
        "{}/.well-known/openid-configuration",
        issuer.trim_end_matches('/')
    ));
    for u in &as_urls {
        if let Some(meta) = get_json(http, u).await {
            let field = |k: &str| meta[k].as_str().map(str::to_string);
            if let (Some(authorization_endpoint), Some(token_endpoint)) =
                (field("authorization_endpoint"), field("token_endpoint"))
            {
                return Ok(AuthServer {
                    authorization_endpoint,
                    token_endpoint,
                    registration_endpoint: field("registration_endpoint"),
                    scopes,
                });
            }
        }
    }
    // The 2025-03 spec's fallback: endpoints at fixed paths on the server's origin.
    Ok(AuthServer {
        authorization_endpoint: format!("{origin}/authorize"),
        token_endpoint: format!("{origin}/token"),
        registration_endpoint: Some(format!("{origin}/register")),
        scopes,
    })
}

/// Register the harness as an OAuth client (RFC 7591).
async fn register_client(
    http: &reqwest::Client,
    endpoint: &str,
    redirect_uri: &str,
) -> Result<(String, Option<String>), String> {
    let resp = http
        .post(endpoint)
        .json(&json!({
            "client_name": "ai-harness",
            "redirect_uris": [redirect_uri],
            "grant_types": ["authorization_code", "refresh_token"],
            "response_types": ["code"],
            "token_endpoint_auth_method": "none",
        }))
        .timeout(std::time::Duration::from_secs(15))
        .send()
        .await
        .map_err(|e| e.to_string())?;
    let status = resp.status();
    let body: Value = resp.json().await.unwrap_or(Value::Null);
    let id = body["client_id"].as_str().filter(|s| !s.is_empty());
    match id {
        Some(id) if status.is_success() => Ok((
            id.to_string(),
            body["client_secret"].as_str().map(str::to_string),
        )),
        _ => Err(format!(
            "the server did not register the harness as a client ({status}) — paste a client \
             ID from the provider instead"
        )),
    }
}

fn pkce_challenge(verifier: &str) -> String {
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(Sha256::digest(verifier.as_bytes()))
}

fn nonce() -> String {
    format!(
        "{}{}",
        uuid::Uuid::new_v4().simple(),
        uuid::Uuid::new_v4().simple()
    )
}

fn build_authorize_url(auth: &AuthServer, p: &Pending, state: &str) -> String {
    let mut url = reqwest::Url::parse(&auth.authorization_endpoint)
        .unwrap_or_else(|_| reqwest::Url::parse("http://invalid/").expect("static url"));
    {
        let mut q = url.query_pairs_mut();
        q.append_pair("response_type", "code")
            .append_pair("client_id", &p.client_id)
            .append_pair("redirect_uri", &p.redirect_uri)
            .append_pair("code_challenge", &pkce_challenge(&p.verifier))
            .append_pair("code_challenge_method", "S256")
            .append_pair("state", state)
            .append_pair("resource", &p.resource);
        if !auth.scopes.is_empty() {
            q.append_pair("scope", &auth.scopes.join(" "));
        }
    }
    url.to_string()
}

#[derive(Debug, Deserialize, Default)]
pub struct StartInput {
    /// For a provider without dynamic registration: a client an admin made there.
    #[serde(default)]
    client_id: String,
    #[serde(default)]
    client_secret: String,
}

/// `POST /api/mcp-servers/{name}/oauth/start` — `{ url }` to send the browser to.
pub async fn oauth_start(
    _: AdminOnly,
    Extension(state): Extension<Arc<RunsState>>,
    Path(name): Path<String>,
    body: Option<Json<StartInput>>,
) -> Response {
    let input = body.map(|b| b.0).unwrap_or_default();
    let (store, creds) = match (state.mcp_server_store().await, state.cred_store().await) {
        (Ok(s), Ok(c)) => (s, c),
        (Err(e), _) | (_, Err(e)) => return err(StatusCode::SERVICE_UNAVAILABLE, e),
    };
    let server = match store.get(&name).await {
        Ok(Some(s)) if s.auth == "oauth" => s,
        Ok(Some(_)) => return err(StatusCode::BAD_REQUEST, "this server doesn't use OAuth"),
        Ok(None) => return err(StatusCode::NOT_FOUND, format!("no server `{name}`")),
        Err(e) => return err(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()),
    };
    let Some(base) = state.public_url() else {
        return err(
            StatusCode::PRECONDITION_FAILED,
            "no public URL configured — set one under Settings → General so the sign-in has \
             somewhere to return to",
        );
    };
    let redirect_uri = format!("{}{CALLBACK_PATH}", base.trim_end_matches('/'));
    let http = reqwest::Client::new();
    let auth = match discover(&http, &server.url).await {
        Ok(a) => a,
        Err(e) => return err(StatusCode::BAD_GATEWAY, e),
    };
    let saved = creds_of(creds, &name).await;
    let field = |k: &str| saved.get(k).filter(|v| !v.is_empty()).cloned();
    // A client pasted now wins; then one registered (or pasted) before; then
    // register a new one.
    let (client_id, client_secret) = if !input.client_id.trim().is_empty() {
        (
            input.client_id.trim().to_string(),
            Some(input.client_secret.trim().to_string()).filter(|s| !s.is_empty()),
        )
    } else if let Some(id) = field("oauth_client_id") {
        (id, field("oauth_client_secret"))
    } else {
        let Some(endpoint) = auth.registration_endpoint.as_deref() else {
            return err(
                StatusCode::PRECONDITION_FAILED,
                "this server's sign-in doesn't let the harness register itself — create an \
                 OAuth client with the provider and paste its client ID",
            );
        };
        match register_client(&http, endpoint, &redirect_uri).await {
            Ok(c) => c,
            Err(e) => return err(StatusCode::BAD_GATEWAY, e),
        }
    };
    let mut fields = BTreeMap::from([("oauth_client_id".to_string(), client_id.clone())]);
    fields.insert(
        "oauth_client_secret".to_string(),
        client_secret.clone().unwrap_or_default(),
    );
    if let Err(e) = creds.set(&mcp_credential_key(&name), &fields).await {
        return err(StatusCode::INTERNAL_SERVER_ERROR, e.to_string());
    }
    let pending = Pending {
        server: name.clone(),
        verifier: nonce(),
        token_endpoint: auth.token_endpoint.clone(),
        client_id,
        client_secret,
        redirect_uri: redirect_uri.clone(),
        resource: server.url.clone(),
        created_ms: now_ms(),
    };
    let state_nonce = nonce();
    let url = build_authorize_url(&auth, &pending, &state_nonce);
    if let Ok(mut map) = PENDING.lock() {
        let now = now_ms();
        map.retain(|_, p| now - p.created_ms < PENDING_TTL_MS);
        map.insert(state_nonce, pending);
    }
    Json(json!({ "url": url, "callback_url": redirect_uri })).into_response()
}

fn take_pending(nonce: &str) -> Option<Pending> {
    let p = PENDING.lock().ok()?.remove(nonce)?;
    (now_ms() - p.created_ms < PENDING_TTL_MS).then_some(p)
}

fn back(status: &str, name: Option<&str>, detail: Option<&str>) -> Response {
    let mut url = reqwest::Url::parse("http://x/").expect("static url");
    {
        let mut q = url.query_pairs_mut();
        q.append_pair("mcp", status);
        if let Some(n) = name {
            q.append_pair("server", n);
        }
        if let Some(d) = detail {
            q.append_pair("message", &d.chars().take(200).collect::<String>());
        }
    }
    let location = format!("{UI_PATH}?{}", url.query().unwrap_or_default());
    (StatusCode::SEE_OTHER, [(header::LOCATION, location)]).into_response()
}

#[derive(Debug, Deserialize)]
pub struct CallbackQuery {
    code: Option<String>,
    state: Option<String>,
    error: Option<String>,
    error_description: Option<String>,
}

/// The token endpoint's answer, as stored.
fn token_fields(
    body: &Value,
    keep_refresh: Option<&str>,
) -> Result<BTreeMap<String, String>, String> {
    let access = body["access_token"]
        .as_str()
        .filter(|s| !s.is_empty())
        .ok_or_else(|| {
            let e = body["error_description"]
                .as_str()
                .or(body["error"].as_str())
                .unwrap_or("no access token in the answer");
            e.to_string()
        })?;
    let mut f = BTreeMap::new();
    f.insert("access_token".to_string(), access.to_string());
    // A refresh that doesn't rotate the refresh token keeps the old one.
    let refresh = body["refresh_token"]
        .as_str()
        .map(str::to_string)
        .or_else(|| keep_refresh.map(str::to_string))
        .unwrap_or_default();
    f.insert("refresh_token".to_string(), refresh);
    let expires = body["expires_in"]
        .as_i64()
        .or_else(|| body["expires_in"].as_str().and_then(|s| s.parse().ok()))
        .map(|secs| (now_ms() + secs * 1000).to_string())
        .unwrap_or_default();
    f.insert("expires_at_ms".to_string(), expires);
    f.insert("refresh_error".to_string(), String::new());
    Ok(f)
}

async fn post_token(
    endpoint: &str,
    form: &[(&str, &str)],
    client_secret: Option<&str>,
) -> Result<Value, String> {
    let mut req = reqwest::Client::new()
        .post(endpoint)
        .header(header::ACCEPT, "application/json")
        .timeout(std::time::Duration::from_secs(20));
    let mut form: Vec<(&str, &str)> = form.to_vec();
    if let Some(secret) = client_secret.filter(|s| !s.is_empty()) {
        form.push(("client_secret", secret));
    }
    req = req.form(&form);
    let resp = req.send().await.map_err(|e| e.to_string())?;
    let status = resp.status();
    let body: Value = resp.json().await.unwrap_or(Value::Null);
    if !status.is_success() {
        let e = body["error_description"]
            .as_str()
            .or(body["error"].as_str())
            .unwrap_or("");
        return Err(format!("{status} {e}").trim().to_string());
    }
    Ok(body)
}

/// `GET /api/mcp-servers/oauth/callback`
pub async fn oauth_callback(
    Extension(state): Extension<Arc<RunsState>>,
    Query(q): Query<CallbackQuery>,
) -> Response {
    let Some(pending) = q.state.as_deref().and_then(take_pending) else {
        return back(
            "error",
            None,
            Some("the sign-in expired or was already used — start again"),
        );
    };
    let name = pending.server.clone();
    if let Some(e) = q.error.as_deref() {
        let detail = q.error_description.as_deref().unwrap_or(e);
        return back("denied", Some(&name), Some(detail));
    }
    let Some(code) = q.code.as_deref() else {
        return back("error", Some(&name), Some("the provider sent no code"));
    };
    let body = match post_token(
        &pending.token_endpoint,
        &[
            ("grant_type", "authorization_code"),
            ("code", code),
            ("redirect_uri", &pending.redirect_uri),
            ("client_id", &pending.client_id),
            ("code_verifier", &pending.verifier),
            ("resource", &pending.resource),
        ],
        pending.client_secret.as_deref(),
    )
    .await
    {
        Ok(b) => b,
        Err(e) => return back("error", Some(&name), Some(&e)),
    };
    let mut fields = match token_fields(&body, None) {
        Ok(f) => f,
        Err(e) => return back("error", Some(&name), Some(&e)),
    };
    fields.insert("token_endpoint".to_string(), pending.token_endpoint.clone());
    let creds = match state.cred_store().await {
        Ok(c) => c,
        Err(e) => return back("error", Some(&name), Some(&e)),
    };
    if let Err(e) = creds.set(&mcp_credential_key(&name), &fields).await {
        return back("error", Some(&name), Some(&e.to_string()));
    }
    let listed = refresh_tools(&state, &name).await;
    sync_catalog(&state).await;
    match listed {
        Ok(()) => back("connected", Some(&name), None),
        Err(e) => back("error", Some(&name), Some(&e)),
    }
}

/// `POST /api/mcp-servers/{name}/oauth/disconnect` — forget the tokens (the
/// registered client stays, so reconnecting doesn't register another).
pub async fn oauth_disconnect(
    _: AdminOnly,
    Extension(state): Extension<Arc<RunsState>>,
    Path(name): Path<String>,
) -> Response {
    let creds = match state.cred_store().await {
        Ok(c) => c,
        Err(e) => return err(StatusCode::SERVICE_UNAVAILABLE, e),
    };
    let key = mcp_credential_key(&name);
    let mut fields = creds_of(creds, &name).await;
    for k in [
        "access_token",
        "refresh_token",
        "expires_at_ms",
        "refresh_error",
    ] {
        fields.remove(k);
    }
    let _ = creds.delete(&key).await;
    if !fields.is_empty() {
        if let Err(e) = creds.set(&key, &fields).await {
            return err(StatusCode::INTERNAL_SERVER_ERROR, e.to_string());
        }
    }
    one(&state, &name).await
}

/// The access token to use now, refreshed first when it is close to expiry. A
/// refresh that fails is recorded, so Settings shows "reconnect".
async fn fresh_access_token(
    store: &CredentialStore,
    name: &str,
    creds: &mut BTreeMap<String, String>,
) -> Result<String, String> {
    let field = |c: &BTreeMap<String, String>, k: &str| c.get(k).filter(|v| !v.is_empty()).cloned();
    let access = field(creds, "access_token").ok_or_else(|| {
        format!("{name} is not signed in — connect it under Settings → MCP servers")
    })?;
    let expires: Option<i64> = field(creds, "expires_at_ms").and_then(|v| v.parse().ok());
    let due = expires.is_some_and(|at| now_ms() + REFRESH_MARGIN_MS >= at);
    if !due {
        return Ok(access);
    }
    let (Some(refresh), Some(endpoint), Some(client_id)) = (
        field(creds, "refresh_token"),
        field(creds, "token_endpoint"),
        field(creds, "oauth_client_id"),
    ) else {
        let msg = "the sign-in expired and can't be renewed — reconnect".to_string();
        record_refresh_error(store, name, &msg).await;
        return Err(format!("{name}: {msg}"));
    };
    let secret = field(creds, "oauth_client_secret");
    let body = post_token(
        &endpoint,
        &[
            ("grant_type", "refresh_token"),
            ("refresh_token", &refresh),
            ("client_id", &client_id),
        ],
        secret.as_deref(),
    )
    .await;
    let fields = body.and_then(|b| token_fields(&b, Some(&refresh)));
    match fields {
        Ok(fields) => {
            store
                .set(&mcp_credential_key(name), &fields)
                .await
                .map_err(|e| e.to_string())?;
            let token = fields["access_token"].clone();
            creds.extend(fields);
            Ok(token)
        }
        Err(e) => {
            let msg = format!("renewing the sign-in failed ({e}) — reconnect");
            record_refresh_error(store, name, &msg).await;
            Err(format!("{name}: {msg}"))
        }
    }
}

async fn record_refresh_error(store: &CredentialStore, name: &str, msg: &str) {
    let f = BTreeMap::from([("refresh_error".to_string(), msg.to_string())]);
    let _ = store.set(&mcp_credential_key(name), &f).await;
}

#[cfg(test)]
mod tests {
    use super::*;

    fn input(transport: &str) -> ServerInput {
        ServerInput {
            name: "x".into(),
            label: " Mail ".into(),
            transport: transport.into(),
            command: "npx".into(),
            args: vec!["-y".into(), "pkg".into()],
            url: "https://mcp.example.com/mcp".into(),
            auth: "none".into(),
            secret_keys: vec![],
            secrets: BTreeMap::new(),
        }
    }

    #[test]
    fn a_local_server_keeps_its_command_and_a_remote_one_its_url() {
        let mut s = McpServer::new("x", "stdio");
        apply(&mut s, &input("stdio")).unwrap();
        assert_eq!((s.command.as_str(), s.url.as_str()), ("npx", ""));
        assert_eq!(s.label, "Mail");
        apply(&mut s, &input("http")).unwrap();
        assert_eq!(s.command, "");
        assert!(s.args.0.is_empty());
        assert_eq!(s.url, "https://mcp.example.com/mcp");
    }

    #[test]
    fn bad_inputs_are_refused_with_a_reason() {
        let mut s = McpServer::new("x", "http");
        let mut i = input("http");
        i.url = "ftp://x".into();
        assert!(apply(&mut s, &i).unwrap_err().contains("https://"));
        let mut i = input("stdio");
        i.auth = "oauth".into();
        assert!(apply(&mut s, &i).unwrap_err().contains("remote"));
        let mut i = input("stdio");
        i.secrets.insert("BAD KEY".into(), "v".into());
        assert!(apply(&mut s, &i).unwrap_err().contains("BAD KEY"));
        let mut i = input("stdio");
        i.command = "  ".into();
        assert!(apply(&mut s, &i).is_err());
    }

    #[test]
    fn secret_names_are_the_kept_ones_plus_the_new_ones() {
        let mut s = McpServer::new("x", "stdio");
        let mut i = input("stdio");
        i.secret_keys = vec!["B".into()];
        i.secrets.insert("A".into(), "1".into());
        i.secrets.insert("B".into(), "2".into());
        apply(&mut s, &i).unwrap();
        assert_eq!(s.secret_keys.0, vec!["A".to_string(), "B".to_string()]);
    }

    #[test]
    fn status_reads_what_the_credentials_and_last_error_say() {
        let mut s = McpServer::new("x", "http");
        s.auth = "oauth".into();
        let none = BTreeMap::new();
        assert_eq!(status_of(&s, &none), "not_connected");
        let mut c = BTreeMap::from([("access_token".to_string(), "t".to_string())]);
        assert_eq!(status_of(&s, &c), "ok");
        c.insert("refresh_error".into(), "expired".into());
        assert_eq!(status_of(&s, &c), "reconnect");
        let mut plain = McpServer::new("y", "stdio");
        plain.last_error = "could not start".into();
        assert_eq!(status_of(&plain, &none), "error");
    }

    #[test]
    fn well_known_urls_try_the_path_first() {
        assert_eq!(
            well_known("https://mcp.example.com/v1/mcp", "oauth-protected-resource"),
            vec![
                "https://mcp.example.com/.well-known/oauth-protected-resource/v1/mcp".to_string(),
                "https://mcp.example.com/.well-known/oauth-protected-resource".to_string(),
            ]
        );
        assert_eq!(
            well_known("https://auth.example.com", "oauth-authorization-server"),
            vec!["https://auth.example.com/.well-known/oauth-authorization-server".to_string()]
        );
    }

    #[test]
    fn the_resource_metadata_hint_is_read_from_www_authenticate() {
        let h = r#"Bearer error="invalid_token", resource_metadata="https://x/.well-known/oauth-protected-resource""#;
        assert_eq!(
            resource_metadata_hint(h).as_deref(),
            Some("https://x/.well-known/oauth-protected-resource")
        );
        assert_eq!(resource_metadata_hint("Bearer realm=\"x\""), None);
    }

    #[test]
    fn the_authorize_url_carries_pkce_state_and_resource() {
        let auth = AuthServer {
            authorization_endpoint: "https://auth.example.com/authorize".into(),
            token_endpoint: "https://auth.example.com/token".into(),
            registration_endpoint: None,
            scopes: vec!["read".into(), "mail".into()],
        };
        let p = Pending {
            server: "m365".into(),
            verifier: "v".repeat(64),
            token_endpoint: auth.token_endpoint.clone(),
            client_id: "cid".into(),
            client_secret: None,
            redirect_uri: "https://harness.example.com/api/mcp-servers/oauth/callback".into(),
            resource: "https://mcp.example.com/mcp".into(),
            created_ms: 0,
        };
        let url = reqwest::Url::parse(&build_authorize_url(&auth, &p, "st")).unwrap();
        let q: HashMap<String, String> = url.query_pairs().into_owned().collect();
        assert_eq!(q["code_challenge"], pkce_challenge(&"v".repeat(64)));
        assert_eq!(q["code_challenge_method"], "S256");
        assert_eq!(q["state"], "st");
        assert_eq!(q["resource"], "https://mcp.example.com/mcp");
        assert_eq!(q["scope"], "read mail");
        assert_eq!(q["client_id"], "cid");
    }

    #[test]
    fn pkce_challenge_matches_the_rfc_example() {
        // RFC 7636, appendix B.
        assert_eq!(
            pkce_challenge("dBjftJeZ4CVP-mB92K27uhbUJU1p1r_wW1gFWFOEjXk"),
            "E9Melhoa2OwvFrEMTJguCHaoeK1t8URWbuGJSstw-cM"
        );
    }

    #[test]
    fn a_refresh_that_does_not_rotate_keeps_the_refresh_token() {
        let f = token_fields(
            &json!({ "access_token": "a2", "expires_in": 3600 }),
            Some("r1"),
        )
        .unwrap();
        assert_eq!(f["refresh_token"], "r1");
        assert!(f["expires_at_ms"].parse::<i64>().unwrap() > now_ms());
        assert_eq!(f["refresh_error"], "");
        let e = token_fields(&json!({ "error": "invalid_grant" }), None).unwrap_err();
        assert_eq!(e, "invalid_grant");
    }

    #[test]
    fn placeholders_are_filled_and_unknown_ones_left_empty() {
        let env = HashMap::from([("T".to_string(), "tok".to_string())]);
        assert_eq!(expand_placeholders("--org=${T}!", &env), "--org=tok!");
        assert_eq!(expand_placeholders("${NOPE}", &env), "");
        assert_eq!(
            expand_placeholders("plain ${unclosed", &env),
            "plain ${unclosed"
        );
    }

    #[test]
    fn the_callback_returns_to_settings_with_an_encoded_message() {
        let r = back("error", Some("m365"), Some("a & b"));
        let loc = r.headers()[header::LOCATION].to_str().unwrap();
        assert_eq!(
            loc,
            "/settings/mcp-servers?mcp=error&server=m365&message=a+%26+b"
        );
    }
}
