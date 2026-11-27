//! **MCP servers** a workflow step can ask for with `mcp: [name]`.
//!
//! Every server is one an admin added under Settings → MCP servers. None is
//! built in: Sentry and Grafana are *integrations* (Settings → Integrations),
//! which scripts and the chat read through the harness itself, and anyone who
//! also wants their MCP server adds it like any other.
//!
//! A step never holds a server's credentials. It reaches each server through
//! the harness's MCP gateway (`/api/run/mcp/<server>`), which holds them and
//! forwards only the tools the step may use.

use std::collections::HashMap;

use serde_json::{json, Value};

/// Providers whose agents can be given an MCP server for a step.
pub const MCP_PROVIDERS: &[&str] = &["claude", "pi", "omp", "kimi"];

/// Whether `provider` (absent = the default, Claude) can attach MCP servers.
pub fn provider_supports_mcp(provider: Option<&str>) -> bool {
    provider.is_none_or(|p| MCP_PROVIDERS.contains(&p))
}

// ── The registry, as save-time validation sees it ────────────────────────────

/// Server name → the tools an admin enabled for it, for every server added in
/// Settings. Kept here, process-wide, because a workflow is saved through many
/// doors (the editor, the MCP tools one node at a time, the library) that all
/// end in [`crate::authoring::validate_workflow`], and none of them carries a
/// database handle. The server replaces it at start-up and on every registry
/// change; until then only the built-in servers are known.
static CATALOG: std::sync::LazyLock<std::sync::RwLock<HashMap<String, Vec<String>>>> =
    std::sync::LazyLock::new(Default::default);

/// Replace the registry snapshot save-time validation checks against.
pub fn set_catalog(servers: HashMap<String, Vec<String>>) {
    *CATALOG.write().unwrap_or_else(|p| p.into_inner()) = servers;
}

/// The tools an admin enabled for `server`. `None` = no such server.
fn allowed_tools(server: &str) -> Option<Vec<String>> {
    CATALOG
        .read()
        .unwrap_or_else(|p| p.into_inner())
        .get(server)
        .cloned()
}

fn known_servers() -> Vec<String> {
    let mut names: Vec<String> = CATALOG
        .read()
        .unwrap_or_else(|p| p.into_inner())
        .keys()
        .cloned()
        .collect();
    names.sort();
    names
}

/// Check one node's `mcp:` against the registry: the agent can take MCP
/// servers, every server exists, and every tool it names is one the admin
/// enabled. A step can narrow a server's tools but never widen them.
pub fn check_uses(
    node: &str,
    provider: Option<&str>,
    uses: &[harness_dag::McpUse],
) -> Result<(), String> {
    if uses.is_empty() {
        return Ok(());
    }
    if !provider_supports_mcp(provider) {
        return Err(format!(
            "step `{node}` asks for MCP servers, but its provider `{}` cannot be limited to \
             chosen tools; use one of: {}",
            provider.unwrap_or_default(),
            MCP_PROVIDERS.join(", ")
        ));
    }
    let mut seen = std::collections::HashSet::new();
    for u in uses {
        if !seen.insert(u.server.as_str()) {
            return Err(format!(
                "step `{node}` lists MCP server `{}` twice; list it once",
                u.server
            ));
        }
        let Some(allowed) = allowed_tools(&u.server) else {
            return Err(format!(
                "step `{node}` asks for MCP server `{}`, which is not set up; known: {}",
                u.server,
                known_servers().join(", ")
            ));
        };
        if let Some(tools) = &u.tools {
            if tools.is_empty() {
                return Err(format!(
                    "step `{node}` gives MCP server `{}` an empty tool list; name at least \
                     one tool, or list the server by name for all of its enabled tools",
                    u.server
                ));
            }
            let not_enabled: Vec<&str> = tools
                .iter()
                .filter(|t| !allowed.contains(t))
                .map(String::as_str)
                .collect();
            if !not_enabled.is_empty() {
                return Err(format!(
                    "step `{node}` asks MCP server `{}` for {}, which {} not enabled for it \
                     (enable tools under Settings → MCP servers)",
                    u.server,
                    not_enabled.join(", "),
                    if not_enabled.len() == 1 { "is" } else { "are" }
                ));
            }
        }
    }
    Ok(())
}

// ── Opening a step's servers ─────────────────────────────────────────────────

/// What a step gets: its agent's MCP config, the servers it asked for but
/// can't have, and the gateway session to close when the step ends.
pub struct OpenedMcp {
    pub config: Option<Value>,
    pub missing: Vec<String>,
    pub session: Option<GatewaySession>,
}

/// A step's session on the server's MCP gateway; [`GatewaySession::close`]
/// ends it so the token stops working as soon as the step does.
pub struct GatewaySession {
    base: String,
    token: String,
}

impl GatewaySession {
    pub async fn close(self) {
        let _ = reqwest::Client::new()
            .delete(format!("{}/api/run/mcp/session", self.base))
            .bearer_auth(&self.token)
            .timeout(std::time::Duration::from_secs(10))
            .send()
            .await;
    }
}

/// The gateway's answer to opening a session.
#[derive(serde::Deserialize)]
struct SessionReply {
    token: String,
    /// Server name → the gateway URL serving it for this session.
    servers: std::collections::BTreeMap<String, String>,
    #[serde(default)]
    missing: Vec<String>,
}

/// Give a step the servers it asked for.
///
/// Every server goes through the server's MCP gateway: the agent talks to
/// `HARNESS_RUN_URL/api/run/mcp/<server>` with a token for this step alone,
/// and the gateway, which holds the real credentials, lists and allows only
/// the step's tools. The agent never sees a credential it could use to go
/// around that list.
pub async fn open(
    uses: &[harness_dag::McpUse],
    node: &str,
    env: &HashMap<String, String>,
) -> OpenedMcp {
    if uses.is_empty() {
        return OpenedMcp {
            config: None,
            missing: Vec::new(),
            session: None,
        };
    }
    let uses: Vec<&harness_dag::McpUse> = uses.iter().collect();
    let mut servers = serde_json::Map::new();
    let mut missing = Vec::new();
    let mut session = None;
    match open_session(&uses, node, env).await {
        Ok(reply) => {
            for (name, url) in reply.servers {
                servers.insert(
                    name,
                    json!({
                        "type": "http",
                        "url": url,
                        "headers": { "Authorization": format!("Bearer {}", reply.token) },
                    }),
                );
            }
            missing.extend(reply.missing);
            session = Some(GatewaySession {
                base: run_url(env).unwrap_or_default(),
                token: reply.token,
            });
        }
        Err(e) => {
            tracing::warn!(node, "mcp gateway: {e}");
            missing.extend(uses.iter().map(|u| u.server.clone()));
        }
    }
    OpenedMcp {
        config: (!servers.is_empty()).then(|| json!({ "mcpServers": servers })),
        missing,
        session,
    }
}

fn run_url(env: &HashMap<String, String>) -> Option<String> {
    env_value(env, "HARNESS_RUN_URL").map(|s| s.trim_end_matches('/').to_string())
}

async fn open_session(
    uses: &[&harness_dag::McpUse],
    node: &str,
    env: &HashMap<String, String>,
) -> Result<SessionReply, String> {
    let base = run_url(env).ok_or("no HARNESS_RUN_URL (the server has no public URL set)")?;
    let token = env_value(env, "HARNESS_RUN_TOKEN").ok_or("no HARNESS_RUN_TOKEN")?;
    let resp = reqwest::Client::new()
        .post(format!("{base}/api/run/mcp/session"))
        .bearer_auth(token)
        .json(&json!({ "node": node, "uses": uses }))
        .timeout(std::time::Duration::from_secs(60))
        .send()
        .await
        .map_err(|e| e.to_string())?;
    let status = resp.status();
    if !status.is_success() {
        let body = resp.text().await.unwrap_or_default();
        return Err(format!("{status}: {body}"));
    }
    resp.json::<SessionReply>().await.map_err(|e| e.to_string())
}

fn env_value<'a>(env: &'a HashMap<String, String>, key: &str) -> Option<&'a str> {
    env.get(key)
        .map(String::as_str)
        .filter(|v| !v.trim().is_empty())
}

/// The name the always-on browser server goes by in a Claude step's MCP
/// config. Not a server anyone asks for with `mcp:`:
/// it is not listed anywhere a person picks servers, and the registry refuses
/// the name so a configured server can never stand in for it.
pub const BROWSER_SERVER: &str = "browser";

/// Where the image's Chromium is, or `None` when there is no browser to drive
/// (a developer machine, an image built without it). `CHROME_PATH` is what the
/// image sets; `/usr/bin/chromium` is where Debian puts it.
pub fn chromium_path() -> Option<std::path::PathBuf> {
    let candidate = std::env::var("CHROME_PATH")
        .ok()
        .filter(|v| !v.trim().is_empty())
        .unwrap_or_else(|| "/usr/bin/chromium".into());
    let path = std::path::PathBuf::from(candidate);
    path.is_file().then_some(path)
}

/// The browser every Claude step gets: Playwright's MCP server driving the
/// image's own Chromium, headless, with a throwaway profile. Screenshots it
/// saves without a path land in `screenshots_dir`, which the server keeps
/// after the run.
///
/// omp steps are not given this: omp has its own browser tool on the same
/// Chromium.
pub fn browser_config(chromium: &std::path::Path, screenshots_dir: &std::path::Path) -> Value {
    let command = std::env::var("HARNESS_BROWSER_MCP")
        .ok()
        .filter(|v| !v.is_empty())
        .unwrap_or_else(|| "playwright-mcp".into());
    json!({
        "type": "stdio",
        "command": command,
        "args": [
            "--headless",
            // In memory: nothing from one step's session leaks into the next.
            "--isolated",
            // A container has no user namespaces for Chromium's sandbox; the
            // image's own CHROMIUM_FLAGS says the same.
            "--no-sandbox",
            "--executable-path",
            chromium.to_string_lossy(),
            "--output-dir",
            screenshots_dir.to_string_lossy(),
        ],
        "env": {
            // It must never fetch a browser of its own at run time.
            "PLAYWRIGHT_SKIP_BROWSER_DOWNLOAD": "1",
        }
    })
}

/// Add the browser to a step's MCP config document, creating the document when
/// the step asked for no other server. A step's own servers are kept.
pub fn with_browser(config: Option<Value>, browser: Value) -> Value {
    let mut doc = config.unwrap_or_else(|| json!({ "mcpServers": {} }));
    if let Some(servers) = doc.get_mut("mcpServers").and_then(Value::as_object_mut) {
        servers.insert(BROWSER_SERVER.to_string(), browser);
    }
    doc
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
            "Note: this step asked for the MCP server(s) {names}, which this harness can't \
             reach (not set up under Settings → MCP servers, or not signed in), so those \
             tools are not available. Work without them."
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

    #[test]
    fn mcp_providers() {
        assert!(provider_supports_mcp(None));
        assert!(provider_supports_mcp(Some("claude")));
        assert!(provider_supports_mcp(Some("pi")));
        assert!(!provider_supports_mcp(Some("codex")));
        assert!(!provider_supports_mcp(Some("cursor")));
    }

    /// One test, because the catalog is process-wide and tests run in parallel.
    #[test]
    fn a_step_may_narrow_a_servers_enabled_tools_but_never_widen_them() {
        use harness_dag::McpUse;
        let narrowed = |server: &str, tools: &[&str]| McpUse {
            server: server.into(),
            tools: Some(tools.iter().map(|t| t.to_string()).collect()),
        };
        set_catalog(HashMap::from([(
            "m365".to_string(),
            vec!["outlook_email_search".to_string()],
        )]));

        // Servers are known once the registry has them; nothing is built in.
        assert!(check_uses("s", None, &[McpUse::server("sentry")]).is_err());
        assert!(check_uses("s", None, &[McpUse::server("m365")]).is_ok());
        assert!(check_uses("s", None, &[narrowed("m365", &["outlook_email_search"])]).is_ok());

        let unknown = check_uses("s", None, &[McpUse::server("datadog")]).unwrap_err();
        assert!(
            unknown.contains("`datadog`") && unknown.contains("m365"),
            "{unknown}"
        );

        let widened = check_uses("s", None, &[narrowed("m365", &["send_mail"])]).unwrap_err();
        assert!(
            widened.contains("send_mail") && widened.contains("not enabled"),
            "{widened}"
        );

        assert!(check_uses("s", None, &[narrowed("m365", &[])]).is_err());
        assert!(check_uses("s", None, &[McpUse::server("m365"), McpUse::server("m365")]).is_err());

        let cursor = check_uses("s", Some("cursor"), &[McpUse::server("m365")]).unwrap_err();
        assert!(cursor.contains("cursor"), "{cursor}");
        // No servers is fine for any provider.
        assert!(check_uses("s", Some("cursor"), &[]).is_ok());

        // A server removed from the registry is unknown again.
        set_catalog(HashMap::new());
        assert!(check_uses("s", None, &[McpUse::server("m365")]).is_err());
    }

    /// The browser drives the image's Chromium, headless and throwaway, and
    /// writes screenshots where the server keeps them.
    #[test]
    fn the_browser_drives_the_images_chromium_and_saves_to_the_run() {
        let cfg = browser_config(
            std::path::Path::new("/usr/bin/chromium"),
            std::path::Path::new("/w/.harness/artifacts/screenshots"),
        );
        assert_eq!(cfg["type"], "stdio");
        let args: Vec<&str> = cfg["args"]
            .as_array()
            .unwrap()
            .iter()
            .map(|a| a.as_str().unwrap())
            .collect();
        for flag in ["--headless", "--isolated", "--no-sandbox"] {
            assert!(args.contains(&flag), "{flag} missing: {args:?}");
        }
        let after = |flag: &str| args[args.iter().position(|a| *a == flag).unwrap() + 1];
        assert_eq!(after("--executable-path"), "/usr/bin/chromium");
        assert_eq!(after("--output-dir"), "/w/.harness/artifacts/screenshots");
        assert_eq!(cfg["env"]["PLAYWRIGHT_SKIP_BROWSER_DOWNLOAD"], "1");
    }

    /// Added beside the step's own servers, and never offered to a person.
    #[test]
    fn the_browser_joins_a_steps_servers_and_is_not_listed() {
        let own = json!({ "mcpServers": { "m365": { "type": "http" } } });
        let doc = with_browser(Some(own), json!({ "type": "stdio" }));
        assert!(doc["mcpServers"]["m365"].is_object());
        assert!(doc["mcpServers"][BROWSER_SERVER].is_object());

        let alone = with_browser(None, json!({ "type": "stdio" }));
        assert_eq!(alone["mcpServers"].as_object().unwrap().len(), 1);

        assert!(!known_servers().contains(&BROWSER_SERVER.to_string()));
    }
}
