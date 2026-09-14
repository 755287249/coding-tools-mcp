import { randomUUID } from 'node:crypto';
import path from 'node:path';
import {
  CURRENT_CONFIG_SCHEMA_VERSION,
  normalizeConfig,
  resolveDataDir,
  validateConfigDocument,
  writeConfigDocument,
  type LoadedConfig
} from '../config.js';
import {
  exportWorkspacePack,
  overlayAgentDocumentOnCanonical,
  type CanonicalWorkspace
} from '../workspaceDocument.js';
import { ABSOLUTE_COMMAND_TIMEOUT_MAX_MS } from '../executionLimits.js';
import { allFolderRuntimes, applyWorkspaceFolderConfiguration } from '../folderRuntime.js';
import { Semaphore } from '../runtime.js';
import {
  restoreAgentSecretFiles,
  snapshotAgentSecretFiles,
  writeAgentSecrets,
  type SecretStoreState
} from '../secrets.js';
import {
  compatibilityPermissionMode,
  compatibilityToolProfile,
  normalizeSecurityPolicy
} from '../securityPolicy.js';
import { parseBuiltinPublicUrl } from '../tunnel.js';
import { sandboxBackends } from '../sandbox.js';
import type {
  AgentConfig,
  AgentConfigDocument,
  AgentSecrets,
  JsonObject,
  ToolContext,
  WorkspaceFolder
} from '../types.js';
import { canonicalizeWorkspaceFolders } from '../workspace.js';
import { normalizeWorkspacePath } from '../wsl.js';
import type { RuntimeHotApplyTarget } from './runtimeContract.js';
import {
  readSharedWorkspace,
  restoreSharedSecrets,
  sharedSecretsFile,
  sharedStoreAvailable,
  snapshotSharedSecrets,
  writeSharedAgentSecrets,
  writeSharedWorkspace
} from '../sharedStore.js';

const environmentKeys = [
  'CTMCP_HOST', 'CTMCP_PORT', 'CTMCP_PUBLIC_BASE_URL', 'CTMCP_DATA_DIR', 'CTMCP_PERMISSION_MODE', 'CTMCP_TOOL_PROFILE',
  'CTMCP_UI_ENABLED', 'CTMCP_UI_TRUST_PRIVATE_PROXY', 'CTMCP_OAUTH_CLIENT_ID', 'CTMCP_OAUTH_CLIENT_SECRET', 'CTMCP_OAUTH_PASSWORD',
  'CTMCP_OAUTH_TOKEN_SECRET', 'CTMCP_OAUTH_TOKEN_TTL_SECONDS', 'CTMCP_WORKSPACES', 'CTMCP_BLOCKING_CONCURRENCY',
  'CTMCP_PROCESS_CONCURRENCY', 'CTMCP_GLOBAL_BLOCKING_CONCURRENCY', 'CTMCP_GLOBAL_PROCESS_CONCURRENCY',
  'CTMCP_ACTIVE_SESSION_LIMIT', 'CTMCP_MAX_OUTPUT_BYTES',
  'CTMCP_ALLOWED_COMMANDS', 'CTMCP_WORKSPACE_LOCAL_ENTRIES', 'CTMCP_WORKSPACE_SCRIPT_EXTENSIONS', 'CTMCP_MAX_PATCH_BYTES',
  'CTMCP_SANDBOX_ENABLED', 'CTMCP_SANDBOX_BACKEND', 'CTMCP_WSLC_IMAGE', 'CTMCP_WSLC_NETWORK',
  'CTMCP_WSLC_SESSION_STORAGE', 'CTMCP_DOCKER_IMAGE', 'CTMCP_DOCKER_NETWORK',
  'CTMCP_PODMAN_IMAGE', 'CTMCP_PODMAN_NETWORK',
  'CTMCP_BUILTIN_ENABLED', 'CTMCP_BUILTIN_PUBLIC_URL', 'CTMCP_BUILTIN_ENROLLMENT_URL'
] as const;

function record(value: unknown, name: string): JsonObject {
  if (!value || typeof value !== 'object' || Array.isArray(value)) {
    throw new Error(`${name} must be an object`);
  }
  return value as JsonObject;
}

function stringValue(value: unknown, name: string, fallback = '', maxLength = 4096): string {
  const output = value === undefined ? fallback : String(value).trim();
  if (output.length > maxLength) throw new Error(`${name} exceeds ${maxLength} characters`);
  return output;
}

function secretValue(value: unknown, name: string, maxLength = 4096): string {
  if (value === undefined || value === null) return '';
  if (typeof value !== 'string') throw new Error(`${name} must be a string`);
  if (value.length > maxLength) throw new Error(`${name} exceeds ${maxLength} characters`);
  return value;
}

function integerValue(
  value: unknown,
  name: string,
  fallback: number,
  minimum: number,
  maximum: number
): number {
  const output = value === undefined ? fallback : Number(value);
  if (!Number.isInteger(output) || output < minimum || output > maximum) {
    throw new Error(`${name} must be an integer between ${minimum} and ${maximum}`);
  }
  return output;
}

function booleanValue(value: unknown, fallback: boolean): boolean {
  return typeof value === 'boolean' ? value : fallback;
}

function parseSandbox(value: unknown, current: AgentConfig['sandbox']): AgentConfigDocument['sandbox'] {
  const input = record(value ?? current, 'sandbox');
  const externalPathsValue = input.externalPaths ?? current.externalPaths;
  if (!Array.isArray(externalPathsValue)) throw new Error('sandbox.externalPaths must be an array');
  const externalPaths = externalPathsValue.map((entry, index) => {
    const grant = record(entry, `sandbox.externalPaths[${index}]`);
    const grantPath = stringValue(grant.path, `sandbox.externalPaths[${index}].path`, '', 4096);
    if (!grantPath) throw new Error(`sandbox.externalPaths[${index}].path is required`);
    const access = stringValue(grant.access, `sandbox.externalPaths[${index}].access`, 'read_only', 16);
    if (access !== 'read_only' && access !== 'modify') {
      throw new Error(`sandbox.externalPaths[${index}].access must be read_only or modify`);
    }
    return { path: grantPath, access: access as 'read_only' | 'modify' };
  });
  const optionsInput = record(input.options ?? current.options, 'sandbox.options');
  const options: Record<string, string> = {};
  for (const [key, raw] of Object.entries(optionsInput)) {
    const optionKey = key.trim();
    if (!optionKey || optionKey.length > 128) throw new Error('sandbox option keys must contain 1 to 128 characters');
    options[optionKey] = stringValue(raw, `sandbox.options.${optionKey}`, '', 4096);
  }
  return {
    enabled: booleanValue(input.enabled, current.enabled),
    backend: stringValue(input.backend, 'sandbox.backend', current.backend, 128) || 'appcontainer',
    externalPaths,
    options
  };
}

function parseSkills(value: unknown, current: AgentConfig['skills']): AgentConfigDocument['skills'] {
  const input = record(value ?? current, 'skills');
  const disabledValue = input.disabled ?? current.disabled;
  if (!Array.isArray(disabledValue) || disabledValue.length > 1024) {
    throw new Error('skills.disabled must contain at most 1024 entries');
  }
  const disabled = new Set<string>();
  for (const [index, raw] of disabledValue.entries()) {
    if (typeof raw !== 'string') throw new Error(`skills.disabled[${index}] must be a string`);
    const key = raw.trim();
    if (!key || key.length > 4096) {
      throw new Error(`skills.disabled[${index}] must contain 1 to 4096 characters`);
    }
    disabled.add(key);
  }
  return {
    active: booleanValue(input.active, current.active),
    disabled: [...disabled].sort()
  };
}

function parseExtensionKeyList(value: unknown, name: string): string[] {
  if (!Array.isArray(value) || value.length > 1024) throw new Error(`${name} must contain at most 1024 entries`);
  const enabled = new Set<string>();
  for (const [index, raw] of value.entries()) {
    if (typeof raw !== 'string') throw new Error(`${name}[${index}] must be a string`);
    const key = raw.trim();
    if (!key || key.length > 4096) throw new Error(`${name}[${index}] must contain 1 to 4096 characters`);
    enabled.add(key);
  }
  return [...enabled].sort();
}

function parseExtensions(value: unknown, current: AgentConfig['extensions']): AgentConfigDocument['extensions'] {
  const input = record(value ?? current, 'extensions');
  const hooks = record(input.hooks ?? current.hooks, 'extensions.hooks');
  const mcp = record(input.mcp ?? current.mcp, 'extensions.mcp');
  return {
    hooks: {
      active: booleanValue(hooks.active, current.hooks.active),
      enabled: parseExtensionKeyList(hooks.enabled ?? current.hooks.enabled, 'extensions.hooks.enabled')
    },
    mcp: {
      active: booleanValue(mcp.active, current.mcp.active),
      enabled: parseExtensionKeyList(mcp.enabled ?? current.mcp.enabled, 'extensions.mcp.enabled')
    }
  };
}

function parseFolders(value: unknown): WorkspaceFolder[] {
  if (!Array.isArray(value) || value.length < 1 || value.length > 100) {
    throw new Error('folders must contain 1 to 100 entries');
  }
  const ids = new Set<string>();
  return value.map((entry, index) => {
    const folder = record(entry, `folders[${index}]`);
    const folderPath = stringValue(folder.path, `folders[${index}].path`, '', 4096);
    if (!folderPath) throw new Error(`folders[${index}].path is required`);
    const normalizedPath = normalizeWorkspacePath(folderPath);
    const id = stringValue(folder.id, `folders[${index}].id`, '', 64).trim()
      || randomUUID().replaceAll('-', '');
    const name = stringValue(folder.name, `folders[${index}].name`, '', 128).trim()
      || path.basename(normalizedPath);
    if (!/^[A-Za-z0-9._-]+$/.test(id)) {
      throw new Error(`folders[${index}].id contains unsupported characters`);
    }
    if (ids.has(id)) throw new Error(`workspace folder id must be unique: ${id}`);
    ids.add(id);
    return { id, name, path: normalizedPath };
  });
}

function safeConfig(config: AgentConfig, secrets: AgentSecrets, effective: boolean): JsonObject {
  return {
    host: config.host,
    port: config.port,
    publicBaseUrl: config.publicBaseUrl ?? '',
    dataDir: config.dataDir,
    permissionMode: config.permissionMode,
    toolProfile: config.toolProfile,
    activeToolProfile: config.activeToolProfile,
    securityPolicy: config.securityPolicy,
    policy: config.policy,
    management: { enabled: config.management.enabled },
    skills: structuredClone(config.skills),
    extensions: structuredClone(config.extensions),
    sandbox: structuredClone(config.sandbox),
    sandboxBackends: sandboxBackends(),
    oauth: {
      clientId: config.oauth.clientId,
      passwordConfigured: effective ? config.oauth.password !== 'change-me' : Boolean(secrets.oauthPassword),
      clientSecretConfigured: effective ? Boolean(config.oauth.clientSecret) : Boolean(secrets.oauthClientSecret),
      tokenSecretSource: process.env.CTMCP_OAUTH_TOKEN_SECRET
        ? 'environment'
        : secrets.oauthTokenSecret
          ? 'secret-store'
          : 'missing'
    },
    folders: config.folders,
    limits: config.limits,
    tunnel: config.tunnel ? {
      enabled: config.tunnel.enabled,
      publicUrl: config.tunnel.publicUrl,
      enrollmentConfigured: effective
        ? Boolean(config.tunnel.enrollmentUrl)
        : Boolean(secrets.tunnelEnrollmentUrl)
    } : {
      enabled: false,
      publicUrl: '',
      enrollmentConfigured: false
    }
  };
}

function restartRequired(
  current: AgentConfig,
  document: AgentConfigDocument,
  secrets: AgentSecrets
): boolean {
  const desired = normalizeConfig(document, secrets);
  desired.workspaceId = current.workspaceId;
  desired.workspaceName = current.workspaceName;
  return JSON.stringify(current) !== JSON.stringify(desired);
}

function sameRuntimeValue(left: unknown, right: unknown): boolean {
  return JSON.stringify(left) === JSON.stringify(right);
}

const hotApplyLimitKeys = ['activeSessionLimit', 'maxOutputBytes', 'commandTimeoutMaxMs'] as const;
type ConcurrencyLimitKey =
  | 'blockingConcurrency'
  | 'processConcurrency'
  | 'globalBlockingConcurrency'
  | 'globalProcessConcurrency';

interface RuntimeConfigurationApplyResult {
  applied: string[];
  deferredReasons: string[];
}

function applyConcurrencyLimit(
  current: AgentConfig,
  context: ToolContext,
  desired: AgentConfig,
  key: ConcurrencyLimitKey,
  lanes: readonly Semaphore[],
  replace: (limit: number) => void,
  result: RuntimeConfigurationApplyResult
): void {
  if (context.config.limits[key] === desired.limits[key]) return;
  if (lanes.some(lane => lane.active > 0 || lane.queued > 0)) {
    result.deferredReasons.push(`${key} is still in use by active or queued work.`);
    return;
  }
  replace(desired.limits[key]);
  current.limits[key] = desired.limits[key];
  context.config.limits[key] = desired.limits[key];
  result.applied.push(`limits.${key}`);
}

async function applyRuntimeConfiguration(
  current: AgentConfig,
  runtime: RuntimeHotApplyTarget,
  desired: AgentConfig
): Promise<RuntimeConfigurationApplyResult> {
  const context = runtime.context;
  const result: RuntimeConfigurationApplyResult = { applied: [], deferredReasons: [] };
  if (!sameRuntimeValue(context.config.policy, desired.policy)) {
    current.policy = structuredClone(desired.policy);
    context.config.policy = structuredClone(desired.policy);
    result.applied.push('policy');
  }

  for (const key of hotApplyLimitKeys) {
    if (context.config.limits[key] === desired.limits[key]) continue;
    current.limits[key] = desired.limits[key];
    context.config.limits[key] = desired.limits[key];
    result.applied.push(`limits.${key}`);
  }

  const folderRuntimes = allFolderRuntimes(context);
  if (!sameRuntimeValue(context.config.skills, desired.skills)) {
    current.skills = structuredClone(desired.skills);
    context.config.skills = structuredClone(desired.skills);
    for (const folderRuntime of folderRuntimes) {
      folderRuntime.skillRegistry.setActive(desired.skills.active);
      folderRuntime.skillRegistry.setDisabledSkillKeys(desired.skills.disabled);
    }
    result.applied.push('skills');
  }

  if (!sameRuntimeValue(context.config.extensions, desired.extensions)) {
    current.extensions = structuredClone(desired.extensions);
    context.config.extensions = structuredClone(desired.extensions);
    await context.extensions.setConfiguration(desired.extensions);
    result.applied.push('extensions');
  }

  applyConcurrencyLimit(
    current,
    context,
    desired,
    'blockingConcurrency',
    folderRuntimes.map(folderRuntime => folderRuntime.admission.blocking),
    limit => {
      for (const folderRuntime of folderRuntimes) {
        folderRuntime.admission.blocking = new Semaphore(limit);
      }
    },
    result
  );
  applyConcurrencyLimit(
    current,
    context,
    desired,
    'processConcurrency',
    folderRuntimes.map(folderRuntime => folderRuntime.admission.process),
    limit => {
      for (const folderRuntime of folderRuntimes) {
        folderRuntime.admission.process = new Semaphore(limit);
      }
    },
    result
  );
  applyConcurrencyLimit(
    current,
    context,
    desired,
    'globalBlockingConcurrency',
    [context.hubAdmission.blocking],
    limit => {
      context.hubAdmission.blocking = new Semaphore(limit);
    },
    result
  );
  applyConcurrencyLimit(
    current,
    context,
    desired,
    'globalProcessConcurrency',
    [context.hubAdmission.process],
    limit => {
      context.hubAdmission.process = new Semaphore(limit);
    },
    result
  );

  const sandboxChanged = !sameRuntimeValue(context.config.sandbox, desired.sandbox);
  if (sandboxChanged) {
    current.sandbox = structuredClone(desired.sandbox);
    context.config.sandbox = structuredClone(desired.sandbox);
    result.applied.push('sandbox');
  }

  const catalogChanged = context.config.activeToolProfile !== desired.activeToolProfile;
  const securityChanged = context.config.permissionMode !== desired.permissionMode
    || !sameRuntimeValue(context.config.securityPolicy, desired.securityPolicy)
    || catalogChanged;
  if (securityChanged) {
    context.usageStore.setRedactTelemetry(desired.securityPolicy.redactTelemetry);
    for (const target of [current, context.config]) {
      target.permissionMode = desired.permissionMode;
      target.toolProfile = desired.toolProfile;
      target.activeToolProfile = desired.activeToolProfile;
      target.securityPolicy = structuredClone(desired.securityPolicy);
      target.securityPolicyCustomized = desired.securityPolicyCustomized;
    }
    result.applied.push('securityPolicy');
    if (catalogChanged) result.applied.push('toolCatalog');
  } else {
    current.toolProfile = desired.toolProfile;
    context.config.toolProfile = desired.toolProfile;
    current.securityPolicyCustomized = desired.securityPolicyCustomized;
    context.config.securityPolicyCustomized = desired.securityPolicyCustomized;
  }

  if ((sandboxChanged || securityChanged) && runtime.tunnel) {
    await runtime.tunnel.enforceSecurity();
  }

  if (!sameRuntimeValue(context.config.oauth, desired.oauth) && runtime.oauth) {
    runtime.oauth.update(desired.oauth);
    current.oauth = structuredClone(desired.oauth);
    context.config.oauth = structuredClone(desired.oauth);
    result.applied.push('oauth');
  }

  if (!sameRuntimeValue(context.config.tunnel, desired.tunnel) && runtime.tunnel) {
    try {
      await runtime.tunnel.reconfigure(desired.tunnel, desired.publicBaseUrl);
      if (context.config.tunnel) current.tunnel = structuredClone(context.config.tunnel);
      else delete current.tunnel;
      if (context.config.publicBaseUrl) current.publicBaseUrl = context.config.publicBaseUrl;
      else delete current.publicBaseUrl;
      result.applied.push('tunnel');
    } catch (error) {
      result.deferredReasons.push(
        `Tunnel reconfiguration failed: ${error instanceof Error ? error.message : String(error)}`
      );
    }
  }
  return result;
}

export class ConfigStore {
  readonly configPath: string;
  readonly current: AgentConfig;
  private document: AgentConfigDocument;
  private canonical: CanonicalWorkspace;
  private secrets: AgentSecrets;
  private secretStorePath: string;
  private readonly migrationApplied: boolean;
  private readonly migratedFromSchema?: number;

  constructor(loaded: LoadedConfig) {
    this.configPath = loaded.configPath;
    this.current = loaded.config;
    this.document = structuredClone(loaded.document);
    this.canonical = structuredClone(loaded.canonical);
    this.secrets = structuredClone(loaded.secrets);
    this.secretStorePath = loaded.secretStorePath;
    this.migrationApplied = loaded.migrationApplied;
    this.migratedFromSchema = loaded.migratedFromSchema;
  }

  private async persist(document: AgentConfigDocument): Promise<void> {
    const workspaceId = this.current.workspaceId;
    if (workspaceId && sharedStoreAvailable()) {
      const latest = await readSharedWorkspace(workspaceId) ?? this.canonical;
      this.canonical = overlayAgentDocumentOnCanonical(document, {
        id: workspaceId,
        name: this.current.workspaceName
      }, latest);
      await writeSharedWorkspace(workspaceId, this.canonical);
      return;
    }
    this.canonical = await writeConfigDocument(this.configPath, document, {
      identity: { id: workspaceId, name: this.current.workspaceName },
      canonical: this.canonical
    });
  }

  private async persistSecrets(
    secrets: AgentSecrets,
    document: AgentConfigDocument = this.document
  ): Promise<SecretStoreState> {
    const workspaceId = this.current.workspaceId;
    if (workspaceId && sharedStoreAvailable()) {
      const storePath = await writeSharedAgentSecrets(workspaceId, secrets);
      return {
        secrets: structuredClone(secrets),
        storePath: storePath ?? sharedSecretsFile(workspaceId),
        keyPath: '',
        created: false,
        changed: true
      };
    }
    return writeAgentSecrets(resolveDataDir(document), secrets);
  }

  exportSecretDocument(): JsonObject {
    return {
      oauthPassword: this.secrets.oauthPassword ?? '',
      oauthClientSecret: this.secrets.oauthClientSecret ?? '',
      oauthTokenSecret: this.secrets.oauthTokenSecret ?? '',
      tunnelEnrollmentUrl: this.secrets.tunnelEnrollmentUrl ?? ''
    };
  }

  async replaceImportedSecrets(secrets: AgentSecrets): Promise<void> {
    const next = { ...this.secrets };
    if (secrets.oauthPassword) next.oauthPassword = secrets.oauthPassword;
    if (secrets.oauthClientSecret) next.oauthClientSecret = secrets.oauthClientSecret;
    if (secrets.oauthTokenSecret) next.oauthTokenSecret = secrets.oauthTokenSecret;
    if (secrets.tunnelEnrollmentUrl) next.tunnelEnrollmentUrl = secrets.tunnelEnrollmentUrl;
    if (secrets.named) next.named = { ...(next.named ?? {}), ...secrets.named };
    const secretState = await this.persistSecrets(next);
    this.secrets = secretState.secrets;
    this.secretStorePath = secretState.storePath;
    if (next.oauthPassword) this.current.oauth.password = next.oauthPassword;
    if (next.oauthClientSecret) this.current.oauth.clientSecret = next.oauthClientSecret;
    if (next.oauthTokenSecret) this.current.oauth.tokenSecret = next.oauthTokenSecret;
  }

  exportCanonicalDocument(): CanonicalWorkspace {
    return structuredClone(this.canonical);
  }

  exportWorkspacePack(): JsonObject {
    return exportWorkspacePack(this.canonical, {
      oauthPassword: Boolean(this.secrets.oauthPassword),
      oauthClientSecret: Boolean(this.secrets.oauthClientSecret),
      oauthTokenSecret: Boolean(this.secrets.oauthTokenSecret),
      tunnelEnrollmentUrl: Boolean(this.secrets.tunnelEnrollmentUrl)
    });
  }

  snapshot(): JsonObject {
    const saved = validateConfigDocument(this.document);
    return {
      schemaVersion: CURRENT_CONFIG_SCHEMA_VERSION,
      configPath: this.configPath,
      secretStorePath: this.secretStorePath,
      migrationApplied: this.migrationApplied,
      migratedFromSchema: this.migratedFromSchema ?? null,
      restartRequired: restartRequired(this.current, this.document, this.secrets),
      effective: safeConfig(this.current, this.secrets, true),
      saved: safeConfig(saved, this.secrets, false),
      environmentOverrides: environmentKeys.filter(key => process.env[key] !== undefined)
    };
  }

  secret(key: 'oauthPassword'): string {
    if (key === 'oauthPassword' && process.env.CTMCP_OAUTH_PASSWORD !== undefined) {
      return this.current.oauth.password;
    }
    return this.secrets[key] ?? this.current.oauth.password;
  }

  secretValue(reference: string): string | undefined {
    switch (reference) {
      case 'oauth_password': return process.env.CTMCP_OAUTH_PASSWORD !== undefined
        ? this.current.oauth.password
        : this.secrets.oauthPassword;
      case 'oauth_client_secret': return this.secrets.oauthClientSecret;
      case 'oauth_token_secret': return this.secrets.oauthTokenSecret;
      case 'builtin_tunnel_enrollment_url': return this.secrets.tunnelEnrollmentUrl;
      default: return this.secrets.named?.[reference];
    }
  }

  async replaceNamedSecret(reference: string, value: string): Promise<void> {
    const key = reference.trim();
    if (!/^[A-Za-z0-9._-]{1,128}$/.test(key)) {
      throw new Error('Named secret references must use 1-128 letters, numbers, dot, underscore, or hyphen');
    }
    if (['oauth_password', 'oauth_client_secret', 'oauth_token_secret', 'builtin_tunnel_enrollment_url'].includes(key)) {
      throw new Error(`Named secret reference is reserved: ${key}`);
    }
    if (!value.trim()) throw new Error(`${key} must not be blank`);
    if (value.length > 4096) throw new Error(`${key} exceeds 4096 characters`);
    const nextSecrets: AgentSecrets = {
      ...this.secrets,
      named: { ...(this.secrets.named ?? {}), [key]: value }
    };
    const secretState = await this.persistSecrets(nextSecrets);
    this.secrets = secretState.secrets;
    this.secretStorePath = secretState.storePath;
  }

  async rotateOAuthPassword(value: string): Promise<void> {
    if (process.env.CTMCP_OAUTH_PASSWORD !== undefined) {
      throw new Error('One-time OAuth password rotation is unavailable while CTMCP_OAUTH_PASSWORD overrides the secret store');
    }
    if (!value.trim()) throw new Error('oauthPassword must not be blank');
    const nextSecrets = { ...this.secrets, oauthPassword: value };
    const secretState = await this.persistSecrets(nextSecrets);
    this.secrets = secretState.secrets;
    this.secretStorePath = secretState.storePath;
    this.current.oauth.password = value;
  }

  async replaceSecret(
    key: 'oauthPassword' | 'tunnelEnrollmentUrl',
    value: string,
    runtime?: RuntimeHotApplyTarget
  ): Promise<boolean> {
    if (!value.trim()) throw new Error(`${key} must not be blank`);
    const nextSecrets = { ...this.secrets, [key]: value };
    const secretState = await this.persistSecrets(nextSecrets);
    this.secrets = secretState.secrets;
    this.secretStorePath = secretState.storePath;

    if (key === 'oauthPassword') {
      if (!runtime?.oauth || process.env.CTMCP_OAUTH_PASSWORD !== undefined) return false;
      const oauth = { ...runtime.context.config.oauth, password: value };
      runtime.oauth.update(oauth);
      this.current.oauth = structuredClone(oauth);
      runtime.context.config.oauth = structuredClone(oauth);
      return true;
    }

    if (!runtime?.tunnel || !runtime.context.config.tunnel || process.env.CTMCP_BUILTIN_ENROLLMENT_URL !== undefined) {
      return false;
    }
    const tunnel = { ...runtime.context.config.tunnel, enrollmentUrl: value };
    await runtime.tunnel.reconfigure(tunnel, runtime.context.config.publicBaseUrl);
    this.current.tunnel = structuredClone(tunnel);
    runtime.context.config.tunnel = structuredClone(tunnel);
    return true;
  }

  async applyResolvedBuiltinTunnel(publicUrl: string, enrollmentCompleted: boolean): Promise<void> {
    const endpoint = parseBuiltinPublicUrl(publicUrl);
    const document = structuredClone(this.document);
    document.tunnel = {
      ...document.tunnel,
      enabled: true,
      publicUrl: endpoint.publicUrl
    };
    document.publicBaseUrl = endpoint.baseUrl;
    const nextSecrets = structuredClone(this.secrets);
    if (enrollmentCompleted) delete nextSecrets.tunnelEnrollmentUrl;

    validateConfigDocument(document);
    const targetDataDir = resolveDataDir(document);
    const secretSnapshot = await snapshotAgentSecretFiles(targetDataDir);
    const sharedSecretSnapshot = this.current.workspaceId
      ? await snapshotSharedSecrets(this.current.workspaceId)
      : undefined;
    let secretState: SecretStoreState;
    try {
      secretState = await this.persistSecrets(nextSecrets, document);
      await this.persist(document);
    } catch (error) {
      try {
        await restoreAgentSecretFiles(secretSnapshot);
        await restoreSharedSecrets(sharedSecretSnapshot);
      } catch (rollbackError) {
        throw new AggregateError(
          [error, rollbackError],
          'Resolved tunnel persistence failed and the encrypted secret store rollback also failed.'
        );
      }
      throw error;
    }

    this.document = document;
    this.secrets = secretState.secrets;
    this.secretStorePath = secretState.storePath;
    if (this.current.tunnel) {
      this.current.tunnel.publicUrl = endpoint.publicUrl;
      if (enrollmentCompleted) delete this.current.tunnel.enrollmentUrl;
    }
    this.current.publicBaseUrl = endpoint.baseUrl;
  }

  async setSkillActive(
    active: boolean,
    runtime?: RuntimeHotApplyTarget
  ): Promise<JsonObject> {
    const savedBefore = validateConfigDocument(this.document);
    const document = structuredClone(this.document);
    document.skills = {
      active,
      disabled: [...savedBefore.skills.disabled]
    };
    validateConfigDocument(document);
    await this.persist(document);
    this.document = document;

    const appliedImmediately: string[] = [];
    if (runtime) {
      const skills = { active, disabled: [...savedBefore.skills.disabled] };
      this.current.skills = skills;
      runtime.context.config.skills = structuredClone(skills);
      for (const folderRuntime of allFolderRuntimes(runtime.context)) {
        folderRuntime.skillRegistry.setActive(active);
        folderRuntime.skillRegistry.setDisabledSkillKeys(skills.disabled);
      }
      appliedImmediately.push('skills');
    }

    const saved = validateConfigDocument(document);
    const needsRestart = restartRequired(this.current, document, this.secrets);
    return {
      ok: true,
      schemaVersion: CURRENT_CONFIG_SCHEMA_VERSION,
      configPath: this.configPath,
      secretStorePath: this.secretStorePath,
      restartRequired: needsRestart,
      appliedImmediately,
      hotApplyDeferredReason: null,
      saved: safeConfig(saved, this.secrets, false),
      environmentOverrides: environmentKeys.filter(environmentKey => process.env[environmentKey] !== undefined),
      warning: needsRestart
        ? 'The Skills master setting was applied immediately; other saved settings still require an Agent restart.'
        : null
    };
  }

  async setSkillEnabled(
    key: string,
    enabled: boolean,
    runtime?: RuntimeHotApplyTarget
  ): Promise<JsonObject> {
    const normalizedKey = key.trim();
    if (!normalizedKey || normalizedKey.length > 4096) {
      throw new Error('skill key must contain 1 to 4096 characters');
    }
    const savedBefore = validateConfigDocument(this.document);
    const disabled = new Set(savedBefore.skills.disabled);
    if (enabled) disabled.delete(normalizedKey);
    else disabled.add(normalizedKey);
    const document = structuredClone(this.document);
    document.skills = {
      active: savedBefore.skills.active,
      disabled: [...disabled].sort()
    };
    validateConfigDocument(document);
    await this.persist(document);
    this.document = document;

    const appliedImmediately: string[] = [];
    if (runtime) {
      const skills = {
        active: document.skills?.active ?? true,
        disabled: [...(document.skills?.disabled ?? [])]
      };
      this.current.skills = skills;
      runtime.context.config.skills = structuredClone(skills);
      for (const folderRuntime of allFolderRuntimes(runtime.context)) {
        folderRuntime.skillRegistry.setActive(skills.active);
        folderRuntime.skillRegistry.setDisabledSkillKeys(skills.disabled);
      }
      appliedImmediately.push('skills');
    }

    const saved = validateConfigDocument(document);
    const needsRestart = restartRequired(this.current, document, this.secrets);
    return {
      ok: true,
      schemaVersion: CURRENT_CONFIG_SCHEMA_VERSION,
      configPath: this.configPath,
      secretStorePath: this.secretStorePath,
      restartRequired: needsRestart,
      appliedImmediately,
      hotApplyDeferredReason: null,
      saved: safeConfig(saved, this.secrets, false),
      environmentOverrides: environmentKeys.filter(environmentKey => process.env[environmentKey] !== undefined),
      warning: needsRestart
        ? 'The Skill setting was applied immediately; other saved settings still require an Agent restart.'
        : null
    };
  }

  async setExtensionActive(
    kind: 'hook' | 'mcp',
    active: boolean,
    runtime?: RuntimeHotApplyTarget
  ): Promise<JsonObject> {
    const savedBefore = validateConfigDocument(this.document);
    const document = structuredClone(this.document);
    document.extensions = {
      hooks: {
        active: kind === 'hook' ? active : savedBefore.extensions.hooks.active,
        enabled: [...savedBefore.extensions.hooks.enabled]
      },
      mcp: {
        active: kind === 'mcp' ? active : savedBefore.extensions.mcp.active,
        enabled: [...savedBefore.extensions.mcp.enabled]
      }
    };
    validateConfigDocument(document);
    await this.persist(document);
    this.document = document;

    const appliedImmediately: string[] = [];
    if (runtime) {
      const extensions = {
        hooks: {
          active: document.extensions?.hooks?.active ?? true,
          enabled: [...(document.extensions?.hooks?.enabled ?? [])]
        },
        mcp: {
          active: document.extensions?.mcp?.active ?? true,
          enabled: [...(document.extensions?.mcp?.enabled ?? [])]
        }
      };
      this.current.extensions = extensions;
      runtime.context.config.extensions = structuredClone(extensions);
      await runtime.context.extensions.setActive(kind, active);
      appliedImmediately.push('extensions');
    }

    const saved = validateConfigDocument(document);
    const needsRestart = restartRequired(this.current, document, this.secrets);
    return {
      ok: true,
      schemaVersion: CURRENT_CONFIG_SCHEMA_VERSION,
      configPath: this.configPath,
      secretStorePath: this.secretStorePath,
      restartRequired: needsRestart,
      appliedImmediately,
      hotApplyDeferredReason: null,
      saved: safeConfig(saved, this.secrets, false),
      environmentOverrides: environmentKeys.filter(environmentKey => process.env[environmentKey] !== undefined),
      warning: needsRestart
        ? 'The extension master setting was applied immediately; other saved settings still require an Agent restart.'
        : null
    };
  }

  async setExtensionEnabled(
    kind: 'hook' | 'mcp',
    key: string,
    enabled: boolean,
    runtime?: RuntimeHotApplyTarget
  ): Promise<JsonObject> {
    const normalizedKey = key.trim();
    if (!normalizedKey || normalizedKey.length > 4096) throw new Error('extension key must contain 1 to 4096 characters');
    const savedBefore = validateConfigDocument(this.document);
    const hooks = new Set(savedBefore.extensions.hooks.enabled);
    const mcp = new Set(savedBefore.extensions.mcp.enabled);
    const target = kind === 'hook' ? hooks : mcp;
    if (enabled) target.add(normalizedKey);
    else target.delete(normalizedKey);
    const document = structuredClone(this.document);
    document.extensions = {
      hooks: { active: savedBefore.extensions.hooks.active, enabled: [...hooks].sort() },
      mcp: { active: savedBefore.extensions.mcp.active, enabled: [...mcp].sort() }
    };
    validateConfigDocument(document);
    await this.persist(document);
    this.document = document;

    const appliedImmediately: string[] = [];
    if (runtime) {
      const extensions = {
        hooks: {
          active: document.extensions?.hooks?.active ?? true,
          enabled: [...(document.extensions?.hooks?.enabled ?? [])]
        },
        mcp: {
          active: document.extensions?.mcp?.active ?? true,
          enabled: [...(document.extensions?.mcp?.enabled ?? [])]
        }
      };
      this.current.extensions = extensions;
      runtime.context.config.extensions = structuredClone(extensions);
      await runtime.context.extensions.setEnabled(kind, kind === 'hook' ? extensions.hooks.enabled : extensions.mcp.enabled);
      appliedImmediately.push('extensions');
    }

    const saved = validateConfigDocument(document);
    const needsRestart = restartRequired(this.current, document, this.secrets);
    return {
      ok: true,
      schemaVersion: CURRENT_CONFIG_SCHEMA_VERSION,
      configPath: this.configPath,
      secretStorePath: this.secretStorePath,
      restartRequired: needsRestart,
      appliedImmediately,
      hotApplyDeferredReason: null,
      saved: safeConfig(saved, this.secrets, false),
      environmentOverrides: environmentKeys.filter(environmentKey => process.env[environmentKey] !== undefined),
      warning: needsRestart ? 'The extension setting was applied immediately; other saved settings still require an Agent restart.' : null
    };
  }

  async save(value: unknown, runtime?: RuntimeHotApplyTarget): Promise<JsonObject> {
    const input = record(value, 'config');
    const oauthInput = record(input.oauth ?? {}, 'oauth');
    const limitsInput = record(input.limits ?? {}, 'limits');
    const policyInput = record(input.policy ?? this.current.policy, 'policy');
    const securityPolicyInput = record(
      input.securityPolicy ?? this.current.securityPolicy,
      'securityPolicy'
    );
    const managementInput = record(input.management ?? {}, 'management');
    const skills = parseSkills(input.skills, this.current.skills);
    const extensions = parseExtensions(input.extensions, this.current.extensions);
    const sandbox = parseSandbox(input.sandbox, this.current.sandbox);
    const tunnelInput = record(input.tunnel ?? {}, 'tunnel');
    const nextSecrets = structuredClone(this.secrets);

    const securityPolicy = normalizeSecurityPolicy(
      securityPolicyInput as Partial<AgentConfig['securityPolicy']>
    );
    const mode = compatibilityPermissionMode(securityPolicy);
    const toolProfile = compatibilityToolProfile(securityPolicy);

    const oauth: AgentConfigDocument['oauth'] = {
      clientId: stringValue(oauthInput.clientId, 'oauth.clientId', this.current.oauth.clientId, 256)
    };
    const password = secretValue(oauthInput.password, 'oauth.password');
    if (password) nextSecrets.oauthPassword = password;
    const clientSecret = secretValue(oauthInput.clientSecret, 'oauth.clientSecret');
    if (clientSecret) nextSecrets.oauthClientSecret = clientSecret;
    else if (oauthInput.clearClientSecret === true) delete nextSecrets.oauthClientSecret;

    const tunnelPublicUrl = stringValue(tunnelInput.publicUrl, 'tunnel.publicUrl', '', 2048);
    let tunnel: AgentConfigDocument['tunnel'];
    if (tunnelPublicUrl) {
      let normalizedTunnelPublicUrl: string;
      try {
        normalizedTunnelPublicUrl = parseBuiltinPublicUrl(tunnelPublicUrl).publicUrl;
      } catch (error) {
        throw new Error(
          `tunnel.publicUrl is invalid: ${error instanceof Error ? error.message : String(error)}`
        );
      }
      tunnel = {
        enabled: booleanValue(tunnelInput.enabled, true),
        publicUrl: normalizedTunnelPublicUrl
      };
      const enrollmentUrl = secretValue(tunnelInput.enrollmentUrl, 'tunnel.enrollmentUrl');
      if (enrollmentUrl) nextSecrets.tunnelEnrollmentUrl = enrollmentUrl;
      else if (tunnelInput.clearEnrollmentUrl === true) delete nextSecrets.tunnelEnrollmentUrl;
      if (this.document.tunnel?.stateFile) tunnel.stateFile = this.document.tunnel.stateFile;
    } else {
      delete nextSecrets.tunnelEnrollmentUrl;
    }

    const folders = await canonicalizeWorkspaceFolders(
      parseFolders(input.folders ?? this.current.folders)
    );

    const document: AgentConfigDocument = {
      schema_version: CURRENT_CONFIG_SCHEMA_VERSION,
      host: stringValue(input.host, 'host', this.current.host, 255),
      port: integerValue(input.port, 'port', this.current.port, 1, 65_535),
      dataDir: path.resolve(stringValue(input.dataDir, 'dataDir', this.current.dataDir, 4096)),
      permissionMode: mode,
      toolProfile,
      securityPolicy,
      policy: {
        allowedCommands: Array.isArray(policyInput.allowedCommands)
          ? policyInput.allowedCommands.map(String)
          : String(policyInput.allowedCommands ?? '')
            .split(',')
            .map(value => value.trim())
            .filter(Boolean),
        workspaceLocalEntries: booleanValue(
          policyInput.workspaceLocalEntries,
          this.current.policy.workspaceLocalEntries
        ),
        workspaceScriptExtensions: Array.isArray(policyInput.workspaceScriptExtensions)
          ? policyInput.workspaceScriptExtensions.map(String)
          : String(policyInput.workspaceScriptExtensions ?? '')
            .split(',')
            .map(value => value.trim())
            .filter(Boolean),
        maxPatchBytes: integerValue(
          policyInput.maxPatchBytes,
          'policy.maxPatchBytes',
          this.current.policy.maxPatchBytes,
          1,
          16 * 1024 * 1024
        )
      },
      management: { enabled: booleanValue(managementInput.enabled, true) },
      skills,
      extensions,
      sandbox,
      oauth,
      folders,
      limits: {
        blockingConcurrency: integerValue(
          limitsInput.blockingConcurrency,
          'limits.blockingConcurrency',
          this.current.limits.blockingConcurrency,
          1,
          65_535
        ),
        processConcurrency: integerValue(
          limitsInput.processConcurrency,
          'limits.processConcurrency',
          this.current.limits.processConcurrency,
          1,
          65_535
        ),
        globalBlockingConcurrency: integerValue(
          limitsInput.globalBlockingConcurrency,
          'limits.globalBlockingConcurrency',
          this.current.limits.globalBlockingConcurrency,
          1,
          65_535
        ),
        globalProcessConcurrency: integerValue(
          limitsInput.globalProcessConcurrency,
          'limits.globalProcessConcurrency',
          this.current.limits.globalProcessConcurrency,
          1,
          65_535
        ),
        activeSessionLimit: integerValue(
          limitsInput.activeSessionLimit,
          'limits.activeSessionLimit',
          this.current.limits.activeSessionLimit,
          1,
          65_535
        ),
        maxOutputBytes: integerValue(
          limitsInput.maxOutputBytes,
          'limits.maxOutputBytes',
          this.current.limits.maxOutputBytes,
          1_024,
          16 * 1024 * 1024
        ),
        commandTimeoutMaxMs: integerValue(
          limitsInput.commandTimeoutMaxMs,
          'limits.commandTimeoutMaxMs',
          this.current.limits.commandTimeoutMaxMs,
          1,
          ABSOLUTE_COMMAND_TIMEOUT_MAX_MS
        )
      },
      ...(tunnel ? { tunnel } : {})
    };
    const publicBaseUrl = stringValue(input.publicBaseUrl, 'publicBaseUrl', '', 2048);
    if (publicBaseUrl) document.publicBaseUrl = publicBaseUrl;

    validateConfigDocument(document);
    const desiredBeforeSave = normalizeConfig(document, nextSecrets);
    desiredBeforeSave.workspaceId = this.current.workspaceId;
    desiredBeforeSave.workspaceName = this.current.workspaceName;
    if (runtime
      && desiredBeforeSave.sandbox.enabled
      && !sameRuntimeValue(runtime.context.config.sandbox, desiredBeforeSave.sandbox)) {
      if (!runtime.preflightSandbox) {
        throw new Error('Sandbox hot-apply preflight is unavailable for this runtime.');
      }
      await runtime.preflightSandbox(desiredBeforeSave.sandbox);
    }
    const targetDataDir = resolveDataDir(document);
    const secretSnapshot = await snapshotAgentSecretFiles(targetDataDir);
    const sharedSecretSnapshot = this.current.workspaceId
      ? await snapshotSharedSecrets(this.current.workspaceId)
      : undefined;
    let secretState: SecretStoreState;
    try {
      secretState = await this.persistSecrets(nextSecrets, document);
      await this.persist(document);
    } catch (error) {
      try {
        await restoreAgentSecretFiles(secretSnapshot);
        await restoreSharedSecrets(sharedSecretSnapshot);
      } catch (rollbackError) {
        throw new AggregateError(
          [error, rollbackError],
          'Configuration save failed and the encrypted secret store rollback also failed.'
        );
      }
      throw error;
    }
    this.document = document;
    this.secrets = secretState.secrets;
    this.secretStorePath = secretState.storePath;
    const saved = validateConfigDocument(document);
    const desired = normalizeConfig(document, this.secrets);
    desired.workspaceId = this.current.workspaceId;
    desired.workspaceName = this.current.workspaceName;
    const currentSecurityStable = this.current.permissionMode === desired.permissionMode
      && this.current.activeToolProfile === desired.activeToolProfile
      && sameRuntimeValue(this.current.securityPolicy, desired.securityPolicy);
    if (currentSecurityStable) {
      this.current.toolProfile = desired.toolProfile;
      this.current.securityPolicyCustomized = desired.securityPolicyCustomized;
    }
    const appliedImmediately: string[] = [];
    const hotApplyDeferredReasons: string[] = [];
    if (runtime) {
      const folderApply = await applyWorkspaceFolderConfiguration(runtime.context, desired.folders);
      if (folderApply.applied) {
        this.current.folders = desired.folders.map(folder => ({ ...folder }));
        appliedImmediately.push('folders');
      } else if (folderApply.changed && folderApply.deferredReason) {
        hotApplyDeferredReasons.push(folderApply.deferredReason);
      }
      const runtimeApply = await applyRuntimeConfiguration(this.current, runtime, desired);
      appliedImmediately.push(...runtimeApply.applied);
      hotApplyDeferredReasons.push(...runtimeApply.deferredReasons);
    }
    const hotApplyDeferredReason = hotApplyDeferredReasons.length
      ? hotApplyDeferredReasons.join(' ')
      : null;
    const needsRestart = restartRequired(this.current, document, this.secrets);
    return {
      ok: true,
      schemaVersion: CURRENT_CONFIG_SCHEMA_VERSION,
      configPath: this.configPath,
      secretStorePath: this.secretStorePath,
      restartRequired: needsRestart,
      appliedImmediately,
      hotApplyDeferredReason,
      saved: safeConfig(saved, this.secrets, false),
      environmentOverrides: environmentKeys.filter(key => process.env[key] !== undefined),
      warning: needsRestart
        ? hotApplyDeferredReason ?? (appliedImmediately.length
          ? 'Some settings were applied immediately. Restart the agent to apply the remaining settings.'
          : 'The configuration file was saved. Restart the agent to apply it.')
        : null
    };
  }
}
