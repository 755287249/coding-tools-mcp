import {
  DEFAULT_SERVICE_PORT,
  sandboxConfig,
  securityPolicyConfig,
  type RuntimeStatus,
  type SandboxBackendDescriptor,
  type SandboxConfig,
  type SecurityPolicy,
  type WorkspaceProfile,
} from "../types";
import {
  canonicalToWorkspaceProfile,
  migrateDesktopProfile,
  migrateNodeV1Document,
} from "./workspace-document";

const POLICY_FIELDS = [
  ["restrict_tool_catalog", "restrictToolCatalog"],
  ["enforce_command_allowlist", "enforceCommandAllowlist"],
  ["require_dangerous_confirmation", "requireDangerousConfirmation"],
  ["require_shell_confirmation", "requireShellConfirmation"],
  ["block_network_commands", "blockNetworkCommands"],
  ["enforce_workspace_boundary", "enforceWorkspaceBoundary"],
  ["protect_repository_metadata", "protectRepositoryMetadata"],
  ["block_symlink_escape", "blockSymlinkEscape"],
  ["protect_environment_variables", "protectEnvironmentVariables"],
  ["enforce_harness_baseline", "enforceHarnessBaseline"],
  ["require_write_confirmation", "requireWriteConfirmation"],
  ["verify_write_conflicts", "verifyWriteConflicts"],
  ["enforce_resource_limits", "enforceResourceLimits"],
  ["redact_sensitive_output", "redactSensitiveOutput"],
  ["withhold_sensitive_source_output", "withholdSensitiveSourceOutput"],
  ["redact_telemetry", "redactTelemetry"],
  ["redact_history", "redactHistory"],
] as const;

export interface NodeFolder {
  id: string;
  name: string;
  path: string;
}

export interface NodeLimits {
  blockingConcurrency: number;
  processConcurrency: number;
  globalBlockingConcurrency: number;
  globalProcessConcurrency: number;
  activeSessionLimit: number;
  maxOutputBytes: number;
  commandTimeoutMaxMs: number;
}

export interface NodeSafeConfig {
  host: string;
  port: number;
  publicBaseUrl: string;
  dataDir: string;
  permissionMode: string;
  toolProfile: string;
  activeToolProfile: string;
  securityPolicy: Record<string, boolean>;
  management: { enabled: boolean };
  sandbox: {
    enabled: boolean;
    backend: string;
    externalPaths: Array<{ path: string; access: string }>;
    options: Record<string, string>;
  };
  sandboxBackends?: SandboxBackendDescriptor[];
  oauth: {
    clientId: string;
    passwordConfigured: boolean;
    clientSecretConfigured: boolean;
    tokenSecretSource: string;
    tokenTtlSeconds?: number;
  };
  policy: {
    allowedCommands: string[];
    workspaceLocalEntries: boolean;
    workspaceScriptExtensions: string[];
    maxPatchBytes: number;
  };
  folders: NodeFolder[];
  limits: NodeLimits;
  tunnel: {
    enabled: boolean;
    publicUrl: string;
    enrollmentConfigured: boolean;
  };
}

export interface NodeWorkspaceSnapshot {
  id: string;
  name: string;
  restartRequired?: boolean;
  effective: NodeSafeConfig;
  saved: NodeSafeConfig;
}

export interface NodeConfigSnapshot {
  primaryWorkspaceId: string;
  workspaces: NodeWorkspaceSnapshot[];
}

export interface NodeConfigUpdatePayload {
  name: string;
  host: string;
  port: number;
  publicBaseUrl: string;
  dataDir: string;
  securityPolicy: Record<string, boolean>;
  management: { enabled: boolean };
  sandbox: NodeSafeConfig["sandbox"];
  oauth: {
    clientId: string;
    tokenTtlSeconds?: number;
    password: string;
    clientSecret: string;
    clearClientSecret: boolean;
  };
  policy: NodeSafeConfig["policy"];
  folders: Array<{ id?: string; name?: string; path: string }>;
  limits: NodeLimits;
  tunnel: {
    enabled: boolean;
    publicUrl: string;
    enrollmentUrl: string;
    clearEnrollmentUrl: boolean;
  };
}

export function toDesktopPolicy(source: Record<string, boolean> | undefined): SecurityPolicy {
  const policy = {} as SecurityPolicy;
  for (const [desktop, node] of POLICY_FIELDS) {
    policy[desktop] = Boolean(source?.[node]);
  }
  return policy;
}

export function toNodePolicy(source: SecurityPolicy): Record<string, boolean> {
  const policy: Record<string, boolean> = {};
  for (const [desktop, node] of POLICY_FIELDS) {
    policy[node] = Boolean(source[desktop]);
  }
  return policy;
}

export function toDesktopSandbox(source: NodeSafeConfig["sandbox"] | undefined): SandboxConfig {
  return {
    enabled: Boolean(source?.enabled),
    backend: source?.backend || "appcontainer",
    external_paths: (source?.externalPaths ?? []).map((entry) => ({
      path: entry.path,
      access: entry.access === "modify" ? "modify" : "read_only",
    })),
    options: { ...(source?.options ?? {}) },
  };
}

export function toNodeSandbox(source: SandboxConfig): NodeSafeConfig["sandbox"] {
  return {
    enabled: source.enabled,
    backend: source.backend || "appcontainer",
    externalPaths: source.external_paths.map((entry) => ({
      path: entry.path,
      access: entry.access,
    })),
    options: { ...source.options },
  };
}

export function mapNodeWorkspace(snapshot: NodeWorkspaceSnapshot): WorkspaceProfile {
  const config = snapshot.saved ?? snapshot.effective;
  const canonical = migrateNodeV1Document(
    {
      schema_version: 1,
      host: config.host,
      port: config.port,
      publicBaseUrl: config.publicBaseUrl,
      dataDir: config.dataDir,
      permissionMode: config.permissionMode,
      toolProfile: config.toolProfile || config.activeToolProfile,
      securityPolicy: config.securityPolicy,
      management: config.management,
      sandbox: config.sandbox,
      oauth: { clientId: config.oauth?.clientId },
      policy: config.policy,
      folders: config.folders,
      limits: config.limits,
      tunnel: {
        enabled: config.tunnel?.enabled,
        publicUrl: config.tunnel?.publicUrl || config.publicBaseUrl,
      },
    },
    { id: snapshot.id, name: snapshot.name },
  );
  return canonicalToWorkspaceProfile(canonical);
}

export function runtimeStatusFromConfig(config: NodeSafeConfig): RuntimeStatus {
  const host = config.host === "0.0.0.0" ? "127.0.0.1" : config.host || "127.0.0.1";
  const port = config.port || DEFAULT_SERVICE_PORT;
  return {
    state: "running",
    pid: null,
    localMessage: "",
    publicMessage: "",
    localEndpoint: `http://${host}:${port}/mcp`,
    publicEndpoint: config.tunnel?.publicUrl || config.publicBaseUrl || "",
  };
}

export function overlayProfileOnConfig(
  saved: NodeSafeConfig,
  profile: WorkspaceProfile,
): NodeConfigUpdatePayload {
  const canonical = migrateDesktopProfile(profile);
  const folders = canonical.folders.map((folder) => ({
    id: folder.id,
    name: folder.name,
    path: folder.path,
  }));
  const commands = canonical.policy.allowedCommands.length
    ? canonical.policy.allowedCommands
    : saved.policy.allowedCommands;
  const extensions = canonical.policy.workspaceScriptExtensions.length
    ? canonical.policy.workspaceScriptExtensions
    : saved.policy.workspaceScriptExtensions;

  return {
    name: canonical.name,
    host: canonical.bind.host || saved.host,
    port: canonical.bind.port || saved.port,
    publicBaseUrl: saved.publicBaseUrl,
    dataDir: saved.dataDir,
    securityPolicy: Object.keys(canonical.securityPolicy).length
      ? canonical.securityPolicy
      : toNodePolicy(securityPolicyConfig(profile.runtime)),
    management: saved.management,
    sandbox: toNodeSandbox(sandboxConfig(profile.runtime)),
    oauth: {
      clientId: canonical.auth.oauthClientId || saved.oauth.clientId,
      tokenTtlSeconds: canonical.auth.oauthTokenTtlSeconds || saved.oauth.tokenTtlSeconds || 3650 * 24 * 60 * 60,
      password: "",
      clientSecret: "",
      clearClientSecret: false,
    },
    policy: {
      allowedCommands: commands,
      workspaceLocalEntries: canonical.policy.workspaceLocalEntries,
      workspaceScriptExtensions: extensions,
      maxPatchBytes: saved.policy.maxPatchBytes,
    },
    folders,
    limits: {
      blockingConcurrency: canonical.limits.blockingConcurrency || saved.limits.blockingConcurrency,
      processConcurrency: canonical.limits.processConcurrency || saved.limits.processConcurrency,
      globalBlockingConcurrency:
        canonical.limits.globalBlockingConcurrency || saved.limits.globalBlockingConcurrency,
      globalProcessConcurrency:
        canonical.limits.globalProcessConcurrency || saved.limits.globalProcessConcurrency,
      activeSessionLimit: canonical.limits.activeSessionLimit || saved.limits.activeSessionLimit,
      maxOutputBytes: saved.limits.maxOutputBytes,
      commandTimeoutMaxMs: saved.limits.commandTimeoutMaxMs,
    },
    tunnel: {
      enabled: canonical.tunnel.builtin.enabled || saved.tunnel.enabled,
      publicUrl: canonical.tunnel.builtin.publicUrl || saved.tunnel.publicUrl,
      enrollmentUrl: "",
      clearEnrollmentUrl: false,
    },
  };
}
