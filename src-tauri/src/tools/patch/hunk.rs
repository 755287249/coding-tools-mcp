use serde_json::{json, Value};

use crate::tools::workspace::WorkspaceError;

use super::parser::{Hunk, HunkLine};

pub(super) fn apply_hunks(
    original: &str,
    hunks: &[Hunk],
    file: Option<&str>,
) -> Result<String, WorkspaceError> {
    let line_ending = if original.contains("\r\n") {
        "\r\n"
    } else {
        "\n"
    };
    let had_trailing_newline = original.ends_with('\n');
    let mut lines: Vec<String> = if original.is_empty() {
        Vec::new()
    } else {
        original
            .split_terminator('\n')
            .map(|line| line.trim_end_matches('\r').to_string())
            .collect()
    };
    let mut offset: i64 = 0;
    let mut issues = Vec::<WorkspaceError>::new();

    for (hunk_index, hunk) in hunks.iter().enumerate() {
        let hunk_old: Vec<String> = hunk
            .lines
            .iter()
            .filter_map(|l| match l {
                HunkLine::Context(s) | HunkLine::Remove(s) => Some(s.clone()),
                HunkLine::Add(_) => None,
            })
            .collect();
        let hunk_new: Vec<String> = hunk
            .lines
            .iter()
            .filter_map(|l| match l {
                HunkLine::Context(s) | HunkLine::Add(s) => Some(s.clone()),
                HunkLine::Remove(_) => None,
            })
            .collect();

        let preferred = hunk.old_start.map(|line| {
            ((line.saturating_sub(1)) as i64 + offset)
                .max(0)
                .min(lines.len() as i64) as usize
        });
        let pos =
            match find_hunk_position(&lines, &hunk_old, &hunk_new, preferred, hunk_index, file) {
                Ok(position) => position,
                Err(error) => {
                    issues.push(error);
                    continue;
                }
            };

        let mut idx = pos;
        let mut added = 0i64;
        let mut removed = 0i64;
        for hl in &hunk.lines {
            match hl {
                HunkLine::Context(_) => idx += 1,
                HunkLine::Remove(_) => {
                    if idx < lines.len() {
                        lines.remove(idx);
                        removed += 1;
                    }
                }
                HunkLine::Add(s) => {
                    lines.insert(idx, s.clone());
                    idx += 1;
                    added += 1;
                }
            }
        }
        offset += added - removed;
    }
    if issues.len() == 1 {
        return Err(issues.pop().expect("single patch issue"));
    }
    if !issues.is_empty() {
        let issue_values = issues
            .iter()
            .map(WorkspaceError::to_error_value)
            .collect::<Vec<_>>();
        return Err(WorkspaceError::ToolDetails {
            code: "PATCH_PREFLIGHT_FAILED",
            message: format!("{} patch hunks failed preflight.", issue_values.len()),
            category: "validation",
            retryable: false,
            details: json!({
                "issue_count": issue_values.len(),
                "issues": issue_values,
                "recommended_tool": "edit",
                "suggestion": "Resolve all listed hunk issues before retrying. Prefer edit for precise replacements.",
                "recovery_actions": [{
                    "action": "switch_to_precise_edits",
                    "tool": "edit",
                    "required_arguments": ["files"],
                    "reason": "multiple_patch_hunks_failed_preflight"
                }]
            }),
        });
    }
    let mut output = lines.join(line_ending);
    if !output.is_empty() && (had_trailing_newline || original.is_empty()) {
        output.push_str(line_ending);
    }
    Ok(output)
}

fn find_hunk_position(
    lines: &[String],
    pattern: &[String],
    replacement: &[String],
    preferred: Option<usize>,
    hunk_index: usize,
    file: Option<&str>,
) -> Result<usize, WorkspaceError> {
    if pattern.is_empty() {
        return Ok(preferred.unwrap_or(lines.len()).min(lines.len()));
    }
    if let Some(position) = preferred {
        if hunk_matches_at(lines, pattern, position) {
            return Ok(position);
        }
    }

    let mut candidates = Vec::new();
    if pattern.len() <= lines.len() {
        for position in 0..=lines.len() - pattern.len() {
            if hunk_matches_at(lines, pattern, position) {
                candidates.push(position);
            }
        }
    }
    match candidates.as_slice() {
        [position] => Ok(*position),
        [] => Err(WorkspaceError::ToolDetails {
            code: "PATCH_CONTEXT_NOT_FOUND",
            message: format!("Hunk {hunk_index} context did not match file content."),
            category: "validation",
            retryable: false,
            details: json!({
                "hunk_index": hunk_index,
                "preferred_line": preferred.map(|line| line + 1),
                "pattern_preview": pattern.iter().take(8).collect::<Vec<_>>(),
                "nearby_contexts": preferred
                    .map(|position| nearby_contexts(lines, &[position], 3))
                    .unwrap_or_default(),
                "recommended_tool": "edit",
                "suggestion": "Read the exact target range and use edit for a single precise replacement, or include more unique patch context.",
                "recovery_actions": [{
                    "action": "read_target_range",
                    "tool": "read_file",
                    "required_arguments": ["path"],
                    "arguments": {
                        "start_line": preferred.map(|line| line.saturating_sub(3).max(1)),
                        "end_line": preferred.map(|line| line.saturating_add(4))
                    },
                    "reason": "patch_context_not_found"
                }, {
                    "action": "switch_to_precise_edit",
                    "tool": "edit",
                    "required_arguments": ["files"],
                    "reason": "patch_context_not_found"
                }]
            }),
        }),
        _ => {
            let recovery_actions = if let Some(path) = file {
                candidates
                    .iter()
                    .take(8)
                    .enumerate()
                    .map(|(candidate_index, position)| {
                        json!({
                            "action": "apply_candidate_range",
                            "action_id": format!("patch-candidate-{hunk_index}-{}", candidate_index + 1),
                            "tool": "edit",
                            "required_arguments": [],
                            "candidate_line": position + 1,
                            "arguments": {
                                "files": [{
                                    "path": path,
                                    "edits": [{
                                        "type": "replace_lines",
                                        "start_line": position + 1,
                                        "end_line": position + pattern.len(),
                                        "expected_text": pattern.join("\n"),
                                        "new_text": replacement.join("\n")
                                    }]
                                }]
                            },
                            "reason": "patch_context_ambiguous"
                        })
                    })
                    .collect::<Vec<_>>()
            } else {
                vec![json!({
                    "action": "select_candidate_range",
                    "tool": "edit",
                    "required_arguments": ["files"],
                    "candidate_lines": candidates
                        .iter()
                        .map(|position| position + 1)
                        .collect::<Vec<_>>(),
                    "reason": "patch_context_ambiguous"
                })]
            };
            Err(WorkspaceError::ToolDetails {
                code: "PATCH_CONTEXT_AMBIGUOUS",
                message: format!(
                    "Hunk {hunk_index} context matched multiple locations; add more context or line numbers."
                ),
                category: "validation",
                retryable: false,
                details: json!({
                    "hunk_index": hunk_index,
                    "candidate_lines": candidates
                        .iter()
                        .map(|position| position + 1)
                        .collect::<Vec<_>>(),
                    "nearby_contexts": nearby_contexts(lines, &candidates, 3),
                    "recommended_tool": "edit",
                    "suggestion": "Choose one candidate recovery action to apply the hunk as a guarded precise edit, or add unique patch context.",
                    "recovery_actions": recovery_actions
                }),
            })
        }
    }
}

fn nearby_contexts(lines: &[String], positions: &[usize], radius: usize) -> Vec<Value> {
    positions
        .iter()
        .take(8)
        .map(|position| {
            let start = position.saturating_sub(radius);
            let end = (position.saturating_add(radius + 1)).min(lines.len());
            json!({
                "line": position + 1,
                "start_line": start + 1,
                "end_line": end,
                "preview": lines[start..end]
            })
        })
        .collect()
}

fn hunk_matches_at(lines: &[String], pattern: &[String], position: usize) -> bool {
    position <= lines.len()
        && pattern.len() <= lines.len().saturating_sub(position)
        && lines[position..position + pattern.len()]
            .iter()
            .zip(pattern.iter())
            .all(|(actual, expected)| actual == expected)
}
