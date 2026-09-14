import { processRedactionWarning, REDACTED } from '../redaction.js';
import { startupDiagnosticsJson } from '../processStartup.js';
import type { JsonObject, ProcessOutputEvent, ProcessSession, ToolContext } from '../types.js';

const SESSION_EVENT_BYTES = 1_048_576;

function boundedViewInteger(
  value: unknown,
  fallback: number,
  minimum: number,
  maximum: number
): number {
  const parsed = Number(value);
  return Number.isFinite(parsed)
    ? Math.max(minimum, Math.min(maximum, Math.trunc(parsed)))
    : fallback;
}

interface ProcessViewOptions {
  cursor?: number;
  max_output_bytes?: number;
  output_mode?: string;
  event_detail?: string;
  tail_lines?: number;
  request_timed_out?: boolean;
  deduplicated?: boolean;
  attached_to_session_id?: string | null;
  operation_lock_wait_ms?: number;
}

function safeUtf8Tail(buffer: Buffer, maxBytes: number): Buffer {
  if (buffer.length <= maxBytes) return buffer;
  let start = buffer.length - maxBytes;
  while (start < buffer.length && (buffer[start] & 0xc0) === 0x80) start += 1;
  return buffer.subarray(start);
}

function safeUtf8Prefix(buffer: Buffer): Buffer {
  if (!buffer.length) return buffer;
  const decoder = new TextDecoder('utf-8', { fatal: true });
  for (let trim = 0; trim <= Math.min(3, buffer.length); trim += 1) {
    const candidate = trim === 0 ? buffer : buffer.subarray(0, buffer.length - trim);
    try { decoder.decode(candidate); return candidate; } catch { /* trim incomplete suffix */ }
  }
  return Buffer.alloc(0);
}

function retainOutput(current: string, chunk: Buffer, maxBytes: number): string {
  return safeUtf8Tail(Buffer.concat([Buffer.from(current), chunk]), maxBytes).toString('utf8');
}

export function appendOutput(ctx: ToolContext, session: ProcessSession, stream: 'stdout' | 'stderr', chunk: Buffer): void {
  const offset = stream === 'stdout' ? session.stdoutBytes : session.stderrBytes;
  if (session.firstOutputAt === undefined) session.firstOutputAt = Date.now();
  const text = chunk.toString('utf8');
  session.sequence += 1;
  const outputEvent: ProcessOutputEvent = { sequence: session.sequence, stream, offset, data: text };
  session.outputEvents.push(outputEvent);
  session.outputEventBytes += chunk.length;
  while (session.outputEventBytes > SESSION_EVENT_BYTES && session.outputEvents.length) {
    const removed = session.outputEvents.shift();
    if (removed) session.outputEventBytes = Math.max(0, session.outputEventBytes - Buffer.byteLength(removed.data));
  }
  if (stream === 'stdout') {
    session.stdoutBytes += chunk.length;
    session.stdout = retainOutput(session.stdout, chunk, ctx.config.limits.maxOutputBytes);
    session.stdoutStart = Math.max(0, session.stdoutBytes - Buffer.byteLength(session.stdout));
  } else {
    session.stderrBytes += chunk.length;
    session.stderr = retainOutput(session.stderr, chunk, ctx.config.limits.maxOutputBytes);
    session.stderrStart = Math.max(0, session.stderrBytes - Buffer.byteLength(session.stderr));
  }
  session.events.emit('change');
}

export function terminationReason(session: ProcessSession): string {
  return session.terminationReason ?? (session.endedAt ? 'exited' : 'running');
}

export function retainedNextActions(session: ProcessSession, waitTimeoutMaxMs: number): JsonObject[] {
  if (session.finalizedAt) return [];
  return [{
    tool: 'wait_command',
    arguments: {
      session_id: session.id,
      cursor: session.sequence,
      timeout_ms: waitTimeoutMaxMs,
      until: 'output_or_exit',
      output_mode: 'delta'
    }
  }];
}

function recoverySuggestion(reason: string): string {
  switch (reason) {
    case 'process_timeout': return '读取保留输出，调整 timeout_ms 后重试';
    case 'detached_timeout': return '连接失联超过宽限时间；确认没有可恢复 session 后再使用新的 operation_id 重试';
    case 'killed': return '确认终止原因后重新执行命令';
    case 'exited': return '检查 process_exit_code、stderr 与 post_checks';
    case 'crashed': return '检查 stderr 后重试或恢复工作区';
    default: return '使用 wait_command 等待新输出或进程结束';
  }
}

function recoverable(reason: string): boolean {
  return ['process_timeout', 'killed', 'spawn_failed', 'server_restart', 'detached_timeout'].includes(reason);
}

export function processStatus(session: ProcessSession): 'running' | 'verifying' | 'exited' | 'timed_out' | 'killed' {
  if (!session.endedAt) return 'running';
  if (session.postChecksPending || !session.finalizedAt) return 'verifying';
  const reason = terminationReason(session);
  if (reason === 'process_timeout') return 'timed_out';
  if (reason === 'killed' || reason === 'detached_timeout' || reason === 'server_restart') return 'killed';
  return 'exited';
}

export function capTail(value: string, maxBytes: number): { content: string; truncated: boolean } {
  const buffer = Buffer.from(value);
  if (buffer.length <= maxBytes) return { content: value, truncated: false };
  return { content: safeUtf8Tail(buffer, maxBytes).toString('utf8'), truncated: true };
}

function summarizeStream(value: string, maxBytes: number, tailLineCount: number): { content: string; truncated: boolean } {
  const collapsed: string[] = [];
  for (const line of value.trimEnd().split(/\r?\n/)) {
    if (!line.trim()) continue;
    if (collapsed.at(-1) !== line) collapsed.push(line);
  }
  return capTail(collapsed.slice(-tailLineCount).join('\n'), maxBytes);
}

function outputView(session: ProcessSession, options: ProcessViewOptions): {
  mode: string;
  stdout: string;
  stderr: string;
  events: JsonObject[];
  stdoutTruncated: boolean;
  stderrTruncated: boolean;
  cursorExpired: boolean;
  nextCursor: number;
  hasMore: boolean;
} {
  const requestedMode = String(options.output_mode ?? 'tail');
  const mode = ['delta', 'tail', 'all', 'none', 'summary'].includes(requestedMode) ? requestedMode : 'tail';
  const cursor = boundedViewInteger(options.cursor, 0, 0, Number.MAX_SAFE_INTEGER);
  const maxBytes = boundedViewInteger(options.max_output_bytes, 65_536, 1, 1_048_576);
  const lines = boundedViewInteger(options.tail_lines, 100, 1, 10_000);
  const latestCursor = session.sequence;
  const oldest = session.outputEvents[0]?.sequence;
  const cursorExpired = oldest !== undefined && cursor + 1 < oldest;
  const effectiveCursor = cursorExpired ? oldest! - 1 : cursor;

  if (mode === 'none') {
    return { mode, stdout: '', stderr: '', events: [], stdoutTruncated: false, stderrTruncated: false, cursorExpired: false, nextCursor: latestCursor, hasMore: false };
  }
  if (mode === 'delta') {
    const selected: ProcessOutputEvent[] = [];
    let selectedBytes = 0;
    for (const event of session.outputEvents) {
      if (event.sequence <= effectiveCursor) continue;
      const eventBytes = Buffer.byteLength(event.data);
      if (selected.length && selectedBytes + eventBytes > maxBytes) break;
      selected.push(event);
      selectedBytes += eventBytes;
    }
    const nextCursor = selected.at(-1)?.sequence ?? Math.min(effectiveCursor, latestCursor);
    const eventDetail = String(options.event_detail ?? 'full').trim().toLowerCase() === 'compact' ? 'compact' : 'full';
    const events = selected.map(event => ({
      sequence: event.sequence,
      stream: event.stream,
      stream_offset: event.offset,
      ...(eventDetail === 'full' ? {
        decoded_offset: event.offset,
        offset: event.offset,
        encoding: 'utf-8',
        data: event.data
      } : {})
    }));
    return {
      mode,
      stdout: selected.filter(event => event.stream === 'stdout').map(event => event.data).join(''),
      stderr: selected.filter(event => event.stream === 'stderr').map(event => event.data).join(''),
      events,
      stdoutTruncated: false,
      stderrTruncated: false,
      cursorExpired,
      nextCursor,
      hasMore: nextCursor < latestCursor
    };
  }
  if (mode === 'summary') {
    const stdout = summarizeStream(session.stdout, maxBytes, lines);
    const stderr = summarizeStream(session.stderr, maxBytes, lines);
    return { mode, stdout: stdout.content, stderr: stderr.content, events: [], stdoutTruncated: stdout.truncated, stderrTruncated: stderr.truncated, cursorExpired: false, nextCursor: latestCursor, hasMore: false };
  }
  const stdout = capTail(session.stdout, maxBytes);
  const stderr = capTail(session.stderr, maxBytes);
  return { mode, stdout: stdout.content, stderr: stderr.content, events: [], stdoutTruncated: stdout.truncated, stderrTruncated: stderr.truncated, cursorExpired: false, nextCursor: latestCursor, hasMore: false };
}

export function processResult(session: ProcessSession, options: ProcessViewOptions = {}): JsonObject {
  const view = outputView(session, options);
  const status = processStatus(session);
  const reason = terminationReason(session);
  const verificationOk = session.postChecksPending ? null : (session.verificationOk ?? true);
  const executionOk = session.endedAt ? reason === 'exited' && session.exitCode === 0 : null;
  const commandOk = executionOk === null || verificationOk === null ? null : executionOk && verificationOk;
  const failureOrigin = commandOk !== false
    ? null
    : executionOk === true && verificationOk === false
      ? 'post_check'
      : reason === 'exited'
        ? 'child_process'
        : 'process_lifecycle';
  const diagnosticSummary = failureOrigin === 'post_check'
    ? 'Child process completed, but post-check verification failed. MCP policy did not block execution.'
    : failureOrigin === 'child_process'
      ? `Child process started and exited with code ${session.exitCode ?? 'unknown'}. MCP policy did not block execution.`
      : failureOrigin === 'process_lifecycle'
        ? `Child process started but ended because of ${reason}. MCP policy did not block execution.`
        : null;
  let stdout = view.stdout;
  let stderr = view.stderr;
  let redactionCount = 0;
  const events = view.events.map(event => ({ ...event }));
  if (session.sensitiveOutput) {
    if (stdout) { stdout = REDACTED; redactionCount += 1; }
    if (stderr) { stderr = REDACTED; redactionCount += 1; }
    for (const event of events) {
      if (typeof event.data === 'string' && event.data) {
        event.data = REDACTED;
        redactionCount += 1;
      }
    }
  }
  return {
    // A retained process result means the MCP request and child launch were accepted.
    // Command success is reported independently through command_ok, matching Rust.
    ok: true,
    command_ok: commandOk,
    execution_ok: executionOk,
    verification_ok: verificationOk,
    transport_ok: true,
    failure_origin: failureOrigin,
    policy_blocked: false,
    execution_attempted: true,
    process_started: true,
    ...(diagnosticSummary ? { diagnostic_summary: diagnosticSummary } : {}),
    session_id: session.id,
    startup: startupDiagnosticsJson(session.startupDiagnostics),
    workspace_folder_id: session.folderId,
    workspace_path: session.workspacePath,
    operation_id: session.operationId ?? null,
    command_fingerprint: session.fingerprint,
    command: session.command,
    program: session.program,
    args: session.argv,
    ...(session.testRunnerCapability ? { test_runner_capability: session.testRunnerCapability } : {}),
    ...(session.testWorkflow ? { test_workflow: session.testWorkflow } : {}),
    shell: session.shell ? 'shell' : 'none',
    cwd: session.cwd,
    interactive: session.interactive,
    stdin_open: session.stdinOpen,
    status,
    termination_reason: reason,
    recoverable: recoverable(reason),
    suggestion: recoverySuggestion(reason),
    ...(session.timeoutContract ? {
      execution_mode: session.timeoutContract.executionMode,
      requested_process_timeout_ms: session.timeoutContract.requestedTimeoutMs,
      effective_process_timeout_ms: session.timeoutContract.effectiveTimeoutMs,
      process_timeout_limit_ms: session.timeoutContract.limitMs,
      process_deadline_ts_ms: session.processDeadlineMs,
      process_timeout_remaining_ms: session.endedAt ? 0 : Math.max(0, (session.processDeadlineMs ?? session.startedAt) - Date.now()),
      timeout_clamped: false,
      polling_extends_process_deadline: false,
      timeout_scope: 'process'
    } : {}),
    request_timed_out: options.request_timed_out === true,
    process_timed_out: reason === 'process_timeout',
    process_still_running: !session.endedAt,
    process_id: session.child?.pid ?? session.processId ?? null,
    restart_recoverable: Boolean(session.durableDirectory),
    exit_code: session.exitCode ?? null,
    process_exit_code: session.exitCode ?? null,
    signal: session.signal ?? null,
    output_mode: view.mode,
    event_detail: view.mode === 'delta' ? (String(options.event_detail ?? 'full').trim().toLowerCase() === 'compact' ? 'compact' : 'full') : 'none',
    stdout,
    stderr,
    stdout_bytes: session.stdoutBytes,
    stderr_bytes: session.stderrBytes,
    stdout_retained_from: session.stdoutStart,
    stderr_retained_from: session.stderrStart,
    stdout_truncated: view.stdoutTruncated,
    stderr_truncated: view.stderrTruncated,
    events,
    cursor: boundedViewInteger(options.cursor, 0, 0, Number.MAX_SAFE_INTEGER),
    latest_cursor: session.sequence,
    next_cursor: view.nextCursor,
    cursor_expired: view.cursorExpired,
    has_more_output: view.hasMore,
    elapsed_ms: Math.max(0, Date.now() - session.startedAt),
    first_output_ms: session.firstOutputAt === undefined ? null : Math.max(0, session.firstOutputAt - session.startedAt),
    started_ts_ms: session.startedAt,
    ended_ts_ms: session.endedAt ?? null,
    finalized_ts_ms: session.finalizedAt ?? null,
    killed: session.killed,
    sandbox_enforced: session.sandboxEnforced ?? false,
    sandbox_backend: session.sandboxBackend ?? null,
    execution_boundary: session.executionBoundary ?? 'policy_only',
    sandbox_phase_durations_ms: {
      prepare_ms: session.sandboxPrepareMs ?? null,
      startup_ms: session.sandboxStartupMs ?? null,
      cleanup_ms: session.sandboxCleanupMs ?? null
    },
    process_tree_contained: session.processTreeContained ?? (process.platform !== 'win32'),
    process_tree_control: session.processTreeControl ?? (process.platform === 'win32' ? 'taskkill_tree' : 'process_group'),
    post_checks: session.postChecks,
    post_checks_pending: session.postChecksPending,
    resource_lock_group: session.resourceLockGroup ?? null,
    resource_lock_target: session.resourceLockTarget ?? null,
    operation_lock_wait_ms: options.operation_lock_wait_ms ?? session.operationLockWaitMs,
    resource_lock_wait_ms: session.resourceLockWaitMs,
    deduplicated: options.deduplicated === true,
    attached_to_session_id: options.attached_to_session_id ?? null,
    detached: session.detachedGeneration !== 0,
    sensitive_data_redacted: session.sensitiveOutput,
    redaction_count: redactionCount,
    warnings: session.sensitiveOutput ? [processRedactionWarning()] : [],
    output_refs: {
      stdout: `output://${session.id}/stdout`,
      stderr: `output://${session.id}/stderr`
    }
  };
}

export function readSessionOutput(session: ProcessSession, stream: 'stdout' | 'stderr', offset: number, limit: number): JsonObject {
  const outputReference = `output://${session.id}/${stream}`;
  const retained = session.sensitiveOutput ? REDACTED : stream === 'stdout' ? session.stdout : session.stderr;
  const retainedStart = session.sensitiveOutput ? 0 : stream === 'stdout' ? session.stdoutStart : session.stderrStart;
  const total = session.sensitiveOutput ? Buffer.byteLength(REDACTED) : stream === 'stdout' ? session.stdoutBytes : session.stderrBytes;
  const requested = Math.max(0, Math.trunc(offset));
  let effective = Math.max(retainedStart, Math.min(requested, total));
  const source = Buffer.from(retained);
  let relative = Math.max(0, effective - retainedStart);
  const originalRelative = relative;
  while (relative < source.length && (source[relative] & 0xc0) === 0x80) relative += 1;
  effective += relative - originalRelative;
  const selected = safeUtf8Prefix(source.subarray(relative, relative + boundedViewInteger(limit, 4_096, 1, 1_048_576)));
  const content = selected.toString('utf8');
  const bytes = selected.length;
  const nextOffsetValue = effective + bytes;
  const hasMore = nextOffsetValue < total;
  const cursorExpired = requested < retainedStart;
  const aligned = effective !== Math.max(retainedStart, Math.min(requested, total));
  const warnings: string[] = [];
  if (cursorExpired) warnings.push('requested offset expired; response starts at the oldest retained byte');
  if (aligned) warnings.push('requested offset was aligned to the start of a complete character');
  return {
    ok: true,
    output_ref: outputReference,
    stream_output_ref: outputReference,
    stream,
    offset: effective,
    requested_offset: requested,
    retained_start_offset: retainedStart,
    retained_from: retainedStart,
    cursor_expired: cursorExpired,
    limit: boundedViewInteger(limit, 4_096, 1, 1_048_576),
    encoding: 'utf-8',
    content,
    next_offset: hasMore ? nextOffsetValue : null,
    total_retained_bytes: source.length,
    total_stream_bytes: total,
    total_bytes: total,
    truncated: hasMore,
    has_more: hasMore,
    warnings
  };
}
