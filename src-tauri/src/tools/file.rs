use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use ignore::WalkBuilder;
use regex::Regex;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use walkdir::WalkDir;

use crate::tools::workspace::{relative_display, tool_ok, Workspace, WorkspaceError};

pub fn read_file(ws: &Workspace, args: &Value) -> Result<Value, WorkspaceError> {
    let path = args
        .get("path")
        .and_then(Value::as_str)
        .ok_or_else(|| WorkspaceError::invalid_argument("path is required"))?;
    let max_bytes = args
        .get("max_bytes")
        .and_then(Value::as_u64)
        .unwrap_or(131_072) as usize;
    let start_line = args
        .get("start_line")
        .and_then(Value::as_u64)
        .unwrap_or(1)
        .max(1) as usize;
    let end_line = args
        .get("end_line")
        .and_then(Value::as_u64)
        .map(|v| v as usize);

    read_file_value(ws, path, start_line, end_line, max_bytes)
}

pub fn read_many(ws: &Workspace, args: &Value) -> Result<Value, WorkspaceError> {
    let mut requests = Vec::new();
    if let Some(items) = args.get("items").and_then(Value::as_array) {
        for (index, item) in items.iter().enumerate() {
            requests.push(read_request_from_value(item, index, 0)?);
        }
    }
    let context_lines = args
        .get("context_lines")
        .and_then(Value::as_u64)
        .unwrap_or(20) as usize;
    if let Some(matches) = args.get("matches").and_then(Value::as_array) {
        let base_index = requests.len();
        for (offset, item) in matches.iter().enumerate() {
            requests.push(read_request_from_value(
                item,
                base_index + offset,
                context_lines,
            )?);
        }
    }
    if requests.is_empty() {
        return Err(WorkspaceError::invalid_argument(
            "items or matches must contain at least one read request",
        ));
    }

    let merge_overlaps = args
        .get("merge_overlaps")
        .and_then(Value::as_bool)
        .unwrap_or(true);
    if merge_overlaps {
        requests = merge_requests(requests);
    }
    let max_total_bytes = args
        .get("max_total_bytes")
        .and_then(Value::as_u64)
        .unwrap_or(262_144) as usize;
    let default_max_bytes = args
        .get("max_bytes_per_file")
        .and_then(Value::as_u64)
        .unwrap_or(131_072) as usize;
    let line_numbers = args
        .get("line_numbers")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let content_mode = args
        .get("content_mode")
        .and_then(Value::as_str)
        .unwrap_or(if line_numbers { "numbered" } else { "plain" });
    if !matches!(content_mode, "plain" | "numbered" | "both") {
        return Err(WorkspaceError::invalid_argument(
            "content_mode must be one of: plain, numbered, both",
        ));
    }
    let include_plain_content = content_mode != "numbered";
    let include_numbered_content = content_mode != "plain";

    let mut remaining = max_total_bytes;
    let mut results = Vec::with_capacity(requests.len());
    let mut failed = 0usize;
    let mut truncated = false;

    for request in &requests {
        if remaining == 0 {
            failed += 1;
            truncated = true;
            results.push(json!({
                "index": request.index,
                "source_indexes": request.source_indexes,
                "path": request.path,
                "ok": false,
                "error": {
                    "code": "BATCH_LIMIT_REACHED",
                    "message": "max_total_bytes reached before this item was read",
                    "category": "limit",
                    "retryable": true,
                    "details": { "max_total_bytes": max_total_bytes }
                }
            }));
            continue;
        }
        let item_limit = request
            .max_bytes
            .unwrap_or(default_max_bytes)
            .min(remaining);
        match read_file_value(
            ws,
            &request.path,
            request.start_line,
            request.end_line,
            item_limit,
        ) {
            Ok(mut value) => {
                let bytes_read =
                    value.get("bytes_read").and_then(Value::as_u64).unwrap_or(0) as usize;
                remaining = remaining.saturating_sub(bytes_read);
                truncated |= value
                    .get("truncated")
                    .and_then(Value::as_bool)
                    .unwrap_or(false);
                let numbered_content = if include_numbered_content {
                    let start =
                        value.get("start_line").and_then(Value::as_u64).unwrap_or(1) as usize;
                    let content = value.get("content").and_then(Value::as_str).unwrap_or("");
                    Some(number_content(content, start))
                } else {
                    None
                };
                if let Some(object) = value.as_object_mut() {
                    object.insert("index".into(), json!(request.index));
                    object.insert("source_indexes".into(), json!(request.source_indexes));
                    object.insert("content_mode".into(), json!(content_mode));
                    if !include_plain_content {
                        object.remove("content");
                        object.insert("content_omitted".into(), json!(true));
                    }
                    if let Some(numbered_content) = numbered_content {
                        object.insert("numbered_content".into(), Value::String(numbered_content));
                    }
                }
                results.push(value);
            }
            Err(error) => {
                failed += 1;
                results.push(json!({
                    "index": request.index,
                    "source_indexes": request.source_indexes,
                    "path": request.path,
                    "ok": false,
                    "error": error.to_error_value()
                }));
            }
        }
    }

    Ok(tool_ok(json!({
        "results": results,
        "requested_count": requests.iter().map(|r| r.source_indexes.len()).sum::<usize>(),
        "result_count": requests.len(),
        "merged_count": requests.iter().map(|r| r.source_indexes.len().saturating_sub(1)).sum::<usize>(),
        "failed_count": failed,
        "content_mode": content_mode,
        "bytes_read": max_total_bytes.saturating_sub(remaining),
        "max_total_bytes": max_total_bytes,
        "truncated": truncated,
        "warnings": if truncated { vec!["one or more reads were truncated"] } else { vec![] }
    })))
}

#[derive(Clone, Debug)]
struct ReadRequest {
    index: usize,
    source_indexes: Vec<usize>,
    path: String,
    start_line: usize,
    end_line: Option<usize>,
    max_bytes: Option<usize>,
}

fn read_request_from_value(
    item: &Value,
    index: usize,
    context_lines: usize,
) -> Result<ReadRequest, WorkspaceError> {
    let path = item
        .get("path")
        .and_then(Value::as_str)
        .ok_or_else(|| WorkspaceError::invalid_argument("item.path is required"))?
        .to_string();
    let match_line = item.get("line").and_then(Value::as_u64).map(|v| v as usize);
    let start_line = item
        .get("start_line")
        .and_then(Value::as_u64)
        .map(|v| v.max(1) as usize)
        .or_else(|| match_line.map(|line| line.saturating_sub(context_lines).max(1)))
        .unwrap_or(1);
    let end_line = item
        .get("end_line")
        .and_then(Value::as_u64)
        .map(|v| v as usize)
        .or_else(|| match_line.map(|line| line.saturating_add(context_lines)));
    if end_line.is_some_and(|end| end < start_line) {
        return Err(WorkspaceError::invalid_argument(
            "item.end_line must be >= item.start_line",
        ));
    }
    Ok(ReadRequest {
        index,
        source_indexes: vec![index],
        path,
        start_line,
        end_line,
        max_bytes: item
            .get("max_bytes")
            .and_then(Value::as_u64)
            .map(|v| v as usize),
    })
}

fn merge_requests(requests: Vec<ReadRequest>) -> Vec<ReadRequest> {
    let mut grouped: BTreeMap<String, Vec<ReadRequest>> = BTreeMap::new();
    for request in requests {
        grouped
            .entry(request.path.clone())
            .or_default()
            .push(request);
    }
    let mut merged: Vec<ReadRequest> = Vec::new();
    for (_, mut group) in grouped {
        group.sort_by_key(|item| item.start_line);
        for request in group {
            if let Some(last) = merged.last_mut() {
                let last_end = last.end_line.unwrap_or(usize::MAX);
                if last.path == request.path
                    && request.start_line <= last_end.saturating_add(1)
                    && last.max_bytes == request.max_bytes
                {
                    last.end_line = match (last.end_line, request.end_line) {
                        (None, _) | (_, None) => None,
                        (Some(a), Some(b)) => Some(a.max(b)),
                    };
                    last.source_indexes.extend(request.source_indexes);
                    continue;
                }
            }
            merged.push(request);
        }
    }
    merged.sort_by_key(|item| item.index);
    merged
}

fn number_content(content: &str, start_line: usize) -> String {
    content
        .split_inclusive('\n')
        .enumerate()
        .map(|(index, line)| format!("{:>6} | {}", start_line + index, line))
        .collect()
}

fn read_file_value(
    ws: &Workspace,
    path: &str,
    start_line: usize,
    end_line: Option<usize>,
    max_bytes: usize,
) -> Result<Value, WorkspaceError> {
    let resolved = ws.resolve_read_path(path)?;
    if resolved.path.is_dir() {
        return Err(WorkspaceError::Tool {
            code: "IS_DIRECTORY",
            message: "Path is a directory.".into(),
            category: "validation",
            retryable: false,
        });
    }
    let data = fs::read(&resolved.path).map_err(|_| WorkspaceError::not_found("File not found"))?;
    let sha256 = sha256_hex(&data);
    let (text, encoding, bom) = decode_text(&data)?;
    let lines: Vec<&str> = text.split_inclusive('\n').collect();
    let total_lines = lines.len();
    let end = end_line.unwrap_or(total_lines).min(total_lines);
    let selected: String = if end < start_line || start_line > total_lines {
        String::new()
    } else {
        lines[(start_line - 1)..end].concat()
    };
    let (content, truncated, truncated_by) = truncate_bytes(&selected, max_bytes);
    let actual_end = if truncated && !content.is_empty() {
        start_line + content.lines().count().saturating_sub(1)
    } else {
        end
    };
    let mut warnings = Vec::new();
    if truncated {
        warnings.push("content truncated".to_string());
    }
    Ok(tool_ok(json!({
        "path": resolved.display,
        "content": content,
        "encoding": encoding,
        "bom": bom,
        "sha256": sha256,
        "newline": newline_style(&text),
        "start_line": start_line,
        "end_line": actual_end,
        "requested_end_line": end_line,
        "total_lines": total_lines,
        "total_bytes": data.len(),
        "bytes_read": content.len(),
        "truncated": truncated,
        "truncated_by": truncated_by,
        "warnings": warnings
    })))
}

pub fn list_files(ws: &Workspace, args: &Value) -> Result<Value, WorkspaceError> {
    let path = args.get("path").and_then(Value::as_str).unwrap_or(".");
    let resolved = ws.resolve_read_path(path)?;
    if !resolved.path.is_dir() {
        return Err(WorkspaceError::not_a_directory("Path is not a directory"));
    }
    let patterns = list_files_patterns(args);
    let exclude_patterns = string_list_arg(args, "exclude_patterns");
    let mut entry_types = string_list_arg(args, "entry_types");
    if entry_types.is_empty() {
        entry_types = vec!["file".into(), "symlink".into()];
    }
    let recursive = args
        .get("recursive")
        .and_then(Value::as_bool)
        .unwrap_or(true);
    let max_depth = args
        .get("max_depth")
        .and_then(Value::as_u64)
        .unwrap_or(20)
        .clamp(1, 20) as usize;
    let max_results = args
        .get("max_results")
        .and_then(Value::as_u64)
        .unwrap_or(1000) as usize;
    let include_hidden = args
        .get("include_hidden")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let include_ignored = args
        .get("include_ignored")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let include_generated = args
        .get("include_generated")
        .and_then(Value::as_bool)
        .unwrap_or(false);

    let mut entries = Vec::new();
    let mut truncated = false;
    let walk_depth = if recursive { max_depth } else { 1 };
    for entry in WalkDir::new(&resolved.path)
        .follow_links(false)
        .min_depth(1)
        .max_depth(walk_depth)
        .into_iter()
        .filter_entry(|entry| {
            entry.path() == resolved.path
                || !ws.is_ignored_scan_path(
                    &resolved.path,
                    entry.path(),
                    include_hidden,
                    include_ignored,
                    include_generated,
                )
        })
        .filter_map(Result::ok)
    {
        let p = entry.path();
        if !ws.is_safe_read_path(p) {
            continue;
        }
        let entry_type = if entry.file_type().is_symlink() {
            "symlink"
        } else if entry.file_type().is_dir() {
            "directory"
        } else if entry.file_type().is_file() {
            "file"
        } else {
            continue;
        };
        if !entry_types.iter().any(|value| value == entry_type) {
            continue;
        }
        let rel = relative_display(ws.root(), p);
        if !patterns.iter().any(|pat| glob_match(pat, &rel)) {
            continue;
        }
        if exclude_patterns.iter().any(|pat| glob_match(pat, &rel)) {
            continue;
        }
        let meta = p.symlink_metadata().ok();
        entries.push(json!({
            "path": rel,
            "type": entry_type,
            "size_bytes": meta.as_ref().map(|m| m.len()).unwrap_or(0),
            "modified": meta.and_then(|m| format_mtime(m.modified().ok()))
        }));
        if entries.len() >= max_results {
            truncated = true;
            break;
        }
    }
    entries.sort_by(|a, b| a["path"].as_str().cmp(&b["path"].as_str()));
    let returned_count = entries.len();
    Ok(tool_ok(json!({
        "path": resolved.display,
        "entries": entries,
        "returned_count": returned_count,
        "entry_types": entry_types,
        "recursive": recursive,
        "max_depth": walk_depth,
        "truncated": truncated,
        "warnings": if truncated { vec!["result limit reached"] } else { vec![] }
    })))
}

#[derive(Debug, Clone, Copy, Default)]
pub struct SearchTextExecutionOptions {
    pub exact_total_fast_tail: bool,
}

pub fn search_text(ws: &Workspace, args: &Value) -> Result<Value, WorkspaceError> {
    search_text_with_options(ws, args, SearchTextExecutionOptions::default())
}

pub fn search_text_with_options(
    ws: &Workspace,
    args: &Value,
    options: SearchTextExecutionOptions,
) -> Result<Value, WorkspaceError> {
    let queries = parse_search_queries(args)?;
    let filename_query = args.get("filename_query").and_then(Value::as_str);
    if queries.is_empty() && filename_query.is_none() {
        return Err(WorkspaceError::invalid_argument(
            "query, queries, or filename_query is required",
        ));
    }
    let path = args.get("path").and_then(Value::as_str).unwrap_or(".");
    let resolved = ws.resolve_read_path(path)?;
    let requested_max_results = args
        .get("max_results")
        .and_then(Value::as_u64)
        .unwrap_or(200);
    let max_results = requested_max_results.clamp(1, 10_000) as usize;
    let cursor = args.get("cursor").and_then(Value::as_u64).unwrap_or(0) as usize;
    let requested_max_preview = args
        .get("max_preview_bytes")
        .and_then(Value::as_u64)
        .unwrap_or(512);
    let max_preview = requested_max_preview.clamp(64, 4_096) as usize;
    let max_file_bytes = args
        .get("max_file_bytes")
        .and_then(Value::as_u64)
        .unwrap_or(8 * 1024 * 1024) as usize;
    let max_matches_per_file = args
        .get("max_matches_per_file")
        .and_then(Value::as_u64)
        .unwrap_or(usize::MAX as u64) as usize;
    let include_hidden = args
        .get("include_hidden")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let include_ignored = args
        .get("include_ignored")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let include_generated = args
        .get("include_generated")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let files_only = args
        .get("files_only")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let count_only = args
        .get("count_only")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let calculate_total = count_only
        || args
            .get("calculate_total")
            .and_then(Value::as_bool)
            .unwrap_or(false);
    let (include_globs, exclude_globs) = search_globs(args);
    let requested_context_lines = args
        .get("context_lines")
        .and_then(Value::as_u64)
        .unwrap_or(0);
    let context_lines = requested_context_lines.min(20) as usize;
    let arguments_normalized = requested_max_results != max_results as u64
        || requested_max_preview != max_preview as u64
        || requested_context_lines != context_lines as u64;
    let filename_matcher = filename_query
        .map(|query| {
            build_matcher(
                query,
                args.get("filename_regex")
                    .and_then(Value::as_bool)
                    .unwrap_or(false),
                args.get("filename_case_sensitive")
                    .and_then(Value::as_bool)
                    .unwrap_or(false),
            )
        })
        .transpose()?;

    let file_paths = search_file_paths(
        &resolved.path,
        include_hidden,
        include_ignored,
        include_generated,
    );
    let mut matches = Vec::new();
    let mut files = Vec::new();
    let response_bytes_limit = 128 * 1024usize;
    let context_line_bytes_limit = max_preview.min(2_048);
    let mut response_bytes_estimate = 0usize;
    let mut response_limit_reached = false;
    let mut query_counts = vec![0usize; queries.len()];
    let mut files_considered = 0usize;
    let mut scanned_files = 0usize;
    let mut matched_files = 0usize;
    let mut skipped_large_files = 0usize;
    let mut skipped = 0usize;
    let mut truncated = false;
    let mut stopped_early = false;
    let mut exact_total_fast_tail_skipped_match_details = 0usize;

    'files: for p in file_paths {
        if !ws.is_safe_read_path(&p)
            || ws.is_ignored_scan_path(
                &resolved.path,
                &p,
                include_hidden,
                include_ignored,
                include_generated,
            )
        {
            continue;
        }
        let rel = relative_display(ws.root(), &p);
        if !passes_glob_filters(&rel, &include_globs, &exclude_globs) {
            continue;
        }
        let filename_matches = filename_matcher
            .as_ref()
            .is_some_and(|matcher| matcher.is_match(&rel));
        if filename_matcher.is_some() && !filename_matches {
            continue;
        }
        files_considered += 1;
        if files_only && queries.is_empty() {
            if skipped < cursor {
                skipped += 1;
                continue;
            }
            if files.len() >= max_results {
                truncated = true;
                if calculate_total {
                    continue;
                }
                stopped_early = true;
                break;
            }
            files.push(json!({
                "path": rel,
                "match_id": stable_match_id(&rel, 0, 0, "filename"),
                "matched_by": "filename"
            }));
            continue;
        }

        let size = p
            .metadata()
            .map(|metadata| metadata.len() as usize)
            .unwrap_or(0);
        if size > max_file_bytes {
            skipped_large_files += 1;
            continue;
        }
        let bytes = match fs::read(&p) {
            Ok(bytes) if !bytes.contains(&0) => bytes,
            _ => continue,
        };
        let content = match std::str::from_utf8(&bytes) {
            Ok(content) => content,
            Err(_) => continue,
        };
        scanned_files += 1;
        let lines: Vec<&str> = content.lines().collect();
        let mut file_match_count = 0usize;
        let mut file_recorded = false;
        for (query_index, query) in queries.iter().enumerate() {
            for (idx, line) in lines.iter().enumerate() {
                for found in query.matcher.find_iter(line) {
                    query_counts[query_index] += 1;
                    file_match_count += 1;
                    if !file_recorded {
                        matched_files += 1;
                        file_recorded = true;
                    }
                    if file_match_count > max_matches_per_file {
                        continue;
                    }
                    if skipped < cursor {
                        skipped += 1;
                        continue;
                    }
                    if files_only {
                        if !files.iter().any(|item| item["path"] == rel) {
                            if files.len() >= max_results {
                                truncated = true;
                                if calculate_total {
                                    continue;
                                }
                                stopped_early = true;
                                break 'files;
                            }
                            files.push(json!({
                                "path": rel,
                                "match_id": stable_match_id(&rel, idx + 1, query_index, &query.query),
                                "matched_by": "content",
                                "query_index": query_index,
                                "query": query.query
                            }));
                        }
                        continue;
                    }
                    if count_only {
                        continue;
                    }
                    if response_limit_reached {
                        if options.exact_total_fast_tail && calculate_total {
                            exact_total_fast_tail_skipped_match_details += 1;
                        }
                        continue;
                    }
                    if matches.len() >= max_results {
                        truncated = true;
                        if calculate_total {
                            if options.exact_total_fast_tail {
                                exact_total_fast_tail_skipped_match_details += 1;
                            }
                            continue;
                        }
                        stopped_early = true;
                        break 'files;
                    }
                    let column = line[..found.start()].chars().count() + 1;
                    let end_column = line[..found.end()].chars().count() + 1;
                    let (bounded_match, match_truncated) =
                        truncate_utf8_prefix(&line[found.start()..found.end()], max_preview);
                    let (bounded_query, query_truncated) =
                        truncate_utf8_prefix(&query.query, 1_024);
                    let mut item = json!({
                        "match_id": stable_match_id(&rel, idx + 1, query_index, &query.query),
                        "path": rel,
                        "line": idx + 1,
                        "column": column,
                        "end_column": end_column,
                        "query_index": query_index,
                        "query": bounded_query,
                        "match": bounded_match,
                        "preview": preview_around_match(line, found.start(), found.end(), max_preview)
                    });
                    if query_truncated {
                        item["query_truncated"] = json!(true);
                    }
                    if match_truncated {
                        item["match_truncated"] = json!(true);
                    }
                    if context_lines > 0 {
                        let start = idx.saturating_sub(context_lines);
                        let end = (idx + 1 + context_lines).min(lines.len());
                        let mut context_truncated = false;
                        let before = lines[start..idx]
                            .iter()
                            .map(|line| {
                                let (value, truncated) =
                                    truncate_utf8_prefix(line, context_line_bytes_limit);
                                context_truncated |= truncated;
                                value
                            })
                            .collect::<Vec<_>>();
                        let after = lines[idx + 1..end]
                            .iter()
                            .map(|line| {
                                let (value, truncated) =
                                    truncate_utf8_prefix(line, context_line_bytes_limit);
                                context_truncated |= truncated;
                                value
                            })
                            .collect::<Vec<_>>();
                        item["before"] = json!(before);
                        item["after"] = json!(after);
                        if context_truncated {
                            item["context_truncated"] = json!(true);
                        }
                    }
                    let mut item_bytes = serde_json::to_vec(&item)
                        .map(|value| value.len())
                        .unwrap_or(0);
                    if matches.is_empty()
                        && item_bytes > response_bytes_limit
                        && (item.get("before").is_some() || item.get("after").is_some())
                    {
                        if let Some(object) = item.as_object_mut() {
                            object.remove("before");
                            object.remove("after");
                            object.insert(
                                "context_omitted_due_to_response_limit".to_string(),
                                json!(true),
                            );
                        }
                        item_bytes = serde_json::to_vec(&item)
                            .map(|value| value.len())
                            .unwrap_or(0);
                    }
                    if response_bytes_estimate.saturating_add(item_bytes) > response_bytes_limit {
                        truncated = true;
                        response_limit_reached = true;
                        if calculate_total {
                            if options.exact_total_fast_tail {
                                exact_total_fast_tail_skipped_match_details += 1;
                            }
                            continue;
                        }
                        stopped_early = true;
                        break 'files;
                    }
                    response_bytes_estimate = response_bytes_estimate.saturating_add(item_bytes);
                    matches.push(item);
                }
            }
        }
    }

    let returned = if files_only {
        files.len()
    } else {
        matches.len()
    };
    let next_cursor = if truncated {
        Some(cursor + returned)
    } else {
        None
    };
    let exact_total_forced_full_scan =
        calculate_total && !count_only && truncated && !stopped_early;
    let truncation_reason = if response_limit_reached {
        Some("response_limit")
    } else if truncated {
        Some("result_limit")
    } else {
        None
    };
    let early_stop_reason = if stopped_early {
        truncation_reason
    } else {
        None
    };
    let search_recommendation = if exact_total_forced_full_scan {
        Some(if response_limit_reached {
            "calculate_total=true forced a full scan after the response payload limit was reached. Omit calculate_total for discovery, narrow path/include_globs/filename_query, or use count_only=true when only the exact count is needed."
        } else {
            "calculate_total=true forced a full scan after the result limit was reached. Omit calculate_total for discovery, narrow path/include_globs/filename_query, or use count_only=true when only the exact count is needed."
        })
    } else if response_limit_reached {
        Some("Search stopped at the response payload limit. Continue with next_cursor, reduce context_lines/max_preview_bytes, or narrow path/include_globs.")
    } else if stopped_early && path == "." && include_globs.is_empty() && filename_query.is_none() {
        Some("Search stopped at max_results. Narrow path/include_globs/filename_query, or set calculate_total=true only when an exact total is required.")
    } else {
        None
    };
    let mut warnings = Vec::new();
    if truncated {
        warnings.push(if response_limit_reached {
            "response payload limit reached"
        } else {
            "result limit reached"
        });
    }
    if exact_total_forced_full_scan {
        warnings.push(if response_limit_reached {
            "calculate_total forced a full scan after the response payload limit was reached"
        } else {
            "calculate_total forced a full scan after the result limit was reached"
        });
    }
    Ok(tool_ok(json!({
        "query": args.get("query"),
        "queries": queries.iter().enumerate().map(|(index, query)| json!({
            "index": index,
            "query": query.query,
            "regex": query.regex,
            "case_sensitive": query.case_sensitive,
            "matches": query_counts[index]
        })).collect::<Vec<_>>(),
        "filename_query": filename_query,
        "matches": if files_only || count_only { Vec::<Value>::new() } else { matches },
        "files": files,
        "total_matches": query_counts.iter().sum::<usize>(),
        "total_matches_exact": !stopped_early,
        "calculate_total": calculate_total,
        "returned_count": returned,
        "matched_files": matched_files,
        "files_considered": files_considered,
        "scanned_files": scanned_files,
        "skipped_large_files": skipped_large_files,
        "cursor": cursor,
        "next_cursor": next_cursor,
        "scan_completed": !stopped_early,
        "early_stop_reason": early_stop_reason,
        "search_recommendation": search_recommendation,
        "exact_total_forced_full_scan": exact_total_forced_full_scan,
        "exact_total_fast_tail": options.exact_total_fast_tail && calculate_total && !count_only,
        "exact_total_fast_tail_skipped_match_details": exact_total_fast_tail_skipped_match_details,
        "response_bytes_limit": response_bytes_limit,
        "response_bytes_estimate": response_bytes_estimate,
        "context_line_bytes_limit": context_line_bytes_limit,
        "response_limit_reached": response_limit_reached,
        "arguments_normalized": arguments_normalized,
        "normalized_arguments": if arguments_normalized {
            json!({
                "max_results": max_results,
                "max_preview_bytes": max_preview,
                "context_lines": context_lines
            })
        } else {
            Value::Null
        },
        "truncated": truncated,
        "warnings": warnings
    })))
}

struct SearchNeedle {
    query: String,
    regex: bool,
    case_sensitive: bool,
    matcher: Regex,
}

fn parse_search_queries(args: &Value) -> Result<Vec<SearchNeedle>, WorkspaceError> {
    let default_regex = args.get("regex").and_then(Value::as_bool).unwrap_or(false);
    let default_case = args
        .get("case_sensitive")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let mut values = Vec::new();
    if let Some(query) = args.get("query").and_then(Value::as_str) {
        if !query.is_empty() {
            values.push((query.to_string(), default_regex, default_case));
        }
    }
    if let Some(queries) = args.get("queries").and_then(Value::as_array) {
        for item in queries {
            match item {
                Value::String(query) if !query.is_empty() => {
                    values.push((query.clone(), default_regex, default_case));
                }
                Value::Object(object) => {
                    let query = object.get("query").and_then(Value::as_str).ok_or_else(|| {
                        WorkspaceError::invalid_argument("queries[].query is required")
                    })?;
                    values.push((
                        query.to_string(),
                        object
                            .get("regex")
                            .and_then(Value::as_bool)
                            .unwrap_or(default_regex),
                        object
                            .get("case_sensitive")
                            .and_then(Value::as_bool)
                            .unwrap_or(default_case),
                    ));
                }
                _ => {
                    return Err(WorkspaceError::invalid_argument(
                        "queries entries must be strings or query objects",
                    ));
                }
            }
        }
    }
    values
        .into_iter()
        .map(|(query, regex, case_sensitive)| {
            let matcher = build_matcher(&query, regex, case_sensitive)?;
            Ok(SearchNeedle {
                query,
                regex,
                case_sensitive,
                matcher,
            })
        })
        .collect()
}

fn stable_match_id(path: &str, line: usize, query_index: usize, query: &str) -> String {
    let raw = format!("{path}\u{1f}{line}\u{1f}{query_index}\u{1f}{query}");
    format!("match-{}", &sha256_hex(raw.as_bytes())[..16])
}

fn build_matcher(
    query: &str,
    use_regex: bool,
    case_sensitive: bool,
) -> Result<Regex, WorkspaceError> {
    let pattern = if use_regex {
        query.to_string()
    } else {
        regex::escape(query)
    };
    let pattern = if case_sensitive {
        pattern
    } else {
        format!("(?i:{pattern})")
    };
    Regex::new(&pattern)
        .map_err(|error| WorkspaceError::invalid_argument(format!("Invalid regex: {error}")))
}

fn search_file_paths(
    path: &Path,
    include_hidden: bool,
    include_ignored: bool,
    include_generated: bool,
) -> Vec<PathBuf> {
    if path.is_file() {
        return vec![path.to_path_buf()];
    }

    let mut builder = WalkBuilder::new(path);
    builder
        .follow_links(false)
        .hidden(!include_hidden)
        .require_git(false);
    builder.filter_entry(move |entry| {
        entry.file_name().to_str().map_or(true, |name| {
            !name.eq_ignore_ascii_case(".git")
                && (include_generated
                    || !crate::tools::workspace::DEFAULT_EXCLUDED_NAMES.contains(&name))
        })
    });
    if include_ignored {
        builder
            .ignore(false)
            .git_ignore(false)
            .git_global(false)
            .git_exclude(false)
            .parents(false);
    }
    builder
        .build()
        .filter_map(Result::ok)
        .filter(|entry| entry.file_type().is_some_and(|kind| kind.is_file()))
        .map(|entry| entry.into_path())
        .collect()
}

fn truncate_utf8_prefix(value: &str, max_bytes: usize) -> (String, bool) {
    if value.len() <= max_bytes {
        return (value.to_string(), false);
    }
    let mut end = max_bytes.min(value.len());
    while end > 0 && !value.is_char_boundary(end) {
        end -= 1;
    }
    (value[..end].to_string(), true)
}

fn preview_around_match(line: &str, start: usize, end: usize, max_bytes: usize) -> String {
    if line.len() <= max_bytes {
        return line.to_string();
    }
    let match_len = end.saturating_sub(start);
    let left_budget = max_bytes.saturating_sub(match_len) / 2;
    let mut window_start = start.saturating_sub(left_budget);
    while window_start > 0 && !line.is_char_boundary(window_start) {
        window_start -= 1;
    }
    let mut window_end = (window_start + max_bytes).min(line.len());
    while window_end > window_start && !line.is_char_boundary(window_end) {
        window_end -= 1;
    }
    if window_end < end {
        window_end = end;
        window_start = window_end.saturating_sub(max_bytes);
        while window_start > 0 && !line.is_char_boundary(window_start) {
            window_start -= 1;
        }
    }
    format!(
        "{}{}{}",
        if window_start > 0 { "..." } else { "" },
        &line[window_start..window_end],
        if window_end < line.len() { "..." } else { "" }
    )
}

fn decode_text(data: &[u8]) -> Result<(String, &'static str, bool), WorkspaceError> {
    if data.starts_with(&[0xEF, 0xBB, 0xBF]) {
        let text = String::from_utf8(data[3..].to_vec()).map_err(|_| unsupported_encoding())?;
        return Ok((text, "utf-8", true));
    }
    if data.starts_with(&[0xFF, 0xFE]) {
        if (data.len() - 2) % 2 != 0 {
            return Err(unsupported_encoding());
        }
        let units = data[2..]
            .chunks_exact(2)
            .map(|chunk| u16::from_le_bytes([chunk[0], chunk[1]]))
            .collect::<Vec<_>>();
        return String::from_utf16(&units)
            .map(|text| (text, "utf-16le", true))
            .map_err(|_| unsupported_encoding());
    }
    if data.starts_with(&[0xFE, 0xFF]) {
        if (data.len() - 2) % 2 != 0 {
            return Err(unsupported_encoding());
        }
        let units = data[2..]
            .chunks_exact(2)
            .map(|chunk| u16::from_be_bytes([chunk[0], chunk[1]]))
            .collect::<Vec<_>>();
        return String::from_utf16(&units)
            .map(|text| (text, "utf-16be", true))
            .map_err(|_| unsupported_encoding());
    }
    if data.iter().take(4096).any(|byte| *byte == 0) {
        return Err(WorkspaceError::Tool {
            code: "BINARY_FILE",
            message: "Binary file read blocked for text tool.".into(),
            category: "validation",
            retryable: false,
        });
    }
    String::from_utf8(data.to_vec())
        .map(|text| (text, "utf-8", false))
        .map_err(|_| unsupported_encoding())
}

fn unsupported_encoding() -> WorkspaceError {
    WorkspaceError::Tool {
        code: "UNSUPPORTED_ENCODING",
        message: "File encoding is not supported; expected UTF-8 or BOM-marked UTF-16.".into(),
        category: "validation",
        retryable: false,
    }
}

fn truncate_bytes(text: &str, max_bytes: usize) -> (String, bool, Option<&'static str>) {
    let bytes = text.as_bytes();
    if bytes.len() <= max_bytes {
        return (text.to_string(), false, None);
    }
    let mut end = max_bytes;
    while end > 0 && !text.is_char_boundary(end) {
        end -= 1;
    }
    (text[..end].to_string(), true, Some("bytes"))
}

fn sha256_hex(bytes: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    format!("{:x}", hasher.finalize())
}

fn newline_style(text: &str) -> &'static str {
    if text.contains("\r\n") {
        "crlf"
    } else if text.contains('\n') {
        "lf"
    } else {
        "none"
    }
}

fn string_list_arg(args: &Value, key: &str) -> Vec<String> {
    args.get(key)
        .and_then(Value::as_array)
        .map(|arr| {
            arr.iter()
                .filter_map(|v| v.as_str().map(str::to_string))
                .collect()
        })
        .unwrap_or_default()
}

fn list_files_patterns(args: &Value) -> Vec<String> {
    let patterns = string_list_arg(args, "patterns");
    if !patterns.is_empty() {
        return patterns;
    }
    if let Some(glob) = args.get("glob").and_then(Value::as_str) {
        if !glob.is_empty() {
            return vec![glob.to_string()];
        }
    }
    vec!["**/*".to_string()]
}

fn search_globs(args: &Value) -> (Vec<String>, Vec<String>) {
    let mut include = string_list_arg(args, "include_globs");
    if let Some(glob) = args.get("glob").and_then(Value::as_str) {
        if !glob.is_empty() {
            include.push(glob.to_string());
        }
    }
    (include, string_list_arg(args, "exclude_globs"))
}

fn passes_glob_filters(rel: &str, include: &[String], exclude: &[String]) -> bool {
    if !include.is_empty() && !include.iter().any(|pat| glob_match(pat, rel)) {
        return false;
    }
    !exclude.iter().any(|pat| glob_match(pat, rel))
}

fn glob_match(pattern: &str, path: &str) -> bool {
    let pat = pattern.replace('\\', "/");
    let p = path.replace('\\', "/");
    if pat == "**/*" || pat == "*" {
        return true;
    }
    if let Some(suffix) = pat.strip_prefix("**/") {
        return simple_glob(suffix, &p) || p.split('/').any(|part| simple_glob(suffix, part));
    }
    simple_glob(&pat, &p)
}

fn simple_glob(pattern: &str, text: &str) -> bool {
    glob::Pattern::new(pattern)
        .map(|p| p.matches(text))
        .unwrap_or(false)
}

fn format_mtime(st: Option<SystemTime>) -> Option<String> {
    st.map(|t| {
        let d = t.duration_since(std::time::UNIX_EPOCH).unwrap_or_default();
        format!("{}.{:03}Z", d.as_secs(), d.subsec_millis())
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use tempfile::tempdir;

    #[test]
    fn search_uses_gitignore_and_reports_unicode_column() {
        let workspace = tempdir().expect("workspace");
        fs::write(workspace.path().join(".gitignore"), "ignored/\n").expect("gitignore");
        fs::write(workspace.path().join("visible.rs"), "前綴 needle 後綴\n").expect("visible");
        fs::create_dir_all(workspace.path().join("ignored")).expect("ignored dir");
        fs::write(workspace.path().join("ignored/hidden.rs"), "needle\n").expect("ignored");
        let ws = Workspace::new(workspace.path().to_path_buf()).expect("workspace");

        let result = search_text(
            &ws,
            &json!({
                "query": "needle",
                "path": ".",
                "max_preview_bytes": 8,
                "max_results": 20
            }),
        )
        .expect("search");
        assert_eq!(result["matches"].as_array().unwrap().len(), 1);
        assert_eq!(result["matches"][0]["path"], "visible.rs");
        assert_eq!(result["matches"][0]["column"], 4);
        assert!(result["matches"][0]["preview"]
            .as_str()
            .unwrap()
            .contains("needle"));
    }

    #[test]
    fn search_normalizes_relaxed_public_schema_values() {
        let workspace = tempdir().expect("workspace");
        fs::write(workspace.path().join("visible.rs"), "needle\n").expect("visible");
        let ws = Workspace::new(workspace.path().to_path_buf()).expect("workspace");

        let result = search_text(
            &ws,
            &json!({
                "query": "needle",
                "context_lines": 25,
                "max_preview_bytes": 32,
                "max_results": 20
            }),
        )
        .expect("search");

        assert_eq!(result["arguments_normalized"], true);
        assert_eq!(result["normalized_arguments"]["context_lines"], 20);
        assert_eq!(result["normalized_arguments"]["max_preview_bytes"], 64);
    }

    #[test]
    fn search_can_stop_early_or_continue_for_an_exact_total() {
        let workspace = tempdir().expect("workspace");
        for name in ["a.txt", "b.txt", "c.txt"] {
            fs::write(workspace.path().join(name), "needle\n").expect("search fixture");
        }
        let ws = Workspace::new(workspace.path().to_path_buf()).expect("workspace");

        let bounded = search_text(&ws, &json!({"query": "needle", "max_results": 1}))
            .expect("bounded search");
        assert_eq!(bounded["returned_count"], 1);
        assert_eq!(bounded["truncated"], true);
        assert_eq!(bounded["calculate_total"], false);
        assert_eq!(bounded["scan_completed"], false);
        assert_eq!(bounded["total_matches_exact"], false);
        assert_eq!(bounded["early_stop_reason"], "result_limit");
        assert!(bounded["search_recommendation"]
            .as_str()
            .unwrap_or_default()
            .contains("Narrow path"));
        assert!(bounded["files_considered"].as_u64().unwrap_or_default() < 3);

        let exact = search_text(
            &ws,
            &json!({"query": "needle", "max_results": 1, "calculate_total": true}),
        )
        .expect("exact search");
        assert_eq!(exact["returned_count"], 1);
        assert_eq!(exact["truncated"], true);
        assert_eq!(exact["calculate_total"], true);
        assert_eq!(exact["scan_completed"], true);
        assert_eq!(exact["total_matches_exact"], true);
        assert_eq!(exact["total_matches"], 3);
        assert_eq!(exact["files_considered"], 3);
        assert!(exact["early_stop_reason"].is_null());
    }

    #[test]
    fn search_exact_total_fast_tail_preserves_results_and_counts_skipped_details() {
        let workspace = tempdir().expect("workspace");
        let content = (0..40)
            .map(|index| format!("needle {index:02}"))
            .collect::<Vec<_>>()
            .join("\n");
        fs::write(workspace.path().join("tail.txt"), format!("{content}\n")).expect("fixture");
        let ws = Workspace::new(workspace.path().to_path_buf()).expect("workspace");
        let args = json!({
            "query": "needle",
            "path": "tail.txt",
            "max_results": 3,
            "calculate_total": true
        });

        let baseline = search_text(&ws, &args).expect("baseline");
        let fast_tail = search_text_with_options(
            &ws,
            &args,
            SearchTextExecutionOptions {
                exact_total_fast_tail: true,
            },
        )
        .expect("fast tail");

        assert_eq!(fast_tail["total_matches"], baseline["total_matches"]);
        assert_eq!(fast_tail["total_matches_exact"], true);
        assert_eq!(fast_tail["matches"], baseline["matches"]);
        assert_eq!(fast_tail["returned_count"], baseline["returned_count"]);
        assert_eq!(fast_tail["exact_total_fast_tail"], true);
        assert!(
            fast_tail["exact_total_fast_tail_skipped_match_details"]
                .as_u64()
                .unwrap_or_default()
                > 0
        );
    }

    #[test]
    fn search_bounds_match_payload_and_preserves_exact_totals() {
        let workspace = tempdir().expect("workspace");
        let content = (0..300)
            .map(|index| format!("needle {index:03} {}", "x".repeat(2_000)))
            .collect::<Vec<_>>()
            .join("\n");
        fs::write(workspace.path().join("large.txt"), format!("{content}\n")).expect("fixture");
        let ws = Workspace::new(workspace.path().to_path_buf()).expect("workspace");

        let bounded = search_text(
            &ws,
            &json!({
                "query": "needle",
                "path": "large.txt",
                "max_results": 1000,
                "max_preview_bytes": 4096
            }),
        )
        .expect("bounded");
        assert_eq!(bounded["response_limit_reached"], true);
        assert_eq!(bounded["truncated"], true);
        assert_eq!(bounded["early_stop_reason"], "response_limit");
        assert_eq!(bounded["scan_completed"], false);
        assert_eq!(bounded["total_matches_exact"], false);
        let returned = bounded["returned_count"].as_u64().unwrap();
        assert!(returned > 0 && returned < 300);
        assert!(
            bounded["response_bytes_estimate"].as_u64().unwrap()
                <= bounded["response_bytes_limit"].as_u64().unwrap()
        );
        assert_eq!(bounded["next_cursor"].as_u64().unwrap(), returned);

        let next = search_text(
            &ws,
            &json!({
                "query": "needle",
                "path": "large.txt",
                "max_results": 1000,
                "max_preview_bytes": 4096,
                "cursor": returned
            }),
        )
        .expect("next page");
        assert_eq!(next["matches"][0]["line"].as_u64().unwrap(), returned + 1);

        let exact = search_text(
            &ws,
            &json!({
                "query": "needle",
                "path": "large.txt",
                "max_results": 1000,
                "max_preview_bytes": 4096,
                "calculate_total": true
            }),
        )
        .expect("exact");
        assert_eq!(exact["response_limit_reached"], true);
        assert_eq!(exact["truncated"], true);
        assert_eq!(exact["scan_completed"], true);
        assert_eq!(exact["total_matches_exact"], true);
        assert_eq!(exact["total_matches"], 300);
        assert!(exact["early_stop_reason"].is_null());
        assert_eq!(exact["exact_total_forced_full_scan"], true);
        assert!(exact["returned_count"].as_u64().unwrap() < 300);
    }

    #[test]
    fn list_files_can_return_directories_without_a_separate_tool() {
        let workspace = tempdir().expect("workspace");
        fs::create_dir_all(workspace.path().join("src/nested")).expect("directories");
        fs::write(workspace.path().join("src/lib.rs"), "pub fn value() {}\n").expect("file");
        let ws = Workspace::new(workspace.path().to_path_buf()).expect("workspace");

        let result = list_files(
            &ws,
            &json!({
                "path": ".",
                "entry_types": ["directory"],
                "recursive": false,
                "max_depth": 1
            }),
        )
        .expect("list entries");

        assert_eq!(result["returned_count"], 1);
        assert_eq!(result["entries"][0]["path"], "src");
        assert_eq!(result["entries"][0]["type"], "directory");
    }

    #[test]
    fn generated_trees_require_explicit_opt_in_for_list_and_search() {
        let workspace = tempdir().expect("workspace");
        fs::write(workspace.path().join("visible.txt"), "needle\n").expect("visible");
        fs::create_dir_all(workspace.path().join("node_modules/pkg")).expect("generated dir");
        fs::write(
            workspace.path().join("node_modules/pkg/index.txt"),
            "needle\n",
        )
        .expect("generated");
        fs::create_dir_all(workspace.path().join(".git/objects")).expect("git dir");
        fs::write(workspace.path().join(".git/objects/secret.txt"), "needle\n").expect("git file");
        let ws = Workspace::new(workspace.path().to_path_buf()).expect("workspace");

        let listed = list_files(&ws, &json!({"include_ignored": true})).expect("listed");
        let paths = listed["entries"]
            .as_array()
            .expect("entries")
            .iter()
            .filter_map(|entry| entry["path"].as_str())
            .collect::<Vec<_>>();
        assert!(paths.contains(&"visible.txt"));
        assert!(!paths.iter().any(|path| path.starts_with("node_modules/")));
        assert!(!paths.iter().any(|path| path.starts_with(".git/")));

        let generated = list_files(
            &ws,
            &json!({"include_ignored": true, "include_generated": true}),
        )
        .expect("generated list");
        let generated_paths = generated["entries"]
            .as_array()
            .expect("entries")
            .iter()
            .filter_map(|entry| entry["path"].as_str())
            .collect::<Vec<_>>();
        assert!(generated_paths.contains(&"node_modules/pkg/index.txt"));
        assert!(!generated_paths.iter().any(|path| path.starts_with(".git/")));

        let searched =
            search_text(&ws, &json!({"query": "needle", "include_ignored": true})).expect("search");
        assert_eq!(searched["matches"].as_array().unwrap().len(), 1);
        assert_eq!(searched["matches"][0]["path"], "visible.txt");

        let searched_generated = search_text(
            &ws,
            &json!({"query": "needle", "include_ignored": true, "include_generated": true}),
        )
        .expect("generated search");
        let matched_paths = searched_generated["matches"]
            .as_array()
            .expect("matches")
            .iter()
            .filter_map(|entry| entry["path"].as_str())
            .collect::<Vec<_>>();
        assert!(matched_paths.contains(&"visible.txt"));
        assert!(matched_paths.contains(&"node_modules/pkg/index.txt"));
        assert!(!matched_paths.iter().any(|path| path.starts_with(".git/")));
    }

    #[test]
    fn read_many_avoids_duplicate_numbered_content_by_default() {
        let workspace = tempdir().expect("workspace");
        fs::write(workspace.path().join("a.txt"), "one\ntwo\n").expect("a");
        let ws = Workspace::new(workspace.path().to_path_buf()).expect("workspace");

        let numbered = read_many(
            &ws,
            &json!({
                "items": [{ "path": "a.txt", "start_line": 1, "end_line": 2 }],
                "line_numbers": true
            }),
        )
        .expect("numbered");
        assert_eq!(numbered["content_mode"], "numbered");
        let numbered_result = numbered["results"][0].as_object().expect("numbered result");
        assert!(numbered_result.get("content").is_none());
        assert_eq!(numbered_result.get("content_omitted"), Some(&json!(true)));
        assert!(numbered_result["numbered_content"]
            .as_str()
            .unwrap_or_default()
            .contains("1 | one"));

        let both = read_many(
            &ws,
            &json!({
                "items": [{ "path": "a.txt", "start_line": 1, "end_line": 2 }],
                "content_mode": "both"
            }),
        )
        .expect("both");
        assert_eq!(both["content_mode"], "both");
        assert_eq!(both["results"][0]["content"], "one\ntwo\n");
        assert!(both["results"][0]["numbered_content"]
            .as_str()
            .unwrap_or_default()
            .contains("1 | one"));
    }

    #[test]
    fn read_many_returns_hashes_and_independent_ranges() {
        let workspace = tempdir().expect("workspace");
        fs::write(workspace.path().join("a.txt"), "one\ntwo\nthree\n").expect("a");
        fs::write(workspace.path().join("b.txt"), "alpha\nbeta\n").expect("b");
        let ws = Workspace::new(workspace.path().to_path_buf()).expect("workspace");

        let result = read_many(
            &ws,
            &json!({
                "items": [
                    { "path": "a.txt", "start_line": 2, "end_line": 3 },
                    { "path": "b.txt", "start_line": 1, "end_line": 1 }
                ]
            }),
        )
        .expect("read many");
        assert_eq!(result["failed_count"], 0);
        assert_eq!(result["results"][0]["content"], "two\nthree\n");
        assert_eq!(result["results"][1]["content"], "alpha\n");
        assert_eq!(result["results"][0]["sha256"].as_str().unwrap().len(), 64);
    }
}
