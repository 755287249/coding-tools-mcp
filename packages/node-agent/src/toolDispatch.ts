import { chatToolHandlers } from './chat/tools.js';
import { toolNames } from './catalog.js';
import { permissionToolHandlers } from './permissionTools.js';
import { desktopToolHandlers } from './toolDispatchers/desktop.js';
import { gitToolHandlers } from './toolDispatchers/git.js';
import { historyToolHandlers } from './toolDispatchers/history.js';
import { processToolHandlers } from './toolDispatchers/process.js';
import { runtimeToolHandlers } from './toolDispatchers/runtime.js';
import { taskToolHandlers } from './toolDispatchers/task.js';
import { workspaceToolHandlers } from './toolDispatchers/workspace.js';
import { toolRuntimeFor, type ToolDomain } from './toolRuntime.js';
import type { ToolDispatchRequest, ToolHandler, ToolHandlerMap } from './toolDispatch/contract.js';
import type { JsonObject } from './types.js';
import { currentExecutionBinding } from './executionScope.js';
import { applyDefaultCwdArgs } from './toolDispatch/defaultCwd.js';

export type {
  ResumeToolRequest,
  ToolDispatchRequest,
  ToolHandler,
  ToolHandlerMap
} from './toolDispatch/contract.js';

interface ToolHandlerModule {
  readonly name: string;
  readonly handlers: ToolHandlerMap;
}

const DELEGATED_DOMAINS = new Set<ToolDomain>([
  'harness',
  'history',
  'task',
  'filesystem',
  'search',
  'quality',
  'process',
  'git',
  'runtime',
  'desktop'
]);

const TOOL_HANDLER_MODULES: readonly ToolHandlerModule[] = [
  { name: 'chat', handlers: chatToolHandlers },
  { name: 'desktop', handlers: desktopToolHandlers },
  { name: 'workspace', handlers: workspaceToolHandlers },
  { name: 'git', handlers: gitToolHandlers },
  { name: 'task', handlers: taskToolHandlers },
  { name: 'history', handlers: historyToolHandlers },
  { name: 'process', handlers: processToolHandlers },
  { name: 'runtime', handlers: runtimeToolHandlers },
  { name: 'permission', handlers: permissionToolHandlers }
];

const catalogNames = new Set(toolNames);
const handlerByName = new Map<string, ToolHandler>();

for (const moduleDefinition of TOOL_HANDLER_MODULES) {
  for (const [name, handler] of Object.entries(moduleDefinition.handlers)) {
    if (!catalogNames.has(name)) {
      throw new Error(`Node ${moduleDefinition.name} dispatcher references an unknown tool: ${name}`);
    }
    if (handlerByName.has(name)) {
      throw new Error(`Duplicate Node domain tool handler: ${name}`);
    }
    handlerByName.set(name, handler);
  }
}

const missingHandlers = toolNames.filter(name =>
  DELEGATED_DOMAINS.has(toolRuntimeFor(name).domain) && !handlerByName.has(name)
);
if (missingHandlers.length) {
  throw new Error(`Missing Node domain tool handlers: ${missingHandlers.join(', ')}`);
}

export function registeredDomainToolNames(): string[] {
  return toolNames.filter(name => handlerByName.has(name));
}

export function dispatchDomainTool(
  name: string,
  request: ToolDispatchRequest
): Promise<JsonObject> | undefined {
  const handler = handlerByName.get(name);
  if (!handler) return undefined;
  const binding = currentExecutionBinding(request.ctx, request.key);
  const workspaceRoot = binding?.folderId
    ? request.ctx.config.folders.find(folder => folder.id === binding.folderId)?.path
    : undefined;
  const args = applyDefaultCwdArgs(name, request.args, binding?.defaultCwd ?? '.', workspaceRoot);
  return Promise.resolve(handler(args === request.args ? request : { ...request, args }));
}
