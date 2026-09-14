//! Unified patch orchestration behind the public patch facade.

use std::collections::HashMap;
use std::fs;

use serde_json::{json, Value};
use uuid::Uuid;

use crate::tools::context::ToolContext;
use crate::tools::workspace::{tool_ok, WorkspaceError};

use super::hunk::apply_hunks;
use super::parser::parse_unified_diff;
use super::support::{
    affected_paths, dangerous_operation, expected_hash_for, is_critical_file,
    is_protected_repository_asset, patch_failed, protected_repository_asset, sha256_hex,
    unified_diff, verify_file_version, version_mismatch,
};
use super::transaction::commit_staged;

pub(super) fn run_patch(ctx: &ToolContext, args: &Value) -> Result<Value, WorkspaceError> {
    let ws = &ctx.workspace;
    let security = ctx.runtime_config().policy.security_policy;
    let patch = args
        .get("patch")
        .and_then(Value::as_str)
        .ok_or_else(|| WorkspaceError::invalid_argument("patch is required"))?;
    let dry_run = args
        .get("dry_run")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let confirm = args
        .get("confirm")
        .and_then(Value::as_bool)
        .unwrap_or(false);

    let file_patches = parse_unified_diff(patch)?;
    if file_patches.is_empty() {
        return Err(patch_failed("No files were modified."));
    }
    if security.protect_repository_metadata {
        if let Some(path) = file_patches
            .iter()
            .find(|file| is_protected_repository_asset(&file.path))
            .map(|file| file.path.as_str())
        {
            return Err(protected_repository_asset(format!(
                "禁止删除仓库保护资产: {path}"
            )));
        }
    }
    if security.require_write_confirmation && !confirm {
        if let Some(path) = file_patches
            .iter()
            .find(|file| file.is_deleted && is_critical_file(&file.path))
            .map(|file| file.path.as_str())
        {
            return Err(dangerous_operation(format!(
                "删除关键项目文件需要 confirm=true: {path}"
            )));
        }
    }

    let mut affected = Vec::new();
    let mut summaries = Vec::new();
    let mut staged: HashMap<String, Option<String>> = HashMap::new();
    let mut preflight_versions: HashMap<String, Option<String>> = HashMap::new();
    let mut file_versions = Vec::new();
    let mut diff = String::new();

    for fp in &file_patches {
        ws.reject_unsafe_text(&fp.path)?;
        if security.block_symlink_escape {
            ws.reject_write_symlink(&fp.path)?;
        }
        let resolved = if fp.is_new_file {
            ws.resolve_for_write(&fp.path)?
        } else {
            ws.resolve_existing(&fp.path)?
        };

        let actual_bytes =
            if resolved.existed {
                Some(fs::read(&resolved.path).map_err(|_| {
                    WorkspaceError::not_found(format!("File not found: {}", fp.path))
                })?)
            } else {
                None
            };
        let before_sha256 = actual_bytes.as_deref().map(sha256_hex);
        if security.verify_write_conflicts {
            if let Some(expected) = expected_hash_for(args, &fp.path) {
                match before_sha256.as_deref() {
                    Some(actual) if expected.eq_ignore_ascii_case(actual) => {}
                    Some(actual) => return Err(version_mismatch(&fp.path, expected, actual)),
                    None => {
                        return Err(version_mismatch(&fp.path, expected, "missing"));
                    }
                }
            }
        }

        let replacing_after_delete = staged.get(&resolved.display).is_some_and(Option::is_none);
        if fp.is_new_file && resolved.existed && !replacing_after_delete {
            return Err(WorkspaceError::ToolDetails {
                code: "FILE_ALREADY_EXISTS",
                message: format!("Add File target already exists: {}", fp.path),
                category: "conflict",
                retryable: false,
                details: json!({
                    "path": fp.path,
                    "actual_sha256": before_sha256
                }),
            });
        }

        let original = if fp.is_new_file {
            String::new()
        } else {
            let bytes = actual_bytes
                .as_deref()
                .ok_or_else(|| WorkspaceError::not_found(format!("File not found: {}", fp.path)))?;
            String::from_utf8(bytes.to_vec()).map_err(|_| WorkspaceError::Tool {
                code: "UNSUPPORTED_ENCODING",
                message: format!("File is not valid utf-8: {}", fp.path),
                category: "validation",
                retryable: false,
            })?
        };

        preflight_versions
            .entry(resolved.display.clone())
            .or_insert_with(|| before_sha256.clone());

        if fp.is_deleted {
            diff.push_str(&unified_diff(&resolved.display, &original, "", false, true));
            staged.insert(resolved.display.clone(), None);
            affected.push(json!({ "path": resolved.display, "operation": "delete" }));
            summaries.push(format!("D {}", resolved.display));
            file_versions.push(json!({
                "path": resolved.display,
                "before_sha256": before_sha256,
                "after_sha256": Value::Null
            }));
            continue;
        }

        let updated = apply_hunks(&original, &fp.hunks, Some(&fp.path))?;
        if updated == original {
            return Err(patch_failed(format!(
                "Patch produced no changes for {}",
                fp.path
            )));
        }
        let op = if fp.is_new_file && !replacing_after_delete {
            "add"
        } else {
            "update"
        };
        let after_sha256 = sha256_hex(updated.as_bytes());
        diff.push_str(&unified_diff(
            &resolved.display,
            &original,
            &updated,
            fp.is_new_file,
            false,
        ));
        staged.insert(resolved.display.clone(), Some(updated));
        affected.push(json!({ "path": resolved.display, "operation": op }));
        summaries.push(format!(
            "{} {}",
            if op == "add" { "A" } else { "M" },
            resolved.display
        ));
        file_versions.push(json!({
            "path": resolved.display,
            "before_sha256": before_sha256,
            "after_sha256": after_sha256
        }));
    }

    let files_created = affected_paths(&affected, "add");
    let files_modified = affected_paths(&affected, "update");
    let files_deleted = affected_paths(&affected, "delete");

    if !dry_run && security.verify_write_conflicts {
        for (path, expected) in &preflight_versions {
            verify_file_version(ws, path, expected.as_deref())?;
        }
        let _transaction_backups = commit_staged(ws, &staged)?;
        let change_id = Uuid::new_v4().simple().to_string();
        return Ok(tool_ok(json!({
            "dry_run": false,
            "preflight": true,
            "applied": true,
            "clean": true,
            "change_id": change_id,
            "summary": summaries.join("\n"),
            "diff": diff,
            "file_versions": file_versions,
            "affected_files": affected,
            "files_created": files_created,
            "files_modified": files_modified,
            "files_deleted": files_deleted,
            "recovery": "git",
            "warnings": []
        })));
    }

    Ok(tool_ok(json!({
        "dry_run": true,
        "preflight": true,
        "applied": false,
        "clean": true,
        "summary": summaries.join("\n"),
        "diff": diff,
        "file_versions": file_versions,
        "affected_files": affected,
        "would_create": files_created,
        "would_modify": files_modified,
        "would_delete": files_deleted,
        "warnings": []
    })))
}

pub(super) fn run_check(ctx: &ToolContext, args: &Value) -> Result<Value, WorkspaceError> {
    let mut check_args = args.clone();
    check_args["dry_run"] = Value::Bool(true);
    let mut result = run_patch(ctx, &check_args)?;
    if let Some(object) = result.as_object_mut() {
        object.insert("preflight".into(), Value::Bool(true));
    }
    Ok(result)
}
