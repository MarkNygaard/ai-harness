//! **S3 uploads a run may ask for**, authorised by its run grant.
//!
//! One operation: a presigned URL to `PUT` one object under the project's
//! folder. The run never holds the access key, can't read or delete anything,
//! and can't name a bucket or step outside its folder — see [`crate::s3`].

use std::sync::Arc;

use axum::extract::Extension;
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::Json;
use chrono::Utc;
use serde::Deserialize;

use super::run_linear_routes::grant;
use super::runs_routes::RunsState;

fn err(status: StatusCode, msg: impl Into<String>) -> Response {
    (status, Json(serde_json::json!({ "error": msg.into() }))).into_response()
}

#[derive(Debug, Deserialize)]
pub struct UploadUrlRequest {
    /// The object's name under the project's folder (`exports/today.tar.gz`).
    /// Omitted → the date and `file_name`.
    #[serde(default)]
    pub name: Option<String>,
    /// The local file's name, for the default.
    pub file_name: String,
}

/// `POST /api/run/s3/upload-url` — where to `PUT` one object, for a few
/// minutes.
pub async fn upload_url(
    Extension(state): Extension<Arc<RunsState>>,
    headers: HeaderMap,
    Json(req): Json<UploadUrlRequest>,
) -> Response {
    let g = match grant(&headers) {
        Ok(g) => g,
        Err((s, m)) => return err(s, m),
    };
    let target = match state.s3_target(&g.project).await {
        Ok(t) => t,
        Err(m) => return err(StatusCode::CONFLICT, m),
    };
    let now = Utc::now();
    let name = req
        .name
        .filter(|n| !n.trim().is_empty())
        .unwrap_or_else(|| crate::s3::default_name(req.file_name.trim(), now));
    let key = match crate::s3::object_key(&target.folder, &name) {
        Ok(k) => k,
        Err(m) => return err(StatusCode::BAD_REQUEST, m),
    };
    tracing::info!(
        run_id = %g.run_id,
        "s3: presigned an upload to s3://{}/{key}",
        target.bucket
    );
    Json(serde_json::json!({
        "url": crate::s3::presign_put(&target, &key, now),
        "bucket": target.bucket,
        "key": key,
    }))
    .into_response()
}
