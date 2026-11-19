//! **A run naming itself**, authorised by its run grant.
//!
//! A webhook or a schedule names its run before the run has found out what it
//! will work on: every conflict-resolving run was "push to main → resolve
//! conflicts — <time>", whichever PRs it fixed, or none. A step that knows can
//! now say so. The run is taken from the grant, so a run can rename only
//! itself.

use std::sync::Arc;

use axum::extract::Extension;
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::Json;
use serde::Deserialize;

use super::run_linear_routes::grant;
use super::runs_routes::RunsState;

/// Longer than this and the runs list cuts it anyway; a run that wants to say
/// more has its summary for that.
const TITLE_MAX_CHARS: usize = 200;

fn err(status: StatusCode, msg: impl Into<String>) -> Response {
    (status, Json(serde_json::json!({ "error": msg.into() }))).into_response()
}

#[derive(Debug, Deserialize)]
pub struct TitleRequest {
    pub title: String,
}

/// The title as it is stored: one line, and refused rather than cut when it is
/// empty or too long, so the step that set it sees why.
fn clean_title(raw: &str) -> Result<String, String> {
    let title = raw.split_whitespace().collect::<Vec<_>>().join(" ");
    if title.is_empty() {
        return Err("a run title needs some text".into());
    }
    let len = title.chars().count();
    if len > TITLE_MAX_CHARS {
        return Err(format!(
            "a run title is at most {TITLE_MAX_CHARS} characters and this one is {len}"
        ));
    }
    Ok(title)
}

/// `POST /api/run/title` — rename the run this grant belongs to.
pub async fn set_title(
    Extension(state): Extension<Arc<RunsState>>,
    headers: HeaderMap,
    Json(req): Json<TitleRequest>,
) -> Response {
    let g = match grant(&headers) {
        Ok(g) => g,
        Err((s, m)) => return err(s, m),
    };
    let title = match clean_title(&req.title) {
        Ok(t) => t,
        Err(m) => return err(StatusCode::BAD_REQUEST, m),
    };
    let store = match state.store().await {
        Ok(s) => s,
        Err(m) => return err(StatusCode::SERVICE_UNAVAILABLE, m),
    };
    match store.set_run_title(&g.run_id, &title).await {
        Ok(true) => Json(serde_json::json!({ "run_id": g.run_id, "title": title })).into_response(),
        Ok(false) => err(
            StatusCode::NOT_FOUND,
            format!("run {} is not recorded yet", g.run_id),
        ),
        Err(e) => err(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_title_is_one_line_and_refused_when_empty_or_too_long() {
        assert_eq!(
            clean_title("  Resolve conflicts:\n #1182,  #1171 ").unwrap(),
            "Resolve conflicts: #1182, #1171"
        );
        assert!(clean_title(" \n\t ").is_err());
        assert!(clean_title(&"x".repeat(TITLE_MAX_CHARS)).is_ok());
        let long = clean_title(&"x".repeat(TITLE_MAX_CHARS + 1)).unwrap_err();
        assert!(
            long.contains("at most 200 characters and this one is 201"),
            "{long}"
        );
    }
}
