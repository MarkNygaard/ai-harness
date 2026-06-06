//! **Cluster-hosted MCP endpoint** — a single `POST /mcp` route that speaks
//! JSON-RPC 2.0 (the MCP "Streamable HTTP" transport) so an editor (Claude Code,
//! …) can drive the harness over HTTP **with no local binary**: its `.mcp.json`
//! is just `{ "type": "http", "url": ".../mcp", "headers": { Authorization } }`.
//!
//! It exposes the same authoring tools as the stdio MCP **plus** run control —
//! all tools are project-scoped (`project` argument) and dispatch in-process to
//! the existing logic:
//!
//! - `run_trigger` / `run_list` / `run_status` → [`super::runs_routes`]
//! - `workflow_*` → [`harness_runner::authoring`] (same core as the web editor)
//!
//! Auth is the global bearer-token middleware (`Authorization: Bearer …`); this
//! route is not exempt, so the cluster token gates every call.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use axum::extract::Extension;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::Json;
use harness_runner::authoring;
use serde_json::{json, Value};

use super::runs_routes::{start_run, CreateRunRequest, RunsState};

const MCP_PROTOCOL_VERSION: &str = "2024-11-05";
const JSONRPC_METHOD_NOT_FOUND: i32 = -32601;

/// `POST /mcp` — handle one JSON-RPC request (or a batch array). Notifications
/// (no `id`) produce no response; a request with an `id` produces one.
pub async fn handle_mcp(
    Extension(state): Extension<Arc<RunsState>>,
    Json(body): Json<Value>,
) -> Response {
    if let Some(batch) = body.as_array() {
        let mut out = Vec::new();
        for req in batch {
            if let Some(resp) = handle_one(&state, req).await {
                out.push(resp);
            }
        }
        if out.is_empty() {
            return StatusCode::ACCEPTED.into_response();
        }
        return Json(Value::Array(out)).into_response();
    }
    match handle_one(&state, &body).await {
        Some(resp) => Json(resp).into_response(),
        None => StatusCode::ACCEPTED.into_response(),
    }
}

async fn handle_one(state: &Arc<RunsState>, req: &Value) -> Option<Value> {
    let id = req.get("id").cloned();
    let method = req.get("method").and_then(Value::as_str).unwrap_or("");
    let params = req.get("params").cloned().unwrap_or_else(|| json!({}));

    match method {
        "initialize" => {
            // Echo the client's requested protocol version when present.
            let pv = params
                .get("protocolVersion")
                .and_then(Value::as_str)
                .unwrap_or(MCP_PROTOCOL_VERSION)
                .to_string();
            success(
                id,
                json!({
                    "protocolVersion": pv,
                    "capabilities": { "tools": { "listChanged": false } },
                    "serverInfo": { "name": "harness", "version": env!("CARGO_PKG_VERSION") },
                }),
            )
        }
        "ping" => success(id, json!({})),
        "tools/list" => success(id, json!({ "tools": mcp_tools() })),
        "tools/call" => {
            let name = params
                .get("name")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string();
            let args = params
                .get("arguments")
                .cloned()
                .unwrap_or_else(|| json!({}));
            let result = call_tool(state, &name, &args).await;
            success(id, result)
        }
        // Any notification (initialized, cancelled, …) — no reply.
        m if m.starts_with("notifications/") => None,
        other => error(
            id,
            JSONRPC_METHOD_NOT_FOUND,
            format!("method not found: {other}"),
        ),
    }
}

/// Resolve a registered project to its on-disk checkout dir (or an error string).
async fn project_dir(state: &Arc<RunsState>, project: &str) -> Result<PathBuf, String> {
    if project.is_empty() {
        return Err("`project` is required".to_string());
    }
    let store = state.project_store().await?;
    match store.get(project).await {
        Ok(Some(_)) => Ok(state.projects_dir.join(project)),
        Ok(None) => Err(format!("unknown project `{project}`")),
        Err(e) => Err(e.to_string()),
    }
}

/// After a successful authoring mutation, report the resulting DAG's node
/// summaries (the build→validate→fix loop sees the new state).
fn state_after(dir: &Path, name: &str, msg: String) -> Value {
    match authoring::get_workflow(dir, name) {
        Ok(src) => to_result(msg, &authoring::validate_workflow(&src.yaml)),
        Err(e) => tool_error(e),
    }
}

async fn call_tool(state: &Arc<RunsState>, name: &str, args: &Value) -> Value {
    let s = |k: &str| {
        args.get(k)
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string()
    };

    match name {
        // ── Run control ─────────────────────────────────────────────────────
        "run_trigger" => {
            let req = CreateRunRequest {
                workflow: s("workflow"),
                title: args
                    .get("title")
                    .and_then(Value::as_str)
                    .map(str::to_string),
                description: s("description"),
                args: String::new(),
                // A trigger from an editor means "actually run it" unless told not to.
                real: args.get("real").and_then(Value::as_bool).unwrap_or(true),
                base_branch: args
                    .get("base_branch")
                    .and_then(Value::as_str)
                    .map(str::to_string),
                project: Some(s("project")),
            };
            match start_run(state, req).await {
                Ok(run_id) => to_result(
                    format!("run started: {run_id}"),
                    &json!({ "run_id": run_id }),
                ),
                Err((_status, msg)) => tool_error(msg),
            }
        }
        "run_list" => {
            let store = match state.store().await {
                Ok(s) => s,
                Err(e) => return tool_error(e),
            };
            let limit = args.get("limit").and_then(Value::as_i64).unwrap_or(50);
            match store.list_runs(limit).await {
                Ok(runs) => to_result(format!("{} run(s)", runs.len()), &runs),
                Err(e) => tool_error(e.to_string()),
            }
        }
        "run_status" => {
            let id = s("run_id");
            if id.is_empty() {
                return tool_error("`run_id` is required");
            }
            let store = match state.store().await {
                Ok(s) => s,
                Err(e) => return tool_error(e),
            };
            match store.get_run(&id).await {
                Ok(Some(detail)) => to_result(format!("run {id}"), &detail),
                Ok(None) => tool_error(format!("run `{id}` not found")),
                Err(e) => tool_error(e.to_string()),
            }
        }

        // ── Authoring (project-scoped) ──────────────────────────────────────
        "workflow_catalog" => match project_dir(state, &s("project")).await {
            Ok(dir) => to_result("catalog".to_string(), &authoring::catalog(&dir)),
            Err(e) => tool_error(e),
        },
        "workflow_list" => match project_dir(state, &s("project")).await {
            Ok(dir) => {
                let list = authoring::list_workflows(&dir);
                to_result(format!("{} workflow(s)", list.len()), &list)
            }
            Err(e) => tool_error(e),
        },
        "workflow_get" => match project_dir(state, &s("project")).await {
            Ok(dir) => match authoring::get_workflow(&dir, &s("name")) {
                Ok(src) => to_result(src.yaml.clone(), &src),
                Err(e) => tool_error(e),
            },
            Err(e) => tool_error(e),
        },
        "workflow_validate" => match project_dir(state, &s("project")).await {
            Ok(_) => {
                let v = authoring::validate_workflow(&s("yaml"));
                let text = if v.valid {
                    format!("valid: {} step(s)", v.nodes.len())
                } else {
                    format!("invalid: {}", v.error.clone().unwrap_or_default())
                };
                to_result(text, &v)
            }
            Err(e) => tool_error(e),
        },
        "workflow_save" => match project_dir(state, &s("project")).await {
            Ok(dir) => match authoring::save_workflow(&dir, &s("name"), &s("yaml")) {
                Ok(()) => to_result(
                    format!("saved `{}`", s("name")),
                    &json!({ "saved": true, "name": s("name") }),
                ),
                Err(e) => tool_error(e),
            },
            Err(e) => tool_error(e),
        },
        "workflow_create" => match project_dir(state, &s("project")).await {
            Ok(dir) => {
                let r = authoring::create_workflow(
                    &dir,
                    &s("name"),
                    args.get("description").and_then(Value::as_str),
                    args.get("provider").and_then(Value::as_str),
                    args.get("model").and_then(Value::as_str),
                );
                match r {
                    Ok(()) => state_after(&dir, &s("name"), format!("created `{}`", s("name"))),
                    Err(e) => tool_error(e),
                }
            }
            Err(e) => tool_error(e),
        },
        "workflow_set_node" => match project_dir(state, &s("project")).await {
            Ok(dir) => {
                let node = args.get("node").cloned().unwrap_or(Value::Null);
                match authoring::set_node(&dir, &s("name"), node) {
                    Ok(()) => state_after(&dir, &s("name"), format!("set node in `{}`", s("name"))),
                    Err(e) => tool_error(e),
                }
            }
            Err(e) => tool_error(e),
        },
        "workflow_remove_node" => match project_dir(state, &s("project")).await {
            Ok(dir) => match authoring::remove_node(&dir, &s("name"), &s("id")) {
                Ok(()) => state_after(
                    &dir,
                    &s("name"),
                    format!("removed `{}` from `{}`", s("id"), s("name")),
                ),
                Err(e) => tool_error(e),
            },
            Err(e) => tool_error(e),
        },
        "workflow_connect" => match project_dir(state, &s("project")).await {
            Ok(dir) => match authoring::connect_nodes(&dir, &s("name"), &s("from"), &s("to")) {
                Ok(()) => state_after(
                    &dir,
                    &s("name"),
                    format!("connected `{}` -> `{}`", s("from"), s("to")),
                ),
                Err(e) => tool_error(e),
            },
            Err(e) => tool_error(e),
        },
        other => tool_error(format!("unknown tool `{other}`")),
    }
}

/// The tool catalog. Every tool is project-scoped (`project` argument), since
/// one hosted endpoint serves all registered cluster projects.
fn mcp_tools() -> Vec<Value> {
    let project = json!({ "type": "string", "description": "Registered cluster project." });
    vec![
        json!({
            "name": "run_trigger",
            "description": "Start a workflow run in a project (e.g. idea-to-pr). Executes for real and returns the run_id; track it with run_status.",
            "inputSchema": {
                "type": "object",
                "additionalProperties": false,
                "properties": {
                    "project": project,
                    "workflow": { "type": "string", "description": "Workflow name; empty = the project's default workflow." },
                    "description": { "type": "string", "description": "The task spec / what to build (fed to nodes as $ARGUMENTS)." },
                    "title": { "type": "string", "description": "Human title for the run." },
                    "base_branch": { "type": "string", "description": "Git base branch; empty = project default." },
                    "real": { "type": "boolean", "description": "Actually execute (default true). false = echo/dry-run." }
                },
                "required": ["project", "description"],
            }
        }),
        json!({
            "name": "run_list",
            "description": "List recent runs (most recent first) with status and per-node rows.",
            "inputSchema": {
                "type": "object",
                "additionalProperties": false,
                "properties": {
                    "limit": { "type": "integer", "description": "Max runs to return (default 50)." }
                }
            }
        }),
        json!({
            "name": "run_status",
            "description": "Get one run's status and per-node detail by run_id.",
            "inputSchema": {
                "type": "object",
                "additionalProperties": false,
                "properties": { "run_id": { "type": "string" } },
                "required": ["run_id"],
            }
        }),
        json!({
            "name": "workflow_catalog",
            "description": "List the building blocks for authoring a workflow in this project: node kinds, provider/model hints, commands, trigger rules.",
            "inputSchema": {
                "type": "object",
                "additionalProperties": false,
                "properties": { "project": project },
                "required": ["project"],
            }
        }),
        json!({
            "name": "workflow_list",
            "description": "List workflows available to the project (bundled defaults + project .harness/workflows; project shadows bundled).",
            "inputSchema": {
                "type": "object",
                "additionalProperties": false,
                "properties": { "project": project },
                "required": ["project"],
            }
        }),
        json!({
            "name": "workflow_get",
            "description": "Get a workflow's editable YAML by name.",
            "inputSchema": {
                "type": "object",
                "additionalProperties": false,
                "properties": { "project": project, "name": { "type": "string" } },
                "required": ["project", "name"],
            }
        }),
        json!({
            "name": "workflow_validate",
            "description": "Validate candidate workflow YAML (parse + cycle/dependency/body checks). Returns the first error or the node summaries.",
            "inputSchema": {
                "type": "object",
                "additionalProperties": false,
                "properties": { "project": project, "yaml": { "type": "string" } },
                "required": ["project", "yaml"],
            }
        }),
        json!({
            "name": "workflow_save",
            "description": "Validate then save a workflow to the project's .harness/workflows/<name>.yaml.",
            "inputSchema": {
                "type": "object",
                "additionalProperties": false,
                "properties": { "project": project, "name": { "type": "string" }, "yaml": { "type": "string" } },
                "required": ["project", "name", "yaml"],
            }
        }),
        json!({
            "name": "workflow_create",
            "description": "Create a new, empty workflow (build it up with workflow_set_node / workflow_connect). Errors if one already exists.",
            "inputSchema": {
                "type": "object",
                "additionalProperties": false,
                "properties": {
                    "project": project,
                    "name": { "type": "string" },
                    "description": { "type": "string" },
                    "provider": { "type": "string", "description": "Default provider (claude/codex/pi)." },
                    "model": { "type": "string" }
                },
                "required": ["project", "name"],
            }
        }),
        json!({
            "name": "workflow_set_node",
            "description": "Add or replace (by id) one node — no YAML. `node` has `id` and exactly one body field (prompt | bash | command | script | loop | approval | cancel) plus optional depends_on, when, category, provider, model, context, trigger_rule, timeout, output_format. Validates the whole DAG.",
            "inputSchema": {
                "type": "object",
                "additionalProperties": false,
                "properties": {
                    "project": project,
                    "name": { "type": "string" },
                    "node": {
                        "type": "object",
                        "properties": { "id": { "type": "string" } },
                        "required": ["id"]
                    }
                },
                "required": ["project", "name", "node"],
            }
        }),
        json!({
            "name": "workflow_remove_node",
            "description": "Remove a node by id and strip it from every dependent's depends_on.",
            "inputSchema": {
                "type": "object",
                "additionalProperties": false,
                "properties": { "project": project, "name": { "type": "string" }, "id": { "type": "string" } },
                "required": ["project", "name", "id"],
            }
        }),
        json!({
            "name": "workflow_connect",
            "description": "Add a dependency edge: `to` now depends on `from`. Catches unknown ids and cycles.",
            "inputSchema": {
                "type": "object",
                "additionalProperties": false,
                "properties": {
                    "project": project,
                    "name": { "type": "string" },
                    "from": { "type": "string", "description": "Upstream node id (runs first)." },
                    "to": { "type": "string", "description": "Downstream node id (gains the dependency)." }
                },
                "required": ["project", "name", "from", "to"],
            }
        }),
    ]
}

// ── JSON-RPC / MCP result helpers ────────────────────────────────────────────

fn success(id: Option<Value>, result: Value) -> Option<Value> {
    let id = id?;
    Some(json!({ "jsonrpc": "2.0", "id": id, "result": result }))
}

fn error(id: Option<Value>, code: i32, message: impl Into<String>) -> Option<Value> {
    let id = id.unwrap_or(Value::Null);
    Some(json!({
        "jsonrpc": "2.0",
        "id": id,
        "error": { "code": code, "message": message.into() },
    }))
}

/// A successful tool result: human text + structured content.
fn to_result(text: impl Into<String>, structured: &impl serde::Serialize) -> Value {
    let structured = serde_json::to_value(structured).unwrap_or(Value::Null);
    json!({
        "content": [ { "type": "text", "text": text.into() } ],
        "structuredContent": structured,
        "isError": false,
    })
}

fn tool_error(message: impl Into<String>) -> Value {
    json!({
        "content": [ { "type": "text", "text": message.into() } ],
        "isError": true,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn catalog_exposes_run_and_authoring_tools() {
        let tools = mcp_tools();
        let by_name = |n: &str| tools.iter().find(|t| t["name"] == n).cloned();

        for expected in [
            "run_trigger",
            "run_list",
            "run_status",
            "workflow_catalog",
            "workflow_create",
            "workflow_set_node",
            "workflow_connect",
        ] {
            assert!(by_name(expected).is_some(), "missing tool `{expected}`");
        }

        // run_trigger needs a project + the task spec; authoring tools need project.
        let trigger = by_name("run_trigger").unwrap();
        let req = trigger["inputSchema"]["required"].as_array().unwrap();
        assert!(req.iter().any(|v| v == "project"));
        assert!(req.iter().any(|v| v == "description"));

        let catalog = by_name("workflow_catalog").unwrap();
        let req = catalog["inputSchema"]["required"].as_array().unwrap();
        assert!(req.iter().any(|v| v == "project"));
    }

    #[test]
    fn jsonrpc_helpers_shape() {
        // A request (id present) gets a response; a notification (no id) doesn't.
        let ok = success(Some(json!(7)), json!({"a": 1})).unwrap();
        assert_eq!(ok["jsonrpc"], "2.0");
        assert_eq!(ok["id"], 7);
        assert_eq!(ok["result"]["a"], 1);
        assert!(success(None, json!({})).is_none());

        let e = error(Some(json!(1)), JSONRPC_METHOD_NOT_FOUND, "nope").unwrap();
        assert_eq!(e["error"]["code"], JSONRPC_METHOD_NOT_FOUND);
        assert_eq!(e["error"]["message"], "nope");

        // Tool results carry the MCP envelope.
        let r = to_result("hi", &json!({ "run_id": "x" }));
        assert_eq!(r["isError"], false);
        assert_eq!(r["structuredContent"]["run_id"], "x");
        assert_eq!(r["content"][0]["text"], "hi");
        assert_eq!(tool_error("boom")["isError"], true);
    }
}
