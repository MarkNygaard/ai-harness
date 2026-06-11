use axum::{extract::State, http::StatusCode, Json};
use chrono::{DateTime, Duration, Utc};
use serde::Serialize;
use serde_json::Value;
use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use crate::http::AppState;

const DEFAULT_TOKEN_USAGE_WINDOW_HOURS: i64 = 24;

#[derive(Debug, Default, Clone, Serialize)]
struct UsageBucket {
    input_tokens: u64,
    output_tokens: u64,
    cache_read_tokens: u64,
    cache_create_tokens: u64,
    request_count: u64,
    session_count: u64,
    /// Notional USD cost, priced per-record at each record's model rate (see
    /// [`record_cost`]) and summed in. Model-aware — unlike a flat post-aggregate
    /// estimate, a bucket spanning several models reflects each model's price.
    cost_usd: f64,
}

/// Per-hour, per-model token totals for the trend chart.
#[derive(Debug, Default, Clone, Serialize)]
struct HourModelBucket {
    tokens: u64,
    requests: u64,
}

#[derive(Debug, Clone)]
struct ParsedUsageRecord {
    input: u64,
    output: u64,
    cache_read: u64,
    cache_create: u64,
    model: String,
    occurred_at: DateTime<Utc>,
    day: String,
    hour: String,
}

#[derive(Debug, Clone, Copy)]
struct TokenUsageWindow {
    hours: i64,
    since: DateTime<Utc>,
    now: DateTime<Utc>,
}

impl TokenUsageWindow {
    fn last_hours(now: DateTime<Utc>, hours: i64) -> Self {
        Self {
            hours,
            since: now - Duration::hours(hours),
            now,
        }
    }

    fn contains(&self, ts: DateTime<Utc>) -> bool {
        ts >= self.since && ts <= self.now
    }

    fn to_json(self) -> Value {
        serde_json::json!({
            "hours": self.hours,
            "since": self.since.to_rfc3339(),
            "now": self.now.to_rfc3339(),
        })
    }
}

#[derive(Debug, Clone)]
struct ParsedUsageTimestamp {
    occurred_at: DateTime<Utc>,
    day: String,
    hour: String,
}

fn home_dir() -> Result<PathBuf, String> {
    std::env::var("HOME")
        .map(PathBuf::from)
        .map_err(|_| "HOME is not set; cannot locate token usage source".to_string())
}

/// GET /api/token-usage — aggregate token usage from Claude CLI session JSONL files.
///
/// Returns hourly request counts, per-model time series, daily totals,
/// and per-task breakdowns. Timestamps are taken from each JSONL entry's
/// `timestamp` field (ISO-8601) for accurate bucketing.
///
/// This endpoint is intentionally strict: malformed source data returns an
/// explicit error response instead of silently producing partial/empty metrics.
pub async fn token_usage(State(state): State<Arc<AppState>>) -> (StatusCode, Json<Value>) {
    let window = TokenUsageWindow::last_hours(Utc::now(), DEFAULT_TOKEN_USAGE_WINDOW_HOURS);
    let home = match home_dir() {
        Ok(path) => path,
        Err(err) => return error_response(err),
    };

    let claude_projects_dir = home.join(".claude").join("projects");

    // When --no-session-persistence is active, Claude Code does not write
    // session JSONL files to disk. Distinguish three cases:
    //
    //   1. Projects directory absent (NotFound) — potentially a misconfiguration
    //      (wrong $HOME, deleted directory, path regression). Emit a warning
    //      so operators can detect it in logs, and include a diagnostic field
    //      in the response so callers can distinguish from genuine empty usage.
    //
    //   2. Directory present but no JSONL files — the expected steady-state
    //      when --no-session-persistence is active. Silently return empty.
    //
    //   3. Metadata error other than NotFound (e.g. permission denied) — this
    //      is a real OS-level failure that must surface as an error, not be
    //      silently treated as "directory absent".
    match std::fs::metadata(&claude_projects_dir) {
        Ok(meta) if meta.is_dir() => {}
        Ok(_) => {
            return error_response(format!(
                "token usage path is not a directory: {}",
                claude_projects_dir.display()
            ));
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            // NotFound is the expected steady state when --no-session-persistence
            // is active.  Log at debug so that normal deployments do not flood
            // warn-level logs on every 5 s dashboard poll.  The source_dir_missing
            // flag in the JSON response surfaces the diagnostic to operators via
            // the dashboard UI instead.
            tracing::debug!(
                path = %claude_projects_dir.display(),
                "token_usage: session projects directory not found; \
                 returning empty metrics (expected with --no-session-persistence)"
            );
            return missing_dir_response(window);
        }
        Err(e) => {
            return error_response(format!(
                "cannot access token usage directory {}: {e}",
                claude_projects_dir.display()
            ));
        }
    }

    let files = match collect_session_files(&claude_projects_dir) {
        Ok(files) => files,
        Err(err) => return error_response(err),
    };
    let files = match filter_session_files_for_window(files, window) {
        Ok(files) => files,
        Err(err) => return error_response(err),
    };

    if files.is_empty() {
        return empty_usage_response(window);
    }

    let mut by_day: BTreeMap<String, UsageBucket> = BTreeMap::new();
    let mut by_hour: BTreeMap<String, UsageBucket> = BTreeMap::new();
    let mut model_trend: BTreeMap<String, HashMap<String, HourModelBucket>> = BTreeMap::new();
    let mut by_model: HashMap<String, UsageBucket> = HashMap::new();
    let mut totals = UsageBucket::default();
    let mut task_usage: HashMap<String, UsageBucket> = HashMap::new();

    let all_tasks = match state.core.tasks.list_all_summaries_with_terminal().await {
        Ok(tasks) => tasks,
        Err(e) => {
            return error_response(format!("failed to list tasks for usage attribution: {e}"))
        }
    };
    let task_ids: std::collections::HashSet<String> =
        all_tasks.iter().map(|t| t.id.0.clone()).collect();

    for file in &files {
        let task_id = extract_task_id(file);
        let mut sess = UsageBucket::default();

        let content = match std::fs::read_to_string(file) {
            Ok(content) => content,
            Err(err) => {
                return error_response(format!(
                    "failed to read token usage file {}: {err}",
                    file.display()
                ));
            }
        };

        for (idx, line) in content.lines().enumerate() {
            if line.trim().is_empty() {
                continue;
            }
            let entry: Value = match serde_json::from_str(line) {
                Ok(entry) => entry,
                Err(err) => {
                    return error_response(format!(
                        "invalid JSON in {}:{}: {err}",
                        file.display(),
                        idx + 1
                    ));
                }
            };

            let ctx = format!("{}:{}", file.display(), idx + 1);
            let parsed = match parse_usage_record(&entry, &ctx) {
                Ok(Some(record)) => record,
                Ok(None) => continue,
                Err(err) => return error_response(err),
            };
            if !window.contains(parsed.occurred_at) {
                continue;
            }

            let total_ctx = parsed.input + parsed.cache_read + parsed.cache_create;
            // Price each record at its own model's rate, then sum into the buckets.
            // Computed before `parsed.model` is moved into `by_model` below.
            let cost = record_cost(
                &parsed.model,
                parsed.input,
                parsed.output,
                parsed.cache_read,
                parsed.cache_create,
            );

            let hb = by_hour.entry(parsed.hour.clone()).or_default();
            hb.request_count += 1;
            hb.input_tokens += parsed.input;
            hb.output_tokens += parsed.output;
            hb.cache_read_tokens += parsed.cache_read;
            hb.cache_create_tokens += parsed.cache_create;
            hb.cost_usd += cost;

            let mb = model_trend
                .entry(parsed.hour.clone())
                .or_default()
                .entry(parsed.model.clone())
                .or_default();
            mb.tokens += total_ctx;
            mb.requests += 1;

            let db = by_day.entry(parsed.day).or_default();
            db.request_count += 1;
            db.input_tokens += parsed.input;
            db.output_tokens += parsed.output;
            db.cache_read_tokens += parsed.cache_read;
            db.cache_create_tokens += parsed.cache_create;
            db.cost_usd += cost;

            let by_model_bucket = by_model.entry(parsed.model).or_default();
            by_model_bucket.request_count += 1;
            by_model_bucket.input_tokens += parsed.input;
            by_model_bucket.output_tokens += parsed.output;
            by_model_bucket.cache_read_tokens += parsed.cache_read;
            by_model_bucket.cache_create_tokens += parsed.cache_create;
            by_model_bucket.cost_usd += cost;

            sess.input_tokens += parsed.input;
            sess.output_tokens += parsed.output;
            sess.cache_read_tokens += parsed.cache_read;
            sess.cache_create_tokens += parsed.cache_create;
            sess.request_count += 1;
            sess.cost_usd += cost;
        }

        if sess.request_count == 0 {
            continue;
        }
        sess.session_count = 1;

        if let Some(tid) = &task_id {
            if task_ids.contains(tid) {
                accumulate(task_usage.entry(tid.clone()).or_default(), &sess);
            }
        }

        accumulate(&mut totals, &sess);
    }

    let cost = totals.cost_usd;

    let task_usage_vec: Vec<Value> = {
        let mut items: Vec<_> = task_usage.into_iter().collect();
        items.sort_by(|a, b| {
            let ca = a.1.input_tokens + a.1.cache_read_tokens + a.1.cache_create_tokens;
            let cb = b.1.input_tokens + b.1.cache_read_tokens + b.1.cache_create_tokens;
            cb.cmp(&ca)
        });
        items
            .into_iter()
            .take(30)
            .map(|(tid, usage)| {
                let task_meta = all_tasks.iter().find(|t| t.id.0 == tid);
                let ctx = usage.input_tokens + usage.cache_read_tokens + usage.cache_create_tokens;
                serde_json::json!({
                    "task_id": tid,
                    "repo": task_meta.and_then(|t| t.repo.clone()),
                    "status": task_meta.map(|t| format!("{:?}", t.status)),
                    "context_tokens": ctx,
                    "output_tokens": usage.output_tokens,
                    "requests": usage.request_count,
                    "cost_usd": usage.cost_usd,
                })
            })
            .collect()
    };

    let mut all_models: Vec<String> = by_model.keys().cloned().collect();
    all_models.sort_by(|a, b| {
        let ta = by_model[a].input_tokens + by_model[a].cache_read_tokens;
        let tb = by_model[b].input_tokens + by_model[b].cache_read_tokens;
        tb.cmp(&ta)
    });

    let total_context = totals.input_tokens + totals.cache_read_tokens + totals.cache_create_tokens;

    let body = serde_json::json!({
        "window": window.to_json(),
        "by_day": by_day,
        "by_hour": by_hour,
        "model_trend": model_trend,
        "by_model": by_model,
        "models": all_models,
        "totals": totals,
        "total_context": total_context,
        "total_requests": totals.request_count,
        "estimated_cost_usd": cost,
        "task_usage": task_usage_vec,
        "session_count": totals.session_count,
    });

    (StatusCode::OK, Json(body))
}

fn error_response(message: String) -> (StatusCode, Json<Value>) {
    tracing::error!("token_usage: {message}");
    (
        StatusCode::INTERNAL_SERVER_ERROR,
        Json(serde_json::json!({ "error": message })),
    )
}

/// Return zeroed metrics with 200 OK when the projects directory is missing.
///
/// Unlike `empty_usage_response`, this includes `"source_dir_missing": true`
/// so callers and monitoring can distinguish "directory absent" (potentially a
/// misconfiguration) from "directory present but no sessions yet" (expected
/// in --no-session-persistence environments).
fn missing_dir_response(window: TokenUsageWindow) -> (StatusCode, Json<Value>) {
    let body = serde_json::json!({
        "window": window.to_json(),
        "by_day": {},
        "by_hour": {},
        "model_trend": {},
        "by_model": {},
        "models": [],
        "totals": UsageBucket::default(),
        "total_context": 0,
        "total_requests": 0,
        "estimated_cost_usd": 0.0,
        "task_usage": [],
        "session_count": 0,
        "source_dir_missing": true,
    });
    (StatusCode::OK, Json(body))
}

/// Return zeroed metrics with 200 OK when no session files are available.
///
/// This is the correct response when `--no-session-persistence` is active:
/// agents do not write JSONL files, so an absent or empty projects directory
/// is expected, not an error.
fn empty_usage_response(window: TokenUsageWindow) -> (StatusCode, Json<Value>) {
    let body = serde_json::json!({
        "window": window.to_json(),
        "by_day": {},
        "by_hour": {},
        "model_trend": {},
        "by_model": {},
        "models": [],
        "totals": UsageBucket::default(),
        "total_context": 0,
        "total_requests": 0,
        "estimated_cost_usd": 0.0,
        "task_usage": [],
        "session_count": 0,
    });
    (StatusCode::OK, Json(body))
}

fn parse_usage_record(entry: &Value, ctx: &str) -> Result<Option<ParsedUsageRecord>, String> {
    let Some(usage) = entry.pointer("/message/usage") else {
        return Ok(None);
    };

    let input = required_u64(usage, "input_tokens", ctx)?;
    let output = required_u64(usage, "output_tokens", ctx)?;
    // Cache fields are optional — Codex agent and older Claude CLI builds
    // report all context as input_tokens without a cache breakdown.
    let cache_read = optional_u64(usage, "cache_read_input_tokens");
    let cache_create = optional_u64(usage, "cache_creation_input_tokens");

    let model = required_str_at_pointer(entry, "/message/model", ctx)?.to_string();
    let ts = required_str_field(entry, "timestamp", ctx)?;
    let timestamp = parse_timestamp(ts).map_err(|err| format!("{ctx}: {err}"))?;

    Ok(Some(ParsedUsageRecord {
        input,
        output,
        cache_read,
        cache_create,
        model,
        occurred_at: timestamp.occurred_at,
        day: timestamp.day,
        hour: timestamp.hour,
    }))
}

fn required_u64(obj: &Value, key: &str, ctx: &str) -> Result<u64, String> {
    obj.get(key)
        .and_then(Value::as_u64)
        .ok_or_else(|| format!("{ctx}: missing or invalid usage field '{key}'"))
}

fn optional_u64(obj: &Value, key: &str) -> u64 {
    obj.get(key).and_then(Value::as_u64).unwrap_or(0)
}

fn required_str_field<'a>(obj: &'a Value, key: &str, ctx: &str) -> Result<&'a str, String> {
    obj.get(key)
        .and_then(Value::as_str)
        .ok_or_else(|| format!("{ctx}: missing or invalid string field '{key}'"))
}

fn required_str_at_pointer<'a>(
    obj: &'a Value,
    pointer: &str,
    ctx: &str,
) -> Result<&'a str, String> {
    obj.pointer(pointer)
        .and_then(Value::as_str)
        .ok_or_else(|| format!("{ctx}: missing or invalid string field at '{pointer}'"))
}

fn accumulate(dst: &mut UsageBucket, src: &UsageBucket) {
    dst.input_tokens += src.input_tokens;
    dst.output_tokens += src.output_tokens;
    dst.cache_read_tokens += src.cache_read_tokens;
    dst.cache_create_tokens += src.cache_create_tokens;
    dst.request_count += src.request_count;
    dst.session_count += src.session_count;
    dst.cost_usd += src.cost_usd;
}

/// Per-MTok USD rates for a model family.
struct ModelRates {
    input: f64,
    output: f64,
    cache_read: f64,
    cache_write: f64,
}

/// Notional per-MTok price table, matched by substring on the (lowercased) model
/// id so id variants resolve to a family (`claude-opus-4-8`, `openai-codex/gpt-5.5`,
/// `kimi-for-coding`, …).
///
/// This is a **notional** cost basis — it lets runs on subscription models be
/// compared against API-billed ones on a common dollar scale, NOT an invoice.
/// Claude rates are list price (input/output from the model catalog; cache-read =
/// 0.1× input, 5-minute cache-write = 1.25× input). The GPT-5.5 and Kimi K2.6
/// rows are the providers' published direct API prices (Jun 2026) — verified, but
/// the entries most likely to drift, so re-check them here when prices change.
/// Unknown models fall back to Sonnet-tier, which preserves the prior flat-rate
/// behavior.
fn rates_for_model(model: &str) -> ModelRates {
    let m = model.to_ascii_lowercase();
    if m.contains("opus") {
        ModelRates {
            input: 5.0,
            output: 25.0,
            cache_read: 0.5,
            cache_write: 6.25,
        }
    } else if m.contains("haiku") {
        ModelRates {
            input: 1.0,
            output: 5.0,
            cache_read: 0.1,
            cache_write: 1.25,
        }
    } else if m.contains("fable") {
        ModelRates {
            input: 10.0,
            output: 50.0,
            cache_read: 1.0,
            cache_write: 12.5,
        }
    } else if m.contains("sonnet") {
        ModelRates {
            input: 3.0,
            output: 15.0,
            cache_read: 0.3,
            cache_write: 3.75,
        }
    } else if m.contains("gpt-5") || m.contains("codex") || m.contains("openai") {
        // OpenAI GPT-5.5 (openai.com/api/pricing, Jun 2026): $5 in / $30 out,
        // cached input $0.50 (90% off). OpenAI does not surcharge cache writes,
        // so cache_write = input. (>272K-token prompts carry a 2x/1.5x
        // long-context surcharge not modeled here.)
        ModelRates {
            input: 5.0,
            output: 30.0,
            cache_read: 0.50,
            cache_write: 5.0,
        }
    } else if m.contains("kimi") || m.contains("moonshot") {
        // Moonshot Kimi K2.6 (platform.moonshot direct, Jun 2026): $0.95
        // cache-miss in / $4.00 out, cache-hit input $0.16. No cache-write
        // surcharge, so cache_write = input.
        ModelRates {
            input: 0.95,
            output: 4.0,
            cache_read: 0.16,
            cache_write: 0.95,
        }
    } else if m.contains("composer") {
        // Cursor Composer 2.5 (cursor.com/docs/models-and-pricing, Jun 2026):
        // standard tier $0.50 in / $2.50 out / $0.20 cache-read. cache_read
        // dominates a coding run (millions of cached tokens), so the published
        // $0.20 — NOT a 0.1x-of-input guess — is what reconciles notional cost
        // with Cursor's usage dashboard. No write-cache rate is published; keep
        // input-rate as a safe upper bound (writes are ~0 in practice).
        // (A "fast" tier at $3/$15/$0.35 exists; standard is the default.)
        ModelRates {
            input: 0.50,
            output: 2.50,
            cache_read: 0.20,
            cache_write: 0.50,
        }
    } else {
        // Unknown model — Sonnet-tier fallback (matches the prior estimate).
        ModelRates {
            input: 3.0,
            output: 15.0,
            cache_read: 0.3,
            cache_write: 3.75,
        }
    }
}

/// Notional USD cost for a single usage record, priced at its model's rate.
fn record_cost(model: &str, input: u64, output: u64, cache_read: u64, cache_create: u64) -> f64 {
    let r = rates_for_model(model);
    let has_cache = cache_read > 0 || cache_create > 0;
    let (eff_input, eff_cache_read) = if has_cache {
        (input as f64, cache_read as f64)
    } else {
        // No cache breakdown (Codex agent / older Claude CLI builds report all
        // context as input_tokens): assume a 90% cache-read hit rate. Same
        // heuristic as the prior aggregate estimate, applied per record.
        let total = input as f64;
        (total * 0.10, total * 0.90)
    };
    eff_input / 1e6 * r.input
        + output as f64 / 1e6 * r.output
        + eff_cache_read / 1e6 * r.cache_read
        + cache_create as f64 / 1e6 * r.cache_write
}

/// Parse an ISO-8601 timestamp into (YYYY-MM-DD, YYYY-MM-DD HH:00).
fn parse_timestamp(ts: &str) -> Result<ParsedUsageTimestamp, String> {
    if ts.as_bytes().get(10) != Some(&b'T') {
        return Err(format!("invalid timestamp '{ts}': missing 'T' separator"));
    }
    let occurred_at = DateTime::parse_from_rfc3339(ts)
        .map_err(|err| format!("invalid timestamp '{ts}': {err}"))?
        .with_timezone(&Utc);
    let (day, hour) = if ts.ends_with('Z') || ts.ends_with("+00:00") {
        let day = ts
            .get(..10)
            .ok_or_else(|| format!("invalid timestamp '{ts}': missing date component"))?;
        let hour = ts
            .get(11..13)
            .ok_or_else(|| format!("invalid timestamp '{ts}': missing hour component"))?;
        (day.to_string(), format!("{day} {hour}:00"))
    } else {
        (
            occurred_at.format("%Y-%m-%d").to_string(),
            occurred_at.format("%Y-%m-%d %H:00").to_string(),
        )
    };
    Ok(ParsedUsageTimestamp {
        occurred_at,
        day,
        hour,
    })
}

fn collect_session_files(claude_projects_dir: &Path) -> Result<Vec<PathBuf>, String> {
    let mut files = Vec::new();
    let entries = std::fs::read_dir(claude_projects_dir).map_err(|err| {
        format!(
            "failed to read token usage root directory {}: {err}",
            claude_projects_dir.display()
        )
    })?;

    for entry in entries {
        let entry = entry.map_err(|err| {
            format!(
                "failed to iterate token usage root directory {}: {err}",
                claude_projects_dir.display()
            )
        })?;
        let name = entry.file_name();
        if !name.to_string_lossy().contains("harness-workspaces") {
            continue;
        }
        let dir = entry.path();
        if !dir.is_dir() {
            continue;
        }
        collect_jsonl_in_dir(&dir, &mut files)?;

        let sub_entries = std::fs::read_dir(&dir)
            .map_err(|err| format!("failed to read {}: {err}", dir.display()))?;
        for sub in sub_entries {
            let sub = sub.map_err(|err| format!("failed to iterate {}: {err}", dir.display()))?;
            let sub_path = sub.path();
            if sub_path.is_dir() {
                let subagents_dir = sub_path.join("subagents");
                if subagents_dir.is_dir() {
                    collect_jsonl_in_dir(&subagents_dir, &mut files)?;
                }
            }
        }
    }

    Ok(files)
}

fn filter_session_files_for_window(
    files: Vec<PathBuf>,
    window: TokenUsageWindow,
) -> Result<Vec<PathBuf>, String> {
    let mut filtered = Vec::new();
    for file in files {
        if file_may_have_window_records(&file, window)? {
            filtered.push(file);
        }
    }
    Ok(filtered)
}

fn file_may_have_window_records(path: &Path, window: TokenUsageWindow) -> Result<bool, String> {
    let metadata = std::fs::metadata(path)
        .map_err(|err| format!("failed to stat token usage file {}: {err}", path.display()))?;
    let modified = metadata.modified().map_err(|err| {
        format!(
            "failed to read token usage file mtime {}: {err}",
            path.display()
        )
    })?;
    let modified_at: DateTime<Utc> = modified.into();
    Ok(modified_at >= window.since)
}

fn collect_jsonl_in_dir(dir: &Path, out: &mut Vec<PathBuf>) -> Result<(), String> {
    let entries = std::fs::read_dir(dir).map_err(|err| {
        format!(
            "failed to read token usage directory {}: {err}",
            dir.display()
        )
    })?;

    for entry in entries {
        let entry = entry.map_err(|err| {
            format!(
                "failed to iterate token usage directory {}: {err}",
                dir.display()
            )
        })?;
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) == Some("jsonl") {
            out.push(path);
        }
    }

    Ok(())
}

fn extract_task_id(path: &Path) -> Option<String> {
    path.parent()
        .and_then(|p| p.file_name())
        .and_then(|n| n.to_str())
        .and_then(extract_task_uuid)
        .or_else(|| {
            path.parent()
                .and_then(|p| p.parent())
                .and_then(|p| p.parent())
                .and_then(|p| p.file_name())
                .and_then(|n| n.to_str())
                .and_then(extract_task_uuid)
        })
}

fn extract_task_uuid(name: &str) -> Option<String> {
    let marker = "harness-workspaces-";
    let pos = name.rfind(marker)?;
    let after = &name[pos + marker.len()..];
    if after.len() >= 36 && after.as_bytes()[8] == b'-' && after.as_bytes()[13] == b'-' {
        Some(after[..36].to_string())
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_timestamp_rejects_invalid_input() {
        assert!(parse_timestamp("invalid").is_err());
        assert!(parse_timestamp("2026-03-26 03:05:56Z").is_err());
    }

    #[test]
    fn parse_timestamp_preserves_utc_bucket_for_offset_input() {
        let parsed = parse_timestamp("2026-03-26T23:05:56-02:00").unwrap();
        assert_eq!(parsed.day, "2026-03-27");
        assert_eq!(parsed.hour, "2026-03-27 01:00");
    }

    #[test]
    fn parse_usage_record_treats_cache_fields_as_optional() {
        let entry = serde_json::json!({
            "timestamp": "2026-03-26T03:05:56.523Z",
            "message": {
                "model": "claude-sonnet",
                "usage": {
                    "input_tokens": 100,
                    "output_tokens": 20
                }
            }
        });
        let parsed = parse_usage_record(&entry, "test:1").unwrap().unwrap();
        assert_eq!(parsed.cache_read, 0);
        assert_eq!(parsed.cache_create, 0);
        assert_eq!(parsed.input, 100);
    }

    #[test]
    fn parse_usage_record_accepts_valid_usage_line() {
        let entry = serde_json::json!({
            "timestamp": "2026-03-26T03:05:56.523Z",
            "message": {
                "model": "claude-sonnet",
                "usage": {
                    "input_tokens": 100,
                    "output_tokens": 20,
                    "cache_read_input_tokens": 50,
                    "cache_creation_input_tokens": 10
                }
            }
        });

        let parsed = parse_usage_record(&entry, "test:1").unwrap().unwrap();
        assert_eq!(parsed.input, 100);
        assert_eq!(parsed.output, 20);
        assert_eq!(parsed.cache_read, 50);
        assert_eq!(parsed.cache_create, 10);
        assert_eq!(parsed.day, "2026-03-26");
        assert_eq!(parsed.hour, "2026-03-26 03:00");
    }

    #[test]
    fn usage_window_keeps_only_recent_records() {
        let now = DateTime::parse_from_rfc3339("2026-05-20T10:00:00Z")
            .unwrap()
            .with_timezone(&Utc);
        let window = TokenUsageWindow::last_hours(now, 24);
        let recent = parse_usage_record(
            &serde_json::json!({
                "timestamp": "2026-05-20T09:59:00Z",
                "message": {
                    "model": "claude-sonnet",
                    "usage": {
                        "input_tokens": 100,
                        "output_tokens": 20
                    }
                }
            }),
            "recent:1",
        )
        .unwrap()
        .unwrap();
        let old = parse_usage_record(
            &serde_json::json!({
                "timestamp": "2026-05-18T09:59:00Z",
                "message": {
                    "model": "claude-sonnet",
                    "usage": {
                        "input_tokens": 100,
                        "output_tokens": 20
                    }
                }
            }),
            "old:1",
        )
        .unwrap()
        .unwrap();

        assert!(window.contains(recent.occurred_at));
        assert!(!window.contains(old.occurred_at));
    }

    #[test]
    fn record_cost_is_model_aware() {
        // 1M output tokens priced at each family's output rate.
        let out_only = |model: &str| record_cost(model, 0, 1_000_000, 0, 0);
        assert!((out_only("claude-opus-4-8") - 25.0).abs() < 1e-9);
        assert!((out_only("claude-sonnet-4-6") - 15.0).abs() < 1e-9);
        assert!((out_only("claude-haiku-4-5") - 5.0).abs() < 1e-9);
        assert!((out_only("claude-fable-5") - 50.0).abs() < 1e-9);
        assert!((out_only("openai-codex/gpt-5.5") - 30.0).abs() < 1e-9);
        assert!((out_only("kimi-for-coding") - 4.0).abs() < 1e-9);
        assert!((out_only("composer-2.5") - 2.50).abs() < 1e-9);
        // Unknown model falls back to Sonnet-tier (prior flat-rate behavior).
        assert!((out_only("some-future-model") - 15.0).abs() < 1e-9);
    }

    #[test]
    fn record_cost_splits_input_when_no_cache_breakdown() {
        // No cache fields: 90% of input is priced as cache-read, 10% as input.
        // Sonnet: 1M input → 0.1M*$3 + 0.9M*$0.30 = $0.30 + $0.27 = $0.57.
        let cost = record_cost("claude-sonnet-4-6", 1_000_000, 0, 0, 0);
        assert!((cost - 0.57).abs() < 1e-9, "got {cost}");
    }

    #[test]
    fn accumulate_sums_cost() {
        let mut dst = UsageBucket::default();
        let src = UsageBucket {
            cost_usd: 1.5,
            ..Default::default()
        };
        accumulate(&mut dst, &src);
        accumulate(&mut dst, &src);
        assert!((dst.cost_usd - 3.0).abs() < 1e-9);
    }
}
