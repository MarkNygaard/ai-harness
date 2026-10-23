//! `harness s3 upload` — put a file a workflow step produced into the
//! project's S3 folder.
//!
//! The run asks the server for a presigned URL with its grant (see
//! [`crate::linear_cli`] for the grant itself, and [`crate::s3`] for why the
//! run never holds the key), then streams the file straight to S3.

use std::path::Path;

use serde::Deserialize;

#[derive(Deserialize)]
struct UploadUrl {
    url: String,
    bucket: String,
    key: String,
}

/// Upload `file` as `name` (relative to the project's folder; default: the
/// date and the file's name). Returns the object as JSON.
pub async fn upload(file: &Path, name: Option<&str>) -> Result<String, String> {
    let meta = tokio::fs::metadata(file)
        .await
        .map_err(|e| format!("could not read {}: {e}", file.display()))?;
    if !meta.is_file() {
        return Err(format!("{} is not a file", file.display()));
    }
    let file_name = file
        .file_name()
        .and_then(|n| n.to_str())
        .ok_or_else(|| format!("{} has no usable file name", file.display()))?;

    let answer = crate::linear_cli::ask(
        "/api/run/s3/upload-url",
        &serde_json::json!({ "name": name, "file_name": file_name }),
    )
    .await?;
    let target: UploadUrl = serde_json::from_str(&answer)
        .map_err(|e| format!("unexpected answer from the harness: {e}"))?;

    let body = tokio::fs::File::open(file)
        .await
        .map_err(|e| format!("could not open {}: {e}", file.display()))?;
    // No overall timeout: an export can take a while to send. A stalled
    // connection still fails, on the connect and read timeouts.
    let client = reqwest::Client::builder()
        .connect_timeout(std::time::Duration::from_secs(30))
        .read_timeout(std::time::Duration::from_secs(300))
        .build()
        .map_err(|e| format!("http client: {e}"))?;
    let resp = client
        .put(&target.url)
        // S3 refuses a chunked upload to a presigned URL; it needs the length.
        .header(reqwest::header::CONTENT_LENGTH, meta.len())
        // Streamed from disk, not read into memory.
        .body(reqwest::Body::from(body))
        .send()
        .await
        .map_err(|e| {
            format!(
                "upload to s3://{}/{} failed: {e}",
                target.bucket, target.key
            )
        })?;
    let status = resp.status();
    if !status.is_success() {
        let detail = resp.text().await.unwrap_or_default();
        return Err(format!(
            "S3 refused the upload to s3://{}/{} ({status}): {}",
            target.bucket,
            target.key,
            s3_error_message(&detail)
        ));
    }
    Ok(serde_json::json!({
        "bucket": target.bucket,
        "key": target.key,
        "uri": format!("s3://{}/{}", target.bucket, target.key),
        "bytes": meta.len(),
    })
    .to_string())
}

/// The `<Message>` out of S3's XML error body, or the body itself — what a
/// person reading the run log needs ("Access Denied", "The specified bucket
/// does not exist").
fn s3_error_message(body: &str) -> String {
    let between = |tag: &str| {
        let open = format!("<{tag}>");
        let start = body.find(&open)? + open.len();
        let end = body[start..].find(&format!("</{tag}>"))? + start;
        Some(body[start..end].to_string())
    };
    match (between("Code"), between("Message")) {
        (Some(code), Some(msg)) => format!("{code}: {msg}"),
        (None, Some(msg)) => msg,
        _ => body.trim().chars().take(300).collect(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn s3_errors_are_read_out_of_the_xml() {
        let body = "<?xml version=\"1.0\"?><Error><Code>AccessDenied</Code>\
                    <Message>Access Denied</Message><RequestId>x</RequestId></Error>";
        assert_eq!(s3_error_message(body), "AccessDenied: Access Denied");
        assert_eq!(s3_error_message("bad gateway"), "bad gateway");
    }
}
