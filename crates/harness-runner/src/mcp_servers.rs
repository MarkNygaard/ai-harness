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

/// Run-environment keys the harness sets when Grafana is connected. Namespaced
/// like Sentry's: `mcp-grafana` reads `GRAFANA_URL` and
/// `GRAFANA_SERVICE_ACCOUNT_TOKEN`, mapped in its own process only.
pub const GRAFANA_URL_ENV: &str = "HARNESS_GRAFANA_URL";
pub const GRAFANA_TOKEN_ENV: &str = "HARNESS_GRAFANA_TOKEN";

/// The tool categories `mcp-grafana` is started with: what Prometheus/Loki
/// reporting and investigation need. Its default also turns on OnCall,
/// Incident, Tempo, Pyroscope, rendering, the raw API proxy (which the docs
/// say bypasses Loki scoping) and more.
const GRAFANA_CATEGORIES: &str =
    "search,datasource,prometheus,loki,dashboard,folder,navigation,annotations,alerting";

/// The tools `mcp-grafana` 2.0.2 offers with `--disable-write` and
/// [`GRAFANA_CATEGORIES`], as its `tools/list` reports them. Listed for omp's
/// `--tools`; re-check on a version bump.
pub const GRAFANA_TOOLS: &[&str] = &[
    "alerting_manage_routing",
    "alerting_rules_read",
    "alerting_silences_read",
    "analyze_loki_labels",
    "check_datasources_health",
    "generate_deeplink",
    "get_annotation_tags",
    "get_annotations",
    "get_dashboard_by_uid",
    "get_dashboard_panel_queries",
    "get_dashboard_property",
    "get_dashboard_summary",
    "get_datasource",
    "list_dashboard_versions",
    "list_datasources",
    "list_loki_label_names",
    "list_loki_label_values",
    "list_prometheus_label_names",
    "list_prometheus_label_values",
    "list_prometheus_metric_metadata",
    "list_prometheus_metric_names",
    "query_loki_logs",
    "query_loki_patterns",
    "query_loki_stats",
    "query_prometheus",
    "query_prometheus_histogram",
    "search_dashboards",
    "search_folders",
];

/// The tools a known server offers, by name (empty for an unknown one).
pub fn tool_names(server: &str) -> &'static [&'static str] {
    match server {
        "sentry" => SENTRY_TOOLS,
        "grafana" => GRAFANA_TOOLS,
        _ => &[],
    }
}

/// Providers whose agents can be given an MCP server for a step.
pub const MCP_PROVIDERS: &[&str] = &["claude", "pi", "omp", "kimi"];

/// Whether `provider` (absent = the default, Claude) can attach MCP servers.
pub fn provider_supports_mcp(provider: Option<&str>) -> bool {
    provider.is_none_or(|p| MCP_PROVIDERS.contains(&p))
}

/// The servers built into the harness. Every other name is a server someone
/// added in Settings → MCP servers, and a registry entry may not take one of
/// these names.
pub const BUILTIN_SERVERS: &[&str] = &["sentry", "grafana"];

pub fn is_builtin(name: &str) -> bool {
    BUILTIN_SERVERS.contains(&name)
}

/// The run-environment keys each built-in server's config reads.
const SERVER_ENV: &[(&str, &[&str])] = &[
    (
        "sentry",
        &[SENTRY_TOKEN_ENV, SENTRY_ORG_ENV, SENTRY_PROJECT_ENV],
    ),
    ("grafana", &[GRAFANA_URL_ENV, GRAFANA_TOKEN_ENV]),
];

/// A step's environment: the run's, without the Sentry and Grafana
/// credentials unless the step asked for that server in `mcp:`. The run
/// carries them for the steps that do, and every other step, a bash step
/// included, has no use for a token it was never meant to have.
pub fn env_for_step(
    env: &HashMap<String, String>,
    requested: &[harness_dag::McpUse],
) -> HashMap<String, String> {
    let mut out = env.clone();
    for (server, keys) in SERVER_ENV {
        if !requested.iter().any(|u| u.server == *server) {
            for key in *keys {
                out.remove(*key);
            }
        }
    }
    out
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

/// The tools a step may name for `server`: a built-in's fixed set, or the ones
/// an admin enabled for a registry server. `None` = no such server.
fn allowed_tools(server: &str) -> Option<Vec<String>> {
    if is_builtin(server) {
        return Some(tool_names(server).iter().map(|t| t.to_string()).collect());
    }
    CATALOG
        .read()
        .unwrap_or_else(|p| p.into_inner())
        .get(server)
        .cloned()
}

fn known_servers() -> Vec<String> {
    let mut names: Vec<String> = BUILTIN_SERVERS.iter().map(|s| s.to_string()).collect();
    names.extend(
        CATALOG
            .read()
            .unwrap_or_else(|p| p.into_inner())
            .keys()
            .cloned(),
    );
    names.sort();
    names.dedup();
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
/// A built-in server the step takes whole is started by the agent directly, as
/// before. Everything else — a server added in Settings, or a built-in the step
/// narrows to some tools — goes through the server's MCP gateway: the agent
/// talks to `HARNESS_RUN_URL/api/run/mcp/<server>` with a token for this step
/// alone, and the gateway, which holds the real credentials, lists and allows
/// only the step's tools. The agent never sees a credential it could use to go
/// around that list.
pub async fn open(
    uses: &[harness_dag::McpUse],
    node: &str,
    env: &HashMap<String, String>,
) -> OpenedMcp {
    let (direct, gated): (Vec<_>, Vec<_>) = uses
        .iter()
        .partition(|u| is_builtin(&u.server) && u.tools.is_none());
    let direct_names: Vec<String> = direct.iter().map(|u| u.server.clone()).collect();
    let (doc, mut missing) = resolve(&direct_names, env);
    let mut servers = doc
        .and_then(|d| d.get("mcpServers").and_then(Value::as_object).cloned())
        .unwrap_or_default();
    let mut session = None;
    if !gated.is_empty() {
        match open_session(&gated, node, env).await {
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
                missing.extend(gated.iter().map(|u| u.server.clone()));
            }
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

/// Grafana's own MCP server (`mcp-grafana`), for Prometheus metrics, Loki logs
/// and dashboards.
///
/// Read-only twice over, like Sentry's: the service account the harness asks
/// for has the Viewer role, and `--disable-write` drops every create/update
/// tool. Only the categories in [`GRAFANA_CATEGORIES`] are on, and its
/// anonymous usage reporting to Grafana Labs (on by default) is off.
fn grafana(env: &HashMap<String, String>) -> Option<Value> {
    env_value(env, GRAFANA_URL_ENV)?;
    env_value(env, GRAFANA_TOKEN_ENV)?;
    let command = std::env::var("HARNESS_GRAFANA_MCP")
        .ok()
        .filter(|v| !v.is_empty())
        .unwrap_or_else(|| "mcp-grafana".into());
    Some(json!({
        "type": "stdio",
        "command": command,
        "args": [
            "--usage-stats=disabled",
            "--disable-write",
            format!("--enabled-tools={GRAFANA_CATEGORIES}"),
        ],
        "env": {
            "GRAFANA_URL": format!("${{{GRAFANA_URL_ENV}}}"),
            "GRAFANA_SERVICE_ACCOUNT_TOKEN": format!("${{{GRAFANA_TOKEN_ENV}}}"),
            // Belt and braces with --usage-stats=disabled.
            "DO_NOT_TRACK": "1",
        }
    }))
}

/// The name the always-on browser server goes by in a Claude step's MCP
/// config. Not a [`BUILTIN_SERVERS`] entry: nobody asks for it with `mcp:`,
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

/// One requested server's config, or `None` when the project doesn't have it.
fn config_for(name: &str, env: &HashMap<String, String>) -> Option<Value> {
    match name {
        "sentry" => sentry(env),
        "grafana" => grafana(env),
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
    fn grafana_is_read_only_focused_and_quiet() {
        let e = env(&[
            (GRAFANA_URL_ENV, "https://grafana.dilling.example"),
            (GRAFANA_TOKEN_ENV, "glsa_secret"),
        ]);
        let (doc, missing) = resolve(&["grafana".into(), "sentry".into()], &e);
        // Sentry isn't connected here; Grafana is.
        assert_eq!(missing, ["sentry"]);
        let g = &doc.unwrap()["mcpServers"]["grafana"];
        let args: Vec<&str> = g["args"]
            .as_array()
            .unwrap()
            .iter()
            .map(|a| a.as_str().unwrap())
            .collect();
        assert!(args.contains(&"--disable-write"));
        assert!(args.contains(&"--usage-stats=disabled"));
        let cats = args
            .iter()
            .find_map(|a| a.strip_prefix("--enabled-tools="))
            .unwrap();
        assert!(cats.contains("prometheus") && cats.contains("loki"));
        assert!(!cats.contains("api"), "the raw API proxy stays off");
        assert_eq!(g["env"]["GRAFANA_URL"], "${HARNESS_GRAFANA_URL}");
        assert_eq!(
            g["env"]["GRAFANA_SERVICE_ACCOUNT_TOKEN"],
            "${HARNESS_GRAFANA_TOKEN}"
        );
        assert!(!g.to_string().contains("glsa_secret"));
        assert_eq!(tool_names("grafana").len(), 28);

        // Without a token it isn't offered.
        let (doc, missing) = resolve(
            &["grafana".into()],
            &env(&[(GRAFANA_URL_ENV, "https://g.example")]),
        );
        assert!(doc.is_none());
        assert_eq!(missing, ["grafana"]);
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

        // Built-ins are always known; registry servers once the catalog has them.
        assert!(check_uses("s", None, &[McpUse::server("sentry")]).is_ok());
        assert!(check_uses("s", None, &[McpUse::server("m365")]).is_ok());
        assert!(check_uses("s", None, &[narrowed("m365", &["outlook_email_search"])]).is_ok());
        assert!(check_uses("s", None, &[narrowed("sentry", &["search_issues"])]).is_ok());

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

        let cursor = check_uses("s", Some("cursor"), &[McpUse::server("sentry")]).unwrap_err();
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
        let own = json!({ "mcpServers": { "sentry": { "type": "stdio" } } });
        let doc = with_browser(Some(own), json!({ "type": "stdio" }));
        assert!(doc["mcpServers"]["sentry"].is_object());
        assert!(doc["mcpServers"][BROWSER_SERVER].is_object());

        let alone = with_browser(None, json!({ "type": "stdio" }));
        assert_eq!(alone["mcpServers"].as_object().unwrap().len(), 1);

        assert!(!is_builtin(BROWSER_SERVER));
        assert!(!known_servers().contains(&BROWSER_SERVER.to_string()));
    }
}

#[cfg(test)]
mod step_env_tests {
    use super::*;

    fn run_env() -> HashMap<String, String> {
        [
            (SENTRY_TOKEN_ENV, "sntry"),
            (SENTRY_ORG_ENV, "org"),
            (GRAFANA_URL_ENV, "https://g"),
            (GRAFANA_TOKEN_ENV, "gfn"),
            ("HARNESS_CACHE_DIR", "/cache"),
        ]
        .into_iter()
        .map(|(k, v)| (k.to_string(), v.to_string()))
        .collect()
    }

    #[test]
    fn a_step_gets_only_the_server_credentials_it_asked_for() {
        let sentry_only = env_for_step(&run_env(), &[harness_dag::McpUse::server("sentry")]);
        assert_eq!(
            sentry_only.get(SENTRY_TOKEN_ENV).map(String::as_str),
            Some("sntry")
        );
        assert!(!sentry_only.contains_key(GRAFANA_TOKEN_ENV));
        assert!(!sentry_only.contains_key(GRAFANA_URL_ENV));

        // A bash step, or an agent step without `mcp:`, gets neither.
        let none = env_for_step(&run_env(), &[]);
        assert!(!none.contains_key(SENTRY_TOKEN_ENV));
        assert!(!none.contains_key(SENTRY_ORG_ENV));
        assert!(!none.contains_key(GRAFANA_TOKEN_ENV));
        // Everything else in the run's environment is untouched.
        assert_eq!(
            none.get("HARNESS_CACHE_DIR").map(String::as_str),
            Some("/cache")
        );
    }
}
