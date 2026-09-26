import { getBackend } from "$lib/backend";

export type ActivityKind =
  | "read"
  | "search"
  | "edit"
  | "check"
  | "exec"
  | "plan"
  | "git"
  | "desktop"
  | "other";

export type ActivityStatus = "running" | "success" | "error";

export interface ActivityChange {
  path: string;
  operation: string;
  bytesBefore?: number;
  bytesAfter?: number;
  added: number;
  removed: number;
}

export interface ActivityEvent {
  seq: number;
  rev: number;
  startedMs: number;
  durationMs?: number;
  tool: string;
  kind: ActivityKind;
  status: ActivityStatus;
  title: string;
  paths: string[];
  detail?: string;
  error?: string;
  changes: ActivityChange[];
  dryRun: boolean;
  hasDiff: boolean;
  diff?: string;
  diffTruncated: boolean;
  requestBytes: number;
  responseBytes: number;
}

export type ActivityTodoStatus = "pending" | "in_progress" | "completed";

export interface ActivityTodo {
  id: string;
  title: string;
  status: ActivityTodoStatus;
}

/** Latest agent progress report (report_progress / update_plan explanation). */
export interface ActivityProgress {
  message: string;
  phase?: string;
  /** Agent estimate 0–100; never derived automatically. */
  percent?: number;
  todoId?: string;
  updatedMs: number;
}

export interface ActivityPlan {
  taskId: string;
  objective: string;
  status: string;
  completedSteps: string[];
  pendingSteps: string[];
  updatedMs: number;
  /** `task` = harness task tools, `agent` = set_todos / update_plan / report_progress. */
  source?: "task" | "agent" | string;
  externalTaskId?: string;
  /** Unified checklist. Older desktop builds omit it; derive from the step lists then. */
  todos?: ActivityTodo[];
  progress?: ActivityProgress;
}

export interface ActivityStats {
  total: number;
  success: number;
  errors: number;
  running: number;
  totalDurationMs: number;
  avgDurationMs: number;
  p95DurationMs: number;
  bytesIn: number;
  bytesOut: number;
  reads: number;
  edits: number;
  execs: number;
  searches: number;
  filesChanged: number;
  filesViewed: number;
  bytesAdded: number;
  bytesRemoved: number;
  linesAdded: number;
  linesRemoved: number;
  firstMs: number;
  lastMs: number;
}

export interface ActivitySnapshot {
  events: ActivityEvent[];
  rev: number;
  stats: ActivityStats;
  plan?: ActivityPlan;
  reset: boolean;
}

export function emptyActivityStats(): ActivityStats {
  return {
    total: 0,
    success: 0,
    errors: 0,
    running: 0,
    totalDurationMs: 0,
    avgDurationMs: 0,
    p95DurationMs: 0,
    bytesIn: 0,
    bytesOut: 0,
    reads: 0,
    edits: 0,
    execs: 0,
    searches: 0,
    filesChanged: 0,
    filesViewed: 0,
    bytesAdded: 0,
    bytesRemoved: 0,
    linesAdded: 0,
    linesRemoved: 0,
    firstMs: 0,
    lastMs: 0,
  };
}

export function emptyActivitySnapshot(): ActivitySnapshot {
  return { events: [], rev: 0, stats: emptyActivityStats(), reset: true };
}

export function readWorkspaceActivity(
  workspaceId: string,
  sinceRev = 0,
): Promise<ActivitySnapshot> {
  return getBackend().activity.read(workspaceId, sinceRev);
}

export function readWorkspaceActivityDetail(
  workspaceId: string,
  seq: number,
): Promise<ActivityEvent | null> {
  return getBackend().activity.detail(workspaceId, seq);
}

export function clearWorkspaceActivity(workspaceId: string): Promise<void> {
  return getBackend().activity.clear(workspaceId);
}
