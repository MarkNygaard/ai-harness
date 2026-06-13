//! Template-variable substitution for node prompts and scripts.
//!
//! Substitution recognizes a fixed set of harness variables (e.g.
//! `$ARTIFACTS_DIR`, `$BASE_BRANCH`) plus positional command args (`$1`..`$9`)
//! and **upstream node outputs** (`$node-id.output`, with best-effort JSON field
//! access via `$node-id.output.field`). Only recognized names are touched, so
//! shell syntax like `$HOME` or `${results[@]}` in a `bash` body is left
//! untouched. Referencing a recognized harness variable that has no value in the
//! current context is a hard error ([`DagError::MissingVariable`]) — we fail loud
//! rather than silently emit an empty string. A node-output reference whose node
//! produced no usable value resolves to the empty string (the upstream node may
//! legitimately have been skipped), so it is lenient by design.

use std::collections::HashMap;
use std::sync::OnceLock;

use regex::Regex;

use crate::error::DagError;

/// The recognized harness variable names. A `$NAME` / `${NAME}` token is only
/// substituted if `NAME` is in this set; anything else is passed through.
pub const RECOGNIZED_VARS: &[&str] = &[
    "WORKFLOW_ID",
    "USER_MESSAGE",
    "ARGUMENTS",
    "ARTIFACTS_DIR",
    "BASE_BRANCH",
    "EXTERNAL_URL",
    "DOCS_DIR",
    "CONTEXT",
    "EXTERNAL_CONTEXT",
    "ISSUE_CONTEXT",
    "LOOP_USER_INPUT",
    "LOOP_PREV_OUTPUT",
    "REJECTION_REASON",
];

/// Values available for substitution in the current run/node context.
///
/// Holds named harness variables, up to nine positional command args, and the
/// outputs of upstream nodes keyed by node id. Positional args `$1`..`$9` are
/// recognized when present here.
#[derive(Debug, Default, Clone)]
pub struct VarContext {
    named: HashMap<String, String>,
    positional: Vec<String>,
    node_outputs: HashMap<String, String>,
}

impl VarContext {
    /// Create an empty context.
    pub fn new() -> Self {
        Self::default()
    }

    /// Set a named variable. The name should be one of [`RECOGNIZED_VARS`];
    /// unrecognized names will never be substituted and are effectively inert.
    pub fn set(mut self, name: impl Into<String>, value: impl Into<String>) -> Self {
        self.named.insert(name.into(), value.into());
        self
    }

    /// Set the positional args (`$1` = `args[0]`, …, up to `$9`).
    pub fn with_positional(mut self, args: impl IntoIterator<Item = String>) -> Self {
        self.positional = args.into_iter().collect();
        self
    }

    /// Record an upstream node's output, so `$node-id.output` (and JSON field
    /// access on it) resolves for downstream nodes and `when:` conditions.
    pub fn set_node_output(mut self, id: impl Into<String>, output: impl Into<String>) -> Self {
        self.node_outputs.insert(id.into(), output.into());
        self
    }

    /// Whether `name` is a recognized variable (named or positional) regardless
    /// of whether it currently has a value.
    fn is_recognized(name: &str) -> bool {
        RECOGNIZED_VARS.contains(&name) || is_positional(name)
    }

    /// Resolve a recognized variable's value, if available.
    fn lookup(&self, name: &str) -> Option<&str> {
        if let Some(idx) = positional_index(name) {
            self.positional.get(idx).map(String::as_str)
        } else {
            self.named.get(name).map(String::as_str)
        }
    }

    /// Resolve a node-output reference of the form `id.output` or
    /// `id.output.field[.field…]` to a string. The raw text is used for a bare
    /// `.output`; a deeper path triggers best-effort JSON parsing of the output
    /// and navigation into objects/arrays. A missing node, unparseable JSON, or
    /// absent field all resolve to the empty string (lenient — see module docs).
    pub fn resolve_node_ref(&self, reference: &str) -> String {
        let mut parts = reference.splitn(2, '.');
        let id = parts.next().unwrap_or("");
        let rest = parts.next().unwrap_or(""); // "output" or "output.a.b"
        let raw = self.node_outputs.get(id).map(String::as_str).unwrap_or("");

        // Field path after the leading "output" segment.
        let path: Vec<&str> = rest.split('.').skip(1).collect();
        if path.is_empty() {
            return raw.to_string();
        }
        match extract_json(raw) {
            Some(value) => navigate_json(&value, &path),
            None => String::new(),
        }
    }
}

/// Best-effort JSON parse of an agent's output for field access. Agents commonly
/// wrap their JSON in a ```` ```json ```` fence or surround it with prose despite
/// instructions, so if a direct parse fails we extract the outermost `{…}`/`[…]`
/// span and parse that. Returns `None` if no JSON is found.
fn extract_json(raw: &str) -> Option<serde_json::Value> {
    let trimmed = raw.trim();
    if let Ok(v) = serde_json::from_str::<serde_json::Value>(trimmed) {
        return Some(v);
    }
    let start = trimmed.find(['{', '['])?;
    let end = trimmed.rfind(['}', ']'])?;
    if end <= start {
        return None;
    }
    serde_json::from_str::<serde_json::Value>(&trimmed[start..=end]).ok()
}

/// Walk a JSON value along a dotted field path, rendering the leaf as a plain
/// string (string contents verbatim; scalars via `to_string`; objects/arrays as
/// compact JSON). A missing key or out-of-range index yields the empty string.
fn navigate_json(root: &serde_json::Value, path: &[&str]) -> String {
    let mut cur = root;
    for seg in path {
        cur = match cur {
            serde_json::Value::Object(map) => match map.get(*seg) {
                Some(v) => v,
                None => return String::new(),
            },
            serde_json::Value::Array(arr) => {
                match seg.parse::<usize>().ok().and_then(|i| arr.get(i)) {
                    Some(v) => v,
                    None => return String::new(),
                }
            }
            _ => return String::new(),
        };
    }
    match cur {
        serde_json::Value::String(s) => s.clone(),
        serde_json::Value::Null => String::new(),
        other => other.to_string(),
    }
}

/// Collect the node ids referenced via `$id.output…` tokens in `text`. Used by
/// workflow validation to ensure every reference points to a declared node.
pub fn referenced_node_ids(text: &str) -> Vec<String> {
    token_regex()
        .captures_iter(text)
        .filter_map(|caps| caps.get(2).or_else(|| caps.get(4)))
        .filter_map(|m| m.as_str().split('.').next().map(str::to_string))
        .collect()
}

fn is_positional(name: &str) -> bool {
    positional_index(name).is_some()
}

/// `"1"`..`"9"` -> `Some(0..8)`; anything else -> `None`.
fn positional_index(name: &str) -> Option<usize> {
    if name.len() == 1 {
        match name.as_bytes()[0] {
            b'1'..=b'9' => Some((name.as_bytes()[0] - b'1') as usize),
            _ => None,
        }
    } else {
        None
    }
}

fn token_regex() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    // Four alternatives, braced forms first so `${…}` wins over the bare form:
    //   1: ${NAME} / 3: $NAME      — harness var, upper-snake identifier or digit
    //   2: ${id.output…} / 4: $id.output…  — node-output reference (kebab id)
    // Harness vars start uppercase, node refs lowercase, so they never overlap.
    RE.get_or_init(|| {
        Regex::new(concat!(
            r"\$\{([A-Z_][A-Z0-9_]*|\d)\}",
            r"|\$\{([a-z][a-z0-9-]*\.output(?:\.[A-Za-z0-9_]+)*)\}",
            r"|\$([A-Z_][A-Z0-9_]*|\d)",
            r"|\$([a-z][a-z0-9-]*\.output(?:\.[A-Za-z0-9_]+)*)",
        ))
        .unwrap()
    })
}

/// Substitute recognized variables in `template`. Harness vars (`$NAME`) and
/// node-output refs (`$id.output…`) are resolved; unrecognized `$tokens` are
/// left verbatim. Errors only if a recognized harness variable is referenced but
/// unset — node refs are lenient (missing → empty string).
pub fn substitute(template: &str, ctx: &VarContext) -> Result<String, DagError> {
    let re = token_regex();
    let mut out = String::with_capacity(template.len());
    let mut last = 0;

    for caps in re.captures_iter(template) {
        let m = caps.get(0).unwrap();
        // Copy the gap between the previous match and this one.
        out.push_str(&template[last..m.start()]);

        if let Some(name) = caps.get(1).or_else(|| caps.get(3)) {
            // Harness variable (or shell-style token passed through verbatim).
            let name = name.as_str();
            if VarContext::is_recognized(name) {
                match ctx.lookup(name) {
                    Some(value) => out.push_str(value),
                    // Positional args (`$1`..`$9`) only exist for command-triggered
                    // runs. When a run carries no positional args at all, a bare
                    // `$1` is shell syntax (e.g. a function's positional parameter
                    // inside a `bash` node), not a harness variable — pass it
                    // through verbatim. Command runs (positional present) still fail
                    // loud on an out-of-range arg, and named harness vars always do.
                    None if is_positional(name) && ctx.positional.is_empty() => {
                        out.push_str(m.as_str())
                    }
                    None => return Err(DagError::MissingVariable(name.to_string())),
                }
            } else {
                out.push_str(m.as_str());
            }
        } else if let Some(node_ref) = caps.get(2).or_else(|| caps.get(4)) {
            // Node-output reference — always resolved (lenient, may be empty).
            out.push_str(&ctx.resolve_node_ref(node_ref.as_str()));
        }

        last = m.end();
    }
    out.push_str(&template[last..]);
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ctx() -> VarContext {
        VarContext::new()
            .set("ARGUMENTS", "do the thing")
            .set_node_output("create-plan", "PLAN BODY")
            .set_node_output("classify", r#"{"type":"BUG","nested":{"k":"v"},"n":3}"#)
    }

    #[test]
    fn substitutes_bare_and_braced_node_output() {
        assert_eq!(
            substitute("plan: $create-plan.output", &ctx()).unwrap(),
            "plan: PLAN BODY"
        );
        assert_eq!(
            substitute("plan: ${create-plan.output}!", &ctx()).unwrap(),
            "plan: PLAN BODY!"
        );
    }

    #[test]
    fn json_field_access() {
        assert_eq!(
            substitute("type=$classify.output.type", &ctx()).unwrap(),
            "type=BUG"
        );
        assert_eq!(
            substitute("k=$classify.output.nested.k n=$classify.output.n", &ctx()).unwrap(),
            "k=v n=3"
        );
    }

    #[test]
    fn json_field_access_through_markdown_fence() {
        // Agents often wrap the JSON verdict in a ```json fence despite
        // instructions; field access must still work (the validate→gate path).
        let ctx = VarContext::new().set_node_output(
            "validate",
            "```json\n{\"passed\": true, \"summary\": \"all green\"}\n```",
        );
        assert_eq!(substitute("$validate.output.passed", &ctx).unwrap(), "true");
        assert_eq!(
            substitute("$validate.output.summary", &ctx).unwrap(),
            "all green"
        );
    }

    #[test]
    fn missing_node_ref_is_empty_not_error() {
        assert_eq!(
            substitute("x=$nope.output.field y", &ctx()).unwrap(),
            "x= y"
        );
        // A node that produced non-JSON, asked for a field → empty.
        assert_eq!(substitute("[$create-plan.output.x]", &ctx()).unwrap(), "[]");
    }

    #[test]
    fn harness_vars_and_shell_vars_still_behave() {
        assert_eq!(
            substitute("msg: $ARGUMENTS", &ctx()).unwrap(),
            "msg: do the thing"
        );
        // Lowercase shell-style and unknown upper tokens pass through verbatim.
        assert_eq!(
            substitute("$HOME and ${PATH}", &ctx()).unwrap(),
            "$HOME and ${PATH}"
        );
        // A recognized-but-unset harness var is still a hard error.
        assert!(substitute("$BASE_BRANCH", &VarContext::new()).is_err());
    }

    #[test]
    fn positional_args_substitute_for_command_runs() {
        let ctx = VarContext::new().with_positional(["alpha".to_string(), "beta".to_string()]);
        assert_eq!(substitute("$1 and $2", &ctx).unwrap(), "alpha and beta");
        assert_eq!(substitute("${1}/${2}", &ctx).unwrap(), "alpha/beta");
        // An out-of-range positional in a command run is still a hard error —
        // the run carries args, so a missing one is a genuine authoring mistake.
        assert!(substitute("$3", &ctx).is_err());
    }

    #[test]
    fn bare_positional_passes_through_when_no_args() {
        // A run with no positional args (e.g. idea-to-pr triggered from an issue,
        // not a command) must treat `$1` as shell syntax — e.g. a `bash` node's
        // `install_in() { cd "$1"; }`. It passes through verbatim, not an error.
        let ctx = VarContext::new();
        assert_eq!(
            substitute("install_in() { cd \"$1\" || exit 1; }", &ctx).unwrap(),
            "install_in() { cd \"$1\" || exit 1; }"
        );
        assert_eq!(substitute("$1 ${2}", &ctx).unwrap(), "$1 ${2}");
    }

    #[test]
    fn referenced_ids_scans_node_refs_only() {
        let ids =
            referenced_node_ids("use $a.output and ${b-two.output.field} but not $UPPER or $x");
        assert_eq!(ids, vec!["a".to_string(), "b-two".to_string()]);
    }
}
