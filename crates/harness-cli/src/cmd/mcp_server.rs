use anyhow::Context;
use harness_agents::{
    claude::ClaudeCodeAgent, claude_adapter::ClaudeAdapter, codex::CodexAgent,
    codex_adapter::CodexAdapter, registry::AgentRegistry,
};
use harness_core::{agent::AgentRequest, config::HarnessConfig, prompts, types::ThreadId};
use serde::Deserialize;
use serde_json::{json, Value};
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::sync::RwLock;

const MCP_PROTOCOL_VERSION: &str = "2024-11-05";
const JSONRPC_PARSE_ERROR: i32 = -32700;
const JSONRPC_INVALID_REQUEST: i32 = -32600;
const JSONRPC_METHOD_NOT_FOUND: i32 = -32601;
const JSONRPC_INVALID_PARAMS: i32 = -32602;

#[async_trait::async_trait]
trait PromptExecutor: Send + Sync {
    async fn execute(
        &self,
        agent: &str,
        project_root: PathBuf,
        prompt: String,
    ) -> anyhow::Result<String>;
}

struct RegistryExecutor {
    agent_registry: Arc<AgentRegistry>,
}

impl RegistryExecutor {
    fn new(agent_registry: Arc<AgentRegistry>) -> Self {
        Self { agent_registry }
    }
}

#[async_trait::async_trait]
impl PromptExecutor for RegistryExecutor {
    async fn execute(
        &self,
        agent: &str,
        project_root: PathBuf,
        prompt: String,
    ) -> anyhow::Result<String> {
        let code_agent = self.agent_registry.get(agent).with_context(|| {
            let available = self.agent_registry.list().join(", ");
            format!("unknown agent `{agent}` (available: [{available}])")
        })?;

        let response = code_agent
            .execute(AgentRequest {
                prompt,
                project_root,
                ..Default::default()
            })
            .await?;
        Ok(response.output)
    }
}

#[derive(Debug, Clone)]
struct SessionTurn {
    user_prompt: String,
    assistant_output: String,
}

#[derive(Debug, Clone)]
struct SessionState {
    project_root: PathBuf,
    agent: String,
    turns: Vec<SessionTurn>,
}

/// HTTP client for a cluster's **per-project** authoring API — the remote MCP
/// mode. Enabled by `HARNESS_REMOTE_URL` (+ `HARNESS_TOKEN`) so an MCP client's
/// `.mcp.json` `env` block points the workflow tools at the hosted harness.
struct RemoteAuthoring {
    client: reqwest::Client,
    base: String,
    token: Option<String>,
}

impl RemoteAuthoring {
    fn from_env() -> Option<Self> {
        let base = std::env::var("HARNESS_REMOTE_URL")
            .ok()
            .map(|s| s.trim().trim_end_matches('/').to_string())
            .filter(|s| !s.is_empty())?;
        Some(Self {
            client: reqwest::Client::new(),
            base,
            token: std::env::var("HARNESS_TOKEN")
                .ok()
                .filter(|t| !t.is_empty()),
        })
    }

    async fn send(&self, rb: reqwest::RequestBuilder) -> Result<Value, String> {
        let rb = match &self.token {
            Some(t) => rb.bearer_auth(t),
            None => rb,
        };
        let resp = rb
            .send()
            .await
            .map_err(|e| format!("request failed: {e}"))?;
        let status = resp.status();
        let body: Value = resp.json().await.unwrap_or(Value::Null);
        if status.is_success() {
            Ok(body)
        } else {
            let msg = body
                .get("error")
                .and_then(Value::as_str)
                .unwrap_or("request error");
            Err(format!("{msg} (HTTP {})", status.as_u16()))
        }
    }

    async fn get(&self, project: &str, path: &str) -> Result<Value, String> {
        let url = format!("{}/api/projects/{project}/authoring/{path}", self.base);
        self.send(self.client.get(url)).await
    }

    async fn post(&self, project: &str, path: &str, body: Value) -> Result<Value, String> {
        let url = format!("{}/api/projects/{project}/authoring/{path}", self.base);
        self.send(self.client.post(url).json(&body)).await
    }
}

/// Route a `workflow_*` tool call to the remote cluster API. `project` is a
/// required argument in remote mode. Returns a tool result.
async fn remote_workflow_tool(remote: &RemoteAuthoring, name: &str, args: &Value) -> Value {
    let project = match args.get("project").and_then(Value::as_str) {
        Some(p) if !p.is_empty() => p,
        _ => return tool_error_result("`project` is required (remote authoring mode)"),
    };
    let s = |k: &str| {
        args.get(k)
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string()
    };
    let result = match name {
        "workflow_catalog" => remote.get(project, "catalog").await,
        "workflow_list" => remote.get(project, "workflows").await,
        "workflow_get" => {
            remote
                .get(project, &format!("workflows/{}", s("name")))
                .await
        }
        "workflow_validate" => {
            remote
                .post(project, "validate", json!({ "yaml": s("yaml") }))
                .await
        }
        "workflow_save" => {
            remote
                .post(
                    project,
                    "workflows",
                    json!({ "name": s("name"), "yaml": s("yaml") }),
                )
                .await
        }
        "workflow_create" => {
            let mut body = serde_json::Map::new();
            body.insert("name".into(), json!(s("name")));
            for k in ["description", "provider", "model"] {
                if let Some(v) = args.get(k) {
                    body.insert(k.into(), v.clone());
                }
            }
            remote.post(project, "create", Value::Object(body)).await
        }
        "workflow_set_node" => {
            let node = args.get("node").cloned().unwrap_or(Value::Null);
            remote
                .post(
                    project,
                    "set-node",
                    json!({ "name": s("name"), "node": node }),
                )
                .await
        }
        "workflow_remove_node" => {
            remote
                .post(
                    project,
                    "remove-node",
                    json!({ "name": s("name"), "id": s("id") }),
                )
                .await
        }
        "workflow_connect" => {
            remote
                .post(
                    project,
                    "connect",
                    json!({ "name": s("name"), "from": s("from"), "to": s("to") }),
                )
                .await
        }
        _ => return tool_error_result(format!("unknown tool `{name}`")),
    };
    match result {
        Ok(value) => tool_success_result(format!("{name} ok"), value),
        Err(e) => tool_error_result(e),
    }
}

struct McpServer {
    default_agent: String,
    sessions: Arc<RwLock<HashMap<String, SessionState>>>,
    executor: Arc<dyn PromptExecutor>,
    /// When set, the `workflow_*` tools author against a remote cluster's
    /// per-project API instead of the local filesystem.
    remote: Option<RemoteAuthoring>,
}

impl McpServer {
    fn new(default_agent: String, executor: Arc<dyn PromptExecutor>) -> Self {
        Self {
            default_agent,
            sessions: Arc::new(RwLock::new(HashMap::new())),
            executor,
            remote: RemoteAuthoring::from_env(),
        }
    }

    async fn serve_stdio(&self) -> anyhow::Result<()> {
        let stdin = tokio::io::stdin();
        let mut lines = BufReader::new(stdin).lines();
        let mut stdout = tokio::io::stdout();

        while let Some(line) = lines.next_line().await? {
            if line.trim().is_empty() {
                continue;
            }

            let request: Value = match serde_json::from_str(&line) {
                Ok(value) => value,
                Err(error) => {
                    let response = jsonrpc_response(
                        Some(Value::Null),
                        None,
                        Some(jsonrpc_error_payload(
                            JSONRPC_PARSE_ERROR,
                            format!("parse error: {error}"),
                        )),
                    );
                    write_json_line(&mut stdout, &response).await?;
                    continue;
                }
            };

            if let Some(response) = self.handle_request(request).await {
                write_json_line(&mut stdout, &response).await?;
            }
        }

        Ok(())
    }

    async fn handle_request(&self, request: Value) -> Option<Value> {
        let id = request.get("id").cloned();
        let method = match request.get("method").and_then(Value::as_str) {
            Some(method) => method,
            None => {
                return jsonrpc_error_response(
                    id,
                    JSONRPC_INVALID_REQUEST,
                    "missing `method` in request",
                );
            }
        };

        let params = request.get("params").cloned().unwrap_or_else(|| json!({}));

        match method {
            "initialize" => jsonrpc_success_response(
                id,
                json!({
                    "protocolVersion": MCP_PROTOCOL_VERSION,
                    "capabilities": {
                        "tools": {
                            "listChanged": false
                        }
                    },
                    "serverInfo": {
                        "name": "harness",
                        "version": env!("CARGO_PKG_VERSION"),
                    }
                }),
            ),
            "notifications/initialized" => {
                if id.is_some() {
                    jsonrpc_success_response(id, json!({}))
                } else {
                    None
                }
            }
            "ping" => jsonrpc_success_response(id, json!({})),
            "tools/list" => {
                jsonrpc_success_response(id, json!({ "tools": mcp_tools(self.remote.is_some()) }))
            }
            "tools/call" => {
                let call_params: ToolCallParams = match serde_json::from_value(params) {
                    Ok(value) => value,
                    Err(error) => {
                        return jsonrpc_error_response(
                            id,
                            JSONRPC_INVALID_PARAMS,
                            format!("invalid tools/call params: {error}"),
                        );
                    }
                };

                let arguments = call_params.arguments.unwrap_or_else(|| json!({}));
                let result = self.call_tool(call_params.name, arguments).await;
                jsonrpc_success_response(id, result)
            }
            other if other.starts_with("notifications/") => None,
            _ => jsonrpc_error_response(
                id,
                JSONRPC_METHOD_NOT_FOUND,
                format!("method not found: {method}"),
            ),
        }
    }

    async fn call_tool(&self, tool_name: String, arguments: Value) -> Value {
        // In remote mode, the workflow tools author against the cluster API.
        if let Some(remote) = &self.remote {
            if tool_name.starts_with("workflow_") {
                return remote_workflow_tool(remote, &tool_name, &arguments).await;
            }
        }
        match tool_name.as_str() {
            "harness" => self.run_harness_tool(arguments).await,
            "harness-reply" => self.run_harness_reply_tool(arguments).await,
            // Workflow authoring (Phase 4.6) — build/edit workflows with an AI.
            // Backed by the same `harness_runner::authoring` core the visual
            // editor uses, so both front-ends behave identically.
            "workflow_catalog" => workflow_catalog_tool(arguments),
            "workflow_list" => workflow_list_tool(arguments),
            "workflow_get" => workflow_get_tool(arguments),
            "workflow_validate" => workflow_validate_tool(arguments),
            "workflow_save" => workflow_save_tool(arguments),
            // Structured (node-level) authoring — build a workflow without YAML.
            "workflow_create" => workflow_create_tool(arguments),
            "workflow_set_node" => workflow_set_node_tool(arguments),
            "workflow_remove_node" => workflow_remove_node_tool(arguments),
            "workflow_connect" => workflow_connect_tool(arguments),
            _ => tool_error_result(format!("unknown tool `{tool_name}`")),
        }
    }

    async fn run_harness_tool(&self, arguments: Value) -> Value {
        let args: HarnessToolArgs = match serde_json::from_value(arguments) {
            Ok(value) => value,
            Err(error) => return tool_error_result(format!("invalid `harness` args: {error}")),
        };

        if args.prompt.trim().is_empty() {
            return tool_error_result("`prompt` must not be empty");
        }

        let project_root = match resolve_project_root(args.project_root) {
            Ok(path) => path,
            Err(error) => return tool_error_result(error.to_string()),
        };

        let agent = args
            .agent
            .as_deref()
            .unwrap_or(&self.default_agent)
            .to_string();

        let prompt = prompts::wrap_external_data(&args.prompt);
        let output = match self
            .executor
            .execute(&agent, project_root.clone(), prompt)
            .await
        {
            Ok(value) => value,
            Err(error) => return tool_error_result(format!("`harness` execution failed: {error}")),
        };

        let thread_id = ThreadId::new().to_string();
        let session = SessionState {
            project_root: project_root.clone(),
            agent: agent.clone(),
            turns: vec![SessionTurn {
                user_prompt: args.prompt,
                assistant_output: output.clone(),
            }],
        };
        self.sessions
            .write()
            .await
            .insert(thread_id.clone(), session);

        tool_success_result(
            format!("thread_id={thread_id}\n\n{output}"),
            json!({
                "thread_id": thread_id,
                "output": output,
                "agent": agent,
                "project_root": project_root.display().to_string(),
            }),
        )
    }

    async fn run_harness_reply_tool(&self, arguments: Value) -> Value {
        let args: HarnessReplyToolArgs = match serde_json::from_value(arguments) {
            Ok(value) => value,
            Err(error) => {
                return tool_error_result(format!("invalid `harness-reply` args: {error}"));
            }
        };

        if args.prompt.trim().is_empty() {
            return tool_error_result("`prompt` must not be empty");
        }

        let existing = {
            let sessions = self.sessions.read().await;
            sessions.get(&args.thread_id).cloned()
        };

        let Some(existing) = existing else {
            return tool_error_result(format!("thread `{}` not found", args.thread_id));
        };

        let prompt = compose_reply_prompt(&existing.turns, &args.prompt);
        let output = match self
            .executor
            .execute(&existing.agent, existing.project_root.clone(), prompt)
            .await
        {
            Ok(value) => value,
            Err(error) => {
                return tool_error_result(format!("`harness-reply` execution failed: {error}"));
            }
        };

        {
            let mut sessions = self.sessions.write().await;
            if let Some(session) = sessions.get_mut(&args.thread_id) {
                session.turns.push(SessionTurn {
                    user_prompt: args.prompt.clone(),
                    assistant_output: output.clone(),
                });
            }
        }

        tool_success_result(
            format!("thread_id={}\n\n{}", args.thread_id, output),
            json!({
                "thread_id": args.thread_id,
                "output": output,
                "agent": existing.agent,
                "project_root": existing.project_root.display().to_string(),
            }),
        )
    }
}

// ── Workflow authoring tools (Phase 4.6) ─────────────────────────────────────

fn to_value_result(v: impl serde::Serialize, text: String) -> Value {
    match serde_json::to_value(v) {
        Ok(structured) => tool_success_result(text, structured),
        Err(e) => tool_error_result(format!("failed to serialize result: {e}")),
    }
}

/// `workflow_catalog` — the building blocks (node kinds, provider/model hints,
/// commands, context modes, trigger rules) an AI may use.
fn workflow_catalog_tool(arguments: Value) -> Value {
    let args: ProjectRootArgs = match serde_json::from_value(arguments) {
        Ok(v) => v,
        Err(e) => return tool_error_result(format!("invalid `workflow_catalog` args: {e}")),
    };
    let root = match resolve_project_root(args.project_root) {
        Ok(p) => p,
        Err(e) => return tool_error_result(e.to_string()),
    };
    let catalog = harness_runner::authoring::catalog(&root);
    let kinds = catalog
        .node_kinds
        .iter()
        .map(|k| k.kind)
        .collect::<Vec<_>>()
        .join(", ");
    to_value_result(
        &catalog,
        format!(
            "node kinds: {kinds}; {} providers; {} commands; {} prebuilt steps",
            catalog.providers.len(),
            catalog.commands.len(),
            catalog.prebuilt_steps.len()
        ),
    )
}

/// `workflow_list` — workflows available to the project (bundled + project).
fn workflow_list_tool(arguments: Value) -> Value {
    let args: ProjectRootArgs = match serde_json::from_value(arguments) {
        Ok(v) => v,
        Err(e) => return tool_error_result(format!("invalid `workflow_list` args: {e}")),
    };
    let root = match resolve_project_root(args.project_root) {
        Ok(p) => p,
        Err(e) => return tool_error_result(e.to_string()),
    };
    let workflows = harness_runner::authoring::list_workflows(&root);
    let text = workflows
        .iter()
        .map(|w| format!("{} ({:?}, {} steps)", w.name, w.source, w.node_count))
        .collect::<Vec<_>>()
        .join("\n");
    to_value_result(&workflows, text)
}

/// `workflow_get` — a workflow's editable YAML source by name.
fn workflow_get_tool(arguments: Value) -> Value {
    let args: WorkflowGetArgs = match serde_json::from_value(arguments) {
        Ok(v) => v,
        Err(e) => return tool_error_result(format!("invalid `workflow_get` args: {e}")),
    };
    let root = match resolve_project_root(args.project_root) {
        Ok(p) => p,
        Err(e) => return tool_error_result(e.to_string()),
    };
    match harness_runner::authoring::get_workflow(&root, &args.name) {
        Ok(src) => {
            let text = src.yaml.clone();
            to_value_result(&src, text)
        }
        Err(e) => tool_error_result(e),
    }
}

/// `workflow_validate` — validate candidate YAML (parse + cycle check). Returns
/// the structural error or the node summaries — the build→validate→fix loop.
fn workflow_validate_tool(arguments: Value) -> Value {
    let args: WorkflowYamlArgs = match serde_json::from_value(arguments) {
        Ok(v) => v,
        Err(e) => return tool_error_result(format!("invalid `workflow_validate` args: {e}")),
    };
    let result = harness_runner::authoring::validate_workflow(&args.yaml);
    let text = if result.valid {
        format!("valid: {} step(s)", result.nodes.len())
    } else {
        format!(
            "invalid: {}",
            result
                .error
                .clone()
                .unwrap_or_else(|| "unknown error".into())
        )
    };
    to_value_result(&result, text)
}

/// `workflow_save` — validate then save to `.harness/workflows/<name>.yaml`.
fn workflow_save_tool(arguments: Value) -> Value {
    let args: WorkflowSaveArgs = match serde_json::from_value(arguments) {
        Ok(v) => v,
        Err(e) => return tool_error_result(format!("invalid `workflow_save` args: {e}")),
    };
    let root = match resolve_project_root(args.project_root) {
        Ok(p) => p,
        Err(e) => return tool_error_result(e.to_string()),
    };
    match harness_runner::authoring::save_workflow(&root, &args.name, &args.yaml) {
        Ok(()) => tool_success_result(
            format!("saved workflow `{}`", args.name),
            json!({ "saved": true, "name": args.name }),
        ),
        Err(e) => tool_error_result(e),
    }
}

/// After a successful node mutation, report the resulting workflow's node
/// summaries (the build→validate→fix loop sees the new DAG state).
fn workflow_state_result(root: &std::path::Path, name: &str, msg: String) -> Value {
    match harness_runner::authoring::get_workflow(root, name) {
        Ok(src) => {
            let v = harness_runner::authoring::validate_workflow(&src.yaml);
            to_value_result(&v, msg)
        }
        Err(e) => tool_error_result(e),
    }
}

/// `workflow_create` — make a new, empty workflow in the project.
fn workflow_create_tool(arguments: Value) -> Value {
    let args: WorkflowCreateArgs = match serde_json::from_value(arguments) {
        Ok(v) => v,
        Err(e) => return tool_error_result(format!("invalid `workflow_create` args: {e}")),
    };
    let root = match resolve_project_root(args.project_root) {
        Ok(p) => p,
        Err(e) => return tool_error_result(e.to_string()),
    };
    match harness_runner::authoring::create_workflow(
        &root,
        &args.name,
        args.description.as_deref(),
        args.provider.as_deref(),
        args.model.as_deref(),
    ) {
        Ok(()) => workflow_state_result(
            &root,
            &args.name,
            format!("created workflow `{}`", args.name),
        ),
        Err(e) => tool_error_result(e),
    }
}

/// `workflow_set_node` — add or replace a node (by id) from a JSON description.
fn workflow_set_node_tool(arguments: Value) -> Value {
    let args: WorkflowSetNodeArgs = match serde_json::from_value(arguments) {
        Ok(v) => v,
        Err(e) => return tool_error_result(format!("invalid `workflow_set_node` args: {e}")),
    };
    let root = match resolve_project_root(args.project_root) {
        Ok(p) => p,
        Err(e) => return tool_error_result(e.to_string()),
    };
    match harness_runner::authoring::set_node(&root, &args.name, args.node) {
        Ok(()) => workflow_state_result(&root, &args.name, format!("set node in `{}`", args.name)),
        Err(e) => tool_error_result(e),
    }
}

/// `workflow_remove_node` — delete a node and strip it from dependents' edges.
fn workflow_remove_node_tool(arguments: Value) -> Value {
    let args: WorkflowRemoveNodeArgs = match serde_json::from_value(arguments) {
        Ok(v) => v,
        Err(e) => return tool_error_result(format!("invalid `workflow_remove_node` args: {e}")),
    };
    let root = match resolve_project_root(args.project_root) {
        Ok(p) => p,
        Err(e) => return tool_error_result(e.to_string()),
    };
    match harness_runner::authoring::remove_node(&root, &args.name, &args.id) {
        Ok(()) => workflow_state_result(
            &root,
            &args.name,
            format!("removed node `{}` from `{}`", args.id, args.name),
        ),
        Err(e) => tool_error_result(e),
    }
}

/// `workflow_connect` — add a dependency edge (`to` depends on `from`).
fn workflow_connect_tool(arguments: Value) -> Value {
    let args: WorkflowConnectArgs = match serde_json::from_value(arguments) {
        Ok(v) => v,
        Err(e) => return tool_error_result(format!("invalid `workflow_connect` args: {e}")),
    };
    let root = match resolve_project_root(args.project_root) {
        Ok(p) => p,
        Err(e) => return tool_error_result(e.to_string()),
    };
    match harness_runner::authoring::connect_nodes(&root, &args.name, &args.from, &args.to) {
        Ok(()) => workflow_state_result(
            &root,
            &args.name,
            format!(
                "connected `{}` -> `{}` in `{}`",
                args.from, args.to, args.name
            ),
        ),
        Err(e) => tool_error_result(e),
    }
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct ToolCallParams {
    name: String,
    #[serde(default)]
    arguments: Option<Value>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct HarnessToolArgs {
    prompt: String,
    #[serde(default)]
    project_root: Option<PathBuf>,
    #[serde(default)]
    agent: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct HarnessReplyToolArgs {
    thread_id: String,
    prompt: String,
}

#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct ProjectRootArgs {
    #[serde(default)]
    project_root: Option<PathBuf>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct WorkflowGetArgs {
    name: String,
    #[serde(default)]
    project_root: Option<PathBuf>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct WorkflowYamlArgs {
    yaml: String,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct WorkflowSaveArgs {
    name: String,
    yaml: String,
    #[serde(default)]
    project_root: Option<PathBuf>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct WorkflowCreateArgs {
    name: String,
    #[serde(default)]
    description: Option<String>,
    #[serde(default)]
    provider: Option<String>,
    #[serde(default)]
    model: Option<String>,
    #[serde(default)]
    project_root: Option<PathBuf>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct WorkflowSetNodeArgs {
    name: String,
    node: Value,
    #[serde(default)]
    project_root: Option<PathBuf>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct WorkflowRemoveNodeArgs {
    name: String,
    id: String,
    #[serde(default)]
    project_root: Option<PathBuf>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct WorkflowConnectArgs {
    name: String,
    from: String,
    to: String,
    #[serde(default)]
    project_root: Option<PathBuf>,
}

/// Tool definitions. In `remote` mode the `workflow_*` tools gain a required
/// `project` argument (they target a registered cluster project).
fn mcp_tools(remote: bool) -> Vec<Value> {
    let mut tools = mcp_tools_base();
    if remote {
        for t in tools.iter_mut() {
            let is_workflow = t
                .get("name")
                .and_then(Value::as_str)
                .is_some_and(|n| n.starts_with("workflow_"));
            if !is_workflow {
                continue;
            }
            if let Some(props) = t
                .pointer_mut("/inputSchema/properties")
                .and_then(Value::as_object_mut)
            {
                props.insert(
                    "project".into(),
                    json!({ "type": "string", "description": "Registered project on the cluster." }),
                );
            }
            match t
                .pointer_mut("/inputSchema/required")
                .and_then(Value::as_array_mut)
            {
                Some(req) => req.insert(0, json!("project")),
                None => {
                    if let Some(schema) =
                        t.pointer_mut("/inputSchema").and_then(Value::as_object_mut)
                    {
                        schema.insert("required".into(), json!(["project"]));
                    }
                }
            }
        }
    }
    tools
}

fn mcp_tools_base() -> Vec<Value> {
    vec![
        json!({
            "name": "harness",
            "description": "Start a new harness session and execute a prompt.",
            "inputSchema": {
                "type": "object",
                "additionalProperties": false,
                "properties": {
                    "prompt": {
                        "type": "string",
                        "description": "User prompt to execute.",
                    },
                    "project_root": {
                        "type": "string",
                        "description": "Project directory path. Defaults to server cwd.",
                    },
                    "agent": {
                        "type": "string",
                        "description": "Agent name (for example: claude, codex). Defaults to configured default agent.",
                    }
                },
                "required": ["prompt"],
            }
        }),
        json!({
            "name": "harness-reply",
            "description": "Continue an existing harness session by thread ID.",
            "inputSchema": {
                "type": "object",
                "additionalProperties": false,
                "properties": {
                    "thread_id": {
                        "type": "string",
                        "description": "Thread ID returned by the harness tool.",
                    },
                    "prompt": {
                        "type": "string",
                        "description": "Follow-up user prompt for this thread.",
                    }
                },
                "required": ["thread_id", "prompt"],
            }
        }),
        json!({
            "name": "workflow_catalog",
            "description": "List the building blocks available for authoring a workflow: node kinds (agent step, command, shell, loop, script), provider/model hints, available commands, context modes, and trigger rules.",
            "inputSchema": {
                "type": "object",
                "additionalProperties": false,
                "properties": {
                    "project_root": { "type": "string", "description": "Project directory. Defaults to server cwd." }
                }
            }
        }),
        json!({
            "name": "workflow_list",
            "description": "List workflows available to the project (bundled defaults + project .harness/workflows; project shadows bundled).",
            "inputSchema": {
                "type": "object",
                "additionalProperties": false,
                "properties": {
                    "project_root": { "type": "string", "description": "Project directory. Defaults to server cwd." }
                }
            }
        }),
        json!({
            "name": "workflow_get",
            "description": "Get a workflow's editable YAML source by name (project shadows bundled). Use this to read the default pipeline before editing it.",
            "inputSchema": {
                "type": "object",
                "additionalProperties": false,
                "properties": {
                    "name": { "type": "string", "description": "Workflow name." },
                    "project_root": { "type": "string", "description": "Project directory. Defaults to server cwd." }
                },
                "required": ["name"],
            }
        }),
        json!({
            "name": "workflow_validate",
            "description": "Validate candidate workflow YAML (parse + cycle/dependency/body checks). Returns the first structural error or the node summaries — the build->validate->fix loop. Always validate before saving.",
            "inputSchema": {
                "type": "object",
                "additionalProperties": false,
                "properties": {
                    "yaml": { "type": "string", "description": "Candidate workflow YAML." }
                },
                "required": ["yaml"],
            }
        }),
        json!({
            "name": "workflow_save",
            "description": "Validate then save a workflow to the project's .harness/workflows/<name>.yaml. Refuses invalid workflows and unsafe names.",
            "inputSchema": {
                "type": "object",
                "additionalProperties": false,
                "properties": {
                    "name": { "type": "string", "description": "Workflow name (file stem)." },
                    "yaml": { "type": "string", "description": "Workflow YAML to save." },
                    "project_root": { "type": "string", "description": "Project directory. Defaults to server cwd." }
                },
                "required": ["name", "yaml"],
            }
        }),
        json!({
            "name": "workflow_create",
            "description": "Create a new, empty workflow in the project (build it up with workflow_set_node / workflow_connect — no YAML needed). Errors if one already exists. Returns the (empty) node summary.",
            "inputSchema": {
                "type": "object",
                "additionalProperties": false,
                "properties": {
                    "name": { "type": "string", "description": "Workflow name (file stem)." },
                    "description": { "type": "string" },
                    "provider": { "type": "string", "description": "Default provider (claude/codex/pi) — see workflow_catalog." },
                    "model": { "type": "string", "description": "Default model." },
                    "project_root": { "type": "string", "description": "Project directory. Defaults to server cwd." }
                },
                "required": ["name"],
            }
        }),
        json!({
            "name": "workflow_set_node",
            "description": "Add or replace (by id) one node in a workflow — no YAML. `node` is an object with `id` and exactly one body field (prompt | bash | command | script | loop | approval | cancel) plus optional depends_on, when, category, provider, model, context, trigger_rule, timeout, output_format. Validates the whole DAG and reports node summaries; rejects a node with zero/multiple bodies or dangling refs. Use workflow_catalog for legal kinds/providers/commands.",
            "inputSchema": {
                "type": "object",
                "additionalProperties": false,
                "properties": {
                    "name": { "type": "string", "description": "Workflow to edit (project shadows bundled)." },
                    "node": {
                        "type": "object",
                        "description": "Node spec. e.g. {\"id\":\"classify\",\"prompt\":\"...\",\"depends_on\":[\"explore\"],\"category\":\"planning\"}.",
                        "properties": { "id": { "type": "string" } },
                        "required": ["id"]
                    },
                    "project_root": { "type": "string", "description": "Project directory. Defaults to server cwd." }
                },
                "required": ["name", "node"],
            }
        }),
        json!({
            "name": "workflow_remove_node",
            "description": "Remove a node by id and strip it from every other node's depends_on. Validates and reports the resulting node summaries.",
            "inputSchema": {
                "type": "object",
                "additionalProperties": false,
                "properties": {
                    "name": { "type": "string", "description": "Workflow to edit." },
                    "id": { "type": "string", "description": "Node id to remove." },
                    "project_root": { "type": "string", "description": "Project directory. Defaults to server cwd." }
                },
                "required": ["name", "id"],
            }
        }),
        json!({
            "name": "workflow_connect",
            "description": "Add a dependency edge: `to` now depends on `from`. Validates (catches unknown ids and cycles).",
            "inputSchema": {
                "type": "object",
                "additionalProperties": false,
                "properties": {
                    "name": { "type": "string", "description": "Workflow to edit." },
                    "from": { "type": "string", "description": "Upstream node id (runs first)." },
                    "to": { "type": "string", "description": "Downstream node id (gains the dependency)." },
                    "project_root": { "type": "string", "description": "Project directory. Defaults to server cwd." }
                },
                "required": ["name", "from", "to"],
            }
        }),
    ]
}

fn compose_reply_prompt(history: &[SessionTurn], next_prompt: &str) -> String {
    if history.is_empty() {
        return prompts::wrap_external_data(next_prompt);
    }

    let mut transcript = String::from(
        "Continue the conversation using this transcript. Keep prior context consistent.\n\n",
    );
    for (index, turn) in history.iter().enumerate() {
        let step = index + 1;
        transcript.push_str(&format!("User #{step}:\n{}\n\n", turn.user_prompt));
        transcript.push_str(&format!(
            "Assistant #{step}:\n{}\n\n",
            turn.assistant_output
        ));
    }
    transcript.push_str(&format!("User #{}:\n{}", history.len() + 1, next_prompt));
    prompts::wrap_external_data(&transcript)
}

fn resolve_project_root(project_root: Option<PathBuf>) -> anyhow::Result<PathBuf> {
    let cwd = std::env::current_dir().context("failed to resolve current working directory")?;
    let path = project_root.unwrap_or_else(|| cwd.clone());
    if path.is_absolute() {
        Ok(path)
    } else {
        Ok(cwd.join(path))
    }
}

fn jsonrpc_success_response(id: Option<Value>, result: Value) -> Option<Value> {
    match id {
        Some(request_id) => Some(jsonrpc_response(Some(request_id), Some(result), None)),
        None => None,
    }
}

fn jsonrpc_error_response(
    id: Option<Value>,
    code: i32,
    message: impl Into<String>,
) -> Option<Value> {
    match id {
        Some(request_id) => Some(jsonrpc_response(
            Some(request_id),
            None,
            Some(jsonrpc_error_payload(code, message)),
        )),
        None => None,
    }
}

fn jsonrpc_response(id: Option<Value>, result: Option<Value>, error: Option<Value>) -> Value {
    let mut response = serde_json::Map::new();
    response.insert("jsonrpc".to_string(), Value::String("2.0".to_string()));
    response.insert("id".to_string(), id.unwrap_or(Value::Null));
    if let Some(result) = result {
        response.insert("result".to_string(), result);
    }
    if let Some(error) = error {
        response.insert("error".to_string(), error);
    }
    Value::Object(response)
}

fn jsonrpc_error_payload(code: i32, message: impl Into<String>) -> Value {
    json!({
        "code": code,
        "message": message.into(),
    })
}

fn tool_success_result(text: String, structured_content: Value) -> Value {
    json!({
        "content": [
            {
                "type": "text",
                "text": text,
            }
        ],
        "structuredContent": structured_content,
        "isError": false,
    })
}

fn tool_error_result(message: impl Into<String>) -> Value {
    json!({
        "content": [
            {
                "type": "text",
                "text": message.into(),
            }
        ],
        "isError": true,
    })
}

async fn write_json_line(stdout: &mut tokio::io::Stdout, value: &Value) -> anyhow::Result<()> {
    let line = serde_json::to_string(value)?;
    stdout.write_all(line.as_bytes()).await?;
    stdout.write_all(b"\n").await?;
    stdout.flush().await?;
    Ok(())
}

pub async fn run(config: HarnessConfig) -> anyhow::Result<()> {
    let mut agent_registry = AgentRegistry::new(&config.agents.default_agent);
    agent_registry.set_complexity_preferences(config.agents.complexity_preferred_agents.clone());
    agent_registry.register(
        "claude",
        Arc::new(
            ClaudeCodeAgent::new(
                config.agents.claude.cli_path.clone(),
                config.agents.claude.default_model.clone(),
                config.agents.sandbox_mode,
            )
            .with_no_session_persistence_probe()
            .with_stream_timeout(config.agents.stream_timeout_secs),
        ),
    );
    agent_registry
        .register_adapter(
            "claude",
            Arc::new(ClaudeAdapter::new(
                config.agents.claude.cli_path.clone(),
                config.agents.claude.default_model.clone(),
            )),
        )
        .context("failed to attach claude adapter")?;
    agent_registry.register(
        "codex",
        Arc::new(
            CodexAgent::from_config(config.agents.codex.clone(), config.agents.sandbox_mode)
                .with_stream_timeout(config.agents.stream_timeout_secs),
        ),
    );
    agent_registry
        .register_adapter(
            "codex",
            Arc::new(CodexAdapter::new(config.agents.codex.cli_path.clone())),
        )
        .context("failed to attach codex adapter")?;

    let default_agent_name = agent_registry
        .resolved_default_agent_name()
        .unwrap_or(config.agents.default_agent.as_str())
        .to_string();
    let executor = Arc::new(RegistryExecutor::new(Arc::new(agent_registry)));
    let server = McpServer::new(default_agent_name, executor);
    server.serve_stdio().await
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;
    use tokio::sync::Mutex;

    #[derive(Debug, Clone)]
    struct MockExecutionCall {
        agent: String,
        project_root: PathBuf,
        prompt: String,
    }

    #[derive(Default)]
    struct MockExecutor {
        calls: Mutex<Vec<MockExecutionCall>>,
    }

    #[async_trait::async_trait]
    impl PromptExecutor for MockExecutor {
        async fn execute(
            &self,
            agent: &str,
            project_root: PathBuf,
            prompt: String,
        ) -> anyhow::Result<String> {
            self.calls.lock().await.push(MockExecutionCall {
                agent: agent.to_string(),
                project_root,
                prompt: prompt.clone(),
            });
            Ok(format!("mock-output::{agent}::{prompt}"))
        }
    }

    fn make_request(id: i64, method: &str, params: Value) -> Value {
        json!({
            "jsonrpc": "2.0",
            "id": id,
            "method": method,
            "params": params,
        })
    }

    fn extract_result(response: Value) -> Value {
        response
            .get("result")
            .cloned()
            .expect("response has result")
    }

    #[tokio::test]
    async fn tools_list_returns_harness_and_reply() {
        let executor = Arc::new(MockExecutor::default());
        let server = McpServer::new("mock-default".to_string(), executor);

        let response = server
            .handle_request(make_request(1, "tools/list", json!({})))
            .await
            .expect("tools/list should respond");
        let tools = extract_result(response)
            .get("tools")
            .and_then(Value::as_array)
            .cloned()
            .expect("tools array");

        let names = tools
            .iter()
            .filter_map(|t| t.get("name").and_then(Value::as_str))
            .collect::<Vec<_>>();
        for expected in [
            "harness",
            "harness-reply",
            "workflow_catalog",
            "workflow_list",
            "workflow_get",
            "workflow_validate",
            "workflow_save",
            "workflow_create",
            "workflow_set_node",
            "workflow_remove_node",
            "workflow_connect",
        ] {
            assert!(names.contains(&expected), "missing tool `{expected}`");
        }
    }

    #[tokio::test]
    async fn workflow_validate_and_catalog_tools() {
        let server = McpServer::new(
            "mock-default".to_string(),
            Arc::new(MockExecutor::default()),
        );

        // A valid workflow: tool succeeds, structured says valid with node summaries.
        let resp = server
            .handle_request(make_request(
                1,
                "tools/call",
                json!({
                    "name": "workflow_validate",
                    "arguments": { "yaml": "name: d\nnodes:\n  - id: a\n    bash: \"echo hi\"\n" },
                }),
            ))
            .await
            .expect("respond");
        let r = extract_result(resp);
        assert_eq!(r["isError"], Value::Bool(false));
        assert_eq!(r["structuredContent"]["valid"], Value::Bool(true));
        assert_eq!(r["structuredContent"]["nodes"][0]["kind"], "bash");

        // An invalid workflow (unknown dep): the *tool* still succeeds, but the
        // result reports invalid with an error — the build->validate->fix loop.
        let resp = server
            .handle_request(make_request(
                2,
                "tools/call",
                json!({
                    "name": "workflow_validate",
                    "arguments": { "yaml": "name: d\nnodes:\n  - id: a\n    depends_on: [ghost]\n    bash: \"x\"\n" },
                }),
            ))
            .await
            .expect("respond");
        let r = extract_result(resp);
        assert_eq!(r["isError"], Value::Bool(false));
        assert_eq!(r["structuredContent"]["valid"], Value::Bool(false));

        // Catalog exposes the building blocks.
        let tmp = tempfile::tempdir().unwrap();
        let resp = server
            .handle_request(make_request(
                3,
                "tools/call",
                json!({
                    "name": "workflow_catalog",
                    "arguments": { "project_root": tmp.path().to_string_lossy() },
                }),
            ))
            .await
            .expect("respond");
        let r = extract_result(resp);
        assert_eq!(r["isError"], Value::Bool(false));
        assert!(
            r["structuredContent"]["node_kinds"]
                .as_array()
                .unwrap()
                .len()
                >= 5,
            "catalog should list node kinds"
        );
    }

    #[tokio::test]
    async fn workflow_structured_authoring_tools() {
        let server = McpServer::new(
            "mock-default".to_string(),
            Arc::new(MockExecutor::default()),
        );
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().to_string_lossy().to_string();
        let call = |id: i64, name: &str, args: Value| {
            let mut a = args;
            a["project_root"] = Value::String(root.clone());
            make_request(id, "tools/call", json!({ "name": name, "arguments": a }))
        };

        // create → add two nodes → connect → the DAG is valid with 2 nodes.
        let r = extract_result(
            server
                .handle_request(call(1, "workflow_create", json!({ "name": "built" })))
                .await
                .unwrap(),
        );
        assert_eq!(r["isError"], Value::Bool(false));

        for (i, node) in [
            json!({ "id": "explore", "prompt": "look" }),
            json!({ "id": "plan", "command": "create-plan" }),
        ]
        .into_iter()
        .enumerate()
        {
            let r = extract_result(
                server
                    .handle_request(call(
                        2 + i as i64,
                        "workflow_set_node",
                        json!({ "name": "built", "node": node }),
                    ))
                    .await
                    .unwrap(),
            );
            assert_eq!(r["isError"], Value::Bool(false), "set_node {i}");
        }

        let r = extract_result(
            server
                .handle_request(call(
                    10,
                    "workflow_connect",
                    json!({ "name": "built", "from": "explore", "to": "plan" }),
                ))
                .await
                .unwrap(),
        );
        assert_eq!(r["structuredContent"]["valid"], Value::Bool(true));
        assert_eq!(r["structuredContent"]["nodes"].as_array().unwrap().len(), 2);

        // A node with two bodies is rejected (the tool errors, file untouched).
        let r = extract_result(
            server
                .handle_request(call(
                    11,
                    "workflow_set_node",
                    json!({ "name": "built", "node": { "id": "bad", "prompt": "x", "bash": "y" } }),
                ))
                .await
                .unwrap(),
        );
        assert_eq!(r["isError"], Value::Bool(true));

        // remove a node → back to 1.
        let r = extract_result(
            server
                .handle_request(call(
                    12,
                    "workflow_remove_node",
                    json!({ "name": "built", "id": "explore" }),
                ))
                .await
                .unwrap(),
        );
        assert_eq!(r["structuredContent"]["nodes"].as_array().unwrap().len(), 1);
    }

    #[tokio::test]
    async fn harness_then_reply_reuses_thread_and_history() {
        let executor = Arc::new(MockExecutor::default());
        let server = McpServer::new("mock-default".to_string(), executor.clone());

        let first = server
            .handle_request(make_request(
                1,
                "tools/call",
                json!({
                    "name": "harness",
                    "arguments": {
                        "prompt": "hello",
                        "project_root": ".",
                        "agent": "mock-agent",
                    }
                }),
            ))
            .await
            .expect("harness call should respond");
        let first_result = extract_result(first);
        assert_eq!(first_result["isError"], Value::Bool(false));
        let thread_id = first_result["structuredContent"]["thread_id"]
            .as_str()
            .expect("thread_id in structuredContent")
            .to_string();

        let second = server
            .handle_request(make_request(
                2,
                "tools/call",
                json!({
                    "name": "harness-reply",
                    "arguments": {
                        "thread_id": thread_id,
                        "prompt": "continue",
                    }
                }),
            ))
            .await
            .expect("harness-reply should respond");
        let second_result = extract_result(second);
        assert_eq!(second_result["isError"], Value::Bool(false));
        assert!(second_result["structuredContent"]["output"]
            .as_str()
            .expect("output text")
            .contains("continue"));

        let calls = executor.calls.lock().await.clone();
        assert_eq!(calls.len(), 2);
        assert_eq!(calls[0].agent, "mock-agent");
        assert!(calls[1].prompt.contains("User #1"));
        assert!(calls[1].prompt.contains("continue"));
        assert!(!calls[0].project_root.as_os_str().is_empty());
    }

    #[tokio::test]
    async fn unknown_tool_returns_tool_error() {
        let executor = Arc::new(MockExecutor::default());
        let server = McpServer::new("mock-default".to_string(), executor);

        let response = server
            .handle_request(make_request(
                1,
                "tools/call",
                json!({
                    "name": "missing-tool",
                    "arguments": {},
                }),
            ))
            .await
            .expect("tools/call should respond");
        let result = extract_result(response);
        assert_eq!(result["isError"], Value::Bool(true));
        assert!(result["content"][0]["text"]
            .as_str()
            .expect("error text")
            .contains("unknown tool"));
    }

    fn remote_server() -> McpServer {
        McpServer {
            default_agent: "mock".to_string(),
            sessions: Arc::new(RwLock::new(HashMap::new())),
            executor: Arc::new(MockExecutor::default()),
            remote: Some(RemoteAuthoring {
                client: reqwest::Client::new(),
                base: "http://127.0.0.1:0".to_string(),
                token: None,
            }),
        }
    }

    #[tokio::test]
    async fn remote_mode_workflow_tools_require_project() {
        let server = remote_server();
        // tools/list: workflow tools gain a required `project`; others don't.
        let resp = server
            .handle_request(make_request(1, "tools/list", json!({})))
            .await
            .unwrap();
        let tools = resp["result"]["tools"].as_array().unwrap();
        let create = tools
            .iter()
            .find(|t| t["name"] == "workflow_create")
            .unwrap();
        let required = create["inputSchema"]["required"].as_array().unwrap();
        assert!(required.iter().any(|v| v == "project"), "project required");
        let harness = tools.iter().find(|t| t["name"] == "harness").unwrap();
        let hreq = harness["inputSchema"]["required"].as_array().unwrap();
        assert!(
            !hreq.iter().any(|v| v == "project"),
            "non-workflow unchanged"
        );

        // A workflow call without `project` errors before any HTTP request.
        let resp = server
            .handle_request(make_request(
                2,
                "tools/call",
                json!({ "name": "workflow_list", "arguments": {} }),
            ))
            .await
            .unwrap();
        let r = extract_result(resp);
        assert_eq!(r["isError"], Value::Bool(true));
        assert!(r["content"][0]["text"]
            .as_str()
            .unwrap()
            .contains("project"));
    }
}
