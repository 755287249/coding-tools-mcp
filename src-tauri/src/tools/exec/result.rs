use std::path::Path;
use std::time::Instant;

use serde_json::{json, Value};

use crate::tools::context::ToolContext;
use crate::tools::process_start::{ProcessStartError, StartupDiagnostics};
use crate::tools::session::WAIT_COMMAND_TIMEOUT_DEFAULT_MS;
use crate::tools::workspace::WorkspaceError;

use super::spec::ExecSpec;

pub(super) fn attach_session_capacity(ctx: &ToolContext, value: &mut Value) {
    if let Some(object) = value.as_object_mut() {
        object.insert(
            "active_session_limit".into(),
            json!(ctx.sessions.active_session_limit()),
        );
        object.insert(
            "active_session_slots_available".into(),
            json!(ctx.sessions.active_slots_available()),
        );
    }
}

pub(super) fn process_start_error_json(error: &ProcessStartError) -> Value {
    match error {
        ProcessStartError::Spawn(source) => json!({
            "message": source.to_string(),
            "termination_reason": "spawn_failed",
            "recoverable": true
        }),
        ProcessStartError::LoaderInitialization {
            exit_code,
            diagnostics,
        } => json!({
            "message": error.to_string(),
            "termination_reason": "loader_initialization_failed",
            "recoverable": true,
            "process_exit_code": exit_code,
            "ntstatus": "0xc0000142",
            "startup": diagnostics.to_json()
        }),
    }
}

pub(super) fn process_start_workspace_error(error: ProcessStartError) -> WorkspaceError {
    let details = process_start_error_json(&error);
    match error {
        ProcessStartError::Spawn(source) => WorkspaceError::ToolDetails {
            code: "COMMAND_SPAWN_FAILED",
            message: format!("Failed to start command: {source}"),
            category: "runtime",
            retryable: true,
            details: json!({
                "termination_reason": "spawn_failed",
                "recoverable": true,
                "suggestion": "检查命令路径、权限和运行时环境后重试"
            }),
        },
        ProcessStartError::LoaderInitialization { .. } => WorkspaceError::ToolDetails {
            code: "COMMAND_START_TRANSIENT_FAILURE",
            message: "Windows could not initialize the child process after controlled retries."
                .into(),
            category: "runtime",
            retryable: true,
            details,
        },
    }
}

pub(super) fn execution_failure_result(
    error: &WorkspaceError,
    spec: &ExecSpec,
    cwd: &Path,
) -> Option<Value> {
    let code = match &error {
        WorkspaceError::Tool { code, .. } | WorkspaceError::ToolDetails { code, .. } => *code,
    };
    if !matches!(
        code,
        "COMMAND_REJECTED" | "COMMAND_SPAWN_FAILED" | "TIMEOUT"
    ) {
        return None;
    }

    let error_value = error.to_error_value();
    let details = error_value
        .get("details")
        .cloned()
        .unwrap_or_else(|| json!({}));
    let mut result = details.get("session").cloned().unwrap_or_else(|| {
        json!({
            "status": "spawn_failed",
            "termination_reason": "spawn_failed",
            "recoverable": error_value["retryable"].as_bool().unwrap_or(false),
            "process_exit_code": Value::Null,
            "exit_code": Value::Null,
            "request_timed_out": false,
            "process_timed_out": false,
            "process_still_running": false,
            "stdout": "",
            "stderr": "",
            "stdout_truncated": false,
            "stderr_truncated": false
        })
    });
    if let Some(object) = result.as_object_mut() {
        object.insert("command".into(), json!(spec.display));
        object.insert("program".into(), json!(spec.program));
        object.insert("args".into(), json!(spec.args));
        object.insert("shell".into(), json!(spec.shell));
        object.insert(
            "environment_keys".into(),
            json!(spec.env.iter().map(|(key, _)| key).collect::<Vec<_>>()),
        );
        object.insert("removed_environment_keys".into(), json!(spec.remove_env));
        object.insert("resolved_cwd".into(), json!(cwd.display().to_string()));
        object.insert("execution_mode".into(), json!("direct"));
        object.insert("filesystem_scope".into(), json!("workspace"));
        object.insert("sandbox_enforced".into(), Value::Bool(false));
        object.insert("execution_boundary".into(), json!("policy_only"));
        object.insert("child_process".into(), Value::Bool(true));
        object.insert("transport_ok".into(), Value::Bool(true));
        object.insert("command_ok".into(), Value::Bool(false));
        object.insert("error".into(), error_value);
        if code == "TIMEOUT" {
            object.insert("status".into(), json!("timed_out"));
            object.insert("termination_reason".into(), json!("process_timeout"));
            object.insert("process_timed_out".into(), Value::Bool(true));
        } else {
            object.insert("status".into(), json!("spawn_failed"));
            object.insert("termination_reason".into(), json!("spawn_failed"));
        }
    }
    Some(result)
}

pub(super) fn merge_exec_result(
    mut snapshot: Value,
    start: Instant,
    spec: &ExecSpec,
    cwd: &Path,
    keep_session: bool,
    startup_diagnostics: Option<&StartupDiagnostics>,
) -> Value {
    if let Some(obj) = snapshot.as_object_mut() {
        let duration_ms = start.elapsed().as_millis();
        obj.insert("command".into(), json!(spec.display));
        obj.insert("program".into(), json!(spec.program));
        obj.insert("args".into(), json!(spec.args));
        obj.insert("shell".into(), json!(spec.shell));
        obj.insert(
            "environment_keys".into(),
            json!(spec.env.iter().map(|(key, _)| key).collect::<Vec<_>>()),
        );
        obj.insert("removed_environment_keys".into(), json!(spec.remove_env));
        let process_tree_contained = obj
            .get("process_tree_contained")
            .and_then(Value::as_bool)
            .unwrap_or(false);
        obj.insert(
            "process_tree_control".into(),
            json!(if process_tree_contained {
                "windows_job_object"
            } else if cfg!(windows) {
                "new_process_group"
            } else {
                "child_process"
            }),
        );
        obj.insert("resolved_cwd".into(), json!(cwd.display().to_string()));
        obj.insert("duration_ms".into(), json!(duration_ms));
        obj.insert("elapsed_ms".into(), json!(duration_ms));
        obj.insert("transport_ok".into(), Value::Bool(true));
        obj.insert("execution_mode".into(), json!("direct"));
        obj.insert("warnings".into(), json!(Vec::<&str>::new()));
        if let Some(diagnostics) = startup_diagnostics {
            obj.insert("startup".into(), diagnostics.to_json());
        }
        if keep_session {
            let session_id = obj
                .get("session_id")
                .and_then(Value::as_str)
                .map(str::to_string);
            if let Some(session_id) = session_id {
                let cursor = obj.get("next_cursor").and_then(Value::as_u64).unwrap_or(0);
                obj.insert(
                    "next_actions".into(),
                    json!([{
                        "tool": "wait_command",
                        "arguments": {
                            "session_id": session_id,
                            "cursor": cursor,
                            "timeout_ms": WAIT_COMMAND_TIMEOUT_DEFAULT_MS,
                            "until": "output_or_exit",
                            "output_mode": "delta"
                        }
                    }]),
                );
                obj.insert(
                    "suggestion".into(),
                    json!("Call wait_command with next_actions[0].arguments to continue without polling duplicate output"),
                );
            }
        }
    }
    snapshot
}
