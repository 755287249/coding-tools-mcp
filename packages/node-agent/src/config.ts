import { randomBytes, randomUUID } from 'node:crypto';
import { homedir } from 'node:os';
import { chmod, copyFile, mkdir, readFile, rename, rm, stat, writeFile } from 'node:fs/promises';
import path from 'node:path';
import { ensureAgentSecrets } from './secrets.js';
import { defaultPolicy, mergeAllowedCommands, normalizeScriptExtensions } from './policy.js';
import { configuredToolProfile, resolveToolProfile } from './catalog.js';
import { compatibilityToolProfile, normalizeSecurityPolicy } from './securityPolicy.js';
import type {
  AgentConfig, AgentConfigDocument, AgentSecrets, PermissionMode, SandboxConfig, SandboxPathGrant, WorkspaceFolder, WorkspaceFolderDocument
} from './types.js';
import { normalizeWorkspacePath, workspaceBasename } from './wsl.js';
import { canonicalizeWorkspaceFolders, validateUniqueWorkspaceFolders } from './workspace.js';
import { ABSOLUTE_COMMAND_TIMEOUT_MAX_MS, DEFAULT_COMMAND_TIMEOUT_MAX_MS } from './executionLimits.js';
import {
  CANONICAL_SCHEMA_VERSION,
  canonicalToAgentConfigDocument,
  extractPlaintextSecrets,
  looksLikeCanonicalWorkspace,
  overlayAgentDocumentOnCanonical,
  parseCanonicalWorkspace,
  resolveWorkspaceIdentity,
  serializeCanonicalWorkspace,
  type CanonicalWorkspace,
  type WorkspaceIdentity
} from './workspaceDocument.js';

export const CURRENT_CONFIG_SCHEMA_VERSION = 1 as const;
export { CANONICAL_SCHEMA_VERSION };

export function createOAuthClientId(): string {
  return `chatgpt-client-${randomUUID().slice(0, 12)}`;
}

export function createWorkspaceFolderId(): string {
  return randomUUID().replaceAll('-', '');
}

export function normalizeWorkspaceFolderDocuments(
  folders: readonly WorkspaceFolderDocument[]
): WorkspaceFolder[] {
  const ids = new Set<string>();
  return folders.map((folder, index) => {
    const folderPath = normalizeWorkspacePath(folder.path);
    const id = folder.id?.trim() || createWorkspaceFolderId();
    const name = folder.name?.trim() || workspaceBasename(folderPath);
    if (!/^[A-Za-z0-9._-]+$/.test(id)) throw new Error(`folders[${index}].id contains unsupported characters`);
    if (ids.has(id)) throw new Error(`workspace folder id must be unique: ${id}`);
    ids.add(id);
    return { id, name, path: folderPath };
  });
}

export const defaultDataDir = process.platform === 'win32'
  ? path.join(process.env.LOCALAPPDATA ?? homedir(), 'CodingToolsMCPNode')
  : path.join(homedir(), '.coding-tools-mcp-node');

export interface LoadedConfig {
  config: AgentConfig;
  configPath: string;
  document: AgentConfigDocument;
  canonical: CanonicalWorkspace;
  secrets: AgentSecrets;
  secretStorePath: string;
  secretKeyPath: string;
  migrationApplied: boolean;
  migratedFromSchema?: number;
}

export interface WriteConfigDocumentOptions {
  identity?: WorkspaceIdentity;
  canonical?: CanonicalWorkspace;
}

interface MigrationResult {
  document: AgentConfigDocument;
  canonical?: CanonicalWorkspace;
  secrets: AgentSecrets;
  changed: boolean;
  fromSchema: number;
}

function defaultWorkspaceFolders(): WorkspaceFolder[] {
  return [{ id: 'default', name: path.basename(process.cwd()), path: process.cwd() }];
}

function parseFolders(value: string | undefined): WorkspaceFolder[] {
  const raw = value?.trim();
  if (!raw) return defaultWorkspaceFolders();
  const folderPaths = raw.split(path.delimiter).map(folderPath => folderPath.trim()).filter(Boolean);
  if (!folderPaths.length) return defaultWorkspaceFolders();
  return folderPaths.map((folderPath, index) => ({
    id: `folder-${index + 1}`,
    name: workspaceBasename(folderPath),
    path: normalizeWorkspacePath(folderPath)
  }));
}

function missingWorkspacePath(error: unknown): boolean {
  return Boolean(
    error
    && typeof error === 'object'
    && 'code' in error
    && ['ENOENT', 'ENOTDIR'].includes(String((error as NodeJS.ErrnoException).code))
  );
}

export async function recoverWorkspaceFolders(
  config: AgentConfig,
  document: AgentConfigDocument
): Promise<{
  config: AgentConfig;
  document: AgentConfigDocument;
  removed: WorkspaceFolder[];
  fallbackCreated: boolean;
}> {
  const available: WorkspaceFolder[] = [];
  const removed: WorkspaceFolder[] = [];
  for (const folder of config.folders) {
    try {
      const metadata = await stat(folder.path);
      if (!metadata.isDirectory()) {
        removed.push(folder);
        continue;
      }
      available.push(folder);
    } catch (error) {
      if (!missingWorkspacePath(error)) throw error;
      removed.push(folder);
    }
  }

  let fallbackCreated = false;
  if (!available.length && removed.length) {
    const fallbackPath = path.join(config.dataDir, 'workspace');
    await mkdir(fallbackPath, { recursive: true, mode: 0o700 });
    available.push({ id: 'default', name: 'workspace', path: fallbackPath });
    fallbackCreated = true;
  }

  const folders = await canonicalizeWorkspaceFolders(available.length ? available : config.folders);
  if (!removed.length) {
    return { config: { ...config, folders }, document, removed, fallbackCreated };
  }

  return {
    config: { ...config, folders },
    document: {
      ...document,
      folders: folders.map(folder => ({ id: folder.id, name: folder.name, path: folder.path }))
    },
    removed,
    fallbackCreated
  };
}

function positiveInt(value: unknown, fallback: number, max = Number.MAX_SAFE_INTEGER): number {
  const parsed = Number(value);
  return Number.isInteger(parsed) && parsed > 0 ? Math.min(parsed, max) : fallback;
}

function permissionMode(value: unknown): PermissionMode {
  return ['read-only', 'guarded', 'trusted', 'dangerous'].includes(String(value))
    ? String(value) as PermissionMode
    : 'trusted';
}

function enabled(value: unknown, fallback: boolean): boolean {
  if (typeof value === 'boolean') return value;
  if (typeof value === 'string') return !['0', 'false', 'no', 'off'].includes(value.toLowerCase());
  return fallback;
}

function disabledSkillKeys(value: unknown): string[] {
  if (value === undefined) return [];
  if (!Array.isArray(value) || value.length > 1024) {
    throw new Error('skills.disabled must contain at most 1024 entries');
  }
  const unique = new Set<string>();
  for (const [index, raw] of value.entries()) {
    if (typeof raw !== 'string') throw new Error(`skills.disabled[${index}] must be a string`);
    const key = raw.trim();
    if (!key || key.length > 4096) {
      throw new Error(`skills.disabled[${index}] must contain 1 to 4096 characters`);
    }
    unique.add(key);
  }
  return [...unique].sort();
}

function enabledExtensionKeys(value: unknown, name: string): string[] {
  if (value === undefined) return [];
  if (!Array.isArray(value) || value.length > 1024) {
    throw new Error(`${name} must contain at most 1024 entries`);
  }
  const unique = new Set<string>();
  for (const [index, raw] of value.entries()) {
    if (typeof raw !== 'string') throw new Error(`${name}[${index}] must be a string`);
    const key = raw.trim();
    if (!key || key.length > 4096) throw new Error(`${name}[${index}] must contain 1 to 4096 characters`);
    unique.add(key);
  }
  return [...unique].sort();
}

function sandboxConfig(input: AgentConfigDocument['sandbox'], environment: NodeJS.ProcessEnv): SandboxConfig {
  const externalPaths: SandboxPathGrant[] = (input?.externalPaths ?? []).map((grant, index) => {
    if (!grant || typeof grant !== 'object') throw new Error(`sandbox.externalPaths[${index}] must be an object`);
    const grantPath = String(grant.path ?? '').trim();
    if (!grantPath) throw new Error(`sandbox.externalPaths[${index}].path is required`);
    const access = String(grant.access ?? 'read_only');
    if (access !== 'read_only' && access !== 'modify') {
      throw new Error(`sandbox.externalPaths[${index}].access must be read_only or modify`);
    }
    return { path: grantPath, access };
  });
  const options: Record<string, string> = {};
  for (const [key, value] of Object.entries(input?.options ?? {})) {
    const normalizedKey = key.trim();
    if (!normalizedKey || normalizedKey.length > 128) throw new Error('sandbox option keys must contain 1 to 128 characters');
    const normalizedValue = String(value);
    if (normalizedValue.length > 4096) throw new Error(`sandbox option ${normalizedKey} exceeds 4096 characters`);
    options[normalizedKey] = normalizedValue;
  }
  if (environment.CTMCP_WSLC_IMAGE !== undefined) options['wslc.image'] = environment.CTMCP_WSLC_IMAGE;
  if (environment.CTMCP_WSLC_NETWORK !== undefined) options['wslc.network'] = environment.CTMCP_WSLC_NETWORK;
  if (environment.CTMCP_WSLC_SESSION_STORAGE !== undefined) {
    options['wslc.session_storage'] = environment.CTMCP_WSLC_SESSION_STORAGE;
  }
  if (environment.CTMCP_DOCKER_IMAGE !== undefined) options['docker.image'] = environment.CTMCP_DOCKER_IMAGE;
  if (environment.CTMCP_DOCKER_NETWORK !== undefined) options['docker.network'] = environment.CTMCP_DOCKER_NETWORK;
  if (environment.CTMCP_PODMAN_IMAGE !== undefined) options['podman.image'] = environment.CTMCP_PODMAN_IMAGE;
  if (environment.CTMCP_PODMAN_NETWORK !== undefined) options['podman.network'] = environment.CTMCP_PODMAN_NETWORK;
  const requestedEnabled = enabled(environment.CTMCP_SANDBOX_ENABLED ?? input?.enabled, false);
  return {
    enabled: requestedEnabled,
    backend: String(environment.CTMCP_SANDBOX_BACKEND ?? input?.backend ?? 'appcontainer').trim() || 'appcontainer',
    externalPaths,
    options
  };
}

function record(value: unknown, name: string): Record<string, unknown> {
  if (!value || typeof value !== 'object' || Array.isArray(value)) throw new Error(`${name} must be an object`);
  return value as Record<string, unknown>;
}

function optionalRecord(value: unknown, name: string): Record<string, unknown> {
  return value === undefined ? {} : record(value, name);
}

function legacySecret(value: unknown, name: string): string | undefined {
  if (value === undefined || value === null || value === '') return undefined;
  if (typeof value !== 'string') throw new Error(`${name} must be a string`);
  if (value.length > 4096) throw new Error(`${name} exceeds 4096 characters`);
  return value;
}

export function resolveConfigPath(file?: string): string {
  const configuredDataDir = path.resolve(process.env.CTMCP_DATA_DIR ?? defaultDataDir);
  return path.resolve(file ?? process.env.CTMCP_CONFIG_FILE ?? path.join(configuredDataDir, 'agent.json'));
}

export function resolveDataDir(document: AgentConfigDocument, environment: NodeJS.ProcessEnv = process.env): string {
  return path.resolve(environment.CTMCP_DATA_DIR ?? document.dataDir ?? defaultDataDir);
}

export function normalizeConfig(
  input: AgentConfigDocument,
  secrets: AgentSecrets = {},
  environment: NodeJS.ProcessEnv = process.env
): AgentConfig {
  const dataDir = resolveDataDir(input, environment);
  const publicUrl = environment.CTMCP_BUILTIN_PUBLIC_URL ?? input.tunnel?.publicUrl ?? '';
  const tokenSecret = environment.CTMCP_OAUTH_TOKEN_SECRET ?? secrets.oauthTokenSecret ?? '';
  const environmentFolders = environment.CTMCP_WORKSPACES;
  const effectivePermissionMode = permissionMode(environment.CTMCP_PERMISSION_MODE ?? input.permissionMode);
  const toolProfile = configuredToolProfile(environment.CTMCP_TOOL_PROFILE ?? input.toolProfile);
  const securityPolicy = normalizeSecurityPolicy(input.securityPolicy, effectivePermissionMode, toolProfile);
  const securityPolicyCustomized = input.securityPolicy !== undefined;
  const activeToolProfile = input.securityPolicy
    ? resolveToolProfile(compatibilityToolProfile(securityPolicy), 'trusted')
    : resolveToolProfile(toolProfile, effectivePermissionMode);
  return {
    host: environment.CTMCP_HOST ?? input.host ?? '127.0.0.1',
    port: positiveInt(environment.CTMCP_PORT ?? input.port, 3789, 65_535),
    publicBaseUrl: environment.CTMCP_PUBLIC_BASE_URL ?? input.publicBaseUrl ?? derivePublicBase(publicUrl),
    dataDir,
    permissionMode: effectivePermissionMode,
    toolProfile,
    activeToolProfile,
    securityPolicy,
    securityPolicyCustomized,
    policy: {
      allowedCommands: mergeAllowedCommands(environment.CTMCP_ALLOWED_COMMANDS ?? input.policy?.allowedCommands),
      workspaceLocalEntries: enabled(environment.CTMCP_WORKSPACE_LOCAL_ENTRIES ?? input.policy?.workspaceLocalEntries, true),
      workspaceScriptExtensions: normalizeScriptExtensions(environment.CTMCP_WORKSPACE_SCRIPT_EXTENSIONS ?? input.policy?.workspaceScriptExtensions),
      maxPatchBytes: positiveInt(environment.CTMCP_MAX_PATCH_BYTES ?? input.policy?.maxPatchBytes, defaultPolicy().maxPatchBytes, 16 * 1024 * 1024)
    },
    management: {
      enabled: enabled(environment.CTMCP_UI_ENABLED ?? input.management?.enabled, true)
    },
    skills: {
      active: enabled(input.skills?.active, true),
      disabled: disabledSkillKeys(input.skills?.disabled)
    },
    extensions: {
      hooks: {
        active: enabled(input.extensions?.hooks?.active, true),
        enabled: enabledExtensionKeys(input.extensions?.hooks?.enabled, 'extensions.hooks.enabled')
      },
      mcp: {
        active: enabled(input.extensions?.mcp?.active, true),
        enabled: enabledExtensionKeys(input.extensions?.mcp?.enabled, 'extensions.mcp.enabled')
      }
    },
    sandbox: sandboxConfig(input.sandbox, environment),
    oauth: {
      clientId: environment.CTMCP_OAUTH_CLIENT_ID ?? input.oauth?.clientId ?? createOAuthClientId(),
      clientSecret: environment.CTMCP_OAUTH_CLIENT_SECRET ?? secrets.oauthClientSecret,
      password: environment.CTMCP_OAUTH_PASSWORD ?? secrets.oauthPassword ?? 'change-me',
      tokenSecret,
      // Keepalive: ten-year tokens; the legacy workspace TTL field is ignored (env override only).
      tokenTtlSeconds: positiveInt(
        environment.CTMCP_OAUTH_TOKEN_TTL_SECONDS,
        3650 * 24 * 60 * 60,
        3650 * 24 * 60 * 60
      ),
      rotatePassword: environment.CTMCP_OAUTH_ROTATE_PASSWORD === '1'
    },
    folders: environmentFolders !== undefined
      ? parseFolders(environmentFolders)
      : input.folders?.length
        ? normalizeWorkspaceFolderDocuments(input.folders)
        : parseFolders(undefined),
    limits: {
      blockingConcurrency: positiveInt(environment.CTMCP_BLOCKING_CONCURRENCY ?? input.limits?.blockingConcurrency, 128, 65_535),
      processConcurrency: positiveInt(environment.CTMCP_PROCESS_CONCURRENCY ?? input.limits?.processConcurrency, 64, 65_535),
      globalBlockingConcurrency: positiveInt(environment.CTMCP_GLOBAL_BLOCKING_CONCURRENCY ?? input.limits?.globalBlockingConcurrency, 1_024, 65_535),
      globalProcessConcurrency: positiveInt(environment.CTMCP_GLOBAL_PROCESS_CONCURRENCY ?? input.limits?.globalProcessConcurrency, 512, 65_535),
      activeSessionLimit: positiveInt(environment.CTMCP_ACTIVE_SESSION_LIMIT ?? input.limits?.activeSessionLimit, 512, 65_535),
      maxOutputBytes: positiveInt(environment.CTMCP_MAX_OUTPUT_BYTES ?? input.limits?.maxOutputBytes, 1024 * 1024, 16 * 1024 * 1024),
      commandTimeoutMaxMs: positiveInt(
        environment.CTMCP_COMMAND_TIMEOUT_MAX_MS ?? input.limits?.commandTimeoutMaxMs,
        DEFAULT_COMMAND_TIMEOUT_MAX_MS,
        ABSOLUTE_COMMAND_TIMEOUT_MAX_MS
      )
    },
    tunnel: publicUrl ? {
      enabled: enabled(environment.CTMCP_BUILTIN_ENABLED ?? input.tunnel?.enabled, true),
      publicUrl,
      enrollmentUrl: environment.CTMCP_BUILTIN_ENROLLMENT_URL ?? secrets.tunnelEnrollmentUrl,
      stateFile: path.resolve(input.tunnel?.stateFile ?? path.join(dataDir, 'builtin-tunnel-identity.enc.json'))
    } : undefined
  };
}

function derivePublicBase(publicUrl: string): string | undefined {
  if (!publicUrl) return undefined;
  try {
    const url = new URL(publicUrl);
    url.pathname = url.pathname.replace(/\/mcp\/?$/, '');
    url.search = '';
    url.hash = '';
    return url.toString().replace(/\/$/, '');
  } catch { return undefined; }
}

function migrateConfigDocument(value: unknown, identity?: WorkspaceIdentity): MigrationResult {
  const source = record(value, 'config');
  if (source.schemaVersion !== undefined) {
    const parsed = Number(source.schemaVersion);
    if (!Number.isInteger(parsed) || parsed < 0) throw new Error('schemaVersion must be a non-negative integer');
    if (parsed !== CANONICAL_SCHEMA_VERSION) {
      throw new Error(`Unsupported workspace schemaVersion ${parsed}; this Agent supports ${CANONICAL_SCHEMA_VERSION}`);
    }
  }

  if (looksLikeCanonicalWorkspace(source)) {
    const extracted = extractPlaintextSecrets(source);
    const missingFolders = !Array.isArray(source.folders) || source.folders.length === 0;
    const fallbackFolders = missingFolders ? defaultWorkspaceFolders() : undefined;
    const canonicalSource = fallbackFolders
      ? { ...source, folders: fallbackFolders, activeFolderId: fallbackFolders[0].id }
      : source;
    const parsed = parseCanonicalWorkspace(canonicalSource);
    const canonical = resolveWorkspaceIdentity(parsed, identity);
    const identityChanged = canonical.id !== parsed.id || canonical.name !== parsed.name;
    return {
      document: canonicalToAgentConfigDocument(canonical),
      canonical,
      secrets: extracted.secrets,
      changed: extracted.found || identityChanged || missingFolders,
      fromSchema: CANONICAL_SCHEMA_VERSION
    };
  }

  const rawVersion = source.schema_version;
  if (rawVersion !== undefined
    && typeof rawVersion !== 'number'
    && (typeof rawVersion !== 'string' || !/^\d+$/.test(rawVersion.trim()))) {
    throw new Error('schema_version must be a non-negative integer');
  }
  const fromSchema = rawVersion === undefined ? 0 : Number(rawVersion);
  if (!Number.isInteger(fromSchema) || fromSchema < 0) throw new Error('schema_version must be a non-negative integer');
  if (fromSchema > CURRENT_CONFIG_SCHEMA_VERSION) {
    throw new Error(`Unsupported config schema_version ${fromSchema}; this Agent supports ${CURRENT_CONFIG_SCHEMA_VERSION}`);
  }
  if (fromSchema !== 0 && fromSchema !== CURRENT_CONFIG_SCHEMA_VERSION) {
    throw new Error(`Unsupported config schema_version ${fromSchema}`);
  }

  const document = structuredClone(source) as unknown as AgentConfigDocument;
  const oauth = optionalRecord(source.oauth, 'oauth');
  const tunnel = optionalRecord(source.tunnel, 'tunnel');
  const secrets: AgentSecrets = {
    oauthPassword: legacySecret(oauth.password, 'oauth.password'),
    oauthClientSecret: legacySecret(oauth.clientSecret, 'oauth.clientSecret'),
    oauthTokenSecret: legacySecret(oauth.tokenSecret, 'oauth.tokenSecret'),
    tunnelEnrollmentUrl: legacySecret(tunnel.enrollmentUrl, 'tunnel.enrollmentUrl')
  };

  document.schema_version = CURRENT_CONFIG_SCHEMA_VERSION;
  if (document.oauth) {
    delete (document.oauth as Record<string, unknown>).password;
    delete (document.oauth as Record<string, unknown>).clientSecret;
    delete (document.oauth as Record<string, unknown>).tokenSecret;
  }
  if (document.tunnel) delete (document.tunnel as Record<string, unknown>).enrollmentUrl;

  return {
    document,
    secrets,
    changed: true,
    fromSchema
  };
}

async function readConfigInput(configPath: string): Promise<{ value: unknown; exists: boolean }> {
  try {
    return { value: JSON.parse(await readFile(configPath, 'utf8')) as unknown, exists: true };
  } catch (error) {
    if ((error as NodeJS.ErrnoException).code === 'ENOENT') {
      return { value: { schema_version: CURRENT_CONFIG_SCHEMA_VERSION }, exists: false };
    }
    throw error;
  }
}

async function readLegacyTokenSecret(dataDir: string): Promise<{ filePath: string; value?: string }> {
  const filePath = path.join(dataDir, 'oauth-token-secret');
  try {
    const value = (await readFile(filePath, 'utf8')).trim();
    if (!value) throw new Error(`Persisted OAuth token secret is invalid: ${filePath}`);
    return { filePath, value };
  } catch (error) {
    if ((error as NodeJS.ErrnoException).code === 'ENOENT') return { filePath };
    throw error;
  }
}

export function validateConfig(config: AgentConfig): void {
  if (!config.folders.length) throw new Error('at least one workspace folder is required');
  if (!config.host.trim()) throw new Error('host is required');
  const ids = new Set<string>();
  for (const folder of config.folders) {
    if (!folder.id || ids.has(folder.id)) throw new Error(`workspace folder id must be unique: ${folder.id}`);
    if (!folder.path.trim()) throw new Error(`workspace folder path is required: ${folder.id}`);
    ids.add(folder.id);
  }
  validateUniqueWorkspaceFolders(config.folders);
  if (!config.oauth.clientId.trim()) throw new Error('OAuth client ID is required');
  if (!config.oauth.password.trim()) throw new Error('OAuth password is required');
  if (!config.oauth.tokenSecret.trim()) throw new Error('OAuth token secret is not configured');
  if (!config.toolProfile.trim() || !config.activeToolProfile.trim()) throw new Error('tool profile is required');
  if (!config.sandbox.backend.trim()) throw new Error('sandbox backend is required');
  for (const [index, grant] of config.sandbox.externalPaths.entries()) {
    if (!grant.path.trim()) throw new Error(`sandbox.externalPaths[${index}].path is required`);
    if (grant.access !== 'read_only' && grant.access !== 'modify') throw new Error(`sandbox.externalPaths[${index}].access is invalid`);
  }
  if (!config.policy.allowedCommands.length) throw new Error('at least one allowed command is required');
  if (!config.policy.workspaceScriptExtensions.length) throw new Error('at least one workspace script extension is required');
  if (config.policy.maxPatchBytes < 1) throw new Error('maxPatchBytes must be positive');
  if (config.tunnel?.enabled && !config.tunnel.publicUrl.startsWith('https://')) throw new Error('Built-in tunnel public URL must use HTTPS');
}

export function validateConfigDocument(document: AgentConfigDocument): AgentConfig {
  if (document.schema_version !== CURRENT_CONFIG_SCHEMA_VERSION) {
    throw new Error(`schema_version must be ${CURRENT_CONFIG_SCHEMA_VERSION}`);
  }
  const normalized = normalizeConfig(document, {
    oauthTokenSecret: 'managed-token-secret-placeholder-value'
  }, {});
  validateConfig(normalized);
  return normalized;
}

export async function writeConfigDocument(
  configPath: string,
  document: AgentConfigDocument,
  options: WriteConfigDocumentOptions = {}
): Promise<CanonicalWorkspace> {
  const normalized = validateConfigDocument(document);
  const withFolders = document.folders?.length
    ? document
    : { ...document, folders: normalized.folders };
  const canonical = overlayAgentDocumentOnCanonical(withFolders, {
    id: options.identity?.id || options.canonical?.id || '',
    name: options.identity?.name || options.canonical?.name
  }, options.canonical);
  await mkdir(path.dirname(configPath), { recursive: true, mode: 0o700 });
  const temporary = `${configPath}.${process.pid}.${randomBytes(6).toString('hex')}.tmp`;
  const content = `${JSON.stringify(serializeCanonicalWorkspace(canonical), null, 2)}\n`;
  await writeFile(temporary, content, { flag: 'wx', mode: 0o600 });
  try {
    await rename(temporary, configPath);
  } catch (error) {
    await rm(temporary, { force: true }).catch(() => undefined);
    throw error;
  }
  await chmod(configPath, 0o600).catch(() => undefined);
  return canonical;
}

export async function loadConfigBundle(
  file?: string,
  options?: { identity?: WorkspaceIdentity }
): Promise<LoadedConfig> {
  const configPath = resolveConfigPath(file);
  const input = await readConfigInput(configPath);
  const migration = migrateConfigDocument(input.value, options?.identity);
  if (!migration.document.oauth?.clientId?.trim()) {
    migration.document.oauth = { ...migration.document.oauth, clientId: createOAuthClientId() };
    migration.changed = true;
  }
  if (!migration.document.folders?.length) {
    migration.document.folders = parseFolders(process.env.CTMCP_WORKSPACES);
    migration.changed = true;
  }
  if (migration.document.folders?.length) {
    const normalizedFolders = normalizeWorkspaceFolderDocuments(migration.document.folders);
    if (JSON.stringify(normalizedFolders) !== JSON.stringify(migration.document.folders)) {
      migration.document.folders = normalizedFolders;
      migration.changed = true;
    }
  }
  let canonical = overlayAgentDocumentOnCanonical(migration.document, {
    id: options?.identity?.id || migration.canonical?.id || '',
    name: options?.identity?.name || migration.canonical?.name
  }, migration.canonical);
  const dataDir = resolveDataDir(migration.document);
  const legacyToken = await readLegacyTokenSecret(dataDir);
  const seed: AgentSecrets = {
    ...migration.secrets,
    ...(!migration.secrets.oauthTokenSecret && legacyToken.value
      ? { oauthTokenSecret: legacyToken.value }
      : {})
  };
  const secretState = await ensureAgentSecrets(
    dataDir,
    seed,
    process.env.CTMCP_OAUTH_TOKEN_SECRET === undefined
  );
  let config = normalizeConfig(migration.document, secretState.secrets);
  validateConfig(config);
  const folderRecovery = await recoverWorkspaceFolders(config, migration.document);
  config = folderRecovery.config;
  if (folderRecovery.removed.length) {
    migration.document = folderRecovery.document;
    migration.changed = true;
    canonical = overlayAgentDocumentOnCanonical(migration.document, {
      id: canonical.id,
      name: canonical.name
    }, canonical);
    console.warn(
      `Removed unavailable workspace folder(s) from ${configPath}: ${folderRecovery.removed.map(folder => folder.path).join(', ')}`
    );
    if (folderRecovery.fallbackCreated) {
      console.warn(`No configured workspace folders remained; created fallback workspace: ${config.folders[0]?.path ?? ''}`);
    }
  }

  if (!input.exists || migration.changed) {
    if (input.exists) await copyFile(configPath, `${configPath}.bak`);
    canonical = await writeConfigDocument(configPath, migration.document, {
      identity: { id: canonical.id, name: canonical.name },
      canonical
    });
  }
  if (legacyToken.value && secretState.secrets.oauthTokenSecret) {
    await rm(legacyToken.filePath, { force: true });
  }

  return {
    config,
    configPath,
    document: migration.document,
    canonical,
    secrets: secretState.secrets,
    secretStorePath: secretState.storePath,
    secretKeyPath: secretState.keyPath,
    migrationApplied: input.exists && migration.changed,
    ...(migration.changed ? { migratedFromSchema: migration.fromSchema } : {})
  };
}

export async function loadConfig(file?: string): Promise<AgentConfig> {
  return (await loadConfigBundle(file)).config;
}
