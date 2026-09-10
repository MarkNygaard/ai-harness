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

/// What an install may ask for. Optional in full: the common case is a `POST`
/// with no body at all.
#[derive(Debug, Default, serde::Deserialize)]
pub struct InstallRequest {
    /// Install under this name instead of the slug. How the caller answers a
    /// `409` — the name is chosen by whoever is installing, not invented here.
    #[serde(default)]
    pub name: Option<String>,
}

/// `POST /api/library/{slug}/install`
///
/// Installs the latest version, or moves an existing install up to it. The same
/// route for both because they are the same operation: fetch a version, write
/// it, record it.
///
/// Answers `409` when the name is already in use here, naming the conflict and
/// suggesting a free one. The caller repeats the request with `name` set.
pub async fn install(
    State(state): State<Arc<AppState>>,
    axum::extract::Extension(runs): axum::extract::Extension<Arc<RunsState>>,
    headers: axum::http::HeaderMap,
    Path(slug): Path<String>,
    body: Option<Json<InstallRequest>>,
) -> Response {
    let req = body.map(|Json(b)| b).unwrap_or_default();
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

    let existing = store.by_slug(&slug).await.ok().flatten();
    let name = match destination(
        &state.core.project_root,
        &slug,
        req.name.as_deref(),
        existing.as_ref().map(|i| i.name.as_str()),
    ) {
        Destination::Free(name) => name,
        // A question rather than a workaround: the caller is asked which name
        // to use, and nothing is written until it answers. The suggestion is
        // offered, not applied.
        Destination::Taken { name, suggestion } => {
            return (
                StatusCode::CONFLICT,
                Json(serde_json::json!({
                    "error": format!(
                        "this harness already has a workflow called `{name}`"
                    ),
                    "conflict": name,
                    "suggested_name": suggestion,
                })),
            )
                .into_response()
        }
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

/// Whether a workflow name is already spoken for on this harness.
///
/// Bundled counts even with no file on disk: a project file of the same name
/// *shadows* the bundled workflow, so writing one would replace what people rely
/// on without removing anything, which is the worst version of a collision —
/// nothing looks wrong until a run does the other thing.
fn name_taken(project_root: &std::path::Path, name: &str) -> bool {
    project_root
        .join(".harness")
        .join("workflows")
        .join(format!("{name}.yaml"))
        .is_file()
        || harness_runner::defaults::default_workflow(name).is_some()
}

/// A free name near `slug`, for suggesting one when the obvious name is taken.
fn suggest_name(project_root: &std::path::Path, slug: &str) -> Option<String> {
    (2..=20)
        .map(|n| format!("{slug}-{n}"))
        .find(|candidate| !name_taken(project_root, candidate))
}

/// Where an install would be written, or why it cannot be.
enum Destination {
    /// Free, or the name this slug is already installed under.
    Free(String),
    /// Something else holds it. Carries a suggestion, if one is available.
    Taken {
        name: String,
        suggestion: Option<String>,
    },
}

/// Decide the local file name for an install.
///
/// The slug is the library's identifier and the file name is the harness's, and
/// they are allowed to differ — which matters, because a slug must never be able
/// to take the name of a workflow that is already here.
///
/// **A collision is refused rather than worked around.** Quietly installing as
/// `geo-audit-2` leaves somebody with a workflow whose name they did not choose
/// and no idea why, which is the sort of surprise the whole copy-on-install
/// design exists to avoid. The caller turns this into a question.
fn destination(
    project_root: &std::path::Path,
    slug: &str,
    requested: Option<&str>,
    already_installed_as: Option<&str>,
) -> Destination {
    // Reinstall or update: keep the name it already has. Choosing a fresh one
    // would leave the old file behind as an orphan that still runs.
    if let Some(existing) = already_installed_as {
        return Destination::Free(existing.to_string());
    }
    let name = requested.unwrap_or(slug);
    if name_taken(project_root, name) {
        return Destination::Taken {
            name: name.to_string(),
            suggestion: suggest_name(project_root, slug),
        };
    }
    Destination::Free(name.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn free(d: Destination) -> Option<String> {
        match d {
            Destination::Free(name) => Some(name),
            Destination::Taken { .. } => None,
        }
    }

    /// A free name is used as-is — the ordinary case, and it must not acquire a
    /// suffix for no reason.
    #[test]
    fn a_free_name_is_used_unchanged() {
        let dir = TempDir::new().unwrap();
        assert_eq!(
            free(destination(dir.path(), "geo-audit-ecommerce", None, None)).as_deref(),
            Some("geo-audit-ecommerce")
        );
    }

    /// The collision that matters most, and the one with no visible symptom: a
    /// project file *shadows* a bundled workflow of the same name, so writing
    /// one would replace what people rely on while removing nothing. It has to
    /// be refused even though no file is on disk.
    #[test]
    fn a_bundled_name_is_taken_even_with_no_file_on_disk() {
        let dir = TempDir::new().unwrap();
        match destination(dir.path(), "idea-to-pr", None, None) {
            Destination::Taken { name, suggestion } => {
                assert_eq!(name, "idea-to-pr");
                assert_eq!(suggestion.as_deref(), Some("idea-to-pr-2"));
            }
            Destination::Free(n) => panic!("bundled name must not be free, got {n}"),
        }
    }

    /// Somebody's own workflow is refused too, with a suggestion that skips the
    /// names already used rather than colliding again.
    #[test]
    fn an_existing_workflow_is_refused_and_a_free_name_suggested() {
        let dir = TempDir::new().unwrap();
        let workflows = dir.path().join(".harness").join("workflows");
        std::fs::create_dir_all(&workflows).unwrap();
        std::fs::write(workflows.join("mine.yaml"), "name: mine").unwrap();

        match destination(dir.path(), "mine", None, None) {
            Destination::Taken { suggestion, .. } => {
                assert_eq!(suggestion.as_deref(), Some("mine-2"))
            }
            Destination::Free(n) => panic!("taken name must not be free, got {n}"),
        }

        std::fs::write(workflows.join("mine-2.yaml"), "name: mine-2").unwrap();
        match destination(dir.path(), "mine", None, None) {
            Destination::Taken { suggestion, .. } => {
                assert_eq!(suggestion.as_deref(), Some("mine-3"))
            }
            Destination::Free(n) => panic!("taken name must not be free, got {n}"),
        }
    }

    /// The caller answering a `409` picks the name, and that answer is honoured
    /// — but only if it is actually free. A chosen name that also collides is
    /// still a collision.
    #[test]
    fn a_requested_name_is_honoured_but_still_checked() {
        let dir = TempDir::new().unwrap();
        let workflows = dir.path().join(".harness").join("workflows");
        std::fs::create_dir_all(&workflows).unwrap();
        std::fs::write(workflows.join("mine.yaml"), "name: mine").unwrap();

        assert_eq!(
            free(destination(
                dir.path(),
                "mine",
                Some("mine-ecommerce"),
                None
            ))
            .as_deref(),
            Some("mine-ecommerce")
        );
        assert!(free(destination(dir.path(), "mine", Some("idea-to-pr"), None)).is_none());
    }

    /// An update keeps the name the workflow already has. Choosing a new one
    /// would leave the old file behind as an orphan that still runs — and it
    /// must not be refused as a collision with itself.
    #[test]
    fn an_update_keeps_the_name_it_is_already_installed_under() {
        let dir = TempDir::new().unwrap();
        let workflows = dir.path().join(".harness").join("workflows");
        std::fs::create_dir_all(&workflows).unwrap();
        std::fs::write(workflows.join("geo.yaml"), "name: geo").unwrap();

        assert_eq!(
            free(destination(
                dir.path(),
                "geo-audit-ecommerce",
                None,
                Some("geo")
            ))
            .as_deref(),
            Some("geo")
        );
    }
}
