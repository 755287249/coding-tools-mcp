use std::collections::HashMap;

use regex::Regex;
use serde_json::{json, Value};

use crate::tools::workspace::WorkspaceError;

#[derive(Debug, Clone)]
struct ResolvedEdit {
    input_index: usize,
    start_byte: usize,
    end_byte: usize,
    replacement: String,
}

pub(super) fn validate_precise_edit_contract(edits: &[Value]) -> Result<(), WorkspaceError> {
    let mut issues = Vec::new();
    for (edit_index, edit) in edits.iter().enumerate() {
        let Some(object) = edit.as_object() else {
            issues.push(json!({
                "edit_index": edit_index,
                "field": Value::Null,
                "reason": "edit_must_be_object"
            }));
            continue;
        };
        let Some(edit_type) = object.get("type").and_then(Value::as_str) else {
            issues.push(json!({
                "edit_index": edit_index,
                "field": "type",
                "reason": "type_required"
            }));
            continue;
        };

        let (allowed, required, non_empty_strings): (&[&str], &[&str], &[&str]) = match edit_type {
            "replace" => (
                &[
                    "type",
                    "old_text",
                    "new_text",
                    "match_mode",
                    "before_context",
                    "after_context",
                    "expected_occurrences",
                    "start_line",
                    "end_line",
                ],
                &["type", "old_text", "new_text"],
                &["old_text"],
            ),
            "insert_before" | "insert_after" => (
                &[
                    "type",
                    "anchor",
                    "text",
                    "match_mode",
                    "before_context",
                    "after_context",
                    "expected_occurrences",
                    "start_line",
                    "end_line",
                ],
                &["type", "anchor", "text"],
                &["anchor", "text"],
            ),
            "replace_lines" => (
                &[
                    "type",
                    "start_line",
                    "end_line",
                    "new_text",
                    "expected_text",
                ],
                &["type", "start_line", "end_line", "new_text"],
                &[],
            ),
            "delete_lines" => (
                &["type", "start_line", "end_line", "expected_text"],
                &["type", "start_line", "end_line"],
                &[],
            ),
            other => {
                issues.push(json!({
                    "edit_index": edit_index,
                    "field": "type",
                    "edit_type": other,
                    "reason": "unsupported_type",
                    "allowed_values": ["replace", "insert_before", "insert_after", "replace_lines", "delete_lines"]
                }));
                continue;
            }
        };

        for key in object.keys() {
            if !allowed.contains(&key.as_str()) {
                issues.push(json!({
                    "edit_index": edit_index,
                    "edit_type": edit_type,
                    "field": key,
                    "reason": "unexpected_field",
                    "allowed_fields": allowed
                }));
            }
        }
        for field in required {
            if !object.contains_key(*field) {
                issues.push(json!({
                    "edit_index": edit_index,
                    "edit_type": edit_type,
                    "field": field,
                    "reason": "missing_required_field"
                }));
            }
        }

        for field in [
            "old_text",
            "new_text",
            "anchor",
            "text",
            "expected_text",
            "before_context",
            "after_context",
        ] {
            if let Some(value) = object.get(field) {
                match value.as_str() {
                    Some(text) if non_empty_strings.contains(&field) && text.is_empty() => {
                        issues.push(json!({
                            "edit_index": edit_index,
                            "edit_type": edit_type,
                            "field": field,
                            "reason": "field_must_be_non_empty"
                        }));
                    }
                    Some(_) => {}
                    None => issues.push(json!({
                        "edit_index": edit_index,
                        "edit_type": edit_type,
                        "field": field,
                        "reason": "field_must_be_string"
                    })),
                }
            }
        }

        if let Some(value) = object.get("match_mode") {
            if !matches!(value.as_str(), Some("exact" | "whitespace")) {
                issues.push(json!({
                    "edit_index": edit_index,
                    "edit_type": edit_type,
                    "field": "match_mode",
                    "reason": "invalid_enum_value",
                    "allowed_values": ["exact", "whitespace"]
                }));
            }
        }
        if let Some(value) = object.get("expected_occurrences") {
            if value.as_u64().is_none_or(|count| count == 0) {
                issues.push(json!({
                    "edit_index": edit_index,
                    "edit_type": edit_type,
                    "field": "expected_occurrences",
                    "reason": "field_must_be_positive_integer"
                }));
            }
        }

        let start_line = object.get("start_line");
        let end_line = object.get("end_line");
        for (field, value) in [("start_line", start_line), ("end_line", end_line)] {
            if let Some(value) = value {
                if value.as_u64().is_none_or(|line| line == 0) {
                    issues.push(json!({
                        "edit_index": edit_index,
                        "edit_type": edit_type,
                        "field": field,
                        "reason": "field_must_be_positive_integer"
                    }));
                }
            }
        }
        if matches!(edit_type, "replace" | "insert_before" | "insert_after")
            && start_line.is_some() != end_line.is_some()
        {
            issues.push(json!({
                "edit_index": edit_index,
                "edit_type": edit_type,
                "field": "start_line,end_line",
                "reason": "line_range_pair_required"
            }));
        }
        if let (Some(start), Some(end)) = (
            start_line.and_then(Value::as_u64),
            end_line.and_then(Value::as_u64),
        ) {
            if end < start {
                issues.push(json!({
                    "edit_index": edit_index,
                    "edit_type": edit_type,
                    "field": "end_line",
                    "reason": "line_range_order_invalid",
                    "start_line": start,
                    "end_line": end
                }));
            }
        }
    }

    if issues.is_empty() {
        Ok(())
    } else {
        Err(WorkspaceError::ToolDetails {
            code: "EDIT_CONTRACT_INVALID",
            message: "Precise edit contract validation failed".into(),
            category: "validation",
            retryable: false,
            details: json!({
                "issue_count": issues.len(),
                "issues": issues,
                "suggestion": "Rebuild each edit using only the fields required by its type"
            }),
        })
    }
}

pub(super) fn apply_precise_edits(
    original: &str,
    edits: &[Value],
) -> Result<String, WorkspaceError> {
    let mut resolved = Vec::with_capacity(edits.len());
    for (index, edit) in edits.iter().enumerate() {
        resolved.extend(resolve_precise_edit(original, edit, index)?);
    }
    validate_resolved_edits(&resolved)?;

    resolved.sort_by(|left, right| {
        right
            .start_byte
            .cmp(&left.start_byte)
            .then_with(|| right.end_byte.cmp(&left.end_byte))
            .then_with(|| right.input_index.cmp(&left.input_index))
    });

    let mut content = original.to_string();
    for edit in resolved {
        content.replace_range(edit.start_byte..edit.end_byte, &edit.replacement);
    }
    Ok(content)
}

fn resolve_precise_edit(
    original: &str,
    edit: &Value,
    index: usize,
) -> Result<Vec<ResolvedEdit>, WorkspaceError> {
    let edit_type = edit.get("type").and_then(Value::as_str).ok_or_else(|| {
        WorkspaceError::invalid_argument(format!("edits[{index}].type is required"))
    })?;
    match edit_type {
        "replace" => {
            let old_text = required_edit_text(edit, index, "old_text")?;
            let requested_replacement = edit.get("new_text").and_then(Value::as_str).unwrap_or("");
            let targets = resolve_text_targets(original, edit, old_text, index)?;
            Ok(targets
                .into_iter()
                .map(|(start_byte, end_byte)| ResolvedEdit {
                    input_index: index,
                    start_byte,
                    end_byte,
                    replacement: adapt_newlines_to_range(
                        requested_replacement,
                        original,
                        start_byte,
                        end_byte,
                    ),
                })
                .collect())
        }
        "insert_before" | "insert_after" => {
            let anchor = required_edit_text(edit, index, "anchor")?;
            let requested_text = required_edit_text(edit, index, "text")?;
            let targets = resolve_text_targets(original, edit, anchor, index)?;
            Ok(targets
                .into_iter()
                .map(|(start, end)| {
                    let position = if edit_type == "insert_before" {
                        start
                    } else {
                        end
                    };
                    ResolvedEdit {
                        input_index: index,
                        start_byte: position,
                        end_byte: position,
                        replacement: adapt_newlines_to_range(requested_text, original, start, end),
                    }
                })
                .collect())
        }
        "replace_lines" | "delete_lines" => {
            let start_line = required_line(edit, index, "start_line")?;
            let end_line = required_line(edit, index, "end_line")?;
            let range = line_edit_range(original, start_line, end_line, index)?;
            if let Some(expected) = edit.get("expected_text").and_then(Value::as_str) {
                let actual = &original[range.start_byte..range.content_end];
                if normalize_newlines(actual) != normalize_newlines(expected) {
                    return Err(WorkspaceError::ToolDetails {
                        code: "EDIT_EXPECTED_TEXT_MISMATCH",
                        message: format!(
                            "edits[{index}] line range content did not match expected_text"
                        ),
                        category: "conflict",
                        retryable: true,
                        details: json!({
                            "edit_index": index,
                            "start_line": start_line,
                            "end_line": end_line,
                            "actual_text": actual
                        }),
                    });
                }
            }
            if edit_type == "delete_lines" {
                let delete_start =
                    if range.trailing_newline_len > 0 || range.preceding_newline_len == 0 {
                        range.start_byte
                    } else {
                        range.start_byte - range.preceding_newline_len
                    };
                Ok(vec![ResolvedEdit {
                    input_index: index,
                    start_byte: delete_start,
                    end_byte: range.end_byte,
                    replacement: String::new(),
                }])
            } else {
                Ok(vec![ResolvedEdit {
                    input_index: index,
                    start_byte: range.start_byte,
                    end_byte: range.content_end,
                    replacement: adapt_newlines_to_range(
                        edit.get("new_text").and_then(Value::as_str).unwrap_or(""),
                        original,
                        range.start_byte,
                        range.end_byte,
                    ),
                }])
            }
        }
        other => Err(WorkspaceError::invalid_argument(format!(
            "Unsupported edits[{index}].type: {other}"
        ))),
    }
}

fn resolve_text_targets(
    original: &str,
    edit: &Value,
    target: &str,
    index: usize,
) -> Result<Vec<(usize, usize)>, WorkspaceError> {
    let before_context = edit.get("before_context").and_then(Value::as_str);
    let after_context = edit.get("after_context").and_then(Value::as_str);
    let start_line = edit
        .get("start_line")
        .and_then(Value::as_u64)
        .map(|v| v as usize);
    let end_line = edit
        .get("end_line")
        .and_then(Value::as_u64)
        .map(|v| v as usize);

    let search_range = match (start_line, end_line) {
        (None, None) => (0, original.len()),
        (Some(start), Some(end)) => line_range_bytes(original, start, end, index)?,
        _ => {
            return Err(WorkspaceError::invalid_argument(format!(
                "edits[{index}].start_line and end_line must be provided together"
            )))
        }
    };

    let match_mode = edit
        .get("match_mode")
        .and_then(Value::as_str)
        .unwrap_or("exact");
    let candidates = match match_mode {
        "exact" => exact_text_candidates(original, target, search_range),
        "whitespace" => whitespace_text_candidates(original, target, search_range, index)?,
        other => {
            return Err(WorkspaceError::invalid_argument(format!(
                "edits[{index}].match_mode must be exact or whitespace, got {other}"
            )))
        }
    }
    .into_iter()
    .filter(|(start, end)| {
        context_matches(
            original,
            *start,
            *end,
            before_context,
            after_context,
            match_mode,
        )
    })
    .collect::<Vec<_>>();

    let expected = expected_occurrences(edit);
    if candidates.len() != expected {
        return Err(WorkspaceError::ToolDetails {
            code: "EDIT_MATCH_COUNT_MISMATCH",
            message: format!(
                "edits[{index}] expected {expected} guarded matches but found {}",
                candidates.len()
            ),
            category: "validation",
            retryable: false,
            details: json!({
                "edit_index": index,
                "expected_occurrences": expected,
                "actual_occurrences": candidates.len(),
                "candidate_lines": candidates.iter().map(|(start, _)| byte_to_line(original, *start)).collect::<Vec<_>>(),
                "candidate_ranges": candidates.iter().map(|(start, end)| json!({
                    "start_line": byte_to_line(original, *start),
                    "end_line": byte_to_line(original, end.saturating_sub(1))
                })).collect::<Vec<_>>(),
                "candidate_contexts": text_candidate_contexts(original, &candidates, 3),
                "candidate_context_limit": 8,
                "candidate_contexts_truncated": candidates.len() > 8,
                "recovery_reason": if candidates.is_empty() {
                    "target_text_not_found"
                } else {
                    "target_text_not_unique"
                }
            }),
        });
    }
    Ok(candidates)
}

fn text_candidate_contexts(
    original: &str,
    candidates: &[(usize, usize)],
    radius: usize,
) -> Vec<Value> {
    let lines = original
        .split('\n')
        .map(|line| line.strip_suffix('\r').unwrap_or(line).to_string())
        .collect::<Vec<_>>();
    candidates
        .iter()
        .take(8)
        .map(|(start, end)| {
            let match_start = byte_to_line(original, *start);
            let match_end = byte_to_line(original, end.saturating_sub(1));
            let context_start = match_start.saturating_sub(radius).max(1);
            let context_end = match_end.saturating_add(radius).min(lines.len());
            json!({
                "start_line": match_start,
                "end_line": match_end,
                "context_start_line": context_start,
                "context_end_line": context_end,
                "preview": lines[context_start - 1..context_end]
            })
        })
        .collect()
}

fn exact_text_candidates(
    original: &str,
    target: &str,
    search_range: (usize, usize),
) -> Vec<(usize, usize)> {
    let haystack = &original[search_range.0..search_range.1];
    if !target.contains('\n') {
        return haystack
            .match_indices(target)
            .map(|(offset, _)| {
                let start = search_range.0 + offset;
                (start, start + target.len())
            })
            .collect();
    }

    let normalized_target = normalize_newlines(target);
    let (normalized_haystack, original_boundaries) = normalize_newlines_with_boundary_map(haystack);
    normalized_haystack
        .match_indices(&normalized_target)
        .map(|(normalized_start, matched)| {
            let normalized_end = normalized_start + matched.len();
            (
                search_range.0 + original_boundaries[normalized_start],
                search_range.0 + original_boundaries[normalized_end],
            )
        })
        .collect()
}

fn normalize_newlines_with_boundary_map(value: &str) -> (String, Vec<usize>) {
    let bytes = value.as_bytes();
    let mut normalized = Vec::with_capacity(bytes.len());
    let mut original_boundaries = Vec::with_capacity(bytes.len() + 1);
    original_boundaries.push(0);

    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] == b'\r' && bytes.get(index + 1) == Some(&b'\n') {
            normalized.push(b'\n');
            index += 2;
        } else {
            normalized.push(bytes[index]);
            index += 1;
        }
        original_boundaries.push(index);
    }

    (
        String::from_utf8(normalized).expect("normalizing CRLF preserves valid UTF-8"),
        original_boundaries,
    )
}

pub(super) fn whitespace_text_candidates(
    original: &str,
    target: &str,
    search_range: (usize, usize),
    edit_index: usize,
) -> Result<Vec<(usize, usize)>, WorkspaceError> {
    let pattern = whitespace_flexible_pattern(target);
    let regex = Regex::new(&pattern).map_err(|error| {
        WorkspaceError::invalid_argument(format!(
            "edits[{edit_index}] could not build whitespace matcher: {error}"
        ))
    })?;
    Ok(regex
        .find_iter(&original[search_range.0..search_range.1])
        .map(|matched| {
            (
                search_range.0 + matched.start(),
                search_range.0 + matched.end(),
            )
        })
        .collect())
}

fn whitespace_flexible_pattern(target: &str) -> String {
    let mut pattern = String::new();
    let mut literal = String::new();
    let mut in_whitespace = false;
    for character in target.chars() {
        if character.is_whitespace() {
            if !literal.is_empty() {
                pattern.push_str(&regex::escape(&literal));
                literal.clear();
            }
            if !in_whitespace {
                pattern.push_str(r"\s+");
                in_whitespace = true;
            }
        } else {
            literal.push(character);
            in_whitespace = false;
        }
    }
    if !literal.is_empty() {
        pattern.push_str(&regex::escape(&literal));
    }
    pattern
}

fn context_matches(
    original: &str,
    start: usize,
    end: usize,
    before_context: Option<&str>,
    after_context: Option<&str>,
    match_mode: &str,
) -> bool {
    let before_matches = before_context.is_none_or(|before| match match_mode {
        "whitespace" => flexible_suffix_matches(&original[..start], before),
        _ => newline_flexible_suffix_matches(&original[..start], before),
    });
    let after_matches = after_context.is_none_or(|after| match match_mode {
        "whitespace" => flexible_prefix_matches(&original[end..], after),
        _ => newline_flexible_prefix_matches(&original[end..], after),
    });
    before_matches && after_matches
}

fn newline_flexible_suffix_matches(haystack: &str, expected: &str) -> bool {
    if !expected.contains('\n') {
        return haystack.ends_with(expected);
    }

    let normalized_expected = normalize_newlines(expected);
    let haystack = haystack.as_bytes();
    let expected = normalized_expected.as_bytes();
    let mut haystack_index = haystack.len();
    let mut expected_index = expected.len();

    while expected_index > 0 {
        let expected_byte = expected[expected_index - 1];
        if expected_byte == b'\n' {
            if haystack_index >= 2
                && haystack[haystack_index - 2] == b'\r'
                && haystack[haystack_index - 1] == b'\n'
            {
                haystack_index -= 2;
            } else if haystack_index >= 1 && haystack[haystack_index - 1] == b'\n' {
                haystack_index -= 1;
            } else {
                return false;
            }
        } else if haystack_index >= 1 && haystack[haystack_index - 1] == expected_byte {
            haystack_index -= 1;
        } else {
            return false;
        }
        expected_index -= 1;
    }

    true
}

fn newline_flexible_prefix_matches(haystack: &str, expected: &str) -> bool {
    if !expected.contains('\n') {
        return haystack.starts_with(expected);
    }

    let normalized_expected = normalize_newlines(expected);
    let haystack = haystack.as_bytes();
    let expected = normalized_expected.as_bytes();
    let mut haystack_index = 0;
    let mut expected_index = 0;

    while expected_index < expected.len() {
        let expected_byte = expected[expected_index];
        if expected_byte == b'\n' {
            if haystack.get(haystack_index) == Some(&b'\r')
                && haystack.get(haystack_index + 1) == Some(&b'\n')
            {
                haystack_index += 2;
            } else if haystack.get(haystack_index) == Some(&b'\n') {
                haystack_index += 1;
            } else {
                return false;
            }
        } else if haystack.get(haystack_index) == Some(&expected_byte) {
            haystack_index += 1;
        } else {
            return false;
        }
        expected_index += 1;
    }

    true
}

fn flexible_suffix_matches(haystack: &str, expected: &str) -> bool {
    Regex::new(&format!(r"(?:{})$", whitespace_flexible_pattern(expected)))
        .is_ok_and(|regex| regex.is_match(haystack))
}

fn flexible_prefix_matches(haystack: &str, expected: &str) -> bool {
    Regex::new(&format!(r"^(?:{})", whitespace_flexible_pattern(expected)))
        .is_ok_and(|regex| regex.is_match(haystack))
}

fn validate_resolved_edits(edits: &[ResolvedEdit]) -> Result<(), WorkspaceError> {
    for (i, left) in edits.iter().enumerate() {
        for right in edits.iter().skip(i + 1) {
            let overlap = left.start_byte < right.end_byte && right.start_byte < left.end_byte;
            let insertion_inside = (left.start_byte == left.end_byte
                && left.start_byte > right.start_byte
                && left.start_byte < right.end_byte)
                || (right.start_byte == right.end_byte
                    && right.start_byte > left.start_byte
                    && right.start_byte < left.end_byte);
            if overlap || insertion_inside {
                return Err(WorkspaceError::ToolDetails {
                    code: "EDIT_RANGES_OVERLAP",
                    message: format!(
                        "edits[{}] overlaps edits[{}] on the original file",
                        left.input_index, right.input_index
                    ),
                    category: "validation",
                    retryable: false,
                    details: json!({
                        "first_edit_index": left.input_index,
                        "second_edit_index": right.input_index,
                        "first_range": [left.start_byte, left.end_byte],
                        "second_range": [right.start_byte, right.end_byte]
                    }),
                });
            }
        }
    }
    Ok(())
}

pub(super) fn required_edit_text<'a>(
    edit: &'a Value,
    index: usize,
    key: &str,
) -> Result<&'a str, WorkspaceError> {
    let value = edit.get(key).and_then(Value::as_str).ok_or_else(|| {
        WorkspaceError::invalid_argument(format!("edits[{index}].{key} is required"))
    })?;
    if value.is_empty() {
        return Err(WorkspaceError::invalid_argument(format!(
            "edits[{index}].{key} must not be empty"
        )));
    }
    Ok(value)
}

fn required_line(edit: &Value, index: usize, key: &str) -> Result<usize, WorkspaceError> {
    edit.get(key)
        .and_then(Value::as_u64)
        .filter(|line| *line > 0)
        .map(|line| line as usize)
        .ok_or_else(|| {
            WorkspaceError::invalid_argument(format!(
                "edits[{index}].{key} must be a positive integer"
            ))
        })
}

pub(super) fn expected_occurrences(edit: &Value) -> usize {
    edit.get("expected_occurrences")
        .and_then(Value::as_u64)
        .unwrap_or(1)
        .max(1) as usize
}

fn normalize_newlines(value: &str) -> String {
    value.replace("\r\n", "\n")
}

fn newline_at(value: &str, newline_index: usize) -> &'static str {
    if newline_index > 0 && value.as_bytes()[newline_index - 1] == b'\r' {
        "\r\n"
    } else {
        "\n"
    }
}

pub(super) fn preferred_line_ending_near_range(
    original: &str,
    start: usize,
    end: usize,
) -> &'static str {
    let bytes = original.as_bytes();
    let bounded_start = start.min(bytes.len());
    let bounded_end = end.max(bounded_start).min(bytes.len());

    if let Some(offset) = bytes[bounded_start..bounded_end]
        .iter()
        .position(|byte| *byte == b'\n')
    {
        return newline_at(original, bounded_start + offset);
    }
    if let Some(offset) = bytes[bounded_end..].iter().position(|byte| *byte == b'\n') {
        return newline_at(original, bounded_end + offset);
    }
    if let Some(index) = bytes[..bounded_start]
        .iter()
        .rposition(|byte| *byte == b'\n')
    {
        return newline_at(original, index);
    }
    "\n"
}

pub(super) fn adapt_newlines_to_range(
    value: &str,
    original: &str,
    start: usize,
    end: usize,
) -> String {
    let normalized = normalize_newlines(value);
    if preferred_line_ending_near_range(original, start, end) == "\r\n" {
        normalized.replace('\n', "\r\n")
    } else {
        normalized
    }
}

pub(super) fn newline_style(value: &str) -> &'static str {
    let bytes = value.as_bytes();
    let mut has_crlf = false;
    let mut has_lf = false;
    for (index, byte) in bytes.iter().enumerate() {
        if *byte != b'\n' {
            continue;
        }
        if index > 0 && bytes[index - 1] == b'\r' {
            has_crlf = true;
        } else {
            has_lf = true;
        }
    }
    match (has_crlf, has_lf) {
        (true, true) => "mixed",
        (true, false) => "crlf",
        (false, true) => "lf",
        (false, false) => "none",
    }
}

pub(super) fn line_range_bytes(
    content: &str,
    start_line: usize,
    end_line: usize,
    edit_index: usize,
) -> Result<(usize, usize), WorkspaceError> {
    let mut starts = vec![0usize];
    for (index, byte) in content.bytes().enumerate() {
        if byte == b'\n' {
            starts.push(index + 1);
        }
    }
    let total_lines = starts.len();
    if start_line == 0 || start_line > end_line || end_line > total_lines {
        return Err(WorkspaceError::ToolDetails {
            code: "EDIT_LINE_RANGE_INVALID",
            message: format!(
                "edits[{edit_index}] line range {start_line}-{end_line} is outside 1-{total_lines}"
            ),
            category: "validation",
            retryable: false,
            details: json!({
                "edit_index": edit_index,
                "start_line": start_line,
                "end_line": end_line,
                "total_lines": total_lines
            }),
        });
    }
    let start = starts[start_line - 1];
    let end = if end_line < total_lines {
        starts[end_line]
    } else {
        content.len()
    };
    Ok((start, end))
}

#[derive(Debug, Clone, Copy)]
struct LineEditRange {
    start_byte: usize,
    end_byte: usize,
    content_end: usize,
    trailing_newline_len: usize,
    preceding_newline_len: usize,
}

fn newline_ending_len_at(value: &str, end_offset: usize) -> usize {
    if end_offset == 0 || value.as_bytes()[end_offset - 1] != b'\n' {
        return 0;
    }
    if end_offset > 1 && value.as_bytes()[end_offset - 2] == b'\r' {
        2
    } else {
        1
    }
}

fn line_edit_range(
    content: &str,
    start_line: usize,
    end_line: usize,
    edit_index: usize,
) -> Result<LineEditRange, WorkspaceError> {
    let (start_byte, end_byte) = line_range_bytes(content, start_line, end_line, edit_index)?;
    let trailing_newline_len = if end_byte > start_byte {
        newline_ending_len_at(content, end_byte)
    } else {
        0
    };
    let preceding_newline_len = newline_ending_len_at(content, start_byte);
    Ok(LineEditRange {
        start_byte,
        end_byte,
        content_end: end_byte - trailing_newline_len,
        trailing_newline_len,
        preceding_newline_len,
    })
}

#[derive(Debug, Clone)]
struct LineEndingToken<'a> {
    content: &'a str,
    eol: &'static str,
}

fn line_ending_tokens(value: &str) -> Vec<LineEndingToken<'_>> {
    let bytes = value.as_bytes();
    let mut tokens = Vec::new();
    let mut start = 0usize;
    while start < bytes.len() {
        let Some(offset) = bytes[start..].iter().position(|byte| *byte == b'\n') else {
            tokens.push(LineEndingToken {
                content: &value[start..],
                eol: "",
            });
            break;
        };
        let newline = start + offset;
        let crlf = newline > start && bytes[newline - 1] == b'\r';
        tokens.push(LineEndingToken {
            content: &value[start..if crlf { newline - 1 } else { newline }],
            eol: if crlf { "\r\n" } else { "\n" },
        });
        start = newline + 1;
    }
    if bytes.last() == Some(&b'\n') {
        tokens.push(LineEndingToken {
            content: "",
            eol: "",
        });
    }
    tokens
}

pub(super) fn unchanged_line_ending_changes(original: &str, updated: &str) -> Vec<Value> {
    let before = line_ending_tokens(original);
    let after = line_ending_tokens(updated);
    let mut before_counts = HashMap::<&str, usize>::new();
    let mut after_counts = HashMap::<&str, usize>::new();
    for token in &before {
        *before_counts.entry(token.content).or_default() += 1;
    }
    for token in &after {
        *after_counts.entry(token.content).or_default() += 1;
    }
    let unambiguous = |content: &str| {
        before_counts.get(content) == Some(&1) && after_counts.get(content) == Some(&1)
    };

    let limit = before.len().min(after.len());
    let mut prefix = 0usize;
    while prefix < limit && before[prefix].content == after[prefix].content {
        prefix += 1;
    }
    let mut suffix = 0usize;
    while suffix < limit - prefix
        && before[before.len() - 1 - suffix].content == after[after.len() - 1 - suffix].content
    {
        suffix += 1;
    }

    let mut changes = Vec::new();
    for index in 0..prefix {
        if before[index].eol == after[index].eol || !unambiguous(before[index].content) {
            continue;
        }
        let allowed_final_deletion_boundary = index == prefix - 1
            && index == after.len() - 1
            && after[index].eol.is_empty()
            && before.len() > after.len();
        if !allowed_final_deletion_boundary {
            changes.push(json!({
                "before_line": index + 1,
                "after_line": index + 1,
                "before_eol": before[index].eol,
                "after_eol": after[index].eol
            }));
        }
    }
    for offset in 0..suffix {
        let before_index = before.len() - 1 - offset;
        let after_index = after.len() - 1 - offset;
        if before[before_index].eol == after[after_index].eol
            || !unambiguous(before[before_index].content)
        {
            continue;
        }
        changes.push(json!({
            "before_line": before_index + 1,
            "after_line": after_index + 1,
            "before_eol": before[before_index].eol,
            "after_eol": after[after_index].eol
        }));
    }
    changes.sort_by_key(|change| {
        (
            change["before_line"].as_u64().unwrap_or(0),
            change["after_line"].as_u64().unwrap_or(0),
        )
    });
    changes
}

pub(super) fn assert_no_unexpected_newline_churn(
    file: &str,
    original: &str,
    updated: &str,
) -> Result<(), WorkspaceError> {
    let changes = unchanged_line_ending_changes(original, updated);
    if changes.is_empty() {
        return Ok(());
    }
    let change_count = changes.len();
    let changes_truncated = change_count > 20;
    let bounded_changes = changes.into_iter().take(20).collect::<Vec<_>>();
    Err(WorkspaceError::ToolDetails {
        code: "EDIT_NEWLINE_CHURN",
        message: format!("Edit would change line endings on unchanged content in {file}"),
        category: "validation",
        retryable: false,
        details: json!({
            "path": file,
            "newline_before": newline_style(original),
            "newline_after": newline_style(updated),
            "unchanged_line_ending_change_count": change_count,
            "unchanged_line_ending_changes": bounded_changes,
            "changes_truncated": changes_truncated,
            "suggestion": "Use a precise target range; line-ending normalization must not rewrite untouched content."
        }),
    })
}

fn semantic_lines(value: &str) -> Vec<&str> {
    value
        .split('\n')
        .map(|line| line.strip_suffix('\r').unwrap_or(line))
        .collect()
}

fn edit_text_line_units(value: Option<&str>) -> usize {
    value
        .filter(|text| !text.is_empty())
        .map(|text| semantic_lines(text).len())
        .unwrap_or(0)
}

fn expected_edit_line_budget(edits: &[Value]) -> usize {
    let mut budget = 0usize;
    for edit in edits {
        let occurrences = edit
            .get("expected_occurrences")
            .and_then(Value::as_u64)
            .unwrap_or(1)
            .max(1) as usize;
        match edit.get("type").and_then(Value::as_str).unwrap_or("") {
            "replace" => {
                budget = budget.saturating_add(
                    occurrences.saturating_mul(
                        edit_text_line_units(edit.get("old_text").and_then(Value::as_str))
                            .saturating_add(edit_text_line_units(
                                edit.get("new_text").and_then(Value::as_str),
                            )),
                    ),
                );
            }
            "insert_before" | "insert_after" => {
                budget = budget.saturating_add(occurrences.saturating_mul(edit_text_line_units(
                    edit.get("text").and_then(Value::as_str),
                )));
            }
            "replace_lines" => {
                let start = edit.get("start_line").and_then(Value::as_u64).unwrap_or(1);
                let end = edit
                    .get("end_line")
                    .and_then(Value::as_u64)
                    .unwrap_or(start);
                let removed = end.saturating_sub(start).saturating_add(1) as usize;
                budget = budget.saturating_add(removed.saturating_add(edit_text_line_units(
                    edit.get("new_text").and_then(Value::as_str),
                )));
            }
            "delete_lines" => {
                let start = edit.get("start_line").and_then(Value::as_u64).unwrap_or(1);
                let end = edit
                    .get("end_line")
                    .and_then(Value::as_u64)
                    .unwrap_or(start);
                budget =
                    budget.saturating_add(end.saturating_sub(start).saturating_add(1) as usize);
            }
            _ => budget = budget.saturating_add(1),
        }
    }
    budget.max(1)
}

fn bounded_line_edit_distance(
    before: &[&str],
    after: &[&str],
    max_distance: usize,
) -> Option<usize> {
    if before.len().abs_diff(after.len()) > max_distance {
        return None;
    }
    let limit = max_distance.min(before.len().saturating_add(after.len()));
    let mut frontier = HashMap::<isize, usize>::from([(1, 0)]);
    for distance in 0..=limit {
        let mut next = HashMap::<isize, usize>::new();
        let distance_i = distance as isize;
        let mut diagonal = -distance_i;
        while diagonal <= distance_i {
            let down = frontier.get(&(diagonal + 1)).copied();
            let right = frontier.get(&(diagonal - 1)).copied();
            let down_rank = down.map(|value| value as isize).unwrap_or(-1);
            let right_rank = right.map(|value| value as isize).unwrap_or(-1);
            let mut x =
                if diagonal == -distance_i || (diagonal != distance_i && right_rank < down_rank) {
                    down.unwrap_or(0)
                } else {
                    right.unwrap_or(0).saturating_add(1)
                };
            let mut y = x as isize - diagonal;
            while x < before.len()
                && y >= 0
                && (y as usize) < after.len()
                && before[x] == after[y as usize]
            {
                x += 1;
                y += 1;
            }
            if x >= before.len() && y >= after.len() as isize {
                return Some(distance);
            }
            next.insert(diagonal, x);
            diagonal += 2;
        }
        frontier = next;
    }
    None
}

pub(super) fn edit_blast_radius(original: &str, updated: &str, edits: &[Value]) -> Value {
    let before = semantic_lines(original);
    let after = semantic_lines(updated);
    let limit = before.len().min(after.len());
    let mut prefix = 0usize;
    while prefix < limit && before[prefix] == after[prefix] {
        prefix += 1;
    }
    let mut suffix = 0usize;
    while suffix < limit - prefix
        && before[before.len() - 1 - suffix] == after[after.len() - 1 - suffix]
    {
        suffix += 1;
    }
    let changed_before = before.len().saturating_sub(prefix + suffix);
    let changed_after = after.len().saturating_sub(prefix + suffix);
    let span_changed_line_count = changed_before.saturating_add(changed_after);
    let expected_budget = if edits.is_empty() {
        span_changed_line_count.max(1)
    } else {
        expected_edit_line_budget(edits)
    };
    let allowed_changed_line_count = 40usize.max(expected_budget.saturating_mul(12));
    let bounded_distance =
        if !edits.is_empty() && span_changed_line_count > allowed_changed_line_count {
            bounded_line_edit_distance(&before, &after, allowed_changed_line_count)
        } else {
            None
        };
    let changed_line_count = bounded_distance.unwrap_or(span_changed_line_count);
    let change_measure = if bounded_distance.is_some() {
        "bounded_line_diff"
    } else {
        "prefix_suffix"
    };
    let denominator = before.len().saturating_add(after.len()).max(1);
    let change_ratio = changed_line_count as f64 / denominator as f64;
    let excessive = before.len().max(after.len()) >= 80
        && changed_line_count > allowed_changed_line_count
        && change_ratio >= 0.6;
    json!({
        "before_line_count": before.len(),
        "after_line_count": after.len(),
        "changed_before_lines": changed_before,
        "changed_after_lines": changed_after,
        "changed_line_count": changed_line_count,
        "expected_change_line_budget": expected_budget,
        "allowed_changed_line_count": allowed_changed_line_count,
        "change_ratio": (change_ratio * 10_000.0).round() / 10_000.0,
        "change_measure": change_measure,
        "span_changed_line_count": span_changed_line_count,
        "excessive": excessive
    })
}

pub(super) fn assert_no_unexpected_edit_blast_radius(
    file: &str,
    original: &str,
    updated: &str,
    edits: &[Value],
) -> Result<Value, WorkspaceError> {
    let radius = edit_blast_radius(original, updated, edits);
    if radius["excessive"].as_bool() != Some(true) {
        return Ok(radius);
    }
    let mut details = radius.as_object().cloned().unwrap_or_default();
    details.insert("path".into(), json!(file));
    details.insert(
        "suggestion".into(),
        json!("Read the target again and use a narrower precise edit. Large intentional rewrites should describe the full replacement in the edit contract."),
    );
    Err(WorkspaceError::ToolDetails {
        code: "EDIT_BLAST_RADIUS",
        message: format!(
            "Edit would change far more content than its guarded edit contract in {file}"
        ),
        category: "validation",
        retryable: false,
        details: Value::Object(details),
    })
}

pub(super) fn byte_to_line(content: &str, byte: usize) -> usize {
    content[..byte]
        .bytes()
        .filter(|value| *value == b'\n')
        .count()
        + 1
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn apply(original: &str, edits: Vec<Value>) -> String {
        apply_precise_edits(original, &edits).expect("precise edit should succeed")
    }

    #[test]
    fn replace_lines_preserves_mixed_line_endings_and_uses_local_eol() {
        let original = "alpha\r\nbeta\r\ngamma\ndelta\n";
        let updated = apply(
            original,
            vec![json!({
                "type": "replace_lines",
                "start_line": 3,
                "end_line": 3,
                "expected_text": "gamma",
                "new_text": "GAMMA\nSECOND"
            })],
        );
        assert_eq!(updated, "alpha\r\nbeta\r\nGAMMA\nSECOND\ndelta\n");
        assert_no_unexpected_newline_churn("mixed.txt", original, &updated).unwrap();
    }

    #[test]
    fn text_replacement_uses_target_local_eol_in_mixed_file() {
        let original = "first\nsecond\r\nthird\r\n";
        let updated = apply(
            original,
            vec![json!({
                "type": "replace",
                "old_text": "first",
                "new_text": "FIRST\nEXTRA"
            })],
        );
        assert_eq!(updated, "FIRST\nEXTRA\nsecond\r\nthird\r\n");
        assert_no_unexpected_newline_churn("mixed.txt", original, &updated).unwrap();
    }

    #[test]
    fn replace_lines_keeps_crlf_only_files_crlf() {
        let original = "alpha\r\nbeta\r\ngamma\r\n";
        let updated = apply(
            original,
            vec![json!({
                "type": "replace_lines",
                "start_line": 2,
                "end_line": 2,
                "new_text": "BETA\nSECOND"
            })],
        );
        assert_eq!(updated, "alpha\r\nBETA\r\nSECOND\r\ngamma\r\n");
    }

    #[test]
    fn replace_lines_preserves_unterminated_final_line() {
        let original = "alpha\nbeta";
        let updated = apply(
            original,
            vec![json!({
                "type": "replace_lines",
                "start_line": 2,
                "end_line": 2,
                "new_text": "BETA"
            })],
        );
        assert_eq!(updated, "alpha\nBETA");
    }

    #[test]
    fn delete_lines_preserves_normal_middle_line_semantics() {
        let original = "alpha\nbeta\ngamma\n";
        let updated = apply(
            original,
            vec![json!({
                "type": "delete_lines",
                "start_line": 2,
                "end_line": 2
            })],
        );
        assert_eq!(updated, "alpha\ngamma\n");
    }

    #[test]
    fn delete_lines_removes_delimiter_before_unterminated_final_line() {
        let original = "alpha\nbeta\r\ngamma";
        let updated = apply(
            original,
            vec![json!({
                "type": "delete_lines",
                "start_line": 3,
                "end_line": 3,
                "expected_text": "gamma"
            })],
        );
        assert_eq!(updated, "alpha\nbeta");
        assert_no_unexpected_newline_churn("mixed.txt", original, &updated).unwrap();
    }

    #[test]
    fn delete_lines_keeps_preceding_newline_for_terminated_final_line() {
        let original = "alpha\nbeta\n";
        let updated = apply(
            original,
            vec![json!({
                "type": "delete_lines",
                "start_line": 2,
                "end_line": 2
            })],
        );
        assert_eq!(updated, "alpha\n");
    }

    #[test]
    fn edit_blast_radius_rejects_whole_file_churn_from_a_narrow_contract() {
        let original = (1..=200)
            .map(|index| format!("line-{index}"))
            .collect::<Vec<_>>()
            .join("\n")
            + "\n";
        let updated = (1..=200)
            .map(|index| format!("rewritten-{index}"))
            .collect::<Vec<_>>()
            .join("\n")
            + "\n";
        let edits = vec![json!({
            "type": "replace",
            "old_text": "line-100",
            "new_text": "LINE-100"
        })];
        let radius = edit_blast_radius(&original, &updated, &edits);
        assert_eq!(radius["excessive"], true);
        let error =
            assert_no_unexpected_edit_blast_radius("large.txt", &original, &updated, &edits)
                .expect_err("semantic churn must be rejected");
        assert_eq!(error.to_error_value()["code"], "EDIT_BLAST_RADIUS");
    }

    #[test]
    fn edit_blast_radius_allows_precise_local_change() {
        let original = (1..=200)
            .map(|index| format!("line-{index}"))
            .collect::<Vec<_>>()
            .join("\n")
            + "\n";
        let updated = original.replacen("line-100", "LINE-100", 1);
        let edits = vec![json!({
            "type": "replace",
            "old_text": "line-100",
            "new_text": "LINE-100"
        })];
        let radius = edit_blast_radius(&original, &updated, &edits);
        assert_eq!(radius["excessive"], false);
        assert_eq!(radius["changed_line_count"], 2);
    }

    #[test]
    fn edit_blast_radius_measures_disjoint_precise_edits() {
        let original = (1..=200)
            .map(|index| format!("line-{index}"))
            .collect::<Vec<_>>()
            .join("\n")
            + "\n";
        let updated = original
            .replacen("line-10", "LINE-10", 1)
            .replacen("line-190", "LINE-190", 1);
        let edits = vec![
            json!({
                "type": "replace",
                "old_text": "line-10",
                "new_text": "LINE-10"
            }),
            json!({
                "type": "replace",
                "old_text": "line-190",
                "new_text": "LINE-190"
            }),
        ];
        let radius = edit_blast_radius(&original, &updated, &edits);
        assert_eq!(radius["excessive"], false);
        assert_eq!(radius["changed_line_count"], 4);
        assert_eq!(radius["change_measure"], "bounded_line_diff");
    }

    #[test]
    fn newline_churn_detector_identifies_unchanged_content_eol_changes() {
        let original = "one\ntwo\r\nthree\nfour\r\nfive\n";
        let updated = "one\r\ntwo\r\nthree\r\nfour\nfive\r\n";
        let changes = unchanged_line_ending_changes(original, updated);
        assert_eq!(
            changes
                .iter()
                .map(|change| change["before_line"].as_u64().unwrap())
                .collect::<Vec<_>>(),
            vec![1, 3, 4, 5]
        );
        let error = assert_no_unexpected_newline_churn("mixed.txt", original, updated)
            .expect_err("newline churn must be rejected");
        assert_eq!(error.to_error_value()["code"], "EDIT_NEWLINE_CHURN");
    }

    #[test]
    fn newline_churn_detector_allows_final_deletion_boundary_change() {
        assert!(unchanged_line_ending_changes("alpha\nbeta", "alpha").is_empty());
        assert_no_unexpected_newline_churn("final.txt", "alpha\nbeta", "alpha").unwrap();
    }

    #[test]
    fn newline_churn_detector_does_not_misalign_repeated_content() {
        let original = "same\nmiddle\r\nsame\r\n";
        let updated = apply(
            original,
            vec![json!({
                "type": "delete_lines",
                "start_line": 1,
                "end_line": 1
            })],
        );
        assert_eq!(updated, "middle\r\nsame\r\n");
        assert_no_unexpected_newline_churn("repeat.txt", original, &updated).unwrap();
    }

    #[test]
    fn trailing_empty_line_is_addressable_like_node() {
        let original = "alpha\nbeta\n";
        assert_eq!(line_range_bytes(original, 3, 3, 0).unwrap(), (11, 11));

        let replaced = apply(
            original,
            vec![json!({
                "type": "replace_lines",
                "start_line": 3,
                "end_line": 3,
                "new_text": "gamma"
            })],
        );
        assert_eq!(replaced, "alpha\nbeta\ngamma");
        assert_no_unexpected_newline_churn("trailing.txt", original, &replaced).unwrap();

        let deleted = apply(
            original,
            vec![json!({
                "type": "delete_lines",
                "start_line": 3,
                "end_line": 3
            })],
        );
        assert_eq!(deleted, "alpha\nbeta");
        assert_no_unexpected_newline_churn("trailing.txt", original, &deleted).unwrap();
    }

    #[test]
    fn newline_style_reports_lf_crlf_mixed_and_none() {
        assert_eq!(newline_style("alpha\nbeta\n"), "lf");
        assert_eq!(newline_style("alpha\r\nbeta\r\n"), "crlf");
        assert_eq!(newline_style("alpha\nbeta\r\n"), "mixed");
        assert_eq!(newline_style("alpha"), "none");
    }
}
