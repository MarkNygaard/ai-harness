use axum::http::{header, HeaderMap, StatusCode, Uri};
use axum::response::{Html, IntoResponse, Response};

pub async fn index() -> impl IntoResponse {
    Html(crate::assets::index_html())
}

/// SPA fallback: serve the React shell for unmatched **browser navigations**
/// (client-side routes like `/runs/{id}` and `/editor`) so deep links and hard
/// refreshes work. Non-HTML requests (API calls) fall through to a 404 so
/// mistyped endpoints don't silently return HTML.
pub async fn spa_fallback(uri: Uri, headers: HeaderMap) -> Response {
    let wants_html = headers
        .get(header::ACCEPT)
        .and_then(|v| v.to_str().ok())
        .is_some_and(|a| a.contains("text/html"));
    // API/asset namespaces are server-owned; an unmatched path there is a real
    // 404, never the SPA shell. (`/runs/{id}`, `/editor` etc. are client routes.)
    let path = uri.path();
    let is_server_path = path.starts_with("/api/") || path.starts_with("/assets/");
    if wants_html && !is_server_path {
        Html(crate::assets::index_html()).into_response()
    } else {
        (StatusCode::NOT_FOUND, "not found").into_response()
    }
}

pub async fn favicon() -> impl IntoResponse {
    (
        [(header::CONTENT_TYPE, "image/svg+xml")],
        r#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 100 100"><text y=".9em" font-size="90">&#9881;</text></svg>"#,
    )
}
