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

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ActivityTodo {
    pub id: String,
    pub title: String,
    /// `pending`, `in_progress` or `completed`.
    pub status: String,
}

#[derive(Debug, Clone, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ActivityProgress {
    pub message: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub phase: String,
    /// Agent estimate (0–100). Never derived automatically.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub percent: Option<u8>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub todo_id: Option<String>,
    pub updated_ms: u64,
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
    /// `task` (harness task tools) or `agent` (set_todos / update_plan /
    /// report_progress).
    pub source: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub external_task_id: Option<String>,
    /// Ordered checklist; harness plans are converted into the same shape.
    pub todos: Vec<ActivityTodo>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub progress: Option<ActivityProgress>,
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
        | "history_session_validate"
        | "set_todos"
        | "update_plan"
        | "report_progress" => "plan",
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
        "set_todos" | "update_plan" => {
            let count = args
                .get(if tool == "set_todos" { "todos" } else { "plan" })
                .and_then(Value::as_array)
                .map(Vec::len)
                .unwrap_or(0);
            match str_arg(args, "goal").filter(|goal| !goal.trim().is_empty()) {
                Some(goal) => format!("{goal} · {count}"),
                None => format!("{count}"),
            }
        }
        "report_progress" => match str_arg(args, "phase").filter(|p| !p.trim().is_empty()) {
            Some(phase) => format!("{phase} · {}", str_arg(args, "message").unwrap_or("")),
            None => str_arg(args, "message").unwrap_or("").to_string(),
        },
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
    let status = task
        .get("status")
        .and_then(|s| s.as_str().map(str::to_string))
        .unwrap_or_default();
    let completed_steps = strings("completed_steps");
    let pending_steps = strings("pending_steps");
    let finished = status == "completed";
    let mut todos: Vec<ActivityTodo> = completed_steps
        .iter()
        .enumerate()
        .map(|(index, title)| ActivityTodo {
            id: format!("done-{}", index + 1),
            title: title.clone(),
            status: "completed".to_string(),
        })
        .collect();
    todos.extend(pending_steps.iter().enumerate().map(|(index, title)| {
        ActivityTodo {
            id: format!("step-{}", index + 1),
            title: title.clone(),
            status: if index == 0 && !finished {
                "in_progress"
            } else {
                "pending"
            }
            .to_string(),
        }
    }));
    Some(ActivityPlan {
        task_id: task
            .get("id")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string(),
        objective: truncate_chars(objective, 400),
        status,
        completed_steps,
        pending_steps,
        updated_ms: now,
        source: "task".to_string(),
        external_task_id: None,
        todos,
        progress: None,
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
        if let Some(mut plan) = plan {
            // Keep the latest agent progress line when a harness task updates.
            plan.progress = feed.plan.as_ref().and_then(|old| old.progress.clone());
            feed.plan = Some(plan);
        }
    }
}

// ───────────── Agent-reported plan (set_todos / update_plan / report_progress) ─────────────

pub(crate) const MAX_TODOS: usize = 24;
pub(crate) const MAX_TODO_ID_CHARS: usize = 80;
pub(crate) const MAX_TODO_TITLE_CHARS: usize = 400;
pub(crate) const MAX_GOAL_CHARS: usize = 400;
pub(crate) const MAX_PROGRESS_CHARS: usize = 2000;
pub(crate) const MAX_PHASE_CHARS: usize = 160;
pub(crate) const MAX_EXTERNAL_ID_CHARS: usize = 100;

pub(crate) const TODO_STATUSES: [&str; 3] = ["pending", "in_progress", "completed"];

/// Checks the checklist invariants shared by `set_todos` and `update_plan`.
pub(crate) fn validate_todos(todos: &[ActivityTodo]) -> Result<(), String> {
    if todos.len() > MAX_TODOS {
        return Err(format!("todos must contain at most {MAX_TODOS} items"));
    }
    let mut ids = HashSet::new();
    let mut running = 0;
    for todo in todos {
        if todo.id.trim().is_empty() || todo.id.chars().count() > MAX_TODO_ID_CHARS {
            return Err(format!("todo id must be 1-{MAX_TODO_ID_CHARS} characters"));
        }
        if todo.title.trim().is_empty() || todo.title.chars().count() > MAX_TODO_TITLE_CHARS {
            return Err(format!(
                "todo title must be 1-{MAX_TODO_TITLE_CHARS} characters"
            ));
        }
        if !ids.insert(todo.id.as_str()) {
            return Err(format!("Duplicate todo id: {}", todo.id));
        }
        if !TODO_STATUSES.contains(&todo.status.as_str()) {
            return Err(format!(
                "Invalid todo status {:?}; use pending, in_progress or completed",
                todo.status
            ));
        }
        if todo.status == "in_progress" {
            running += 1;
        }
    }
    if running > 1 {
        return Err("At most one todo may be in_progress".to_string());
    }
    Ok(())
}

fn agent_plan(
    goal: String,
    external_task_id: Option<String>,
    todos: Vec<ActivityTodo>,
    progress: Option<ActivityProgress>,
    now: u64,
) -> ActivityPlan {
    let completed_steps: Vec<String> = todos
        .iter()
        .filter(|todo| todo.status == "completed")
        .map(|todo| todo.title.clone())
        .collect();
    let pending_steps: Vec<String> = todos
        .iter()
        .filter(|todo| todo.status != "completed")
        .map(|todo| todo.title.clone())
        .collect();
    let status = if !todos.is_empty() && pending_steps.is_empty() {
        "completed"
    } else if todos.iter().any(|todo| todo.status == "in_progress") {
        "in_progress"
    } else {
        "pending"
    };
    ActivityPlan {
        task_id: external_task_id.clone().unwrap_or_default(),
        objective: goal,
        status: status.to_string(),
        completed_steps,
        pending_steps,
        updated_ms: now,
        source: "agent".to_string(),
        external_task_id,
        todos,
        progress,
    }
}

/// Replaces the agent checklist. An empty `todos` clears the plan. Progress is
/// reset because it referred to the previous checklist.
pub(crate) fn set_agent_todos(
    profile_id: &str,
    goal: Option<String>,
    external_task_id: Option<String>,
    todos: Vec<ActivityTodo>,
) -> Result<Option<ActivityPlan>, String> {
    validate_todos(&todos)?;
    let now = now_ms();
    let mut guard = feeds()
        .lock()
        .map_err(|_| "activity feed unavailable".to_string())?;
    let feed = guard.entry(profile_id.to_string()).or_default();
    feed.rev += 1;
    if todos.is_empty() {
        feed.plan = None;
        return Ok(None);
    }
    let previous = feed.plan.as_ref();
    let goal = goal.unwrap_or_else(|| previous.map(|p| p.objective.clone()).unwrap_or_default());
    let external_task_id =
        external_task_id.or_else(|| previous.and_then(|p| p.external_task_id.clone()));
    let plan = agent_plan(goal, external_task_id, todos, None, now);
    feed.plan = Some(plan.clone());
    Ok(Some(plan))
}

/// Codex-style `update_plan`: steps are matched to existing todos by title so
/// ids stay stable; new steps get fresh ids. `explanation` becomes the latest
/// progress line.
pub(crate) fn update_agent_plan(
    profile_id: &str,
    goal: Option<String>,
    steps: Vec<(String, String)>,
    explanation: Option<String>,
) -> Result<Option<ActivityPlan>, String> {
    let now = now_ms();
    let mut guard = feeds()
        .lock()
        .map_err(|_| "activity feed unavailable".to_string())?;
    let feed = guard.entry(profile_id.to_string()).or_default();
    let mut remaining: Vec<ActivityTodo> = feed
        .plan
        .as_ref()
        .map(|plan| plan.todos.clone())
        .unwrap_or_default();
    let mut used: HashSet<String> = HashSet::new();
    let mut next_id = 1usize;
    let todos: Vec<ActivityTodo> = steps
        .into_iter()
        .map(|(title, status)| {
            let id = match remaining.iter().position(|todo| todo.title == title) {
                Some(index) => remaining.remove(index).id,
                None => loop {
                    let candidate = format!("todo-{next_id}");
                    next_id += 1;
                    let taken = remaining.iter().any(|todo| todo.id == candidate)
                        || used.contains(&candidate);
                    if !taken {
                        break candidate;
                    }
                },
            };
            used.insert(id.clone());
            ActivityTodo { id, title, status }
        })
        .collect();
    validate_todos(&todos)?;
    feed.rev += 1;
    if todos.is_empty() {
        feed.plan = None;
        return Ok(None);
    }
    let previous = feed.plan.as_ref();
    let goal = goal.unwrap_or_else(|| previous.map(|p| p.objective.clone()).unwrap_or_default());
    let external_task_id = previous.and_then(|p| p.external_task_id.clone());
    let progress = explanation
        .filter(|text| !text.trim().is_empty())
        .map(|message| ActivityProgress {
            message,
            phase: "计划更新".to_string(),
            percent: None,
            todo_id: None,
            updated_ms: now,
        });
    let plan = agent_plan(goal, external_task_id, todos, progress, now);
    feed.plan = Some(plan.clone());
    Ok(Some(plan))
}

/// Records a free-form progress report. `todo_id` defaults to the step that
/// is in progress; an unknown id is rejected.
pub(crate) fn report_agent_progress(
    profile_id: &str,
    message: String,
    phase: String,
    percent: Option<u8>,
    todo_id: Option<String>,
) -> Result<ActivityProgress, String> {
    let now = now_ms();
    let mut guard = feeds()
        .lock()
        .map_err(|_| "activity feed unavailable".to_string())?;
    let feed = guard.entry(profile_id.to_string()).or_default();
    let todos = feed
        .plan
        .as_ref()
        .map(|plan| plan.todos.as_slice())
        .unwrap_or(&[]);
    let todo_id = match todo_id {
        Some(id) => {
            if !todos.iter().any(|todo| todo.id == id) {
                return Err(format!(
                    "Unknown todo_id: {id}. Call set_todos first or omit todo_id."
                ));
            }
            Some(id)
        }
        None => todos
            .iter()
            .find(|todo| todo.status == "in_progress")
            .map(|todo| todo.id.clone()),
    };
    let progress = ActivityProgress {
        message,
        phase,
        percent,
        todo_id,
        updated_ms: now,
    };
    feed.rev += 1;
    match feed.plan.as_mut() {
        Some(plan) => {
            plan.progress = Some(progress.clone());
            plan.updated_ms = now;
        }
        None => {
            feed.plan = Some(agent_plan(
                String::new(),
                None,
                Vec::new(),
                Some(progress.clone()),
                now,
            ));
        }
    }
    Ok(progress)
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
        assert_eq!(tool_kind("set_todos"), "plan");
        assert_eq!(tool_kind("report_progress"), "plan");
    }

    fn todo(id: &str, title: &str, status: &str) -> ActivityTodo {
        ActivityTodo {
            id: id.to_string(),
            title: title.to_string(),
            status: status.to_string(),
        }
    }

    #[test]
    fn agent_plan_tracks_todos_progress_and_ids() {
        let profile = "activity-test-agent-plan";
        clear(profile);

        // Invariants are enforced.
        let two_running = vec![todo("a", "A", "in_progress"), todo("b", "B", "in_progress")];
        assert!(set_agent_todos(profile, None, None, two_running).is_err());
        let duplicate = vec![todo("a", "A", "pending"), todo("a", "B", "pending")];
        assert!(set_agent_todos(profile, None, None, duplicate).is_err());
        assert!(set_agent_todos(profile, None, None, vec![todo("a", "A", "done")]).is_err());

        let plan = set_agent_todos(
            profile,
            Some("Ship the panel".into()),
            None,
            vec![
                todo("read", "Read code", "completed"),
                todo("build", "Build UI", "in_progress"),
                todo("test", "Test", "pending"),
            ],
        )
        .expect("valid")
        .expect("plan");
        assert_eq!(plan.source, "agent");
        assert_eq!(plan.status, "in_progress");
        assert_eq!(plan.completed_steps, vec!["Read code".to_string()]);
        assert_eq!(plan.pending_steps.len(), 2);

        // Progress defaults to the in-progress step; unknown ids are rejected.
        let progress =
            report_agent_progress(profile, "Styling cards".into(), "UI".into(), Some(40), None)
                .expect("progress");
        assert_eq!(progress.todo_id.as_deref(), Some("build"));
        assert!(report_agent_progress(
            profile,
            "x".into(),
            String::new(),
            None,
            Some("nope".into())
        )
        .is_err());
        let snap = snapshot(profile, 0);
        let plan = snap.plan.expect("plan");
        assert_eq!(plan.progress.expect("progress").percent, Some(40));

        // update_plan keeps ids for matching titles, keeps the goal and records
        // the explanation as progress.
        let plan = update_agent_plan(
            profile,
            None,
            vec![
                ("Read code".into(), "completed".into()),
                ("Build UI".into(), "completed".into()),
                ("Write docs".into(), "in_progress".into()),
            ],
            Some("UI done".into()),
        )
        .expect("valid")
        .expect("plan");
        assert_eq!(plan.objective, "Ship the panel");
        assert_eq!(plan.todos[0].id, "read");
        assert_eq!(plan.todos[1].id, "build");
        assert_eq!(plan.todos[2].id, "todo-1");
        let progress = plan.progress.expect("explanation progress");
        assert_eq!(progress.message, "UI done");
        assert_eq!(progress.phase, "计划更新");

        // Completing everything marks the plan completed; empty clears it.
        let plan = update_agent_plan(
            profile,
            None,
            vec![("Read code".into(), "completed".into())],
            None,
        )
        .expect("valid")
        .expect("plan");
        assert_eq!(plan.status, "completed");
        assert!(set_agent_todos(profile, None, None, Vec::new())
            .expect("clear")
            .is_none());
        assert!(snapshot(profile, 0).plan.is_none());
    }

    #[test]
    fn harness_plan_converts_steps_to_todos() {
        let plan = extract_plan(
            &json!({"task": {"id": "t1", "objective": "Obj", "status": "active",
                "completed_steps": ["one"], "pending_steps": ["two", "three"]}}),
            1,
        )
        .expect("plan");
        let statuses: Vec<_> = plan.todos.iter().map(|t| t.status.as_str()).collect();
        assert_eq!(statuses, vec!["completed", "in_progress", "pending"]);
        assert_eq!(plan.source, "task");
    }
}
