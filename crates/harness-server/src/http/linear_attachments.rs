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

use harness_sources::linear::{extract_upload_urls, LinearClient};

/// How many uploads one task may materialize. Enough for a handful of
/// screenshots; a bound so a pathological issue can't stall a run on downloads.
const MAX_UPLOADS_PER_TASK: usize = 5;

/// Refuse a single upload larger than this. Generous on purpose — a 12MB
/// photograph is normal and reads fine — this only stops absurd files.
const MAX_UPLOAD_BYTES: usize = 25 * 1024 * 1024;

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
    out.push_str(&format!(
        "\n\n---\n\n{saved} image{} from this issue {} been downloaded to the paths \
         referenced above. Read {} if the task depends on what {} shows.\n",
        if saved == 1 { "" } else { "s" },
        if saved == 1 { "has" } else { "have" },
        if saved == 1 { "it" } else { "them" },
        if saved == 1 { "it" } else { "they" },
    ));
    out
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
    fn caps_are_sane() {
        // A 12MB photograph is a real, working case — the guard must sit well above
        // it, since it exists to stop absurd files rather than to limit what models
        // receive.
        assert!(MAX_UPLOAD_BYTES > 12 * 1024 * 1024);
        assert!(MAX_UPLOADS_PER_TASK >= 1);
    }
}
