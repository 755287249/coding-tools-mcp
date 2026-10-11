import { createHash } from 'node:crypto';
import type { AgentConfig, JsonObject, ToolDefinition, WorkspaceFolder } from '../types.js';
import { discoverExtensions, type DiscoveredExtensions, type ExtensionDiscoveryOptions } from './discovery.js';
import { runPostToolHooks, runPreToolHooks, runSessionHooks } from './hooks.js';
import { ExternalMcpConnection } from './mcpClient.js';
import type {
  ExtensionInventorySnapshot,
  ExtensionKind,
  ExternalMcpTool,
  HookDescriptor,
  HookPostResult,
  HookPreResult,
  McpServerDescriptor
} from './types.js';

const REFRESH_TTL_MS = 2_000;

const MAX_ACTIVE_HOOK_SESSIONS = 512;

interface ActiveHookSession {
  sessionId: string;
  cwd: string;
  folderId: string;
}

function activeHookSessionKey(sessionId: string, folderId: string): string {
  return `${folderId}\0${sessionId}`;
}

export interface ExtensionRegistryOptions extends Omit<ExtensionDiscoveryOptions, 'folders'> {
  folders: readonly WorkspaceFolder[];
  hooksActive?: boolean;
  projectIsolation?:boolean;
  mcpActive?: boolean;
  enabledHooks?: readonly string[];
  enabledMcpServers?: readonly string[];
}

export class ExtensionRegistry {
  private folders: WorkspaceFolder[];
  private hooksActive: boolean;
  private mcpActive: boolean;
  private enabledHooks: Set<string>;
  private enabledMcpServers: Set<string>;
  private discovered: DiscoveredExtensions = { hooks: [], mcpServers: [], diagnostics: [], scannedAtMs: 0 };
  private lastRefreshAt = 0;
  private readonly connections = new Map<string, ExternalMcpConnection>();
  private readonly activeHookSessions = new Map<string, ActiveHookSession>();
  private externalTools: ExternalMcpTool[] = [];
  private extensionRevision = createHash('sha256').update('empty').digest('hex').slice(0, 16);
  private refreshPromise?: Promise<void>;

  constructor(private readonly options: ExtensionRegistryOptions) {
    this.folders = options.folders.map(folder => ({ ...folder }));
    this.hooksActive = !options.projectIsolation && (options.hooksActive ?? true);
    this.mcpActive = !options.projectIsolation && (options.mcpActive ?? true);
    this.enabledHooks = new Set(options.enabledHooks ?? []);
    this.enabledMcpServers = new Set(options.enabledMcpServers ?? []);
  }

  get revision(): string { return this.extensionRevision; }

  setFolders(folders: readonly WorkspaceFolder[]): void {
    this.folders = folders.map(folder => ({ ...folder }));
    this.lastRefreshAt = 0;
  }

  async setConfiguration(config: {
    hooks: { active: boolean; enabled: readonly string[] };
    mcp: { active: boolean; enabled: readonly string[] };
  }): Promise<void> {
    this.hooksActive = !this.options.projectIsolation && config.hooks.active;
    this.mcpActive = !this.options.projectIsolation && config.mcp.active;
    this.enabledHooks = new Set(config.hooks.enabled);
    this.enabledMcpServers = new Set(config.mcp.enabled);
    this.lastRefreshAt = 0;
    await this.refresh(true);
  }

  async setActive(kind: ExtensionKind, active: boolean): Promise<void> {
    active=active&&!this.options.projectIsolation;
    if (kind === 'hook') this.hooksActive = active;
    else this.mcpActive = active;
    this.lastRefreshAt = 0;
    await this.refresh(true);
  }

  async setEnabled(kind: ExtensionKind, keys: readonly string[]): Promise<void> {
    if (kind === 'hook') this.enabledHooks = new Set(keys);
    else this.enabledMcpServers = new Set(keys);
    this.lastRefreshAt = 0;
    await this.refresh(true);
  }

  async refresh(force = false): Promise<void> {
    if (!force && Date.now() - this.lastRefreshAt < REFRESH_TTL_MS) return;
    if (this.refreshPromise) return this.refreshPromise;
    this.refreshPromise = this.doRefresh().finally(() => { this.refreshPromise = undefined; });
    return this.refreshPromise;
  }

  private async doRefresh(): Promise<void> {
    this.discovered = await discoverExtensions({ ...this.options, folders: this.folders });
    const activeKeys = new Set(this.mcpActive
      ? this.discovered.mcpServers
        .filter(server => server.supported && server.sourceEnabled && this.enabledMcpServers.has(server.key))
        .map(server => server.key)
      : []);
    for (const [serverKey, connection] of this.connections) {
      if (activeKeys.has(serverKey)) continue;
      await connection.close();
      this.connections.delete(serverKey);
    }
    const tools: ExternalMcpTool[] = [];
    for (const server of this.discovered.mcpServers) {
      if (!activeKeys.has(server.key)) continue;
      let connection = this.connections.get(server.key);
      if (connection && JSON.stringify(connection.server) !== JSON.stringify(server)) {
        await connection.close();
        this.connections.delete(server.key);
        connection = undefined;
      }
      if (!connection) {
        const workspaceRoot = this.folders.find(folder => folder.id === server.folderId)?.path;
        connection = new ExternalMcpConnection(server, workspaceRoot);
        this.connections.set(server.key, connection);
      }
      try { tools.push(...await connection.refreshTools()); }
      catch { /* surfaced through inventory */ }
    }
    this.externalTools = tools.sort((left, right) => left.name.localeCompare(right.name));
    this.extensionRevision = createHash('sha256').update(JSON.stringify({
      hooks: this.hooksActive
        ? this.discovered.hooks.filter(hook => hook.supported && hook.sourceEnabled && this.enabledHooks.has(hook.key)).map(hook => hook.key)
        : [],
      tools: this.externalTools.map(tool => [tool.name, tool.serverKey, tool.toolName])
    })).digest('hex').slice(0, 16);
    this.lastRefreshAt = Date.now();
  }

  async inventory(force = true): Promise<ExtensionInventorySnapshot> {
    await this.refresh(force);
    return {
      hooks: this.discovered.hooks.map(hook => {
        const selected = this.enabledHooks.has(hook.key);
        return {
          hook,
          selected,
          enabled: this.hooksActive && hook.supported && hook.sourceEnabled && selected
        };
      }),
      mcpServers: this.discovered.mcpServers.map(server => {
        const connection = this.connections.get(server.key);
        const selected = this.enabledMcpServers.has(server.key);
        return {
          server,
          selected,
          enabled: this.mcpActive && server.supported && server.sourceEnabled && selected,
          connected: connection?.connected ?? false,
          toolCount: connection?.toolDefinitions.length ?? 0,
          ...(connection?.error ? { error: connection.error } : {})
        };
      }),
      diagnostics: this.discovered.diagnostics,
      scannedAtMs: this.discovered.scannedAtMs
    };
  }

  toolDefinitions(): ToolDefinition[] {
    return this.externalTools.map(tool => structuredClone(tool.definition));
  }

  toolNames(): string[] {
    return this.externalTools.map(tool => tool.name);
  }

  hasExternalTool(name: string): boolean {
    return this.externalTools.some(tool => tool.name === name);
  }

  async callExternalTool(name: string, args: JsonObject, cwd: string, sessionId: string): Promise<JsonObject> {
    await this.refresh();
    const tool = this.externalTools.find(item => item.name === name);
    if (!tool) throw new Error(`External MCP tool was not found: ${name}`);
    const folderId = this.connections.get(tool.serverKey)?.server.folderId;
    const pre = await this.preToolUse(tool.logicalName, args, cwd, sessionId, folderId);
    if (pre.blocked) return {
      ok: false,
      isError: true,
      content: [{ type: 'text', text: pre.blocked.message }],
      error: { code: 'HOOK_BLOCKED', message: pre.blocked.message, hook_key: pre.blocked.hookKey }
    };
    const connection = this.connections.get(tool.serverKey);
    if (!connection) throw new Error(`External MCP server is not connected: ${tool.serverKey}`);
    let result: JsonObject;
    try { result = await connection.call(tool.toolName, pre.input); }
    catch (error) {
      result = { isError: true, content: [{ type: 'text', text: error instanceof Error ? error.message : String(error) }] };
    }
    const post = await this.postToolUse(result.isError === true ? 'PostToolUseFailure' : 'PostToolUse', tool.logicalName, pre.input, result, cwd, sessionId);
    if (pre.context.length) result.hook_context = pre.context;
    if (post.feedback.length) result.hook_feedback = post.feedback;
    return result;
  }

  private enabledHooksFor(event: string, folderId?: string): HookDescriptor[] {
    if (!this.hooksActive) return [];
    return this.discovered.hooks.filter(hook =>
      hook.event === event
      && hook.supported
      && hook.sourceEnabled
      && this.enabledHooks.has(hook.key)
      && (!hook.folderId || !folderId || hook.folderId === folderId)
    );
  }

  private async startSession(cwd: string, sessionId: string, folderId: string, source: string): Promise<HookPostResult> {
    if (!this.hooksActive) return { feedback: [] };
    const key = activeHookSessionKey(sessionId, folderId);
    if (this.activeHookSessions.has(key)) return { feedback: [] };
    if (this.activeHookSessions.size >= MAX_ACTIVE_HOOK_SESSIONS) {
      const oldestKey = this.activeHookSessions.keys().next().value;
      if (oldestKey !== undefined) {
        const oldest = this.activeHookSessions.get(oldestKey);
        this.activeHookSessions.delete(oldestKey);
        if (oldest) {
          await runSessionHooks(
            this.enabledHooksFor('SessionEnd', oldest.folderId),
            'SessionEnd',
            oldest.cwd,
            oldest.sessionId,
            'evicted'
          );
        }
      }
    }
    this.activeHookSessions.set(key, { cwd, sessionId, folderId });
    return runSessionHooks(this.enabledHooksFor('SessionStart', folderId), 'SessionStart', cwd, sessionId, source);
  }

  async sessionStart(cwd: string, sessionId: string, folderId: string, source = 'startup'): Promise<HookPostResult> {
    await this.refresh();
    return this.startSession(cwd, sessionId, folderId, source);
  }

  async sessionEnd(sessionId: string, folderId: string, source = 'shutdown'): Promise<HookPostResult> {
    await this.refresh();
    const key = activeHookSessionKey(sessionId, folderId);
    const active = this.activeHookSessions.get(key);
    if (!active) return { feedback: [] };
    this.activeHookSessions.delete(key);
    return runSessionHooks(this.enabledHooksFor('SessionEnd', folderId), 'SessionEnd', active.cwd, sessionId, source);
  }

  async preToolUse(toolName: string, input: JsonObject, cwd: string, sessionId: string, folderId?: string): Promise<HookPreResult> {
    await this.refresh();
    if (folderId) await this.startSession(cwd, sessionId, folderId, 'startup');
    return runPreToolHooks(this.enabledHooksFor('PreToolUse', folderId), toolName, input, cwd, sessionId);
  }

  async postToolUse(
    event: 'PostToolUse' | 'PostToolUseFailure',
    toolName: string,
    input: JsonObject,
    response: JsonObject,
    cwd: string,
    sessionId: string,
    folderId?: string
  ): Promise<HookPostResult> {
    await this.refresh();
    return runPostToolHooks(this.enabledHooksFor(event, folderId), event, toolName, input, response, cwd, sessionId);
  }

  async close(): Promise<void> {
    const activeSessions = [...this.activeHookSessions.values()];
    this.activeHookSessions.clear();
    await Promise.allSettled(activeSessions.map(session => runSessionHooks(
      this.enabledHooksFor('SessionEnd', session.folderId),
      'SessionEnd',
      session.cwd,
      session.sessionId,
      'shutdown'
    )));
    await Promise.all([...this.connections.values()].map(connection => connection.close()));
    this.connections.clear();
    this.externalTools = [];
  }
}

export function extensionConfig(config: AgentConfig): {
  hooks: { active: boolean; enabled: string[] };
  mcp: { active: boolean; enabled: string[] };
} {
  return {
    hooks: { active: config.extensions?.hooks.active ?? true, enabled: [...(config.extensions?.hooks.enabled ?? [])] },
    mcp: { active: config.extensions?.mcp.active ?? true, enabled: [...(config.extensions?.mcp.enabled ?? [])] }
  };
}
