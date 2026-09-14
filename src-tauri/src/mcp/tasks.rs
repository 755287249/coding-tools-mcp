use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde_json::{json, Value};

use crate::tools::{wrap_mcp_tool_result, SharedToolContext};

pub const TASKS_EXTENSION: &str = "io.modelcontextprotocol/tasks";
pub const TASK_POLL_INTERVAL_MS: u64 = 1_000;
pub const TASK_TTL_MS: u64 = 60 * 60_000;

#[derive(Clone)]
pub struct ProcessTask {
    pub task_id: String,
    pub session_id: String,
    pub tool_name: String,
    pub arguments: Value,
    pub conversation_key: String,
    pub context: SharedToolContext,
    pub created_at_ms: u64,
    pub last_updated_at_ms: u64,
    pub expires_at_ms: u64,
    pub status: &'static str,
    pub status_message: Option<String>,
    pub cancel_requested: bool,
    pub final_result: Option<Value>,
}

static TASKS: OnceLock<Mutex<HashMap<String, ProcessTask>>> = OnceLock::new();

fn tasks() -> &'static Mutex<HashMap<String, ProcessTask>> {
    TASKS.get_or_init(|| Mutex::new(HashMap::new()))
}

fn lock_tasks() -> std::sync::MutexGuard<'static, HashMap<String, ProcessTask>> {
    tasks()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or(Duration::ZERO)
        .as_millis()
        .min(u128::from(u64::MAX)) as u64
}

fn civil_from_days(days: i64) -> (i64, u32, u32) {
    let z = days + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
    let mut year = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = mp + if mp < 10 { 3 } else { -9 };
    if month <= 2 {
        year += 1;
    }
    (year, month as u32, day as u32)
}

fn iso_timestamp(ms: u64) -> String {
    let seconds = (ms / 1_000) as i64;
    let millis = ms % 1_000;
    let days = seconds.div_euclid(86_400);
    let seconds_of_day = seconds.rem_euclid(86_400);
    let (year, month, day) = civil_from_days(days);
    let hour = seconds_of_day / 3_600;
    let minute = (seconds_of_day % 3_600) / 60;
    let second = seconds_of_day % 60;
    format!("{year:04}-{month:02}-{day:02}T{hour:02}:{minute:02}:{second:02}.{millis:03}Z")
}

fn object(value: Option<&Value>) -> Option<&serde_json::Map<String, Value>> {
    value.and_then(Value::as_object)
}

pub fn client_supports_tasks(params: &Value) -> bool {
    object(params.get("_meta"))
        .and_then(|meta| object(meta.get("io.modelcontextprotocol/clientCapabilities")))
        .and_then(|capabilities| object(capabilities.get("extensions")))
        .and_then(|extensions| object(extensions.get(TASKS_EXTENSION)))
        .is_some()
}

pub fn conversation_key(params: &Value) -> String {
    params
        .get("_meta")
        .and_then(|meta| meta.get("openai/session"))
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .unwrap_or("__anonymous_mcp__")
        .to_string()
}

fn cleanup_locked(records: &mut HashMap<String, ProcessTask>) {
    let now = now_ms();
    records.retain(|_, task| task.expires_at_ms > now);
}

fn task_base(task: &ProcessTask) -> Value {
    json!({
        "taskId": task.task_id,
        "status": task.status,
        "statusMessage": task.status_message,
        "createdAt": iso_timestamp(task.created_at_ms),
        "lastUpdatedAt": iso_timestamp(task.last_updated_at_ms),
        "ttlMs": TASK_TTL_MS,
        "pollIntervalMs": TASK_POLL_INTERVAL_MS
    })
}

pub fn create_process_task(
    context: SharedToolContext,
    conversation_key: String,
    tool_name: &str,
    arguments: &Value,
    structured: &Value,
) -> Option<Value> {
    if tool_name != "exec_command" {
        return None;
    }
    let session_id = structured.get("session_id")?.as_str()?.trim();
    let pending = structured
        .get("process_still_running")
        .and_then(Value::as_bool)
        == Some(true)
        || structured
            .get("post_checks_pending")
            .and_then(Value::as_bool)
            == Some(true)
        || structured.get("command_ok").is_some_and(Value::is_null);
    if session_id.is_empty() || !pending {
        return None;
    }
    let now = now_ms();
    let created_at_ms = structured
        .get("started_ts_ms")
        .and_then(Value::as_u64)
        .filter(|value| *value > 0)
        .unwrap_or(now);
    let task_id = format!("exec:{session_id}");
    let record = ProcessTask {
        task_id: task_id.clone(),
        session_id: session_id.to_string(),
        tool_name: tool_name.to_string(),
        arguments: arguments.clone(),
        conversation_key,
        context,
        created_at_ms,
        last_updated_at_ms: now,
        expires_at_ms: created_at_ms.saturating_add(TASK_TTL_MS),
        status: "working",
        status_message: Some(
            if structured
                .get("post_checks_pending")
                .and_then(Value::as_bool)
                == Some(true)
            {
                "Process exited; verification is still pending.".into()
            } else {
                "Process is still running.".into()
            },
        ),
        cancel_requested: false,
        final_result: None,
    };
    let mut result = {
        let mut records = lock_tasks();
        cleanup_locked(&mut records);
        records.insert(task_id, record.clone());
        task_base(&record)
    };
    result["resultType"] = Value::String("task".into());
    Some(result)
}

pub fn require_task(conversation_key: &str, task_id: &str) -> Result<ProcessTask, Value> {
    let task_id = task_id.trim();
    if task_id.is_empty() {
        return Err(task_error(
            "taskId is required",
            "TASK_NOT_FOUND",
            "task_not_found",
            None,
        ));
    }
    let mut records = lock_tasks();
    cleanup_locked(&mut records);
    match records.get(task_id) {
        Some(task) if task.conversation_key == conversation_key => Ok(task.clone()),
        _ => Err(task_error(
            &format!("Task not found: {task_id}"),
            "TASK_NOT_FOUND",
            "task_not_found",
            None,
        )),
    }
}

pub fn detailed_task(task: &ProcessTask) -> Value {
    let mut result = task_base(task);
    result["resultType"] = Value::String("complete".into());
    if task.status == "completed" {
        if let Some(final_result) = &task.final_result {
            result["result"] = final_result.clone();
        }
    }
    result
}

pub fn update_from_snapshot(task_id: &str, structured: &Value) -> Result<Value, Value> {
    let mut records = lock_tasks();
    let task = records.get_mut(task_id).ok_or_else(|| {
        task_error(
            "Task state is unavailable",
            "TASK_STATE_UNAVAILABLE",
            "task_state_unavailable",
            None,
        )
    })?;
    task.last_updated_at_ms = now_ms();
    let working = structured
        .get("process_still_running")
        .and_then(Value::as_bool)
        == Some(true)
        || structured
            .get("post_checks_pending")
            .and_then(Value::as_bool)
            == Some(true)
        || structured.get("command_ok").is_some_and(Value::is_null);
    if working {
        task.status = "working";
        task.status_message = Some(if task.cancel_requested {
            "Cancellation requested; process termination is still pending.".into()
        } else if structured
            .get("post_checks_pending")
            .and_then(Value::as_bool)
            == Some(true)
        {
            "Process exited; verification is still pending.".into()
        } else {
            "Process is still running.".into()
        });
        return Ok(detailed_task(task));
    }

    let reason = structured
        .get("termination_reason")
        .and_then(Value::as_str)
        .unwrap_or_default();
    if task.cancel_requested || matches!(reason, "killed" | "graph_cancelled" | "detached_timeout")
    {
        task.status = "cancelled";
        task.status_message = Some("Task was cancelled.".into());
        task.final_result = None;
        return Ok(detailed_task(task));
    }

    task.status = "completed";
    task.status_message = None;
    let mut final_result =
        wrap_mcp_tool_result(&task.tool_name, &task.arguments, structured.clone());
    final_result["resultType"] = Value::String("complete".into());
    task.final_result = Some(final_result);
    Ok(detailed_task(task))
}

pub fn mark_cancelled(task_id: &str) -> Result<(), Value> {
    let mut records = lock_tasks();
    let task = records.get_mut(task_id).ok_or_else(|| {
        task_error(
            "Task state is unavailable",
            "TASK_STATE_UNAVAILABLE",
            "task_state_unavailable",
            None,
        )
    })?;
    task.cancel_requested = true;
    task.status = "cancelled";
    task.status_message = Some("Task was cancelled.".into());
    task.final_result = None;
    task.last_updated_at_ms = now_ms();
    Ok(())
}

pub fn task_error(message: &str, code: &str, reason: &str, task: Option<&ProcessTask>) -> Value {
    json!({
        "code": -32602,
        "message": message,
        "data": {
            "reason": reason,
            "error_code": code,
            "retryable": false,
            "taskId": task.map(|value| value.task_id.as_str()),
            "status": task.map(|value| value.status)
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn task_timestamps_are_rfc3339_utc() {
        assert_eq!(iso_timestamp(0), "1970-01-01T00:00:00.000Z");
    }
}
