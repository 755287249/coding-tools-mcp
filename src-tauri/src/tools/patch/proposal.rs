use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, SystemTime};

use serde_json::{json, Value};
use uuid::Uuid;

use crate::tools::workspace::WorkspaceError;

use super::hunk::apply_hunks;
use super::parser::parse_unified_diff;
use super::precise_edit::{
    adapt_newlines_to_range, byte_to_line, expected_occurrences, line_range_bytes,
    required_edit_text, whitespace_text_candidates,
};
use super::support::{sha256_hex, unified_diff};

pub(super) const EDIT_PROPOSAL_TTL: Duration = Duration::from_secs(300);
const MAX_EDIT_PROPOSALS: usize = 200;
const MAX_PROPOSAL_PATCH_BYTES: usize = 64 * 1024;
const MAX_PROPOSAL_REPLACEMENT_BYTES: usize = 128 * 1024;
const MAX_PROPOSAL_PREVIEW_BYTES: usize = 128 * 1024;
const SMALL_PROPOSAL_REPLACEMENT_BYTES: usize = 8 * 1024;
const PATCH_EFFICIENCY_PERCENT: usize = 80;

#[derive(Debug, Clone)]
struct EditProposal {
    path: String,
    file_sha256: String,
    start_byte: usize,
    end_byte: usize,
    actual_text: String,
    replacement: String,
    created_at: SystemTime,
}

static EDIT_PROPOSALS: OnceLock<Mutex<HashMap<String, EditProposal>>> = OnceLock::new();

fn proposal_store() -> &'static Mutex<HashMap<String, EditProposal>> {
    EDIT_PROPOSALS.get_or_init(|| Mutex::new(HashMap::new()))
}

fn prune_edit_proposals(proposals: &mut HashMap<String, EditProposal>) {
    proposals.retain(|_, proposal| {
        proposal
            .created_at
            .elapsed()
            .map(|age| age <= EDIT_PROPOSAL_TTL)
            .unwrap_or(false)
    });
    while proposals.len() >= MAX_EDIT_PROPOSALS {
        let Some(oldest) = proposals
            .iter()
            .min_by_key(|(_, proposal)| proposal.created_at)
            .map(|(id, _)| id.clone())
        else {
            break;
        };
        proposals.remove(&oldest);
    }
}

pub(super) fn remove_edit_proposal(proposal_id: &str) {
    proposal_store()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .remove(proposal_id);
}

pub(super) fn build_edit_proposal(
    path: &str,
    file_sha256: &str,
    original: &str,
    edits: &[Value],
) -> Result<Option<Value>, WorkspaceError> {
    if edits.len() != 1 {
        return Ok(None);
    }
    let edit = &edits[0];
    if edit.get("type").and_then(Value::as_str) != Some("replace")
        || edit
            .get("match_mode")
            .and_then(Value::as_str)
            .unwrap_or("exact")
            != "exact"
        || expected_occurrences(edit) != 1
    {
        return Ok(None);
    }
    let old_text = required_edit_text(edit, 0, "old_text")?;
    let requested_replacement = edit.get("new_text").and_then(Value::as_str).unwrap_or("");
    let search_range = match (
        edit.get("start_line").and_then(Value::as_u64),
        edit.get("end_line").and_then(Value::as_u64),
    ) {
        (None, None) => (0, original.len()),
        (Some(start), Some(end)) => line_range_bytes(original, start as usize, end as usize, 0)?,
        _ => return Ok(None),
    };
    let candidates = whitespace_text_candidates(original, old_text, search_range, 0)?;
    if candidates.len() != 1 {
        return Ok(None);
    }
    let (start_byte, end_byte) = candidates[0];
    let replacement =
        adapt_newlines_to_range(requested_replacement, original, start_byte, end_byte);
    let actual_text = original[start_byte..end_byte].to_string();
    let proposal_id = Uuid::new_v4().simple().to_string();
    let proposal = EditProposal {
        path: path.to_string(),
        file_sha256: file_sha256.to_string(),
        start_byte,
        end_byte,
        actual_text: actual_text.clone(),
        replacement: replacement.clone(),
        created_at: SystemTime::now(),
    };
    let mut proposed_content = original.to_string();
    proposed_content.replace_range(start_byte..end_byte, &replacement);
    let proposed_content_bytes = proposed_content.len();
    let proposed_content_sha256 = sha256_hex(proposed_content.as_bytes());
    let proposed_content_included = proposed_content_bytes <= MAX_PROPOSAL_PREVIEW_BYTES;
    let replacement_bytes = replacement.len();
    let preferred_format = if replacement_bytes <= SMALL_PROPOSAL_REPLACEMENT_BYTES {
        "replacement"
    } else {
        "patch"
    };
    let preferred_format_reason = if preferred_format == "replacement" {
        "small_replacement_is_cheaper"
    } else {
        "large_replacement_may_benefit_from_patch"
    };
    let mut proposals = proposal_store()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    prune_edit_proposals(&mut proposals);
    proposals.insert(proposal_id.clone(), proposal);

    Ok(Some(json!({
        "status": "proposal_required",
        "applied": false,
        "proposal_id": proposal_id,
        "proposal_ttl_seconds": EDIT_PROPOSAL_TTL.as_secs(),
        "path": path,
        "file_sha256": file_sha256,
        "candidate_start_line": byte_to_line(original, start_byte),
        "candidate_end_line": byte_to_line(original, end_byte.saturating_sub(1)),
        "actual_text": actual_text,
        "requested_old_text": old_text,
        "requested_new_text": requested_replacement,
        "candidate_diff": unified_diff(path, old_text, &original[start_byte..end_byte], false, false),
        "proposed_content": if proposed_content_included { json!(proposed_content) } else { Value::Null },
        "proposed_content_bytes": proposed_content_bytes,
        "proposed_content_included": proposed_content_included,
        "proposed_content_sha256": proposed_content_sha256,
        "accepted_formats": ["accept", "replacement", "patch"],
        "preferred_format": preferred_format,
        "preferred_format_reason": preferred_format_reason,
        "replacement_bytes": replacement_bytes,
        "replacement_max_bytes": MAX_PROPOSAL_REPLACEMENT_BYTES,
        "small_replacement_threshold_bytes": SMALL_PROPOSAL_REPLACEMENT_BYTES,
        "patch_efficiency_percent": PATCH_EFFICIENCY_PERCENT,
        "proposal_patch_format": "unified_diff_single_file_single_hunk",
        "proposal_patch_max_bytes": MAX_PROPOSAL_PATCH_BYTES,
        "next_action": "apply_proposal",
        "warnings": []
    })))
}

pub(super) fn apply_edit_proposal(
    path: &str,
    file_sha256: &str,
    original: &str,
    apply: &Value,
) -> Result<(String, Option<String>, &'static str), WorkspaceError> {
    let proposal_id = apply
        .get("proposal_id")
        .and_then(Value::as_str)
        .ok_or_else(|| {
            WorkspaceError::invalid_argument("apply_proposal.proposal_id is required")
        })?;
    let proposal_patch = apply.get("patch").and_then(Value::as_str);
    let proposal_replacement = apply.get("replacement").and_then(Value::as_str);
    if proposal_patch.is_some() && proposal_replacement.is_some() {
        return Err(WorkspaceError::invalid_argument(
            "apply_proposal.patch and apply_proposal.replacement are mutually exclusive",
        ));
    }
    if proposal_patch.is_some_and(|patch| patch.len() > MAX_PROPOSAL_PATCH_BYTES) {
        return Err(WorkspaceError::invalid_argument(format!(
            "apply_proposal.patch exceeds {MAX_PROPOSAL_PATCH_BYTES} bytes"
        )));
    }
    if proposal_replacement
        .is_some_and(|replacement| replacement.len() > MAX_PROPOSAL_REPLACEMENT_BYTES)
    {
        return Err(WorkspaceError::invalid_argument(format!(
            "apply_proposal.replacement exceeds {MAX_PROPOSAL_REPLACEMENT_BYTES} bytes"
        )));
    }

    let proposal = {
        let mut proposals = proposal_store()
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        prune_edit_proposals(&mut proposals);
        proposals
            .get(proposal_id)
            .cloned()
            .ok_or_else(|| WorkspaceError::ToolDetails {
                code: "EDIT_PROPOSAL_NOT_FOUND",
                message: "Edit proposal was not found or has expired.".into(),
                category: "conflict",
                retryable: true,
                details: json!({"proposal_id": proposal_id, "reason": "missing_or_expired"}),
            })?
    };

    if proposal.path != path || proposal.file_sha256 != file_sha256 {
        return Err(WorkspaceError::ToolDetails {
            code: "EDIT_PROPOSAL_STALE",
            message: "Edit proposal no longer matches the current file.".into(),
            category: "conflict",
            retryable: true,
            details: json!({"proposal_id": proposal_id, "reason": "file_changed"}),
        });
    }
    if original.get(proposal.start_byte..proposal.end_byte) != Some(proposal.actual_text.as_str()) {
        return Err(WorkspaceError::ToolDetails {
            code: "EDIT_PROPOSAL_STALE",
            message: "Edit proposal candidate no longer matches the current file.".into(),
            category: "conflict",
            retryable: true,
            details: json!({"proposal_id": proposal_id, "reason": "candidate_changed"}),
        });
    }

    let (replacement, apply_format) = if let Some(patch) = proposal_patch {
        let patched = apply_restricted_proposal_patch(&proposal.replacement, patch)?;
        if patch.len().saturating_mul(100) >= patched.len().saturating_mul(PATCH_EFFICIENCY_PERCENT)
        {
            return Err(WorkspaceError::ToolDetails {
                code: "EDIT_PROPOSAL_PATCH_INEFFICIENT",
                message:
                    "Proposal patch costs as much as or more than sending the full replacement."
                        .into(),
                category: "validation",
                retryable: true,
                details: json!({
                    "reason": "replacement_is_cheaper",
                    "patch_bytes": patch.len(),
                    "replacement_bytes": patched.len(),
                    "patch_efficiency_percent": PATCH_EFFICIENCY_PERCENT,
                    "recommended_format": "replacement",
                    "recommended_replacement": patched
                }),
            });
        }
        (patched, "patch")
    } else if let Some(replacement) = proposal_replacement {
        (replacement.to_string(), "replacement")
    } else {
        (proposal.replacement.clone(), "accept")
    };

    let replacement = adapt_newlines_to_range(
        &replacement,
        original,
        proposal.start_byte,
        proposal.end_byte,
    );
    let mut updated = original.to_string();
    updated.replace_range(proposal.start_byte..proposal.end_byte, &replacement);
    Ok((updated, Some(proposal_id.to_string()), apply_format))
}

fn apply_restricted_proposal_patch(
    proposed_text: &str,
    patch: &str,
) -> Result<String, WorkspaceError> {
    let files = parse_unified_diff(patch).map_err(|error| WorkspaceError::ToolDetails {
        code: "EDIT_PROPOSAL_PATCH_INVALID",
        message: "Proposal patch is not a valid unified diff.".into(),
        category: "validation",
        retryable: true,
        details: json!({
            "reason": "invalid_unified_diff",
            "source_error": error.to_error_value()
        }),
    })?;
    if files.len() != 1 || files[0].hunks.len() != 1 || files[0].is_new_file || files[0].is_deleted
    {
        return Err(WorkspaceError::ToolDetails {
            code: "EDIT_PROPOSAL_PATCH_INVALID",
            message: "Proposal patch must contain exactly one file and one update hunk.".into(),
            category: "validation",
            retryable: true,
            details: json!({
                "reason": "single_file_single_hunk_required",
                "file_count": files.len(),
                "hunk_count": files.first().map(|file| file.hunks.len()).unwrap_or(0)
            }),
        });
    }
    let updated = apply_hunks(proposed_text, &files[0].hunks, None).map_err(|error| {
        WorkspaceError::ToolDetails {
            code: "EDIT_PROPOSAL_PATCH_MISMATCH",
            message: "Proposal patch did not apply exactly to the proposed replacement.".into(),
            category: "conflict",
            retryable: true,
            details: json!({
                "reason": "proposal_text_mismatch",
                "source_error": error.to_error_value()
            }),
        }
    })?;
    if updated == proposed_text {
        return Err(WorkspaceError::ToolDetails {
            code: "EDIT_PROPOSAL_PATCH_NO_CHANGES",
            message: "Proposal patch produced no changes.".into(),
            category: "validation",
            retryable: true,
            details: json!({"reason": "no_changes"}),
        });
    }
    Ok(updated)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn apply_edit_proposal_for_test(proposed_text: &str, patch: &str) -> WorkspaceError {
        let patched = apply_restricted_proposal_patch(proposed_text, patch).expect("valid patch");
        if patch.len().saturating_mul(100) >= patched.len().saturating_mul(PATCH_EFFICIENCY_PERCENT)
        {
            WorkspaceError::ToolDetails {
                code: "EDIT_PROPOSAL_PATCH_INEFFICIENT",
                message:
                    "Proposal patch costs as much as or more than sending the full replacement."
                        .into(),
                category: "validation",
                retryable: true,
                details: json!({
                    "reason": "replacement_is_cheaper",
                    "patch_bytes": patch.len(),
                    "replacement_bytes": patched.len(),
                    "recommended_format": "replacement"
                }),
            }
        } else {
            panic!("fixture patch should be inefficient")
        }
    }

    #[test]
    fn proposal_uses_candidate_local_line_ending() {
        let original = "first\nsecond\r\nthird\r\n";
        let proposal = build_edit_proposal(
            "mixed.txt",
            "hash",
            original,
            &[json!({
                "type": "replace",
                "old_text": "first",
                "new_text": "FIRST\nEXTRA"
            })],
        )
        .expect("proposal build should succeed")
        .expect("proposal should be produced");
        assert_eq!(
            proposal["proposed_content"],
            "FIRST\nEXTRA\nsecond\r\nthird\r\n"
        );
    }

    #[test]
    fn inefficient_proposal_patch_recommends_full_replacement() {
        let error = apply_edit_proposal_for_test(
            "let value = 2;",
            "--- a/proposal\n+++ b/proposal\n@@\n-let value = 2;\n+let value = 3;\n",
        );
        assert_eq!(
            error.to_error_value()["code"],
            "EDIT_PROPOSAL_PATCH_INEFFICIENT"
        );
        assert_eq!(
            error.to_error_value()["details"]["recommended_format"],
            "replacement"
        );
    }

    #[test]
    fn restricted_proposal_patch_rejects_multiple_hunks() {
        let error = apply_restricted_proposal_patch(
            "one\ntwo\n",
            "--- a/proposal\n+++ b/proposal\n@@\n-one\n+ONE\n@@\n-two\n+TWO\n",
        )
        .expect_err("multiple hunks must be rejected");
        assert_eq!(
            error.to_error_value()["code"],
            "EDIT_PROPOSAL_PATCH_INVALID"
        );
    }
}
