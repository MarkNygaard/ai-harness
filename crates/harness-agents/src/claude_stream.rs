use crate::claude_adapter::{parse_stream_json_line, parse_stream_json_usage};
use crate::streaming::send_stream_item;
use harness_core::{
    agent::{AgentEvent, StreamItem},
    error::HarnessError,
    types::{Item, TokenUsage},
};
use std::time::Duration;
use tokio::io::{AsyncBufReadExt, BufReader};

const MAX_STREAM_FAILURE_OUTPUT_CHARS: usize = 500;

#[derive(Debug, Default)]
pub(crate) struct ParsedClaudeStreamOutput {
    pub(crate) raw_stdout: String,
    pub(crate) output: String,
    pub(crate) token_usage: TokenUsage,
    completed: bool,
    /// When the usage limit that rejected this run resets (unix seconds), from
    /// a `rate_limit_event` with status `rejected`.
    pub(crate) limit_resets_at: Option<i64>,
}

/// The phrase `harness_dag::usage_limit::RESETS_AT_UNIX` looks for. This crate
/// does not depend on `harness-dag`, so it keeps its own copy;
/// `usage_limit_tests::the_phrase_is_the_one_the_dag_reads` fails if they drift.
const RESETS_AT_UNIX: &str = "usage limit reached, resets at unix ";

/// What to append to a failed run's error so the DAG driver can wait for the
/// reset: empty unless a `rate_limit_event` rejected the run.
pub(crate) fn usage_limit_suffix(parsed: &ParsedClaudeStreamOutput) -> String {
    match parsed.limit_resets_at {
        Some(at) => format!(" ({RESETS_AT_UNIX}{at})"),
        None => String::new(),
    }
}

/// The reset time from a rejected `rate_limit_event` line:
/// `{"type":"rate_limit_event","rate_limit_info":{"status":"rejected","resetsAt":1762700000,…}}`.
/// `None` for any other line, and for `allowed` / `allowed_warning` events,
/// which report the window without stopping the run.
fn rejected_limit_reset(line: &str) -> Option<i64> {
    let v: serde_json::Value = serde_json::from_str(line).ok()?;
    if v.get("type")?.as_str()? != "rate_limit_event" {
        return None;
    }
    let info = v.get("rate_limit_info")?;
    if info.get("status")?.as_str()? != "rejected" {
        return None;
    }
    let at = info.get("resetsAt")?;
    at.as_i64().or_else(|| at.as_f64().map(|f| f as i64))
}

pub(crate) fn claude_stdout_tail(s: &str) -> String {
    let max_chars = MAX_STREAM_FAILURE_OUTPUT_CHARS;
    if max_chars == 0 {
        return String::new();
    }
    match s.char_indices().rev().nth(max_chars - 1) {
        Some((idx, _)) => s[idx..].to_string(),
        None => s.to_string(),
    }
}

fn is_json_line(line: &str) -> bool {
    serde_json::from_str::<serde_json::Value>(line).is_ok()
}

fn push_plaintext_line(
    line: &str,
    parsed: &mut ParsedClaudeStreamOutput,
    emitted_items: &mut Vec<StreamItem>,
) {
    let delta = format!("{line}\n");
    parsed.output.push_str(&delta);
    emitted_items.push(StreamItem::MessageDelta { text: delta });
}

fn apply_claude_stream_line(
    line: &str,
    parsed: &mut ParsedClaudeStreamOutput,
    emitted_items: &mut Vec<StreamItem>,
) {
    if line.trim().is_empty() {
        return;
    }
    if let Some(at) = rejected_limit_reset(line) {
        parsed.limit_resets_at = Some(at);
        return;
    }

    let usage = parse_stream_json_usage(line);
    let event = parse_stream_json_line(line);
    if usage.is_none() && event.is_none() && !is_json_line(line) {
        push_plaintext_line(line, parsed, emitted_items);
        return;
    }

    if let Some(usage) = usage {
        parsed.token_usage = usage.clone();
        emitted_items.push(StreamItem::TokenUsage { usage });
    }

    match event {
        Some(AgentEvent::MessageDelta { text }) => {
            parsed.output.push_str(&text);
            emitted_items.push(StreamItem::MessageDelta { text });
        }
        Some(AgentEvent::ToolOutputDelta { item_id, text }) => {
            emitted_items.push(StreamItem::ToolOutputDelta { item_id, text });
        }
        Some(AgentEvent::ItemStartedPayload { item }) => {
            emitted_items.push(StreamItem::ItemStarted { item });
        }
        Some(AgentEvent::ItemCompletedPayload { item }) => {
            emitted_items.push(StreamItem::ItemCompleted { item });
        }
        Some(AgentEvent::ApprovalRequest { id, command }) => {
            emitted_items.push(StreamItem::ApprovalRequest { id, command });
        }
        Some(AgentEvent::Warning { message }) => {
            emitted_items.push(StreamItem::Warning { message });
        }
        Some(AgentEvent::Error { message }) => {
            emitted_items.push(StreamItem::Error { message });
        }
        Some(AgentEvent::TurnCompleted { output }) => {
            if !output.is_empty() {
                if parsed.output.is_empty() {
                    emitted_items.push(StreamItem::MessageDelta {
                        text: output.clone(),
                    });
                }
                parsed.output = output;
            }
            emitted_items.push(StreamItem::ItemCompleted {
                item: Item::AgentReasoning {
                    content: parsed.output.clone(),
                },
            });
            parsed.completed = true;
        }
        Some(AgentEvent::ToolCall { name, input }) => {
            emitted_items.push(StreamItem::ItemCompleted {
                item: Item::ToolCall {
                    name,
                    input,
                    output: None,
                },
            });
        }
        Some(
            AgentEvent::TurnStarted
            | AgentEvent::ItemStarted { .. }
            | AgentEvent::ItemCompleted
            | AgentEvent::TokenUsage { .. },
        )
        | None => {}
    }

    // Claude Code's `stream-json` nests tool calls inside `assistant` message
    // content (`{"type":"assistant","message":{"content":[{"type":"tool_use",…}]}}`).
    // The event parse above only extracts *text* from those lines, so the tool
    // calls would never surface in the live activity feed. Pull each `tool_use`
    // out here as a ToolCall item (the runner maps these to activity cards).
    for item in claude_tool_use_items(line) {
        emitted_items.push(StreamItem::ItemCompleted { item });
    }
}

/// Extract each `tool_use` block from a Claude Code `assistant` stream line as an
/// [`Item::ToolCall`] (name + input). Empty for non-assistant lines, non-JSON,
/// or messages with no tool calls.
fn claude_tool_use_items(line: &str) -> Vec<Item> {
    let Ok(v) = serde_json::from_str::<serde_json::Value>(line) else {
        return Vec::new();
    };
    if v.get("type").and_then(|t| t.as_str()) != Some("assistant") {
        return Vec::new();
    }
    let Some(content) = v
        .get("message")
        .and_then(|m| m.get("content"))
        .and_then(|c| c.as_array())
    else {
        return Vec::new();
    };
    content
        .iter()
        .filter_map(|block| {
            if block.get("type").and_then(|t| t.as_str()) != Some("tool_use") {
                return None;
            }
            let name = block.get("name").and_then(|n| n.as_str())?.to_string();
            let input = block
                .get("input")
                .cloned()
                .unwrap_or(serde_json::Value::Null);
            Some(Item::ToolCall {
                name,
                input,
                output: None,
            })
        })
        .collect()
}

pub(crate) fn parse_claude_stream_output(stdout: &str) -> ParsedClaudeStreamOutput {
    let mut parsed = ParsedClaudeStreamOutput::default();
    for line in stdout.lines() {
        parsed.raw_stdout.push_str(line);
        parsed.raw_stdout.push('\n');
        let mut emitted_items = Vec::new();
        apply_claude_stream_line(line, &mut parsed, &mut emitted_items);
    }
    parsed
}

fn stream_item_label(item: &StreamItem) -> &'static str {
    match item {
        StreamItem::ItemStarted { .. } => "item_started",
        StreamItem::MessageDelta { .. } => "message_delta",
        StreamItem::ToolOutputDelta { .. } => "tool_output_delta",
        StreamItem::ItemCompleted { .. } => "item_completed",
        StreamItem::TokenUsage { .. } => "token_usage",
        StreamItem::Warning { .. } => "warning",
        StreamItem::Error { .. } => "error",
        StreamItem::ApprovalRequest { .. } => "approval_request",
        StreamItem::Done => "done",
    }
}

async fn read_next_claude_line(
    lines: &mut tokio::io::Lines<BufReader<tokio::process::ChildStdout>>,
    child: &mut tokio::process::Child,
    idle_timeout: Option<Duration>,
) -> harness_core::error::Result<Option<String>> {
    if let Some(duration) = idle_timeout {
        tokio::time::timeout(duration, lines.next_line())
            .await
            .map_err(|_| {
                #[cfg(unix)]
                crate::kill_process_group(child);
                HarnessError::AgentExecution(format!(
                    "claude stream idle timeout after {}s: zombie connection terminated",
                    duration.as_secs()
                ))
            })?
            .map_err(|error| {
                HarnessError::AgentExecution(format!("failed reading claude stdout: {error}"))
            })
    } else {
        lines.next_line().await.map_err(|error| {
            HarnessError::AgentExecution(format!("failed reading claude stdout: {error}"))
        })
    }
}

pub(crate) async fn stream_claude_code_output(
    child: &mut tokio::process::Child,
    tx: &tokio::sync::mpsc::Sender<StreamItem>,
    idle_timeout: Option<Duration>,
) -> harness_core::error::Result<ParsedClaudeStreamOutput> {
    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| HarnessError::AgentExecution("claude stdout unavailable".into()))?;
    let mut lines = BufReader::new(stdout).lines();
    let mut parsed = ParsedClaudeStreamOutput::default();

    while let Some(line) = read_next_claude_line(&mut lines, child, idle_timeout).await? {
        parsed.raw_stdout.push_str(&line);
        parsed.raw_stdout.push('\n');
        let mut emitted_items = Vec::new();
        apply_claude_stream_line(&line, &mut parsed, &mut emitted_items);
        for item in emitted_items {
            let item_label = stream_item_label(&item);
            send_stream_item(tx, item, "claude", item_label).await?;
        }
    }

    let status = child.wait().await.map_err(|error| {
        HarnessError::AgentExecution(format!("failed waiting for claude process: {error}"))
    })?;
    if !status.success() {
        let tail_source = if parsed.output.is_empty() {
            &parsed.raw_stdout
        } else {
            &parsed.output
        };
        let stdout_tail = claude_stdout_tail(tail_source);
        let limit = usage_limit_suffix(&parsed);
        return Err(HarnessError::AgentExecution(if stdout_tail.is_empty() {
            format!("claude exited with {status}{limit}")
        } else {
            format!("claude exited with {status}: stdout_tail=[{stdout_tail}]{limit}")
        }));
    }

    if !parsed.completed {
        send_stream_item(
            tx,
            StreamItem::ItemCompleted {
                item: Item::AgentReasoning {
                    content: parsed.output.clone(),
                },
            },
            "claude",
            "item_completed",
        )
        .await?;
    }

    Ok(parsed)
}

#[cfg(test)]
mod tool_use_tests {
    use super::*;

    #[test]
    fn surfaces_nested_tool_use_from_assistant_line() {
        // Real Claude Code stream-json nests tool_use inside an assistant message.
        let line = r#"{"type":"assistant","message":{"role":"assistant","content":[{"type":"text","text":"Reading the file"},{"type":"tool_use","id":"toolu_1","name":"Read","input":{"file_path":"src/main.rs"}}]}}"#;
        let mut parsed = ParsedClaudeStreamOutput::default();
        let mut emitted = Vec::new();
        apply_claude_stream_line(line, &mut parsed, &mut emitted);

        // The assistant text becomes a MessageDelta; the tool_use becomes a
        // ToolCall item (which the runner maps to a live activity card).
        let tool = emitted.iter().find_map(|it| match it {
            StreamItem::ItemCompleted {
                item: Item::ToolCall { name, input, .. },
            } => Some((name.clone(), input.clone())),
            _ => None,
        });
        let (name, input) = tool.expect("a ToolCall item was emitted");
        assert_eq!(name, "Read");
        assert_eq!(
            input.get("file_path").and_then(|v| v.as_str()),
            Some("src/main.rs")
        );
    }

    #[test]
    fn tool_only_assistant_line_still_emits_toolcall() {
        // No text block → parse_stream_json_line returns None, but the tool call
        // must still surface.
        let line = r#"{"type":"assistant","message":{"content":[{"type":"tool_use","name":"Bash","input":{"command":"cargo test"}}]}}"#;
        let mut parsed = ParsedClaudeStreamOutput::default();
        let mut emitted = Vec::new();
        apply_claude_stream_line(line, &mut parsed, &mut emitted);
        assert!(emitted.iter().any(|it| matches!(
            it,
            StreamItem::ItemCompleted {
                item: Item::ToolCall { .. }
            }
        )));
    }

    #[test]
    fn text_only_assistant_line_has_no_tool_items() {
        let line = r#"{"type":"assistant","message":{"content":[{"type":"text","text":"just thinking"}]}}"#;
        assert!(claude_tool_use_items(line).is_empty());
    }
}

#[cfg(test)]
mod usage_limit_tests {
    use super::*;

    fn parse(lines: &[&str]) -> ParsedClaudeStreamOutput {
        parse_claude_stream_output(&lines.join("\n"))
    }

    #[test]
    fn a_rejected_rate_limit_event_puts_the_reset_in_the_error() {
        let parsed = parse(&[
            r#"{"type":"system","subtype":"init","session_id":"s1"}"#,
            r#"{"type":"rate_limit_event","rate_limit_info":{"status":"rejected","resetsAt":1762700000,"rateLimitType":"five_hour"},"session_id":"s1"}"#,
            r#"{"type":"result","subtype":"success","is_error":true,"result":"You've hit your limit"}"#,
        ]);
        assert_eq!(parsed.limit_resets_at, Some(1_762_700_000));
        assert_eq!(
            usage_limit_suffix(&parsed),
            " (usage limit reached, resets at unix 1762700000)"
        );
    }

    #[test]
    fn a_warning_rate_limit_event_is_not_a_limit() {
        let parsed = parse(&[
            r#"{"type":"rate_limit_event","rate_limit_info":{"status":"allowed_warning","resetsAt":1762700000,"utilization":0.9}}"#,
            r#"{"type":"result","subtype":"success","result":"Done"}"#,
        ]);
        assert_eq!(parsed.limit_resets_at, None);
        assert_eq!(usage_limit_suffix(&parsed), "");
        assert_eq!(parsed.output, "Done");
    }

    #[test]
    fn the_phrase_is_the_one_the_dag_reads() {
        assert_eq!(RESETS_AT_UNIX, harness_dag::usage_limit::RESETS_AT_UNIX);
        let parsed = ParsedClaudeStreamOutput {
            limit_resets_at: Some(1_762_700_000),
            ..Default::default()
        };
        let error = format!(
            "claude exited with exit status: 1{}",
            usage_limit_suffix(&parsed)
        );
        let now = chrono::Utc::now();
        assert_eq!(
            harness_dag::usage_limit::reset_time(&error, now).map(|t| t.timestamp()),
            Some(1_762_700_000)
        );
    }
}
