//! **Signing in with GitHub.**
//!
//! Not OIDC. GitHub's OAuth app flow has no discovery document, no ID token and
//! no PKCE — you exchange the code for an access token and then *ask* who it
//! belongs to. So there is no signature to check; what stands in for it is that
//! the answers come from `api.github.com` over TLS, using a token GitHub only
//! hands out in exchange for a code it issued to this application.
//!
//! Everything that makes the round trip safe is shared with [`super::oidc`] via
//! [`super::sso_flow`]: single-use expiring state, the cookie binding the flow
//! to one browser, and a same-origin destination.
//!
//! - `GET /api/auth/github/start`    — JSON `{ url }` to send the browser to
//! - `GET /api/auth/github/callback` — GitHub redirects here (auth-exempt)
//! - `GET/PUT /api/settings/sso/github`      — configure it (administrator-only)
//! - `POST    /api/settings/sso/github/test` — prove it works before arming it
//!
//! **The allowlist is never "anyone with a GitHub account".** GitHub accounts
//! are free, so unlike a single-tenant OIDC issuer the provider itself is not a
//! membership boundary. Two things can be one, and [`Audience`] is a choice
//! between them, with no third option meaning everybody:
//!
//!   * an **organisation** you control, for a shared install; or
//!   * the accounts that **already exist here**, for a personal one -- matched
//!     on GitHub's verified email, and never creating an account, so the
//!     boundary is "people who were already invited".

use std::collections::BTreeMap;
use std::sync::Arc;

use axum::extract::{Extension, Query};
use axum::http::{header, HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::Json;
use harness_persist::{CredentialStore, NewUser};
use serde::Deserialize;
use serde_json::json;

use super::accounts::{self, AdminOnly, Mode, ROLE_MEMBER};
use super::runs_routes::RunsState;
use super::sso_flow::{
    back, binding_cookie, binding_matches, clear_binding_cookie, enc, err, hash, issue_state,
    random_token, safe_next, take_state, Attempt, Provider,
};

/// Credential provider the configuration lives under.
const PROVIDER: &str = "sso-github";

pub(crate) const CALLBACK_PATH: &str = "/api/auth/github/callback";

const AUTHORIZE_URL: &str = "https://github.com/login/oauth/authorize";
const TOKEN_URL: &str = "https://github.com/login/oauth/access_token";
const API: &str = "https://api.github.com";

/// `read:org` is what makes private organisation membership visible. Without
/// it, only members who have made their membership public would be let in,
/// which is a confusing and arbitrary line.
const SCOPES: &str = "read:user user:email read:org";

/// The same, minus the organisation read.
///
/// Existing-accounts mode never asks GitHub about membership, so asking the
/// person to grant it would be requesting a permission we do not use.
const SCOPES_NO_ORG: &str = "read:user user:email";

/// Every GitHub request needs one, and a descriptive one is good manners.
const UA: &str = "ai-harness";

// ── Configuration ────────────────────────────────────────────────────────────

struct Config {
    client_id: String,
    client_secret: String,
    audience: Audience,
    enabled: bool,
}

/// Where the allowlist comes from. See the module note on why there is no
/// variant meaning "anybody".
#[derive(Debug, Clone, PartialEq, Eq)]
enum Audience {
    /// Members of this organisation, optionally narrowed to one team.
    Org { org: String, team: Option<String> },
    /// People who already have an account here. Enforced in [`link`], which
    /// refuses to create one, so the set is exactly who has been invited.
    Existing,
}

impl Audience {
    /// What to ask GitHub for on the authorize URL.
    fn scopes(&self) -> &'static str {
        match self {
            Audience::Org { .. } => SCOPES,
            Audience::Existing => SCOPES_NO_ORG,
        }
    }
}

/// Read the audience out of stored fields.
///
/// `None` means *not configured* rather than *open*: the default is
/// organisation mode, and that mode without an organisation is incomplete
/// setup. Getting this wrong in the other direction would silently admit
/// every GitHub user, so it is a separate function with its own test.
fn audience_of(fields: &BTreeMap<String, String>) -> Option<Audience> {
    match field(fields, "audience").as_deref() {
        Some("existing") => Some(Audience::Existing),
        _ => Some(Audience::Org {
            org: field(fields, "org")?,
            team: field(fields, "team"),
        }),
    }
}

fn field(fields: &BTreeMap<String, String>, key: &str) -> Option<String> {
    fields
        .get(key)
        .map(|v| v.trim())
        .filter(|v| !v.is_empty())
        .map(str::to_string)
}

async fn config(store: &CredentialStore) -> Option<Config> {
    let fields = store.get(PROVIDER).await.ok().flatten()?;
    Some(Config {
        client_id: field(&fields, "client_id")?,
        client_secret: field(&fields, "client_secret")?,
        audience: audience_of(&fields)?,
        enabled: field(&fields, "enabled").as_deref() == Some("true"),
    })
}

// ── What GitHub tells us ─────────────────────────────────────────────────────

#[derive(Debug, Deserialize)]
struct TokenResponse {
    #[serde(default)]
    access_token: Option<String>,
    /// GitHub answers a refused exchange with HTTP 200 and an error body, so
    /// the status line alone does not say whether this worked.
    #[serde(default)]
    error_description: Option<String>,
    #[serde(default)]
    error: Option<String>,
}

#[derive(Debug, Deserialize)]
struct GhUser {
    login: String,
    #[serde(default)]
    name: Option<String>,
    // `email` is deliberately absent. GitHub's profile email is free text the
    // account holder sets, so reading it here would be an invitation to trust
    // it — see `verified_email`, which asks GitHub what it will vouch for.
}

#[derive(Debug, Deserialize)]
struct GhEmail {
    email: String,
    primary: bool,
    verified: bool,
}

#[derive(Debug, Deserialize)]
struct GhMembership {
    /// `active` once the invitation has been accepted; `pending` before.
    state: String,
}

async fn get_json<T: serde::de::DeserializeOwned>(url: &str, token: &str) -> Result<T, String> {
    let resp = reqwest::Client::new()
        .get(url)
        .bearer_auth(token)
        .header(header::USER_AGENT, UA)
        .header(header::ACCEPT, "application/vnd.github+json")
        .send()
        .await
        .map_err(|e| format!("could not reach GitHub: {e}"))?;
    if !resp.status().is_success() {
        return Err(format!(
            "GitHub answered HTTP {} for {url}",
            resp.status().as_u16()
        ));
    }
    resp.json::<T>()
        .await
        .map_err(|e| format!("GitHub sent something unexpected: {e}"))
}

/// The address GitHub is willing to vouch for.
///
/// The primary **verified** one, and nothing else: `/user`'s `email` field is a
/// free-text profile value the account holder sets, so trusting it would let
/// anyone claim somebody else's account here by typing their address into a
/// GitHub profile.
async fn verified_email(token: &str) -> Result<String, String> {
    let emails: Vec<GhEmail> = get_json(&format!("{API}/user/emails"), token).await?;
    emails
        .iter()
        .find(|e| e.primary && e.verified)
        .or_else(|| emails.iter().find(|e| e.verified))
        .map(|e| e.email.trim().to_lowercase())
        .ok_or_else(|| {
            "your GitHub account has no verified email address, so it cannot be used to sign in \
             here"
                .to_string()
        })
}

/// Whether this account is actually in the organisation (and team) allowed.
///
/// Existing-accounts mode has nothing to ask GitHub here -- its boundary is
/// enforced by [`link`] refusing to create an account -- so it passes through.
async fn permitted(token: &str, login: &str, cfg: &Config) -> Result<(), String> {
    let Audience::Org { org, team } = &cfg.audience else {
        return Ok(());
    };

    let membership: GhMembership = get_json(&format!("{API}/user/memberships/orgs/{org}"), token)
        .await
        .map_err(|_| format!("{login} is not a member of {org}"))?;
    // A pending invitation is not membership.
    if membership.state != "active" {
        return Err(format!(
            "{login}'s membership of {org} is {} rather than active",
            membership.state
        ));
    }

    if let Some(team) = team {
        let url = format!("{API}/orgs/{org}/teams/{team}/memberships/{login}");
        let team_membership: GhMembership = get_json(&url, token)
            .await
            .map_err(|_| format!("{login} is not in the {team} team"))?;
        if team_membership.state != "active" {
            return Err(format!("{login}'s membership of {team} is not active"));
        }
    }
    Ok(())
}

// ── The flow ─────────────────────────────────────────────────────────────────

fn redirect_uri(state: &Arc<RunsState>) -> Result<String, String> {
    let base = state
        .public_url()
        .ok_or("no public URL configured — set it under Settings -> General first")?;
    Ok(format!("{base}{CALLBACK_PATH}"))
}

#[derive(Debug, Default, Deserialize)]
pub struct StartQuery {
    #[serde(default)]
    pub next: Option<String>,
}

/// `GET /api/auth/github/start` — the URL to send the browser to.
pub async fn start(
    Extension(state): Extension<Arc<RunsState>>,
    Query(q): Query<StartQuery>,
) -> Response {
    match authorize_url(&state, safe_next(q.next.as_deref()), false).await {
        Ok((url, cookie)) => {
            ([(header::SET_COOKIE, cookie)], Json(json!({ "url": url }))).into_response()
        }
        Err(e) => err(StatusCode::PRECONDITION_FAILED, e),
    }
}

async fn authorize_url(
    state: &Arc<RunsState>,
    next: String,
    test: bool,
) -> Result<(String, String), String> {
    let store = state.cred_store().await?;
    let cfg = config(store)
        .await
        .ok_or("GitHub sign-in is not configured")?;
    if !cfg.enabled && !test {
        return Err("GitHub sign-in is not switched on".to_string());
    }
    let redirect = redirect_uri(state)?;

    let binding = random_token();
    let state_nonce = issue_state(Attempt {
        provider: Provider::GitHub,
        // GitHub's OAuth app flow has neither.
        verifier: None,
        nonce: None,
        next,
        test,
        binding_hash: hash(&binding),
    });
    let cookie = binding_cookie(&binding, accounts::secure_cookies(state));

    let url = format!(
        "{AUTHORIZE_URL}?client_id={}&redirect_uri={}&scope={}&state={}",
        enc(&cfg.client_id),
        enc(&redirect),
        enc(cfg.audience.scopes()),
        enc(&state_nonce),
    );
    Ok((url, cookie))
}

#[derive(Debug, Deserialize)]
pub struct CallbackQuery {
    #[serde(default)]
    pub code: Option<String>,
    #[serde(default)]
    pub state: Option<String>,
    #[serde(default)]
    pub error: Option<String>,
    #[serde(default)]
    pub error_description: Option<String>,
}

/// `GET /api/auth/github/callback` — GitHub's redirect.
pub async fn callback(
    Extension(state): Extension<Arc<RunsState>>,
    headers: HeaderMap,
    Query(q): Query<CallbackQuery>,
) -> Response {
    if let Some(e) = q.error.as_deref() {
        let detail = q.error_description.as_deref().unwrap_or(e);
        return back("/login", "denied", Some(detail));
    }
    let (Some(code), Some(state_nonce)) = (q.code.as_deref(), q.state.as_deref()) else {
        return back("/login", "error", Some("callback missing code or state"));
    };
    let Some(pending) = take_state(state_nonce, Provider::GitHub) else {
        return back(
            "/login",
            "error",
            Some("that sign-in expired or was already used — try again"),
        );
    };

    let secure = accounts::secure_cookies(&state);
    let landing = if pending.test {
        "/settings/sso"
    } else {
        "/login"
    };

    // The browser that finishes must be the one that started.
    if !binding_matches(&pending, &headers) {
        tracing::warn!("github: callback without the binding cookie that started the flow");
        return (
            [(header::SET_COOKIE, clear_binding_cookie(secure))],
            back(
                landing,
                "error",
                Some("that sign-in did not start in this browser — try again here"),
            ),
        )
            .into_response();
    }

    match complete(&state, code, pending.test).await {
        Ok(Some(user)) => match accounts::open_session(
            &state,
            match state.user_store().await {
                Ok(u) => u,
                Err(e) => return back(landing, "error", Some(&e)),
            },
            &user,
        )
        .await
        {
            Ok(cookie) => (
                [
                    (header::SET_COOKIE, cookie),
                    (header::SET_COOKIE, clear_binding_cookie(secure)),
                ],
                back(&pending.next, "ok", None),
            )
                .into_response(),
            Err(e) => back(landing, "error", Some(&e)),
        },
        Ok(None) => {
            arm(&state).await;
            (
                [(header::SET_COOKIE, clear_binding_cookie(secure))],
                back("/settings/sso", "tested", None),
            )
                .into_response()
        }
        Err(e) => {
            tracing::warn!("github: sign-in failed: {e}");
            (
                [(header::SET_COOKIE, clear_binding_cookie(secure))],
                back(landing, "error", Some(&e)),
            )
                .into_response()
        }
    }
}

/// Exchange the code and work out who it belongs to.
///
/// `Ok(None)` means the round trip worked and this was a test, so nobody was
/// signed in.
async fn complete(
    state: &Arc<RunsState>,
    code: &str,
    test: bool,
) -> Result<Option<harness_persist::User>, String> {
    let store = state.cred_store().await?;
    let cfg = config(store)
        .await
        .ok_or("GitHub sign-in is not configured")?;
    let redirect = redirect_uri(state)?;

    let resp = reqwest::Client::new()
        .post(TOKEN_URL)
        .header(header::ACCEPT, "application/json")
        .header(header::USER_AGENT, UA)
        .form(&[
            ("client_id", cfg.client_id.as_str()),
            ("client_secret", cfg.client_secret.as_str()),
            ("code", code),
            ("redirect_uri", redirect.as_str()),
        ])
        .send()
        .await
        .map_err(|e| format!("the token exchange failed: {e}"))?;

    let tokens: TokenResponse = resp
        .json()
        .await
        .map_err(|e| format!("GitHub's token response was unreadable: {e}"))?;
    // GitHub answers a refused exchange with 200 and an error body, so the
    // absence of a token is the signal, not the status code.
    let token = tokens.access_token.ok_or_else(|| {
        tokens
            .error_description
            .or(tokens.error)
            .unwrap_or_else(|| "GitHub refused the code".to_string())
    })?;

    let profile: GhUser = get_json(&format!("{API}/user"), &token).await?;
    permitted(&token, &profile.login, &cfg).await?;
    let email = verified_email(&token).await?;

    // A test in organisation mode stops here: `permitted` above was the check,
    // and going further would create an account as a side effect of testing.
    //
    // Existing-accounts mode has to keep going. Its allowlist lives entirely in
    // `link`, which never creates -- so a test that returned here would prove
    // only that OAuth works, then arm a provider that cannot actually sign
    // anybody in. That is the opposite of what testing before arming is for.
    if test && matches!(cfg.audience, Audience::Org { .. }) {
        return Ok(None);
    }
    let user = link(state, &profile, &email, &cfg.audience).await?;
    Ok((!test).then_some(user))
}

/// Find or create the account this identity belongs to.
///
/// Matching an existing account by email is safe here because the address came
/// from `/user/emails` marked verified — GitHub vouching for it, not the
/// account holder asserting it.
async fn link(
    state: &Arc<RunsState>,
    profile: &GhUser,
    email: &str,
    audience: &Audience,
) -> Result<harness_persist::User, String> {
    let users = state.user_store().await?;
    if let Some(existing) = users.get_by_email(email).await.map_err(|e| e.to_string())? {
        if existing.disabled_at.is_some() {
            return Err("that account is suspended".to_string());
        }
        return Ok(existing);
    }

    // Nobody signs in to an unclaimed harness through a provider: the first
    // account is deliberately made at /setup.
    if accounts::mode(state).await != Mode::Accounts {
        return Err("this harness has not been set up yet".to_string());
    }

    // Existing-accounts mode: this is the allowlist, and we have reached the
    // end of it. Naming the address is the whole of the fix -- the usual cause
    // is an account here under a different address from the one GitHub
    // vouches for, and without the address there is nothing to compare.
    if *audience == Audience::Existing {
        return Err(format!(
            "no account here uses {email}, the verified address GitHub offered for @{}. \
             Sign in with a password and check the address on your account, or have an \
             administrator invite {email}.",
            profile.login
        ));
    }

    let name = profile
        .name
        .clone()
        .filter(|n| !n.trim().is_empty())
        .unwrap_or_else(|| profile.login.clone());
    users
        .create(&NewUser {
            email: email.to_string(),
            name,
            role: ROLE_MEMBER.to_string(),
            password_hash: None,
        })
        .await
        .map_err(|e| e.to_string())
        .inspect(|_| {
            tracing::info!(
                "github: created an account for {email} (@{})",
                profile.login
            )
        })
}

// ── Settings ─────────────────────────────────────────────────────────────────

/// What the sign-in page may know before anybody has signed in.
pub async fn public_status(Extension(state): Extension<Arc<RunsState>>) -> Response {
    let enabled = match state.cred_store().await {
        Ok(store) => config(store).await.is_some_and(|c| c.enabled),
        Err(_) => false,
    };
    Json(json!({ "enabled": enabled })).into_response()
}

/// `GET /api/settings/sso/github` — the configuration, without the secret.
pub async fn describe(_: AdminOnly, Extension(state): Extension<Arc<RunsState>>) -> Response {
    let store = match state.cred_store().await {
        Ok(s) => s,
        Err(e) => return err(StatusCode::SERVICE_UNAVAILABLE, e),
    };
    let fields = store.get(PROVIDER).await.ok().flatten().unwrap_or_default();
    Json(json!({
        "client_id": field(&fields, "client_id"),
        "client_secret_set": field(&fields, "client_secret").is_some(),
        "audience": match audience_of(&fields) {
            Some(Audience::Existing) => "existing",
            _ => "org",
        },
        "org": field(&fields, "org"),
        "team": field(&fields, "team"),
        "enabled": field(&fields, "enabled").as_deref() == Some("true"),
        "callback_url": state.public_url().map(|b| format!("{b}{CALLBACK_PATH}")),
    }))
    .into_response()
}

#[derive(Debug, Deserialize)]
pub struct ConfigRequest {
    pub client_id: Option<String>,
    /// Omitted leaves the stored one alone.
    pub client_secret: Option<String>,
    pub org: Option<String>,
    pub team: Option<String>,
    /// `"org"` (the default) or `"existing"`.
    pub audience: Option<String>,
}

/// `PUT /api/settings/sso/github` — save the configuration.
///
/// Saving never arms it; a successful test does, and changing anything disarms
/// it again.
pub async fn configure(
    _: AdminOnly,
    Extension(state): Extension<Arc<RunsState>>,
    Json(req): Json<ConfigRequest>,
) -> Response {
    let store = match state.cred_store().await {
        Ok(s) => s,
        Err(e) => return err(StatusCode::SERVICE_UNAVAILABLE, e),
    };
    let mut fields: BTreeMap<String, String> = BTreeMap::new();
    for (key, value) in [
        ("client_id", req.client_id),
        ("org", req.org),
        ("team", req.team),
        // Anything unrecognised stores as organisation mode, which is the
        // mode that then demands an organisation. A typo cannot open it up.
        (
            "audience",
            req.audience
                .map(|a| if a == "existing" { a } else { "org".into() }),
        ),
    ] {
        if let Some(v) = value {
            fields.insert(key.into(), v.trim().to_string());
        }
    }
    if let Some(v) = req.client_secret.filter(|s| !s.is_empty()) {
        fields.insert("client_secret".into(), v);
    }
    if fields.is_empty() {
        return err(StatusCode::BAD_REQUEST, "nothing to save");
    }
    fields.insert("enabled".into(), "false".into());
    match store.set(PROVIDER, &fields).await {
        Ok(()) => describe(AdminOnly, Extension(state)).await,
        Err(e) => err(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()),
    }
}

/// `POST /api/settings/sso/github/test` — a round trip that arms it.
pub async fn test_github(_: AdminOnly, Extension(state): Extension<Arc<RunsState>>) -> Response {
    // `config` returns `None` when organisation mode has no organisation, which
    // is the case worth catching: a GitHub account is free, so a mode whose
    // allowlist is empty would be an allowlist of everybody.
    let ready = match state.cred_store().await {
        Ok(store) => config(store).await.is_some(),
        Err(e) => return err(StatusCode::SERVICE_UNAVAILABLE, e),
    };
    if !ready {
        return err(
            StatusCode::PRECONDITION_FAILED,
            "set a client ID and client secret, and either an organisation or \
             existing-accounts mode, first",
        );
    }
    match authorize_url(&state, "/settings/sso".to_string(), true).await {
        Ok((url, cookie)) => {
            ([(header::SET_COOKIE, cookie)], Json(json!({ "url": url }))).into_response()
        }
        Err(e) => err(StatusCode::PRECONDITION_FAILED, e),
    }
}

async fn arm(state: &Arc<RunsState>) {
    if let Ok(store) = state.cred_store().await {
        let fields = BTreeMap::from([("enabled".to_string(), "true".to_string())]);
        if let Err(e) = store.set(PROVIDER, &fields).await {
            tracing::warn!("github: could not arm the provider: {e}");
        } else {
            tracing::info!("github: provider armed after a successful test");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn emails(raw: &str) -> Vec<GhEmail> {
        serde_json::from_str(raw).expect("parse")
    }

    /// The address must be one GitHub vouches for, not one the account holder
    /// typed — `/user`'s `email` is a free-text profile field.
    #[test]
    fn only_a_verified_address_is_accepted() {
        let list = emails(
            r#"[
              {"email":"unverified@example.test","primary":true,"verified":false},
              {"email":"Secondary@Example.test","primary":false,"verified":true}
            ]"#,
        );
        // Primary-but-unverified loses to secondary-but-verified.
        let chosen = list
            .iter()
            .find(|e| e.primary && e.verified)
            .or_else(|| list.iter().find(|e| e.verified))
            .map(|e| e.email.trim().to_lowercase());
        assert_eq!(chosen.as_deref(), Some("secondary@example.test"));

        // Nothing verified at all: no address to trust.
        let none = emails(r#"[{"email":"a@b.test","primary":true,"verified":false}]"#);
        assert!(none.iter().any(|e| !e.verified));
        assert!(none.iter().find(|e| e.verified).is_none());
    }

    #[test]
    fn the_primary_verified_address_wins() {
        let list = emails(
            r#"[
              {"email":"other@example.test","primary":false,"verified":true},
              {"email":"primary@example.test","primary":true,"verified":true}
            ]"#,
        );
        let chosen = list
            .iter()
            .find(|e| e.primary && e.verified)
            .map(|e| e.email.clone());
        assert_eq!(chosen.as_deref(), Some("primary@example.test"));
    }

    /// GitHub answers a refused exchange with HTTP 200 and an error body, so
    /// the status line cannot be the signal.
    #[test]
    fn a_refused_exchange_is_recognised_despite_a_200() {
        let refused: TokenResponse = serde_json::from_str(
            r#"{"error":"bad_verification_code",
                "error_description":"The code passed is incorrect or expired."}"#,
        )
        .expect("parse");
        assert!(refused.access_token.is_none());
        assert_eq!(
            refused.error_description.as_deref(),
            Some("The code passed is incorrect or expired.")
        );

        let ok: TokenResponse =
            serde_json::from_str(r#"{"access_token":"gho_x","token_type":"bearer"}"#)
                .expect("parse");
        assert_eq!(ok.access_token.as_deref(), Some("gho_x"));
    }

    /// A pending invitation is not membership.
    #[test]
    fn only_active_membership_counts() {
        let pending: GhMembership =
            serde_json::from_str(r#"{"state":"pending","role":"member"}"#).expect("parse");
        assert_ne!(pending.state, "active");
        let active: GhMembership =
            serde_json::from_str(r#"{"state":"active","role":"member"}"#).expect("parse");
        assert_eq!(active.state, "active");
    }

    fn fields(pairs: &[(&str, &str)]) -> BTreeMap<String, String> {
        pairs
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect()
    }

    #[test]
    fn organisation_mode_is_the_default_and_needs_an_organisation() {
        // The direction that matters: a half-configured install must read as
        // unconfigured, never as "anyone with a GitHub account".
        assert_eq!(audience_of(&fields(&[])), None);
        assert_eq!(audience_of(&fields(&[("org", "   ")])), None);
        assert_eq!(
            audience_of(&fields(&[("audience", "org")])),
            None,
            "organisation mode without an organisation is not configured"
        );
        assert_eq!(
            audience_of(&fields(&[("org", "acme")])),
            Some(Audience::Org {
                org: "acme".into(),
                team: None
            })
        );
    }

    #[test]
    fn an_unrecognised_audience_falls_back_to_needing_an_organisation() {
        // A typo, an old value, a hand-edited row: none of them may open it up.
        for odd in ["", "everyone", "any", "EXISTING", "true"] {
            assert_eq!(
                audience_of(&fields(&[("audience", odd)])),
                None,
                "audience={odd:?} must not be usable without an organisation"
            );
        }
    }

    #[test]
    fn existing_mode_needs_no_organisation() {
        assert_eq!(
            audience_of(&fields(&[("audience", "existing")])),
            Some(Audience::Existing)
        );
        // A leftover organisation from a previous mode is ignored rather than
        // quietly still applying.
        assert_eq!(
            audience_of(&fields(&[("audience", "existing"), ("org", "acme")])),
            Some(Audience::Existing)
        );
    }

    #[test]
    fn a_team_narrows_only_organisation_mode() {
        assert_eq!(
            audience_of(&fields(&[("org", "acme"), ("team", "eng")])),
            Some(Audience::Org {
                org: "acme".into(),
                team: Some("eng".into())
            })
        );
    }

    #[test]
    fn existing_mode_does_not_ask_for_permissions_it_will_not_use() {
        // Nothing consults organisation membership in this mode, so nothing
        // should be requesting the right to read it.
        assert!(!Audience::Existing.scopes().contains("read:org"));
        assert!(Audience::Existing.scopes().contains("user:email"));
        assert!(Audience::Org {
            org: "acme".into(),
            team: None
        }
        .scopes()
        .contains("read:org"));
    }

    #[test]
    fn the_authorize_scopes_ask_for_private_org_membership() {
        // Without `read:org`, only members who made their membership public
        // would be let in — an arbitrary line nobody would understand.
        assert!(SCOPES.contains("read:org"), "{SCOPES}");
        assert!(SCOPES.contains("user:email"), "{SCOPES}");
    }
}
