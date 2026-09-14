import { currentFolderRuntime } from '../folderRuntime.js';
import {
  FINALIZED_SESSION_RETENTION_MS,
  findProcessOperation,
  killProcessTree,
  ProcessToolError,
  processResult,
  processStatus,
  pruneProcessSessions,
  readSessionOutput,
  removeProcessSession,
  requireProcessSession,
  runCommandGraph,
  startAndYield,
  touchSessionAttachment,
  WAIT_COMMAND_TIMEOUT_DEFAULT_MS,
  WAIT_COMMAND_TIMEOUT_MAX_MS,
  WAIT_COMMAND_TRANSPORT_SAFE_MS,
  waitForSession
} from '../processes.js';
import { toolErrorResult } from '../toolContract.js';
import type { ToolDispatchRequest, ToolHandlerMap } from '../toolDispatch/contract.js';
import type { JsonObject } from '../types.js';
import { rootAndCwd } from '../workspace.js';
import { parseWslUncPath } from '../wsl.js';
import { normalizedSandboxConfig, sandboxUsesPortableCommand } from '../sandbox.js';

function ok(value: JsonObject = {}): JsonObject {
  return { ok: true, ...value };
}

async function execHealthCheck({ ctx, key }: ToolDispatchRequest): Promise<JsonObject> {
  const startedAt = Date.now();
  const { root } = rootAndCwd(ctx, key);
  const wsl = parseWslUncPath(root);
  const sandbox = normalizedSandboxConfig(ctx.config.sandbox);
  const portableSandbox = sandbox.enabled && sandboxUsesPortableCommand(sandbox.backend);
  const probeArgs: JsonObject = wsl || portableSandbox
    ? {
        script: 'cat >/dev/null; printf exec-health; printf exec-health-stderr >&2',
        shell: 'sh',
        confirm: true,
        timeout_ms: 5_000,
        yield_time_ms: 5_000,
        output_mode: 'tail',
        max_output_bytes: 16_384
      }
    : {
        program: process.platform === 'win32' ? 'node.exe' : 'node',
        args: ['-e', 'process.stdin.on("end", () => { process.stdout.write("exec-health"); process.stderr.write("exec-health-stderr"); }); process.stdin.resume();'],
        timeout_ms: 5_000,
        yield_time_ms: 5_000,
        output_mode: 'tail',
        max_output_bytes: 16_384
      };
  try {
    let probe = await startAndYield(ctx, key, probeArgs);
    const sessionCreate = typeof probe.session_id === 'string' && probe.session_id.length > 0;
    if (sessionCreate) {
      const session = requireProcessSession(currentFolderRuntime(ctx, key), probe.session_id, false);
      while (!session.finalizedAt) await waitForSession(session, session.sequence, 5_000, 'finalized');
      probe = processResult(session, { output_mode: 'tail', max_output_bytes: 16_384 });
    }
    const commandRun = Number(probe.process_exit_code ?? probe.exit_code) === 0;
    const stdoutCapture = String(probe.stdout ?? '').includes('exec-health');
    const stderrCapture = String(probe.stderr ?? '').includes('exec-health-stderr');
    const sandboxVerificationRequired = sandbox.enabled;
    const sandboxVerified = sandboxVerificationRequired
      ? probe.sandbox_enforced === true
        && String(probe.sandbox_backend ?? '') === sandbox.backend
        && String(probe.execution_boundary ?? '') === sandbox.backend
      : null;
    const healthy = sessionCreate && commandRun && stdoutCapture && stderrCapture && sandboxVerified !== false;
    return ok({
      worker: { alive: true },
      session_create: sessionCreate,
      command_run: commandRun,
      stdout_capture: stdoutCapture,
      stderr_capture: stderrCapture,
      sandbox_verification: {
        required: sandboxVerificationRequired,
        verified: sandboxVerified,
        backend: sandboxVerificationRequired ? sandbox.backend : null,
        execution_boundary: probe.execution_boundary ?? null
      },
      duration_ms: Date.now() - startedAt,
      next_actions: healthy ? [] : ['检查 exec worker 日志', '重启运行时'],
      status: healthy ? 'success' : 'error',
      summary: healthy
        ? 'exec worker、session、命令执行和 stdout/stderr 捕获均正常'
        : 'exec health check 未通过，请查看 probe 结果',
      probe
    });
  } catch (error) {
    const normalized = toolErrorResult(error);
    return ok({
      worker: { alive: true },
      session_create: false,
      command_run: false,
      stdout_capture: false,
      stderr_capture: false,
      sandbox_verification: {
        required: sandbox.enabled,
        verified: sandbox.enabled ? false : null,
        backend: sandbox.enabled ? sandbox.backend : null,
        execution_boundary: null
      },
      duration_ms: Date.now() - startedAt,
      next_actions: ['检查 exec worker 日志', '重启运行时'],
      status: 'error',
      summary: 'exec session 创建或探针执行失败',
      error: normalized.error ?? normalized
    });
  }
}

async function waitCommand({ ctx, key, args }: ToolDispatchRequest): Promise<JsonObject> {
  const runtime = currentFolderRuntime(ctx, key);
  const registryStarted = Date.now();
  const session = requireProcessSession(runtime, args.session_id);
  const sessionRegistryWaitMs = Date.now() - registryStarted;
  const cursor = Math.max(0, Number(args.cursor ?? 0));
  const waitTimeoutMs = Math.max(
    0,
    Math.min(WAIT_COMMAND_TIMEOUT_MAX_MS, Number(args.timeout_ms ?? WAIT_COMMAND_TIMEOUT_DEFAULT_MS))
  );
  const heartbeatMs = Math.max(0, Math.min(30_000, Number(args.heartbeat_ms ?? 0)));
  const waitUntil = String(args.until ?? 'output_or_exit');
  const waited = await waitForSession(session, cursor, waitTimeoutMs, waitUntil, heartbeatMs);
  const snapshotStarted = Date.now();
  const requestTimedOut = !waited.changed;
  const processCompleted = session.finalizedAt !== undefined;
  const terminal = processCompleted;
  const progressSinceLastWait = session.sequence > cursor || Boolean(session.endedAt) || processCompleted;
  const nextWaitMs = terminal ? null : (waitTimeoutMs === 0 ? WAIT_COMMAND_TIMEOUT_DEFAULT_MS : Math.min(waitTimeoutMs, WAIT_COMMAND_TRANSPORT_SAFE_MS));
  const eventDetail = String(args.event_detail ?? 'compact').trim().toLowerCase() === 'full' ? 'full' : 'compact';
  const result = processResult(session, {
    ...args,
    event_detail: eventDetail,
    request_timed_out: requestTimedOut
  });
  Object.assign(result, {
    session_registry_wait_ms: sessionRegistryWaitMs,
    actual_wait_ms: waited.actualWaitMs,
    snapshot_ms: Date.now() - snapshotStarted,
    heartbeat: waited.heartbeat,
    request_timed_out: requestTimedOut,
    wait_timed_out: requestTimedOut,
    wait_completed: waited.changed || requestTimedOut,
    process_completed: processCompleted,
    terminal,
    progress_since_last_wait: progressSinceLastWait,
    next_wait_ms: nextWaitMs,
    wait_timeout_ms: waitTimeoutMs,
    effective_wait_ms: waited.effectiveWaitMs,
    heartbeat_ms: heartbeatMs,
    wait_until: waitUntil
  });
  if (session.finalizedAt === undefined) {
    result.next_actions = [{
      tool: 'wait_command',
      arguments: {
        session_id: session.id,
        cursor: result.latest_cursor,
        timeout_ms: waitTimeoutMs === 0 ? WAIT_COMMAND_TIMEOUT_DEFAULT_MS : Math.min(waitTimeoutMs, WAIT_COMMAND_TRANSPORT_SAFE_MS),
        until: waitUntil,
        output_mode: 'delta',
        event_detail: eventDetail
      }
    }];
  }
  if (requestTimedOut && !processCompleted) result.suggestion = 'Wait window ended without process completion; process is still running. Continue with next_actions.';
  else if (!processCompleted) result.suggestion = 'Process made progress but is not complete; continue with next_actions.';
  else if (result.termination_reason === 'exited') result.suggestion = '进程已结束；检查 process_exit_code 与 post_checks';
  return result;
}

function resolveOperation({ ctx, key, args }: ToolDispatchRequest): JsonObject {
  const operationId = String(args.operation_id ?? '').trim();
  const requestedFingerprint = String(args.command_fingerprint ?? '').trim();
  if (!operationId && !requestedFingerprint) {
    throw new ProcessToolError('INVALID_ARGUMENT', 'operation_id or command_fingerprint is required', 'invalid_argument', false);
  }
  const resolved = findProcessOperation(currentFolderRuntime(ctx, key), operationId, requestedFingerprint);
  if (!resolved.session) {
    throw new ProcessToolError('OPERATION_NOT_FOUND', 'retained command operation was not found', 'not_found', false, {
      operation_id: operationId || null,
      command_fingerprint: requestedFingerprint || null,
      retention_seconds: FINALIZED_SESSION_RETENTION_MS / 1000,
      suggestion: 'Use list_sessions to inspect retained command sessions.'
    });
  }
  touchSessionAttachment(resolved.session);
  return {
    ...processResult(resolved.session, {
      ...args,
      deduplicated: true,
      attached_to_session_id: resolved.session.id
    }),
    resolved_by: resolved.resolvedBy
  };
}

function listSessions({ ctx, key, args }: ToolDispatchRequest): JsonObject {
  const runtime = currentFolderRuntime(ctx, key);
  pruneProcessSessions(runtime);
  const includeFinalized = args.include_finalized !== false;
  const status = args.status === undefined ? undefined : String(args.status);
  const limit = Math.max(1, Math.min(1000, Number(args.limit ?? 100)));
  const all = [...runtime.sessions.values()].sort((left, right) => right.startedAt - left.startedAt);
  const sessions = all
    .filter(session => includeFinalized || !session.finalizedAt)
    .filter(session => !status || processStatus(session) === status)
    .slice(0, limit)
    .map(session => processResult(session, { output_mode: 'none' }));
  return ok({
    sessions,
    count: sessions.length,
    include_finalized: includeFinalized,
    retention_seconds: FINALIZED_SESSION_RETENTION_MS / 1000,
    active_count: all.filter(session => !session.endedAt).length,
    verifying_count: all.filter(session => processStatus(session) === 'verifying').length,
    finalized_count: all.filter(session => Boolean(session.finalizedAt)).length
  });
}

function sendInput({ ctx, key, args }: ToolDispatchRequest): JsonObject {
  const session = requireProcessSession(currentFolderRuntime(ctx, key), args.session_id);
  if (!session.child || session.endedAt || !session.stdinOpen) {
    throw new ProcessToolError('SESSION_CLOSED', 'Session stdin is closed.', 'runtime', false, {
      session_id: session.id,
      status: processStatus(session)
    });
  }
  const chars = String(args.chars ?? '');
  if (chars) session.child.stdin.write(chars);
  const closeStdin = args.close_stdin === true;
  if (closeStdin) {
    session.child.stdin.end();
    session.stdinOpen = false;
    session.events.emit('change');
  }
  return {
    ...processResult(session, { output_mode: 'none' }),
    bytes_written: Buffer.byteLength(chars),
    stdin_closed: closeStdin,
    suggestion: closeStdin ? 'stdin 已关闭；使用 wait_command 等待进程结束' : 'stdin 已写入；继续使用 send_input 或 wait_command'
  };
}

async function killSession({ ctx, key, args }: ToolDispatchRequest): Promise<JsonObject> {
  const runtime = currentFolderRuntime(ctx, key);
  const session = requireProcessSession(runtime, args.session_id);
  const waitMs = Math.max(0, Math.min(30_000, Number(args.wait_ms ?? 5000)));
  const waitDeadline = Date.now() + waitMs;
  const killRequested = !session.endedAt;
  if (!session.endedAt) {
    await killProcessTree(session, String(args.signal ?? 'TERM') as 'TERM' | 'KILL' | 'INT', 'killed');
    if (waitMs > 0) await waitForSession(session, session.sequence, waitMs, 'exit');
  }
  const terminated = Boolean(session.endedAt);
  if (terminated && !session.finalizedAt && waitMs > 0) {
    const remainingWaitMs = Math.max(0, waitDeadline - Date.now());
    if (remainingWaitMs > 0) await waitForSession(session, session.sequence, remainingWaitMs, 'finalized');
  }
  const finalized = Boolean(session.finalizedAt);
  const result = processResult(session, {
    output_mode: 'tail',
    max_output_bytes: args.max_output_bytes === undefined ? undefined : Number(args.max_output_bytes),
    tail_lines: 100
  });
  const killed = killRequested && terminated;
  Object.assign(result, {
    ok: true,
    killed,
    status: terminated ? finalized ? killed ? 'killed' : 'exited' : 'verifying' : 'terminating',
    evicted: terminated && finalized
  });
  if (terminated && finalized) removeProcessSession(runtime, session.id);
  else {
    result.warnings = [
      ...(Array.isArray(result.warnings) ? result.warnings : []),
      terminated ? 'process exited; sandbox cleanup or verification is still pending' : 'process termination is still pending'
    ];
    result.suggestion = terminated
      ? '继续使用 wait_command 並指定 until=finalized，等待 sandbox cleanup / verification 完成'
      : '继续使用 wait_command 确认进程已终止';
  }
  return result;
}

function readOutput({ ctx, key, args }: ToolDispatchRequest): JsonObject {
  const runtime = currentFolderRuntime(ctx, key);
  pruneProcessSessions(runtime);
  const reference = String(args.output_ref ?? '');
  const match = /^output:\/\/([^/]+)\/(stdout|stderr)$/.exec(reference);
  if (!match) throw new ProcessToolError('INVALID_OUTPUT_REF', 'Invalid output_ref.', 'invalid_argument', false, { output_ref: reference });
  const session = runtime.sessions.get(match[1]);
  if (!session) throw new ProcessToolError('SESSION_NOT_FOUND', `Session not found: ${match[1]}`, 'not_found', false);
  touchSessionAttachment(session);
  const offset = Math.max(0, Number(args.offset ?? 0));
  const limit = Math.max(1, Math.min(1_048_576, Number(args.limit ?? 4096)));
  return readSessionOutput(session, match[2] as 'stdout' | 'stderr', offset, limit);
}

export const processToolHandlers = {
  exec_health_check: execHealthCheck,
  exec_command: ({ ctx, key, args, processLifecycle }) => startAndYield(ctx, key, args, processLifecycle),
  exec_many: ({ ctx, key, args, processLifecycle }) => runCommandGraph(ctx, key, args, processLifecycle?.signal),
  wait_command: waitCommand,
  resolve_operation: resolveOperation,
  list_sessions: listSessions,
  send_input: sendInput,
  kill_session: killSession,
  read_output: readOutput
} satisfies ToolHandlerMap;
