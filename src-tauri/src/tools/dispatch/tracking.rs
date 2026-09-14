use serde_json::{json, Map, Value};

use crate::harness::model::OperationRecord;
use crate::tools::context::ToolContext;
use crate::tools::tool_runtime::{descriptor as tool_runtime, requires_write_baseline};
use crate::tools::workspace::tool_err_code;

const OPERATION_RESULT_BOOLEAN_FIELDS: &[&str] = &[
    "transport_ok",
    "execution_ok",
    "command_ok",
    "verification_ok",
    "process_timed_out",
    "request_timed_out",
    "recoverable",
    "truncated",
    "stdout_truncated",
    "stderr_truncated",
    "cursor_expired",
    "post_checks_pending",
    "detached",
    "deduplicated",
    "timeout_clamped",
    "polling_extends_process_deadline",
];

const OPERATION_RESULT_TOKEN_FIELDS: &[&str] = &[
    "status",
    "termination_reason",
    "execution_lane",
    "outcome_class",
    "execution_mode",
    "timeout_scope",
];

const OPERATION_RESULT_INTEGER_FIELDS: &[&str] = &[
    "exit_code",
    "process_exit_code",
    "elapsed_ms",
    "actual_wait_ms",
    "first_output_ms",
    "stdout_bytes",
    "stderr_bytes",
    "blocking_queue_wait_ms",
    "workspace_admission_wait_ms",
    "global_admission_wait_ms",
    "admission_queue_wait_ms",
    "workspace_lock_wait_ms",
    "operation_lock_wait_ms",
    "resource_lock_wait_ms",
    "history_lock_wait_ms",
    "session_registry_wait_ms",
    "requested_process_timeout_ms",
    "effective_process_timeout_ms",
    "process_timeout_limit_ms",
    "process_deadline_ts_ms",
    "process_timeout_remaining_ms",
];

fn operation_summary_token(value: Option<&Value>) -> Option<&str> {
    value.and_then(Value::as_str).filter(|text| {
        text.len() <= 128
            && text.bytes().all(|byte| {
                byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b':' | b'-')
            })
    })
}

pub(crate) fn operation_result_summary(name: &str, output: &Value) -> Value {
    let mut summary = Map::new();
    summary.insert(
        "ok".into(),
        Value::Bool(output.get("ok").and_then(Value::as_bool) == Some(true)),
    );
    summary.insert("tool".into(), Value::String(name.to_string()));
    summary.insert(
        "affected_files".into(),
        output.get("affected_files").cloned().unwrap_or(Value::Null),
    );
    for field in OPERATION_RESULT_BOOLEAN_FIELDS {
        if let Some(value) = output.get(*field).and_then(Value::as_bool) {
            summary.insert((*field).into(), Value::Bool(value));
        }
    }
    for field in OPERATION_RESULT_TOKEN_FIELDS {
        if let Some(value) = operation_summary_token(output.get(*field)) {
            summary.insert((*field).into(), Value::String(value.to_string()));
        }
    }
    for field in OPERATION_RESULT_INTEGER_FIELDS {
        if let Some(value) = output
            .get(*field)
            .filter(|value| value.is_i64() || value.is_u64())
        {
            summary.insert((*field).into(), value.clone());
        }
    }
    let error = output.get("error").and_then(Value::as_object);
    if let Some(value) = operation_summary_token(
        error
            .and_then(|object| object.get("code"))
            .or_else(|| output.get("error_code")),
    ) {
        summary.insert("error_code".into(), Value::String(value.to_string()));
    }
    if let Some(value) = operation_summary_token(
        error
            .and_then(|object| object.get("category"))
            .or_else(|| output.get("error_category")),
    ) {
        summary.insert("error_category".into(), Value::String(value.to_string()));
    }
    if let Some(value) = error
        .and_then(|object| object.get("retryable"))
        .or_else(|| output.get("retryable"))
        .and_then(Value::as_bool)
    {
        summary.insert("retryable".into(), Value::Bool(value));
    }
    if let Some(count) = output
        .get("warnings")
        .and_then(Value::as_array)
        .map(Vec::len)
    {
        summary.insert("warning_count".into(), json!(count));
    }
    Value::Object(summary)
}

pub(super) struct TrackedCall {
    task_id: Option<String>,
    operation: Option<OperationRecord>,
}

pub(super) fn begin_tracked_call(
    ctx: &ToolContext,
    name: &str,
    args: &Value,
    effective_args: &Value,
) -> Result<TrackedCall, Value> {
    let task_id = if ctx
        .runtime_config()
        .policy
        .security_policy
        .enforce_harness_baseline
        && requires_write_baseline(name, effective_args)
    {
        let scope_root = ctx.harness.scope_root_for(&ctx.default_cwd_path());
        let task = ctx
            .harness
            .current_task_for_root(&scope_root)
            .ok()
            .flatten();
        if let Some(task) = task {
            if let Err(error) = ctx.harness.check_baseline(&task.id) {
                return Err(attach_harness_status(
                    ctx,
                    tool_err_code(error.code(), error.to_string(), "permission"),
                    false,
                ));
            }
            let _ = ctx.harness.record_event(
                &task.id,
                "operation_started",
                Some(name),
                operation_input(args),
                json!({"ok": true, "tracking": "task"}),
            );
            Some(task.id)
        } else {
            None
        }
    } else {
        None
    };

    let operation = if tool_runtime(name).log_operation {
        ctx.harness
            .record_operation(
                None,
                task_id.as_deref(),
                name,
                "started",
                json!({"arguments_present": !args.is_null()}),
                json!({"ok": true}),
            )
            .ok()
    } else {
        None
    };

    Ok(TrackedCall { task_id, operation })
}

pub(super) fn finish_tracked_call(
    ctx: &ToolContext,
    name: &str,
    args: &Value,
    tracking: TrackedCall,
    mut output: Value,
) -> Value {
    if tracking.task_id.is_none()
        && tool_runtime(name).standalone_operation
        && output.get("ok") == Some(&Value::Bool(true))
    {
        attach_standalone_metadata(
            &mut output,
            "当前操作已在 standalone 模式完成；如需继续，直接调用下一个开发工具。",
        );
    }
    if let Some(operation) = tracking.operation.as_ref() {
        if let Some(object) = output.as_object_mut() {
            let field = if object.contains_key("operation_id") {
                "harness_operation_id"
            } else {
                "operation_id"
            };
            object.insert(field.into(), Value::String(operation.id.clone()));
        }
    }
    if output.get("ok").and_then(Value::as_bool) == Some(false) {
        output = attach_harness_status(ctx, output, tracking.task_id.is_none());
    }
    let deferred_process_operation = tracking.operation.as_ref().is_some_and(|operation| {
        if output.get("command_ok") != Some(&Value::Null) {
            return false;
        }
        let Some(session_id) = output.get("session_id").and_then(Value::as_str) else {
            return false;
        };
        let Ok(session) = ctx.sessions.get(session_id) else {
            return false;
        };
        let input_summary = operation_input(args);
        let mut deferred = operation.clone();
        deferred.reason = input_summary
            .get("reason")
            .and_then(Value::as_str)
            .map(str::to_string);
        deferred.input_summary = input_summary;
        session.attach_harness_operation(ctx.harness.clone(), deferred);
        true
    });
    if let Some(task_id) = tracking.task_id.as_deref() {
        let succeeded = output.get("ok").and_then(Value::as_bool) == Some(true);
        let _ = ctx.harness.record_event(
            task_id,
            "operation_finished",
            Some(name),
            operation_input(args),
            json!({"ok": succeeded, "tool": name}),
        );
        if succeeded {
            let _ = ctx.harness.refresh_expected_state(task_id);
        }
    }
    if let Some(operation) = tracking.operation.filter(|_| !deferred_process_operation) {
        let succeeded = output.get("ok").and_then(Value::as_bool) == Some(true);
        let _ = ctx.harness.record_operation(
            Some(&operation.id),
            tracking.task_id.as_deref(),
            name,
            if succeeded { "completed" } else { "failed" },
            operation_input(args),
            operation_result_summary(name, &output),
        );
    }
    output
}

fn operation_input(args: &Value) -> Value {
    json!({
        "arguments_present": !args.is_null(),
        "reason": args.get("reason")
    })
}

pub(super) fn attach_harness_status(
    ctx: &ToolContext,
    mut output: Value,
    standalone: bool,
) -> Value {
    let scope_root = ctx.harness.scope_root_for(&ctx.default_cwd_path());
    if let Ok(mut status) = ctx.harness.status_for(&scope_root) {
        if standalone && status.task_id.is_none() {
            status.next_actions.clear();
        }
        status.next_actions = filter_exposed_actions(ctx, status.next_actions);
        if let Some(object) = output.as_object_mut() {
            object.insert(
                "harness".into(),
                serde_json::to_value(status).unwrap_or_else(|_| {
                    json!({
                        "status": "unavailable",
                        "reason": "无法序列化 Harness 状态"
                    })
                }),
            );
            if standalone {
                attach_standalone_metadata(
                    &mut output,
                    "命令未成功；请检查 stderr、exit_code 或调整参数后重试。",
                );
            }
        }
    }
    output
}

fn attach_standalone_metadata(output: &mut Value, recovery_hint: &str) {
    if let Some(object) = output.as_object_mut() {
        object.insert("harness_mode".into(), Value::String("standalone".into()));
        object.insert("task_required".into(), Value::Bool(false));
        object.entry("next_actions").or_insert_with(|| json!([]));
        object.insert(
            "recovery_hint".into(),
            Value::String(recovery_hint.to_string()),
        );
    }
}

fn filter_exposed_actions(ctx: &ToolContext, actions: Vec<String>) -> Vec<String> {
    let runtime = ctx.runtime_config();
    let exposed = crate::tools::registry::exposed_tool_names(&runtime.tool_profile);
    actions
        .into_iter()
        .filter(|action| exposed.contains(&action.as_str()))
        .collect()
}
