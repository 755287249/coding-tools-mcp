import { toolsForProfile, toolNamesForProfile, toolsetRevisionForProfile } from '../catalog.js';
import { createHash } from 'node:crypto';
import { realpath } from 'node:fs/promises';
import path from 'node:path';
import { authorizeProjectAttachment } from '../chat/store.js';
import { projectToolAllowed } from '../projectIsolation.js';
import { localChatSkill } from '../rustCatalog.generated.js';
import { wrapMcpToolResult } from '../toolContract.js';
import type { JsonObject, ToolContext } from '../types.js';
import { currentToolCatalog, type ToolCatalogSnapshot } from './catalog.js';

const scopeFields = ['workspace_folder_id', 'chat_id', 'attachment_id'];
export const GLOBAL_INSTRUCTIONS = 'This is the global MCP connection. Projects have fixed directories. Call chat_open with the supplied chat_id and workspace_folder_id. For EVERY subsequent tool call include the returned attachment_id, chat_id and workspace_folder_id; for prompts/resources/tasks place these in params._meta. A client session header is not authorization. Commands require an OS sandbox; run Git and formatters via exec_command. Follow the full local-chat Skill.';
function object(value: unknown): JsonObject {
  return value && typeof value === 'object' && !Array.isArray(value) ? value as JsonObject : {};
}
function denied(message: string): never {
  throw Object.assign(new Error(message), { rpcCode: -32003 });
}
export function projectCatalog(context: ToolContext): ToolCatalogSnapshot {
  const base = context.config.globalConnection
    ? {profile:'advanced' as const,tools:toolsForProfile('advanced'),names:toolNamesForProfile('advanced'),revision:toolsetRevisionForProfile('advanced')}
    : currentToolCatalog(context);
  const tools = structuredClone(base.tools.filter(tool => projectToolAllowed(tool.name)));
  for (const tool of tools) {
    const properties = object(tool.inputSchema.properties);
    for (const field of scopeFields) properties[field] ??= {
      type: 'string', minLength: 1, description: 'Fixed project capability returned by chat_open. Keep it unchanged.'
    };
    tool.inputSchema.properties = properties;
    if (!['chat_open', 'list_workspace_folders'].includes(tool.name)) {
      tool.inputSchema.required = [...new Set([...(Array.isArray(tool.inputSchema.required) ? tool.inputSchema.required : []), ...scopeFields])];
    }
  }
  return { ...base, tools, names: tools.map(tool => tool.name), revision: createHash('sha256').update(JSON.stringify(tools)).digest('hex') };
}
export interface ProjectSelection {
  context: ToolContext;
  catalog: ToolCatalogSnapshot;
  request: JsonObject;
  result?: unknown;
}
/** The provider supplies only currently registered project contexts, never the control root. */
export class ProjectGateway {
  private stopped = false;
  constructor(private readonly contexts: () => Iterable<ToolContext>) {}
  stop(): void { this.stopped = true; }
  folders() { return [...this.contexts()].flatMap(context => context.config.folders); }
  async contextFor(folderId: string): Promise<ToolContext> {
    if (this.stopped) denied('Global connection is stopped');
    const contexts = [...this.contexts()];
    const matches = contexts.filter(item => item.config.folders.some(folder => folder.id === folderId));
    if (matches.length !== 1) denied('Project directory is unavailable or ambiguous');
    const context = matches[0];
    if (!context || context.runtimeClosed || !context.projectIsolation || context.config.folders.length !== 1) denied('Project directory unavailable');
    const root = await realpath(context.config.folders[0].path);
    for (const other of contexts.filter(item => item !== context).flatMap(item => item.config.folders)) {
      let otherRoot: string;
      try { otherRoot = await realpath(other.path); } catch { continue; }
      const overlaps = (a: string, b: string) => { const relative = path.relative(a, b); return !relative || (!relative.startsWith(`..${path.sep}`) && relative !== '..' && !path.isAbsolute(relative)); };
      if (overlaps(root, otherRoot) || overlaps(otherRoot, root)) denied('Project directories overlap. Use separate directory roots before enabling isolated access.');
    }
    if (this.stopped || context.runtimeClosed || ![...this.contexts()].includes(context)) denied('Project connection is stopped');
    return context;
  }
  async select(control: ToolContext, input: JsonObject): Promise<ProjectSelection> {
    if (this.stopped) denied('Global connection is stopped');
    const request = structuredClone(input);
    const method = String(request.method ?? '');
    const params = object(request.params);
    const name = String(params.name ?? '');
    const scope = structuredClone(object(method === 'tools/call' ? params.arguments : params._meta));
    const attached = typeof scope.attachment_id === 'string' && !!scope.attachment_id;
    const selection: ProjectSelection = { context: control, catalog: projectCatalog(control), request };
    if (!attached) {
      if (['initialize', 'server/discover', 'ping', 'notifications/initialized', 'notifications/cancelled', 'tools/list'].includes(method)) return selection;
      if (method === 'resources/list') return { ...selection, result: { resources: [localChatSkill] } };
      if (method === 'resources/read' && params.uri === localChatSkill.uri) return { ...selection, result: { contents: [localChatSkill] } };
      if (method === 'tools/call' && name === 'list_workspace_folders') return { ...selection, result: wrapMcpToolResult(name, scope, { ok: true, folders: this.folders() }) };
      if (method !== 'tools/call' || name !== 'chat_open' || !scope.chat_id) denied('Open a chat first and include its project/chat/attachment capability in every project operation');
    }
    const context = await this.contextFor(String(scope.workspace_folder_id ?? ''));
    if (attached) authorizeProjectAttachment(context.config.folders[0].path, scope, name === 'chat_wait');
    const catalog = projectCatalog(context);
    if (method === 'tools/call') {
      if (!projectToolAllowed(name)) denied('Use exec_command inside the project sandbox for this operation');
      if (name === 'switch_workspace_folder' && scope.folder_id !== scope.workspace_folder_id) denied('Conversation project cannot change');
      const core = currentToolCatalog(context).tools.find(tool => tool.name === name);
      const args = object(params.arguments);
      for (const field of scopeFields) if (core && !(field in object(core.inputSchema.properties))) delete args[field];
      params.arguments = args;
    }
    params._meta = { ...object(params._meta), 'openai/session': `project-chat:${String(scope.attachment_id ?? 'opening')}` };
    request.params = params;
    return { context, catalog, request };
  }
}
