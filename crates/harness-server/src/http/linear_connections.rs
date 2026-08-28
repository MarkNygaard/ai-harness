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

use std::collections::HashMap;
use std::sync::Arc;

use harness_persist::CredentialStore;

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

#[cfg(test)]
mod tests {
    use super::*;

    fn ids(raw: &[&str]) -> Vec<ConnectionId> {
        raw.iter()
            .map(|s| ConnectionId::parse(s).unwrap())
            .collect()
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
