//! The workflow **library** — browsing the registry and installing from it.
//!
//! - `GET    /api/library`                 — the listing, annotated with what is installed here
//! - `POST   /api/library/{slug}/install`  — install, or update to the latest version
//! - `DELETE /api/library/{slug}`          — uninstall
//!
//! **The harness proxies rather than letting the browser call the registry.**
//! Installing writes a file and records a row, which only the server can do; and
//! the listing is only useful once it says which entries are already here, which
//! only the server knows. Sending the browser to the registry directly would
//! mean two round trips and a join done in the client.
//!
//! Every install is a **copy**. Nothing here ever changes a workflow already on
//! disk except when somebody asks for an update, and the YAML goes through
//! `save_workflow` exactly as hand-authored YAML does — so a broken DAG is
//! refused at the door rather than discovered on the next run.

use std::sync::Arc;

use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::Json;
use harness_runner::authoring;
use serde::Serialize;

use super::runs_routes::RunsState;
use super::state::AppState;

fn err(status: StatusCode, msg: impl Into<String>) -> Response {
    (status, Json(serde_json::json!({ "error": msg.into() }))).into_response()
}

/// A library entry as this harness sees it: the registry's row, plus what is
/// true locally.
#[derive(Debug, Serialize)]
pub struct LibraryEntry {
    #[serde(flatten)]
    pub workflow: crate::registry::LibraryWorkflow,
    /// The local file stem, when this is installed here.
    pub installed_as: Option<String>,
    /// The version on disk, which is not necessarily the latest.
    pub installed_version: Option<i32>,
    /// Whether the registry has something newer than what is installed.
    pub update_available: bool,
}

/// `GET /api/library`
///
/// A registry that cannot be reached is reported as an error rather than an
/// empty library: "nothing published yet" and "we could not ask" look identical
/// otherwise, and only one of them is worth retrying.
pub async fn list(
    axum::extract::Extension(runs): axum::extract::Extension<Arc<RunsState>>,
) -> Response {
    let Some(client) = runs.registry() else {
        return err(
            StatusCode::NOT_IMPLEMENTED,
            "the workflow library is switched off on this harness",
        );
    };
    let listing = match client.list().await {
        Ok(l) => l,
        Err(e) => return err(StatusCode::BAD_GATEWAY, e.to_string()),
    };

    // Installed state is a local join. A harness with no database can still
    // browse — it just cannot say what is installed, which is honest rather
    // than wrong.
    let installed = match runs.installed_workflow_store().await {
        Ok(store) => store.all().await.unwrap_or_default(),
        Err(_) => Vec::new(),
    };

    let entries: Vec<LibraryEntry> = listing
        .into_iter()
        .map(|workflow| {
            let local = installed.iter().find(|i| i.slug == workflow.slug);
            // An update is only offered when the registry actually has a higher
            // version. Comparing against `latest_version` being merely
            // *different* would offer a downgrade after a withdrawal.
            let update_available = match (local, workflow.latest_version) {
                (Some(l), Some(latest)) => latest > l.version,
                _ => false,
            };
            LibraryEntry {
                installed_as: local.map(|l| l.name.clone()),
                installed_version: local.map(|l| l.version),
                update_available,
                workflow,
            }
        })
        .collect();

    Json(entries).into_response()
}

/// `POST /api/library/{slug}/install`
///
/// Installs the latest version, or moves an existing install up to it. The same
/// route for both because they are the same operation: fetch a version, write
/// it, record it.
pub async fn install(
    State(state): State<Arc<AppState>>,
    axum::extract::Extension(runs): axum::extract::Extension<Arc<RunsState>>,
    headers: axum::http::HeaderMap,
    Path(slug): Path<String>,
) -> Response {
    let Some(client) = runs.registry() else {
        return err(
            StatusCode::NOT_IMPLEMENTED,
            "the workflow library is switched off on this harness",
        );
    };

    let listing = match client.list().await {
        Ok(l) => l,
        Err(e) => return err(StatusCode::BAD_GATEWAY, e.to_string()),
    };
    let Some(entry) = listing.into_iter().find(|w| w.slug == slug) else {
        return err(
            StatusCode::NOT_FOUND,
            format!("the library has no workflow `{slug}`"),
        );
    };
    let Some(version) = entry.latest_version else {
        return err(
            StatusCode::CONFLICT,
            format!("`{slug}` has no published version to install"),
        );
    };

    let doc = match client.version(&slug, version).await {
        Ok(d) => d,
        Err(e) => return err(StatusCode::BAD_GATEWAY, e.to_string()),
    };

    let store = match runs.installed_workflow_store().await {
        Ok(s) => s,
        Err(e) => return err(StatusCode::SERVICE_UNAVAILABLE, e),
    };

    // Reinstalling keeps the file name it already has. Choosing a fresh one
    // would leave the old file behind as an orphan that still runs.
    let existing = store.by_slug(&slug).await.ok().flatten();
    let name = match existing.as_ref() {
        Some(i) => i.name.clone(),
        None => match local_name_for(&state.core.project_root, &slug) {
            Ok(n) => n,
            Err(e) => return err(StatusCode::CONFLICT, e),
        },
    };

    // Through the same door as hand-authored YAML: validated, and refused if the
    // DAG is broken. A registry that published something unrunnable must not be
    // able to put it on disk here.
    if let Err(e) = authoring::save_workflow(&state.core.project_root, &name, &doc.yaml) {
        return err(StatusCode::BAD_REQUEST, e);
    }

    if let Err(e) = store
        .record(&harness_persist::InstallRecord {
            name: &name,
            slug: &slug,
            version: doc.version,
            publisher: Some(&entry.publisher),
            title: Some(&entry.title),
        })
        .await
    {
        return err(StatusCode::INTERNAL_SERVER_ERROR, e.to_string());
    }

    // Who pressed the button, so the editor can say where this workflow came
    // from and who brought it in.
    super::workflows_routes::record_edit(
        &runs,
        &headers,
        &name,
        super::workflows_routes::EDIT_SOURCE_LIBRARY,
    )
    .await;

    // Best-effort, and last: the workflow is already installed by this point, and
    // a registry that cannot be told must not turn a successful install into a
    // failed request.
    if let Ok(installation_id) = runs.installation_id().await {
        if let Err(e) = client
            .record_install(&slug, &installation_id, doc.version)
            .await
        {
            tracing::warn!("library: installed {slug} but could not report it: {e}");
        }
    }

    Json(serde_json::json!({
        "installed_as": name,
        "version": doc.version,
        "withdrawn": doc.withdrawn,
    }))
    .into_response()
}

/// `DELETE /api/library/{slug}` — remove an installed workflow and stop being
/// counted for it.
pub async fn uninstall(
    State(state): State<Arc<AppState>>,
    axum::extract::Extension(runs): axum::extract::Extension<Arc<RunsState>>,
    Path(slug): Path<String>,
) -> Response {
    let store = match runs.installed_workflow_store().await {
        Ok(s) => s,
        Err(e) => return err(StatusCode::SERVICE_UNAVAILABLE, e),
    };
    let Ok(Some(installed)) = store.by_slug(&slug).await else {
        return err(
            StatusCode::NOT_FOUND,
            format!("`{slug}` is not installed here"),
        );
    };

    if let Err(e) = authoring::delete_project_workflow(&state.core.project_root, &installed.name) {
        return err(StatusCode::BAD_REQUEST, e);
    }
    if let Err(e) = store.forget(&installed.name).await {
        return err(StatusCode::INTERNAL_SERVER_ERROR, e.to_string());
    }
    // The workflow's own provenance goes too, or a later workflow reusing the
    // name inherits an author who never saw it.
    if let Ok(authors) = runs.workflow_author_store().await {
        let _ = authors.forget(&installed.name).await;
    }

    // Best-effort, like the install side.
    if let (Some(client), Ok(installation_id)) = (runs.registry(), runs.installation_id().await) {
        if let Err(e) = client.forget_install(&slug, &installation_id).await {
            tracing::warn!("library: uninstalled {slug} but could not report it: {e}");
        }
    }

    Json(serde_json::json!({ "uninstalled": installed.name })).into_response()
}

/// Pick the local file name for a slug.
///
/// The slug is the library's identifier and the file name is the harness's, and
/// they are allowed to differ — which matters because a slug must never be able
/// to silently shadow a workflow that is already here. A bundled workflow is
/// shadowed by a project file of the same name, so installing
/// `idea-to-pr` from the library would quietly replace the one people rely on.
///
/// So: use the slug when nothing holds that name, and otherwise suffix it rather
/// than overwrite. `-2` is not elegant; silently taking somebody's file name is
/// worse.
fn local_name_for(project_root: &std::path::Path, slug: &str) -> Result<String, String> {
    let taken = |name: &str| {
        project_root
            .join(".harness")
            .join("workflows")
            .join(format!("{name}.yaml"))
            .is_file()
            || harness_runner::defaults::default_workflow(name).is_some()
    };
    if !taken(slug) {
        return Ok(slug.to_string());
    }
    for n in 2..=20 {
        let candidate = format!("{slug}-{n}");
        if !taken(&candidate) {
            return Ok(candidate);
        }
    }
    Err(format!(
        "`{slug}` is already taken here, and so is every name up to `{slug}-20`"
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    /// A library slug must never silently take the name of a workflow already
    /// here — a bundled one especially, since a project file of the same name
    /// shadows it and the replacement would be invisible.
    #[test]
    fn a_slug_never_takes_a_name_that_is_already_in_use() {
        let dir = TempDir::new().unwrap();
        let root = dir.path();

        // Free name: used as-is.
        assert_eq!(
            local_name_for(root, "geo-audit-ecommerce").unwrap(),
            "geo-audit-ecommerce"
        );

        // A bundled workflow's name is taken even with no file on disk.
        assert_eq!(local_name_for(root, "idea-to-pr").unwrap(), "idea-to-pr-2");

        // And a project file on disk is taken too.
        let workflows = root.join(".harness").join("workflows");
        std::fs::create_dir_all(&workflows).unwrap();
        std::fs::write(workflows.join("mine.yaml"), "name: mine").unwrap();
        assert_eq!(local_name_for(root, "mine").unwrap(), "mine-2");

        std::fs::write(workflows.join("mine-2.yaml"), "name: mine-2").unwrap();
        assert_eq!(local_name_for(root, "mine").unwrap(), "mine-3");
    }
}
