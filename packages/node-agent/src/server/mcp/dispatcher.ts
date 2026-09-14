import type { IncomingMessage } from 'node:http';
import { callTool } from '../../tools.js';
import { markMcpConversationMetadata } from '../../conversation.js';
import { findPendingOperation } from '../../folderRuntime.js';
import { permissionInputRequired, permissionMrtrRetry } from '../../mcpMrtr.js';
import {
  clientSupportsMcpTasks,
  createProcessTask,
  detailedProcessTask,
  markMissingCancelledProcessTask,
  markProcessTaskCancellationRequested,
  MCP_TASKS_EXTENSION,
  requireProcessTask,
  updateProcessTaskFromSnapshot
} from '../../mcpTasks.js';
import { wrapMcpToolResult } from '../../toolContract.js';
import {
  LATEST_LEGACY_MCP_PROTOCOL_VERSION,
  LEGACY_MCP_PROTOCOL_VERSIONS,
  MODERN_MCP_PROTOCOL_VERSION
} from '../../mcpTransport.js';
import type { ProcessRequestLifecycle } from '../../processes.js';
import type { JsonObject, ToolContext } from '../../types.js';
import { getSkillPrompt, listSkillPrompts, listSkillResources, readSkillResource } from '../../skills/mcp.js';
import {
  isToolEvolutionResourceUri,
  listToolEvolutionResources,
  readToolEvolutionResource
} from '../../knowledge/mcp.js';
import { AGENT_VERSION } from '../../version.js';
import type { ToolCatalogSnapshot } from '../catalog.js';

const legacyProtocols = new Set<string>(LEGACY_MCP_PROTOCOL_VERSIONS);

const SERVER_INSTRUCTIONS = 'Call conversation_bootstrap before project tools. It reuses an existing folder selection, auto-binds the only configured folder, or returns folder choices when multiple folders are unselected; legacy list_workspace_folders + switch_workspace_folder + history_session_bootstrap remains available. If conversation_bootstrap returns startup_flow=workspace_bootstrapped_history_degraded, continue using project tools; retry history_session_bootstrap only when durable history/checkpointing is needed. Workspace and enabled Codex/Claude user-level Skills are exposed through standard MCP prompts and resources. After workspace selection, use the lightweight skill summaries returned by conversation_bootstrap to identify a clearly relevant Skill, then load only that Skill through prompts/get or resources/read. Skills are workflow guidance and never grant permissions or weaken tool, sandbox, or workspace policy. Enabled Node Agent Hooks may block or rewrite tool calls, and enabled external MCP servers contribute proxied tools to tools/list. Tools whose schema exposes workspace_folder_id may route one call to another allowed folder without changing the conversation selection; process control calls can recover their original folder from a conversation-scoped session_id or output_ref. Prefer exec_many(mode=auto) when two or more independent commands are known in the same reasoning step. For exec_many, ok reports tool/orchestration success; inspect command_ok, graph_execution_ok, failed_command_ids, and skipped_command_ids for child-command outcomes. Hosts should refresh tools/list when x-coding-tools-toolset-revision or runtimeStartedAtMs changes. FRP and Cloudflare transports are intentionally unsupported.';

interface DispatchOptions {
  catalog: ToolCatalogSnapshot;
  context: ToolContext;
  method: string;
  processLifecycle?: ProcessRequestLifecycle;
  req: IncomingMessage;
  request: JsonObject;
  protocolVersion: string;
  startedAt: number;
}

function requestedToolsetRevision(req: IncomingMessage, params: JsonObject): string {
  const metadata = params._meta && typeof params._meta === 'object' && !Array.isArray(params._meta)
    ? params._meta as JsonObject
    : {};
  const metaRevision = String(metadata['coding-tools/toolset-revision'] ?? '').trim();
  if (metaRevision) return metaRevision;
  const header = req.headers['x-coding-tools-toolset-revision'];
  return String(Array.isArray(header) ? header[0] ?? '' : header ?? '').trim();
}

function catalogMismatch(catalog: ToolCatalogSnapshot, clientRevision: string, startedAt: number): Error {
  return Object.assign(
    new Error('Tool catalog revision changed; refresh tools/list before retrying the tool call.'),
    {
      rpcCode: -32602,
      rpcData: {
        reason: 'stale_tool_catalog',
        error_code: 'TOOLSET_REVISION_MISMATCH',
        error_category: 'catalog',
        retryable: true,
        client_toolset_revision: clientRevision,
        toolset_revision: catalog.revision,
        runtime_started_at_ms: startedAt,
        available_tools: catalog.names,
        suggestion: 'Refresh tools/list and retry with the current tool catalog.'
      }
    }
  );
}

function taskRequestError(message: string, data: JsonObject = {}): Error {
  return Object.assign(new Error(message), { rpcCode: -32602, rpcData: data });
}

function unknownTool(catalog: ToolCatalogSnapshot, name: string): Error {
  return Object.assign(new Error(`Unknown tool: ${name}`), {
    rpcCode: -32602,
    rpcData: {
      reason: 'unknown_tool',
      error_code: 'UNKNOWN_TOOL',
      error_category: 'catalog',
      retryable: true,
      suggestion: 'Refresh tools/list and retry with the current tool catalog.',
      toolset_revision: catalog.revision,
      available_tools: catalog.names
    }
  });
}

export function rpcErrorResponse(requestId: unknown, error: unknown): JsonObject {
  const code = typeof error === 'object' && error && 'rpcCode' in error
    ? Number((error as { rpcCode: number }).rpcCode)
    : -32603;
  const data = typeof error === 'object' && error && 'rpcData' in error
    ? (error as { rpcData: JsonObject }).rpcData
    : undefined;
  return {
    jsonrpc: '2.0',
    id: requestId,
    error: {
      code,
      message: error instanceof Error ? error.message : String(error),
      ...(data ? { data } : {})
    }
  };
}

function objectValue(value: unknown): JsonObject {
  return value && typeof value === 'object' && !Array.isArray(value) ? value as JsonObject : {};
}

async function listMcpResources(context: ToolContext): Promise<JsonObject> {
  const [skills, evolution] = await Promise.all([
    listSkillResources(context),
    listToolEvolutionResources(context)
  ]);
  const skillResources = Array.isArray(skills.resources) ? skills.resources : [];
  const evolutionResources = Array.isArray(evolution.resources) ? evolution.resources : [];
  return {
    resources: [...skillResources, ...evolutionResources],
    _meta: {
      ...objectValue(skills._meta),
      ...objectValue(evolution._meta)
    }
  };
}

export async function dispatchMcpMethod(options: DispatchOptions): Promise<unknown> {
  const { catalog, context, method, processLifecycle, protocolVersion, req, request, startedAt } = options;
  const modern = protocolVersion === MODERN_MCP_PROTOCOL_VERSION;

  if (method === 'initialize') {
    if (modern) throw Object.assign(new Error('Method not found: initialize'), { rpcCode: -32601 });
    const params = (request.params ?? {}) as JsonObject;
    const requested = String(params.protocolVersion ?? '');
    return {
      protocolVersion: legacyProtocols.has(requested)
        ? requested
        : LATEST_LEGACY_MCP_PROTOCOL_VERSION,
      capabilities: {
        tools: { listChanged: false },
        prompts: { listChanged: false },
        resources: { subscribe: false, listChanged: false },
        logging: {}
      },
      serverInfo: {
        name: 'coding-tools-mcp-node',
        title: 'Coding Tools MCP Node Agent',
        version: AGENT_VERSION,
        toolsetRevision: catalog.revision,
        runtimeStartedAtMs: startedAt
      },
      instructions: SERVER_INSTRUCTIONS
    };
  }
  if (method === 'ping') {
    if (modern) throw Object.assign(new Error('Method not found: ping'), { rpcCode: -32601 });
    return {};
  }
  if (method === 'server/discover') {
    if (!modern) throw Object.assign(new Error('Method not found: server/discover'), { rpcCode: -32601 });
    return {
      supportedVersions: [MODERN_MCP_PROTOCOL_VERSION],
      capabilities: {
        tools: { listChanged: false },
        prompts: { listChanged: false },
        resources: { subscribe: false, listChanged: false },
        extensions: { [MCP_TASKS_EXTENSION]: {} }
      },
      instructions: SERVER_INSTRUCTIONS
    };
  }
  if (method === 'prompts/list') return listSkillPrompts(context);
  if (method === 'prompts/get') {
    const params = (request.params ?? {}) as JsonObject;
    const meta = markMcpConversationMetadata(params._meta);
    return getSkillPrompt(context, String(params.name ?? ''), context.conversations.identity(meta).key);
  }
  if (method === 'resources/list') return listMcpResources(context);
  if (method === 'resources/read') {
    const params = (request.params ?? {}) as JsonObject;
    const uri = String(params.uri ?? '');
    if (isToolEvolutionResourceUri(uri)) return readToolEvolutionResource(context, uri);
    const meta = markMcpConversationMetadata(params._meta);
    return readSkillResource(context, uri, context.conversations.identity(meta).key);
  }
  if (method === 'tools/list') {
    return { tools: catalog.tools, toolsetRevision: catalog.revision };
  }
  if (method === 'tasks/get' || method === 'tasks/update' || method === 'tasks/cancel') {
    if (!modern) throw Object.assign(new Error(`Method not found: ${method}`), { rpcCode: -32601 });
    const params = (request.params ?? {}) as JsonObject;
    if (!clientSupportsMcpTasks(params)) {
      throw Object.assign(new Error(`Method not found: ${method}`), { rpcCode: -32601 });
    }
    const meta = markMcpConversationMetadata(params._meta);
    const conversationKey = context.conversations.identity(meta).key;
    let task;
    try {
      task = requireProcessTask(context, conversationKey, params.taskId);
    } catch (error) {
      throw taskRequestError(error instanceof Error ? error.message : String(error), {
        reason: 'task_not_found',
        error_code: 'TASK_NOT_FOUND',
        retryable: false
      });
    }

    if (method === 'tasks/update') {
      throw taskRequestError('Task is not waiting for input', {
        reason: 'task_not_input_required',
        error_code: 'TASK_NOT_INPUT_REQUIRED',
        retryable: false,
        taskId: task.taskId,
        status: task.status
      });
    }
    if (method === 'tasks/cancel') {
      if (task.status !== 'working') {
        throw taskRequestError(`Task is already terminal: ${task.status}`, {
          reason: 'task_already_terminal',
          error_code: 'TASK_ALREADY_TERMINAL',
          retryable: false,
          taskId: task.taskId,
          status: task.status
        });
      }
      const structured = await callTool(
        context,
        'kill_session',
        { session_id: task.sessionId, wait_ms: 0 },
        meta
      );
      if (structured.ok === false) {
        throw taskRequestError('Unable to cancel task', {
          reason: 'task_cancel_failed',
          error_code: 'TASK_CANCEL_FAILED',
          retryable: false,
          taskId: task.taskId,
          tool_error: structured.error ?? structured
        });
      }
      markProcessTaskCancellationRequested(task, structured);
      return {};
    }

    if (task.status !== 'working') return detailedProcessTask(task);
    const structured = await callTool(
      context,
      'wait_command',
      {
        session_id: task.sessionId,
        timeout_ms: 0,
        output_mode: 'tail',
        max_output_bytes: 65_536
      },
      meta
    );
    if (structured.ok === false) {
      const cancelled = markMissingCancelledProcessTask(task);
      if (cancelled) return cancelled;
      throw taskRequestError('Unable to resolve task state', {
        reason: 'task_state_unavailable',
        error_code: 'TASK_STATE_UNAVAILABLE',
        retryable: false,
        taskId: task.taskId,
        tool_error: structured.error ?? structured
      });
    }
    return updateProcessTaskFromSnapshot(task, structured);
  }
  if (method === 'tools/call') {
    const params = (request.params ?? {}) as JsonObject;
    const name = String(params.name ?? '');
    const clientToolsetRevision = requestedToolsetRevision(req, params);
    if (clientToolsetRevision && clientToolsetRevision !== catalog.revision) {
      throw catalogMismatch(catalog, clientToolsetRevision, startedAt);
    }
    if (!catalog.names.includes(name)) throw unknownTool(catalog, name);
    if (!processLifecycle) throw new Error('MCP tool call lifecycle is required');
    const argumentsValue = (params.arguments ?? {}) as JsonObject;
    const meta = markMcpConversationMetadata(params._meta);
    delete meta['coding-tools/toolset-revision'];
    if (context.extensions.hasExternalTool(name)) {
      if (context.config.permissionMode === 'read-only') {
        throw Object.assign(new Error('External MCP tools are disabled in read-only permission mode.'), {
          rpcCode: -32602,
          rpcData: { reason: 'external_mcp_read_only', error_code: 'EXTERNAL_MCP_READ_ONLY', retryable: false }
        });
      }
      const identity = context.conversations.identity(meta);
      const folderId = context.selections.get(identity.key);
      const folder = folderId ? context.folderRuntimes.get(folderId) : undefined;
      const cwd = folder?.workspacePath ?? context.config.folders[0]?.path ?? process.cwd();
      return context.extensions.callExternalTool(name, argumentsValue, cwd, identity.key);
    }
    if (modern) {
      let retry;
      try {
        retry = permissionMrtrRetry(params);
      } catch (error) {
        throw Object.assign(error instanceof Error ? error : new Error(String(error)), {
          rpcCode: -32602,
          rpcData: {
            reason: 'invalid_mrtr_permission_response',
            error_code: 'INVALID_MRTR_PERMISSION_RESPONSE',
            retryable: false
          }
        });
      }
      if (retry) {
        const pending = findPendingOperation(context, retry.resumeId);
        if (pending && pending.operation.name !== name) {
          throw Object.assign(new Error('MRTR requestState does not match the retried tool'), {
            rpcCode: -32602,
            rpcData: {
              reason: 'mrtr_request_state_mismatch',
              error_code: 'MRTR_REQUEST_STATE_MISMATCH',
              retryable: false,
              requested_tool: name,
              pending_tool: pending.operation.name
            }
          });
        }
        const structured = await callTool(
          context,
          'request_permissions',
          {
            resume_id: retry.resumeId,
            approve: retry.approved,
            confirm: retry.approved,
            scope: 'once'
          },
          meta,
          false,
          processLifecycle
        );
        if (clientSupportsMcpTasks(params)) {
          const conversationKey = context.conversations.identity(meta).key;
          const task = createProcessTask(context, conversationKey, name, argumentsValue, structured);
          if (task) return task;
        }
        return wrapMcpToolResult(name, argumentsValue, structured);
      }
    }
    const structured = await callTool(
      context,
      name,
      argumentsValue,
      meta,
      false,
      processLifecycle
    );
    if (modern) {
      const inputRequired = permissionInputRequired(structured);
      if (inputRequired) return inputRequired;
      if (clientSupportsMcpTasks(params)) {
        const conversationKey = context.conversations.identity(meta).key;
        const task = createProcessTask(context, conversationKey, name, argumentsValue, structured);
        if (task) return task;
      }
    }
    return wrapMcpToolResult(name, argumentsValue, structured);
  }
  throw Object.assign(new Error(`Method not found: ${method}`), { rpcCode: -32601 });
}
