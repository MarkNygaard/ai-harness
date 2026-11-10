use harness_core::{agent::StreamItem, error::HarnessError};
use std::sync::{Arc, Mutex};
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::sync::mpsc::Sender;

const STDERR_ERROR_KEYWORDS: &[&str] = &[
    "error",
    "warn",
    "warning",
    "failed",
    "fatal",
    "panic",
    "exception",
];
const MAX_STDERR_LINE_LEN: usize = 1000;
const MAX_CAPTURED_STDERR_CHARS: usize = 4000;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum StderrLineDisposition {
    Debug,
    Warning,
}

/// Truncate a string to at most `max_bytes`, snapping to a char boundary.
fn truncate_to_char_boundary(s: &str, max_bytes: usize) -> &str {
    if s.len() <= max_bytes {
        return s;
    }
    let mut end = max_bytes;
    while end > 0 && !s.is_char_boundary(end) {
        end -= 1;
    }
    &s[..end]
}

/// Agent-internal line prefixes that should never surface as WARN.
/// These are progress lines an agent's tools emit, not actionable errors for
/// the harness operator.
const AGENT_INTERNAL_PREFIXES: &[&str] = &[
    // Cargo/rustc build output that agents emit to stderr
    "compiling ",
    "   compiling ",
    "    checking ",
    "    finished ",
];

/// Detect code-content lines an agent echoes to stderr (e.g. `   7        "error",`).
/// These contain source code keywords but are not real errors.
fn looks_like_code_content(line: &str) -> bool {
    let trimmed = line.trim_start();
    let had_indent = trimmed.len() != line.len();
    // Lines starting with a digit followed by whitespace are numbered source lines.
    if trimmed
        .as_bytes()
        .first()
        .is_some_and(|b| b.is_ascii_digit())
        && trimmed
            .find(|c: char| !c.is_ascii_digit())
            .is_some_and(|pos| {
                trimmed
                    .as_bytes()
                    .get(pos)
                    .is_some_and(|b| b.is_ascii_whitespace())
            })
    {
        return true;
    }

    // Diff hunks and inline patch content frequently contain words like
    // "warn"/"error" but should stay debug-only.
    if trimmed.starts_with('+')
        || trimmed.starts_with('-')
        || trimmed.starts_with("@@")
        || trimmed.starts_with("diff --git")
    {
        return true;
    }

    if had_indent {
        const INDENTED_CODE_PREFIXES: &[&str] = &[
            "\"",
            "//",
            "/*",
            "#[",
            "tracing::",
            "sqlx::",
            "tokio::",
            "let ",
            "fn ",
            "pub ",
            "impl ",
            "struct ",
            "enum ",
            "match ",
            "if ",
            "for ",
            "while ",
            "loop",
            "return ",
            "Err(",
            "Ok(",
            "Some(",
            "None",
            ".",
        ];
        if INDENTED_CODE_PREFIXES
            .iter()
            .any(|prefix| trimmed.starts_with(prefix))
        {
            return true;
        }
        if trimmed.ends_with('{')
            || trimmed.ends_with('}')
            || trimmed.ends_with(',')
            || trimmed.ends_with(");")
            || trimmed.ends_with(")]")
            || trimmed.ends_with("=>")
        {
            return true;
        }
    }

    false
}

fn looks_like_git_commit_line(line: &str) -> bool {
    let trimmed = line.trim_start();
    let Some((hash, rest)) = trimmed.split_once(' ') else {
        return false;
    };
    (7..=40).contains(&hash.len())
        && hash.chars().all(|c| c.is_ascii_hexdigit())
        && !rest.is_empty()
}

fn looks_like_search_result(line: &str) -> bool {
    let trimmed = line.trim_start();
    let Some((path, rest)) = trimmed.split_once(':') else {
        return false;
    };
    let Some((line_no, _tail)) = rest.split_once(':') else {
        return false;
    };
    !path.is_empty()
        && !path.contains(char::is_whitespace)
        && !line_no.is_empty()
        && line_no.chars().all(|c| c.is_ascii_digit())
        && (path.starts_with("./")
            || path.starts_with('/')
            || path.contains('/')
            || path.ends_with(".rs")
            || path.ends_with(".md")
            || path.ends_with(".toml"))
}

fn looks_like_markdown_prompt_line(line: &str) -> bool {
    let trimmed = line.trim_start();
    trimmed.starts_with('#')
        || trimmed.starts_with("- ")
        || trimmed.starts_with("* ")
        || trimmed.starts_with("> ")
        || trimmed.starts_with("Pass: ")
        || trimmed.starts_with("Warn: ")
        || trimmed.starts_with("Block: ")
}

fn is_agent_internal(line: &str) -> bool {
    if looks_like_code_content(line)
        || looks_like_git_commit_line(line)
        || looks_like_search_result(line)
        || looks_like_markdown_prompt_line(line)
    {
        return true;
    }
    let lower = line.to_lowercase();
    AGENT_INTERNAL_PREFIXES
        .iter()
        .any(|prefix| lower.starts_with(prefix))
}

fn tail_chars(s: &str, max_chars: usize) -> String {
    if max_chars == 0 {
        return String::new();
    }
    // char_indices().rev().nth(max_chars - 1) finds the byte offset of the
    // (max_chars)-th char from the end in a single pass — no Vec<char> needed.
    match s.char_indices().rev().nth(max_chars - 1) {
        Some((idx, _)) => s[idx..].to_string(),
        None => s.to_string(),
    }
}

fn append_stderr_capture(captured: &Arc<Mutex<String>>, line: &str) {
    let mut guard = captured
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    guard.push_str(line);
    guard.push('\n');
    if guard.chars().rev().nth(MAX_CAPTURED_STDERR_CHARS).is_some() {
        *guard = tail_chars(&guard, MAX_CAPTURED_STDERR_CHARS);
    }
}

fn stderr_line_disposition(line: &str) -> Option<StderrLineDisposition> {
    let trimmed = truncate_to_char_boundary(line, MAX_STDERR_LINE_LEN);
    if trimmed.is_empty() {
        return None;
    }
    if is_agent_internal(trimmed) {
        return Some(StderrLineDisposition::Debug);
    }
    let lower = trimmed.to_lowercase();
    if STDERR_ERROR_KEYWORDS.iter().any(|kw| lower.contains(kw)) {
        Some(StderrLineDisposition::Warning)
    } else {
        Some(StderrLineDisposition::Debug)
    }
}

pub(crate) fn captured_stderr_tail(captured: &Arc<Mutex<String>>) -> String {
    captured
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .trim()
        .to_string()
}

pub(crate) fn enrich_stream_exit_error(error: HarnessError, stderr_tail: &str) -> HarnessError {
    match error {
        HarnessError::AgentExecution(message)
            if message.contains(" exited with ")
                && !message.contains("stderr=[")
                && !stderr_tail.trim().is_empty() =>
        {
            HarnessError::AgentExecution(format!("{message}: stderr=[{stderr_tail}]"))
        }
        other => other,
    }
}

/// Read agent stderr line-by-line. Lines matching error keywords are logged
/// at warn level; agent-internal progress lines are always debug.
pub(crate) async fn filter_agent_stderr_with_capture(
    stderr: tokio::process::ChildStderr,
    agent_name: &str,
    captured: Option<Arc<Mutex<String>>>,
) {
    let reader = BufReader::new(stderr);
    let mut lines = reader.lines();

    while let Ok(Some(line)) = lines.next_line().await {
        let trimmed = truncate_to_char_boundary(&line, MAX_STDERR_LINE_LEN);
        if trimmed.is_empty() {
            continue;
        }
        if let Some(captured) = captured.as_ref() {
            append_stderr_capture(captured, trimmed);
        }
        log_stderr_line(trimmed, agent_name);
    }
}

fn log_stderr_line(line: &str, agent_name: &str) {
    match stderr_line_disposition(line) {
        Some(StderrLineDisposition::Warning) => tracing::warn!(agent = agent_name, "{line}"),
        Some(StderrLineDisposition::Debug) => tracing::debug!(agent = agent_name, "{line}"),
        None => {}
    }
}

/// Log stderr captured from a non-streaming `output()` call.
pub(crate) fn log_captured_stderr(stderr: &str, agent_name: &str) {
    for line in stderr.lines() {
        log_stderr_line(
            truncate_to_char_boundary(line, MAX_STDERR_LINE_LEN),
            agent_name,
        );
    }
}

pub(crate) async fn send_stream_item(
    tx: &Sender<StreamItem>,
    item: StreamItem,
    agent_name: &str,
    item_label: &'static str,
) -> harness_core::error::Result<()> {
    tx.send(item).await.map_err(|err| {
        tracing::error!(
            agent = agent_name,
            stream_item = item_label,
            error = %err,
            "failed to send stream item"
        );
        HarnessError::AgentExecution(format!(
            "{agent_name} stream send failed while sending {item_label}: {err}"
        ))
    })
}

#[cfg(test)]
#[path = "streaming_tests.rs"]
mod tests;
