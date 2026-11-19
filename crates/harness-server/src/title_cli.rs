//! `harness title` — rename the run this step belongs to.
//!
//! A webhook or schedule names its run before the run knows what it will work
//! on; a step that does know can say so, and the runs list then shows it. Uses
//! the run's grant like `harness linear` (see [`crate::linear_cli`]), so it only
//! ever renames its own run.

/// Ask the server to rename this run. Returns the stored title as JSON.
pub async fn set(title: &str) -> Result<String, String> {
    crate::linear_cli::ask("/api/run/title", &serde_json::json!({ "title": title })).await
}
