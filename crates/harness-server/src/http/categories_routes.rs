//! Workflow step **category** registry API (global; seeded with planning /
//! implementation / validation). Categories group steps for the run overview's
//! time-by-category breakdown and bar colouring; a node references one by `id`.
//!
//! - `GET    /api/categories`      — list categories (ordinal order)
//! - `PUT    /api/categories/{id}` — create / update a category
//! - `DELETE /api/categories/{id}` — remove a category

use std::sync::Arc;

use axum::extract::{Extension, Path as AxumPath};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::Json;
use harness_persist::CategoryInput;
use serde::Deserialize;

use super::runs_routes::RunsState;

fn err(status: StatusCode, msg: impl Into<String>) -> Response {
    (status, Json(serde_json::json!({ "error": msg.into() }))).into_response()
}

/// A category id must be a safe slug (it's referenced from workflow YAML).
fn valid_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= 48
        && id
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
}

/// `GET /api/categories` — list all categories.
pub async fn list_categories(Extension(state): Extension<Arc<RunsState>>) -> Response {
    let store = match state.category_store().await {
        Ok(s) => s,
        Err(e) => return err(StatusCode::SERVICE_UNAVAILABLE, e),
    };
    match store.list().await {
        Ok(cats) => Json(cats).into_response(),
        Err(e) => err(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()),
    }
}

#[derive(Debug, Deserialize)]
pub struct SaveCategoryRequest {
    pub label: String,
    pub color: String,
    #[serde(default)]
    pub ordinal: i32,
}

/// `PUT /api/categories/{id}` — create or update a category.
pub async fn save_category(
    Extension(state): Extension<Arc<RunsState>>,
    AxumPath(id): AxumPath<String>,
    Json(req): Json<SaveCategoryRequest>,
) -> Response {
    if !valid_id(&id) {
        return err(
            StatusCode::BAD_REQUEST,
            "id must be 1–48 chars of [A-Za-z0-9_-]",
        );
    }
    if req.label.trim().is_empty() || req.color.trim().is_empty() {
        return err(StatusCode::BAD_REQUEST, "label and color are required");
    }
    let store = match state.category_store().await {
        Ok(s) => s,
        Err(e) => return err(StatusCode::SERVICE_UNAVAILABLE, e),
    };
    let input = CategoryInput {
        label: req.label.trim().to_string(),
        color: req.color.trim().to_string(),
        ordinal: req.ordinal,
    };
    match store.upsert(&id, &input).await {
        Ok(c) => Json(c).into_response(),
        Err(e) => err(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()),
    }
}

/// `DELETE /api/categories/{id}` — remove a category (nodes referencing it fall
/// back to status colour).
pub async fn delete_category(
    Extension(state): Extension<Arc<RunsState>>,
    AxumPath(id): AxumPath<String>,
) -> Response {
    let store = match state.category_store().await {
        Ok(s) => s,
        Err(e) => return err(StatusCode::SERVICE_UNAVAILABLE, e),
    };
    match store.delete(&id).await {
        Ok(()) => Json(serde_json::json!({ "deleted": true, "id": id })).into_response(),
        Err(e) => err(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()),
    }
}
