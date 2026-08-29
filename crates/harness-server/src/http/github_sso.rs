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
//! **Membership in an organisation is the allowlist.** A GitHub account is
//! free and anyone can have one, so unlike a single-tenant OIDC issuer the
//! provider itself is not a membership boundary — which is why an organisation
//! is required here rather than optional.

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

/// Every GitHub request needs one, and a descriptive one is good manners.
const UA: &str = "ai-harness";

// ── Configuration ────────────────────────────────────────────────────────────

struct Config {
    client_id: String,
    client_secret: String,
    /// Members of this organisation may sign in. **Required** — see the module
    /// note on why this is not optional the way OIDC's domain list is.
    org: String,
    /// Optionally narrow it further to one team, by slug.
    team: Option<String>,
    enabled: bool,
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
        org: field(&fields, "org")?,
        team: field(&fields, "team"),
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
async fn permitted(token: &str, login: &str, cfg: &Config) -> Result<(), String> {
    let membership: GhMembership =
        get_json(&format!("{API}/user/memberships/orgs/{}", cfg.org), token)
            .await
            .map_err(|_| format!("{login} is not a member of {}", cfg.org))?;
    // A pending invitation is not membership.
    if membership.state != "active" {
        return Err(format!(
            "{login}'s membership of {} is {} rather than active",
            cfg.org, membership.state
        ));
    }

    if let Some(team) = &cfg.team {
        let url = format!("{API}/orgs/{}/teams/{team}/memberships/{login}", cfg.org);
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
        enc(SCOPES),
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

    if test {
        return Ok(None);
    }
    link(state, &profile, &email).await.map(Some)
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
    // An organisation is required rather than optional: a GitHub account is
    // free, so without one the "allowlist" would be everybody.
    let has_org = match state.cred_store().await {
        Ok(store) => config(store).await.is_some(),
        Err(e) => return err(StatusCode::SERVICE_UNAVAILABLE, e),
    };
    if !has_org {
        return err(
            StatusCode::PRECONDITION_FAILED,
            "set a client ID, client secret and organisation first",
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

    #[test]
    fn the_authorize_scopes_ask_for_private_org_membership() {
        // Without `read:org`, only members who made their membership public
        // would be let in — an arbitrary line nobody would understand.
        assert!(SCOPES.contains("read:org"), "{SCOPES}");
        assert!(SCOPES.contains("user:email"), "{SCOPES}");
    }
}
