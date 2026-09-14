use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::fs;
use std::io::BufRead;
use std::path::{Path, PathBuf};

use serde_json::{json, Value};

use crate::tools::context::ToolContext;
use crate::tools::parallel_stats::{parallelism_report, ParallelHistory};
use crate::tools::workspace::{tool_ok, WorkspaceError};

const DIAGNOSTICS_LOG_FILE: &str = "diagnostics.jsonl";
const TOOL_USAGE_LOG_FILE: &str = "mcp-tool-usage.jsonl";
const DIAGNOSTICS_SCHEMA_VERSION: u64 = 1;
const DIAGNOSTICS_HOST_KIND: &str = "rust_desktop";
const STRUCTURED_LOG_MAX_BYTES: u64 = 20 * 1024 * 1024;
const STRUCTURED_LOG_QUEUE_CAPACITY: usize = 1_024;
const MAX_ROTATED_FILES: usize = 5;
const RECORD_CURSOR_MAX: usize = 10_000;
const DEFAULT_BURST_IDLE_MS: u64 = 120_000;
const PHASE_METRICS: [(&str, &str); 10] = [
    ("preflight", "phase_preflight_ms"),
    ("plan", "phase_plan_ms"),
    ("commit", "phase_commit_ms"),
    ("total", "phase_total_ms"),
    ("baseline_capture", "phase_baseline_capture_ms"),
    ("error_enrichment", "phase_error_enrichment_ms"),
    ("harness_begin", "phase_harness_begin_ms"),
    ("dispatch", "phase_dispatch_ms"),
    ("harness_finish", "phase_harness_finish_ms"),
    ("serialization", "phase_serialization_ms"),
];

#[derive(Default)]
struct PhaseStats {
    total_ms: u128,
    durations: Vec<u64>,
}

#[derive(Default)]
struct ToolStats {
    calls: u64,
    errors: u64,
    tool_errors: u64,
    transport_errors: u64,
    warnings: u64,
    duration_ms: u128,
    queue_wait_ms: u128,
    workspace_admission_wait_ms: u128,
    global_admission_wait_ms: u128,
    blocking_queue_wait_ms: u128,
    workspace_lock_wait_ms: u128,
    history_lock_wait_ms: u128,
    session_registry_wait_ms: u128,
    actual_wait_ms: u128,
    snapshot_ms: u128,
    resource_lock_wait_ms: u128,
    operation_lock_wait_ms: u128,
    batch_queue_wait_ms: u128,
    queue_nonzero: u64,
    request_bytes: u64,
    response_bytes: u64,
    recovery_actions: u64,
    failed_command_ids: u64,
    skipped_command_ids: u64,
    empty_wait_timeouts: u64,
    deduplicated_calls: u64,
    heartbeat_responses: u64,
    detached_responses: u64,
    format_files_requested: u64,
    format_files_supported: u64,
    format_files_changed: u64,
    format_files_unchanged: u64,
    format_files_skipped: u64,
    formatter_groups: u64,
    custom_formatter_groups: u64,
    unavailable_adapters: u64,
    unexpected_changes: u64,
    format_diff_bytes: u64,
    format_apply_calls: u64,
    search_files_considered: u64,
    search_files_scanned: u64,
    search_returned: u64,
    search_matched_files: u64,
    search_zero_result_calls: u64,
    search_early_stops: u64,
    search_exact_total_calls: u64,
    phase_latency: BTreeMap<String, PhaseStats>,
    durations: Vec<u64>,
}

#[derive(Default)]
struct CommandKindStats {
    calls: u64,
    server_duration_ms: u128,
    child_sessions: u64,
    child_process_ms: u128,
}

#[derive(Default)]
struct BurstStats {
    calls: u64,
    first_started_ts_ms: u64,
    last_completed_ts_ms: u64,
    server_duration_ms: u128,
    orchestration_gap_ms: u128,
    tools: BTreeMap<String, u64>,
    sequential_exec_commands: u64,
    exec_many_calls: u64,
}

#[derive(Default)]
struct PerformanceStats {
    tool_calls: u64,
    async_sessions: u64,
    first_started_ts_ms: u64,
    last_completed_ts_ms: u64,
    server_duration_ms: u128,
    queue_wait_ms: u128,
    active_orchestration_gap_ms: u128,
    idle_gap_ms: u128,
    idle_gap_count: u64,
    child_process_ms: u128,
    orchestration_gaps: Vec<u64>,
    child_durations: Vec<u64>,
    first_output_durations: Vec<u64>,
    child_failures: u64,
    child_terminations: BTreeMap<String, u64>,
    command_kinds: BTreeMap<String, CommandKindStats>,
    bursts: BTreeMap<String, BurstStats>,
    inferred_burst_id: u64,
    previous_completed_ts_ms: u64,
}

#[derive(Default)]
struct RepeatedFailureGroup {
    tool: String,
    error_code: String,
    retry_count: u64,
    chain_count: u64,
    wasted_duration_ms: u128,
    max_attempt_count: u64,
}

#[derive(Default)]
struct RepeatedFailureStats {
    retry_count: u64,
    chain_count: u64,
    wasted_duration_ms: u128,
    max_attempt_count: u64,
    groups: BTreeMap<String, RepeatedFailureGroup>,
}

#[derive(Default)]
struct RecoveryChainStats {
    attempts: u64,
    successes: u64,
    failures: u64,
    origin_completed_ts_ms: u64,
    first_started_ts_ms: u64,
    last_completed_ts_ms: u64,
    actions: BTreeMap<String, u64>,
}

pub fn query_tool_usage(ctx: &ToolContext, args: &Value) -> Result<Value, WorkspaceError> {
    query_tool_usage_for_profile(&ctx.profile_id, args)
}

/// Query telemetry for a profile without constructing a tool execution
/// context. The desktop viewer uses this read-only path; MCP callers keep the
/// context-backed wrapper above so existing behavior is unchanged.
pub fn query_tool_usage_for_profile(
    profile_id: &str,
    args: &Value,
) -> Result<Value, WorkspaceError> {
    let limit = args
        .get("limit")
        .and_then(Value::as_u64)
        .unwrap_or(100)
        .clamp(1, 1_000) as usize;
    let cursor = args
        .get("cursor")
        .and_then(Value::as_u64)
        .unwrap_or(0)
        .min(RECORD_CURSOR_MAX as u64) as usize;
    let record_window_limit = cursor.saturating_add(limit);
    let top = args
        .get("top")
        .and_then(Value::as_u64)
        .unwrap_or(20)
        .clamp(1, 100) as usize;
    let scope = args
        .get("scope")
        .and_then(Value::as_str)
        .unwrap_or("current_runtime");
    let sort_by = args
        .get("sort_by")
        .and_then(Value::as_str)
        .unwrap_or("calls");
    let include_records = args
        .get("include_records")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let include_payloads = args
        .get("include_payloads")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let aggregate = args
        .get("aggregate")
        .and_then(Value::as_bool)
        .unwrap_or(true);
    let include_slowest = args
        .get("include_slowest")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let include_largest = args
        .get("include_largest")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let include_performance = args
        .get("include_performance")
        .and_then(Value::as_bool)
        .unwrap_or(true);
    let include_bursts = args
        .get("include_bursts")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let include_async_sessions = args
        .get("include_async_sessions")
        .and_then(Value::as_bool)
        .unwrap_or(true);
    let burst_idle_ms = args
        .get("burst_idle_ms")
        .and_then(Value::as_u64)
        .unwrap_or(DEFAULT_BURST_IDLE_MS)
        .clamp(1_000, 3_600_000);
    let errors_only = args
        .get("errors_only")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let min_duration_ms = args
        .get("min_duration_ms")
        .and_then(Value::as_u64)
        .unwrap_or(0);
    let since_ts_ms = args.get("since_ts_ms").and_then(Value::as_u64).unwrap_or(0);
    let tools = string_filter(args.get("tools"));
    let mut exclude_tools = string_filter(args.get("exclude_tools"));
    if exclude_tools.is_empty() {
        exclude_tools.push("query_tool_usage".into());
    }
    let outcomes = string_filter(args.get("outcomes"));

    let log_dir = crate::tunnel::log_dir_for_profile(profile_id);
    let paths = log_paths(&log_dir, TOOL_USAGE_LOG_FILE)
        .into_iter()
        .map(|path| ("legacy", path))
        .chain(
            log_paths(&log_dir, DIAGNOSTICS_LOG_FILE)
                .into_iter()
                .map(|path| ("canonical", path)),
        )
        .collect::<Vec<_>>();
    let mut invalid_lines = 0u64;
    let mut scanned_lines = 0u64;
    let mut log_bytes_read = 0u64;
    let mut legacy_scanned_lines = 0u64;
    let mut canonical_scanned_lines = 0u64;
    let mut matched_lines = 0u64;
    let mut matched_async_session_events = 0u64;
    let mut recent = VecDeque::with_capacity(record_window_limit);
    let mut stats = BTreeMap::<String, ToolStats>::new();
    let mut outcome_counts = BTreeMap::<String, u64>::new();
    let mut error_counts = BTreeMap::<String, u64>::new();
    let mut tool_error_counts = BTreeMap::<String, u64>::new();
    let mut transport_error_counts = BTreeMap::<String, u64>::new();
    let mut events_by_type = BTreeMap::<String, u64>::new();
    let mut standalone_transport_errors = 0u64;
    let mut repeated_identical_error_count = 0u64;
    let mut persisted_dropped_records = 0u64;
    let mut duplicate_records_ignored = 0u64;
    let mut seen_diagnostic_event_ids = BTreeSet::<String>::new();
    let mut previous_error_signature: Option<(String, String, String)> = None;
    let mut totals = ToolStats::default();
    let mut current_version_totals = ToolStats::default();
    let mut previous_version_totals = ToolStats::default();
    let mut previous_versions = BTreeSet::<String>::new();
    let mut slowest = Vec::<Value>::new();
    let mut largest = Vec::<Value>::new();
    let mut performance = PerformanceStats::default();
    let mut parallel_history = ParallelHistory::default();
    let mut repeated_failures = RepeatedFailureStats::default();
    let mut recovery_chains = BTreeMap::<String, RecoveryChainStats>::new();
    let mut call_completed_ts = BTreeMap::<u64, u64>::new();

    for (source, path) in paths {
        let (scanned, invalid, bytes_read) = visit_complete_jsonl_records(&path, |record| {
            if diagnostic_event_is_duplicate(&record, &mut seen_diagnostic_event_ids) {
                duplicate_records_ignored = duplicate_records_ignored.saturating_add(1);
                return;
            }
            let event = record
                .get("event")
                .and_then(Value::as_str)
                .unwrap_or("tool_call");
            if matches_scope_and_time(&record, scope, since_ts_ms) {
                let event_type = diagnostic_event_type(&record, event);
                *events_by_type.entry(event_type.clone()).or_default() += 1;
                if event_type == "transport_event"
                    && event != "tool_call"
                    && (record.get("failure_domain").and_then(Value::as_str) == Some("transport")
                        || record.get("severity").and_then(Value::as_str) == Some("error"))
                {
                    standalone_transport_errors = standalone_transport_errors.saturating_add(1);
                    if let Some(error_code) = record
                        .get("error_code")
                        .or_else(|| record.get("rpc_error_code"))
                        .and_then(Value::as_str)
                    {
                        *transport_error_counts
                            .entry(error_code.to_string())
                            .or_default() += 1;
                    }
                }
            }
            if event == "async_session_finalized" {
                let exec_requested = tools.is_empty()
                    || tools
                        .iter()
                        .any(|tool| matches!(tool.as_str(), "exec_command" | "exec_many"));
                let exec_excluded = exclude_tools
                    .iter()
                    .any(|tool| matches!(tool.as_str(), "exec_command" | "exec_many"));
                let child_duration = record
                    .get("child_process_total_ms")
                    .and_then(Value::as_u64)
                    .unwrap_or(0);
                if include_performance
                    && include_async_sessions
                    && exec_requested
                    && !exec_excluded
                    && !errors_only
                    && outcomes.is_empty()
                    && child_duration >= min_duration_ms
                    && matches_scope_and_time(&record, scope, since_ts_ms)
                {
                    matched_async_session_events += 1;
                    accumulate_async_session(&record, &mut performance);
                }
                return;
            }
            if event != "tool_call" {
                return;
            }
            if scope == "all"
                && matches_filters(
                    &record,
                    &tools,
                    &exclude_tools,
                    &outcomes,
                    errors_only,
                    min_duration_ms,
                    since_ts_ms,
                    "all",
                )
            {
                let version = record
                    .get("server_version")
                    .and_then(Value::as_str)
                    .unwrap_or("unknown");
                if version == env!("CARGO_PKG_VERSION") {
                    add_stats(&record, &mut current_version_totals);
                } else {
                    add_stats(&record, &mut previous_version_totals);
                    previous_versions.insert(version.to_string());
                }
            }
            if !matches_filters(
                &record,
                &tools,
                &exclude_tools,
                &outcomes,
                errors_only,
                min_duration_ms,
                since_ts_ms,
                scope,
            ) {
                return;
            }
            matched_lines += 1;
            persisted_dropped_records = persisted_dropped_records.saturating_add(
                record
                    .get("telemetry_dropped_before")
                    .and_then(Value::as_u64)
                    .unwrap_or(0),
            );
            parallel_history.accumulate_record(&record);
            let current_error_signature = error_signature(&record);
            if current_error_signature.is_some()
                && current_error_signature == previous_error_signature
            {
                repeated_identical_error_count += 1;
            }
            previous_error_signature = current_error_signature;
            accumulate_repeated_failure(&record, &mut repeated_failures);
            accumulate_recovery_chain(&record, &call_completed_ts, &mut recovery_chains);
            if let (Some(sequence), Some(completed)) = (
                record.get("call_sequence").and_then(Value::as_u64),
                record.get("completed_ts_ms").and_then(Value::as_u64),
            ) {
                call_completed_ts.insert(sequence, completed);
            }
            if include_performance {
                accumulate_performance(&record, &mut performance, burst_idle_ms);
            }
            if aggregate {
                accumulate(
                    &record,
                    &mut totals,
                    &mut stats,
                    &mut outcome_counts,
                    &mut error_counts,
                    &mut tool_error_counts,
                    &mut transport_error_counts,
                );
            }
            if include_slowest {
                push_top_record(&mut slowest, &record, "duration_ms", top);
            }
            if include_largest {
                push_top_record(&mut largest, &record, "response_json_bytes", top);
            }
            if include_records {
                if recent.len() == record_window_limit {
                    recent.pop_front();
                }
                recent.push_back(if include_payloads {
                    record
                } else {
                    compact_record(record)
                });
            }
        })?;
        scanned_lines += scanned;
        invalid_lines += invalid;
        log_bytes_read = log_bytes_read.saturating_add(bytes_read);
        if source == "legacy" {
            legacy_scanned_lines += scanned;
        } else {
            canonical_scanned_lines += scanned;
        }
    }

    let (record_page, next_record_cursor, cursor_limit_reached) =
        paginate_record_window(&recent, matched_lines, cursor, limit);

    let mut tool_stats = stats
        .into_iter()
        .map(|(tool, stats)| {
            json!({
                "tool": tool,
                "calls": stats.calls,
                "errors": stats.errors,
                "tool_errors": stats.tool_errors,
                "transport_errors": stats.transport_errors,
                "warnings": stats.warnings,
                "duration_ms": stats.duration_ms,
                "queue_wait_ms": stats.queue_wait_ms,
                "workspace_admission_wait_ms": stats.workspace_admission_wait_ms,
                "global_admission_wait_ms": stats.global_admission_wait_ms,
                "blocking_queue_wait_ms": stats.blocking_queue_wait_ms,
                "workspace_lock_wait_ms": stats.workspace_lock_wait_ms,
                "history_lock_wait_ms": stats.history_lock_wait_ms,
                "session_registry_wait_ms": stats.session_registry_wait_ms,
                "actual_wait_ms": stats.actual_wait_ms,
                "snapshot_ms": stats.snapshot_ms,
                "resource_lock_wait_ms": stats.resource_lock_wait_ms,
                "operation_lock_wait_ms": stats.operation_lock_wait_ms,
                "batch_queue_wait_ms": stats.batch_queue_wait_ms,
                "queue_nonzero": stats.queue_nonzero,
                "avg_ms": average(stats.duration_ms, stats.calls),
                "p50_ms": percentile(&stats.durations, 50),
                "p95_ms": percentile(&stats.durations, 95),
                "max_ms": stats.durations.iter().copied().max().unwrap_or(0),
                "request_bytes": stats.request_bytes,
                "response_bytes": stats.response_bytes,
                "phase_latency": phase_latency_stats(&stats),
                "optimization": {
                    "recovery_actions": stats.recovery_actions,
                    "failed_command_ids": stats.failed_command_ids,
                    "skipped_command_ids": stats.skipped_command_ids,
                    "empty_wait_timeouts": stats.empty_wait_timeouts,
                    "deduplicated_calls": stats.deduplicated_calls,
                    "heartbeat_responses": stats.heartbeat_responses,
                    "detached_responses": stats.detached_responses
                }
                ,"formatting": formatting_stats(&stats)
                ,"search": search_stats(&stats)
            })
        })
        .collect::<Vec<_>>();
    tool_stats.sort_by(|a, b| {
        metric_u64(b, sort_by)
            .cmp(&metric_u64(a, sort_by))
            .then_with(|| a["tool"].as_str().cmp(&b["tool"].as_str()))
    });
    tool_stats.truncate(top);
    let response_profile =
        if include_records || include_slowest || include_largest || include_bursts {
            "detailed"
        } else {
            "summary"
        };
    let detail_sections = json!({
        "records": include_records,
        "slowest": include_slowest,
        "largest": include_largest,
        "activity_bursts": include_bursts
    });
    let diagnostics_summary = diagnostics_summary_value(
        scope,
        &totals,
        invalid_lines,
        persisted_dropped_records,
        &tool_error_counts,
        &transport_error_counts,
        repeated_identical_error_count,
        &events_by_type,
        standalone_transport_errors,
        crate::mcp::tool_usage_log_health(),
    );
    let aggregate_value = if aggregate {
        json!({
            "calls": totals.calls,
            "errors": totals.errors,
            "tool_errors": totals.tool_errors,
            "transport_errors": totals.transport_errors,
            "warnings": totals.warnings,
            "duration_ms": totals.duration_ms,
            "queue_wait_ms": totals.queue_wait_ms,
            "workspace_admission_wait_ms": totals.workspace_admission_wait_ms,
            "global_admission_wait_ms": totals.global_admission_wait_ms,
            "blocking_queue_wait_ms": totals.blocking_queue_wait_ms,
            "workspace_lock_wait_ms": totals.workspace_lock_wait_ms,
            "history_lock_wait_ms": totals.history_lock_wait_ms,
            "session_registry_wait_ms": totals.session_registry_wait_ms,
            "actual_wait_ms": totals.actual_wait_ms,
            "snapshot_ms": totals.snapshot_ms,
            "resource_lock_wait_ms": totals.resource_lock_wait_ms,
            "operation_lock_wait_ms": totals.operation_lock_wait_ms,
            "batch_queue_wait_ms": totals.batch_queue_wait_ms,
            "queue_nonzero": totals.queue_nonzero,
            "avg_ms": average(totals.duration_ms, totals.calls),
            "p50_ms": percentile(&totals.durations, 50),
            "p95_ms": percentile(&totals.durations, 95),
            "max_ms": totals.durations.iter().copied().max().unwrap_or(0),
            "phase_latency": phase_latency_stats(&totals),
            "request_bytes": totals.request_bytes,
            "response_bytes": totals.response_bytes,
            "outcomes": outcome_counts,
            "errors_by_code": error_counts,
            "tool_errors_by_code": tool_error_counts,
            "transport_errors_by_code": transport_error_counts,
            "tools": tool_stats
        })
    } else {
        Value::Null
    };
    let scope_breakdown = if scope == "all" {
        json!({
            "current_version": {
                "version": env!("CARGO_PKG_VERSION"),
                "stats": version_scope_stats(&current_version_totals)
            },
            "previous_versions": {
                "versions": previous_versions.into_iter().collect::<Vec<_>>(),
                "stats": version_scope_stats(&previous_version_totals)
            },
            "analysis_hint": "Prioritize current_version for active defects; use previous_versions as regression and fixed-history evidence."
        })
    } else {
        Value::Null
    };
    let optimization_value = if aggregate {
        json!({
            "recovery_actions": totals.recovery_actions,
            "failed_command_ids": totals.failed_command_ids,
            "skipped_command_ids": totals.skipped_command_ids,
            "empty_wait_timeouts": totals.empty_wait_timeouts,
            "deduplicated_calls": totals.deduplicated_calls,
            "heartbeat_responses": totals.heartbeat_responses,
            "detached_responses": totals.detached_responses,
            "repeated_identical_error_count": repeated_identical_error_count,
            "repeated_failures": repeated_failure_report(
                &repeated_failures,
                repeated_identical_error_count,
                top,
            ),
            "recovery_chains": recovery_chain_report(&recovery_chains, top)
        })
    } else {
        Value::Null
    };
    let formatting_value = if aggregate {
        formatting_stats(&totals)
    } else {
        Value::Null
    };
    let search_value = if aggregate {
        search_stats(&totals)
    } else {
        Value::Null
    };
    let performance_value = if include_performance {
        performance_report(&performance, top, include_bursts, burst_idle_ms)
    } else {
        Value::Null
    };

    Ok(tool_ok(json!({
        "workspace_id": profile_id,
        "scope": scope,
        "runtime_boot_id": crate::mcp::runtime_boot_id(),
        "server_version": env!("CARGO_PKG_VERSION"),
        "diagnostics_contract": {
            "schema_version": DIAGNOSTICS_SCHEMA_VERSION,
            "host_kind": DIAGNOSTICS_HOST_KIND,
            "host_version": env!("CARGO_PKG_VERSION"),
            "structured_log_file": DIAGNOSTICS_LOG_FILE,
            "structured_log": {
                "file": DIAGNOSTICS_LOG_FILE,
                "max_bytes": STRUCTURED_LOG_MAX_BYTES,
                "retained_files": MAX_ROTATED_FILES,
                "queue_capacity": STRUCTURED_LOG_QUEUE_CAPACITY
            },
            "compatibility": {
                "legacy_file": TOOL_USAGE_LOG_FILE,
                "dual_write": crate::mcp::legacy_compat_write_enabled(),
                "dual_write_default": true,
                "legacy_write_env": crate::mcp::legacy_compat_write_env(),
                "legacy_read_preserved": true,
                "retirement_requires_compatibility_window": true,
                "query_deduplicates_by": "diagnostic_event_id"
            },
            "migration": {
                "duplicate_records_ignored": duplicate_records_ignored,
                "legacy_scanned_lines": legacy_scanned_lines,
                "canonical_scanned_lines": canonical_scanned_lines
            },
            "legacy_errors_include_transport": true
        },
        "diagnostics_summary": if aggregate { diagnostics_summary } else { Value::Null },
        "log_dir": log_dir.display().to_string(),
        "scanned_lines": scanned_lines,
        "matched_lines": matched_lines,
        "matched_async_session_events": matched_async_session_events,
        "invalid_complete_lines": invalid_lines,
        "log_bytes_read": log_bytes_read,
        "response_profile": response_profile,
        "detail_sections": detail_sections,
        "records_pagination": if include_records {
            json!({
                "cursor": cursor,
                "next_cursor": next_record_cursor,
                "total": matched_lines,
                "limit": limit,
                "cursor_origin": "latest",
                "page_order": "oldest_to_newest",
                "cursor_limit_reached": cursor_limit_reached
            })
        } else {
            Value::Null
        },
        "records": record_page,
        "slowest": if include_slowest { Value::Array(slowest) } else { Value::Null },
        "largest": if include_largest { Value::Array(largest) } else { Value::Null },
        "aggregate": aggregate_value,
        "scope_breakdown": scope_breakdown,
        "optimization": optimization_value,
        "formatting": formatting_value,
        "search": search_value,
        "parallelism": parallelism_report(&parallel_history, top),
        "performance": performance_value,
        "warnings": Vec::<String>::new()
    })))
}

fn top_error_counts(counts: &BTreeMap<String, u64>) -> Vec<Value> {
    let mut rows = counts
        .iter()
        .map(|(code, count)| json!({ "code": code, "count": count }))
        .collect::<Vec<_>>();
    rows.sort_by(|left, right| {
        right["count"]
            .as_u64()
            .cmp(&left["count"].as_u64())
            .then_with(|| left["code"].as_str().cmp(&right["code"].as_str()))
    });
    rows.truncate(5);
    rows
}

fn diagnostics_summary_value(
    scope: &str,
    stats: &ToolStats,
    invalid_complete_lines: u64,
    persisted_dropped_records: u64,
    tool_error_counts: &BTreeMap<String, u64>,
    transport_error_counts: &BTreeMap<String, u64>,
    repeated_identical_error_count: u64,
    events_by_type: &BTreeMap<String, u64>,
    standalone_transport_errors: u64,
    log_health: Value,
) -> Value {
    json!({
        "schema_version": DIAGNOSTICS_SCHEMA_VERSION,
        "host_kind": DIAGNOSTICS_HOST_KIND,
        "host_version": env!("CARGO_PKG_VERSION"),
        "scope": scope,
        "calls": stats.calls,
        "tool_errors": stats.tool_errors,
        "transport_errors": stats.transport_errors.saturating_add(standalone_transport_errors),
        "warnings": stats.warnings,
        "invalid_complete_lines": invalid_complete_lines,
        "persisted_dropped_records": persisted_dropped_records,
        "top_tool_errors": top_error_counts(tool_error_counts),
        "top_transport_errors": top_error_counts(transport_error_counts),
        "latency": {
            "p50_ms": percentile(&stats.durations, 50),
            "p95_ms": percentile(&stats.durations, 95),
            "max_ms": stats.durations.iter().copied().max().unwrap_or(0)
        },
        "wait": {
            "actual_wait_ms": stats.actual_wait_ms,
            "empty_wait_timeouts": stats.empty_wait_timeouts
        },
        "recovery": {
            "repeated_identical_error_count": repeated_identical_error_count
        },
        "events_by_type": events_by_type,
        "log_health": log_health,
        "sanitized": true
    })
}

fn version_scope_stats(stats: &ToolStats) -> Value {
    json!({
        "calls": stats.calls,
        "errors": stats.errors,
        "tool_errors": stats.tool_errors,
        "transport_errors": stats.transport_errors,
        "warnings": stats.warnings,
        "duration_ms": stats.duration_ms,
        "avg_ms": average(stats.duration_ms, stats.calls),
        "p50_ms": percentile(&stats.durations, 50),
        "p95_ms": percentile(&stats.durations, 95),
        "max_ms": stats.durations.iter().copied().max().unwrap_or(0),
        "request_bytes": stats.request_bytes,
        "response_bytes": stats.response_bytes
    })
}

fn diagnostic_event_type(record: &Value, legacy_event: &str) -> String {
    if let Some(event_type) = record.get("event_type").and_then(Value::as_str) {
        if !event_type.is_empty() {
            return event_type.to_string();
        }
    }
    if legacy_event == "async_session_finalized" {
        return "process_session".to_string();
    }
    if legacy_event == "tool_call" && is_transport_error_record(record) {
        return "transport_event".to_string();
    }
    legacy_event.to_string()
}

fn diagnostic_event_is_duplicate(record: &Value, seen: &mut BTreeSet<String>) -> bool {
    record
        .get("diagnostic_event_id")
        .and_then(Value::as_str)
        .is_some_and(|event_id| !seen.insert(event_id.to_string()))
}

fn log_paths(log_dir: &Path, file_name: &str) -> Vec<PathBuf> {
    let mut paths = (1..=MAX_ROTATED_FILES)
        .rev()
        .map(|index| log_dir.join(format!("{file_name}.{index}")))
        .collect::<Vec<_>>();
    paths.push(log_dir.join(file_name));
    paths
}

fn visit_complete_jsonl_records<F>(
    path: &Path,
    mut visit: F,
) -> Result<(u64, u64, u64), WorkspaceError>
where
    F: FnMut(Value),
{
    let file = match fs::File::open(path) {
        Ok(file) => file,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok((0, 0, 0)),
        Err(error) => {
            return Err(WorkspaceError::Tool {
                code: "LOG_READ_FAILED",
                message: format!("Unable to read {}: {error}", path.display()),
                category: "runtime",
                retryable: true,
            })
        }
    };
    let mut reader = std::io::BufReader::new(file);
    let mut buffer = Vec::with_capacity(8 * 1024);
    let mut scanned = 0u64;
    let mut invalid = 0u64;
    let mut bytes_read = 0u64;
    loop {
        buffer.clear();
        let read = reader
            .read_until(b'\n', &mut buffer)
            .map_err(|error| WorkspaceError::Tool {
                code: "LOG_READ_FAILED",
                message: format!("Unable to read {}: {error}", path.display()),
                category: "runtime",
                retryable: true,
            })?;
        if read == 0 {
            break;
        }
        bytes_read = bytes_read.saturating_add(read as u64);
        if !buffer.ends_with(b"\n") {
            break;
        }
        while buffer
            .last()
            .is_some_and(|byte| *byte == b'\n' || *byte == b'\r')
        {
            buffer.pop();
        }
        if buffer.is_empty() {
            continue;
        }
        scanned += 1;
        match serde_json::from_slice::<Value>(&buffer) {
            Ok(record) => visit(record),
            Err(_) => invalid += 1,
        }
    }
    Ok((scanned, invalid, bytes_read))
}

fn string_filter(value: Option<&Value>) -> Vec<String> {
    value
        .and_then(Value::as_array)
        .map(|values| {
            values
                .iter()
                .filter_map(Value::as_str)
                .map(str::to_string)
                .collect()
        })
        .unwrap_or_default()
}

fn matches_scope_and_time(record: &Value, scope: &str, since_ts_ms: u64) -> bool {
    let started = record
        .get("started_ts_ms")
        .and_then(Value::as_u64)
        .unwrap_or(0);
    let scope_matches = match scope {
        "all" => true,
        "current_version" => {
            record.get("server_version").and_then(Value::as_str) == Some(env!("CARGO_PKG_VERSION"))
        }
        _ => {
            record.get("runtime_boot_id").and_then(Value::as_str)
                == Some(crate::mcp::runtime_boot_id())
        }
    };
    scope_matches && started >= since_ts_ms
}

fn matches_filters(
    record: &Value,
    tools: &[String],
    exclude_tools: &[String],
    outcomes: &[String],
    errors_only: bool,
    min_duration_ms: u64,
    since_ts_ms: u64,
    scope: &str,
) -> bool {
    let tool = record.get("tool").and_then(Value::as_str).unwrap_or("");
    let outcome = normalized_outcome(record);
    let duration = record
        .get("duration_ms")
        .and_then(Value::as_u64)
        .unwrap_or(0);
    matches_scope_and_time(record, scope, since_ts_ms)
        && (tools.is_empty() || tools.iter().any(|value| value == tool))
        && !exclude_tools.iter().any(|value| value == tool)
        && (outcomes.is_empty() || outcomes.iter().any(|value| value == outcome))
        && (!errors_only || is_error_record(record))
        && duration >= min_duration_ms
}

fn error_signature(record: &Value) -> Option<(String, String, String)> {
    if !is_error_record(record) {
        return None;
    }
    Some((
        record.get("tool").and_then(Value::as_str)?.to_string(),
        record
            .get("arguments_sha256")
            .and_then(Value::as_str)?
            .to_string(),
        record
            .get("error_code")
            .or_else(|| record.get("rpc_error_code"))
            .and_then(Value::as_str)
            .unwrap_or("unknown")
            .to_string(),
    ))
}

fn accumulate(
    record: &Value,
    totals: &mut ToolStats,
    by_tool: &mut BTreeMap<String, ToolStats>,
    outcomes: &mut BTreeMap<String, u64>,
    errors: &mut BTreeMap<String, u64>,
    tool_errors: &mut BTreeMap<String, u64>,
    transport_errors: &mut BTreeMap<String, u64>,
) {
    let tool = record
        .get("tool")
        .and_then(Value::as_str)
        .unwrap_or("unknown")
        .to_string();
    let outcome = normalized_outcome(record).to_string();
    *outcomes.entry(outcome.clone()).or_default() += 1;
    let error_code = record
        .get("error_code")
        .or_else(|| record.get("rpc_error_code"))
        .and_then(Value::as_str);
    if let Some(error_code) = error_code {
        *errors.entry(error_code.to_string()).or_default() += 1;
        let domain_errors = if is_transport_error_record(record) {
            Some(transport_errors)
        } else if is_tool_error_record(record) {
            Some(tool_errors)
        } else {
            None
        };
        if let Some(domain_errors) = domain_errors {
            *domain_errors.entry(error_code.to_string()).or_default() += 1;
        }
    }
    add_stats(record, totals);
    add_stats(record, by_tool.entry(tool).or_default());
}

fn add_stats(record: &Value, stats: &mut ToolStats) {
    let duration = record
        .get("duration_ms")
        .and_then(Value::as_u64)
        .unwrap_or(0);
    stats.calls += 1;
    stats.errors += u64::from(is_error_record(record));
    stats.tool_errors += u64::from(is_tool_error_record(record));
    stats.transport_errors += u64::from(is_transport_error_record(record));
    stats.warnings += record
        .get("warning_count")
        .and_then(Value::as_u64)
        .unwrap_or(0);
    stats.duration_ms += duration as u128;
    let queue_wait = record
        .get("admission_queue_wait_ms")
        .and_then(Value::as_u64)
        .unwrap_or(0)
        + record
            .get("blocking_queue_wait_ms")
            .and_then(Value::as_u64)
            .unwrap_or(0);
    stats.queue_wait_ms += queue_wait as u128;
    stats.workspace_admission_wait_ms += metric(record, "workspace_admission_wait_ms");
    stats.global_admission_wait_ms += metric(record, "global_admission_wait_ms");
    stats.blocking_queue_wait_ms += metric(record, "blocking_queue_wait_ms");
    stats.workspace_lock_wait_ms += metric(record, "workspace_lock_wait_ms");
    stats.history_lock_wait_ms += metric(record, "history_lock_wait_ms");
    stats.session_registry_wait_ms += metric(record, "session_registry_wait_ms");
    stats.actual_wait_ms += metric(record, "actual_wait_ms");
    stats.snapshot_ms += metric(record, "snapshot_ms");
    stats.resource_lock_wait_ms += metric(record, "resource_lock_wait_ms");
    stats.operation_lock_wait_ms += metric(record, "operation_lock_wait_ms");
    stats.batch_queue_wait_ms += metric(record, "batch_queue_wait_ms");
    stats.queue_nonzero += u64::from(queue_wait > 0);
    stats.request_bytes += record
        .get("request_json_bytes")
        .and_then(Value::as_u64)
        .unwrap_or(0);
    stats.response_bytes += record
        .get("response_json_bytes")
        .and_then(Value::as_u64)
        .unwrap_or(0);
    stats.recovery_actions += record
        .get("recovery_action_count")
        .and_then(Value::as_u64)
        .unwrap_or(0);
    stats.failed_command_ids += record
        .get("failed_command_id_count")
        .and_then(Value::as_u64)
        .unwrap_or(0);
    stats.skipped_command_ids += record
        .get("skipped_command_id_count")
        .and_then(Value::as_u64)
        .unwrap_or(0);
    stats.empty_wait_timeouts += u64::from(
        record.get("tool").and_then(Value::as_str) == Some("wait_command")
            && record.get("request_timed_out").and_then(Value::as_bool) == Some(true)
            && record
                .get("event_count")
                .and_then(Value::as_u64)
                .unwrap_or(0)
                == 0,
    );
    stats.deduplicated_calls +=
        u64::from(record.get("deduplicated").and_then(Value::as_bool) == Some(true));
    stats.heartbeat_responses +=
        u64::from(record.get("heartbeat").and_then(Value::as_bool) == Some(true));
    stats.detached_responses +=
        u64::from(record.get("detached").and_then(Value::as_bool) == Some(true));
    stats.format_files_requested += record
        .get("format_files_requested")
        .and_then(Value::as_u64)
        .unwrap_or(0);
    stats.format_files_supported += record
        .get("format_files_supported")
        .and_then(Value::as_u64)
        .unwrap_or(0);
    stats.format_files_changed += record
        .get("format_files_changed_count")
        .and_then(Value::as_u64)
        .unwrap_or(0);
    stats.format_files_unchanged += record
        .get("format_files_unchanged_count")
        .and_then(Value::as_u64)
        .unwrap_or(0);
    stats.format_files_skipped += record
        .get("format_files_skipped_count")
        .and_then(Value::as_u64)
        .unwrap_or(0);
    stats.formatter_groups += record
        .get("format_formatter_group_count")
        .and_then(Value::as_u64)
        .unwrap_or(0);
    stats.custom_formatter_groups += record
        .get("format_custom_formatter_group_count")
        .and_then(Value::as_u64)
        .unwrap_or(0);
    stats.unavailable_adapters += record
        .get("format_unavailable_adapter_count")
        .and_then(Value::as_u64)
        .unwrap_or(0);
    stats.unexpected_changes += record
        .get("format_unexpected_change_count")
        .and_then(Value::as_u64)
        .unwrap_or(0);
    stats.format_diff_bytes += record
        .get("format_diff_bytes")
        .and_then(Value::as_u64)
        .unwrap_or(0);
    stats.format_apply_calls +=
        u64::from(record.get("format_applied").and_then(Value::as_bool) == Some(true));
    if record.get("tool").and_then(Value::as_str) == Some("search_text") {
        let returned = record
            .get("returned_count")
            .and_then(Value::as_u64)
            .unwrap_or(0);
        stats.search_files_considered += record
            .get("files_considered")
            .and_then(Value::as_u64)
            .unwrap_or(0);
        stats.search_files_scanned += record
            .get("scanned_files")
            .and_then(Value::as_u64)
            .unwrap_or(0);
        stats.search_returned += returned;
        stats.search_matched_files += record
            .get("matched_files")
            .and_then(Value::as_u64)
            .unwrap_or(0);
        stats.search_zero_result_calls += u64::from(returned == 0);
        stats.search_early_stops += u64::from(
            record.get("early_stop_reason").and_then(Value::as_str) == Some("result_limit"),
        );
        stats.search_exact_total_calls +=
            u64::from(record.get("total_matches_exact").and_then(Value::as_bool) == Some(true));
    }
    for (phase, field) in PHASE_METRICS {
        if let Some(duration) = record.get(field).and_then(Value::as_u64) {
            let phase_stats = stats.phase_latency.entry(phase.to_string()).or_default();
            phase_stats.total_ms += duration as u128;
            phase_stats.durations.push(duration);
        }
    }
    stats.durations.push(duration);
}

fn formatting_stats(stats: &ToolStats) -> Value {
    json!({
        "files_requested": stats.format_files_requested,
        "files_supported": stats.format_files_supported,
        "files_changed": stats.format_files_changed,
        "files_unchanged": stats.format_files_unchanged,
        "files_skipped": stats.format_files_skipped,
        "formatter_groups": stats.formatter_groups,
        "custom_formatter_groups": stats.custom_formatter_groups,
        "unavailable_adapters": stats.unavailable_adapters,
        "unexpected_changes": stats.unexpected_changes,
        "diff_bytes": stats.format_diff_bytes,
        "apply_calls": stats.format_apply_calls
    })
}

fn search_stats(stats: &ToolStats) -> Value {
    json!({
        "files_considered": stats.search_files_considered,
        "files_scanned": stats.search_files_scanned,
        "returned_results": stats.search_returned,
        "matched_files": stats.search_matched_files,
        "zero_result_calls": stats.search_zero_result_calls,
        "early_stop_calls": stats.search_early_stops,
        "exact_total_calls": stats.search_exact_total_calls
    })
}

fn phase_latency_stats(stats: &ToolStats) -> Value {
    let mut phases = serde_json::Map::new();
    for (phase, _) in PHASE_METRICS {
        let values = stats.phase_latency.get(phase);
        let total_ms = values.map(|value| value.total_ms).unwrap_or(0);
        let durations = values
            .map(|value| value.durations.as_slice())
            .unwrap_or_default();
        let samples = durations.len() as u64;
        phases.insert(
            phase.to_string(),
            json!({
                "samples": samples,
                "total_ms": total_ms,
                "avg_ms": average(total_ms, samples),
                "p50_ms": percentile(durations, 50),
                "p95_ms": percentile(durations, 95),
                "max_ms": durations.iter().copied().max().unwrap_or(0)
            }),
        );
    }
    Value::Object(phases)
}

fn metric(record: &Value, name: &str) -> u128 {
    record.get(name).and_then(Value::as_u64).unwrap_or(0) as u128
}

fn accumulate_repeated_failure(record: &Value, stats: &mut RepeatedFailureStats) {
    if record.get("repeated_failure").and_then(Value::as_bool) != Some(true) {
        return;
    }
    let Some(signature) = record.get("failure_signature").and_then(Value::as_str) else {
        return;
    };
    let attempt_count = record
        .get("repeat_failure_count")
        .and_then(Value::as_u64)
        .unwrap_or(2)
        .max(2);
    let duration = record
        .get("duration_ms")
        .and_then(Value::as_u64)
        .unwrap_or(0);
    let group = stats
        .groups
        .entry(signature.to_string())
        .or_insert_with(|| RepeatedFailureGroup {
            tool: record
                .get("tool")
                .and_then(Value::as_str)
                .unwrap_or("unknown")
                .to_string(),
            error_code: record
                .get("error_code")
                .or_else(|| record.get("rpc_error_code"))
                .and_then(Value::as_str)
                .unwrap_or("unknown")
                .to_string(),
            ..RepeatedFailureGroup::default()
        });
    stats.retry_count += 1;
    stats.chain_count += u64::from(attempt_count == 2);
    stats.wasted_duration_ms += duration as u128;
    stats.max_attempt_count = stats.max_attempt_count.max(attempt_count);
    group.retry_count += 1;
    group.chain_count += u64::from(attempt_count == 2);
    group.wasted_duration_ms += duration as u128;
    group.max_attempt_count = group.max_attempt_count.max(attempt_count);
}

fn deterministic_failure_weight(error_code: &str) -> u128 {
    let upper = error_code.to_ascii_uppercase();
    if [
        "INVALID",
        "MISMATCH",
        "NOT_FOUND",
        "PATH_",
        "POLICY",
        "CONTRACT",
        "EXPECTED_",
        "PROTECTED",
        "PATCH_",
    ]
    .iter()
    .any(|marker| upper.contains(marker))
    {
        2
    } else {
        1
    }
}

fn repeated_failure_friction_score(group: &RepeatedFailureGroup) -> u128 {
    u128::from(group.retry_count)
        .saturating_mul(group.wasted_duration_ms.max(1))
        .saturating_mul(deterministic_failure_weight(&group.error_code))
}

fn repeated_failure_report(
    stats: &RepeatedFailureStats,
    legacy_adjacent_retry_count: u64,
    top: usize,
) -> Value {
    let friction_score = stats
        .groups
        .values()
        .map(repeated_failure_friction_score)
        .fold(0u128, u128::saturating_add);
    let mut groups = stats
        .groups
        .iter()
        .map(|(signature, group)| {
            let group_friction_score = repeated_failure_friction_score(group);
            json!({
                "signature": signature,
                "tool": group.tool,
                "error_code": group.error_code,
                "retry_count": group.retry_count,
                "chain_count": group.chain_count,
                "wasted_duration_ms": group.wasted_duration_ms,
                "max_attempt_count": group.max_attempt_count,
                "deterministic_error_weight": deterministic_failure_weight(&group.error_code),
                "friction_score": group_friction_score
            })
        })
        .collect::<Vec<_>>();
    groups.sort_by(|left, right| {
        metric_u64(right, "friction_score")
            .cmp(&metric_u64(left, "friction_score"))
            .then_with(|| metric_u64(right, "retry_count").cmp(&metric_u64(left, "retry_count")))
            .then_with(|| {
                metric_u64(right, "wasted_duration_ms").cmp(&metric_u64(left, "wasted_duration_ms"))
            })
            .then_with(|| left["signature"].as_str().cmp(&right["signature"].as_str()))
    });
    groups.truncate(top);
    json!({
        "retry_count": stats.retry_count,
        "chain_count": stats.chain_count,
        "wasted_duration_ms": stats.wasted_duration_ms,
        "max_attempt_count": stats.max_attempt_count,
        "friction_score": friction_score,
        "legacy_adjacent_retry_count": legacy_adjacent_retry_count,
        "top": groups,
        "recovery_hint": if stats.retry_count > 0 {
            Value::String(
                "Stop retrying unchanged arguments. Change the target or guard, or follow the tool recovery_actions before retrying."
                    .to_string(),
            )
        } else {
            Value::Null
        }
    })
}

fn recovery_chain_key(record: &Value) -> Option<String> {
    if let Some(operation) = record
        .get("recovery_of_operation_id_hash")
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
    {
        return Some(format!("operation:{operation}"));
    }
    record
        .get("retry_of_call_sequence")
        .and_then(Value::as_u64)
        .filter(|value| *value > 0)
        .map(|sequence| format!("call:{sequence}"))
}

fn accumulate_recovery_chain(
    record: &Value,
    call_completed_ts: &BTreeMap<u64, u64>,
    chains: &mut BTreeMap<String, RecoveryChainStats>,
) {
    let Some(key) = recovery_chain_key(record) else {
        return;
    };
    let started = record
        .get("started_ts_ms")
        .and_then(Value::as_u64)
        .unwrap_or(0);
    let completed = record
        .get("completed_ts_ms")
        .and_then(Value::as_u64)
        .unwrap_or(started);
    let chain = chains.entry(key).or_default();
    chain.attempts = chain.attempts.saturating_add(1);
    if is_error_record(record) {
        chain.failures = chain.failures.saturating_add(1);
    } else {
        chain.successes = chain.successes.saturating_add(1);
    }
    if chain.first_started_ts_ms == 0 || started < chain.first_started_ts_ms {
        chain.first_started_ts_ms = started;
    }
    chain.last_completed_ts_ms = chain.last_completed_ts_ms.max(completed);
    if let Some(origin) = record
        .get("retry_of_call_sequence")
        .and_then(Value::as_u64)
        .and_then(|sequence| call_completed_ts.get(&sequence).copied())
    {
        if chain.origin_completed_ts_ms == 0 || origin < chain.origin_completed_ts_ms {
            chain.origin_completed_ts_ms = origin;
        }
    }
    if let Some(action) = record
        .get("recovery_action_id")
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
    {
        *chain.actions.entry(action.to_string()).or_default() += 1;
    }
}

fn recovery_chain_report(chains: &BTreeMap<String, RecoveryChainStats>, top: usize) -> Value {
    let mut rows = chains
        .iter()
        .map(|(key, chain)| {
            let origin = if chain.origin_completed_ts_ms > 0 {
                chain.origin_completed_ts_ms
            } else {
                chain.first_started_ts_ms
            };
            json!({
                "chain": key,
                "attempts": chain.attempts,
                "successes": chain.successes,
                "failures": chain.failures,
                "succeeded": chain.successes > 0,
                "elapsed_ms": chain.last_completed_ts_ms.saturating_sub(origin),
                "actions": chain.actions
            })
        })
        .collect::<Vec<_>>();
    rows.sort_by(|left, right| {
        metric_u64(right, "attempts")
            .cmp(&metric_u64(left, "attempts"))
            .then_with(|| left["chain"].as_str().cmp(&right["chain"].as_str()))
    });
    rows.truncate(top);
    json!({
        "chain_count": chains.len(),
        "attempts": chains.values().map(|chain| chain.attempts).sum::<u64>(),
        "successful_chains": chains.values().filter(|chain| chain.successes > 0).count(),
        "failed_chains": chains.values().filter(|chain| chain.successes == 0).count(),
        "top": rows
    })
}

fn accumulate_performance(record: &Value, performance: &mut PerformanceStats, burst_idle_ms: u64) {
    let started = record
        .get("started_ts_ms")
        .and_then(Value::as_u64)
        .unwrap_or(0);
    let duration = record
        .get("duration_ms")
        .and_then(Value::as_u64)
        .unwrap_or(0);
    let completed = record
        .get("completed_ts_ms")
        .and_then(Value::as_u64)
        .unwrap_or_else(|| started.saturating_add(duration));
    let queue_wait = record
        .get("admission_queue_wait_ms")
        .and_then(Value::as_u64)
        .unwrap_or(0)
        .saturating_add(
            record
                .get("blocking_queue_wait_ms")
                .and_then(Value::as_u64)
                .unwrap_or(0),
        );
    let recorded_gap = record.get("orchestration_gap_ms").and_then(Value::as_u64);
    let concurrent_request = record
        .get("concurrent_request")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let derived_gap = (!concurrent_request && performance.previous_completed_ts_ms > 0)
        .then(|| started.saturating_sub(performance.previous_completed_ts_ms));
    let gap = if concurrent_request {
        None
    } else {
        recorded_gap.or(derived_gap)
    };

    performance.tool_calls += 1;
    performance.server_duration_ms += duration as u128;
    performance.queue_wait_ms += queue_wait as u128;
    if performance.first_started_ts_ms == 0 || started < performance.first_started_ts_ms {
        performance.first_started_ts_ms = started;
    }
    performance.last_completed_ts_ms = performance.last_completed_ts_ms.max(completed);
    performance.previous_completed_ts_ms = performance.previous_completed_ts_ms.max(completed);

    if let Some(gap) = gap {
        if gap > burst_idle_ms {
            performance.idle_gap_ms += gap as u128;
            performance.idle_gap_count += 1;
        } else {
            performance.active_orchestration_gap_ms += gap as u128;
            performance.orchestration_gaps.push(gap);
        }
    }

    if performance.inferred_burst_id == 0 {
        performance.inferred_burst_id = 1;
    } else if gap.is_some_and(|gap| gap > burst_idle_ms) {
        performance.inferred_burst_id = performance.inferred_burst_id.saturating_add(1);
    }
    let runtime_boot_id = record
        .get("runtime_boot_id")
        .and_then(Value::as_str)
        .unwrap_or("legacy");
    let burst_id = record
        .get("activity_burst_id")
        .and_then(Value::as_u64)
        .unwrap_or(performance.inferred_burst_id);
    let burst_key = format!("{runtime_boot_id}:{burst_id}");
    let burst = performance.bursts.entry(burst_key).or_default();
    burst.calls += 1;
    if burst.first_started_ts_ms == 0 || started < burst.first_started_ts_ms {
        burst.first_started_ts_ms = started;
    }
    burst.last_completed_ts_ms = burst.last_completed_ts_ms.max(completed);
    burst.server_duration_ms += duration as u128;
    if let Some(gap) = gap.filter(|gap| *gap <= burst_idle_ms) {
        burst.orchestration_gap_ms += gap as u128;
    }
    let tool = record
        .get("tool")
        .and_then(Value::as_str)
        .unwrap_or("unknown");
    *burst.tools.entry(tool.to_string()).or_default() += 1;
    if tool == "exec_command" && !concurrent_request {
        burst.sequential_exec_commands += 1;
    } else if tool == "exec_many" {
        burst.exec_many_calls += 1;
    }

    if let Some(kind) = record.get("command_kind").and_then(Value::as_str) {
        let kind_stats = performance
            .command_kinds
            .entry(kind.to_string())
            .or_default();
        kind_stats.calls += 1;
        kind_stats.server_duration_ms += duration as u128;
    }
}

fn accumulate_async_session(record: &Value, performance: &mut PerformanceStats) {
    let started = record
        .get("started_ts_ms")
        .and_then(Value::as_u64)
        .unwrap_or(0);
    let completed = record
        .get("completed_ts_ms")
        .and_then(Value::as_u64)
        .unwrap_or(started);
    let child_process_ms = record
        .get("child_process_total_ms")
        .and_then(Value::as_u64)
        .unwrap_or(0);
    performance.async_sessions += 1;
    if performance.first_started_ts_ms == 0 || started < performance.first_started_ts_ms {
        performance.first_started_ts_ms = started;
    }
    performance.last_completed_ts_ms = performance.last_completed_ts_ms.max(completed);
    performance.child_process_ms += child_process_ms as u128;
    performance.child_durations.push(child_process_ms);
    if let Some(first_output_ms) = record.get("first_output_ms").and_then(Value::as_u64) {
        performance.first_output_durations.push(first_output_ms);
    }
    if record
        .get("exit_code")
        .and_then(Value::as_i64)
        .is_some_and(|exit_code| exit_code != 0)
    {
        performance.child_failures += 1;
    }
    let termination = record
        .get("termination_reason")
        .and_then(Value::as_str)
        .unwrap_or("unknown");
    *performance
        .child_terminations
        .entry(termination.to_string())
        .or_default() += 1;
    let kind = record
        .get("command_kind")
        .and_then(Value::as_str)
        .unwrap_or("process");
    let kind_stats = performance
        .command_kinds
        .entry(kind.to_string())
        .or_default();
    kind_stats.child_sessions += 1;
    kind_stats.child_process_ms += child_process_ms as u128;
}

fn performance_report(
    performance: &PerformanceStats,
    top: usize,
    include_bursts: bool,
    burst_idle_ms: u64,
) -> Value {
    let observed_wall_ms = performance
        .last_completed_ts_ms
        .saturating_sub(performance.first_started_ts_ms);
    let attributed_nonoverlap = performance
        .server_duration_ms
        .saturating_add(performance.active_orchestration_gap_ms);
    let server_share = percentage(performance.server_duration_ms, attributed_nonoverlap);
    let orchestration_share = percentage(
        performance.active_orchestration_gap_ms,
        attributed_nonoverlap,
    );
    let dominant = if performance.active_orchestration_gap_ms > performance.server_duration_ms {
        "client_orchestration_gap"
    } else if performance.server_duration_ms > 0 {
        "server_tool_execution"
    } else {
        "insufficient_data"
    };

    let mut command_kinds = performance
        .command_kinds
        .iter()
        .map(|(kind, stats)| {
            json!({
                "command_kind": kind,
                "calls": stats.calls,
                "server_duration_ms": stats.server_duration_ms,
                "child_sessions": stats.child_sessions,
                "child_process_ms": stats.child_process_ms
            })
        })
        .collect::<Vec<_>>();
    command_kinds.sort_by(|a, b| {
        let a_total = a["server_duration_ms"]
            .as_u64()
            .unwrap_or(0)
            .saturating_add(a["child_process_ms"].as_u64().unwrap_or(0));
        let b_total = b["server_duration_ms"]
            .as_u64()
            .unwrap_or(0)
            .saturating_add(b["child_process_ms"].as_u64().unwrap_or(0));
        b_total.cmp(&a_total)
    });
    command_kinds.truncate(top);

    let opportunity_burst_count = performance
        .bursts
        .values()
        .filter(|burst| burst.sequential_exec_commands >= 2 && burst.exec_many_calls == 0)
        .count();
    let parallelizable_exec_command_candidates = performance
        .bursts
        .values()
        .filter(|burst| burst.sequential_exec_commands >= 2 && burst.exec_many_calls == 0)
        .map(|burst| burst.sequential_exec_commands)
        .sum::<u64>();
    let estimated_tool_call_reduction = performance
        .bursts
        .values()
        .filter(|burst| burst.sequential_exec_commands >= 2 && burst.exec_many_calls == 0)
        .map(|burst| burst.sequential_exec_commands.saturating_sub(1))
        .sum::<u64>();

    let bursts = if include_bursts {
        let mut bursts = performance
            .bursts
            .iter()
            .map(|(burst_id, burst)| {
                json!({
                    "burst_id": burst_id,
                    "calls": burst.calls,
                    "started_ts_ms": burst.first_started_ts_ms,
                    "completed_ts_ms": burst.last_completed_ts_ms,
                    "wall_ms": burst.last_completed_ts_ms.saturating_sub(burst.first_started_ts_ms),
                    "server_duration_ms": burst.server_duration_ms,
                    "orchestration_gap_ms": burst.orchestration_gap_ms,
                    "tools": burst.tools,
                    "sequential_exec_commands": burst.sequential_exec_commands,
                    "exec_many_calls": burst.exec_many_calls,
                    "parallelization_opportunity": burst.sequential_exec_commands >= 2 && burst.exec_many_calls == 0,
                    "estimated_tool_call_reduction": if burst.sequential_exec_commands >= 2 && burst.exec_many_calls == 0 {
                        burst.sequential_exec_commands.saturating_sub(1)
                    } else {
                        0
                    }
                })
            })
            .collect::<Vec<_>>();
        bursts.sort_by(|a, b| {
            b["started_ts_ms"]
                .as_u64()
                .unwrap_or(0)
                .cmp(&a["started_ts_ms"].as_u64().unwrap_or(0))
        });
        bursts.truncate(top);
        Value::Array(bursts)
    } else {
        Value::Null
    };

    json!({
        "tool_calls": performance.tool_calls,
        "async_sessions_finalized": performance.async_sessions,
        "observed_wall_ms": observed_wall_ms,
        "server_tool_duration_ms": performance.server_duration_ms,
        "server_queue_wait_ms": performance.queue_wait_ms,
        "client_orchestration_gap_ms": performance.active_orchestration_gap_ms,
        "client_orchestration_gap_p50_ms": percentile(&performance.orchestration_gaps, 50),
        "client_orchestration_gap_p95_ms": percentile(&performance.orchestration_gaps, 95),
        "idle_gap_ms": performance.idle_gap_ms,
        "idle_gap_count": performance.idle_gap_count,
        "burst_idle_threshold_ms": burst_idle_ms,
        "child_process_lifetime_ms": performance.child_process_ms,
        "child_process_failures": performance.child_failures,
        "child_process_terminations": performance.child_terminations,
        "child_process_p50_ms": percentile(&performance.child_durations, 50),
        "child_process_p95_ms": percentile(&performance.child_durations, 95),
        "first_output_p50_ms": percentile(&performance.first_output_durations, 50),
        "first_output_p95_ms": percentile(&performance.first_output_durations, 95),
        "server_share_of_nonidle_attributed_percent": server_share,
        "client_orchestration_share_of_nonidle_attributed_percent": orchestration_share,
        "dominant_observed_nonidle_source": dominant,
        "parallelization_opportunity_bursts": opportunity_burst_count,
        "parallelizable_exec_command_candidates": parallelizable_exec_command_candidates,
        "estimated_tool_call_reduction": estimated_tool_call_reduction,
        "parallelization_recommendation": if opportunity_burst_count > 0 {
            "Review sequential exec_command calls in the flagged activity bursts and consolidate independent work with exec_many mode=auto."
        } else {
            "No repeated sequential exec_command burst was detected in the selected scope."
        },
        "command_kinds": command_kinds,
        "activity_bursts": bursts,
        "attribution_note": "client_orchestration_gap is observed between the previous tool response completing and the next tool request arriving. It includes model reasoning, platform scheduling, connector/network latency, and client-side orchestration; it is not pure LLM inference time. Child-process lifetime may overlap both server duration and orchestration gaps and must not be added as an independent wall-time component."
    })
}

fn percentage(value: u128, total: u128) -> f64 {
    if total == 0 {
        0.0
    } else {
        value as f64 * 100.0 / total as f64
    }
}

fn normalized_outcome(record: &Value) -> &str {
    if let Some(outcome) = record.get("outcome").and_then(Value::as_str) {
        return outcome;
    }
    if record.get("is_error").and_then(Value::as_bool) == Some(true)
        || record.get("ok").and_then(Value::as_bool) == Some(false)
    {
        "legacy_error"
    } else if record.get("ok").and_then(Value::as_bool) == Some(true) {
        "success"
    } else {
        "legacy_unknown"
    }
}

fn is_error_record(record: &Value) -> bool {
    if record.get("is_error").and_then(Value::as_bool) == Some(true) {
        return true;
    }
    if record.get("ok").and_then(Value::as_bool) == Some(false) {
        return true;
    }
    matches!(
        normalized_outcome(record),
        "rpc_error" | "tool_error" | "worker_failed" | "legacy_error"
    )
}

fn is_transport_error_record(record: &Value) -> bool {
    record.get("failure_domain").and_then(Value::as_str) == Some("transport")
        || normalized_outcome(record) == "rpc_error"
}

fn is_tool_error_record(record: &Value) -> bool {
    is_error_record(record) && !is_transport_error_record(record)
}

fn metric_u64(record: &Value, field: &str) -> u64 {
    match field {
        "p95_ms" => record.get("p95_ms").and_then(Value::as_u64).unwrap_or(0),
        "errors" => record.get("errors").and_then(Value::as_u64).unwrap_or(0),
        "duration_ms" => record
            .get("duration_ms")
            .and_then(Value::as_u64)
            .unwrap_or(0),
        "response_bytes" => record
            .get("response_bytes")
            .and_then(Value::as_u64)
            .unwrap_or(0),
        "request_bytes" => record
            .get("request_bytes")
            .and_then(Value::as_u64)
            .unwrap_or(0),
        "queue_wait_ms" => record
            .get("queue_wait_ms")
            .and_then(Value::as_u64)
            .unwrap_or(0),
        _ => record.get("calls").and_then(Value::as_u64).unwrap_or(0),
    }
}

fn push_top_record(records: &mut Vec<Value>, record: &Value, field: &str, top: usize) {
    records.push(compact_record(record.clone()));
    records.sort_by(|a, b| {
        b.get(field)
            .and_then(Value::as_u64)
            .unwrap_or(0)
            .cmp(&a.get(field).and_then(Value::as_u64).unwrap_or(0))
    });
    records.truncate(top);
}

fn compact_record(mut record: Value) -> Value {
    if let Some(object) = record.as_object_mut() {
        for field in [
            "arguments",
            "arguments_json",
            "argument_field_bytes",
            "stdout",
            "stderr",
        ] {
            object.remove(field);
        }
    }
    record
}

fn average(duration_ms: u128, calls: u64) -> f64 {
    if calls == 0 {
        0.0
    } else {
        duration_ms as f64 / calls as f64
    }
}

fn percentile(values: &[u64], percentile: usize) -> u64 {
    if values.is_empty() {
        return 0;
    }
    let mut sorted = values.to_vec();
    sorted.sort_unstable();
    let index = ((sorted.len() - 1) * percentile).div_ceil(100);
    sorted[index.min(sorted.len() - 1)]
}

fn paginate_record_window(
    records: &VecDeque<Value>,
    total: u64,
    cursor: usize,
    limit: usize,
) -> (Vec<Value>, Option<usize>, bool) {
    let page_end = records.len().saturating_sub(cursor.min(records.len()));
    let page_start = page_end.saturating_sub(limit);
    let page = records
        .iter()
        .skip(page_start)
        .take(page_end.saturating_sub(page_start))
        .cloned()
        .collect::<Vec<_>>();
    let next_candidate = cursor.saturating_add(page.len());
    let has_older_records = total > next_candidate as u64;
    let cursor_limit_reached = has_older_records && next_candidate > RECORD_CURSOR_MAX;
    let next_cursor = if has_older_records && !cursor_limit_reached {
        Some(next_candidate)
    } else {
        None
    };
    (page, next_cursor, cursor_limit_reached)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn percentile_is_bounded_and_stable() {
        assert_eq!(percentile(&[], 95), 0);
        assert_eq!(percentile(&[1, 2, 3, 4, 100], 50), 3);
        assert_eq!(percentile(&[1, 2, 3, 4, 100], 95), 100);
    }

    #[test]
    fn canonical_diagnostics_dual_read_deduplicates_event_ids_but_keeps_legacy_records() {
        let mut seen = BTreeSet::new();
        let canonical = json!({ "diagnostic_event_id": "event-1", "event": "tool_call" });
        let legacy_copy = canonical.clone();
        let legacy_without_id = json!({ "event": "tool_call", "tool": "server_info" });

        assert!(!diagnostic_event_is_duplicate(&canonical, &mut seen));
        assert!(diagnostic_event_is_duplicate(&legacy_copy, &mut seen));
        assert!(!diagnostic_event_is_duplicate(
            &legacy_without_id,
            &mut seen
        ));
        assert!(!diagnostic_event_is_duplicate(
            &legacy_without_id,
            &mut seen
        ));
    }

    #[test]
    fn record_pages_walk_backward_from_latest_without_reversing_page_order() {
        let records = (1..=5)
            .map(|index| json!({ "started_ts_ms": index * 1_000 }))
            .collect::<VecDeque<_>>();

        let (first, first_next, first_limited) = paginate_record_window(&records, 5, 0, 2);
        assert_eq!(
            first
                .iter()
                .map(|record| record["started_ts_ms"].as_u64().unwrap())
                .collect::<Vec<_>>(),
            vec![4_000, 5_000]
        );
        assert_eq!(first_next, Some(2));
        assert!(!first_limited);

        let (second, second_next, second_limited) = paginate_record_window(&records, 5, 2, 2);
        assert_eq!(
            second
                .iter()
                .map(|record| record["started_ts_ms"].as_u64().unwrap())
                .collect::<Vec<_>>(),
            vec![2_000, 3_000]
        );
        assert_eq!(second_next, Some(4));
        assert!(!second_limited);

        let (third, third_next, third_limited) = paginate_record_window(&records, 5, 4, 2);
        assert_eq!(
            third
                .iter()
                .map(|record| record["started_ts_ms"].as_u64().unwrap())
                .collect::<Vec<_>>(),
            vec![1_000]
        );
        assert_eq!(third_next, None);
        assert!(!third_limited);
    }

    #[test]
    fn incomplete_jsonl_tail_is_ignored() {
        let temp = tempfile::tempdir().expect("tempdir");
        let path = temp.path().join("usage.jsonl");
        std::fs::write(
            &path,
            b"{\"tool\":\"server_info\",\"outcome\":\"success\"}\n{\"tool\":",
        )
        .expect("write log");
        let mut records = Vec::new();
        let (scanned, invalid, bytes_read) =
            visit_complete_jsonl_records(&path, |record| records.push(record)).expect("read log");
        assert_eq!(scanned, 1);
        assert_eq!(invalid, 0);
        assert_eq!(
            bytes_read,
            std::fs::metadata(&path).expect("metadata").len()
        );
        assert_eq!(records.len(), 1);
        assert_eq!(records[0]["tool"], "server_info");
    }

    #[test]
    fn error_signature_identifies_exact_retries_only() {
        let failed = json!({
            "tool": "edit_file",
            "outcome": "tool_error",
            "arguments_sha256": "abc",
            "error_code": "FILE_VERSION_MISMATCH"
        });
        let same = failed.clone();
        let different = json!({
            "tool": "edit_file",
            "outcome": "tool_error",
            "arguments_sha256": "def",
            "error_code": "FILE_VERSION_MISMATCH"
        });
        assert_eq!(error_signature(&failed), error_signature(&same));
        assert_ne!(error_signature(&failed), error_signature(&different));
        assert_eq!(
            error_signature(&json!({"tool": "read_file", "outcome": "success"})),
            None
        );
    }

    #[test]
    fn transport_failures_are_split_from_tool_failures() {
        let contract: Value = serde_json::from_str(include_str!(
            "../../../docs/specs/diagnostics/contract-v1.json"
        ))
        .expect("diagnostics contract should be valid JSON");
        assert_eq!(contract["schema_version"], DIAGNOSTICS_SCHEMA_VERSION);
        assert_eq!(contract["host_kinds"]["rust"], DIAGNOSTICS_HOST_KIND);
        assert_eq!(contract["structured_log"]["file"], DIAGNOSTICS_LOG_FILE);
        assert_eq!(
            contract["compatibility"]["legacy_file"],
            TOOL_USAGE_LOG_FILE
        );
        assert_eq!(contract["compatibility"]["dual_write"], true);
        assert_eq!(contract["records_pagination"]["cursor_origin"], "latest");
        assert_eq!(
            contract["records_pagination"]["page_order"],
            "oldest_to_newest"
        );
        assert_eq!(contract["records_pagination"]["max_cursor"], 10_000);
        assert_eq!(contract["compatibility"]["dual_write_default"], true);
        assert_eq!(
            contract["compatibility"]["legacy_write_env"],
            "CODING_TOOLS_DIAGNOSTICS_LEGACY_WRITE"
        );
        assert_eq!(contract["compatibility"]["legacy_read_preserved"], true);
        assert_eq!(
            contract["compatibility"]["retirement_requires_compatibility_window"],
            true
        );
        assert_eq!(
            contract["compatibility"]["query_deduplicates_by"],
            "diagnostic_event_id"
        );
        assert_eq!(contract["diagnostics_summary"]["sanitized"], true);
        let log_health_fields = contract["diagnostics_summary"]["log_health_fields"]
            .as_array()
            .expect("log health fields");
        assert!(log_health_fields
            .iter()
            .any(|field| field == "write_failures"));
        assert!(log_health_fields
            .iter()
            .any(|field| field == "legacy_compat_write_failures"));
        let query = query_tool_usage_for_profile(
            &format!("diagnostics-contract-parity-{}", std::process::id()),
            &json!({ "scope": "all", "exclude_tools": [] }),
        )
        .expect("diagnostics query should succeed");
        let mut actual_query_keys = query
            .as_object()
            .expect("diagnostics query should be an object")
            .keys()
            .cloned()
            .collect::<Vec<_>>();
        let mut expected_query_keys = contract["query_response"]["fields"]
            .as_array()
            .expect("diagnostics query fields")
            .iter()
            .filter_map(Value::as_str)
            .map(str::to_string)
            .collect::<Vec<_>>();
        actual_query_keys.sort();
        expected_query_keys.sort();
        assert_eq!(actual_query_keys, expected_query_keys);
        assert!(query["log_bytes_read"].as_u64().is_some());
        let actual_log_health = crate::mcp::tool_usage_log_health();
        let mut actual_log_health_keys = actual_log_health
            .as_object()
            .expect("log health should be an object")
            .keys()
            .cloned()
            .collect::<Vec<_>>();
        let mut expected_log_health_keys = log_health_fields
            .iter()
            .filter_map(Value::as_str)
            .map(str::to_string)
            .collect::<Vec<_>>();
        actual_log_health_keys.sort();
        expected_log_health_keys.sort();
        assert_eq!(actual_log_health_keys, expected_log_health_keys);
        assert_eq!(
            contract["structured_log"]["max_bytes"],
            STRUCTURED_LOG_MAX_BYTES
        );
        assert_eq!(
            contract["structured_log"]["retained_files"],
            MAX_ROTATED_FILES
        );
        assert_eq!(
            contract["structured_log"]["queue_capacity"],
            STRUCTURED_LOG_QUEUE_CAPACITY
        );
        let mut stats = ToolStats::default();
        add_stats(
            &json!({
                "tool": "edit_file",
                "outcome": "tool_error",
                "is_error": true,
                "error_code": "E_FAIL",
                "duration_ms": 5
            }),
            &mut stats,
        );
        add_stats(
            &json!({
                "tool": "server_info",
                "outcome": "rpc_error",
                "rpc_error_code": "-32603",
                "duration_ms": 0
            }),
            &mut stats,
        );

        assert_eq!(
            stats.errors, 2,
            "legacy combined error count remains compatible"
        );
        assert_eq!(stats.tool_errors, 1);
        assert_eq!(stats.transport_errors, 1);

        let version = version_scope_stats(&stats);
        assert_eq!(version["errors"], 2);
        assert_eq!(version["tool_errors"], 1);
        assert_eq!(version["transport_errors"], 1);

        let summary = diagnostics_summary_value(
            "all",
            &stats,
            2,
            3,
            &BTreeMap::from([("E_FAIL".to_string(), 1)]),
            &BTreeMap::from([("-32603".to_string(), 1)]),
            4,
            &BTreeMap::from([
                ("tool_call".to_string(), 1),
                ("transport_event".to_string(), 2),
            ]),
            1,
            json!({
                "queue_capacity": STRUCTURED_LOG_QUEUE_CAPACITY,
                "pending_records": Value::Null,
                "pending_dropped_records": 5,
                "write_failures": 1,
                "legacy_compat_write_failures": 0,
                "write_error_present": true,
                "last_successful_write_ts_ms": 100,
                "last_write_failure_ts_ms": 200,
                "writer_state": "ready"
            }),
        );
        assert_eq!(summary["schema_version"], DIAGNOSTICS_SCHEMA_VERSION);
        assert_eq!(summary["host_kind"], DIAGNOSTICS_HOST_KIND);
        assert_eq!(summary["scope"], "all");
        assert_eq!(summary["calls"], 2);
        assert_eq!(summary["tool_errors"], 1);
        assert_eq!(summary["transport_errors"], 2);
        assert_eq!(summary["invalid_complete_lines"], 2);
        assert_eq!(summary["persisted_dropped_records"], 3);
        assert_eq!(summary["recovery"]["repeated_identical_error_count"], 4);
        assert_eq!(summary["top_tool_errors"][0]["code"], "E_FAIL");
        assert_eq!(summary["top_transport_errors"][0]["code"], "-32603");
        assert_eq!(summary["events_by_type"]["tool_call"], 1);
        assert_eq!(summary["events_by_type"]["transport_event"], 2);
        assert_eq!(summary["log_health"]["write_failures"], 1);
        assert_eq!(summary["sanitized"], true);
        let mut actual_summary_keys = summary
            .as_object()
            .expect("diagnostics summary should be an object")
            .keys()
            .cloned()
            .collect::<Vec<_>>();
        let mut expected_summary_keys = contract["diagnostics_summary"]["fields"]
            .as_array()
            .expect("diagnostics summary fields")
            .iter()
            .filter_map(Value::as_str)
            .map(str::to_string)
            .collect::<Vec<_>>();
        actual_summary_keys.sort();
        expected_summary_keys.sort();
        assert_eq!(actual_summary_keys, expected_summary_keys);
        let serialized = serde_json::to_string(&summary).expect("summary JSON");
        assert!(!serialized.contains("arguments"));
        assert!(!serialized.contains("stdout"));
        assert!(!serialized.contains("stderr"));
    }

    #[test]
    fn command_coordination_metrics_are_aggregated() {
        let mut stats = ToolStats::default();
        add_stats(
            &json!({
                "tool": "exec_command",
                "duration_ms": 40,
                "operation_lock_wait_ms": 7,
                "resource_lock_wait_ms": 13,
                "deduplicated": true,
                "heartbeat": true,
                "detached": true
            }),
            &mut stats,
        );

        assert_eq!(stats.operation_lock_wait_ms, 7);
        assert_eq!(stats.resource_lock_wait_ms, 13);
        assert_eq!(stats.deduplicated_calls, 1);
        assert_eq!(stats.heartbeat_responses, 1);
        assert_eq!(stats.detached_responses, 1);
    }

    #[test]
    fn format_metrics_are_aggregated() {
        let mut stats = ToolStats::default();
        add_stats(
            &json!({
                "tool": "format_files",
                "duration_ms": 25,
                "format_files_requested": 6,
                "format_files_supported": 5,
                "format_files_changed_count": 2,
                "format_files_unchanged_count": 3,
                "format_files_skipped_count": 1,
                "format_formatter_group_count": 3,
                "format_custom_formatter_group_count": 1,
                "format_unavailable_adapter_count": 1,
                "format_unexpected_change_count": 0,
                "format_diff_bytes": 512,
                "format_applied": true
            }),
            &mut stats,
        );

        assert_eq!(stats.format_files_requested, 6);
        assert_eq!(stats.format_files_supported, 5);
        assert_eq!(stats.format_files_changed, 2);
        assert_eq!(stats.format_files_unchanged, 3);
        assert_eq!(stats.format_files_skipped, 1);
        assert_eq!(stats.formatter_groups, 3);
        assert_eq!(stats.custom_formatter_groups, 1);
        assert_eq!(stats.unavailable_adapters, 1);
        assert_eq!(stats.unexpected_changes, 0);
        assert_eq!(stats.format_diff_bytes, 512);
        assert_eq!(stats.format_apply_calls, 1);

        let value = formatting_stats(&stats);
        assert_eq!(value["files_requested"], 6);
        assert_eq!(value["files_changed"], 2);
        assert_eq!(value["custom_formatter_groups"], 1);
        assert_eq!(value["apply_calls"], 1);
    }

    #[test]
    fn version_scope_stats_exposes_compact_version_summary() {
        let mut stats = ToolStats::default();
        add_stats(
            &json!({
                "tool": "server_info",
                "duration_ms": 25,
                "request_json_bytes": 10,
                "response_json_bytes": 100,
                "outcome": "tool_error"
            }),
            &mut stats,
        );
        let summary = version_scope_stats(&stats);
        assert_eq!(summary["calls"], 1);
        assert_eq!(summary["duration_ms"], 25);
        assert_eq!(summary["request_bytes"], 10);
        assert_eq!(summary["response_bytes"], 100);
    }

    #[test]
    fn phase_latency_metrics_count_only_instrumented_records() {
        let mut stats = ToolStats::default();
        add_stats(
            &json!({
                "tool": "edit_file",
                "duration_ms": 20,
                "phase_preflight_ms": 4,
                "phase_plan_ms": 2,
                "phase_commit_ms": 6,
                "phase_total_ms": 12
            }),
            &mut stats,
        );
        add_stats(
            &json!({
                "tool": "edit_file",
                "duration_ms": 30,
                "phase_preflight_ms": 8,
                "phase_total_ms": 18
            }),
            &mut stats,
        );
        add_stats(
            &json!({
                "tool": "legacy_edit_file",
                "duration_ms": 10
            }),
            &mut stats,
        );

        let phases = phase_latency_stats(&stats);
        assert_eq!(phases["preflight"]["samples"], 2);
        assert_eq!(phases["preflight"]["total_ms"], 12);
        assert_eq!(phases["preflight"]["avg_ms"], 6.0);
        assert_eq!(phases["preflight"]["p50_ms"], 8);
        assert_eq!(phases["preflight"]["p95_ms"], 8);
        assert_eq!(phases["plan"]["samples"], 1);
        assert_eq!(phases["plan"]["total_ms"], 2);
        assert_eq!(phases["commit"]["samples"], 1);
        assert_eq!(phases["total"]["samples"], 2);
        assert_eq!(phases["total"]["max_ms"], 18);
    }

    #[test]
    fn repeated_failure_report_counts_explicit_retry_chains() {
        let mut stats = RepeatedFailureStats::default();
        let signature = "a".repeat(64);
        for (attempt_count, duration_ms) in [(2, 7), (3, 11)] {
            accumulate_repeated_failure(
                &json!({
                    "tool": "edit_file",
                    "error_code": "EDIT_MATCH_COUNT_MISMATCH",
                    "failure_signature": signature,
                    "repeated_failure": true,
                    "repeat_failure_count": attempt_count,
                    "duration_ms": duration_ms
                }),
                &mut stats,
            );
        }
        accumulate_repeated_failure(
            &json!({
                "tool": "edit_file",
                "failure_signature": "b".repeat(64),
                "repeated_failure": false,
                "repeat_failure_count": 1,
                "duration_ms": 100
            }),
            &mut stats,
        );

        let report = repeated_failure_report(&stats, 4, 20);
        assert_eq!(report["retry_count"], 2);
        assert_eq!(report["chain_count"], 1);
        assert_eq!(report["wasted_duration_ms"], 18);
        assert_eq!(report["max_attempt_count"], 3);
        assert_eq!(report["friction_score"], 72);
        assert_eq!(report["legacy_adjacent_retry_count"], 4);
        assert_eq!(report["top"][0]["signature"], signature);
        assert_eq!(report["top"][0]["retry_count"], 2);
        assert_eq!(report["top"][0]["deterministic_error_weight"], 2);
        assert_eq!(report["top"][0]["friction_score"], 72);
        assert!(report["recovery_hint"]
            .as_str()
            .unwrap()
            .contains("Stop retrying"));
    }

    #[test]
    fn search_stats_report_scan_cost_and_usefulness() {
        let mut stats = ToolStats::default();
        add_stats(
            &json!({
                "tool": "search_text",
                "duration_ms": 5,
                "returned_count": 1,
                "files_considered": 2,
                "scanned_files": 2,
                "matched_files": 2,
                "early_stop_reason": "result_limit",
                "total_matches_exact": false,
                "ok": true
            }),
            &mut stats,
        );
        add_stats(
            &json!({
                "tool": "search_text",
                "duration_ms": 5,
                "returned_count": 0,
                "files_considered": 3,
                "scanned_files": 3,
                "matched_files": 0,
                "total_matches_exact": true,
                "ok": true
            }),
            &mut stats,
        );
        let report = search_stats(&stats);
        assert_eq!(report["files_considered"], 5);
        assert_eq!(report["files_scanned"], 5);
        assert_eq!(report["returned_results"], 1);
        assert_eq!(report["matched_files"], 2);
        assert_eq!(report["zero_result_calls"], 1);
        assert_eq!(report["early_stop_calls"], 1);
        assert_eq!(report["exact_total_calls"], 1);
    }

    #[test]
    fn recovery_chain_report_uses_referenced_call_time_and_selected_action() {
        let mut chains = BTreeMap::<String, RecoveryChainStats>::new();
        let call_completed = BTreeMap::from([(41_u64, 1_010_u64)]);
        accumulate_recovery_chain(
            &json!({
                "event": "tool_call",
                "tool": "read_file",
                "retry_of_call_sequence": 41,
                "recovery_of_operation_id_hash": "a".repeat(64),
                "recovery_action_id": "read_current_file",
                "started_ts_ms": 1_050,
                "completed_ts_ms": 1_055,
                "ok": true,
                "outcome": "success"
            }),
            &call_completed,
            &mut chains,
        );
        let report = recovery_chain_report(&chains, 20);
        assert_eq!(report["chain_count"], 1);
        assert_eq!(report["attempts"], 1);
        assert_eq!(report["successful_chains"], 1);
        assert_eq!(report["failed_chains"], 0);
        assert_eq!(report["top"][0]["elapsed_ms"], 45);
        assert_eq!(report["top"][0]["actions"]["read_current_file"], 1);
    }

    #[test]
    fn performance_report_separates_server_gaps_and_child_lifetimes() {
        let mut performance = PerformanceStats::default();
        accumulate_performance(
            &json!({
                "event": "tool_call",
                "runtime_boot_id": "boot",
                "activity_burst_id": 1,
                "tool": "exec_command",
                "command_kind": "cargo_test",
                "started_ts_ms": 1_000,
                "completed_ts_ms": 1_200,
                "duration_ms": 200,
                "orchestration_gap_ms": null
            }),
            &mut performance,
            120_000,
        );
        accumulate_performance(
            &json!({
                "event": "tool_call",
                "runtime_boot_id": "boot",
                "activity_burst_id": 1,
                "tool": "read_output",
                "started_ts_ms": 3_000,
                "completed_ts_ms": 3_010,
                "duration_ms": 10,
                "orchestration_gap_ms": 1_800
            }),
            &mut performance,
            120_000,
        );
        accumulate_async_session(
            &json!({
                "event": "async_session_finalized",
                "command_kind": "cargo_test",
                "child_process_total_ms": 5_000,
                "first_output_ms": 400
            }),
            &mut performance,
        );

        let report = performance_report(&performance, 20, true, 120_000);
        assert_eq!(report["server_tool_duration_ms"], 210);
        assert_eq!(report["client_orchestration_gap_ms"], 1_800);
        assert_eq!(report["child_process_lifetime_ms"], 5_000);
        assert_eq!(report["first_output_p50_ms"], 400);
        assert_eq!(
            report["dominant_observed_nonidle_source"],
            "client_orchestration_gap"
        );
        assert_eq!(report["command_kinds"][0]["command_kind"], "cargo_test");
        assert_eq!(report["command_kinds"][0]["child_sessions"], 1);
        assert_eq!(report["activity_bursts"][0]["calls"], 2);
        assert_eq!(report["parallelization_opportunity_bursts"], 0);
    }

    #[test]
    fn performance_report_flags_repeated_sequential_exec_commands() {
        let mut performance = PerformanceStats::default();
        for (started, completed) in [(1_000, 1_100), (1_200, 1_300), (1_400, 1_500)] {
            accumulate_performance(
                &json!({
                    "event": "tool_call",
                    "runtime_boot_id": "boot",
                    "activity_burst_id": 7,
                    "tool": "exec_command",
                    "started_ts_ms": started,
                    "completed_ts_ms": completed,
                    "duration_ms": completed - started,
                    "concurrent_request": false
                }),
                &mut performance,
                120_000,
            );
        }
        let report = performance_report(&performance, 20, true, 120_000);
        assert_eq!(report["parallelization_opportunity_bursts"], 1);
        assert_eq!(report["parallelizable_exec_command_candidates"], 3);
        assert_eq!(report["estimated_tool_call_reduction"], 2);
        assert_eq!(
            report["activity_bursts"][0]["parallelization_opportunity"],
            true
        );
        assert_eq!(
            report["activity_bursts"][0]["estimated_tool_call_reduction"],
            2
        );
    }
}
