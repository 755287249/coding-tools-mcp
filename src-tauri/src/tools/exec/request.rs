use std::path::PathBuf;

use coding_tools_command_policy::resolved_command_timeout_ms as resolve_command_timeout_ms;
use serde_json::{json, Value};

use crate::mcp::command_kind;
use crate::secret::SecretStore;
use crate::tools::context::{RuntimeToolConfig, ToolContext};
use crate::tools::execution_timeout::{
    configured_job_timeout_max_ms, resolve_process_timeout, ProcessTimeoutContract,
};
use crate::tools::redaction::arguments_reference_sensitive_source;
use crate::tools::session::{OutputMode, OutputOptions};
use crate::tools::workspace::WorkspaceError;
use crate::tools::{ABSOLUTE_COMMAND_TIMEOUT_MAX_MS, DEFAULT_COMMAND_TIMEOUT_MAX_MS};
use crate::workspace::SecurityPolicy;

use super::spec::{
    resolution_target_for_sandbox, resolve_exec_spec_for_target, resolve_post_checks_for_target,
    ExecSpec, PostCheckSpec,
};

pub(super) struct ResolvedExecRequest {
    pub(super) workdir: PathBuf,
    pub(super) filesystem_scope: String,
    pub(super) spec: ExecSpec,
    pub(super) post_checks: Vec<PostCheckSpec>,
    pub(super) output_options: OutputOptions,
    pub(super) legacy_native: bool,
    pub(super) stdin_text: String,
    pub(super) timeout_contract: ProcessTimeoutContract,
}

pub(super) struct ExecRuntimeOptions<'a> {
    pub(super) timeout_ms: u64,
    pub(super) yield_ms: u64,
    pub(super) tty: bool,
    pub(super) stdin_text: &'a str,
    pub(super) sensitive_output: bool,
}

pub(super) fn resolve_exec_request(
    ctx: &ToolContext,
    args: &Value,
    runtime: &RuntimeToolConfig,
) -> Result<ResolvedExecRequest, WorkspaceError> {
    let workdir_raw = args
        .get("workdir")
        .or_else(|| args.get("cwd"))
        .and_then(Value::as_str)
        .unwrap_or(".");
    let workdir = if runtime.policy.security_policy.enforce_workspace_boundary {
        ctx.workspace.resolve_existing(workdir_raw)?
    } else {
        let requested = PathBuf::from(workdir_raw);
        let path = if requested.is_absolute() {
            requested
        } else {
            ctx.default_cwd_path().join(requested)
        };
        crate::tools::workspace::ResolvedPath {
            display: path.to_string_lossy().into_owned(),
            path: path.canonicalize().unwrap_or(path),
            existed: true,
        }
    };
    if !workdir.path.is_dir() {
        return Err(WorkspaceError::not_a_directory(
            "workdir is not a directory",
        ));
    }
    let filesystem_scope = args
        .get("filesystem_scope")
        .and_then(Value::as_str)
        .unwrap_or("workspace")
        .to_string();
    validate_child_process_scope(args, &runtime.policy.security_policy)?;
    let resolution_target = resolution_target_for_sandbox(&runtime.sandbox);
    let mut spec = resolve_exec_spec_for_target(
        args,
        &workdir.path,
        ctx.workspace.root(),
        &runtime.policy,
        resolution_target,
    )?;
    let timeout_contract = resolve_process_timeout(
        args,
        resolved_command_timeout_ms(args, &spec),
        ABSOLUTE_COMMAND_TIMEOUT_MAX_MS,
        configured_job_timeout_max_ms(),
    )?;
    let stdin_text = resolve_secret_inputs(ctx, args, &mut spec)?;
    let post_checks = resolve_post_checks_for_target(
        args,
        &workdir.path,
        ctx.workspace.root(),
        &runtime.policy,
        resolution_target,
    )?;
    let output_options = OutputOptions::from_args(args, OutputMode::Tail);
    let legacy_native = !ctx.workspace.is_wsl()
        && args.get("program").is_none()
        && spec.shell == "none"
        && spec.env.is_empty()
        && spec.remove_env.is_empty()
        && post_checks.is_empty();

    Ok(ResolvedExecRequest {
        workdir: workdir.path,
        filesystem_scope,
        spec,
        post_checks,
        output_options,
        legacy_native,
        stdin_text,
        timeout_contract,
    })
}

pub(super) fn resolve_runtime_options<'a>(
    args: &'a Value,
    timeout_contract: &ProcessTimeoutContract,
    security_policy: &SecurityPolicy,
    stdin_text: &'a str,
) -> ExecRuntimeOptions<'a> {
    ExecRuntimeOptions {
        timeout_ms: timeout_contract.effective_timeout_ms,
        yield_ms: args
            .get("yield_time_ms")
            .and_then(Value::as_u64)
            .unwrap_or(1000)
            .min(30_000),
        tty: args.get("tty").and_then(Value::as_bool).unwrap_or(false),
        stdin_text,
        sensitive_output: security_policy.withhold_sensitive_source_output
            && arguments_reference_sensitive_source(args),
    }
}

fn resolve_secret_inputs(
    ctx: &ToolContext,
    args: &Value,
    spec: &mut ExecSpec,
) -> Result<String, WorkspaceError> {
    if let Some(secret_env) = args.get("secret_env").and_then(Value::as_object) {
        for (name, reference) in secret_env {
            let reference = reference.as_str().ok_or_else(|| {
                WorkspaceError::invalid_argument(
                    "secret_env values must be secret reference strings",
                )
            })?;
            let value = resolve_workspace_secret(ctx, reference)?;
            spec.env.push((name.clone(), value));
        }
    }
    let direct_stdin = args.get("stdin").and_then(Value::as_str).unwrap_or("");
    let Some(reference) = args.get("stdin_secret").and_then(Value::as_str) else {
        return Ok(direct_stdin.to_string());
    };
    if !direct_stdin.is_empty() {
        return Err(WorkspaceError::invalid_argument(
            "stdin and stdin_secret cannot both be provided",
        ));
    }
    resolve_workspace_secret(ctx, reference)
}

fn resolve_workspace_secret(ctx: &ToolContext, reference: &str) -> Result<String, WorkspaceError> {
    let value =
        SecretStore::get(&ctx.profile_id, reference).map_err(|error| WorkspaceError::Tool {
            code: "SECRET_STORE_ERROR",
            message: format!("Unable to read workspace secret {reference}: {error}"),
            category: "runtime",
            retryable: false,
        })?;
    value.filter(|value| !value.is_empty()).ok_or_else(|| WorkspaceError::ToolDetails {
        code: "SECRET_NOT_FOUND",
        message: format!("Workspace secret is not configured: {reference}"),
        category: "validation",
        retryable: false,
        details: json!({
            "reference": reference,
            "suggestion": "Store the secret locally in the selected workspace, then retry using the same secret reference."
        }),
    })
}

fn resolved_command_timeout_ms(args: &Value, spec: &ExecSpec) -> u64 {
    resolve_command_timeout_ms(
        args.get("timeout_ms").and_then(Value::as_u64),
        command_kind(args),
        &spec.display,
        DEFAULT_COMMAND_TIMEOUT_MAX_MS,
        ABSOLUTE_COMMAND_TIMEOUT_MAX_MS,
    )
}

fn validate_child_process_scope(
    args: &Value,
    security_policy: &SecurityPolicy,
) -> Result<(), WorkspaceError> {
    let scope = args
        .get("filesystem_scope")
        .and_then(Value::as_str)
        .unwrap_or("workspace");
    match scope {
        "workspace" => Ok(()),
        "host" if !security_policy.enforce_workspace_boundary => Ok(()),
        "host" => Err(WorkspaceError::ToolDetails {
            code: "EXTERNAL_EXECUTION_NOT_ALLOWED",
            message: "exec_command 只允许在 Workspace 内执行，Workspace 外执行已禁用。".into(),
            category: "permission",
            retryable: false,
            details: json!({
                "stage": "policy",
                "filesystem_scope": "host",
                "sandbox_enforced": false,
                "recoverable": false,
                "suggestion": "将 filesystem_scope 设置为 workspace，并在当前 Workspace 内执行"
            }),
        }),
        _ => Err(WorkspaceError::invalid_argument(
            "filesystem_scope must be workspace",
        )),
    }
}
