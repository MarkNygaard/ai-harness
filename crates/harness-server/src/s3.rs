//! **Uploading to S3 from a workflow run**, without the run holding a key.
//!
//! The server holds the access key. A run asks it — with its run grant, like
//! `harness linear` — for a short-lived presigned `PUT` URL for one object, and
//! uploads the file to it directly. The key never enters the run's
//! environment, and a large export never passes through the server.
//!
//! The signing is AWS Signature Version 4 in query-string form ("presigned
//! URL"), implemented here over `hmac`/`sha2` rather than pulling in the AWS
//! SDK for one request type. It also works against S3-compatible stores
//! (Cloudflare R2, MinIO) through a custom endpoint, addressed path-style.

use chrono::{DateTime, Utc};
use hmac::{Hmac, Mac};
use sha2::{Digest, Sha256};

/// How long a presigned URL can be used to *start* an upload. S3 checks expiry
/// when the request arrives, so a long upload that started in time completes.
pub const PRESIGN_SECONDS: u64 = 15 * 60;

/// Longest object key S3 accepts, in bytes.
const MAX_KEY_BYTES: usize = 1024;

/// Where uploads go and what signs them: the global S3 connection, with a
/// project's own bucket and folder applied.
#[derive(Clone)]
pub struct S3Target {
    pub access_key_id: String,
    pub secret_access_key: String,
    pub region: String,
    pub bucket: String,
    /// Every object this project uploads is under this prefix (no slashes at
    /// either end). Empty = the bucket root.
    pub folder: String,
    /// A custom endpoint (`https://<account>.r2.cloudflarestorage.com`), or
    /// `None` for AWS itself.
    pub endpoint: Option<String>,
}

impl std::fmt::Debug for S3Target {
    // The secret stays out of logs and panics.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("S3Target")
            .field("region", &self.region)
            .field("bucket", &self.bucket)
            .field("folder", &self.folder)
            .field("endpoint", &self.endpoint)
            .finish_non_exhaustive()
    }
}

/// Trim a folder to `a/b`: no leading or trailing slashes, no empty segments.
pub fn normalize_folder(folder: &str) -> String {
    folder
        .split('/')
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .collect::<Vec<_>>()
        .join("/")
}

/// The object key for an upload: `name` (a key relative to the folder, as the
/// run asked) under the target's folder.
///
/// Refuses anything that could step outside the folder or that S3 would
/// mangle: `..` or `.` segments, backslashes, control characters, an empty
/// name, or a key past S3's limit. The folder is the boundary a run is held
/// to, so this is the check that holds it.
pub fn object_key(folder: &str, name: &str) -> Result<String, String> {
    let name = name.trim().trim_start_matches('/');
    if name.is_empty() {
        return Err("the object needs a name".into());
    }
    if name.contains('\\') || name.chars().any(char::is_control) {
        return Err(format!("`{name}` is not a usable object name"));
    }
    if name
        .split('/')
        .any(|s| s == ".." || s == "." || s.is_empty())
    {
        return Err(format!(
            "`{name}` must stay inside the folder: no `..`, `.` or empty segments"
        ));
    }
    let key = if folder.is_empty() {
        name.to_string()
    } else {
        format!("{folder}/{name}")
    };
    if key.len() > MAX_KEY_BYTES {
        return Err(format!(
            "the object key is longer than {MAX_KEY_BYTES} bytes"
        ));
    }
    Ok(key)
}

/// The default name for an upload: the date, then the file's own name — so a
/// nightly export lands as `2026-10-24/production.tar.gz`.
pub fn default_name(file_name: &str, today: DateTime<Utc>) -> String {
    format!("{}/{file_name}", today.format("%Y-%m-%d"))
}

/// Percent-encode as SigV4 wants: everything except `A-Z a-z 0-9 - _ . ~`,
/// and `/` too unless `keep_slash`.
fn uri_encode(s: &str, keep_slash: bool) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.bytes() {
        let unreserved = b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.' | b'~');
        if unreserved || (keep_slash && b == b'/') {
            out.push(b as char);
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

fn hmac(key: &[u8], data: &str) -> Vec<u8> {
    let mut mac = Hmac::<Sha256>::new_from_slice(key).expect("HMAC takes a key of any length");
    mac.update(data.as_bytes());
    mac.finalize().into_bytes().to_vec()
}

/// The host and path an object is addressed at.
///
/// AWS uses virtual-hosted style (`<bucket>.s3.<region>.amazonaws.com/<key>`);
/// a custom endpoint uses path style (`<endpoint>/<bucket>/<key>`), which is
/// what R2 and MinIO accept everywhere.
fn address(target: &S3Target, key: &str) -> (String, String, String) {
    match &target.endpoint {
        Some(endpoint) => {
            let endpoint = endpoint.trim().trim_end_matches('/');
            let (scheme, host) = endpoint.split_once("://").unwrap_or(("https", endpoint));
            (
                scheme.to_string(),
                host.to_string(),
                format!(
                    "/{}/{}",
                    uri_encode(&target.bucket, false),
                    uri_encode(key, true)
                ),
            )
        }
        None => (
            "https".to_string(),
            format!("{}.s3.{}.amazonaws.com", target.bucket, target.region),
            format!("/{}", uri_encode(key, true)),
        ),
    }
}

/// A presigned URL for `method` on `host` + `path` (already encoded).
fn presign_with(
    target: &S3Target,
    method: &str,
    scheme: &str,
    host: &str,
    path: &str,
    now: DateTime<Utc>,
    expires: u64,
) -> String {
    let date = now.format("%Y%m%d").to_string();
    let stamp = now.format("%Y%m%dT%H%M%SZ").to_string();
    let scope = format!("{date}/{}/s3/aws4_request", target.region);
    let credential = format!("{}/{scope}", target.access_key_id);
    // Sorted by name, as the canonical request requires.
    let query = [
        ("X-Amz-Algorithm", "AWS4-HMAC-SHA256".to_string()),
        ("X-Amz-Credential", credential),
        ("X-Amz-Date", stamp.clone()),
        ("X-Amz-Expires", expires.to_string()),
        ("X-Amz-SignedHeaders", "host".to_string()),
    ]
    .iter()
    .map(|(k, v)| format!("{k}={}", uri_encode(v, false)))
    .collect::<Vec<_>>()
    .join("&");
    let canonical = format!("{method}\n{path}\n{query}\nhost:{host}\n\nhost\nUNSIGNED-PAYLOAD");
    let to_sign = format!(
        "AWS4-HMAC-SHA256\n{stamp}\n{scope}\n{}",
        hex(&Sha256::digest(canonical.as_bytes()))
    );
    let key = [date.as_str(), target.region.as_str(), "s3", "aws4_request"]
        .iter()
        .fold(
            format!("AWS4{}", target.secret_access_key).into_bytes(),
            |k, part| hmac(&k, part),
        );
    let signature = hex(&hmac(&key, &to_sign));
    format!("{scheme}://{host}{path}?{query}&X-Amz-Signature={signature}")
}

/// A URL that lets whoever holds it `PUT` one object at `key`, for
/// [`PRESIGN_SECONDS`].
pub fn presign_put(target: &S3Target, key: &str, now: DateTime<Utc>) -> String {
    let (scheme, host, path) = address(target, key);
    presign_with(target, "PUT", &scheme, &host, &path, now, PRESIGN_SECONDS)
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    fn target() -> S3Target {
        S3Target {
            access_key_id: "AKIAIOSFODNN7EXAMPLE".into(),
            secret_access_key: "wJalrXUtnFEMI/K7MDENG/bPxRfiCYEXAMPLEKEY".into(),
            region: "us-east-1".into(),
            bucket: "examplebucket".into(),
            folder: String::new(),
            endpoint: None,
        }
    }

    /// AWS's own worked example of a presigned URL ("Authenticating Requests:
    /// Using Query Parameters", Signature Version 4): a GET of `test.txt`.
    #[test]
    fn signs_like_the_aws_documentation_example() {
        let now = Utc.with_ymd_and_hms(2013, 5, 24, 0, 0, 0).unwrap();
        let url = presign_with(
            &target(),
            "GET",
            "https",
            "examplebucket.s3.amazonaws.com",
            "/test.txt",
            now,
            86400,
        );
        assert!(
            url.ends_with(
                "X-Amz-Signature=aeeed9bbccd4d02ee5c0109b86d86835f995330da4c265957d157751f604d404"
            ),
            "{url}"
        );
        assert!(url.contains(
            "X-Amz-Credential=AKIAIOSFODNN7EXAMPLE%2F20130524%2Fus-east-1%2Fs3%2Faws4_request"
        ));
    }

    #[test]
    fn aws_is_virtual_hosted_and_a_custom_endpoint_is_path_style() {
        let now = Utc.with_ymd_and_hms(2026, 10, 24, 2, 0, 0).unwrap();
        let mut t = target();
        t.region = "eu-north-1".into();
        let aws = presign_put(&t, "exports/2026-10-24/production.tar.gz", now);
        assert!(aws.starts_with(
            "https://examplebucket.s3.eu-north-1.amazonaws.com/exports/2026-10-24/production.tar.gz?"
        ));
        t.endpoint = Some("https://acct.r2.cloudflarestorage.com/".into());
        let r2 = presign_put(&t, "a b.txt", now);
        assert!(r2.starts_with("https://acct.r2.cloudflarestorage.com/examplebucket/a%20b.txt?"));
        assert!(r2.contains("X-Amz-Expires=900"));
    }

    #[test]
    fn keys_stay_inside_the_folder() {
        assert_eq!(
            object_key("backups/sanity", "2026-10-24/production.tar.gz").unwrap(),
            "backups/sanity/2026-10-24/production.tar.gz"
        );
        assert_eq!(object_key("", "/x.tar.gz").unwrap(), "x.tar.gz");
        for bad in ["../x", "a/../../x", "a//b", "./x", "a\\b", "", "  "] {
            assert!(object_key("backups", bad).is_err(), "{bad:?} was accepted");
        }
    }

    #[test]
    fn folders_are_normalized_and_names_dated() {
        assert_eq!(normalize_folder("/backups//sanity/ "), "backups/sanity");
        assert_eq!(normalize_folder(""), "");
        let today = Utc.with_ymd_and_hms(2026, 10, 24, 2, 0, 0).unwrap();
        assert_eq!(
            default_name("production.tar.gz", today),
            "2026-10-24/production.tar.gz"
        );
    }

    #[test]
    fn the_secret_is_not_debug_printed() {
        assert!(!format!("{:?}", target()).contains("EXAMPLEKEY"));
    }
}
