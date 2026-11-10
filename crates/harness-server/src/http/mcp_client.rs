//! A small **MCP client**, for the harness's own use: listing a registered
//! server's tools from Settings, and forwarding a step's allowed calls from the
//! gateway (see [`super::mcp_gateway`]).
//!
//! Two transports, the two the MCP spec defines: **stdio** (start a command,
//! newline-delimited JSON-RPC on its stdin/stdout) and **Streamable HTTP**
//! (POST each message; the reply is JSON or a short SSE stream). Only what the
//! harness needs is here: `initialize`, then requests answered by responses.
//! A server's own requests to the client (sampling, roots) are refused, and its
//! notifications are ignored.

use std::collections::BTreeMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use serde_json::{json, Value};
use tokio::io::{AsyncBufReadExt, AsyncRead, AsyncWrite, AsyncWriteExt, BufReader};
use tokio::sync::Mutex;

/// The protocol version the harness speaks, as client and as gateway.
pub(crate) const PROTOCOL_VERSION: &str = "2025-06-18";

/// How long one request may take. Generous: a tool call can be a search over
/// a large mailbox.
const REQUEST_TIMEOUT: Duration = Duration::from_secs(300);

/// How to reach a server. Values here are real — secrets included — so an
/// `Upstream` only ever exists inside the server process.
#[derive(Clone)]
pub(crate) enum Upstream {
    Stdio {
        command: String,
        args: Vec<String>,
        env: BTreeMap<String, String>,
    },
    Http {
        url: String,
        headers: BTreeMap<String, String>,
    },
}

impl std::fmt::Debug for Upstream {
    /// Never prints a value from `env` or `headers` — they hold the secrets.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Upstream::Stdio { command, args, env } => f
                .debug_struct("Stdio")
                .field("command", command)
                .field("args", args)
                .field("env", &env.keys().collect::<Vec<_>>())
                .finish(),
            Upstream::Http { url, headers } => f
                .debug_struct("Http")
                .field("url", url)
                .field("headers", &headers.keys().collect::<Vec<_>>())
                .finish(),
        }
    }
}

/// Why a request didn't get an answer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum ClientError {
    /// The server refused the credentials (HTTP 401/403): the connection
    /// needs to be made again.
    Unauthorized(String),
    Other(String),
}

impl std::fmt::Display for ClientError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ClientError::Unauthorized(m) => write!(f, "not authorized: {m}"),
            ClientError::Other(m) => f.write_str(m),
        }
    }
}

fn other(e: impl std::fmt::Display) -> ClientError {
    ClientError::Other(e.to_string())
}

/// A JSON-RPC answer: the `result`, or the `error` object as the server sent it.
pub(crate) type RpcAnswer = Result<Value, Value>;

/// A connected, initialized MCP client.
pub(crate) struct McpClient {
    transport: Transport,
    next_id: AtomicU64,
}

enum Transport {
    Stdio(Box<StdioConn>),
    Http(HttpConn),
}

impl McpClient {
    /// Connect to `upstream` and complete the `initialize` handshake.
    pub(crate) async fn connect(upstream: &Upstream) -> Result<Self, ClientError> {
        let transport = match upstream {
            Upstream::Stdio { command, args, env } => {
                Transport::Stdio(Box::new(StdioConn::spawn(command, args, env)?))
            }
            Upstream::Http { url, headers } => Transport::Http(HttpConn::new(url, headers)?),
        };
        let client = Self {
            transport,
            next_id: AtomicU64::new(1),
        };
        client.initialize().await?;
        Ok(client)
    }

    #[cfg(test)]
    fn over_streams(
        reader: impl AsyncRead + Unpin + Send + 'static,
        writer: impl AsyncWrite + Unpin + Send + 'static,
    ) -> Self {
        Self {
            transport: Transport::Stdio(Box::new(StdioConn::from_streams(None, reader, writer))),
            next_id: AtomicU64::new(1),
        }
    }

    async fn initialize(&self) -> Result<(), ClientError> {
        let params = json!({
            "protocolVersion": PROTOCOL_VERSION,
            "capabilities": {},
            "clientInfo": { "name": "ai-harness", "version": env!("CARGO_PKG_VERSION") },
        });
        match self.request("initialize", params).await? {
            Ok(_) => {}
            Err(e) => return Err(ClientError::Other(format!("initialize refused: {e}"))),
        }
        self.notify("notifications/initialized", json!({})).await
    }

    /// Send one request and wait for its answer.
    pub(crate) async fn request(
        &self,
        method: &str,
        params: Value,
    ) -> Result<RpcAnswer, ClientError> {
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        let msg = json!({ "jsonrpc": "2.0", "id": id, "method": method, "params": params });
        let reply = tokio::time::timeout(REQUEST_TIMEOUT, async {
            match &self.transport {
                Transport::Stdio(c) => c.request(&msg, id).await,
                Transport::Http(c) => c.request(&msg, id).await,
            }
        })
        .await
        .map_err(|_| ClientError::Other(format!("`{method}` timed out")))??;
        Ok(answer_of(reply))
    }

    async fn notify(&self, method: &str, params: Value) -> Result<(), ClientError> {
        let msg = json!({ "jsonrpc": "2.0", "method": method, "params": params });
        match &self.transport {
            Transport::Stdio(c) => c.send(&msg).await,
            Transport::Http(c) => c.notify(&msg).await,
        }
    }

    /// Every tool the server offers, following `nextCursor` pages.
    pub(crate) async fn list_tools(&self) -> Result<Vec<Value>, ClientError> {
        let mut tools = Vec::new();
        let mut cursor: Option<String> = None;
        // A bound on pages, so a server that hands back the same cursor
        // forever can't hold the request open.
        for _ in 0..50 {
            let params = match &cursor {
                Some(c) => json!({ "cursor": c }),
                None => json!({}),
            };
            let page = self
                .request("tools/list", params)
                .await?
                .map_err(|e| ClientError::Other(format!("tools/list refused: {e}")))?;
            if let Some(list) = page.get("tools").and_then(Value::as_array) {
                tools.extend(list.iter().cloned());
            }
            cursor = page
                .get("nextCursor")
                .and_then(Value::as_str)
                .filter(|c| !c.is_empty())
                .map(str::to_string);
            if cursor.is_none() {
                break;
            }
        }
        Ok(tools)
    }
}

fn answer_of(reply: Value) -> RpcAnswer {
    match reply.get("error") {
        Some(e) => Err(e.clone()),
        None => Ok(reply.get("result").cloned().unwrap_or(Value::Null)),
    }
}

/// Whether `msg` is the response to request `id`.
fn is_response_to(msg: &Value, id: u64) -> bool {
    msg.get("method").is_none() && msg.get("id").and_then(Value::as_u64) == Some(id)
}

// ── stdio ────────────────────────────────────────────────────────────────────

struct StdioConn {
    /// Held so the process lives exactly as long as the client; killed on drop.
    _child: Option<tokio::process::Child>,
    /// One request at a time: the reader below is shared, and matching a
    /// response to its request is simplest when only one is waiting.
    io: Mutex<StdioIo>,
}

struct StdioIo {
    reader: BufReader<Box<dyn AsyncRead + Unpin + Send>>,
    writer: Box<dyn AsyncWrite + Unpin + Send>,
}

impl StdioConn {
    fn spawn(
        command: &str,
        args: &[String],
        env: &BTreeMap<String, String>,
    ) -> Result<Self, ClientError> {
        let mut cmd = tokio::process::Command::new(command);
        // What the server inherits is not this MCP server's business: neither
        // the control plane's secrets nor the agents' keys. Stripped before
        // `env` is added, so a key an admin configured for this server stays.
        harness_agents::strip_inherited_secrets(&mut cmd);
        cmd.args(args)
            .envs(env)
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::null())
            .kill_on_drop(true);
        let mut child = cmd
            .spawn()
            .map_err(|e| ClientError::Other(format!("could not start `{command}`: {e}")))?;
        let stdout = child.stdout.take().ok_or_else(|| other("no stdout"))?;
        let stdin = child.stdin.take().ok_or_else(|| other("no stdin"))?;
        Ok(Self::from_streams(Some(child), stdout, stdin))
    }

    fn from_streams(
        child: Option<tokio::process::Child>,
        reader: impl AsyncRead + Unpin + Send + 'static,
        writer: impl AsyncWrite + Unpin + Send + 'static,
    ) -> Self {
        Self {
            _child: child,
            io: Mutex::new(StdioIo {
                reader: BufReader::new(Box::new(reader)),
                writer: Box::new(writer),
            }),
        }
    }

    async fn send(&self, msg: &Value) -> Result<(), ClientError> {
        let mut io = self.io.lock().await;
        write_line(&mut io.writer, msg).await
    }

    async fn request(&self, msg: &Value, id: u64) -> Result<Value, ClientError> {
        let mut io = self.io.lock().await;
        write_line(&mut io.writer, msg).await?;
        let mut line = String::new();
        loop {
            line.clear();
            let n = io.reader.read_line(&mut line).await.map_err(other)?;
            if n == 0 {
                return Err(other("the server closed its output"));
            }
            // Servers sometimes log to stdout; a line that isn't JSON-RPC is skipped.
            let Ok(reply) = serde_json::from_str::<Value>(line.trim()) else {
                continue;
            };
            if is_response_to(&reply, id) {
                return Ok(reply);
            }
            // A request from the server (sampling, roots, elicitation): the
            // harness offers none of those, and an unanswered one would stall.
            if let (Some(_), Some(rid)) = (reply.get("method"), reply.get("id")) {
                let refusal = json!({
                    "jsonrpc": "2.0",
                    "id": rid,
                    "error": { "code": -32601, "message": "not supported by this client" },
                });
                write_line(&mut io.writer, &refusal).await?;
            }
        }
    }
}

async fn write_line(
    w: &mut (impl AsyncWrite + Unpin + ?Sized),
    msg: &Value,
) -> Result<(), ClientError> {
    let mut bytes = serde_json::to_vec(msg).map_err(other)?;
    bytes.push(b'\n');
    w.write_all(&bytes).await.map_err(other)?;
    w.flush().await.map_err(other)
}

// ── Streamable HTTP ──────────────────────────────────────────────────────────

struct HttpConn {
    http: reqwest::Client,
    url: String,
    headers: reqwest::header::HeaderMap,
    /// `Mcp-Session-Id` from the server's `initialize` answer, if it gave one.
    session: std::sync::Mutex<Option<String>>,
}

impl HttpConn {
    fn new(url: &str, headers: &BTreeMap<String, String>) -> Result<Self, ClientError> {
        let mut map = reqwest::header::HeaderMap::new();
        for (k, v) in headers {
            let name = reqwest::header::HeaderName::from_bytes(k.as_bytes())
                .map_err(|_| other(format!("`{k}` is not a valid header name")))?;
            let value = reqwest::header::HeaderValue::from_str(v)
                .map_err(|_| other(format!("the value of header `{k}` is not valid")))?;
            map.insert(name, value);
        }
        Ok(Self {
            http: reqwest::Client::new(),
            url: url.to_string(),
            headers: map,
            session: std::sync::Mutex::new(None),
        })
    }

    fn post(&self, msg: &Value) -> reqwest::RequestBuilder {
        let mut req = self
            .http
            .post(&self.url)
            .headers(self.headers.clone())
            .header(
                reqwest::header::ACCEPT,
                "application/json, text/event-stream",
            )
            .header("MCP-Protocol-Version", PROTOCOL_VERSION)
            .json(msg);
        if let Some(id) = self
            .session
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .clone()
        {
            req = req.header("Mcp-Session-Id", id);
        }
        req
    }

    async fn send(&self, msg: &Value) -> Result<reqwest::Response, ClientError> {
        let resp = self.post(msg).send().await.map_err(other)?;
        let status = resp.status();
        if status == reqwest::StatusCode::UNAUTHORIZED || status == reqwest::StatusCode::FORBIDDEN {
            return Err(ClientError::Unauthorized(status.to_string()));
        }
        if !status.is_success() {
            let body: String = resp
                .text()
                .await
                .unwrap_or_default()
                .chars()
                .take(300)
                .collect();
            return Err(other(format!("{status}: {body}")));
        }
        if let Some(id) = resp
            .headers()
            .get("mcp-session-id")
            .and_then(|v| v.to_str().ok())
        {
            *self.session.lock().unwrap_or_else(|p| p.into_inner()) = Some(id.to_string());
        }
        Ok(resp)
    }

    async fn notify(&self, msg: &Value) -> Result<(), ClientError> {
        self.send(msg).await.map(|_| ())
    }

    async fn request(&self, msg: &Value, id: u64) -> Result<Value, ClientError> {
        let mut resp = self.send(msg).await?;
        let is_sse = resp
            .headers()
            .get(reqwest::header::CONTENT_TYPE)
            .and_then(|v| v.to_str().ok())
            .is_some_and(|ct| ct.starts_with("text/event-stream"));
        if !is_sse {
            let body = resp.bytes().await.map_err(other)?;
            let v: Value = serde_json::from_slice(&body)
                .map_err(|e| other(format!("the server's answer is not JSON: {e}")))?;
            return find_response(v, id)
                .ok_or_else(|| other("the server's answer has no response to the request"));
        }
        // An SSE stream: read events until the one answering this request. The
        // server may keep the stream open after it, so stop there.
        let mut sse = SseBuffer::default();
        while let Some(chunk) = resp.chunk().await.map_err(other)? {
            for data in sse.push(&chunk) {
                if let Ok(v) = serde_json::from_str::<Value>(&data) {
                    if let Some(found) = find_response(v, id) {
                        return Ok(found);
                    }
                }
            }
        }
        Err(other("the server's stream ended without a response"))
    }
}

/// The response to `id` in a reply that is one message or a batch.
fn find_response(v: Value, id: u64) -> Option<Value> {
    match v {
        Value::Array(items) => items.into_iter().find(|m| is_response_to(m, id)),
        m if is_response_to(&m, id) => Some(m),
        _ => None,
    }
}

/// Splits a `text/event-stream` body into the `data` of each event, however
/// the bytes arrive.
#[derive(Default)]
struct SseBuffer {
    pending: String,
}

impl SseBuffer {
    fn push(&mut self, bytes: &[u8]) -> Vec<String> {
        self.pending
            .push_str(&String::from_utf8_lossy(bytes).replace("\r\n", "\n"));
        let mut events = Vec::new();
        while let Some(end) = self.pending.find("\n\n") {
            let block: String = self.pending.drain(..end + 2).collect();
            let data: Vec<&str> = block
                .lines()
                .filter_map(|l| l.strip_prefix("data:"))
                .map(|d| d.strip_prefix(' ').unwrap_or(d))
                .collect();
            if !data.is_empty() {
                events.push(data.join("\n"));
            }
        }
        events
    }
}

// ── Tools ────────────────────────────────────────────────────────────────────

/// A tool as the registry keeps it, from one entry of `tools/list`.
pub(crate) fn registry_tool(tool: &Value) -> Option<harness_persist::McpTool> {
    let name = tool.get("name")?.as_str()?.to_string();
    let description = tool
        .get("description")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .chars()
        .take(500)
        .collect();
    let read_only = tool
        .pointer("/annotations/readOnlyHint")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    Some(harness_persist::McpTool {
        name,
        description,
        read_only,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sse_events_are_split_however_the_bytes_arrive() {
        let mut sse = SseBuffer::default();
        assert!(sse.push(b"event: message\r\ndata: {\"a\"").is_empty());
        let got = sse.push(b":1}\r\n\r\ndata: x\n\n: comment\n\n");
        assert_eq!(got, vec!["{\"a\":1}".to_string(), "x".to_string()]);
    }

    /// A fake Streamable HTTP server: checks the bearer header, hands out a
    /// session id, answers `initialize` as JSON and `tools/list` as SSE.
    #[tokio::test]
    async fn http_carries_headers_and_session_and_reads_sse_answers() {
        use axum::http::HeaderMap as AxHeaders;
        async fn mcp(
            headers: AxHeaders,
            axum::Json(msg): axum::Json<Value>,
        ) -> axum::response::Response {
            use axum::response::IntoResponse;
            if headers.get("authorization").and_then(|v| v.to_str().ok()) != Some("Bearer t") {
                return axum::http::StatusCode::UNAUTHORIZED.into_response();
            }
            let method = msg["method"].as_str().unwrap_or_default();
            if method != "initialize" && headers.get("mcp-session-id").is_none() {
                return axum::http::StatusCode::BAD_REQUEST.into_response();
            }
            match method {
                "initialize" => (
                    [("mcp-session-id", "s-1")],
                    axum::Json(json!({ "jsonrpc": "2.0", "id": msg["id"], "result": {} })),
                )
                    .into_response(),
                "tools/list" => {
                    let body = format!(
                        "event: message\ndata: {}\n\ndata: {}\n\n",
                        json!({ "jsonrpc": "2.0", "method": "notifications/message", "params": {} }),
                        json!({ "jsonrpc": "2.0", "id": msg["id"], "result": { "tools": [{ "name": "search" }] } }),
                    );
                    ([("content-type", "text/event-stream")], body).into_response()
                }
                _ => axum::http::StatusCode::ACCEPTED.into_response(),
            }
        }
        let app = axum::Router::new().route("/mcp", axum::routing::post(mcp));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        let url = format!("http://{addr}/mcp");

        let good = Upstream::Http {
            url: url.clone(),
            headers: BTreeMap::from([("Authorization".into(), "Bearer t".into())]),
        };
        let client = McpClient::connect(&good).await.unwrap();
        let tools = client.list_tools().await.unwrap();
        assert_eq!(tools, vec![json!({ "name": "search" })]);

        let bad = Upstream::Http {
            url,
            headers: BTreeMap::from([("Authorization".into(), "Bearer wrong".into())]),
        };
        assert!(matches!(
            McpClient::connect(&bad).await,
            Err(ClientError::Unauthorized(_))
        ));
    }

    #[test]
    fn the_response_is_found_in_a_batch_and_a_notification_is_not_one() {
        let batch = json!([
            { "jsonrpc": "2.0", "method": "notifications/progress", "params": {} },
            { "jsonrpc": "2.0", "id": 7, "result": { "ok": true } },
        ]);
        assert_eq!(find_response(batch, 7).unwrap()["result"]["ok"], true);
        let request_from_server = json!({ "jsonrpc": "2.0", "id": 7, "method": "roots/list" });
        assert!(find_response(request_from_server, 7).is_none());
    }

    #[test]
    fn a_tool_without_a_read_only_hint_counts_as_writing() {
        let t = registry_tool(&json!({ "name": "send", "description": "Send mail" })).unwrap();
        assert!(!t.read_only);
        let t = registry_tool(&json!({
            "name": "search", "annotations": { "readOnlyHint": true }
        }))
        .unwrap();
        assert!(t.read_only);
        assert!(registry_tool(&json!({ "description": "nameless" })).is_none());
    }

    #[test]
    fn debug_output_never_shows_a_secret() {
        let up = Upstream::Http {
            url: "https://x".into(),
            headers: BTreeMap::from([("Authorization".into(), "Bearer s3cret".into())]),
        };
        let shown = format!("{up:?}");
        assert!(
            shown.contains("Authorization") && !shown.contains("s3cret"),
            "{shown}"
        );
    }

    /// A fake stdio server on the other end of two pipes: answers initialize,
    /// asks the client something mid-request, and pages its tool list.
    #[tokio::test]
    async fn stdio_initializes_pages_tools_and_refuses_server_requests() {
        let (client_r, mut server_w) = tokio::io::duplex(64 * 1024);
        let (server_r, client_w) = tokio::io::duplex(64 * 1024);
        let server = tokio::spawn(async move {
            let mut lines = BufReader::new(server_r).lines();
            let mut refused = false;
            while let Ok(Some(line)) = lines.next_line().await {
                let msg: Value = serde_json::from_str(&line).unwrap();
                let reply = |id: &Value, result: Value| {
                    format!(
                        "{}\n",
                        json!({ "jsonrpc": "2.0", "id": id, "result": result })
                    )
                };
                match msg.get("method").and_then(Value::as_str) {
                    Some("initialize") => {
                        server_w.write_all(b"not json, a log line\n").await.unwrap();
                        let r = reply(&msg["id"], json!({ "protocolVersion": PROTOCOL_VERSION }));
                        server_w.write_all(r.as_bytes()).await.unwrap();
                    }
                    Some("tools/list") if msg["params"].get("cursor").is_none() => {
                        let ask = json!({ "jsonrpc": "2.0", "id": "s1", "method": "roots/list" });
                        server_w
                            .write_all(format!("{ask}\n").as_bytes())
                            .await
                            .unwrap();
                        let r = reply(
                            &msg["id"],
                            json!({ "tools": [{ "name": "a" }], "nextCursor": "p2" }),
                        );
                        server_w.write_all(r.as_bytes()).await.unwrap();
                    }
                    Some("tools/list") => {
                        let r = reply(&msg["id"], json!({ "tools": [{ "name": "b" }] }));
                        server_w.write_all(r.as_bytes()).await.unwrap();
                    }
                    None if msg["id"] == "s1" => {
                        refused = msg["error"]["code"] == -32601;
                    }
                    _ => {}
                }
            }
            refused
        });

        let client = McpClient::over_streams(client_r, client_w);
        client.initialize().await.unwrap();
        let tools = client.list_tools().await.unwrap();
        let names: Vec<&str> = tools.iter().filter_map(|t| t["name"].as_str()).collect();
        assert_eq!(names, vec!["a", "b"]);
        drop(client);
        assert!(
            server.await.unwrap(),
            "the server's own request was refused"
        );
    }
}
