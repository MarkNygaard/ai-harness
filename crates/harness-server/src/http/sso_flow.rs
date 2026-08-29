//! The parts of a browser sign-in that every provider shares.
//!
//! Two providers now speak this: [`super::oidc`] (Entra, Google, Okta,
//! Keycloak, Authentik) and [`super::github_sso`]. What they have in common is
//! not the protocol — GitHub's OAuth app flow has neither PKCE nor an ID token
//! — but the *shape* of the round trip and, more importantly, the three things
//! that make it safe:
//!
//!   * a single-use, expiring `state`, which is what authenticates an
//!     otherwise-unauthenticated callback;
//!   * a cookie tying that state to the browser that started the flow, without
//!     which an attacker's `state` would seat an attacker's session in
//!     somebody else's browser;
//!   * a post-sign-in destination that cannot leave this origin.
//!
//! Those live here rather than in each provider because a second copy is a
//! second thing to get right, and the copy that drifts is the one nobody is
//! looking at.

use std::collections::HashMap;
use std::sync::LazyLock;

use axum::http::{header, HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::Json;
use serde_json::json;
use sha2::{Digest, Sha256};

/// How long an unused authorization attempt stays valid.
pub(crate) const PENDING_TTL_MS: i64 = 10 * 60 * 1000;

/// Which provider an attempt belongs to.
///
/// Checked when the state is redeemed: the two callbacks share one map, and a
/// state minted for one provider must not be spendable at the other's, where
/// it would be handed to a different token exchange entirely.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Provider {
    Oidc,
    GitHub,
}

pub(crate) fn err(status: StatusCode, msg: impl Into<String>) -> Response {
    (status, Json(json!({ "error": msg.into() }))).into_response()
}

pub(crate) fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

/// Percent-encode for a query string.
pub(crate) fn enc(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(b as char)
            }
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

/// 256 bits, from two v4 UUIDs — the trick the rest of this crate uses to keep
/// `rand` out of its dependency list.
pub(crate) fn random_token() -> String {
    format!(
        "{}{}",
        uuid::Uuid::new_v4().simple(),
        uuid::Uuid::new_v4().simple()
    )
}

pub(crate) fn hash(value: &str) -> String {
    format!("{:x}", Sha256::digest(value.as_bytes()))
}

// ── What is in flight ────────────────────────────────────────────────────────

/// One authorization attempt.
pub(crate) struct Pending {
    created_at: i64,
    pub(crate) provider: Provider,
    /// PKCE verifier. OIDC only — GitHub's OAuth app flow has no PKCE.
    pub(crate) verifier: Option<String>,
    /// The value the ID token must carry back. OIDC only.
    pub(crate) nonce: Option<String>,
    pub(crate) next: String,
    /// A test proves the configuration works without signing anybody in.
    pub(crate) test: bool,
    /// Hash of the value in the browser's binding cookie.
    ///
    /// **This is what ties the flow to a user agent.** Without it, a `state`
    /// the attacker minted for their own identity would authenticate the
    /// attacker's flow inside somebody else's browser. Hashed rather than
    /// stored, so this map never holds a live credential.
    binding_hash: String,
}

/// In-process: the harness is one container, and the callback lands on the
/// instance that issued the state. A restart mid-flow means clicking again.
static PENDING: LazyLock<std::sync::Mutex<HashMap<String, Pending>>> =
    LazyLock::new(|| std::sync::Mutex::new(HashMap::new()));

/// What a provider needs to record before sending the browser away.
pub(crate) struct Attempt {
    pub provider: Provider,
    pub verifier: Option<String>,
    pub nonce: Option<String>,
    pub next: String,
    pub test: bool,
    pub binding_hash: String,
}

/// Mint a `state`, pruning expired attempts.
pub(crate) fn issue_state(attempt: Attempt) -> String {
    let state = random_token();
    let now = now_ms();
    if let Ok(mut map) = PENDING.lock() {
        map.retain(|_, p| now - p.created_at < PENDING_TTL_MS);
        map.insert(
            state.clone(),
            Pending {
                created_at: now,
                provider: attempt.provider,
                verifier: attempt.verifier,
                nonce: attempt.nonce,
                next: attempt.next,
                test: attempt.test,
                binding_hash: attempt.binding_hash,
            },
        );
    }
    state
}

/// Consume a `state` for `provider`.
///
/// `None` if unknown, already spent, expired, or minted for the other provider
/// — all of which must fail the callback, because this is half of what
/// authenticates it.
pub(crate) fn take_state(state: &str, provider: Provider) -> Option<Pending> {
    let mut map = PENDING.lock().ok()?;
    // Peek before removing: a state for the wrong provider is somebody else's
    // live attempt, and spending it here would break their sign-in.
    if map.get(state)?.provider != provider {
        return None;
    }
    let pending = map.remove(state)?;
    (now_ms() - pending.created_at < PENDING_TTL_MS).then_some(pending)
}

// ── Binding one attempt to one browser ───────────────────────────────────────

const BINDING_COOKIE: &str = "harness_sso";

/// Where the cookie is sent. Narrow, because it is only ever read by a
/// callback.
const BINDING_PATH: &str = "/api/auth";

pub(crate) fn binding_cookie(value: &str, secure: bool) -> String {
    let max_age = PENDING_TTL_MS / 1000;
    format!(
        "{BINDING_COOKIE}={value}; Path={BINDING_PATH}; HttpOnly; SameSite=Lax; \
         Max-Age={max_age}{}",
        if secure { "; Secure" } else { "" }
    )
}

pub(crate) fn clear_binding_cookie(secure: bool) -> String {
    format!(
        "{BINDING_COOKIE}=; Path={BINDING_PATH}; HttpOnly; SameSite=Lax; Max-Age=0{}",
        if secure { "; Secure" } else { "" }
    )
}

/// Read it back off the request.
pub(crate) fn binding_from(headers: &HeaderMap) -> Option<String> {
    headers
        .get(header::COOKIE)
        .and_then(|v| v.to_str().ok())?
        .split(';')
        .map(str::trim)
        .find_map(|pair| pair.strip_prefix(&format!("{BINDING_COOKIE}=")))
        .filter(|v| !v.is_empty())
        .map(str::to_string)
}

/// Whether this request came from the browser that started `pending`.
pub(crate) fn binding_matches(pending: &Pending, headers: &HeaderMap) -> bool {
    match binding_from(headers) {
        Some(value) => hash(&value) == pending.binding_hash,
        None => false,
    }
}

// ── Coming back ──────────────────────────────────────────────────────────────

/// Where to send the browser after signing in.
///
/// **Only a same-origin path.** An absolute URL here would make the sign-in
/// page a phishing hop: a link that authenticates against the real harness and
/// then lands somewhere else entirely.
pub(crate) fn safe_next(raw: Option<&str>) -> String {
    let candidate = raw.unwrap_or("/").trim();

    // Refusing only `//` is not enough, because the browser's parser is more
    // forgiving than a prefix check:
    //
    //   * a backslash is equivalent to a forward slash in the relative states
    //     of a special scheme, so `/\evil.example` parses as `//evil.example`;
    //   * TAB, CR and LF are stripped *before* parsing, so `/<TAB>/evil.example`
    //     collapses to the same thing.
    //
    // Both would satisfy "starts with one slash, not two". Reject the
    // characters that make them possible rather than trying to out-guess the
    // parser.
    if candidate
        .bytes()
        .any(|b| b == b'\\' || b < 0x21 || b == 0x7f)
    {
        return "/".to_string();
    }

    // `//evil.example` is protocol-relative — a path by appearance and an
    // absolute URL by behaviour.
    if candidate.starts_with('/') && !candidate.starts_with("//") {
        candidate.to_string()
    } else {
        "/".to_string()
    }
}

/// How much of a refusal survives the trip back.
///
/// These messages exist to say what to do about the refusal, which takes a
/// sentence; the cap is only here to keep the address from growing without
/// bound.
const MAX_DETAIL: usize = 300;

/// Send the browser back with an outcome. Relative `Location`: it is already on
/// this origin.
pub(crate) fn back(to: &str, status: &str, detail: Option<&str>) -> Response {
    let sep = if to.contains('?') { '&' } else { '?' };
    let mut location = format!("{to}{sep}sso={}", enc(status));
    if let Some(d) = detail {
        location.push_str(&format!("&sso_message={}", enc(&shorten(d))));
    }
    (StatusCode::SEE_OTHER, [(header::LOCATION, location)]).into_response()
}

/// Cut an over-long message at a word boundary, marked as cut.
///
/// The previous version took the first 200 characters and stopped wherever
/// that landed, which in practice was the middle of the email address in the
/// closing sentence -- leaving the reader unable to tell a truncated address
/// from a wrong one. An ellipsis at least says the sentence continues.
fn shorten(detail: &str) -> String {
    if detail.chars().count() <= MAX_DETAIL {
        return detail.to_string();
    }
    let head: String = detail.chars().take(MAX_DETAIL).collect();
    // `rfind` gives a byte index, and a space is always a char boundary.
    let cut = head.rfind(' ').unwrap_or(head.len());
    format!("{}…", head[..cut].trim_end())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn attempt(provider: Provider, next: &str, test: bool, binding: &str) -> Attempt {
        Attempt {
            provider,
            verifier: Some("v".into()),
            nonce: Some("n".into()),
            next: next.into(),
            test,
            binding_hash: hash(binding),
        }
    }

    #[test]
    fn the_post_sign_in_destination_stays_on_this_origin() {
        // The whole point: a link that authenticates against the real harness
        // and then lands somewhere else is a phishing hop.
        for hostile in [
            "https://evil.example/steal",
            "http://evil.example",
            // Protocol-relative: a path by appearance, an absolute URL by
            // behaviour, and the one people forget.
            "//evil.example/steal",
            "///evil.example",
            // A backslash is a forward slash to the browser's URL parser, so
            // these are `//evil.example` by the time it matters.
            "/\\evil.example",
            "/\\/evil.example",
            "\\\\evil.example",
            // Stripped before parsing, collapsing to `//evil.example`.
            "/\t/evil.example",
            "/\r\n/evil.example",
            "javascript:alert(1)",
            "",
            "   ",
        ] {
            assert_eq!(
                safe_next(Some(hostile)),
                "/",
                "{hostile:?} should be refused"
            );
        }
        assert_eq!(safe_next(None), "/");

        for ok in ["/", "/runs", "/settings/sso", "/runs/abc?tab=graph"] {
            assert_eq!(safe_next(Some(ok)), ok, "{ok} should be kept");
        }
    }

    #[test]
    fn a_state_is_single_use_and_carries_its_attempt() {
        let state = issue_state(attempt(Provider::Oidc, "/runs", false, "b1"));
        let other = issue_state(attempt(Provider::GitHub, "/", true, "b2"));
        assert_ne!(state, other, "each attempt gets its own");

        let taken = take_state(&state, Provider::Oidc).expect("live");
        assert_eq!(taken.next, "/runs");
        assert!(!taken.test);
        assert_eq!(taken.verifier.as_deref(), Some("v"));

        // Replaying it must fail: this is what authenticates the callback.
        assert!(take_state(&state, Provider::Oidc).is_none());
        assert!(take_state("never-issued", Provider::Oidc).is_none());

        assert!(take_state(&other, Provider::GitHub).expect("live").test);
    }

    #[test]
    fn a_state_cannot_be_spent_at_the_other_providers_callback() {
        let state = issue_state(attempt(Provider::Oidc, "/", false, "b"));
        // Wrong provider: refused, and — importantly — not consumed, so the
        // real sign-in it belongs to still works.
        assert!(take_state(&state, Provider::GitHub).is_none());
        assert!(take_state(&state, Provider::Oidc).is_some());
    }

    #[test]
    fn an_expired_attempt_is_refused() {
        let state = issue_state(attempt(Provider::Oidc, "/", false, "b"));
        if let Ok(mut map) = PENDING.lock() {
            if let Some(p) = map.get_mut(&state) {
                p.created_at -= PENDING_TTL_MS + 1;
            }
        }
        assert!(take_state(&state, Provider::Oidc).is_none());
    }

    #[test]
    fn an_attempt_is_bound_to_the_browser_that_started_it() {
        // The property that stops login CSRF: a `state` alone is not enough,
        // because the attacker can mint one.
        let binding = random_token();
        let state = issue_state(attempt(Provider::Oidc, "/", false, &binding));
        let pending = take_state(&state, Provider::Oidc).expect("live");

        let mut headers = HeaderMap::new();
        assert!(!binding_matches(&pending, &headers), "no cookie, no match");

        headers.insert(
            header::COOKIE,
            format!("theme=dark; harness_sso={binding}; harness_session=x")
                .parse()
                .unwrap(),
        );
        assert!(binding_matches(&pending, &headers));

        headers.insert(
            header::COOKIE,
            format!("harness_sso={}", random_token()).parse().unwrap(),
        );
        assert!(
            !binding_matches(&pending, &headers),
            "somebody else's browser must not match"
        );

        // The map never holds the live value, only its hash.
        assert_ne!(pending.binding_hash, binding);
    }

    #[test]
    fn the_binding_cookie_is_scoped_and_clearable() {
        let secure = binding_cookie("abc123", true);
        assert!(secure.contains("harness_sso=abc123"));
        assert!(secure.contains("HttpOnly"), "{secure}");
        assert!(secure.contains("SameSite=Lax"), "{secure}");
        assert!(secure.contains("Path=/api/auth"), "{secure}");
        assert!(secure.contains("; Secure"), "{secure}");
        assert!(!binding_cookie("abc123", false).contains("; Secure"));
        assert!(clear_binding_cookie(true).contains("Max-Age=0"));

        // A cleared cookie carries no value, so it must not read as one.
        let mut headers = HeaderMap::new();
        headers.insert(header::COOKIE, "harness_sso=".parse().unwrap());
        assert_eq!(binding_from(&headers), None);
    }

    #[test]
    fn query_encoding_escapes_what_would_break_a_url() {
        assert_eq!(enc("aZ09-_.~"), "aZ09-_.~");
        assert_eq!(enc("openid email profile"), "openid%20email%20profile");
        assert_eq!(
            enc("https://h.example/api/auth/oidc/callback"),
            "https%3A%2F%2Fh.example%2Fapi%2Fauth%2Foidc%2Fcallback"
        );
        // An `&` that survived would let a redirect_uri smuggle a parameter.
        assert_eq!(enc("a&b=c"), "a%26b%3Dc");
    }

    #[test]
    fn a_short_message_is_left_alone() {
        assert_eq!(
            shorten("no account here uses a@b.test"),
            "no account here uses a@b.test"
        );
    }

    #[test]
    fn an_over_long_message_is_cut_between_words() {
        let long = format!("{} tail", "word ".repeat(80));
        let out = shorten(&long);
        assert!(out.ends_with('\u{2026}'), "{out}");
        // The cut lands on a boundary, so no half-word is left behind.
        assert!(!out.contains("wor\u{2026}"), "{out}");
        assert!(out.chars().count() <= MAX_DETAIL + 1);
    }

    #[test]
    fn the_refusal_that_prompted_this_now_fits_whole() {
        // The real one: a GitHub identity with no matching account. It was
        // being cut mid-address, which is what made it useless.
        let real = "no account here uses somebody@example.test, the address GitHub \
                    verified for @somebody. Ask an administrator to invite that \
                    address, or change the address on your account.";
        assert_eq!(shorten(real), real, "this message must survive intact");
    }
}
