#[cfg(test)]
use std::path::Path;
#[cfg(windows)]
use std::path::PathBuf;
use std::time::{Duration, Instant};

use serde_json::{json, Value};

use crate::tools::context::{RuntimeToolConfig, ToolContext};
use crate::tools::session::OutputOptions;
use crate::tools::workspace::{tool_ok, WorkspaceError};
use crate::workspace::SandboxConfig;

mod admission;
mod backend;
mod identity;
mod lifecycle;
mod native_diagnostic;
mod post_check;
mod request;
mod result;
mod runner;
mod spec;
mod test_runner;

use admission::{admit_operation, OperationAdmission};
use backend::CommandExecutionBoundary;
#[cfg(test)]
use identity::cargo_target_lock;
use identity::execution_identity;
use lifecycle::run_command;
use native_diagnostic::run_native_diagnostic;
use request::{resolve_exec_request, resolve_runtime_options};
use result::{attach_session_capacity, execution_failure_result};
#[cfg(test)]
use runner::command_for_program;
pub(crate) use runner::prepare_process_launch_spec;
#[cfg(all(test, windows))]
use runner::windows_batch_command_line;
#[cfg(test)]
use runner::{prepared_command, prepared_process_spec, process_spec_for_program, CommandIoMode};
pub use spec::powershell_environment;
#[cfg(test)]
use spec::{
    is_trusted_wsl_system_program, normalize_wsl_absolute_program_path, resolve_exec_spec,
    resolve_program, ExecSpec,
};
use spec::{resolution_target_for_sandbox, resolve_exec_spec_for_target, ExecResolutionTarget};
use test_runner::{preflight_test_runner_capabilities, test_workflow as planned_test_workflow};

fn attach_test_metadata(result: &mut Value, capability: &Option<Value>, workflow: &Option<Value>) {
    if let Some(object) = result.as_object_mut() {
        if let Some(capability) = capability.clone() {
            object.insert("test_runner_capability".into(), capability);
        }
        if let Some(workflow) = workflow.clone() {
            object.insert("test_workflow".into(), workflow);
        }
    }
}

#[cfg(windows)]
pub(crate) fn selected_powershell_program() -> Option<PathBuf> {
    spec::detected_powershell().map(|runtime| PathBuf::from(&runtime.program))
}

pub(crate) fn prewarm_sandbox_backend(
    ctx: &ToolContext,
    config: &SandboxConfig,
) -> Result<(), WorkspaceError> {
    if !config.enabled {
        return Ok(());
    }
    let boundary = CommandExecutionBoundary::from_config(config, &ctx.workspace)?;
    let _ = boundary.prepare_backend(config, ctx)?;
    Ok(())
}

pub fn exec_command(ctx: &ToolContext, args: &Value) -> Result<Value, WorkspaceError> {
    crate::task_runtime::block_on(exec_command_async(ctx, args))
}

pub async fn exec_command_async(ctx: &ToolContext, args: &Value) -> Result<Value, WorkspaceError> {
    let runtime_config = ctx.runtime_config();
    exec_command_async_with_runtime(ctx, args, runtime_config).await
}

pub async fn exec_command_async_with_runtime(
    ctx: &ToolContext,
    args: &Value,
    runtime_config: RuntimeToolConfig,
) -> Result<Value, WorkspaceError> {
    let request = resolve_exec_request(ctx, args, &runtime_config)?;
    let boundary = CommandExecutionBoundary::from_config(&runtime_config.sandbox, &ctx.workspace)?;
    let test_runner_capability = if runtime_config.sandbox.enabled {
        None
    } else {
        preflight_test_runner_capabilities(&request.spec, &request.workdir).await?
    };
    let test_workflow = planned_test_workflow(
        &request.spec,
        args.get("workdir")
            .or_else(|| args.get("cwd"))
            .and_then(Value::as_str),
    );
    if request.legacy_native
        && request.timeout_contract.execution_mode != "job"
        && boundary.allows_native_diagnostic()
    {
        if let Some(result) = run_native_diagnostic(ctx, &request.spec.display, &request.workdir)? {
            let mut result = result;
            if let Some(object) = result.as_object_mut() {
                object.insert(
                    "filesystem_scope".into(),
                    Value::String(request.filesystem_scope.clone()),
                );
                object.insert("transport_ok".into(), Value::Bool(true));
                object.insert("command_ok".into(), Value::Bool(true));
                object.insert("program".into(), json!(request.spec.program));
                object.insert("args".into(), json!(request.spec.args));
                object.insert("shell".into(), json!(request.spec.shell));
            }
            attach_test_metadata(&mut result, &test_runner_capability, &test_workflow);
            boundary.attach_result_metadata(&mut result, false, false);
            attach_session_capacity(ctx, &mut result);
            return Ok(tool_ok(result));
        }
    }
    let runtime_options = resolve_runtime_options(
        args,
        &request.timeout_contract,
        &runtime_config.policy.security_policy,
        &request.stdin_text,
    );
    let mut identity = execution_identity(
        args,
        &request.spec,
        &request.workdir,
        runtime_options.timeout_ms,
        runtime_options.tty,
        runtime_options.stdin_text,
        &request.post_checks,
    );
    identity.process_timeout = Some(request.timeout_contract.clone());
    let (operation_guard, operation_lock_wait_ms) = match admit_operation(
        ctx,
        &identity,
        &request.spec,
        &request.workdir,
        request.output_options,
        &request.filesystem_scope,
        &boundary,
    )
    .await?
    {
        OperationAdmission::Proceed {
            operation_guard,
            operation_lock_wait_ms,
        } => (operation_guard, operation_lock_wait_ms),
        OperationAdmission::Reattached(mut out) => {
            attach_test_metadata(&mut out, &test_runner_capability, &test_workflow);
            return Ok(tool_ok(out));
        }
    };

    let sandbox_prepare_started = runtime_config.sandbox.enabled.then(Instant::now);
    let backend = boundary.prepare_backend(&runtime_config.sandbox, ctx)?;
    let sandbox_prepare_ms =
        sandbox_prepare_started.map(|started_at| started_at.elapsed().as_millis());

    let result = run_command(
        ctx,
        backend,
        sandbox_prepare_ms,
        &request.spec,
        &request.workdir,
        Duration::from_millis(runtime_options.timeout_ms),
        Duration::from_millis(runtime_options.yield_ms),
        request.output_options,
        runtime_options.tty,
        runtime_options.stdin_text,
        request.post_checks,
        runtime_options.sensitive_output,
        identity,
        operation_lock_wait_ms,
        operation_guard,
    )
    .await;

    match result {
        Ok(mut out) => {
            if let Some(object) = out.as_object_mut() {
                object.insert(
                    "filesystem_scope".into(),
                    Value::String(request.filesystem_scope),
                );
            }
            attach_test_metadata(&mut out, &test_runner_capability, &test_workflow);
            boundary.attach_result_metadata(&mut out, true, true);
            attach_session_capacity(ctx, &mut out);
            Ok(tool_ok(out))
        }
        Err(error) => match execution_failure_result(&error, &request.spec, &request.workdir) {
            Some(mut result) => {
                attach_test_metadata(&mut result, &test_runner_capability, &test_workflow);
                boundary.attach_result_metadata(&mut result, false, false);
                attach_session_capacity(ctx, &mut result);
                Ok(tool_ok(result))
            }
            None => Err(error),
        },
    }
}

pub fn exec_health_check(ctx: &ToolContext) -> Result<Value, WorkspaceError> {
    let start = Instant::now();
    let cwd = ctx.workspace.root().to_path_buf();
    let runtime = ctx.runtime_config();
    let sandbox_verification_required = runtime.sandbox.enabled;
    let expected_sandbox_backend = runtime.sandbox.backend.trim().to_string();
    let resolution_target = resolution_target_for_sandbox(&runtime.sandbox);
    let probe_args =
        if ctx.workspace.is_wsl() || resolution_target == ExecResolutionTarget::PortableSandbox {
            json!({
                "script": "printf exec-health; printf exec-health-stderr >&2",
                "shell": "sh",
                "confirm": true
            })
        } else {
            #[cfg(windows)]
            {
                json!({"cmd": r#"cmd.exe /d /c "echo exec-health & echo exec-health-stderr 1>&2""#})
            }
            #[cfg(not(windows))]
            {
                json!({"cmd": r#"sh -c "printf exec-health; printf exec-health-stderr >&2""#})
            }
        };
    let boundary = CommandExecutionBoundary::from_config(&runtime.sandbox, &ctx.workspace)?;
    let spec = resolve_exec_spec_for_target(
        &probe_args,
        &cwd,
        ctx.workspace.root(),
        &runtime.policy,
        resolution_target,
    )?;
    let sandbox_prepare_started = runtime.sandbox.enabled.then(Instant::now);
    let backend = boundary.prepare_backend(&runtime.sandbox, ctx)?;
    let sandbox_prepare_ms =
        sandbox_prepare_started.map(|started_at| started_at.elapsed().as_millis());
    let identity = execution_identity(&probe_args, &spec, &cwd, 5000, false, "", &[]);
    let result = crate::task_runtime::block_on(run_command(
        ctx,
        backend,
        sandbox_prepare_ms,
        &spec,
        &cwd,
        Duration::from_secs(5),
        Duration::from_secs(5),
        OutputOptions::tail(16_384),
        false,
        "",
        Vec::new(),
        false,
        identity,
        0,
        None,
    ));

    let mut response = json!({
        "worker": {"alive": true},
        "session_create": false,
        "command_run": false,
        "stdout_capture": false,
        "stderr_capture": false,
        "sandbox_verification": {
            "required": sandbox_verification_required,
            "verified": if sandbox_verification_required { Some(false) } else { None::<bool> },
            "backend": if sandbox_verification_required { Some(expected_sandbox_backend.as_str()) } else { None },
            "execution_boundary": Value::Null
        },
        "duration_ms": start.elapsed().as_millis(),
        "next_actions": []
    });

    match result {
        Ok(mut snapshot) => {
            boundary.attach_result_metadata(&mut snapshot, true, true);
            let session_created = snapshot.get("session_id").is_some();
            let command_run = snapshot.get("exit_code").and_then(Value::as_i64) == Some(0);
            let stdout_capture = snapshot
                .get("stdout")
                .and_then(Value::as_str)
                .is_some_and(|value| value.contains("exec-health"));
            let stderr_capture = snapshot
                .get("stderr")
                .and_then(Value::as_str)
                .is_some_and(|value| value.contains("exec-health-stderr"));
            let execution_boundary = snapshot.get("execution_boundary").and_then(Value::as_str);
            let sandbox_verified = !sandbox_verification_required
                || (snapshot.get("sandbox_enforced").and_then(Value::as_bool) == Some(true)
                    && snapshot.get("sandbox_backend").and_then(Value::as_str)
                        == Some(expected_sandbox_backend.as_str())
                    && execution_boundary == Some(expected_sandbox_backend.as_str()));
            let healthy = session_created
                && command_run
                && stdout_capture
                && stderr_capture
                && sandbox_verified;
            response["session_create"] = Value::Bool(session_created);
            response["command_run"] = Value::Bool(command_run);
            response["stdout_capture"] = Value::Bool(stdout_capture);
            response["stderr_capture"] = Value::Bool(stderr_capture);
            response["sandbox_verification"] = json!({
                "required": sandbox_verification_required,
                "verified": if sandbox_verification_required { Some(sandbox_verified) } else { None::<bool> },
                "backend": if sandbox_verification_required { Some(expected_sandbox_backend.as_str()) } else { None },
                "execution_boundary": execution_boundary
            });
            response["status"] = Value::String(if healthy { "success" } else { "error" }.into());
            response["summary"] = Value::String(if healthy {
                "exec worker、session、命令执行和 stdout/stderr 捕获均正常".into()
            } else {
                "exec health check 未通过，请查看 probe 结果".into()
            });
            response["probe"] = snapshot;
            if !healthy {
                response["next_actions"] = json!(["检查 exec worker 日志", "重启运行时"]);
            }
        }
        Err(error) => {
            response["status"] = Value::String("error".into());
            response["summary"] = Value::String("exec session 创建或探针执行失败".into());
            response["error"] = error.to_error_value();
            response["next_actions"] = json!(["检查 exec worker 日志", "重启运行时"]);
        }
    }
    response["duration_ms"] = json!(start.elapsed().as_millis());
    Ok(tool_ok(response))
}

#[cfg(test)]
#[path = "exec/secret_tests.rs"]
mod secret_tests;

#[cfg(test)]
#[allow(clippy::items_after_test_module)]
#[path = "exec/tests.rs"]
mod tests;
