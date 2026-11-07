//! GitHub REST client — **read-only** pull-request status.
//!
//! Answers "which of these PRs should I test?": a PR's state, its CI checks and
//! the preview deployment a host such as Vercel attached to its head commit.
//! It performs no writes; opening, pushing and merging PRs stays with the agents
//! (the orchestration rule in CLAUDE.md). A read over HTTPS is not git work.
//!
//! Parsing is split from the HTTP calls so the response shaping is unit-tested
//! with fixtures (no mock server), matching [`crate::linear`].

use std::collections::HashMap;
use std::time::Duration;

use serde::Serialize;
use serde_json::Value;

const API: &str = "https://api.github.com";
const USER_AGENT: &str = "ai-harness";
const REQUEST_TIMEOUT: Duration = Duration::from_secs(20);
/// Deployments looked at for a preview URL. A head commit usually has one per
/// host; a few more covers redeploys without walking a long history.
const MAX_DEPLOYMENTS: usize = 3;

#[derive(Debug, thiserror::Error)]
#[error("github: {0}")]
pub struct GithubError(pub String);

/// A pull request named by its URL: `https://github.com/<owner>/<repo>/pull/<n>`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct PrRef {
    pub owner: String,
    pub repo: String,
    pub number: u64,
}

/// Parse a PR URL. Tolerates a trailing path (`/files`), query or fragment.
pub fn parse_pr_url(url: &str) -> Option<PrRef> {
    let rest = url
        .trim()
        .strip_prefix("https://")
        .or_else(|| url.trim().strip_prefix("http://"))?
        .strip_prefix("github.com/")?;
    let mut parts = rest.split(['/', '?', '#']);
    let owner = parts.next().filter(|s| !s.is_empty())?;
    let repo = parts.next().filter(|s| !s.is_empty())?;
    if parts.next()? != "pull" {
        return None;
    }
    let number = parts.next()?.parse().ok()?;
    Some(PrRef {
        owner: owner.to_string(),
        repo: repo.to_string(),
        number,
    })
}

/// One file a pull request changes.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct PrFile {
    pub path: String,
    /// `added`, `modified`, `removed`, `renamed`, …
    pub status: String,
    pub additions: u64,
    pub deletions: u64,
    /// The old path of a renamed file.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub previous_path: Option<String>,
    /// The unified diff. `None` for a binary file, or one too large for
    /// GitHub to diff.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub patch: Option<String>,
}

/// Parse a `pulls/{n}/files` page.
pub fn parse_pr_files(v: &Value) -> Vec<PrFile> {
    let s = |f: &Value, k: &str| f.get(k).and_then(Value::as_str).map(str::to_string);
    v.as_array()
        .map(|files| {
            files
                .iter()
                .filter_map(|f| {
                    Some(PrFile {
                        path: s(f, "filename")?,
                        status: s(f, "status").unwrap_or_default(),
                        additions: f.get("additions").and_then(Value::as_u64).unwrap_or(0),
                        deletions: f.get("deletions").and_then(Value::as_u64).unwrap_or(0),
                        previous_path: s(f, "previous_filename"),
                        patch: s(f, "patch"),
                    })
                })
                .collect()
        })
        .unwrap_or_default()
}

/// A PR's diff as an agent reads it: every file listed, diffs included until
/// `budget` characters, the rest marked for a second call by `path`.
///
/// Diffs are never cut mid-file — half a hunk reads as a different change — so
/// a file either comes whole or is left out with a note saying how to get it.
/// `path` narrows to one file (or a folder prefix), whole whatever the budget.
pub fn diff_view(files: &[PrFile], path: Option<&str>, budget: usize) -> Value {
    let wanted: Vec<&PrFile> = match path.map(str::trim).filter(|p| !p.is_empty()) {
        Some(p) => files
            .iter()
            .filter(|f| f.path == p || f.path.starts_with(&format!("{}/", p.trim_end_matches('/'))))
            .collect(),
        None => files.iter().collect(),
    };
    let narrowed = path.is_some_and(|p| !p.trim().is_empty());
    let mut used = 0usize;
    let mut omitted = 0usize;
    let shown: Vec<Value> = wanted
        .iter()
        .map(|f| {
            let mut v = serde_json::json!({
                "path": f.path,
                "status": f.status,
                "additions": f.additions,
                "deletions": f.deletions,
            });
            if let Some(prev) = &f.previous_path {
                v["previous_path"] = Value::String(prev.clone());
            }
            match &f.patch {
                Some(patch) if narrowed || used + patch.len() <= budget => {
                    used += patch.len();
                    v["patch"] = Value::String(patch.clone());
                }
                Some(_) => {
                    omitted += 1;
                    v["patch_omitted"] = Value::String("over budget; ask again with `path`".into());
                }
                None => {
                    v["patch_omitted"] =
                        Value::String("binary, or too large for GitHub to diff".into());
                }
            }
            v
        })
        .collect();
    serde_json::json!({
        "files_changed": files.len(),
        "additions": files.iter().map(|f| f.additions).sum::<u64>(),
        "deletions": files.iter().map(|f| f.deletions).sum::<u64>(),
        "shown": shown,
        "omitted_for_budget": omitted,
    })
}

/// What a person needs to decide whether to test a PR.
#[derive(Debug, Clone, Serialize)]
pub struct PrSnapshot {
    pub url: String,
    pub title: String,
    /// `open`, `closed` or `merged`.
    pub state: String,
    pub draft: bool,
    pub head_branch: String,
    pub head_sha: String,
    pub created_at: Option<String>,
    pub updated_at: Option<String>,
    pub merged_at: Option<String>,
    pub checks: Checks,
    /// The preview deployment of the head commit, when a host reported one.
    pub preview_url: Option<String>,
    /// Parts that could not be read (e.g. a token without `checks:read`). The
    /// snapshot is still useful without them, so they are reported, not fatal.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub warnings: Vec<String>,
}

/// CI on the head commit: check runs and legacy commit statuses together.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct Checks {
    pub passed: u32,
    pub failed: u32,
    pub pending: u32,
    /// Names of the failing checks, so an answer can say *what* is red.
    pub failing: Vec<String>,
}

impl Checks {
    fn add(&mut self, name: &str, outcome: Outcome) {
        match outcome {
            Outcome::Passed => self.passed += 1,
            Outcome::Pending => self.pending += 1,
            Outcome::Failed => {
                self.failed += 1;
                self.failing.push(name.to_string());
            }
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Outcome {
    Passed,
    Failed,
    Pending,
}

/// The fields of `GET /repos/{o}/{r}/pulls/{n}` a snapshot carries.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PullFields {
    pub url: String,
    pub title: String,
    pub state: String,
    pub draft: bool,
    pub head_branch: String,
    pub head_sha: String,
    pub created_at: Option<String>,
    pub updated_at: Option<String>,
    pub merged_at: Option<String>,
    /// The login that opened it.
    pub author: String,
}

fn str_at(v: &Value, path: &[&str]) -> Option<String> {
    let mut cur = v;
    for key in path {
        cur = cur.get(key)?;
    }
    cur.as_str().map(str::to_string)
}

/// Shape a pull-request response. A merged PR reports `state: closed` upstream;
/// it is `merged` here, because that is the difference a reader cares about.
pub fn parse_pull(v: &Value) -> Result<PullFields, GithubError> {
    let head_sha = str_at(v, &["head", "sha"])
        .ok_or_else(|| GithubError("pull request response has no head sha".into()))?;
    let merged_at = str_at(v, &["merged_at"]);
    let state = if merged_at.is_some() {
        "merged".to_string()
    } else {
        str_at(v, &["state"]).unwrap_or_default()
    };
    Ok(PullFields {
        url: str_at(v, &["html_url"]).unwrap_or_default(),
        title: str_at(v, &["title"]).unwrap_or_default(),
        state,
        draft: v.get("draft").and_then(Value::as_bool).unwrap_or(false),
        head_branch: str_at(v, &["head", "ref"]).unwrap_or_default(),
        head_sha,
        created_at: str_at(v, &["created_at"]),
        updated_at: str_at(v, &["updated_at"]),
        merged_at,
        author: str_at(v, &["user", "login"]).unwrap_or_default(),
    })
}

/// When a person other than the author first reviewed a PR, from
/// `GET /repos/{o}/{r}/pulls/{n}/reviews`. Bots, the author's own replies and
/// reviews still pending (not submitted) don't count.
pub fn first_human_review(reviews: &Value, author: &str) -> Option<String> {
    reviews
        .as_array()?
        .iter()
        .filter(|r| str_at(r, &["state"]).as_deref() != Some("PENDING"))
        .filter(|r| str_at(r, &["user", "type"]).as_deref() != Some("Bot"))
        .filter(|r| {
            let login = str_at(r, &["user", "login"]).unwrap_or_default();
            !login.is_empty() && !login.eq_ignore_ascii_case(author)
        })
        .filter_map(|r| str_at(r, &["submitted_at"]))
        // RFC 3339 in UTC sorts as text.
        .min()
}

/// Fold `check-runs` and the combined `status` into one summary.
///
/// A re-run check appears once per attempt; only the latest (highest id) of
/// each name counts, or a check that failed and then passed would read as red.
/// The combined status endpoint already keeps only the latest per context.
pub fn summarize_checks(check_runs: Option<&Value>, combined_status: Option<&Value>) -> Checks {
    let mut checks = Checks::default();
    if let Some(runs) = check_runs
        .and_then(|v| v.get("check_runs"))
        .and_then(Value::as_array)
    {
        let mut latest: HashMap<&str, &Value> = HashMap::new();
        for run in runs {
            let name = run.get("name").and_then(Value::as_str).unwrap_or("check");
            let id = |r: &Value| r.get("id").and_then(Value::as_u64).unwrap_or(0);
            if latest.get(name).is_none_or(|prev| id(run) > id(prev)) {
                latest.insert(name, run);
            }
        }
        let mut names: Vec<&str> = latest.keys().copied().collect();
        names.sort_unstable();
        for name in names {
            let run = latest[name];
            let outcome = if run.get("status").and_then(Value::as_str) != Some("completed") {
                Outcome::Pending
            } else {
                match run.get("conclusion").and_then(Value::as_str) {
                    Some("success" | "neutral" | "skipped") => Outcome::Passed,
                    _ => Outcome::Failed,
                }
            };
            checks.add(name, outcome);
        }
    }
    if let Some(statuses) = combined_status
        .and_then(|v| v.get("statuses"))
        .and_then(Value::as_array)
    {
        for s in statuses {
            let name = s.get("context").and_then(Value::as_str).unwrap_or("status");
            let outcome = match s.get("state").and_then(Value::as_str) {
                Some("success") => Outcome::Passed,
                Some("pending") => Outcome::Pending,
                _ => Outcome::Failed,
            };
            checks.add(name, outcome);
        }
    }
    checks
}

/// Ids of the deployments worth asking for a preview URL, newest first.
/// Production deployments are skipped: their URL is the live site, not this PR.
pub fn preview_deployment_ids(deployments: &Value) -> Vec<u64> {
    deployments
        .as_array()
        .map(|list| {
            list.iter()
                .filter(|d| {
                    let prod = d
                        .get("production_environment")
                        .and_then(Value::as_bool)
                        .unwrap_or(false);
                    let env = d.get("environment").and_then(Value::as_str).unwrap_or("");
                    !prod && !env.eq_ignore_ascii_case("production")
                })
                .filter_map(|d| d.get("id").and_then(Value::as_u64))
                .take(MAX_DEPLOYMENTS)
                .collect()
        })
        .unwrap_or_default()
}

/// The URL of the newest successful status of a deployment. `environment_url`
/// only: `target_url` is often a build log or dashboard rather than the site.
pub fn preview_url_from_statuses(statuses: &Value) -> Option<String> {
    statuses.as_array()?.iter().find_map(|s| {
        (s.get("state").and_then(Value::as_str) == Some("success"))
            .then(|| s.get("environment_url").and_then(Value::as_str))
            .flatten()
            .filter(|u| !u.is_empty())
            .map(str::to_string)
    })
}

/// A read-only GitHub REST client. Without a token it can still read public
/// repositories, at GitHub's much lower anonymous rate limit.
pub struct GithubClient {
    http: reqwest::Client,
    token: Option<String>,
}

impl GithubClient {
    pub fn new(token: Option<String>) -> Self {
        Self {
            http: reqwest::Client::builder()
                .timeout(REQUEST_TIMEOUT)
                .build()
                .unwrap_or_else(|_| reqwest::Client::new()),
            token: token.filter(|t| !t.trim().is_empty()),
        }
    }

    async fn get(&self, path: &str) -> Result<Value, GithubError> {
        let mut req = self
            .http
            .get(format!("{API}{path}"))
            .header(reqwest::header::USER_AGENT, USER_AGENT)
            .header(reqwest::header::ACCEPT, "application/vnd.github+json")
            .header("X-GitHub-Api-Version", "2022-11-28");
        if let Some(token) = &self.token {
            req = req.bearer_auth(token);
        }
        let resp = req
            .send()
            .await
            .map_err(|e| GithubError(format!("request failed: {e}")))?;
        let status = resp.status();
        if !status.is_success() {
            // 404 is also what GitHub answers for a private repo the token cannot see.
            let hint = if status.as_u16() == 404 {
                " (not found, or the token has no access to this repository)"
            } else {
                ""
            };
            return Err(GithubError(format!("GET {path} → {status}{hint}")));
        }
        resp.json()
            .await
            .map_err(|e| GithubError(format!("bad JSON from GET {path}: {e}")))
    }

    /// The pull request itself: state, title, branch and its timestamps. One
    /// request, for callers that only need to know whether it has merged.
    pub async fn pull(&self, pr: &PrRef) -> Result<PullFields, GithubError> {
        parse_pull(
            &self
                .get(&format!(
                    "/repos/{}/{}/pulls/{}",
                    pr.owner, pr.repo, pr.number
                ))
                .await?,
        )
    }

    /// Whether `branch` exists in `owner/repo`. A 404 is `false`; anything
    /// else that fails is an error, so a caller can tell "no such branch" from
    /// "couldn't ask".
    pub async fn branch_exists(
        &self,
        owner: &str,
        repo: &str,
        branch: &str,
    ) -> Result<bool, GithubError> {
        // `git/ref/heads/<name>` takes a name with slashes in it as-is, which
        // `branches/<name>` does not reliably do.
        match self
            .get(&format!("/repos/{owner}/{repo}/git/ref/heads/{branch}"))
            .await
        {
            Ok(_) => Ok(true),
            // The status, not the path: an epic named `AIH-404` is in the path.
            Err(e) if e.0.contains("→ 404") => Ok(false),
            Err(e) => Err(e),
        }
    }

    /// When a person other than `author` first reviewed the PR, if anyone has.
    /// The first 100 reviews are enough to find the earliest.
    pub async fn first_review(
        &self,
        pr: &PrRef,
        author: &str,
    ) -> Result<Option<String>, GithubError> {
        let v = self
            .get(&format!(
                "/repos/{}/{}/pulls/{}/reviews?per_page=100",
                pr.owner, pr.repo, pr.number
            ))
            .await?;
        Ok(first_human_review(&v, author))
    }

    /// The files a pull request changes, with their diffs. Up to 300 files,
    /// which is past where reading a PR file by file stops being useful.
    pub async fn pr_files(&self, pr: &PrRef) -> Result<Vec<PrFile>, GithubError> {
        let mut out = Vec::new();
        for page in 1..=3 {
            let v = self
                .get(&format!(
                    "/repos/{}/{}/pulls/{}/files?per_page=100&page={page}",
                    pr.owner, pr.repo, pr.number
                ))
                .await?;
            let files = parse_pr_files(&v);
            let full = files.len() == 100;
            out.extend(files);
            if !full {
                break;
            }
        }
        Ok(out)
    }

    /// A PR's state, CI and preview URL. Only the PR itself is required; a check
    /// or deployment read that fails becomes a warning on the snapshot.
    pub async fn pr_snapshot(&self, pr: &PrRef) -> Result<PrSnapshot, GithubError> {
        let base = format!("/repos/{}/{}", pr.owner, pr.repo);
        let pull = parse_pull(&self.get(&format!("{base}/pulls/{}", pr.number)).await?)?;
        let sha = &pull.head_sha;
        let mut warnings = Vec::new();

        let check_runs = self
            .get(&format!("{base}/commits/{sha}/check-runs?per_page=100"))
            .await
            .map_err(|e| warnings.push(format!("check runs: {}", e.0)))
            .ok();
        let combined = self
            .get(&format!("{base}/commits/{sha}/status"))
            .await
            .map_err(|e| warnings.push(format!("commit statuses: {}", e.0)))
            .ok();
        let checks = summarize_checks(check_runs.as_ref(), combined.as_ref());

        let mut preview_url = None;
        match self
            .get(&format!("{base}/deployments?sha={sha}&per_page=10"))
            .await
        {
            Ok(deployments) => {
                for id in preview_deployment_ids(&deployments) {
                    match self
                        .get(&format!("{base}/deployments/{id}/statuses?per_page=10"))
                        .await
                    {
                        Ok(statuses) => {
                            preview_url = preview_url_from_statuses(&statuses);
                            if preview_url.is_some() {
                                break;
                            }
                        }
                        Err(e) => warnings.push(format!("deployment {id}: {}", e.0)),
                    }
                }
            }
            Err(e) => warnings.push(format!("deployments: {}", e.0)),
        }

        Ok(PrSnapshot {
            url: pull.url,
            title: pull.title,
            state: pull.state,
            draft: pull.draft,
            head_branch: pull.head_branch,
            head_sha: pull.head_sha,
            created_at: pull.created_at,
            updated_at: pull.updated_at,
            merged_at: pull.merged_at,
            checks,
            preview_url,
            warnings,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn parses_pr_urls_and_rejects_others() {
        assert_eq!(
            parse_pr_url("https://github.com/acme/shop/pull/42"),
            Some(PrRef {
                owner: "acme".into(),
                repo: "shop".into(),
                number: 42
            })
        );
        assert_eq!(
            parse_pr_url("https://github.com/acme/shop/pull/42/files#diff").map(|p| p.number),
            Some(42)
        );
        assert_eq!(parse_pr_url("https://github.com/acme/shop/issues/42"), None);
        assert_eq!(parse_pr_url("https://gitlab.com/acme/shop/pull/42"), None);
        assert_eq!(parse_pr_url("https://github.com/acme/shop/pull/x"), None);
    }

    #[test]
    fn a_merged_pr_reads_as_merged_not_closed() {
        let pr = parse_pull(&json!({
            "html_url": "https://github.com/acme/shop/pull/7",
            "title": "Add wishlist (ECOM-44)",
            "state": "closed",
            "draft": false,
            "merged_at": "2026-10-01T10:00:00Z",
            "head": { "ref": "harness-run/abc", "sha": "deadbeef" }
        }))
        .unwrap();
        assert_eq!(pr.state, "merged");
        assert_eq!(pr.head_sha, "deadbeef");
        assert_eq!(pr.head_branch, "harness-run/abc");

        let open = parse_pull(&json!({
            "state": "open", "merged_at": null, "head": { "sha": "a" }
        }))
        .unwrap();
        assert_eq!(open.state, "open");
        assert!(parse_pull(&json!({ "state": "open" })).is_err());
    }

    #[test]
    fn the_first_review_is_a_submitted_one_by_a_person_who_did_not_write_it() {
        let reviews = json!([
            { "state": "COMMENTED", "submitted_at": "2026-11-01T08:00:00Z",
              "user": { "login": "author", "type": "User" } },
            { "state": "COMMENTED", "submitted_at": "2026-11-01T09:00:00Z",
              "user": { "login": "ci-bot", "type": "Bot" } },
            { "state": "PENDING",
              "user": { "login": "alice", "type": "User" } },
            { "state": "APPROVED", "submitted_at": "2026-11-01T12:00:00Z",
              "user": { "login": "bob", "type": "User" } },
            { "state": "CHANGES_REQUESTED", "submitted_at": "2026-11-01T10:00:00Z",
              "user": { "login": "carol", "type": "User" } }
        ]);
        assert_eq!(
            first_human_review(&reviews, "Author").as_deref(),
            Some("2026-11-01T10:00:00Z")
        );
        assert_eq!(first_human_review(&json!([]), "a"), None);
    }

    #[test]
    fn only_the_latest_attempt_of_a_check_counts() {
        let runs = json!({ "check_runs": [
            { "id": 1, "name": "test", "status": "completed", "conclusion": "failure" },
            { "id": 5, "name": "test", "status": "completed", "conclusion": "success" },
            { "id": 2, "name": "lint", "status": "completed", "conclusion": "failure" },
            { "id": 3, "name": "e2e", "status": "in_progress", "conclusion": null },
            { "id": 4, "name": "docs", "status": "completed", "conclusion": "skipped" }
        ]});
        let statuses = json!({ "statuses": [
            { "context": "Vercel", "state": "success" },
            { "context": "legacy-ci", "state": "error" }
        ]});
        let c = summarize_checks(Some(&runs), Some(&statuses));
        assert_eq!((c.passed, c.failed, c.pending), (3, 2, 1));
        assert_eq!(c.failing, vec!["lint".to_string(), "legacy-ci".to_string()]);
        assert_eq!(summarize_checks(None, None), Checks::default());
    }

    #[test]
    fn preview_comes_from_a_non_production_deployment() {
        let deployments = json!([
            { "id": 10, "environment": "Production", "production_environment": true },
            { "id": 11, "environment": "Preview", "production_environment": false },
            { "id": 12, "environment": "Preview – shop" }
        ]);
        assert_eq!(preview_deployment_ids(&deployments), vec![11, 12]);

        let statuses = json!([
            { "state": "in_progress", "environment_url": "" },
            { "state": "success", "environment_url": "https://shop-git-x.vercel.app",
              "target_url": "https://vercel.com/acme/shop/abc" }
        ]);
        assert_eq!(
            preview_url_from_statuses(&statuses).as_deref(),
            Some("https://shop-git-x.vercel.app")
        );
        // A success with no environment URL is a build, not a site.
        assert_eq!(
            preview_url_from_statuses(&json!([{ "state": "success", "target_url": "https://ci" }])),
            None
        );
    }

    fn files() -> Vec<PrFile> {
        parse_pr_files(&json!([
            { "filename": "apps/web/lib/sentry/filters.ts", "status": "modified",
              "additions": 5, "deletions": 0, "patch": "@@ -90 +90,5 @@\n+  \"logMutedMessage\"," },
            { "filename": "apps/web/lib/sentry/filters.test.ts", "status": "modified",
              "additions": 2, "deletions": 0, "patch": "@@ -137 +137,2 @@\n+  case" },
            { "filename": "public/logo.png", "status": "added", "additions": 0, "deletions": 0 },
            { "filename": "docs/new.md", "previous_filename": "docs/old.md",
              "status": "renamed", "additions": 0, "deletions": 0 }
        ]))
    }

    #[test]
    fn a_prs_files_are_read_with_their_diffs() {
        let f = files();
        assert_eq!(f.len(), 4);
        assert_eq!(f[0].path, "apps/web/lib/sentry/filters.ts");
        assert_eq!(f[0].additions, 5);
        assert!(f[2].patch.is_none(), "binary");
        assert_eq!(f[3].previous_path.as_deref(), Some("docs/old.md"));
    }

    #[test]
    fn a_diff_is_shown_whole_files_within_the_budget() {
        let f = files();
        let all = diff_view(&f, None, 10_000);
        assert_eq!(all["files_changed"], 4);
        assert_eq!(all["additions"], 7);
        assert!(all["shown"][0]["patch"].is_string());
        assert_eq!(all["omitted_for_budget"], 0);
        assert!(all["shown"][2]["patch_omitted"]
            .as_str()
            .unwrap()
            .contains("binary"));

        // A budget that fits the first file only: the second is listed, not cut.
        let first = f[0].patch.as_ref().unwrap().len();
        let tight = diff_view(&f, None, first);
        assert!(tight["shown"][0]["patch"].is_string());
        assert!(tight["shown"][1]["patch"].is_null());
        assert_eq!(tight["omitted_for_budget"], 1);

        // Asking by path gets the file whole, whatever the budget; a folder too.
        let one = diff_view(&f, Some("apps/web/lib/sentry/filters.test.ts"), 0);
        assert_eq!(one["shown"].as_array().unwrap().len(), 1);
        assert!(one["shown"][0]["patch"].is_string());
        let folder = diff_view(&f, Some("apps/web/lib/sentry/"), 0);
        assert_eq!(folder["shown"].as_array().unwrap().len(), 2);
    }
}
