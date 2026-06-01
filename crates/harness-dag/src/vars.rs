//! Template-variable substitution for node prompts and scripts.
//!
//! Substitution recognizes a fixed set of harness variables (e.g.
//! `$ARTIFACTS_DIR`, `$BASE_BRANCH`) plus positional command args (`$1`..`$9`).
//! Only recognized names are touched, so shell syntax like `$HOME` or
//! `${results[@]}` in a `bash` body is left untouched. Referencing a recognized
//! variable that has no value in the current context is a hard error
//! ([`DagError::MissingVariable`]) — we fail loud rather than silently emit an
//! empty string.

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
/// Holds named harness variables and up to nine positional command args.
/// Positional args `$1`..`$9` are recognized when present here.
#[derive(Debug, Default, Clone)]
pub struct VarContext {
    named: HashMap<String, String>,
    positional: Vec<String>,
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
    // $NAME, ${NAME}, $1..$9. NAME is an upper-snake identifier.
    RE.get_or_init(|| Regex::new(r"\$\{([A-Z_][A-Z0-9_]*|\d)\}|\$([A-Z_][A-Z0-9_]*|\d)").unwrap())
}

/// Substitute recognized variables in `template`. Unrecognized `$tokens` are
/// left verbatim. Errors if a recognized variable is referenced but unset.
pub fn substitute(template: &str, ctx: &VarContext) -> Result<String, DagError> {
    let re = token_regex();
    let mut out = String::with_capacity(template.len());
    let mut last = 0;

    for caps in re.captures_iter(template) {
        let m = caps.get(0).unwrap();
        // Either the braced (group 1) or bare (group 2) name matched.
        let name = caps
            .get(1)
            .or_else(|| caps.get(2))
            .map(|g| g.as_str())
            .unwrap();

        // Copy the gap between the previous match and this one.
        out.push_str(&template[last..m.start()]);

        if VarContext::is_recognized(name) {
            match ctx.lookup(name) {
                Some(value) => out.push_str(value),
                None => return Err(DagError::MissingVariable(name.to_string())),
            }
        } else {
            // Not a harness variable (e.g. a shell var) — pass through verbatim.
            out.push_str(m.as_str());
        }

        last = m.end();
    }
    out.push_str(&template[last..]);
    Ok(out)
}
