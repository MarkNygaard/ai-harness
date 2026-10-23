//! Workflow **authoring** API (visual editor + MCP) — thin HTTP over
//! [`harness_runner::authoring`], the shared core. Workflows are **global**: all
//! handlers operate on the cluster's `.harness/workflows` + the bundled defaults
//! (there is no per-project workflow storage).
//!
//! - `GET  /api/authoring/catalog`          — building blocks (kinds, providers, commands)
//! - `GET  /api/authoring/workflows`        — list (bundled + custom)
//! - `GET  /api/authoring/workflows/{name}` — a workflow's editable source
//! - `POST /api/authoring/validate`         — `{yaml}` → structural validation
//! - `POST /api/authoring/workflows`        — `{name, yaml}` → validate + save
//! - `POST /api/authoring/create`           — `{name, …}` → new empty workflow
//! - `POST /api/authoring/set-node`         — `{name, node}` → add/replace a node
//! - `POST /api/authoring/set-ui`           — `{name, ui}` → set/clear the `ui:` block
//! - `POST /api/authoring/remove-node`      — `{name, id}` → delete a node
//! - `POST /api/authoring/connect`          — `{name, from, to}` → add an edge
//! - `POST /api/authoring/steps`            — `{label, description, node}` → save a step
//! - `DELETE /api/authoring/steps/{id}`     — delete a saved step

use std::sync::Arc;

use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::Json;
use harness_runner::authoring;
use serde::Deserialize;

use super::state::AppState;

fn err(status: StatusCode, msg: impl Into<String>) -> Response {
    (status, Json(serde_json::json!({ "error": msg.into() }))).into_response()
}

/// Note who just changed a workflow.
///
/// **Best-effort, and deliberately so.** The workflow is already saved to disk
/// by the time this runs; failing the request because a note about it could not
/// be written would throw away work somebody just did, over bookkeeping. An
/// install with no database therefore authors workflows exactly as before, with
/// nobody recorded — which is the same thing a missing row already means.
pub(crate) async fn record_edit(
    runs: &Arc<super::runs_routes::RunsState>,
    headers: &axum::http::HeaderMap,
    name: &str,
    source: &str,
) {
    let (user_id, actor) = super::accounts::caller_trigger(runs, headers).await;
    record_edit_as(runs, name, user_id.as_deref(), actor.as_deref(), source).await
}

/// [`record_edit`] for a caller already resolved — the MCP endpoint works out
/// whose token it is holding once per request, and re-reading the headers here
/// would be a second lookup for an answer it already has.
pub(crate) async fn record_edit_as(
    runs: &Arc<super::runs_routes::RunsState>,
    name: &str,
    user_id: Option<&str>,
    actor: Option<&str>,
    source: &str,
) {
    let store = match runs.workflow_author_store().await {
        Ok(s) => s,
        Err(e) => {
            tracing::debug!("authoring: not recording who edited {name}: {e}");
            return;
        }
    };
    let editor = harness_persist::Editor {
        user_id,
        actor,
        source: Some(source),
    };
    if let Err(e) = store.record_edit(name, &editor).await {
        tracing::warn!("authoring: could not record who edited {name}: {e}");
    }
}

/// Where an authoring change came from. The editor and an MCP client are the
/// only two ways in, and telling them apart is half of what the record is for.
pub(crate) const EDIT_SOURCE_UI: &str = "ui";
pub(crate) const EDIT_SOURCE_MCP: &str = "mcp";
/// The built-in chat — MCP underneath, but a person in the harness's own UI
/// asking for it, which is a different story to tell about a change.
pub(crate) const EDIT_SOURCE_CHAT: &str = "chat";
/// Not authored here at all — brought in from the library by whoever pressed
/// Install. Worth its own value: "created by" on a library workflow means
/// "installed by", and conflating it with hand-authoring would misattribute
/// somebody else's work.
pub(crate) const EDIT_SOURCE_LIBRARY: &str = "library";

/// `GET /api/authoring/catalog` — the bundled building blocks, plus the steps
/// people saved (`saved_steps`), each marked with whether the caller may
/// delete it.
pub async fn get_catalog(
    State(state): State<Arc<AppState>>,
    axum::extract::Extension(runs): axum::extract::Extension<Arc<super::runs_routes::RunsState>>,
    headers: axum::http::HeaderMap,
) -> Response {
    let creds = crate::http::credentials_routes::connected_clis().await;
    let catalog = authoring::catalog(&state.core.project_root, creds);
    let mut out = match serde_json::to_value(&catalog) {
        Ok(v) => v,
        Err(e) => return err(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()),
    };
    // No database: the catalog is exactly what it always was.
    let saved = match runs.saved_step_store().await {
        Ok(store) => store.list().await.unwrap_or_default(),
        Err(_) => Vec::new(),
    };
    let caller = Caller::of(&runs, &headers).await;
    let saved: Vec<serde_json::Value> = saved
        .into_iter()
        .map(|s| {
            serde_json::json!({
                "id": s.id,
                "label": s.label,
                "description": s.description,
                "node": serde_json::from_str::<serde_json::Value>(&s.node)
                    .unwrap_or(serde_json::Value::Null),
                "created_by_label": s.created_by_label,
                "created_at": s.created_at,
                "deletable": caller.may_delete(s.created_by.as_deref()),
            })
        })
        .collect();
    out["saved_steps"] = serde_json::Value::Array(saved);
    Json(out).into_response()
}

/// Who is asking, as far as saved steps care.
struct Caller {
    /// Without accounts there are no owners: whoever got in may tidy up.
    open: bool,
    user_id: Option<String>,
    admin: bool,
}

impl Caller {
    async fn of(
        runs: &Arc<super::runs_routes::RunsState>,
        headers: &axum::http::HeaderMap,
    ) -> Self {
        use super::accounts::{authenticated_user, mode, Mode, ROLE_ADMIN};
        let open = mode(runs).await != Mode::Accounts;
        let user = authenticated_user(runs, headers).await;
        Caller {
            open,
            admin: user.as_ref().is_some_and(|u| u.role == ROLE_ADMIN),
            user_id: user.map(|u| u.id),
        }
    }

    /// A saved step is deleted by whoever saved it, or an administrator.
    fn may_delete(&self, created_by: Option<&str>) -> bool {
        self.open || self.admin || (self.user_id.is_some() && self.user_id.as_deref() == created_by)
    }
}

#[derive(Deserialize)]
pub struct SaveStepBody {
    pub label: String,
    #[serde(default)]
    pub description: String,
    /// The node, as the editor holds it.
    pub node: serde_json::Value,
}

/// `POST /api/authoring/steps` — save a node as a reusable step. Anyone who
/// can use the builder may. The step is a copy: later changes to it never
/// reach workflows that already inserted it.
pub async fn save_step(
    axum::extract::Extension(runs): axum::extract::Extension<Arc<super::runs_routes::RunsState>>,
    headers: axum::http::HeaderMap,
    Json(req): Json<SaveStepBody>,
) -> Response {
    let label = req.label.trim();
    if label.is_empty() {
        return err(StatusCode::BAD_REQUEST, "give the step a name");
    }
    let node = match authoring::validate_step(req.node) {
        Ok(n) => n,
        Err(e) => return err(StatusCode::BAD_REQUEST, e),
    };
    let store = match runs.saved_step_store().await {
        Ok(s) => s,
        Err(e) => return err(StatusCode::SERVICE_UNAVAILABLE, e),
    };
    let (user, actor) = super::accounts::caller_trigger(&runs, &headers).await;
    let id = uuid::Uuid::new_v4().to_string();
    match store
        .create(
            &id,
            label,
            req.description.trim(),
            &node.to_string(),
            user.as_deref(),
            actor.as_deref(),
        )
        .await
    {
        Ok(s) => (
            StatusCode::CREATED,
            Json(serde_json::json!({ "id": s.id, "label": s.label })),
        )
            .into_response(),
        Err(e @ harness_persist::SaveStepError::DuplicateLabel(_)) => {
            err(StatusCode::CONFLICT, e.to_string())
        }
        Err(e) => err(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()),
    }
}

/// `DELETE /api/authoring/steps/{id}` — by whoever saved it, or an
/// administrator. Workflows that inserted it keep their copy.
pub async fn delete_step(
    axum::extract::Extension(runs): axum::extract::Extension<Arc<super::runs_routes::RunsState>>,
    headers: axum::http::HeaderMap,
    Path(id): Path<String>,
) -> Response {
    let store = match runs.saved_step_store().await {
        Ok(s) => s,
        Err(e) => return err(StatusCode::SERVICE_UNAVAILABLE, e),
    };
    let step = match store.get(&id).await {
        Ok(Some(s)) => s,
        Ok(None) => return err(StatusCode::NOT_FOUND, "no such step"),
        Err(e) => return err(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()),
    };
    if !Caller::of(&runs, &headers)
        .await
        .may_delete(step.created_by.as_deref())
    {
        return err(
            StatusCode::FORBIDDEN,
            "only whoever saved this step, or an administrator, can delete it",
        );
    }
    match store.delete(&id).await {
        Ok(_) => StatusCode::NO_CONTENT.into_response(),
        Err(e) => err(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()),
    }
}

/// `GET /api/authoring/workflows`
///
/// Each custom workflow carries its authorship where one is recorded. Joined in
/// here rather than stored beside the file, and absent rather than empty when
/// unknown — a bundled workflow has no author at all, and one dropped into the
/// directory by hand has none we know of.
pub async fn list_workflows(
    State(state): State<Arc<AppState>>,
    axum::extract::Extension(runs): axum::extract::Extension<Arc<super::runs_routes::RunsState>>,
) -> Response {
    let workflows = authoring::list_workflows(&state.core.project_root);
    let authors: std::collections::HashMap<String, harness_persist::WorkflowAuthorship> =
        match runs.workflow_author_store().await {
            Ok(store) => store
                .all()
                .await
                .unwrap_or_default()
                .into_iter()
                .map(|a| (a.name.clone(), a))
                .collect(),
            // No database: the list is exactly what it always was.
            Err(_) => Default::default(),
        };
    // Which of these came from the library. Read from the local table rather
    // than the registry: this listing must be right with the registry down, and
    // "where did this workflow come from" is a fact about this harness.
    let installed: std::collections::HashMap<String, harness_persist::InstalledWorkflow> =
        match runs.installed_workflow_store().await {
            Ok(store) => store
                .all()
                .await
                .unwrap_or_default()
                .into_iter()
                .map(|i| (i.name.clone(), i))
                .collect(),
            Err(_) => Default::default(),
        };

    let rows: Vec<serde_json::Value> = workflows
        .into_iter()
        .map(|w| {
            let author = authors.get(&w.name);
            let mut row = serde_json::to_value(&w).unwrap_or_else(|_| serde_json::json!({}));
            if let Some(obj) = row.as_object_mut() {
                if let Some(a) = author {
                    obj.insert(
                        "authorship".into(),
                        serde_json::to_value(a).unwrap_or(serde_json::Value::Null),
                    );
                }
                if let Some(i) = installed.get(&w.name) {
                    obj.insert(
                        "installed".into(),
                        serde_json::to_value(i).unwrap_or(serde_json::Value::Null),
                    );
                }
            }
            row
        })
        .collect();
    Json(rows).into_response()
}

/// `GET /api/authoring/workflows/{name}`
pub async fn get_workflow(
    State(state): State<Arc<AppState>>,
    Path(name): Path<String>,
) -> Response {
    match authoring::get_workflow(&state.core.project_root, &name) {
        Ok(src) => Json(src).into_response(),
        Err(e) => err(StatusCode::NOT_FOUND, e),
    }
}

/// `POST /api/authoring/validate`
pub async fn validate_workflow(Json(req): Json<authoring::WorkflowYaml>) -> Response {
    Json(authoring::validate_workflow(&req.yaml)).into_response()
}

/// `POST /api/authoring/workflows`
pub async fn save_workflow(
    State(state): State<Arc<AppState>>,
    axum::extract::Extension(runs): axum::extract::Extension<Arc<super::runs_routes::RunsState>>,
    headers: axum::http::HeaderMap,
    Json(req): Json<authoring::SaveWorkflow>,
) -> Response {
    match authoring::save_workflow(&state.core.project_root, &req.name, &req.yaml) {
        Ok(()) => {
            record_edit(&runs, &headers, &req.name, EDIT_SOURCE_UI).await;
            Json(serde_json::json!({ "saved": true, "name": req.name })).into_response()
        }
        Err(e) => err(StatusCode::BAD_REQUEST, e),
    }
}

/// `DELETE /api/authoring/workflows/{name}` — remove a project override so a
/// bundled workflow reverts to its built-in default. A no-op (`reset: false`)
/// when there's no project copy; never deletes a bundled default.
pub async fn delete_workflow(
    State(state): State<Arc<AppState>>,
    axum::extract::Extension(runs): axum::extract::Extension<Arc<super::runs_routes::RunsState>>,
    Path(name): Path<String>,
) -> Response {
    match authoring::delete_project_workflow(&state.core.project_root, &name) {
        Ok(reset) => {
            // Only when a file actually went away. Forgetting on a no-op would
            // discard the provenance of a workflow that is still there.
            if reset {
                if let Ok(store) = runs.workflow_author_store().await {
                    if let Err(e) = store.forget(&name).await {
                        tracing::warn!("authoring: could not forget who wrote {name}: {e}");
                    }
                }
                // A workflow deleted here is as gone as one uninstalled from
                // the library dialog. Without this the Library would still call
                // it installed and offer an Update for a file that is not
                // there, and the registry would keep counting it.
                super::library_routes::forget_installed(&runs, &name).await;
            }
            Json(serde_json::json!({ "reset": reset, "name": name })).into_response()
        }
        Err(e) => err(StatusCode::BAD_REQUEST, e),
    }
}

/// `GET /api/authoring/workflows/{name}/versions` — earlier versions, newest
/// first.
pub async fn list_versions(
    State(state): State<Arc<AppState>>,
    Path(name): Path<String>,
) -> Response {
    match authoring::list_versions(&state.core.project_root, &name) {
        Ok(versions) => Json(versions).into_response(),
        Err(e) => err(StatusCode::BAD_REQUEST, e),
    }
}

/// `GET /api/authoring/workflows/{name}/versions/{id}` — one version's YAML.
pub async fn get_version(
    State(state): State<Arc<AppState>>,
    Path((name, id)): Path<(String, String)>,
) -> Response {
    match authoring::get_version(&state.core.project_root, &name, &id) {
        Ok(yaml) => {
            Json(serde_json::json!({ "name": name, "id": id, "yaml": yaml })).into_response()
        }
        Err(e) => err(StatusCode::NOT_FOUND, e),
    }
}

/// `POST /api/authoring/workflows/{name}/versions/{id}/restore` — make an
/// earlier version current. What it replaces becomes a version itself.
pub async fn restore_version(
    State(state): State<Arc<AppState>>,
    axum::extract::Extension(runs): axum::extract::Extension<Arc<super::runs_routes::RunsState>>,
    headers: axum::http::HeaderMap,
    Path((name, id)): Path<(String, String)>,
) -> Response {
    match authoring::restore_version(&state.core.project_root, &name, &id) {
        Ok(()) => {
            record_edit(&runs, &headers, &name, EDIT_SOURCE_UI).await;
            Json(serde_json::json!({ "restored": true, "name": name, "id": id })).into_response()
        }
        Err(e) => err(StatusCode::BAD_REQUEST, e),
    }
}

/// `GET /api/authoring/trash` — deleted workflows that can still be restored,
/// newest first.
pub async fn list_trash(State(state): State<Arc<AppState>>) -> Response {
    Json(authoring::list_trash(&state.core.project_root)).into_response()
}

/// `POST /api/authoring/trash/{id}/restore` — put a deleted workflow back.
/// `409` when a workflow of that name exists again: restoring never overwrites.
pub async fn restore_workflow(
    State(state): State<Arc<AppState>>,
    axum::extract::Extension(runs): axum::extract::Extension<Arc<super::runs_routes::RunsState>>,
    headers: axum::http::HeaderMap,
    Path(id): Path<String>,
) -> Response {
    match authoring::restore_workflow(&state.core.project_root, &id) {
        Ok(name) => {
            // The delete forgot who wrote it; the restorer is who brought it
            // back, which is the honest record now.
            record_edit(&runs, &headers, &name, EDIT_SOURCE_UI).await;
            Json(serde_json::json!({ "restored": true, "name": name })).into_response()
        }
        Err(e @ authoring::RestoreError::NameTaken(_)) => err(StatusCode::CONFLICT, e.to_string()),
        Err(e @ authoring::RestoreError::NotInBin(_)) => err(StatusCode::NOT_FOUND, e.to_string()),
        Err(e @ authoring::RestoreError::Io(_)) => {
            err(StatusCode::INTERNAL_SERVER_ERROR, e.to_string())
        }
    }
}

/// Echo the resulting workflow's node summaries after a mutation so the client
/// sees the new DAG state (the build→validate→fix loop).
///
/// Every node-level authoring change routes through here, which is also why the
/// edit is recorded here: one place, so a new mutation cannot be added that
/// quietly forgets to say who made it.
async fn mutation_result(
    runs: &Arc<super::runs_routes::RunsState>,
    headers: &axum::http::HeaderMap,
    root: &std::path::Path,
    name: &str,
    r: Result<(), String>,
) -> Response {
    match r {
        Ok(()) => {
            record_edit(runs, headers, name, EDIT_SOURCE_UI).await;
            match authoring::get_workflow(root, name) {
                Ok(src) => Json(authoring::validate_workflow(&src.yaml)).into_response(),
                Err(e) => err(StatusCode::INTERNAL_SERVER_ERROR, e),
            }
        }
        Err(e) => err(StatusCode::BAD_REQUEST, e),
    }
}

#[derive(Deserialize)]
pub struct CreateBody {
    pub name: String,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub provider: Option<String>,
    #[serde(default)]
    pub model: Option<String>,
}

/// `POST /api/authoring/create` — new empty workflow.
pub async fn create_workflow(
    State(state): State<Arc<AppState>>,
    axum::extract::Extension(runs): axum::extract::Extension<Arc<super::runs_routes::RunsState>>,
    headers: axum::http::HeaderMap,
    Json(req): Json<CreateBody>,
) -> Response {
    let root = &state.core.project_root;
    let r = authoring::create_workflow(
        root,
        &req.name,
        req.description.as_deref(),
        req.provider.as_deref(),
        req.model.as_deref(),
    );
    mutation_result(&runs, &headers, root, &req.name, r).await
}

#[derive(Deserialize)]
pub struct SetNodeBody {
    pub name: String,
    pub node: serde_json::Value,
}

/// `POST /api/authoring/set-node` — add or replace a node by id.
pub async fn set_node(
    State(state): State<Arc<AppState>>,
    axum::extract::Extension(runs): axum::extract::Extension<Arc<super::runs_routes::RunsState>>,
    headers: axum::http::HeaderMap,
    Json(req): Json<SetNodeBody>,
) -> Response {
    let root = &state.core.project_root;
    let r = authoring::set_node(root, &req.name, req.node);
    mutation_result(&runs, &headers, root, &req.name, r).await
}

#[derive(Deserialize)]
pub struct SetUiBody {
    pub name: String,
    /// The `ui` block (`{ nav?, report? }`), or `null` to clear it.
    #[serde(default)]
    pub ui: serde_json::Value,
}

/// `POST /api/authoring/set-ui` — set or clear a workflow's `ui:` block.
pub async fn set_ui(
    State(state): State<Arc<AppState>>,
    axum::extract::Extension(runs): axum::extract::Extension<Arc<super::runs_routes::RunsState>>,
    headers: axum::http::HeaderMap,
    Json(req): Json<SetUiBody>,
) -> Response {
    let root = &state.core.project_root;
    let r = authoring::set_ui(root, &req.name, req.ui);
    mutation_result(&runs, &headers, root, &req.name, r).await
}

#[derive(Deserialize)]
pub struct RemoveNodeBody {
    pub name: String,
    pub id: String,
}

/// `POST /api/authoring/remove-node` — delete a node and strip it from dependents.
pub async fn remove_node(
    State(state): State<Arc<AppState>>,
    axum::extract::Extension(runs): axum::extract::Extension<Arc<super::runs_routes::RunsState>>,
    headers: axum::http::HeaderMap,
    Json(req): Json<RemoveNodeBody>,
) -> Response {
    let root = &state.core.project_root;
    let r = authoring::remove_node(root, &req.name, &req.id);
    mutation_result(&runs, &headers, root, &req.name, r).await
}

#[derive(Deserialize)]
pub struct ConnectBody {
    pub name: String,
    pub from: String,
    pub to: String,
}

/// `POST /api/authoring/connect` — add a dependency edge (`to` depends on `from`).
pub async fn connect_nodes(
    State(state): State<Arc<AppState>>,
    axum::extract::Extension(runs): axum::extract::Extension<Arc<super::runs_routes::RunsState>>,
    headers: axum::http::HeaderMap,
    Json(req): Json<ConnectBody>,
) -> Response {
    let root = &state.core.project_root;
    let r = authoring::connect_nodes(root, &req.name, &req.from, &req.to);
    mutation_result(&runs, &headers, root, &req.name, r).await
}

#[cfg(test)]
mod tests {
    use super::*;

    fn caller(open: bool, user: Option<&str>, admin: bool) -> Caller {
        Caller {
            open,
            user_id: user.map(str::to_string),
            admin,
        }
    }

    #[test]
    fn a_saved_step_is_deleted_by_its_author_or_an_admin() {
        let ann = caller(false, Some("ann"), false);
        assert!(ann.may_delete(Some("ann")));
        assert!(!ann.may_delete(Some("bob")));
        // Saved before accounts existed: nobody owns it, so only an admin.
        assert!(!ann.may_delete(None));
        assert!(caller(false, Some("root"), true).may_delete(Some("bob")));
        assert!(!caller(false, None, false).may_delete(None), "signed out");
        // Without accounts there are no owners to protect.
        assert!(caller(true, None, false).may_delete(Some("bob")));
    }
}
