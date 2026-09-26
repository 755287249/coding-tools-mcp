//! Live, in-memory activity feed that powers the desktop "task panel".
//!
//! Every MCP `tools/call` is recorded here twice: once when it starts (status
//! `running`) and once when it finishes (success / error, duration, file
//! changes and diff). Nothing is persisted to disk; the feed is a bounded ring
//! buffer per workspace profile so it stays cheap even during long sessions.

use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::{Mutex, OnceLock};
use std::time::{SystemTime, UNIX_EPOCH};

use serde::Serialize;
use serde_json::Value;

const MAX_EVENTS: usize = 240;
const MAX_DIFF_BYTES: usize = 256 * 1024;
const MAX_TITLE_CHARS: usize = 160;
const MAX_DETAIL_CHARS: usize = 600;
const MAX_PATHS: usize = 24;

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ActivityChange {
    pub path: String,
    pub operation: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub bytes_before: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub bytes_after: Option<u64>,
    pub added: u32,
    pub removed: u32,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ActivityEvent {
    pub seq: u64,
    pub rev: u64,
    pub started_ms: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub duration_ms: Option<u64>,
    pub tool: String,
    pub kind: &'static str,
    /// `running`, `success` or `error`.
    pub status: &'static str,
    pub title: String,
    pub paths: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    pub changes: Vec<ActivityChange>,
    pub dry_run: bool,
    pub has_diff: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub diff: Option<String>,
    pub diff_truncated: bool,
    pub request_bytes: u64,
    pub response_bytes: u64,
}

#[derive(Debug, Clone, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ActivityPlan {
    pub task_id: String,
    pub objective: String,
    pub status: String,
    pub completed_steps: Vec<String>,
    pub pending_steps: Vec<String>,
    pub updated_ms: u64,
}

#[derive(Debug, Clone, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ActivityStats {
    pub total: u64,
    pub success: u64,
    pub errors: u64,
    pub running: u64,
    pub total_duration_ms: u64,
    pub avg_duration_ms: u64,
    pub p95_duration_ms: u64,
    pub bytes_in: u64,
    pub bytes_out: u64,
    pub reads: u64,
    pub edits: u64,
    pub execs: u64,
    pub searches: u64,
    pub files_changed: u64,
    pub files_viewed: u64,
    pub bytes_added: u64,
    pub bytes_removed: u64,
    pub lines_added: u64,
    pub lines_removed: u64,
    pub first_ms: u64,
    pub last_ms: u64,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ActivitySnapshot {
    pub events: Vec<ActivityEvent>,
    pub rev: u64,
    pub stats: ActivityStats,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub plan: Option<ActivityPlan>,
    /// True when the caller's `since_rev` is older than the retained window
    /// and the client should drop its cached events.
    pub reset: bool,
}

#[derive(Default)]
struct ProfileFeed {
    seq: u64,
    rev: u64,
    events: VecDeque<ActivityEvent>,
    stats: ActivityStats,
    durations: VecDeque<u64>,
    changed: HashSet<String>,
    viewed: HashSet<String>,
    plan: Option<ActivityPlan>,
    /// Oldest revision still fully represented by `events`.
    floor_rev: u64,
}

fn feeds() -> &'static Mutex<HashMap<String, ProfileFeed>> {
    static FEEDS: OnceLock<Mutex<HashMap<String, ProfileFeed>>> = OnceLock::new();
    FEEDS.get_or_init(|| Mutex::new(HashMap::new()))
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

fn truncate_chars(text: &str, max: usize) -> String {
    let trimmed = text.trim();
    if trimmed.chars().count() <= max {
        return trimmed.to_string();
    }
    let mut out: String = trimmed.chars().take(max).collect();
    out.push('…');
    out
}

fn truncate_bytes(text: &str, max: usize) -> (String, bool) {
    if text.len() <= max {
        return (text.to_string(), false);
    }
    let mut end = max;
    while end > 0 && !text.is_char_boundary(end) {
        end -= 1;
    }
    (text[..end].to_string(), true)
}

/// Coarse category used by the UI for icons, colours and counters.
pub fn tool_kind(tool: &str) -> &'static str {
    match tool {
        "read_file" | "read_many" | "view_image" => "read",
        "search_text" | "list_files" | "project_map" | "list_workspace_folders" => "search",
        "edit" | "apply_patch" | "file_ops" | "format_files" => "edit",
        "patch_check" => "check",
        "exec_command" | "exec_many" | "wait_command" | "send_input" | "kill_session"
        | "read_output" | "list_sessions" | "exec_health_check" | "resolve_operation" => "exec",
        "start_task"
        | "update_task"
        | "pause_task"
        | "resume_task"
        | "fail_task"
        | "close_failed_task"
        | "rollback_task"
        | "finish_task"
        | "task_context"
        | "list_task_events"
        | "change_summary"
        | "project_state"
        | "history_session_bootstrap"
        | "history_session_checkpoint"
        | "history_session_validate" => "plan",
        _ if tool.starts_with("git_") => "git",
        _ if tool.starts_with("desktop_") => "desktop",
        _ => "other",
    }
}

fn push_path(paths: &mut Vec<String>, path: &str) {
    let path = path.trim();
    if path.is_empty() || paths.len() >= MAX_PATHS || paths.iter().any(|p| p == path) {
        return;
    }
    paths.push(path.to_string());
}

fn collect_arg_paths(value: &Value, depth: usize, paths: &mut Vec<String>) {
    if depth > 4 || paths.len() >= MAX_PATHS {
        return;
    }
    match value {
        Value::Object(map) => {
            for (key, item) in map {
                match (key.as_str(), item) {
                    ("path" | "destination" | "source" | "file", Value::String(text)) => {
                        push_path(paths, text)
                    }
                    ("paths" | "files", Value::Array(items)) => {
                        for entry in items {
                            match entry {
                                Value::String(text) => push_path(paths, text),
                                other => collect_arg_paths(other, depth + 1, paths),
                            }
                        }
                    }
                    ("patch", Value::String(text)) => collect_patch_paths(text, paths),
                    (_, Value::Object(_) | Value::Array(_)) => {
                        collect_arg_paths(item, depth + 1, paths)
                    }
                    _ => {}
                }
            }
        }
        Value::Array(items) => {
            for item in items {
                collect_arg_paths(item, depth + 1, paths);
            }
        }
        _ => {}
    }
}

fn collect_patch_paths(patch: &str, paths: &mut Vec<String>) {
    for line in patch.lines() {
        for prefix in [
            "*** Update File: ",
            "*** Add File: ",
            "*** Delete File: ",
            "*** Move to: ",
        ] {
            if let Some(rest) = line.strip_prefix(prefix) {
                push_path(paths, rest);
            }
        }
        if let Some(rest) = line.strip_prefix("+++ b/") {
            push_path(paths, rest);
        }
    }
}

fn str_arg<'a>(args: &'a Value, key: &str) -> Option<&'a str> {
    args.get(key)
        .and_then(Value::as_str)
        .filter(|s| !s.trim().is_empty())
}

fn describe_request(tool: &str, args: &Value, paths: &[String]) -> String {
    let joined = || match paths.len() {
        0 => String::new(),
        1 => paths[0].clone(),
        n => format!("{} (+{})", paths[0], n - 1),
    };
    let title = match tool {
        "read_file" => {
            let path = joined();
            match (
                args.get("start_line").and_then(Value::as_u64),
                args.get("end_line").and_then(Value::as_u64),
            ) {
                (Some(start), Some(end)) => format!("{path}:{start}-{end}"),
                (Some(start), None) if start > 1 => format!("{path}:{start}-"),
                _ => path,
            }
        }
        "search_text" => {
            let query = str_arg(args, "query").unwrap_or("");
            let scope = str_arg(args, "path").unwrap_or(".");
            if query.is_empty() {
                scope.to_string()
            } else {
                format!("\"{query}\" in {scope}")
            }
        }
        "list_files" | "project_map" => str_arg(args, "path").unwrap_or(".").to_string(),
        "exec_command" => {
            if let Some(cmd) = str_arg(args, "cmd") {
                cmd.to_string()
            } else if let Some(program) = str_arg(args, "program") {
                let rest = args
                    .get("args")
                    .and_then(Value::as_array)
                    .map(|items| {
                        items
                            .iter()
                            .filter_map(Value::as_str)
                            .collect::<Vec<_>>()
                            .join(" ")
                    })
                    .unwrap_or_default();
                format!("{program} {rest}")
            } else if let Some(script) = str_arg(args, "script") {
                script.lines().next().unwrap_or("").to_string()
            } else {
                String::new()
            }
        }
        "exec_many" => {
            let count = args
                .get("commands")
                .and_then(Value::as_array)
                .map(Vec::len)
                .unwrap_or(0);
            format!("{count} commands")
        }
        "start_task" => str_arg(args, "objective").unwrap_or("").to_string(),
        "finish_task" => str_arg(args, "summary").unwrap_or("").to_string(),
        "switch_workspace_folder" | "conversation_bootstrap" => str_arg(args, "folder")
            .or_else(|| str_arg(args, "folder_id"))
            .unwrap_or("")
            .to_string(),
        _ => joined(),
    };
    truncate_chars(&title.replace('\n', " "), MAX_TITLE_CHARS)
}

/// Splits a multi-file unified diff and counts +/- lines per file.
fn diff_line_counts(diff: &str) -> HashMap<String, (u32, u32)> {
    let mut counts: HashMap<String, (u32, u32)> = HashMap::new();
    let mut current: Option<String> = None;
    let mut old_path: Option<String> = None;
    for line in diff.lines() {
        if let Some(rest) = line.strip_prefix("--- ") {
            old_path = Some(rest.trim_start_matches("a/").to_string());
            continue;
        }
        if let Some(rest) = line.strip_prefix("+++ ") {
            let path = if rest == "/dev/null" {
                old_path.clone().unwrap_or_default()
            } else {
                rest.trim_start_matches("b/").to_string()
            };
            counts.entry(path.clone()).or_default();
            current = Some(path);
            continue;
        }
        let Some(path) = current.as_ref() else {
            continue;
        };
        let entry = counts.entry(path.clone()).or_default();
        if line.starts_with('+') {
            entry.0 += 1;
        } else if line.starts_with('-') {
            entry.1 += 1;
        }
    }
    counts
}

fn text_summary(result: &Value) -> Option<String> {
    let text = result
        .get("content")
        .and_then(Value::as_array)?
        .iter()
        .find_map(|item| item.get("text").and_then(Value::as_str))?;
    let summary = truncate_chars(text, MAX_DETAIL_CHARS);
    (!summary.is_empty()).then_some(summary)
}

fn extract_error(response: &Value) -> Option<String> {
    if let Some(error) = response.get("error") {
        return Some(truncate_chars(
            error
                .get("message")
                .and_then(Value::as_str)
                .unwrap_or("rpc error"),
            MAX_DETAIL_CHARS,
        ));
    }
    let result = response.get("result")?;
    if let Some(error) = result
        .get("structuredContent")
        .and_then(|s| s.get("error"))
        .filter(|e| !e.is_null())
    {
        let code = error.get("code").and_then(Value::as_str).unwrap_or("");
        let message = error
            .get("message")
            .and_then(Value::as_str)
            .map(str::to_string)
            .unwrap_or_else(|| error.to_string());
        let joined = if code.is_empty() {
            message
        } else {
            format!("{code}: {message}")
        };
        return Some(truncate_chars(&joined, MAX_DETAIL_CHARS));
    }
    text_summary(result)
}

fn describe_result(tool: &str, structured: &Value) -> Option<String> {
    match tool {
        "read_file" => {
            let lines = structured.get("total_lines").and_then(Value::as_u64);
            let bytes = structured.get("bytes_read").and_then(Value::as_u64);
            match (lines, bytes) {
                (Some(lines), Some(bytes)) => Some(format!("{lines} lines · {bytes} B")),
                (None, Some(bytes)) => Some(format!("{bytes} B")),
                _ => None,
            }
        }
        "exec_command" | "wait_command" => {
            let exit = structured.get("exit_code").filter(|v| !v.is_null());
            let status = structured.get("status").and_then(Value::as_str);
            match (status, exit) {
                (Some(status), Some(exit)) => Some(format!("{status} · exit {exit}")),
                (Some(status), None) => Some(status.to_string()),
                (None, Some(exit)) => Some(format!("exit {exit}")),
                _ => None,
            }
        }
        _ => None,
    }
}

fn extract_changes(structured: &Value, diff: Option<&str>) -> Vec<ActivityChange> {
    let counts = diff.map(diff_line_counts).unwrap_or_default();
    let mut changes = Vec::new();
    if let Some(items) = structured.get("affected_files").and_then(Value::as_array) {
        for item in items {
            let Some(path) = item.get("path").and_then(Value::as_str) else {
                continue;
            };
            let (added, removed) = counts.get(path).copied().unwrap_or((0, 0));
            changes.push(ActivityChange {
                path: path.to_string(),
                operation: item
                    .get("operation")
                    .and_then(Value::as_str)
                    .unwrap_or("update")
                    .to_string(),
                bytes_before: item.get("bytes_before").and_then(Value::as_u64),
                bytes_after: item.get("bytes_after").and_then(Value::as_u64),
                added,
                removed,
            });
        }
    }
    if changes.is_empty() {
        // format_files and friends only report a list of changed paths.
        if let Some(items) = structured.get("files_changed").and_then(Value::as_array) {
            for path in items.iter().filter_map(Value::as_str) {
                let (added, removed) = counts.get(path).copied().unwrap_or((0, 0));
                changes.push(ActivityChange {
                    path: path.to_string(),
                    operation: "update".into(),
                    bytes_before: None,
                    bytes_after: None,
                    added,
                    removed,
                });
            }
        }
    }
    changes
}

fn extract_plan(structured: &Value, now: u64) -> Option<ActivityPlan> {
    let task = structured.get("task")?;
    let objective = task.get("objective").and_then(Value::as_str)?;
    let strings = |key: &str| {
        task.get(key)
            .and_then(Value::as_array)
            .map(|items| {
                items
                    .iter()
                    .filter_map(Value::as_str)
                    .map(|s| truncate_chars(s, MAX_TITLE_CHARS))
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default()
    };
    Some(ActivityPlan {
        task_id: task
            .get("id")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string(),
        objective: truncate_chars(objective, 400),
        status: task
            .get("status")
            .and_then(|s| s.as_str().map(str::to_string))
            .unwrap_or_default(),
        completed_steps: strings("completed_steps"),
        pending_steps: strings("pending_steps"),
        updated_ms: now,
    })
}

fn percentile_95(durations: &VecDeque<u64>) -> u64 {
    if durations.is_empty() {
        return 0;
    }
    let mut sorted: Vec<u64> = durations.iter().copied().collect();
    sorted.sort_unstable();
    let index = ((sorted.len() as f64) * 0.95).ceil() as usize;
    sorted[index.saturating_sub(1).min(sorted.len() - 1)]
}

/// Records the start of a tool call and returns its sequence number.
pub(crate) fn begin(profile_id: &str, tool: &str, args: &Value, request_bytes: usize) -> u64 {
    let now = now_ms();
    let mut paths = Vec::new();
    collect_arg_paths(args, 0, &mut paths);
    let title = describe_request(tool, args, &paths);
    let kind = tool_kind(tool);
    let Ok(mut guard) = feeds().lock() else {
        return 0;
    };
    let feed = guard.entry(profile_id.to_string()).or_default();
    feed.seq += 1;
    feed.rev += 1;
    let seq = feed.seq;
    feed.stats.running += 1;
    if feed.stats.first_ms == 0 {
        feed.stats.first_ms = now;
    }
    feed.stats.last_ms = now;
    feed.events.push_back(ActivityEvent {
        seq,
        rev: feed.rev,
        started_ms: now,
        duration_ms: None,
        tool: tool.to_string(),
        kind,
        status: "running",
        title,
        paths,
        detail: None,
        error: None,
        changes: Vec::new(),
        dry_run: args
            .get("dry_run")
            .and_then(Value::as_bool)
            .unwrap_or(false),
        has_diff: false,
        diff: None,
        diff_truncated: false,
        request_bytes: request_bytes as u64,
        response_bytes: 0,
    });
    while feed.events.len() > MAX_EVENTS {
        if let Some(dropped) = feed.events.pop_front() {
            if dropped.status == "running" {
                feed.stats.running = feed.stats.running.saturating_sub(1);
            }
            feed.floor_rev = feed.floor_rev.max(dropped.rev);
        }
    }
    seq
}

/// Completes a previously started tool call.
pub(crate) fn finish(
    profile_id: &str,
    seq: u64,
    outcome: &str,
    duration_ms: u128,
    response: Option<&Value>,
    worker_error: Option<&str>,
) {
    let now = now_ms();
    let duration_ms = duration_ms.min(u64::MAX as u128) as u64;
    let success = outcome == "success";
    let response_bytes = response
        .and_then(|value| serde_json::to_vec(value).ok())
        .map(|bytes| bytes.len() as u64)
        .unwrap_or(0);
    let result = response.and_then(|value| value.get("result"));
    static NULL: Value = Value::Null;
    let structured = result
        .and_then(|value| value.get("structuredContent"))
        .unwrap_or(&NULL);
    let raw_diff = structured
        .get("diff")
        .and_then(Value::as_str)
        .filter(|diff| !diff.is_empty());
    let changes = extract_changes(structured, raw_diff);
    let (diff, diff_truncated) = match raw_diff {
        Some(diff) => {
            let (text, truncated) = truncate_bytes(diff, MAX_DIFF_BYTES);
            (Some(text), truncated)
        }
        None => (None, false),
    };
    let error = if success {
        None
    } else {
        worker_error
            .map(|e| truncate_chars(e, MAX_DETAIL_CHARS))
            .or_else(|| response.and_then(extract_error))
    };
    let plan = extract_plan(structured, now);

    let Ok(mut guard) = feeds().lock() else {
        return;
    };
    let Some(feed) = guard.get_mut(profile_id) else {
        return;
    };
    feed.rev += 1;
    let rev = feed.rev;
    let Some(event) = feed.events.iter_mut().find(|event| event.seq == seq) else {
        return;
    };
    if event.status != "running" {
        return;
    }
    let tool = event.tool.clone();
    let kind = event.kind;
    event.rev = rev;
    event.status = if success { "success" } else { "error" };
    event.duration_ms = Some(duration_ms);
    event.response_bytes = response_bytes;
    event.error = error;
    event.detail = describe_result(&tool, structured).or_else(|| result.and_then(text_summary));
    if structured.get("dry_run").and_then(Value::as_bool) == Some(true) {
        event.dry_run = true;
    }
    for change in &changes {
        push_path(&mut event.paths, &change.path);
    }
    event.changes = changes;
    event.has_diff = diff.is_some();
    event.diff = diff;
    event.diff_truncated = diff_truncated;
    let dry_run = event.dry_run;
    let paths = event.paths.clone();
    let changes = event.changes.clone();
    let request_bytes = event.request_bytes;

    let stats = &mut feed.stats;
    stats.running = stats.running.saturating_sub(1);
    stats.total += 1;
    if success {
        stats.success += 1;
    } else {
        stats.errors += 1;
    }
    stats.total_duration_ms = stats.total_duration_ms.saturating_add(duration_ms);
    stats.avg_duration_ms = stats.total_duration_ms / stats.total.max(1);
    stats.bytes_out = stats.bytes_out.saturating_add(response_bytes);
    stats.last_ms = now;
    match kind {
        "read" => stats.reads += 1,
        "edit" => stats.edits += 1,
        "exec" => stats.execs += 1,
        "search" => stats.searches += 1,
        _ => {}
    }
    stats.bytes_in = stats.bytes_in.saturating_add(request_bytes);
    if success && kind == "read" {
        for path in &paths {
            feed.viewed.insert(path.clone());
        }
        stats.files_viewed = feed.viewed.len() as u64;
    }
    if success && !dry_run {
        for change in &changes {
            feed.changed.insert(change.path.clone());
            stats.lines_added += u64::from(change.added);
            stats.lines_removed += u64::from(change.removed);
            if let (Some(before), Some(after)) = (change.bytes_before, change.bytes_after) {
                if after >= before {
                    stats.bytes_added += after - before;
                } else {
                    stats.bytes_removed += before - after;
                }
            }
        }
        stats.files_changed = feed.changed.len() as u64;
    }
    feed.durations.push_back(duration_ms);
    while feed.durations.len() > MAX_EVENTS {
        feed.durations.pop_front();
    }
    feed.stats.p95_duration_ms = percentile_95(&feed.durations);
    if success {
        if let Some(plan) = plan {
            feed.plan = Some(plan);
        }
    }
}

/// Returns events whose revision is newer than `since_rev` (diffs stripped).
pub fn snapshot(profile_id: &str, since_rev: u64) -> ActivitySnapshot {
    let Ok(guard) = feeds().lock() else {
        return empty_snapshot();
    };
    let Some(feed) = guard.get(profile_id) else {
        return empty_snapshot();
    };
    let reset = since_rev > feed.rev || (since_rev > 0 && since_rev < feed.floor_rev);
    let since = if reset { 0 } else { since_rev };
    let events = feed
        .events
        .iter()
        .filter(|event| event.rev > since)
        .map(|event| {
            let mut event = event.clone();
            event.diff = None;
            event
        })
        .collect();
    ActivitySnapshot {
        events,
        rev: feed.rev,
        stats: feed.stats.clone(),
        plan: feed.plan.clone(),
        reset: reset || since_rev == 0,
    }
}

/// Returns a single event including its (possibly truncated) diff.
pub fn detail(profile_id: &str, seq: u64) -> Option<ActivityEvent> {
    let guard = feeds().lock().ok()?;
    guard
        .get(profile_id)?
        .events
        .iter()
        .find(|event| event.seq == seq)
        .cloned()
}

/// Clears the feed for a profile (used by the "clear" button).
pub fn clear(profile_id: &str) {
    if let Ok(mut guard) = feeds().lock() {
        if let Some(feed) = guard.get_mut(profile_id) {
            let rev = feed.rev + 1;
            *feed = ProfileFeed {
                seq: feed.seq,
                rev,
                floor_rev: rev,
                ..ProfileFeed::default()
            };
        }
    }
}

fn empty_snapshot() -> ActivitySnapshot {
    ActivitySnapshot {
        events: Vec::new(),
        rev: 0,
        stats: ActivityStats::default(),
        plan: None,
        reset: true,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn records_edit_with_diff_and_sizes() {
        let profile = "activity-test-edit";
        let seq = begin(
            profile,
            "edit",
            &json!({"files": [{"path": "src/a.rs", "edits": []}]}),
            42,
        );
        let snap = snapshot(profile, 0);
        assert_eq!(snap.stats.running, 1);
        assert_eq!(snap.events[0].status, "running");
        assert_eq!(snap.events[0].paths, vec!["src/a.rs".to_string()]);
        let response = json!({"result": {"structuredContent": {
            "ok": true,
            "diff": "--- a/src/a.rs\n+++ b/src/a.rs\n@@ -1 +1,2 @@\n-old\n+new\n+more\n",
            "affected_files": [{"path": "src/a.rs", "operation": "update", "bytes_before": 4, "bytes_after": 9}]
        }, "content": [{"type": "text", "text": "Edited 1 file"}]}});
        finish(profile, seq, "success", 12, Some(&response), None);
        let snap = snapshot(profile, snap.rev);
        assert!(!snap.reset);
        assert_eq!(snap.events.len(), 1);
        let event = &snap.events[0];
        assert_eq!(event.status, "success");
        assert!(event.has_diff);
        assert!(event.diff.is_none(), "timeline omits diff bodies");
        assert_eq!(event.changes[0].added, 2);
        assert_eq!(event.changes[0].removed, 1);
        assert_eq!(event.changes[0].bytes_after, Some(9));
        assert_eq!(snap.stats.files_changed, 1);
        assert_eq!(snap.stats.bytes_added, 5);
        let full = detail(profile, seq).expect("detail");
        assert!(full.diff.unwrap().contains("+more"));
    }

    #[test]
    fn tracks_errors_plan_and_patch_paths() {
        let profile = "activity-test-plan";
        let seq = begin(
            profile,
            "apply_patch",
            &json!({"patch": "*** Begin Patch\n*** Update File: x/y.ts\n*** End Patch"}),
            10,
        );
        finish(
            profile,
            seq,
            "tool_error",
            5,
            Some(
                &json!({"result": {"isError": true, "structuredContent": {"ok": false, "error": {"code": "PATCH_FAILED", "message": "context mismatch"}}}}),
            ),
            None,
        );
        let seq = begin(profile, "update_task", &json!({"task_id": "t1"}), 10);
        finish(
            profile,
            seq,
            "success",
            3,
            Some(&json!({"result": {"structuredContent": {"task": {
                "id": "t1", "objective": "Ship it", "status": "active",
                "completed_steps": ["a"], "pending_steps": ["b", "c"]
            }}}})),
            None,
        );
        let snap = snapshot(profile, 0);
        assert_eq!(snap.events[0].paths, vec!["x/y.ts".to_string()]);
        assert_eq!(
            snap.events[0].error.as_deref(),
            Some("PATCH_FAILED: context mismatch")
        );
        assert_eq!(snap.stats.errors, 1);
        assert_eq!(snap.stats.success, 1);
        let plan = snap.plan.expect("plan");
        assert_eq!(plan.objective, "Ship it");
        assert_eq!(plan.pending_steps.len(), 2);
        clear(profile);
        assert!(snapshot(profile, 0).events.is_empty());
    }

    #[test]
    fn kinds_cover_common_tools() {
        assert_eq!(tool_kind("read_file"), "read");
        assert_eq!(tool_kind("apply_patch"), "edit");
        assert_eq!(tool_kind("git_status"), "git");
        assert_eq!(tool_kind("exec_command"), "exec");
        assert_eq!(tool_kind("desktop_click"), "desktop");
    }
}
