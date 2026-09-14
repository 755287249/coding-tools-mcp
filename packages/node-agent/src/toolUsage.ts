import { createHash, randomUUID } from 'node:crypto';
import path from 'node:path';
import { isSensitiveKey, redactSensitiveText } from './redaction.js';
import { LATEST_LEGACY_MCP_PROTOCOL_VERSION } from './mcpTransport.js';
import { AGENT_VERSION } from './version.js';
import type { JsonObject } from './types.js';
import { requestMutates, toolUsageFamily } from './toolRuntime.js';
import type {
  AsyncSessionUsageInput,
  DiagnosticEventInput,
  ToolRequestTiming,
  ToolUsageInput,
  ToolUsageStoreContract
} from './toolUsage/contract.js';
import { ToolUsageLogStore } from './toolUsage/logStore.js';

export type {
  AsyncSessionUsageInput,
  DiagnosticEventInput,
  DiagnosticEventType,
  ToolRequestTiming,
  ToolUsageInput,
  ToolUsageStoreContract
} from './toolUsage/contract.js';

export const DIAGNOSTICS_LOG_FILE = 'diagnostics.jsonl';
export const TOOL_USAGE_LOG_FILE = 'mcp-tool-usage.jsonl';
export const DIAGNOSTICS_LEGACY_WRITE_ENV = 'CODING_TOOLS_DIAGNOSTICS_LEGACY_WRITE';
export const TOOL_USAGE_SCHEMA_VERSION = 7;
export const DIAGNOSTICS_SCHEMA_VERSION = 1;
export const DIAGNOSTICS_HOST_KIND = 'node_agent';
export const TOOL_USAGE_LOG_MAX_BYTES = 20 * 1024 * 1024;
export const TOOL_USAGE_RETAINED_FILES = 5;
export const TOOL_USAGE_QUEUE_CAPACITY = 1_024;
export const TOOL_USAGE_RECORD_CURSOR_MAX = 10_000;
export const DEFAULT_BURST_IDLE_MS = 120_000;

const MAX_LOG_STRING_CHARS = 4 * 1024;
const MAX_ARGUMENT_RECORD_BYTES = 4 * 1024;
const MAX_ARGUMENT_PREVIEW_BYTES = 512;
const FAILURE_IDENTITY_FIELDS = [
  'path', 'file_index', 'edit_index', 'start_line', 'end_line',
  'expected_sha256', 'actual_sha256', 'expected_occurrences', 'actual_occurrences',
  'recovery_reason'
] as const;
const PHASE_METRICS = [
  ['preflight', 'phase_preflight_ms'],
  ['plan', 'phase_plan_ms'],
  ['commit', 'phase_commit_ms'],
  ['total', 'phase_total_ms'],
  ['baseline_capture', 'phase_baseline_capture_ms'],
  ['baseline_refresh', 'phase_baseline_refresh_ms'],
  ['error_enrichment', 'phase_error_enrichment_ms'],
  ['harness_begin', 'phase_harness_begin_ms'],
  ['dispatch', 'phase_dispatch_ms'],
  ['harness_finish', 'phase_harness_finish_ms'],
  ['serialization', 'phase_serialization_ms']
] as const;
const REDACTED = '[REDACTED]';
const STRUCTURED_FIELDS = [
  'ok', 'transport_ok', 'execution_ok', 'command_ok', 'verification_ok', 'status',
  'termination_reason', 'exit_code', 'process_exit_code', 'process_still_running',
  'process_timed_out', 'request_timed_out', 'recoverable', 'task_required', 'truncated',
  'stdout_truncated', 'stderr_truncated', 'has_more_output', 'cursor_expired', 'cursor',
  'next_cursor', 'latest_cursor', 'output_mode', 'operation_id', 'session_id',
  'harness_mode', 'execution_mode', 'execution_lane', 'resumed_execution_lane',
  'blocking_queue_wait_ms', 'admission_lane', 'admission_mode', 'admission_limit', 'global_admission_limit',
  'admission_scope', 'workspace_admission_wait_ms', 'global_admission_wait_ms',
  'admission_queue_wait_ms', 'resource_class', 'io_heavy_admission_limit', 'io_heavy_admission_wait_ms',
  'workspace_lock_scope', 'workspace_lock_groups',
  'workspace_lock_wait_ms', 'operation_lock_wait_ms', 'resource_lock_wait_ms',
  'resource_lock_group', 'resource_lock_target', 'session_registry_wait_ms',
  'actual_wait_ms', 'snapshot_ms', 'active_session_limit', 'active_session_slots_available',
  'execution_boundary', 'sandbox_enforced', 'program', 'shell', 'resolved_cwd',
  'child_process', 'interactive', 'stdin_open', 'elapsed_ms', 'first_output_ms',
  'returned_count', 'total_matches', 'total_matches_exact', 'calculate_total', 'matched_files',
  'files_considered', 'scanned_files', 'scan_excluded_worktree_container_count', 'scan_completed', 'early_stop_reason', 'skipped_large_files', 'bytes_read',
  'exact_total_forced_full_scan', 'exact_total_fast_tail', 'exact_total_fast_tail_skipped_match_details',
  'knowledge_canary_id', 'knowledge_canary_implementation', 'knowledge_canary_eligible', 'knowledge_canary_stage',
  'knowledge_canary_selected', 'knowledge_canary_applied', 'knowledge_canary_bucket',
  'total_bytes', 'total_lines', 'total_stream_bytes', 'total_retained_bytes',
  'retained_start_offset', 'requested_offset', 'offset', 'limit', 'clean', 'applied',
  'dry_run', 'proposal_ttl_seconds', 'candidate_start_line', 'candidate_end_line',
  'transaction_stage', 'selected_path_count', 'staged_path_count_before', 'staged_path_count',
  'index_clean_before', 'staged_by_tool', 'index_restored',
  'proposal_apply_format', 'preferred_format', 'preferred_format_reason',
  'replacement_bytes', 'proposed_content_bytes', 'proposed_content_included', 'next_action',
  'failed_command_count', 'skipped_command_count', 'batch_summary', 'mode',
  'requested_mode', 'auto_selected', 'parallel_decision_source', 'parallel_confidence',
  'parallel_history_samples', 'parallel_blocked_pair_count', 'recommended_max_parallel',
  'inferred_lock_group_count', 'parallel_observation_count',
  'parallelism_observation_truncated', 'wait_timeout_ms', 'effective_wait_ms',
  'wait_timed_out', 'wait_completed', 'process_completed', 'terminal',
  'progress_since_last_wait', 'next_wait_ms', 'heartbeat_ms', 'heartbeat',
  'deduplicated', 'coalesced_inflight', 'coalesced_wait_ms',
  'graph_operation_id', 'graph_action', 'graph_status', 'graph_completed', 'graph_execution_ok', 'graph_progress_ok', 'control_ok', 'reattached', 'graph_deduplicated', 'graph_yield_ms', 'graph_wait_ms',
  'cancel_requested', 'cancel_accepted', 'cancelled_session_count', 'forgotten',
  'result_mode', 'results_included', 'result_output_included', 'results_omitted_count',
  'graph_created_ts_ms', 'graph_completed_ts_ms', 'retention_expires_ts_ms', 'retention_remaining_ms',
  'retained_graph_count', 'capacity_evicted_graph_count',
  'completed_command_count', 'running_command_count', 'pending_command_count',
  'attached_to_session_id', 'detached', 'command_fingerprint', 'process_id',
  'process_tree_contained', 'process_tree_control',
  'resolved_by', 'retention_seconds', 'wait_until', 'sensitive_data_redacted',
  'requested_workspace_id', 'resolved_workspace_id', 'workspace_route_source',
  'workspace_route_changed', 'conversation_selection_changed',
  'failure_id', 'retry_of_call_sequence', 'recovery_of_operation_id_hash',
  'recovery_action_id', 'recovery_attempt', 'recovery_succeeded',
  'baseline_refresh_mode', 'baseline_refresh_file_count', 'redaction_count'
] as const;

const ARRAY_COUNTS: ReadonlyArray<readonly [string, string]> = [
  ['warnings', 'warning_count'],
  ['next_actions', 'next_action_count'],
  ['recovery_actions', 'recovery_action_count'],
  ['failed_command_ids', 'failed_command_id_count'],
  ['skipped_command_ids', 'skipped_command_id_count'],
  ['completed_command_ids', 'completed_command_id_count'],
  ['running_command_ids', 'running_command_id_count'],
  ['pending_command_ids', 'pending_command_id_count'],
  ['events', 'event_count'],
  ['affected_files', 'affected_file_count'],
  ['entries', 'entry_count'],
  ['matches', 'match_count'],
  ['commits', 'commit_count'],
  ['files', 'file_count'],
  ['would_create', 'would_create_count'],
  ['would_modify', 'would_modify_count'],
  ['would_delete', 'would_delete_count']
];

function legacyCompatWriteEnabled(value = process.env[DIAGNOSTICS_LEGACY_WRITE_ENV]): boolean {
  const normalized = value?.trim().toLowerCase();
  return !['0', 'false', 'no', 'off'].includes(normalized ?? '');
}

export interface ToolUsageStoreOptions {
  profileId?: string;
  runtimeBootId?: string;
  serverVersion?: string;
  maxBytes?: number;
  retainedFiles?: number;
  queueCapacity?: number;
  redactTelemetry?: boolean;
  legacyWriteEnabled?: boolean;
  now?: () => number;
}

type PhaseName = typeof PHASE_METRICS[number][0];

interface PhaseStats {
  totalMs: number;
  durations: number[];
}

interface ToolStats {
  calls: number;
  errors: number;
  toolErrors: number;
  transportErrors: number;
  warnings: number;
  durationMs: number;
  queueWaitMs: number;
  workspaceAdmissionWaitMs: number;
  globalAdmissionWaitMs: number;
  blockingQueueWaitMs: number;
  workspaceLockWaitMs: number;
  historyLockWaitMs: number;
  sessionRegistryWaitMs: number;
  actualWaitMs: number;
  snapshotMs: number;
  resourceLockWaitMs: number;
  operationLockWaitMs: number;
  batchQueueWaitMs: number;
  queueNonzero: number;
  requestBytes: number;
  responseBytes: number;
  recoveryActions: number;
  failedCommandIds: number;
  skippedCommandIds: number;
  emptyWaitTimeouts: number;
  deduplicatedCalls: number;
  heartbeatResponses: number;
  detachedResponses: number;
  formatFilesRequested: number;
  formatFilesSupported: number;
  formatFilesChanged: number;
  formatFilesUnchanged: number;
  formatFilesSkipped: number;
  formatterGroups: number;
  customFormatterGroups: number;
  unavailableAdapters: number;
  unexpectedChanges: number;
  formatDiffBytes: number;
  formatApplyCalls: number;
  searchFilesConsidered: number;
  searchFilesScanned: number;
  searchReturned: number;
  searchMatchedFiles: number;
  searchZeroResultCalls: number;
  searchEarlyStops: number;
  searchExactTotalCalls: number;
  phaseLatency: Record<PhaseName, PhaseStats>;
  durations: number[];
}

interface CommandKindStats {
  calls: number;
  serverDurationMs: number;
  childSessions: number;
  childProcessMs: number;
}

interface BurstStats {
  calls: number;
  firstStartedTsMs: number;
  lastCompletedTsMs: number;
  serverDurationMs: number;
  orchestrationGapMs: number;
  tools: Map<string, number>;
  sequentialExecCommands: number;
  execManyCalls: number;
}

interface PerformanceStats {
  toolCalls: number;
  asyncSessions: number;
  firstStartedTsMs: number;
  lastCompletedTsMs: number;
  serverDurationMs: number;
  queueWaitMs: number;
  activeOrchestrationGapMs: number;
  idleGapMs: number;
  idleGapCount: number;
  childProcessMs: number;
  orchestrationGaps: number[];
  childDurations: number[];
  firstOutputDurations: number[];
  childFailures: number;
  childTerminations: Map<string, number>;
  commandKinds: Map<string, CommandKindStats>;
  bursts: Map<string, BurstStats>;
  inferredBurstId: number;
  previousCompletedTsMs: number;
}

interface ParallelPairStats {
  attempts: number;
  successes: number;
  conflicts: number;
  serialized: number;
  failures: number;
  notOverlapped: number;
  overlapMs: number;
  lockWaitMs: number;
}

interface ParallelHistory {
  pairs: Map<string, ParallelPairStats>;
  totalObservations: number;
}

interface RepeatedFailureGroup {
  signature: string;
  tool: string;
  errorCode: string;
  retryCount: number;
  chainCount: number;
  wastedDurationMs: number;
  maxAttemptCount: number;
}

interface RepeatedFailureStats {
  retryCount: number;
  chainCount: number;
  wastedDurationMs: number;
  maxAttemptCount: number;
  groups: Map<string, RepeatedFailureGroup>;
}

interface RecoveryChainStats {
  attempts: number;
  successes: number;
  failures: number;
  originCompletedTsMs: number;
  firstStartedTsMs: number;
  lastCompletedTsMs: number;
  actions: Map<string, number>;
}

function isRecord(value: unknown): value is JsonObject {
  return Boolean(value) && typeof value === 'object' && !Array.isArray(value);
}

function numberValue(value: unknown, fallback = 0): number {
  const parsed = Number(value);
  return Number.isFinite(parsed) ? Math.max(0, parsed) : fallback;
}

function integer(value: unknown, fallback: number, minimum: number, maximum: number): number {
  return Math.max(minimum, Math.min(maximum, Math.trunc(numberValue(value, fallback))));
}

function jsonBytes(value: unknown): Buffer {
  try { return Buffer.from(JSON.stringify(value)); }
  catch { return Buffer.from('null'); }
}

function truncateUtf8(value: string, maxBytes: number): string {
  const bytes = Buffer.from(value);
  if (bytes.length <= maxBytes) return value;
  const decoder = new TextDecoder('utf-8', { fatal: true });
  for (let end = maxBytes; end >= Math.max(0, maxBytes - 4); end -= 1) {
    try { return `${decoder.decode(bytes.subarray(0, end))}...[TRUNCATED]`; }
    catch { /* try a complete UTF-8 boundary */ }
  }
  return '...[TRUNCATED]';
}

function sanitizeValue(value: unknown, key?: string, redact = true): unknown {
  if (redact && key && isSensitiveKey(key)) return REDACTED;
  if (Array.isArray(value)) return value.map(item => sanitizeValue(item, undefined, redact));
  if (isRecord(value)) {
    return Object.fromEntries(Object.entries(value).map(([childKey, child]) => [childKey, sanitizeValue(child, childKey, redact)]));
  }
  if (typeof value === 'string') {
    const sanitized = redact ? redactSensitiveText(value).value : value;
    const characters = [...sanitized];
    return characters.length > MAX_LOG_STRING_CHARS
      ? `${characters.slice(0, MAX_LOG_STRING_CHARS).join('')}...[TRUNCATED]`
      : sanitized;
  }
  return value;
}

function commandPreview(args: JsonObject, redact = true): string | undefined {
  if (typeof args.cmd === 'string') return redact ? redactSensitiveText(args.cmd).value : args.cmd;
  if (typeof args.script === 'string') return redact ? redactSensitiveText(args.script).value : args.script;
  if (typeof args.program !== 'string') return undefined;
  const argv = Array.isArray(args.args) ? args.args.map(String) : [];
  const preview = [args.program, ...argv].join(' ');
  return redact ? redactSensitiveText(preview).value : preview;
}

export function classifyCommandText(command: string): string {
  const value = command.toLowerCase();
  if (value.includes('wait-process') || value.includes('get-process cargo') || value.includes('start-sleep')) return 'wait_poll';
  if (value.includes('mcp-tool-usage') || value.includes('mcp-requests.log') || value.includes('query_tool_usage')) return 'log_query';
  if (value.includes('cargo test') || value.includes('test-fast.ps1') || value.includes('test-full.ps1')) return 'cargo_test';
  if (value.includes('cargo check') || value.includes('cargo-local.ps1 check')) return 'cargo_check';
  if (value.includes('cargo build') || value.includes('tauri build') || value.includes('npm run build') || value.includes('pnpm build')) return 'build';
  if (value.includes('rustfmt') || value.includes('cargo fmt')) return 'format';
  if (value.trimStart().startsWith('git ') || value.includes(' git ') || value.includes('git.exe')) return 'git';
  if (value.includes('pytest') || value.includes('python -m pytest')) return 'test';
  if (value.includes('powershell') || value.includes('pwsh')) return 'shell';
  if (value.includes('python')) return 'python';
  return 'process';
}

export function classifyCommandKind(args: JsonObject): string {
  return classifyCommandText(commandPreview(args) ?? JSON.stringify(args));
}

function warningSeverityCounts(result: JsonObject): JsonObject {
  const output = { notice_count: 0, deprecation_count: 0, recoverable_warning_count: 0, security_warning_count: 0, data_loss_warning_count: 0 };
  const warnings = Array.isArray(result.warnings) ? result.warnings.map(String) : [];
  for (const warning of warnings) {
    const lowered = warning.toLowerCase();
    if (lowered.includes('deprecated') || lowered.includes('deprecation')) output.deprecation_count += 1;
    else if (['security', 'sandbox', 'unsafe', 'permission', 'credential'].some(keyword => lowered.includes(keyword))) output.security_warning_count += 1;
    else if (['data loss', 'overwrite', 'delete', 'irreversible'].some(keyword => lowered.includes(keyword))) output.data_loss_warning_count += 1;
    else if (['retry', 'recover', 'temporary', 'retained'].some(keyword => lowered.includes(keyword))) output.recoverable_warning_count += 1;
    else output.notice_count += 1;
  }
  return output;
}

function copyResultMetrics(record: JsonObject, tool: string, result: JsonObject, redact: boolean): void {
  for (const field of STRUCTURED_FIELDS) if (result[field] !== undefined) record[field] = sanitizeValue(result[field], field, redact);
  if (isRecord(result.phase_durations_ms)) {
    for (const [phase] of PHASE_METRICS) {
      const source = `${phase}_ms`;
      if (result.phase_durations_ms[source] !== undefined) record[`phase_${source}`] = numberValue(result.phase_durations_ms[source]);
    }
  }
  if (tool === 'exec_many') {
    if (Array.isArray(result.parallelism_observations)) record.parallelism_observations = sanitizeValue(result.parallelism_observations.slice(0, 128), undefined, redact);
    if (Array.isArray(result.parallel_decision_reasons)) record.parallel_decision_reasons = sanitizeValue(result.parallel_decision_reasons.slice(0, 16), undefined, redact);
  }
  if (tool === 'format_files') {
    const mappings: ReadonlyArray<readonly [string, string]> = [
      ['mode', 'format_mode'], ['scope', 'format_scope'], ['files_requested', 'format_files_requested'],
      ['files_supported', 'format_files_supported'], ['files_changed_count', 'format_files_changed_count'],
      ['files_unchanged_count', 'format_files_unchanged_count'], ['files_skipped_count', 'format_files_skipped_count'],
      ['formatter_group_count', 'format_formatter_group_count'],
      ['custom_formatter_group_count', 'format_custom_formatter_group_count'],
      ['diff_bytes', 'format_diff_bytes'], ['diff_truncated', 'format_diff_truncated'], ['applied', 'format_applied']
    ];
    for (const [source, destination] of mappings) if (result[source] !== undefined) record[destination] = result[source];
    record.format_unavailable_adapter_count = Array.isArray(result.unavailable_adapters) ? result.unavailable_adapters.length : 0;
    record.format_unexpected_change_count = Array.isArray(result.unexpected_changes) ? result.unexpected_changes.length : 0;
  }
  if (isRecord(result.error)) {
    if (result.error.code !== undefined) record.error_code = sanitizeValue(result.error.code, 'code', redact);
    if (result.error.category !== undefined) record.error_category = sanitizeValue(result.error.category, 'category', redact);
    if (result.error.retryable !== undefined) record.error_retryable = result.error.retryable;
    if (isRecord(result.error.details)) {
      for (const field of [
        'stage', 'reason', 'suggestion', 'recommended_tool', 'recommended_format',
        'patch_bytes', 'replacement_bytes', 'path', 'file_index', 'edit_index',
        'start_line', 'end_line', 'actual_sha256', 'expected_sha256',
        'actual_occurrences', 'expected_occurrences', 'recovery_reason',
        'direct_recovery_action_count',
        'transaction_stage', 'selected_path_count', 'staged_path_count_before',
        'staged_path_count', 'index_clean_before', 'staged_by_tool', 'index_restored'
      ]) {
        if (result.error.details[field] !== undefined) record[`error_${field}`] = sanitizeValue(result.error.details[field], field, redact);
      }
      record.recovery_action_count = Array.isArray(result.error.details.recovery_actions) ? result.error.details.recovery_actions.length : numberValue(record.recovery_action_count);
    }
  }
  if (result.duration_ms !== undefined) record.tool_reported_duration_ms = result.duration_ms;
  for (const field of ['suggestion', 'recovery_hint']) if (result[field] !== undefined) record[field] = sanitizeValue(result[field], field, redact);
  if (typeof result.stdout === 'string') record.stdout_bytes = Buffer.byteLength(result.stdout);
  if (typeof result.stderr === 'string') record.stderr_bytes = Buffer.byteLength(result.stderr);
  for (const [source, destination] of ARRAY_COUNTS) if (Array.isArray(result[source])) record[destination] = result[source].length;
  Object.assign(record, warningSeverityCounts(result));
}

function classifyOutcome(record: JsonObject, outcome: string): string {
  const code = String(record.error_code ?? record.rpc_error_code ?? '');
  const category = String(record.error_category ?? '');
  if (record.process_timed_out === true) return 'timeout';
  if (record.request_timed_out === true) return 'wait_timeout';
  if (record.command_ok === false) {
    if (record.verification_ok === false) return 'verification_failure';
    if (Number(record.process_exit_code ?? 0) !== 0) return 'process_failure';
    return 'command_failure';
  }
  if (outcome === 'rpc_error') return 'transport_error';
  if (outcome === 'success') return 'success';
  if (code === 'UNKNOWN_TOOL') return 'catalog_mismatch';
  if (['policy', 'permission', 'security'].includes(category)
    || code.includes('POLICY')
    || code.includes('PERMISSION')
    || code.startsWith('DANGEROUS_OPERATION_')
    || code.startsWith('PROTECTED_')) return 'policy_rejection';
  if (code === 'GIT_REPO_TARGET_MISMATCH' || category === 'workspace_routing' || code.includes('ROUTING')) return 'routing_error';
  if ([
    'EDIT_MATCH_COUNT_MISMATCH',
    'PATCH_CONTEXT_AMBIGUOUS',
    'PATCH_CONTEXT_NOT_FOUND',
    'PATCH_HUNK_COUNT_MISMATCH',
    'NOT_FOUND',
    'NOT_GIT_REPOSITORY',
    'EDIT_PROPOSAL_NOT_FOUND'
  ].includes(code) || category === 'not_found') return 'target_resolution_error';
  if ([
    'FILE_VERSION_MISMATCH',
    'EDIT_EXPECTED_TEXT_MISMATCH',
    'GIT_HEAD_MISMATCH',
    'GIT_INDEX_NOT_CLEAN',
    'BASELINE_STALE',
    'EXPECTED_HEAD_MISMATCH'
  ].includes(code) || category === 'conflict') return 'state_conflict';
  if (code.includes('TIMEOUT')) return 'timeout';
  if (code.includes('CANCEL')) return 'cancelled';
  if (code.includes('BUSY') || code.includes('LIMIT_REACHED')) return 'admission_rejected';
  if (Number(record.process_exit_code ?? 0) !== 0) return 'process_failure';
  if (category === 'validation' || ['INVALID_ARGUMENT', 'EDIT_CONTRACT_INVALID'].includes(code)) return 'caller_argument_error';
  return 'internal_error';
}

function failureSignature(record: JsonObject): string | null {
  if (!isErrorRecord(record)) return null;
  const identity = [
    'tool-failure-v1',
    record.tool ?? null,
    record.arguments_sha256 ?? null,
    record.error_code ?? record.rpc_error_code ?? null,
    record.error_category ?? null,
    ...FAILURE_IDENTITY_FIELDS.map(field => record[`error_${field}`] ?? null)
  ];
  return createHash('sha256').update(JSON.stringify(identity)).digest('hex');
}

function semanticToolArguments(argumentsValue: JsonObject): JsonObject {
  const semantic = { ...argumentsValue };
  delete semantic.retry_of_call_sequence;
  delete semantic.recovery_of_operation_id;
  delete semantic.recovery_action_id;
  return semantic;
}

function buildToolCallRecord(store: ToolUsageStore, input: ToolUsageInput): JsonObject {
  const sanitizedArguments = sanitizeValue(input.arguments, undefined, store.redactTelemetry) as JsonObject;
  const argumentBytes = jsonBytes(sanitizedArguments);
  const sanitizedSemanticArguments = sanitizeValue(
    semanticToolArguments(input.arguments),
    undefined,
    store.redactTelemetry
  ) as JsonObject;
  const semanticArgumentBytes = jsonBytes(sanitizedSemanticArguments);
  const resultBytes = jsonBytes(input.result);
  const outcome = input.result.ok === true ? 'success' : 'tool_error';
  const record: JsonObject = {
    schema_version: TOOL_USAGE_SCHEMA_VERSION,
    diagnostic_schema_version: DIAGNOSTICS_SCHEMA_VERSION,
    event: 'tool_call',
    event_type: 'tool_call',
    host_kind: DIAGNOSTICS_HOST_KIND,
    host_version: store.serverVersion,
    timestamp_ms: input.startedTsMs,
    conversation_operation_id: typeof input.result.harness_operation_id === 'string'
      ? input.result.harness_operation_id
      : null,
    started_ts_ms: input.startedTsMs,
    completed_ts_ms: input.startedTsMs + input.durationMs,
    workspace_id: store.profileId,
    selected_workspace_id: input.workspaceId ?? null,
    runtime_boot_id: store.runtimeBootId,
    server_version: store.serverVersion,
    transport_mode: input.transportMode ?? 'direct',
    protocol_version: input.protocolVersion ?? LATEST_LEGACY_MCP_PROTOCOL_VERSION,
    request_id: input.requestId ?? null,
    call_sequence: store.nextCallSequence(),
    method: input.method ?? 'tools/call',
    tool: input.tool,
    tool_family: toolUsageFamily(input.tool),
    mutating_tool: requestMutates(input.tool, input.arguments),
    deprecated_tool: false,
    rpc_fast_path: input.rpcFastPath === true,
    previous_response_completed_ts_ms: input.requestTiming.previousResponseCompletedTsMs,
    orchestration_gap_ms: input.requestTiming.orchestrationGapMs,
    activity_burst_id: input.requestTiming.activityBurstId,
    activity_burst_sequence: input.requestTiming.activityBurstSequence,
    concurrent_request: input.requestTiming.concurrentRequest,
    orchestration_gap_semantics: 'time from the previous completed tool response to this request being received; includes client, network, platform, and model orchestration',
    duration_ms: input.durationMs,
    outcome,
    request_json_bytes: input.requestJsonBytes ?? argumentBytes.length,
    arguments_json_bytes: argumentBytes.length,
    arguments_sha256: createHash('sha256').update(semanticArgumentBytes).digest('hex'),
    semantic_arguments_json_bytes: semanticArgumentBytes.length,
    arguments_truncated: argumentBytes.length > MAX_ARGUMENT_RECORD_BYTES,
    response_json_bytes: resultBytes.length,
    result_json_bytes: resultBytes.length,
    structured_content_json_bytes: resultBytes.length,
    structured_field_count: Object.keys(input.result).length,
    is_error: input.result.ok === false
  };
  if (argumentBytes.length <= MAX_ARGUMENT_RECORD_BYTES) record.arguments = sanitizedArguments;
  else record.arguments_preview = truncateUtf8(JSON.stringify(sanitizedArguments), MAX_ARGUMENT_PREVIEW_BYTES);
  const keys = Object.keys(sanitizedArguments).sort();
  record.argument_keys = keys;
  record.argument_field_bytes = Object.fromEntries(keys.map(key => [key, jsonBytes(sanitizedArguments[key]).length]));
  if (Number.isSafeInteger(input.arguments.retry_of_call_sequence) && Number(input.arguments.retry_of_call_sequence) > 0) {
    record.retry_of_call_sequence = Number(input.arguments.retry_of_call_sequence);
  }
  if (typeof input.arguments.recovery_of_operation_id === 'string' && input.arguments.recovery_of_operation_id) {
    record.recovery_of_operation_id_hash = createHash('sha256')
      .update(input.arguments.recovery_of_operation_id)
      .digest('hex');
  }
  if (typeof input.arguments.recovery_action_id === 'string' && input.arguments.recovery_action_id) {
    record.recovery_action_id = sanitizeValue(
      input.arguments.recovery_action_id,
      'recovery_action_id',
      store.redactTelemetry
    );
  }
  record.recovery_attempt = record.retry_of_call_sequence !== undefined
    || record.recovery_of_operation_id_hash !== undefined
    || record.recovery_action_id !== undefined;
  const preview = commandPreview(input.arguments, store.redactTelemetry);
  if (preview !== undefined) {
    record.command_kind = classifyCommandText(preview);
    record.command_preview = sanitizeValue(preview, undefined, store.redactTelemetry);
  }
  for (const field of ['reason', 'workdir', 'path', 'output_mode']) if (input.arguments[field] !== undefined) record[`argument_${field}`] = sanitizeValue(input.arguments[field], field, store.redactTelemetry);
  copyResultMetrics(record, input.tool, input.result, store.redactTelemetry);
  record.outcome_class = classifyOutcome(record, outcome);
  record.failure_domain = outcome === 'success' ? 'none' : 'tool';
  record.severity = record.failure_domain === 'none' ? 'info' : 'error';
  return record;
}

function newPhaseLatency(): Record<PhaseName, PhaseStats> {
  return {
    preflight: { totalMs: 0, durations: [] },
    plan: { totalMs: 0, durations: [] },
    commit: { totalMs: 0, durations: [] },
    total: { totalMs: 0, durations: [] },
    baseline_capture: { totalMs: 0, durations: [] },
    baseline_refresh: { totalMs: 0, durations: [] },
    error_enrichment: { totalMs: 0, durations: [] },
    harness_begin: { totalMs: 0, durations: [] },
    dispatch: { totalMs: 0, durations: [] },
    harness_finish: { totalMs: 0, durations: [] },
    serialization: { totalMs: 0, durations: [] }
  };
}

function newToolStats(): ToolStats {
  return {
    calls: 0, errors: 0, toolErrors: 0, transportErrors: 0, warnings: 0, durationMs: 0, queueWaitMs: 0,
    workspaceAdmissionWaitMs: 0, globalAdmissionWaitMs: 0, blockingQueueWaitMs: 0,
    workspaceLockWaitMs: 0, historyLockWaitMs: 0, sessionRegistryWaitMs: 0,
    actualWaitMs: 0, snapshotMs: 0, resourceLockWaitMs: 0, operationLockWaitMs: 0,
    batchQueueWaitMs: 0, queueNonzero: 0, requestBytes: 0, responseBytes: 0,
    recoveryActions: 0, failedCommandIds: 0, skippedCommandIds: 0,
    emptyWaitTimeouts: 0, deduplicatedCalls: 0, heartbeatResponses: 0,
    detachedResponses: 0, formatFilesRequested: 0, formatFilesSupported: 0,
    formatFilesChanged: 0, formatFilesUnchanged: 0, formatFilesSkipped: 0,
    formatterGroups: 0, customFormatterGroups: 0, unavailableAdapters: 0,
    unexpectedChanges: 0, formatDiffBytes: 0, formatApplyCalls: 0,
    searchFilesConsidered: 0, searchFilesScanned: 0, searchReturned: 0,
    searchMatchedFiles: 0, searchZeroResultCalls: 0, searchEarlyStops: 0,
    searchExactTotalCalls: 0,
    phaseLatency: newPhaseLatency(), durations: []
  };
}

function metric(record: JsonObject, name: string): number { return numberValue(record[name]); }

function normalizedOutcome(record: JsonObject): string {
  if (typeof record.outcome === 'string') return record.outcome;
  if (record.is_error === true || record.ok === false) return 'legacy_error';
  if (record.ok === true) return 'success';
  return 'legacy_unknown';
}

function isErrorRecord(record: JsonObject): boolean {
  if (record.is_error === true || record.ok === false) return true;
  return ['rpc_error', 'tool_error', 'worker_failed', 'legacy_error'].includes(normalizedOutcome(record));
}

function isTransportErrorRecord(record: JsonObject): boolean {
  return record.failure_domain === 'transport' || normalizedOutcome(record) === 'rpc_error';
}

function isToolErrorRecord(record: JsonObject): boolean {
  return isErrorRecord(record) && !isTransportErrorRecord(record);
}

function addStats(record: JsonObject, stats: ToolStats): void {
  const duration = metric(record, 'duration_ms');
  const queueWait = metric(record, 'admission_queue_wait_ms') + metric(record, 'blocking_queue_wait_ms');
  stats.calls += 1;
  stats.errors += isErrorRecord(record) ? 1 : 0;
  stats.toolErrors += isToolErrorRecord(record) ? 1 : 0;
  stats.transportErrors += isTransportErrorRecord(record) ? 1 : 0;
  stats.warnings += metric(record, 'warning_count');
  stats.durationMs += duration;
  stats.queueWaitMs += queueWait;
  stats.workspaceAdmissionWaitMs += metric(record, 'workspace_admission_wait_ms');
  stats.globalAdmissionWaitMs += metric(record, 'global_admission_wait_ms');
  stats.blockingQueueWaitMs += metric(record, 'blocking_queue_wait_ms');
  stats.workspaceLockWaitMs += metric(record, 'workspace_lock_wait_ms');
  stats.historyLockWaitMs += metric(record, 'history_lock_wait_ms');
  stats.sessionRegistryWaitMs += metric(record, 'session_registry_wait_ms');
  stats.actualWaitMs += metric(record, 'actual_wait_ms');
  stats.snapshotMs += metric(record, 'snapshot_ms');
  stats.resourceLockWaitMs += metric(record, 'resource_lock_wait_ms');
  stats.operationLockWaitMs += metric(record, 'operation_lock_wait_ms');
  stats.batchQueueWaitMs += metric(record, 'batch_queue_wait_ms');
  stats.queueNonzero += queueWait > 0 ? 1 : 0;
  stats.requestBytes += metric(record, 'request_json_bytes');
  stats.responseBytes += metric(record, 'response_json_bytes');
  stats.recoveryActions += metric(record, 'recovery_action_count');
  stats.failedCommandIds += metric(record, 'failed_command_id_count');
  stats.skippedCommandIds += metric(record, 'skipped_command_id_count');
  stats.emptyWaitTimeouts += record.tool === 'wait_command' && record.request_timed_out === true && metric(record, 'event_count') === 0 ? 1 : 0;
  stats.deduplicatedCalls += record.deduplicated === true ? 1 : 0;
  stats.heartbeatResponses += record.heartbeat === true ? 1 : 0;
  stats.detachedResponses += record.detached === true ? 1 : 0;
  stats.formatFilesRequested += metric(record, 'format_files_requested');
  stats.formatFilesSupported += metric(record, 'format_files_supported');
  stats.formatFilesChanged += metric(record, 'format_files_changed_count');
  stats.formatFilesUnchanged += metric(record, 'format_files_unchanged_count');
  stats.formatFilesSkipped += metric(record, 'format_files_skipped_count');
  stats.formatterGroups += metric(record, 'format_formatter_group_count');
  stats.customFormatterGroups += metric(record, 'format_custom_formatter_group_count');
  stats.unavailableAdapters += metric(record, 'format_unavailable_adapter_count');
  stats.unexpectedChanges += metric(record, 'format_unexpected_change_count');
  stats.formatDiffBytes += metric(record, 'format_diff_bytes');
  stats.formatApplyCalls += record.format_applied === true ? 1 : 0;
  if (record.tool === 'search_text') {
    const returned = metric(record, 'returned_count');
    stats.searchFilesConsidered += metric(record, 'files_considered');
    stats.searchFilesScanned += metric(record, 'scanned_files');
    stats.searchReturned += returned;
    stats.searchMatchedFiles += metric(record, 'matched_files');
    stats.searchZeroResultCalls += returned === 0 ? 1 : 0;
    stats.searchEarlyStops += record.early_stop_reason === 'result_limit' ? 1 : 0;
    stats.searchExactTotalCalls += record.total_matches_exact === true ? 1 : 0;
  }
  for (const [phase, field] of PHASE_METRICS) {
    if (record[field] === undefined || record[field] === null) continue;
    const phaseDuration = numberValue(record[field]);
    stats.phaseLatency[phase].totalMs += phaseDuration;
    stats.phaseLatency[phase].durations.push(phaseDuration);
  }
  stats.durations.push(duration);
}

function percentile(values: number[], requested: number): number {
  if (!values.length) return 0;
  const sorted = [...values].sort((left, right) => left - right);
  const index = Math.ceil(((sorted.length - 1) * requested) / 100);
  return sorted[Math.min(sorted.length - 1, Math.max(0, index))];
}

function average(total: number, count: number): number { return count ? total / count : 0; }
function percentage(value: number, total: number): number { return total ? value * 100 / total : 0; }
function round3(value: number): number { return Math.round(value * 1000) / 1000; }
function mapObject<T>(map: Map<string, T>): Record<string, T> { return Object.fromEntries([...map.entries()].sort(([left], [right]) => left.localeCompare(right))); }

function diagnosticEventType(record: JsonObject, legacyEvent: string): string {
  if (typeof record.event_type === 'string' && record.event_type) return record.event_type;
  if (legacyEvent === 'async_session_finalized') return 'process_session';
  if (legacyEvent === 'tool_call' && isTransportErrorRecord(record)) return 'transport_event';
  return legacyEvent === 'tool_call' ? 'tool_call' : legacyEvent;
}

function topErrorCounts(counts: Map<string, number>, limit = 5): JsonObject[] {
  return [...counts.entries()]
    .sort(([leftCode, leftCount], [rightCode, rightCount]) => rightCount - leftCount || leftCode.localeCompare(rightCode))
    .slice(0, limit)
    .map(([code, count]) => ({ code, count }));
}

function diagnosticsSummaryValue(
  hostKind: string,
  hostVersion: string,
  scope: string,
  stats: ToolStats,
  invalidCompleteLines: number,
  persistedDroppedRecords: number,
  toolErrorCounts: Map<string, number>,
  transportErrorCounts: Map<string, number>,
  repeatedIdenticalErrorCount: number,
  eventsByType: Map<string, number>,
  standaloneTransportErrors: number,
  logHealth: JsonObject
): JsonObject {
  return {
    schema_version: DIAGNOSTICS_SCHEMA_VERSION,
    host_kind: hostKind,
    host_version: hostVersion,
    scope,
    calls: stats.calls,
    tool_errors: stats.toolErrors,
    transport_errors: stats.transportErrors + standaloneTransportErrors,
    warnings: stats.warnings,
    invalid_complete_lines: invalidCompleteLines,
    persisted_dropped_records: persistedDroppedRecords,
    top_tool_errors: topErrorCounts(toolErrorCounts),
    top_transport_errors: topErrorCounts(transportErrorCounts),
    latency: {
      p50_ms: percentile(stats.durations, 50),
      p95_ms: percentile(stats.durations, 95),
      max_ms: stats.durations.length ? Math.max(...stats.durations) : 0
    },
    wait: {
      actual_wait_ms: stats.actualWaitMs,
      empty_wait_timeouts: stats.emptyWaitTimeouts
    },
    recovery: { repeated_identical_error_count: repeatedIdenticalErrorCount },
    events_by_type: mapObject(eventsByType),
    log_health: logHealth,
    sanitized: true
  };
}

function formattingStats(stats: ToolStats): JsonObject {
  return {
    files_requested: stats.formatFilesRequested,
    files_supported: stats.formatFilesSupported,
    files_changed: stats.formatFilesChanged,
    files_unchanged: stats.formatFilesUnchanged,
    files_skipped: stats.formatFilesSkipped,
    formatter_groups: stats.formatterGroups,
    custom_formatter_groups: stats.customFormatterGroups,
    unavailable_adapters: stats.unavailableAdapters,
    unexpected_changes: stats.unexpectedChanges,
    diff_bytes: stats.formatDiffBytes,
    apply_calls: stats.formatApplyCalls
  };
}

function searchStats(stats: ToolStats): JsonObject {
  return {
    files_considered: stats.searchFilesConsidered,
    files_scanned: stats.searchFilesScanned,
    returned_results: stats.searchReturned,
    matched_files: stats.searchMatchedFiles,
    zero_result_calls: stats.searchZeroResultCalls,
    early_stop_calls: stats.searchEarlyStops,
    exact_total_calls: stats.searchExactTotalCalls
  };
}

function phaseLatencyStats(stats: ToolStats): JsonObject {
  return Object.fromEntries(PHASE_METRICS.map(([phase]) => {
    const phaseStats = stats.phaseLatency[phase];
    const samples = phaseStats.durations.length;
    return [phase, {
      samples,
      total_ms: phaseStats.totalMs,
      avg_ms: average(phaseStats.totalMs, samples),
      p50_ms: percentile(phaseStats.durations, 50),
      p95_ms: percentile(phaseStats.durations, 95),
      max_ms: samples ? Math.max(...phaseStats.durations) : 0
    }];
  }));
}

function statsRecord(tool: string, stats: ToolStats): JsonObject {
  return {
    tool, calls: stats.calls, errors: stats.errors,
    tool_errors: stats.toolErrors, transport_errors: stats.transportErrors,
    warnings: stats.warnings,
    duration_ms: stats.durationMs, queue_wait_ms: stats.queueWaitMs,
    workspace_admission_wait_ms: stats.workspaceAdmissionWaitMs,
    global_admission_wait_ms: stats.globalAdmissionWaitMs,
    blocking_queue_wait_ms: stats.blockingQueueWaitMs,
    workspace_lock_wait_ms: stats.workspaceLockWaitMs,
    history_lock_wait_ms: stats.historyLockWaitMs,
    session_registry_wait_ms: stats.sessionRegistryWaitMs,
    actual_wait_ms: stats.actualWaitMs, snapshot_ms: stats.snapshotMs,
    resource_lock_wait_ms: stats.resourceLockWaitMs,
    operation_lock_wait_ms: stats.operationLockWaitMs,
    batch_queue_wait_ms: stats.batchQueueWaitMs, queue_nonzero: stats.queueNonzero,
    avg_ms: average(stats.durationMs, stats.calls), p50_ms: percentile(stats.durations, 50),
    p95_ms: percentile(stats.durations, 95), max_ms: stats.durations.length ? Math.max(...stats.durations) : 0,
    request_bytes: stats.requestBytes, response_bytes: stats.responseBytes,
    phase_latency: phaseLatencyStats(stats),
    optimization: {
      recovery_actions: stats.recoveryActions, failed_command_ids: stats.failedCommandIds,
      skipped_command_ids: stats.skippedCommandIds, empty_wait_timeouts: stats.emptyWaitTimeouts,
      deduplicated_calls: stats.deduplicatedCalls, heartbeat_responses: stats.heartbeatResponses,
      detached_responses: stats.detachedResponses
    },
    formatting: formattingStats(stats),
    search: searchStats(stats)
  };
}

function versionScopeStats(stats: ToolStats): JsonObject {
  return {
    calls: stats.calls,
    errors: stats.errors,
    tool_errors: stats.toolErrors,
    transport_errors: stats.transportErrors,
    warnings: stats.warnings,
    duration_ms: stats.durationMs,
    avg_ms: average(stats.durationMs, stats.calls),
    p50_ms: percentile(stats.durations, 50),
    p95_ms: percentile(stats.durations, 95),
    max_ms: stats.durations.length ? Math.max(...stats.durations) : 0,
    request_bytes: stats.requestBytes,
    response_bytes: stats.responseBytes
  };
}

function newRecoveryChainStats(): RecoveryChainStats {
  return {
    attempts: 0,
    successes: 0,
    failures: 0,
    originCompletedTsMs: 0,
    firstStartedTsMs: 0,
    lastCompletedTsMs: 0,
    actions: new Map()
  };
}

function recoveryChainKey(record: JsonObject): string | undefined {
  if (typeof record.recovery_of_operation_id_hash === 'string' && record.recovery_of_operation_id_hash) {
    return `operation:${record.recovery_of_operation_id_hash}`;
  }
  const sequence = numberValue(record.retry_of_call_sequence);
  return sequence > 0 ? `call:${Math.trunc(sequence)}` : undefined;
}

function accumulateRecoveryChain(
  record: JsonObject,
  callCompletedTs: Map<number, number>,
  chains: Map<string, RecoveryChainStats>
): void {
  const key = recoveryChainKey(record);
  if (!key) return;
  const chain = chains.get(key) ?? newRecoveryChainStats();
  const started = numberValue(record.started_ts_ms);
  const completed = numberValue(record.completed_ts_ms, started);
  chain.attempts += 1;
  if (isErrorRecord(record)) chain.failures += 1;
  else chain.successes += 1;
  if (chain.firstStartedTsMs === 0 || started < chain.firstStartedTsMs) chain.firstStartedTsMs = started;
  chain.lastCompletedTsMs = Math.max(chain.lastCompletedTsMs, completed);
  const retrySequence = Math.trunc(numberValue(record.retry_of_call_sequence));
  const origin = retrySequence > 0 ? callCompletedTs.get(retrySequence) : undefined;
  if (origin !== undefined && (chain.originCompletedTsMs === 0 || origin < chain.originCompletedTsMs)) {
    chain.originCompletedTsMs = origin;
  }
  if (typeof record.recovery_action_id === 'string' && record.recovery_action_id) {
    chain.actions.set(record.recovery_action_id, (chain.actions.get(record.recovery_action_id) ?? 0) + 1);
  }
  chains.set(key, chain);
}

function recoveryChainReport(chains: Map<string, RecoveryChainStats>, top: number): JsonObject {
  const rows = [...chains.entries()].map(([chain, stats]) => {
    const origin = stats.originCompletedTsMs || stats.firstStartedTsMs;
    return {
      chain,
      attempts: stats.attempts,
      successes: stats.successes,
      failures: stats.failures,
      succeeded: stats.successes > 0,
      elapsed_ms: Math.max(0, stats.lastCompletedTsMs - origin),
      actions: mapObject(stats.actions)
    };
  }).sort((left, right) => right.attempts - left.attempts || left.chain.localeCompare(right.chain)).slice(0, top);
  return {
    chain_count: chains.size,
    attempts: [...chains.values()].reduce((sum, chain) => sum + chain.attempts, 0),
    successful_chains: [...chains.values()].filter(chain => chain.successes > 0).length,
    failed_chains: [...chains.values()].filter(chain => chain.successes === 0).length,
    top: rows
  };
}

function newRepeatedFailureStats(): RepeatedFailureStats {
  return {
    retryCount: 0,
    chainCount: 0,
    wastedDurationMs: 0,
    maxAttemptCount: 0,
    groups: new Map()
  };
}

function accumulateRepeatedFailure(record: JsonObject, stats: RepeatedFailureStats): void {
  if (record.repeated_failure !== true || typeof record.failure_signature !== 'string') return;
  const attemptCount = Math.max(2, Math.trunc(numberValue(record.repeat_failure_count, 2)));
  const duration = metric(record, 'duration_ms');
  const signature = record.failure_signature;
  const group = stats.groups.get(signature) ?? {
    signature,
    tool: typeof record.tool === 'string' ? record.tool : 'unknown',
    errorCode: typeof record.error_code === 'string'
      ? record.error_code
      : typeof record.rpc_error_code === 'string' ? record.rpc_error_code : 'unknown',
    retryCount: 0,
    chainCount: 0,
    wastedDurationMs: 0,
    maxAttemptCount: 0
  };
  stats.retryCount += 1;
  stats.chainCount += attemptCount === 2 ? 1 : 0;
  stats.wastedDurationMs += duration;
  stats.maxAttemptCount = Math.max(stats.maxAttemptCount, attemptCount);
  group.retryCount += 1;
  group.chainCount += attemptCount === 2 ? 1 : 0;
  group.wastedDurationMs += duration;
  group.maxAttemptCount = Math.max(group.maxAttemptCount, attemptCount);
  stats.groups.set(signature, group);
}

function deterministicFailureWeight(errorCode: string): number {
  return /(?:INVALID|MISMATCH|NOT_FOUND|PATH_|POLICY|CONTRACT|EXPECTED_|PROTECTED|PATCH_)/i.test(errorCode) ? 2 : 1;
}

function repeatedFailureFrictionScore(retryCount: number, wastedDurationMs: number, errorCode: string): number {
  return retryCount * Math.max(1, wastedDurationMs) * deterministicFailureWeight(errorCode);
}

function repeatedFailureReport(stats: RepeatedFailureStats, legacyAdjacentRetryCount: number, top: number): JsonObject {
  const scored = [...stats.groups.values()].map(group => ({
    group,
    frictionScore: repeatedFailureFrictionScore(group.retryCount, group.wastedDurationMs, group.errorCode)
  }));
  const frictionScore = scored.reduce((sum, item) => sum + item.frictionScore, 0);
  const groups = scored
    .sort((left, right) => right.frictionScore - left.frictionScore
      || right.group.retryCount - left.group.retryCount
      || right.group.wastedDurationMs - left.group.wastedDurationMs
      || left.group.signature.localeCompare(right.group.signature))
    .slice(0, top)
    .map(({ group, frictionScore: groupFrictionScore }) => ({
      signature: group.signature,
      tool: group.tool,
      error_code: group.errorCode,
      retry_count: group.retryCount,
      chain_count: group.chainCount,
      wasted_duration_ms: group.wastedDurationMs,
      max_attempt_count: group.maxAttemptCount,
      deterministic_error_weight: deterministicFailureWeight(group.errorCode),
      friction_score: groupFrictionScore
    }));
  return {
    retry_count: stats.retryCount,
    chain_count: stats.chainCount,
    wasted_duration_ms: stats.wastedDurationMs,
    max_attempt_count: stats.maxAttemptCount,
    friction_score: frictionScore,
    legacy_adjacent_retry_count: legacyAdjacentRetryCount,
    top: groups,
    recovery_hint: stats.retryCount > 0
      ? 'Stop retrying unchanged arguments. Change the target or guard, or follow the tool recovery_actions before retrying.'
      : null
  };
}

function newPerformanceStats(): PerformanceStats {
  return {
    toolCalls: 0, asyncSessions: 0, firstStartedTsMs: 0, lastCompletedTsMs: 0,
    serverDurationMs: 0, queueWaitMs: 0, activeOrchestrationGapMs: 0,
    idleGapMs: 0, idleGapCount: 0, childProcessMs: 0, orchestrationGaps: [],
    childDurations: [], firstOutputDurations: [], childFailures: 0,
    childTerminations: new Map(), commandKinds: new Map(), bursts: new Map(),
    inferredBurstId: 0, previousCompletedTsMs: 0
  };
}

function commandKindStats(map: Map<string, CommandKindStats>, kind: string): CommandKindStats {
  const current = map.get(kind) ?? { calls: 0, serverDurationMs: 0, childSessions: 0, childProcessMs: 0 };
  map.set(kind, current);
  return current;
}

function burstStats(map: Map<string, BurstStats>, key: string): BurstStats {
  const current = map.get(key) ?? {
    calls: 0, firstStartedTsMs: 0, lastCompletedTsMs: 0, serverDurationMs: 0,
    orchestrationGapMs: 0, tools: new Map(), sequentialExecCommands: 0, execManyCalls: 0
  };
  map.set(key, current);
  return current;
}

function accumulatePerformance(record: JsonObject, performance: PerformanceStats, burstIdleMs: number): void {
  const started = metric(record, 'started_ts_ms');
  const duration = metric(record, 'duration_ms');
  const completed = metric(record, 'completed_ts_ms') || started + duration;
  const queueWait = metric(record, 'admission_queue_wait_ms') + metric(record, 'blocking_queue_wait_ms');
  const concurrent = record.concurrent_request === true;
  const recordedGap = typeof record.orchestration_gap_ms === 'number' ? numberValue(record.orchestration_gap_ms) : undefined;
  const derivedGap = !concurrent && performance.previousCompletedTsMs > 0 ? Math.max(0, started - performance.previousCompletedTsMs) : undefined;
  const gap = concurrent ? undefined : recordedGap ?? derivedGap;
  performance.toolCalls += 1;
  performance.serverDurationMs += duration;
  performance.queueWaitMs += queueWait;
  if (!performance.firstStartedTsMs || started < performance.firstStartedTsMs) performance.firstStartedTsMs = started;
  performance.lastCompletedTsMs = Math.max(performance.lastCompletedTsMs, completed);
  performance.previousCompletedTsMs = Math.max(performance.previousCompletedTsMs, completed);
  if (gap !== undefined) {
    if (gap > burstIdleMs) { performance.idleGapMs += gap; performance.idleGapCount += 1; }
    else { performance.activeOrchestrationGapMs += gap; performance.orchestrationGaps.push(gap); }
  }
  if (!performance.inferredBurstId) performance.inferredBurstId = 1;
  else if (gap !== undefined && gap > burstIdleMs) performance.inferredBurstId += 1;
  const runtime = typeof record.runtime_boot_id === 'string' ? record.runtime_boot_id : 'legacy';
  const burstId = numberValue(record.activity_burst_id, performance.inferredBurstId) || performance.inferredBurstId;
  const burst = burstStats(performance.bursts, `${runtime}:${burstId}`);
  burst.calls += 1;
  if (!burst.firstStartedTsMs || started < burst.firstStartedTsMs) burst.firstStartedTsMs = started;
  burst.lastCompletedTsMs = Math.max(burst.lastCompletedTsMs, completed);
  burst.serverDurationMs += duration;
  if (gap !== undefined && gap <= burstIdleMs) burst.orchestrationGapMs += gap;
  const tool = typeof record.tool === 'string' ? record.tool : 'unknown';
  burst.tools.set(tool, (burst.tools.get(tool) ?? 0) + 1);
  if (tool === 'exec_command' && !concurrent) burst.sequentialExecCommands += 1;
  else if (tool === 'exec_many') burst.execManyCalls += 1;
  if (typeof record.command_kind === 'string') {
    const kind = commandKindStats(performance.commandKinds, record.command_kind);
    kind.calls += 1;
    kind.serverDurationMs += duration;
  }
}

function accumulateAsyncSession(record: JsonObject, performance: PerformanceStats): void {
  const started = metric(record, 'started_ts_ms');
  const completed = metric(record, 'completed_ts_ms') || started;
  const childMs = metric(record, 'child_process_total_ms');
  performance.asyncSessions += 1;
  if (!performance.firstStartedTsMs || started < performance.firstStartedTsMs) performance.firstStartedTsMs = started;
  performance.lastCompletedTsMs = Math.max(performance.lastCompletedTsMs, completed);
  performance.childProcessMs += childMs;
  performance.childDurations.push(childMs);
  if (typeof record.first_output_ms === 'number') performance.firstOutputDurations.push(numberValue(record.first_output_ms));
  if (typeof record.exit_code === 'number' && record.exit_code !== 0) performance.childFailures += 1;
  const termination = typeof record.termination_reason === 'string' ? record.termination_reason : 'unknown';
  performance.childTerminations.set(termination, (performance.childTerminations.get(termination) ?? 0) + 1);
  const kindName = typeof record.command_kind === 'string' ? record.command_kind : 'process';
  const kind = commandKindStats(performance.commandKinds, kindName);
  kind.childSessions += 1;
  kind.childProcessMs += childMs;
}

function performanceReport(performance: PerformanceStats, top: number, includeBursts: boolean, burstIdleMs: number): JsonObject {
  const observedWallMs = Math.max(0, performance.lastCompletedTsMs - performance.firstStartedTsMs);
  const attributed = performance.serverDurationMs + performance.activeOrchestrationGapMs;
  const commandKinds = [...performance.commandKinds.entries()].map(([commandKind, stats]) => ({
    command_kind: commandKind, calls: stats.calls, server_duration_ms: stats.serverDurationMs,
    child_sessions: stats.childSessions, child_process_ms: stats.childProcessMs
  })).sort((left, right) => (right.server_duration_ms + right.child_process_ms) - (left.server_duration_ms + left.child_process_ms)).slice(0, top);
  const opportunities = [...performance.bursts.values()].filter(burst => burst.sequentialExecCommands >= 2 && burst.execManyCalls === 0);
  const activityBursts = includeBursts ? [...performance.bursts.entries()].map(([burstId, burst]) => ({
    burst_id: burstId, calls: burst.calls, started_ts_ms: burst.firstStartedTsMs,
    completed_ts_ms: burst.lastCompletedTsMs, wall_ms: Math.max(0, burst.lastCompletedTsMs - burst.firstStartedTsMs),
    server_duration_ms: burst.serverDurationMs, orchestration_gap_ms: burst.orchestrationGapMs,
    tools: mapObject(burst.tools), sequential_exec_commands: burst.sequentialExecCommands,
    exec_many_calls: burst.execManyCalls,
    parallelization_opportunity: burst.sequentialExecCommands >= 2 && burst.execManyCalls === 0,
    estimated_tool_call_reduction: burst.sequentialExecCommands >= 2 && burst.execManyCalls === 0 ? burst.sequentialExecCommands - 1 : 0
  })).sort((left, right) => right.started_ts_ms - left.started_ts_ms).slice(0, top) : null;
  return {
    tool_calls: performance.toolCalls,
    async_sessions_finalized: performance.asyncSessions,
    observed_wall_ms: observedWallMs,
    server_tool_duration_ms: performance.serverDurationMs,
    server_queue_wait_ms: performance.queueWaitMs,
    client_orchestration_gap_ms: performance.activeOrchestrationGapMs,
    client_orchestration_gap_p50_ms: percentile(performance.orchestrationGaps, 50),
    client_orchestration_gap_p95_ms: percentile(performance.orchestrationGaps, 95),
    idle_gap_ms: performance.idleGapMs,
    idle_gap_count: performance.idleGapCount,
    burst_idle_threshold_ms: burstIdleMs,
    child_process_lifetime_ms: performance.childProcessMs,
    child_process_failures: performance.childFailures,
    child_process_terminations: mapObject(performance.childTerminations),
    child_process_p50_ms: percentile(performance.childDurations, 50),
    child_process_p95_ms: percentile(performance.childDurations, 95),
    first_output_p50_ms: percentile(performance.firstOutputDurations, 50),
    first_output_p95_ms: percentile(performance.firstOutputDurations, 95),
    server_share_of_nonidle_attributed_percent: percentage(performance.serverDurationMs, attributed),
    client_orchestration_share_of_nonidle_attributed_percent: percentage(performance.activeOrchestrationGapMs, attributed),
    dominant_observed_nonidle_source: performance.activeOrchestrationGapMs > performance.serverDurationMs ? 'client_orchestration_gap' : performance.serverDurationMs > 0 ? 'server_tool_execution' : 'insufficient_data',
    parallelization_opportunity_bursts: opportunities.length,
    parallelizable_exec_command_candidates: opportunities.reduce((sum, burst) => sum + burst.sequentialExecCommands, 0),
    estimated_tool_call_reduction: opportunities.reduce((sum, burst) => sum + burst.sequentialExecCommands - 1, 0),
    parallelization_recommendation: opportunities.length
      ? 'Review sequential exec_command calls in the flagged activity bursts and consolidate independent work with exec_many mode=auto.'
      : 'No repeated sequential exec_command burst was detected in the selected scope.',
    command_kinds: commandKinds,
    activity_bursts: activityBursts,
    attribution_note: 'client_orchestration_gap is observed between the previous tool response completing and the next tool request arriving. It includes model reasoning, platform scheduling, connector/network latency, and client-side orchestration; it is not pure LLM inference time. Child-process lifetime may overlap both server duration and orchestration gaps and must not be added as an independent wall-time component.'
  };
}

function newParallelHistory(): ParallelHistory { return { pairs: new Map(), totalObservations: 0 }; }

function applyParallelObservation(history: ParallelHistory, observation: unknown): void {
  if (!isRecord(observation)) return;
  const pair = typeof observation.pair === 'string' ? observation.pair.trim() : '';
  const outcome = typeof observation.outcome === 'string' ? observation.outcome : '';
  if (!pair || pair.length > 320 || !['success', 'conflict', 'serialized', 'failure', 'not_overlapped'].includes(outcome)) return;
  const stats = history.pairs.get(pair) ?? { attempts: 0, successes: 0, conflicts: 0, serialized: 0, failures: 0, notOverlapped: 0, overlapMs: 0, lockWaitMs: 0 };
  stats.attempts += 1;
  stats.overlapMs += numberValue(observation.overlap_ms);
  stats.lockWaitMs += numberValue(observation.lock_wait_ms);
  if (outcome === 'success') stats.successes += 1;
  else if (outcome === 'conflict') stats.conflicts += 1;
  else if (outcome === 'serialized') stats.serialized += 1;
  else if (outcome === 'failure') stats.failures += 1;
  else stats.notOverlapped += 1;
  history.pairs.set(pair, stats);
  history.totalObservations += 1;
}

function accumulateParallelHistory(history: ParallelHistory, record: JsonObject): void {
  if (Array.isArray(record.parallelism_observations)) for (const observation of record.parallelism_observations) applyParallelObservation(history, observation);
}

function ratio(value: number, total: number): number { return total ? value / total : 0; }

function parallelSafetyLowerBound(stats: ParallelPairStats): number {
  const samples = stats.successes + stats.conflicts;
  if (!samples) return 0;
  const z = 1.2815515655446004;
  const n = samples;
  const p = stats.successes / n;
  const z2 = z * z;
  const center = p + z2 / (2 * n);
  const margin = z * Math.sqrt((p * (1 - p) + z2 / (4 * n)) / n);
  return Math.max(0, Math.min(1, (center - margin) / (1 + z2 / n)));
}

function parallelismReport(history: ParallelHistory, top: number): JsonObject {
  const rows = [...history.pairs.entries()].map(([pair, stats]) => {
    const safetySamples = stats.successes + stats.conflicts;
    const lower = parallelSafetyLowerBound(stats);
    return {
      pair, attempts: stats.attempts, successes: stats.successes, conflicts: stats.conflicts,
      serialized: stats.serialized, failures: stats.failures, not_overlapped: stats.notOverlapped,
      safety_samples: safetySamples, success_rate: round3(ratio(stats.successes, safetySamples)),
      conflict_rate: round3(ratio(stats.conflicts, safetySamples)),
      serialization_rate: round3(ratio(stats.serialized, stats.attempts)),
      safety_lower_bound_80: round3(lower), overlap_ms: stats.overlapMs, lock_wait_ms: stats.lockWaitMs,
      confident_safe: safetySamples >= 5 && lower >= 0.7 && stats.conflicts < 2,
      conflict_prone: stats.conflicts >= 2 || (safetySamples >= 3 && ratio(stats.conflicts, safetySamples) >= 0.1),
      serialization_prone: stats.attempts >= 3 && ratio(stats.serialized, stats.attempts) >= 0.6
    };
  }).sort((left, right) => right.conflicts - left.conflicts || right.attempts - left.attempts || left.pair.localeCompare(right.pair)).slice(0, top);
  const all = [...history.pairs.values()];
  const confidentSafe = all.filter(stats => stats.successes + stats.conflicts >= 5 && parallelSafetyLowerBound(stats) >= 0.7 && stats.conflicts < 2).length;
  const conflicts = all.filter(stats => stats.conflicts >= 2 || (stats.successes + stats.conflicts >= 3 && ratio(stats.conflicts, stats.successes + stats.conflicts) >= 0.1)).length;
  const serialized = all.filter(stats => stats.attempts >= 3 && ratio(stats.serialized, stats.attempts) >= 0.6).length;
  const recommendations: string[] = [];
  if (conflicts) recommendations.push(`Keep ${conflicts} conflict-prone command pair(s) sequential or assign an explicit shared lock_group.`);
  if (serialized) recommendations.push(`Reduce max_parallel or separate ${serialized} pair(s) that mostly wait on the same resource lock.`);
  if (confidentSafe) recommendations.push(`The LLM may batch ${confidentSafe} statistically supported pair(s) with exec_many mode=auto.`);
  if (!history.totalObservations) recommendations.push('Collect exec_many observations before allowing evidence-required command pairs to run in parallel.');
  return {
    decision_method: 'hard_rules_plus_wilson_statistics', machine_learning_enabled: false,
    minimum_confident_samples: 5, safe_lower_bound_threshold: 0.7,
    observed_pairs: history.pairs.size, total_observations: history.totalObservations,
    confident_safe_pairs: confidentSafe, conflict_pairs: conflicts,
    serialization_prone_pairs: serialized, pairs: rows, llm_recommendations: recommendations,
    future_model_note: 'Consider a contextual bandit only after stable command signatures, explicit conflict labels, and enough observations per context are available.'
  };
}

function compactRecord(record: JsonObject): JsonObject {
  const result = { ...record };
  for (const field of ['arguments', 'arguments_json', 'argument_field_bytes', 'stdout', 'stderr']) delete result[field];
  return result;
}

function matchesScope(record: JsonObject, scope: string, sinceTsMs: number, runtimeBootId: string, serverVersion: string): boolean {
  const started = metric(record, 'started_ts_ms');
  const scopeMatches = scope === 'all' ? true : scope === 'current_version'
    ? record.server_version === serverVersion
    : record.runtime_boot_id === runtimeBootId;
  return scopeMatches && started >= sinceTsMs;
}

function errorSignature(record: JsonObject): string | undefined {
  if (!isErrorRecord(record)) return undefined;
  if (typeof record.tool !== 'string' || typeof record.arguments_sha256 !== 'string') return undefined;
  return `${record.tool}\u0000${record.arguments_sha256}\u0000${String(record.error_code ?? record.rpc_error_code ?? 'unknown')}`;
}

function sortMetric(record: JsonObject, field: string): number {
  if (field === 'p95_ms') return metric(record, 'p95_ms');
  if (['errors', 'duration_ms', 'response_bytes', 'request_bytes', 'queue_wait_ms'].includes(field)) return metric(record, field);
  return metric(record, 'calls');
}

export class ToolUsageStore implements ToolUsageStoreContract {
  readonly profileId: string;
  readonly runtimeBootId: string;
  readonly serverVersion: string;
  readonly logDir: string;
  readonly diagnosticsLogFile: string;
  readonly logFile: string;
  readonly maxBytes: number;
  readonly retainedFiles: number;
  readonly queueCapacity: number;
  readonly legacyWriteEnabled: boolean;
  redactTelemetry: boolean;
  readonly now: () => number;
  #queue: JsonObject[] = [];
  #drainPromise?: Promise<void>;
  #droppedRecords = 0;
  #lastWriteError?: string;
  #writeFailures = 0;
  #legacyWriteFailures = 0;
  #lastSuccessfulWriteTsMs = 0;
  #lastWriteFailureTsMs = 0;
  #lastCompletedTsMs = 0;
  #burstId = 0;
  #burstSequence = 0;
  #activeRequests = 0;
  #callSequence = 0;
  #failureCountsByBurst = new Map<string, number>();
  #failureBurstId = 0;
  #dashboardCache?: { createdAt: number; value: JsonObject };
  readonly #diagnosticsLogStore: ToolUsageLogStore;
  readonly #logStore: ToolUsageLogStore;

  constructor(dataDir: string, options: ToolUsageStoreOptions = {}) {
    const resolvedDataDir = path.resolve(dataDir);
    const stableDataDir = process.platform === 'win32' ? resolvedDataDir.toLowerCase() : resolvedDataDir;
    this.profileId = options.profileId ?? `node-${createHash('sha256').update(stableDataDir).digest('hex').slice(0, 24)}`;
    this.runtimeBootId = options.runtimeBootId ?? randomUUID();
    this.serverVersion = options.serverVersion ?? AGENT_VERSION;
    this.logDir = path.join(dataDir, 'logs');
    this.diagnosticsLogFile = path.join(this.logDir, DIAGNOSTICS_LOG_FILE);
    this.logFile = path.join(this.logDir, TOOL_USAGE_LOG_FILE);
    this.maxBytes = Math.max(1, Math.trunc(options.maxBytes ?? TOOL_USAGE_LOG_MAX_BYTES));
    this.retainedFiles = Math.max(0, Math.trunc(options.retainedFiles ?? TOOL_USAGE_RETAINED_FILES));
    this.queueCapacity = Math.max(1, Math.trunc(options.queueCapacity ?? TOOL_USAGE_QUEUE_CAPACITY));
    this.legacyWriteEnabled = options.legacyWriteEnabled ?? legacyCompatWriteEnabled();
    this.redactTelemetry = options.redactTelemetry ?? true;
    this.now = options.now ?? Date.now;
    this.#diagnosticsLogStore = new ToolUsageLogStore({
      logDir: this.logDir,
      logFile: this.diagnosticsLogFile,
      maxBytes: this.maxBytes,
      retainedFiles: this.retainedFiles
    });
    this.#logStore = new ToolUsageLogStore({
      logDir: this.logDir,
      logFile: this.logFile,
      maxBytes: this.maxBytes,
      retainedFiles: this.retainedFiles
    });
  }

  nextCallSequence(): number { this.#callSequence += 1; return this.#callSequence; }

  setRedactTelemetry(value: boolean): void {
    this.redactTelemetry = value;
  }

  #annotateRepeatedFailure(record: JsonObject): void {
    const burstId = numberValue(record.activity_burst_id);
    if (burstId !== this.#failureBurstId) {
      this.#failureBurstId = burstId;
      this.#failureCountsByBurst.clear();
    }
    const signature = failureSignature(record);
    if (!signature) {
      if (!isErrorRecord(record)) this.#failureCountsByBurst.clear();
      return;
    }
    const count = (this.#failureCountsByBurst.get(signature) ?? 0) + 1;
    this.#failureCountsByBurst.set(signature, count);
    record.failure_signature = signature;
    record.repeat_failure_count = count;
    record.repeated_failure = count > 1;
    record.retry_without_change = count > 1;
    record.concurrent_duplicate_failure = count > 1 && record.concurrent_request === true;
  }

  beginRequest(startedTsMs = this.now()): ToolRequestTiming {
    const concurrentRequest = this.#activeRequests > 0;
    const previous = this.#lastCompletedTsMs > 0 ? this.#lastCompletedTsMs : null;
    const gap = concurrentRequest || previous === null ? null : Math.max(0, startedTsMs - previous);
    if (!this.#burstId || (gap !== null && gap > DEFAULT_BURST_IDLE_MS)) {
      this.#burstId += 1;
      this.#burstSequence = 0;
    }
    this.#burstSequence += 1;
    this.#activeRequests += 1;
    return {
      previousResponseCompletedTsMs: previous,
      orchestrationGapMs: gap,
      activityBurstId: this.#burstId,
      activityBurstSequence: this.#burstSequence,
      concurrentRequest
    };
  }

  recordToolCall(input: ToolUsageInput): JsonObject {
    const record = buildToolCallRecord(this, input);
    this.#annotateRepeatedFailure(record);
    this.#lastCompletedTsMs = Math.max(this.#lastCompletedTsMs, input.startedTsMs + input.durationMs);
    this.#activeRequests = Math.max(0, this.#activeRequests - 1);
    this.enqueue(record);
    if (record.recovery_attempt === true) {
      const fields: JsonObject = {
        linked_diagnostic_event_id: record.diagnostic_event_id,
        tool: input.tool,
        recovery_succeeded: input.result.ok === true
      };
      for (const field of ['retry_of_call_sequence', 'recovery_of_operation_id_hash', 'recovery_action_id']) {
        if (record[field] !== undefined) fields[field] = record[field];
      }
      this.recordDiagnosticEvent({
        eventType: 'recovery_event',
        event: 'recovery_attempt',
        severity: input.result.ok === true ? 'info' : 'warning',
        failureDomain: input.result.ok === true ? 'none' : 'tool',
        timestampMs: input.startedTsMs + input.durationMs,
        fields
      });
    }
    return record;
  }

  recordAsyncSession(input: AsyncSessionUsageInput): JsonObject {
    const record: JsonObject = {
      schema_version: TOOL_USAGE_SCHEMA_VERSION,
      diagnostic_schema_version: DIAGNOSTICS_SCHEMA_VERSION,
      event: 'async_session_finalized',
      event_type: 'process_session',
      host_kind: DIAGNOSTICS_HOST_KIND,
      host_version: this.serverVersion,
      timestamp_ms: input.startedTsMs,
      conversation_operation_id: null,
      severity: (input.exitCode ?? 0) === 0 ? 'info' : 'error',
      failure_domain: (input.exitCode ?? 0) === 0 ? 'none' : 'process',
      workspace_id: this.profileId,
      runtime_boot_id: this.runtimeBootId,
      server_version: this.serverVersion,
      session_id: input.sessionId,
      command_kind: input.commandKind,
      started_ts_ms: input.startedTsMs,
      completed_ts_ms: input.completedTsMs ?? this.now(),
      child_process_total_ms: input.childProcessTotalMs,
      first_output_ms: input.firstOutputMs ?? null,
      exit_code: input.exitCode ?? null,
      termination_reason: input.terminationReason,
      stdout_bytes: input.stdoutBytes,
      stderr_bytes: input.stderrBytes
    };
    this.enqueue(record);
    return record;
  }

  recordDiagnosticEvent(input: DiagnosticEventInput): JsonObject {
    const timestampMs = input.timestampMs ?? this.now();
    const fields = sanitizeValue(input.fields ?? {}, undefined, this.redactTelemetry) as JsonObject;
    const record: JsonObject = {
      ...fields,
      schema_version: TOOL_USAGE_SCHEMA_VERSION,
      diagnostic_schema_version: DIAGNOSTICS_SCHEMA_VERSION,
      event: input.event,
      event_type: input.eventType,
      host_kind: DIAGNOSTICS_HOST_KIND,
      host_version: this.serverVersion,
      timestamp_ms: timestampMs,
      conversation_operation_id: null,
      severity: input.severity ?? 'info',
      failure_domain: input.failureDomain ?? 'none',
      workspace_id: this.profileId,
      runtime_boot_id: this.runtimeBootId,
      server_version: this.serverVersion,
      started_ts_ms: timestampMs,
      completed_ts_ms: timestampMs
    };
    this.enqueue(record);
    return record;
  }

  enqueue(record: JsonObject): void {
    this.#dashboardCache = undefined;
    if (typeof record.diagnostic_event_id !== 'string' || !record.diagnostic_event_id) {
      record.diagnostic_event_id = randomUUID();
    }
    if (this.#queue.length >= this.queueCapacity) {
      this.#droppedRecords += 1;
      return;
    }
    const queuedRecord = this.#droppedRecords > 0
      ? { ...record, telemetry_dropped_before: this.#droppedRecords }
      : record;
    this.#droppedRecords = 0;
    this.#queue.push(queuedRecord);
    this.startDrain();
  }

  private startDrain(): void {
    if (this.#drainPromise) return;
    this.#drainPromise = this.drainQueue().finally(() => {
      this.#drainPromise = undefined;
      if (this.#queue.length) this.startDrain();
    });
  }

  async flush(): Promise<void> {
    while (this.#drainPromise) await this.#drainPromise;
  }

  private async drainQueue(): Promise<void> {
    while (this.#queue.length) {
      const record = this.#queue.shift();
      if (!record) continue;
      const sanitized = sanitizeValue(record);
      try {
        await this.#diagnosticsLogStore.append(sanitized);
        this.#lastWriteError = undefined;
        this.#lastSuccessfulWriteTsMs = this.now();
      } catch (error) {
        this.#lastWriteError = error instanceof Error ? error.message : String(error);
        this.#writeFailures += 1;
        this.#lastWriteFailureTsMs = this.now();
      }
      if (this.legacyWriteEnabled) {
        try {
          await this.#logStore.append(sanitized);
        } catch {
          this.#legacyWriteFailures += 1;
        }
      }
    }
  }

  async dashboardSummary(): Promise<JsonObject> {
    const now = this.now();
    if (this.#dashboardCache && now - this.#dashboardCache.createdAt <= 5_000) {
      return structuredClone(this.#dashboardCache.value);
    }
    const queried = await this.query({
      scope: 'current_version',
      include_records: false,
      include_slowest: false,
      include_largest: false,
      include_bursts: false,
      top: 20
    });
    const value: JsonObject = {
      enabled: true,
      workspaceId: queried.workspace_id,
      runtimeBootId: queried.runtime_boot_id,
      serverVersion: queried.server_version,
      scannedLines: queried.scanned_lines,
      matchedLines: queried.matched_lines,
      matchedAsyncSessionEvents: queried.matched_async_session_events,
      invalidCompleteLines: queried.invalid_complete_lines,
      aggregate: queried.aggregate,
      optimization: queried.optimization,
      formatting: queried.formatting,
      parallelism: queried.parallelism,
      performance: queried.performance,
      warnings: queried.warnings
    };
    this.#dashboardCache = { createdAt: now, value: structuredClone(value) };
    return value;
  }


  async query(args: JsonObject): Promise<JsonObject> {
    await this.flush();
    const limit = integer(args.limit, 100, 1, 1000);
    const cursor = integer(args.cursor, 0, 0, TOOL_USAGE_RECORD_CURSOR_MAX);
    const top = integer(args.top, 20, 1, 100);
    const scope = typeof args.scope === 'string' ? args.scope : 'current_runtime';
    const sortBy = typeof args.sort_by === 'string' ? args.sort_by : 'calls';
    const includeRecords = args.include_records === true;
    const includePayloads = args.include_payloads === true;
    const aggregateEnabled = args.aggregate !== false;
    const includeSlowest = args.include_slowest === true;
    const includeLargest = args.include_largest === true;
    const includePerformance = args.include_performance !== false;
    const includeBursts = args.include_bursts === true;
    const includeAsyncSessions = args.include_async_sessions !== false;
    const burstIdleMs = integer(args.burst_idle_ms, DEFAULT_BURST_IDLE_MS, 1000, 3_600_000);
    const errorsOnly = args.errors_only === true;
    const minDurationMs = numberValue(args.min_duration_ms);
    const sinceTsMs = numberValue(args.since_ts_ms);
    const tools = new Set(Array.isArray(args.tools) ? args.tools.map(String) : []);
    const excludeTools = new Set(Array.isArray(args.exclude_tools) && args.exclude_tools.length ? args.exclude_tools.map(String) : ['query_tool_usage']);
    const outcomes = new Set(Array.isArray(args.outcomes) ? args.outcomes.map(String) : []);
    const recent: JsonObject[] = [];
    let recentStart = 0;
    const recordWindowLimit = cursor + limit;
    const totals = newToolStats();
    const currentVersionTotals = newToolStats();
    const previousVersionTotals = newToolStats();
    const previousVersions = new Set<string>();
    const byTool = new Map<string, ToolStats>();
    const outcomeCounts = new Map<string, number>();
    const errorCounts = new Map<string, number>();
    const toolErrorCounts = new Map<string, number>();
    const transportErrorCounts = new Map<string, number>();
    const eventsByType = new Map<string, number>();
    const slowest: JsonObject[] = [];
    const largest: JsonObject[] = [];
    const performance = newPerformanceStats();
    const parallelHistory = newParallelHistory();
    const repeatedFailures = newRepeatedFailureStats();
    const recoveryChains = new Map<string, RecoveryChainStats>();
    const callCompletedTs = new Map<number, number>();
    let matchedLines = 0;
    let matchedAsync = 0;
    let repeatedIdenticalErrorCount = 0;
    let standaloneTransportErrors = 0;
    let persistedDroppedRecords = 0;
    let duplicateRecordsIgnored = 0;
    let previousError: string | undefined;
    const seenDiagnosticEventIds = new Set<string>();

    const visitRecord = (record: JsonObject): void => {
      const diagnosticEventId = typeof record.diagnostic_event_id === 'string' ? record.diagnostic_event_id : undefined;
      if (diagnosticEventId) {
        if (seenDiagnosticEventIds.has(diagnosticEventId)) {
          duplicateRecordsIgnored += 1;
          return;
        }
        seenDiagnosticEventIds.add(diagnosticEventId);
      }
      const event = typeof record.event === 'string' ? record.event : 'tool_call';
      if (matchesScope(record, scope, sinceTsMs, this.runtimeBootId, this.serverVersion)) {
        const eventType = diagnosticEventType(record, event);
        eventsByType.set(eventType, (eventsByType.get(eventType) ?? 0) + 1);
        if (eventType === 'transport_event' && event !== 'tool_call'
          && (record.failure_domain === 'transport' || record.severity === 'error')) {
          standaloneTransportErrors += 1;
          const errorCode = typeof record.error_code === 'string'
            ? record.error_code
            : typeof record.rpc_error_code === 'string' ? record.rpc_error_code : undefined;
          if (errorCode) transportErrorCounts.set(errorCode, (transportErrorCounts.get(errorCode) ?? 0) + 1);
        }
      }
      if (event === 'async_session_finalized') {
        const execRequested = !tools.size || tools.has('exec_command') || tools.has('exec_many');
        const execExcluded = excludeTools.has('exec_command') || excludeTools.has('exec_many');
        if (includePerformance && includeAsyncSessions && execRequested && !execExcluded && !errorsOnly && !outcomes.size
          && metric(record, 'child_process_total_ms') >= minDurationMs
          && matchesScope(record, scope, sinceTsMs, this.runtimeBootId, this.serverVersion)) {
          matchedAsync += 1;
          accumulateAsyncSession(record, performance);
        }
        return;
      }
      if (event !== 'tool_call') return;
      const tool = typeof record.tool === 'string' ? record.tool : '';
      const outcome = normalizedOutcome(record);
      const matchesRequestedFilters = (tools.size === 0 || tools.has(tool)) && !excludeTools.has(tool)
        && (outcomes.size === 0 || outcomes.has(outcome))
        && (!errorsOnly || isErrorRecord(record)) && metric(record, 'duration_ms') >= minDurationMs;
      if (scope === 'all' && matchesRequestedFilters
        && matchesScope(record, 'all', sinceTsMs, this.runtimeBootId, this.serverVersion)) {
        const version = typeof record.server_version === 'string' ? record.server_version : 'unknown';
        if (version === this.serverVersion) addStats(record, currentVersionTotals);
        else {
          addStats(record, previousVersionTotals);
          previousVersions.add(version);
        }
      }
      if (!matchesScope(record, scope, sinceTsMs, this.runtimeBootId, this.serverVersion)
        || !matchesRequestedFilters) return;
      matchedLines += 1;
      persistedDroppedRecords += metric(record, 'telemetry_dropped_before');
      accumulateParallelHistory(parallelHistory, record);
      const signature = errorSignature(record);
      if (signature !== undefined && signature === previousError) repeatedIdenticalErrorCount += 1;
      previousError = signature;
      accumulateRepeatedFailure(record, repeatedFailures);
      accumulateRecoveryChain(record, callCompletedTs, recoveryChains);
      const callSequence = Math.trunc(numberValue(record.call_sequence));
      const completedTs = numberValue(record.completed_ts_ms);
      if (callSequence > 0 && completedTs > 0) callCompletedTs.set(callSequence, completedTs);
      if (includePerformance) accumulatePerformance(record, performance, burstIdleMs);
      if (aggregateEnabled) {
        addStats(record, totals);
        const current = byTool.get(tool) ?? newToolStats();
        addStats(record, current);
        byTool.set(tool, current);
        outcomeCounts.set(outcome, (outcomeCounts.get(outcome) ?? 0) + 1);
        const errorCode = typeof record.error_code === 'string' ? record.error_code : typeof record.rpc_error_code === 'string' ? record.rpc_error_code : undefined;
        if (errorCode) {
          errorCounts.set(errorCode, (errorCounts.get(errorCode) ?? 0) + 1);
          const domainCounts = isTransportErrorRecord(record) ? transportErrorCounts : isToolErrorRecord(record) ? toolErrorCounts : undefined;
          if (domainCounts) domainCounts.set(errorCode, (domainCounts.get(errorCode) ?? 0) + 1);
        }
      }
      const compact = compactRecord(record);
      if (includeSlowest) {
        slowest.push(compact);
        slowest.sort((left, right) => metric(right, 'duration_ms') - metric(left, 'duration_ms'));
        slowest.splice(top);
      }
      if (includeLargest) {
        largest.push(compact);
        largest.sort((left, right) => metric(right, 'response_json_bytes') - metric(left, 'response_json_bytes'));
        largest.splice(top);
      }
      if (includeRecords) {
        recent.push(includePayloads ? structuredClone(record) : compact);
        if (recent.length - recentStart > recordWindowLimit) recentStart += 1;
        if (recentStart >= 1024 && recentStart * 2 >= recent.length) {
          recent.splice(0, recentStart);
          recentStart = 0;
        }
      }
    };
    const legacyScan = await this.#logStore.visitCompleteRecords(visitRecord);
    const canonicalScan = await this.#diagnosticsLogStore.visitCompleteRecords(visitRecord);
    const scannedLines = legacyScan.scannedLines + canonicalScan.scannedLines;
    const invalidLines = legacyScan.invalidLines + canonicalScan.invalidLines;
    const logBytesRead = legacyScan.bytesRead + canonicalScan.bytesRead;
    const recordWindow = includeRecords ? recent.slice(recentStart) : [];
    const pageEnd = Math.max(0, recordWindow.length - Math.min(cursor, recordWindow.length));
    const pageStart = Math.max(0, pageEnd - limit);
    const recordPage = includeRecords ? recordWindow.slice(pageStart, pageEnd) : [];
    const nextCursorCandidate = cursor + recordPage.length;
    const hasOlderRecords = includeRecords && matchedLines > nextCursorCandidate;
    const cursorLimitReached = hasOlderRecords && nextCursorCandidate > TOOL_USAGE_RECORD_CURSOR_MAX;
    const nextCursor = hasOlderRecords && !cursorLimitReached ? nextCursorCandidate : null;

    const toolStats = [...byTool.entries()].map(([tool, stats]) => statsRecord(tool, stats));
    toolStats.sort((left, right) => sortMetric(right, sortBy) - sortMetric(left, sortBy) || String(left.tool).localeCompare(String(right.tool)));
    toolStats.splice(top);
    const aggregate = aggregateEnabled ? {
      calls: totals.calls, errors: totals.errors,
      tool_errors: totals.toolErrors, transport_errors: totals.transportErrors,
      warnings: totals.warnings,
      duration_ms: totals.durationMs, queue_wait_ms: totals.queueWaitMs,
      workspace_admission_wait_ms: totals.workspaceAdmissionWaitMs,
      global_admission_wait_ms: totals.globalAdmissionWaitMs,
      blocking_queue_wait_ms: totals.blockingQueueWaitMs,
      workspace_lock_wait_ms: totals.workspaceLockWaitMs,
      history_lock_wait_ms: totals.historyLockWaitMs,
      session_registry_wait_ms: totals.sessionRegistryWaitMs,
      actual_wait_ms: totals.actualWaitMs, snapshot_ms: totals.snapshotMs,
      resource_lock_wait_ms: totals.resourceLockWaitMs,
      operation_lock_wait_ms: totals.operationLockWaitMs,
      batch_queue_wait_ms: totals.batchQueueWaitMs, queue_nonzero: totals.queueNonzero,
      avg_ms: average(totals.durationMs, totals.calls), p50_ms: percentile(totals.durations, 50),
      p95_ms: percentile(totals.durations, 95), max_ms: totals.durations.length ? Math.max(...totals.durations) : 0,
      phase_latency: phaseLatencyStats(totals),
      request_bytes: totals.requestBytes, response_bytes: totals.responseBytes,
      outcomes: mapObject(outcomeCounts), errors_by_code: mapObject(errorCounts),
      tool_errors_by_code: mapObject(toolErrorCounts), transport_errors_by_code: mapObject(transportErrorCounts),
      tools: toolStats
    } : null;
    const diagnosticsSummary = diagnosticsSummaryValue(
      DIAGNOSTICS_HOST_KIND,
      this.serverVersion,
      scope,
      totals,
      invalidLines,
      persistedDroppedRecords,
      toolErrorCounts,
      transportErrorCounts,
      repeatedIdenticalErrorCount,
      eventsByType,
      standaloneTransportErrors,
      {
        queue_capacity: this.queueCapacity,
        pending_records: this.#queue.length,
        pending_dropped_records: this.#droppedRecords,
        write_failures: this.#writeFailures,
        legacy_compat_write_failures: this.#legacyWriteFailures,
        write_error_present: this.#lastWriteError !== undefined,
        last_successful_write_ts_ms: this.#lastSuccessfulWriteTsMs || null,
        last_write_failure_ts_ms: this.#lastWriteFailureTsMs || null,
        writer_state: this.#lastWriteError ? 'degraded' : 'ready'
      }
    );
    return {
      ok: true,
      workspace_id: this.profileId,
      scope,
      runtime_boot_id: this.runtimeBootId,
      server_version: this.serverVersion,
      diagnostics_contract: {
        schema_version: DIAGNOSTICS_SCHEMA_VERSION,
        host_kind: DIAGNOSTICS_HOST_KIND,
        host_version: this.serverVersion,
        structured_log_file: DIAGNOSTICS_LOG_FILE,
        structured_log: {
          file: DIAGNOSTICS_LOG_FILE,
          max_bytes: this.maxBytes,
          retained_files: this.retainedFiles,
          queue_capacity: this.queueCapacity
        },
        compatibility: {
          legacy_file: TOOL_USAGE_LOG_FILE,
          dual_write: this.legacyWriteEnabled,
          dual_write_default: true,
          legacy_write_env: DIAGNOSTICS_LEGACY_WRITE_ENV,
          legacy_read_preserved: true,
          retirement_requires_compatibility_window: true,
          query_deduplicates_by: 'diagnostic_event_id'
        },
        migration: {
          duplicate_records_ignored: duplicateRecordsIgnored,
          legacy_scanned_lines: legacyScan.scannedLines,
          canonical_scanned_lines: canonicalScan.scannedLines
        },
        legacy_errors_include_transport: true
      },
      diagnostics_summary: aggregateEnabled ? diagnosticsSummary : null,
      log_dir: this.logDir,
      scanned_lines: scannedLines,
      matched_lines: matchedLines,
      matched_async_session_events: matchedAsync,
      invalid_complete_lines: invalidLines,
      log_bytes_read: logBytesRead,
      response_profile: includeRecords || includeSlowest || includeLargest || includeBursts ? 'detailed' : 'summary',
      detail_sections: {
        records: includeRecords,
        slowest: includeSlowest,
        largest: includeLargest,
        activity_bursts: includeBursts
      },
      records_pagination: includeRecords ? {
        cursor,
        next_cursor: nextCursor,
        total: matchedLines,
        limit,
        cursor_origin: 'latest',
        page_order: 'oldest_to_newest',
        cursor_limit_reached: cursorLimitReached
      } : null,
      records: recordPage,
      slowest: includeSlowest ? slowest : null,
      largest: includeLargest ? largest : null,
      aggregate,
      scope_breakdown: scope === 'all' ? {
        current_version: { version: this.serverVersion, stats: versionScopeStats(currentVersionTotals) },
        previous_versions: { versions: [...previousVersions].sort(), stats: versionScopeStats(previousVersionTotals) },
        analysis_hint: 'Prioritize current_version for active defects; use previous_versions as regression and fixed-history evidence.'
      } : null,
      optimization: aggregateEnabled ? {
        recovery_actions: totals.recoveryActions, failed_command_ids: totals.failedCommandIds,
        skipped_command_ids: totals.skippedCommandIds, empty_wait_timeouts: totals.emptyWaitTimeouts,
        deduplicated_calls: totals.deduplicatedCalls, heartbeat_responses: totals.heartbeatResponses,
        detached_responses: totals.detachedResponses, repeated_identical_error_count: repeatedIdenticalErrorCount,
        repeated_failures: repeatedFailureReport(repeatedFailures, repeatedIdenticalErrorCount, top),
        recovery_chains: recoveryChainReport(recoveryChains, top)
      } : null,
      formatting: aggregateEnabled ? formattingStats(totals) : null,
      search: aggregateEnabled ? searchStats(totals) : null,
      parallelism: parallelismReport(parallelHistory, top),
      performance: includePerformance ? performanceReport(performance, top, includeBursts, burstIdleMs) : null,
      warnings: this.#lastWriteError ? [`tool usage telemetry write failed: ${this.#lastWriteError}`] : []
    };
  }
}
