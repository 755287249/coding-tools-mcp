use std::collections::{HashMap, HashSet};
use std::fs;

use serde_json::{json, Value};
use uuid::Uuid;

use crate::tools::context::ToolContext;
use crate::tools::workspace::{tool_ok, WorkspaceError};

use super::support::{
    dangerous_operation, is_critical_file, patch_failed, sha256_hex, unified_diff,
    verify_file_version, version_mismatch,
};
use super::transaction::{commit_staged_bytes, restore_backups};

pub(super) fn run(ctx: &ToolContext, args: &Value) -> Result<Value, WorkspaceError> {
    let operations = args
        .get("operations")
        .and_then(Value::as_array)
        .ok_or_else(|| WorkspaceError::invalid_argument("operations is required"))?;
    if operations.is_empty() {
        return Err(WorkspaceError::invalid_argument(
            "operations must not be empty",
        ));
    }
    let dry_run = args
        .get("dry_run")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let confirm = args
        .get("confirm")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let ws = &ctx.workspace;
    let security = ctx.runtime_config().policy.security_policy;
    let mut staged: HashMap<String, Option<Vec<u8>>> = HashMap::new();
    let mut versions: HashMap<String, Option<String>> = HashMap::new();
    let mut directories = Vec::new();
    let mut affected = Vec::new();
    let mut diffs = String::new();
    let mut touched = HashSet::new();

    for (index, operation) in operations.iter().enumerate() {
        let kind = operation
            .get("type")
            .and_then(Value::as_str)
            .ok_or_else(|| WorkspaceError::invalid_argument("operations[].type is required"))?;
        let path = operation
            .get("path")
            .and_then(Value::as_str)
            .ok_or_else(|| WorkspaceError::invalid_argument("operations[].path is required"))?;
        ws.reject_unsafe_text(path)?;
        if security.block_symlink_escape {
            ws.reject_write_symlink(path)?;
        }
        if security.protect_repository_metadata {
            ws.reject_protected_write_path(path)?;
        }
        match kind {
            "create" => {
                let resolved = ws.resolve_for_write(path)?;
                let overwrite = operation
                    .get("overwrite")
                    .and_then(Value::as_bool)
                    .unwrap_or(false);
                if resolved.existed && overwrite && security.require_write_confirmation && !confirm
                {
                    return Err(dangerous_operation(format!(
                        "Overwriting an existing file requires confirm=true: {path}"
                    )));
                }
                if resolved.existed && !overwrite {
                    return Err(WorkspaceError::ToolDetails {
                        code: "FILE_ALREADY_EXISTS",
                        message: format!("Create target already exists: {path}"),
                        category: "conflict",
                        retryable: false,
                        details: json!({"path": path, "operation_index": index}),
                    });
                }
                if !touched.insert(resolved.display.clone()) {
                    return Err(WorkspaceError::invalid_argument(format!(
                        "duplicate file_ops target: {path}"
                    )));
                }
                let before = if resolved.existed {
                    Some(fs::read(&resolved.path).map_err(|e| patch_failed(e.to_string()))?)
                } else {
                    None
                };
                if security.verify_write_conflicts {
                    check_operation_hash(operation, &resolved.display, before.as_deref())?;
                }
                versions.insert(resolved.display.clone(), before.as_deref().map(sha256_hex));
                let content = operation
                    .get("content")
                    .and_then(Value::as_str)
                    .unwrap_or("")
                    .as_bytes()
                    .to_vec();
                if let (Some(before_text), Ok(after_text)) = (
                    before.as_ref().and_then(|b| std::str::from_utf8(b).ok()),
                    std::str::from_utf8(&content),
                ) {
                    diffs.push_str(&unified_diff(
                        &resolved.display,
                        before_text,
                        after_text,
                        false,
                        false,
                    ));
                } else if let Ok(after_text) = std::str::from_utf8(&content) {
                    diffs.push_str(&unified_diff(
                        &resolved.display,
                        "",
                        after_text,
                        true,
                        false,
                    ));
                }
                let (bytes_before, bytes_after) =
                    (before.as_ref().map_or(0, Vec::len), content.len());
                staged.insert(resolved.display.clone(), Some(content));
                affected.push(json!({
                    "path": resolved.display,
                    "operation": if resolved.existed { "update" } else { "add" },
                    "bytes_before": bytes_before,
                    "bytes_after": bytes_after
                }));
            }
            "delete" => {
                if is_critical_file(path) && security.require_write_confirmation && !confirm {
                    return Err(dangerous_operation(format!(
                        "Deleting a critical project file requires confirm=true: {path}"
                    )));
                }
                let resolved = ws.resolve_existing(path)?;
                if !resolved.path.is_file() {
                    return Err(WorkspaceError::invalid_argument(format!(
                        "delete target must be a file: {path}"
                    )));
                }
                if !touched.insert(resolved.display.clone()) {
                    return Err(WorkspaceError::invalid_argument(format!(
                        "duplicate file_ops target: {path}"
                    )));
                }
                let before = fs::read(&resolved.path).map_err(|e| patch_failed(e.to_string()))?;
                if security.verify_write_conflicts {
                    check_operation_hash(operation, &resolved.display, Some(&before))?;
                }
                versions.insert(resolved.display.clone(), Some(sha256_hex(&before)));
                if let Ok(before_text) = std::str::from_utf8(&before) {
                    diffs.push_str(&unified_diff(
                        &resolved.display,
                        before_text,
                        "",
                        false,
                        true,
                    ));
                }
                let bytes_before = before.len();
                staged.insert(resolved.display.clone(), None);
                affected.push(json!({
                    "path": resolved.display,
                    "operation": "delete",
                    "bytes_before": bytes_before,
                    "bytes_after": 0
                }));
            }
            "copy" | "move" => {
                let source = ws.resolve_existing(path)?;
                if !source.path.is_file() {
                    return Err(WorkspaceError::invalid_argument(format!(
                        "{kind} source must be a file: {path}"
                    )));
                }
                let destination = operation
                    .get("destination")
                    .and_then(Value::as_str)
                    .ok_or_else(|| {
                        WorkspaceError::invalid_argument("copy/move destination is required")
                    })?;
                ws.reject_unsafe_text(destination)?;
                if security.block_symlink_escape {
                    ws.reject_write_symlink(destination)?;
                }
                if security.protect_repository_metadata {
                    ws.reject_protected_write_path(destination)?;
                }
                let target = ws.resolve_for_write(destination)?;
                if source.display == target.display {
                    return Err(WorkspaceError::invalid_argument(
                        "source and destination must differ",
                    ));
                }
                let overwrite = operation
                    .get("overwrite")
                    .and_then(Value::as_bool)
                    .unwrap_or(false);
                if target.existed && overwrite && security.require_write_confirmation && !confirm {
                    return Err(dangerous_operation(format!(
                        "Overwriting a destination requires confirm=true: {destination}"
                    )));
                }
                if target.existed && !overwrite {
                    return Err(WorkspaceError::ToolDetails {
                        code: "FILE_ALREADY_EXISTS",
                        message: format!("Destination already exists: {destination}"),
                        category: "conflict",
                        retryable: false,
                        details: json!({"path": destination, "operation_index": index}),
                    });
                }
                if !touched.insert(target.display.clone())
                    || (kind == "move" && !touched.insert(source.display.clone()))
                {
                    return Err(WorkspaceError::invalid_argument(
                        "file_ops paths may only be touched once per transaction",
                    ));
                }
                let bytes = fs::read(&source.path).map_err(|e| patch_failed(e.to_string()))?;
                if security.verify_write_conflicts {
                    check_operation_hash(operation, &source.display, Some(&bytes))?;
                }
                versions.insert(source.display.clone(), Some(sha256_hex(&bytes)));
                versions.insert(
                    target.display.clone(),
                    if target.existed {
                        Some(sha256_hex(
                            &fs::read(&target.path).map_err(|e| patch_failed(e.to_string()))?,
                        ))
                    } else {
                        None
                    },
                );
                staged.insert(target.display.clone(), Some(bytes));
                if kind == "move" {
                    staged.insert(source.display.clone(), None);
                    affected.push(json!({"path": source.display, "destination": target.display, "operation": "move"}));
                } else {
                    affected.push(json!({"path": source.display, "destination": target.display, "operation": "copy"}));
                }
            }
            "mkdir" => {
                let resolved = ws.resolve_for_write(path)?;
                if resolved.existed && !resolved.path.is_dir() {
                    return Err(WorkspaceError::invalid_argument(format!(
                        "mkdir target is a file: {path}"
                    )));
                }
                directories.push((
                    resolved.display.clone(),
                    resolved.path.clone(),
                    resolved.existed,
                ));
                affected.push(json!({"path": resolved.display, "operation": "mkdir"}));
            }
            _ => {
                return Err(WorkspaceError::invalid_argument(format!(
                    "unsupported file operation: {kind}"
                )))
            }
        }
    }

    if !dry_run && security.verify_write_conflicts {
        for (path, expected) in &versions {
            verify_file_version(ws, path, expected.as_deref())?;
        }
        let backups = commit_staged_bytes(ws, &staged)?;
        let mut created_dirs = Vec::new();
        for (_, path, existed) in &directories {
            if !*existed {
                if let Err(error) = fs::create_dir_all(path) {
                    restore_backups(&backups);
                    for created in created_dirs.iter().rev() {
                        let _ = fs::remove_dir(created);
                    }
                    return Err(patch_failed(format!("Failed to create directory: {error}")));
                }
                created_dirs.push(path.clone());
            }
        }
    }

    let created = affected
        .iter()
        .filter(|v| v["operation"] == "add")
        .filter_map(|v| v["path"].as_str().map(str::to_string))
        .collect::<Vec<_>>();
    let modified = affected
        .iter()
        .filter(|v| v["operation"] == "update")
        .filter_map(|v| v["path"].as_str().map(str::to_string))
        .collect::<Vec<_>>();
    let deleted = affected
        .iter()
        .filter(|v| v["operation"] == "delete")
        .filter_map(|v| v["path"].as_str().map(str::to_string))
        .collect::<Vec<_>>();
    Ok(tool_ok(json!({
        "dry_run": dry_run,
        "preflight": true,
        "applied": !dry_run,
        "atomic": true,
        "change_id": if dry_run { Value::Null } else { json!(Uuid::new_v4().simple().to_string()) },
        "diff": diffs,
        "affected_files": affected,
        "files_created": created,
        "files_modified": modified,
        "files_deleted": deleted,
        "warnings": []
    })))
}

fn check_operation_hash(
    operation: &Value,
    path: &str,
    bytes: Option<&[u8]>,
) -> Result<(), WorkspaceError> {
    if let Some(expected) = operation.get("expected_sha256").and_then(Value::as_str) {
        let actual = bytes.map(sha256_hex).unwrap_or_else(|| "missing".into());
        if !expected.eq_ignore_ascii_case(&actual) {
            return Err(version_mismatch(path, expected, &actual));
        }
    }
    Ok(())
}
