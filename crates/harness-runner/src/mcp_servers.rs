//! **External MCP servers** a workflow step can ask for with `mcp: [name]`.
//!
//! Each known server is started by the agent CLI itself (stdio), from a config
//! this module builds out of the run's environment. A server is only offered
//! when the project has it configured — the run's environment carries its
//! credentials — so a shared workflow that asks for Sentry still runs, without
//! it, on a project that has none.
//!
//! Secrets stay out of config files: a config names `${HARNESS_SENTRY_TOKEN}`,
//! and the agent CLI expands it from its own environment when it starts the
//! server.

use std::collections::HashMap;

use serde_json::{json, Map, Value};

/// Run-environment keys the harness sets for a project with Sentry.
///
/// Namespaced on purpose. `SENTRY_AUTH_TOKEN`, `SENTRY_ORG` and
/// `SENTRY_PROJECT` are what Sentry's build tooling (`sentry-cli`, the
/// Next.js plugin) reads to upload source maps; a run's build would pick them
/// up and try an upload with a read-only token. Only the MCP server sees the
/// name it wants (`SENTRY_ACCESS_TOKEN`), inside its own process.
pub const SENTRY_TOKEN_ENV: &str = "HARNESS_SENTRY_TOKEN";
pub const SENTRY_ORG_ENV: &str = "HARNESS_SENTRY_ORG";
pub const SENTRY_PROJECT_ENV: &str = "HARNESS_SENTRY_PROJECT";

/// The variable `sentry-mcp` reads its token from.
const SENTRY_MCP_TOKEN_VAR: &str = "SENTRY_ACCESS_TOKEN";

/// The tools `sentry-mcp` 0.42.0 offers with `--skills=inspect`, as its
/// `tools/list` reports them. Needed where tools are listed by name (omp's
/// `--tools`, which is the whole active set); re-check on a version bump.
pub const SENTRY_TOOLS: &[&str] = &[
    "find_projects",
    "search_events",
    "search_issues",
    "get_sentry_resource",
    "search_sentry_tools",
    "execute_sentry_tool",
];

/// The tools a known server offers, by name (empty for an unknown one).
pub fn tool_names(server: &str) -> &'static [&'static str] {
    match server {
        "sentry" => SENTRY_TOOLS,
        _ => &[],
    }
}

/// Providers whose agents can be given an MCP server for a step.
pub const MCP_PROVIDERS: &[&str] = &["claude", "pi", "omp", "kimi"];

/// Whether `provider` (absent = the default, Claude) can attach MCP servers.
pub fn provider_supports_mcp(provider: Option<&str>) -> bool {
    provider.is_none_or(|p| MCP_PROVIDERS.contains(&p))
}

fn env_value<'a>(env: &'a HashMap<String, String>, key: &str) -> Option<&'a str> {
    env.get(key)
        .map(String::as_str)
        .filter(|v| !v.trim().is_empty())
}

/// Sentry's own MCP server (`@sentry/mcp-server`, binary `sentry-mcp`).
///
/// Read-only twice over: the token the harness asks for has only read scopes,
/// and `--skills=inspect` is the server's read-only skill — `triage` (resolve,
/// assign), `project-management` and `seer` (which spends Seer quota) stay
/// off. Its natural-language search runs an embedded agent on an OpenAI,
/// Anthropic or OpenRouter key when one is in its environment; all three are
/// blanked so it cannot quietly spend a key the harness holds.
fn sentry(env: &HashMap<String, String>) -> Option<Value> {
    env_value(env, SENTRY_TOKEN_ENV)?;
    let org = env_value(env, SENTRY_ORG_ENV)?;
    let mut args = vec![
        "--skills=inspect".to_string(),
        format!("--organization-slug={org}"),
    ];
    if let Some(project) = env_value(env, SENTRY_PROJECT_ENV) {
        args.push(format!("--project-slug={project}"));
    }
    let command = std::env::var("HARNESS_SENTRY_MCP")
        .ok()
        .filter(|v| !v.is_empty())
        .unwrap_or_else(|| "sentry-mcp".into());
    Some(json!({
        "type": "stdio",
        "command": command,
        "args": args,
        "env": {
            SENTRY_MCP_TOKEN_VAR: format!("${{{SENTRY_TOKEN_ENV}}}"),
            // Its search tools then take Sentry query syntax directly (checked
            // against 0.42.0: "AI-powered search tools will be unavailable").
            "OPENAI_API_KEY": "",
            "ANTHROPIC_API_KEY": "",
            "OPENROUTER_API_KEY": "",
        }
    }))
}

/// One requested server's config, or `None` when the project doesn't have it.
fn config_for(name: &str, env: &HashMap<String, String>) -> Option<Value> {
    match name {
        "sentry" => sentry(env),
        _ => None,
    }
}

/// The `{ "mcpServers": { … } }` document for the servers a step asked for
/// that this run can start, and the names it asked for but can't have.
pub fn resolve(names: &[String], env: &HashMap<String, String>) -> (Option<Value>, Vec<String>) {
    let mut servers = Map::new();
    let mut missing = Vec::new();
    for name in names {
        match config_for(name, env) {
            Some(cfg) => {
                servers.insert(name.clone(), cfg);
            }
            None => missing.push(name.clone()),
        }
    }
    let doc = (!servers.is_empty()).then(|| json!({ "mcpServers": servers }));
    (doc, missing)
}

/// A line for the step's prompt when it asked for servers it won't get, so the
/// agent doesn't go looking for tools that aren't there.
pub fn unavailable_note(missing: &[String], provider_supported: bool) -> Option<String> {
    if missing.is_empty() {
        return None;
    }
    let names = missing.join(", ");
    Some(if provider_supported {
        format!(
            "Note: this step asked for {names}, but this project has no {names} connection, \
             so those tools are not available. Work without them."
        )
    } else {
        format!(
            "Note: this step asked for {names}, but its agent cannot be given MCP servers, \
             so those tools are not available. Work without them."
        )
    })
}

/// Where omp looks for project MCP servers, relative to its working directory.
const OMP_MCP_RELATIVE: &str = ".omp/mcp.json";

/// A step's MCP config placed where omp reads it; removed when dropped, so it
/// never outlives the step it was written for.
pub struct OmpMcpFile {
    path: std::path::PathBuf,
    created_dir: Option<std::path::PathBuf>,
}

impl Drop for OmpMcpFile {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.path);
        if let Some(dir) = &self.created_dir {
            let _ = std::fs::remove_dir(dir);
        }
    }
}

/// The `info/exclude` file of the repository `cwd` belongs to, if `cwd` is a
/// repo root or a worktree of one. Read from `.git` directly — a worktree's
/// `.git` is a file naming its gitdir, whose `commondir` names the shared one.
fn info_exclude(cwd: &std::path::Path) -> Option<std::path::PathBuf> {
    let dot_git = cwd.join(".git");
    let git_dir = if dot_git.is_dir() {
        dot_git
    } else {
        let text = std::fs::read_to_string(&dot_git).ok()?;
        let rel = text.strip_prefix("gitdir:")?.trim();
        let dir = cwd.join(rel);
        match std::fs::read_to_string(dir.join("commondir")) {
            Ok(common) => dir.join(common.trim()),
            Err(_) => dir,
        }
    };
    Some(git_dir.join("info").join("exclude"))
}

/// Write `config` to `<cwd>/.omp/mcp.json` for an omp step.
///
/// `cwd` is usually the run's git worktree, so the file is first listed in the
/// repo's `info/exclude` — an agent that commits everything (`git add -A`)
/// must not commit it. A project that already has its own `.omp/mcp.json` is
/// left alone: `Ok(None)`, and the step runs without the extra servers rather
/// than having a tracked file rewritten under it.
pub fn place_omp_config(
    cwd: &std::path::Path,
    config: &Value,
) -> Result<Option<OmpMcpFile>, String> {
    let path = cwd.join(OMP_MCP_RELATIVE);
    if path.exists() {
        return Ok(None);
    }
    if let Some(exclude) = info_exclude(cwd) {
        let line = format!("/{OMP_MCP_RELATIVE}");
        let existing = std::fs::read_to_string(&exclude).unwrap_or_default();
        if !existing.lines().any(|l| l.trim() == line) {
            if let Some(dir) = exclude.parent() {
                std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
            }
            let sep = if existing.is_empty() || existing.ends_with('\n') {
                ""
            } else {
                "\n"
            };
            std::fs::write(&exclude, format!("{existing}{sep}{line}\n"))
                .map_err(|e| format!("could not update {}: {e}", exclude.display()))?;
        }
    }
    let dir = path.parent().expect("has a parent").to_path_buf();
    let created_dir = (!dir.exists()).then(|| dir.clone());
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    std::fs::write(&path, serde_json::to_vec_pretty(config).unwrap_or_default())
        .map_err(|e| e.to_string())?;
    Ok(Some(OmpMcpFile { path, created_dir }))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn git(dir: &std::path::Path, args: &[&str]) {
        let ok = std::process::Command::new("git")
            .arg("-C")
            .arg(dir)
            .args(args)
            .output()
            .expect("git")
            .status
            .success();
        assert!(ok, "git {args:?}");
    }

    #[test]
    fn the_omp_config_is_excluded_from_the_worktree_and_removed_after() {
        let repo = tempfile::tempdir().unwrap();
        git(repo.path(), &["init", "-q"]);
        git(repo.path(), &["config", "user.email", "t@example.com"]);
        git(repo.path(), &["config", "user.name", "t"]);
        std::fs::write(repo.path().join("a.txt"), "a").unwrap();
        git(repo.path(), &["add", "."]);
        git(repo.path(), &["commit", "-q", "-m", "init"]);
        let wt = repo.path().join("wt");
        git(
            repo.path(),
            &[
                "worktree",
                "add",
                "-q",
                "--detach",
                &wt.to_string_lossy(),
                "HEAD",
            ],
        );

        let cfg = json!({ "mcpServers": { "sentry": {} } });
        let placed = place_omp_config(&wt, &cfg).unwrap().expect("placed");
        assert!(wt.join(".omp/mcp.json").is_file());
        // `git add -A` in the worktree would not pick it up.
        let status = std::process::Command::new("git")
            .arg("-C")
            .arg(&wt)
            .args(["status", "--porcelain", "--untracked-files=all"])
            .output()
            .unwrap();
        assert_eq!(String::from_utf8_lossy(&status.stdout).trim(), "");
        drop(placed);
        assert!(!wt.join(".omp").exists(), "removed with its directory");

        // A project's own .omp/mcp.json is never overwritten.
        std::fs::create_dir_all(wt.join(".omp")).unwrap();
        std::fs::write(wt.join(".omp/mcp.json"), "{\"own\":true}").unwrap();
        assert!(place_omp_config(&wt, &cfg).unwrap().is_none());
        assert_eq!(
            std::fs::read_to_string(wt.join(".omp/mcp.json")).unwrap(),
            "{\"own\":true}"
        );
    }

    #[test]
    fn outside_a_repo_the_config_is_just_written() {
        let dir = tempfile::tempdir().unwrap();
        let placed = place_omp_config(dir.path(), &json!({})).unwrap().unwrap();
        assert!(dir.path().join(".omp/mcp.json").is_file());
        drop(placed);
        assert!(!dir.path().join(".omp").exists());
    }

    fn env(pairs: &[(&str, &str)]) -> HashMap<String, String> {
        pairs
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect()
    }

    #[test]
    fn sentry_is_read_only_scoped_and_holds_no_secret() {
        let e = env(&[
            (SENTRY_TOKEN_ENV, "sntryu_secret"),
            (SENTRY_ORG_ENV, "dilling"),
            (SENTRY_PROJECT_ENV, "webshop"),
        ]);
        let (doc, missing) = resolve(&["sentry".into()], &e);
        assert!(missing.is_empty());
        let s = &doc.unwrap()["mcpServers"]["sentry"];
        assert_eq!(s["type"], "stdio");
        let args: Vec<&str> = s["args"]
            .as_array()
            .unwrap()
            .iter()
            .map(|a| a.as_str().unwrap())
            .collect();
        assert_eq!(
            args,
            [
                "--skills=inspect",
                "--organization-slug=dilling",
                "--project-slug=webshop"
            ]
        );
        // The server gets the name it reads, from the harness's own name.
        assert_eq!(s["env"]["SENTRY_ACCESS_TOKEN"], "${HARNESS_SENTRY_TOKEN}");
        // And no name Sentry's build tooling reads is ever in the run env.
        for name in [SENTRY_TOKEN_ENV, SENTRY_ORG_ENV, SENTRY_PROJECT_ENV] {
            assert!(name.starts_with("HARNESS_SENTRY_"), "{name}");
        }
        assert_eq!(s["env"]["OPENAI_API_KEY"], "");
        assert_eq!(s["env"]["ANTHROPIC_API_KEY"], "");
        assert_eq!(s["env"]["OPENROUTER_API_KEY"], "");
        assert!(!s.to_string().contains("sntryu_secret"));
    }

    #[test]
    fn a_project_without_sentry_gets_none_and_a_note() {
        let (doc, missing) = resolve(&["sentry".into()], &env(&[(SENTRY_ORG_ENV, "d")]));
        assert!(doc.is_none());
        assert_eq!(missing, ["sentry"]);
        assert!(unavailable_note(&missing, true)
            .unwrap()
            .contains("no sentry connection"));
        assert!(unavailable_note(&missing, false)
            .unwrap()
            .contains("cannot be given MCP servers"));
        assert_eq!(unavailable_note(&[], true), None);
    }

    #[test]
    fn the_project_slug_is_optional() {
        let e = env(&[(SENTRY_TOKEN_ENV, "t"), (SENTRY_ORG_ENV, "d")]);
        let (doc, _) = resolve(&["sentry".into()], &e);
        let args = doc.unwrap()["mcpServers"]["sentry"]["args"].clone();
        assert_eq!(args.as_array().unwrap().len(), 2);
    }

    #[test]
    fn mcp_providers() {
        assert!(provider_supports_mcp(None));
        assert!(provider_supports_mcp(Some("claude")));
        assert!(provider_supports_mcp(Some("pi")));
        assert!(!provider_supports_mcp(Some("codex")));
        assert!(!provider_supports_mcp(Some("cursor")));
    }
}
