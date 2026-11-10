//! **Screenshots of the UI a run changed**, kept after the run and shown on its
//! page.
//!
//! Every agent step that can drive a browser is told to screenshot the pages its
//! work changes, into `$ARTIFACTS_DIR/screenshots/` with a `screenshots.json`
//! beside them (see `harness_runner::local`). That folder is in the run's
//! worktree, which is deleted when the run ends, so [`keep`] copies it to the
//! persistent volume first: images only, and a bounded number of them.
//!
//! They are evidence for a reviewer, not a record, so they expire.
//! [`spawn_sweeper`] deletes a run's images once they are older than
//! `HARNESS_SCREENSHOT_TTL_DAYS` (default 7) and leaves a marker, so the run's
//! page can say they were deleted rather than imply there never were any.

use std::path::{Component, Path, PathBuf};
use std::sync::Arc;

use axum::extract::{Extension, Path as AxumPath};
use axum::http::{header, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::Json;
use serde::{Deserialize, Serialize};
use serde_json::json;

use super::runs_routes::RunsState;

/// Days a run's screenshots are kept. `0` keeps them forever.
pub(crate) const TTL_DAYS_DEFAULT: u64 = 7;

/// At most this many images are kept from one run…
const MAX_FILES: usize = 40;
/// …and at most this many bytes of them.
const MAX_BYTES: u64 = 50 * 1024 * 1024;

const IMAGE_EXTENSIONS: &[&str] = &["png", "jpg", "jpeg", "webp"];

/// Left in a run's folder when its images expire: how many there were.
const EXPIRED_MARKER: &str = "expired.json";

/// One screenshot, as the agents describe it in `screenshots.json`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub(crate) struct Shot {
    pub file: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub page: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub viewport: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
}

/// `HARNESS_SCREENSHOT_TTL_DAYS`, or the default.
pub(crate) fn ttl_days() -> u64 {
    std::env::var("HARNESS_SCREENSHOT_TTL_DAYS")
        .ok()
        .and_then(|v| v.trim().parse().ok())
        .unwrap_or(TTL_DAYS_DEFAULT)
}

/// Where every run's kept screenshots live (`<projects_dir>/.run-screenshots`).
pub(crate) fn root(projects_dir: &Path) -> PathBuf {
    projects_dir.join(".run-screenshots")
}

/// One run's folder under `root`, or `None` for an id that is not a single
/// plain path component.
fn run_dir(root: &Path, run_id: &str) -> Option<PathBuf> {
    plain_name(run_id).then(|| root.join(run_id))
}

/// A single, ordinary file or folder name: no separators, no `..`.
fn plain_name(name: &str) -> bool {
    let mut parts = Path::new(name).components();
    matches!(parts.next(), Some(Component::Normal(_))) && parts.next().is_none()
}

fn is_image(name: &str) -> bool {
    Path::new(name)
        .extension()
        .and_then(|e| e.to_str())
        .is_some_and(|e| IMAGE_EXTENSIONS.contains(&e.to_ascii_lowercase().as_str()))
}

/// Copy a finished run's screenshots out of its worktree. Returns how many
/// were kept. Images only, in name order, up to [`MAX_FILES`] / [`MAX_BYTES`].
///
/// The manifest is kept for the images that were, with a later entry for the
/// same file replacing an earlier one. An image the agents never described is
/// listed by its file name alone, so nothing kept is invisible.
pub(crate) fn keep(artifacts: &Path, root: &Path, run_id: &str) -> usize {
    let src = artifacts.join(harness_runner::SCREENSHOTS_DIR);
    let Ok(entries) = std::fs::read_dir(&src) else {
        return 0;
    };
    let mut images: Vec<(String, u64)> = entries
        .flatten()
        .filter(|e| e.file_type().is_ok_and(|t| t.is_file()))
        .filter_map(|e| {
            let name = e.file_name().to_str()?.to_string();
            let size = e.metadata().ok()?.len();
            (is_image(&name) && plain_name(&name)).then_some((name, size))
        })
        .collect();
    if images.is_empty() {
        return 0;
    }
    images.sort();
    let Some(dest) = run_dir(root, run_id) else {
        return 0;
    };
    if std::fs::create_dir_all(&dest).is_err() {
        return 0;
    }

    let mut kept: Vec<String> = Vec::new();
    let mut bytes = 0u64;
    for (name, size) in images {
        if kept.len() >= MAX_FILES || bytes + size > MAX_BYTES {
            break;
        }
        if std::fs::copy(src.join(&name), dest.join(&name)).is_ok() {
            bytes += size;
            kept.push(name);
        }
    }

    let described: Vec<Shot> = std::fs::read(src.join(harness_runner::SCREENSHOTS_MANIFEST))
        .ok()
        .and_then(|b| serde_json::from_slice(&b).ok())
        .unwrap_or_default();
    let manifest = manifest_for(&kept, described);
    let _ = std::fs::write(
        dest.join(harness_runner::SCREENSHOTS_MANIFEST),
        serde_json::to_vec_pretty(&manifest).unwrap_or_default(),
    );
    kept.len()
}

/// The kept files, described where the agents described them.
fn manifest_for(kept: &[String], described: Vec<Shot>) -> Vec<Shot> {
    let mut out: Vec<Shot> = Vec::new();
    for shot in described {
        // A path the agent wrote out in full still names the file it saved.
        let file = Path::new(&shot.file)
            .file_name()
            .and_then(|f| f.to_str())
            .unwrap_or_default()
            .to_string();
        if !kept.contains(&file) {
            continue;
        }
        out.retain(|s| s.file != file);
        out.push(Shot { file, ..shot });
    }
    for file in kept {
        if !out.iter().any(|s| &s.file == file) {
            out.push(Shot {
                file: file.clone(),
                page: None,
                viewport: None,
                note: None,
            });
        }
    }
    out
}

/// Delete the images of every run kept longer than `ttl_days`, leaving a
/// marker. Returns how many runs were swept. `0` days keeps everything.
pub(crate) fn sweep(root: &Path, ttl_days: u64) -> usize {
    if ttl_days == 0 {
        return 0;
    }
    let ttl = std::time::Duration::from_secs(ttl_days.saturating_mul(24 * 60 * 60));
    let Ok(runs) = std::fs::read_dir(root) else {
        return 0;
    };
    let mut swept = 0;
    for run in runs.flatten() {
        let dir = run.path();
        if !dir.is_dir() || dir.join(EXPIRED_MARKER).exists() {
            continue;
        }
        // Dated by the manifest, which `keep` writes once as the run ends.
        let manifest = dir.join(harness_runner::SCREENSHOTS_MANIFEST);
        let age = std::fs::metadata(&manifest)
            .or_else(|_| std::fs::metadata(&dir))
            .and_then(|m| m.modified())
            .ok()
            .and_then(|t| t.elapsed().ok());
        if age.is_none_or(|a| a < ttl) {
            continue;
        }
        let count = shots(&dir).len();
        let Ok(files) = std::fs::read_dir(&dir) else {
            continue;
        };
        for f in files.flatten() {
            let _ = std::fs::remove_file(f.path());
        }
        let _ = std::fs::write(
            dir.join(EXPIRED_MARKER),
            json!({ "count": count, "ttl_days": ttl_days }).to_string(),
        );
        swept += 1;
    }
    swept
}

fn shots(dir: &Path) -> Vec<Shot> {
    std::fs::read(dir.join(harness_runner::SCREENSHOTS_MANIFEST))
        .ok()
        .and_then(|b| serde_json::from_slice(&b).ok())
        .unwrap_or_default()
}

/// Sweep once an hour. Unlike the cache sweeper this runs while runs are live:
/// it only ever touches folders of runs that finished days ago.
pub(crate) fn spawn_sweeper(state: Arc<RunsState>) {
    if tokio::runtime::Handle::try_current().is_err() {
        return;
    }
    let root = root(&state.projects_dir);
    tokio::spawn(async move {
        let mut tick = tokio::time::interval(std::time::Duration::from_secs(60 * 60));
        tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        loop {
            tick.tick().await;
            let root = root.clone();
            let ttl = ttl_days();
            match tokio::task::spawn_blocking(move || sweep(&root, ttl)).await {
                Ok(0) => {}
                Ok(n) => tracing::info!("screenshot sweeper: expired {n} run(s) older than {ttl}d"),
                Err(e) => tracing::warn!("screenshot sweeper: join error: {e}"),
            }
        }
    });
}

/// `GET /api/runs/{id}/screenshots` — `{ screenshots, expired, ttl_days }`.
/// `expired` is the number of screenshots that were deleted, or absent.
pub async fn list_screenshots(
    Extension(state): Extension<Arc<RunsState>>,
    AxumPath(id): AxumPath<String>,
) -> Response {
    let ttl = ttl_days();
    let Some(dir) = run_dir(&root(&state.projects_dir), &id) else {
        return (StatusCode::BAD_REQUEST, "bad run id").into_response();
    };
    let expired = std::fs::read(dir.join(EXPIRED_MARKER))
        .ok()
        .and_then(|b| serde_json::from_slice::<serde_json::Value>(&b).ok())
        .and_then(|v| v["count"].as_u64());
    Json(json!({
        "screenshots": if expired.is_some() { Vec::new() } else { shots(&dir) },
        "expired": expired,
        "ttl_days": ttl,
    }))
    .into_response()
}

/// `GET /api/runs/{id}/screenshots/{file}` — one kept image.
pub async fn get_screenshot(
    Extension(state): Extension<Arc<RunsState>>,
    AxumPath((id, file)): AxumPath<(String, String)>,
) -> Response {
    if !plain_name(&file) || !is_image(&file) {
        return (StatusCode::BAD_REQUEST, "not a screenshot").into_response();
    }
    let Some(dir) = run_dir(&root(&state.projects_dir), &id) else {
        return (StatusCode::BAD_REQUEST, "bad run id").into_response();
    };
    match tokio::fs::read(dir.join(&file)).await {
        Ok(bytes) => {
            let ext = file.rsplit('.').next().unwrap_or("").to_ascii_lowercase();
            let mime = match ext.as_str() {
                "jpg" | "jpeg" => "image/jpeg",
                "webp" => "image/webp",
                _ => "image/png",
            };
            (
                [
                    (header::CONTENT_TYPE, mime),
                    (header::CACHE_CONTROL, "private, max-age=3600"),
                ],
                bytes,
            )
                .into_response()
        }
        Err(_) => (StatusCode::NOT_FOUND, "no such screenshot").into_response(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write(dir: &Path, name: &str, bytes: &[u8]) {
        std::fs::create_dir_all(dir).unwrap();
        std::fs::write(dir.join(name), bytes).unwrap();
    }

    #[test]
    fn a_runs_images_and_their_descriptions_are_kept() {
        let tmp = tempfile::tempdir().unwrap();
        let artifacts = tmp.path().join("artifacts");
        let shots_dir = artifacts.join("screenshots");
        write(&shots_dir, "pdp-desktop.png", b"png");
        write(&shots_dir, "pdp-mobile.png", b"png");
        write(&shots_dir, "notes.txt", b"not an image");
        write(
            &shots_dir,
            "screenshots.json",
            br#"[
              {"file": "pdp-desktop.png", "page": "/p/1", "viewport": "desktop", "note": "old"},
              {"file": "/abs/path/pdp-desktop.png", "page": "/p/1", "viewport": "desktop", "note": "new"},
              {"file": "gone.png", "note": "never saved"}
            ]"#,
        );
        let root = tmp.path().join("kept");

        assert_eq!(keep(&artifacts, &root, "task-1"), 2);
        let kept = shots(&root.join("task-1"));
        assert_eq!(kept.len(), 2, "{kept:?}");
        assert_eq!(
            kept[0].note.as_deref(),
            Some("new"),
            "a later entry replaces"
        );
        assert_eq!(
            kept[1].file, "pdp-mobile.png",
            "undescribed images still show"
        );
        assert!(!root.join("task-1").join("notes.txt").exists());
    }

    #[test]
    fn a_run_without_screenshots_keeps_nothing() {
        let tmp = tempfile::tempdir().unwrap();
        assert_eq!(keep(tmp.path(), &tmp.path().join("kept"), "r"), 0);
        assert!(!tmp.path().join("kept").exists());
    }

    #[test]
    fn names_that_climb_out_are_refused() {
        assert!(plain_name("task-1") && plain_name("pdp.png"));
        for bad in ["../x", "a/b", "/etc/passwd", "..", ""] {
            assert!(!plain_name(bad), "{bad}");
        }
        assert!(run_dir(Path::new("/r"), "../other").is_none());
        assert!(is_image("A.PNG") && !is_image("x.svg") && !is_image("x"));
    }

    /// Past the TTL the images go and a marker stays; within it, nothing moves.
    #[test]
    fn screenshots_expire_and_say_so() {
        let tmp = tempfile::tempdir().unwrap();
        let run = tmp.path().join("task-1");
        write(&run, "a.png", b"png");
        write(&run, "screenshots.json", br#"[{"file": "a.png"}]"#);

        assert_eq!(sweep(tmp.path(), 7), 0, "a fresh run is kept");
        assert_eq!(sweep(tmp.path(), 0), 0, "0 keeps forever");

        let old = std::time::SystemTime::now() - std::time::Duration::from_secs(8 * 86_400);
        std::fs::File::options()
            .write(true)
            .open(run.join("screenshots.json"))
            .unwrap()
            .set_modified(old)
            .unwrap();
        assert_eq!(sweep(tmp.path(), 7), 1);
        assert!(!run.join("a.png").exists());
        let marker: serde_json::Value =
            serde_json::from_slice(&std::fs::read(run.join(EXPIRED_MARKER)).unwrap()).unwrap();
        assert_eq!(marker["count"], 1);
        assert_eq!(sweep(tmp.path(), 7), 0, "an expired run is not swept twice");
    }
}
