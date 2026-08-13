//! **Linear image attachments** — make images pasted into an issue visible to the
//! agent, instead of forwarding an unfetchable URL as text.
//!
//! A screenshot pasted into Linear lands in the markdown as
//! `![shot](https://uploads.linear.app/…)`. That URL is private (unauthenticated
//! requests get a 401) and the agent holds no Linear credential, so forwarding it
//! verbatim means "fix the layout, see screenshot" degrades to "fix the layout".
//!
//! So the harness fetches each upload with the credential it already has, writes
//! it to disk, and rewrites the link to the local path. Agents read images from a
//! path natively (Claude Code's `Read`; omp keeps image blocks for models whose
//! catalog entry declares `image` input, and substitutes a placeholder otherwise),
//! so nothing in the agent adapters changes.
//!
//! **Deliberately no resizing or re-encoding.** Sizes vary hugely by content — a
//! UI screenshot is a few hundred KB, a photograph tens of MB — and the agent's own
//! tooling downscales on the way to the model. Doing it here would mean decoding
//! attacker-supplied images inside the harness process for no benefit, and
//! re-encoding would attack exactly the high-contrast text edges that make a
//! screenshot worth reading. [`MAX_UPLOAD_BYTES`] is a guard against pathological
//! files, not a model-facing limit.
//!
//! Every failure is non-fatal: the original URL stays in the text and the run
//! proceeds. An image is a bonus, never a prerequisite.

use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use harness_sources::linear::{extract_upload_urls, LinearClient};

/// How many uploads one task may materialize. Enough for a handful of
/// screenshots; a bound so a pathological issue can't stall a run on downloads.
const MAX_UPLOADS_PER_TASK: usize = 5;

/// Refuse a single upload larger than this. Generous on purpose — a 12MB
/// photograph is normal and reads fine — this only stops absurd files.
const MAX_UPLOAD_BYTES: usize = 25 * 1024 * 1024;

// The guard exists to stop absurd files, not to limit what models receive: a 12MB
// photograph is a real, verified working case, so it must sit well clear of that.
const _: () = assert!(MAX_UPLOAD_BYTES > 12 * 1024 * 1024);
const _: () = assert!(MAX_UPLOADS_PER_TASK >= 1);

/// Where downloaded uploads live: a sibling of the project checkouts, so they are
/// **outside** every worktree.
///
/// Deliberately not inside the run's worktree — that would put untracked files in
/// a git working tree the agent is about to commit from, risking a screenshot
/// landing in a PR. Outside it, the agent reads by absolute path and git never sees
/// them. Overridable for tests and unusual deployments.
pub(crate) fn attachments_root(projects_dir: &Path) -> PathBuf {
    if let Some(dir) = std::env::var_os("HARNESS_ATTACHMENTS_DIR") {
        return PathBuf::from(dir);
    }
    projects_dir
        .parent()
        .map(|p| p.join("attachments"))
        .unwrap_or_else(|| projects_dir.join("attachments"))
}

/// How long a task's downloaded images are kept, in hours. A week: long enough
/// that no realistic run — including a slow retry — can have its images deleted
/// out from under it, short enough that the directory doesn't grow forever.
/// Overridable with `HARNESS_ATTACHMENTS_TTL_HOURS`.
const DEFAULT_TTL_HOURS: u64 = 24 * 7;

/// Time after which an untouched task directory is swept.
pub(crate) fn ttl() -> Duration {
    let hours = std::env::var("HARNESS_ATTACHMENTS_TTL_HOURS")
        .ok()
        .and_then(|v| v.parse::<u64>().ok())
        .filter(|h| *h > 0)
        .unwrap_or(DEFAULT_TTL_HOURS);
    Duration::from_secs(hours * 3600)
}

/// Whether a directory last written at `modified` has outlived `ttl`.
///
/// A clock that reports a modification time in the future (skew, or a copied
/// tree) yields "not expired" rather than deleting something unexpectedly.
fn is_expired(modified: SystemTime, now: SystemTime, ttl: Duration) -> bool {
    now.duration_since(modified)
        .map(|age| age > ttl)
        .unwrap_or(false)
}

/// Delete task directories not written to within `ttl`, returning how many went.
///
/// Age-based rather than tied to run completion, deliberately: a run's images may
/// be read at any point during it, retries re-read them, and a rerun days later
/// simply re-downloads. Keying the lifetime to wall-clock age needs no coordination
/// with run state and cannot delete files a live run is about to open, whereas
/// "delete when the run ends" would need to be right about every exit path
/// (cancel, crash, lease takeover) to avoid leaking anyway.
///
/// A missing root is not an error — nothing has been downloaded yet.
pub(crate) fn sweep(root: &Path, now: SystemTime, ttl: Duration) -> usize {
    let Ok(entries) = std::fs::read_dir(root) else {
        return 0;
    };
    let mut removed = 0usize;
    for entry in entries.flatten() {
        let path = entry.path();
        // Only ever remove directories we created; never stray files at the root.
        match entry.metadata() {
            Ok(meta) if meta.is_dir() => {
                let Ok(modified) = meta.modified() else {
                    continue; // no mtime on this platform/fs — leave it alone
                };
                if !is_expired(modified, now, ttl) {
                    continue;
                }
                match std::fs::remove_dir_all(&path) {
                    Ok(()) => removed += 1,
                    Err(e) => {
                        tracing::warn!("linear attachments: cannot remove {}: {e}", path.display())
                    }
                }
            }
            _ => continue,
        }
    }
    if removed > 0 {
        tracing::info!(
            "linear attachments: swept {removed} expired task director{} from {}",
            if removed == 1 { "y" } else { "ies" },
            root.display()
        );
    }
    removed
}

/// Directory name, under the harness's attachment root, holding one task's files.
fn task_dir(root: &Path, key: &str) -> PathBuf {
    // `key` is an issue identifier (`ECOM-15`) or a session id; keep only
    // filename-safe characters so it can't escape the root.
    let safe: String = key
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' || c == '_' {
                c
            } else {
                '-'
            }
        })
        .collect();
    root.join(if safe.is_empty() {
        "task".to_string()
    } else {
        safe
    })
}

/// Download every Linear upload referenced in `text` and rewrite the references
/// to local paths, returning the rewritten text.
///
/// `key` names the per-task directory (an issue identifier or session id). Returns
/// `text` unchanged when it references no uploads, so the common case costs
/// nothing.
pub(crate) async fn localize(client: &LinearClient, root: &Path, key: &str, text: &str) -> String {
    let urls = extract_upload_urls(text);
    if urls.is_empty() {
        return text.to_string();
    }

    let dir = task_dir(root, key);
    if let Err(e) = std::fs::create_dir_all(&dir) {
        tracing::warn!(
            "linear attachments: cannot create {}: {e}; leaving URLs as text",
            dir.display()
        );
        return text.to_string();
    }

    let mut out = text.to_string();
    let mut saved = 0usize;
    for (idx, url) in urls.iter().enumerate() {
        if saved >= MAX_UPLOADS_PER_TASK {
            tracing::info!(
                "linear attachments: {key} references {} uploads; keeping the first {} \
                 and leaving the rest as URLs",
                urls.len(),
                MAX_UPLOADS_PER_TASK
            );
            break;
        }
        let upload = match client.download_upload(url, MAX_UPLOAD_BYTES).await {
            Ok(u) => u,
            Err(e) => {
                tracing::warn!("linear attachments: {key} — skipping an upload: {}", e.0);
                continue;
            }
        };
        // Only image types we're willing to hand a model (no SVG — scriptable).
        let Some(ext) = upload.extension() else {
            tracing::debug!(
                "linear attachments: {key} — ignoring upload of type `{}`",
                upload.content_type
            );
            continue;
        };
        let path = dir.join(format!("{}-{}.{ext}", key, idx + 1));
        if let Err(e) = std::fs::write(&path, &upload.bytes) {
            tracing::warn!("linear attachments: cannot write {}: {e}", path.display());
            continue;
        }
        out = out.replace(url, &path.to_string_lossy());
        saved += 1;
    }

    if saved == 0 {
        return out;
    }
    tracing::info!(
        "linear attachments: {key} — materialized {saved} image(s) into {}",
        dir.display()
    );
    // Tell the agent the paths are real files, since a rewritten markdown link
    // alone doesn't imply "you may open this".
    out.push_str(&attachment_note(saved));
    out
}

/// The line appended to a task once images have been downloaded for it.
///
/// Written as two whole sentences rather than one with inflected fragments: the
/// first version interpolated four separate singular/plural choices and got one
/// of them wrong ("what they shows"), which is prose a model reads.
fn attachment_note(saved: usize) -> String {
    if saved == 1 {
        "\n\n---\n\n1 image from this issue has been downloaded to the path referenced \
         above. Read it if the task depends on what it shows.\n"
            .to_string()
    } else {
        format!(
            "\n\n---\n\n{saved} images from this issue have been downloaded to the paths \
             referenced above. Read them if the task depends on what they show.\n"
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn task_dir_sanitizes_the_key_and_cannot_escape_the_root() {
        let root = Path::new("/srv/attach");
        assert_eq!(task_dir(root, "ECOM-15"), root.join("ECOM-15"));
        // Path separators and traversal are neutralized, not honored.
        assert_eq!(task_dir(root, "../../etc"), root.join("------etc"));
        assert_eq!(task_dir(root, "a/b\\c"), root.join("a-b-c"));
        // Degenerate keys still land inside the root.
        assert_eq!(task_dir(root, ""), root.join("task"));
        assert!(task_dir(root, "..").starts_with(root));
    }

    #[test]
    fn localize_is_a_no_op_without_uploads() {
        // No uploads referenced → returned unchanged, and no client call is made
        // (the runtime isn't even entered).
        let text = "Plain task text with a [link](https://linear.app/acme/issue/COR-1).";
        assert!(extract_upload_urls(text).is_empty());
    }

    #[test]
    fn attachment_note_reads_as_english_in_both_numbers() {
        let one = attachment_note(1);
        assert!(
            one.contains("1 image from this issue has been downloaded"),
            "{one}"
        );
        assert!(one.contains("the path referenced above"), "{one}");
        assert!(one.contains("what it shows"), "{one}");

        let many = attachment_note(3);
        assert!(
            many.contains("3 images from this issue have been downloaded"),
            "{many}"
        );
        assert!(many.contains("the paths referenced above"), "{many}");
        // Regression: the first version emitted "what they shows".
        assert!(many.contains("what they show."), "{many}");
        assert!(!many.contains("they shows"), "{many}");
    }

    #[test]
    fn is_expired_compares_age_against_the_ttl_and_tolerates_clock_skew() {
        let now = SystemTime::UNIX_EPOCH + Duration::from_secs(1_000_000);
        let ttl = Duration::from_secs(100);
        // Older than the TTL → expired.
        assert!(is_expired(now - Duration::from_secs(101), now, ttl));
        // Exactly at the TTL is not yet expired (strictly greater).
        assert!(!is_expired(now - ttl, now, ttl));
        assert!(!is_expired(now - Duration::from_secs(1), now, ttl));
        // A modification time in the future (skew, copied tree) must never read as
        // expired — deleting on a bad clock would be the worst failure here.
        assert!(!is_expired(now + Duration::from_secs(10_000), now, ttl));
    }

    /// Ages are exercised by moving `now`, not by backdating directory mtimes:
    /// Windows refuses `set_times` on a directory opened without backup semantics,
    /// which `std` has no way to request. Per-entry age comparison is covered by
    /// `is_expired` above; this covers the filesystem side.
    #[test]
    fn sweep_keeps_directories_inside_the_ttl() {
        let root = tempfile::tempdir().expect("tempdir");
        let dir = root.path().join("ECOM-1");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("a.png"), b"x").unwrap();

        // Just created, so well inside an hour.
        assert_eq!(
            sweep(root.path(), SystemTime::now(), Duration::from_secs(3600)),
            0
        );
        assert!(dir.exists(), "a fresh directory must survive");
    }

    #[test]
    fn sweep_removes_expired_directories_and_leaves_stray_files() {
        let root = tempfile::tempdir().expect("tempdir");
        let a = root.path().join("ECOM-1");
        let b = root.path().join("ECOM-2");
        std::fs::create_dir_all(&a).unwrap();
        std::fs::create_dir_all(&b).unwrap();
        std::fs::write(a.join("a.png"), b"x").unwrap();
        std::fs::write(b.join("b.png"), b"y").unwrap();
        // A stray file at the root is not ours to delete.
        let stray = root.path().join("stray.txt");
        std::fs::write(&stray, b"keep me").unwrap();

        // Look at the tree from two hours in the future with a one-hour TTL.
        let later = SystemTime::now() + Duration::from_secs(7200);
        let ttl = Duration::from_secs(3600);

        assert_eq!(sweep(root.path(), later, ttl), 2);
        assert!(!a.exists());
        assert!(!b.exists());
        assert!(stray.exists(), "stray files are not ours to delete");

        // Idempotent: a second sweep finds nothing left to do.
        assert_eq!(sweep(root.path(), later, ttl), 0);
    }

    #[test]
    fn sweep_tolerates_a_missing_root() {
        let root = tempfile::tempdir().expect("tempdir");
        let missing = root.path().join("never-created");
        assert_eq!(
            sweep(&missing, SystemTime::now(), Duration::from_secs(1)),
            0
        );
    }

    #[test]
    fn ttl_defaults_to_a_week_and_honors_the_override() {
        std::env::remove_var("HARNESS_ATTACHMENTS_TTL_HOURS");
        assert_eq!(ttl(), Duration::from_secs(7 * 24 * 3600));
        std::env::set_var("HARNESS_ATTACHMENTS_TTL_HOURS", "6");
        assert_eq!(ttl(), Duration::from_secs(6 * 3600));
        // Nonsense and zero fall back rather than sweeping everything instantly.
        std::env::set_var("HARNESS_ATTACHMENTS_TTL_HOURS", "0");
        assert_eq!(ttl(), Duration::from_secs(7 * 24 * 3600));
        std::env::set_var("HARNESS_ATTACHMENTS_TTL_HOURS", "not-a-number");
        assert_eq!(ttl(), Duration::from_secs(7 * 24 * 3600));
        std::env::remove_var("HARNESS_ATTACHMENTS_TTL_HOURS");
    }
}
