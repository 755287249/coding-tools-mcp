import { randomUUID } from 'node:crypto';
import { EventEmitter } from 'node:events';
import { spawn, type ChildProcessWithoutNullStreams } from 'node:child_process';
import path from 'node:path';
import type { FolderRuntime, JsonObject, ProcessSession, SandboxConfig, ToolContext } from './types.js';
import { relativeInside, resolveExistingDirectory, rootAndCwd } from './workspace.js';
import { sleep } from './runtime.js';
import { argumentsReferenceSensitiveSource, isSensitiveKey, redactSensitiveText } from './redaction.js';
import { resolveCommandSpec, resolvePortableCommandSpec } from './policy.js';
import { classifyCommandKind } from './toolUsage.js';
import { wslInvocationForPath } from './wsl.js';
import { allFolderRuntimes, currentFolderRuntime, runtimeForFolderId } from './folderRuntime.js';
import { processStartupController } from './processStartup.js';
import { appendOutput, processResult, retainedNextActions, terminationReason } from './processes/output.js';
import { nativeLaunchSpec } from './processes/nativeLaunch.js';
import { cargoTargetLock, commandFingerprint, nodeGeneratedLock, safeAutomaticDedup } from './processes/identity.js';
import { boundedInteger, commandTimeoutMaxMs, resolveProcessTimeout } from './processes/timeoutPolicy.js';
import { ProcessToolError, startupToolError } from './processes/errors.js';
import { commandEnvironment, explicitEnvironment, removedEnvironment, resolveSecretInputs } from './processes/environment.js';
import {
  FINALIZED_SESSION_RETENTION_MS,
  MAX_RETAINED_FINALIZED_SESSIONS,
  findProcessOperation,
  pruneProcessSessions,
  removeProcessSession,
  requireProcessSession,
  touchSessionAttachment
} from './processes/sessionRegistry.js';
import { attachHarnessOperation, recordHarnessOperationFinalization } from './processes/harnessTracking.js';
import { waitForChildStreams } from './processes/childStreams.js';
import { runProcessPostChecks } from './processes/postChecks.js';
import {
  launchDurableWorker,
  monitorDurableSession,
  persistDurableSession,
  restoreDurableSessions,
  type DurableWorkerResult
} from './processes/durableSession.js';
import { preflightTestRunnerCapabilities, testWorkflow } from './processes/testRunnerCapabilities.js';
import {
  abortRetainedCommandGraphs,
  COMMAND_GRAPH_RETENTION_MS,
  MAX_RETAINED_COMMAND_GRAPHS,
  pruneRetainedCommandGraphs,
  runCommandGraph as runCommandGraphCore,
  type CommandGraphProcessDependencies
} from './processes/commandGraph.js';
import {
  normalizedSandboxConfig,
  prepareSandboxLaunch,
  sandboxBoundary,
  sandboxUsesPortableCommand,
  type SandboxLaunch
} from './sandbox.js';

export { processResult, processStatus, readSessionOutput } from './processes/output.js';
export { nativeLaunchSpec } from './processes/nativeLaunch.js';
export { cargoTargetLock, nodeGeneratedLock } from './processes/identity.js';
export { resolvedCommandTimeoutMs } from './processes/timeoutPolicy.js';
export { ProcessToolError } from './processes/errors.js';
export { attachHarnessOperation } from './processes/harnessTracking.js';
export {
  FINALIZED_SESSION_RETENTION_MS,
  MAX_RETAINED_FINALIZED_SESSIONS,
  findProcessOperation,
  pruneProcessSessions,
  removeProcessSession,
  requireProcessSession,
  touchSessionAttachment
} from './processes/sessionRegistry.js';
export {
  COMMAND_GRAPH_RETENTION_MS,
  MAX_RETAINED_COMMAND_GRAPHS,
  pruneRetainedCommandGraphs
} from './processes/commandGraph.js';

export const DETACHED_SESSION_GRACE_MS = 90_000;
export const WAIT_COMMAND_TRANSPORT_SAFE_MS = 20_000;
export const WAIT_COMMAND_TIMEOUT_DEFAULT_MS = WAIT_COMMAND_TRANSPORT_SAFE_MS;
export const WAIT_COMMAND_TIMEOUT_MAX_MS = 60 * 60_000;

const WINDOWS_TASKKILL_TIMEOUT_MS = 10_000;

interface CommandSpec { program: string; argv: string[]; display: string; shell: boolean }

interface StartedProcess {
  session: ProcessSession;
  deduplicated: boolean;
  attachedToSessionId: string | null;
  operationLockWaitMs: number;
  testRunnerCapability?: JsonObject;
  testWorkflow?: JsonObject;
}

export class ProcessRequestLifecycle {
  #session?: ProcessSession;
  #aborted = false;
  #completed = false;
  #abortController = new AbortController();

  constructor(private readonly ctx: ToolContext, private readonly graceMs = DETACHED_SESSION_GRACE_MS) {}

  attach(session: ProcessSession): void {
    this.#session = session;
    if (this.#aborted && !this.#completed) markSessionDetached(this.ctx, session, this.graceMs);
  }

  abort(): void {
    if (this.#completed || this.#aborted) return;
    this.#aborted = true;
    this.#abortController.abort(new Error('request aborted'));
    if (this.#session) markSessionDetached(this.ctx, this.#session, this.graceMs);
  }

  complete(): void {
    if (this.#completed) return;
    this.#completed = true;
    if (this.#session && !this.#aborted) touchSessionAttachment(this.#session);
  }

  get aborted(): boolean { return this.#aborted; }
  get signal(): AbortSignal { return this.#abortController.signal; }
}

function fallbackKillChild(child: ChildProcessWithoutNullStreams): void {
  if (child.exitCode !== null || child.signalCode !== null) return;
  try { child.kill('SIGKILL'); } catch { /* best effort */ }
}

async function terminateWindowsProcessTree(child: ChildProcessWithoutNullStreams): Promise<void> {
  if (!child.pid || child.exitCode !== null || child.signalCode !== null) return;
  await new Promise<void>(resolve => {
    let settled = false;
    let timer: NodeJS.Timeout | undefined;
    const killer = spawn('taskkill.exe', ['/pid', String(child.pid), '/t', '/f'], {
      windowsHide: true,
      stdio: 'ignore'
    });
    const finish = (success: boolean) => {
      if (settled) return;
      settled = true;
      if (timer) clearTimeout(timer);
      if (!success) fallbackKillChild(child);
      resolve();
    };
    killer.once('exit', code => finish(code === 0));
    killer.once('error', () => finish(false));
    timer = setTimeout(() => {
      try { killer.kill('SIGKILL'); } catch { /* best effort */ }
      finish(false);
    }, WINDOWS_TASKKILL_TIMEOUT_MS);
    timer.unref();
  });
}

async function terminateUntrackedChild(child: ChildProcessWithoutNullStreams): Promise<void> {
  if (child.exitCode !== null || child.signalCode !== null) return;
  if (process.platform === 'win32' && child.pid) {
    await terminateWindowsProcessTree(child);
    return;
  }
  try {
    if (child.pid) process.kill(-child.pid, 'SIGKILL');
    else child.kill('SIGKILL');
  } catch {
    try { child.kill('SIGKILL'); } catch { /* best effort */ }
  }
}

async function finalizeSession(ctx: ToolContext, session: ProcessSession, verificationOk: boolean): Promise<void> {
  if (session.finalizedAt) return;
  session.postChecksPending = false;
  session.verificationOk = verificationOk;
  session.finalizedAt = Date.now();
  if (!session.telemetryRecorded) {
    session.telemetryRecorded = true;
    ctx.usageStore.recordAsyncSession({
      sessionId: session.id,
      commandKind: session.telemetryCommandKind,
      startedTsMs: session.startedAt,
      completedTsMs: session.finalizedAt,
      childProcessTotalMs: Math.max(0, session.finalizedAt - session.startedAt),
      firstOutputMs: session.firstOutputAt === undefined ? null : Math.max(0, session.firstOutputAt - session.startedAt),
      exitCode: session.exitCode ?? null,
      terminationReason: terminationReason(session),
      stdoutBytes: session.stdoutBytes,
      stderrBytes: session.stderrBytes
    });
  }
  await recordHarnessOperationFinalization(ctx, session);
  if (session.timeoutTimer) clearTimeout(session.timeoutTimer);
  if (session.detachedTimer) clearTimeout(session.detachedTimer);
  session.lockRelease?.();
  session.lockRelease = undefined;
  session.events.emit('change');
  session.events.emit('finalized');
  await persistDurableSession(session);
}

export function markSessionDetached(ctx: ToolContext, session: ProcessSession, graceMs = DETACHED_SESSION_GRACE_MS): number {
  const generation = session.attachmentGeneration + 1;
  session.attachmentGeneration = generation;
  session.detachedGeneration = generation;
  if (session.detachedTimer) clearTimeout(session.detachedTimer);
  if (session.durableDirectory) {
    void persistDurableSession(session).catch(() => { /* best effort; worker remains authoritative */ });
    session.events.emit('change');
    return generation;
  }
  session.detachedTimer = setTimeout(() => {
    void (async () => {
      if (session.detachedGeneration !== generation || session.attachmentGeneration !== generation || session.finalizedAt) return;
      session.terminationReason = 'detached_timeout';
      if (!session.endedAt) {
        await killProcessTree(session, 'KILL', 'detached_timeout');
        await waitForSession(session, session.sequence, 5_000, 'exit');
      }
      if (!session.finalizedAt) {
        await waitForSession(session, session.sequence, 5_000, 'finalized');
      }
      pruneProcessSessions(runtimeForFolderId(ctx, session.folderId));
    })();
  }, Math.max(1, graceMs));
  session.detachedTimer.unref();
  session.events.emit('change');
  return generation;
}

async function completeDurableSession(ctx: ToolContext, session: ProcessSession, result: DurableWorkerResult): Promise<void> {
  if (session.finalizedAt) return;
  session.exitCode = result.exitCode;
  session.signal = result.signal;
  session.endedAt = result.endedAt;
  session.stdinOpen = false;
  if (!session.terminationReason || session.terminationReason === 'running') session.terminationReason = result.terminationReason;
  if (result.error) appendOutput(ctx, session, 'stderr', Buffer.from(`${result.error}\n`));
  session.timedOut = session.terminationReason === 'process_timeout';
  session.events.emit('change');
  session.events.emit('exit');
  await finalizeSession(ctx, session, result.exitCode === 0 && terminationReason(session) === 'exited');
}

function attachDurableSessionMonitor(ctx: ToolContext, session: ProcessSession): void {
  if (session.finalizedAt) return;
  monitorDurableSession(ctx, session, result => completeDurableSession(ctx, session, result));
}

export async function restoreProcessSessions(ctx: ToolContext): Promise<void> {
  const restored = await restoreDurableSessions(ctx);
  for (const session of restored) {
    if (!session.finalizedAt) attachDurableSessionMonitor(ctx, session);
  }
  for (const runtime of allFolderRuntimes(ctx)) pruneProcessSessions(runtime);
}

export async function startProcess(
  ctx: ToolContext,
  key: string,
  args: JsonObject,
  signal?: AbortSignal
): Promise<StartedProcess> {
  const runtime = currentFolderRuntime(ctx, key);
  pruneProcessSessions(runtime);
  const { folder, root, cwd: defaultCwd } = rootAndCwd(ctx, key);
  const requestedWorkdir = String(args.workdir ?? args.cwd ?? relativeInside(root, defaultCwd));
  const resolvedCwd = await resolveExistingDirectory(
    root,
    requestedWorkdir,
    'Command workdir must be a directory'
  );
  const cwd = resolvedCwd.full;
  const sandbox = sandboxBoundary(ctx.config.sandbox, folder);
  const spec = sandbox.enabled && sandboxUsesPortableCommand(sandbox.backendId)
    ? await resolvePortableCommandSpec(ctx, key, args)
    : await resolveCommandSpec(ctx, key, args);
  const testRunnerCapability = sandbox.enabled ? undefined : await preflightTestRunnerCapabilities(
    { program: spec.program, args: spec.argv },
    cwd,
    async (program, helpArgs, probeCwd) => runBuffered(
      program,
      helpArgs,
      probeCwd,
      undefined,
      5_000,
      commandEnvironment(args),
      {
        routeWsl: Boolean(wslInvocationForPath(
          probeCwd,
          program,
          helpArgs,
          explicitEnvironment(args),
          removedEnvironment(args)
        )),
        environment: explicitEnvironment(args),
        removeEnvironment: removedEnvironment(args)
      }
    )
  );
  const testRunnerWorkflow = testWorkflow({ program: spec.program, args: spec.argv }, requestedWorkdir);
  const timeoutContract = resolveProcessTimeout(args, spec.display, commandTimeoutMaxMs(ctx));
  const timeoutMs = timeoutContract.effectiveTimeoutMs;
  if (timeoutContract.executionMode === 'job') {
    const unsupported: string[] = [];
    if (sandbox.enabled) unsupported.push('sandbox');
    if (args.tty === true) unsupported.push('tty');
    if (args.secret_env && typeof args.secret_env === 'object') unsupported.push('secret_env');
    if (Object.hasOwn(args, 'stdin') && String(args.stdin ?? '').length) unsupported.push('stdin');
    if (argumentsReferenceSensitiveSource(args)) unsupported.push('sensitive_source');
    const explicitEnv = args.env && typeof args.env === 'object' && !Array.isArray(args.env) ? args.env as JsonObject : undefined;
    if (explicitEnv && Object.keys(explicitEnv).some(isSensitiveKey)) unsupported.push('sensitive_env');
    if (redactSensitiveText([spec.program, ...spec.argv].join(' ')).count > 0) unsupported.push('sensitive_command');
    if (Array.isArray(args.post_checks) && args.post_checks.length) unsupported.push('post_checks');
    if (unsupported.length) {
      throw new ProcessToolError(
        'DURABLE_JOB_UNSUPPORTED',
        `Restart-persistent managed jobs do not support: ${unsupported.join(', ')}.`,
        'validation',
        false,
        { process_started: false, execution_mode: 'job', restart_recoverable: false, unsupported_features: unsupported }
      );
    }
  }
  const resolvedSecrets = resolveSecretInputs(ctx, args);
  const commandFingerprintValue = commandFingerprint(cwd, spec, args, timeoutMs, ctx.config.sandbox, {
    ...resolvedSecrets.fingerprint,
    stdin: resolvedSecrets.stdin
  });
  const explicitOperationId = String(args.operation_id ?? '').trim();
  const deduplicate = Boolean(explicitOperationId) || args.deduplicate === true || (args.deduplicate === undefined && safeAutomaticDedup(spec));
  const operationId = explicitOperationId || (deduplicate ? `auto:${commandFingerprintValue.slice(0, 32)}` : undefined);
  const operationLockStarted = Date.now();
  const operationRelease = operationId ? await runtime.admission.locks.acquire([`exec-operation:${operationId}`]) : undefined;
  const operationLockWaitMs = Date.now() - operationLockStarted;

  try {
    if (operationId) {
      const existing = [...runtime.sessions.values()].find(session => session.operationId === operationId);
      if (existing) {
        if (explicitOperationId && existing.fingerprint !== commandFingerprintValue) {
          throw new ProcessToolError('OPERATION_ID_CONFLICT', `operation_id already belongs to a different command: ${operationId}`, 'conflict', false, {
            operation_id: operationId,
            existing_session_id: existing.id,
            existing_fingerprint: existing.fingerprint,
            requested_fingerprint: commandFingerprintValue
          });
        }
        const reusable = explicitOperationId || !existing.finalizedAt;
        if (reusable) {
          touchSessionAttachment(existing);
          return { session: existing, deduplicated: true, attachedToSessionId: existing.id, operationLockWaitMs, testRunnerCapability, testWorkflow: testRunnerWorkflow };
        }
        removeProcessSession(runtime, existing.id);
      }
    }
    if (deduplicate && !explicitOperationId) {
      const existingId = runtime.operationsByFingerprint.get(commandFingerprintValue);
      const existing = existingId ? runtime.sessions.get(existingId) : undefined;
      if (existing) {
        const reusable = !existing.finalizedAt;
        if (reusable) {
          touchSessionAttachment(existing);
          return { session: existing, deduplicated: true, attachedToSessionId: existing.id, operationLockWaitMs, testRunnerCapability, testWorkflow: testRunnerWorkflow };
        }
        removeProcessSession(runtime, existing.id);
      }
    }
    const activeSessions = [...runtime.sessions.values()].filter(session => !session.finalizedAt).length;
    if (activeSessions >= ctx.config.limits.activeSessionLimit) {
      throw new ProcessToolError('SESSION_LIMIT_REACHED', `Active command session limit reached (${ctx.config.limits.activeSessionLimit}).`, 'runtime', true, {
        stage: 'session_admission',
        active_session_limit: ctx.config.limits.activeSessionLimit,
        active_session_slots_available: 0,
        suggestion: '等待现有命令完成，或使用 kill_session 终止不再需要的长任务'
      });
    }

    const automaticCargoLock = cargoTargetLock(cwd, spec, args);
    const automaticNodeLock = nodeGeneratedLock(cwd, spec);
    const automaticResourceLock = automaticCargoLock ?? automaticNodeLock;
    const explicitLockGroup = String(args.lock_group ?? '').trim();
    const lockGroup = explicitLockGroup || automaticResourceLock?.group || '';
    const lockTarget = explicitLockGroup ? cwd : automaticResourceLock?.target;
    const resourceLockStarted = Date.now();
    const lockRelease = lockGroup ? await runtime.admission.locks.acquire([`process:${lockGroup}`]) : undefined;
    const resourceLockWaitMs = Date.now() - resourceLockStarted;
    const id = randomUUID();
    let sandboxLaunch: SandboxLaunch | undefined;
    let sandboxPrepareMs: number | undefined;
    try {
      if (sandbox.enabled) {
        const sandboxPrepareStartedAt = Date.now();
        sandboxLaunch = await prepareSandboxLaunch(
          normalizedSandboxConfig(ctx.config.sandbox),
          root,
          ctx.config.dataDir,
          cwd,
          spec,
          explicitEnvironment(args, resolvedSecrets.environment),
          removedEnvironment(args),
          signal,
          timeoutMs
        );
        sandboxPrepareMs = Date.now() - sandboxPrepareStartedAt;
      }
    } catch (error) {
      lockRelease?.();
      throw error;
    }
    const processStartedAt = Date.now();
    const deadlineMs = processStartedAt + timeoutMs;
    const wsl = sandboxLaunch
      ? undefined
      : wslInvocationForPath(cwd, spec.program, spec.argv, explicitEnvironment(args, resolvedSecrets.environment), removedEnvironment(args));
    const launchEnvironment = sandboxLaunch?.environmentMode === 'forwarded' || wsl ? process.env : commandEnvironment(args, resolvedSecrets.environment);
    const nativeLaunch = sandboxLaunch || wsl
      ? undefined
      : nativeLaunchSpec(spec.program, spec.argv, cwd, launchEnvironment);
    const launchProgram = sandboxLaunch?.program ?? wsl?.program ?? nativeLaunch?.program ?? spec.program;
    const launchArgs = sandboxLaunch?.args ?? wsl?.args ?? nativeLaunch?.args ?? spec.argv;
    if (timeoutContract.executionMode === 'job' && wsl) {
      lockRelease?.();
      throw new ProcessToolError(
        'DURABLE_JOB_UNSUPPORTED',
        'Restart-persistent managed jobs do not yet support WSL path routing.',
        'validation',
        false,
        { process_started: false, execution_mode: 'job', restart_recoverable: false, unsupported_features: ['wsl'] }
      );
    }
    if (timeoutContract.executionMode === 'job') {
      const session: ProcessSession = {
        id,
        folderId: folder.id,
        workspacePath: folder.path,
        operationId,
        fingerprint: commandFingerprintValue,
        command: spec.display,
        program: spec.program,
        argv: [...spec.argv],
        shell: spec.shell,
        cwd,
        startupDiagnostics: {
          attempts: 1,
          gateWaitMs: 0,
          retryDelaysMs: [],
          errorDialogSuppressed: false,
          startupSlots: 0,
          startIntervalMs: 0
        },
        startedAt: processStartedAt,
        timeoutContract,
        processDeadlineMs: deadlineMs,
        stdout: '',
        stderr: '',
        stdoutBytes: 0,
        stderrBytes: 0,
        stdoutStart: 0,
        stderrStart: 0,
        sequence: 0,
        outputEvents: [],
        outputEventBytes: 0,
        interactive: false,
        stdinOpen: false,
        timedOut: false,
        killed: false,
        sandboxEnforced: false,
        executionBoundary: 'durable_worker',
        processTreeContained: true,
        processTreeControl: process.platform === 'win32' ? 'durable_worker_taskkill_tree' : 'durable_worker_process_group',
        telemetryCommandKind: classifyCommandKind(args),
        testRunnerCapability,
        testWorkflow: testRunnerWorkflow,
        sensitiveOutput: argumentsReferenceSensitiveSource(args),
        postChecks: [],
        postChecksPending: false,
        resourceLockGroup: lockGroup || undefined,
        resourceLockTarget: lockTarget,
        operationLockWaitMs,
        resourceLockWaitMs,
        lockRelease,
        attachmentGeneration: 1,
        detachedGeneration: 0,
        events: new EventEmitter()
      };
      runtime.sessions.set(id, session);
      runtime.operationsByFingerprint.set(commandFingerprintValue, id);
      try {
        await launchDurableWorker(ctx, session, {
          program: launchProgram,
          args: launchArgs,
          cwd,
          shell: spec.shell,
          windowsVerbatimArguments: nativeLaunch?.windowsVerbatimArguments,
          environment: launchEnvironment
        });
      } catch (error) {
        removeProcessSession(runtime, id);
        throw startupToolError(error);
      }
      attachDurableSessionMonitor(ctx, session);
      return { session, deduplicated: false, attachedToSessionId: null, operationLockWaitMs, testRunnerCapability, testWorkflow: testRunnerWorkflow };
    }
    const startupOutput = new WeakMap<ChildProcessWithoutNullStreams, {
      stdout: Buffer[];
      stderr: Buffer[];
      onStdout: (chunk: Buffer) => void;
      onStderr: (chunk: Buffer) => void;
    }>();
    const sandboxStartupStartedAt = sandbox.enabled ? Date.now() : undefined;
    let controlled;
    try {
      controlled = await processStartupController.start(() => {
        const launched = spawn(launchProgram, launchArgs, {
          cwd: sandboxLaunch || wsl ? undefined : cwd,
          env: launchEnvironment,
          windowsHide: true,
          windowsVerbatimArguments: nativeLaunch?.windowsVerbatimArguments,
          detached: process.platform !== 'win32',
          shell: spec.shell,
          stdio: 'pipe'
        });
        sandboxLaunch?.onSpawn?.(launched.pid);
        const captured = {
          stdout: [] as Buffer[],
          stderr: [] as Buffer[],
          onStdout: (chunk: Buffer) => captured.stdout.push(Buffer.from(chunk)),
          onStderr: (chunk: Buffer) => captured.stderr.push(Buffer.from(chunk))
        };
        launched.stdout.on('data', captured.onStdout);
        launched.stderr.on('data', captured.onStderr);
        startupOutput.set(launched, captured);
        return launched;
      }, {
        signal,
        // Match Rust retained-session semantics: startup admission and the
        // Windows loader probe may consume the command budget, but they must
        // not fail before a session is registered. The original deadline is
        // enforced immediately by the session timeout timer after retention.
        terminate: async child => {
          let backendError: unknown;
          if (sandboxLaunch) {
            try { await sandboxLaunch.kill(); } catch (error) { backendError = error; }
          }
          await terminateUntrackedChild(child);
          if (backendError) throw backendError;
        }
      });
    } catch (error) {
      lockRelease?.();
      if (sandboxLaunch) {
        try { await sandboxLaunch.cleanup(); } catch { /* preserve startup error */ }
      }
      throw startupToolError(error);
    }
    const child = controlled.child;
    const sandboxStartupMs = sandboxStartupStartedAt === undefined ? undefined : Date.now() - sandboxStartupStartedAt;
    const interactive = args.tty === true;
    const session: ProcessSession = {
      id,
      folderId: folder.id,
      workspacePath: folder.path,
      operationId,
      fingerprint: commandFingerprintValue,
      command: spec.display,
      program: spec.program,
      argv: [...spec.argv],
      shell: spec.shell,
      cwd,
      startupDiagnostics: controlled.diagnostics,
      startedAt: processStartedAt,
      timeoutContract,
      processDeadlineMs: deadlineMs,
      stdout: '',
      stderr: '',
      stdoutBytes: 0,
      stderrBytes: 0,
      stdoutStart: 0,
      stderrStart: 0,
      sequence: 0,
      outputEvents: [],
      outputEventBytes: 0,
      child,
      interactive,
      stdinOpen: true,
      timedOut: false,
      killed: false,
      sandboxEnforced: sandbox.enabled,
      sandboxBackend: sandbox.backendId,
      executionBoundary: sandbox.executionBoundary,
      sandboxPrepareMs,
      sandboxStartupMs,
      processTreeContained: sandboxLaunch?.processTreeContained ?? (process.platform !== 'win32'),
      processTreeControl: sandboxLaunch?.processTreeControl ?? (process.platform === 'win32' ? 'taskkill_tree' : 'process_group'),
      backendKill: sandboxLaunch?.kill,
      telemetryCommandKind: classifyCommandKind(args),
      testRunnerCapability,
      testWorkflow: testRunnerWorkflow,
      sensitiveOutput: argumentsReferenceSensitiveSource(args),
      postChecks: [],
      postChecksPending: false,
      resourceLockGroup: lockGroup || undefined,
      resourceLockTarget: lockTarget,
      operationLockWaitMs,
      resourceLockWaitMs,
      lockRelease,
      attachmentGeneration: 1,
      detachedGeneration: 0,
      events: new EventEmitter()
    };
    runtime.sessions.set(id, session);
    runtime.operationsByFingerprint.set(commandFingerprintValue, id);

    child.stdout.on('data', (chunk: Buffer) => appendOutput(ctx, session, 'stdout', chunk));
    child.stderr.on('data', (chunk: Buffer) => appendOutput(ctx, session, 'stderr', chunk));
    const capturedStartupOutput = startupOutput.get(child);
    if (capturedStartupOutput) {
      child.stdout.off('data', capturedStartupOutput.onStdout);
      child.stderr.off('data', capturedStartupOutput.onStderr);
      for (const chunk of capturedStartupOutput.stdout) appendOutput(ctx, session, 'stdout', chunk);
      for (const chunk of capturedStartupOutput.stderr) appendOutput(ctx, session, 'stderr', chunk);
      startupOutput.delete(child);
    }
    const onChildError = (error: Error) => {
      if (!session.terminationReason) session.terminationReason = 'crashed';
      appendOutput(ctx, session, 'stderr', Buffer.from(`${error.message}\n`));
    };
    child.on('error', onChildError);
    let closed = false;
    const closeChild = (code: number | null, childSignal: NodeJS.Signals | null) => {
      if (closed) return;
      closed = true;
      child.off('error', onChildError);
      session.exitCode = code;
      session.signal = childSignal;
      session.endedAt = Date.now();
      session.stdinOpen = false;
      session.child = undefined;
      session.terminationReason ??= 'exited';
      session.events.emit('change');
      session.events.emit('exit');
      const checks = Array.isArray(args.post_checks) ? args.post_checks as JsonObject[] : [];
      void (async () => {
        if (sandboxLaunch) {
          const sandboxCleanupStartedAt = Date.now();
          try {
            await sandboxLaunch.cleanup();
          } finally {
            session.sandboxCleanupMs = Date.now() - sandboxCleanupStartedAt;
          }
        }
        await runProcessPostChecks({ runBuffered, finalizeSession }, ctx, key, session, checks.slice(0, 16), cwd);
      })().catch(async error => {
        session.postChecks.push({ ok: false, error: error instanceof Error ? error.message : String(error) });
        await finalizeSession(ctx, session, false);
      });
    };
    child.once('close', closeChild);
    for (const startupError of controlled.handoff()) onChildError(startupError);

    const stdin = resolvedSecrets.stdin;
    if (controlled.earlyExit) {
      session.stdinOpen = false;
      try { child.stdin.end(); } catch { /* child already exited */ }
    } else if (!interactive) {
      if (stdin) child.stdin.write(stdin);
      child.stdin.end();
      session.stdinOpen = false;
    }
    if (controlled.earlyExit) {
      await waitForChildStreams(child);
      closeChild(child.exitCode, child.signalCode);
    } else {
      queueMicrotask(() => {
        if (closed || (child.exitCode === null && child.signalCode === null)) return;
        void waitForChildStreams(child).then(() => closeChild(child.exitCode, child.signalCode));
      });
    }

    if (!controlled.earlyExit) {
      const remainingTimeoutMs = Math.max(1, deadlineMs - Date.now());
      session.timeoutTimer = setTimeout(() => {
        if (!session.endedAt) {
          session.timedOut = true;
          session.terminationReason = 'process_timeout';
          void killProcessTree(session, 'KILL', 'process_timeout');
        }
      }, remainingTimeoutMs);
      session.timeoutTimer.unref();
      session.events.once('exit', () => { if (session.timeoutTimer) clearTimeout(session.timeoutTimer); });
    }
    return { session, deduplicated: false, attachedToSessionId: null, operationLockWaitMs, testRunnerCapability, testWorkflow: testRunnerWorkflow };
  } finally {
    operationRelease?.();
  }
}

export async function startAndYield(ctx: ToolContext, key: string, args: JsonObject, lifecycle?: ProcessRequestLifecycle): Promise<JsonObject> {
  const started = await startProcess(ctx, key, args, lifecycle?.signal);
  const session = started.session;
  lifecycle?.attach(session);
  const yieldMs = boundedInteger(args.yield_time_ms, 1_000, 0, 30_000);
  if (!session.endedAt && yieldMs > 0 && !session.interactive) await waitForSession(session, session.sequence, yieldMs, 'output_or_exit');
  const result = processResult(session, {
    ...args,
    deduplicated: started.deduplicated,
    attached_to_session_id: started.attachedToSessionId,
    operation_lock_wait_ms: started.operationLockWaitMs
  });
  const nextActions = retainedNextActions(session, WAIT_COMMAND_TIMEOUT_DEFAULT_MS);
  if (nextActions.length) result.next_actions = nextActions;
  if (started.testRunnerCapability) result.test_runner_capability = started.testRunnerCapability;
  return result;
}

export async function waitForSession(
  session: ProcessSession,
  cursor: number,
  timeoutMs: number,
  until: string,
  _heartbeatMs = 0
): Promise<{ heartbeat: boolean; changed: boolean; actualWaitMs: number; effectiveWaitMs: number }> {
  const startedAt = Date.now();
  const satisfied = () => {
    if (until === 'finalized') return Boolean(session.finalizedAt);
    if (until === 'exit') return Boolean(session.endedAt);
    return Boolean(session.endedAt) || session.sequence > cursor || (cursor === 0 && session.firstOutputAt !== undefined);
  };
  const boundedTimeout = boundedInteger(
    timeoutMs,
    WAIT_COMMAND_TIMEOUT_DEFAULT_MS,
    0,
    WAIT_COMMAND_TIMEOUT_MAX_MS
  );
  const effectiveWaitMs = Math.min(boundedTimeout, WAIT_COMMAND_TRANSPORT_SAFE_MS);
  if (satisfied() || effectiveWaitMs <= 0) return { heartbeat: false, changed: satisfied(), actualWaitMs: 0, effectiveWaitMs };
  return new Promise(resolve => {
    let changed = false;
    const done = () => {
      cleanup();
      resolve({
        heartbeat: false,
        changed,
        actualWaitMs: Date.now() - startedAt,
        effectiveWaitMs
      });
    };
    const onChange = () => { if (satisfied()) { changed = true; done(); } };
    const timer = setTimeout(done, effectiveWaitMs);
    const cleanup = () => { clearTimeout(timer); session.events.off('change', onChange); };
    session.events.on('change', onChange);
    if (satisfied()) { changed = true; done(); }
  });
}

export async function killProcessTree(session: ProcessSession, signal: 'TERM' | 'KILL' | 'INT' = 'TERM', reason = 'killed'): Promise<void> {
  if (session.endedAt) return;
  const child = session.child;
  session.terminationReason = reason;
  session.killed = reason === 'killed';
  session.events.emit('change');
  void persistDurableSession(session).catch(() => { /* best effort */ });
  let backendError: unknown;
  if (session.backendKill) {
    try { await session.backendKill(); } catch (error) { backendError = error; }
  }
  if (!child?.pid && session.processId) {
    if (process.platform === 'win32') {
      await new Promise<void>(resolve => {
        const killer = spawn('taskkill.exe', ['/pid', String(session.processId), '/t', '/f'], { windowsHide: true, stdio: 'ignore' });
        killer.once('exit', () => resolve());
        killer.once('error', () => resolve());
      });
    } else {
      const mappedSignal = signal === 'INT' ? 'SIGINT' : signal === 'KILL' ? 'SIGKILL' : 'SIGTERM';
      try { process.kill(-session.processId, mappedSignal); }
      catch { try { process.kill(session.processId, mappedSignal); } catch { /* already exited */ } }
    }
  }
  if (child?.pid) {
    if (process.platform === 'win32') {
      // Windows cannot reliably deliver POSIX TERM/INT semantics to arbitrary console
      // process trees. Match the Rust runtime: every kill_session signal terminates
      // the managed Windows tree forcefully so the control call completes reliably.
      await terminateWindowsProcessTree(child);
    } else {
      try { process.kill(-child.pid, signal === 'INT' ? 'SIGINT' : signal === 'KILL' ? 'SIGKILL' : 'SIGTERM'); }
      catch { child.kill(signal === 'INT' ? 'SIGINT' : signal === 'KILL' ? 'SIGKILL' : 'SIGTERM'); }
    }
  }
  if (backendError) throw backendError;
}

export async function disposeProcessSessions(ctx: ToolContext): Promise<void> {
  const runtimes = allFolderRuntimes(ctx);
  const graphCompletions = abortRetainedCommandGraphs(runtimes);
  const sessions = runtimes.flatMap(runtime => [...runtime.sessions.values()]);
  await Promise.all(sessions.map(async session => {
    if (session.finalizedAt) return;
    if (session.durableDirectory) {
      if (session.durableMonitor) clearInterval(session.durableMonitor);
      session.durableMonitor = undefined;
      await persistDurableSession(session);
      return;
    }
    session.terminationReason = 'server_restart';
    if (!session.endedAt) {
      await killProcessTree(session, 'KILL', 'server_restart');
      await waitForSession(session, session.sequence, 5_000, 'exit');
    }
    if (!session.finalizedAt) {
      await waitForSession(session, session.sequence, 5_000, 'finalized');
    }
  }));
  await Promise.allSettled(graphCompletions);
}

export interface BufferedRunRouting {
  routeWsl?: boolean;
  environment?: Array<[string, string]>;
  removeEnvironment?: string[];
  platform?: NodeJS.Platform;
  wrappedProcess?: boolean;
  backendKill?: () => Promise<void>;
  onSpawn?: (pid: number | undefined) => void;
  cleanEnvironment?: boolean;
}

export async function runBuffered(
  program: string,
  args: string[],
  cwd: string,
  input?: string,
  timeoutMs = 30_000,
  environment?: NodeJS.ProcessEnv,
  routing: BufferedRunRouting = {}
): Promise<{ code: number | null; stdout: string; stderr: string }> {
  const wsl = routing.routeWsl
    ? wslInvocationForPath(
      cwd,
      program,
      args,
      routing.environment ?? [],
      routing.removeEnvironment ?? [],
      routing.platform,
      routing.cleanEnvironment ?? false
    )
    : undefined;
  const startedAt = Date.now();
  const deadlineMs = startedAt + Math.max(1, timeoutMs);
  const startupOutput = new WeakMap<ChildProcessWithoutNullStreams, {
    stdout: Buffer[];
    stderr: Buffer[];
    onStdout: (chunk: Buffer) => void;
    onStderr: (chunk: Buffer) => void;
  }>();
  const launchEnvironment = environment ?? process.env;
  const nativeLaunch = wsl || routing.wrappedProcess
    ? undefined
    : nativeLaunchSpec(program, args, cwd, launchEnvironment, routing.platform ?? process.platform);
  let controlled;
  try {
    controlled = await processStartupController.start(() => {
      const launched = spawn(wsl?.program ?? nativeLaunch?.program ?? program, wsl?.args ?? nativeLaunch?.args ?? args, {
        cwd: wsl || routing.wrappedProcess ? undefined : cwd,
        env: launchEnvironment,
        windowsHide: true,
        windowsVerbatimArguments: nativeLaunch?.windowsVerbatimArguments,
        detached: process.platform !== 'win32',
        stdio: 'pipe'
      });
      routing.onSpawn?.(launched.pid);
      const captured = {
        stdout: [] as Buffer[],
        stderr: [] as Buffer[],
        onStdout: (chunk: Buffer) => captured.stdout.push(Buffer.from(chunk)),
        onStderr: (chunk: Buffer) => captured.stderr.push(Buffer.from(chunk))
      };
      launched.stdout.on('data', captured.onStdout);
      launched.stderr.on('data', captured.onStderr);
      startupOutput.set(launched, captured);
      return launched;
    }, {
      deadlineMs,
      terminate: async child => {
        let backendError: unknown;
        if (routing.backendKill) {
          try { await routing.backendKill(); } catch (error) { backendError = error; }
        }
        await terminateUntrackedChild(child);
        if (backendError) throw backendError;
      }
    });
  } catch (error) {
    throw startupToolError(error);
  }

  const child = controlled.child;
  const capturedStartupOutput = startupOutput.get(child);
  if (capturedStartupOutput) {
    child.stdout.off('data', capturedStartupOutput.onStdout);
    child.stderr.off('data', capturedStartupOutput.onStderr);
    startupOutput.delete(child);
  }
  return new Promise((resolve, reject) => {
    let stdout = capturedStartupOutput ? Buffer.concat(capturedStartupOutput.stdout).toString() : '';
    let stderr = capturedStartupOutput ? Buffer.concat(capturedStartupOutput.stderr).toString() : '';
    let settled = false;
    let timer: NodeJS.Timeout | undefined;
    const cleanup = () => {
      if (timer) clearTimeout(timer);
      child.off('error', onError);
      child.off('close', onClose);
    };
    const finish = (error?: Error, code: number | null = child.exitCode) => {
      if (settled) return;
      settled = true;
      cleanup();
      if (error) reject(error); else resolve({ code, stdout, stderr });
    };
    const onError = (error: Error) => finish(error);
    const onClose = (code: number | null) => finish(undefined, code);
    child.stdout.on('data', (data: Buffer) => { stdout += data.toString(); });
    child.stderr.on('data', (data: Buffer) => { stderr += data.toString(); });
    child.on('error', onError);
    child.once('close', onClose);
    if (!controlled.earlyExit) {
      timer = setTimeout(() => {
        void (async () => {
          try { if (routing.backendKill) await routing.backendKill(); }
          finally { await terminateUntrackedChild(child); }
        })();
      }, Math.max(1, timeoutMs));
      timer.unref();
    }
    for (const startupError of controlled.handoff()) onError(startupError);
    try { child.stdin.end(input); } catch { /* child already exited */ }
    queueMicrotask(() => {
      if (settled || (child.exitCode === null && child.signalCode === null)) return;
      void waitForChildStreams(child).then(() => finish(undefined, child.exitCode));
    });
  });
}

const commandGraphDependencies: CommandGraphProcessDependencies = {
  startProcess,
  waitForSession,
  killProcessTree,
  normalizeError(error) {
    return error instanceof ProcessToolError ? error : startupToolError(error);
  },
  error(code, message, category = 'runtime', retryable = false, details = {}) {
    return new ProcessToolError(code, message, category, retryable, details);
  }
};

export async function runCommandGraph(
  ctx: ToolContext,
  key: string,
  args: JsonObject,
  signal?: AbortSignal
): Promise<JsonObject> {
  return runCommandGraphCore(commandGraphDependencies, ctx, key, args, signal);
}
