import { createHash, randomUUID } from 'node:crypto';
import { lstat, readFile, readdir, readlink, realpath } from 'node:fs/promises';
import path from 'node:path';
import type {
  ChangeSet, JsonObject, OperationRecord, ProjectBaseline, TaskEvent, TaskRecord, TaskStatus, ToolContext
} from './types.js';
import { attachHarnessOperation } from './processes.js';
import { runGitBuffered } from './gitProcess.js';
import { worktreeContainerPrefixesFromPorcelain } from './gitWorktreeBoundaries.js';
import { operationResultSummary } from './operationSummary.js';
import { relativeInside, resolveInside, rootAndCwd, selectedFolder } from './workspace.js';
import { fastContainingGitRoot, fastWorkspaceGitIdentity } from './runtimeRevision.js';

const BASELINE_SKIPPED_NAMES = new Set([
  '.git', '.mcp-probe-kit', 'node_modules', 'target', 'dist', 'build', '.svelte-kit'
]);
const BASELINE_READ_CONCURRENCY = 16;

export class HarnessError extends Error {
  readonly code: string;
  readonly category = 'permission';
  readonly retryable: boolean;
  readonly details: JsonObject;

  constructor(code: string, message: string, retryable = false, details: JsonObject = {}) {
    super(message);
    this.name = 'HarnessError';
    this.code = code;
    this.retryable = retryable;
    this.details = details;
  }
}

export interface HarnessTracking {
  workspaceId?: string;
  taskId?: string;
  taskRoot?: string;
  operation?: OperationRecord;
  baselineChecked?: boolean;
  baselineCaptureMs?: number;
  baselineBeforeWrite?: ProjectBaseline;
}

function elapsedPhaseMs(startedAt: number): number {
  return Math.max(0, Math.round(performance.now() - startedAt));
}

function addPhaseDuration(result: JsonObject, phase: string, durationMs: number): void {
  const phases = result.phase_durations_ms && typeof result.phase_durations_ms === 'object' && !Array.isArray(result.phase_durations_ms)
    ? { ...result.phase_durations_ms as JsonObject }
    : {};
  const previous = Number(phases[phase] ?? 0);
  phases[phase] = Math.max(0, Math.round(Number.isFinite(previous) ? previous : 0)) + Math.max(0, Math.round(durationMs));
  result.phase_durations_ms = phases;
}

const workspaceIdCache = new Map<string, Promise<string>>();

function now(): string { return String(Date.now()); }

function rustCanonicalPath(value: string): string {
  if (process.platform !== 'win32') return value;
  if (value.startsWith('\\\\?\\')) return value;
  if (value.startsWith('\\\\')) return `\\\\?\\UNC\\${value.slice(2)}`;
  return /^[A-Za-z]:\\/.test(value) ? `\\\\?\\${value}` : value;
}

export async function harnessWorkspaceId(root: string): Promise<string> {
  let pending = workspaceIdCache.get(root);
  if (!pending) {
    pending = realpath(root).then(resolved => createHash('sha256')
      .update(rustCanonicalPath(resolved))
      .digest('hex')
      .slice(0, 32));
    workspaceIdCache.set(root, pending);
  }
  return pending;
}

async function workspaceContext(ctx: ToolContext, key: string) {
  const folder = selectedFolder(ctx, key);
  const workspaceId = await harnessWorkspaceId(folder.path);
  await ctx.state.migrateWorkspace(folder.id, workspaceId);
  const { cwd } = rootAndCwd(ctx, key);
  const workspaceRoot = await realpath(folder.path).catch(() => path.resolve(folder.path));
  const fastTaskRoot = await fastContainingGitRoot(workspaceRoot, cwd);
  let taskRoot = fastTaskRoot ?? workspaceRoot;
  if (!fastTaskRoot) {
    const topLevel = await git(cwd, ['rev-parse', '--show-toplevel']);
    if (topLevel.code === 0 && topLevel.stdout.trim()) {
      const candidate = await realpath(topLevel.stdout.trim()).catch(() => path.resolve(topLevel.stdout.trim()));
      const relative = path.relative(workspaceRoot, candidate);
      if (!relative || (!relative.startsWith('..') && !path.isAbsolute(relative))) taskRoot = candidate;
    }
  }
  const scopeId = path.relative(workspaceRoot, taskRoot) === '' ? workspaceId : await harnessWorkspaceId(taskRoot);
  return { folder, workspaceId, scopeId, taskRoot };
}

function taskRootFor(task: TaskRecord, fallback: string): string {
  return task.scope_root || fallback;
}

function comparePaths(left: string, right: string): number {
  return Buffer.compare(Buffer.from(left), Buffer.from(right));
}

function event(
  workspaceId: string,
  taskId: string,
  kind: string,
  input: JsonObject = {},
  result: JsonObject = { ok: true },
  toolName?: string
): TaskEvent {
  return {
    id: randomUUID().replaceAll('-', ''),
    task_id: taskId,
    operation_id: randomUUID().replaceAll('-', ''),
    kind,
    ...(toolName ? { tool_name: toolName } : {}),
    input_summary: { workspace_id: workspaceId, payload: input },
    result_summary: result,
    affected_files: [],
    created_at: now()
  };
}

async function git(cwd: string, args: string[]): Promise<{ code: number | null; stdout: string; stderr: string }> {
  return runGitBuffered(cwd, args).catch(error => ({ code: 1, stdout: '', stderr: String(error) }));
}

async function gitIdentity(root: string): Promise<Pick<ProjectBaseline, 'branch' | 'head'>> {
  const fast = await fastWorkspaceGitIdentity(root);
  if (fast) return fast;
  const [branch, head] = await Promise.all([
    git(root, ['rev-parse', '--abbrev-ref', 'HEAD']),
    git(root, ['rev-parse', 'HEAD'])
  ]);
  const branchValue = branch.code === 0 && branch.stdout.trim() ? branch.stdout.trim() : undefined;
  const headValue = head.code === 0 && head.stdout.trim() ? head.stdout.trim() : undefined;
  return {
    ...(branchValue ? { branch: branchValue } : {}),
    ...(headValue ? { head: headValue } : {})
  };
}

function gitPathList(output: string): string[] {
  return output
    .split('\0')
    .map(value => value.replaceAll('\\', '/'))
    .filter(value => value.length > 0 && !value.endsWith('/'));
}

async function gitBaselinePaths(root: string): Promise<string[] | undefined> {
  const [tracked, untracked, worktrees] = await Promise.all([
    git(root, ['ls-files', '--cached', '-z']),
    git(root, ['ls-files', '--others', '--exclude-standard', '-z']),
    git(root, ['worktree', 'list', '--porcelain', '-z'])
  ]);
  if (tracked.code !== 0 || untracked.code !== 0) return undefined;
  const excludedPrefixes = worktrees.code === 0
    ? worktreeContainerPrefixesFromPorcelain(root, worktrees.stdout)
    : [];
  const outsideManagedWorktreeContainer = (relative: string): boolean => !excludedPrefixes.some(prefix =>
    relative === prefix || relative.startsWith(`${prefix}/`)
  );
  return [...new Set([
    ...gitPathList(tracked.stdout),
    ...gitPathList(untracked.stdout).filter(outsideManagedWorktreeContainer)
  ])].sort(comparePaths);
}

async function baselineEntry(root: string, relative: string): Promise<ProjectBaseline['entries'][number] | undefined> {
  const full = path.join(root, relative);
  try {
    const metadata = await lstat(full);
    const bytes = metadata.isSymbolicLink()
      ? Buffer.from((await readlink(full)).replaceAll('\\', '/'))
      : metadata.isFile() ? await readFile(full) : undefined;
    if (!bytes) return undefined;
    return {
      path: relative.replaceAll('\\', '/'),
      exists: true,
      is_binary: metadata.isSymbolicLink() ? false : bytes.includes(0),
      sha256: createHash('sha256').update(bytes).digest('hex'),
      bytes: bytes.length
    };
  } catch {
    return undefined;
  }
}

async function baselineEntriesForPaths(root: string, paths: string[]): Promise<ProjectBaseline['entries']> {
  if (!paths.length) return [];
  const results = new Array<ProjectBaseline['entries'][number] | undefined>(paths.length);
  let nextIndex = 0;
  const worker = async (): Promise<void> => {
    while (nextIndex < paths.length) {
      const index = nextIndex;
      nextIndex += 1;
      results[index] = await baselineEntry(root, paths[index]);
    }
  };
  const workerCount = Math.min(BASELINE_READ_CONCURRENCY, paths.length);
  await Promise.all(Array.from({ length: workerCount }, () => worker()));
  return results.filter((entry): entry is ProjectBaseline['entries'][number] => Boolean(entry));
}

async function baselineEntries(root: string): Promise<ProjectBaseline['entries']> {
  const gitPaths = await gitBaselinePaths(root);
  if (gitPaths) return baselineEntriesForPaths(root, gitPaths);

  const paths: string[] = [];
  async function visit(directory: string, relative: string): Promise<void> {
    let children;
    try { children = await readdir(directory, { withFileTypes: true }); }
    catch { return; }
    children.sort((left, right) => comparePaths(left.name, right.name));
    for (const child of children) {
      if (BASELINE_SKIPPED_NAMES.has(child.name)) continue;
      const childRelative = relative ? `${relative}/${child.name}` : child.name;
      const full = path.join(directory, child.name);
      if (child.isDirectory()) {
        await visit(full, childRelative);
        continue;
      }
      if (child.isFile()) paths.push(childRelative.replaceAll('\\', '/'));
    }
  }
  await visit(root, '');
  paths.sort(comparePaths);
  return baselineEntriesForPaths(root, paths);
}

function worktreeFingerprint(entries: ProjectBaseline['entries']): string {
  const fingerprint = createHash('sha256');
  for (const entry of entries) {
    fingerprint.update(Buffer.from(entry.path));
    fingerprint.update(Buffer.from(entry.sha256));
    const size = Buffer.alloc(8);
    size.writeBigUInt64LE(BigInt(entry.bytes));
    fingerprint.update(size);
  }
  return fingerprint.digest('hex');
}

export async function captureBaseline(root: string): Promise<ProjectBaseline> {
  const [entries, identity] = await Promise.all([
    baselineEntries(root),
    gitIdentity(root)
  ]);
  return {
    ...identity,
    worktree_fingerprint: worktreeFingerprint(entries),
    entries,
    captured_at: now()
  };
}

function requireTask(ctx: ToolContext, folderId: string, taskId: unknown): TaskRecord {
  const id = String(taskId ?? '').trim();
  if (!id) throw new HarnessError('INVALID_ARGUMENT', 'task_id is required');
  const task = ctx.state.taskById(folderId, id);
  if (!task) throw new HarnessError('TASK_NOT_FOUND', `Task ${id} was not found`);
  return task;
}

function canTransition(from: TaskStatus, to: TaskStatus): boolean {
  return (from === 'active' && ['paused', 'verifying', 'failed', 'completed_unverified'].includes(to))
    || (from === 'paused' && to === 'active')
    || (from === 'verifying' && ['completed', 'completed_unverified', 'failed'].includes(to))
    || (from === 'failed' && ['active', 'failed_final', 'rolled_back'].includes(to));
}

function taskForArgs(ctx: ToolContext, workspaceId: string, scopeId: string, args: JsonObject): TaskRecord | undefined {
  const taskId = String(args.task_id ?? '').trim();
  return taskId ? ctx.state.taskById(workspaceId, taskId) : ctx.state.task(scopeId);
}

function baselineMatches(task: TaskRecord, current: ProjectBaseline): boolean {
  return task.baseline.branch === current.branch
    && task.baseline.head === current.head
    && task.expected_fingerprint === current.worktree_fingerprint;
}

export async function checkTaskBaseline(
  root: string,
  task: TaskRecord
): Promise<{ matches: boolean; current: ProjectBaseline; changes: string[] }> {
  const current = await captureBaseline(root);
  const changes: string[] = [];
  if (current.branch !== task.baseline.branch || current.head !== task.baseline.head) {
    changes.push('git_identity');
  }
  if (current.worktree_fingerprint !== task.expected_fingerprint) {
    changes.push('worktree_fingerprint');
  }
  return { matches: changes.length === 0, current, changes };
}

export async function checkTaskOriginalBaseline(
  root: string,
  task: TaskRecord
): Promise<{ matches: boolean; current: ProjectBaseline; changes: string[] }> {
  const current = await captureBaseline(root);
  const changes: string[] = [];
  if (current.branch !== task.baseline.branch || current.head !== task.baseline.head) {
    changes.push('git_identity');
  }
  if (current.worktree_fingerprint !== task.baseline.worktree_fingerprint) {
    changes.push('worktree_fingerprint');
  }
  return { matches: changes.length === 0, current, changes };
}

export async function refreshExpectedState(ctx: ToolContext, folderId: string, root: string, taskId: string): Promise<TaskRecord> {
  const task = requireTask(ctx, folderId, taskId);
  const current = await captureBaseline(root);
  task.baseline.branch = current.branch;
  task.baseline.head = current.head;
  task.expected_fingerprint = current.worktree_fingerprint;
  task.updated_at = now();
  await ctx.state.setTask(task.scope_id ?? folderId, task);
  return task;
}

async function harnessStatusValue(
  ctx: ToolContext,
  key: string,
  mode: 'full' | 'cached' = 'full'
): Promise<JsonObject> {
  const { workspaceId, scopeId, taskRoot } = await workspaceContext(ctx, key);
  const task = ctx.state.task(scopeId);
  const baselineCheckEnabled = ctx.config.securityPolicy.enforceHarnessBaseline;
  const baselineCheckPerformed = mode === 'full' && baselineCheckEnabled && Boolean(task);
  let baselineCaptureMs = 0;
  let current: ProjectBaseline | undefined;
  if (baselineCheckPerformed) {
    const baselineStartedAt = performance.now();
    current = await captureBaseline(taskRoot);
    baselineCaptureMs = elapsedPhaseMs(baselineStartedAt);
  }
  const identity = current
    ?? (task ? { branch: task.baseline.branch, head: task.baseline.head } : mode === 'full' ? await gitIdentity(taskRoot) : {});
  const matches = task && current ? baselineMatches(task, current) : undefined;
  const writable = task ? ['active', 'paused', 'verifying', 'failed'].includes(task.status) : true;
  const taskId = task?.id;
  const taskCapabilityReason = !baselineCheckEnabled
    ? '自动基线检查已关闭；活动任务仍会记录操作事件'
    : !baselineCheckPerformed
      ? '当前响应使用缓存状态，未执行工作区基线扫描'
      : matches === false
        ? '工作区基线已变化；Harness 只记录漂移，下一次写入或执行会自动采用当前状态'
        : '活动任务可继续执行';
  const capabilities = {
    read: { status: 'available', reason: '工作区读取不依赖活动任务', recoverable: true },
    write: {
      status: writable ? 'available' : 'denied',
      reason: writable
        ? taskId ? taskCapabilityReason : '无任务模式允许直接修改，建议需要长期追踪时调用 start_task'
        : '当前任务状态不可写；基线漂移本身不会阻挡操作',
      recoverable: true
    },
    exec: {
      status: writable ? 'available' : 'denied',
      reason: writable
        ? taskId ? taskCapabilityReason : '无任务模式允许直接执行，建议需要长期追踪时调用 start_task'
        : '当前任务状态不可执行；基线漂移本身不会阻挡操作',
      recoverable: true
    },
    git: {
      status: identity.branch && identity.head ? 'available' : 'degraded',
      reason: identity.branch && identity.head ? '已读取当前分支和 HEAD' : '当前工作区不是可读取 Git 状态的仓库',
      recoverable: true
    },
    network: { status: 'managed_by_policy', reason: '网络权限由工具策略控制，不由 Harness 任务状态决定', recoverable: true }
  };
  const nextActions: string[] = [];
  if (!taskId) nextActions.push('start_task');
  else if (matches === false) nextActions.push('project_state', 'git_diff');
  if (taskId && !writable) nextActions.push('resume_task');
  nextActions.push('read_file', 'git_status');
  const status: JsonObject = {
    schema_version: 1,
    workspace_id: workspaceId,
    scope_id: scopeId,
    scope_root: taskRoot,
    task_id: taskId ?? null,
    task_state: task?.status ?? null,
    task_updated_at: task?.updated_at ?? null,
    writable,
    reason: task
      ? !baselineCheckEnabled
        ? '任务可继续执行；自动基线检查已关闭，任务与操作记录仍会保留'
        : !baselineCheckPerformed
          ? '任务状态可继续执行；当前响应未执行工作区基线扫描'
          : matches ? '任务可继续执行' : '工作区基线已变化；漂移只会记录，不会暂停写入或执行'
      : '当前没有活动任务，工作区采用无任务模式；修改不会进入任务事件流',
    recoverable: true,
    baseline_check_enabled: baselineCheckEnabled,
    baseline_check_performed: baselineCheckPerformed,
    branch: identity.branch ?? null,
    head: identity.head ?? null,
    baseline_matches: matches ?? null,
    capabilities,
    next_actions: nextActions
  };
  if (baselineCheckPerformed) addPhaseDuration(status, 'baseline_capture_ms', baselineCaptureMs);
  return status;
}

export async function harnessStatus(ctx: ToolContext, key: string): Promise<JsonObject> {
  return { ok: true, ...await harnessStatusValue(ctx, key, 'full') };
}

export async function attachHarnessStatus(
  ctx: ToolContext,
  key: string,
  result: JsonObject,
  standalone: boolean,
  exposedTools?: Set<string>
): Promise<JsonObject> {
  const enrichmentStartedAt = performance.now();
  try {
    const status = await harnessStatusValue(ctx, key, 'cached');
    if (standalone && status.task_id === null) status.next_actions = [];
    if (exposedTools && Array.isArray(status.next_actions)) {
      status.next_actions = status.next_actions.filter(action => exposedTools.has(String(action)));
    }
    result.harness = status;
    if (standalone) {
      result.harness_mode = 'standalone';
      result.task_required = false;
      if (!Array.isArray(result.next_actions)) result.next_actions = [];
      result.recovery_hint = '命令未成功；请检查 stderr、exit_code 或调整参数后重试。';
    }
  } catch {
    result.harness = { status: 'unavailable', reason: '无法序列化 Harness 状态' };
  }
  addPhaseDuration(result, 'error_enrichment_ms', elapsedPhaseMs(enrichmentStartedAt));
  return result;
}

export async function operationLog(ctx: ToolContext, key: string, args: JsonObject): Promise<JsonObject> {
  const { workspaceId } = await workspaceContext(ctx, key);
  const cursor = Math.max(0, Math.trunc(Number(args.cursor ?? 0)) || 0);
  const limit = Math.max(1, Math.min(200, Math.trunc(Number(args.limit ?? 50)) || 50));
  const order = String(args.order ?? 'desc').trim().toLowerCase();
  if (!['asc', 'desc'].includes(order)) throw new HarnessError('INVALID_ARGUMENT', 'operation_log order must be asc or desc');
  const tool = typeof args.tool === 'string' && args.tool.trim() ? args.tool.trim() : undefined;
  const kind = typeof args.kind === 'string' && args.kind.trim() ? args.kind.trim() : undefined;
  const errorsOnly = args.errors_only === true;
  const sinceTsMs = Number.isFinite(Number(args.since_ts_ms)) ? Math.max(0, Number(args.since_ts_ms)) : undefined;
  const all = await ctx.state.listOperations(workspaceId, 0, Number.MAX_SAFE_INTEGER);
  const filtered = all.filter(operation => {
    const createdAt = Number(operation.created_at);
    return (!tool || operation.tool === tool)
      && (!kind || operation.kind === kind)
      && (sinceTsMs === undefined || (Number.isFinite(createdAt) && createdAt >= sinceTsMs))
      && (!errorsOnly || operation.kind === 'failed' || operation.result_summary?.ok === false);
  });
  if (order === 'desc') filtered.reverse();
  const totalMatching = filtered.length;
  const operations = filtered.slice(cursor, cursor + limit);
  const nextOffset = cursor + operations.length;
  const hasMore = nextOffset < totalMatching;
  return {
    ok: true,
    operations,
    order,
    filters: { tool: tool ?? null, kind: kind ?? null, errors_only: errorsOnly, since_ts_ms: sinceTsMs ?? null },
    total_matching: totalMatching,
    has_more: hasMore,
    next_cursor: hasMore ? nextOffset : null
  };
}

export async function projectState(ctx: ToolContext, key: string, args: JsonObject = {}): Promise<JsonObject> {
  const { workspaceId, scopeId, taskRoot } = await workspaceContext(ctx, key);
  const maxFiles = Math.max(1, Math.min(10_000, Math.trunc(Number(args.max_files ?? 200)) || 200));
  const baselineStartedAt = performance.now();
  const current = await captureBaseline(taskRoot);
  const baselineCaptureMs = elapsedPhaseMs(baselineStartedAt);
  const task = ctx.state.task(scopeId);
  const baselineMap = new Map((task?.baseline.entries ?? []).map(entry => [entry.path, entry]));
  const currentMap = new Map(current.entries.map(entry => [entry.path, entry]));
  const paths = [...new Set([...baselineMap.keys(), ...currentMap.keys()])].sort(comparePaths);
  const allFiles = paths.map(filePath => {
    const before = baselineMap.get(filePath);
    const entry = currentMap.get(filePath);
    const status = before && entry
      ? before.sha256 === entry.sha256 ? 'unchanged' : 'modified'
      : before ? 'deleted' : entry ? 'added' : 'unknown';
    return {
      path: filePath,
      status,
      sha256: entry?.sha256 ?? '',
      bytes: entry?.bytes ?? 0
    };
  });
  const clean = allFiles.every(file => file.status === 'unchanged');
  const files = allFiles.slice(0, maxFiles);
  return {
    ok: true,
    schema_version: 1,
    workspace_id: workspaceId,
    scope_id: scopeId,
    scope_root: taskRoot,
    branch: current.branch ?? null,
    head: current.head ?? null,
    clean,
    files,
    total_files: allFiles.length,
    truncated: allFiles.length > maxFiles,
    active_task_id: task?.id ?? null,
    task: task ?? null,
    recent_events: task ? ctx.state.taskEvents(task.id).slice(0, 100).length : 0,
    phase_durations_ms: { baseline_capture_ms: baselineCaptureMs }
  };
}

function changeFiles(task: TaskRecord, current: ProjectBaseline): JsonObject[] {
  const baselineMap = new Map(task.baseline.entries.map(entry => [entry.path, entry]));
  const currentMap = new Map(current.entries.map(entry => [entry.path, entry]));
  const paths = [...new Set([...baselineMap.keys(), ...currentMap.keys()])].sort(comparePaths);
  return paths.map(filePath => {
    const before = baselineMap.get(filePath);
    const after = currentMap.get(filePath);
    const status = before && after
      ? before.sha256 === after.sha256 ? 'unchanged' : 'modified'
      : before ? 'deleted' : after ? 'added' : 'unknown';
    return {
      path: filePath,
      status,
      before_sha256: before?.sha256 ?? null,
      after_sha256: after?.sha256 ?? null
    };
  }).filter(file => file.status !== 'unchanged');
}

function finishReason(task: TaskRecord, args: JsonObject): { text: string; source: string } {
  if (args.summary === undefined) return { text: task.objective, source: 'task_objective' };
  if (typeof args.summary !== 'string') throw new HarnessError('INVALID_ARGUMENT', 'summary must be a string');
  const summary = args.summary.trim();
  if (!summary) throw new HarnessError('INVALID_ARGUMENT', 'summary must not be empty');
  return { text: summary, source: 'finish_task_summary' };
}

function normalizedChangeId(value: unknown): string | undefined {
  if (value === undefined) return undefined;
  if (typeof value !== 'string') throw new HarnessError('INVALID_ARGUMENT', 'change_id must be a string');
  const changeId = value.trim();
  if (!/^[0-9a-f]{32}$/.test(changeId)) throw new HarnessError('INVALID_ARGUMENT', 'change_id must be a 32-character lowercase hexadecimal ID');
  return changeId;
}

function normalizedOptionalString(value: unknown, field: string): string | undefined {
  if (value === undefined) return undefined;
  if (typeof value !== 'string') throw new HarnessError('INVALID_ARGUMENT', `${field} must be a string`);
  const normalized = value.trim();
  if (!normalized) throw new HarnessError('INVALID_ARGUMENT', `${field} must not be empty`);
  return normalized;
}

function changeSummaryValue(task: TaskRecord, change: ChangeSet | undefined, files: JsonObject[], events: TaskEvent[]): JsonObject {
  const reason = change?.reason ?? { text: task.objective, source: 'task_objective' };
  const commandIds = new Set(change?.command_ids ?? []);
  const evidence = change
    ? events.filter(item => commandIds.has(item.operation_id)).slice(0, 100)
    : events.slice(0, 100);
  return {
    ok: true,
    change_id: change?.id ?? null,
    task_id: task.id,
    objective: task.objective,
    why: reason,
    files,
    evidence,
    verification: change?.verification_ids ?? [],
    risks: change?.risks ?? [],
    rollback_capability: 'not_available_in_foundation'
  };
}

async function finalizeVerifyingTask(ctx: ToolContext, workspaceId: string, task: TaskRecord): Promise<JsonObject> {
  if (task.status !== 'verifying') {
    throw new HarnessError('INVALID_TASK_TRANSITION', `Cannot finalize task from ${task.status}`);
  }
  const changeId = task.latest_change_id;
  if (!changeId) throw new HarnessError('CHANGE_NOT_FOUND', 'Verifying task has no captured change to finalize.');
  const change = ctx.state.changeById(workspaceId, changeId);
  if (!change || change.task_id !== task.id) {
    throw new HarnessError('CHANGE_NOT_FOUND', `Captured change ${changeId} was not found for task ${task.id}.`);
  }
  if (!canTransition(task.status, 'completed')) {
    throw new HarnessError('INVALID_TASK_TRANSITION', `Cannot transition ${task.status} to completed`);
  }
  task.status = 'completed';
  task.updated_at = now();
  const finalizedEvent = event(
    workspaceId,
    task.id,
    'task_status_changed',
    { status: 'completed', change_id: change.id },
    { ok: true, status: 'completed', change_id: change.id }
  );
  await ctx.state.setTask(task.scope_id ?? workspaceId, task, finalizedEvent);
  return {
    ok: true,
    task,
    summary: change.reason.text,
    change_id: change.id,
    change_summary: changeSummaryValue(task, change, change.files, ctx.state.taskEvents(task.id)),
    finalized_existing_change: true,
    phase_durations_ms: { baseline_capture_ms: 0 }
  };
}

export async function startTask(ctx: ToolContext, key: string, args: JsonObject): Promise<JsonObject> {
  const { workspaceId, scopeId, taskRoot } = await workspaceContext(ctx, key);
  const objective = String(args.objective ?? '').trim();
  if (!objective) throw new HarnessError('INVALID_ARGUMENT', 'objective is required');
  const existingTaskMode = String(args.existing_task ?? 'error');
  if (!['error', 'finish_if_complete'].includes(existingTaskMode)) {
    throw new HarnessError('INVALID_ARGUMENT', 'existing_task must be error or finish_if_complete');
  }
  const existing = ctx.state.task(scopeId);
  let recoveredTaskId: string | undefined;
  if (existing && existingTaskMode === 'finish_if_complete'
    && existing.status === 'verifying' && existing.pending_steps.length === 0 && existing.latest_change_id) {
    await finalizeVerifyingTask(ctx, workspaceId, existing);
    recoveredTaskId = existing.id;
  } else if (existing) {
    const recoveryActions: JsonObject[] = [
      {
        action: 'inspect_active_task',
        tool: 'task_context',
        required_arguments: ['task_id'],
        arguments: { task_id: existing.id }
      },
      {
        action: 'finish_active_task',
        tool: 'finish_task',
        required_arguments: ['task_id'],
        arguments: { task_id: existing.id }
      }
    ];
    if (existing.status === 'verifying' && existing.pending_steps.length === 0 && existing.latest_change_id) {
      recoveryActions.push({
        action: 'finalize_completed_verification_and_retry',
        tool: 'start_task',
        required_arguments: ['objective', 'existing_task'],
        arguments: { existing_task: 'finish_if_complete' }
      });
    }
    throw new HarnessError(
      'TASK_ALREADY_ACTIVE',
      `Workspace already has active task ${existing.id}`,
      false,
      {
        retry_mode: 'after_state_change',
        blocking_condition: 'active_task_exists',
        active_task_id: existing.id,
        suggestion: 'Continue or finish the active task before starting another task.',
        recovery_actions: recoveryActions
      }
    );
  }
  const baselineStartedAt = performance.now();
  const baseline = await captureBaseline(taskRoot);
  const baselineCaptureMs = elapsedPhaseMs(baselineStartedAt);
  const timestamp = now();
  const task: TaskRecord = {
    id: randomUUID().replaceAll('-', ''),
    workspace_id: workspaceId,
    scope_id: scopeId,
    scope_root: taskRoot,
    objective,
    status: 'active',
    baseline,
    expected_fingerprint: baseline.worktree_fingerprint,
    completed_steps: [],
    pending_steps: [],
    created_at: timestamp,
    updated_at: timestamp
  };
  await ctx.state.setTask(scopeId, task, event(workspaceId, task.id, 'task_started'));
  return {
    ok: true,
    task,
    next: ['project_state', 'task_context'],
    ...(recoveredTaskId ? { recovered_task_id: recoveredTaskId } : {}),
    phase_durations_ms: { baseline_capture_ms: baselineCaptureMs }
  };
}

export async function updateTask(ctx: ToolContext, key: string, args: JsonObject): Promise<JsonObject> {
  const { workspaceId } = await workspaceContext(ctx, key);
  const task = requireTask(ctx, workspaceId, args.task_id);
  if (args.completed_steps !== undefined && !Array.isArray(args.completed_steps)) throw new HarnessError('INVALID_ARGUMENT', 'completed_steps must be an array');
  if (args.pending_steps !== undefined && !Array.isArray(args.pending_steps)) throw new HarnessError('INVALID_ARGUMENT', 'pending_steps must be an array');
  if (Array.isArray(args.completed_steps)) task.completed_steps = args.completed_steps.map(String);
  if (Array.isArray(args.pending_steps)) task.pending_steps = args.pending_steps.map(String);
  task.updated_at = now();
  await ctx.state.setTask(workspaceId, task, event(workspaceId, task.id, 'task_updated', {
    completed_steps: task.completed_steps,
    pending_steps: task.pending_steps
  }));
  return { ok: true, task };
}

export async function setTaskStatus(ctx: ToolContext, key: string, status: TaskStatus, args: JsonObject): Promise<JsonObject> {
  const { workspaceId } = await workspaceContext(ctx, key);
  const task = requireTask(ctx, workspaceId, args.task_id);
  if (!canTransition(task.status, status)) throw new HarnessError('INVALID_TASK_TRANSITION', `Cannot transition ${task.status} to ${status}`);
  task.status = status;
  task.updated_at = now();
  await ctx.state.setTask(workspaceId, task, event(workspaceId, task.id, 'task_status_changed', { status }));
  return { ok: true, task };
}

export async function failTask(ctx: ToolContext, key: string, args: JsonObject): Promise<JsonObject> {
  return setTaskStatus(ctx, key, 'failed', args);
}

export async function closeFailedTask(ctx: ToolContext, key: string, args: JsonObject): Promise<JsonObject> {
  return setTaskStatus(ctx, key, 'failed_final', args);
}

export async function rollbackTask(ctx: ToolContext, key: string, args: JsonObject): Promise<JsonObject> {
  const { workspaceId, taskRoot } = await workspaceContext(ctx, key);
  const task = requireTask(ctx, workspaceId, args.task_id);
  if (task.status !== 'failed') {
    throw new HarnessError('INVALID_TASK_TRANSITION', `Cannot transition ${task.status} to rolled_back`);
  }
  const check = await checkTaskOriginalBaseline(taskRootFor(task, taskRoot), task);
  if (!check.matches) {
    throw new HarnessError(
      'ROLLBACK_NOT_VERIFIED',
      'Workspace does not match the task baseline; rollback cannot be finalized.',
      true,
      {
        changes: check.changes,
        expected_branch: task.baseline.branch ?? null,
        expected_head: task.baseline.head ?? null,
        expected_fingerprint: task.baseline.worktree_fingerprint,
        current_branch: check.current.branch ?? null,
        current_head: check.current.head ?? null,
        current_fingerprint: check.current.worktree_fingerprint,
        recovery_actions: [
          { action: 'inspect_project_state', tool: 'project_state' },
          { action: 'inspect_workspace_diff', tool: 'git_diff' }
        ]
      }
    );
  }
  task.status = 'rolled_back';
  task.updated_at = now();
  await ctx.state.setTask(workspaceId, task, event(workspaceId, task.id, 'task_status_changed', {
    status: 'rolled_back',
    baseline_verified: true
  }));
  return { ok: true, task, baseline_verified: true };
}

export async function finishTask(ctx: ToolContext, key: string, args: JsonObject): Promise<JsonObject> {
  const { taskRoot, workspaceId } = await workspaceContext(ctx, key);
  const task = requireTask(ctx, workspaceId, args.task_id);
  if (task.status === 'verifying' && args.allow_unverified !== true) {
    return finalizeVerifyingTask(ctx, workspaceId, task);
  }
  const status: TaskStatus = args.allow_unverified === true ? 'completed_unverified' : 'verifying';
  if (!canTransition(task.status, status)) throw new HarnessError('INVALID_TASK_TRANSITION', `Cannot transition ${task.status} to ${status}`);
  const reason = finishReason(task, args);
  const timestamp = now();
  const changeId = randomUUID().replaceAll('-', '');
  const sourceEvents = ctx.state.taskEvents(task.id);
  const finishedEvent = event(
    workspaceId,
    task.id,
    'task_finished',
    { summary: reason.text, status, change_id: changeId },
    { ok: true, status, change_id: changeId }
  );
  finishedEvent.reason = reason;
  finishedEvent.created_at = timestamp;
  const baselineStartedAt = performance.now();
  const current = await captureBaseline(taskRootFor(task, taskRoot));
  const baselineCaptureMs = elapsedPhaseMs(baselineStartedAt);
  const change: ChangeSet = {
    id: changeId,
    task_id: task.id,
    objective: task.objective,
    reason,
    files: changeFiles(task, current),
    command_ids: [...sourceEvents.map(item => item.operation_id), finishedEvent.operation_id],
    verification_ids: [],
    risks: [],
    created_at: timestamp
  };
  task.status = status;
  task.latest_change_id = change.id;
  task.updated_at = timestamp;
  await ctx.state.setTaskAndChange(workspaceId, task, change, finishedEvent);
  return {
    ok: true,
    task,
    summary: reason.text,
    change_id: change.id,
    change_summary: changeSummaryValue(task, change, change.files, [...sourceEvents, finishedEvent]),
    phase_durations_ms: { baseline_capture_ms: baselineCaptureMs }
  };
}

const TASK_CONTEXT_MIN_BYTES = 8_192;
const TASK_CONTEXT_MAX_BYTES = 131_072;
const TASK_CONTEXT_DEFAULT_BYTES = 32_768;
const TASK_CONTEXT_RESULT_METADATA_BYTES = 512;

function boundedTaskContext(task: TaskRecord, sourceEvents: TaskEvent[], maxBytes: number, payloadBudget: number): JsonObject {
  const boundedTask = task;
  let events = sourceEvents.slice(0, 100);
  let truncated = sourceEvents.length > events.length;
  const response = (): JsonObject => ({
    ok: true,
    task: boundedTask,
    events,
    truncated,
    max_bytes: maxBytes
  });
  const serializedBytes = (): number => Buffer.byteLength(JSON.stringify(response()));
  if (serializedBytes() <= payloadBudget) return response();
  truncated = true;

  const fitLargestPrefix = <T>(source: T[], assign: (next: T[]) => void): JsonObject | undefined => {
    let lower = 0;
    let upper = source.length;
    let best = -1;
    while (lower <= upper) {
      const middle = lower + Math.floor((upper - lower) / 2);
      assign(source.slice(0, middle));
      if (serializedBytes() <= payloadBudget) {
        best = middle;
        lower = middle + 1;
      } else {
        upper = middle - 1;
      }
    }
    if (best >= 0) {
      assign(source.slice(0, best));
      return response();
    }
    assign([]);
    return undefined;
  };

  const baselineEntries = boundedTask.baseline.entries;
  const baselineResult = fitLargestPrefix(baselineEntries, next => { boundedTask.baseline.entries = next; });
  if (baselineResult) return baselineResult;

  const sourceEventPrefix = events;
  const eventResult = fitLargestPrefix(sourceEventPrefix, next => { events = next; });
  if (eventResult) return eventResult;

  const pendingSteps = boundedTask.pending_steps;
  const pendingResult = fitLargestPrefix(pendingSteps, next => { boundedTask.pending_steps = next; });
  if (pendingResult) return pendingResult;

  const completedSteps = boundedTask.completed_steps;
  const completedResult = fitLargestPrefix(completedSteps, next => { boundedTask.completed_steps = next; });
  if (completedResult) return completedResult;

  let result = response();
  while (Buffer.byteLength(JSON.stringify(result)) > payloadBudget) {
    const objective = [...boundedTask.objective];
    if (objective.length <= 1) break;
    boundedTask.objective = objective.slice(0, Math.max(1, Math.floor(objective.length / 2))).join('');
    result = response();
  }
  return result;
}

export async function taskContext(ctx: ToolContext, key: string, args: JsonObject): Promise<JsonObject> {
  const { workspaceId, scopeId } = await workspaceContext(ctx, key);
  const maxBytes = Math.max(
    TASK_CONTEXT_MIN_BYTES,
    Math.min(TASK_CONTEXT_MAX_BYTES, Math.trunc(Number(args.max_bytes ?? TASK_CONTEXT_DEFAULT_BYTES)) || TASK_CONTEXT_DEFAULT_BYTES)
  );
  const task = taskForArgs(ctx, workspaceId, scopeId, args);
  if (!task) return { ok: true, task: null, message: '当前没有活动任务', truncated: false, max_bytes: maxBytes };
  const payloadBudget = Math.max(1, maxBytes - TASK_CONTEXT_RESULT_METADATA_BYTES);
  return boundedTaskContext(task, ctx.state.taskEvents(task.id), maxBytes, payloadBudget);
}

export async function listTaskEvents(ctx: ToolContext, key: string, args: JsonObject): Promise<JsonObject> {
  const { workspaceId } = await workspaceContext(ctx, key);
  const task = requireTask(ctx, workspaceId, args.task_id);
  const cursor = Math.max(0, Math.trunc(Number(args.cursor ?? 0)) || 0);
  const limit = Math.max(1, Math.min(200, Math.trunc(Number(args.limit ?? 50)) || 50));
  const events = ctx.state.taskEvents(task.id).slice(cursor, cursor + limit);
  return { ok: true, events, next_cursor: cursor + events.length };
}

export async function changeSummary(ctx: ToolContext, key: string, args: JsonObject = {}): Promise<JsonObject> {
  const { workspaceId, scopeId, taskRoot } = await workspaceContext(ctx, key);
  const requestedTaskId = normalizedOptionalString(args.task_id, 'task_id');
  const requestedChangeId = normalizedChangeId(args.change_id);
  if (requestedChangeId) {
    const change = ctx.state.changeById(workspaceId, requestedChangeId);
    if (!change) throw new HarnessError('CHANGE_NOT_FOUND', `Change ${requestedChangeId} was not found`);
    if (requestedTaskId && requestedTaskId !== change.task_id) {
      throw new HarnessError('CHANGE_TASK_MISMATCH', `Change ${requestedChangeId} does not belong to task ${requestedTaskId}`);
    }
    const task = requireTask(ctx, workspaceId, change.task_id);
    return changeSummaryValue(task, change, change.files, ctx.state.taskEvents(task.id));
  }
  const task = requestedTaskId ? requireTask(ctx, workspaceId, requestedTaskId) : ctx.state.task(scopeId);
  if (!task) throw new HarnessError('TASK_STATE_REQUIRED', 'No active task is available to summarize');
  if (task.latest_change_id) {
    const change = ctx.state.changeById(workspaceId, task.latest_change_id);
    if (!change) throw new HarnessError('CHANGE_NOT_FOUND', `Change ${task.latest_change_id} was not found`);
    return changeSummaryValue(task, change, change.files, ctx.state.taskEvents(task.id));
  }
  const baselineStartedAt = performance.now();
  const current = await captureBaseline(taskRootFor(task, taskRoot));
  const summary = changeSummaryValue(task, undefined, changeFiles(task, current), ctx.state.taskEvents(task.id));
  addPhaseDuration(summary, 'baseline_capture_ms', elapsedPhaseMs(baselineStartedAt));
  return summary;
}

function requiresWriteBaseline(name: string, args: JsonObject): boolean {
  if (name === 'exec_command') return true;
  if (['apply_patch', 'edit', 'edit_file', 'edit_many', 'file_ops'].includes(name)) return args.dry_run !== true;
  if (name === 'format_files') return args.mode === 'apply';
  if (['git_branch', 'git_worktree', 'git_stage', 'git_commit', 'git_restore'].includes(name)) return args.dry_run !== true;
  return false;
}

function standaloneOperation(name: string): boolean {
  return [
    'patch_check', 'apply_patch', 'edit', 'edit_file', 'edit_many', 'file_ops', 'format_files',
    'exec_command', 'git_branch', 'git_worktree', 'git_stage', 'git_commit', 'git_push', 'git_restore'
  ].includes(name);
}

function shouldLogOperation(name: string): boolean {
  return standaloneOperation(name) || [
    'git_status', 'git_diff', 'git_log', 'git_show', 'git_blame',
    'git_branch', 'git_worktree', 'git_stage', 'git_commit', 'git_push', 'git_restore'
  ].includes(name);
}

function operationInput(args: JsonObject): JsonObject {
  return { arguments_present: true, reason: args.reason ?? null };
}

interface EditMutationProof {
  path: string;
  afterSha256: string;
}

function editMutationProofs(name: string, result: JsonObject): EditMutationProof[] | undefined {
  if (!['edit', 'edit_file', 'edit_many'].includes(name) || result.ok !== true) return undefined;
  if (typeof result.path === 'string' && typeof result.after_sha256 === 'string') {
    return [{ path: result.path, afterSha256: result.after_sha256 }];
  }
  if (!Array.isArray(result.results)) return undefined;
  const proofs: EditMutationProof[] = [];
  for (const item of result.results) {
    if (!item || typeof item !== 'object' || Array.isArray(item)) return undefined;
    const value = item as JsonObject;
    if (value.changed !== true) continue;
    if (typeof value.path !== 'string' || typeof value.after_sha256 !== 'string') return undefined;
    proofs.push({ path: value.path, afterSha256: value.after_sha256 });
  }
  return proofs;
}

async function refreshEditExpectedStateIncremental(
  ctx: ToolContext,
  key: string,
  tracking: HarnessTracking,
  result: JsonObject
): Promise<{ refreshed: boolean; fileCount: number }> {
  if (!tracking.taskId || !tracking.workspaceId || !tracking.taskRoot || !tracking.baselineBeforeWrite) {
    return { refreshed: false, fileCount: 0 };
  }
  const proofs = editMutationProofs(String(result.tool ?? ''), result);
  if (proofs === undefined) return { refreshed: false, fileCount: 0 };
  const baseline = tracking.baselineBeforeWrite;
  const entries = new Map(baseline.entries.map(entry => [entry.path, entry]));
  const workspaceRoot = rootAndCwd(ctx, key).root;
  let refreshedFiles = 0;
  for (const proof of proofs) {
    let relative: string;
    try {
      const absolute = resolveInside(workspaceRoot, proof.path);
      relative = relativeInside(tracking.taskRoot, absolute).replaceAll('\\', '/');
    } catch {
      return { refreshed: false, fileCount: 0 };
    }
    if (!relative || relative === '.' || !entries.has(relative)) continue;
    const updated = await baselineEntry(tracking.taskRoot, relative);
    if (!updated || updated.sha256.toLowerCase() !== proof.afterSha256.toLowerCase()) {
      return { refreshed: false, fileCount: 0 };
    }
    entries.set(relative, updated);
    refreshedFiles += 1;
  }
  const ordered = [...entries.values()].sort((left, right) => comparePaths(left.path, right.path));
  const task = requireTask(ctx, tracking.workspaceId, tracking.taskId);
  task.baseline.branch = baseline.branch;
  task.baseline.head = baseline.head;
  task.expected_fingerprint = worktreeFingerprint(ordered);
  task.updated_at = now();
  await ctx.state.setTask(task.scope_id ?? tracking.workspaceId, task);
  return { refreshed: true, fileCount: refreshedFiles };
}

export async function beginHarnessTracking(
  ctx: ToolContext,
  key: string,
  name: string,
  args: JsonObject
): Promise<HarnessTracking> {
  const tracksTask = requiresWriteBaseline(name, args);
  const needsBaseline = ctx.config.securityPolicy.enforceHarnessBaseline && tracksTask;
  const needsOperation = shouldLogOperation(name);
  if (!tracksTask && !needsOperation) return {};
  const { workspaceId, scopeId, taskRoot } = await workspaceContext(ctx, key);
  let taskId: string | undefined;
  let baselineCaptureMs = 0;
  let baselineBeforeWrite: ProjectBaseline | undefined;
  if (tracksTask) {
    const task = ctx.state.task(scopeId);
    if (task) {
      if (needsBaseline) {
        const baselineStartedAt = performance.now();
        const baselineCheck = await checkTaskBaseline(taskRoot, task);
        baselineBeforeWrite = baselineCheck.current;
        baselineCaptureMs += elapsedPhaseMs(baselineStartedAt);
        if (!baselineCheck.matches) {
          const previous = {
            branch: task.baseline.branch ?? null,
            head: task.baseline.head ?? null,
            expected_fingerprint: task.expected_fingerprint
          };
          task.baseline.branch = baselineCheck.current.branch;
          task.baseline.head = baselineCheck.current.head;
          task.expected_fingerprint = baselineCheck.current.worktree_fingerprint;
          task.updated_at = now();
          await ctx.state.setTask(scopeId, task).catch(() => undefined);
          await ctx.state.addTaskEvent(event(
            workspaceId,
            task.id,
            'baseline_drift_adopted',
            { changes: baselineCheck.changes, previous },
            {
              ok: true,
              branch: baselineCheck.current.branch ?? null,
              head: baselineCheck.current.head ?? null,
              expected_fingerprint: baselineCheck.current.worktree_fingerprint
            },
            name
          )).catch(() => undefined);
        }
      }
      await ctx.state.addTaskEvent(event(
        workspaceId,
        task.id,
        'operation_started',
        operationInput(args),
        { ok: true, tracking: 'task', baseline_check_performed: needsBaseline },
        name
      ));
      taskId = task.id;
    }
  }
  let operation: OperationRecord | undefined;
  if (needsOperation) {
    operation = {
      id: randomUUID().replaceAll('-', ''),
      workspace_id: workspaceId,
      ...(taskId ? { task_id: taskId } : {}),
      tool: name,
      kind: 'started',
      input_summary: { arguments_present: true },
      result_summary: { ok: true },
      affected_files: [],
      created_at: now()
    };
    await ctx.state.addOperation(workspaceId, operation).catch(() => undefined);
  }
  return {
    workspaceId,
    ...(taskId ? {
      taskId,
      taskRoot,
      baselineChecked: needsBaseline,
      baselineCaptureMs,
      ...(baselineBeforeWrite ? { baselineBeforeWrite } : {})
    } : {}),
    ...(operation ? { operation } : {})
  };
}

export async function finishHarnessTracking(
  ctx: ToolContext,
  key: string,
  name: string,
  args: JsonObject,
  tracking: HarnessTracking,
  result: JsonObject,
  exposedTools?: Set<string>
): Promise<JsonObject> {
  if (!tracking.taskId && standaloneOperation(name) && result.ok === true) {
    result.harness_mode = 'standalone';
    result.task_required = false;
    if (!Array.isArray(result.next_actions)) result.next_actions = [];
    result.recovery_hint = '当前操作已在 standalone 模式完成；如需继续，直接调用下一个开发工具。';
  }
  if (tracking.operation) {
    const field = Object.hasOwn(result, 'operation_id') ? 'harness_operation_id' : 'operation_id';
    result[field] = tracking.operation.id;
  }
  if (tracking.baselineCaptureMs !== undefined && tracking.baselineCaptureMs > 0) {
    addPhaseDuration(result, 'baseline_capture_ms', tracking.baselineCaptureMs);
  }
  if (result.ok === false) result = await attachHarnessStatus(ctx, key, result, !tracking.taskId, exposedTools);
  let deferredProcessOperation = false;
  if (tracking.operation && result.command_ok === null && typeof result.session_id === 'string') {
    const completedInput = operationInput(args);
    deferredProcessOperation = await attachHarnessOperation(ctx, result.session_id, {
      ...tracking.operation,
      input_summary: completedInput,
      ...(typeof completedInput.reason === 'string' ? { reason: completedInput.reason } : {})
    });
  }
  if (tracking.taskId && tracking.workspaceId) {
    const succeeded = result.ok === true;
    await ctx.state.addTaskEvent(event(
      tracking.workspaceId,
      tracking.taskId,
      'operation_finished',
      operationInput(args),
      { ok: succeeded, tool: name },
      name
    )).catch(() => undefined);
    if (succeeded && tracking.baselineChecked && ctx.config.securityPolicy.enforceHarnessBaseline) {
      const refreshStartedAt = performance.now();
      let refreshMode = 'full';
      let refreshFileCount = 0;
      const editResult = { ...result, tool: name };
      const incremental = await refreshEditExpectedStateIncremental(ctx, key, tracking, editResult).catch(() => ({ refreshed: false, fileCount: 0 }));
      if (incremental.refreshed) {
        refreshMode = 'incremental_verified_edit';
        refreshFileCount = incremental.fileCount;
      } else {
        await refreshExpectedState(ctx, tracking.workspaceId, tracking.taskRoot ?? selectedFolder(ctx, key).path, tracking.taskId).catch(() => undefined);
      }
      result.baseline_refresh_mode = refreshMode;
      result.baseline_refresh_file_count = refreshFileCount;
      addPhaseDuration(result, 'baseline_refresh_ms', elapsedPhaseMs(refreshStartedAt));
    }
  }
  if (tracking.operation && tracking.workspaceId && !deferredProcessOperation) {
    const succeeded = result.ok === true;
    const completedInput = operationInput(args);
    const completed: OperationRecord = {
      ...tracking.operation,
      kind: succeeded ? 'completed' : 'failed',
      input_summary: completedInput,
      ...(typeof completedInput.reason === 'string' ? { reason: completedInput.reason } : {}),
      result_summary: operationResultSummary(name, result),
      created_at: now()
    };
    await ctx.state.addOperation(tracking.workspaceId, completed).catch(() => undefined);
  }
  return result;
}
