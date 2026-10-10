import { createHash, randomBytes } from 'node:crypto';
import { createServer, type Server } from 'node:http';
import path from 'node:path';
import type { AgentConfig, ToolContext } from './types.js';
import { resolveToolProfile } from './catalog.js';
import { externalBase, OAuthRuntime, sendJson } from './oauth.js';
import { KeyedMutex, Semaphore } from './runtime.js';
import { StateStore } from './state.js';
import {
  ConfigStore, handleManagementRequest,
  type WorkspaceManagementStore, type WorkspaceRuntimeRecord
} from './management.js';
import { defaultPolicy } from './policy.js';
import { legacySecurityPolicy } from './securityPolicy.js';
import { disposeProcessSessions, restoreProcessSessions } from './processes.js';
import { preflightSandboxConfiguration } from './sandbox.js';
import { ToolUsageStore } from './toolUsage.js';
import { allFolderRuntimes, createFolderRuntime, syncToolEvolutionRuntimes } from './folderRuntime.js';
import { ExtensionRegistry } from './extensions/registry.js';
import { canonicalizeWorkspaceFolders } from './workspace.js';
import { ConversationStore, deriveWorkspaceProfileId } from './conversation.js';
import { currentToolCatalog, setRuntimeRevisionHeaders } from './server/catalog.js';
import { localPath, routePrefix, sendText } from './server/http.js';
import { rpcErrorResponse } from './server/mcp/dispatcher.js';
import { handleMcpRoute } from './server/routes/mcp.js';
import { handleOAuthRoute } from './server/routes/oauth.js';
import { markPairing } from './chat/pairing.js';
import { handleSystemRoute } from './server/routes/system.js';

export async function createToolContext(config: AgentConfig): Promise<ToolContext> {
  config.policy ??= defaultPolicy();
  config.toolProfile ??= 'advanced';
  config.activeToolProfile ??= resolveToolProfile(config.toolProfile, config.permissionMode);
  config.securityPolicy ??= legacySecurityPolicy(config.permissionMode, config.toolProfile);
  config.securityPolicyCustomized ??= false;
  config.skills ??= { active: true, disabled: [] };
  config.skills.active ??= true;
  config.extensions ??= { hooks: { active: true, enabled: [] }, mcp: { active: true, enabled: [] } };
  config.extensions.hooks ??= { active: true, enabled: [] };
  config.extensions.mcp ??= { active: true, enabled: [] };
  config.extensions.hooks.active ??= true;
  config.extensions.mcp.active ??= true;
  const folders = await canonicalizeWorkspaceFolders(config.folders);
  config = { ...config, folders };
  const state = new StateStore(config.dataDir);
  await state.load();
  const usageStore = new ToolUsageStore(config.dataDir, { redactTelemetry: config.securityPolicy.redactTelemetry });
  const folderRuntimes = new Map(config.folders.map(folder => [folder.id, createFolderRuntime(config, folder)]));
  if (!folderRuntimes.size) throw new Error('at least one workspace folder is required');
  await syncToolEvolutionRuntimes([...folderRuntimes.values()], usageStore, 'tool_evolution_startup');
  const extensions = new ExtensionRegistry({
    folders: config.folders,
    hooksActive: config.extensions.hooks.active,
    mcpActive: config.extensions.mcp.active,
    enabledHooks: config.extensions.hooks.enabled,
    enabledMcpServers: config.extensions.mcp.enabled
  });
  await extensions.refresh(true);
  const hubAdmission = {
    blocking: new Semaphore(config.limits.globalBlockingConcurrency ?? 1_024),
    process: new Semaphore(config.limits.globalProcessConcurrency ?? 512),
    locks: new KeyedMutex()
  };
  const workspaceProfileId = config.workspaceId ?? deriveWorkspaceProfileId(config.folders);
  const dataDirIdentity = createHash('sha256').update(path.resolve(config.dataDir)).digest('hex').slice(0, 16);
  const conversations = await ConversationStore.open({
    fallbackKey: `runtime-fallback:${workspaceProfileId}:${dataDirIdentity}`,
    persistencePath: path.join(config.dataDir, `conversation-state-${workspaceProfileId}.json`),
    allowedFolderIds: config.folders.map(folder => folder.id)
  });
  const context: ToolContext = {
    config,
    conversations,
    workspaceProfileId,
    selections: conversations.selectionMap,
    defaultCwds: conversations.cwdMap,
    folderRuntimes,
    extensions,
    hubAdmission,
    usage: [],
    usageStore,
    state,
    tunnelStatus: config.tunnel?.enabled
      ? { enabled: true, state: 'stopped', publicUrl: config.tunnel.publicUrl, workers: 1, connectedWorkers: 0, completedRequests: 0 }
      : { enabled: false, state: 'disabled', workers: 0, connectedWorkers: 0, completedRequests: 0 }
  };
  await restoreProcessSessions(context);
  return context;
}

export interface AgentRuntime {
  server: Server;
  context: ToolContext;
  oauth: OAuthRuntime;
  close(): Promise<void>;
}

export interface AgentRuntimeOptions {
  configStore?: ConfigStore;
  mcpHeartbeatIntervalMs?: number;
  requestRestart?: () => void;
  workspaceStore?: WorkspaceManagementStore;
  runtimeRegistry?: Map<string, WorkspaceRuntimeRecord>;
  secretResolver?: (name: string) => string | undefined;
  persistOAuthPassword?: (password: string) => Promise<void>;
}

export async function createAgentRuntime(config: AgentConfig, options: AgentRuntimeOptions = {}): Promise<AgentRuntime> {
  const context = await createToolContext(config);
  context.resolveSecret = options.secretResolver;
  const oauth = new OAuthRuntime(config.oauth, Date.now, options.persistOAuthPassword);
  const startedAt = Date.now();
  const workspaceId = config.workspaceId ?? context.workspaceProfileId;
  context.usageStore.recordDiagnosticEvent({
    eventType: 'lifecycle_event',
    event: 'runtime_created',
    fields: { runtime_kind: 'node_agent' }
  });
  options.runtimeRegistry?.set(workspaceId, {
    context,
    oauth,
    startedAt,
    preflightSandbox: sandbox => preflightSandboxConfiguration(sandbox, context.config.folders, context.config.dataDir)
  });
  const adminToken = options.configStore ? randomBytes(32).toString('base64url') : '';
  const server = createServer(async (req, res) => {
    try {
      const base = externalBase(req.headers, config);
      const prefix = routePrefix(config);
      const url = new URL(req.url ?? '/', `http://${req.headers.host ?? `${config.host}:${config.port}`}`);
      const pathname = localPath(url.pathname, prefix);
      // No auth/session/catalog work on this bounded status-only endpoint.
      if (pathname === '/mcp/pairing') {
        if (req.method !== 'POST') {res.setHeader('Allow','POST');sendJson(res,405,{error:'method_not_allowed'});return;}
        const ticket=req.headers['x-chat-pairing'];
        const accepted=typeof ticket==='string'&&markPairing(ticket,context.config.folders.map(f=>f.path));
        sendJson(res,accepted?202:404,accepted?{status:'preparing',authenticated:false}:{error:'pairing_unavailable'});return;
      }
      await context.extensions.refresh();
      const catalog = currentToolCatalog(context);
      setRuntimeRevisionHeaders(res, catalog, startedAt);

      if (options.configStore && await handleManagementRequest(req, res, url.pathname, {
        configStore: options.configStore,
        context,
        oauth,
        startedAt,
        adminToken,
        requestRestart: options.requestRestart,
        workspaceStore: options.workspaceStore,
        runtimeRegistry: options.runtimeRegistry
      })) return;

      if (await handleOAuthRoute(req, res, { base, localPathname: pathname, oauth, prefix, url })) return;
      if (handleSystemRoute(req, res, { catalog, config, context, pathname, startedAt })) return;
      if (await handleMcpRoute(req, res, {
        base,
        catalog,
        config,
        context,
        heartbeatIntervalMs: options.mcpHeartbeatIntervalMs,
        oauth,
        pathname,
        server,
        startedAt
      })) return;
      if (pathname !== '/mcp') return sendText(res, 404, 'Not found');
    } catch (error) {
      const response = rpcErrorResponse(null, error);
      if (!res.headersSent) return sendJson(res, 200, response);
      res.destroy();
    }
  });
  let cleanupPromise: Promise<void> | undefined;
  const cleanup = (): Promise<void> => {
    if (cleanupPromise) return cleanupPromise;
    cleanupPromise = (async () => {
      if (options.runtimeRegistry?.get(workspaceId)?.context === context) {
        options.runtimeRegistry.delete(workspaceId);
      }
      oauth.dispose();
      const errors: unknown[] = [];
      try {
        await disposeProcessSessions(context);
      } catch (error) {
        errors.push(error);
      }
      try {
        await context.extensions.close();
      } catch (error) {
        errors.push(error);
      }
      const results = await Promise.allSettled([
        context.conversations.flush(),
        context.usageStore.flush(),
        ...allFolderRuntimes(context).flatMap(runtime => [
          runtime.knowledgeIngestor.flush(),
          runtime.toolEvolutionBenchmarkCollector.flush()
        ])
      ]);
      errors.push(...results
        .filter((result): result is PromiseRejectedResult => result.status === 'rejected')
        .map(result => result.reason));
      if (errors.length) throw new AggregateError(errors, 'Agent runtime cleanup failed.');
    })();
    return cleanupPromise;
  };
  let hasListened = false;
  let closed = false;
  let resolveServerClosed: (() => void) | undefined;
  const serverClosed = new Promise<void>(resolve => {
    resolveServerClosed = resolve;
  });
  server.once('listening', () => {
    hasListened = true;
    context.usageStore.recordDiagnosticEvent({
      eventType: 'service_event',
      event: 'mcp_listener_started',
      fields: { service: 'mcp_listener', transport_mode: 'streamable-http' }
    });
  });
  server.once('close', () => {
    closed = true;
    if (hasListened) {
      context.usageStore.recordDiagnosticEvent({
        eventType: 'service_event',
        event: 'mcp_listener_stopped',
        fields: { service: 'mcp_listener', transport_mode: 'streamable-http' }
      });
    }
    context.usageStore.recordDiagnosticEvent({
      eventType: 'lifecycle_event',
      event: 'runtime_closed',
      fields: { runtime_kind: 'node_agent' }
    });
    resolveServerClosed?.();
    void cleanup().catch(() => undefined);
  });
  let closePromise: Promise<void> | undefined;
  const close = (): Promise<void> => {
    if (closePromise) return closePromise;
    closePromise = (async () => {
      if (server.listening) {
        await new Promise<void>((resolve, reject) => {
          server.close(error => error ? reject(error) : resolve());
        });
      } else if (hasListened && !closed) {
        await serverClosed;
      }
      await cleanup();
    })();
    return closePromise;
  };
  return { server, context, oauth, close };
}

export async function createAgentServer(config: AgentConfig, options: AgentRuntimeOptions = {}): Promise<Server> {
  return (await createAgentRuntime(config, options)).server;
}
