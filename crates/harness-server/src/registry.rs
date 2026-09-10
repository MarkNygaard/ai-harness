//! Client for the workflow **library** — the public registry of workflows a
//! harness can browse, install and later publish to.
//!
//! The registry is a separate service. This half is deliberately thin: it
//! fetches, it reports, and it has no opinions the registry does not already
//! enforce. Everything that must be true across all installs — who may publish a
//! version, which slugs are reserved, whether a YAML parses — is checked there,
//! because a client cannot enforce a rule against itself.
//!
//! **Every call is best-effort from the harness's point of view.** Installing a
//! workflow writes a file; telling the registry about it is bookkeeping. A
//! registry that is down, slow, or switched off must never stop somebody
//! installing or removing a workflow on their own machine, so the callers treat
//! a failure here as "no library today", not as an error worth failing on.

use std::time::Duration;

use serde::{Deserialize, Serialize};

/// How long to wait on the registry before giving up.
///
/// Short on purpose: this sits behind a dialog somebody just opened, and a
/// library that has not answered in five seconds is one they would rather be
/// told about than wait for.
const TIMEOUT: Duration = Duration::from_secs(5);

/// Identifies this harness to the registry, so an install can be counted once
/// and uncounted on removal. Opaque and per-install — not a user, not a
/// hostname. Stored in settings under this key.
pub const INSTALLATION_ID_KEY: &str = "registry_installation_id";

/// A workflow as the library lists it.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct LibraryWorkflow {
    pub slug: String,
    pub title: String,
    pub description: String,
    #[serde(default)]
    pub tags: Vec<String>,
    pub official: bool,
    /// The publisher's GitHub login.
    pub publisher: String,
    /// `None` for a workflow whose only versions have all been withdrawn.
    pub latest_version: Option<i32>,
    pub installs: i64,
    pub updated_at: String,
}

/// One published version's document.
#[derive(Debug, Clone, Deserialize)]
pub struct LibraryVersion {
    pub slug: String,
    pub version: i32,
    pub yaml: String,
    #[serde(default)]
    pub changelog: Option<String>,
    /// A withdrawn version is still served rather than removed, so an install
    /// holding it is told rather than broken.
    #[serde(default)]
    pub withdrawn: bool,
}

/// A failure talking to the library. Carries a sentence a person can act on;
/// the callers surface it rather than a status code.
#[derive(Debug)]
pub struct RegistryError(pub String);

impl std::fmt::Display for RegistryError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

type Result<T> = std::result::Result<T, RegistryError>;

/// Talks to one registry.
#[derive(Clone)]
pub struct RegistryClient {
    base: String,
    http: reqwest::Client,
}

impl RegistryClient {
    /// Build a client for `base`, or `None` when the library is switched off.
    ///
    /// An empty `registry_url` is how an air-gapped install disables the feature
    /// outright, so the absence of a client is a supported state rather than a
    /// misconfiguration — every caller treats it as "there is no library here".
    pub fn new(base: Option<&str>) -> Option<Self> {
        let base = base.map(str::trim).filter(|b| !b.is_empty())?;
        let http = reqwest::Client::builder().timeout(TIMEOUT).build().ok()?;
        Some(Self {
            base: base.trim_end_matches('/').to_string(),
            http,
        })
    }

    fn url(&self, path: &str) -> String {
        format!("{}{path}", self.base)
    }

    /// Everything listed, most-installed first — the registry's own default,
    /// which is what keeps a workflow nobody installs off the top of the list.
    pub async fn list(&self) -> Result<Vec<LibraryWorkflow>> {
        let resp = self
            .http
            .get(self.url("/v1/workflows"))
            .send()
            .await
            .map_err(|e| RegistryError(format!("could not reach the workflow library: {e}")))?;
        if !resp.status().is_success() {
            return Err(RegistryError(format!(
                "the workflow library answered {}",
                resp.status()
            )));
        }
        resp.json().await.map_err(|e| {
            RegistryError(format!(
                "the workflow library sent something unreadable: {e}"
            ))
        })
    }

    /// One version's YAML. `version` is the registry's per-workflow counter.
    pub async fn version(&self, slug: &str, version: i32) -> Result<LibraryVersion> {
        let path = format!("/v1/workflows/{}/versions/{version}", urlencode(slug));
        let resp = self
            .http
            .get(self.url(&path))
            .send()
            .await
            .map_err(|e| RegistryError(format!("could not reach the workflow library: {e}")))?;
        if resp.status() == reqwest::StatusCode::NOT_FOUND {
            return Err(RegistryError(format!(
                "the library has no version {version} of `{slug}`"
            )));
        }
        if !resp.status().is_success() {
            return Err(RegistryError(format!(
                "the workflow library answered {}",
                resp.status()
            )));
        }
        resp.json().await.map_err(|e| {
            RegistryError(format!(
                "the workflow library sent something unreadable: {e}"
            ))
        })
    }

    /// Say this harness installed a version, or is still holding one.
    ///
    /// Idempotent at the registry: a retry and a reinstall are the same row, and
    /// the call doubles as the heartbeat that keeps the install counted as live.
    pub async fn record_install(
        &self,
        slug: &str,
        installation_id: &str,
        version: i32,
    ) -> Result<()> {
        let path = format!("/v1/workflows/{}/installs", urlencode(slug));
        let resp = self
            .http
            .put(self.url(&path))
            .json(&serde_json::json!({
                "installation_id": installation_id,
                "version": version,
            }))
            .send()
            .await
            .map_err(|e| RegistryError(format!("could not reach the workflow library: {e}")))?;
        if !resp.status().is_success() {
            return Err(RegistryError(format!(
                "the workflow library answered {}",
                resp.status()
            )));
        }
        Ok(())
    }

    /// Say this harness no longer has it, so the count comes down.
    ///
    /// A `404` is success: the registry had nothing to forget, which is the
    /// state the caller wanted. Treating it as a failure would make an
    /// uninstall look broken for having already happened.
    pub async fn forget_install(&self, slug: &str, installation_id: &str) -> Result<()> {
        let path = format!(
            "/v1/workflows/{}/installs/{}",
            urlencode(slug),
            urlencode(installation_id)
        );
        let resp = self
            .http
            .delete(self.url(&path))
            .send()
            .await
            .map_err(|e| RegistryError(format!("could not reach the workflow library: {e}")))?;
        if resp.status().is_success() || resp.status() == reqwest::StatusCode::NOT_FOUND {
            return Ok(());
        }
        Err(RegistryError(format!(
            "the workflow library answered {}",
            resp.status()
        )))
    }
}

/// Percent-encode one path segment.
///
/// Slugs are validated hard by the registry — lowercase, digits and single
/// dashes — so in practice nothing here needs escaping. This is not for the
/// well-formed case: a slug arrives here from a listing the client did not
/// author, and a `../` reaching a URL join is exactly the shape of a path
/// traversal. Encoding it costs nothing and removes the question.
fn urlencode(segment: &str) -> String {
    let mut out = String::with_capacity(segment.len());
    for byte in segment.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(byte as char)
            }
            other => out.push_str(&format!("%{other:02X}")),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// An empty or missing URL is how the library is switched off, and both must
    /// produce "no library" rather than a client pointed at nothing.
    #[test]
    fn no_url_means_no_library() {
        assert!(RegistryClient::new(None).is_none());
        assert!(RegistryClient::new(Some("")).is_none());
        assert!(RegistryClient::new(Some("   ")).is_none());
        assert!(RegistryClient::new(Some("https://registry.example")).is_some());
    }

    /// A trailing slash in configuration must not become a double slash in every
    /// request path.
    #[test]
    fn a_trailing_slash_is_trimmed_once() {
        let client = RegistryClient::new(Some("https://registry.example/")).expect("client");
        assert_eq!(
            client.url("/v1/workflows"),
            "https://registry.example/v1/workflows"
        );
    }

    /// The point is the traversal case, not the pretty one: a slug is data from
    /// a listing, and `../` must never survive into a request path.
    #[test]
    fn a_path_segment_cannot_escape_its_position() {
        assert_eq!(urlencode("geo-audit-ecommerce"), "geo-audit-ecommerce");
        assert_eq!(urlencode("../admin"), "..%2Fadmin");
        assert_eq!(urlencode("a b"), "a%20b");
        assert_eq!(urlencode("a/b?c=d#e"), "a%2Fb%3Fc%3Dd%23e");
    }
}
