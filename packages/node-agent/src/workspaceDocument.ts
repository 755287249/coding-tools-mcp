import { randomUUID } from 'node:crypto';
import type {
  AgentConfigDocument,
  AgentSecrets,
  PermissionMode,
  SandboxPathAccess,
  WorkspaceFolderDocument
} from './types.js';

export const CANONICAL_SCHEMA_VERSION = 2 as const;

const securityPolicyKeys = [
  'restrictToolCatalog',
  'enforceCommandAllowlist',
  'requireDangerousConfirmation',
  'requireShellConfirmation',
  'blockNetworkCommands',
  'enforceWorkspaceBoundary',
  'protectRepositoryMetadata',
  'blockSymlinkEscape',
  'protectEnvironmentVariables',
  'enforceHarnessBaseline',
  'requireWriteConfirmation',
  'verifyWriteConflicts',
  'enforceResourceLimits',
  'redactSensitiveOutput',
  'withholdSensitiveSourceOutput',
  'redactTelemetry',
  'redactHistory'
] as const;

const SECRET_KEYS = new Set([
  'password',
  'clientSecret',
  'tokenSecret',
  'enrollmentUrl',
  'cloudflareToken',
  'oauthPassword',
  'oauthClientSecret',
  'oauthTokenSecret',
  'tunnelEnrollmentUrl'
]);

const NODE_V1_KNOWN = new Set([
  'schema_version',
  'schemaVersion',
  'host',
  'port',
  'publicBaseUrl',
  'dataDir',
  'permissionMode',
  'toolProfile',
  'securityPolicy',
  'policy',
  'management',
  'skills',
  'extensions',
  'sandbox',
  'oauth',
  'folders',
  'limits',
  'tunnel'
]);

export interface CanonicalFolder {
  id: string;
  name: string;
  path: string;
  [key: string]: unknown;
}

export interface CanonicalWorkspace {
  schemaVersion: typeof CANONICAL_SCHEMA_VERSION;
  id: string;
  name: string;
  folders: CanonicalFolder[];
  activeFolderId: string;
  bind: { host: string; port: number };
  publicBaseUrl: string;
  auth: { type: string; oauthClientId: string; oauthTokenTtlSeconds: number };
  toolProfile: string;
  permissionMode: string;
  securityPolicy: Record<string, boolean>;
  policy: {
    allowedCommands: string[];
    workspaceLocalEntries: boolean;
    workspaceScriptExtensions: string[];
    maxPatchBytes: number;
  };
  sandbox: {
    enabled: boolean;
    backend: string;
    externalPaths: Array<{ path: string; access: string }>;
    options: Record<string, string>;
  };
  limits: {
    blockingConcurrency: number;
    processConcurrency: number;
    globalBlockingConcurrency: number;
    globalProcessConcurrency: number;
    activeSessionLimit: number;
    maxOutputBytes: number;
    commandTimeoutMaxMs: number;
  };
  tunnel: { builtin: { enabled: boolean; publicUrl: string } };
  skills: { active: boolean; disabled: string[] };
  extensions: {
    hooks: { active: boolean; enabled: string[] };
    mcp: { active: boolean; enabled: string[] };
  };
  host: { desktop: Record<string, unknown>; node: Record<string, unknown> };
  extra: Record<string, unknown>;
}

export interface WorkspaceIdentity {
  id?: string;
  name?: string;
}

function isRecord(value: unknown): value is Record<string, unknown> {
  return Boolean(value) && typeof value === 'object' && !Array.isArray(value);
}

function text(value: unknown, fallback = ''): string {
  return value === undefined || value === null ? fallback : String(value).trim();
}

function integer(value: unknown, fallback: number): number {
  const parsed = value === undefined || value === null || value === '' ? fallback : Number(value);
  return Number.isInteger(parsed) ? parsed : fallback;
}

function bool(value: unknown, fallback: boolean): boolean {
  return typeof value === 'boolean' ? value : fallback;
}

function stringList(value: unknown): string[] {
  if (Array.isArray(value)) return value.map(item => String(item).trim()).filter(Boolean);
  if (typeof value === 'string') {
    return value.split(',').map(item => item.trim()).filter(Boolean);
  }
  return [];
}

function stripSecrets(value: unknown): unknown {
  if (Array.isArray(value)) return value.map(stripSecrets);
  if (!isRecord(value)) return value;
  const output: Record<string, unknown> = {};
  for (const [key, entry] of Object.entries(value)) {
    if (SECRET_KEYS.has(key)) continue;
    output[key] = stripSecrets(entry);
  }
  return output;
}

function recordValue(value: unknown): Record<string, unknown> {
  return isRecord(value) ? { ...value } : {};
}

function parseFolders(value: unknown): CanonicalFolder[] {
  if (!Array.isArray(value)) return [];
  return value.flatMap((entry, index) => {
    if (!isRecord(entry)) return [];
    const folderPath = text(entry.path);
    if (!folderPath) return [];
    const { id, name, path: _path, ...extra } = entry;
    return [{
      id: text(id, `folder-${index + 1}`),
      name: text(name, folderPath.split(/[\\/]/).filter(Boolean).at(-1) || 'folder'),
      path: folderPath,
      ...recordValue(stripSecrets(extra) as Record<string, unknown>)
    }];
  });
}

function parseSecurityPolicy(value: unknown): Record<string, boolean> {
  const source = recordValue(value);
  const policy: Record<string, boolean> = {};
  for (const key of securityPolicyKeys) {
    if (typeof source[key] === 'boolean') policy[key] = source[key];
  }
  return policy;
}

function parseHost(value: unknown): CanonicalWorkspace['host'] {
  const source = recordValue(value);
  return {
    desktop: recordValue(stripSecrets(source.desktop)),
    node: recordValue(stripSecrets(source.node))
  };
}

function sandboxAccess(value: unknown): SandboxPathAccess {
  return text(value) === 'modify' ? 'modify' : 'read_only';
}

function workspaceId(): string {
  return randomUUID().replaceAll('-', '');
}

export function looksLikeCanonicalWorkspace(value: unknown): boolean {
  if (!isRecord(value)) return false;
  if (integer(value.schemaVersion, 0) === CANONICAL_SCHEMA_VERSION) return true;
  return integer(value.schema_version, 0) === CANONICAL_SCHEMA_VERSION
    && (isRecord(value.bind) || isRecord(value.auth) || isRecord(value.host));
}

export function parseCanonicalWorkspace(value: unknown): CanonicalWorkspace {
  if (!isRecord(value)) throw new Error('workspace document must be an object');
  const schemaVersion = integer(value.schemaVersion ?? value.schema_version, 0);
  if (schemaVersion !== CANONICAL_SCHEMA_VERSION) {
    throw new Error(`Unsupported workspace schemaVersion ${schemaVersion}`);
  }
  const folders = parseFolders(value.folders);
  if (folders.length < 1) throw new Error('workspace document must contain at least one folder');
  const bind = recordValue(value.bind);
  const auth = recordValue(value.auth);
  const policy = recordValue(value.policy);
  const sandbox = recordValue(value.sandbox);
  const limits = recordValue(value.limits);
  const tunnel = recordValue(value.tunnel);
  const builtin = recordValue(tunnel.builtin);
  const skills = recordValue(value.skills);
  const extensions = recordValue(value.extensions);
  const hooks = recordValue(extensions.hooks);
  const mcp = recordValue(extensions.mcp);
  const known = new Set([
    'schemaVersion', 'schema_version', 'id', 'name', 'folders', 'activeFolderId', 'bind',
    'publicBaseUrl', 'auth', 'toolProfile', 'permissionMode', 'securityPolicy', 'policy',
    'sandbox', 'limits', 'tunnel', 'skills', 'extensions', 'host'
  ]);
  const extra: Record<string, unknown> = {};
  for (const [key, entry] of Object.entries(value)) {
    if (!known.has(key)) extra[key] = stripSecrets(entry);
  }
  return {
    schemaVersion: CANONICAL_SCHEMA_VERSION,
    id: text(value.id),
    name: text(value.name, folders[0]?.name || 'Workspace'),
    folders,
    activeFolderId: text(value.activeFolderId, folders[0]?.id ?? ''),
    bind: {
      host: text(bind.host, '127.0.0.1'),
      port: integer(bind.port, 3789)
    },
    publicBaseUrl: text(value.publicBaseUrl),
    auth: {
      type: text(auth.type, 'oauth') || 'oauth',
      oauthClientId: text(auth.oauthClientId ?? auth.oauth_client_id),
      oauthTokenTtlSeconds: integer(auth.oauthTokenTtlSeconds ?? auth.oauth_token_ttl_seconds, 3650 * 24 * 60 * 60)
    },
    toolProfile: text(value.toolProfile, 'core') || 'core',
    permissionMode: text(value.permissionMode, 'trusted') || 'trusted',
    securityPolicy: parseSecurityPolicy(value.securityPolicy),
    policy: {
      allowedCommands: stringList(policy.allowedCommands),
      workspaceLocalEntries: bool(policy.workspaceLocalEntries, true),
      workspaceScriptExtensions: stringList(policy.workspaceScriptExtensions),
      maxPatchBytes: integer(policy.maxPatchBytes, 1024 * 1024)
    },
    sandbox: {
      enabled: bool(sandbox.enabled, false),
      backend: text(sandbox.backend, 'appcontainer') || 'appcontainer',
      externalPaths: Array.isArray(sandbox.externalPaths)
        ? sandbox.externalPaths.flatMap(entry => {
            if (!isRecord(entry) || !text(entry.path)) return [];
            return [{ path: text(entry.path), access: text(entry.access, 'read_only') || 'read_only' }];
          })
        : [],
      options: Object.fromEntries(
        Object.entries(recordValue(sandbox.options)).map(([key, entry]) => [key, String(entry)])
      )
    },
    limits: {
      blockingConcurrency: integer(limits.blockingConcurrency, 128),
      processConcurrency: integer(limits.processConcurrency, 64),
      globalBlockingConcurrency: integer(limits.globalBlockingConcurrency, 1024),
      globalProcessConcurrency: integer(limits.globalProcessConcurrency, 512),
      activeSessionLimit: integer(limits.activeSessionLimit, 512),
      maxOutputBytes: integer(limits.maxOutputBytes, 1024 * 1024),
      commandTimeoutMaxMs: integer(limits.commandTimeoutMaxMs, 0)
    },
    tunnel: {
      builtin: {
        enabled: bool(builtin.enabled, true),
        publicUrl: text(builtin.publicUrl)
      }
    },
    skills: {
      active: bool(skills.active, true),
      disabled: stringList(skills.disabled)
    },
    extensions: {
      hooks: { active: bool(hooks.active, true), enabled: stringList(hooks.enabled) },
      mcp: { active: bool(mcp.active, true), enabled: stringList(mcp.enabled) }
    },
    host: parseHost(value.host),
    extra
  };
}

export function serializeCanonicalWorkspace(document: CanonicalWorkspace): Record<string, unknown> {
  return stripSecrets({
    schemaVersion: CANONICAL_SCHEMA_VERSION,
    id: document.id,
    name: document.name,
    folders: document.folders,
    activeFolderId: document.activeFolderId,
    bind: document.bind,
    publicBaseUrl: document.publicBaseUrl,
    auth: document.auth,
    toolProfile: document.toolProfile,
    permissionMode: document.permissionMode,
    securityPolicy: document.securityPolicy,
    policy: document.policy,
    sandbox: document.sandbox,
    limits: document.limits,
    tunnel: document.tunnel,
    skills: document.skills,
    extensions: document.extensions,
    host: document.host,
    ...document.extra
  }) as Record<string, unknown>;
}

export function migrateNodeV1Document(
  value: unknown,
  identity: { id: string; name?: string }
): CanonicalWorkspace {
  if (!isRecord(value)) throw new Error('Node v1 config must be an object');
  const oauth = recordValue(value.oauth);
  const tunnel = recordValue(value.tunnel);
  const management = recordValue(value.management);
  const folders = parseFolders(value.folders);
  const name = text(identity.name, folders[0]?.name || 'Workspace');
  const extra: Record<string, unknown> = {};
  for (const [key, entry] of Object.entries(value)) {
    if (!NODE_V1_KNOWN.has(key)) extra[key] = stripSecrets(entry);
  }
  const nodeHost: Record<string, unknown> = {
    management: { enabled: bool(management.enabled, true) }
  };
  const dataDir = text(value.dataDir);
  if (dataDir) nodeHost.dataDir = dataDir;
  const stateFile = text(tunnel.stateFile);
  if (stateFile) nodeHost.tunnelStateFile = stateFile;
  return parseCanonicalWorkspace({
    ...extra,
    schemaVersion: CANONICAL_SCHEMA_VERSION,
    id: identity.id,
    name,
    folders,
    bind: { host: text(value.host, '127.0.0.1'), port: integer(value.port, 3789) },
    publicBaseUrl: text(value.publicBaseUrl),
    auth: {
      type: 'oauth',
      oauthClientId: text(oauth.clientId),
      oauthTokenTtlSeconds: integer(oauth.tokenTtlSeconds, 3650 * 24 * 60 * 60)
    },
    toolProfile: text(value.toolProfile, 'core') || 'core',
    permissionMode: text(value.permissionMode, 'trusted') || 'trusted',
    securityPolicy: value.securityPolicy,
    policy: value.policy,
    sandbox: value.sandbox,
    limits: value.limits,
    tunnel: {
      builtin: {
        enabled: bool(tunnel.enabled, true),
        publicUrl: text(tunnel.publicUrl)
      }
    },
    skills: value.skills,
    extensions: value.extensions,
    host: {
      node: nodeHost
    }
  });
}

export function overlayAgentDocumentOnCanonical(
  document: AgentConfigDocument,
  identity: { id: string; name?: string },
  base?: CanonicalWorkspace
): CanonicalWorkspace {
  const migrated = migrateNodeV1Document(document, {
    id: identity.id || base?.id || workspaceId(),
    name: identity.name || base?.name
  });
  if (!base) return migrated;
  const folders = migrated.folders.map(folder => {
    const previous = base.folders.find(entry => entry.id === folder.id);
    if (!previous) return folder;
    const { id: _id, name: _name, path: _path, ...extra } = previous;
    return { ...extra, ...folder };
  });
  return {
    ...migrated,
    folders,
    extra: { ...base.extra, ...migrated.extra },
    host: {
      desktop: base.host.desktop,
      node: { ...base.host.node, ...migrated.host.node }
    }
  };
}

export function canonicalToAgentConfigDocument(canonical: CanonicalWorkspace): AgentConfigDocument {
  const node = canonical.host.node;
  const management = recordValue(node.management);
  const folders: WorkspaceFolderDocument[] = canonical.folders.map(folder => ({
    id: folder.id,
    name: folder.name,
    path: folder.path
  }));
  const document: AgentConfigDocument = {
    schema_version: 1,
    host: canonical.bind.host,
    port: canonical.bind.port,
    permissionMode: canonical.permissionMode as PermissionMode,
    toolProfile: canonical.toolProfile,
    securityPolicy: canonical.securityPolicy,
    policy: canonical.policy,
    management: { enabled: bool(management.enabled, true) },
    skills: canonical.skills,
    extensions: canonical.extensions,
    sandbox: {
      enabled: canonical.sandbox.enabled,
      backend: canonical.sandbox.backend,
      externalPaths: canonical.sandbox.externalPaths.map(entry => ({
        path: entry.path,
        access: sandboxAccess(entry.access)
      })),
      options: { ...canonical.sandbox.options }
    },
    oauth: {
      clientId: canonical.auth.oauthClientId,
      tokenTtlSeconds: canonical.auth.oauthTokenTtlSeconds
    },
    folders,
    limits: canonical.limits
  };
  if (canonical.publicBaseUrl) document.publicBaseUrl = canonical.publicBaseUrl;
  const dataDir = text(node.dataDir);
  if (dataDir) document.dataDir = dataDir;
  const stateFile = text(node.tunnelStateFile);
  const publicUrl = canonical.tunnel.builtin.publicUrl;
  if (publicUrl || canonical.tunnel.builtin.enabled === false || stateFile) {
    document.tunnel = {
      enabled: canonical.tunnel.builtin.enabled,
      ...(publicUrl ? { publicUrl } : {}),
      ...(stateFile ? { stateFile } : {})
    };
  }
  return document;
}

function optionalSecret(value: unknown): string | undefined {
  if (value === undefined || value === null || value === '') return undefined;
  if (typeof value !== 'string') return undefined;
  if (value.length > 4096) throw new Error('secret value exceeds 4096 characters');
  return value;
}

export function extractPlaintextSecrets(value: unknown): { secrets: AgentSecrets; found: boolean } {
  if (!isRecord(value)) return { secrets: {}, found: false };
  const oauth = recordValue(value.oauth);
  const auth = recordValue(value.auth);
  const tunnel = recordValue(value.tunnel);
  const builtin = recordValue(tunnel.builtin);
  const secrets: AgentSecrets = {
    oauthPassword: optionalSecret(oauth.password ?? auth.password),
    oauthClientSecret: optionalSecret(oauth.clientSecret ?? auth.clientSecret),
    oauthTokenSecret: optionalSecret(oauth.tokenSecret ?? auth.tokenSecret),
    tunnelEnrollmentUrl: optionalSecret(
      tunnel.enrollmentUrl ?? builtin.enrollmentUrl
    )
  };
  const found = Boolean(
    secrets.oauthPassword
    || secrets.oauthClientSecret
    || secrets.oauthTokenSecret
    || secrets.tunnelEnrollmentUrl
  );
  return { secrets, found };
}

export interface SecretPresence {
  oauthPassword?: boolean;
  oauthClientSecret?: boolean;
  oauthTokenSecret?: boolean;
  tunnelEnrollmentUrl?: boolean;
}

export function exportWorkspacePack(
  canonical: CanonicalWorkspace,
  presence: SecretPresence = {}
): Record<string, unknown> {
  const serialized = serializeCanonicalWorkspace(canonical);
  const host = isRecord(serialized.host) ? { ...serialized.host } : {};
  const node = isRecord(host.node) ? { ...host.node } : {};
  delete node.dataDir;
  host.node = node;
  return {
    ...serialized,
    host,
    secretPresence: {
      oauthPassword: Boolean(presence.oauthPassword),
      oauthClientSecret: Boolean(presence.oauthClientSecret),
      oauthTokenSecret: Boolean(presence.oauthTokenSecret),
      tunnelEnrollmentUrl: Boolean(presence.tunnelEnrollmentUrl)
    }
  };
}

export function parseWorkspacePack(value: unknown): {
  canonical: CanonicalWorkspace;
  secretPresence: SecretPresence;
} {
  if (!isRecord(value)) throw new Error('workspace pack must be an object');
  const { secretPresence: rawPresence, ...rest } = value;
  if (isRecord(rest.host) && isRecord(rest.host.node)) {
    const node = { ...rest.host.node };
    delete node.dataDir;
    rest.host = { ...rest.host, node };
  }
  const presence = isRecord(rawPresence) ? rawPresence : {};
  return {
    canonical: parseCanonicalWorkspace(rest),
    secretPresence: {
      oauthPassword: Boolean(presence.oauthPassword),
      oauthClientSecret: Boolean(presence.oauthClientSecret),
      oauthTokenSecret: Boolean(presence.oauthTokenSecret),
      tunnelEnrollmentUrl: Boolean(presence.tunnelEnrollmentUrl)
    }
  };
}

export function resolveWorkspaceIdentity(
  canonical: CanonicalWorkspace,
  identity?: WorkspaceIdentity
): CanonicalWorkspace {
  const id = text(identity?.id, canonical.id) || workspaceId();
  const name = text(identity?.name, canonical.name) || canonical.folders[0]?.name || 'Workspace';
  if (id === canonical.id && name === canonical.name) return canonical;
  return { ...canonical, id, name };
}
