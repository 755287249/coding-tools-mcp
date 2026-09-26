import type { ActivityEvent, ActivityKind, ActivityPlan, ActivityTodo } from "$lib/api/activity";
import type { MessageKey } from "$lib/i18n";

/** Checklist for a plan. Older desktop builds only send step lists. */
export function planTodos(plan: ActivityPlan | undefined): ActivityTodo[] {
  if (!plan) return [];
  if (plan.todos && plan.todos.length > 0) return plan.todos;
  const finished = plan.status === "completed";
  return [
    ...plan.completedSteps.map((title, index) => ({ id: `done-${index + 1}`, title, status: "completed" as const })),
    ...plan.pendingSteps.map((title, index) => ({
      id: `step-${index + 1}`,
      title,
      status: index === 0 && !finished ? ("in_progress" as const) : ("pending" as const),
    })),
  ];
}

export interface PlanOverview {
  todos: ActivityTodo[];
  done: number;
  total: number;
  /** Share of completed steps (0–100). */
  pct: number;
  current: ActivityTodo | undefined;
  finished: boolean;
}

export function planOverview(plan: ActivityPlan | undefined): PlanOverview {
  const todos = planTodos(plan);
  const done = todos.filter((todo) => todo.status === "completed").length;
  const total = todos.length;
  const finished = plan?.status === "completed" || (total > 0 && done === total);
  return {
    todos,
    done,
    total,
    pct: finished ? 100 : total > 0 ? Math.round((done / total) * 100) : 0,
    current: todos.find((todo) => todo.status === "in_progress"),
    finished,
  };
}

export const TODO_GLYPH: Record<ActivityTodo["status"], string> = {
  completed: "●",
  in_progress: "◐",
  pending: "○",
};

/** Tool categories for the grouped view, in display order. */
export type ActivityCategory = "edit" | "files" | "terminal" | "diagnostics" | "git" | "tasks" | "desktop" | "other";

export const CATEGORY_ORDER: readonly ActivityCategory[] = [
  "edit",
  "files",
  "terminal",
  "diagnostics",
  "git",
  "tasks",
  "desktop",
  "other",
];

export const CATEGORY_META: Record<ActivityCategory, { emoji: string; label: MessageKey }> = {
  edit: { emoji: "✏️", label: "Edits" },
  files: { emoji: "📄", label: "Files & search" },
  terminal: { emoji: "💻", label: "Terminal" },
  diagnostics: { emoji: "🩺", label: "Diagnostics" },
  git: { emoji: "🌿", label: "Git" },
  tasks: { emoji: "🔧", label: "Tasks & sessions" },
  desktop: { emoji: "🖱️", label: "Desktop" },
  other: { emoji: "🧩", label: "Other" },
};

const KIND_CATEGORY: Record<ActivityKind, ActivityCategory> = {
  edit: "edit",
  read: "files",
  search: "files",
  exec: "terminal",
  check: "diagnostics",
  git: "git",
  plan: "tasks",
  desktop: "desktop",
  other: "other",
};

export function categoryOf(event: ActivityEvent): ActivityCategory {
  if (event.tool === "format_files" || event.tool === "exec_health_check") return "diagnostics";
  return KIND_CATEGORY[event.kind] ?? "other";
}

export interface ActivityGroup {
  category: ActivityCategory;
  events: ActivityEvent[];
  /** All calls in the category, including those beyond `perGroup`. */
  total: number;
  errors: number;
  running: number;
  durationMs: number;
}

/** Groups the newest events by category (events are newest first). */
export function groupEvents(events: ActivityEvent[], perGroup = 40): ActivityGroup[] {
  const groups = new Map<ActivityCategory, ActivityGroup>();
  for (const event of events) {
    const category = categoryOf(event);
    let group = groups.get(category);
    if (!group) {
      group = { category, events: [], total: 0, errors: 0, running: 0, durationMs: 0 };
      groups.set(category, group);
    }
    group.total += 1;
    if (event.status === "error") group.errors += 1;
    if (event.status === "running") group.running += 1;
    group.durationMs += event.durationMs ?? 0;
    if (group.events.length < perGroup) group.events.push(event);
  }
  return CATEGORY_ORDER.flatMap((category) => {
    const group = groups.get(category);
    return group ? [group] : [];
  });
}

export interface ChangedFile {
  path: string;
  /** Operation of the most recent change. */
  operation: string;
  added: number;
  removed: number;
  edits: number;
  lastSeq: number;
  lastMs: number;
}

/** Files touched by successful, non-preview edits (newest first). */
export function changedFiles(events: ActivityEvent[]): ChangedFile[] {
  const files = new Map<string, ChangedFile>();
  for (const event of events) {
    if (event.status !== "success" || event.dryRun) continue;
    for (const change of event.changes) {
      const existing = files.get(change.path);
      if (existing) {
        existing.added += change.added;
        existing.removed += change.removed;
        existing.edits += 1;
        continue;
      }
      files.set(change.path, {
        path: change.path,
        operation: change.operation,
        added: change.added,
        removed: change.removed,
        edits: 1,
        lastSeq: event.seq,
        lastMs: event.startedMs,
      });
    }
  }
  return [...files.values()].sort((a, b) => b.lastSeq - a.lastSeq);
}
