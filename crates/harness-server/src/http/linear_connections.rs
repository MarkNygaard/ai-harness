//! **Named Linear connections** — one per installed Linear workspace.
//!
//! The harness used to hold exactly one Linear credential, and every project
//! used it. That breaks as soon as two Linear accounts are in play: projects
//! belonging to account B need B's token, B's app user id and B's webhook
//! secret, not A's.
//!
//! A **connection** is one installed workspace, identified by a short slug. Its
//! secrets live in the existing encrypted credential store under a per-connection
//! provider key — `linear:<id>`, with the bare `linear` row from before this
//! existed reading as the id [`ConnectionId::DEFAULT`]. Nothing is re-encrypted
//! or migrated: a single-account install keeps the row it already has.
//!
//! A **project** points at one connection via `harness_projects.linear_connection`.
//! Unpinned (`NULL`) is the normal state for a single-account install — see
//! [`resolve_for_project`] for how that resolves.

use std::collections::{BTreeMap, HashMap};
use std::sync::Arc;

use axum::extract::{Extension, Path as AxumPath};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::Json;
use harness_persist::CredentialStore;
use serde::Deserialize;

use super::linear_oauth::{describe, revoke_for, ConnectionSummary};
use super::runs_routes::RunsState;

/// Provider key of the pre-multi-connection Linear credential. Read as
/// [`ConnectionId::DEFAULT`] so existing installs need no migration.
const LEGACY_PROVIDER: &str = "linear";

/// Prefix for every other connection's provider key.
const PROVIDER_PREFIX: &str = "linear:";

/// Longest accepted connection id.
const MAX_ID_LEN: usize = 32;

/// A connection's stable id: a short slug, unique across connections.
///
/// Ids are lowercase `[a-z0-9-]`, start with an alphanumeric, and are at most
/// [`MAX_ID_LEN`] characters — they appear in provider keys, query strings and
/// the `harness_projects.linear_connection` column.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub(crate) struct ConnectionId(String);

impl ConnectionId {
    /// Id of the connection backed by the legacy bare `linear` credential — the
    /// one a single-account install already has.
    pub(crate) const DEFAULT: &'static str = "default";

    /// Validate and wrap a connection id.
    ///
    /// [`Self::DEFAULT`] is accepted: it is addressable like any other id (a
    /// `?connection=default` query, a pinned project). What makes it special is
    /// only that it maps to the legacy provider key.
    pub(crate) fn parse(raw: &str) -> Result<Self, String> {
        let id = raw.trim();
        if id.is_empty() {
            return Err("connection id is empty".to_string());
        }
        if id.len() > MAX_ID_LEN {
            return Err(format!(
                "connection id `{id}` is longer than {MAX_ID_LEN} characters"
            ));
        }
        if !id.starts_with(|c: char| c.is_ascii_lowercase() || c.is_ascii_digit()) {
            return Err(format!(
                "connection id `{id}` must start with a lowercase letter or digit"
            ));
        }
        if !id
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
        {
            return Err(format!(
                "connection id `{id}` may only contain lowercase letters, digits and `-`"
            ));
        }
        Ok(Self(id.to_string()))
    }

    pub(crate) fn as_str(&self) -> &str {
        &self.0
    }

    pub(crate) fn is_default(&self) -> bool {
        self.0 == Self::DEFAULT
    }

    /// Where this connection's secrets are stored in [`CredentialStore`].
    pub(crate) fn provider_key(&self) -> String {
        if self.is_default() {
            LEGACY_PROVIDER.to_string()
        } else {
            format!("{PROVIDER_PREFIX}{}", self.0)
        }
    }

    /// Inverse of [`Self::provider_key`]. `None` for providers that aren't
    /// Linear connections at all (`claude`, `github`, …).
    ///
    /// `linear:default` is rejected rather than folded into the default: it
    /// would be a second spelling of the legacy key, and two provider keys for
    /// one connection is how a credential goes missing.
    pub(crate) fn from_provider_key(provider: &str) -> Option<Self> {
        if provider == LEGACY_PROVIDER {
            return Some(Self(Self::DEFAULT.to_string()));
        }
        let id = provider.strip_prefix(PROVIDER_PREFIX)?;
        if id == Self::DEFAULT {
            return None;
        }
        Self::parse(id).ok()
    }
}

impl Default for ConnectionId {
    fn default() -> Self {
        Self(Self::DEFAULT.to_string())
    }
}

impl std::fmt::Display for ConnectionId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

/// Every configured connection, sorted by id.
///
/// "Configured" means a credential row exists — not that the workspace is
/// connected. A row holding only a half-entered OAuth client still counts, so
/// resolution keeps pointing at it and the caller reports the same "not
/// connected" error it reports today.
pub(crate) async fn list_ids(store: &CredentialStore) -> Result<Vec<ConnectionId>, String> {
    let providers = store.list_configured().await.map_err(|e| e.to_string())?;
    let mut ids: Vec<ConnectionId> = providers
        .iter()
        .filter_map(|p| ConnectionId::from_provider_key(p))
        .collect();
    ids.sort();
    ids.dedup();
    Ok(ids)
}

/// Which connection a project's Linear traffic belongs to.
///
/// See [`choose`] for the rule. Resolution is deliberately forgiving — the only
/// failure is genuine ambiguity, because every other case has a better answer
/// than refusing to work.
pub(crate) async fn resolve_for_project(
    state: &Arc<RunsState>,
    project: &str,
) -> Result<ConnectionId, String> {
    let store = state.cred_store().await?;
    let available = list_ids(store).await?;
    choose(
        pinned_for_project(state, project).await.as_deref(),
        &available,
    )
}

/// Resolve many projects in one pass, reusing a single connection listing.
///
/// The webhook path needs this: attributing a delivery means filtering every
/// binding down to the ones whose project belongs to the sending workspace, and
/// calling [`resolve_for_project`] per binding would re-list the connections
/// each time. Projects whose resolution is ambiguous are simply absent from the
/// map, which reads as "not this connection" at every call site.
pub(crate) async fn resolve_for_projects(
    state: &Arc<RunsState>,
    projects: &[&str],
) -> HashMap<String, ConnectionId> {
    let Ok(store) = state.cred_store().await else {
        return HashMap::new();
    };
    let Ok(available) = list_ids(store).await else {
        return HashMap::new();
    };
    let mut out: HashMap<String, ConnectionId> = HashMap::new();
    for project in projects {
        let project = *project;
        if out.contains_key(project) {
            continue;
        }
        let pinned = pinned_for_project(state, project).await;
        if let Ok(conn) = choose(pinned.as_deref(), &available) {
            out.insert(project.to_string(), conn);
        }
    }
    out
}

/// The connection a project is pinned to, if any.
///
/// Best-effort: an unreachable project store or an unregistered project reads as
/// "not pinned" rather than an error, so resolution falls through to the
/// single-connection rule instead of breaking a run.
async fn pinned_for_project(state: &Arc<RunsState>, project: &str) -> Option<String> {
    let store = state.project_store().await.ok()?;
    let row = store.get(project).await.ok()??;
    row.linear_connection.filter(|c| !c.trim().is_empty())
}

/// The resolution rule, split out so it is testable without a database.
///
/// 1. Pinned to a connection that exists → that one.
/// 2. Otherwise, exactly one connection configured → that one. This is what
///    keeps a single-account install zero-config: no project is ever pinned and
///    everything routes to the only workspace there is.
/// 3. Otherwise → ambiguous, and refused.
///
/// Two cases deliberately fall through to rule 2 rather than failing. **Nothing
/// configured** resolves to [`ConnectionId::DEFAULT`] so the caller produces its
/// existing "Linear is not connected" message instead of a second, vaguer one
/// about connections. **Pinned to a connection that no longer exists** resolves
/// like an unpinned project, so deleting a connection degrades to the
/// single-account behaviour rather than hard-failing every project that named it.
fn choose(pinned: Option<&str>, available: &[ConnectionId]) -> Result<ConnectionId, String> {
    if let Some(pin) = pinned {
        if let Some(found) = available.iter().find(|c| c.as_str() == pin) {
            return Ok(found.clone());
        }
    }
    match available {
        [] => Ok(ConnectionId::default()),
        [only] => Ok(only.clone()),
        many => {
            let names: Vec<&str> = many.iter().map(ConnectionId::as_str).collect();
            Err(format!(
                "several Linear connections are configured ({}) but this project is not \
                 assigned to one — pick its Linear account on the Projects page",
                names.join(", ")
            ))
        }
    }
}

// ── Managing connections ─────────────────────────────────────────────────────
//
// - `GET    /api/linear/connections`        — every connection + what uses it
// - `POST   /api/linear/connections`        — add one, then connect it via OAuth
// - `DELETE /api/linear/connections/{id}`   — revoke and remove
// - `PUT    /api/projects/{project}/linear-connection` — pin a project

fn err(status: StatusCode, msg: impl Into<String>) -> Response {
    (status, Json(serde_json::json!({ "error": msg.into() }))).into_response()
}

/// Turn a human label into a connection id: lowercase, non-alphanumerics folded
/// to single dashes, trimmed to [`MAX_ID_LEN`].
fn slugify(label: &str) -> String {
    let mut out = String::with_capacity(label.len());
    for c in label.chars() {
        if c.is_ascii_alphanumeric() {
            out.push(c.to_ascii_lowercase());
        } else if !out.ends_with('-') {
            out.push('-');
        }
    }
    let out = out.trim_matches('-');
    // Truncating can re-expose a trailing dash, and a leading digit is fine but a
    // leading dash is not.
    out.chars()
        .take(MAX_ID_LEN)
        .collect::<String>()
        .trim_matches('-')
        .to_string()
}

/// The slug for `label`, suffixed until it doesn't collide with `taken`.
fn unique_id(label: &str, taken: &[ConnectionId]) -> Result<ConnectionId, String> {
    let base = slugify(label);
    if base.is_empty() {
        return Err(format!(
            "`{label}` has no letters or digits to make a name from"
        ));
    }
    let is_taken = |c: &str| taken.iter().any(|t| t.as_str() == c);
    if !is_taken(&base) {
        return ConnectionId::parse(&base);
    }
    for n in 2..100 {
        // Keep room for the suffix rather than overflowing the length cap.
        let suffix = format!("-{n}");
        let head: String = base.chars().take(MAX_ID_LEN - suffix.len()).collect();
        let candidate = format!("{}{suffix}", head.trim_end_matches('-'));
        if !is_taken(&candidate) {
            return ConnectionId::parse(&candidate);
        }
    }
    Err(format!("too many connections named like `{base}`"))
}

/// Every connection, with the projects pinned to each.
async fn summaries(state: &Arc<RunsState>) -> Result<Vec<ConnectionSummary>, String> {
    let store = state.cred_store().await?;
    let mut out = Vec::new();
    for id in list_ids(store).await? {
        let mut summary = describe(store, &id).await;
        if let Ok(projects) = state.project_store().await {
            summary.projects = projects
                .names_using_linear_connection(id.as_str())
                .await
                .unwrap_or_default();
        }
        out.push(summary);
    }
    Ok(out)
}

/// `GET /api/linear/connections` — every connected (or half-configured) Linear
/// account, and which projects use each.
pub async fn list_connections(Extension(state): Extension<Arc<RunsState>>) -> Response {
    match summaries(&state).await {
        Ok(list) => Json(list).into_response(),
        Err(e) => err(StatusCode::SERVICE_UNAVAILABLE, e),
    }
}

#[derive(Debug, Deserialize)]
pub struct CreateConnectionRequest {
    /// What to call this account, e.g. "Acme". The id is derived from it.
    pub label: String,
}

/// `POST /api/linear/connections` — add an account, ready to be connected.
///
/// This only creates the row. The operator then saves the OAuth app's client id
/// and secret against it and runs the connect flow with `?connection=<id>`.
pub async fn create_connection(
    Extension(state): Extension<Arc<RunsState>>,
    Json(req): Json<CreateConnectionRequest>,
) -> Response {
    let label = req.label.trim();
    if label.is_empty() {
        return err(StatusCode::BAD_REQUEST, "a name is required");
    }
    let store = match state.cred_store().await {
        Ok(s) => s,
        Err(e) => return err(StatusCode::SERVICE_UNAVAILABLE, e),
    };
    let existing = match list_ids(store).await {
        Ok(e) => e,
        Err(e) => return err(StatusCode::INTERNAL_SERVER_ERROR, e),
    };
    let id = match unique_id(label, &existing) {
        Ok(id) => id,
        Err(e) => return err(StatusCode::BAD_REQUEST, e),
    };

    // Creating the row is what makes the connection exist — `list_ids` reads the
    // credential store, so a connection with no credential row is not a thing.
    let fields = BTreeMap::from([("label".to_string(), label.to_string())]);
    if let Err(e) = store.set(&id.provider_key(), &fields).await {
        return err(StatusCode::INTERNAL_SERVER_ERROR, e.to_string());
    }

    // Going from one connection to two is the moment unpinned projects stop
    // resolving: rule 2 only applies while there is exactly one. Pin them all to
    // the account they were already using, so adding a second changes nothing
    // for the projects already running and the operator only has to move the
    // ones that belong to the new account.
    if existing.len() == 1 {
        match state.project_store().await {
            Ok(projects) => match projects
                .backfill_linear_connection(existing[0].as_str())
                .await
            {
                Ok(n) if n > 0 => tracing::info!(
                    "linear: pinned {n} project(s) to `{}` on adding a second connection",
                    existing[0]
                ),
                Ok(_) => {}
                Err(e) => tracing::warn!("linear: could not pin existing projects: {e}"),
            },
            Err(e) => tracing::warn!("linear: could not pin existing projects: {e}"),
        }
    }

    (StatusCode::CREATED, Json(describe(store, &id).await)).into_response()
}

/// `DELETE /api/linear/connections/{id}` — revoke the token and remove it.
///
/// Refused while any project is pinned to it: without the guard those projects
/// would silently fall back to whichever connection is left, which is how issues
/// end up being worked in the wrong account.
pub async fn delete_connection(
    Extension(state): Extension<Arc<RunsState>>,
    AxumPath(id): AxumPath<String>,
) -> Response {
    let id = match ConnectionId::parse(&id) {
        Ok(id) => id,
        Err(e) => return err(StatusCode::BAD_REQUEST, e),
    };
    let store = match state.cred_store().await {
        Ok(s) => s,
        Err(e) => return err(StatusCode::SERVICE_UNAVAILABLE, e),
    };
    if let Ok(projects) = state.project_store().await {
        match projects.names_using_linear_connection(id.as_str()).await {
            Ok(using) if !using.is_empty() => {
                return err(
                    StatusCode::CONFLICT,
                    format!(
                        "`{id}` is still used by {} — point them at another Linear account first",
                        using.join(", ")
                    ),
                );
            }
            Ok(_) => {}
            Err(e) => return err(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()),
        }
    }
    revoke_for(store, &id).await;
    match store.delete(&id.provider_key()).await {
        Ok(()) => {
            Json(serde_json::json!({ "deleted": true, "connection": id.as_str() })).into_response()
        }
        Err(e) => err(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()),
    }
}

#[derive(Debug, Deserialize)]
pub struct PinProjectRequest {
    /// The connection to use, or `null` to resolve automatically (only
    /// unambiguous while exactly one connection is configured).
    pub connection: Option<String>,
}

/// `PUT /api/projects/{project}/linear-connection` — which Linear account this
/// project's issues come from.
pub async fn set_project_connection(
    Extension(state): Extension<Arc<RunsState>>,
    AxumPath(project): AxumPath<String>,
    Json(req): Json<PinProjectRequest>,
) -> Response {
    let requested = match req.connection.as_deref().map(str::trim) {
        None | Some("") => None,
        Some(raw) => match ConnectionId::parse(raw) {
            Ok(id) => Some(id),
            Err(e) => return err(StatusCode::BAD_REQUEST, e),
        },
    };
    let store = match state.cred_store().await {
        Ok(s) => s,
        Err(e) => return err(StatusCode::SERVICE_UNAVAILABLE, e),
    };
    // Pinning to a connection that doesn't exist would read as unpinned, which
    // is not what the operator asked for.
    if let Some(id) = &requested {
        match list_ids(store).await {
            Ok(available) if available.contains(id) => {}
            Ok(_) => {
                return err(
                    StatusCode::BAD_REQUEST,
                    format!("there is no Linear connection called `{id}`"),
                )
            }
            Err(e) => return err(StatusCode::INTERNAL_SERVER_ERROR, e),
        }
    }
    let projects = match state.project_store().await {
        Ok(p) => p,
        Err(e) => return err(StatusCode::SERVICE_UNAVAILABLE, e),
    };
    match projects
        .set_linear_connection(&project, requested.as_ref().map(ConnectionId::as_str))
        .await
    {
        Ok(Some(p)) => Json(p).into_response(),
        Ok(None) => err(
            StatusCode::NOT_FOUND,
            format!("project `{project}` not found"),
        ),
        Err(e) => err(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ids(raw: &[&str]) -> Vec<ConnectionId> {
        raw.iter()
            .map(|s| ConnectionId::parse(s).unwrap())
            .collect()
    }

    #[test]
    fn labels_become_ids() {
        assert_eq!(slugify("Acme"), "acme");
        assert_eq!(slugify("Acme Corp"), "acme-corp");
        assert_eq!(slugify("  Dilling (EU)  "), "dilling-eu");
        assert_eq!(slugify("A—B"), "a-b");
        assert_eq!(slugify("2024 Team"), "2024-team");
        // Nothing to make a name from.
        assert_eq!(slugify("!!!"), "");
        assert_eq!(slugify(""), "");
        // Truncation must not leave a trailing dash behind.
        let long = slugify(&format!("{} tail", "a".repeat(MAX_ID_LEN - 1)));
        assert!(long.len() <= MAX_ID_LEN, "{long}");
        assert!(!long.ends_with('-'), "{long}");
        // Whatever comes out has to be a valid id.
        for label in ["Acme", "Acme Corp", "  Dilling (EU)  ", "2024 Team"] {
            assert!(ConnectionId::parse(&slugify(label)).is_ok(), "{label}");
        }
    }

    #[test]
    fn ids_are_suffixed_until_they_stop_colliding() {
        let taken = ids(&["acme"]);
        assert_eq!(unique_id("Acme", &taken).unwrap().as_str(), "acme-2");

        let taken = ids(&["acme", "acme-2"]);
        assert_eq!(unique_id("Acme", &taken).unwrap().as_str(), "acme-3");

        // A free name is used as-is.
        assert_eq!(unique_id("Acme", &[]).unwrap().as_str(), "acme");

        // `default` is only taken once the legacy connection exists, and then a
        // label that slugs to it steps aside rather than colliding with it.
        assert_eq!(unique_id("Default", &[]).unwrap().as_str(), "default");
        let taken = ids(&["default"]);
        assert_eq!(unique_id("Default", &taken).unwrap().as_str(), "default-2");
    }

    #[test]
    fn a_label_with_nothing_to_name_it_by_is_refused() {
        let e = unique_id("!!!", &[]).unwrap_err();
        assert!(e.contains("!!!"), "{e}");
    }

    #[test]
    fn provider_key_round_trips_including_the_legacy_row() {
        // The pre-multi-connection credential is the default connection. This is
        // the whole migration: an existing install keeps the row it has.
        let default = ConnectionId::default();
        assert_eq!(default.as_str(), "default");
        assert_eq!(default.provider_key(), "linear");
        assert_eq!(
            ConnectionId::from_provider_key("linear"),
            Some(ConnectionId::default())
        );

        let acme = ConnectionId::parse("acme").unwrap();
        assert_eq!(acme.provider_key(), "linear:acme");
        assert_eq!(ConnectionId::from_provider_key("linear:acme"), Some(acme));

        // Other providers are not connections.
        for other in ["claude", "codex", "github", "cursor", "env", "kimi-code"] {
            assert_eq!(ConnectionId::from_provider_key(other), None, "{other}");
        }
        // A second spelling of the legacy key would split one connection's
        // secrets across two rows.
        assert_eq!(ConnectionId::from_provider_key("linear:default"), None);
        // Prefix-but-invalid never becomes a connection.
        assert_eq!(ConnectionId::from_provider_key("linear:Acme"), None);
        assert_eq!(ConnectionId::from_provider_key("linear:"), None);
        assert_eq!(ConnectionId::from_provider_key("linearly"), None);
    }

    #[test]
    fn parse_accepts_slugs_and_rejects_everything_else() {
        for ok in ["default", "acme", "acme-2", "a", "0", "dilling-eu"] {
            assert!(ConnectionId::parse(ok).is_ok(), "{ok} should parse");
        }
        assert_eq!(ConnectionId::parse("  acme  ").unwrap().as_str(), "acme");

        for bad in [
            "",
            "   ",
            "-acme",
            "Acme",
            "acme_2",
            "acme.2",
            "acme workspace",
            "acme:2",
            "acmé",
        ] {
            assert!(
                ConnectionId::parse(bad).is_err(),
                "{bad:?} should not parse"
            );
        }
        assert!(ConnectionId::parse(&"a".repeat(MAX_ID_LEN)).is_ok());
        assert!(ConnectionId::parse(&"a".repeat(MAX_ID_LEN + 1)).is_err());
    }

    #[test]
    fn nothing_configured_resolves_to_default() {
        // So the caller reports its existing "Linear is not connected" error
        // rather than a second, vaguer one about connections.
        assert_eq!(choose(None, &[]).unwrap(), ConnectionId::default());
        assert_eq!(choose(Some("acme"), &[]).unwrap(), ConnectionId::default());
    }

    #[test]
    fn one_connection_wins_whether_or_not_the_project_is_pinned() {
        // The single-account install: nothing is ever pinned, everything routes
        // to the only workspace there is.
        let one = ids(&["default"]);
        assert_eq!(choose(None, &one).unwrap().as_str(), "default");

        // Same when the sole connection is not the legacy one.
        let one = ids(&["acme"]);
        assert_eq!(choose(None, &one).unwrap().as_str(), "acme");
        assert_eq!(choose(Some("acme"), &one).unwrap().as_str(), "acme");
    }

    #[test]
    fn a_pin_selects_among_several() {
        let many = ids(&["acme", "default"]);
        assert_eq!(choose(Some("acme"), &many).unwrap().as_str(), "acme");
        assert_eq!(choose(Some("default"), &many).unwrap().as_str(), "default");
    }

    #[test]
    fn several_connections_and_no_pin_is_refused() {
        let many = ids(&["acme", "default"]);
        let e = choose(None, &many).unwrap_err();
        // The message has to name the candidates — that's what makes it fixable.
        assert!(e.contains("acme") && e.contains("default"), "{e}");
    }

    #[test]
    fn a_pin_to_a_deleted_connection_degrades_instead_of_failing() {
        // One connection left: behave as if unpinned rather than break the project.
        let one = ids(&["acme"]);
        assert_eq!(choose(Some("gone"), &one).unwrap().as_str(), "acme");
        // Several left: ambiguous, and the operator has to choose again.
        let many = ids(&["acme", "default"]);
        assert!(choose(Some("gone"), &many).is_err());
    }
}
