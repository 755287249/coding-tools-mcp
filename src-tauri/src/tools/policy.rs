use std::collections::HashSet;
use std::path::{Component, Path};

use serde_json::Value;

use crate::tools::workspace::Workspace;
use crate::workspace::{ActionsConfig, SecurityPolicy};

use super::registry::{is_allowed_tool, is_mcp_only_tool};
use super::ABSOLUTE_COMMAND_TIMEOUT_MAX_MS;

static NETWORK_COMMAND_PATTERN: std::sync::OnceLock<regex::Regex> = std::sync::OnceLock::new();
static DANGEROUS_COMMAND_PATTERN: std::sync::OnceLock<regex::Regex> = std::sync::OnceLock::new();
static INTERPRETER_MUTATION_PATTERN: std::sync::OnceLock<regex::Regex> = std::sync::OnceLock::new();

const BASIC_READ_ONLY_COMMANDS: &[&str] = &[
    "pwd", "ls", "dir", "cat", "head", "tail", "grep", "find", "which", "echo",
];

const DEFAULT_ALLOWED_COMMANDS: &[&str] = &[
    "pytest",
    "python",
    "python3",
    "npm",
    "npx",
    "node",
    "pnpm",
    "yarn",
    "make",
    "mvn",
    "mvnw",
    "gradle",
    "gradlew",
    "cargo",
    "go",
    "ruff",
    "mypy",
    "eslint",
    "tsc",
    "msbuild",
    "dotnet",
    "deno",
    "bun",
    "ruby",
    "java",
    "javac",
    "cmake",
    "clang",
    "gcc",
    "g++",
    "git",
    "cmd",
    "powershell",
    "pwsh",
    "sh",
];

#[derive(Debug, Clone)]
pub struct PolicySettings {
    pub allowed_commands: HashSet<String>,
    pub workspace_local_entries: bool,
    pub workspace_script_extensions: HashSet<String>,
    pub max_patch_bytes: usize,
    pub permission_mode: String,
    pub security_policy: SecurityPolicy,
    pub explicit_security_policy: bool,
    pub sandbox_enabled: bool,
}

fn validate_edit_file(arguments: &Value, policy: &PolicySettings) -> Result<(), PolicyError> {
    let path = arguments
        .get("path")
        .and_then(Value::as_str)
        .ok_or_else(|| PolicyError("edit_file requires a path".into()))?;
    if path.trim().is_empty() {
        return Err(PolicyError("edit_file requires a non-empty path".into()));
    }
    let edits = arguments.get("edits").and_then(Value::as_array);
    let apply_proposal = arguments.get("apply_proposal").and_then(Value::as_object);
    match (edits, apply_proposal) {
        (Some(edits), None) if !edits.is_empty() => {}
        (None, Some(_)) => {}
        (Some(edits), Some(_)) if !edits.is_empty() => {}
        _ => {
            return Err(PolicyError(
                "edit_file requires non-empty edits or apply_proposal".into(),
            ));
        }
    }
    let size = serde_json::to_vec(arguments)
        .map_err(|_| PolicyError("edit_file arguments could not be serialized".into()))?
        .len();
    if policy.security_policy.enforce_resource_limits && size > policy.max_patch_bytes {
        return Err(PolicyError("Edit payload is too large".into()));
    }
    Ok(())
}

fn validate_edit(arguments: &Value, policy: &PolicySettings) -> Result<(), PolicyError> {
    let files = arguments
        .get("files")
        .and_then(Value::as_array)
        .ok_or_else(|| PolicyError("edit requires files".into()))?;
    if files.is_empty() || (policy.security_policy.enforce_resource_limits && files.len() > 100) {
        return Err(PolicyError("edit requires between 1 and 100 files".into()));
    }
    for (index, file) in files.iter().enumerate() {
        let object = file
            .as_object()
            .ok_or_else(|| PolicyError(format!("edit files[{index}] must be an object")))?;
        let path = object
            .get("path")
            .and_then(Value::as_str)
            .ok_or_else(|| PolicyError(format!("edit files[{index}].path is required")))?;
        if path.trim().is_empty() {
            return Err(PolicyError(format!(
                "edit files[{index}].path must not be empty"
            )));
        }
        if policy.security_policy.enforce_resource_limits
            && object
                .get("edits")
                .and_then(Value::as_array)
                .is_some_and(|edits| edits.len() > 100)
        {
            return Err(PolicyError(format!(
                "edit files[{index}].edits supports at most 100 operations"
            )));
        }
    }
    validate_bounded_mutation(arguments, policy, "edit")
}

fn validate_bounded_mutation(
    arguments: &Value,
    policy: &PolicySettings,
    tool_name: &str,
) -> Result<(), PolicyError> {
    let size = serde_json::to_vec(arguments)
        .map_err(|_| PolicyError(format!("{tool_name} arguments could not be serialized")))?
        .len();
    if policy.security_policy.enforce_resource_limits
        && size > policy.max_patch_bytes.saturating_mul(4)
    {
        return Err(PolicyError(format!("{tool_name} payload is too large")));
    }
    Ok(())
}

fn validate_format_files(arguments: &Value, policy: &PolicySettings) -> Result<(), PolicyError> {
    validate_bounded_mutation(arguments, policy, "format_files")?;

    let mode = arguments
        .get("mode")
        .and_then(Value::as_str)
        .unwrap_or("plan");
    if !matches!(mode, "plan" | "check" | "apply") {
        return Err(PolicyError(
            "format_files mode must be plan, check, or apply".into(),
        ));
    }
    let scope = arguments
        .get("scope")
        .and_then(Value::as_str)
        .unwrap_or("files");
    if !matches!(scope, "files" | "changed" | "staged" | "project") {
        return Err(PolicyError(
            "format_files scope must be files, changed, staged, or project".into(),
        ));
    }

    if let Some(paths) = arguments.get("paths") {
        let paths = paths
            .as_array()
            .ok_or_else(|| PolicyError("format_files paths must be an array".into()))?;
        for path in paths {
            let path = path
                .as_str()
                .ok_or_else(|| PolicyError("format_files paths entries must be strings".into()))?;
            validate_format_path(path, policy.security_policy.enforce_workspace_boundary)?;
        }
    }
    if let Some(expected) = arguments.get("expected_sha256") {
        let expected = expected
            .as_object()
            .ok_or_else(|| PolicyError("format_files expected_sha256 must be an object".into()))?;
        for path in expected.keys() {
            validate_format_path(path, policy.security_policy.enforce_workspace_boundary)?;
        }
    }
    for key in ["include_patterns", "exclude_patterns"] {
        if let Some(patterns) = arguments.get(key) {
            let patterns = patterns
                .as_array()
                .ok_or_else(|| PolicyError(format!("format_files {key} must be an array")))?;
            for pattern in patterns {
                let pattern = pattern.as_str().ok_or_else(|| {
                    PolicyError(format!("format_files {key} entries must be strings"))
                })?;
                if policy.security_policy.enforce_workspace_boundary
                    && (Path::new(pattern).is_absolute()
                        || Path::new(pattern)
                            .components()
                            .any(|component| component == Component::ParentDir))
                {
                    return Err(PolicyError(format!(
                        "format_files {key} must stay inside the configured workspace"
                    )));
                }
            }
        }
    }

    let confirm = arguments
        .get("confirm")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let max_files = arguments
        .get("max_files")
        .and_then(Value::as_u64)
        .unwrap_or(500);
    if policy.security_policy.require_write_confirmation
        && mode == "apply"
        && scope == "project"
        && !confirm
    {
        return Err(PolicyError(
            "DANGEROUS_OPERATION_REQUIRES_CONFIRMATION: project-wide formatting requires confirm=true"
                .into(),
        ));
    }
    if policy.security_policy.require_write_confirmation
        && policy.security_policy.enforce_resource_limits
        && mode == "apply"
        && max_files > 2_000
        && !confirm
    {
        return Err(PolicyError(
            "DANGEROUS_OPERATION_REQUIRES_CONFIRMATION: formatting more than 2000 files requires confirm=true"
                .into(),
        ));
    }
    Ok(())
}

fn validate_format_path(path: &str, enforce_workspace_boundary: bool) -> Result<(), PolicyError> {
    if path.trim().is_empty() {
        return Err(PolicyError("format_files paths must not be empty".into()));
    }
    let path = Path::new(path);
    if enforce_workspace_boundary
        && (path.is_absolute()
            || path
                .components()
                .any(|component| component == Component::ParentDir))
    {
        return Err(PolicyError(
            "format_files paths must stay inside the configured workspace".into(),
        ));
    }
    Ok(())
}

impl Default for PolicySettings {
    fn default() -> Self {
        Self {
            allowed_commands: default_allowed_command_set(),
            workspace_local_entries: true,
            workspace_script_extensions: default_workspace_script_extension_set(),
            max_patch_bytes: 200_000,
            permission_mode: "trusted".into(),
            security_policy: SecurityPolicy::default(),
            explicit_security_policy: false,
            sandbox_enabled: false,
        }
    }
}

impl PolicySettings {
    pub fn from_runtime(runtime: &crate::workspace::RuntimeConfig) -> Self {
        let explicit = runtime.security_policy.is_some();
        let security_policy = runtime.security_policy.clone().unwrap_or_else(|| {
            SecurityPolicy::legacy(&runtime.permission_mode, &runtime.tool_profile)
        });
        Self {
            allowed_commands: merge_default_allowed_commands(&runtime.allowed_commands),
            workspace_local_entries: runtime.workspace_local_entries,
            workspace_script_extensions: parse_workspace_script_extensions(
                &runtime.workspace_script_extensions,
            ),
            max_patch_bytes: 200_000,
            permission_mode: security_policy.compatibility_permission_mode().into(),
            security_policy,
            explicit_security_policy: explicit,
            sandbox_enabled: runtime.sandbox.enabled,
        }
    }

    pub fn from_actions_config(actions: &ActionsConfig) -> Self {
        let security_policy = SecurityPolicy::legacy(&actions.permission_mode, "core");
        Self {
            allowed_commands: merge_default_allowed_commands(&actions.allowed_commands),
            workspace_local_entries: true,
            workspace_script_extensions: default_workspace_script_extension_set(),
            max_patch_bytes: actions.max_patch_bytes as usize,
            permission_mode: security_policy.compatibility_permission_mode().into(),
            security_policy,
            explicit_security_policy: false,
            sandbox_enabled: false,
        }
    }

    pub fn network_allowed(&self) -> bool {
        !self.security_policy.block_network_commands
    }

    pub fn skip_permission_gates(&self) -> bool {
        !self.security_policy.require_dangerous_confirmation
            && !self.security_policy.require_shell_confirmation
            && !self.security_policy.require_write_confirmation
    }
}

#[derive(Debug, thiserror::Error)]
#[error("{0}")]
pub struct PolicyError(pub String);

pub fn parse_allowed_commands(configured: &str) -> HashSet<String> {
    let trimmed = configured.trim();
    if trimmed.is_empty() {
        return default_allowed_command_set();
    }
    let mut commands: HashSet<String> = trimmed
        .split(',')
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string)
        .collect();
    // 基础诊断命令是工作区可用性的最低保障，不应因 Actions 配置遗漏而失效。
    commands.extend(BASIC_READ_ONLY_COMMANDS.iter().map(|s| s.to_string()));
    commands
}

pub fn parse_workspace_script_extensions(configured: &str) -> HashSet<String> {
    let mut extensions = configured
        .split(',')
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(|value| {
            if value.starts_with('.') {
                value.to_ascii_lowercase()
            } else {
                format!(".{}", value.to_ascii_lowercase())
            }
        })
        .collect::<HashSet<_>>();
    if extensions.is_empty() {
        extensions = default_workspace_script_extension_set();
    }
    extensions
}

fn default_allowed_command_set() -> HashSet<String> {
    DEFAULT_ALLOWED_COMMANDS
        .iter()
        .map(|s| s.to_string())
        .chain(BASIC_READ_ONLY_COMMANDS.iter().map(|s| s.to_string()))
        .collect()
}

fn merge_default_allowed_commands(configured: &str) -> HashSet<String> {
    let mut commands = default_allowed_command_set();
    commands.extend(parse_allowed_commands(configured));
    commands
}

fn default_workspace_script_extension_set() -> HashSet<String> {
    [".exe", ".bat", ".cmd", ".ps1"]
        .into_iter()
        .map(str::to_string)
        .collect()
}

pub fn validate_tool_arguments(
    tool_name: &str,
    arguments: &Value,
    policy: &PolicySettings,
) -> Result<(), PolicyError> {
    validate_tool_arguments_for_workspace(tool_name, arguments, policy, None)
}

pub fn validate_tool_arguments_for_workspace(
    tool_name: &str,
    arguments: &Value,
    policy: &PolicySettings,
    workspace: Option<&Workspace>,
) -> Result<(), PolicyError> {
    match tool_name {
        "exec_command" => validate_command_for_workspace(arguments, policy, workspace),
        "git_push" if !policy.network_allowed() => Err(PolicyError(
            "Network operation requires permission before git_push".into(),
        )),
        "apply_patch" | "patch_check" => validate_patch(arguments, policy),
        "edit" => validate_edit(arguments, policy),
        "edit_file" => validate_edit_file(arguments, policy),
        "edit_many" | "file_ops" => validate_bounded_mutation(arguments, policy, tool_name),
        "format_files" => validate_format_files(arguments, policy),
        _ => Ok(()),
    }
}

/// Actions OpenAPI 暴露层校验：仅限制「能否调用」，不参与执行逻辑。
pub fn validate_actions_exposure(tool_name: &str) -> Result<(), PolicyError> {
    if is_mcp_only_tool(tool_name) {
        return Err(PolicyError(format!(
            "Tool requires MCP conversation metadata and is not exposed through Actions: {tool_name}"
        )));
    }
    if is_allowed_tool(tool_name) {
        Ok(())
    } else {
        Err(PolicyError(format!("Tool is not exposed: {tool_name}")))
    }
}

pub fn validate_command(arguments: &Value, policy: &PolicySettings) -> Result<(), PolicyError> {
    validate_command_for_workspace(arguments, policy, None)
}

pub fn validate_command_for_workspace(
    arguments: &Value,
    policy: &PolicySettings,
    workspace: Option<&Workspace>,
) -> Result<(), PolicyError> {
    let cmd = arguments.get("cmd").and_then(Value::as_str);
    let script = arguments.get("script").and_then(Value::as_str);
    let structured_program = arguments.get("program").and_then(Value::as_str);
    let supplied = usize::from(cmd.is_some())
        + usize::from(script.is_some())
        + usize::from(structured_program.is_some());
    if supplied != 1 {
        return Err(PolicyError(
            "exec_command requires exactly one of cmd, script, or program".into(),
        ));
    }
    let shell = arguments
        .get("shell")
        .and_then(Value::as_str)
        .unwrap_or("none")
        .to_ascii_lowercase();
    if !matches!(shell.as_str(), "none" | "cmd" | "powershell" | "sh") {
        return Err(PolicyError(
            "shell must be none, cmd, powershell, or sh".into(),
        ));
    }
    if structured_program.is_some() && shell != "none" {
        return Err(PolicyError("program/args mode requires shell=none".into()));
    }
    if script.is_some() && shell == "none" {
        return Err(PolicyError(
            "script mode requires shell=powershell, cmd, or sh".into(),
        ));
    }
    if policy.security_policy.require_shell_confirmation
        && shell != "none"
        && !arguments
            .get("confirm")
            .and_then(Value::as_bool)
            .unwrap_or(false)
    {
        return Err(PolicyError(
            "DANGEROUS_OPERATION_REQUIRES_CONFIRMATION: explicit shell execution requires confirm=true"
                .into(),
        ));
    }

    let mut command = match (cmd, script, structured_program) {
        (Some(command), None, None) | (None, Some(command), None) => command.to_string(),
        (None, None, Some(program)) => {
            let mut parts = vec![program.to_string()];
            if let Some(args) = arguments.get("args").and_then(Value::as_array) {
                for arg in args {
                    let arg = arg
                        .as_str()
                        .ok_or_else(|| PolicyError("args entries must be strings".into()))?;
                    parts.push(arg.to_string());
                }
            }
            parts.join(" ")
        }
        _ => unreachable!(),
    };
    if command.trim().is_empty() {
        return Err(PolicyError(
            "exec_command requires a non-empty command".into(),
        ));
    }
    if policy.security_policy.enforce_resource_limits && command.len() > 64_000 {
        return Err(PolicyError("Command is too long".into()));
    }
    let filesystem_scope = arguments
        .get("filesystem_scope")
        .and_then(Value::as_str)
        .unwrap_or("workspace");
    if policy.security_policy.enforce_workspace_boundary && filesystem_scope != "workspace" {
        return Err(PolicyError(
            "EXTERNAL_EXECUTION_NOT_ALLOWED: exec_command 只允许在 Workspace 内执行".into(),
        ));
    }
    for key in ["workdir", "cwd"] {
        if let Some(workdir) = arguments.get(key).and_then(Value::as_str) {
            let path = Path::new(workdir);
            if policy.security_policy.enforce_workspace_boundary
                && (path.is_absolute()
                    || path.components().any(|part| part == Component::ParentDir))
            {
                return Err(PolicyError(
                    "workdir must stay inside the configured workspace".into(),
                ));
            }
        }
    }
    if shell == "none" && cmd.is_some_and(has_forbidden_shell_syntax) {
        return Err(PolicyError(
            "Shell chaining, redirection and expansion require an explicit shell mode".into(),
        ));
    }
    if policy.security_policy.protect_repository_metadata
        && (dangerous_command_pattern().is_match(&command)
            || interpreter_mutation_pattern().is_match(&command))
        && command_targets_protected_repository_asset(&command)
    {
        return Err(PolicyError(
            "PROTECTED_REPOSITORY_ASSET: 禁止删除或递归清空 .git/.github".into(),
        ));
    }
    if policy.security_policy.enforce_workspace_boundary
        && !policy.sandbox_enabled
        && interpreter_mutation_pattern().is_match(&command)
        && command_contains_external_path(&command)
    {
        return Err(PolicyError(
            "WORKSPACE_PATH_PROTECTED: workspace scope 禁止通过子进程写入 Workspace 外部路径"
                .into(),
        ));
    }
    if policy.security_policy.require_dangerous_confirmation
        && dangerous_command_pattern().is_match(&command)
        && !arguments
            .get("confirm")
            .and_then(Value::as_bool)
            .unwrap_or(false)
    {
        return Err(PolicyError(
            "DANGEROUS_OPERATION_REQUIRES_CONFIRMATION: dangerous command requires confirm=true"
                .into(),
        ));
    }
    if policy.security_policy.block_network_commands && network_command_pattern().is_match(&command)
    {
        return Err(PolicyError(
            "Network-looking commands are blocked in safe permission mode".into(),
        ));
    }

    let executable = if let Some(program) = structured_program {
        program.to_string()
    } else if shell != "none" {
        match shell.as_str() {
            "cmd" => "cmd".into(),
            "powershell" => "powershell".into(),
            "sh" => "sh".into(),
            _ => unreachable!(),
        }
    } else {
        let parts = shell_words::split(cmd.expect("cmd is required for shell=none"))
            .map_err(|_| PolicyError("Invalid command syntax".into()))?;
        if parts.is_empty() {
            return Err(PolicyError("Empty command".into()));
        }
        parts[0].clone()
    };
    let executable = executable.trim_start_matches("./");
    let base_name = executable.rsplit(['/', '\\']).next().unwrap_or(executable);
    let stem = base_name
        .strip_suffix(".exe")
        .or_else(|| base_name.strip_suffix(".cmd"))
        .or_else(|| base_name.strip_suffix(".bat"))
        .unwrap_or(base_name);

    let workspace_entry_candidate = workspace_local_entry_exists(workspace, arguments, executable)
        || executable.contains(['/', '\\'])
        || policy
            .workspace_script_extensions
            .iter()
            .any(|extension| base_name.to_ascii_lowercase().ends_with(extension));
    if policy.security_policy.enforce_command_allowlist
        && !(policy.allowed_commands.contains(stem)
            || (policy.workspace_local_entries && workspace_entry_candidate))
    {
        return Err(PolicyError(format!("Command is not allowlisted: {stem}")));
    }

    validate_environment_arguments(
        arguments,
        policy.security_policy.protect_environment_variables,
        policy.security_policy.enforce_resource_limits,
    )?;
    crate::tools::execution_timeout::resolve_process_timeout(
        arguments,
        30_000,
        ABSOLUTE_COMMAND_TIMEOUT_MAX_MS,
        crate::tools::execution_timeout::configured_job_timeout_max_ms(),
    )
    .map_err(|error| PolicyError(error.to_string()))?;
    if let Some(post_checks) = arguments.get("post_checks") {
        let post_checks = post_checks
            .as_array()
            .ok_or_else(|| PolicyError("post_checks must be an array".into()))?;
        if policy.security_policy.enforce_resource_limits && post_checks.len() > 16 {
            return Err(PolicyError("post_checks supports at most 16 checks".into()));
        }
        for (index, check) in post_checks.iter().enumerate() {
            let object = check
                .as_object()
                .ok_or_else(|| PolicyError(format!("post_checks[{index}] must be an object")))?;
            if object.contains_key("job_timeout_ms") {
                return Err(PolicyError(
                    "post_checks cannot use job_timeout_ms; use a separate managed command".into(),
                ));
            }
            if object.contains_key("post_checks") {
                return Err(PolicyError("nested post_checks are not allowed".into()));
            }
            validate_command_for_workspace(check, policy, workspace).map_err(|error| {
                PolicyError(format!("post_checks[{index}] rejected: {}", error.0))
            })?;
        }
    }
    command.clear();
    Ok(())
}

fn validate_environment_arguments(
    arguments: &Value,
    protect_environment_variables: bool,
    enforce_resource_limits: bool,
) -> Result<(), PolicyError> {
    let blocked = [
        "PATH",
        "PATHEXT",
        "COMSPEC",
        "LD_PRELOAD",
        "LD_LIBRARY_PATH",
        "DYLD_INSERT_LIBRARIES",
        "DYLD_LIBRARY_PATH",
    ];
    if let Some(env) = arguments.get("env") {
        let env = env
            .as_object()
            .ok_or_else(|| PolicyError("env must be an object of string values".into()))?;
        if enforce_resource_limits && env.len() > 64 {
            return Err(PolicyError("env contains too many entries".into()));
        }
        for (key, value) in env {
            validate_environment_key(key)?;
            if protect_environment_variables
                && blocked.iter().any(|item| item.eq_ignore_ascii_case(key))
            {
                return Err(PolicyError(format!(
                    "Environment variable is protected: {key}"
                )));
            }
            let value = value
                .as_str()
                .ok_or_else(|| PolicyError("env values must be strings".into()))?;
            if (enforce_resource_limits && value.len() > 4096) || value.contains('\0') {
                return Err(PolicyError(format!("Invalid environment value for {key}")));
            }
        }
    }
    if let Some(secret_env) = arguments.get("secret_env") {
        let secret_env = secret_env.as_object().ok_or_else(|| {
            PolicyError(
                "secret_env must be an object mapping environment names to secret references"
                    .into(),
            )
        })?;
        if enforce_resource_limits && secret_env.len() > 64 {
            return Err(PolicyError("secret_env contains too many entries".into()));
        }
        let direct_env = arguments.get("env").and_then(Value::as_object);
        let removed_env = arguments.get("remove_env").and_then(Value::as_array);
        for (key, reference) in secret_env {
            validate_environment_key(key)?;
            if protect_environment_variables
                && blocked.iter().any(|item| item.eq_ignore_ascii_case(key))
            {
                return Err(PolicyError(format!(
                    "Environment variable is protected: {key}"
                )));
            }
            let reference = reference.as_str().ok_or_else(|| {
                PolicyError("secret_env values must be secret reference strings".into())
            })?;
            validate_secret_reference(reference)?;
            let direct_conflict = direct_env.is_some_and(|values| {
                values
                    .keys()
                    .any(|candidate| candidate.eq_ignore_ascii_case(key))
            });
            let remove_conflict = removed_env.is_some_and(|values| {
                values
                    .iter()
                    .filter_map(Value::as_str)
                    .any(|candidate| candidate.eq_ignore_ascii_case(key))
            });
            if direct_conflict || remove_conflict {
                return Err(PolicyError(format!(
                    "Environment variable {key} cannot be configured by both env/remove_env and secret_env"
                )));
            }
        }
    }
    if let Some(reference) = arguments.get("stdin_secret") {
        let reference = reference
            .as_str()
            .ok_or_else(|| PolicyError("stdin_secret must be a secret reference string".into()))?;
        validate_secret_reference(reference)?;
        if arguments
            .get("stdin")
            .and_then(Value::as_str)
            .is_some_and(|value| !value.is_empty())
        {
            return Err(PolicyError(
                "stdin and stdin_secret cannot both be provided".into(),
            ));
        }
    }
    if let Some(remove_env) = arguments.get("remove_env") {
        let remove_env = remove_env
            .as_array()
            .ok_or_else(|| PolicyError("remove_env must be an array".into()))?;
        if enforce_resource_limits && remove_env.len() > 64 {
            return Err(PolicyError("remove_env contains too many entries".into()));
        }
        for key in remove_env {
            let key = key
                .as_str()
                .ok_or_else(|| PolicyError("remove_env entries must be strings".into()))?;
            validate_environment_key(key)?;
        }
    }
    Ok(())
}

fn validate_secret_reference(reference: &str) -> Result<(), PolicyError> {
    if reference.is_empty()
        || reference.len() > 128
        || !reference
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '.' | '_' | '-'))
    {
        return Err(PolicyError(
            "Secret references must use 1-128 letters, numbers, dot, underscore, or hyphen".into(),
        ));
    }
    Ok(())
}

fn validate_environment_key(key: &str) -> Result<(), PolicyError> {
    let mut chars = key.chars();
    let first = chars
        .next()
        .ok_or_else(|| PolicyError("Environment key must not be empty".into()))?;
    if !(first == '_' || first.is_ascii_alphabetic())
        || !chars.all(|ch| ch == '_' || ch.is_ascii_alphanumeric())
        || key.len() > 128
    {
        return Err(PolicyError(format!("Invalid environment key: {key}")));
    }
    Ok(())
}

fn workspace_local_entry_exists(
    workspace: Option<&Workspace>,
    arguments: &Value,
    executable: &str,
) -> bool {
    let Some(workspace) = workspace else {
        return false;
    };
    let workdir = arguments
        .get("workdir")
        .or_else(|| arguments.get("cwd"))
        .and_then(Value::as_str)
        .unwrap_or(".");
    let Ok(base) = workspace.resolve_existing(workdir) else {
        return false;
    };
    let candidate = if Path::new(executable).is_absolute() {
        Path::new(executable).to_path_buf()
    } else {
        base.path.join(executable)
    };
    candidate
        .canonicalize()
        .map(|path| path.is_file() && path.starts_with(workspace.root()))
        .unwrap_or(false)
}

pub fn validate_patch(arguments: &Value, policy: &PolicySettings) -> Result<(), PolicyError> {
    let patch = arguments
        .get("patch")
        .and_then(Value::as_str)
        .ok_or_else(|| PolicyError("apply_patch requires a patch".into()))?;
    if patch.trim().is_empty() {
        return Err(PolicyError("apply_patch requires a patch".into()));
    }

    if patch.len() > policy.max_patch_bytes {
        return Err(PolicyError("Patch is too large".into()));
    }

    Ok(())
}

fn has_forbidden_shell_syntax(command: &str) -> bool {
    if command.contains(['\r', '\n']) {
        return true;
    }

    let chars: Vec<char> = command.chars().collect();
    let mut quote = None;
    let mut escaped = false;
    let mut index = 0;
    while index < chars.len() {
        let ch = chars[index];
        if escaped {
            escaped = false;
            index += 1;
            continue;
        }

        match quote {
            Some('\'') => {
                if ch == '\'' {
                    quote = None;
                }
            }
            Some('"') => {
                if ch == '\\' {
                    escaped = true;
                } else if ch == '"' {
                    quote = None;
                }
            }
            Some(_) => {}
            None => {
                if ch == '\\' {
                    escaped = true;
                } else if ch == '\'' || ch == '"' {
                    quote = Some(ch);
                } else if matches!(ch, ';' | '&' | '|' | '>' | '<' | '`')
                    || (ch == '$'
                        && chars
                            .get(index + 1)
                            .is_some_and(|next| *next == '(' || *next == '{'))
                {
                    return true;
                }
            }
        }
        index += 1;
    }
    false
}

fn network_command_pattern() -> &'static regex::Regex {
    NETWORK_COMMAND_PATTERN.get_or_init(|| {
        regex::Regex::new(
            r"(?i)(https?://|urllib\.request|requests\.|http\.client|\bcurl\b|\bwget\b|\bssh\b|\bscp\b|\bftp\b)",
        )
        .expect("valid regex")
    })
}

fn dangerous_command_pattern() -> &'static regex::Regex {
    DANGEROUS_COMMAND_PATTERN.get_or_init(|| {
        regex::Regex::new(
            r"(?i)(git\s+reset\s+--hard|git\s+clean\s+-[^\r\n]*f|git\s+checkout\s+--\s+\.|(^|\s)rm\s+(-[^\r\n]*r[^\r\n]*f|--recursive)|remove-item\s+[^\r\n]*-recurse|(^|\s)(rmdir|del)\s+/s\b)",
        )
        .expect("valid regex")
    })
}

fn interpreter_mutation_pattern() -> &'static regex::Regex {
    INTERPRETER_MUTATION_PATTERN.get_or_init(|| {
        regex::Regex::new(
            r#"(?i)(shutil\.(rmtree|move)|os\.(remove|unlink|rmdir)|pathlib\.[^\s;]+\.(unlink|rename)|write_text|write_bytes|fs\.(writefile|writefilesync|unlink|rm)|set-content|out-file|new-item|files?\.(write|delete)|open\([^)]*['\"]w)"#,
        )
        .expect("valid regex")
    })
}

fn command_contains_external_path(command: &str) -> bool {
    let normalized = command.replace('\\', "/");
    normalized.contains("../")
        || normalized.contains("..\\")
        || regex::Regex::new(r#"(?i)(^|["'\s])/[^"]"#)
            .expect("valid regex")
            .is_match(&normalized)
        || regex::Regex::new(r"(?i)\b[A-Z]:/")
            .expect("valid regex")
            .is_match(&normalized)
}

fn command_targets_protected_repository_asset(command: &str) -> bool {
    let normalized_command = command.to_ascii_lowercase().replace('\\', "/");
    let references_protected_asset =
        normalized_command.contains(".git") || normalized_command.contains(".github");
    if !references_protected_asset {
        return false;
    }

    let mutating_operation = [
        "rm ",
        "remove-item",
        "rmdir",
        "del ",
        "unlink",
        "rmtree",
        "write_text",
        "writefile",
        "rename",
        "move",
        "checkout",
        "clean ",
    ]
    .iter()
    .any(|needle| normalized_command.contains(needle));
    if mutating_operation {
        return true;
    }

    command.split_whitespace().any(|part| {
        let token = part
            .trim_matches(|ch: char| matches!(ch, '\'' | '"' | '`' | ',' | ';'))
            .replace('\\', "/");
        let token = token.strip_prefix("./").unwrap_or(&token);
        token == ".git"
            || token.starts_with(".git/")
            || token == ".github"
            || token.starts_with(".github/")
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn actions_exposes_folder_listing_but_rejects_global_switch() {
        assert!(validate_actions_exposure("list_workspace_folders").is_ok());
        assert!(validate_actions_exposure("switch_workspace_folder").is_err());
    }

    #[test]
    fn edit_policy_defers_mixed_modes_to_the_typed_tool_contract() {
        let policy = PolicySettings::default();
        assert!(validate_edit_file(
            &json!({
                "path": "main.rs",
                "edits": [{ "type": "replace", "old_text": "old", "new_text": "new" }],
                "apply_proposal": { "proposal_id": "00000000000000000000000000000000" }
            }),
            &policy,
        )
        .is_ok());
        assert!(validate_edit_file(&json!({ "path": "main.rs" }), &policy).is_err());
    }

    #[test]
    fn workspace_allowed_commands_override_defaults() {
        let actions = ActionsConfig {
            allowed_commands: "cargo,go".into(),
            ..ActionsConfig::default()
        };
        let policy = PolicySettings::from_actions_config(&actions);
        assert!(policy.allowed_commands.contains("cargo"));
        assert!(policy.allowed_commands.contains("pytest"));
    }

    #[test]
    fn trusted_mode_accepts_any_configured_workspace_script_extension() {
        let policy = PolicySettings {
            workspace_local_entries: true,
            workspace_script_extensions: parse_workspace_script_extensions(".cmd,.launcher"),
            ..PolicySettings::default()
        };
        assert!(
            validate_command(&serde_json::json!({ "cmd": "anything.launcher" }), &policy).is_ok()
        );
        assert!(validate_command(
            &serde_json::json!({ "cmd": "scripts/another-name.cmd" }),
            &policy
        )
        .is_ok());
    }

    #[test]
    fn trusted_mode_accepts_an_extensionless_workspace_entry() {
        let dir = tempfile::tempdir().expect("workspace");
        std::fs::write(dir.path().join("project-entry"), "#!/bin/sh\necho ok\n").expect("entry");
        let workspace = Workspace::new(dir.path().to_path_buf()).expect("workspace");
        assert!(validate_command_for_workspace(
            &serde_json::json!({ "cmd": "project-entry", "workdir": "." }),
            &PolicySettings::default(),
            Some(&workspace),
        )
        .is_ok());
    }

    #[test]
    fn patch_size_uses_workspace_limit() {
        let actions = ActionsConfig {
            max_patch_bytes: 10,
            ..ActionsConfig::default()
        };
        let policy = PolicySettings::from_actions_config(&actions);
        let err = validate_patch(&json!({ "patch": "01234567890" }), &policy).unwrap_err();
        assert!(err.0.contains("too large"));
    }

    #[test]
    fn basic_diagnostic_commands_are_allowed() {
        let policy = PolicySettings::default();
        for command in BASIC_READ_ONLY_COMMANDS {
            validate_command(&json!({"cmd": command}), &policy)
                .unwrap_or_else(|err| panic!("{command} should be allowed: {err}"));
        }
    }

    #[test]
    fn configured_commands_keep_basic_diagnostics() {
        let actions = ActionsConfig {
            allowed_commands: "cargo,go".into(),
            ..ActionsConfig::default()
        };
        let policy = PolicySettings::from_actions_config(&actions);
        assert!(validate_command(&json!({"cmd": "pwd"}), &policy).is_ok());
        assert!(validate_command(&json!({"cmd": "pytest"}), &policy).is_ok());
    }

    #[test]
    fn format_files_project_apply_requires_confirmation() {
        let policy = PolicySettings::default();
        let denied = validate_tool_arguments(
            "format_files",
            &json!({"scope": "project", "mode": "apply"}),
            &policy,
        )
        .expect_err("project apply must require confirmation");
        assert!(denied
            .0
            .contains("DANGEROUS_OPERATION_REQUIRES_CONFIRMATION"));

        assert!(validate_tool_arguments(
            "format_files",
            &json!({"scope": "project", "mode": "apply", "confirm": true}),
            &policy,
        )
        .is_ok());
    }

    #[test]
    fn format_files_rejects_unsafe_paths_and_invalid_modes() {
        let policy = PolicySettings::default();
        assert!(validate_tool_arguments(
            "format_files",
            &json!({"paths": ["../outside.rs"], "mode": "plan"}),
            &policy,
        )
        .is_err());
        assert!(validate_tool_arguments(
            "format_files",
            &json!({"paths": ["src/lib.rs"], "mode": "rewrite"}),
            &policy,
        )
        .is_err());
        assert!(validate_tool_arguments(
            "format_files",
            &json!({"paths": ["src/lib.rs"], "mode": "plan"}),
            &policy,
        )
        .is_ok());
    }

    #[test]
    fn enabled_sandbox_owns_external_mutation_enforcement() {
        let command = json!({
            "cmd": "node -e \"const fs=require('fs'); fs.writeFileSync('C:/outside.txt','x')\""
        });
        let disabled = PolicySettings::default();
        let denied = validate_command(&command, &disabled)
            .expect_err("policy-only mode must stay workspace-bound");
        assert!(denied.0.contains("WORKSPACE_PATH_PROTECTED"));

        let enabled = PolicySettings {
            sandbox_enabled: true,
            ..PolicySettings::default()
        };
        validate_command(&command, &enabled)
            .expect("fail-closed sandbox backend should enforce external path grants");

        let protected = json!({
            "cmd": "node -e \"const fs=require('fs'); fs.writeFileSync('C:/repo/.git/config','x')\""
        });
        let denied = validate_command(&protected, &enabled)
            .expect_err("repository metadata protection must remain policy-enforced");
        assert!(denied.0.contains("PROTECTED_REPOSITORY_ASSET"));
    }

    #[test]
    fn quoted_python_code_is_not_treated_as_shell_chaining() {
        let policy = PolicySettings::default();
        assert!(validate_command(
            &json!({"cmd": "python -c \"import os; print(os.getcwd())\""}),
            &policy
        )
        .is_ok());
        assert!(validate_command(
            &json!({"cmd": "python -c \"print(1)\" && echo nope"}),
            &policy
        )
        .is_err());
        assert!(validate_command(&json!({"cmd": "echo hello > output.txt"}), &policy).is_err());
    }

    #[test]
    fn security_policy_gates_are_independently_configurable() {
        let mut policy = PolicySettings::default();
        policy.security_policy.require_shell_confirmation = false;
        assert!(validate_command(
            &json!({"script": "Write-Output ok", "shell": "powershell"}),
            &policy
        )
        .is_ok());

        policy.security_policy.require_shell_confirmation = true;
        assert!(validate_command(
            &json!({"script": "Write-Output ok", "shell": "powershell"}),
            &policy
        )
        .is_err());

        policy.security_policy.require_dangerous_confirmation = false;
        policy.security_policy.enforce_command_allowlist = false;
        assert!(validate_command(&json!({"cmd": "rm -rf build"}), &policy).is_ok());

        policy.security_policy.block_network_commands = true;
        assert!(validate_command(&json!({"cmd": "curl https://example.com"}), &policy).is_err());
    }
}
