use std::path::{Path, PathBuf};
use std::sync::OnceLock;
use std::time::Duration;

use serde_json::{json, Value};

use crate::tools::policy::PolicySettings;
use crate::tools::workspace::WorkspaceError;
use crate::tools::ABSOLUTE_COMMAND_TIMEOUT_MAX_MS;

#[derive(Clone, Debug)]
pub(super) struct ExecSpec {
    pub(super) display: String,
    pub(super) program: String,
    pub(super) args: Vec<String>,
    pub(super) shell: String,
    pub(super) env: Vec<(String, String)>,
    pub(super) remove_env: Vec<String>,
}

#[derive(Clone, Debug)]
pub(super) struct PowerShellRuntime {
    pub(super) program: String,
    pub(super) edition: &'static str,
}

#[derive(Clone, Debug)]
struct PowerShellDetection {
    selected: Option<PowerShellRuntime>,
    modern: Option<String>,
    legacy: Option<String>,
}

static POWERSHELL_DETECTION: OnceLock<PowerShellDetection> = OnceLock::new();

fn powershell_detection() -> &'static PowerShellDetection {
    POWERSHELL_DETECTION.get_or_init(|| {
        let modern = which::which("pwsh")
            .ok()
            .map(|path| path.to_string_lossy().into_owned());
        let legacy = which::which("powershell")
            .ok()
            .map(|path| path.to_string_lossy().into_owned());
        let selected = modern
            .as_ref()
            .map(|program| PowerShellRuntime {
                program: program.clone(),
                edition: "PowerShell Core",
            })
            .or_else(|| {
                legacy.as_ref().map(|program| PowerShellRuntime {
                    program: program.clone(),
                    edition: "Windows PowerShell",
                })
            });
        PowerShellDetection {
            selected,
            modern,
            legacy,
        }
    })
}

pub(super) fn detected_powershell() -> Option<&'static PowerShellRuntime> {
    powershell_detection().selected.as_ref()
}

pub fn powershell_environment() -> Value {
    let detection = powershell_detection();
    json!({
        "selected": detected_powershell().map(|runtime| runtime.program.as_str()),
        "edition": detected_powershell().map(|runtime| runtime.edition),
        "pwsh_available": detection.modern.is_some(),
        "pwsh_path": detection.modern.as_deref(),
        "windows_powershell_available": detection.legacy.is_some(),
        "windows_powershell_path": detection.legacy.as_deref(),
        "selection_policy": "pwsh_then_windows_powershell",
        "output_encoding": "utf-8"
    })
}

fn preferred_powershell_program() -> Result<String, WorkspaceError> {
    detected_powershell()
        .map(|runtime| runtime.program.clone())
        .ok_or_else(|| WorkspaceError::Tool {
            code: "COMMAND_REJECTED",
            message: "PowerShell 7 and Windows PowerShell are both unavailable.".into(),
            category: "runtime",
            retryable: false,
        })
}

fn powershell_utf8_script(script: &str) -> String {
    format!(
        "$__ctmcp_utf8=[System.Text.UTF8Encoding]::new($false); \
[Console]::InputEncoding = $__ctmcp_utf8; \
[Console]::OutputEncoding = $__ctmcp_utf8; \
$global:OutputEncoding = $__ctmcp_utf8; {script}"
    )
}

pub(super) fn powershell_script(script: &str) -> String {
    powershell_utf8_script(script)
}

pub(super) fn powershell_literal(value: &str) -> String {
    format!("'{}'", value.replace('\'', "''"))
}

fn normalize_powershell_args(args: &mut [String]) {
    if let Some(index) = args
        .iter()
        .position(|arg| arg.eq_ignore_ascii_case("-command") || arg.eq_ignore_ascii_case("-c"))
    {
        if let Some(script) = args.get_mut(index + 1) {
            *script = powershell_script(script);
        }
    }
}

fn is_powershell_name(value: &str) -> bool {
    Path::new(value)
        .file_stem()
        .and_then(|value| value.to_str())
        .is_some_and(|value| {
            value.eq_ignore_ascii_case("pwsh") || value.eq_ignore_ascii_case("powershell")
        })
}

#[derive(Clone, Debug)]
pub(super) struct PostCheckSpec {
    pub(super) name: String,
    pub(super) exec: ExecSpec,
    pub(super) expected_exit_code: i32,
    pub(super) timeout: Duration,
    pub(super) max_output_bytes: usize,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum ExecResolutionTarget {
    Host,
    PortableSandbox,
}

pub(super) fn resolution_target_for_sandbox(
    config: &crate::workspace::SandboxConfig,
) -> ExecResolutionTarget {
    if config.enabled && crate::tools::sandbox::uses_portable_command(&config.backend) {
        ExecResolutionTarget::PortableSandbox
    } else {
        ExecResolutionTarget::Host
    }
}

#[cfg(test)]
pub(super) fn resolve_exec_spec(
    arguments: &Value,
    cwd: &Path,
    workspace_root: &Path,
    policy: &PolicySettings,
) -> Result<ExecSpec, WorkspaceError> {
    resolve_exec_spec_for_target(
        arguments,
        cwd,
        workspace_root,
        policy,
        ExecResolutionTarget::Host,
    )
}

pub(super) fn resolve_exec_spec_for_target(
    arguments: &Value,
    cwd: &Path,
    workspace_root: &Path,
    policy: &PolicySettings,
    target: ExecResolutionTarget,
) -> Result<ExecSpec, WorkspaceError> {
    let wsl_workspace = crate::workspace::parse_wsl_path(workspace_root).is_some();
    let shell = arguments
        .get("shell")
        .and_then(Value::as_str)
        .unwrap_or("none")
        .to_ascii_lowercase();
    let env = arguments
        .get("env")
        .and_then(Value::as_object)
        .map(|values| {
            values
                .iter()
                .filter_map(|(key, value)| {
                    value.as_str().map(|value| (key.clone(), value.to_string()))
                })
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    let remove_env = arguments
        .get("remove_env")
        .and_then(Value::as_array)
        .map(|values| {
            values
                .iter()
                .filter_map(Value::as_str)
                .map(str::to_string)
                .collect()
        })
        .unwrap_or_default();

    if let Some(program) = arguments.get("program").and_then(Value::as_str) {
        if shell != "none" {
            return Err(WorkspaceError::invalid_argument(
                "program/args mode requires shell=none",
            ));
        }
        let mut args = arguments
            .get("args")
            .and_then(Value::as_array)
            .map(|values| {
                values
                    .iter()
                    .map(|value| {
                        value.as_str().map(str::to_string).ok_or_else(|| {
                            WorkspaceError::invalid_argument("args entries must be strings")
                        })
                    })
                    .collect::<Result<Vec<_>, _>>()
            })
            .transpose()?
            .unwrap_or_default();
        let mut program = resolve_program_for_target(program, cwd, workspace_root, policy, target)?;
        if target == ExecResolutionTarget::Host && !wsl_workspace && is_powershell_name(&program) {
            program = preferred_powershell_program()?;
            normalize_powershell_args(&mut args);
        }
        let display = std::iter::once(program.clone())
            .chain(args.iter().cloned())
            .collect::<Vec<_>>()
            .join(" ");
        return finalize_exec_spec(
            ExecSpec {
                display,
                program,
                args,
                shell,
                env,
                remove_env,
            },
            cwd,
        );
    }

    let cmd = match (
        arguments.get("cmd").and_then(Value::as_str),
        arguments.get("script").and_then(Value::as_str),
    ) {
        (Some(_), Some(_)) => {
            return Err(WorkspaceError::invalid_argument(
                "Provide only one of cmd or script",
            ))
        }
        (Some(cmd), None) => cmd,
        (None, Some(script)) if shell != "none" => script,
        (None, Some(_)) => {
            return Err(WorkspaceError::invalid_argument(
                "script requires shell=powershell, cmd, or sh",
            ))
        }
        (None, None) => {
            return Err(WorkspaceError::invalid_argument(
                "cmd, script, or program is required",
            ))
        }
    };
    if shell == "none" {
        let (mut program, mut args) = parse_and_resolve(cmd, cwd, workspace_root, policy, target)?;
        if target == ExecResolutionTarget::Host && !wsl_workspace && is_powershell_name(&program) {
            program = preferred_powershell_program()?;
            normalize_powershell_args(&mut args);
        }
        return finalize_exec_spec(
            ExecSpec {
                display: cmd.to_string(),
                program,
                args,
                shell,
                env,
                remove_env,
            },
            cwd,
        );
    }

    let (shell_program, shell_args): (&str, Vec<String>) = match shell.as_str() {
        "cmd" => {
            if target == ExecResolutionTarget::PortableSandbox {
                return Err(WorkspaceError::invalid_argument(
                    "shell=cmd is unavailable in Linux sandbox backends; use shell=sh",
                ));
            }
            if wsl_workspace {
                return Err(WorkspaceError::invalid_argument(
                    "shell=cmd is unavailable for WSL workspaces; use shell=sh",
                ));
            }
            #[cfg(windows)]
            {
                (
                    "cmd.exe",
                    vec!["/d".into(), "/s".into(), "/c".into(), cmd.into()],
                )
            }
            #[cfg(not(windows))]
            {
                return Err(WorkspaceError::invalid_argument(
                    "shell=cmd is only available on Windows",
                ));
            }
        }
        "powershell" => {
            if target == ExecResolutionTarget::PortableSandbox {
                return Err(WorkspaceError::invalid_argument(
                    "shell=powershell is unavailable by default in Linux sandbox backends; use shell=sh or program=pwsh when installed inside the sandbox",
                ));
            }
            if wsl_workspace {
                return Err(WorkspaceError::invalid_argument(
                    "shell=powershell is unavailable for WSL workspaces; use shell=sh",
                ));
            }
            let program = preferred_powershell_program()?;
            let args = vec![
                "-NoLogo".into(),
                "-NoProfile".into(),
                "-NonInteractive".into(),
                "-Command".into(),
                powershell_script(cmd),
            ];
            return finalize_exec_spec(
                ExecSpec {
                    display: cmd.to_string(),
                    program,
                    args,
                    shell,
                    env,
                    remove_env,
                },
                cwd,
            );
        }
        "sh" => ("sh", vec!["-c".into(), cmd.into()]),
        _ => {
            return Err(WorkspaceError::invalid_argument(
                "shell must be none, cmd, powershell, or sh",
            ))
        }
    };
    let program = resolve_program_for_target(shell_program, cwd, workspace_root, policy, target)?;
    finalize_exec_spec(
        ExecSpec {
            display: cmd.to_string(),
            program,
            args: shell_args,
            shell,
            env,
            remove_env,
        },
        cwd,
    )
}

fn finalize_exec_spec(spec: ExecSpec, cwd: &Path) -> Result<ExecSpec, WorkspaceError> {
    validate_wsl_exec_paths(cwd, &spec)?;
    Ok(spec)
}

fn validate_wsl_exec_paths(cwd: &Path, spec: &ExecSpec) -> Result<(), WorkspaceError> {
    let Some(workspace_location) = crate::workspace::parse_wsl_path(cwd) else {
        return Ok(());
    };
    for (key, value) in &spec.env {
        if !valid_wsl_environment_key(key) || value.contains('\0') {
            return Err(WorkspaceError::ToolDetails {
                code: "WSL_ENVIRONMENT_INVALID",
                message: format!("env contains an invalid WSL environment entry: {key}"),
                category: "validation",
                retryable: false,
                details: json!({
                    "key": key,
                    "suggestion": "Use a non-empty environment name that does not start with '-' and contains neither '=' nor NUL."
                }),
            });
        }
    }
    for key in &spec.remove_env {
        if !valid_wsl_environment_key(key) {
            return Err(WorkspaceError::ToolDetails {
                code: "WSL_ENVIRONMENT_INVALID",
                message: format!("remove_env contains an invalid WSL environment name: {key}"),
                category: "validation",
                retryable: false,
                details: json!({
                    "key": key,
                    "suggestion": "Use a non-empty environment name that does not start with '-' and contains neither '=' nor NUL."
                }),
            });
        }
    }
    let values = std::iter::once(("program".to_string(), spec.program.as_str())).chain(
        spec.args
            .iter()
            .enumerate()
            .map(|(index, value)| (format!("args[{index}]"), value.as_str())),
    );
    for (position, value) in values {
        if let Some(path_location) = crate::workspace::parse_wsl_path(Path::new(value)) {
            if !path_location
                .distro
                .eq_ignore_ascii_case(&workspace_location.distro)
            {
                return Err(WorkspaceError::ToolDetails {
                    code: "WSL_CROSS_DISTRIBUTION_PATH",
                    message: format!(
                        "{position} references WSL distribution '{}' while the workspace runs in '{}'",
                        path_location.distro, workspace_location.distro
                    ),
                    category: "validation",
                    retryable: false,
                    details: json!({
                        "position": position,
                        "path": value,
                        "workspace_distro": workspace_location.distro,
                        "path_distro": path_location.distro,
                        "suggestion": "Copy the file into the workspace distribution or pass a Linux path available inside that distribution."
                    }),
                });
            }
            continue;
        }
        if looks_like_windows_host_path(value) {
            return Err(WorkspaceError::ToolDetails {
                code: "WSL_HOST_PATH_REQUIRES_TRANSLATION",
                message: format!(
                    "{position} uses a Windows host path that is not valid as a Linux command argument"
                ),
                category: "validation",
                retryable: true,
                details: json!({
                    "position": position,
                    "path": value,
                    "workspace_distro": workspace_location.distro,
                    "suggestion": "Use a workspace-relative path or the corresponding Linux mount path such as /mnt/c/..."
                }),
            });
        }
    }
    Ok(())
}

fn looks_like_windows_drive_path(value: &str) -> bool {
    let bytes = value.as_bytes();
    bytes.len() >= 3
        && bytes[0].is_ascii_alphabetic()
        && bytes[1] == b':'
        && matches!(bytes[2], b'\\' | b'/')
}

fn looks_like_windows_host_path(value: &str) -> bool {
    looks_like_windows_drive_path(value) || value.starts_with(r"\\")
}

fn valid_wsl_environment_key(key: &str) -> bool {
    !key.is_empty() && !key.starts_with('-') && !key.contains(['=', '\0'])
}

pub(super) fn resolve_post_checks_for_target(
    arguments: &Value,
    cwd: &Path,
    workspace_root: &Path,
    policy: &PolicySettings,
    target: ExecResolutionTarget,
) -> Result<Vec<PostCheckSpec>, WorkspaceError> {
    let Some(checks) = arguments.get("post_checks") else {
        return Ok(Vec::new());
    };
    let checks = checks
        .as_array()
        .ok_or_else(|| WorkspaceError::invalid_argument("post_checks must be an array"))?;
    if checks.len() > 16 {
        return Err(WorkspaceError::invalid_argument(
            "post_checks supports at most 16 checks",
        ));
    }

    checks
        .iter()
        .enumerate()
        .map(|(index, check)| {
            if check.get("job_timeout_ms").is_some() {
                return Err(WorkspaceError::invalid_argument(
                    "post_checks cannot use job_timeout_ms",
                ));
            }
            let timeout_contract = crate::tools::execution_timeout::resolve_process_timeout(
                check,
                30_000,
                ABSOLUTE_COMMAND_TIMEOUT_MAX_MS,
                0,
            )?;
            let exec = resolve_exec_spec_for_target(check, cwd, workspace_root, policy, target)?;
            let name = check
                .get("name")
                .and_then(Value::as_str)
                .filter(|value| !value.trim().is_empty())
                .map(str::to_string)
                .unwrap_or_else(|| format!("post-check-{}", index + 1));
            let expected_exit_code = check
                .get("expected_exit_code")
                .and_then(Value::as_i64)
                .unwrap_or(0)
                .clamp(i32::MIN as i64, i32::MAX as i64)
                as i32;
            let timeout = Duration::from_millis(timeout_contract.effective_timeout_ms);
            let max_output_bytes = check
                .get("max_output_bytes")
                .and_then(Value::as_u64)
                .unwrap_or(16_384)
                .clamp(1, 1_048_576) as usize;
            Ok(PostCheckSpec {
                name,
                exec,
                expected_exit_code,
                timeout,
                max_output_bytes,
            })
        })
        .collect()
}

fn parse_and_resolve(
    cmd: &str,
    cwd: &Path,
    workspace_root: &Path,
    policy: &PolicySettings,
    target: ExecResolutionTarget,
) -> Result<(String, Vec<String>), WorkspaceError> {
    let parts = shell_words::split(cmd)
        .map_err(|_| WorkspaceError::invalid_argument("Invalid command syntax"))?;
    if parts.is_empty() {
        return Err(WorkspaceError::invalid_argument("Empty command"));
    }

    let program = resolve_program_for_target(&parts[0], cwd, workspace_root, policy, target)?;
    Ok((program, parts[1..].to_vec()))
}

#[cfg(test)]
pub(super) fn resolve_program(
    raw: &str,
    cwd: &Path,
    workspace_root: &Path,
    policy: &PolicySettings,
) -> Result<String, WorkspaceError> {
    resolve_program_for_target(raw, cwd, workspace_root, policy, ExecResolutionTarget::Host)
}

fn resolve_program_for_target(
    raw: &str,
    cwd: &Path,
    workspace_root: &Path,
    policy: &PolicySettings,
    target: ExecResolutionTarget,
) -> Result<String, WorkspaceError> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return Err(WorkspaceError::invalid_argument("Empty program"));
    }

    let wsl_workspace = crate::workspace::parse_wsl_path(workspace_root).is_some();
    if wsl_workspace && trimmed.starts_with('/') {
        return resolve_wsl_absolute_program(trimmed, workspace_root, policy);
    }
    let explicit_path = trimmed.contains(['/', '\\']);
    let candidate = if Path::new(trimmed).is_absolute() {
        Path::new(trimmed).to_path_buf()
    } else {
        cwd.join(trimmed)
    };
    if candidate.is_file() {
        let resolved = candidate.canonicalize().map_err(|_| WorkspaceError::Tool {
            code: "COMMAND_REJECTED",
            message: format!("Program not found: {trimmed}"),
            category: "runtime",
            retryable: false,
        })?;
        let canonical_workspace =
            workspace_root
                .canonicalize()
                .map_err(|_| WorkspaceError::Tool {
                    code: "COMMAND_REJECTED",
                    message: "Workspace root is unavailable".into(),
                    category: "runtime",
                    retryable: true,
                })?;
        if policy.security_policy.enforce_workspace_boundary
            && !resolved.starts_with(&canonical_workspace)
        {
            return Err(WorkspaceError::Tool {
                code: "EXECUTABLE_OUTSIDE_WORKSPACE",
                message: format!("Workspace 外可执行文件被拒绝: {trimmed}"),
                category: "security",
                retryable: false,
            });
        }
        if workspace_local_program_allowed(&resolved, policy) {
            if wsl_workspace {
                return Ok(trimmed.replace('\\', "/"));
            }
            return Ok(resolved.to_string_lossy().into_owned());
        }
        return Err(WorkspaceError::Tool {
            code: "COMMAND_REJECTED",
            message: format!("Workspace 本地入口未获允许: {trimmed}"),
            category: "policy",
            retryable: false,
        });
    }

    if explicit_path {
        return Err(WorkspaceError::Tool {
            code: "COMMAND_REJECTED",
            message: format!("Program not found: {trimmed}"),
            category: "runtime",
            retryable: false,
        });
    }

    if wsl_workspace {
        // The policy layer already validates the inner executable name against
        // allowed_commands. Availability is determined by the target distro,
        // not by the Windows PATH used by this desktop process.
        return Ok(trimmed.to_string());
    }

    if target == ExecResolutionTarget::PortableSandbox {
        return Ok(trimmed.to_string());
    }

    which::which(trimmed)
        .map(|p| p.to_string_lossy().into_owned())
        .map_err(|_| WorkspaceError::Tool {
            code: "COMMAND_REJECTED",
            message: format!("Program not found on PATH: {trimmed}"),
            category: "runtime",
            retryable: false,
        })
}

fn resolve_wsl_absolute_program(
    raw: &str,
    workspace_root: &Path,
    policy: &PolicySettings,
) -> Result<String, WorkspaceError> {
    let normalized =
        normalize_wsl_absolute_program_path(raw).ok_or_else(|| WorkspaceError::Tool {
            code: "EXECUTABLE_OUTSIDE_WORKSPACE",
            message: format!("Workspace 外可执行文件被拒绝: {raw}"),
            category: "security",
            retryable: false,
        })?;
    let location = crate::workspace::parse_wsl_path(workspace_root)
        .ok_or_else(|| WorkspaceError::invalid_argument("WSL workspace location is unavailable"))?;
    let host_candidate = PathBuf::from(crate::workspace::wsl_unc_path(
        &location.distro,
        &normalized,
    ));

    if host_candidate.is_file() {
        if let (Ok(resolved), Ok(canonical_workspace)) =
            (host_candidate.canonicalize(), workspace_root.canonicalize())
        {
            if !policy.security_policy.enforce_workspace_boundary
                || resolved.starts_with(&canonical_workspace)
            {
                if workspace_local_program_allowed(&resolved, policy) {
                    return Ok(normalized);
                }
                return Err(WorkspaceError::Tool {
                    code: "COMMAND_REJECTED",
                    message: format!("Workspace 本地入口未获允许: {raw}"),
                    category: "policy",
                    retryable: false,
                });
            }
        }
    }

    let base_name = normalized.rsplit('/').next().unwrap_or(&normalized);
    let stem = base_name.strip_suffix(".exe").unwrap_or(base_name);
    if policy.allowed_commands.contains(stem) && is_trusted_wsl_system_program(&normalized) {
        return Ok(normalized);
    }

    Err(WorkspaceError::Tool {
        code: "EXECUTABLE_OUTSIDE_WORKSPACE",
        message: format!("Workspace 外可执行文件被拒绝: {raw}"),
        category: "security",
        retryable: false,
    })
}

pub(super) fn normalize_wsl_absolute_program_path(raw: &str) -> Option<String> {
    if !raw.starts_with('/') || raw.contains(['\\', '\0']) || raw.chars().any(char::is_control) {
        return None;
    }
    let mut segments = Vec::new();
    for segment in raw.split('/') {
        match segment {
            "" | "." => {}
            ".." => {
                segments.pop()?;
            }
            value => segments.push(value),
        }
    }
    Some(if segments.is_empty() {
        "/".to_string()
    } else {
        format!("/{}", segments.join("/"))
    })
}

pub(super) fn is_trusted_wsl_system_program(path: &str) -> bool {
    let Some((parent, name)) = path.rsplit_once('/') else {
        return false;
    };
    !name.is_empty()
        && matches!(
            parent,
            "/bin"
                | "/sbin"
                | "/usr/bin"
                | "/usr/sbin"
                | "/usr/local/bin"
                | "/usr/local/sbin"
                | "/snap/bin"
        )
}

fn workspace_local_program_allowed(resolved: &Path, policy: &PolicySettings) -> bool {
    let extension = resolved
        .extension()
        .and_then(|value| value.to_str())
        .map(|value| format!(".{}", value.to_ascii_lowercase()))
        .unwrap_or_default();
    !policy.security_policy.enforce_command_allowlist
        || (policy.workspace_local_entries
            && (extension.is_empty() || policy.workspace_script_extensions.contains(&extension)))
}
