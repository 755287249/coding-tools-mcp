import path from 'node:path';
import type { IncomingMessage, ServerResponse } from 'node:http';
import { sendJson } from '../../oauth.js';
import { readManagementBody, sendManagementError } from '../http.js';
import { runtimeRecord } from '../runtime.js';
import type { ManagementOptions } from '../types.js';

async function workspaceSkillsPayload(options: ManagementOptions, workspaceId: string) {
  const selected = runtimeRecord(options, workspaceId);
  if (!selected) throw new Error(`Workspace runtime is unavailable: ${workspaceId}`);
  const skills = new Map<string, {
    key: string;
    name: string;
    description: string;
    source: string;
    scope: string;
    relativePath: string;
    rootRelativePath: string;
    version: string | null;
    selected: boolean;
    enabled: boolean;
    folderId: string | null;
    folderName: string | null;
  }>();
  const diagnostics = new Map<string, unknown>();
  for (const folder of selected.context.config.folders) {
    const folderRuntime = selected.context.folderRuntimes.get(folder.id);
    if (!folderRuntime) continue;
    const inventory = await folderRuntime.skillRegistry.inventory();
    for (const item of inventory.skills) {
      const skill = item.skill;
      if (skills.has(skill.key)) continue;
      skills.set(skill.key, {
        key: skill.key,
        name: skill.name,
        description: skill.description,
        source: skill.source,
        scope: skill.scope,
        relativePath: skill.relativePath,
        rootRelativePath: skill.rootRelativePath,
        version: skill.version ?? null,
        selected: item.selected,
        enabled: item.enabled,
        folderId: skill.scope === 'workspace' ? folder.id : null,
        folderName: skill.scope === 'workspace' ? folder.name : null
      });
    }
    for (const diagnostic of inventory.diagnostics) {
      const identity = JSON.stringify([
        diagnostic.code,
        diagnostic.path ?? '',
        diagnostic.name ?? '',
        diagnostic.source ?? '',
        diagnostic.scope ?? ''
      ]);
      if (!diagnostics.has(identity)) diagnostics.set(identity, diagnostic);
    }
  }
  return {
    ok: true,
    workspaceId,
    active: selected.context.config.skills.active,
    skills: [...skills.values()].sort((left, right) => left.name.localeCompare(right.name) || left.key.localeCompare(right.key)),
    diagnostics: [...diagnostics.values()]
  };
}

function endpointLabel(value: string | undefined): string | null {
  if (!value) return null;
  try { return new URL(value).origin; }
  catch { return null; }
}

async function workspaceExtensionsPayload(options: ManagementOptions, workspaceId: string) {
  const selected = runtimeRecord(options, workspaceId);
  if (!selected) throw new Error(`Workspace runtime is unavailable: ${workspaceId}`);
  const inventory = await selected.context.extensions.inventory(true);
  return {
    ok: true,
    workspaceId,
    hooksActive: selected.context.config.extensions.hooks.active,
    mcpActive: selected.context.config.extensions.mcp.active,
    hooks: inventory.hooks.map(item => ({
      key: item.hook.key,
      provider: item.hook.provider,
      scope: item.hook.scope,
      folderId: item.hook.folderId ?? null,
      event: item.hook.event,
      matcher: item.hook.matcher ?? null,
      handlerType: item.hook.handlerType,
      sourcePath: item.hook.sourcePath,
      sourceEnabled: item.hook.sourceEnabled,
      supported: item.hook.supported,
      selected: item.selected,
      enabled: item.enabled,
      command: item.hook.command ? path.basename(item.hook.command) : null,
      endpoint: endpointLabel(item.hook.url)
    })),
    mcpServers: inventory.mcpServers.map(item => ({
      key: item.server.key,
      provider: item.server.provider,
      scope: item.server.scope,
      folderId: item.server.folderId ?? null,
      name: item.server.name,
      transport: item.server.transport,
      sourcePath: item.server.sourcePath,
      sourceEnabled: item.server.sourceEnabled,
      supported: item.server.supported,
      selected: item.selected,
      enabled: item.enabled,
      connected: item.connected,
      toolCount: item.toolCount,
      command: item.server.command ? path.basename(item.server.command) : null,
      endpoint: endpointLabel(item.server.url),
      error: item.error ? 'Connection failed. See Agent logs for details.' : null
    })),
    diagnostics: inventory.diagnostics
  };
}

export async function handleManagementConfigurationRoute(
  req: IncomingMessage,
  res: ServerResponse,
  pathname: string,
  options: ManagementOptions
): Promise<boolean> {
  if (pathname === '/admin/api/restart' && req.method === 'POST') {
    if (!options.requestRestart) {
      sendJson(res, 409, {
        error: {
          code: 'RESTART_UNAVAILABLE',
          message: 'Restart requires a supervisor. Launch the Agent with start-node-agent.bat.'
        }
      });
      return true;
    }
    sendJson(res, 202, { ok: true, restarting: true });
    setImmediate(() => options.requestRestart?.());
    return true;
  }

  if (pathname === '/admin/api/config' && req.method === 'GET') {
    sendJson(res, 200, options.workspaceStore?.snapshot() ?? options.configStore.snapshot());
    return true;
  }
  if (pathname === '/admin/api/config' && req.method === 'PUT') {
    try {
      const workspaceId = options.context.config.workspaceId ?? options.context.workspaceProfileId;
      const result = await options.configStore.save(
        await readManagementBody(req),
        runtimeRecord(options, workspaceId)
      );
      sendJson(res, 200, result);
    } catch (error) {
      sendManagementError(res, error);
    }
    return true;
  }

  if (pathname === '/admin/api/workspaces' && req.method === 'POST') {
    if (!options.workspaceStore) {
      sendJson(res, 409, {
        error: { code: 'WORKSPACE_REGISTRY_UNAVAILABLE', message: 'Workspace registry is unavailable.' }
      });
      return true;
    }
    try {
      const body = await readManagementBody(req);
      const input = body && typeof body === 'object' && !Array.isArray(body) ? body as Record<string, unknown> : {};
      sendJson(res, 200, await options.workspaceStore.addWorkspace(
        String(input.path ?? ''),
        input.name === undefined ? undefined : String(input.name)
      ));
    } catch (error) {
      sendManagementError(res, error);
    }
    return true;
  }

  const deleteWorkspace = pathname.match(/^\/admin\/api\/workspaces\/([A-Za-z0-9._-]{1,128})$/);
  if (deleteWorkspace && req.method === 'DELETE') {
    if (!options.workspaceStore) {
      sendJson(res, 409, {
        error: { code: 'WORKSPACE_REGISTRY_UNAVAILABLE', message: 'Workspace registry is unavailable.' }
      });
      return true;
    }
    try {
      sendJson(res, 200, await options.workspaceStore.deleteWorkspace(deleteWorkspace[1]!));
    } catch (error) {
      sendManagementError(res, error);
    }
    return true;
  }

  const tunnelRoute = pathname.match(
    /^\/admin\/api\/workspaces\/([A-Za-z0-9._-]{1,128})\/tunnel\/(start|stop|test|restart)$/
  );
  if (tunnelRoute && req.method === 'POST') {
    const workspaceId = tunnelRoute[1]!;
    const action = tunnelRoute[2]!;
    const selected = runtimeRecord(options, workspaceId);
    if (!selected) {
      sendJson(res, 404, {
        error: { code: 'WORKSPACE_NOT_FOUND', message: `Workspace was not found: ${workspaceId}` }
      });
      return true;
    }
    try {
      const body = await readManagementBody(req).catch(() => ({}));
      const input = body && typeof body === 'object' && !Array.isArray(body) ? body as Record<string, unknown> : {};
      if (String(input.service ?? 'mcp') === 'actions') {
        sendJson(res, 400, {
          error: { code: 'ACTIONS_UNAVAILABLE', message: 'The Node Agent does not run Actions tunnels.' }
        });
        return true;
      }
      const tunnel = selected.tunnel;
      if (action === 'stop') {
        if (typeof tunnel?.stop !== 'function') throw new Error('Tunnel runtime is unavailable');
        await tunnel.stop();
      } else {
        if (typeof tunnel?.start !== 'function') throw new Error('Tunnel runtime is unavailable');
        if (action === 'restart' && typeof tunnel.stop === 'function') await tunnel.stop();
        await tunnel.start();
      }
      const status = selected.context.tunnelStatus;
      const publicUrl = status?.publicUrl ?? selected.context.config.tunnel?.publicUrl ?? '';
      const running = status?.state === 'running' || status?.state === 'starting';
      sendJson(res, 200, action === 'test'
        ? {
            success: running && Boolean(publicUrl || status?.state === 'running'),
            publicUrl,
            keptRunning: true,
            message: running
              ? 'Built-in WSS is running.'
              : (status?.lastError ?? 'Built-in WSS is not connected.')
          }
        : {
            state: status?.state ?? 'stopped',
            publicUrl,
            tunnelPid: null,
            configuredWorkers: status?.workers ?? null,
            connectedWorkers: status?.connectedWorkers ?? null,
            idleWorkers: status?.idleWorkers ?? null,
            busyWorkers: status?.busyWorkers ?? null,
            recycledWorkers: status?.recycledWorkers ?? null,
            policyRevision: status?.policyRevision ?? null,
            lastError: status?.lastError ?? null
          });
    } catch (error) {
      sendManagementError(res, error);
    }
    return true;
  }

  const extensionRoute = pathname.match(/^\/admin\/api\/workspaces\/([A-Za-z0-9._-]{1,128})\/extensions$/);
  if (extensionRoute) {
    const workspaceId = extensionRoute[1]!;
    try {
      if (req.method === 'GET') {
        sendJson(res, 200, await workspaceExtensionsPayload(options, workspaceId));
        return true;
      }
      if (req.method === 'PUT') {
        const body = await readManagementBody(req);
        if (!body || typeof body !== 'object' || Array.isArray(body)) throw new Error('extension update must be an object');
        const input = body as Record<string, unknown>;
        const kind = String(input.kind ?? '').trim();
        if (kind !== 'hook' && kind !== 'mcp') throw new Error('kind must be hook or mcp');
        if ('active' in input) {
          if (typeof input.active !== 'boolean') throw new Error('active must be a boolean');
          if ('key' in input || 'enabled' in input) throw new Error('master extension update must not include key or enabled');
          const selected = runtimeRecord(options, workspaceId);
          const result = options.workspaceStore
            ? await options.workspaceStore.setExtensionActive(workspaceId, kind, input.active, selected)
            : await options.configStore.setExtensionActive(kind, input.active, selected);
          sendJson(res, 200, { ...result, workspaceId, extensionKind: kind, active: input.active });
          return true;
        }
        const key = String(input.key ?? '').trim();
        if (!key || key.length > 4096) throw new Error('extension key must contain 1 to 4096 characters');
        if (typeof input.enabled !== 'boolean') throw new Error('enabled must be a boolean');
        const inventory = await workspaceExtensionsPayload(options, workspaceId);
        const candidates = kind === 'hook' ? inventory.hooks : inventory.mcpServers;
        const candidate = candidates.find(item => item.key === key);
        if (!candidate) throw new Error(`Extension was not found: ${key}`);
        if (!candidate.supported) throw new Error('This extension type is discoverable but not supported by Node Agent.');
        if (!candidate.sourceEnabled && input.enabled) throw new Error('This extension is disabled by its source configuration.');
        const selected = runtimeRecord(options, workspaceId);
        const result = options.workspaceStore
          ? await options.workspaceStore.setExtensionEnabled(workspaceId, kind, key, input.enabled, selected)
          : await options.configStore.setExtensionEnabled(kind, key, input.enabled, selected);
        sendJson(res, 200, { ...result, workspaceId, extensionKind: kind, extensionKey: key, enabled: input.enabled });
        return true;
      }
    } catch (error) {
      sendManagementError(res, error);
      return true;
    }
    return false;
  }

  const skillRoute = pathname.match(/^\/admin\/api\/workspaces\/([A-Za-z0-9._-]{1,128})\/skills$/);
  if (skillRoute) {
    const workspaceId = skillRoute[1]!;
    try {
      if (req.method === 'GET') {
        sendJson(res, 200, await workspaceSkillsPayload(options, workspaceId));
        return true;
      }
      if (req.method === 'PUT') {
        const body = await readManagementBody(req);
        if (!body || typeof body !== 'object' || Array.isArray(body)) throw new Error('skill update must be an object');
        const input = body as Record<string, unknown>;
        if ('active' in input) {
          if (typeof input.active !== 'boolean') throw new Error('active must be a boolean');
          if ('key' in input || 'enabled' in input) throw new Error('master Skill update must not include key or enabled');
          const selected = runtimeRecord(options, workspaceId);
          const result = options.workspaceStore
            ? await options.workspaceStore.setSkillActive(workspaceId, input.active, selected)
            : await options.configStore.setSkillActive(input.active, selected);
          sendJson(res, 200, { ...result, workspaceId, active: input.active });
          return true;
        }
        const key = String(input.key ?? '').trim();
        if (!key || key.length > 4096) throw new Error('skill key must contain 1 to 4096 characters');
        if (typeof input.enabled !== 'boolean') throw new Error('enabled must be a boolean');
        const inventory = await workspaceSkillsPayload(options, workspaceId);
        if (!inventory.skills.some(skill => skill.key === key)) throw new Error(`Skill was not found: ${key}`);
        const selected = runtimeRecord(options, workspaceId);
        const result = options.workspaceStore
          ? await options.workspaceStore.setSkillEnabled(workspaceId, key, input.enabled, selected)
          : await options.configStore.setSkillEnabled(key, input.enabled, selected);
        sendJson(res, 200, {
          ...result,
          workspaceId,
          skillKey: key,
          enabled: input.enabled
        });
        return true;
      }
    } catch (error) {
      sendManagementError(res, error);
      return true;
    }
    return false;
  }

  const namedSecretRoute = pathname.match(
    /^\/admin\/api\/workspaces\/([A-Za-z0-9._-]{1,128})\/secrets\/named\/([A-Za-z0-9._-]{1,128})$/
  );
  if (namedSecretRoute && options.workspaceStore && req.method === 'PUT') {
    const [, workspaceId, reference] = namedSecretRoute;
    try {
      const body = await readManagementBody(req);
      if (!body || typeof body !== 'object' || Array.isArray(body)) throw new Error('secret update must be an object');
      const value = (body as Record<string, unknown>).value;
      if (typeof value !== 'string') throw new Error('secret value must be a string');
      sendJson(res, 200, await options.workspaceStore.replaceNamedSecret(workspaceId, reference, value));
    } catch (error) {
      sendManagementError(res, error);
    }
    return true;
  }

  const route = pathname.match(
    /^\/admin\/api\/workspaces\/([A-Za-z0-9._-]{1,128})\/(config|secrets\/(?:oauth-password(?:\/regenerate)?|builtin-tunnel-enrollment-url))$/
  );
  if (!route || !options.workspaceStore) return false;

  const [, workspaceId, action] = route;
  try {
    if (action === 'config' && req.method === 'PUT') {
      const selected = runtimeRecord(options, workspaceId);
      sendJson(res, 200, await options.workspaceStore.saveWorkspace(
        workspaceId,
        await readManagementBody(req),
        selected
      ));
      return true;
    }
    if (action === 'secrets/oauth-password' && req.method === 'GET') {
      sendJson(res, 200, {
        ok: true,
        workspaceId,
        value: options.workspaceStore.secret(workspaceId, 'oauthPassword')
      });
      return true;
    }
    if ((action === 'secrets/oauth-password' || action === 'secrets/builtin-tunnel-enrollment-url') && req.method === 'PUT') {
      const body = await readManagementBody(req);
      if (!body || typeof body !== 'object' || Array.isArray(body)) throw new Error('secret update must be an object');
      const value = (body as Record<string, unknown>).value;
      if (typeof value !== 'string') throw new Error('secret value must be a string');
      sendJson(res, 200, await options.workspaceStore.replaceSecret(
        workspaceId,
        action === 'secrets/oauth-password' ? 'oauthPassword' : 'tunnelEnrollmentUrl',
        value,
        runtimeRecord(options, workspaceId)
      ));
      return true;
    }
    if (action === 'secrets/oauth-password/regenerate' && req.method === 'POST') {
      sendJson(res, 200, await options.workspaceStore.regenerateSecret(
        workspaceId,
        'oauthPassword',
        runtimeRecord(options, workspaceId)
      ));
      return true;
    }
  } catch (error) {
    sendManagementError(res, error);
    return true;
  }
  return false;
}
