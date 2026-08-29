//! **Signing in with an identity provider** — Entra, Google, Okta, Keycloak,
//! Authentik, anything that speaks OIDC discovery.
//!
//! One implementation, configured by issuer URL. Building "Entra support" and
//! "Google support" separately would be the same code three times with
//! different constants.
//!
//! - `GET /api/auth/oidc/start`              — JSON `{ url }` to send the browser to
//! - `GET /api/auth/oidc/callback?code=&state=` — the provider redirects here (auth-exempt)
//! - `GET/PUT /api/settings/sso`             — configure it (administrator-only)
//! - `POST    /api/settings/sso/test`        — prove it works before arming it
//!
//! **Nothing here can lock you out.** Local password sign-in is never disabled;
//! this adds a button beside it. And a provider only becomes usable after a
//! round trip that actually succeeded — saving a typo leaves the previous state
//! alone. Between those two, a misconfiguration costs a retry rather than a
//! `harness admin create` on the server.
//!
//! **What is validated.** The ID token's signature against the issuer's JWKS,
//! and then `iss`, `aud`, `exp` and the `nonce` this server minted. A signature
//! check alone would accept a token minted for somebody else's application.

use std::collections::BTreeMap;
use std::sync::Arc;

use axum::extract::{Extension, Query};
use axum::http::{header, HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::Json;
use base64::Engine;
use harness_persist::{CredentialStore, NewUser};
use jsonwebtoken::{decode, decode_header, Algorithm, DecodingKey, Validation};
use serde::Deserialize;
use serde_json::json;
use sha2::{Digest, Sha256};

use super::accounts::{self, AdminOnly, Mode, ROLE_MEMBER};
use super::runs_routes::RunsState;
use super::sso_flow::{
    back, binding_cookie, binding_matches, clear_binding_cookie, enc, err, hash, issue_state,
    random_token, safe_next, take_state, Attempt, Pending, Provider,
};

/// Credential provider the configuration lives under.
const PROVIDER: &str = "sso-oidc";

/// Where the provider sends the browser back. Must match what is registered
/// with the provider exactly, and is auth-exempt for the same reason Linear's
/// callback is: a redirect cannot carry a bearer token, and the single-use
/// `state` nonce is what authenticates it.
pub(crate) const CALLBACK_PATH: &str = "/api/auth/oidc/callback";

fn b64url(bytes: &[u8]) -> String {
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(bytes)
}

// ── Configuration ────────────────────────────────────────────────────────────

/// How this harness talks to one identity provider.
struct Config {
    issuer: String,
    client_id: String,
    client_secret: String,
    /// Only these email domains may sign in. Empty = any the provider vouches
    /// for, which is the right default for a single-tenant issuer and the wrong
    /// one for a multi-tenant provider — see [`describe`].
    allowed_domains: Vec<String>,
    /// Whether a successful round trip has been completed. Set only by
    /// [`test_sso`]; saving configuration never arms it.
    enabled: bool,
    label: String,
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
        issuer: field(&fields, "issuer")?.trim_end_matches('/').to_string(),
        client_id: field(&fields, "client_id")?,
        client_secret: field(&fields, "client_secret")?,
        allowed_domains: field(&fields, "allowed_domains")
            .map(|d| {
                d.split(',')
                    .map(|s| s.trim().trim_start_matches('@').to_lowercase())
                    .filter(|s| !s.is_empty())
                    .collect()
            })
            .unwrap_or_default(),
        enabled: field(&fields, "enabled").as_deref() == Some("true"),
        label: field(&fields, "label").unwrap_or_else(|| "your provider".into()),
    })
}

// ── Discovery and JWKS ───────────────────────────────────────────────────────

#[derive(Debug, Deserialize)]
struct Discovery {
    authorization_endpoint: String,
    token_endpoint: String,
    jwks_uri: String,
    issuer: String,
}

async fn discover(issuer: &str) -> Result<Discovery, String> {
    let url = format!("{issuer}/.well-known/openid-configuration");
    let resp = reqwest::Client::new()
        .get(&url)
        .send()
        .await
        .map_err(|e| format!("could not reach {url}: {e}"))?;
    if !resp.status().is_success() {
        return Err(format!(
            "{url} answered HTTP {} — is the issuer URL right?",
            resp.status().as_u16()
        ));
    }
    resp.json::<Discovery>()
        .await
        .map_err(|e| format!("{url} is not an OIDC discovery document: {e}"))
}

#[derive(Debug, Deserialize)]
struct Jwk {
    kid: Option<String>,
    kty: String,
    n: Option<String>,
    e: Option<String>,
}

#[derive(Debug, Deserialize)]
struct Jwks {
    keys: Vec<Jwk>,
}

/// The signing key for one `kid`, fetched fresh.
///
/// Not cached: sign-ins are rare, providers rotate keys without warning, and a
/// stale cache turns a rotation into an outage nobody can debug.
async fn signing_key(jwks_uri: &str, kid: Option<&str>) -> Result<DecodingKey, String> {
    let jwks: Jwks = reqwest::Client::new()
        .get(jwks_uri)
        .send()
        .await
        .map_err(|e| format!("could not fetch the signing keys: {e}"))?
        .json()
        .await
        .map_err(|e| format!("the signing keys are not a JWKS: {e}"))?;

    let key = jwks
        .keys
        .iter()
        .find(|k| match (kid, &k.kid) {
            (Some(want), Some(have)) => want == have,
            // A provider publishing one key need not label it.
            (None, _) | (_, None) => true,
        })
        .ok_or("the provider did not publish the key this token was signed with")?;

    if key.kty != "RSA" {
        return Err(format!("unsupported key type `{}`", key.kty));
    }
    let (n, e) = (
        key.n.as_deref().ok_or("signing key has no modulus")?,
        key.e.as_deref().ok_or("signing key has no exponent")?,
    );
    DecodingKey::from_rsa_components(n, e).map_err(|e| format!("unusable signing key: {e}"))
}

/// The claims this harness reads. Everything else the provider sends is ignored.
#[derive(Debug, Deserialize)]
struct Claims {
    sub: String,
    #[serde(default)]
    email: Option<String>,
    /// Whether the *provider* vouches for the address. Account linking depends
    /// on it, so its absence is treated as `false`.
    #[serde(default)]
    email_verified: Option<bool>,
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    preferred_username: Option<String>,
    #[serde(default)]
    nonce: Option<String>,
}

// ── The flow ─────────────────────────────────────────────────────────────────

/// Where the provider must be told to send the browser back.
fn redirect_uri(state: &Arc<RunsState>) -> Result<String, String> {
    let base = state
        .public_url()
        .ok_or("no public URL configured — set it under Settings -> General first")?;
    Ok(format!("{base}{CALLBACK_PATH}"))
}

#[derive(Debug, Default, Deserialize)]
pub struct StartQuery {
    /// Where to land afterwards. Same-origin paths only.
    #[serde(default)]
    pub next: Option<String>,
}

/// `GET /api/auth/oidc/start` — the URL to send the browser to.
///
/// JSON rather than a redirect so the sign-in page can surface a configuration
/// error in place, instead of bouncing the browser to a provider that will
/// reject it.
pub async fn start(
    Extension(state): Extension<Arc<RunsState>>,
    Query(q): Query<StartQuery>,
) -> Response {
    match authorize_url(&state, safe_next(q.next.as_deref()), false).await {
        // The cookie rides along on the same response the sign-in page is
        // already reading, so binding the flow costs nothing in the UI.
        Ok((url, cookie)) => {
            ([(header::SET_COOKIE, cookie)], Json(json!({ "url": url }))).into_response()
        }
        Err(e) => err(StatusCode::PRECONDITION_FAILED, e),
    }
}

/// Build the authorization URL and record what the callback will need.
async fn authorize_url(
    state: &Arc<RunsState>,
    next: String,
    test: bool,
) -> Result<(String, String), String> {
    let store = state.cred_store().await?;
    let cfg = config(store).await.ok_or("sign-in is not configured")?;
    // A test may run before the provider is armed; a real sign-in may not.
    if !cfg.enabled && !test {
        return Err("sign-in with a provider is not switched on".to_string());
    }
    let redirect = redirect_uri(state)?;
    let discovery = discover(&cfg.issuer).await?;

    // PKCE: the verifier never leaves this server, so an intercepted code is
    // useless without it.
    let verifier = format!("{}{}", random_token(), random_token());
    let challenge = b64url(&Sha256::digest(verifier.as_bytes()));
    // Bound into the ID token, and checked on the way back — this is what stops
    // a token minted for a different sign-in being replayed into ours.
    let nonce = random_token();
    // The browser keeps this; the map keeps only its hash. Presenting it back
    // at the callback is what proves the same browser started the flow.
    let binding = random_token();
    let state_nonce = issue_state(Attempt {
        provider: Provider::Oidc,
        verifier: Some(verifier),
        nonce: Some(nonce.clone()),
        next,
        test,
        binding_hash: hash(&binding),
    });
    let cookie = binding_cookie(&binding, accounts::secure_cookies(state));

    let url = format!(
        "{}?response_type=code&client_id={}&redirect_uri={}&scope={}&state={}&nonce={}\
         &code_challenge={}&code_challenge_method=S256",
        discovery.authorization_endpoint,
        enc(&cfg.client_id),
        enc(&redirect),
        enc("openid email profile"),
        enc(&state_nonce),
        enc(&nonce),
        enc(&challenge),
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

#[derive(Debug, Deserialize)]
struct TokenResponse {
    id_token: String,
}

/// `GET /api/auth/oidc/callback` — the provider's redirect.
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
    // Single-use, and half of what authenticates this request.
    let Some(pending) = take_state(state_nonce, Provider::Oidc) else {
        return back(
            "/login",
            "error",
            Some("that sign-in expired or was already used — try again"),
        );
    };

    // The other half: this browser must be the one that started the flow.
    // Without it a `state` minted elsewhere would seat somebody else's session
    // here, which is login CSRF however carefully the token itself is checked.
    let secure = accounts::secure_cookies(&state);
    if !binding_matches(&pending, &headers) {
        tracing::warn!("oidc: callback without the binding cookie that started the flow");
        return (
            [(header::SET_COOKIE, clear_binding_cookie(secure))],
            back(
                if pending.test {
                    "/settings/sso"
                } else {
                    "/login"
                },
                "error",
                Some("that sign-in did not start in this browser — try again here"),
            ),
        )
            .into_response();
    }

    match complete(&state, code, &pending).await {
        Ok(Some(cookie)) => (
            [
                (header::SET_COOKIE, cookie),
                (header::SET_COOKIE, clear_binding_cookie(secure)),
            ],
            back(&pending.next, "ok", None),
        )
            .into_response(),
        // A test proves the round trip without signing anybody in — and only
        // then is the provider offered on the sign-in page.
        Ok(None) => {
            arm(&state).await;
            back("/settings/sso", "tested", None)
        }
        Err(e) => {
            tracing::warn!("oidc: sign-in failed: {e}");
            back(
                if pending.test {
                    "/settings/sso"
                } else {
                    "/login"
                },
                "error",
                Some(&e),
            )
        }
    }
}

/// Exchange the code, validate the token, and turn it into a session.
///
/// `Ok(None)` means the round trip worked and this was a test, so nothing was
/// signed in.
async fn complete(
    state: &Arc<RunsState>,
    code: &str,
    pending: &Pending,
) -> Result<Option<String>, String> {
    let store = state.cred_store().await?;
    let cfg = config(store).await.ok_or("sign-in is not configured")?;
    let redirect = redirect_uri(state)?;
    let discovery = discover(&cfg.issuer).await?;

    let form = [
        ("grant_type", "authorization_code"),
        ("code", code),
        ("redirect_uri", redirect.as_str()),
        ("client_id", cfg.client_id.as_str()),
        ("client_secret", cfg.client_secret.as_str()),
        (
            "code_verifier",
            pending.verifier.as_deref().unwrap_or_default(),
        ),
    ];
    let resp = reqwest::Client::new()
        .post(&discovery.token_endpoint)
        .form(&form)
        .send()
        .await
        .map_err(|e| format!("the token exchange failed: {e}"))?;
    if !resp.status().is_success() {
        let status = resp.status().as_u16();
        let body = resp.text().await.unwrap_or_default();
        let short: String = body.chars().take(200).collect();
        return Err(format!(
            "the provider refused the code (HTTP {status}): {short}"
        ));
    }
    let tokens: TokenResponse = resp
        .json()
        .await
        .map_err(|e| format!("the token response had no id_token: {e}"))?;

    let expected_nonce = pending
        .nonce
        .as_deref()
        .ok_or("this sign-in recorded no nonce to check the token against")?;
    let claims = validate(&tokens.id_token, &discovery, &cfg, expected_nonce).await?;

    if pending.test {
        return Ok(None);
    }
    let user = link(state, &cfg, &claims).await?;
    let users = state.user_store().await?;
    accounts::open_session(state, users, &user).await.map(Some)
}

/// Check the ID token is genuine, current, addressed to us, and ours.
async fn validate(
    id_token: &str,
    discovery: &Discovery,
    cfg: &Config,
    expected_nonce: &str,
) -> Result<Claims, String> {
    let header = decode_header(id_token).map_err(|e| format!("unreadable id_token: {e}"))?;

    // **The algorithm is never taken from the token's own header.** That is how
    // a signature check becomes a no-op: `alg: none` skips it entirely, and
    // `alg: HS256` invites the verifier to use the issuer's *public* key as an
    // HMAC secret — which anyone can also do. Only RSA over the JWKS.
    const ALLOWED: [Algorithm; 3] = [Algorithm::RS256, Algorithm::RS384, Algorithm::RS512];
    if !ALLOWED.contains(&header.alg) {
        return Err(format!(
            "the id_token is signed with {:?}, which this harness does not accept",
            header.alg
        ));
    }
    let key = signing_key(&discovery.jwks_uri, header.kid.as_deref()).await?;

    let mut validation = Validation::new(header.alg);
    validation.algorithms = ALLOWED.to_vec();
    // `aud` must be this application: a signature check alone would accept a
    // token the provider minted for somebody else's client.
    validation.set_audience(&[cfg.client_id.as_str()]);
    // The issuer the discovery document names, not the URL we typed.
    validation.set_issuer(&[discovery.issuer.as_str()]);
    // `exp` is validated by default; state it so a future edit cannot quietly
    // drop it.
    validation.validate_exp = true;

    let data = decode::<Claims>(id_token, &key, &validation)
        .map_err(|e| format!("the id_token did not validate: {e}"))?;

    // The nonce this server minted, returned inside a signed token — which is
    // what makes replay of somebody else's token fail here.
    match data.claims.nonce.as_deref() {
        Some(got) if got == expected_nonce => {}
        _ => return Err("the id_token was not minted for this sign-in".to_string()),
    }
    Ok(data.claims)
}

/// Find or create the account this identity belongs to.
///
/// **Matching an existing account by email requires the provider to have
/// verified it.** Otherwise anyone who can set an unverified address at the
/// provider could claim somebody else's account here — the classic takeover
/// route through SSO, and the reason `email_verified` is not decoration.
async fn link(
    state: &Arc<RunsState>,
    cfg: &Config,
    claims: &Claims,
) -> Result<harness_persist::User, String> {
    let email = claims
        .email
        .as_deref()
        .map(|e| e.trim().to_lowercase())
        .filter(|e| e.contains('@'))
        .ok_or("the provider did not send an email address")?;

    if claims.email_verified != Some(true) {
        return Err(format!(
            "{} has not verified {email}, so it cannot be used to sign in here",
            cfg.label
        ));
    }

    if !cfg.allowed_domains.is_empty() {
        let domain = email.rsplit('@').next().unwrap_or_default().to_string();
        if !cfg.allowed_domains.contains(&domain) {
            return Err(format!("{email} is not in an allowed domain"));
        }
    }

    let users = state.user_store().await?;
    if let Some(existing) = users
        .get_by_email(&email)
        .await
        .map_err(|e| e.to_string())?
    {
        if existing.disabled_at.is_some() {
            return Err("that account is suspended".to_string());
        }
        return Ok(existing);
    }

    // Nobody signs in to an unclaimed harness through a provider: the first
    // account is deliberately made at /setup, by whoever can read the server's
    // log, and anything else would be a race to be first.
    if accounts::mode(state).await != Mode::Accounts {
        return Err("this harness has not been set up yet".to_string());
    }

    let name = claims
        .name
        .clone()
        .or_else(|| claims.preferred_username.clone())
        .unwrap_or_else(|| email.clone());
    users
        .create(&NewUser {
            email: email.clone(),
            name,
            role: ROLE_MEMBER.to_string(),
            // No password: this account signs in through the provider. It can
            // still be given one through the reset flow.
            password_hash: None,
        })
        .await
        .map_err(|e| e.to_string())
        .inspect(|_| tracing::info!("oidc: created an account for {email} (sub {})", claims.sub))
}

// ── Settings ─────────────────────────────────────────────────────────────────

/// What the sign-in page needs to know, without being signed in.
pub async fn public_status(Extension(state): Extension<Arc<RunsState>>) -> Response {
    let cfg = match state.cred_store().await {
        Ok(store) => config(store).await,
        Err(_) => None,
    };
    Json(json!({
        // Only an armed provider is offered — a half-configured one would be a
        // button that always fails.
        "enabled": cfg.as_ref().is_some_and(|c| c.enabled),
        "label": cfg.as_ref().map(|c| c.label.clone()),
    }))
    .into_response()
}

/// `GET /api/settings/sso` — the configuration, without the secret.
pub async fn describe(_: AdminOnly, Extension(state): Extension<Arc<RunsState>>) -> Response {
    let store = match state.cred_store().await {
        Ok(s) => s,
        Err(e) => return err(StatusCode::SERVICE_UNAVAILABLE, e),
    };
    let fields = store.get(PROVIDER).await.ok().flatten().unwrap_or_default();
    Json(json!({
        "issuer": field(&fields, "issuer"),
        "client_id": field(&fields, "client_id"),
        "client_secret_set": field(&fields, "client_secret").is_some(),
        "allowed_domains": field(&fields, "allowed_domains"),
        "label": field(&fields, "label"),
        "enabled": field(&fields, "enabled").as_deref() == Some("true"),
        // Register this with the provider, exactly.
        "callback_url": state.public_url().map(|b| format!("{b}{CALLBACK_PATH}")),
    }))
    .into_response()
}

#[derive(Debug, Deserialize)]
pub struct ConfigRequest {
    pub issuer: Option<String>,
    pub client_id: Option<String>,
    /// Omitted leaves the stored one alone.
    pub client_secret: Option<String>,
    pub allowed_domains: Option<String>,
    pub label: Option<String>,
    /// Only ever `false` here. Turning it *on* is what a successful test does.
    pub enabled: Option<bool>,
}

/// `PUT /api/settings/sso` — save the configuration.
///
/// Saving never arms the provider. A typo that armed itself would sign nobody
/// in and hide a working local password form behind a broken button.
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
    if let Some(v) = req.issuer {
        let v = v.trim().trim_end_matches('/').to_string();
        if !v.is_empty() && !v.starts_with("https://") {
            return err(
                StatusCode::BAD_REQUEST,
                "the issuer must be an https:// URL",
            );
        }
        fields.insert("issuer".into(), v);
    }
    for (key, value) in [
        ("client_id", req.client_id),
        ("allowed_domains", req.allowed_domains),
        ("label", req.label),
    ] {
        if let Some(v) = value {
            fields.insert(key.into(), v.trim().to_string());
        }
    }
    if let Some(v) = req.client_secret.filter(|s| !s.is_empty()) {
        fields.insert("client_secret".into(), v);
    }
    // Changing anything un-arms it: the next test is what proves the new
    // settings work, and until then the button should not be offered.
    let disable = req.enabled == Some(false) || !fields.is_empty();
    if disable {
        fields.insert("enabled".into(), "false".into());
    }
    if fields.is_empty() {
        return err(StatusCode::BAD_REQUEST, "nothing to save");
    }
    match store.set(PROVIDER, &fields).await {
        Ok(()) => describe(AdminOnly, Extension(state)).await,
        Err(e) => err(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()),
    }
}

/// `POST /api/settings/sso/test` — the URL for a round trip that arms it.
///
/// The provider is switched on by the callback, only after a token has come
/// back and validated. That is the difference between "these settings look
/// right" and "this works".
pub async fn test_sso(_: AdminOnly, Extension(state): Extension<Arc<RunsState>>) -> Response {
    match authorize_url(&state, "/settings/sso".to_string(), true).await {
        Ok((url, cookie)) => {
            ([(header::SET_COOKIE, cookie)], Json(json!({ "url": url }))).into_response()
        }
        Err(e) => err(StatusCode::PRECONDITION_FAILED, e),
    }
}

/// Arm the provider. Called by the callback once a test has actually worked.
async fn arm(state: &Arc<RunsState>) {
    if let Ok(store) = state.cred_store().await {
        let fields = BTreeMap::from([("enabled".to_string(), "true".to_string())]);
        if let Err(e) = store.set(PROVIDER, &fields).await {
            tracing::warn!("oidc: could not arm the provider: {e}");
        } else {
            tracing::info!("oidc: provider armed after a successful test");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_pkce_challenge_is_the_sha256_of_the_verifier() {
        // Known-answer from RFC 7636 appendix B, so this pins the encoding as
        // well as the hash — a base64 with padding would be quietly wrong.
        let verifier = "dBjftJeZ4CVP-mB92K27uhbUJU1p1r_wW1gFWFOEjXk";
        assert_eq!(
            b64url(&Sha256::digest(verifier.as_bytes())),
            "E9Melhoa2OwvFrEMTJguCHaoeK1t8URWbuGJSstw-cM"
        );
    }

    #[test]
    fn allowed_domains_are_parsed_forgivingly() {
        let fields = BTreeMap::from([(
            "allowed_domains".to_string(),
            " Example.com, @other.test ,,".to_string(),
        )]);
        let parsed: Vec<String> = field(&fields, "allowed_domains")
            .map(|d| {
                d.split(',')
                    .map(|s| s.trim().trim_start_matches('@').to_lowercase())
                    .filter(|s| !s.is_empty())
                    .collect()
            })
            .unwrap_or_default();
        assert_eq!(parsed, vec!["example.com", "other.test"]);
    }
}
