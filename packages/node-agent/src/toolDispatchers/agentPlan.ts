import {chatPlan} from '../chat/store.js';
import {selectedFolder} from '../workspace.js';
import type { ToolDispatchRequest, ToolHandlerMap } from '../toolDispatch/contract.js';
import type { JsonObject } from '../types.js';

/**
 * Agent-reported plan tools (set_todos / update_plan / report_progress).
 * Mirrors src-tauri/src/tools/agent_plan.rs: the state is in-memory per
 * conversation and never touches workspace files.
 */

const MAX_TODOS = 24;
const MAX_ID = 80;
const MAX_TITLE = 400;
const MAX_GOAL = 400;
const MAX_PROGRESS = 2000;
const MAX_PHASE = 160;
const MAX_EXTERNAL_ID = 100;
const STATUSES = new Set(['pending', 'in_progress', 'completed']);

interface Todo {
  id: string;
  title: string;
  status: string;
}

interface Progress {
  message: string;
  phase?: string;
  percent?: number;
  todo_id?: string;
  updated_ms: number;
}

interface PlanState {
  goal: string;
  externalTaskId?: string;
  todos: Todo[];
  progress?: Progress;
}

const MAX_PLANS = 256;

/** Conversation key → plan, oldest first so the map stays bounded. */
class PlanStore {
  private readonly entries = new Map<string, PlanState>();

  get(key: string): PlanState | undefined {
    return this.entries.get(key);
  }

  set(key: string, plan: PlanState): void {
    this.entries.delete(key);
    this.entries.set(key, plan);
    while (this.entries.size > MAX_PLANS) {
      const oldest = this.entries.keys().next().value;
      if (oldest === undefined) break;
      this.entries.delete(oldest);
    }
  }

  delete(key: string): void {
    this.entries.delete(key);
  }
}

const plans = new PlanStore();

class AgentPlanError extends Error {
  readonly code = 'INVALID_ARGUMENT';
  readonly category = 'validation';

  constructor(message: string) {
    super(message);
    this.name = 'AgentPlanError';
  }
}

function optionalText(args: JsonObject, key: string, max: number): string | undefined {
  const value = args[key];
  if (value === undefined || value === null) return undefined;
  if (typeof value !== 'string') throw new AgentPlanError(`${key} must be a string`);
  if ([...value].length > max) throw new AgentPlanError(`${key} must be at most ${max} characters`);
  return value.trim();
}

function list(args: JsonObject, key: string): unknown[] {
  const value = args[key];
  if (!Array.isArray(value)) throw new AgentPlanError(`${key} must be an array`);
  if (value.length > MAX_TODOS) throw new AgentPlanError(`${key} must contain at most ${MAX_TODOS} items`);
  return value;
}

function itemText(item: unknown, key: string, index: number): string {
  const value = item && typeof item === 'object' ? (item as Record<string, unknown>)[key] : undefined;
  if (typeof value !== 'string') throw new AgentPlanError(`item ${index}: ${key} must be a string`);
  return value.trim();
}

function validateTodos(todos: Todo[]): void {
  const ids = new Set<string>();
  let running = 0;
  for (const todo of todos) {
    if (!todo.id || [...todo.id].length > MAX_ID) throw new AgentPlanError(`todo id must be 1-${MAX_ID} characters`);
    if (!todo.title || [...todo.title].length > MAX_TITLE) throw new AgentPlanError(`todo title must be 1-${MAX_TITLE} characters`);
    if (ids.has(todo.id)) throw new AgentPlanError(`Duplicate todo id: ${todo.id}`);
    ids.add(todo.id);
    if (!STATUSES.has(todo.status)) throw new AgentPlanError(`Invalid todo status "${todo.status}"; use pending, in_progress or completed`);
    if (todo.status === 'in_progress') running += 1;
  }
  if (running > 1) throw new AgentPlanError('At most one todo may be in_progress');
}

function summary(plan: PlanState | undefined): JsonObject {
  if (!plan || plan.todos.length === 0 && !plan.progress) return { cleared: true, todos: [] };
  const done = plan.todos.filter(todo => todo.status === 'completed').length;
  const current = plan.todos.find(todo => todo.status === 'in_progress');
  const status = plan.todos.length > 0 && done === plan.todos.length ? 'completed' : current ? 'in_progress' : 'pending';
  return {
    goal: plan.goal,
    status,
    completed: done,
    total: plan.todos.length,
    current: current ? { id: current.id, title: current.title } : null,
    todos: plan.todos.map(todo => ({ ...todo })),
    external_task_id: plan.externalTaskId ?? null,
    progress: plan.progress ? { ...plan.progress } : null
  };
}

function setTodos({ ctx, key, args }: ToolDispatchRequest): JsonObject {
  if (['chat_id','attachment_id','reply_to'].some(k=>Object.hasOwn(args,k))) return chatPlan(selectedFolder(ctx,key).path,'set_todos',args);
  const goal = optionalText(args, 'goal', MAX_GOAL);
  const externalTaskId = optionalText(args, 'external_task_id', MAX_EXTERNAL_ID) || undefined;
  const todos = list(args, 'todos').map((item, index) => ({
    id: itemText(item, 'id', index),
    title: itemText(item, 'title', index),
    status: itemText(item, 'status', index)
  }));
  validateTodos(todos);
  if (todos.length === 0) {
    plans.delete(key);
    return { ok: true, plan: summary(undefined) };
  }
  const previous = plans.get(key);
  const next: PlanState = {
    goal: goal ?? previous?.goal ?? '',
    externalTaskId: externalTaskId ?? previous?.externalTaskId,
    todos
  };
  plans.set(key, next);
  return { ok: true, plan: summary(next) };
}

function updatePlan({ ctx, key, args }: ToolDispatchRequest): JsonObject {
  if (['chat_id','attachment_id','reply_to'].some(k=>Object.hasOwn(args,k))) return chatPlan(selectedFolder(ctx,key).path,'update_plan',args);
  const goal = optionalText(args, 'goal', MAX_GOAL);
  const explanation = optionalText(args, 'explanation', MAX_PROGRESS);
  const previous = plans.get(key);
  const remaining = [...(previous?.todos ?? [])];
  const used = new Set<string>();
  let nextId = 1;
  const todos = list(args, 'plan').map((item, index) => {
    const title = itemText(item, 'step', index);
    if (!title || [...title].length > MAX_TITLE) throw new AgentPlanError(`item ${index}: step must be 1-${MAX_TITLE} characters`);
    const status = itemText(item, 'status', index);
    const match = remaining.findIndex(todo => todo.title === title);
    let id: string;
    if (match >= 0) {
      id = remaining.splice(match, 1)[0].id;
    } else {
      do {
        id = `todo-${nextId}`;
        nextId += 1;
      } while (used.has(id) || remaining.some(todo => todo.id === id));
    }
    used.add(id);
    return { id, title, status };
  });
  validateTodos(todos);
  if (todos.length === 0) {
    plans.delete(key);
    return { ok: true, plan: summary(undefined) };
  }
  const next: PlanState = {
    goal: goal ?? previous?.goal ?? '',
    externalTaskId: previous?.externalTaskId,
    todos,
    ...(explanation ? { progress: { message: explanation, phase: '计划更新', updated_ms: Date.now() } } : {})
  };
  plans.set(key, next);
  return { ok: true, plan: summary(next) };
}

function reportProgress({ ctx, key, args }: ToolDispatchRequest): JsonObject {
  if (['chat_id','attachment_id','reply_to'].some(k=>Object.hasOwn(args,k))) return chatPlan(selectedFolder(ctx,key).path,'report_progress',args);
  const message = optionalText(args, 'message', MAX_PROGRESS);
  if (!message) throw new AgentPlanError('message is required');
  const phase = optionalText(args, 'phase', MAX_PHASE) ?? '';
  let percent: number | undefined;
  if (args.percent !== undefined && args.percent !== null) {
    if (!Number.isInteger(args.percent) || (args.percent as number) < 0 || (args.percent as number) > 100) {
      throw new AgentPlanError('percent must be an integer from 0 to 100');
    }
    percent = args.percent as number;
  }
  const requested = optionalText(args, 'todo_id', MAX_ID) || undefined;
  const plan = plans.get(key) ?? { goal: '', todos: [] };
  let todoId: string | undefined;
  if (requested !== undefined) {
    if (!plan.todos.some(todo => todo.id === requested)) {
      throw new AgentPlanError(`Unknown todo_id: ${requested}. Call set_todos first or omit todo_id.`);
    }
    todoId = requested;
  } else {
    todoId = plan.todos.find(todo => todo.status === 'in_progress')?.id;
  }
  const progress: Progress = {
    message,
    ...(phase ? { phase } : {}),
    ...(percent !== undefined ? { percent } : {}),
    ...(todoId ? { todo_id: todoId } : {}),
    updated_ms: Date.now()
  };
  plans.set(key, { ...plan, progress });
  return { ok: true, progress: { ...progress } };
}

export const agentPlanToolHandlers = {
  set_todos: setTodos,
  update_plan: updatePlan,
  report_progress: reportProgress
} satisfies ToolHandlerMap;
