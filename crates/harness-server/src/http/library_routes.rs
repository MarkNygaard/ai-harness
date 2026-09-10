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

use axum::extract::Path;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::Json;
use harness_runner::authoring;
use serde::Serialize;

use super::runs_routes::RunsState;

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

/// What went wrong, in terms both front doors can render.
///
/// The dialog needs a status code and a structured conflict so it can prompt
/// for a name; an MCP client has only a sentence. Neither can be the other's
/// shape, so the shared code returns the fact and each renders it.
pub(crate) enum LibraryError {
    /// No library configured here.
    Off,
    /// The registry could not be reached, or answered badly.
    Unreachable(String),
    NotFound(String),
    /// The name is taken on this harness. Not a failure — a question.
    Conflict {
        name: String,
        suggestion: Option<String>,
    },
    /// No publisher token on this harness. Not a failure of the request —
    /// the normal state of an install that has never published.
    NoToken,
    Failed(String),
}

impl LibraryError {
    /// A sentence, for callers with nowhere to put structure.
    pub(crate) fn message(&self) -> String {
        match self {
            Self::Off => "the workflow library is switched off on this harness".into(),
            Self::NoToken => "no publisher token is connected on this harness — add one under Settings, Integrations to publish".into(),
            Self::Unreachable(e) | Self::NotFound(e) | Self::Failed(e) => e.clone(),
            Self::Conflict { name, suggestion } => match suggestion {
                Some(s) => format!(
                    "this harness already has a workflow called `{name}` — retry with name `{s}`, \
                     or any other free name"
                ),
                None => format!(
                    "this harness already has a workflow called `{name}`, and so is every \
                     obvious alternative — retry with a name of your own"
                ),
            },
        }
    }

    fn status(&self) -> StatusCode {
        match self {
            Self::Off => StatusCode::NOT_IMPLEMENTED,
            Self::NoToken => StatusCode::FORBIDDEN,
            Self::Unreachable(_) => StatusCode::BAD_GATEWAY,
            Self::NotFound(_) => StatusCode::NOT_FOUND,
            Self::Conflict { .. } => StatusCode::CONFLICT,
            Self::Failed(_) => StatusCode::INTERNAL_SERVER_ERROR,
        }
    }

    fn into_response(self) -> Response {
        // A conflict carries the taken name and a suggestion, so the caller can
        // ask rather than guess. Everything else is a sentence.
        if let Self::Conflict { name, suggestion } = &self {
            return (
                StatusCode::CONFLICT,
                Json(serde_json::json!({
                    "error": self.message(),
                    "conflict": name,
                    "suggested_name": suggestion,
                })),
            )
                .into_response();
        }
        err(self.status(), self.message())
    }
}

/// What an install did.
#[derive(Debug, Serialize)]
pub(crate) struct Installed {
    pub installed_as: String,
    pub version: i32,
    /// The version taken has since been withdrawn by its publisher. Installed
    /// anyway — it is the latest there is — but the caller is told.
    pub withdrawn: bool,
}

/// The library listing, annotated with what is installed here.
///
/// A registry that cannot be reached is an error rather than an empty library:
/// "nothing published yet" and "we could not ask" look identical otherwise, and
/// only one of them is worth retrying.
pub(crate) async fn browse(runs: &Arc<RunsState>) -> Result<Vec<LibraryEntry>, LibraryError> {
    let client = runs.registry().ok_or(LibraryError::Off)?;
    let listing = client
        .list()
        .await
        .map_err(|e| LibraryError::Unreachable(e.to_string()))?;

    // Installed state is a local join. A harness with no database can still
    // browse — it just cannot say what is installed, which is honest rather
    // than wrong.
    let installed = match runs.installed_workflow_store().await {
        Ok(store) => store.all().await.unwrap_or_default(),
        Err(_) => Vec::new(),
    };

    Ok(listing
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
        .collect())
}

/// `GET /api/library`
pub async fn list(
    axum::extract::Extension(runs): axum::extract::Extension<Arc<RunsState>>,
) -> Response {
    match browse(&runs).await {
        Ok(entries) => Json(entries).into_response(),
        Err(e) => e.into_response(),
    }
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

/// Install a library workflow, or move an installed one to the latest version —
/// the same operation either way: fetch a version, write it, record it.
///
/// Shared by the HTTP route and the MCP tool so the two cannot drift. The
/// difference between them is only how a [`LibraryError`] is rendered.
pub(crate) async fn install_workflow(
    runs: &Arc<RunsState>,
    slug: &str,
    requested_name: Option<&str>,
    by: &super::runs_routes::TriggerInfo,
) -> Result<Installed, LibraryError> {
    let client = runs.registry().ok_or(LibraryError::Off)?;
    let listing = client
        .list()
        .await
        .map_err(|e| LibraryError::Unreachable(e.to_string()))?;
    let entry = listing
        .into_iter()
        .find(|w| w.slug == slug)
        .ok_or_else(|| LibraryError::NotFound(format!("the library has no workflow `{slug}`")))?;
    let version = entry.latest_version.ok_or_else(|| {
        LibraryError::Failed(format!("`{slug}` has no published version to install"))
    })?;
    let doc = client
        .version(slug, version)
        .await
        .map_err(|e| LibraryError::Unreachable(e.to_string()))?;

    let store = runs
        .installed_workflow_store()
        .await
        .map_err(LibraryError::Failed)?;
    let existing = store.by_slug(slug).await.ok().flatten();

    let name = match destination(
        &runs.project_root,
        slug,
        requested_name,
        existing.as_ref().map(|i| i.name.as_str()),
    ) {
        Destination::Free(name) => name,
        // A question rather than a workaround: the caller is asked which name
        // to use, and nothing is written until it answers.
        Destination::Taken { name, suggestion } => {
            return Err(LibraryError::Conflict { name, suggestion })
        }
    };

    // Through the same door as hand-authored YAML: validated, and refused if the
    // DAG is broken. A registry that published something unrunnable must not be
    // able to put it on disk here.
    authoring::save_workflow(&runs.project_root, &name, &doc.yaml).map_err(LibraryError::Failed)?;

    store
        .record(&harness_persist::InstallRecord {
            name: &name,
            slug,
            version: doc.version,
            publisher: Some(&entry.publisher),
            title: Some(&entry.title),
            // Installed, not published here: this harness took somebody else's
            // version rather than sending one.
            published: false,
        })
        .await
        .map_err(|e| LibraryError::Failed(e.to_string()))?;

    // Who brought it in, so the editor can say where this workflow came from.
    super::workflows_routes::record_edit_as(
        runs,
        &name,
        by.user_id.as_deref(),
        by.actor.as_deref(),
        super::workflows_routes::EDIT_SOURCE_LIBRARY,
    )
    .await;

    // Best-effort, and last: the workflow is already installed by this point, so
    // a registry that cannot be told must not turn a success into a failure.
    if let Ok(installation_id) = runs.installation_id().await {
        if let Err(e) = client
            .record_install(slug, &installation_id, doc.version)
            .await
        {
            tracing::warn!("library: installed {slug} but could not report it: {e}");
        }
    }

    Ok(Installed {
        installed_as: name,
        version: doc.version,
        withdrawn: doc.withdrawn,
    })
}

/// `POST /api/library/{slug}/install`
///
/// Answers `409` when the name is already in use here, naming the conflict and
/// suggesting a free one. The caller repeats the request with `name` set.
pub async fn install(
    axum::extract::Extension(runs): axum::extract::Extension<Arc<RunsState>>,
    headers: axum::http::HeaderMap,
    Path(slug): Path<String>,
    body: Option<Json<InstallRequest>>,
) -> Response {
    let req = body.map(|Json(b)| b).unwrap_or_default();
    let by = super::runs_routes::TriggerInfo::from_caller(
        &runs,
        &headers,
        super::runs_routes::SOURCE_UI,
    )
    .await;
    match install_workflow(&runs, &slug, req.name.as_deref(), &by).await {
        Ok(done) => Json(done).into_response(),
        Err(e) => e.into_response(),
    }
}

/// Remove an installed workflow. Returns the local name it had.
pub(crate) async fn uninstall_workflow(
    runs: &Arc<RunsState>,
    slug: &str,
) -> Result<String, LibraryError> {
    let store = runs
        .installed_workflow_store()
        .await
        .map_err(LibraryError::Failed)?;
    let installed = store
        .by_slug(slug)
        .await
        .ok()
        .flatten()
        .ok_or_else(|| LibraryError::NotFound(format!("`{slug}` is not installed here")))?;

    authoring::delete_project_workflow(&runs.project_root, &installed.name)
        .map_err(LibraryError::Failed)?;
    forget_installed(runs, &installed.name).await;
    // The workflow's own provenance goes too, or a later workflow reusing the
    // name inherits an author who never saw it.
    if let Ok(authors) = runs.workflow_author_store().await {
        let _ = authors.forget(&installed.name).await;
    }
    Ok(installed.name)
}

/// Forget that a workflow was installed from the library, and stop being
/// counted for it.
///
/// **Called from every path that removes a workflow file**, not only the
/// library's own uninstall: a workflow deleted through the editor or over MCP
/// is just as gone, and leaving the record behind would have the Library dialog
/// still calling it installed — offering an Update for a file that is not there
/// — while the registry kept counting an install that no longer exists.
///
/// A no-op for a workflow that did not come from the library, which is the
/// common case for those callers.
pub(crate) async fn forget_installed(runs: &Arc<RunsState>, name: &str) {
    let Ok(store) = runs.installed_workflow_store().await else {
        return;
    };
    let Ok(Some(installed)) = store.get(name).await else {
        return; // not from the library
    };
    if let Err(e) = store.forget(name).await {
        tracing::warn!("library: could not forget the install of {name}: {e}");
        return;
    }
    // Best-effort, like every other registry call: the file is already gone.
    if let (Some(client), Ok(id)) = (runs.registry(), runs.installation_id().await) {
        if let Err(e) = client.forget_install(&installed.slug, &id).await {
            tracing::warn!("library: removed {name} but could not report it: {e}");
        }
    }
}

/// `DELETE /api/library/{slug}` — remove an installed workflow and stop being
/// counted for it.
pub async fn uninstall(
    axum::extract::Extension(runs): axum::extract::Extension<Arc<RunsState>>,
    Path(slug): Path<String>,
) -> Response {
    match uninstall_workflow(&runs, &slug).await {
        Ok(name) => Json(serde_json::json!({ "uninstalled": name })).into_response(),
        Err(e) => e.into_response(),
    }
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

// ── Publishing ──────────────────────────────────────────────────────────────
//
// Publishing needs a **publisher token**, stored encrypted like any other
// credential and never sent to the browser. The proxy rule the read side
// follows for its own reasons applies with much more force here: a token that
// reached the page could publish under its owner's name from anything that
// could read it.
//
// Whether a publish creates an entry or adds a version is not asked. The local
// record already knows — a workflow this harness published has a row saying so —
// and making the caller choose invites choosing wrong, which is either a
// duplicate slug or a version pushed at somebody else's workflow.

/// Read the publisher token, or say what is missing in a way that names the fix.
async fn publisher_token(runs: &Arc<RunsState>) -> Result<String, LibraryError> {
    let store = runs.cred_store().await.map_err(LibraryError::Failed)?;
    store
        .get("registry")
        .await
        .map_err(|e| LibraryError::Failed(e.to_string()))?
        .and_then(|c| c.get("token").filter(|t| !t.is_empty()).map(String::from))
        .ok_or(LibraryError::NoToken)
}

/// Who this harness publishes as, or `None` when no token is configured.
///
/// Not an error without one: "you have not connected a publisher token" is the
/// normal state of most installs, and the UI shows a different thing for it
/// rather than an error.
pub(crate) async fn publisher(
    runs: &Arc<RunsState>,
) -> Result<Option<crate::registry::Publisher>, LibraryError> {
    let client = runs.registry().ok_or(LibraryError::Off)?;
    let token = match publisher_token(runs).await {
        Ok(t) => t,
        Err(LibraryError::NoToken) => return Ok(None),
        Err(e) => return Err(e),
    };
    client
        .me(&token)
        .await
        .map(Some)
        .map_err(|e| LibraryError::Unreachable(e.to_string()))
}

/// What a publish sends.
#[derive(Debug, serde::Deserialize)]
pub struct PublishRequest {
    /// The local workflow to publish, by file stem.
    pub name: String,
    /// Shown in the library. Defaults to a title derived from the name.
    #[serde(default)]
    pub title: Option<String>,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub tags: Vec<String>,
    /// What changed, for a version after the first.
    #[serde(default)]
    pub changelog: Option<String>,
    /// Rename the publisher before publishing, so the name on the entry is the
    /// one the author expects rather than whatever an operator recorded when
    /// minting their token.
    #[serde(default)]
    pub publish_as: Option<String>,
}

/// What a publish produced.
#[derive(Debug, Serialize)]
pub(crate) struct PublishResult {
    pub slug: String,
    pub version: i32,
    /// The name the entry now carries.
    pub publisher: String,
    /// This was the workflow's first appearance in the library.
    pub created: bool,
}

/// Publish a local workflow, or add a version to one this harness published.
///
/// Shared by the HTTP route and the MCP tool, like the install path.
pub(crate) async fn publish_workflow(
    runs: &Arc<RunsState>,
    req: &PublishRequest,
) -> Result<PublishResult, LibraryError> {
    let client = runs.registry().ok_or(LibraryError::Off)?;
    let token = publisher_token(runs).await?;

    // The YAML is read from disk rather than taken from the request: what gets
    // published must be what this harness actually runs, not what a caller says
    // it is.
    let source = authoring::get_workflow(&runs.project_root, &req.name)
        .map_err(|_| LibraryError::NotFound(format!("no workflow called `{}` here", req.name)))?;

    // A built-in cannot be published. The registry refuses those slugs anyway,
    // but failing here says why in terms of this harness rather than reporting
    // a rejection from a service the author did not know was involved.
    if matches!(source.source, authoring::Source::Bundled) {
        return Err(LibraryError::Failed(format!(
            "`{}` is a built-in workflow — save a copy under your own name and publish that",
            req.name
        )));
    }

    if let Some(name) = req
        .publish_as
        .as_deref()
        .map(str::trim)
        .filter(|n| !n.is_empty())
    {
        client
            .set_display_name(&token, name)
            .await
            .map_err(|e| LibraryError::Failed(e.to_string()))?;
    }

    let store = runs
        .installed_workflow_store()
        .await
        .map_err(LibraryError::Failed)?;
    let existing = store.get(&req.name).await.ok().flatten();

    // Publish a version only for a workflow this harness published. A row that
    // came from an *install* names somebody else's slug, and pushing a version
    // at it would be publishing into their entry — which the registry refuses,
    // but which should never be attempted.
    let own = existing.as_ref().filter(|r| r.published);

    let published = match own {
        Some(record) => client
            .publish_version(&token, &record.slug, &source.yaml, req.changelog.as_deref())
            .await
            .map_err(|e| LibraryError::Failed(e.to_string()))?,
        None => {
            let title = req
                .title
                .as_deref()
                .map(str::trim)
                .filter(|t| !t.is_empty())
                .unwrap_or(&req.name);
            let description = req.description.as_deref().unwrap_or_default();
            client
                .create(
                    &token,
                    &crate::registry::NewWorkflow {
                        slug: &req.name,
                        title,
                        description,
                        tags: &req.tags,
                        yaml: &source.yaml,
                        changelog: req.changelog.as_deref(),
                    },
                )
                .await
                .map_err(|e| LibraryError::Failed(e.to_string()))?
        }
    };

    let who = client
        .me(&token)
        .await
        .map(|p| p.name().to_string())
        .unwrap_or_default();

    // Record the link locally, so the next publish adds a version instead of
    // trying to create the entry again — and so the editor can say this one is
    // published without asking the registry.
    store
        .record(&harness_persist::InstallRecord {
            name: &req.name,
            slug: &published.slug,
            version: published.version,
            publisher: Some(&who),
            title: req.title.as_deref(),
            published: true,
        })
        .await
        .map_err(|e| LibraryError::Failed(e.to_string()))?;

    Ok(PublishResult {
        slug: published.slug,
        version: published.version,
        publisher: who,
        created: own.is_none(),
    })
}

pub async fn publish(
    axum::extract::Extension(runs): axum::extract::Extension<Arc<RunsState>>,
    Json(req): Json<PublishRequest>,
) -> Response {
    match publish_workflow(&runs, &req).await {
        Ok(done) => Json(done).into_response(),
        Err(e) => e.into_response(),
    }
}

pub async fn who_publishes(
    axum::extract::Extension(runs): axum::extract::Extension<Arc<RunsState>>,
) -> Response {
    match publisher(&runs).await {
        Ok(p) => Json(serde_json::json!({
            "configured": p.is_some(),
            "name": p.as_ref().map(|p| p.name()),
            "login": p.as_ref().map(|p| p.github_login.clone()),
        }))
        .into_response(),
        Err(e) => e.into_response(),
    }
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
