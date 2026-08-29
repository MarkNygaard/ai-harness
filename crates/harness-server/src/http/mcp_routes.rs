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
//! **Auth is this route's own.** `/mcp` is exempt from the global bearer-token
//! middleware, because an editor holding only the MCP key would otherwise be
//! turned away by a middleware that knows nothing about that key. Instead every
//! call goes through [`super::mcp_key::authorized`], which accepts the MCP key
//! or the legacy shared token — the same two-layer arrangement `/ws` already
//! uses, and for the same reason.

use std::path::Path;
use std::sync::Arc;

use axum::extract::Extension;
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::Json;
use harness_runner::authoring;
use serde_json::{json, Value};

use super::mcp_key;
use super::runs_routes::{
    resolve_workflow_models, start_run, start_run_pair, CreateRunPairRequest, CreateRunRequest,
    ModelRef, RunsState,
};

const MCP_PROTOCOL_VERSION: &str = "2024-11-05";
const JSONRPC_METHOD_NOT_FOUND: i32 = -32601;

/// `POST /mcp` — handle one JSON-RPC request (or a batch array). Notifications
/// (no `id`) produce no response; a request with an `id` produces one.
pub async fn handle_mcp(
    Extension(state): Extension<Arc<RunsState>>,
    headers: HeaderMap,
    Json(body): Json<Value>,
) -> Response {
    // Whose token this is, if it is a personal one. The shared MCP key belongs
    // to nobody in particular, so a run it starts is attributed to nobody.
    let actor = super::accounts::caller_id(&state, &headers).await;
    if !mcp_key::authorized(&state, &headers).await {
        // A plain 401 rather than a JSON-RPC error: the caller never got as far
        // as a session, and MCP clients surface the HTTP status.
        return (
            StatusCode::UNAUTHORIZED,
            Json(json!({
                "error": "missing or invalid MCP key — copy the connection snippet from \
                          Settings → Editor connection"
            })),
        )
            .into_response();
    }
    if let Some(batch) = body.as_array() {
        let mut out = Vec::new();
        for req in batch {
            if let Some(resp) = handle_one(&state, actor.as_deref(), req).await {
                out.push(resp);
            }
        }
        if out.is_empty() {
            return StatusCode::ACCEPTED.into_response();
        }
        return Json(Value::Array(out)).into_response();
    }
    match handle_one(&state, actor.as_deref(), &body).await {
        Some(resp) => Json(resp).into_response(),
        None => StatusCode::ACCEPTED.into_response(),
    }
}

async fn handle_one(state: &Arc<RunsState>, actor: Option<&str>, req: &Value) -> Option<Value> {
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
            let result = call_tool(state, actor, &name, &args).await;
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

/// After a successful authoring mutation, report the resulting DAG's node
/// summaries (the build→validate→fix loop sees the new state).
fn state_after(dir: &Path, name: &str, msg: String) -> Value {
    match authoring::get_workflow(dir, name) {
        Ok(src) => to_result(msg, &authoring::validate_workflow(&src.yaml)),
        Err(e) => tool_error(e),
    }
}

async fn call_tool(state: &Arc<RunsState>, actor: Option<&str>, name: &str, args: &Value) -> Value {
    let s = |k: &str| {
        args.get(k)
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string()
    };
    let i = |k: &str, default: i64| args.get(k).and_then(Value::as_i64).unwrap_or(default);

    match name {
        // ── Run control ─────────────────────────────────────────────────────
        "run_trigger" => {
            let req = CreateRunRequest {
                // An editor triggering a run is not acting on a Linear issue.
                issue_id: None,
                triggered_by: actor.map(str::to_string),
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
                swap_from: None,
                swap_to: None,
                ab_pair_id: None,
                ab_arm: None,
                ab_label: None,
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
                // `structuredContent` must be a JSON object, never a bare array.
                Ok(runs) => to_result(format!("{} run(s)", runs.len()), &json!({ "runs": runs })),
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
        "run_findings" => {
            let id = s("run_id");
            if id.is_empty() {
                return tool_error("`run_id` is required");
            }
            let store = match state.finding_store().await {
                Ok(s) => s,
                Err(e) => return tool_error(e),
            };
            match store.list_for_run(&id).await {
                Ok(rows) => to_result(
                    format!("{} finding state(s) for run {id}", rows.len()),
                    &json!({ "findings": rows }),
                ),
                Err(e) => tool_error(e.to_string()),
            }
        }
        "run_activity_errors" => {
            let store = match state.store().await {
                Ok(s) => s,
                Err(e) => return tool_error(e),
            };
            let project = s("project");
            let days = i("days", 14) as i32;
            // Bounds the raw scan, not the reply: the grouping collapses it.
            let scan = i("scan_limit", 5000);
            let limit = i("limit", 25).clamp(1, 200) as usize;
            let project = (!project.is_empty()).then_some(project);
            match store
                .activity_error_groups(project.as_deref(), days, scan)
                .await
            {
                Ok(mut groups) => {
                    let total: i64 = groups.iter().map(|g| g.count).sum();
                    groups.truncate(limit);
                    to_result(
                        format!(
                            "{} distinct failure(s), {total} occurrence(s), last {days}d",
                            groups.len()
                        ),
                        &json!({ "groups": groups }),
                    )
                }
                Err(e) => tool_error(e.to_string()),
            }
        }
        "run_linear_claim" => {
            let id = s("run_id");
            if id.is_empty() {
                return tool_error("`run_id` is required");
            }
            let store = match state.linear_claim_store().await {
                Ok(s) => s,
                Err(e) => return tool_error(e),
            };
            match store.claim_for_run(&id).await {
                Ok(Some(claim)) => to_result(
                    format!(
                        "run {id} — {} / phase {} / session {}",
                        claim.identifier,
                        claim.phase,
                        claim.agent_session_id.as_deref().unwrap_or("(none)")
                    ),
                    &claim,
                ),
                // Not an error: most runs are not Linear-triggered at all, and
                // "no row" is itself the answer when one was expected.
                Ok(None) => to_result(
                    format!("run {id} has no Linear claim"),
                    &json!({ "claim": null }),
                ),
                Err(e) => tool_error(e.to_string()),
            }
        }
        "workflow_models" => {
            match resolve_workflow_models(state, &s("workflow"), Some(&s("project"))) {
                Ok(pairs) => to_result(
                    format!("{} model pair(s) in `{}`", pairs.len(), s("workflow")),
                    &json!({ "pairs": pairs }),
                ),
                Err((_status, msg)) => tool_error(msg),
            }
        }
        "run_trigger_pair" => {
            // swap_from / variant_a / variant_b are {provider, model} objects.
            let model_ref = |k: &str| -> Option<ModelRef> {
                args.get(k)
                    .and_then(|v| serde_json::from_value::<ModelRef>(v.clone()).ok())
            };
            let (Some(swap_from), Some(variant_a), Some(variant_b)) = (
                model_ref("swap_from"),
                model_ref("variant_a"),
                model_ref("variant_b"),
            ) else {
                return tool_error(
                    "`swap_from`, `variant_a`, and `variant_b` are required {provider, model} objects",
                );
            };
            let req = CreateRunPairRequest {
                triggered_by: actor.map(str::to_string),
                workflow: s("workflow"),
                title: args
                    .get("title")
                    .and_then(Value::as_str)
                    .map(str::to_string),
                description: s("description"),
                args: String::new(),
                real: args.get("real").and_then(Value::as_bool).unwrap_or(true),
                base_branch: args
                    .get("base_branch")
                    .and_then(Value::as_str)
                    .map(str::to_string),
                project: Some(s("project")),
                swap_from,
                variant_a,
                variant_b,
            };
            match start_run_pair(state, req).await {
                Ok(resp) => to_result(
                    format!(
                        "A/B pair started: {} (a={}, b={})",
                        resp.pair_id, resp.run_id_a, resp.run_id_b
                    ),
                    &resp,
                ),
                Err((_status, msg)) => tool_error(msg),
            }
        }

        // ── Authoring ───────────────────────────────────────────────────────
        // Custom workflows are GLOBAL (like bundled): they live in the server's
        // `project_root` and apply to every project, so these ignore `project`
        // and resolve against the global root the editor authors into.
        "workflow_catalog" => {
            let creds = crate::http::credentials_routes::connected_clis().await;
            to_result(
                "catalog".to_string(),
                &authoring::catalog(&state.project_root, creds),
            )
        }
        "workflow_list" => {
            let list = authoring::list_workflows(&state.project_root);
            to_result(
                format!("{} workflow(s)", list.len()),
                &json!({ "workflows": list }),
            )
        }
        "workflow_get" => match authoring::get_workflow(&state.project_root, &s("name")) {
            Ok(src) => to_result(src.yaml.clone(), &src),
            Err(e) => tool_error(e),
        },
        "workflow_validate" => {
            let v = authoring::validate_workflow(&s("yaml"));
            let text = if v.valid {
                format!("valid: {} step(s)", v.nodes.len())
            } else {
                format!("invalid: {}", v.error.clone().unwrap_or_default())
            };
            to_result(text, &v)
        }
        "workflow_save" => {
            match authoring::save_workflow(&state.project_root, &s("name"), &s("yaml")) {
                Ok(()) => to_result(
                    format!("saved `{}`", s("name")),
                    &json!({ "saved": true, "name": s("name") }),
                ),
                Err(e) => tool_error(e),
            }
        }
        // Delete a CUSTOM workflow (bundled defaults have no file → can't be
        // deleted; the call reports that rather than silently no-op'ing).
        "workflow_delete" => {
            match authoring::delete_project_workflow(&state.project_root, &s("name")) {
                Ok(true) => to_result(
                    format!("deleted custom workflow `{}`", s("name")),
                    &json!({ "deleted": true, "name": s("name") }),
                ),
                Ok(false) => tool_error(format!(
                    "`{}` is not a custom workflow — bundled defaults can't be deleted",
                    s("name")
                )),
                Err(e) => tool_error(e),
            }
        }
        "workflow_create" => {
            let r = authoring::create_workflow(
                &state.project_root,
                &s("name"),
                args.get("description").and_then(Value::as_str),
                args.get("provider").and_then(Value::as_str),
                args.get("model").and_then(Value::as_str),
            );
            match r {
                Ok(()) => state_after(
                    &state.project_root,
                    &s("name"),
                    format!("created `{}`", s("name")),
                ),
                Err(e) => tool_error(e),
            }
        }
        "workflow_set_node" => {
            let node = args.get("node").cloned().unwrap_or(Value::Null);
            match authoring::set_node(&state.project_root, &s("name"), node) {
                Ok(()) => state_after(
                    &state.project_root,
                    &s("name"),
                    format!("set node in `{}`", s("name")),
                ),
                Err(e) => tool_error(e),
            }
        }
        "workflow_set_ui" => {
            let ui = args.get("ui").cloned().unwrap_or(Value::Null);
            match authoring::set_ui(&state.project_root, &s("name"), ui) {
                Ok(()) => state_after(
                    &state.project_root,
                    &s("name"),
                    format!("set ui on `{}`", s("name")),
                ),
                Err(e) => tool_error(e),
            }
        }
        "workflow_remove_node" => {
            match authoring::remove_node(&state.project_root, &s("name"), &s("id")) {
                Ok(()) => state_after(
                    &state.project_root,
                    &s("name"),
                    format!("removed `{}` from `{}`", s("id"), s("name")),
                ),
                Err(e) => tool_error(e),
            }
        }
        "workflow_connect" => {
            match authoring::connect_nodes(&state.project_root, &s("name"), &s("from"), &s("to")) {
                Ok(()) => state_after(
                    &state.project_root,
                    &s("name"),
                    format!("connected `{}` -> `{}`", s("from"), s("to")),
                ),
                Err(e) => tool_error(e),
            }
        }
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
            "name": "run_findings",
            "description": "Read the per-finding state a human set in a run's report: each finding's `finding_key` (category::title) + `action` — built | issued | ignored | checked | passed | failed — plus any `ref_run_id` / Linear `issue_identifier`+`issue_url`. Use to see which report items a person marked, e.g. which manual test scenarios passed vs failed. Findings with no recorded state are absent from the list.",
            "inputSchema": {
                "type": "object",
                "additionalProperties": false,
                "properties": { "run_id": { "type": "string" } },
                "required": ["run_id"],
            }
        }),
        json!({
            "name": "run_activity_errors",
            "description": "Recurring agent-side failures across runs — what the agents keep tripping over, rather than one run's feed. Each group is identical failures collapsed together: `count` occurrences over `runs` distinct runs, the `workflow`, the `nodes` they happened in, a verbatim `sample`, and first/last seen. Most-repeated first. Use it to decide what belongs in a project's CLAUDE.md: a failure with a high `runs` count is a property of the project (a missing generated file, an absent credential, a command that isn't where the agent looked), not one agent's bad luck. Optional `project` narrows to one project, `days` sets the window (default 14), `limit` caps the groups returned (default 25). Note that a search matching nothing is not counted as a failure.",
            "inputSchema": {
                "type": "object",
                "additionalProperties": false,
                "properties": {
                    "project": { "type": "string" },
                    "days": { "type": "integer" },
                    "limit": { "type": "integer" },
                    "scan_limit": { "type": "integer" }
                },
            }
        }),
        json!({
            "name": "run_linear_claim",
            "description": "Read how a run is tied back to Linear — the claim row the poller sweeps every 30s to report progress and move the issue. Returns `identifier`, `issue_id`, `phase` (claimed | in_review | done), `agent_session_id` (the delegated agent session progress is posted into; null for a column-polled run), `reported_nodes` (which steps have already been announced) and `last_activity_at` (when the session last heard anything). Use when a Linear issue or agent session is not receiving updates for a running run: no row means the run was never linked, a null `agent_session_id` means there is no session to post into, and a `last_activity_at` far behind the run's progress means the posts are being rejected or the poller is not sweeping.",
            "inputSchema": {
                "type": "object",
                "additionalProperties": false,
                "properties": { "run_id": { "type": "string" } },
                "required": ["run_id"],
            }
        }),
        json!({
            "name": "workflow_models",
            "description": "List the distinct provider+model pairs a workflow uses (default, nodes, loop bodies) — the candidate swap targets for an A/B test.",
            "inputSchema": {
                "type": "object",
                "additionalProperties": false,
                "properties": {
                    "project": project,
                    "workflow": { "type": "string", "description": "Workflow name." }
                },
                "required": ["project", "workflow"],
            }
        }),
        json!({
            "name": "run_trigger_pair",
            "description": "Start an A/B pair: two runs of the same task that differ only by which model the swap_from steps use. Arm A swaps swap_from→variant_a, arm B swaps swap_from→variant_b; pick variant_a == swap_from to make A the baseline. Returns pair_id + both run_ids.",
            "inputSchema": {
                "type": "object",
                "additionalProperties": false,
                "properties": {
                    "project": project,
                    "workflow": { "type": "string", "description": "Workflow name; empty = the project's default." },
                    "description": { "type": "string", "description": "The task spec (fed to nodes as $ARGUMENTS)." },
                    "title": { "type": "string", "description": "Human title for the pair." },
                    "base_branch": { "type": "string", "description": "Git base branch; empty = project default." },
                    "swap_from": { "type": "object", "description": "The steps under test, by current provider+model.", "properties": { "provider": { "type": "string" }, "model": { "type": "string" } }, "required": ["provider", "model"] },
                    "variant_a": { "type": "object", "description": "Arm A model (often == swap_from).", "properties": { "provider": { "type": "string" }, "model": { "type": "string" } }, "required": ["provider", "model"] },
                    "variant_b": { "type": "object", "description": "Arm B model (the challenger).", "properties": { "provider": { "type": "string" }, "model": { "type": "string" } }, "required": ["provider", "model"] }
                },
                "required": ["project", "description", "swap_from", "variant_a", "variant_b"],
            }
        }),
        json!({
            "name": "workflow_catalog",
            "description": "List the building blocks for authoring a workflow: node kinds, provider/model hints, commands, trigger rules. Workflows are global.",
            "inputSchema": { "type": "object", "additionalProperties": false, "properties": {} }
        }),
        json!({
            "name": "workflow_list",
            "description": "List workflows available (bundled defaults + global custom workflows; custom shadows bundled).",
            "inputSchema": { "type": "object", "additionalProperties": false, "properties": {} }
        }),
        json!({
            "name": "workflow_get",
            "description": "Get a workflow's editable YAML by name.",
            "inputSchema": {
                "type": "object",
                "additionalProperties": false,
                "properties": { "name": { "type": "string" } },
                "required": ["name"],
            }
        }),
        json!({
            "name": "workflow_validate",
            "description": "Validate candidate workflow YAML (parse + cycle/dependency/body checks). Returns the first error or the node summaries.",
            "inputSchema": {
                "type": "object",
                "additionalProperties": false,
                "properties": { "yaml": { "type": "string" } },
                "required": ["yaml"],
            }
        }),
        json!({
            "name": "workflow_save",
            "description": "Validate then save a global workflow to .harness/workflows/<name>.yaml (runnable by every project). The YAML may include a top-level `ui:` block ({ nav: { label, icon }, report: { label, verdict_node?, scored } }) to give the workflow a left-nav entry + a findings/report tab; or set it incrementally with workflow_set_ui.",
            "inputSchema": {
                "type": "object",
                "additionalProperties": false,
                "properties": { "name": { "type": "string" }, "yaml": { "type": "string" } },
                "required": ["name", "yaml"],
            }
        }),
        json!({
            "name": "workflow_delete",
            "description": "Delete a CUSTOM workflow by name. Bundled defaults have no file and can't be deleted.",
            "inputSchema": {
                "type": "object",
                "additionalProperties": false,
                "properties": { "name": { "type": "string" } },
                "required": ["name"],
            }
        }),
        json!({
            "name": "workflow_create",
            "description": "Create a new, empty workflow (build it up with workflow_set_node / workflow_connect, and optionally workflow_set_ui for a nav entry + report tab). Errors if one already exists.",
            "inputSchema": {
                "type": "object",
                "additionalProperties": false,
                "properties": {
                    "name": { "type": "string" },
                    "description": { "type": "string" },
                    "provider": { "type": "string", "description": "Default provider (claude/codex/pi)." },
                    "model": { "type": "string" }
                },
                "required": ["name"],
            }
        }),
        json!({
            "name": "workflow_set_node",
            "description": "Add or replace (by id) one node — no YAML. `node` has `id` and exactly one body field (prompt | bash | command | script | loop | approval | cancel) plus optional depends_on, when, category, provider, model, context, trigger_rule, timeout, output_format. Validates the whole DAG.",
            "inputSchema": {
                "type": "object",
                "additionalProperties": false,
                "properties": {
                    "name": { "type": "string" },
                    "node": {
                        "type": "object",
                        "properties": { "id": { "type": "string" } },
                        "required": ["id"]
                    }
                },
                "required": ["name", "node"],
            }
        }),
        json!({
            "name": "workflow_set_ui",
            "description": "Set (or clear) the workflow's `ui:` block — a left-nav entry and/or a findings/report tab on its runs. Pass `ui` as { nav?: { label, icon? }, report?: { label, verdict_node?, scored?, actions?, status? } }, or null to clear. `icon` is a key from: shield, world-search, zoom-code, search, report, checklist. `report.verdict_node` names the node whose JSON output ({ summary?, score?, findings: [{ title, severity?, category?, detail?, fix?, location? }] }) is the verdict; `scored:true` shows a score + dimension bars + history. `actions` opts into per-finding buttons (default none = a clean read-only list): any of build (idea-to-pr), issue (Linear), ignore. `status` adds a per-item control: none (default), check (a tested checkbox), or pass_fail (Passed/Failed) — the marks are readable back via run_findings.",
            "inputSchema": {
                "type": "object",
                "additionalProperties": false,
                "properties": {
                    "name": { "type": "string" },
                    "ui": {
                        "type": ["object", "null"],
                        "properties": {
                            "nav": {
                                "type": "object",
                                "properties": {
                                    "label": { "type": "string" },
                                    "icon": { "type": "string" }
                                },
                                "required": ["label"]
                            },
                            "report": {
                                "type": "object",
                                "properties": {
                                    "label": { "type": "string" },
                                    "verdict_node": { "type": "string" },
                                    "scored": { "type": "boolean" },
                                    "actions": {
                                        "type": "array",
                                        "items": { "enum": ["build", "issue", "ignore"] }
                                    },
                                    "status": { "enum": ["none", "check", "pass_fail"] }
                                },
                                "required": ["label"]
                            }
                        }
                    }
                },
                "required": ["name", "ui"],
            }
        }),
        json!({
            "name": "workflow_remove_node",
            "description": "Remove a node by id and strip it from every dependent's depends_on.",
            "inputSchema": {
                "type": "object",
                "additionalProperties": false,
                "properties": { "name": { "type": "string" }, "id": { "type": "string" } },
                "required": ["name", "id"],
            }
        }),
        json!({
            "name": "workflow_connect",
            "description": "Add a dependency edge: `to` now depends on `from`. Catches unknown ids and cycles.",
            "inputSchema": {
                "type": "object",
                "additionalProperties": false,
                "properties": {
                    "name": { "type": "string" },
                    "from": { "type": "string", "description": "Upstream node id (runs first)." },
                    "to": { "type": "string", "description": "Downstream node id (gains the dependency)." }
                },
                "required": ["name", "from", "to"],
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
            "run_trigger_pair",
            "workflow_models",
            "run_list",
            "run_status",
            "run_findings",
            "run_linear_claim",
            "run_activity_errors",
            "workflow_catalog",
            "workflow_create",
            "workflow_set_node",
            "workflow_set_ui",
            "workflow_connect",
        ] {
            assert!(by_name(expected).is_some(), "missing tool `{expected}`");
        }

        // run_trigger needs a project + the task spec; authoring tools are global.
        let trigger = by_name("run_trigger").unwrap();
        let req = trigger["inputSchema"]["required"].as_array().unwrap();
        assert!(req.iter().any(|v| v == "project"));
        assert!(req.iter().any(|v| v == "description"));

        // The A/B pair tool requires the swap target and both variants.
        let pair = by_name("run_trigger_pair").unwrap();
        let req = pair["inputSchema"]["required"].as_array().unwrap();
        for field in [
            "project",
            "description",
            "swap_from",
            "variant_a",
            "variant_b",
        ] {
            assert!(
                req.iter().any(|v| v == field),
                "pair tool missing required `{field}`"
            );
        }

        // Authoring tools are global — `workflow_catalog` takes no `project`.
        let catalog = by_name("workflow_catalog").unwrap();
        let has_project = catalog["inputSchema"]
            .get("required")
            .and_then(|v| v.as_array())
            .is_some_and(|r| r.iter().any(|v| v == "project"));
        assert!(!has_project, "authoring tools are global — no project arg");
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

// ── Connecting an editor ─────────────────────────────────────────────────────

/// Tools this endpoint exposes, grouped as the settings page lists them.
const RUN_TOOLS: &[&str] = &[
    "run_trigger",
    "run_trigger_pair",
    "run_list",
    "run_status",
    "run_findings",
];
const AUTHORING_TOOLS: &[&str] = &[
    "workflow_list",
    "workflow_get",
    "workflow_create",
    "workflow_save",
    "workflow_validate",
    "workflow_set_node",
    "workflow_remove_node",
    "workflow_connect",
    "workflow_set_ui",
    "workflow_delete",
    "workflow_catalog",
    "workflow_models",
];

fn connection_body(state: &Arc<RunsState>, token: Option<String>) -> Value {
    json!({
        // `None` when no public URL is configured — the page says so rather
        // than rendering a snippet that would not work.
        "endpoint": state.public_url().map(|b| format!("{b}/mcp")),
        "token": token,
        // True when the endpoint is reachable without any credential, which is
        // worth saying out loud on the page.
        "unauthenticated": token.is_none() && state.api_token().is_none(),
        "run_tools": RUN_TOOLS,
        "authoring_tools": AUTHORING_TOOLS,
    })
}

/// `GET /api/mcp/connection` — everything needed to connect an editor.
///
/// Returns the key in plaintext: the page's whole job is to hand over a snippet
/// you can paste. It sits behind the normal middleware, so in a deployment with
/// a shared token you already had to authenticate to get here.
pub async fn connection(Extension(state): Extension<Arc<RunsState>>) -> Response {
    let token = mcp_key::ensure(&state).await;
    Json(connection_body(&state, token)).into_response()
}

/// `POST /api/mcp/connection` — replace the key.
pub async fn regenerate_connection(Extension(state): Extension<Arc<RunsState>>) -> Response {
    match mcp_key::regenerate(&state).await {
        Ok(token) => Json(connection_body(&state, Some(token))).into_response(),
        Err(e) => (StatusCode::SERVICE_UNAVAILABLE, Json(json!({ "error": e }))).into_response(),
    }
}
