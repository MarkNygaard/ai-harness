//! The workflow DAG model.
//!
//! A [`Workflow`] is a list of [`Node`]s connected by `depends_on` edges. Each
//! node carries optional AI options (provider/model/context) and exactly one
//! executable *body* ([`NodeKind`]): a prompt, a bash script, a referenced
//! command, an inline script, a convergence loop, a human approval gate, or a
//! cancel.
//!
//! The format is a deliberately concise pipeline YAML, typed and validated in
//! Rust. Parsing and validation live in [`crate::parse`]; this module is the
//! data model only.

use serde::{Deserialize, Serialize};

/// `#[serde(skip_serializing_if)]` predicate for `u32` fields that default to 0.
fn is_zero(n: &u32) -> bool {
    *n == 0
}

/// Session-handling mode for an AI node.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ContextMode {
    /// Always run with a brand-new agent session (parallelizable).
    Fresh,
    /// Inherit the session from the previous sequential node (default).
    #[default]
    Shared,
}

/// How a node reacts to the terminal states of its upstream dependencies.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TriggerRule {
    /// Run only if every dependency succeeded (default).
    #[default]
    AllSuccess,
    /// Run if at least one dependency succeeded.
    OneSuccess,
    /// Run if no dependency failed and at least one succeeded.
    NoneFailedMinOneSuccess,
    /// Run once every dependency reached a terminal state, regardless of result.
    AllDone,
    /// Run only if a step before this one failed: a dependency, or anything
    /// it depends on in turn. A step after a failure is *skipped*, not failed,
    /// so a cleanup step at the end would never see the failure in its direct
    /// dependencies alone. For handling a failure — labelling the issue,
    /// saying what broke. Skipped when nothing failed; the run stays failed
    /// either way.
    OneFailed,
}

/// Runtime for an inline [`NodeKind::Script`] body.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ScriptRuntime {
    /// TypeScript/JavaScript via `bun`.
    Bun,
    /// Python via `uv`.
    Uv,
}

/// Configuration for a convergence [`NodeKind::Loop`].
///
/// The loop re-runs `prompt` until the agent emits the `until` signal (see
/// [`crate::vars`] consumers / the executor for detection) or `max_iterations`
/// is reached.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LoopConfig {
    /// Prompt executed each iteration.
    pub prompt: String,
    /// Completion signal string; emitting it (e.g. wrapped in a tag) ends the loop.
    pub until: String,
    /// Hard cap on iterations.
    pub max_iterations: u32,
    /// Provider for the loop body, overriding the node/workflow provider when set.
    /// Some workflows declare provider/model *inside* the `loop:` block.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider: Option<String>,
    /// Model for the loop body, overriding the node/workflow model when set.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    /// Start each iteration with a fresh session instead of reusing the loop's.
    #[serde(default)]
    pub fresh_context: bool,
    /// Optional shell command run after each iteration; exit 0 also ends the loop.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub until_bash: Option<String>,
    /// Pause for user input between iterations.
    #[serde(default)]
    pub interactive: bool,
    /// Message shown when paused (used with `interactive`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub gate_message: Option<String>,
}
/// Configuration for a human [`NodeKind::Approval`] gate.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ApprovalConfig {
    /// Message presented to the human approver.
    pub message: String,
    /// Whether the approver's free-text response is captured for downstream use.
    #[serde(default)]
    pub capture_response: bool,
    /// Node id to route to if the human rejects (optional).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub on_reject: Option<String>,
}

/// One entry of a node's `mcp:` list: a server by name, optionally narrowed to
/// some of its tools.
///
/// Written as a bare name (`mcp: [sentry]` — every tool the admin enabled for
/// that server) or as a map (`mcp: [{server: m365, tools: [mail_search]}]`).
/// Whether the server exists, and whether the tools are ones the admin enabled,
/// is checked when the workflow is saved, against the install's MCP registry —
/// parsing cannot see it. A run that meets a server it can't start runs the
/// step without it and says so in the prompt.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct McpUse {
    pub server: String,
    /// `None` = every tool the admin enabled for the server. `Some` narrows
    /// that set; it can never widen it.
    pub tools: Option<Vec<String>>,
}

impl McpUse {
    pub fn server(name: impl Into<String>) -> Self {
        Self {
            server: name.into(),
            tools: None,
        }
    }
}

impl Serialize for McpUse {
    fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        match &self.tools {
            None => s.serialize_str(&self.server),
            Some(tools) => {
                use serde::ser::SerializeMap;
                let mut m = s.serialize_map(Some(2))?;
                m.serialize_entry("server", &self.server)?;
                m.serialize_entry("tools", tools)?;
                m.end()
            }
        }
    }
}

impl<'de> Deserialize<'de> for McpUse {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        #[derive(Deserialize)]
        #[serde(untagged)]
        enum Raw {
            Name(String),
            Full {
                server: String,
                #[serde(default)]
                tools: Option<Vec<String>>,
            },
        }
        Ok(match Raw::deserialize(d)? {
            Raw::Name(server) => McpUse {
                server,
                tools: None,
            },
            Raw::Full { server, tools } => McpUse { server, tools },
        })
    }
}

/// The effort levels a node may set. Each agent that takes effort knows all
/// of them: Claude's `--effort` and omp's `--thinking`.
pub const EFFORT_LEVELS: &[&str] = &["low", "medium", "high", "xhigh", "max"];

/// The providers whose CLI takes an effort level. cursor-agent has no such
/// setting, so effort on a cursor step is ignored; validation says so.
pub const EFFORT_PROVIDERS: &[&str] = &["claude", "pi", "omp", "kimi"];

/// Whether `provider` (absent = the default agent, Claude) takes `effort`.
pub fn provider_takes_effort(provider: Option<&str>) -> bool {
    provider.is_none_or(|p| EFFORT_PROVIDERS.contains(&p))
}

/// Whether `name` can name an MCP server: lowercase letters, digits, `-` and
/// `_`, starting with a letter or digit, at most 40 long. The name becomes the
/// prefix of the agent's tool names (`mcp__<name>__<tool>`), so it stays plain.
pub fn is_valid_mcp_server_name(name: &str) -> bool {
    let mut chars = name.chars();
    matches!(chars.next(), Some(c) if c.is_ascii_lowercase() || c.is_ascii_digit())
        && name.len() <= 40
        && chars.all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-' || c == '_')
}

/// Provider-agnostic per-node tool hooks. Translated by the runner into Claude
/// Code settings.json hooks, the omp hook extension, or Cursor project hooks.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct NodeHooks {
    /// Fired before a tool runs; a `deny` decision blocks the call.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub pre_tool_use: Vec<HookRule>,
    /// Fired after a tool runs; used for non-blocking steering / context.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub post_tool_use: Vec<HookRule>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HookRule {
    /// Regex over the tool name (e.g. "Write|Edit", "Bash", "Read").
    /// Absent/empty = match every tool.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub matcher: Option<String>,
    /// Decision for matching tools (pre_tool_use only). `None` = observe/inject.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub decision: Option<HookDecision>,
    /// Reason surfaced to the agent when denied.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    /// Extra context injected for the agent (non-blocking).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub additional_context: Option<String>,
    /// System-level message surfaced to the user/agent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub system_message: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HookDecision {
    Allow,
    Deny,
    Ask,
}

/// One Linear operation, performed declaratively by a step.
///
/// These already exist as run-scoped endpoints under `/api/run/linear/*`, but
/// until now the only ways to reach them were an AI step *choosing* to call the
/// MCP tool, or the poller applying a binding's status map. Neither is visible
/// in the graph. As a node kind the operation is explicit, ordered against the
/// rest of the DAG, and skipped on its own terms when Linear isn't in play.
///
/// Every variant acts on the issue the run was started from (`$ISSUE_ID`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LinearAction {
    /// Move the issue to a status, by name (resolved against the issue's team)
    /// or by id.
    MoveState(String),
    /// Post a comment on the issue.
    Comment(String),
    /// Add labels to the issue, by name.
    AddLabels(Vec<String>),
    /// Remove labels from the issue, by name.
    RemoveLabels(Vec<String>),
}

impl LinearAction {
    /// A short phrase naming what this does, for logs and the skip reason.
    pub fn summary(&self) -> String {
        match self {
            LinearAction::MoveState(s) => format!("move the issue to {s}"),
            LinearAction::Comment(_) => "comment on the issue".to_string(),
            LinearAction::AddLabels(l) => format!("label the issue {}", l.join(", ")),
            LinearAction::RemoveLabels(l) => format!("unlabel the issue {}", l.join(", ")),
        }
    }
}

/// The executable body of a node. Exactly one variant per node.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NodeKind {
    /// Inline AI prompt.
    Prompt(String),
    /// Shell script body.
    Bash(String),
    /// Reference to a markdown command resolved from `.harness/commands/`.
    Command(String),
    /// Inline script with an explicit runtime and optional dependencies.
    Script {
        script: String,
        runtime: ScriptRuntime,
        deps: Vec<String>,
    },
    /// Convergence loop.
    Loop(LoopConfig),
    /// Human approval gate.
    Approval(ApprovalConfig),
    /// Terminate the run with a reason.
    Cancel(String),
    /// A Linear operation on the issue the run came from. Skipped, not failed,
    /// when the run has no issue or the project has no Linear connection — so a
    /// prebuilt workflow can carry these steps and stay usable without Linear.
    Linear(LinearAction),
    /// Choose one of several named routes. Downstream steps follow a route
    /// with `when: "$<id>.output.route == '<name>'"`.
    Route(RouteConfig),
}

impl NodeKind {
    /// Whether this body is executed by an AI provider (vs. deterministic).
    /// A route is, when it may ask a model to choose.
    pub fn is_ai(&self) -> bool {
        match self {
            NodeKind::Prompt(_) | NodeKind::Command(_) | NodeKind::Loop(_) => true,
            NodeKind::Route(cfg) => cfg.prompt.is_some(),
            _ => false,
        }
    }
}

/// One exit of a [`NodeKind::Route`] node.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RouteOption {
    /// What downstream steps compare against: `$<id>.output.route == '<name>'`.
    pub name: String,
    /// What this route means — for a person reading the workflow, and for the
    /// model when it chooses.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub description: String,
    /// A rule that takes this route without asking a model, over earlier
    /// steps' outputs. Checked in declaration order; the first true one wins.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub when: Option<String>,
}

/// A route node: one input, a named exit per outcome.
///
/// It decides in three steps, stopping at the first that answers:
/// 1. each route's `when:` rule, in order — certain and free;
/// 2. the `prompt`, when given — a model picks a route from their
///    descriptions, for what no field can answer;
/// 3. the `default`.
///
/// Its output is `{"route": "<name>", "reason": "…", "by": "rule" | "model" |
/// "default"}`, so a downstream step reads `$<id>.output.route`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RouteConfig {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub prompt: Option<String>,
    pub routes: Vec<RouteOption>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default: Option<String>,
}

impl RouteConfig {
    pub fn names(&self) -> Vec<&str> {
        self.routes.iter().map(|r| r.name.as_str()).collect()
    }

    /// The JSON a model must answer with: one of the route names.
    pub fn output_schema(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "route": { "type": "string", "enum": self.names() },
                "reason": { "type": "string" }
            },
            "required": ["route"]
        })
    }

    /// Appended to the prompt: the routes to choose between.
    pub fn choices(&self) -> String {
        let mut out =
            String::from("\n\n## Choose a route\n\nPick exactly one of these, by name:\n\n");
        for r in &self.routes {
            if r.description.trim().is_empty() {
                out.push_str(&format!("- `{}`\n", r.name));
            } else {
                out.push_str(&format!("- `{}` — {}\n", r.name, r.description.trim()));
            }
        }
        out.push_str(
            "\nAnswer with the route's name in `route`, and in `reason` one sentence on why.",
        );
        out
    }

    /// The route a model's answer names, with its reason — `None` when the
    /// answer names none of them.
    pub fn read_choice(&self, answer: &str) -> Option<(String, String)> {
        let t = answer.trim();
        let t = t
            .strip_prefix("```json")
            .or_else(|| t.strip_prefix("```"))
            .map(|s| s.trim_end().trim_end_matches("```"))
            .unwrap_or(t)
            .trim();
        let value: serde_json::Value = serde_json::from_str(t).ok().or_else(|| {
            let (a, b) = (t.find('{')?, t.rfind('}')?);
            serde_json::from_str(&t[a..=b]).ok()
        })?;
        let route = value.get("route")?.as_str()?.trim();
        let name = self
            .routes
            .iter()
            .find(|r| r.name.eq_ignore_ascii_case(route))?
            .name
            .clone();
        let reason = value
            .get("reason")
            .and_then(|r| r.as_str())
            .unwrap_or_default()
            .trim()
            .to_string();
        Some((name, reason))
    }
}

/// Marks a node as a *scorer*: a step that grades the run it is in. Its
/// output is JSON `{ score, classification?, summary? }` (give it an
/// `output_format`), and the server records the grade with the run, so a
/// scorer's grades can be followed across runs on its own page.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ScorerConfig {
    /// The scorer's name, shared by every workflow that runs it, so its
    /// grades add up on one page (e.g. `redundant-tests`). Lowercase letters,
    /// digits and hyphens.
    pub name: String,
    /// The lowest score that passes, 0–1. Default 0.5.
    #[serde(default = "default_pass_at")]
    pub pass_at: f64,
    /// What it judges, in a sentence, for its page.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
}

// `pass_at` is checked to be a number in 0..=1 when the workflow is parsed, so
// it is never NaN and equality is total.
impl Eq for ScorerConfig {}

fn default_pass_at() -> f64 {
    0.5
}

/// Whether `name` can name a scorer: lowercase letters, digits and hyphens,
/// starting with a letter.
pub fn is_valid_scorer_name(name: &str) -> bool {
    let mut chars = name.chars();
    matches!(chars.next(), Some('a'..='z'))
        && chars.all(|c| matches!(c, 'a'..='z' | '0'..='9' | '-'))
        && name.len() <= 64
}

/// A single node in the workflow DAG.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Node {
    /// Unique node identifier within the workflow.
    pub id: String,
    /// Ids of nodes that must reach a terminal state before this one runs.
    pub depends_on: Vec<String>,
    /// Optional conditional-execution expression (evaluated by the executor).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub when: Option<String>,
    /// How this node reacts to its dependencies' terminal states.
    pub trigger_rule: TriggerRule,
    /// Provider override (falls back to the workflow default, then config).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub provider: Option<String>,
    /// Model override.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    /// Reasoning-effort override for AI bodies, one of [`EFFORT_LEVELS`],
    /// passed to the agent CLI: `--effort` for Claude, `--thinking` for omp.
    /// cursor-agent has no such setting and
    /// ignores it. Higher effort suits high-leverage steps like planning.
    /// `None` → agent default.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub effort: Option<String>,
    /// Session-handling mode for AI bodies.
    pub context: ContextMode,
    /// Optional category id (e.g. `planning`, `implementation`, `validation`)
    /// grouping this step for the run overview's time-by-category breakdown and
    /// bar colouring. Free-form; resolved to a colour via the categories
    /// registry at render time. `None` → the node falls back to status colour.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub category: Option<String>,
    /// Optional artifact this node produces — a path relative to the run's
    /// artifacts dir (e.g. `exploration.md`). Surfaced in the step popup.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub artifact: Option<String>,
    /// Timeout in milliseconds (for `bash`/`script` bodies).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub timeout: Option<u64>,
    /// How many times to **re-run this single node** if it fails, before the
    /// failure propagates and fails the run. `0` (default) = run once, no retry.
    /// The retry re-executes only this node (in the same worktree/session),
    /// not the whole workflow. Use it for steps that can fail transiently
    /// (tests, builds, flaky network) — NOT for side-effecting steps like a
    /// push/PR step, where a retry could double-act. Capped at a small ceiling
    /// by the executor.
    #[serde(default, skip_serializing_if = "is_zero")]
    pub retries: u32,
    /// Optional JSON schema the AI body's output should conform to. When set on a
    /// `prompt`/`command` node the runner instructs the agent to emit JSON
    /// matching it, so downstream `$node.output.field` access and `when:`
    /// conditions read a stable shape. Ignored for deterministic bodies.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub output_format: Option<serde_json::Value>,
    /// Provider-agnostic tool hooks translated per provider at dispatch.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hooks: Option<NodeHooks>,
    /// External MCP servers this step's agent gets, each optionally narrowed
    /// to some of its tools (see [`McpUse`]). Attached only when the server is
    /// connected; otherwise the step runs without it. AI bodies only.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub mcp: Vec<McpUse>,
    /// When the run was started from a Linear issue, post this node's output
    /// to the issue when the run completes — in place of the bare "completed"
    /// note. At most one node per workflow.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub linear_comment: bool,
    /// This node grades the run: its JSON output is recorded as a scorer's
    /// grade (see [`ScorerConfig`]).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub scorer: Option<ScorerConfig>,
    /// The executable body.
    pub kind: NodeKind,
}

impl Node {
    /// Every inline text this node contributes to variable substitution: its
    /// body text plus its `when:` expression. Used to collect `$id.output`
    /// references for validation.
    pub fn substitutable_text(&self) -> Vec<&str> {
        let mut texts: Vec<&str> = Vec::new();
        if let Some(w) = &self.when {
            texts.push(w);
        }
        match &self.kind {
            NodeKind::Prompt(t) | NodeKind::Bash(t) | NodeKind::Cancel(t) => texts.push(t),
            NodeKind::Script { script, .. } => texts.push(script),
            NodeKind::Loop(cfg) => {
                texts.push(&cfg.prompt);
                if let Some(b) = &cfg.until_bash {
                    texts.push(b);
                }
            }
            // A comment is the obvious place to want `$TASK_TITLE` or an
            // upstream node's output; a status or label name can be built the
            // same way, so every Linear field substitutes.
            NodeKind::Linear(action) => match action {
                LinearAction::MoveState(t) | LinearAction::Comment(t) => texts.push(t),
                LinearAction::AddLabels(l) | LinearAction::RemoveLabels(l) => {
                    texts.extend(l.iter().map(String::as_str))
                }
            },
            // The prompt substitutes like any other; each rule is a `when:`.
            NodeKind::Route(cfg) => {
                if let Some(p) = &cfg.prompt {
                    texts.push(p);
                }
                texts.extend(cfg.routes.iter().filter_map(|r| r.when.as_deref()));
            }
            // `command` is a file reference (resolved by the runner); `approval`
            // carries a human message, not substitutable node refs.
            NodeKind::Command(_) | NodeKind::Approval(_) => {}
        }
        texts
    }
}

/// Optional UI surfaces a workflow opts into. Absent → the workflow has no
/// special UI (it's just a run in the list). Lets any workflow — including ones
/// authored via the MCP endpoint — declare a left-nav entry and/or a
/// findings/report tab on its runs, instead of the UI hard-coding workflow names.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct WorkflowUi {
    /// A left-nav entry (shown once the workflow has at least one run).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub nav: Option<WorkflowNav>,
    /// A structured findings/report tab on the run detail page.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub report: Option<WorkflowReport>,
}

/// A left-navigation entry for a workflow.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorkflowNav {
    /// Sidebar label (e.g. "Security Audit").
    pub label: String,
    /// Icon key from the UI's curated allow-list (e.g. `shield`, `world-search`);
    /// the UI falls back to a default when unset or unknown.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub icon: Option<String>,
}

/// A findings/report tab for a workflow's runs. The report is rendered from a
/// node's JSON output — the verdict — shaped as `{ summary, findings[], score?,
/// categories? }` (usually produced via that node's `output_format`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorkflowReport {
    /// Tab label on the run detail page (e.g. "Findings", "Review").
    pub label: String,
    /// Node id whose JSON output carries the verdict; when unset the UI scans
    /// the run's nodes for one matching the verdict shape.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub verdict_node: Option<String>,
    /// Show a score gauge + score-history sparkline (GEO-style). Default: the
    /// findings list only (review-style).
    #[serde(default)]
    pub scored: bool,
    /// Opt-in per-finding action buttons. Default empty → a clean, read-only
    /// list (right for a test checklist); a bug/finding report opts into
    /// `[build, issue, ignore]`.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub actions: Vec<ReportAction>,
    /// Per-item status control shown on each finding. Default `none`; a test
    /// report uses `check` (a "tested" checkbox) or `pass_fail` (Passed/Failed).
    #[serde(default, skip_serializing_if = "ReportStatus::is_none")]
    pub status: ReportStatus,
    /// How the findings are laid out. Default `list` (one card each); `table`
    /// is for reports with many findings (a test audit): totals, filters and
    /// one sortable row per finding.
    #[serde(default, skip_serializing_if = "ReportLayout::is_list")]
    pub layout: ReportLayout,
    /// What acts on the approved findings: an "Apply" button that starts this
    /// workflow with them. Pairs with the `approve` action.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub apply: Option<ReportApply>,
}

/// A report's apply step: the workflow its "Apply" button starts, in the same
/// project, with the approved findings as its input (`$ARGUMENTS`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReportApply {
    /// The workflow to start.
    pub workflow: String,
    /// The button's label (e.g. "Open PRs"); default "Apply".
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
    /// The finding categories it acts on (`[delete, review, merge]`). Only
    /// those rows can be approved, and only they are sent. Empty: every row.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub categories: Vec<String>,
}

/// How a report lays out its findings.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ReportLayout {
    /// One card per finding (default) — right for a handful.
    #[default]
    List,
    /// Totals per category, filters, and a sortable table — right for hundreds.
    Table,
}

impl ReportLayout {
    /// `true` for the default `List` — used to skip serialization.
    pub fn is_list(&self) -> bool {
        matches!(self, ReportLayout::List)
    }
}

/// An opt-in per-finding action a report offers.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ReportAction {
    /// "Build this" — fire `idea-to-pr` from the finding's fix.
    Build,
    /// "Create issue" — file the finding into Linear.
    Issue,
    /// "Ignore" — dim/skip the finding.
    Ignore,
    /// "Approve" / "Reject" — decide on what the finding proposes; the
    /// report's `apply` workflow then acts on the approved ones.
    Approve,
}

/// The per-item status control a report shows on each finding.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ReportStatus {
    /// No status control (default).
    #[default]
    None,
    /// A single "tested / done" checkbox.
    Check,
    /// Passed / Failed buttons (e.g. manual browser test outcomes).
    PassFail,
}

impl ReportStatus {
    /// `true` for the `None` variant — used to skip serialization.
    pub fn is_none(&self) -> bool {
        matches!(self, ReportStatus::None)
    }
}

/// Improvement suggestions for a workflow: after every `every` finished runs
/// of it in a project, the harness reviews those runs and suggests changes —
/// mostly to the project's repo (its `CLAUDE.md`, skills, checks), sometimes
/// to the workflow itself.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ImprovementsConfig {
    /// Finished runs per project between reviews.
    pub every: u32,
}

/// The range `improvements.every` accepts. Fewer than a handful of runs can't
/// show what *keeps* happening, which is the point; more than a hundred would
/// review stale history.
pub const IMPROVEMENTS_EVERY: std::ops::RangeInclusive<u32> = 3..=100;

/// A parsed, validated workflow DAG.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Workflow {
    /// Workflow name.
    pub name: String,
    /// Optional human description.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    /// Workflow-level default provider (overridden per node).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub provider: Option<String>,
    /// Workflow-level default model.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    /// Optional UI surfaces (left-nav entry, report tab) this workflow opts into.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ui: Option<WorkflowUi>,
    /// Improvement suggestions, when the workflow opts in.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub improvements: Option<ImprovementsConfig>,
    /// The harness's own machinery, started by the server and never by a
    /// person: hidden from every workflow list and picker.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub internal: bool,
    /// The workflow runs Linear epics itself: it is handed an epic (an issue
    /// with sub-issues) rather than the server passing it to
    /// `linear-epic-supervise`, and a sub-issue of an epic it has started is
    /// based on that epic's branch. Without it a delegated epic goes to the
    /// supervisor, as it always has.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub handles_epics: bool,
    /// Nodes in declaration order.
    pub nodes: Vec<Node>,
}

impl Workflow {
    /// Look up a node by id.
    pub fn node(&self, id: &str) -> Option<&Node> {
        self.nodes.iter().find(|n| n.id == id)
    }

    /// The Linear statuses this workflow's `linear:` steps move an issue to, as
    /// written, each once, in node order. A status that uses a variable is left
    /// out: what it names is only known once the run has started.
    ///
    /// The point is to check them against the issue's team before a run spends
    /// anything: a move to a status the team lacks fails only when its step is
    /// reached, which for "In Review" is after the pull request is open.
    pub fn linear_target_states(&self) -> Vec<&str> {
        let mut out: Vec<&str> = Vec::new();
        for node in &self.nodes {
            if let NodeKind::Linear(LinearAction::MoveState(state)) = &node.kind {
                let state = state.trim();
                if !state.is_empty() && !state.contains('$') && !out.contains(&state) {
                    out.push(state);
                }
            }
        }
        out
    }
}
