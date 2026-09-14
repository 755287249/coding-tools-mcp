import path from 'node:path';
import { toolNamesForProfile, toolsetRevisionForProfile } from './catalog.js';
import { allFolderRuntimes } from './folderRuntime.js';
import { documentTitle, historySummary, metadata, parseCheckpointRecords, truncateChars } from './historyMarkdown.js';
import { canonicalPath, DEFAULT_HISTORY_DIR, scanHistory } from './historyStorage.js';
import { LATEST_LEGACY_MCP_PROTOCOL_VERSION } from './mcpTransport.js';
import { processStatus } from './processes.js';
import { redactSensitiveText } from './redaction.js';
import { harnessWorkspaceId } from './taskTools.js';
import type { JsonObject, OperationRecord, ToolContext, WorkspaceFolder } from './types.js';
import { AGENT_VERSION } from './version.js';

const TELEMETRY_RECORD_FIELDS = [
  'event', 'started_ts_ms', 'completed_ts_ms', 'tool', 'tool_family', 'mutating_tool',
  'duration_ms', 'outcome', 'outcome_class', 'is_error', 'error_code', 'error_category',
  'error_retryable', 'warning_count', 'admission_queue_wait_ms', 'blocking_queue_wait_ms',
  'workspace_admission_wait_ms', 'global_admission_wait_ms', 'workspace_lock_wait_ms',
  'history_lock_wait_ms', 'session_registry_wait_ms', 'actual_wait_ms', 'snapshot_ms',
  'resource_lock_wait_ms', 'operation_lock_wait_ms', 'batch_queue_wait_ms',
  'phase_preflight_ms', 'phase_plan_ms', 'phase_commit_ms', 'phase_total_ms',
  'phase_baseline_capture_ms', 'phase_error_enrichment_ms', 'phase_harness_begin_ms',
  'phase_dispatch_ms', 'phase_harness_finish_ms', 'phase_serialization_ms',
  'failure_signature', 'repeat_failure_count', 'repeated_failure', 'retry_without_change',
  'request_json_bytes', 'response_json_bytes', 'response_bytes', 'status',
  'termination_reason', 'exit_code', 'verification_ok', 'process_timed_out',
  'request_timed_out', 'child_process_total_ms', 'first_output_ms', 'stdout_bytes',
  'stderr_bytes', 'command_kind', 'telemetry_dropped_before', 'heartbeat', 'deduplicated',
  'returned_count', 'total_matches', 'total_matches_exact', 'calculate_total',
  'matched_files', 'files_considered', 'scanned_files', 'scan_completed', 'early_stop_reason',
  'transaction_stage', 'selected_path_count', 'staged_path_count_before', 'staged_path_count',
  'index_clean_before', 'staged_by_tool', 'index_restored',
  'error_transaction_stage', 'error_selected_path_count', 'error_staged_path_count_before',
  'error_staged_path_count', 'error_index_clean_before', 'error_staged_by_tool', 'error_index_restored',
  'detached', 'concurrent_request'
] as const;

const TELEMETRY_SCOPES = new Set(['current_runtime', 'current_version', 'all']);
const TELEMETRY_SORTS = new Set([
  'calls', 'errors', 'duration_ms', 'p95_ms', 'response_bytes', 'request_bytes', 'queue_wait_ms'
]);

export class ManagementObservabilityError extends Error {
  constructor(
    readonly status: number,
    readonly code: string,
    message: string
  ) {
    super(message);
    this.name = 'ManagementObservabilityError';
  }
}

function object(value: unknown): JsonObject {
  return value && typeof value === 'object' && !Array.isArray(value) ? value as JsonObject : {};
}

function integer(value: string | null, fallback: number, minimum: number, maximum: number): number {
  if (value === null || value.trim() === '') return fallback;
  const parsed = Number(value);
  if (!Number.isInteger(parsed) || parsed < minimum || parsed > maximum) {
    throw new ManagementObservabilityError(400, 'INVALID_QUERY', `Expected an integer from ${minimum} to ${maximum}.`);
  }
  return parsed;
}

function configuredFolder(ctx: ToolContext, folderId?: string | null): WorkspaceFolder {
  const selected = folderId ? ctx.config.folders.find(folder => folder.id === folderId) : ctx.config.folders[0];
  if (!selected) throw new ManagementObservabilityError(404, 'WORKSPACE_FOLDER_NOT_FOUND', 'Workspace folder not found.');
  return selected;
}

function safeTelemetryRecord(value: unknown): JsonObject {
  const source = object(value);
  const result: JsonObject = {};
  for (const field of TELEMETRY_RECORD_FIELDS) {
    if (source[field] !== undefined) result[field] = structuredClone(source[field]);
  }
  return result;
}

function safePerformance(value: unknown): JsonObject | null {
  const source = object(value);
  if (!Object.keys(source).length) return null;
  const result = { ...source };
  delete result.activity_bursts;
  return result;
}

function safeParallelism(value: unknown): JsonObject | null {
  const source = object(value);
  if (!Object.keys(source).length) return null;
  const result = { ...source };
  delete result.pairs;
  return result;
}

function telemetryView(queried: JsonObject): JsonObject {
  return {
    ok: true,
    generated_at: Date.now(),
    scope: queried.scope,
    scanned_lines: queried.scanned_lines,
    matched_lines: queried.matched_lines,
    matched_async_session_events: queried.matched_async_session_events,
    invalid_complete_lines: queried.invalid_complete_lines,
    records: Array.isArray(queried.records) ? queried.records.map(safeTelemetryRecord) : [],
    slowest: Array.isArray(queried.slowest) ? queried.slowest.map(safeTelemetryRecord) : [],
    largest: Array.isArray(queried.largest) ? queried.largest.map(safeTelemetryRecord) : [],
    aggregate: queried.aggregate ?? null,
    optimization: queried.optimization ?? null,
    formatting: queried.formatting ?? null,
    search: queried.search ?? null,
    parallelism: safeParallelism(queried.parallelism),
    performance: safePerformance(queried.performance),
    warnings: Array.isArray(queried.warnings) ? queried.warnings.map(String) : []
  };
}

export async function managementTelemetryPayload(ctx: ToolContext, searchParams: URLSearchParams): Promise<JsonObject> {
  const requestedScope = searchParams.get('scope') ?? 'current_runtime';
  if (!TELEMETRY_SCOPES.has(requestedScope)) {
    throw new ManagementObservabilityError(400, 'INVALID_QUERY', 'Unsupported telemetry scope.');
  }
  const requestedSort = searchParams.get('sortBy') ?? 'calls';
  if (!TELEMETRY_SORTS.has(requestedSort)) {
    throw new ManagementObservabilityError(400, 'INVALID_QUERY', 'Unsupported telemetry sort field.');
  }
  const queried = await ctx.usageStore.query({
    scope: requestedScope,
    limit: integer(searchParams.get('limit'), 100, 1, 200),
    top: 50,
    errors_only: searchParams.get('errorsOnly') === 'true',
    min_duration_ms: integer(searchParams.get('minDurationMs'), 0, 0, 86_400_000),
    sort_by: requestedSort,
    include_records: true,
    include_payloads: false,
    include_slowest: true,
    include_largest: true,
    include_performance: true,
    include_bursts: false,
    include_async_sessions: true
  });
  return telemetryView(queried);
}

const OPERATION_LOG_STATUSES = new Set(['all', 'completed', 'failed', 'incomplete']);

function operationTimestamp(value: unknown): number {
  const parsed = Number(value);
  return Number.isFinite(parsed) && parsed >= 0 ? parsed : 0;
}

function escapePattern(value: string): string {
  return value.replace(/[.*+?^${}()|[\]\\]/g, '\\$&');
}

function safeOperationReason(value: unknown, workspaceRoot: string): string | null {
  if (typeof value !== 'string' || !value.trim()) return null;
  const firstLine = value.trim().split(/\r?\n/u, 1)[0]?.trim() ?? '';
  let redacted = redactSensitiveText(firstLine).value;
  const rootVariants = new Set([workspaceRoot, path.resolve(workspaceRoot), workspaceRoot.replaceAll('\\', '/')]);
  for (const root of rootVariants) {
    if (!root) continue;
    redacted = redacted.replace(new RegExp(escapePattern(root), process.platform === 'win32' ? 'gi' : 'g'), '[WORKSPACE]');
  }
  redacted = redacted
    .replace(/\b(?:command(?:\s+failed)?|cmd|program|arguments?)\s*[:=]\s*[^;·]+/giu, match => `${match.split(/[:=]/u, 1)[0]}: [REDACTED]`)
    .replace(/\bspawn\s+[^;·]+/giu, 'spawn [REDACTED]')
    .replace(/\bfile:\/\/\/[^\s"'`;,]+/giu, '[PATH]')
    .replace(/(["'`])(?:[A-Za-z]:[\\/]|\\\\|\/(?!\/))[^"'`]+\1/gu, '$1[PATH]$1')
    .replace(/\b[A-Za-z]:[\\/][^\s"'`;,)]*/gu, '[PATH]')
    .replace(/\\\\[^\\\s"'`;,]+\\[^\s"'`;,)]*/gu, '[PATH]')
    .replace(/(^|[\s(=])\/(?!\/)[^\s"'`;,)]*/gu, '$1[PATH]');
  return truncateChars(redacted, 600);
}

function operationDiagnosticToken(value: unknown): string | null {
  return typeof value === 'string' && value.length <= 128 && /^[A-Za-z0-9._:-]+$/.test(value)
    ? value
    : null;
}

function operationDiagnosticBoolean(value: unknown): boolean | null {
  return typeof value === 'boolean' ? value : null;
}

function operationDiagnosticInteger(value: unknown): number | null {
  return typeof value === 'number' && Number.isSafeInteger(value) ? value : null;
}

function operationDiagnostics(summary: JsonObject): JsonObject {
  const waitMs = {
    blocking: operationDiagnosticInteger(summary.blocking_queue_wait_ms),
    workspaceAdmission: operationDiagnosticInteger(summary.workspace_admission_wait_ms),
    globalAdmission: operationDiagnosticInteger(summary.global_admission_wait_ms),
    admissionQueue: operationDiagnosticInteger(summary.admission_queue_wait_ms),
    workspaceLock: operationDiagnosticInteger(summary.workspace_lock_wait_ms),
    operationLock: operationDiagnosticInteger(summary.operation_lock_wait_ms),
    resourceLock: operationDiagnosticInteger(summary.resource_lock_wait_ms),
    historyLock: operationDiagnosticInteger(summary.history_lock_wait_ms),
    sessionRegistry: operationDiagnosticInteger(summary.session_registry_wait_ms)
  };
  return {
    commandOk: operationDiagnosticBoolean(summary.command_ok),
    transportOk: operationDiagnosticBoolean(summary.transport_ok),
    executionOk: operationDiagnosticBoolean(summary.execution_ok),
    verificationOk: operationDiagnosticBoolean(summary.verification_ok),
    errorCode: operationDiagnosticToken(summary.error_code),
    errorCategory: operationDiagnosticToken(summary.error_category),
    retryable: operationDiagnosticBoolean(summary.retryable),
    runtimeStatus: operationDiagnosticToken(summary.status),
    terminationReason: operationDiagnosticToken(summary.termination_reason),
    executionLane: operationDiagnosticToken(summary.execution_lane),
    outcomeClass: operationDiagnosticToken(summary.outcome_class),
    exitCode: operationDiagnosticInteger(summary.process_exit_code ?? summary.exit_code),
    processTimedOut: operationDiagnosticBoolean(summary.process_timed_out),
    requestTimedOut: operationDiagnosticBoolean(summary.request_timed_out),
    recoverable: operationDiagnosticBoolean(summary.recoverable),
    truncated: operationDiagnosticBoolean(summary.truncated),
    stdoutTruncated: operationDiagnosticBoolean(summary.stdout_truncated),
    stderrTruncated: operationDiagnosticBoolean(summary.stderr_truncated),
    cursorExpired: operationDiagnosticBoolean(summary.cursor_expired),
    postChecksPending: operationDiagnosticBoolean(summary.post_checks_pending),
    detached: operationDiagnosticBoolean(summary.detached),
    deduplicated: operationDiagnosticBoolean(summary.deduplicated),
    elapsedMs: operationDiagnosticInteger(summary.elapsed_ms),
    actualWaitMs: operationDiagnosticInteger(summary.actual_wait_ms),
    firstOutputMs: operationDiagnosticInteger(summary.first_output_ms),
    stdoutBytes: operationDiagnosticInteger(summary.stdout_bytes),
    stderrBytes: operationDiagnosticInteger(summary.stderr_bytes),
    warningCount: operationDiagnosticInteger(summary.warning_count),
    waitMs
  };
}

function operationSummaryFailed(summary: JsonObject): boolean {
  if (summary.ok === false || operationDiagnosticToken(summary.error_code)) return true;
  return ['command_ok', 'transport_ok', 'execution_ok', 'verification_ok']
    .some(field => summary[field] === false);
}

function operationAffectedFileCount(record: OperationRecord): number {
  const summary = object(record.result_summary);
  const summaryFiles = Array.isArray(summary.affected_files) ? summary.affected_files.length : 0;
  return Math.max(record.affected_files.length, summaryFiles);
}

function operationRecordIsTerminal(record: OperationRecord): boolean {
  if (record.kind === 'failed') return true;
  if (record.kind !== 'completed') return false;
  const summary = object(record.result_summary);
  return !['running', 'verifying'].includes(String(summary.status ?? ''));
}

function safeOperationGroup(records: OperationRecord[], workspaceRoot: string): JsonObject {
  const ordered = [...records].sort((left, right) => operationTimestamp(left.created_at) - operationTimestamp(right.created_at));
  const started = ordered.find(record => record.kind === 'started');
  const terminal = [...ordered].reverse().find(operationRecordIsTerminal);
  const latest = terminal ?? ordered.at(-1)!;
  const startedAt = started ? operationTimestamp(started.created_at) : null;
  const finishedAt = terminal ? operationTimestamp(terminal.created_at) : null;
  const resultSummary = object(latest.result_summary);
  const status = !terminal
    ? 'incomplete'
    : terminal.kind === 'failed' || operationSummaryFailed(resultSummary)
      ? 'failed'
      : 'completed';
  const reason = safeOperationReason(latest.reason, workspaceRoot)
    ?? safeOperationReason(object(latest.input_summary).reason, workspaceRoot)
    ?? safeOperationReason(ordered.map(record => record.reason).find(Boolean), workspaceRoot);
  const affectedFileCount = ordered.reduce((maximum, record) => Math.max(maximum, operationAffectedFileCount(record)), 0);
  return {
    id: latest.id,
    tool: latest.tool,
    status,
    startedAt,
    finishedAt,
    durationMs: startedAt !== null && finishedAt !== null ? Math.max(0, finishedAt - startedAt) : null,
    taskTracked: ordered.some(record => Boolean(record.task_id)),
    affectedFileCount,
    diagnostics: operationDiagnostics(resultSummary),
    ...(reason ? { reason } : {}),
    events: ordered.map(record => ({
      kind: record.kind,
      createdAt: operationTimestamp(record.created_at),
      ok: object(record.result_summary).ok === true
    }))
  };
}

export async function managementOperationLogPayload(ctx: ToolContext, searchParams: URLSearchParams): Promise<JsonObject> {
  const folder = configuredFolder(ctx, searchParams.get('folderId'));
  const canonicalWorkspaceId = await harnessWorkspaceId(folder.path);
  const cursor = integer(searchParams.get('cursor'), 0, 0, 5_000);
  const limit = integer(searchParams.get('limit'), 50, 1, 200);
  const requestedStatus = searchParams.get('status') ?? 'all';
  if (!OPERATION_LOG_STATUSES.has(requestedStatus)) {
    throw new ManagementObservabilityError(400, 'INVALID_QUERY', 'Unsupported operation-log status.');
  }
  const requestedTool = searchParams.get('tool')?.trim() ?? '';
  if (requestedTool.length > 128 || (requestedTool && !/^[A-Za-z0-9._-]+$/.test(requestedTool))) {
    throw new ManagementObservabilityError(400, 'INVALID_QUERY', 'Unsupported operation-log tool filter.');
  }

  const byId = new Map<string, OperationRecord[]>();
  for (const record of ctx.state.operations()) {
    if (record.workspace_id !== canonicalWorkspaceId && record.workspace_id !== folder.id) continue;
    const existing = byId.get(record.id) ?? [];
    existing.push(record);
    byId.set(record.id, existing);
  }
  const allOperations = [...byId.values()]
    .map(records => safeOperationGroup(records, folder.path))
    .sort((left, right) => Number(right.finishedAt ?? right.startedAt ?? 0) - Number(left.finishedAt ?? left.startedAt ?? 0));
  const summary = {
    total: allOperations.length,
    completed: allOperations.filter(operation => operation.status === 'completed').length,
    failed: allOperations.filter(operation => operation.status === 'failed').length,
    incomplete: allOperations.filter(operation => operation.status === 'incomplete').length
  };
  const errorsOnly = searchParams.get('errorsOnly') === 'true';
  const filtered = allOperations.filter(operation => {
    if (requestedTool && operation.tool !== requestedTool) return false;
    if (requestedStatus !== 'all' && operation.status !== requestedStatus) return false;
    if (errorsOnly && operation.status !== 'failed' && operation.status !== 'incomplete') return false;
    return true;
  });
  const operations = filtered.slice(cursor, cursor + limit);
  const nextCursor = cursor + operations.length < filtered.length ? cursor + operations.length : null;
  return {
    ok: true,
    generatedAt: Date.now(),
    folder: { id: folder.id, name: folder.name },
    source: 'operation_log',
    cursor,
    limit,
    nextCursor,
    matched: filtered.length,
    summary,
    operations
  };
}

function insidePath(root: string, candidate: string): boolean {
  const relative = path.relative(root, candidate);
  return relative === '' || (!relative.startsWith(`..${path.sep}`) && relative !== '..' && !path.isAbsolute(relative));
}

function filesystemErrorCode(error: unknown): string {
  return error && typeof error === 'object' && 'code' in error ? String((error as { code?: unknown }).code ?? '') : '';
}

async function historyLocation(folder: WorkspaceFolder): Promise<{ root: string; dir: string }> {
  const root = await canonicalPath(folder.path);
  const candidate = path.join(root, DEFAULT_HISTORY_DIR);
  try {
    const dir = await canonicalPath(candidate);
    if (!insidePath(root, dir)) {
      throw new ManagementObservabilityError(403, 'HISTORY_PATH_OUTSIDE_WORKSPACE', 'History directory resolves outside the workspace.');
    }
    return { root, dir };
  } catch (error) {
    if (error instanceof ManagementObservabilityError) throw error;
    if (filesystemErrorCode(error) === 'ENOENT') return { root, dir: candidate };
    throw error;
  }
}

export async function managementHistoryListPayload(ctx: ToolContext, folderId?: string | null): Promise<JsonObject> {
  const folder = configuredFolder(ctx, folderId);
  const location = await historyLocation(folder);
  const report = await scanHistory(location.root, location.dir);
  return {
    ok: true,
    folder: { id: folder.id, name: folder.name },
    sessions: [...report.documents].reverse().map(document => {
      const records = parseCheckpointRecords(document.content);
      return {
        number: document.number,
        title: documentTitle(document.content, document.number),
        status: metadata(document.content, 'Status') ?? 'unknown',
        createdAt: document.created_at ?? null,
        updatedAt: document.updated_at ?? null,
        checkpointCount: records.length,
        summary: truncateChars(historySummary(document.content), 600),
        path: document.path
      };
    }),
    integrity: {
      missingNumbers: report.missing_numbers,
      invalidFiles: report.invalid_files,
      emptyFiles: report.empty_files,
      duplicateSessionKeyCount: report.duplicate_session_keys.length
    }
  };
}

export async function managementHistoryDetailPayload(
  ctx: ToolContext,
  sessionNumber: number,
  folderId?: string | null
): Promise<JsonObject> {
  if (!Number.isSafeInteger(sessionNumber) || sessionNumber < 1) {
    throw new ManagementObservabilityError(400, 'INVALID_HISTORY_SESSION', 'History session number is invalid.');
  }
  const folder = configuredFolder(ctx, folderId);
  const location = await historyLocation(folder);
  const report = await scanHistory(location.root, location.dir);
  const document = report.documents.find(item => item.number === sessionNumber);
  if (!document) throw new ManagementObservabilityError(404, 'HISTORY_SESSION_NOT_FOUND', 'History session not found.');
  return {
    ok: true,
    folder: { id: folder.id, name: folder.name },
    number: document.number,
    title: documentTitle(document.content, document.number),
    status: metadata(document.content, 'Status') ?? 'unknown',
    createdAt: document.created_at ?? null,
    updatedAt: document.updated_at ?? null,
    path: document.path,
    records: parseCheckpointRecords(document.content).map(record => ({
      turnId: record.turn_id,
      timestamp: record.timestamp,
      userIntent: record.user_intent,
      findings: record.findings,
      decisions: record.decisions,
      filesChanged: record.files_changed,
      tests: record.tests,
      runtimeState: record.runtime_state,
      remainingIssues: record.remaining_issues,
      nextActions: record.next_actions,
      notes: record.notes
    })),
    content: document.content.replace(/^\*\*Session key:\*\*.*$/m, '**Session key:** [REDACTED]')
  };
}

interface HealthItem extends JsonObject {
  id: string;
  label: string;
  ok: boolean;
  required: boolean;
  detail: string;
  hint?: string;
  status?: number;
  durationMs?: number;
}

interface HealthPayloadValidation {
  ok: boolean;
  detail: string;
}

function stringArray(value: unknown): string[] | undefined {
  if (!Array.isArray(value) || value.some(item => typeof item !== 'string')) return undefined;
  return value as string[];
}

export function validateManagementHealthPayload(pathname: string, value: unknown): HealthPayloadValidation {
  const payload = object(value);
  if (!Object.keys(payload).length) return { ok: false, detail: 'Response is not a JSON object.' };

  if (pathname === '/health') {
    const ok = payload.ok === true
      && payload.server === 'coding-tools-mcp-node'
      && typeof payload.version === 'string'
      && payload.version.length > 0;
    return {
      ok,
      detail: ok
        ? `Agent ${payload.version} · ${String(payload.toolProfile ?? 'unknown profile')}`
        : 'Expected a healthy coding-tools-mcp-node response with a version.'
    };
  }

  if (pathname === '/mcp/info') {
    const protocols = stringArray(payload.supportedProtocolVersions);
    const tools = stringArray(payload.tools);
    const ok = payload.name === 'coding-tools-mcp-node'
      && payload.transport === 'streamable-http'
      && typeof payload.version === 'string'
      && Boolean(protocols?.length)
      && Boolean(tools?.length);
    return {
      ok,
      detail: ok
        ? `streamable-http · ${tools!.length} tools · ${protocols!.length} protocol versions`
        : 'MCP discovery is missing its transport, tool catalog, or supported protocol versions.'
    };
  }

  if (pathname === '/.well-known/oauth-authorization-server') {
    const responseTypes = stringArray(payload.response_types_supported);
    const grantTypes = stringArray(payload.grant_types_supported);
    const challengeMethods = stringArray(payload.code_challenge_methods_supported);
    const tokenAuthMethods = stringArray(payload.token_endpoint_auth_methods_supported);
    const ok = typeof payload.issuer === 'string'
      && typeof payload.authorization_endpoint === 'string'
      && typeof payload.token_endpoint === 'string'
      && Boolean(responseTypes?.includes('code'))
      && Boolean(grantTypes?.includes('authorization_code'))
      && Boolean(challengeMethods?.includes('S256'))
      && Boolean(tokenAuthMethods?.length);
    return {
      ok,
      detail: ok
        ? `authorization_code · PKCE S256 · ${tokenAuthMethods!.join('/')}`
        : 'OAuth authorization metadata is missing required authorization-code or PKCE fields.'
    };
  }

  if (pathname === '/.well-known/oauth-protected-resource/mcp') {
    const authorizationServers = stringArray(payload.authorization_servers);
    const bearerMethods = stringArray(payload.bearer_methods_supported);
    const scopes = stringArray(payload.scopes_supported);
    const ok = typeof payload.resource === 'string'
      && payload.resource.endsWith('/mcp')
      && Boolean(authorizationServers?.length)
      && Boolean(bearerMethods?.includes('header'))
      && Boolean(scopes?.includes('mcp'));
    return {
      ok,
      detail: ok
        ? `Authorization header · ${authorizationServers!.length} authorization server${authorizationServers!.length === 1 ? '' : 's'}`
        : 'OAuth protected-resource metadata is missing its resource, issuer, authorization method, or MCP scope.'
    };
  }

  return { ok: false, detail: 'Unsupported health payload contract.' };
}

const MANAGEMENT_HEALTH_PROBE_TIMEOUT_MS = 5_000;

async function fixedProbe(baseUrl: string, pathname: string, label: string): Promise<HealthItem> {
  const controller = new AbortController();
  const timeout = setTimeout(() => controller.abort(), MANAGEMENT_HEALTH_PROBE_TIMEOUT_MS);
  const startedAt = Date.now();
  try {
    const response = await fetch(new URL(pathname, baseUrl), {
      signal: controller.signal,
      cache: 'no-store',
      redirect: 'error',
      headers: { accept: 'application/json' }
    });
    const payload = await response.json().catch(() => null);
    const validation = validateManagementHealthPayload(pathname, payload);
    const ok = response.ok && validation.ok;
    return {
      id: pathname,
      label,
      ok,
      required: true,
      detail: response.ok ? `HTTP ${response.status} · ${validation.detail}` : `HTTP ${response.status}`,
      hint: ok ? undefined : 'Check the local listener, route prefix, and saved OAuth settings.',
      status: response.status,
      durationMs: Date.now() - startedAt
    };
  } catch (error) {
    return {
      id: pathname,
      label,
      ok: false,
      required: true,
      detail: error instanceof Error ? error.message : String(error),
      hint: 'Restart the Agent and verify that the local listener is reachable.',
      durationMs: Date.now() - startedAt
    };
  } finally {
    clearTimeout(timeout);
  }
}

async function mcpAuthenticationProbe(baseUrl: string): Promise<HealthItem> {
  const controller = new AbortController();
  const timeout = setTimeout(() => controller.abort(), MANAGEMENT_HEALTH_PROBE_TIMEOUT_MS);
  const startedAt = Date.now();
  const endpoint = new URL('/mcp', baseUrl);
  try {
    const response = await fetch(endpoint, {
      method: 'POST',
      signal: controller.signal,
      cache: 'no-store',
      redirect: 'error',
      headers: {
        accept: 'application/json, text/event-stream',
        'content-type': 'application/json',
        origin: endpoint.origin,
        'mcp-protocol-version': LATEST_LEGACY_MCP_PROTOCOL_VERSION
      },
      body: JSON.stringify({
        jsonrpc: '2.0',
        id: 'management-health',
        method: 'initialize',
        params: {
          protocolVersion: LATEST_LEGACY_MCP_PROTOCOL_VERSION,
          capabilities: {},
          clientInfo: { name: 'coding-tools-management-health', version: AGENT_VERSION }
        }
      })
    });
    const challenge = response.headers.get('www-authenticate') ?? '';
    const ok = response.status === 401
      && /^Bearer\b/i.test(challenge)
      && /resource_metadata=/i.test(challenge);
    return {
      id: '/mcp',
      label: 'MCP OAuth challenge',
      ok,
      required: true,
      detail: ok
        ? 'HTTP 401 · OAuth protected-resource challenge available'
        : `HTTP ${response.status} · expected an OAuth protection challenge`,
      hint: ok ? undefined : 'Verify that the MCP route is enabled and protected by OAuth.',
      status: response.status,
      durationMs: Date.now() - startedAt
    };
  } catch (error) {
    return {
      id: '/mcp',
      label: 'MCP OAuth challenge',
      ok: false,
      required: true,
      detail: error instanceof Error ? error.message : String(error),
      hint: 'Restart the Agent and verify that the local MCP listener is reachable.',
      durationMs: Date.now() - startedAt
    };
  } finally {
    clearTimeout(timeout);
  }
}

export async function managementHealthPayload(ctx: ToolContext, localBaseUrl: string): Promise<JsonObject> {
  const probes = await Promise.all([
    fixedProbe(localBaseUrl, '/health', 'Local Agent health'),
    fixedProbe(localBaseUrl, '/mcp/info', 'MCP discovery'),
    fixedProbe(localBaseUrl, '/.well-known/oauth-authorization-server', 'OAuth authorization metadata'),
    fixedProbe(localBaseUrl, '/.well-known/oauth-protected-resource/mcp', 'OAuth protected-resource metadata'),
    mcpAuthenticationProbe(localBaseUrl)
  ]);
  const oauthConfigured: HealthItem = {
    id: 'oauth-configuration',
    label: 'OAuth configuration',
    ok: Boolean(ctx.config.oauth.clientId && ctx.config.oauth.password && ctx.config.oauth.tokenSecret),
    required: true,
    detail: ctx.config.oauth.clientId ? `Client ID ${ctx.config.oauth.clientId} is configured.` : 'OAuth Client ID is missing.',
    hint: ctx.config.oauth.clientId ? undefined : 'Set an OAuth Client ID in workspace settings.'
  };
  const tunnel = ctx.tunnelStatus;
  const tunnelConfigured = Boolean(ctx.config.tunnel?.enabled);
  const tunnelHealthy = !tunnelConfigured || Boolean(tunnel && tunnel.enabled && tunnel.state === 'running' && tunnel.publicUrl);
  const tunnelItem: HealthItem = {
    id: 'builtin-wss',
    label: 'Built-in WSS tunnel',
    ok: tunnelHealthy,
    required: false,
    detail: tunnelConfigured
      ? tunnel ? `${tunnel.state} · ${tunnel.connectedWorkers}/${tunnel.workers} workers connected` : 'Tunnel is configured but runtime status is unavailable.'
      : 'Built-in WSS is disabled.',
    hint: tunnelHealthy ? undefined : 'Review the Public MCP URL and enrollment settings, then restart the Agent.'
  };
  const items = [...probes, oauthConfigured, tunnelItem];
  return {
    ok: items.filter(item => item.required).every(item => item.ok),
    generatedAt: Date.now(),
    items
  };
}

function taskSummary(ctx: ToolContext): JsonObject {
  const byStatus: Record<string, number> = {};
  for (const task of Object.values(ctx.state.tasks())) byStatus[task.status] = (byStatus[task.status] ?? 0) + 1;
  return { total: Object.values(ctx.state.tasks()).length, byStatus };
}

export async function managementDiagnosticsPayload(ctx: ToolContext, startedAt: number): Promise<JsonObject> {
  const runtimes = allFolderRuntimes(ctx);
  const sessions = runtimes.flatMap(runtime => [...runtime.sessions.values()]);
  const queried = telemetryView(await ctx.usageStore.query({
    scope: 'current_version',
    include_records: false,
    include_slowest: false,
    include_largest: false,
    include_bursts: false,
    top: 50
  }));
  const tunnel = ctx.tunnelStatus;
  const retainedOperations = ctx.state.operations();
  const terminalOperationIds = new Set(retainedOperations
    .filter(operation => operation.kind === 'completed' || operation.kind === 'failed')
    .map(operation => operation.id));
  return {
    schemaVersion: 1,
    generatedAt: Date.now(),
    agent: {
      version: AGENT_VERSION,
      nodeVersion: process.version,
      platform: process.platform,
      arch: process.arch,
      uptimeMs: Math.max(0, Date.now() - startedAt),
      configuredToolProfile: ctx.config.toolProfile,
      activeToolProfile: ctx.config.activeToolProfile,
      toolsetRevision: toolsetRevisionForProfile(ctx.config.activeToolProfile),
      toolCount: toolNamesForProfile(ctx.config.activeToolProfile).length
    },
    workspace: {
      id: ctx.config.workspaceId ?? 'primary',
      name: ctx.config.workspaceName ?? 'Workspace',
      folderCount: ctx.config.folders.length,
      permissionMode: ctx.config.permissionMode,
      policy: {
        allowedCommandCount: ctx.config.policy.allowedCommands.length,
        workspaceLocalEntries: ctx.config.policy.workspaceLocalEntries,
        workspaceScriptExtensions: ctx.config.policy.workspaceScriptExtensions,
        maxPatchBytes: ctx.config.policy.maxPatchBytes
      },
      limits: ctx.config.limits
    },
    runtime: {
      admission: {
        blocking: {
          limit: runtimes.reduce((sum, runtime) => sum + runtime.admission.blocking.limit, 0),
          active: runtimes.reduce((sum, runtime) => sum + runtime.admission.blocking.active, 0),
          queued: runtimes.reduce((sum, runtime) => sum + runtime.admission.blocking.queued, 0)
        },
        process: {
          limit: runtimes.reduce((sum, runtime) => sum + runtime.admission.process.limit, 0),
          active: runtimes.reduce((sum, runtime) => sum + runtime.admission.process.active, 0),
          queued: runtimes.reduce((sum, runtime) => sum + runtime.admission.process.queued, 0)
        }
      },
      sessions: {
        total: sessions.length,
        running: sessions.filter(session => processStatus(session) === 'running').length,
        verifying: sessions.filter(session => processStatus(session) === 'verifying').length,
        finalized: sessions.filter(session => Boolean(session.finalizedAt)).length
      },
      permissions: {
        pending: runtimes.reduce((sum, runtime) => sum + runtime.pendingOperations.size, 0),
        byWorkspace: runtimes.map(runtime => ({ workspaceFolderId: runtime.folderId, pending: runtime.pendingOperations.size }))
      },
      tasks: taskSummary(ctx),
      operations: {
        retained: retainedOperations.length,
        failed: retainedOperations.filter(operation => operation.kind === 'failed').length,
        incomplete: new Set(retainedOperations
          .filter(operation => operation.kind === 'started' && !terminalOperationIds.has(operation.id))
          .map(operation => operation.id)).size
      },
      tunnel: tunnel ? {
        enabled: tunnel.enabled,
        state: tunnel.state,
        workers: tunnel.workers,
        connectedWorkers: tunnel.connectedWorkers,
        connectingWorkers: tunnel.connectingWorkers ?? 0,
        idleWorkers: tunnel.idleWorkers ?? 0,
        busyWorkers: tunnel.busyWorkers ?? 0,
        recycledWorkers: tunnel.recycledWorkers ?? 0,
        completedRequests: tunnel.completedRequests,
        policyRevision: tunnel.policyRevision ?? null,
        lastRequestTimeout: tunnel.lastRequestTimeout ?? null,
        lastRequestTimeoutAt: tunnel.lastRequestTimeoutAt ?? null
      } : { enabled: false, state: 'disabled' }
    },
    telemetry: queried
  };
}
