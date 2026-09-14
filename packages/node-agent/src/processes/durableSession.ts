import { spawn } from 'node:child_process';
import { createHash } from 'node:crypto';
import { EventEmitter } from 'node:events';
import { mkdir, open, readFile, readdir, rename, rm, stat, writeFile } from 'node:fs/promises';
import path from 'node:path';
import { fileURLToPath } from 'node:url';
import type { JsonObject, ProcessSession, ToolContext } from '../types.js';
import { appendOutput } from './output.js';

const DURABLE_SCHEMA_VERSION = 1;
const DURABLE_MONITOR_MS = 250;
const DURABLE_READ_CHUNK_BYTES = 1024 * 1024;

interface DurableLaunchManifest {
  version: 1;
  program: string;
  args: string[];
  cwd: string;
  shell: boolean;
  windowsVerbatimArguments?: boolean;
  deadlineMs: number;
  stdoutPath: string;
  stderrPath: string;
  runtimePath: string;
  resultPath: string;
}

interface DurableSessionSnapshot {
  version: 1;
  id: string;
  folderId: string;
  workspacePath: string;
  operationId?: string;
  fingerprint: string;
  command: string;
  program: string;
  argv: string[];
  shell: boolean;
  cwd: string;
  startupDiagnostics: ProcessSession['startupDiagnostics'];
  startedAt: number;
  timeoutContract?: ProcessSession['timeoutContract'];
  processDeadlineMs?: number;
  firstOutputAt?: number;
  endedAt?: number;
  finalizedAt?: number;
  exitCode?: number | null;
  signal?: string | null;
  terminationReason?: string;
  verificationOk?: boolean;
  stdoutBytes: number;
  stderrBytes: number;
  sequence: number;
  workerPid?: number;
  processPid?: number;
  telemetryCommandKind: string;
  testRunnerCapability?: JsonObject;
  testWorkflow?: JsonObject;
  telemetryRecorded?: boolean;
  resourceLockGroup?: string;
  resourceLockTarget?: string;
  operationLockWaitMs: number;
  resourceLockWaitMs: number;
}

interface DurableWorkerRuntime {
  version: 1;
  workerPid: number;
  processPid: number | null;
  startedAt: number;
}

export interface DurableWorkerResult {
  version: 1;
  exitCode: number | null;
  signal: string | null;
  terminationReason: string;
  error?: string | null;
  endedAt: number;
}

interface DurablePaths {
  dir: string;
  snapshot: string;
  manifest: string;
  stdout: string;
  stderr: string;
  runtime: string;
  result: string;
}

function folderKey(folderId: string): string {
  return `${encodeURIComponent(folderId)}-${createHash('sha256').update(folderId).digest('hex').slice(0, 8)}`;
}

function durableRoot(ctx: ToolContext): string {
  return path.join(ctx.config.dataDir, 'process-sessions');
}

function durablePaths(ctx: ToolContext, folderId: string, sessionId: string): DurablePaths {
  const dir = path.join(durableRoot(ctx), folderKey(folderId), sessionId);
  return {
    dir,
    snapshot: path.join(dir, 'session.json'),
    manifest: path.join(dir, 'launch.json'),
    stdout: path.join(dir, 'stdout.log'),
    stderr: path.join(dir, 'stderr.log'),
    runtime: path.join(dir, 'runtime.json'),
    result: path.join(dir, 'result.json')
  };
}

async function atomicWriteJson(target: string, value: unknown): Promise<void> {
  const temp = `${target}.${process.pid}.${Date.now()}.tmp`;
  await writeFile(temp, `${JSON.stringify(value)}\n`, { encoding: 'utf8', mode: 0o600 });
  await rename(temp, target);
}

async function readJson<T>(file: string): Promise<T | undefined> {
  try { return JSON.parse(await readFile(file, 'utf8')) as T; }
  catch { return undefined; }
}

function snapshotFor(session: ProcessSession): DurableSessionSnapshot {
  return {
    version: DURABLE_SCHEMA_VERSION,
    id: session.id,
    folderId: session.folderId,
    workspacePath: session.workspacePath,
    operationId: session.operationId,
    fingerprint: session.fingerprint,
    command: session.command,
    program: session.program,
    argv: [...session.argv],
    shell: session.shell,
    cwd: session.cwd,
    startupDiagnostics: session.startupDiagnostics,
    startedAt: session.startedAt,
    timeoutContract: session.timeoutContract,
    processDeadlineMs: session.processDeadlineMs,
    firstOutputAt: session.firstOutputAt,
    endedAt: session.endedAt,
    finalizedAt: session.finalizedAt,
    exitCode: session.exitCode,
    signal: session.signal,
    terminationReason: session.terminationReason,
    verificationOk: session.verificationOk,
    stdoutBytes: session.stdoutBytes,
    stderrBytes: session.stderrBytes,
    sequence: session.sequence,
    workerPid: session.durableWorkerPid,
    processPid: session.processId,
    telemetryCommandKind: session.telemetryCommandKind,
    testRunnerCapability: session.testRunnerCapability,
    testWorkflow: session.testWorkflow,
    telemetryRecorded: session.telemetryRecorded,
    resourceLockGroup: session.resourceLockGroup,
    resourceLockTarget: session.resourceLockTarget,
    operationLockWaitMs: session.operationLockWaitMs,
    resourceLockWaitMs: session.resourceLockWaitMs
  };
}

export async function persistDurableSession(session: ProcessSession): Promise<void> {
  if (!session.durableDirectory) return;
  await atomicWriteJson(path.join(session.durableDirectory, 'session.json'), snapshotFor(session));
}

async function readRange(file: string, offset: number, length: number): Promise<Buffer> {
  if (length <= 0) return Buffer.alloc(0);
  const handle = await open(file, 'r');
  try {
    const buffer = Buffer.alloc(length);
    const { bytesRead } = await handle.read(buffer, 0, length, offset);
    return buffer.subarray(0, bytesRead);
  } finally {
    await handle.close();
  }
}

async function hydrateRetainedTail(ctx: ToolContext, session: ProcessSession, paths: DurablePaths, snapshot: DurableSessionSnapshot): Promise<void> {
  const maxBytes = ctx.config.limits.maxOutputBytes;
  for (const [stream, file, persistedBytes] of [
    ['stdout', paths.stdout, snapshot.stdoutBytes] as const,
    ['stderr', paths.stderr, snapshot.stderrBytes] as const
  ]) {
    let size = 0;
    try { size = (await stat(file)).size; } catch { /* empty */ }
    const boundedBytes = Math.min(Math.max(0, persistedBytes), size);
    const start = Math.max(0, boundedBytes - maxBytes);
    const tail = await readRange(file, start, boundedBytes - start);
    if (stream === 'stdout') {
      session.stdout = tail.toString('utf8');
      session.stdoutBytes = boundedBytes;
      session.stdoutStart = start;
    } else {
      session.stderr = tail.toString('utf8');
      session.stderrBytes = boundedBytes;
      session.stderrStart = start;
    }
  }
}

async function syncStream(ctx: ToolContext, session: ProcessSession, stream: 'stdout' | 'stderr', file: string): Promise<boolean> {
  let size: number;
  try { size = (await stat(file)).size; } catch { return false; }
  const offset = stream === 'stdout' ? session.stdoutBytes : session.stderrBytes;
  if (size <= offset) return false;
  const chunk = await readRange(file, offset, Math.min(size - offset, DURABLE_READ_CHUNK_BYTES));
  if (!chunk.length) return false;
  appendOutput(ctx, session, stream, chunk);
  return true;
}

function pidAlive(pid: number | undefined): boolean {
  if (!pid || pid <= 0) return false;
  try { process.kill(pid, 0); return true; }
  catch { return false; }
}

async function syncDurableSession(ctx: ToolContext, session: ProcessSession): Promise<DurableWorkerResult | undefined> {
  if (!session.durableDirectory) return undefined;
  const paths = durablePaths(ctx, session.folderId, session.id);
  const runtime = await readJson<DurableWorkerRuntime>(paths.runtime);
  let runtimeChanged = false;
  if (runtime?.workerPid && session.durableWorkerPid !== runtime.workerPid) {
    session.durableWorkerPid = runtime.workerPid;
    runtimeChanged = true;
  }
  if (runtime?.processPid && session.processId !== runtime.processPid) {
    session.processId = runtime.processPid;
    runtimeChanged = true;
  }
  const stdoutChanged = await syncStream(ctx, session, 'stdout', paths.stdout);
  const stderrChanged = await syncStream(ctx, session, 'stderr', paths.stderr);
  const result = await readJson<DurableWorkerResult>(paths.result);
  if ((stdoutChanged || stderrChanged || runtimeChanged) && !result) await persistDurableSession(session);
  if (result) return result;
  if (session.durableWorkerPid && !pidAlive(session.durableWorkerPid)) {
    return {
      version: DURABLE_SCHEMA_VERSION,
      exitCode: null,
      signal: null,
      terminationReason: session.terminationReason === 'killed' ? 'killed' : 'crashed',
      error: 'Durable worker exited without writing a result.',
      endedAt: Date.now()
    };
  }
  return undefined;
}

export function monitorDurableSession(
  ctx: ToolContext,
  session: ProcessSession,
  onResult: (result: DurableWorkerResult) => Promise<void>
): void {
  if (session.durableMonitor) clearInterval(session.durableMonitor);
  let completing = false;
  const tick = async () => {
    if (completing || session.finalizedAt) return;
    const result = await syncDurableSession(ctx, session);
    if (!result) return;
    completing = true;
    if (session.durableMonitor) clearInterval(session.durableMonitor);
    session.durableMonitor = undefined;
    await syncStream(ctx, session, 'stdout', path.join(session.durableDirectory!, 'stdout.log'));
    await syncStream(ctx, session, 'stderr', path.join(session.durableDirectory!, 'stderr.log'));
    await onResult(result);
  };
  session.durableMonitor = setInterval(() => { void tick().catch(() => { /* retry */ }); }, DURABLE_MONITOR_MS);
  session.durableMonitor.unref();
  void tick().catch(() => { /* retry */ });
}

export async function launchDurableWorker(
  ctx: ToolContext,
  session: ProcessSession,
  launch: { program: string; args: string[]; cwd: string; shell: boolean; windowsVerbatimArguments?: boolean; environment: NodeJS.ProcessEnv }
): Promise<void> {
  const paths = durablePaths(ctx, session.folderId, session.id);
  await mkdir(paths.dir, { recursive: true, mode: 0o700 });
  await Promise.all([
    writeFile(paths.stdout, '', { flag: 'a', mode: 0o600 }),
    writeFile(paths.stderr, '', { flag: 'a', mode: 0o600 })
  ]);
  const manifest: DurableLaunchManifest = {
    version: DURABLE_SCHEMA_VERSION,
    program: launch.program,
    args: [...launch.args],
    cwd: launch.cwd,
    shell: launch.shell,
    windowsVerbatimArguments: launch.windowsVerbatimArguments,
    deadlineMs: session.processDeadlineMs ?? Date.now() + 30_000,
    stdoutPath: paths.stdout,
    stderrPath: paths.stderr,
    runtimePath: paths.runtime,
    resultPath: paths.result
  };
  await atomicWriteJson(paths.manifest, manifest);
  session.durableDirectory = paths.dir;
  const workerEntry = fileURLToPath(new URL('./durableWorker.js', import.meta.url));
  const worker = spawn(process.execPath, [workerEntry, paths.manifest], {
    cwd: launch.cwd,
    env: launch.environment,
    detached: true,
    windowsHide: true,
    stdio: 'ignore'
  });
  session.durableWorkerPid = worker.pid;
  worker.unref();
  const startupDeadline = Date.now() + 2_000;
  while (Date.now() < startupDeadline) {
    const runtime = await readJson<DurableWorkerRuntime>(paths.runtime);
    if (runtime?.processPid) {
      session.durableWorkerPid = runtime.workerPid;
      session.processId = runtime.processPid;
      await persistDurableSession(session);
      return;
    }
    const result = await readJson<DurableWorkerResult>(paths.result);
    if (result?.error) throw new Error(result.error);
    if (!pidAlive(worker.pid)) throw new Error('Durable worker exited before registering the managed process.');
    await new Promise(resolve => setTimeout(resolve, 20));
  }
  throw new Error('Durable worker did not register the managed process within 2000 ms.');
}

function restoredSession(snapshot: DurableSessionSnapshot, dir: string): ProcessSession {
  return {
    id: snapshot.id,
    folderId: snapshot.folderId,
    workspacePath: snapshot.workspacePath,
    operationId: snapshot.operationId,
    fingerprint: snapshot.fingerprint,
    command: snapshot.command,
    program: snapshot.program,
    argv: [...snapshot.argv],
    shell: snapshot.shell,
    cwd: snapshot.cwd,
    startupDiagnostics: snapshot.startupDiagnostics,
    startedAt: snapshot.startedAt,
    timeoutContract: snapshot.timeoutContract,
    processDeadlineMs: snapshot.processDeadlineMs,
    firstOutputAt: snapshot.firstOutputAt,
    endedAt: snapshot.endedAt,
    finalizedAt: snapshot.finalizedAt,
    exitCode: snapshot.exitCode,
    signal: snapshot.signal,
    stdout: '',
    stderr: '',
    stdoutBytes: 0,
    stderrBytes: 0,
    stdoutStart: 0,
    stderrStart: 0,
    sequence: snapshot.sequence,
    outputEvents: [],
    outputEventBytes: 0,
    processId: snapshot.processPid,
    interactive: false,
    stdinOpen: false,
    timedOut: snapshot.terminationReason === 'process_timeout',
    killed: snapshot.terminationReason === 'killed',
    sandboxEnforced: false,
    executionBoundary: 'durable_worker',
    processTreeContained: true,
    processTreeControl: process.platform === 'win32' ? 'durable_worker_taskkill_tree' : 'durable_worker_process_group',
    terminationReason: snapshot.terminationReason,
    telemetryCommandKind: snapshot.telemetryCommandKind,
    testRunnerCapability: snapshot.testRunnerCapability,
    testWorkflow: snapshot.testWorkflow,
    telemetryRecorded: snapshot.telemetryRecorded,
    sensitiveOutput: false,
    postChecks: [],
    postChecksPending: false,
    verificationOk: snapshot.verificationOk,
    resourceLockGroup: snapshot.resourceLockGroup,
    resourceLockTarget: snapshot.resourceLockTarget,
    operationLockWaitMs: snapshot.operationLockWaitMs,
    resourceLockWaitMs: snapshot.resourceLockWaitMs,
    attachmentGeneration: 1,
    detachedGeneration: 0,
    events: new EventEmitter(),
    durableDirectory: dir,
    durableWorkerPid: snapshot.workerPid
  };
}

export async function restoreDurableSessions(ctx: ToolContext): Promise<ProcessSession[]> {
  const root = durableRoot(ctx);
  const restored: ProcessSession[] = [];
  let folderDirs: string[] = [];
  try { folderDirs = await readdir(root); } catch { return restored; }
  const runtimeByFolder = new Map([...ctx.folderRuntimes.values()].map(runtime => [folderKey(runtime.folderId), runtime]));
  for (const folderDir of folderDirs) {
    const runtime = runtimeByFolder.get(folderDir);
    if (!runtime) continue;
    let sessionDirs: string[] = [];
    try { sessionDirs = await readdir(path.join(root, folderDir)); } catch { continue; }
    for (const sessionId of sessionDirs) {
      const paths = durablePaths(ctx, runtime.folderId, sessionId);
      const snapshot = await readJson<DurableSessionSnapshot>(paths.snapshot);
      if (!snapshot || snapshot.version !== DURABLE_SCHEMA_VERSION || snapshot.id !== sessionId) continue;
      const session = restoredSession(snapshot, paths.dir);
      await hydrateRetainedTail(ctx, session, paths, snapshot);
      if (!session.finalizedAt && session.resourceLockGroup) {
        session.lockRelease = await runtime.admission.locks.acquire([`process:${session.resourceLockGroup}`]);
      }
      runtime.sessions.set(session.id, session);
      runtime.operationsByFingerprint.set(session.fingerprint, session.id);
      restored.push(session);
    }
  }
  return restored;
}

export async function removeDurableSessionArtifacts(session: ProcessSession): Promise<void> {
  if (!session.durableDirectory) return;
  if (session.durableMonitor) clearInterval(session.durableMonitor);
  session.durableMonitor = undefined;
  await rm(session.durableDirectory, { recursive: true, force: true, maxRetries: 3, retryDelay: 50 });
}

export function durableWorkerAlive(session: ProcessSession): boolean {
  return pidAlive(session.durableWorkerPid);
}
