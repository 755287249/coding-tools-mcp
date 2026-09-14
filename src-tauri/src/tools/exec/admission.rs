use std::path::Path;
use std::time::Instant;

use serde_json::{json, Value};
use tokio::sync::OwnedMutexGuard;

use crate::tools::context::ToolContext;
use crate::tools::session::OutputOptions;
use crate::tools::workspace::WorkspaceError;

use super::backend::CommandExecutionBoundary;
use super::identity::{sha256_hex, ExecutionIdentity};
use super::result::{attach_session_capacity, merge_exec_result};
use super::spec::ExecSpec;

pub(super) enum OperationAdmission {
    Proceed {
        operation_guard: Option<OwnedMutexGuard<()>>,
        operation_lock_wait_ms: u128,
    },
    Reattached(Value),
}

pub(super) async fn admit_operation(
    ctx: &ToolContext,
    identity: &ExecutionIdentity,
    spec: &ExecSpec,
    cwd: &Path,
    output_options: OutputOptions,
    filesystem_scope: &str,
    boundary: &CommandExecutionBoundary,
) -> Result<OperationAdmission, WorkspaceError> {
    let operation_lock_started = Instant::now();
    let operation_guard = if let Some(operation_id) = identity.operation_id.as_deref() {
        let operation_lock_group = format!(
            "exec-operation:{}",
            &sha256_hex(operation_id.as_bytes())[..24]
        );
        Some(ctx.resource_lock(&operation_lock_group).lock_owned().await)
    } else {
        None
    };
    let operation_lock_wait_ms = operation_lock_started.elapsed().as_millis();

    if let Some(operation_id) = identity.operation_id.as_deref() {
        if let Some(session) = ctx.sessions.get_by_operation(operation_id) {
            let automatic_operation = operation_id.starts_with("auto:");
            let reuse_session = !automatic_operation || !session.is_finalized();
            if automatic_operation && !reuse_session {
                ctx.sessions.remove(&session.session_id);
            } else {
                if session.command_fingerprint() != Some(identity.command_fingerprint.as_str()) {
                    return Err(WorkspaceError::ToolDetails {
                        code: "OPERATION_ID_CONFLICT",
                        message: "The operation_id is already associated with a different command."
                            .into(),
                        category: "validation",
                        retryable: false,
                        details: json!({
                            "operation_id": operation_id,
                            "requested_command_fingerprint": identity.command_fingerprint,
                            "existing_command_fingerprint": session.command_fingerprint(),
                            "existing_session_id": session.session_id,
                            "suggestion": "Reuse the original command arguments or choose a new operation_id."
                        }),
                    });
                }
                session.touch_attachment();
                session.refresh_status().await;
                let keep_session = !session.is_finalized();
                let mut out = merge_exec_result(
                    session.snapshot_with_options(output_options),
                    session.started_at,
                    spec,
                    cwd,
                    keep_session,
                    None,
                );
                if let Some(object) = out.as_object_mut() {
                    object.insert("deduplicated".into(), Value::Bool(true));
                    object.insert(
                        "attached_to_session_id".into(),
                        Value::String(session.session_id.clone()),
                    );
                    object.insert(
                        "operation_lock_wait_ms".into(),
                        json!(operation_lock_wait_ms),
                    );
                    object.insert(
                        "filesystem_scope".into(),
                        Value::String(filesystem_scope.to_string()),
                    );
                }
                boundary.attach_result_metadata(&mut out, true, true);
                drop(operation_guard);
                attach_session_capacity(ctx, &mut out);
                return Ok(OperationAdmission::Reattached(out));
            }
        }
    }

    Ok(OperationAdmission::Proceed {
        operation_guard,
        operation_lock_wait_ms,
    })
}
