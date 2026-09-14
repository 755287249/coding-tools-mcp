use std::path::{Path, PathBuf};

use serde_json::{json, Value};
use sha2::{Digest, Sha256};

use crate::mcp::command_kind;

use super::spec::{ExecSpec, PostCheckSpec};

#[derive(Clone, Debug)]
pub(super) struct ExecutionIdentity {
    pub(super) operation_id: Option<String>,
    pub(super) command_fingerprint: String,
    pub(super) resource_lock_group: Option<String>,
    pub(super) resource_lock_target: Option<String>,
    pub(super) process_timeout: Option<crate::tools::execution_timeout::ProcessTimeoutContract>,
}

pub(super) fn sha256_hex(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

fn executable_stem(program: &str) -> &str {
    let basename = program.rsplit(['/', '\\']).next().unwrap_or(program);
    basename
        .strip_suffix(".exe")
        .or_else(|| basename.strip_suffix(".EXE"))
        .unwrap_or(basename)
}

fn is_cargo_command(spec: &ExecSpec) -> bool {
    executable_stem(&spec.program).eq_ignore_ascii_case("cargo")
        || spec.display.to_ascii_lowercase().contains("cargo ")
        || spec.display.to_ascii_lowercase().contains("tauri build")
}

fn command_argument_value(args: &[String], name: &str) -> Option<String> {
    for (index, argument) in args.iter().enumerate() {
        if argument == name {
            return args.get(index + 1).cloned();
        }
        if let Some(value) = argument.strip_prefix(&format!("{name}=")) {
            return Some(value.to_string());
        }
    }
    None
}

fn normalized_lock_path(path: PathBuf) -> PathBuf {
    std::fs::canonicalize(&path).unwrap_or(path)
}

pub(super) fn cargo_target_lock(spec: &ExecSpec, cwd: &Path) -> Option<(String, String)> {
    if !is_cargo_command(spec) {
        return None;
    }
    let env_target = spec
        .env
        .iter()
        .find(|(key, _)| key.eq_ignore_ascii_case("CARGO_TARGET_DIR"))
        .map(|(_, value)| value.clone());
    let target = command_argument_value(&spec.args, "--target-dir")
        .or(env_target)
        .map(PathBuf::from)
        .map(|path| {
            if path.is_absolute() {
                path
            } else {
                cwd.join(path)
            }
        })
        .or_else(|| {
            command_argument_value(&spec.args, "--manifest-path").map(|manifest| {
                let manifest = PathBuf::from(manifest);
                let manifest = if manifest.is_absolute() {
                    manifest
                } else {
                    cwd.join(manifest)
                };
                manifest.parent().unwrap_or(cwd).join("target")
            })
        })
        .or_else(|| {
            let lower = spec.display.to_ascii_lowercase();
            let tauri_root = cwd.join("src-tauri");
            (lower.contains("tauri") && tauri_root.join("Cargo.toml").is_file())
                .then(|| tauri_root.join("target"))
        })
        .unwrap_or_else(|| cwd.join("target"));
    let target = normalized_lock_path(target);
    let display = target.to_string_lossy().into_owned();
    let digest = sha256_hex(display.as_bytes());
    Some((format!("cargo-target:{}", &digest[..24]), display))
}

pub(super) fn execution_identity(
    args: &Value,
    spec: &ExecSpec,
    cwd: &Path,
    timeout_ms: u64,
    tty: bool,
    stdin_text: &str,
    post_checks: &[PostCheckSpec],
) -> ExecutionIdentity {
    let mut env = spec.env.clone();
    env.sort();
    let mut remove_env = spec.remove_env.clone();
    remove_env.sort();
    let post_checks = post_checks
        .iter()
        .map(|check| {
            json!({
                "name": check.name,
                "program": check.exec.program,
                "args": check.exec.args,
                "shell": check.exec.shell,
                "env": check.exec.env,
                "remove_env": check.exec.remove_env,
                "expected_exit_code": check.expected_exit_code,
                "timeout_ms": check.timeout.as_millis(),
                "max_output_bytes": check.max_output_bytes
            })
        })
        .collect::<Vec<_>>();
    let automatic_cargo_dedupe = is_cargo_command(spec)
        && matches!(
            command_kind(args),
            "cargo_test" | "cargo_check" | "build" | "format"
        );
    let explicit_operation_id = args
        .get("operation_id")
        .and_then(Value::as_str)
        .filter(|value| !value.trim().is_empty())
        .map(str::to_string);
    let deduplicate = explicit_operation_id.is_some()
        || args
            .get("deduplicate")
            .and_then(Value::as_bool)
            .unwrap_or(automatic_cargo_dedupe);
    let automatic_lock = cargo_target_lock(spec, cwd);
    let resource_lock_group = args
        .get("lock_group")
        .and_then(Value::as_str)
        .filter(|value| !value.trim().is_empty())
        .map(str::to_string)
        .or_else(|| automatic_lock.as_ref().map(|(group, _)| group.clone()));
    let resource_lock_target = automatic_lock.map(|(_, target)| target);
    let mut material = json!({
        "cwd": cwd.to_string_lossy(),
        "program": spec.program,
        "args": spec.args,
        "shell": spec.shell,
        "env": env,
        "remove_env": remove_env,
        "timeout_ms": timeout_ms,
        "tty": tty,
        "stdin_sha256": sha256_hex(stdin_text.as_bytes()),
        "post_checks": post_checks,
        "resource_lock_group": resource_lock_group
    });
    if args.get("job_timeout_ms").is_some() {
        material["execution_mode"] = json!("job");
    }
    let command_fingerprint = sha256_hex(&serde_json::to_vec(&material).unwrap_or_default());
    let operation_id = explicit_operation_id
        .or_else(|| deduplicate.then(|| format!("auto:{}", &command_fingerprint[..32])));
    ExecutionIdentity {
        operation_id,
        command_fingerprint,
        resource_lock_group,
        resource_lock_target,
        process_timeout: None,
    }
}
