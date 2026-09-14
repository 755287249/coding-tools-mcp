use std::collections::HashSet;

use serde_json::{json, Value};

use crate::knowledge::{enqueue_task_outcome, KnowledgeTaskOutcome};
use crate::tools::workspace::{tool_ok, WorkspaceError};
use crate::tools::ToolContext;

use super::model::{ChangeSet, HarnessEvent, TaskSession, TaskStatus};
use super::store::HarnessError;

pub const TOOL_NAMES: &[&str] = &[
    "harness_status",
    "operation_log",
    "project_state",
    "start_task",
    "update_task",
    "pause_task",
    "resume_task",
    "fail_task",
    "close_failed_task",
    "rollback_task",
    "finish_task",
    "task_context",
    "list_task_events",
    "change_summary",
];

pub fn call(ctx: &ToolContext, name: &str, args: &Value) -> Result<Value, WorkspaceError> {
    let value = match name {
        "harness_status" => harness_status(ctx),
        "operation_log" => operation_log(ctx, args),
        "project_state" => project_state(ctx, args),
        "start_task" => start_task(ctx, args),
        "update_task" => update_task(ctx, args),
        "pause_task" => transition(ctx, args, TaskStatus::Paused),
        "resume_task" => transition(ctx, args, TaskStatus::Active),
        "fail_task" => transition(ctx, args, TaskStatus::Failed),
        "close_failed_task" => close_failed_task(ctx, args),
        "rollback_task" => rollback_task(ctx, args),
        "finish_task" => finish_task(ctx, args),
        "task_context" => task_context(ctx, args),
        "list_task_events" => list_task_events(ctx, args),
        "change_summary" => change_summary(ctx, args),
        _ => return Err(tool_error("INVALID_ARGUMENT", "未知 Harness 工具")),
    }?;
    Ok(tool_ok(value))
}

fn harness_status(ctx: &ToolContext) -> Result<Value, WorkspaceError> {
    let scope_root = ctx.harness.scope_root_for(&ctx.default_cwd_path());
    serde_json::to_value(ctx.harness.status_for(&scope_root).map_err(map_error)?)
        .map_err(|e| tool_error("SERIALIZE_FAILED", e.to_string()))
}

fn operation_log(ctx: &ToolContext, args: &Value) -> Result<Value, WorkspaceError> {
    let offset = args.get("cursor").and_then(Value::as_u64).unwrap_or(0) as usize;
    let limit = args
        .get("limit")
        .and_then(Value::as_u64)
        .unwrap_or(50)
        .clamp(1, 200) as usize;
    let order = args.get("order").and_then(Value::as_str).unwrap_or("desc");
    if !matches!(order, "asc" | "desc") {
        return Err(tool_error("INVALID_ARGUMENT", "order must be asc or desc"));
    }
    let tool_filter = args.get("tool").and_then(Value::as_str);
    let kind_filter = args.get("kind").and_then(Value::as_str);
    let errors_only = args
        .get("errors_only")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let since_ts_ms = args.get("since_ts_ms").and_then(Value::as_u64);

    let mut operations = ctx
        .harness
        .list_operations(0, usize::MAX)
        .map_err(map_error)?;
    operations.retain(|operation| {
        tool_filter.map_or(true, |tool| operation.tool == tool)
            && kind_filter.map_or(true, |kind| operation.kind == kind)
            && since_ts_ms.map_or(true, |since| {
                operation
                    .created_at
                    .parse::<u64>()
                    .is_ok_and(|created_at| created_at >= since)
            })
            && (!errors_only
                || operation.kind == "failed"
                || operation.result_summary.get("ok").and_then(Value::as_bool) == Some(false))
    });
    if order == "desc" {
        operations.reverse();
    }

    let total_matching = operations.len();
    let page = operations
        .into_iter()
        .skip(offset)
        .take(limit)
        .collect::<Vec<_>>();
    let next_offset = offset.saturating_add(page.len());
    let has_more = next_offset < total_matching;

    Ok(json!({
        "operations": page,
        "order": order,
        "filters": {
            "tool": tool_filter,
            "kind": kind_filter,
            "errors_only": errors_only,
            "since_ts_ms": since_ts_ms
        },
        "total_matching": total_matching,
        "has_more": has_more,
        "next_cursor": has_more.then_some(next_offset)
    }))
}

fn project_state(ctx: &ToolContext, args: &Value) -> Result<Value, WorkspaceError> {
    let max_files = args.get("max_files").and_then(Value::as_u64).unwrap_or(200) as usize;
    let scope_root = ctx.harness.scope_root_for(&ctx.default_cwd_path());
    serde_json::to_value(
        ctx.harness
            .project_state_for(&scope_root, max_files)
            .map_err(map_error)?,
    )
    .map_err(|e| tool_error("SERIALIZE_FAILED", e.to_string()))
}

fn finalize_verifying_task(
    ctx: &ToolContext,
    task: &TaskSession,
) -> Result<(TaskSession, ChangeSet), WorkspaceError> {
    if task.status != TaskStatus::Verifying {
        return Err(tool_error(
            "INVALID_TASK_TRANSITION",
            "Only a verifying task can finalize an existing captured change.",
        ));
    }
    let change_id = task.latest_change_id.as_deref().ok_or_else(|| {
        tool_error(
            "CHANGE_NOT_FOUND",
            "Verifying task has no captured change to finalize.",
        )
    })?;
    let change = ctx.harness.change(change_id).map_err(map_error)?;
    if change.task_id != task.id {
        return Err(tool_error(
            "CHANGE_TASK_MISMATCH",
            "Captured change does not belong to the verifying task.",
        ));
    }
    let completed = ctx
        .harness
        .transition(&task.id, TaskStatus::Completed)
        .map_err(map_error)?;
    enqueue_task_outcome(
        ctx,
        &completed.id,
        KnowledgeTaskOutcome::VerifiedSuccess,
        completed.updated_at.parse::<u64>().unwrap_or(0),
    );
    Ok((completed, change))
}

fn start_task(ctx: &ToolContext, args: &Value) -> Result<Value, WorkspaceError> {
    let objective = args
        .get("objective")
        .and_then(Value::as_str)
        .ok_or_else(|| tool_error("INVALID_ARGUMENT", "objective 是必填项"))?;
    let existing_task = args
        .get("existing_task")
        .and_then(Value::as_str)
        .unwrap_or("error");
    if !matches!(existing_task, "error" | "finish_if_complete") {
        return Err(tool_error(
            "INVALID_ARGUMENT",
            "existing_task must be error or finish_if_complete",
        ));
    }
    let scope_root = ctx.harness.scope_root_for(&ctx.default_cwd_path());
    let mut recovered_task_id = None;
    if existing_task == "finish_if_complete" {
        if let Some(existing) = ctx
            .harness
            .current_task_for_root(&scope_root)
            .map_err(map_error)?
        {
            if existing.status == TaskStatus::Verifying
                && existing.pending_steps.is_empty()
                && existing.latest_change_id.is_some()
            {
                finalize_verifying_task(ctx, &existing)?;
                recovered_task_id = Some(existing.id);
            }
        }
    }
    let task = ctx
        .harness
        .start_task_for(objective, &scope_root)
        .map_err(map_error)?;
    Ok(json!({
        "task": task,
        "next": ["project_state", "task_context"],
        "recovered_task_id": recovered_task_id
    }))
}

fn update_task(ctx: &ToolContext, args: &Value) -> Result<Value, WorkspaceError> {
    let task_id = task_id(args)?;
    let completed_steps = string_list(args.get("completed_steps"))?;
    let pending_steps = string_list(args.get("pending_steps"))?;
    let task = ctx
        .harness
        .update_steps(task_id, completed_steps, pending_steps)
        .map_err(map_error)?;
    Ok(json!({"task": task}))
}

fn transition(
    ctx: &ToolContext,
    args: &Value,
    status: TaskStatus,
) -> Result<Value, WorkspaceError> {
    let task = ctx
        .harness
        .transition(task_id(args)?, status)
        .map_err(map_error)?;
    Ok(json!({"task": task}))
}

fn close_failed_task(ctx: &ToolContext, args: &Value) -> Result<Value, WorkspaceError> {
    let task = ctx
        .harness
        .transition(task_id(args)?, TaskStatus::FailedFinal)
        .map_err(map_error)?;
    enqueue_task_outcome(
        ctx,
        &task.id,
        KnowledgeTaskOutcome::Failure,
        task.updated_at.parse::<u64>().unwrap_or(0),
    );
    Ok(json!({"task": task}))
}

fn rollback_task(ctx: &ToolContext, args: &Value) -> Result<Value, WorkspaceError> {
    let task_id = task_id(args)?;
    let current = ctx.harness.task(task_id).map_err(map_error)?;
    if current.status != TaskStatus::Failed {
        return Err(tool_error(
            "INVALID_TASK_TRANSITION",
            format!("Cannot transition {:?} to rolled_back", current.status),
        ));
    }
    let (matches, baseline) = ctx
        .harness
        .original_baseline_matches(task_id)
        .map_err(map_error)?;
    if !matches {
        return Err(tool_error(
            "ROLLBACK_NOT_VERIFIED",
            format!(
                "Workspace does not match the task baseline; rollback cannot be finalized. expected_branch={:?} current_branch={:?} expected_head={:?} current_head={:?}",
                current.baseline.branch,
                baseline.branch,
                current.baseline.head,
                baseline.head
            ),
        ));
    }
    let task = ctx
        .harness
        .transition(task_id, TaskStatus::RolledBack)
        .map_err(map_error)?;
    enqueue_task_outcome(
        ctx,
        &task.id,
        KnowledgeTaskOutcome::RolledBack,
        task.updated_at.parse::<u64>().unwrap_or(0),
    );
    Ok(json!({"task": task, "baseline_verified": true}))
}

fn finish_task(ctx: &ToolContext, args: &Value) -> Result<Value, WorkspaceError> {
    let task_id = task_id(args)?;
    let allow_unverified = args
        .get("allow_unverified")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let current = ctx.harness.task(task_id).map_err(map_error)?;
    if current.status == TaskStatus::Verifying && !allow_unverified {
        let (task, change) = finalize_verifying_task(ctx, &current)?;
        let summary = stored_change_summary(ctx, &task, &change)?;
        return Ok(json!({
            "task": task,
            "summary": change.reason.text,
            "change_id": change.id,
            "change_summary": summary,
            "finalized_existing_change": true
        }));
    }
    let summary = match args.get("summary") {
        None => None,
        Some(value) => Some(
            value
                .as_str()
                .ok_or_else(|| tool_error("INVALID_ARGUMENT", "summary 必须是字符串"))?,
        ),
    };
    let status = if allow_unverified {
        TaskStatus::CompletedUnverified
    } else {
        TaskStatus::Verifying
    };
    let (task, change) = ctx
        .harness
        .finish_task(task_id, summary, status)
        .map_err(map_error)?;
    if task.status == TaskStatus::CompletedUnverified {
        enqueue_task_outcome(
            ctx,
            &task.id,
            KnowledgeTaskOutcome::UnverifiedSuccess,
            task.updated_at.parse::<u64>().unwrap_or(0),
        );
    }
    let summary = stored_change_summary(ctx, &task, &change)?;
    Ok(json!({
        "task": task,
        "summary": change.reason.text,
        "change_id": change.id,
        "change_summary": summary
    }))
}

const TASK_CONTEXT_MIN_BYTES: usize = 8_192;
const TASK_CONTEXT_MAX_BYTES: usize = 131_072;
const TASK_CONTEXT_DEFAULT_BYTES: usize = 32_768;

fn task_context_value(
    task: &TaskSession,
    events: &[HarnessEvent],
    truncated: bool,
    max_bytes: usize,
) -> Result<(Value, usize), WorkspaceError> {
    let value = json!({
        "task": task,
        "events": events,
        "truncated": truncated,
        "max_bytes": max_bytes
    });
    let serialized = serde_json::to_vec(&tool_ok(value.clone()))
        .map_err(|error| tool_error("SERIALIZE_FAILED", error.to_string()))?;
    Ok((value, serialized.len()))
}

fn bounded_task_context(
    mut task: TaskSession,
    mut events: Vec<HarnessEvent>,
    max_bytes: usize,
) -> Result<Value, WorkspaceError> {
    let mut truncated = events.len() > 100;
    events.truncate(100);
    let (value, serialized_len) = task_context_value(&task, &events, truncated, max_bytes)?;
    if serialized_len <= max_bytes {
        return Ok(value);
    }
    truncated = true;

    let baseline_entries = std::mem::take(&mut task.baseline.entries);
    if !baseline_entries.is_empty() {
        let mut lower = 0usize;
        let mut upper = baseline_entries.len();
        let mut best = None;
        while lower <= upper {
            let middle = lower + (upper - lower) / 2;
            task.baseline.entries = baseline_entries[..middle].to_vec();
            let (_, size) = task_context_value(&task, &events, truncated, max_bytes)?;
            if size <= max_bytes {
                best = Some(middle);
                lower = middle + 1;
            } else if middle == 0 {
                break;
            } else {
                upper = middle - 1;
            }
        }
        if let Some(keep) = best {
            task.baseline.entries = baseline_entries[..keep].to_vec();
            return task_context_value(&task, &events, truncated, max_bytes)
                .map(|(value, _)| value);
        }
        task.baseline.entries.clear();
    }

    let source_events = std::mem::take(&mut events);
    if !source_events.is_empty() {
        let mut lower = 0usize;
        let mut upper = source_events.len();
        let mut best = None;
        while lower <= upper {
            let middle = lower + (upper - lower) / 2;
            events = source_events[..middle].to_vec();
            let (_, size) = task_context_value(&task, &events, truncated, max_bytes)?;
            if size <= max_bytes {
                best = Some(middle);
                lower = middle + 1;
            } else if middle == 0 {
                break;
            } else {
                upper = middle - 1;
            }
        }
        if let Some(keep) = best {
            events = source_events[..keep].to_vec();
            return task_context_value(&task, &events, truncated, max_bytes)
                .map(|(value, _)| value);
        }
        events.clear();
    }

    let pending_steps = std::mem::take(&mut task.pending_steps);
    if !pending_steps.is_empty() {
        let mut lower = 0usize;
        let mut upper = pending_steps.len();
        let mut best = None;
        while lower <= upper {
            let middle = lower + (upper - lower) / 2;
            task.pending_steps = pending_steps[..middle].to_vec();
            let (_, size) = task_context_value(&task, &events, truncated, max_bytes)?;
            if size <= max_bytes {
                best = Some(middle);
                lower = middle + 1;
            } else if middle == 0 {
                break;
            } else {
                upper = middle - 1;
            }
        }
        if let Some(keep) = best {
            task.pending_steps = pending_steps[..keep].to_vec();
            return task_context_value(&task, &events, truncated, max_bytes)
                .map(|(value, _)| value);
        }
        task.pending_steps.clear();
    }

    let completed_steps = std::mem::take(&mut task.completed_steps);
    if !completed_steps.is_empty() {
        let mut lower = 0usize;
        let mut upper = completed_steps.len();
        let mut best = None;
        while lower <= upper {
            let middle = lower + (upper - lower) / 2;
            task.completed_steps = completed_steps[..middle].to_vec();
            let (_, size) = task_context_value(&task, &events, truncated, max_bytes)?;
            if size <= max_bytes {
                best = Some(middle);
                lower = middle + 1;
            } else if middle == 0 {
                break;
            } else {
                upper = middle - 1;
            }
        }
        if let Some(keep) = best {
            task.completed_steps = completed_steps[..keep].to_vec();
            return task_context_value(&task, &events, truncated, max_bytes)
                .map(|(value, _)| value);
        }
        task.completed_steps.clear();
    }

    loop {
        let (value, size) = task_context_value(&task, &events, truncated, max_bytes)?;
        if size <= max_bytes {
            return Ok(value);
        }
        let objective_len = task.objective.chars().count();
        if objective_len <= 1 {
            return Ok(value);
        }
        task.objective = task
            .objective
            .chars()
            .take((objective_len / 2).max(1))
            .collect();
    }
}

fn task_context(ctx: &ToolContext, args: &Value) -> Result<Value, WorkspaceError> {
    let max_bytes =
        args.get("max_bytes")
            .and_then(Value::as_u64)
            .unwrap_or(TASK_CONTEXT_DEFAULT_BYTES as u64)
            .clamp(TASK_CONTEXT_MIN_BYTES as u64, TASK_CONTEXT_MAX_BYTES as u64) as usize;
    let task = if let Some(task_id) = args.get("task_id").and_then(Value::as_str) {
        Some(ctx.harness.task(task_id).map_err(map_error)?)
    } else {
        let scope_root = ctx.harness.scope_root_for(&ctx.default_cwd_path());
        ctx.harness
            .current_task_for_root(&scope_root)
            .map_err(map_error)?
    };
    let Some(task) = task else {
        return Ok(json!({
            "task": null,
            "message": "当前没有活动任务",
            "truncated": false,
            "max_bytes": max_bytes
        }));
    };
    let events = ctx
        .harness
        .list_events(&task.id, 0, 101)
        .map_err(map_error)?;
    bounded_task_context(task, events, max_bytes)
}

fn list_task_events(ctx: &ToolContext, args: &Value) -> Result<Value, WorkspaceError> {
    let task_id = task_id(args)?;
    let offset = args.get("cursor").and_then(Value::as_u64).unwrap_or(0) as usize;
    let limit = args
        .get("limit")
        .and_then(Value::as_u64)
        .unwrap_or(50)
        .clamp(1, 200) as usize;
    let events = ctx
        .harness
        .list_events(task_id, offset, limit)
        .map_err(map_error)?;
    Ok(json!({"events": events, "next_cursor": offset + events.len()}))
}

fn change_summary(ctx: &ToolContext, args: &Value) -> Result<Value, WorkspaceError> {
    let requested_task_id = optional_nonempty_string(args, "task_id")?;
    let requested_change_id = optional_change_id(args)?;
    if let Some(change_id) = requested_change_id {
        let change = ctx.harness.change(change_id).map_err(map_error)?;
        if requested_task_id.is_some_and(|task_id| task_id != change.task_id) {
            return Err(tool_error(
                "CHANGE_TASK_MISMATCH",
                format!("变更集 {change_id} 不属于请求的任务"),
            ));
        }
        let task = ctx.harness.task(&change.task_id).map_err(map_error)?;
        return stored_change_summary(ctx, &task, &change);
    }
    let task = if let Some(task_id) = requested_task_id {
        ctx.harness.task(task_id).map_err(map_error)?
    } else {
        ctx.harness
            .current_task()
            .map_err(map_error)?
            .ok_or_else(|| tool_error("TASK_STATE_REQUIRED", "没有可总结的活动任务"))?
    };
    if let Some(change_id) = task.latest_change_id.as_deref() {
        let change = ctx.harness.change(change_id).map_err(map_error)?;
        return stored_change_summary(ctx, &task, &change);
    }
    let files = ctx.harness.change_files(&task.id).map_err(map_error)?;
    let events = ctx
        .harness
        .list_events(&task.id, 0, 100)
        .map_err(map_error)?;
    Ok(json!({
        "change_id": null,
        "task_id": task.id,
        "objective": task.objective,
        "why": {"text": task.objective, "source": "task_objective"},
        "files": files,
        "evidence": events,
        "verification": [],
        "risks": [],
        "rollback_capability": "not_available_in_foundation"
    }))
}

fn stored_change_summary(
    ctx: &ToolContext,
    task: &TaskSession,
    change: &super::model::ChangeSet,
) -> Result<Value, WorkspaceError> {
    let command_ids = change.command_ids.iter().collect::<HashSet<_>>();
    let events = ctx
        .harness
        .list_events(&task.id, 0, 2_000)
        .map_err(map_error)?
        .into_iter()
        .filter(|event| command_ids.contains(&event.operation_id))
        .take(100)
        .collect::<Vec<_>>();
    Ok(json!({
        "change_id": change.id,
        "task_id": task.id,
        "objective": task.objective,
        "why": change.reason,
        "files": change.files,
        "evidence": events,
        "verification": change.verification_ids,
        "risks": change.risks,
        "rollback_capability": "not_available_in_foundation"
    }))
}

fn optional_nonempty_string<'a>(
    args: &'a Value,
    field: &str,
) -> Result<Option<&'a str>, WorkspaceError> {
    let Some(value) = args.get(field) else {
        return Ok(None);
    };
    let value = value
        .as_str()
        .ok_or_else(|| tool_error("INVALID_ARGUMENT", format!("{field} 必须是字符串")))?;
    let value = value.trim();
    if value.is_empty() {
        return Err(tool_error("INVALID_ARGUMENT", format!("{field} 不能为空")));
    }
    Ok(Some(value))
}

fn optional_change_id(args: &Value) -> Result<Option<&str>, WorkspaceError> {
    let Some(change_id) = optional_nonempty_string(args, "change_id")? else {
        return Ok(None);
    };
    if change_id.len() != 32
        || !change_id
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    {
        return Err(tool_error(
            "INVALID_ARGUMENT",
            "change_id 必须是 32 位小写十六进制 ID",
        ));
    }
    Ok(Some(change_id))
}

fn task_id(args: &Value) -> Result<&str, WorkspaceError> {
    args.get("task_id")
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| tool_error("INVALID_ARGUMENT", "task_id 是必填项"))
}

fn string_list(value: Option<&Value>) -> Result<Option<Vec<String>>, WorkspaceError> {
    let Some(value) = value else { return Ok(None) };
    let list = value
        .as_array()
        .ok_or_else(|| tool_error("INVALID_ARGUMENT", "步骤必须是字符串数组"))?
        .iter()
        .map(|item| {
            item.as_str()
                .map(str::to_string)
                .ok_or_else(|| tool_error("INVALID_ARGUMENT", "步骤必须是字符串数组"))
        })
        .collect::<Result<Vec<_>, _>>()?;
    Ok(Some(list))
}

fn map_error(error: HarnessError) -> WorkspaceError {
    tool_error(error.code(), error.to_string())
}

fn tool_error(code: &'static str, message: impl Into<String>) -> WorkspaceError {
    WorkspaceError::Tool {
        code,
        message: message.into(),
        category: "permission",
        retryable: matches!(
            code,
            "TASK_ALREADY_ACTIVE"
                | "FILE_CHANGED_EXTERNALLY"
                | "BASELINE_STALE"
                | "ROLLBACK_NOT_VERIFIED"
        ),
    }
}

#[cfg(test)]
mod tests {
    use std::time::{Duration, Instant};

    use super::*;
    use crate::harness::model::{BaselineEntry, ProjectBaseline};
    use crate::tools::ToolContext;

    #[test]
    fn operation_log_defaults_to_recent_first_and_filters_failures() {
        let workspace = tempfile::tempdir().expect("workspace tempdir");
        let harness = tempfile::tempdir().expect("harness tempdir");
        let ctx =
            ToolContext::for_test(workspace.path().to_path_buf(), harness.path().to_path_buf())
                .expect("tool context");

        for (id, tool, kind, ok) in [
            ("one", "git_status", "completed", true),
            ("two", "exec_command", "failed", false),
            ("three", "git_commit", "completed", true),
        ] {
            ctx.harness
                .record_operation(
                    Some(id),
                    None,
                    tool,
                    kind,
                    json!({"reason": id}),
                    json!({"ok": ok}),
                )
                .expect("record operation");
        }

        let recent = operation_log(&ctx, &json!({"limit": 2})).expect("recent operation log");
        let recent = recent["operations"].as_array().expect("recent operations");
        assert_eq!(recent.len(), 2);
        assert_eq!(recent[0]["id"], "three");
        assert_eq!(recent[1]["id"], "two");

        let errors = operation_log(&ctx, &json!({"errors_only": true})).expect("error log");
        let errors = errors["operations"].as_array().expect("error operations");
        assert_eq!(errors.len(), 1);
        assert_eq!(errors[0]["id"], "two");
    }

    #[test]
    fn failed_task_is_recoverable_until_explicitly_closed() {
        let workspace = tempfile::tempdir().expect("workspace tempdir");
        let harness = tempfile::tempdir().expect("harness tempdir");
        std::fs::write(workspace.path().join("tracked.txt"), "initial\n").expect("tracked file");
        let ctx =
            ToolContext::for_test(workspace.path().to_path_buf(), harness.path().to_path_buf())
                .expect("tool context");
        let started =
            start_task(&ctx, &json!({"objective": "recoverable failure"})).expect("start task");
        let task_id = started["task"]["id"].as_str().expect("task id");

        let failed =
            transition(&ctx, &json!({"task_id": task_id}), TaskStatus::Failed).expect("fail task");
        assert_eq!(failed["task"]["status"], "failed");
        assert!(ctx.harness.current_task().expect("current task").is_some());

        let closed =
            close_failed_task(&ctx, &json!({"task_id": task_id})).expect("close failed task");
        assert_eq!(closed["task"]["status"], "failed_final");
        assert!(ctx.harness.current_task().expect("current task").is_none());
    }

    #[test]
    fn rollback_task_requires_original_baseline() {
        let workspace = tempfile::tempdir().expect("workspace tempdir");
        let harness = tempfile::tempdir().expect("harness tempdir");
        let tracked = workspace.path().join("tracked.txt");
        std::fs::write(&tracked, "initial\n").expect("tracked file");
        let ctx =
            ToolContext::for_test(workspace.path().to_path_buf(), harness.path().to_path_buf())
                .expect("tool context");
        let started =
            start_task(&ctx, &json!({"objective": "verified rollback"})).expect("start task");
        let task_id = started["task"]["id"].as_str().expect("task id");
        transition(&ctx, &json!({"task_id": task_id}), TaskStatus::Failed).expect("fail task");

        std::fs::write(&tracked, "changed\n").expect("change tracked file");
        let rejected = rollback_task(&ctx, &json!({"task_id": task_id}));
        assert!(rejected.is_err());
        assert_eq!(
            ctx.harness.task(task_id).expect("failed task").status,
            TaskStatus::Failed
        );

        std::fs::write(&tracked, "initial\n").expect("restore tracked file");
        let rolled_back = rollback_task(&ctx, &json!({"task_id": task_id})).expect("rollback task");
        assert_eq!(rolled_back["task"]["status"], "rolled_back");
        assert_eq!(rolled_back["baseline_verified"], true);
        assert!(ctx.harness.current_task().expect("current task").is_none());
    }

    #[test]
    fn task_context_trims_large_baseline_without_quadratic_work() {
        let entries = (0..10_000)
            .map(|index| BaselineEntry {
                path: format!("generated/{index:05}.txt"),
                exists: true,
                is_binary: false,
                sha256: "0".repeat(64),
                bytes: 1,
            })
            .collect::<Vec<_>>();
        let task = TaskSession {
            id: "task".into(),
            workspace_id: "workspace".into(),
            scope_id: None,
            scope_root: None,
            objective: "Bound a large baseline".into(),
            status: TaskStatus::Active,
            baseline: ProjectBaseline {
                branch: Some("main".into()),
                head: Some("0".repeat(40)),
                worktree_fingerprint: "0".repeat(64),
                entries,
                captured_at: "0".into(),
            },
            expected_fingerprint: "0".repeat(64),
            completed_steps: Vec::new(),
            pending_steps: Vec::new(),
            latest_change_id: None,
            latest_verification_id: None,
            created_at: "0".into(),
            updated_at: "0".into(),
        };

        let started = Instant::now();
        let value = bounded_task_context(task, Vec::new(), 8_192).expect("bounded context");
        let elapsed = started.elapsed();
        let serialized = serde_json::to_vec(&tool_ok(value.clone())).expect("serialize context");
        let retained = value["task"]["baseline"]["entries"]
            .as_array()
            .expect("baseline entries")
            .len();

        assert!(serialized.len() <= 8_192);
        assert!(retained < 10_000);
        assert!(
            elapsed < Duration::from_secs(5),
            "large task_context took {elapsed:?}"
        );
    }
}
