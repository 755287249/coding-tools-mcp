import { randomBytes, randomUUID } from 'node:crypto';
import { chmod, mkdir, readFile, rename, stat, writeFile } from 'node:fs/promises';
import path from 'node:path';
import {
  createOAuthClientId,
  loadConfigBundle,
  normalizeConfig,
  recoverWorkspaceFolders,
  resolveConfigPath,
  validateConfig,
  writeConfigDocument,
  type LoadedConfig
} from './config.js';
import { ConfigStore, type RuntimeHotApplyTarget } from './management.js';
import { generateAuthorizationPassword } from './secrets.js';
import type { AgentConfigDocument, JsonObject, WorkspaceRegistryDocument } from './types.js';
import { unwrapJson } from './protect.js';
import { canonicalToAgentConfigDocument, overlayAgentDocumentOnCanonical, parseWorkspacePack } from './workspaceDocument.js';
import {
  findCounterpartSharedWorkspaceId,
  readSharedAgentSecrets,
  readSharedWorkspace,
  sharedSecretsFile,
  sharedStoreAvailable,
  sharedWorkspaceFile,
  sharedWorkspacesRoot,
  writeSharedAgentSecrets,
  writeSharedWorkspace
} from './sharedStore.js';

export { sharedSecretsFile, sharedWorkspaceFile, sharedWorkspacesRoot } from './sharedStore.js';

async function activateSharedStore(loaded: LoadedConfig, id: string, name: string): Promise<LoadedConfig> {
  if (!sharedStoreAvailable()) return loaded;
  const sharedCanonical = await readSharedWorkspace(id);
  const sharedSecrets = await readSharedAgentSecrets(id);
  if (sharedCanonical) {
    loaded.canonical = { ...sharedCanonical, id };
    loaded.document = canonicalToAgentConfigDocument(loaded.canonical);
    if (sharedSecrets) loaded.secrets = sharedSecrets;
    else await writeSharedAgentSecrets(id, loaded.secrets);
    loaded.config = normalizeConfig(loaded.document, loaded.secrets);
    validateConfig(loaded.config);
    const folderRecovery = await recoverWorkspaceFolders(loaded.config, loaded.document);
    loaded.config = folderRecovery.config;
    loaded.document = folderRecovery.document;
    if (folderRecovery.removed.length) {
      loaded.canonical = overlayAgentDocumentOnCanonical(loaded.document, {
        id,
        name: loaded.canonical.name || name
      }, loaded.canonical);
      await writeSharedWorkspace(id, loaded.canonical);
      console.warn(
        `[${name}] Removed unavailable shared workspace folder(s): ${folderRecovery.removed.map(folder => folder.path).join(', ')}`
      );
      if (folderRecovery.fallbackCreated) {
        console.warn(
          `[${name}] No shared workspace folders remained; created fallback workspace: ${loaded.config.folders[0]?.path ?? ''}`
        );
      }
    }
  } else {
    loaded.canonical = { ...loaded.canonical, id, name: loaded.canonical.name || name };
    await writeSharedWorkspace(id, loaded.canonical);
    await writeSharedAgentSecrets(id, loaded.secrets);
  }
  loaded.configPath = sharedWorkspaceFile(id);
  loaded.secretStorePath = sharedSecretsFile(id);
  loaded.secretKeyPath = '';
  return loaded;
}

export interface LoadedWorkspaceProfile {
  id: string;
  name: string;
  loaded: LoadedConfig;
}

export interface LoadedApplication {
  registryPath: string;
  registry: WorkspaceRegistryDocument;
  workspaces: LoadedWorkspaceProfile[];
  registryCreated: boolean;
}

interface ManagedWorkspaceProfile {
  id: string;
  name: string;
  store: ConfigStore;
}

function workspaceId(): string {
  return randomUUID().replaceAll('-', '');
}

function inferredWorkspaceName(loaded: LoadedConfig): string {
  const folders = loaded.config.folders;
  if (folders.length === 1 && folders[0]?.name.trim()) return folders[0].name.trim();
  return path.basename(path.dirname(loaded.configPath)) || 'Workspace';
}

function registryPathFor(configPath: string): string {
  return path.join(path.dirname(configPath), 'workspace-profiles.json');
}

function validateRegistry(value: unknown, registryPath: string): WorkspaceRegistryDocument {
  if (!value || typeof value !== 'object' || Array.isArray(value)) {
    throw new Error(`Workspace registry must be an object: ${registryPath}`);
  }
  const input = value as Record<string, unknown>;
  if (input.schema_version !== 1) throw new Error(`Workspace registry schema_version must be 1: ${registryPath}`);
  if (!Array.isArray(input.workspaces) || input.workspaces.length < 1 || input.workspaces.length > 100) {
    throw new Error(`Workspace registry must contain 1 to 100 workspaces: ${registryPath}`);
  }
  const ids = new Set<string>();
  const configPaths = new Set<string>();
  const base = path.dirname(registryPath);
  const workspaces = input.workspaces.map((raw, index) => {
    if (!raw || typeof raw !== 'object' || Array.isArray(raw)) throw new Error(`workspaces[${index}] must be an object`);
    const entry = raw as Record<string, unknown>;
    const id = String(entry.id ?? '').trim();
    const name = String(entry.name ?? '').trim();
    const rawConfigPath = String(entry.configPath ?? '').trim();
    if (!rawConfigPath) throw new Error(`workspaces[${index}].configPath is required`);
    const configPath = path.resolve(base, rawConfigPath);
    if (!/^[A-Za-z0-9._-]{1,128}$/.test(id)) throw new Error(`workspaces[${index}].id is invalid`);
    if (!name || name.length > 128) throw new Error(`workspaces[${index}].name is invalid`);
    const comparable = process.platform === 'win32' ? configPath.toLowerCase() : configPath;
    if (ids.has(id)) throw new Error(`Workspace registry ID is duplicated: ${id}`);
    if (configPaths.has(comparable)) throw new Error(`Workspace registry config path is duplicated: ${configPath}`);
    ids.add(id);
    configPaths.add(comparable);
    return { id, name, configPath };
  });
  return { schema_version: 1, workspaces };
}

async function writeRegistry(registryPath: string, registry: WorkspaceRegistryDocument): Promise<void> {
  const validated = validateRegistry(registry, registryPath);
  await mkdir(path.dirname(registryPath), { recursive: true, mode: 0o700 });
  const temporary = `${registryPath}.${process.pid}.${randomBytes(6).toString('hex')}.tmp`;
  await writeFile(temporary, `${JSON.stringify(validated, null, 2)}\n`, { flag: 'wx', mode: 0o600 });
  await rename(temporary, registryPath);
  await chmod(registryPath, 0o600).catch(() => undefined);
}

async function readRegistry(registryPath: string): Promise<WorkspaceRegistryDocument | undefined> {
  try {
    return validateRegistry(JSON.parse(await readFile(registryPath, 'utf8')), registryPath);
  } catch (error) {
    if ((error as NodeJS.ErrnoException).code === 'ENOENT') return undefined;
    throw error;
  }
}

function sameConfigPath(left: string, right: string): boolean {
  return process.platform === 'win32'
    ? left.toLowerCase() === right.toLowerCase()
    : left === right;
}

export async function loadApplication(configFile?: string): Promise<LoadedApplication> {
  const primaryConfigPath = resolveConfigPath(configFile);
  const registryPath = registryPathFor(primaryConfigPath);
  let registry = await readRegistry(registryPath);
  let registryCreated = false;
  const primaryEntry = registry?.workspaces.find(entry => sameConfigPath(entry.configPath, primaryConfigPath));
  const primary = await loadConfigBundle(primaryConfigPath, {
    identity: primaryEntry ? { id: primaryEntry.id, name: primaryEntry.name } : undefined
  });
  if (!registry) {
    registry = {
      schema_version: 1,
      workspaces: [{
        id: primary.canonical.id || workspaceId(),
        name: primary.canonical.name || inferredWorkspaceName(primary),
        configPath: primaryConfigPath
      }]
    };
    await writeRegistry(registryPath, registry);
    registryCreated = true;
  }

  const workspaces: LoadedWorkspaceProfile[] = [];
  const ports = new Map<string, string>();
  for (const entry of registry.workspaces) {
    const legacyLoaded = sameConfigPath(entry.configPath, primaryConfigPath)
      ? primary
      : await loadConfigBundle(entry.configPath, { identity: { id: entry.id, name: entry.name } });
    const counterpartId = await findCounterpartSharedWorkspaceId(legacyLoaded.canonical, entry.id, 'desktop');
    if (counterpartId && counterpartId !== entry.id) entry.id = counterpartId;
    const loaded = await activateSharedStore(legacyLoaded, entry.id, entry.name);
    if (loaded.canonical.name && loaded.canonical.name !== entry.name) entry.name = loaded.canonical.name;
    loaded.config.workspaceId = entry.id;
    loaded.config.workspaceName = entry.name;
    const address = `${loaded.config.host}:${loaded.config.port}`.toLowerCase();
    const conflict = ports.get(address);
    if (conflict) throw new Error(`Workspace ports conflict: ${conflict} and ${entry.name} both use ${address}`);
    ports.set(address, entry.name);
    workspaces.push({ id: entry.id, name: entry.name, loaded });
  }
  if (sharedStoreAvailable()) await writeRegistry(registryPath, registry);
  return { registryPath, registry, workspaces, registryCreated };
}

export class ApplicationConfigStore {
  readonly registryPath: string;
  readonly primaryWorkspaceId: string;
  private registry: WorkspaceRegistryDocument;
  private readonly workspaces = new Map<string, ManagedWorkspaceProfile>();

  constructor(application: LoadedApplication) {
    this.registryPath = application.registryPath;
    this.registry = structuredClone(application.registry);
    this.primaryWorkspaceId = application.workspaces[0]?.id ?? '';
    for (const workspace of application.workspaces) {
      this.workspaces.set(workspace.id, {
        id: workspace.id,
        name: workspace.name,
        store: new ConfigStore(workspace.loaded)
      });
    }
  }

  entries(): Array<{ id: string; name: string; store: ConfigStore }> {
    return this.registry.workspaces.map(entry => {
      const workspace = this.workspaces.get(entry.id);
      if (!workspace) throw new Error(`Workspace registry entry is not loaded: ${entry.id}`);
      return workspace;
    });
  }

  snapshot(): JsonObject {
    return {
      schemaVersion: 1,
      registryPath: this.registryPath,
      primaryWorkspaceId: this.primaryWorkspaceId,
      workspaces: this.entries().map(workspace => ({
        id: workspace.id,
        name: workspace.name,
        ...workspace.store.snapshot()
      }))
    };
  }

  async addWorkspace(folderPath: string, name?: string): Promise<JsonObject> {
    const folder = path.resolve(folderPath);
    const metadata = await stat(folder);
    if (!metadata.isDirectory()) throw new Error('Workspace folder must be an existing directory');
    if (this.registry.workspaces.length >= 100) {
      throw new Error('Workspace registry must contain 1 to 100 workspaces');
    }
    const id = workspaceId();
    const workspaceName = (name?.trim() || path.basename(folder) || 'Workspace').slice(0, 128);
    const usedPorts = new Set(this.entries().map(item => item.store.current.port));
    let port = 3789;
    while (usedPorts.has(port)) port += 1;
    if (port > 65_535) throw new Error('No free workspace port is available');
    const workspaceHome = path.join(path.dirname(this.registryPath), 'workspaces', id);
    const configPath = path.join(workspaceHome, 'agent.json');
    const document: AgentConfigDocument = {
      schema_version: 1,
      host: '127.0.0.1',
      port,
      dataDir: workspaceHome,
      folders: [{ path: folder, name: workspaceName }],
      management: { enabled: false },
      oauth: { clientId: createOAuthClientId() },
      tunnel: { enabled: true }
    };
    await writeConfigDocument(configPath, document, { identity: { id, name: workspaceName } });
    const legacyLoaded = await loadConfigBundle(configPath, { identity: { id, name: workspaceName } });
    const loaded = await activateSharedStore(legacyLoaded, id, workspaceName);
    this.workspaces.set(id, { id, name: workspaceName, store: new ConfigStore(loaded) });
    this.registry.workspaces.push({ id, name: workspaceName, configPath });
    await writeRegistry(this.registryPath, this.registry);
    return {
      ...this.workspace(id).store.snapshot(),
      ok: true,
      id,
      name: workspaceName,
      restartRequired: true
    };
  }

  exportWorkspacePack(id: string): JsonObject {
    return this.workspace(id).store.exportWorkspacePack();
  }

  async importSharedWorkspace(id: string): Promise<JsonObject> {
    return this.importSharedWorkspaceFile(sharedWorkspaceFile(id));
  }

  async importSharedWorkspaceFile(filePath: string): Promise<JsonObject> {
    const pack = unwrapJson(JSON.parse(await readFile(filePath, 'utf8')) as unknown, filePath);
    const imported = await this.importWorkspacePack(pack);
    const secretsPath = path.join(path.dirname(filePath), 'secrets.json');
    try {
      const secrets = unwrapJson(JSON.parse(await readFile(secretsPath, 'utf8')) as unknown, secretsPath) as Record<string, unknown>;
      const values = secrets.values && typeof secrets.values === 'object' && !Array.isArray(secrets.values)
        ? secrets.values as Record<string, unknown>
        : {};
      const workspace = this.workspace(String(imported.id));
      await workspace.store.replaceImportedSecrets({
        oauthPassword: typeof values.oauth_password === 'string'
          ? values.oauth_password
          : typeof secrets.oauthPassword === 'string' ? secrets.oauthPassword : undefined,
        oauthClientSecret: typeof values.oauth_client_secret === 'string'
          ? values.oauth_client_secret
          : typeof secrets.oauthClientSecret === 'string' ? secrets.oauthClientSecret : undefined,
        oauthTokenSecret: typeof values.oauth_token_secret === 'string'
          ? values.oauth_token_secret
          : typeof secrets.oauthTokenSecret === 'string' ? secrets.oauthTokenSecret : undefined,
        tunnelEnrollmentUrl: typeof values.builtin_tunnel_enrollment_url === 'string'
          ? values.builtin_tunnel_enrollment_url
          : typeof secrets.tunnelEnrollmentUrl === 'string' ? secrets.tunnelEnrollmentUrl : undefined
      });
    } catch (error) {
      if ((error as NodeJS.ErrnoException).code !== 'ENOENT') throw error;
    }
    return imported;
  }

  async exportSharedWorkspace(id: string): Promise<string> {
    const workspace = this.workspace(id);
    const filePath = await writeSharedWorkspace(id, workspace.store.exportCanonicalDocument());
    const secrets = workspace.store.exportSecretDocument();
    await writeSharedAgentSecrets(id, {
      oauthPassword: typeof secrets.oauthPassword === 'string' ? secrets.oauthPassword : undefined,
      oauthClientSecret: typeof secrets.oauthClientSecret === 'string' ? secrets.oauthClientSecret : undefined,
      oauthTokenSecret: typeof secrets.oauthTokenSecret === 'string' ? secrets.oauthTokenSecret : undefined,
      tunnelEnrollmentUrl: typeof secrets.tunnelEnrollmentUrl === 'string' ? secrets.tunnelEnrollmentUrl : undefined
    });
    return filePath ?? sharedWorkspaceFile(id);
  }

  async importWorkspacePack(pack: unknown): Promise<JsonObject> {
    const { canonical } = parseWorkspacePack(pack);
    for (const folder of canonical.folders) {
      const metadata = await stat(folder.path);
      if (!metadata.isDirectory()) {
        throw new Error(`Workspace folder is missing: ${folder.path}`);
      }
    }
    if (this.registry.workspaces.length >= 100) {
      throw new Error('Workspace registry must contain 1 to 100 workspaces');
    }
    let id = canonical.id.trim();
    if (!id || this.workspaces.has(id) || !/^[A-Za-z0-9._-]{1,128}$/.test(id)) {
      id = workspaceId();
    }
    const workspaceName = (canonical.name.trim() || canonical.folders[0]?.name || 'Workspace').slice(0, 128);
    const usedPorts = new Set(this.entries().map(item => item.store.current.port));
    let port = canonical.bind.port || 3789;
    while (usedPorts.has(port)) port += 1;
    if (port > 65_535) throw new Error('No free workspace port is available');
    const workspaceHome = path.join(path.dirname(this.registryPath), 'workspaces', id);
    const configPath = path.join(workspaceHome, 'agent.json');
    const document = {
      schema_version: 1 as const,
      host: canonical.bind.host || '127.0.0.1',
      port,
      dataDir: workspaceHome,
      publicBaseUrl: canonical.publicBaseUrl || undefined,
      permissionMode: canonical.permissionMode as AgentConfigDocument['permissionMode'],
      toolProfile: canonical.toolProfile,
      securityPolicy: canonical.securityPolicy,
      policy: canonical.policy,
      management: { enabled: false },
      skills: canonical.skills,
      extensions: canonical.extensions,
      sandbox: {
        enabled: canonical.sandbox.enabled,
        backend: canonical.sandbox.backend,
        externalPaths: canonical.sandbox.externalPaths.map(entry => ({
          path: entry.path,
          access: entry.access === 'modify' ? 'modify' as const : 'read_only' as const
        })),
        options: canonical.sandbox.options
      },
      oauth: { clientId: canonical.auth.oauthClientId || createOAuthClientId() },
      folders: canonical.folders.map(folder => ({ id: folder.id, name: folder.name, path: folder.path })),
      limits: canonical.limits,
      ...(canonical.tunnel.builtin.publicUrl
        ? { tunnel: { enabled: canonical.tunnel.builtin.enabled, publicUrl: canonical.tunnel.builtin.publicUrl } }
        : { tunnel: { enabled: canonical.tunnel.builtin.enabled } })
    };
    await writeConfigDocument(configPath, document, {
      identity: { id, name: workspaceName },
      canonical: { ...canonical, id, name: workspaceName, bind: { ...canonical.bind, port } }
    });
    const legacyLoaded = await loadConfigBundle(configPath, { identity: { id, name: workspaceName } });
    const loaded = await activateSharedStore(legacyLoaded, id, workspaceName);
    this.workspaces.set(id, { id, name: workspaceName, store: new ConfigStore(loaded) });
    this.registry.workspaces.push({ id, name: workspaceName, configPath });
    await writeRegistry(this.registryPath, this.registry);
    return {
      ...this.workspace(id).store.snapshot(),
      ok: true,
      id,
      name: workspaceName,
      restartRequired: true
    };
  }

  async deleteWorkspace(id: string): Promise<JsonObject> {
    if (id === this.primaryWorkspaceId) throw new Error('The primary workspace cannot be deleted');
    if (this.registry.workspaces.length <= 1) throw new Error('At least one workspace must remain');
    if (!this.workspaces.has(id)) throw new Error(`Workspace was not found: ${id}`);
    this.registry.workspaces = this.registry.workspaces.filter(item => item.id !== id);
    this.workspaces.delete(id);
    await writeRegistry(this.registryPath, this.registry);
    return { ok: true, id, restartRequired: true };
  }

  workspace(id: string): { id: string; name: string; store: ConfigStore } {
    const workspace = this.workspaces.get(id);
    if (!workspace) throw new Error(`Workspace was not found: ${id}`);
    return workspace;
  }

  async saveWorkspace(id: string, value: unknown, runtime?: RuntimeHotApplyTarget): Promise<JsonObject> {
    const workspace = this.workspace(id);
    const input = value && typeof value === 'object' && !Array.isArray(value)
      ? value as Record<string, unknown>
      : {};
    const name = String(input.name ?? workspace.name).trim();
    if (!name || name.length > 128) throw new Error('workspace name must contain 1 to 128 characters');
    const previousName = workspace.store.current.workspaceName;
    workspace.store.current.workspaceName = name;
    let result: JsonObject;
    try {
      result = await workspace.store.save(input, runtime);
    } catch (error) {
      workspace.store.current.workspaceName = previousName;
      throw error;
    }
    if (name !== workspace.name) {
      workspace.name = name;
      const entry = this.registry.workspaces.find(item => item.id === id);
      if (!entry) throw new Error(`Workspace registry entry is missing: ${id}`);
      entry.name = name;
      await writeRegistry(this.registryPath, this.registry);
      workspace.store.current.workspaceName = name;
      if (runtime) runtime.context.config.workspaceName = name;
    }
    return { ...result, id, name };
  }

  async setSkillActive(
    id: string,
    active: boolean,
    runtime?: RuntimeHotApplyTarget
  ): Promise<JsonObject> {
    const workspace = this.workspace(id);
    const result = await workspace.store.setSkillActive(active, runtime);
    return { ...result, id, name: workspace.name };
  }

  async setSkillEnabled(
    id: string,
    key: string,
    enabled: boolean,
    runtime?: RuntimeHotApplyTarget
  ): Promise<JsonObject> {
    const workspace = this.workspace(id);
    const result = await workspace.store.setSkillEnabled(key, enabled, runtime);
    return { ...result, id, name: workspace.name };
  }

  async setExtensionActive(
    id: string,
    kind: 'hook' | 'mcp',
    active: boolean,
    runtime?: RuntimeHotApplyTarget
  ): Promise<JsonObject> {
    const workspace = this.workspace(id);
    const result = await workspace.store.setExtensionActive(kind, active, runtime);
    return { ...result, id, name: workspace.name };
  }

  async setExtensionEnabled(
    id: string,
    kind: 'hook' | 'mcp',
    key: string,
    enabled: boolean,
    runtime?: RuntimeHotApplyTarget
  ): Promise<JsonObject> {
    const workspace = this.workspace(id);
    const result = await workspace.store.setExtensionEnabled(kind, key, enabled, runtime);
    return { ...result, id, name: workspace.name };
  }

  secret(id: string, key: 'oauthPassword'): string {
    return this.workspace(id).store.secret(key);
  }

  async replaceSecret(
    id: string,
    key: 'oauthPassword' | 'tunnelEnrollmentUrl',
    value: string,
    runtime?: RuntimeHotApplyTarget
  ): Promise<JsonObject> {
    const workspace = this.workspace(id);
    const applied = await workspace.store.replaceSecret(key, value, runtime);
    return {
      ok: true,
      workspaceId: id,
      key,
      restartRequired: !applied,
      appliedImmediately: applied ? [key === 'oauthPassword' ? 'oauth' : 'tunnel'] : []
    };
  }

  async replaceNamedSecret(id: string, reference: string, value: string): Promise<JsonObject> {
    const workspace = this.workspace(id);
    await workspace.store.replaceNamedSecret(reference, value);
    return {
      ok: true,
      workspaceId: id,
      key: reference,
      restartRequired: false,
      appliedImmediately: ['process-secrets']
    };
  }

  async regenerateSecret(id: string, key: 'oauthPassword', runtime?: RuntimeHotApplyTarget): Promise<JsonObject> {
    const value = generateAuthorizationPassword();
    const result = await this.replaceSecret(id, key, value, runtime);
    return { ...result, value };
  }

  applyResolvedBuiltinTunnel(id: string, publicUrl: string, enrollmentCompleted: boolean): Promise<void> {
    return this.workspace(id).store.applyResolvedBuiltinTunnel(publicUrl, enrollmentCompleted);
  }
}
