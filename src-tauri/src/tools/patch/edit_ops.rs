//! Single-file and atomic multi-file edit orchestration behind the public patch facade.

use std::collections::{HashMap, HashSet};
use std::fs;
use std::time::Instant;

use serde_json::{json, Value};
use uuid::Uuid;

use crate::tools::context::ToolContext;
use crate::tools::workspace::{tool_ok, WorkspaceError};

use super::precise_edit::{
    apply_precise_edits, assert_no_unexpected_edit_blast_radius,
    assert_no_unexpected_newline_churn, newline_style, validate_precise_edit_contract,
};
use super::proposal::{
    apply_edit_proposal, build_edit_proposal, remove_edit_proposal, EDIT_PROPOSAL_TTL,
};
use super::support::{
    enrich_edit_error, enrich_edit_many_error, patch_failed, replay_reason, replayable_edit_plan,
    sha256_hex, unified_diff, verify_file_version, version_mismatch,
};
use super::transaction::commit_staged;

pub(super) fn run_file(ctx: &ToolContext, args: &Value) -> Result<Value, WorkspaceError> {
    let started = Instant::now();
    let ws = &ctx.workspace;
    let security = ctx.runtime_config().policy.security_policy;
    let path = args
        .get("path")
        .and_then(Value::as_str)
        .ok_or_else(|| WorkspaceError::invalid_argument("path is required"))?;
    if args.get("edits").is_some() && args.get("apply_proposal").is_some() {
        return Err(WorkspaceError::ToolDetails {
            code: "EDIT_CONTRACT_INVALID",
            message: "edit_file accepts either edits or apply_proposal, not both".into(),
            category: "validation",
            retryable: false,
            details: json!({
                "path": path,
                "issue_count": 1,
                "issues": [{
                    "field": "edits,apply_proposal",
                    "reason": "mutually_exclusive_fields"
                }],
                "suggestion": "Send precise edits or apply one stored proposal",
                "recovery_actions": [{
                    "action": "choose_edit_mode",
                    "tool": "edit",
                    "arguments": { "files": [{ "path": path }] },
                    "required_arguments": ["files[0].edits_or_apply_proposal"],
                    "reason": "mutually_exclusive_fields"
                }]
            }),
        });
    }
    let dry_run = args
        .get("dry_run")
        .and_then(Value::as_bool)
        .unwrap_or(false);

    ws.reject_unsafe_text(path)?;
    if security.block_symlink_escape {
        ws.reject_write_symlink(path)?;
    }
    let resolved = ws.resolve_existing(path)?;
    if !resolved.path.is_file() {
        return Err(WorkspaceError::Tool {
            code: "IS_DIRECTORY",
            message: "edit_file target must be a file".into(),
            category: "validation",
            retryable: false,
        });
    }
    let original_bytes = fs::read(&resolved.path)
        .map_err(|_| WorkspaceError::not_found(format!("File not found: {path}")))?;
    if original_bytes.iter().take(4096).any(|byte| *byte == 0) {
        return Err(WorkspaceError::Tool {
            code: "BINARY_FILE",
            message: "Binary file edit blocked for text tool.".into(),
            category: "validation",
            retryable: false,
        });
    }
    let before_sha256 = sha256_hex(&original_bytes);
    if security.verify_write_conflicts {
        if let Some(expected) = args.get("expected_sha256").and_then(Value::as_str) {
            if !expected.eq_ignore_ascii_case(&before_sha256) {
                return Err(version_mismatch(
                    &resolved.display,
                    expected,
                    &before_sha256,
                ));
            }
        }
    }
    let original = String::from_utf8(original_bytes).map_err(|_| WorkspaceError::Tool {
        code: "UNSUPPORTED_ENCODING",
        message: "File is not valid utf-8.".into(),
        category: "validation",
        retryable: false,
    })?;

    let (updated, proposal_id, proposal_apply_format) =
        if let Some(apply) = args.get("apply_proposal") {
            apply_edit_proposal(&resolved.display, &before_sha256, &original, apply)?
        } else {
            let edits = args.get("edits").and_then(Value::as_array).ok_or_else(|| {
                WorkspaceError::invalid_argument("edits or apply_proposal is required")
            })?;
            if edits.is_empty() {
                return Err(WorkspaceError::invalid_argument("edits must not be empty"));
            }
            if let Err(error) = validate_precise_edit_contract(edits) {
                return Err(enrich_edit_error(error, &resolved.display, &before_sha256));
            }
            match apply_precise_edits(&original, edits) {
                Ok(updated) => (updated, None, "direct"),
                Err(error) => {
                    if let Some(proposal) =
                        build_edit_proposal(&resolved.display, &before_sha256, &original, edits)?
                    {
                        return Ok(tool_ok(proposal));
                    }
                    return Err(enrich_edit_error(error, &resolved.display, &before_sha256));
                }
            }
        };

    if updated == original {
        return Err(patch_failed("Edits produced no changes."));
    }
    assert_no_unexpected_newline_churn(&resolved.display, &original, &updated)?;
    let blast_radius_edits = args
        .get("edits")
        .and_then(Value::as_array)
        .map(Vec::as_slice)
        .unwrap_or(&[]);
    let blast_radius = assert_no_unexpected_edit_blast_radius(
        &resolved.display,
        &original,
        &updated,
        blast_radius_edits,
    )?;
    let newline_before = newline_style(&original);
    let newline_after = newline_style(&updated);
    let after_sha256 = sha256_hex(updated.as_bytes());
    let diff = unified_diff(&resolved.display, &original, &updated, false, false);
    let (bytes_before, bytes_after) = (original.len(), updated.len());
    let change_id = if dry_run {
        Value::Null
    } else {
        Value::String(Uuid::new_v4().simple().to_string())
    };
    let preflight_finished = Instant::now();
    let edit_plan = if dry_run {
        let mut replay_file = serde_json::Map::new();
        replay_file.insert("path".into(), json!(resolved.display));
        replay_file.insert("expected_sha256".into(), json!(before_sha256));
        if let Some(edits) = args.get("edits") {
            replay_file.insert("edits".into(), edits.clone());
        }
        if let Some(apply_proposal) = args.get("apply_proposal") {
            replay_file.insert("apply_proposal".into(), apply_proposal.clone());
        }
        let mut replay_arguments = serde_json::Map::new();
        replay_arguments.insert(
            "files".into(),
            Value::Array(vec![Value::Object(replay_file)]),
        );
        replay_arguments.insert("dry_run".into(), Value::Bool(false));
        if let Some(reason) = replay_reason(args) {
            replay_arguments.insert("reason".into(), reason);
        }
        let stateful_dependencies = proposal_id.as_ref().map_or_else(
            || json!([]),
            |proposal_id| {
                json!([{
                    "type": "edit_proposal",
                    "proposal_id": proposal_id,
                    "ttl_seconds": EDIT_PROPOSAL_TTL.as_secs()
                }])
            },
        );
        replayable_edit_plan(
            "edit",
            Value::Object(replay_arguments),
            json!([{
                "path": resolved.display,
                "before_sha256": before_sha256,
                "after_sha256": after_sha256
            }]),
            stateful_dependencies,
        )
    } else {
        Value::Null
    };
    let plan_finished = Instant::now();
    if !dry_run && security.verify_write_conflicts {
        verify_file_version(ws, &resolved.display, Some(&before_sha256))?;
        let mut staged = HashMap::new();
        staged.insert(resolved.display.clone(), Some(updated));
        let _transaction_backups = commit_staged(ws, &staged)?;
        if let Some(proposal_id) = proposal_id.as_deref() {
            remove_edit_proposal(proposal_id);
        }
    }

    let completed = Instant::now();
    let phase_durations_ms = json!({
        "preflight_ms": preflight_finished.duration_since(started).as_millis(),
        "plan_ms": plan_finished.duration_since(preflight_finished).as_millis(),
        "commit_ms": completed.duration_since(plan_finished).as_millis(),
        "total_ms": completed.duration_since(started).as_millis()
    });
    Ok(tool_ok(json!({
        "status": if proposal_id.is_some() { "proposal_applied" } else { "edited" },
        "proposal_id": proposal_id,
        "proposal_apply_format": proposal_apply_format,
        "dry_run": dry_run,
        "preflight": true,
        "applied": !dry_run,
        "clean": true,
        "change_id": change_id,
        "path": resolved.display,
        "operation": "update",
        "before_sha256": before_sha256,
        "after_sha256": after_sha256,
        "edit_plan": edit_plan,
        "newline_before": newline_before,
        "newline_after": newline_after,
        "newline_guard": "passed",
        "blast_radius_guard": "passed",
        "blast_radius": blast_radius,
        "diff": diff,
        "phase_durations_ms": phase_durations_ms,
        "affected_files": [{
            "path": resolved.display,
            "operation": "update",
            "bytes_before": bytes_before,
            "bytes_after": bytes_after
        }],
        "files_created": Vec::<String>::new(),
        "files_modified": [resolved.display],
        "files_deleted": Vec::<String>::new(),
        "recovery": if dry_run { Value::Null } else { json!("git") },
        "warnings": []
    })))
}

pub(super) fn run_edit(ctx: &ToolContext, args: &Value) -> Result<Value, WorkspaceError> {
    let files = args
        .get("files")
        .and_then(Value::as_array)
        .ok_or_else(|| WorkspaceError::invalid_argument("files is required"))?;
    if files.is_empty() {
        return Err(WorkspaceError::invalid_argument("files must not be empty"));
    }
    if ctx
        .runtime_config()
        .policy
        .security_policy
        .enforce_resource_limits
        && files.len() > 100
    {
        return Err(WorkspaceError::invalid_argument(
            "edit supports at most 100 files",
        ));
    }
    if files.len() == 1 {
        let file = files[0]
            .as_object()
            .ok_or_else(|| WorkspaceError::invalid_argument("files[0] must be an object"))?;
        let mut single = serde_json::Map::new();
        for field in ["path", "expected_sha256", "edits", "apply_proposal"] {
            if let Some(value) = file.get(field) {
                single.insert(field.to_string(), value.clone());
            }
        }
        for field in ["dry_run", "reason"] {
            if let Some(value) = args.get(field) {
                single.insert(field.to_string(), value.clone());
            }
        }
        let mut result = run_file(ctx, &Value::Object(single))?;
        if let Some(object) = result.as_object_mut() {
            object.insert("atomic".into(), Value::Bool(true));
        }
        return Ok(result);
    }
    if files
        .iter()
        .any(|file| file.get("apply_proposal").is_some())
    {
        return Err(WorkspaceError::ToolDetails {
            code: "EDIT_CONTRACT_INVALID",
            message: "apply_proposal is supported only when edit contains one file".into(),
            category: "validation",
            retryable: false,
            details: json!({
                "file_count": files.len(),
                "suggestion": "Apply a proposal in a single-file edit call",
                "recovery_actions": [{
                    "action": "split_proposal_edit",
                    "tool": "edit",
                    "required_arguments": ["files"],
                    "reason": "proposal_requires_single_file"
                }]
            }),
        });
    }
    run_many(ctx, args)
}

pub(super) fn run_many(ctx: &ToolContext, args: &Value) -> Result<Value, WorkspaceError> {
    let started = Instant::now();
    let files = args
        .get("files")
        .and_then(Value::as_array)
        .ok_or_else(|| WorkspaceError::invalid_argument("files is required"))?;
    if files.is_empty() {
        return Err(WorkspaceError::invalid_argument("files must not be empty"));
    }
    let dry_run = args
        .get("dry_run")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let ws = &ctx.workspace;
    let security = ctx.runtime_config().policy.security_policy;
    let mut staged = HashMap::new();
    let mut versions = HashMap::new();
    let mut file_versions = Vec::new();
    let mut affected = Vec::new();
    let mut diffs = String::new();
    let mut seen = HashSet::new();

    for (file_index, file) in files.iter().enumerate() {
        let path = file
            .get("path")
            .and_then(Value::as_str)
            .ok_or_else(|| WorkspaceError::invalid_argument("files[].path is required"))?;
        if !seen.insert(path.to_string()) {
            return Err(WorkspaceError::invalid_argument(format!(
                "duplicate edit_many path: {path}"
            )));
        }
        let edits = file
            .get("edits")
            .and_then(Value::as_array)
            .ok_or_else(|| WorkspaceError::invalid_argument("files[].edits is required"))?;
        if edits.is_empty() {
            return Err(WorkspaceError::invalid_argument(
                "files[].edits must not be empty",
            ));
        }
        ws.reject_unsafe_text(path)?;
        if security.block_symlink_escape {
            ws.reject_write_symlink(path)?;
        }
        let resolved = ws.resolve_existing(path)?;
        if !resolved.path.is_file() {
            return Err(WorkspaceError::invalid_argument(format!(
                "edit_many target must be a file: {path}"
            )));
        }
        let original_bytes = fs::read(&resolved.path)
            .map_err(|_| WorkspaceError::not_found(format!("File not found: {path}")))?;
        if original_bytes.iter().take(4096).any(|byte| *byte == 0) {
            return Err(WorkspaceError::Tool {
                code: "BINARY_FILE",
                message: format!("Binary file edit blocked: {path}"),
                category: "validation",
                retryable: false,
            });
        }
        let before_sha256 = sha256_hex(&original_bytes);
        if security.verify_write_conflicts {
            if let Some(expected) = file.get("expected_sha256").and_then(Value::as_str) {
                if !expected.eq_ignore_ascii_case(&before_sha256) {
                    return Err(enrich_edit_many_error(
                        version_mismatch(&resolved.display, expected, &before_sha256),
                        &resolved.display,
                        &before_sha256,
                        file_index,
                    ));
                }
            }
        }
        let original = String::from_utf8(original_bytes).map_err(|_| WorkspaceError::Tool {
            code: "UNSUPPORTED_ENCODING",
            message: format!("File is not valid UTF-8: {path}"),
            category: "validation",
            retryable: false,
        })?;
        if let Err(error) = validate_precise_edit_contract(edits) {
            return Err(enrich_edit_many_error(
                error,
                &resolved.display,
                &before_sha256,
                file_index,
            ));
        }
        let updated = apply_precise_edits(&original, edits).map_err(|error| {
            enrich_edit_many_error(error, &resolved.display, &before_sha256, file_index)
        })?;
        if updated == original {
            return Err(patch_failed(format!("Edits produced no changes: {path}")));
        }
        assert_no_unexpected_newline_churn(&resolved.display, &original, &updated).map_err(
            |error| enrich_edit_many_error(error, &resolved.display, &before_sha256, file_index),
        )?;
        let blast_radius =
            assert_no_unexpected_edit_blast_radius(&resolved.display, &original, &updated, edits)
                .map_err(|error| {
                enrich_edit_many_error(error, &resolved.display, &before_sha256, file_index)
            })?;
        let newline_before = newline_style(&original);
        let newline_after = newline_style(&updated);
        let after_sha256 = sha256_hex(updated.as_bytes());
        diffs.push_str(&unified_diff(
            &resolved.display,
            &original,
            &updated,
            false,
            false,
        ));
        versions.insert(resolved.display.clone(), Some(before_sha256.clone()));
        let (bytes_before, bytes_after) = (original.len(), updated.len());
        staged.insert(resolved.display.clone(), Some(updated));
        file_versions.push(json!({
            "path": resolved.display,
            "before_sha256": before_sha256,
            "after_sha256": after_sha256,
            "newline_before": newline_before,
            "newline_after": newline_after,
            "newline_guard": "passed",
            "blast_radius_guard": "passed",
            "blast_radius": blast_radius
        }));
        affected.push(json!({
            "path": resolved.display,
            "operation": "update",
            "bytes_before": bytes_before,
            "bytes_after": bytes_after
        }));
    }

    let preflight_finished = Instant::now();

    if !dry_run && security.verify_write_conflicts {
        for (path, expected) in &versions {
            verify_file_version(ws, path, expected.as_deref())?;
        }
        let _backups = commit_staged(ws, &staged)?;
    }
    let commit_finished = Instant::now();
    let modified = affected
        .iter()
        .filter_map(|item| item.get("path").and_then(Value::as_str).map(str::to_string))
        .collect::<Vec<_>>();
    let edit_plan = if dry_run {
        let replay_files = files
            .iter()
            .zip(file_versions.iter())
            .map(|(file, version)| {
                json!({
                    "path": version["path"],
                    "edits": file["edits"],
                    "expected_sha256": version["before_sha256"]
                })
            })
            .collect::<Vec<_>>();
        let mut replay_arguments = serde_json::Map::new();
        replay_arguments.insert("files".into(), Value::Array(replay_files));
        replay_arguments.insert("dry_run".into(), Value::Bool(false));
        if let Some(reason) = replay_reason(args) {
            replay_arguments.insert("reason".into(), reason);
        }
        replayable_edit_plan(
            "edit",
            Value::Object(replay_arguments),
            Value::Array(file_versions.clone()),
            json!([]),
        )
    } else {
        Value::Null
    };
    let completed = Instant::now();
    let phase_durations_ms = json!({
        "preflight_ms": preflight_finished.duration_since(started).as_millis(),
        "commit_ms": commit_finished.duration_since(preflight_finished).as_millis(),
        "plan_ms": completed.duration_since(commit_finished).as_millis(),
        "total_ms": completed.duration_since(started).as_millis()
    });
    Ok(tool_ok(json!({
        "dry_run": dry_run,
        "preflight": true,
        "applied": !dry_run,
        "atomic": true,
        "change_id": if dry_run { Value::Null } else { json!(Uuid::new_v4().simple().to_string()) },
        "diff": diffs,
        "file_versions": file_versions,
        "affected_files": affected,
        "edit_plan": edit_plan,
        "phase_durations_ms": phase_durations_ms,
        "files_created": Vec::<String>::new(),
        "files_modified": modified,
        "files_deleted": Vec::<String>::new(),
        "warnings": []
    })))
}
