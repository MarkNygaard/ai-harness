//! **Which projects a signed-in person may see.**
//!
//! By default, every one. An administrator can restrict an individual member to
//! a named set instead, and from then on that member sees only those projects —
//! their runs, their reports, their credentials — and can only start work on
//! them.
//!
//! **Empty means unrestricted, not "nothing".** The table stores only the
//! restrictions that exist, so an install that never touches this feature has
//! no rows and behaves exactly as it did. The cost is that an empty set is the
//! *widest* state rather than the narrowest, which is the opposite of how these
//! things usually read — hence [`ProjectScope`] as a two-case enum rather than a
//! bare `Vec<String>` that a later caller could mistake for a deny-list.
//!
//! **Administrators are never restricted.** They manage projects, credentials
//! and accounts, including this setting, so a scope on an administrator would
//! be a lock whose key is in the same drawer. Setting one is refused rather than
//! stored-and-ignored, so nobody believes they have restricted someone they
//! have not.
//!
//! **The allowed set comes from the session, never from the request** — the same
//! property [`super::run_grants`] relies on. A handler may read `?project=` to
//! narrow what it returns, but narrowing is all it can do: the result is
//! intersected with this scope, so naming someone else's project returns
//! nothing rather than their data.

use std::collections::BTreeSet;
use std::sync::Arc;

use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};

use super::accounts::{authenticated_user, mode, Mode, ROLE_ADMIN};
use super::runs_routes::RunsState;

/// What a caller is allowed to look at.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum ProjectScope {
    /// Every project, including runs that belong to none. Administrators, the
    /// `open`/`token` modes, and any member nobody has restricted.
    All,
    /// Only these projects. Always non-empty: an empty restriction is not a
    /// thing that can be stored, because it would be indistinguishable from no
    /// restriction at all.
    Only(BTreeSet<String>),
    /// Nothing at all. Not a state anyone can configure — it is what this
    /// module returns when it cannot find out what someone is allowed to see.
    /// Failing open on a database blip would hand out every project, so the
    /// unknown answer is the narrow one.
    Nothing,
}

/// For [`ProjectScope::allowed`] on [`ProjectScope::Nothing`]: a filter that
/// matches no project, so a caller building SQL from it returns no rows.
static NO_PROJECTS: BTreeSet<String> = BTreeSet::new();

impl ProjectScope {
    /// Build a scope from what the store returned. An empty list is [`All`] —
    /// the one place that conversion is allowed to happen.
    ///
    /// [`All`]: ProjectScope::All
    pub(crate) fn from_rows(projects: Vec<String>) -> Self {
        let set: BTreeSet<String> = projects
            .into_iter()
            .map(|p| p.trim().to_string())
            .filter(|p| !p.is_empty())
            .collect();
        if set.is_empty() {
            Self::All
        } else {
            Self::Only(set)
        }
    }

    /// Whether this caller may see `project`.
    ///
    /// `None` is a run with no project. Only an unrestricted caller sees those:
    /// a restricted member was given a list, and a run that is on no list is
    /// not on theirs.
    pub(crate) fn allows(&self, project: Option<&str>) -> bool {
        match self {
            Self::All => true,
            Self::Nothing => false,
            Self::Only(allowed) => match project.map(str::trim).filter(|p| !p.is_empty()) {
                Some(p) => allowed.contains(p),
                None => false,
            },
        }
    }

    /// The allowed names, or `None` when unrestricted. For building a SQL
    /// filter; prefer [`allows`](Self::allows) for a single check.
    pub(crate) fn allowed(&self) -> Option<&BTreeSet<String>> {
        match self {
            Self::All => None,
            Self::Only(allowed) => Some(allowed),
            Self::Nothing => Some(&NO_PROJECTS),
        }
    }

    pub(crate) fn is_restricted(&self) -> bool {
        !matches!(self, Self::All)
    }

    /// Narrow a requested project to what the caller may actually see.
    ///
    /// Returns `Err` when they asked for one they may not have — a refusal
    /// rather than a silent empty list, so a mistyped project name does not
    /// read as "that project has no runs".
    pub(crate) fn narrow<'a>(&self, requested: Option<&'a str>) -> Result<Option<&'a str>, ()> {
        match requested {
            Some(p) if !self.allows(Some(p)) => Err(()),
            other => Ok(other),
        }
    }
}

/// The signed-in caller's scope.
///
/// Unrestricted outside `accounts` mode, where there are no accounts to
/// restrict, and unrestricted for anyone this harness cannot identify — those
/// requests are already refused by the global middleware, and inventing a
/// narrow scope for them here would only make a failure look like an empty
/// dashboard.
pub(crate) async fn caller_scope(state: &Arc<RunsState>, headers: &HeaderMap) -> ProjectScope {
    if mode(state).await != Mode::Accounts {
        return ProjectScope::All;
    }
    let Some(user) = authenticated_user(state, headers).await else {
        return ProjectScope::All;
    };
    if user.role == ROLE_ADMIN {
        return ProjectScope::All;
    }
    let Ok(users) = state.user_store().await else {
        tracing::warn!("project scope: no user store; {} sees nothing", user.id);
        return ProjectScope::Nothing;
    };
    match users.project_scope(&user.id).await {
        Ok(rows) => ProjectScope::from_rows(rows),
        Err(e) => {
            tracing::warn!("project scope lookup failed for {}: {e}", user.id);
            ProjectScope::Nothing
        }
    }
}

/// [`caller_scope`] as a route argument, so a handler cannot forget to ask —
/// the reasoning behind [`AdminOnly`](super::accounts::AdminOnly), applied to
/// the other half of the question.
pub(crate) struct CallerScope(pub(crate) ProjectScope);

impl<S> axum::extract::FromRequestParts<S> for CallerScope
where
    S: Send + Sync,
{
    type Rejection = Response;

    async fn from_request_parts(
        parts: &mut axum::http::request::Parts,
        _state: &S,
    ) -> Result<Self, Self::Rejection> {
        let Some(state) = parts.extensions.get::<Arc<RunsState>>().cloned() else {
            return Err((
                StatusCode::INTERNAL_SERVER_ERROR,
                axum::Json(serde_json::json!({ "error": "server state is unavailable" })),
            )
                .into_response());
        };
        Ok(CallerScope(caller_scope(&state, &parts.headers).await))
    }
}

/// Guard a handler that works on one run: may this caller touch it?
///
/// Returns the refusal to hand straight back, or `Ok(())` to carry on. A run
/// that does not exist and one the caller may not see get the same answer, for
/// the reason in [`not_in_scope`] — otherwise run ids could be probed one at a
/// time for which projects exist.
///
/// Skips the lookup entirely for an unrestricted caller, which is every caller
/// on an install that does not use this feature.
pub(crate) async fn run_in_scope(
    state: &Arc<RunsState>,
    scope: &ProjectScope,
    run_id: &str,
) -> Result<(), Response> {
    if !scope.is_restricted() {
        return Ok(());
    }
    let store = state.store().await.map_err(|e| {
        (
            StatusCode::SERVICE_UNAVAILABLE,
            axum::Json(serde_json::json!({ "error": e })),
        )
            .into_response()
    })?;
    match store.project_for_run(run_id).await {
        Ok(Some(project)) if scope.allows(project.as_deref()) => Ok(()),
        // No such run, or not theirs — indistinguishable on purpose, and the
        // same wording a genuinely missing run gets from its own handler.
        Ok(_) => Err((
            StatusCode::NOT_FOUND,
            axum::Json(serde_json::json!({ "error": format!("run `{run_id}` not found") })),
        )
            .into_response()),
        Err(e) => Err((
            StatusCode::INTERNAL_SERVER_ERROR,
            axum::Json(serde_json::json!({ "error": e.to_string() })),
        )
            .into_response()),
    }
}

/// The refusal a scoped caller gets for a project that is not theirs.
///
/// Deliberately the same answer as a project that does not exist. Telling
/// someone a name they may not use is a real name would leak the shape of the
/// install one guess at a time.
pub(crate) fn not_in_scope() -> Response {
    (
        StatusCode::NOT_FOUND,
        axum::Json(serde_json::json!({ "error": "no such project" })),
    )
        .into_response()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn only(names: &[&str]) -> ProjectScope {
        ProjectScope::Only(names.iter().map(|s| s.to_string()).collect())
    }

    #[test]
    fn no_rows_means_every_project() {
        assert_eq!(ProjectScope::from_rows(vec![]), ProjectScope::All);
        // Blank rows are not a restriction either — they name no project.
        assert_eq!(
            ProjectScope::from_rows(vec!["  ".into(), String::new()]),
            ProjectScope::All
        );
    }

    #[test]
    fn rows_restrict_to_exactly_those_projects() {
        let scope = ProjectScope::from_rows(vec!["niles".into(), " shop ".into()]);
        assert!(scope.allows(Some("niles")));
        assert!(
            scope.allows(Some("shop")),
            "names are trimmed on the way in"
        );
        assert!(!scope.allows(Some("other")));
        assert!(scope.is_restricted());
    }

    #[test]
    fn an_unrestricted_caller_sees_runs_with_no_project() {
        assert!(ProjectScope::All.allows(None));
    }

    #[test]
    fn a_restricted_caller_does_not_see_runs_with_no_project() {
        // A run on no project is on nobody's list.
        assert!(!only(&["niles"]).allows(None));
        assert!(!only(&["niles"]).allows(Some("")));
        assert!(!only(&["niles"]).allows(Some("   ")));
    }

    #[test]
    fn narrow_passes_through_a_project_the_caller_may_see() {
        assert_eq!(only(&["niles"]).narrow(Some("niles")), Ok(Some("niles")));
        assert_eq!(
            ProjectScope::All.narrow(Some("anything")),
            Ok(Some("anything"))
        );
        assert_eq!(ProjectScope::All.narrow(None), Ok(None));
    }

    #[test]
    fn narrow_refuses_a_project_the_caller_may_not_see() {
        // Not an empty result — that would read as "this project has no runs".
        assert_eq!(only(&["niles"]).narrow(Some("secret")), Err(()));
    }

    #[test]
    fn the_unknown_answer_is_the_narrow_one() {
        // A database blip must not widen anyone's access.
        let scope = ProjectScope::Nothing;
        assert!(!scope.allows(Some("niles")));
        assert!(!scope.allows(None));
        assert!(scope.is_restricted());
        assert_eq!(scope.narrow(Some("niles")), Err(()));
        assert!(
            scope.allowed().is_some_and(|a| a.is_empty()),
            "a SQL filter built from this matches nothing"
        );
    }

    #[test]
    fn an_unscoped_request_from_a_restricted_caller_stays_unscoped_here() {
        // `narrow` only validates what was asked for; listing handlers still
        // have to apply `allowed()` to the query itself.
        assert_eq!(only(&["niles"]).narrow(None), Ok(None));
        assert_eq!(
            only(&["niles"]).allowed().map(|a| a.len()),
            Some(1),
            "the filter the caller must apply"
        );
    }
}
