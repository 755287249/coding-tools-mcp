import {
  changeSummary,
  closeFailedTask,
  failTask,
  finishTask,
  harnessStatus,
  listTaskEvents,
  operationLog,
  projectState,
  rollbackTask,
  setTaskStatus,
  startTask,
  taskContext,
  updateTask
} from '../taskTools.js';
import type { ToolHandlerMap } from '../toolDispatch/contract.js';

export const taskToolHandlers = {
  harness_status: ({ ctx, key }) => harnessStatus(ctx, key),
  operation_log: ({ ctx, key, args }) => operationLog(ctx, key, args),
  project_state: ({ ctx, key, args }) => projectState(ctx, key, args),
  start_task: ({ ctx, key, args }) => startTask(ctx, key, args),
  update_task: ({ ctx, key, args }) => updateTask(ctx, key, args),
  pause_task: ({ ctx, key, args }) => setTaskStatus(ctx, key, 'paused', args),
  resume_task: ({ ctx, key, args }) => setTaskStatus(ctx, key, 'active', args),
  fail_task: ({ ctx, key, args }) => failTask(ctx, key, args),
  close_failed_task: ({ ctx, key, args }) => closeFailedTask(ctx, key, args),
  rollback_task: ({ ctx, key, args }) => rollbackTask(ctx, key, args),
  finish_task: ({ ctx, key, args }) => finishTask(ctx, key, args),
  task_context: ({ ctx, key, args }) => taskContext(ctx, key, args),
  list_task_events: ({ ctx, key, args }) => listTaskEvents(ctx, key, args),
  change_summary: ({ ctx, key, args }) => changeSummary(ctx, key, args)
} satisfies ToolHandlerMap;
