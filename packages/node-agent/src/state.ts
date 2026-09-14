import { appendFile, mkdir, readFile, readdir, rename, writeFile } from 'node:fs/promises';
import path from 'node:path';
import type {
  ChangeSet,
  OperationRecord,
  PersistentState,
  StateJsonObject,
  StateStoreContract,
  TaskEvent,
  TaskRecord
} from './state/contract.js';

function writable(status: TaskRecord['status']): boolean {
  return ['active', 'paused', 'verifying', 'failed'].includes(status);
}

function operationFromUnknown(value: unknown): OperationRecord | undefined {
  if (!value || typeof value !== 'object') return undefined;
  const row = value as Record<string, unknown>;
  if (typeof row.id === 'string' && typeof row.workspace_id === 'string' && typeof row.tool === 'string'
    && typeof row.kind === 'string' && typeof row.created_at === 'string') {
    return {
      id: row.id,
      workspace_id: row.workspace_id,
      ...(typeof row.task_id === 'string' ? { task_id: row.task_id } : {}),
      tool: row.tool,
      kind: row.kind,
      input_summary: (row.input_summary && typeof row.input_summary === 'object' ? row.input_summary : {}) as StateJsonObject,
      result_summary: (row.result_summary && typeof row.result_summary === 'object' ? row.result_summary : {}) as StateJsonObject,
      ...(typeof row.reason === 'string' ? { reason: row.reason } : {}),
      affected_files: Array.isArray(row.affected_files) ? row.affected_files as StateJsonObject[] : [],
      created_at: row.created_at
    };
  }
  if (typeof row.id === 'string' && typeof row.tool === 'string' && typeof row.startedAt === 'number') {
    return {
      id: row.id,
      workspace_id: typeof row.folderId === 'string' ? row.folderId : 'hub',
      tool: row.tool,
      kind: 'completed',
      input_summary: {},
      result_summary: {
        ok: row.ok === true,
        duration_ms: typeof row.durationMs === 'number' ? row.durationMs : 0
      },
      ...(typeof row.summary === 'string' ? { reason: row.summary } : {}),
      affected_files: [],
      created_at: String(row.startedAt)
    };
  }
  return undefined;
}

export class StateStore implements StateStoreContract {
  readonly file: string;
  readonly harnessRoot: string;
  #state: PersistentState = { tasks: {}, currentTasks: {}, taskEvents: {}, changeSets: {} };
  #operations: OperationRecord[] = [];
  #writeTail: Promise<void> = Promise.resolve();
  #operationWriteTails = new Map<string, Promise<void>>();
  #operationDirectoryReady = new Map<string, Promise<void>>();

  constructor(dataDir: string) {
    this.file = path.join(dataDir, 'state.json');
    this.harnessRoot = path.join(dataDir, 'harness');
  }

  async load(): Promise<void> {
    let legacyOperations: OperationRecord[] = [];
    try {
      const parsed = JSON.parse(await readFile(this.file, 'utf8')) as Partial<PersistentState> & { operations?: unknown[] };
      const tasks = parsed.tasks ?? {};
      const currentTasks = parsed.currentTasks ?? {};
      for (const [key, task] of Object.entries(tasks)) {
        if (!task || typeof task !== 'object' || !('workspace_id' in task)) delete tasks[key];
      }
      this.#state = {
        tasks,
        currentTasks,
        taskEvents: parsed.taskEvents ?? {},
        changeSets: parsed.changeSets ?? {}
      };
      legacyOperations = Array.isArray(parsed.operations)
        ? parsed.operations.map(operationFromUnknown).filter((row): row is OperationRecord => Boolean(row))
        : [];
      for (const [scopeId, taskId] of Object.entries(this.#state.currentTasks)) {
        const task = this.#state.tasks[taskId];
        const taskScopeId = task?.scope_id ?? task?.workspace_id;
        if (!task || taskScopeId !== scopeId || !writable(task.status)) delete this.#state.currentTasks[scopeId];
      }
    } catch (error) {
      if ((error as NodeJS.ErrnoException).code !== 'ENOENT') throw error;
    }
    this.#operations = [...legacyOperations, ...await this.loadOperationLogs()].slice(-5_000);
  }

  task(scopeId: string): TaskRecord | undefined {
    const taskId = this.#state.currentTasks[scopeId];
    return taskId ? structuredClone(this.#state.tasks[taskId]) : undefined;
  }

  taskById(folderId: string, taskId: string): TaskRecord | undefined {
    const task = this.#state.tasks[taskId];
    return task?.workspace_id === folderId ? structuredClone(task) : undefined;
  }

  tasks(folderId?: string): Record<string, TaskRecord> {
    const entries = Object.entries(this.#state.tasks).filter(([, task]) => !folderId || task.workspace_id === folderId);
    return structuredClone(Object.fromEntries(entries));
  }

  taskEvents(taskId: string): TaskEvent[] { return structuredClone(this.#state.taskEvents[taskId] ?? []); }

  changeById(folderId: string, changeId: string): ChangeSet | undefined {
    const change = this.#state.changeSets[changeId];
    if (!change) return undefined;
    const task = this.#state.tasks[change.task_id];
    return task?.workspace_id === folderId ? structuredClone(change) : undefined;
  }

  operations(workspaceId?: string, limit?: number): OperationRecord[] {
    const filtered = workspaceId ? this.#operations.filter(row => row.workspace_id === workspaceId) : this.#operations;
    const source = limit === undefined ? filtered : filtered.slice(-Math.max(1, limit));
    return structuredClone(source);
  }

  async listOperations(workspaceId: string, offset: number, limit: number): Promise<OperationRecord[]> {
    const file = this.operationsPath(workspaceId);
    try {
      const lines = (await readFile(file, 'utf8')).split(/\r?\n/);
      const operations: OperationRecord[] = [];
      for (const line of lines.slice(Math.max(0, offset))) {
        if (!line) continue;
        let parsed: OperationRecord | undefined;
        try { parsed = operationFromUnknown(JSON.parse(line)); } catch { break; }
        if (!parsed) break;
        operations.push(parsed);
        if (operations.length >= Math.max(1, limit)) break;
      }
      return operations;
    } catch (error) {
      if ((error as NodeJS.ErrnoException).code === 'ENOENT') {
        return this.operations(workspaceId).slice(Math.max(0, offset), Math.max(0, offset) + Math.max(1, limit));
      }
      throw error;
    }
  }

  async migrateWorkspace(legacyId: string, workspaceId: string): Promise<void> {
    if (legacyId === workspaceId) return;
    let changed = false;
    for (const task of Object.values(this.#state.tasks)) {
      if (task.workspace_id !== legacyId) continue;
      task.workspace_id = workspaceId;
      changed = true;
    }
    const currentTaskId = this.#state.currentTasks[legacyId];
    if (currentTaskId) {
      if (!this.#state.currentTasks[workspaceId]) this.#state.currentTasks[workspaceId] = currentTaskId;
      delete this.#state.currentTasks[legacyId];
      changed = true;
    }
    if (changed) await this.save();
  }

  async setTask(scopeId: string, task: TaskRecord, event?: TaskEvent): Promise<void> {
    this.#state.tasks[task.id] = task;
    const activeScopeId = task.scope_id ?? scopeId;
    if (writable(task.status)) this.#state.currentTasks[activeScopeId] = task.id;
    else if (this.#state.currentTasks[activeScopeId] === task.id) delete this.#state.currentTasks[activeScopeId];
    if (event) this.pushTaskEvent(event);
    await this.save();
  }

  async setTaskAndChange(scopeId: string, task: TaskRecord, change: ChangeSet, event: TaskEvent): Promise<void> {
    this.#state.tasks[task.id] = task;
    this.#state.changeSets[change.id] = change;
    const activeScopeId = task.scope_id ?? scopeId;
    if (writable(task.status)) this.#state.currentTasks[activeScopeId] = task.id;
    else if (this.#state.currentTasks[activeScopeId] === task.id) delete this.#state.currentTasks[activeScopeId];
    this.pushTaskEvent(event);
    await this.save();
  }

  async addTaskEvent(event: TaskEvent): Promise<void> {
    this.pushTaskEvent(event);
    await this.save();
  }

  async addOperation(workspaceId: string, operation: OperationRecord): Promise<void> {
    const record = { ...operation, workspace_id: workspaceId };
    this.#operations.push(record);
    if (this.#operations.length > 5_000) this.#operations.splice(0, 1_000);
    const file = this.operationsPath(workspaceId);
    const directory = path.dirname(file);
    let directoryReady = this.#operationDirectoryReady.get(workspaceId);
    if (!directoryReady) {
      directoryReady = mkdir(directory, { recursive: true }).then(() => undefined);
      this.#operationDirectoryReady.set(workspaceId, directoryReady);
      directoryReady.catch(() => this.#operationDirectoryReady.delete(workspaceId));
    }
    const ready = directoryReady;
    const line = `${JSON.stringify(record)}\n`;
    const prior = this.#operationWriteTails.get(workspaceId) ?? Promise.resolve();
    const next = prior.catch(() => undefined).then(async () => {
      await ready;
      try {
        await appendFile(file, line, { mode: 0o600 });
      } catch (error) {
        if ((error as NodeJS.ErrnoException).code !== 'ENOENT') throw error;
        const retryReady = mkdir(directory, { recursive: true }).then(() => undefined);
        this.#operationDirectoryReady.set(workspaceId, retryReady);
        retryReady.catch(() => this.#operationDirectoryReady.delete(workspaceId));
        await retryReady;
        await appendFile(file, line, { mode: 0o600 });
      }
    });
    this.#operationWriteTails.set(workspaceId, next);
    await next;
  }

  async save(): Promise<void> {
    this.#writeTail = this.#writeTail.catch(() => undefined).then(async () => {
      await mkdir(path.dirname(this.file), { recursive: true });
      const temp = `${this.file}.${process.pid}.tmp`;
      await writeFile(temp, `${JSON.stringify(this.#state, null, 2)}\n`, { mode: 0o600 });
      await rename(temp, this.file);
    });
    await this.#writeTail;
  }

  private pushTaskEvent(event: TaskEvent): void {
    const events = this.#state.taskEvents[event.task_id] ??= [];
    events.push(event);
    if (events.length > 2_000) events.splice(0, events.length - 2_000);
  }

  private operationsPath(workspaceId: string): string {
    return path.join(this.harnessRoot, 'workspaces', workspaceId, 'operations.jsonl');
  }

  private async loadOperationLogs(): Promise<OperationRecord[]> {
    const root = path.join(this.harnessRoot, 'workspaces');
    let directories;
    try { directories = await readdir(root, { withFileTypes: true }); }
    catch (error) {
      if ((error as NodeJS.ErrnoException).code === 'ENOENT') return [];
      throw error;
    }
    const operations: OperationRecord[] = [];
    for (const directory of directories) {
      if (!directory.isDirectory()) continue;
      try {
        const lines = (await readFile(path.join(root, directory.name, 'operations.jsonl'), 'utf8')).split(/\r?\n/);
        for (const line of lines) {
          if (!line) continue;
          let parsed: OperationRecord | undefined;
          try { parsed = operationFromUnknown(JSON.parse(line)); } catch { break; }
          if (!parsed) break;
          operations.push(parsed);
        }
      } catch (error) {
        if ((error as NodeJS.ErrnoException).code !== 'ENOENT') throw error;
      }
    }
    operations.sort((left, right) => Number(left.created_at) - Number(right.created_at));
    return operations;
  }
}
