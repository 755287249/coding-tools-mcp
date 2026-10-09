import {
  DEFAULT_SERVICE_PORT,
  type SecurityPolicy,
  type WorkspaceExecutionTarget,
  type WorkspaceProfile,
} from "../types";

export const CANONICAL_SCHEMA_VERSION = 2 as const;

const securityPolicyKeys = [
  "restrictToolCatalog",
  "enforceCommandAllowlist",
  "requireDangerousConfirmation",
  "requireShellConfirmation",
  "blockNetworkCommands",
  "enforceWorkspaceBoundary",
  "protectRepositoryMetadata",
  "blockSymlinkEscape",
  "protectEnvironmentVariables",
  "enforceHarnessBaseline",
  "requireWriteConfirmation",
  "verifyWriteConflicts",
  "enforceResourceLimits",
  "redactSensitiveOutput",
  "withholdSensitiveSourceOutput",
  "redactTelemetry",
  "redactHistory",
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

function workspaceExecutionTarget(value: unknown): WorkspaceExecutionTarget | undefined {
  if (!isRecord(value)) return undefined;
  const kind = text(value.kind);
  if (kind === "host") return { kind: "host" };
  if (kind !== "wsl") return undefined;
  const distro = text(value.distro);
  const linuxPath = text(value.linux_path ?? value.linuxPath);
  if (!distro || !linuxPath) return undefined;
  return { kind: "wsl", distro, linux_path: linuxPath };
}

function parseFolders(value: unknown): CanonicalFolder[] {
  if (!Array.isArray(value)) return [];
  return value.flatMap((entry, index) => {
    if (!isRecord(entry)) return [];
    const path = text(entry.path);
    if (!path) return [];
    const { id, name, path: folderPath, ...extra } = entry;
    return [{
      id: text(id, `folder-${index + 1}`),
      name: text(name, path.split(/[\\/]/).filter(Boolean).at(-1) || 'folder'),
      path,
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
  return parseCanonicalWorkspace({
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
      node: {
        dataDir: text(value.dataDir),
        management: { enabled: bool(management.enabled, true) }
      }
    }
  });
}

export function migrateDesktopProfile(value: unknown): CanonicalWorkspace {
  if (!isRecord(value)) throw new Error('Desktop profile must be an object');
  const runtime = recordValue(value.runtime);
  const auth = recordValue(value.auth);
  const tunnel = recordValue(value.tunnel);
  const actions = recordValue(value.actions);
  const sandbox = recordValue(runtime.sandbox);
  const folders = Array.isArray(value.folders) && value.folders.length
    ? parseFolders(value.folders)
    : parseFolders([{ id: 'legacy', name: text(value.name), path: text(value.path) }]);
  const tunnelType = text(tunnel.type, 'none') || 'none';
  const desktopHost: Record<string, unknown> = {};
  if (text(auth.type) && text(auth.type) !== 'oauth') desktopHost.authType = text(auth.type);
  if (bool(auth.use_shared_secrets, false)) desktopHost.useSharedSecrets = true;
  if (text(runtime.transport_mode) && text(runtime.transport_mode) !== 'streamable-http') {
    desktopHost.transportMode = text(runtime.transport_mode);
  }
  if (text(runtime.runtime_command)) desktopHost.runtimeCommand = text(runtime.runtime_command);
  if (tunnelType && tunnelType !== 'builtin' && tunnelType !== 'none') {
    desktopHost.tunnel = {
      type: tunnelType,
      frpServer: text(tunnel.frp_server),
      frpSubdomain: text(tunnel.frp_subdomain),
      frpProfileId: text(tunnel.frp_profile_id),
      frpServerPort: integer(tunnel.frp_server_port, 443),
      cloudflareMode: text(tunnel.cloudflare_mode, 'quick'),
      publicUrl: text(tunnel.public_url),
      useProxy: bool(tunnel.use_proxy, true)
    };
  } else if (bool(tunnel.use_proxy, true) === false) {
    desktopHost.useProxy = false;
  }
  if (Object.keys(actions).length) {
    desktopHost.actions = stripSecrets(actions);
  }
  const policy: Record<string, unknown> = {};
  const sourcePolicy = recordValue(runtime.security_policy);
  for (const [snake, camel] of [
    ['restrict_tool_catalog', 'restrictToolCatalog'],
    ['enforce_command_allowlist', 'enforceCommandAllowlist'],
    ['require_dangerous_confirmation', 'requireDangerousConfirmation'],
    ['require_shell_confirmation', 'requireShellConfirmation'],
    ['block_network_commands', 'blockNetworkCommands'],
    ['enforce_workspace_boundary', 'enforceWorkspaceBoundary'],
    ['protect_repository_metadata', 'protectRepositoryMetadata'],
    ['block_symlink_escape', 'blockSymlinkEscape'],
    ['protect_environment_variables', 'protectEnvironmentVariables'],
    ['enforce_harness_baseline', 'enforceHarnessBaseline'],
    ['require_write_confirmation', 'requireWriteConfirmation'],
    ['verify_write_conflicts', 'verifyWriteConflicts'],
    ['enforce_resource_limits', 'enforceResourceLimits'],
    ['redact_sensitive_output', 'redactSensitiveOutput'],
    ['withhold_sensitive_source_output', 'withholdSensitiveSourceOutput'],
    ['redact_telemetry', 'redactTelemetry'],
    ['redact_history', 'redactHistory']
  ] as const) {
    if (typeof sourcePolicy[snake] === 'boolean') policy[camel] = sourcePolicy[snake];
  }
  return parseCanonicalWorkspace({
    schemaVersion: CANONICAL_SCHEMA_VERSION,
    id: text(value.id),
    name: text(value.name, folders[0]?.name || 'Workspace'),
    folders,
    activeFolderId: text(value.active_folder_id, folders[0]?.id ?? ''),
    bind: {
      host: text(runtime.bind_address, '127.0.0.1'),
      port: integer(runtime.local_port, 18790)
    },
    publicBaseUrl: tunnelType === 'builtin' ? text(tunnel.public_url).replace(/\/mcp\/?$/, '') : '',
    auth: {
      type: 'oauth',
      oauthClientId: text(auth.oauth_client_id),
      oauthTokenTtlSeconds: integer(auth.oauth_token_ttl_seconds, 3650 * 24 * 60 * 60)
    },
    toolProfile: text(runtime.tool_profile, 'core') || 'core',
    permissionMode: text(runtime.permission_mode, 'trusted') || 'trusted',
    securityPolicy: policy,
    policy: {
      allowedCommands: stringList(runtime.allowed_commands),
      workspaceLocalEntries: bool(runtime.workspace_local_entries, true),
      workspaceScriptExtensions: stringList(runtime.workspace_script_extensions),
      maxPatchBytes: integer(recordValue(value.actions).max_patch_bytes, 1024 * 1024)
    },
    sandbox: {
      enabled: bool(sandbox.enabled, false),
      backend: text(sandbox.backend, 'appcontainer'),
      externalPaths: Array.isArray(sandbox.external_paths)
        ? sandbox.external_paths.flatMap(entry => {
            if (!isRecord(entry) || !text(entry.path)) return [];
            return [{ path: text(entry.path), access: text(entry.access, 'read_only') || 'read_only' }];
          })
        : [],
      options: recordValue(sandbox.options)
    },
    limits: {
      blockingConcurrency: integer(runtime.blocking_admission_limit, 128),
      processConcurrency: integer(runtime.process_admission_limit, 64),
      globalBlockingConcurrency: integer(runtime.global_blocking_admission_limit, 1024),
      globalProcessConcurrency: integer(runtime.global_process_admission_limit, 512),
      activeSessionLimit: integer(runtime.active_session_limit, 512),
      maxOutputBytes: 1024 * 1024,
      commandTimeoutMaxMs: 0
    },
    tunnel: {
      builtin: {
        enabled: tunnelType === 'builtin',
        publicUrl: tunnelType === 'builtin' ? text(tunnel.public_url) : ''
      }
    },
    host: { desktop: desktopHost, node: {} }
  });
}

export function canonicalSharedFields(document: CanonicalWorkspace): Record<string, unknown> {
  const { host, extra, ...shared } = document;
  void host;
  void extra;
  return shared;
}

function camelToSnakePolicy(source: Record<string, boolean>): SecurityPolicy {
  const policy = {} as SecurityPolicy;
  for (const camel of securityPolicyKeys) {
    const snake = camel.replace(/[A-Z]/g, (ch) => `_${ch.toLowerCase()}`) as keyof SecurityPolicy;
    policy[snake] = Boolean(source[camel]);
  }
  return policy;
}

export function canonicalToWorkspaceProfile(document: CanonicalWorkspace): WorkspaceProfile {
  const folders = document.folders.map((folder) => {
    const { id, name, path, execution, ...extra } = folder;
    void extra;
    const target = workspaceExecutionTarget(execution);
    return {
      id,
      name,
      path,
      ...(target ? { execution: target } : {}),
    };
  });
  const desktop = document.host.desktop;
  const desktopTunnel = isRecord(desktop.tunnel) ? desktop.tunnel : {};
  const tunnelType = document.tunnel.builtin.enabled
    ? "builtin"
    : text(desktopTunnel.type, "none") || "none";
  const actions = isRecord(desktop.actions) ? desktop.actions : undefined;
  return {
    id: document.id,
    name: document.name,
    path: folders[0]?.path ?? "",
    folders,
    active_folder_id: document.activeFolderId,
    tunnel: {
      type: tunnelType,
      public_url: document.tunnel.builtin.enabled
        ? document.tunnel.builtin.publicUrl
        : text(desktopTunnel.publicUrl),
      frp_server: text(desktopTunnel.frpServer),
      frp_subdomain: text(desktopTunnel.frpSubdomain),
      frp_profile_id: text(desktopTunnel.frpProfileId) || undefined,
      frp_server_port: integer(desktopTunnel.frpServerPort, 443),
      cloudflare_mode: text(desktopTunnel.cloudflareMode, "quick") || "quick",
      use_proxy: bool(
        desktopTunnel.useProxy !== undefined ? desktopTunnel.useProxy : desktop.useProxy,
        true,
      ),
    },
    auth: {
      type: text(desktop.authType, document.auth.type) || "oauth",
      oauth_client_id: document.auth.oauthClientId,
      oauth_token_ttl_seconds: document.auth.oauthTokenTtlSeconds,
      use_shared_secrets: bool(desktop.useSharedSecrets, false),
    },
    runtime: {
      local_port: document.bind.port || DEFAULT_SERVICE_PORT,
      bind_address: document.bind.host || "127.0.0.1",
      transport_mode: (text(desktop.transportMode, "streamable-http") || "streamable-http") as
        | "streamable-http"
        | "legacy-json",
      tool_profile: document.toolProfile,
      permission_mode: document.permissionMode,
      security_policy: camelToSnakePolicy(document.securityPolicy),
      runtime_command: text(desktop.runtimeCommand) || undefined,
      allowed_commands: document.policy.allowedCommands.join(","),
      workspace_local_entries: document.policy.workspaceLocalEntries,
      workspace_script_extensions: document.policy.workspaceScriptExtensions.join(","),
      blocking_admission_limit: document.limits.blockingConcurrency,
      process_admission_limit: document.limits.processConcurrency,
      global_blocking_admission_limit: document.limits.globalBlockingConcurrency,
      global_process_admission_limit: document.limits.globalProcessConcurrency,
      sandbox: {
        enabled: document.sandbox.enabled,
        backend: document.sandbox.backend,
        external_paths: document.sandbox.externalPaths.map((entry) => ({
          path: entry.path,
          access: entry.access === "modify" ? "modify" : "read_only",
        })),
        options: { ...document.sandbox.options },
      },
      active_session_limit: document.limits.activeSessionLimit,
    },
    ...(actions
      ? {
          actions: {
            public_url: text(actions.public_url),
            tunnel_type: text(actions.tunnel_type, "none") || "none",
            frp_server: text(actions.frp_server),
            frp_subdomain: text(actions.frp_subdomain),
            frp_profile_id: text(actions.frp_profile_id) || undefined,
            frp_server_port: integer(actions.frp_server_port, 443),
            cloudflare_mode: text(actions.cloudflare_mode, "quick") || "quick",
            local_port: integer(actions.local_port, document.bind.port + 1),
            bind_address: text(actions.bind_address, document.bind.host),
            permission_mode: text(actions.permission_mode, document.permissionMode),
            runtime_command: text(actions.runtime_command) || undefined,
            auth_type: text(actions.auth_type, "oauth") || "oauth",
            oauth_client_id: text(actions.oauth_client_id, document.auth.oauthClientId),
            oauth_scopes: text(actions.oauth_scopes) || undefined,
            allowed_commands: text(actions.allowed_commands) || undefined,
            max_patch_bytes: integer(actions.max_patch_bytes, document.policy.maxPatchBytes),
            use_shared_secrets: bool(actions.use_shared_secrets, false),
          },
        }
      : {}),
  };
}
