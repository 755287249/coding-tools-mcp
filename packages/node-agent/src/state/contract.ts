export type StateJsonObject = Record<string, unknown>;

export type TaskStatus = 'active' | 'paused' | 'verifying' | 'failed' | 'failed_final' | 'completed' | 'completed_unverified' | 'rolled_back';

export interface ProjectBaseline {
  branch?: string;
  head?: string;
  worktree_fingerprint: string;
  entries: Array<{
    path: string;
    exists: boolean;
    is_binary: boolean;
    sha256: string;
    bytes: number;
  }>;
  captured_at: string;
}

export interface TaskRecord {
  id: string;
  workspace_id: string;
  scope_id?: string;
  scope_root?: string;
  objective: string;
  status: TaskStatus;
  baseline: ProjectBaseline;
  expected_fingerprint: string;
  completed_steps: string[];
  pending_steps: string[];
  latest_change_id?: string;
  latest_verification_id?: string;
  created_at: string;
  updated_at: string;
}

export interface TaskEvent {
  id: string;
  task_id: string;
  operation_id: string;
  kind: string;
  tool_name?: string;
  input_summary: StateJsonObject;
  result_summary: StateJsonObject;
  reason?: { text: string; source: string };
  affected_files: StateJsonObject[];
  created_at: string;
}

export interface ChangeSet {
  id: string;
  task_id: string;
  objective: string;
  reason: { text: string; source: string };
  files: StateJsonObject[];
  command_ids: string[];
  verification_ids: string[];
  risks: string[];
  created_at: string;
}

export interface OperationRecord {
  id: string;
  workspace_id: string;
  task_id?: string;
  tool: string;
  kind: string;
  input_summary: StateJsonObject;
  result_summary: StateJsonObject;
  reason?: string;
  affected_files: StateJsonObject[];
  created_at: string;
}

export interface PersistentState {
  tasks: Record<string, TaskRecord>;
  currentTasks: Record<string, string>;
  taskEvents: Record<string, TaskEvent[]>;
  changeSets: Record<string, ChangeSet>;
}

export interface StateStoreContract {
  readonly file: string;
  readonly harnessRoot: string;
  load(): Promise<void>;
  task(scopeId: string): TaskRecord | undefined;
  taskById(folderId: string, taskId: string): TaskRecord | undefined;
  tasks(folderId?: string): Record<string, TaskRecord>;
  taskEvents(taskId: string): TaskEvent[];
  changeById(folderId: string, changeId: string): ChangeSet | undefined;
  operations(workspaceId?: string, limit?: number): OperationRecord[];
  listOperations(workspaceId: string, offset: number, limit: number): Promise<OperationRecord[]>;
  migrateWorkspace(legacyId: string, workspaceId: string): Promise<void>;
  setTask(scopeId: string, task: TaskRecord, event?: TaskEvent): Promise<void>;
  setTaskAndChange(scopeId: string, task: TaskRecord, change: ChangeSet, event: TaskEvent): Promise<void>;
  addTaskEvent(event: TaskEvent): Promise<void>;
  addOperation(workspaceId: string, operation: OperationRecord): Promise<void>;
  save(): Promise<void>;
}
