mod exec_many;
mod tracking;

use crate::knowledge::{
    knowledge_store_for_context, CanaryImpactStore, CanaryStrategyEngine, KnowledgeCanaryDecision,
    ToolStrategyImplementationId,
};
use std::borrow::Cow;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use crate::tools::context::{SharedToolContext, ToolContext};
use crate::tools::policy::{validate_tool_arguments_for_workspace, PolicyError};
use crate::tools::redaction::{redact_tool_output_with_policy, OutputRedactionContext};
use crate::tools::tool_runtime::{descriptor as tool_runtime, MutationLockGroup};
use crate::tools::workspace::{relative_display, tool_err, tool_err_code, tool_ok, WorkspaceError};
use crate::tools::{
    agent_plan, desktop, exec, file, file_action, git, history, image_tool, patch, project, session,
};
use serde_json::{json, Value};

use exec_many::{call_exec_many_async, call_exec_many_sync};
pub(crate) use tracking::operation_result_summary;
use tracking::{attach_harness_status, begin_tracked_call, finish_tracked_call};

#[cfg(test)]
use exec_many::{
    collect_parallelism_observations, command_parallel_signature, default_exec_many_parallelism,
    parallel_pair_key, parse_exec_batch_commands, resolve_exec_many_decision_with_history,
};

const ADMISSION_TIMEOUT: Duration = Duration::from_secs(30);

fn policy_tool_err(
    ctx: &ToolContext,
    tool_name: &str,
    arguments: &Value,
    err: PolicyError,
) -> Value {
    let raw = err.0.as_str();
    let dangerous = raw.strip_prefix("DANGEROUS_OPERATION_REQUIRES_CONFIRMATION: ");
    let protected = raw.strip_prefix("PROTECTED_REPOSITORY_ASSET: ");
    let workspace_path = raw.strip_prefix("WORKSPACE_PATH_PROTECTED: ");
    let external_execution = raw.strip_prefix("EXTERNAL_EXECUTION_NOT_ALLOWED: ");
    let code = if protected.is_some() {
        "PROTECTED_REPOSITORY_ASSET"
    } else if dangerous.is_some() {
        "DANGEROUS_OPERATION_REQUIRES_CONFIRMATION"
    } else if workspace_path.is_some() {
        "WORKSPACE_PATH_PROTECTED"
    } else if external_execution.is_some() {
        "EXTERNAL_EXECUTION_NOT_ALLOWED"
    } else if raw.contains("Network-looking commands are blocked") {
        "NETWORK_COMMAND_BLOCKED"
    } else if raw.contains("allowlisted") {
        "COMMAND_REJECTED"
    } else if raw == "workdir must stay inside the configured workspace" {
        "PATH_OUTSIDE_WORKSPACE"
    } else if raw.contains("Environment variable is protected") {
        "ENVIRONMENT_VARIABLE_PROTECTED"
    } else if raw.contains("Shell chaining, redirection and expansion") {
        "SHELL_MODE_REQUIRED"
    } else {
        "POLICY_REJECTED"
    };
    let policy_rule = match code {
        "NETWORK_COMMAND_BLOCKED" => Some("block_network_commands"),
        "WORKSPACE_PATH_PROTECTED"
        | "EXTERNAL_EXECUTION_NOT_ALLOWED"
        | "PATH_OUTSIDE_WORKSPACE" => Some("enforce_workspace_boundary"),
        "COMMAND_REJECTED" => Some("enforce_command_allowlist"),
        "DANGEROUS_OPERATION_REQUIRES_CONFIRMATION" => Some("require_dangerous_confirmation"),
        "PROTECTED_REPOSITORY_ASSET" => Some("protect_repository_metadata"),
        "ENVIRONMENT_VARIABLE_PROTECTED" => Some("protect_environment_variables"),
        _ => None,
    };
    let message = protected
        .or(dangerous)
        .or(workspace_path)
        .or(external_execution)
        .unwrap_or(raw)
        .to_string();
    let (reason, suggestion) = match code {
        "DANGEROUS_OPERATION_REQUIRES_CONFIRMATION" => (
            "confirmation_required",
            "Retry with confirm=true only after the destructive action has been explicitly approved.",
        ),
        "NETWORK_COMMAND_BLOCKED" => (
            "network_blocked",
            "Disable block_network_commands for this workspace if network access is intended, then retry.",
        ),
        "WORKSPACE_PATH_PROTECTED" | "EXTERNAL_EXECUTION_NOT_ALLOWED" | "PATH_OUTSIDE_WORKSPACE" => (
            "workspace_boundary",
            "Keep the operation inside the workspace, or disable enforce_workspace_boundary if host access is explicitly intended.",
        ),
        "COMMAND_REJECTED" => (
            "command_rejected",
            "Add the executable to the workspace command allowlist or disable enforce_command_allowlist.",
        ),
        "PROTECTED_REPOSITORY_ASSET" => (
            "protected_repository_asset",
            "Do not directly overwrite or recursively delete protected .git/.github metadata.",
        ),
        "ENVIRONMENT_VARIABLE_PROTECTED" => (
            "environment_protected",
            "Avoid overriding protected process/loader variables, or disable protect_environment_variables if explicitly intended.",
        ),
        "SHELL_MODE_REQUIRED" => (
            "shell_mode_required",
            "For &&, pipes, redirection or expansion set shell to \"cmd\" or \"powershell\" (Windows) or \"sh\"; otherwise run each command as a separate call.",
        ),
        _ => ("policy_rejected", "Correct the rejected arguments and retry."),
    };
    let permission = permission_kind(&message);
    let pending = permission.map(|permission| {
        ctx.pending_operations.insert(
            tool_name,
            arguments,
            permission,
            &message,
            Duration::from_secs(300),
        )
    });
    let recoverable = pending.is_some() || reason != "confirmation_required";
    let diagnostic_summary = policy_rule.map_or_else(
        || "Request was rejected by MCP policy before child-process startup.".to_string(),
        |rule| format!("Request was rejected by MCP policy before child-process startup ({rule})."),
    );
    tool_err(WorkspaceError::ToolDetails {
        code,
        message,
        category: "policy",
        retryable: false,
        details: json!({
            "stage": "policy",
            "reason": reason,
            "failure_origin": "policy_preflight",
            "execution_attempted": false,
            "process_started": false,
            "policy_blocked": true,
            "policy_rule": policy_rule,
            "diagnostic_summary": diagnostic_summary,
            "recoverable": recoverable,
            "suggestion": suggestion,
            "permission_request": pending.map(|operation| json!({
                "resume_id": operation.resume_id,
                "tool_name": operation.tool_name,
                "permission": operation.permission,
                "reason": operation.reason,
                "ttl_seconds": 300,
                "resume_with": "request_permissions"
            }))
        }),
    })
}

fn permission_kind(message: &str) -> Option<&'static str> {
    let lowered = message.to_ascii_lowercase();
    if lowered.contains("network") {
        Some("network")
    } else if lowered.contains("shell") {
        Some("shell_expansion")
    } else if lowered.contains("dangerous") || lowered.contains("confirmation") {
        Some("destructive_command")
    } else {
        None
    }
}

#[derive(Debug, Clone, Default)]
struct ToolCallExecutionContext {
    canary_sample_key: Option<String>,
}

/// **唯一工具执行入口**。MCP `tools/call` 与 Actions `POST /actions/{tool}` 必须且只能调用此函数。
/// 策略校验、分发、错误格式在此统一，两路传输层不得另做执行前校验（Actions 仅允许额外的暴露层 `validate_actions_exposure`）。
pub fn call_tool(ctx: &ToolContext, name: &str, args: &Value) -> Value {
    call_tool_with_execution_context(ctx, name, args, &ToolCallExecutionContext::default())
}

fn call_tool_with_execution_context(
    ctx: &ToolContext,
    name: &str,
    args: &Value,
    execution: &ToolCallExecutionContext,
) -> Value {
    let policy = ctx.runtime_config().policy.security_policy;
    redact_tool_output_with_policy(
        name,
        args,
        call_tool_inner(ctx, name, args, false, execution),
        &policy,
    )
}

pub async fn call_tool_async(ctx: SharedToolContext, name: String, args: Value) -> Value {
    call_tool_async_with_canary_sample_key(ctx, name, args, None).await
}

pub(crate) async fn call_tool_async_with_canary_sample_key(
    ctx: SharedToolContext,
    name: String,
    args: Value,
    canary_sample_key: Option<String>,
) -> Value {
    let policy = ctx.runtime_config().policy.security_policy;
    let redaction = OutputRedactionContext::new_with_policy(&name, &args, &policy);
    let lock_groups = mutation_lock_groups(ctx.as_ref(), &name, &args);
    let lock_started = Instant::now();
    let mut mutation_guards = Vec::with_capacity(lock_groups.len());
    for group in &lock_groups {
        mutation_guards.push(ctx.mutation_lock_for(*group).lock_owned().await);
    }
    let workspace_lock_wait_ms = lock_started.elapsed().as_millis();
    let execution = ToolCallExecutionContext { canary_sample_key };
    let mut output = call_tool_async_inner(ctx, name, args, execution).await;
    if let Some(object) = output.as_object_mut() {
        let lock_names = lock_groups
            .iter()
            .map(|group| group.as_str())
            .collect::<Vec<_>>();
        object.insert(
            "workspace_lock_scope".into(),
            json!(if lock_names.is_empty() {
                "none".to_string()
            } else {
                lock_names.join("+")
            }),
        );
        object.insert("workspace_lock_groups".into(), json!(lock_names));
        object.insert(
            "workspace_lock_wait_ms".into(),
            json!(workspace_lock_wait_ms),
        );
    }
    drop(mutation_guards);
    redaction.redact(output)
}

fn mutation_lock_groups(ctx: &ToolContext, name: &str, args: &Value) -> Vec<MutationLockGroup> {
    let effective_name = if name == "request_permissions" {
        args.get("resume_id")
            .and_then(Value::as_str)
            .and_then(|resume_id| ctx.pending_operations.tool_name(resume_id))
            .unwrap_or_else(|| name.to_string())
    } else {
        name.to_string()
    };
    tool_runtime(&effective_name).lock_groups.to_vec()
}

async fn call_tool_async_inner(
    ctx: SharedToolContext,
    name: String,
    args: Value,
    execution: ToolCallExecutionContext,
) -> Value {
    if matches!(name.as_str(), "chat_open" | "chat_wait" | "chat_reply" | "chat_close" | "chat_upload") {
        let runtime = ctx.runtime_config();
        if let Err(error) = validate_tool_arguments_for_workspace(&name, &args, &runtime.policy, Some(&ctx.workspace)) {
            return policy_tool_err(ctx.as_ref(), &name, &args, error);
        }
        let result = if name == "chat_wait" {
            super::chat::wait(ctx.workspace.root(), &args).await
        } else { super::chat::tool(ctx.workspace.root(), &name, &args) };
        return result.unwrap_or_else(|error| error.to_error_value());
    }
    if name == "exec_many" {
        return call_exec_many_async(ctx, &args).await;
    }
    if matches!(
        name.as_str(),
        "wait_command"
            | "resolve_operation"
            | "list_sessions"
            | "send_input"
            | "read_output"
            | "kill_session"
    ) {
        return call_session_tool_async(ctx.as_ref(), &name, &args).await;
    }

    let Some((
        admission_lane,
        admission_limit,
        admission,
        global_admission_limit,
        global_admission,
    )) = ctx.admission_for(&name)
    else {
        let mut value = call_tool_with_execution_context(ctx.as_ref(), &name, &args, &execution);
        if let Some(object) = value.as_object_mut() {
            object.insert("execution_lane".into(), json!("inline_fast"));
            object.insert("blocking_queue_wait_ms".into(), json!(0));
            object.insert("admission_lane".into(), json!("fast"));
            object.insert("admission_limit".into(), json!(0));
            object.insert("global_admission_limit".into(), json!(0));
            object.insert("workspace_admission_wait_ms".into(), json!(0));
            object.insert("global_admission_wait_ms".into(), json!(0));
            object.insert("admission_queue_wait_ms".into(), json!(0));
        }
        return value;
    };

    let admission_started = Instant::now();
    let workspace_started = Instant::now();
    let permit = match tokio::time::timeout(ADMISSION_TIMEOUT, admission.acquire_owned()).await {
        Ok(Ok(permit)) => permit,
        Ok(Err(error)) => {
            return admission_error(
                admission_lane,
                "workspace",
                admission_limit,
                workspace_started.elapsed().as_millis(),
                0,
                format!("Workspace tool admission lane closed: {error}"),
            )
        }
        Err(_) => {
            return admission_error(
                admission_lane,
                "workspace",
                admission_limit,
                workspace_started.elapsed().as_millis(),
                0,
                "Workspace tool admission queue exceeded 30 seconds".into(),
            )
        }
    };
    let workspace_admission_wait_ms = workspace_started.elapsed().as_millis();
    let remaining = ADMISSION_TIMEOUT.saturating_sub(admission_started.elapsed());
    let global_started = Instant::now();
    let global_permit =
        match tokio::time::timeout(remaining, global_admission.acquire_owned()).await {
            Ok(Ok(permit)) => permit,
            Ok(Err(error)) => {
                return admission_error(
                    admission_lane,
                    "global",
                    global_admission_limit,
                    workspace_admission_wait_ms,
                    global_started.elapsed().as_millis(),
                    format!("Global tool admission lane closed: {error}"),
                )
            }
            Err(_) => {
                return admission_error(
                    admission_lane,
                    "global",
                    global_admission_limit,
                    workspace_admission_wait_ms,
                    global_started.elapsed().as_millis(),
                    "Combined workspace/global admission queue exceeded 30 seconds".into(),
                )
            }
        };
    let global_admission_wait_ms = global_started.elapsed().as_millis();
    let admission_queue_wait_ms = admission_started.elapsed().as_millis();

    if name == "exec_command" {
        let _global_permit = global_permit;
        let _permit = permit;
        let mut value = call_exec_tool_async(ctx.as_ref(), &name, &args).await;
        if let Some(object) = value.as_object_mut() {
            object.insert("execution_lane".into(), json!("async_process"));
            object.insert("blocking_queue_wait_ms".into(), json!(0));
            attach_admission_metadata(
                object,
                admission_lane,
                admission_limit,
                global_admission_limit,
                workspace_admission_wait_ms,
                global_admission_wait_ms,
                admission_queue_wait_ms,
            );
        }
        return value;
    }

    if name == "request_permissions" {
        let _global_permit = global_permit;
        let _permit = permit;
        let mut value = call_permission_tool_async(ctx.clone(), &name, &args).await;
        if let Some(object) = value.as_object_mut() {
            object.insert("execution_lane".into(), json!("async_permission"));
            object.insert("blocking_queue_wait_ms".into(), json!(0));
            attach_admission_metadata(
                object,
                admission_lane,
                admission_limit,
                global_admission_limit,
                workspace_admission_wait_ms,
                global_admission_wait_ms,
                admission_queue_wait_ms,
            );
        }
        return value;
    }

    let queued_at = Instant::now();
    let result = tokio::task::spawn_blocking(move || {
        let _global_permit = global_permit;
        let _permit = permit;
        let queue_wait_ms = queued_at.elapsed().as_millis();
        let mut value = call_tool_with_execution_context(ctx.as_ref(), &name, &args, &execution);
        if let Some(object) = value.as_object_mut() {
            object.insert("execution_lane".into(), json!("blocking_worker"));
            object.insert("blocking_queue_wait_ms".into(), json!(queue_wait_ms));
            attach_admission_metadata(
                object,
                admission_lane,
                admission_limit,
                global_admission_limit,
                workspace_admission_wait_ms,
                global_admission_wait_ms,
                admission_queue_wait_ms,
            );
        }
        value
    })
    .await;
    match result {
        Ok(value) => value,
        Err(error) => {
            let mut value = tool_err(WorkspaceError::ToolDetails {
                code: "TOOL_WORKER_FAILED",
                message: format!("Tool worker failed: {error}"),
                category: "runtime",
                retryable: true,
                details: json!({
                    "stage": "tool_worker",
                    "reason": "join_failed",
                    "suggestion": "重试请求或重启 MCP 运行时"
                }),
            });
            if let Some(object) = value.as_object_mut() {
                object.insert("execution_lane".into(), json!("blocking_worker"));
                object.insert("blocking_queue_wait_ms".into(), json!(0));
                attach_admission_metadata(
                    object,
                    admission_lane,
                    admission_limit,
                    global_admission_limit,
                    workspace_admission_wait_ms,
                    global_admission_wait_ms,
                    admission_queue_wait_ms,
                );
            }
            value
        }
    }
}

fn attach_admission_metadata(
    object: &mut serde_json::Map<String, Value>,
    lane: &str,
    workspace_limit: usize,
    global_limit: usize,
    workspace_wait_ms: u128,
    global_wait_ms: u128,
    total_wait_ms: u128,
) {
    object.insert("admission_lane".into(), json!(lane));
    object.insert("admission_limit".into(), json!(workspace_limit));
    object.insert("global_admission_limit".into(), json!(global_limit));
    object.insert(
        "workspace_admission_wait_ms".into(),
        json!(workspace_wait_ms),
    );
    object.insert("global_admission_wait_ms".into(), json!(global_wait_ms));
    object.insert("admission_queue_wait_ms".into(), json!(total_wait_ms));
}

fn admission_error(
    lane: &str,
    scope: &str,
    limit: usize,
    workspace_wait_ms: u128,
    global_wait_ms: u128,
    message: String,
) -> Value {
    let queue_wait_ms = workspace_wait_ms.saturating_add(global_wait_ms);
    let mut value = tool_err(WorkspaceError::ToolDetails {
        code: "TOOL_BUSY",
        message,
        category: "runtime",
        retryable: true,
        details: json!({
            "stage": "admission",
            "reason": "concurrency_limit",
            "lane": lane,
            "scope": scope,
            "limit": limit,
            "timeout_ms": ADMISSION_TIMEOUT.as_millis(),
            "suggestion": "稍后重试，或等待当前长任务完成"
        }),
    });
    if let Some(object) = value.as_object_mut() {
        object.insert("execution_lane".into(), json!("admission_control"));
        object.insert("blocking_queue_wait_ms".into(), json!(0));
        object.insert("admission_lane".into(), json!(lane));
        object.insert("admission_limit".into(), json!(limit));
        object.insert("admission_scope".into(), json!(scope));
        object.insert(
            "workspace_admission_wait_ms".into(),
            json!(workspace_wait_ms),
        );
        object.insert("global_admission_wait_ms".into(), json!(global_wait_ms));
        object.insert("admission_queue_wait_ms".into(), json!(queue_wait_ms));
    }
    value
}

async fn call_exec_tool_async(ctx: &ToolContext, name: &str, args: &Value) -> Value {
    call_exec_tool_async_with_policy(ctx, name, args, false).await
}

async fn call_exec_tool_async_with_policy(
    ctx: &ToolContext,
    name: &str,
    args: &Value,
    permission_override: bool,
) -> Value {
    let effective_args = apply_default_cwd(ctx, name, args);
    let runtime = ctx.runtime_config();
    let mut override_policy;
    let policy = if permission_override {
        override_policy = runtime.policy.clone();
        override_policy.permission_mode = "dangerous".into();
        &override_policy
    } else {
        &runtime.policy
    };
    if let Err(error) =
        validate_tool_arguments_for_workspace(name, &effective_args, policy, Some(&ctx.workspace))
    {
        return policy_tool_err(ctx, name, &effective_args, error);
    }

    let tracking = match begin_tracked_call(ctx, name, args, &effective_args) {
        Ok(tracking) => tracking,
        Err(output) => return output,
    };
    let output = match exec::exec_command_async_with_runtime(ctx, &effective_args, runtime).await {
        Ok(value) => value,
        Err(error) => tool_err(error),
    };
    finish_tracked_call(ctx, name, args, tracking, output)
}

async fn call_permission_tool_async(ctx: SharedToolContext, name: &str, args: &Value) -> Value {
    let effective_args = apply_default_cwd(ctx.as_ref(), name, args);
    let runtime = ctx.runtime_config();
    if let Err(error) = validate_tool_arguments_for_workspace(
        name,
        &effective_args,
        &runtime.policy,
        Some(&ctx.workspace),
    ) {
        return policy_tool_err(ctx.as_ref(), name, &effective_args, error);
    }

    let tracking = match begin_tracked_call(ctx.as_ref(), name, args, &effective_args) {
        Ok(tracking) => tracking,
        Err(output) => return output,
    };
    let result = if effective_args.get("resume_id").is_some() {
        resume_pending_operation_async(ctx.clone(), &effective_args).await
    } else {
        request_permissions(ctx.as_ref(), &effective_args)
    };
    let output = match result {
        Ok(value) => value,
        Err(error) => tool_err(error),
    };
    finish_tracked_call(ctx.as_ref(), name, args, tracking, output)
}

async fn resume_pending_operation_async(
    ctx: SharedToolContext,
    args: &Value,
) -> Result<Value, WorkspaceError> {
    let resume_id = args
        .get("resume_id")
        .and_then(Value::as_str)
        .ok_or_else(|| WorkspaceError::invalid_argument("resume_id is required"))?;
    let operation = ctx.pending_operations.take(resume_id)?;
    let explicitly_approved = args
        .get("approve")
        .and_then(Value::as_bool)
        .unwrap_or(false)
        && args
            .get("confirm")
            .and_then(Value::as_bool)
            .unwrap_or(false);
    let runtime = ctx.runtime_config();
    let approved = explicitly_approved || runtime.policy.skip_permission_gates();
    if !approved {
        ctx.pending_operations.put_back(operation);
        return Err(WorkspaceError::ToolDetails {
            code: "PERMISSION_NOT_APPROVED",
            message: "The pending operation was not approved.".into(),
            category: "permission",
            retryable: true,
            details: json!({
                "resume_id": resume_id,
                "suggestion": "取得用户授权后以 approve=true、confirm=true 重试 request_permissions"
            }),
        });
    }

    let mut resumed_args = operation.arguments.clone();
    if matches!(
        operation.permission.as_str(),
        "destructive_command" | "shell_expansion" | "inline_script"
    ) {
        if let Some(object) = resumed_args.as_object_mut() {
            object.insert("confirm".into(), Value::Bool(true));
        }
    }

    let resumed_execution_lane;
    let mut resumed = if operation.tool_name == "exec_command" {
        resumed_execution_lane = "async_process";
        call_exec_tool_async_with_policy(ctx.as_ref(), &operation.tool_name, &resumed_args, true)
            .await
    } else {
        resumed_execution_lane = "blocking_worker";
        let retry_operation = operation.clone();
        let worker_ctx = ctx.clone();
        let tool_name = operation.tool_name.clone();
        let worker_args = resumed_args.clone();
        match tokio::task::spawn_blocking(move || {
            call_tool_inner(
                worker_ctx.as_ref(),
                &tool_name,
                &worker_args,
                true,
                &ToolCallExecutionContext::default(),
            )
        })
        .await
        {
            Ok(value) => value,
            Err(error) => {
                ctx.pending_operations.put_back(retry_operation);
                return Err(WorkspaceError::ToolDetails {
                    code: "TOOL_WORKER_FAILED",
                    message: format!("Permission resume worker failed: {error}"),
                    category: "runtime",
                    retryable: true,
                    details: json!({
                        "stage": "permission_resume",
                        "reason": "join_failed",
                        "resume_id": resume_id,
                        "suggestion": "使用同一 resume_id 重试 request_permissions"
                    }),
                });
            }
        }
    };

    if let Some(object) = resumed.as_object_mut() {
        object.insert("resumed".into(), Value::Bool(true));
        object.insert("resume_id".into(), Value::String(operation.resume_id));
        object.insert(
            "resumed_execution_lane".into(),
            Value::String(resumed_execution_lane.into()),
        );
        object.insert(
            "permission_grant".into(),
            json!({
                "status": "granted_and_resumed",
                "permission": operation.permission,
                "reason": operation.reason,
                "scope": args.get("scope").and_then(Value::as_str).unwrap_or("once")
            }),
        );
    }
    Ok(resumed)
}

async fn call_session_tool_async(ctx: &ToolContext, name: &str, args: &Value) -> Value {
    let effective_args = apply_default_cwd(ctx, name, args);
    let runtime = ctx.runtime_config();
    if let Err(error) = validate_tool_arguments_for_workspace(
        name,
        &effective_args,
        &runtime.policy,
        Some(&ctx.workspace),
    ) {
        return policy_tool_err(ctx, name, &effective_args, error);
    }

    let result = match name {
        "wait_command" => session::wait_command_async(&ctx.sessions, &effective_args).await,
        "resolve_operation" => {
            session::resolve_operation_async(&ctx.sessions, &effective_args).await
        }
        "list_sessions" => session::list_sessions(&ctx.sessions, &effective_args),
        "send_input" => session::send_input_async(&ctx.sessions, &effective_args).await,
        "read_output" => session::read_output_async(&ctx.sessions, &effective_args).await,
        "kill_session" => session::kill_session_async(&ctx.sessions, &effective_args).await,
        _ => unreachable!("non-session tool routed to async session dispatcher"),
    };
    let mut output = match result {
        Ok(value) => value,
        Err(error) => tool_err(error),
    };
    if let Some(object) = output.as_object_mut() {
        object.insert("execution_lane".into(), json!("async_control"));
        object.insert("blocking_queue_wait_ms".into(), json!(0));
        object.insert("admission_lane".into(), json!("async_control"));
        object.insert("admission_limit".into(), json!(0));
        object.insert("admission_queue_wait_ms".into(), json!(0));
    }
    if output.get("ok").and_then(Value::as_bool) == Some(false) {
        output = attach_harness_status(ctx, output, true);
    }
    output
}

fn call_tool_inner(
    ctx: &ToolContext,
    name: &str,
    args: &Value,
    permission_override: bool,
    execution: &ToolCallExecutionContext,
) -> Value {
    let effective_args = apply_default_cwd(ctx, name, args);
    let runtime = ctx.runtime_config();
    let mut override_policy;
    let policy = if permission_override {
        override_policy = runtime.policy.clone();
        override_policy.permission_mode = "dangerous".into();
        &override_policy
    } else {
        &runtime.policy
    };
    if let Err(e) =
        validate_tool_arguments_for_workspace(name, &effective_args, policy, Some(&ctx.workspace))
    {
        return policy_tool_err(ctx, name, &effective_args, e);
    }

    if crate::harness::tools::TOOL_NAMES.contains(&name) {
        return match crate::harness::tools::call(ctx, name, args) {
            Ok(value) => value,
            Err(error) => attach_harness_status(ctx, tool_err(error), false),
        };
    }

    if name == "edit" {
        let preflight_started = Instant::now();
        let mut preflight_args = effective_args.as_ref().clone();
        if let Some(object) = preflight_args.as_object_mut() {
            object.insert("dry_run".into(), Value::Bool(true));
        }
        let preflight = patch::edit(ctx, &preflight_args);
        let terminal = args.get("dry_run").and_then(Value::as_bool) == Some(true)
            || preflight
                .as_ref()
                .ok()
                .and_then(|value| value.get("status"))
                .and_then(Value::as_str)
                == Some("proposal_required")
            || preflight.is_err();
        if terminal {
            let mut output = match preflight {
                Ok(value) => value,
                Err(error) => tool_err(error),
            };
            if let Some(object) = output.as_object_mut() {
                object.insert(
                    "phase_durations_ms".into(),
                    json!({"preflight_ms": preflight_started.elapsed().as_millis()}),
                );
            }
            return output;
        }
    }

    let tracking = match begin_tracked_call(ctx, name, args, &effective_args) {
        Ok(tracking) => tracking,
        Err(output) => return output,
    };

    let ws = &ctx.workspace;
    let result = match name {
        "history_session_bootstrap" => history::bootstrap(ctx, &effective_args),
        "history_session_checkpoint" => history::checkpoint(ctx, &effective_args),
        "history_session_validate" => history::validate(ctx, &effective_args),
        "server_info" => server_info(ctx),
        "query_tool_usage" => crate::tools::tool_usage::query_tool_usage(ctx, &effective_args),
        "exec_health_check" => exec::exec_health_check(ctx),
        "set_default_cwd" => set_default_cwd(ctx, &effective_args),
        "set_todos" => agent_plan::set_todos(ctx, &effective_args),
        "update_plan" => agent_plan::update_plan(ctx, &effective_args),
        "report_progress" => agent_plan::report_progress(ctx, &effective_args),
        "read_file" => file::read_file(ws, &effective_args),
        "read_many" => file::read_many(ws, &effective_args),
        "project_map" => project::project_map(ws, &effective_args),
        "list_files" => file::list_files(ws, &effective_args),
        "search_text" => search_text_with_knowledge_canary(
            ctx,
            ws,
            &effective_args,
            execution.canary_sample_key.as_deref(),
        ),
        "patch_check" => patch::patch_check(ctx, &effective_args),
        "apply_patch" => patch::apply_patch(ctx, &effective_args),
        "edit" => patch::edit(ctx, &effective_args),
        "edit_file" => patch::edit_file(ctx, &effective_args),
        "edit_many" => patch::edit_many(ctx, &effective_args),
        "file_ops" => patch::file_ops(ctx, &effective_args),
        "format_files" => file_action::format_files(ctx, &effective_args),
        "exec_command" => exec::exec_command(ctx, &effective_args),
        "exec_many" => Ok(call_exec_many_sync(ctx, &effective_args)),
        "wait_command" => session::wait_command(&ctx.sessions, &effective_args),
        "resolve_operation" => session::resolve_operation(&ctx.sessions, &effective_args),
        "list_sessions" => session::list_sessions(&ctx.sessions, &effective_args),
        "send_input" => session::send_input(&ctx.sessions, &effective_args),
        "read_output" => session::read_output(&ctx.sessions, &effective_args),
        "kill_session" => session::kill_session(&ctx.sessions, &effective_args),
        "git_status" => git::git_status(ws, &effective_args),
        "git_diff" => git::git_diff(ws, &effective_args),
        "git_log" => git::git_log(ws, &effective_args),
        "git_show" => git::git_show(ws, &effective_args),
        "git_blame" => git::git_blame(ws, &effective_args),
        "git_branch" => git::git_branch(ws, &effective_args),
        "git_worktree" => git::git_worktree(ws, &effective_args),
        "git_stage" => git::git_stage(ws, &effective_args),
        "git_commit" => git::git_commit(ws, &effective_args),
        "git_push" => git::git_push(ws, &effective_args),
        "git_restore" => git::git_restore(ws, &effective_args),
        "view_image" => image_tool::view_image(ws, &effective_args),
        "desktop_displays" => desktop::displays(&effective_args),
        "desktop_screenshot" => desktop::screenshot(&effective_args),
        "desktop_click" => desktop::click(&effective_args),
        "desktop_drag" => desktop::drag(&effective_args),
        "desktop_scroll" => desktop::scroll(&effective_args),
        "desktop_type" => desktop::type_text(&effective_args),
        "desktop_key" => desktop::key(&effective_args),
        "request_permissions" => request_permissions(ctx, &effective_args),
        _ => {
            return tool_err_code(
                "INVALID_ARGUMENT",
                format!("Unknown tool: {name}"),
                "validation",
            )
        }
    };
    let output = match result {
        Ok(v) => v,
        Err(e) => tool_err(e),
    };
    finish_tracked_call(ctx, name, args, tracking, output)
}

fn search_text_with_knowledge_canary(
    ctx: &ToolContext,
    ws: &crate::tools::workspace::Workspace,
    args: &Value,
    request_sample_key: Option<&str>,
) -> Result<Value, WorkspaceError> {
    let decision = knowledge_search_canary_decision(ctx, args, request_sample_key);
    let exact_total_fast_tail = decision.as_ref().is_some_and(|decision| {
        decision.applied
            && decision.implementation == ToolStrategyImplementationId::SearchExactTotalFastTail
    });
    let mut value = file::search_text_with_options(
        ws,
        args,
        file::SearchTextExecutionOptions {
            exact_total_fast_tail,
        },
    )?;
    if let (Some(decision), Some(object)) = (decision, value.as_object_mut()) {
        object.insert("knowledge_canary_id".into(), json!(decision.knowledge_id));
        object.insert(
            "knowledge_canary_implementation".into(),
            serde_json::to_value(decision.implementation).unwrap_or(Value::Null),
        );
        object.insert("knowledge_canary_eligible".into(), json!(decision.eligible));
        object.insert(
            "knowledge_canary_stage".into(),
            serde_json::to_value(decision.stage).unwrap_or(Value::Null),
        );
        object.insert("knowledge_canary_selected".into(), json!(true));
        object.insert("knowledge_canary_applied".into(), json!(decision.applied));
        object.insert("knowledge_canary_bucket".into(), json!(decision.bucket));
    }
    Ok(value)
}

fn knowledge_search_canary_decision(
    ctx: &ToolContext,
    args: &Value,
    request_sample_key: Option<&str>,
) -> Option<KnowledgeCanaryDecision> {
    let scope_root = ctx.harness.scope_root_for(&ctx.default_cwd_path());
    let task_id = ctx
        .harness
        .current_task_for_root(&scope_root)
        .ok()
        .flatten()
        .map(|task| task.id);
    let sample_key = canary_sample_key(task_id.as_deref(), request_sample_key, &ctx.profile_id);
    let store = knowledge_store_for_context(ctx);
    let impact = CanaryImpactStore::new(store.root());
    CanaryStrategyEngine::new(store, impact)
        .decide("search_text", args, sample_key)
        .ok()
        .flatten()
}

fn canary_sample_key<'a>(
    task_id: Option<&'a str>,
    request_sample_key: Option<&'a str>,
    profile_id: &'a str,
) -> &'a str {
    task_id
        .filter(|value| !value.is_empty())
        .or_else(|| request_sample_key.filter(|value| !value.is_empty()))
        .unwrap_or(profile_id)
}

fn request_permissions(ctx: &ToolContext, args: &Value) -> Result<Value, WorkspaceError> {
    let runtime = ctx.runtime_config();
    if let Some(resume_id) = args.get("resume_id").and_then(Value::as_str) {
        let operation = ctx.pending_operations.take(resume_id)?;
        let explicitly_approved = args
            .get("approve")
            .and_then(Value::as_bool)
            .unwrap_or(false)
            && args
                .get("confirm")
                .and_then(Value::as_bool)
                .unwrap_or(false);
        let approved = explicitly_approved || runtime.policy.skip_permission_gates();
        if !approved {
            ctx.pending_operations.put_back(operation);
            return Err(WorkspaceError::ToolDetails {
                code: "PERMISSION_NOT_APPROVED",
                message: "The pending operation was not approved.".into(),
                category: "permission",
                retryable: true,
                details: json!({
                    "resume_id": resume_id,
                    "suggestion": "取得用户授权后以 approve=true、confirm=true 重试 request_permissions"
                }),
            });
        }

        let mut resumed_args = operation.arguments.clone();
        if matches!(
            operation.permission.as_str(),
            "destructive_command" | "shell_expansion" | "inline_script"
        ) {
            if let Some(object) = resumed_args.as_object_mut() {
                object.insert("confirm".into(), Value::Bool(true));
            }
        }
        let mut resumed = call_tool_inner(
            ctx,
            &operation.tool_name,
            &resumed_args,
            true,
            &ToolCallExecutionContext::default(),
        );
        if let Some(object) = resumed.as_object_mut() {
            object.insert("resumed".into(), Value::Bool(true));
            object.insert("resume_id".into(), Value::String(operation.resume_id));
            object.insert(
                "permission_grant".into(),
                json!({
                    "status": "granted_and_resumed",
                    "permission": operation.permission,
                    "reason": operation.reason,
                    "scope": args.get("scope").and_then(Value::as_str).unwrap_or("once")
                }),
            );
        }
        return Ok(resumed);
    }

    if runtime.policy.skip_permission_gates() {
        Ok(tool_ok(json!({
            "ok": true,
            "status": "granted",
            "grant_id": "dangerously-skip-all-permissions",
            "expires_at": null,
            "constraints": {
                "mode": "dangerous",
                "workspace": ctx.workspace.root_display(),
                "requested": args
            },
            "warnings": [
                "dangerous permission mode is enabled; permission-gated operations are auto-granted"
            ]
        })))
    } else {
        Ok(tool_ok(json!({
            "ok": false,
            "status": "unsupported",
            "grant_id": null,
            "expires_at": null,
            "next_actions": [],
            "error": {
                "code": "RESUME_ID_REQUIRED",
                "message": "Provide the resume_id returned by the blocked operation.",
                "category": "permission",
                "retryable": true,
                "details": { "requested": args }
            }
        })))
    }
}

fn apply_default_cwd<'a>(ctx: &ToolContext, name: &str, args: &'a Value) -> Cow<'a, Value> {
    let mut cwd = ctx.default_cwd_path();
    if !cwd.is_dir() {
        cwd = ctx.workspace.root().to_path_buf();
        ctx.set_default_cwd(cwd.clone());
    }
    let base = if cwd == ctx.workspace.root() {
        ".".to_string()
    } else {
        relative_display(ctx.workspace.root(), &cwd)
    };
    let security = ctx.runtime_config().policy.security_policy;
    let security_normalization_needed =
        !security.require_write_confirmation || !security.verify_write_conflicts;
    if base == "." && !security_normalization_needed {
        return Cow::Borrowed(args);
    }

    let mut effective = args.clone();
    match name {
        "exec_command" if effective.get("workdir").is_none() && effective.get("cwd").is_none() => {
            effective["workdir"] = Value::String(base.clone());
        }
        "list_files" | "project_map" | "search_text" | "git_status" | "git_log" => {
            let path = effective.get("path").and_then(Value::as_str).unwrap_or(".");
            effective["path"] = Value::String(prefix_relative_path(&base, path));
        }
        "read_file" | "git_blame" | "view_image" => {
            if let Some(path) = effective.get("path").and_then(Value::as_str) {
                effective["path"] = Value::String(prefix_relative_path(&base, path));
            }
        }
        "read_many" => {
            if let Some(items) = effective.get("items").and_then(Value::as_array).cloned() {
                effective["items"] = Value::Array(
                    items
                        .into_iter()
                        .map(|mut item| {
                            if let Some(path) = item.get("path").and_then(Value::as_str) {
                                item["path"] = Value::String(prefix_relative_path(&base, path));
                            }
                            item
                        })
                        .collect(),
                );
            }
        }
        "git_diff" | "git_show" => {
            // Git read targets use the same coordinate contract as Git mutators:
            // repo_path is workspace-relative and pathspecs are repo-relative.
            if effective.get("repo_path").is_none() {
                effective["repo_path"] = Value::String(base.clone());
            }
        }
        "format_files" => {
            if let Some(paths) = effective.get("paths").and_then(Value::as_array).cloned() {
                effective["paths"] = Value::Array(
                    paths
                        .iter()
                        .map(|path| {
                            path.as_str()
                                .map(|value| Value::String(prefix_relative_path(&base, value)))
                                .unwrap_or_else(|| path.clone())
                        })
                        .collect(),
                );
            } else if matches!(
                effective.get("scope").and_then(Value::as_str),
                Some("changed" | "staged" | "project")
            ) {
                effective["paths"] = json!([base.clone()]);
            }
            if let Some(hashes) = effective
                .get("expected_sha256")
                .and_then(Value::as_object)
                .cloned()
            {
                effective["expected_sha256"] = Value::Object(
                    hashes
                        .into_iter()
                        .map(|(path, hash)| (prefix_relative_path(&base, &path), hash))
                        .collect(),
                );
            }
        }
        "apply_patch" | "patch_check" => {
            if let Some(patch) = effective.get("patch").and_then(Value::as_str) {
                effective["patch"] = Value::String(prefix_patch_paths(&base, patch));
            }
            if let Some(hashes) = effective
                .get("expected_sha256")
                .and_then(Value::as_object)
                .cloned()
            {
                effective["expected_sha256"] = Value::Object(
                    hashes
                        .into_iter()
                        .map(|(path, hash)| (prefix_relative_path(&base, &path), hash))
                        .collect(),
                );
            }
        }
        "edit_file" => {
            if let Some(path) = effective.get("path").and_then(Value::as_str) {
                effective["path"] = Value::String(prefix_relative_path(&base, path));
            }
        }
        "edit" | "edit_many" => {
            prefix_array_paths(&mut effective, "files", &base, &["path"]);
        }
        "file_ops" => {
            prefix_array_paths(
                &mut effective,
                "operations",
                &base,
                &["path", "destination"],
            );
        }
        "git_branch" | "git_worktree" | "git_stage" | "git_commit" | "git_push" | "git_restore" => {
            // Git mutator repo_path is workspace-relative; pathspecs are repo-relative.
            // Only an omitted repo_path inherits the conversation default cwd.
            if effective.get("repo_path").is_none() {
                effective["repo_path"] = Value::String(base.clone());
            }
        }
        _ => {}
    }
    apply_security_defaults(&security, name, &mut effective);
    Cow::Owned(effective)
}

fn apply_security_defaults(
    security: &crate::workspace::SecurityPolicy,
    name: &str,
    effective: &mut Value,
) {
    if !security.require_write_confirmation
        && matches!(
            name,
            "apply_patch"
                | "edit"
                | "edit_file"
                | "edit_many"
                | "file_ops"
                | "format_files"
                | "git_branch"
                | "git_restore"
        )
    {
        effective["confirm"] = Value::Bool(true);
    }
    if security.verify_write_conflicts {
        return;
    }
    if let Some(object) = effective.as_object_mut() {
        object.remove("expected_sha256");
        for key in ["files", "operations"] {
            if let Some(items) = object.get_mut(key).and_then(Value::as_array_mut) {
                for item in items {
                    if let Some(item) = item.as_object_mut() {
                        item.remove("expected_sha256");
                    }
                }
            }
        }
    }
}

fn prefix_array_paths(value: &mut Value, array_key: &str, base: &str, keys: &[&str]) {
    if let Some(items) = value.get(array_key).and_then(Value::as_array).cloned() {
        value[array_key] = Value::Array(
            items
                .into_iter()
                .map(|mut item| {
                    for key in keys {
                        if let Some(path) = item.get(*key).and_then(Value::as_str) {
                            item[*key] = Value::String(prefix_relative_path(base, path));
                        }
                    }
                    item
                })
                .collect(),
        );
    }
}

fn prefix_relative_path(base: &str, path: &str) -> String {
    if path == "." || path.is_empty() {
        return base.to_string();
    }
    if Path::new(path).is_absolute() || path.starts_with("..") {
        return path.to_string();
    }
    format!("{base}/{}", path.trim_start_matches("./"))
}

fn prefix_patch_paths(base: &str, patch: &str) -> String {
    patch
        .lines()
        .map(|line| {
            for marker in ["--- a/", "+++ b/"] {
                if let Some(path) = line.strip_prefix(marker) {
                    return format!("{marker}{base}/{path}");
                }
            }
            line.to_string()
        })
        .collect::<Vec<_>>()
        .join("\n")
}

struct GitMetadata {
    git_dir: PathBuf,
    common_dir: PathBuf,
}

fn canonical(path: impl AsRef<Path>) -> Option<PathBuf> {
    fs::canonicalize(path).ok()
}

fn git_metadata_for_canonical_workspace(workspace: &Path) -> Option<GitMetadata> {
    let marker = workspace.join(".git");
    if marker.is_dir() {
        let git_dir = canonical(&marker)?;
        return git_dir.starts_with(&workspace).then(|| GitMetadata {
            common_dir: git_dir.clone(),
            git_dir,
        });
    }

    let pointer = fs::read_to_string(&marker).ok()?;
    let raw = pointer.trim().strip_prefix("gitdir:")?.trim();
    let requested_git_dir = if Path::new(raw).is_absolute() {
        PathBuf::from(raw)
    } else {
        workspace.join(raw)
    };
    let git_dir = canonical(requested_git_dir)?;
    let common_raw = fs::read_to_string(git_dir.join("commondir")).ok()?;
    let common_raw = common_raw.trim();
    let requested_common = if Path::new(common_raw).is_absolute() {
        PathBuf::from(common_raw)
    } else {
        git_dir.join(common_raw)
    };
    let common_dir = canonical(requested_common)?;
    if common_dir.file_name().and_then(|name| name.to_str()) != Some(".git") {
        return None;
    }
    let repository_root = common_dir.parent()?;
    if !workspace.starts_with(repository_root) || !git_dir.starts_with(common_dir.join("worktrees"))
    {
        return None;
    }
    Some(GitMetadata {
        git_dir,
        common_dir,
    })
}

fn git_metadata_for_workspace(root: &Path) -> Option<GitMetadata> {
    let workspace = canonical(root)?;
    git_metadata_for_canonical_workspace(&workspace)
}

fn valid_git_hash(value: &str) -> bool {
    matches!(value.len(), 40 | 64) && value.bytes().all(|byte| byte.is_ascii_hexdigit())
}

fn valid_git_ref(value: &str) -> bool {
    value.starts_with("refs/")
        && !value.contains('\\')
        && value
            .split('/')
            .all(|part| !part.is_empty() && part != "." && part != "..")
}

fn valid_git_head_marker(head: &str) -> bool {
    let reference = head.strip_prefix("ref: ").map(str::trim);
    reference
        .map(valid_git_ref)
        .unwrap_or_else(|| valid_git_hash(head))
}

pub(crate) fn fast_containing_git_root(workspace_root: &Path, cwd: &Path) -> Option<PathBuf> {
    let workspace = canonical(workspace_root)?;
    let mut current = canonical(cwd)?;
    if !current.starts_with(&workspace) {
        return None;
    }
    loop {
        if let Some(metadata) = git_metadata_for_canonical_workspace(&current) {
            if let Ok(head) = fs::read_to_string(metadata.git_dir.join("HEAD")) {
                if valid_git_head_marker(head.trim()) {
                    return Some(current);
                }
            }
        }
        if current == workspace {
            return None;
        }
        current = current.parent()?.to_path_buf();
        if !current.starts_with(&workspace) {
            return None;
        }
    }
}

pub(crate) fn fast_workspace_git_identity(root: &Path) -> Option<(String, String)> {
    let metadata = git_metadata_for_workspace(root)?;
    let head = fs::read_to_string(metadata.git_dir.join("HEAD")).ok()?;
    let head = head.trim();
    let Some(reference) = head.strip_prefix("ref: ").map(str::trim) else {
        return valid_git_hash(head).then(|| ("HEAD".into(), head.to_ascii_lowercase()));
    };
    if !valid_git_ref(reference) {
        return None;
    }
    let branch = reference
        .strip_prefix("refs/heads/")
        .unwrap_or(reference)
        .to_string();
    for base in [&metadata.git_dir, &metadata.common_dir] {
        if let Ok(value) = fs::read_to_string(base.join(reference)) {
            let value = value.trim();
            if valid_git_hash(value) {
                return Some((branch.clone(), value.to_ascii_lowercase()));
            }
        }
    }
    for base in [&metadata.git_dir, &metadata.common_dir] {
        let Ok(packed) = fs::read_to_string(base.join("packed-refs")) else {
            continue;
        };
        for line in packed.lines() {
            if line.starts_with('#') || line.starts_with('^') {
                continue;
            }
            let Some((value, name)) = line.split_once(' ') else {
                continue;
            };
            if name.trim() == reference && valid_git_hash(value.trim()) {
                return Some((branch.clone(), value.trim().to_ascii_lowercase()));
            }
        }
    }
    None
}

fn fast_workspace_git_head(root: &Path) -> Option<String> {
    fast_workspace_git_identity(root).map(|(_, head)| head)
}

fn is_runtime_source_workspace(root: &Path) -> bool {
    let node_package = fs::read_to_string(root.join("packages/node-agent/package.json")).ok();
    let cargo_manifest = fs::read_to_string(root.join("src-tauri/Cargo.toml")).ok();
    node_package
        .as_deref()
        .is_some_and(|value| value.contains("\"name\": \"@coding-tools/node-agent\""))
        && cargo_manifest
            .as_deref()
            .is_some_and(|value| value.contains("name = \"coding-tools-mcp-desktop\""))
}

pub fn server_info(ctx: &ToolContext) -> Result<Value, WorkspaceError> {
    let runtime = ctx.runtime_config();
    let tools = crate::tools::registry::exposed_tool_names(&runtime.tool_profile);
    let toolset_revision = crate::tools::registry::toolset_revision(&runtime.tool_profile);
    let runtime_build_git_sha = option_env!("CTMCP_BUILD_GIT_SHA").unwrap_or("unknown");
    let runtime_build_source_clean = match option_env!("CTMCP_BUILD_SOURCE_CLEAN") {
        Some("true") => Some(true),
        Some("false") => Some(false),
        _ => None,
    };
    let workspace_git_head = fast_workspace_git_head(ctx.workspace.root());
    let source_workspace = is_runtime_source_workspace(ctx.workspace.root());
    let runtime_matches_workspace = if source_workspace {
        workspace_git_head.as_deref().and_then(|head| {
            (runtime_build_git_sha != "unknown").then_some(head == runtime_build_git_sha)
        })
    } else {
        None
    };
    let runtime_trust_state = if !source_workspace {
        "not_applicable"
    } else {
        match (runtime_build_source_clean, runtime_matches_workspace) {
            (Some(false), _) => "dirty_build",
            (Some(true), Some(true)) => "revision_match_unverified",
            (Some(true), Some(false)) => "mismatch",
            _ => "unknown",
        }
    };
    let runtime_trusted =
        matches!(runtime_trust_state, "dirty_build" | "mismatch").then_some(false);
    let runtime_revision_warning = match runtime_trust_state {
        "dirty_build" => Some("The MCP binary was built from a dirty worktree; rebuild from a clean commit before trusting live schemas or behavior.".to_string()),
        "mismatch" => Some(format!(
            "Running MCP build {runtime_build_git_sha} differs from workspace HEAD {}. Restart/rebuild before trusting live schemas or behavior.",
            workspace_git_head.as_deref().unwrap_or("unknown")
        )),
        "revision_match_unverified" => Some("Build commit matches workspace HEAD, but server_info does not inspect uncommitted worktree changes. Confirm git_status.clean before treating runtime and source as identical.".to_string()),
        _ => None,
    };
    let (blocking_limit, global_blocking_limit) = ctx
        .admission_for("read_file")
        .map(|(_, local, _, global, _)| (local, global))
        .unwrap_or((0, 0));
    let (process_limit, global_process_limit) = ctx
        .admission_for("exec_command")
        .map(|(_, local, _, global, _)| (local, global))
        .unwrap_or((0, 0));
    let sandbox_backend_id = runtime.sandbox.backend.trim();
    let sandbox_backend = crate::tools::sandbox::backend(sandbox_backend_id);
    let sandbox_supported =
        sandbox_backend.is_some_and(|backend| backend.supports_workspace(&ctx.workspace));
    let sandbox_ready =
        sandbox_backend.is_some_and(|backend| backend.descriptor().enforcement_ready);
    let sandbox_available = sandbox_supported && sandbox_ready;
    let sandbox_enforced = runtime.sandbox.enabled && sandbox_available;
    let sandbox_boundary = if sandbox_enforced {
        sandbox_backend_id
    } else if runtime.sandbox.enabled {
        "sandbox_unavailable"
    } else {
        "policy_only"
    };
    Ok(tool_ok(json!({
        "server": "coding-tools-mcp",
        "title": "Coding Tools MCP",
        "version": env!("CARGO_PKG_VERSION"),
        "protocol_version": crate::mcp::LATEST_PROTOCOL_VERSION,
        "supported_protocol_versions": crate::mcp::SUPPORTED_PROTOCOL_VERSIONS,
        "workspace": ctx.workspace.root_display(),
        "permission_mode": &runtime.permission_mode,
        "default_cwd": ctx.default_cwd_display(),
        "network_allowed": runtime.policy.network_allowed(),
        "tool_profile": &runtime.tool_profile,
        "toolset_revision": toolset_revision,
        "runtime_revision": {
            "build_git_sha": runtime_build_git_sha,
            "workspace_git_head": workspace_git_head,
            "source_workspace": source_workspace,
            "matches_workspace": runtime_matches_workspace,
            "source_clean": runtime_build_source_clean,
            "workspace_clean_verified": false,
            "workspace_clean_verification_tool": "git_status",
            "trusted": runtime_trusted,
            "trust_state": runtime_trust_state,
            "warning": runtime_revision_warning
        },
        "auth_enabled": ctx.auth.auth_enabled(),
        "auth_type": ctx.auth.auth_type,
        "endpoint_path": "/mcp",
        "concurrency": {
            "fast_lane": "inline",
            "shared_across_transports": true,
            "workspace_blocking_admission_limit": blocking_limit,
            "workspace_process_admission_limit": process_limit,
            "global_blocking_admission_limit": global_blocking_limit,
            "global_process_admission_limit": global_process_limit,
            "admission_scope": "global_plus_workspace",
            "active_session_limit": ctx.sessions.active_session_limit(),
            "active_session_slots_available": ctx.sessions.active_slots_available(),
            "admission_timeout_ms": ADMISSION_TIMEOUT.as_millis(),
            "session_admission_timeout_ms": 1000
        },
        "environment": {
            "os": std::env::consts::OS,
            "arch": std::env::consts::ARCH,
            "current_executable": std::env::current_exe().ok().map(|path| path.display().to_string()),
            "powershell": exec::powershell_environment(),
            "filesystem_sandbox": {
                "available": sandbox_available,
                "enforced": sandbox_enforced,
                "enabled": runtime.sandbox.enabled,
                "backend": sandbox_backend_id,
                "verification_tool": "exec_health_check",
                "live_verification_required": runtime.sandbox.enabled,
                "default_scope": "workspace",
                "host_scope_available": false
            },
            "workspace_exec": {
                "available": !runtime.sandbox.enabled || sandbox_available,
                "sandbox_enforced": sandbox_enforced,
                "sandbox_backend": sandbox_backend_id,
                "boundary": sandbox_boundary,
                "workspace_local_entries": runtime.policy.workspace_local_entries,
                "script_extensions": runtime.policy.workspace_script_extensions.iter().cloned().collect::<Vec<_>>(),
                "system_command_allowlist": runtime.policy.allowed_commands.iter().cloned().collect::<Vec<_>>()
            }
        },
        "learning": crate::knowledge::learning_bootstrap_summary(ctx).unwrap_or_else(|_| json!({
            "state": "metadata_unavailable",
            "runtime_policy": "read_only_rust_parity",
            "knowledge_loading_policy": "Persistent knowledge is not injected into inference context."
        })),
        "tools": tools,
        "tool_count": tools.len()
    })))
}

pub fn set_default_cwd(ctx: &ToolContext, args: &Value) -> Result<Value, WorkspaceError> {
    let path = args.get("path").and_then(Value::as_str).unwrap_or(".");
    let resolved = ctx.workspace.resolve_existing(path)?;
    if !resolved.path.is_dir() {
        return Err(WorkspaceError::not_a_directory(
            "Default cwd must be a directory",
        ));
    }
    ctx.set_default_cwd(resolved.path.clone());
    Ok(tool_ok(json!({
        "workspace": ctx.workspace.root_display(),
        "default_cwd": resolved.display,
        "resolved_cwd": resolved.path.display().to_string()
    })))
}

#[cfg(test)]
mod tests {
    use std::borrow::Cow;
    use std::collections::BTreeMap;
    use std::sync::Arc;

    use serde_json::json;

    use crate::tools::parallel_stats::ParallelPairStats;
    use crate::tools::ToolContext;

    use super::{
        apply_default_cwd, begin_tracked_call, call_tool, call_tool_async, canary_sample_key,
        collect_parallelism_observations, command_parallel_signature,
        default_exec_many_parallelism, finish_tracked_call, parallel_pair_key,
        parse_exec_batch_commands, resolve_exec_many_decision_with_history,
    };

    #[test]
    fn canary_sample_key_prefers_task_then_request_session_then_profile() {
        assert_eq!(
            canary_sample_key(Some("task"), Some("session"), "profile"),
            "task"
        );
        assert_eq!(
            canary_sample_key(None, Some("session"), "profile"),
            "session"
        );
        assert_eq!(canary_sample_key(None, None, "profile"), "profile");
        assert_eq!(
            canary_sample_key(Some(""), Some("session"), "profile"),
            "session"
        );
        assert_eq!(canary_sample_key(None, Some(""), "profile"), "profile");
    }

    #[test]
    fn fast_workspace_git_head_reads_repo_local_refs_and_rejects_external_pointers() {
        let workspace = tempfile::tempdir().expect("workspace tempdir");
        let git_dir = workspace.path().join(".git");
        std::fs::create_dir_all(git_dir.join("refs/heads")).expect("git refs");
        std::fs::write(git_dir.join("HEAD"), "ref: refs/heads/main\n").expect("head");
        std::fs::write(
            git_dir.join("refs/heads/main"),
            "0123456789abcdef0123456789abcdef01234567\n",
        )
        .expect("ref");
        assert_eq!(
            super::fast_workspace_git_identity(workspace.path()),
            Some((
                "main".into(),
                "0123456789abcdef0123456789abcdef01234567".into()
            ))
        );
        assert_eq!(
            super::fast_workspace_git_head(workspace.path()).as_deref(),
            Some("0123456789abcdef0123456789abcdef01234567")
        );
        let deep = workspace.path().join("src/deep");
        std::fs::create_dir_all(&deep).expect("deep workspace path");
        let workspace_canonical =
            std::fs::canonicalize(workspace.path()).expect("workspace canonical");
        assert_eq!(
            super::fast_containing_git_root(workspace.path(), &deep).as_deref(),
            Some(workspace_canonical.as_path())
        );

        let nested = workspace.path().join("packages/nested");
        let nested_git = nested.join(".git");
        let nested_deep = nested.join("src/deep");
        std::fs::create_dir_all(&nested_git).expect("nested git dir");
        std::fs::create_dir_all(&nested_deep).expect("nested deep path");
        std::fs::write(
            nested_git.join("HEAD"),
            "89abcdef0123456789abcdef0123456789abcdef\n",
        )
        .expect("nested head");
        let nested_canonical = std::fs::canonicalize(&nested).expect("nested canonical");
        assert_eq!(
            super::fast_containing_git_root(workspace.path(), &nested_deep).as_deref(),
            Some(nested_canonical.as_path())
        );

        let repository = tempfile::tempdir().expect("repository tempdir");
        let common = repository.path().join(".git");
        let linked = repository.path().join(".worktrees/linked");
        let worktree_git = common.join("worktrees/linked");
        std::fs::create_dir_all(&linked).expect("linked workspace");
        std::fs::create_dir_all(&worktree_git).expect("worktree git dir");
        std::fs::create_dir_all(common.join("refs/heads")).expect("common refs");
        std::fs::write(
            linked.join(".git"),
            format!("gitdir: {}\n", worktree_git.display()),
        )
        .expect("git pointer");
        std::fs::write(worktree_git.join("HEAD"), "ref: refs/heads/linked\n").expect("head");
        std::fs::write(worktree_git.join("commondir"), "../..\n").expect("common pointer");
        std::fs::write(
            common.join("refs/heads/linked"),
            "fedcba9876543210fedcba9876543210fedcba98\n",
        )
        .expect("linked ref");
        assert_eq!(
            super::fast_workspace_git_identity(&linked),
            Some((
                "linked".into(),
                "fedcba9876543210fedcba9876543210fedcba98".into()
            ))
        );
        assert_eq!(
            super::fast_workspace_git_head(&linked).as_deref(),
            Some("fedcba9876543210fedcba9876543210fedcba98")
        );
        let linked_deep = linked.join("src/deep");
        std::fs::create_dir_all(&linked_deep).expect("linked deep path");
        let linked_canonical = std::fs::canonicalize(&linked).expect("linked canonical");
        assert_eq!(
            super::fast_containing_git_root(repository.path(), &linked_deep).as_deref(),
            Some(linked_canonical.as_path())
        );

        let malicious = tempfile::tempdir().expect("malicious workspace");
        let external = tempfile::tempdir().expect("external directory");
        std::fs::write(
            external.path().join("HEAD"),
            "0123456789abcdef0123456789abcdef01234567\n",
        )
        .expect("external head");
        std::fs::write(
            malicious.path().join(".git"),
            format!("gitdir: {}\n", external.path().display()),
        )
        .expect("malicious pointer");
        assert_eq!(super::fast_workspace_git_head(malicious.path()), None);
        assert_eq!(
            super::fast_containing_git_root(malicious.path(), malicious.path()),
            None
        );
    }

    #[test]
    fn default_cwd_rewrite_borrows_until_a_path_change_is_required() {
        let workspace = tempfile::tempdir().expect("workspace tempdir");
        let harness = tempfile::tempdir().expect("harness tempdir");
        let ctx =
            ToolContext::for_test(workspace.path().to_path_buf(), harness.path().to_path_buf())
                .expect("tool context");
        let arguments = json!({
            "patch": "x".repeat(256 * 1024),
            "confirm": true
        });

        let unchanged = apply_default_cwd(&ctx, "apply_patch", &arguments);
        assert!(matches!(unchanged, Cow::Borrowed(_)));

        let subdir = workspace.path().join("subdir");
        std::fs::create_dir(&subdir).expect("create subdir");
        ctx.set_default_cwd(subdir);
        let rewritten = apply_default_cwd(&ctx, "apply_patch", &arguments);
        assert!(matches!(rewritten, Cow::Owned(_)));

        std::fs::remove_dir_all(workspace.path().join("subdir")).expect("remove stale cwd");
        let root_read = json!({"path": "main.txt"});
        let repaired = apply_default_cwd(&ctx, "read_file", &root_read);
        assert!(matches!(repaired, Cow::Borrowed(_)));
        assert_eq!(ctx.default_cwd_display(), ".");
    }

    #[test]
    fn default_cwd_keeps_git_pathspecs_repo_relative_and_only_defaults_missing_repo_path() {
        let workspace = tempfile::tempdir().expect("workspace tempdir");
        let harness = tempfile::tempdir().expect("harness tempdir");
        let nested = workspace.path().join("nested-repo");
        std::fs::create_dir(&nested).expect("nested repo fixture");
        let ctx =
            ToolContext::for_test(workspace.path().to_path_buf(), harness.path().to_path_buf())
                .expect("tool context");
        ctx.set_default_cwd(nested);

        let explicit_args = json!({"repo_path": "nested-repo", "paths": ["tracked.txt"]});
        let explicit = apply_default_cwd(&ctx, "git_stage", &explicit_args);
        assert_eq!(explicit["repo_path"], "nested-repo");
        assert_eq!(explicit["paths"], json!(["tracked.txt"]));

        let implicit_args = json!({"paths": ["tracked.txt"]});
        let implicit = apply_default_cwd(&ctx, "git_stage", &implicit_args);
        assert_eq!(implicit["repo_path"], "nested-repo");
        assert_eq!(implicit["paths"], json!(["tracked.txt"]));

        let explicit_diff_args = json!({"repo_path": "nested-repo", "paths": ["tracked.txt"]});
        let explicit_diff = apply_default_cwd(&ctx, "git_diff", &explicit_diff_args);
        assert_eq!(explicit_diff["repo_path"], "nested-repo");
        assert_eq!(explicit_diff["paths"], json!(["tracked.txt"]));

        let implicit_diff_args = json!({"paths": ["tracked.txt"]});
        let implicit_diff = apply_default_cwd(&ctx, "git_diff", &implicit_diff_args);
        assert_eq!(implicit_diff["repo_path"], "nested-repo");
        assert_eq!(implicit_diff["paths"], json!(["tracked.txt"]));

        let implicit_show_args = json!({"rev": "HEAD"});
        let implicit_show = apply_default_cwd(&ctx, "git_show", &implicit_show_args);
        assert_eq!(implicit_show["repo_path"], "nested-repo");
    }

    #[test]
    fn default_cwd_scan_tools_can_operate_inside_hidden_linked_worktree() {
        let workspace = tempfile::tempdir().expect("workspace tempdir");
        let harness = tempfile::tempdir().expect("harness tempdir");
        let linked = workspace.path().join(".worktrees/linked");
        std::fs::create_dir_all(linked.join("src")).expect("linked worktree fixture");
        std::fs::write(
            linked.join("package.json"),
            r#"{"name":"linked-fixture","scripts":{"test":"echo ok"}}"#,
        )
        .expect("package fixture");
        std::fs::write(linked.join("src/marker.txt"), "linked-worktree-needle\n")
            .expect("marker fixture");

        let ctx =
            ToolContext::for_test(workspace.path().to_path_buf(), harness.path().to_path_buf())
                .expect("tool context");
        ctx.set_default_cwd(linked.canonicalize().expect("canonical linked worktree"));

        let listed = call_tool(&ctx, "list_files", &json!({"recursive": true}));
        assert_eq!(listed["ok"], true, "{listed}");
        assert!(
            listed["entries"]
                .as_array()
                .expect("entries")
                .iter()
                .any(|entry| entry["path"] == ".worktrees/linked/src/marker.txt"),
            "{listed}"
        );

        let searched = call_tool(
            &ctx,
            "search_text",
            &json!({"query": "linked-worktree-needle"}),
        );
        assert_eq!(searched["ok"], true, "{searched}");
        assert_eq!(searched["returned_count"], 1, "{searched}");
        assert_eq!(
            searched["matches"][0]["path"], ".worktrees/linked/src/marker.txt",
            "{searched}"
        );

        let mapped = call_tool(&ctx, "project_map", &json!({"max_depth": 3}));
        assert_eq!(mapped["ok"], true, "{mapped}");
        assert!(
            mapped["scanned_files"].as_u64().unwrap_or_default() >= 2,
            "{mapped}"
        );
        assert!(
            mapped["manifests"]
                .as_array()
                .expect("manifests")
                .iter()
                .any(|manifest| manifest["path"] == ".worktrees/linked/package.json"),
            "{mapped}"
        );
    }

    #[test]
    fn canonical_edit_rejects_noop_before_harness_tracking() {
        let workspace = tempfile::tempdir().expect("workspace tempdir");
        let harness = tempfile::tempdir().expect("harness tempdir");
        std::fs::write(workspace.path().join("main.txt"), "same\n").expect("fixture");
        let ctx =
            ToolContext::for_test(workspace.path().to_path_buf(), harness.path().to_path_buf())
                .expect("tool context");

        let result = call_tool(
            &ctx,
            "edit",
            &json!({
                "files": [{
                    "path": "main.txt",
                    "edits": [{"type": "replace", "old_text": "same", "new_text": "same"}]
                }]
            }),
        );

        assert_eq!(result["ok"], false, "{result}");
        assert_eq!(result["error"]["code"], "PATCH_FAILED", "{result}");
        assert!(
            result["phase_durations_ms"]["preflight_ms"].is_number(),
            "{result}"
        );
        assert!(
            result["phase_durations_ms"]["harness_begin_ms"].is_null(),
            "{result}"
        );
        assert_eq!(
            std::fs::read_to_string(workspace.path().join("main.txt")).expect("fixture"),
            "same\n"
        );
    }

    #[test]
    fn harness_tracking_preserves_execution_operation_id() {
        let workspace = tempfile::tempdir().expect("workspace tempdir");
        let harness = tempfile::tempdir().expect("harness tempdir");
        let ctx =
            ToolContext::for_test(workspace.path().to_path_buf(), harness.path().to_path_buf())
                .expect("tool context");
        let args = json!({"program": "cargo", "args": ["test"]});
        let tracking =
            begin_tracked_call(&ctx, "exec_command", &args, &args).expect("tracked call");

        let result = finish_tracked_call(
            &ctx,
            "exec_command",
            &args,
            tracking,
            json!({
                "ok": true,
                "operation_id": "auto:execution-operation",
                "command_ok": false,
                "verification_ok": false,
                "termination_reason": "exited",
                "process_exit_code": 7,
                "execution_mode": "job",
                "timeout_scope": "process",
                "timeout_clamped": false,
                "polling_extends_process_deadline": false,
                "requested_process_timeout_ms": 7_200_000,
                "effective_process_timeout_ms": 7_200_000,
                "process_timeout_limit_ms": 21_600_000,
                "process_deadline_ts_ms": 123_456_789,
                "process_timeout_remaining_ms": 7_199_000,
                "warnings": ["bounded warning"],
                "command": "must-not-persist",
                "stdout": "must-not-persist"
            }),
        );

        assert_eq!(
            result["operation_id"], "auto:execution-operation",
            "{result}"
        );
        assert!(result["harness_operation_id"].is_string(), "{result}");
        assert_ne!(
            result["harness_operation_id"], result["operation_id"],
            "{result}"
        );
        let operations = ctx.harness.list_operations(0, 10).expect("operation log");
        let terminal = operations
            .iter()
            .find(|operation| operation.kind == "completed")
            .expect("completed operation");
        assert_eq!(terminal.result_summary["command_ok"], false);
        assert_eq!(terminal.result_summary["verification_ok"], false);
        assert_eq!(terminal.result_summary["termination_reason"], "exited");
        assert_eq!(terminal.result_summary["process_exit_code"], 7);
        assert_eq!(terminal.result_summary["execution_mode"], "job");
        assert_eq!(terminal.result_summary["timeout_scope"], "process");
        assert_eq!(
            terminal.result_summary["polling_extends_process_deadline"],
            false
        );
        assert_eq!(
            terminal.result_summary["effective_process_timeout_ms"],
            7_200_000
        );
        assert_eq!(
            terminal.result_summary["process_timeout_limit_ms"],
            21_600_000
        );
        assert_eq!(terminal.result_summary["warning_count"], 1);
        assert!(terminal.result_summary.get("command").is_none());
        assert!(terminal.result_summary.get("stdout").is_none());
    }

    #[test]
    fn exec_many_auto_scheduler_combines_hard_rules_and_history() {
        let independent = parse_exec_batch_commands(&[
            json!({"program": "python", "args": ["--version"]}),
            json!({"program": "node", "args": ["--version"]}),
        ])
        .expect("independent commands");
        let decision =
            resolve_exec_many_decision_with_history("auto", &independent, 8, &BTreeMap::new());
        assert_eq!(decision.mode, "parallel");
        assert_eq!(decision.source, "hard_rules");

        let dag = parse_exec_batch_commands(&[
            json!({"id": "prepare", "program": "python", "args": ["--version"]}),
            json!({"id": "finish", "depends_on": ["prepare"], "program": "node", "args": ["--version"]}),
        ])
        .expect("dag commands");
        let decision = resolve_exec_many_decision_with_history("auto", &dag, 8, &BTreeMap::new());
        assert_eq!(decision.mode, "dag");
        assert_eq!(decision.source, "dependency_graph");

        let opaque = parse_exec_batch_commands(&[
            json!({"cmd": "echo first"}),
            json!({"cmd": "echo second"}),
        ])
        .expect("opaque commands");
        let decision =
            resolve_exec_many_decision_with_history("auto", &opaque, 8, &BTreeMap::new());
        assert_eq!(decision.mode, "sequential");
        assert_eq!(decision.source, "hard_safety_rule");

        let evidence_required = parse_exec_batch_commands(&[
            json!({"program": "python", "args": ["first.py"], "workdir": "a"}),
            json!({"program": "node", "args": ["second.js"], "workdir": "b"}),
        ])
        .expect("evidence-required commands");
        let decision = resolve_exec_many_decision_with_history(
            "auto",
            &evidence_required,
            8,
            &BTreeMap::new(),
        );
        assert_eq!(decision.mode, "sequential");
        assert_eq!(decision.source, "insufficient_history");

        let pair = parallel_pair_key(
            &evidence_required[0].parallel_signature,
            &evidence_required[1].parallel_signature,
        );
        let mut safe_history = BTreeMap::new();
        safe_history.insert(
            pair.clone(),
            ParallelPairStats {
                attempts: 5,
                successes: 5,
                ..Default::default()
            },
        );
        let decision =
            resolve_exec_many_decision_with_history("auto", &evidence_required, 8, &safe_history);
        assert_eq!(decision.mode, "parallel");
        assert_eq!(decision.source, "historical_statistics");
        assert_eq!(decision.history_samples, 5);

        let mut conflict_history = BTreeMap::new();
        conflict_history.insert(
            pair,
            ParallelPairStats {
                attempts: 5,
                successes: 3,
                conflicts: 2,
                ..Default::default()
            },
        );
        let decision = resolve_exec_many_decision_with_history(
            "auto",
            &evidence_required,
            8,
            &conflict_history,
        );
        assert_eq!(decision.mode, "sequential");
        assert_eq!(decision.source, "historical_conflict");

        let locked = parse_exec_batch_commands(&[
            json!({"program": "cargo", "args": ["test"], "workdir": "crate-a"}),
            json!({"program": "git", "args": ["commit", "-m", "test"], "workdir": "."}),
            json!({"program": "npm", "args": ["install"], "workdir": "web"}),
        ])
        .expect("locked commands");
        assert_eq!(
            locked[0].lock_group.as_deref(),
            Some("cargo-target:crate-a")
        );
        assert_eq!(locked[1].lock_group.as_deref(), Some("git-index:."));
        assert_eq!(locked[2].lock_group.as_deref(), Some("node-generated:web"));
        assert!(locked.iter().all(|command| command.lock_group_inferred));

        let private_signature = command_parallel_signature(&json!({
            "program": "C:\\private\\CustomerDeployTool.exe",
            "args": ["run"],
            "workdir": "customer-secret-workspace"
        }));
        assert!(private_signature.starts_with("custom-"));
        assert!(!private_signature.contains("customerdeploytool"));
        assert!(!private_signature.contains("customer-secret-workspace"));

        assert_eq!(default_exec_many_parallelism(20, 64), 8);
        assert_eq!(default_exec_many_parallelism(20, 4), 4);
        assert_eq!(default_exec_many_parallelism(1, 64), 1);
    }

    #[test]
    fn exec_many_parallel_observations_classify_overlap_conflict_and_serialization() {
        let commands = parse_exec_batch_commands(&[
            json!({"program": "python", "args": ["--version"]}),
            json!({"program": "node", "args": ["--version"]}),
        ])
        .expect("commands");
        let results = vec![
            json!({
                "id": "command-0",
                "command_ok": true,
                "resource_lock_wait_ms": 0,
                "result": {"started_ts_ms": 1000, "elapsed_ms": 500}
            }),
            json!({
                "id": "command-1",
                "command_ok": true,
                "resource_lock_wait_ms": 0,
                "result": {"started_ts_ms": 1200, "elapsed_ms": 500}
            }),
        ];
        let (observations, truncated) =
            collect_parallelism_observations(&commands, &results, "parallel");
        assert!(!truncated);
        assert_eq!(observations.len(), 1);
        assert_eq!(observations[0]["outcome"], "success");
        assert_eq!(observations[0]["overlap_ms"], 300);

        let conflict_results = vec![
            json!({
                "id": "command-0",
                "command_ok": false,
                "resource_lock_wait_ms": 0,
                "result": {
                    "started_ts_ms": 1000,
                    "elapsed_ms": 500,
                    "stderr": "fatal: Unable to create '.git/index.lock': File exists"
                }
            }),
            json!({
                "id": "command-1",
                "command_ok": true,
                "resource_lock_wait_ms": 0,
                "result": {"started_ts_ms": 1200, "elapsed_ms": 500}
            }),
        ];
        let (observations, _) =
            collect_parallelism_observations(&commands, &conflict_results, "parallel");
        assert_eq!(observations[0]["outcome"], "conflict");

        let locked_commands = parse_exec_batch_commands(&[
            json!({"program": "cargo", "args": ["test"], "workdir": "."}),
            json!({"program": "cargo", "args": ["check"], "workdir": "."}),
        ])
        .expect("locked commands");
        let serialized_results = vec![
            json!({
                "id": "command-0",
                "command_ok": true,
                "resource_lock_wait_ms": 0,
                "result": {"started_ts_ms": 1000, "elapsed_ms": 200}
            }),
            json!({
                "id": "command-1",
                "command_ok": true,
                "resource_lock_wait_ms": 250,
                "result": {"started_ts_ms": 1300, "elapsed_ms": 200}
            }),
        ];
        let (observations, _) =
            collect_parallelism_observations(&locked_commands, &serialized_results, "parallel");
        assert_eq!(observations[0]["outcome"], "serialized");
    }

    #[tokio::test]
    async fn exec_many_runs_sequentially_and_stops_after_failure() {
        let workspace = tempfile::tempdir().expect("workspace tempdir");
        let harness = tempfile::tempdir().expect("harness tempdir");
        let ctx = Arc::new(
            ToolContext::for_test(workspace.path().to_path_buf(), harness.path().to_path_buf())
                .expect("tool context"),
        );

        let result = call_tool_async(
            ctx,
            "exec_many".into(),
            json!({
                "mode": "sequential",
                "commands": [
                    { "program": "cargo", "args": ["--version"] },
                    { "program": "coding-tools-command-that-does-not-exist" },
                    { "program": "cargo", "args": ["--version"] }
                ],
                "stop_on_error": true
            }),
        )
        .await;

        assert_eq!(result["commands_requested"], 3);
        assert_eq!(result["commands_executed"], 2);
        assert_eq!(result["failed_command_count"], 1);
        assert_eq!(result["failed_command_ids"], json!(["command-1"]));
        assert_eq!(result["skipped_command_ids"], json!([]));
        assert_eq!(result["first_failed_command"]["id"], "command-1");
        assert!(result["batch_summary"]
            .as_str()
            .expect("batch summary")
            .contains("1 failed"));
        assert_eq!(result["recovery_actions"].as_array().unwrap().len(), 2);
        assert_eq!(result["skipped_command_count"], 1);
        assert_eq!(result["stopped_early"], true);
        assert_eq!(result["command_ok"], false);
        assert_eq!(result["outcome_class"], "partial_failure");
        assert_eq!(result["execution_lane"], "async_batch");
    }

    #[tokio::test]
    #[serial_test::serial(process_runtime)]
    async fn exec_many_rejects_invalid_child_timeout_before_starting_any_process() {
        let workspace = tempfile::tempdir().expect("workspace tempdir");
        let harness = tempfile::tempdir().expect("harness tempdir");
        let ctx = Arc::new(
            ToolContext::for_test(workspace.path().to_path_buf(), harness.path().to_path_buf())
                .expect("tool context"),
        );

        let result = call_tool_async(
            ctx.clone(),
            "exec_many".into(),
            json!({
                "mode": "sequential",
                "commands": [
                    { "id": "valid", "program": "cargo", "args": ["--version"] },
                    { "id": "invalid", "program": "cargo", "args": ["--version"], "job_timeout_ms": 2_000 }
                ]
            }),
        )
        .await;

        assert_eq!(result["ok"], false, "{result}");
        assert!(
            result
                .to_string()
                .contains("commands[1] rejected before scheduling"),
            "{result}"
        );
        let sessions =
            crate::tools::session::list_sessions(&ctx.sessions, &json!({})).expect("list sessions");
        assert_eq!(sessions["count"], 0, "{sessions}");
    }

    #[tokio::test]
    #[serial_test::serial(process_runtime)]
    async fn exec_many_detaches_long_graph_and_reattaches_without_duplicate_children() {
        let workspace = tempfile::tempdir().expect("workspace tempdir");
        let harness = tempfile::tempdir().expect("harness tempdir");
        let ctx = Arc::new(
            ToolContext::for_test(workspace.path().to_path_buf(), harness.path().to_path_buf())
                .expect("tool context"),
        );
        let operation_id = "rust-retained-long-graph";
        #[cfg(windows)]
        let slow = json!({
            "id": "slow",
            "program": "powershell",
            "args": ["-NoProfile", "-Command", "Start-Sleep -Milliseconds 2000; Write-Output slow-done"],
            "timeout_ms": 5_000
        });
        #[cfg(unix)]
        let slow = json!({
            "id": "slow",
            "program": "sh",
            "args": ["-c", "sleep 2; printf slow-done"],
            "timeout_ms": 5_000
        });

        let started_at = std::time::Instant::now();
        let started = call_tool_async(
            ctx.clone(),
            "exec_many".into(),
            json!({
                "operation_id": operation_id,
                "yield_time_ms": 10,
                "mode": "sequential",
                "commands": [slow]
            }),
        )
        .await;

        assert!(
            started_at.elapsed() < std::time::Duration::from_millis(1_500),
            "{started}"
        );
        assert_eq!(started["ok"], true, "{started}");
        assert_eq!(started["graph_operation_id"], operation_id, "{started}");
        assert_eq!(started["graph_completed"], false, "{started}");
        assert_eq!(started["graph_status"], "running", "{started}");
        assert_eq!(started["detached"], true, "{started}");
        assert_eq!(started["graph_requested_yield_ms"], 10, "{started}");
        assert_eq!(started["graph_yield_ms"], 10, "{started}");
        assert_eq!(started["next_actions"][0]["tool"], "exec_many", "{started}");
        assert_eq!(
            started["next_actions"][0]["arguments"]["operation_id"], operation_id,
            "{started}"
        );
        assert_eq!(
            started["next_actions"][0]["arguments"]["yield_time_ms"], 20_000,
            "{started}"
        );

        let finalized = call_tool_async(
            ctx.clone(),
            "exec_many".into(),
            json!({
                "operation_id": operation_id,
                "yield_time_ms": 5_000,
                "result_mode": "full"
            }),
        )
        .await;
        assert_eq!(finalized["ok"], true, "{finalized}");
        assert_eq!(finalized["graph_completed"], true, "{finalized}");
        assert_eq!(finalized["reattached"], true, "{finalized}");
        assert_eq!(finalized["commands_executed"], 1, "{finalized}");
        assert!(
            finalized["results"][0]["result"]["stdout"]
                .as_str()
                .unwrap_or_default()
                .contains("slow-done"),
            "{finalized}"
        );

        let sessions =
            crate::tools::session::list_sessions(&ctx.sessions, &json!({})).expect("list sessions");
        assert_eq!(sessions["count"], 1, "{sessions}");
    }
    #[tokio::test]
    #[serial_test::serial(process_runtime)]
    async fn exec_many_status_cancel_and_forget_do_not_leave_orphan_child() {
        let workspace = tempfile::tempdir().expect("workspace tempdir");
        let harness = tempfile::tempdir().expect("harness tempdir");
        let ctx = Arc::new(
            ToolContext::for_test(workspace.path().to_path_buf(), harness.path().to_path_buf())
                .expect("tool context"),
        );
        let operation_id = "rust-retained-cancel-graph";
        #[cfg(windows)]
        let slow = json!({
            "id": "slow",
            "program": "powershell",
            "args": ["-NoProfile", "-Command", "Start-Sleep -Seconds 30; Write-Output should-not-complete"],
            "timeout_ms": 60_000
        });
        #[cfg(unix)]
        let slow = json!({
            "id": "slow",
            "program": "sh",
            "args": ["-c", "sleep 30; printf should-not-complete"],
            "timeout_ms": 60_000
        });

        let mut slow_two = slow.clone();
        slow_two["id"] = json!("slow-2");
        let started = call_tool_async(
            ctx.clone(),
            "exec_many".into(),
            json!({
                "operation_id": operation_id,
                "yield-time_ms": 10,
                "mode": "sequential",
                "commands": [slow, slow_two]
            }),
        )
        .await;
        assert_eq!(started["graph_completed"], false, "{started}");

        let status_started = std::time::Instant::now();
        let status = call_tool_async(
            ctx.clone(),
            "exec_many".into(),
            json!({"operation_id": operation_id, "action": "status"}),
        )
        .await;
        assert!(
            status_started.elapsed() < std::time::Duration::from_millis(1_000),
            "{status}"
        );
        assert_eq!(status["ok"], true, "{status}");
        assert_eq!(status["control_ok"], true, "{status}");
        assert_eq!(status["graph_status"], "running", "{status}");

        let cancelled = call_tool_async(
            ctx.clone(),
            "exec_many".into(),
            json!({
                "operation_id": operation_id,
                "action": "cancel",
                "yield-time_ms": 5_000,
                "reason": "regression test"
            }),
        )
        .await;
        assert_eq!(cancelled["ok"], true, "{cancelled}");
        assert_eq!(cancelled["control_ok"], true, "{cancelled}");
        assert_eq!(cancelled["cancel_requested"], true, "{cancelled}");
        assert_eq!(cancelled["graph_completed"], true, "{cancelled}");
        assert_eq!(cancelled["graph_status"], "cancelled", "{cancelled}");

        let sessions =
            crate::tools::session::list_sessions(&ctx.sessions, &json!({})).expect("list sessions");
        assert_eq!(sessions["count"], 1, "{sessions}");
        let session = sessions["sessions"]
            .as_array()
            .and_then(|items| items.first())
            .expect("retained child session");
        assert_eq!(session["process_still_running"], false, "{sessions}");

        let forgotten = call_tool_async(
            ctx.clone(),
            "exec_many".into(),
            json!({"operation_id": operation_id, "action": "forget"}),
        )
        .await;
        assert_eq!(forgotten["ok"], true, "{forgotten}");
        assert_eq!(forgotten["forgotten"], true, "{forgotten}");

        let missing = call_tool_async(
            ctx,
            "exec_many".into(),
            json!({"operation_id": operation_id, "action": "status"}),
        )
        .await;
        assert_eq!(missing["ok"], false, "{missing}");
        assert_eq!(
            missing["error"]["code"], "COMMAND_GRAPH_OPERATION_NOT_FOUND",
            "{missing}"
        );
    }
    #[tokio::test]
    #[serial_test::serial(process_runtime)]
    async fn exec_many_parallel_runs_independent_commands_concurrently() {
        let workspace = tempfile::tempdir().expect("workspace tempdir");
        let harness = tempfile::tempdir().expect("harness tempdir");
        let ctx = Arc::new(
            ToolContext::for_test(workspace.path().to_path_buf(), harness.path().to_path_buf())
                .expect("tool context"),
        );
        #[cfg(windows)]
        let sleep = json!({"program": "powershell", "args": ["-NoProfile", "-Command", "Start-Sleep -Milliseconds 1000"]});
        #[cfg(unix)]
        let sleep = json!({"program": "sh", "args": ["-c", "sleep 1"]});

        let result = call_tool_async(
            ctx,
            "exec_many".into(),
            json!({
                "mode": "parallel",
                "max_parallel": 2,
                "stop_on_error": false,
                "commands": [sleep.clone(), sleep]
            }),
        )
        .await;

        assert_eq!(result["all_commands_ok"], true, "{result}");
        assert_eq!(result["mode"], "parallel");
        let observation = result["parallelism_observations"]
            .as_array()
            .and_then(|items| items.first())
            .expect("parallel overlap observation");
        assert_eq!(observation["same_lock_group"], false, "{result}");
        assert_eq!(observation["outcome"], "success", "{result}");
        assert!(
            observation["overlap_ms"].as_u64().unwrap_or(0) >= 500,
            "parallel commands did not overlap enough: {result}"
        );
    }

    #[tokio::test]
    #[serial_test::serial(process_runtime)]
    async fn exec_many_lock_group_serializes_shared_resources() {
        let workspace = tempfile::tempdir().expect("workspace tempdir");
        let harness = tempfile::tempdir().expect("harness tempdir");
        let ctx = Arc::new(
            ToolContext::for_test(workspace.path().to_path_buf(), harness.path().to_path_buf())
                .expect("tool context"),
        );
        #[cfg(windows)]
        let sleep = json!({"program": "powershell", "args": ["-NoProfile", "-Command", "Start-Sleep -Milliseconds 500"], "lock_group": "cargo-target"});
        #[cfg(unix)]
        let sleep =
            json!({"program": "sh", "args": ["-c", "sleep 0.5"], "lock_group": "cargo-target"});

        let started = std::time::Instant::now();
        let result = call_tool_async(
            ctx,
            "exec_many".into(),
            json!({
                "mode": "parallel",
                "max_parallel": 2,
                "stop_on_error": false,
                "commands": [sleep.clone(), sleep]
            }),
        )
        .await;

        assert_eq!(result["all_commands_ok"], true, "{result}");
        assert!(
            started.elapsed() >= std::time::Duration::from_millis(900),
            "{result}"
        );
        let max_resource_wait = result["results"]
            .as_array()
            .expect("batch results")
            .iter()
            .filter_map(|item| item["resource_lock_wait_ms"].as_u64())
            .max()
            .unwrap_or(0);
        assert!(max_resource_wait >= 400, "{result}");
    }

    #[tokio::test]
    #[serial_test::serial(process_runtime)]
    async fn exec_many_io_heavy_resource_class_serializes_host_io() {
        let workspace = tempfile::tempdir().expect("workspace tempdir");
        let harness = tempfile::tempdir().expect("harness tempdir");
        let ctx = Arc::new(
            ToolContext::for_test(workspace.path().to_path_buf(), harness.path().to_path_buf())
                .expect("tool context"),
        );
        #[cfg(windows)]
        let sleep = json!({"program": "powershell", "args": ["-NoProfile", "-Command", "Start-Sleep -Milliseconds 500"], "resource_class": "io_heavy"});
        #[cfg(unix)]
        let sleep =
            json!({"program": "sh", "args": ["-c", "sleep 0.5"], "resource_class": "io_heavy"});

        let started = std::time::Instant::now();
        let result = call_tool_async(
            ctx,
            "exec_many".into(),
            json!({
                "mode": "parallel",
                "max_parallel": 2,
                "stop_on_error": false,
                "commands": [sleep.clone(), sleep]
            }),
        )
        .await;

        assert_eq!(result["all_commands_ok"], true, "{result}");
        assert!(
            started.elapsed() >= std::time::Duration::from_millis(900),
            "{result}"
        );
        let results = result["results"].as_array().expect("batch results");
        assert!(
            results
                .iter()
                .all(|item| item["result"]["resource_class"] == "io_heavy"),
            "{result}"
        );
        assert!(
            results.iter().any(|item| {
                item["result"]["io_heavy_admission_wait_ms"]
                    .as_u64()
                    .unwrap_or(0)
                    >= 400
            }),
            "{result}"
        );
    }

    #[tokio::test]
    #[serial_test::serial(process_runtime)]
    async fn exec_many_dag_skips_failed_dependencies_but_runs_independent_work() {
        let workspace = tempfile::tempdir().expect("workspace tempdir");
        let harness = tempfile::tempdir().expect("harness tempdir");
        let ctx = Arc::new(
            ToolContext::for_test(workspace.path().to_path_buf(), harness.path().to_path_buf())
                .expect("tool context"),
        );

        let result = call_tool_async(
            ctx,
            "exec_many".into(),
            json!({
                "mode": "dag",
                "max_parallel": 4,
                "stop_on_error": false,
                "commands": [
                    {"id": "fail", "program": "coding-tools-command-that-does-not-exist"},
                    {"id": "blocked", "depends_on": ["fail"], "program": "cargo", "args": ["--version"]},
                    {"id": "independent", "program": "cargo", "args": ["--version"]}
                ]
            }),
        )
        .await;

        assert_eq!(result["successful_command_count"], 1, "{result}");
        assert_eq!(result["failed_command_count"], 1, "{result}");
        assert_eq!(result["skipped_command_count"], 1, "{result}");
        assert_eq!(result["failed_command_ids"], json!(["fail"]), "{result}");
        assert_eq!(
            result["skipped_command_ids"],
            json!(["blocked"]),
            "{result}"
        );
        assert_eq!(result["results"][1]["id"], "blocked");
        assert_eq!(result["results"][1]["skip_reason"], "dependency_failed");
    }

    #[tokio::test]
    #[serial_test::serial(process_runtime)]
    async fn exec_many_dag_run_if_runs_generic_finalizers_after_failure() {
        let workspace = tempfile::tempdir().expect("workspace tempdir");
        let harness = tempfile::tempdir().expect("harness tempdir");
        let ctx = Arc::new(
            ToolContext::for_test(workspace.path().to_path_buf(), harness.path().to_path_buf())
                .expect("tool context"),
        );

        let result = call_tool_async(
            ctx,
            "exec_many".into(),
            json!({
                "mode": "dag",
                "max_parallel": 4,
                "stop_on_error": true,
                "commands": [
                    {"id": "fail", "program": "coding-tools-command-that-does-not-exist"},
                    {"id": "cleanup", "depends_on": ["fail"], "run_if": "always", "program": "git", "args": ["--version"]},
                    {"id": "on-failure", "depends_on": ["fail"], "run_if": "failure", "program": "git", "args": ["--version"]},
                    {"id": "success-only", "depends_on": ["fail"], "program": "git", "args": ["--version"]}
                ]
            }),
        )
        .await;

        assert_eq!(result["successful_command_count"], 2, "{result}");
        assert_eq!(result["failed_command_count"], 1, "{result}");
        assert_eq!(result["skipped_command_count"], 1, "{result}");
        assert_eq!(result["failed_command_ids"], json!(["fail"]), "{result}");
        assert_eq!(result["results"][1]["id"], "cleanup", "{result}");
        assert_eq!(result["results"][1]["command_ok"], true, "{result}");
        assert_eq!(result["results"][1]["run_if"], "always", "{result}");
        assert_eq!(result["results"][2]["id"], "on-failure", "{result}");
        assert_eq!(result["results"][2]["command_ok"], true, "{result}");
        assert_eq!(result["results"][2]["run_if"], "failure", "{result}");
        assert_eq!(
            result["results"][3]["skip_reason"], "dependency_failed",
            "{result}"
        );
    }

    #[tokio::test]
    #[serial_test::serial(process_runtime)]
    async fn exec_many_dag_failure_branch_is_neutral_when_dependencies_succeed() {
        let workspace = tempfile::tempdir().expect("workspace tempdir");
        let harness = tempfile::tempdir().expect("harness tempdir");
        let ctx = Arc::new(
            ToolContext::for_test(workspace.path().to_path_buf(), harness.path().to_path_buf())
                .expect("tool context"),
        );

        let result = call_tool_async(
            ctx,
            "exec_many".into(),
            json!({
                "mode": "dag",
                "commands": [
                    {"id": "pass", "program": "git", "args": ["--version"]},
                    {"id": "cleanup", "depends_on": ["pass"], "run_if": "always", "program": "git", "args": ["--version"]},
                    {"id": "on-failure", "depends_on": ["pass"], "run_if": "failure", "program": "git", "args": ["--version"]}
                ]
            }),
        )
        .await;

        assert_eq!(result["command_ok"], true, "{result}");
        assert_eq!(result["successful_command_count"], 2, "{result}");
        assert_eq!(result["skipped_command_count"], 1, "{result}");
        assert_eq!(result["conditional_skipped_command_count"], 1, "{result}");
        assert_eq!(
            result["conditional_skipped_command_ids"],
            json!(["on-failure"]),
            "{result}"
        );
        assert_eq!(
            result["results"][2]["skip_reason"], "run_condition_not_met",
            "{result}"
        );
    }

    #[tokio::test]
    #[serial_test::serial(process_runtime)]
    async fn exec_many_conditional_run_if_requires_dag_mode() {
        let workspace = tempfile::tempdir().expect("workspace tempdir");
        let harness = tempfile::tempdir().expect("harness tempdir");
        let ctx = Arc::new(
            ToolContext::for_test(workspace.path().to_path_buf(), harness.path().to_path_buf())
                .expect("tool context"),
        );

        let result = call_tool_async(
            ctx,
            "exec_many".into(),
            json!({
                "mode": "sequential",
                "commands": [
                    {"id": "first", "program": "git", "args": ["--version"]},
                    {"id": "cleanup", "depends_on": ["first"], "run_if": "always", "program": "git", "args": ["--version"]}
                ]
            }),
        )
        .await;

        assert_eq!(result["ok"], false, "{result}");
        assert!(
            result["error"]["message"]
                .as_str()
                .unwrap_or_default()
                .contains("requires dag mode"),
            "{result}"
        );
    }

    #[tokio::test]
    #[serial_test::serial(process_runtime)]
    async fn exec_command_redacts_sensitive_file_output_before_transport() {
        let workspace = tempfile::tempdir().expect("workspace tempdir");
        let harness = tempfile::tempdir().expect("harness tempdir");
        std::fs::write(
            workspace.path().join("profiles.json"),
            "bare-secret-without-label",
        )
        .expect("write sensitive fixture");
        let ctx = Arc::new(
            ToolContext::for_test(workspace.path().to_path_buf(), harness.path().to_path_buf())
                .expect("tool context"),
        );

        #[cfg(windows)]
        let command = "cmd /d /c type profiles.json";
        #[cfg(unix)]
        let command = "sh -c \"cat profiles.json\"";

        let result = call_tool_async(
            ctx,
            "exec_command".into(),
            json!({
                "cmd": command,
                "timeout_ms": 10_000,
                "yield_time_ms": 10_000,
                "output_mode": "tail"
            }),
        )
        .await;

        assert_eq!(result["command_ok"], true, "{result}");
        assert_eq!(result["stdout"], "[REDACTED]", "{result}");
        assert_eq!(result["sensitive_data_redacted"], true, "{result}");
        assert!(!result.to_string().contains("bare-secret-without-label"));
    }

    #[tokio::test]
    async fn format_files_plan_routes_without_modifying_workspace() {
        let workspace = tempfile::tempdir().expect("workspace tempdir");
        let harness = tempfile::tempdir().expect("harness tempdir");
        std::fs::write(workspace.path().join("data.json"), "{\"b\":2,\"a\":1}\n")
            .expect("write json fixture");
        let ctx = Arc::new(
            ToolContext::for_test(workspace.path().to_path_buf(), harness.path().to_path_buf())
                .expect("tool context"),
        );

        let result = call_tool_async(
            ctx,
            "format_files".into(),
            json!({"paths": ["data.json"], "mode": "plan"}),
        )
        .await;

        assert_eq!(result["ok"], true, "{result}");
        assert_eq!(result["status"], "planned", "{result}");
        assert_eq!(
            result["groups"][0]["adapter_id"], "builtin-json",
            "{result}"
        );
        assert_eq!(result["applied"], false, "{result}");
        assert_eq!(
            std::fs::read_to_string(workspace.path().join("data.json")).expect("read fixture"),
            "{\"b\":2,\"a\":1}\n"
        );
    }

    #[tokio::test]
    #[serial_test::serial(process_runtime)]
    async fn retained_exec_preserves_the_wait_command_next_action() {
        let workspace = tempfile::tempdir().expect("workspace tempdir");
        let harness = tempfile::tempdir().expect("harness tempdir");
        let ctx = Arc::new(
            ToolContext::for_test(workspace.path().to_path_buf(), harness.path().to_path_buf())
                .expect("tool context"),
        );

        #[cfg(windows)]
        let command = "powershell -NoProfile -Command \"Start-Sleep -Seconds 30\"";
        #[cfg(unix)]
        let command = "sh -c \"sleep 30\"";

        let result = call_tool_async(
            ctx.clone(),
            "exec_command".into(),
            json!({
                "cmd": command,
                "deduplicate": true,
                "timeout_ms": 60_000,
                "yield_time_ms": 0,
                "output_mode": "none"
            }),
        )
        .await;

        assert_eq!(result["process_still_running"], true, "{result}");
        assert_eq!(
            result["next_actions"][0]["tool"], "wait_command",
            "{result}"
        );
        assert_eq!(
            result["next_actions"][0]["arguments"]["session_id"], result["session_id"],
            "{result}"
        );
        assert_eq!(
            result["next_actions"][0]["arguments"]["timeout_ms"],
            crate::tools::session::WAIT_COMMAND_TRANSPORT_SAFE_MS,
            "{result}"
        );
        assert_eq!(
            result["next_actions"][0]["arguments"]["until"], "output_or_exit",
            "{result}"
        );

        let reattached = call_tool_async(
            ctx.clone(),
            "exec_command".into(),
            json!({
                "cmd": command,
                "deduplicate": true,
                "timeout_ms": 60_000,
                "yield_time_ms": 0,
                "output_mode": "none"
            }),
        )
        .await;
        assert_eq!(
            reattached["session_id"], result["session_id"],
            "{reattached}"
        );
        assert_eq!(reattached["deduplicated"], true, "{reattached}");
        assert_ne!(
            reattached["harness_operation_id"], result["harness_operation_id"],
            "{reattached}"
        );

        let harness_operation_ids = [
            result["harness_operation_id"]
                .as_str()
                .expect("harness operation id")
                .to_string(),
            reattached["harness_operation_id"]
                .as_str()
                .expect("reattached harness operation id")
                .to_string(),
        ];
        let session_id = result["session_id"].as_str().expect("session id");
        let killed = call_tool_async(
            ctx.clone(),
            "kill_session".into(),
            json!({"session_id": session_id, "wait_ms": 10_000}),
        )
        .await;
        assert_eq!(killed["killed"], true, "{killed}");

        let operations = ctx.harness.list_operations(0, 20).expect("operation log");
        for operation_id in &harness_operation_ids {
            let correlated = operations
                .iter()
                .filter(|operation| operation.id == *operation_id)
                .collect::<Vec<_>>();
            assert_eq!(
                correlated
                    .iter()
                    .map(|operation| operation.kind.as_str())
                    .collect::<Vec<_>>(),
                vec!["started", "failed"]
            );
        }
        let terminal = operations
            .iter()
            .find(|operation| {
                operation.id == harness_operation_ids[0] && operation.kind == "failed"
            })
            .expect("terminal operation");
        assert_eq!(terminal.result_summary["command_ok"], false);
        assert_eq!(terminal.result_summary["termination_reason"], "killed");
        assert!(terminal.result_summary.get("command").is_none());
        assert!(terminal.result_summary.get("stdout").is_none());
    }
}
