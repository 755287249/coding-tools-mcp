export type RuntimeState = "stopped" | "starting" | "running" | "stopping" | "error";

export const DEFAULT_SERVICE_PORT = 28766;
export const DEFAULT_ACTIONS_PORT = 8787;

export interface TunnelConfig {
  type: string;
  public_url: string;
  frp_server: string;
  frp_subdomain: string;
  frp_profile_id?: string;
  frp_server_port?: number;
  cloudflare_mode: string;
  use_proxy?: boolean;
}

export interface AuthConfig {
  type: string;
  oauth_client_id: string;
  oauth_token_ttl_seconds?: number;
  use_shared_secrets?: boolean;
}

export type SandboxPathAccess = "read_only" | "modify";

export interface SandboxPathGrant {
  path: string;
  access: SandboxPathAccess;
}

export interface SandboxConfig {
  enabled: boolean;
  backend: string;
  external_paths: SandboxPathGrant[];
  options: Record<string, string>;
}

export interface SandboxBackendOptionDescriptor {
  id: string;
  label: string;
  description: string;
  placeholder: string;
  defaultValue: string;
  required: boolean;
}

export interface SandboxBackendDescriptor {
  id: string;
  label: string;
  description: string;
  hostSupported: boolean;
  supportsWsl: boolean;
  enforcementReady: boolean;
  experimental: boolean;
  options: SandboxBackendOptionDescriptor[];
}

export interface SecurityPolicy {
  restrict_tool_catalog: boolean;
  enforce_command_allowlist: boolean;
  require_dangerous_confirmation: boolean;
  require_shell_confirmation: boolean;
  block_network_commands: boolean;
  enforce_workspace_boundary: boolean;
  protect_repository_metadata: boolean;
  block_symlink_escape: boolean;
  protect_environment_variables: boolean;
  enforce_harness_baseline: boolean;
  require_write_confirmation: boolean;
  verify_write_conflicts: boolean;
  enforce_resource_limits: boolean;
  redact_sensitive_output: boolean;
  withhold_sensitive_source_output: boolean;
  redact_telemetry: boolean;
  redact_history: boolean;
}

export function securityPolicyConfig(runtime: RuntimeConfig): SecurityPolicy {
  return {
    restrict_tool_catalog: !["advanced", "compat-readonly-all"].includes(runtime.tool_profile),
    enforce_command_allowlist: true,
    require_dangerous_confirmation: runtime.permission_mode !== "dangerous",
    require_shell_confirmation: runtime.permission_mode !== "dangerous",
    block_network_commands: !["trusted", "dangerous"].includes(runtime.permission_mode),
    enforce_workspace_boundary: true,
    protect_repository_metadata: true,
    block_symlink_escape: true,
    protect_environment_variables: true,
    enforce_harness_baseline: true,
    require_write_confirmation: true,
    verify_write_conflicts: true,
    enforce_resource_limits: true,
    redact_sensitive_output: true,
    withhold_sensitive_source_output: true,
    redact_telemetry: true,
    redact_history: true,
    ...runtime.security_policy,
  };
}

export function compatibilityPermissionMode(policy: SecurityPolicy): string {
  if (!policy.require_dangerous_confirmation && !policy.require_shell_confirmation && !policy.require_write_confirmation) {
    return "dangerous";
  }
  return policy.block_network_commands ? "safe" : "trusted";
}

export function compatibilityToolProfile(policy: SecurityPolicy): string {
  return policy.restrict_tool_catalog ? "trusted-core" : "advanced";
}

export interface RuntimeConfig {
  local_port: number;
  bind_address?: string;
  transport_mode?: "streamable-http" | "legacy-json";
  tool_profile: string;
  permission_mode: string;
  security_policy?: SecurityPolicy;
  runtime_command?: string;
  allowed_commands?: string;
  workspace_local_entries?: boolean;
  workspace_script_extensions?: string;
  blocking_admission_limit?: number;
  process_admission_limit?: number;
  global_blocking_admission_limit?: number;
  global_process_admission_limit?: number;
  sandbox?: SandboxConfig;
  active_session_limit?: number;
}

export function sandboxConfig(runtime: RuntimeConfig): SandboxConfig {
  return {
    enabled: false,
    backend: "appcontainer",
    external_paths: [],
    options: {},
    ...runtime.sandbox,
  };
}

export interface ActionsConfig {
  public_url: string;
  tunnel_type: string;
  frp_server: string;
  frp_subdomain: string;
  frp_profile_id?: string;
  frp_server_port?: number;
  cloudflare_mode: string;
  cloudflare_token?: string;
  use_proxy?: boolean;
  local_port: number;
  bind_address?: string;
  permission_mode: string;
  runtime_command?: string;
  auth_type: string;
  oauth_client_id?: string;
  oauth_scopes?: string;
  allowed_commands?: string;
  max_patch_bytes?: number;
  use_shared_secrets?: boolean;
}

export interface WorkspaceFolder {
  id: string;
  name: string;
  path: string;
  execution?: WorkspaceExecutionTarget;
}

export type WorkspaceExecutionTarget =
  | { kind: "host" }
  | { kind: "wsl"; distro: string; linux_path: string };

export interface WorkspaceProfile {
  id: string;
  name: string;
  path: string;
  folders?: WorkspaceFolder[];
  active_folder_id?: string;
  tunnel: TunnelConfig;
  auth: AuthConfig;
  runtime: RuntimeConfig;
  actions?: ActionsConfig;
}

export function workspaceFolders(profile: WorkspaceProfile): WorkspaceFolder[] {
  if (profile.folders?.length) return profile.folders;
  return [
    {
      id: `legacy-${profile.id}`,
      name: profile.path.replace(/\\/g, "/").split("/").filter(Boolean).at(-1) ?? "workspace",
      path: profile.path,
    },
  ];
}

export function activeWorkspaceFolder(profile: WorkspaceProfile): WorkspaceFolder {
  const folders = workspaceFolders(profile);
  return folders.find((folder) => folder.id === profile.active_folder_id) ?? folders[0];
}

export interface RuntimeStatus {
  state: RuntimeState;
  pid: number | null;
  localMessage: string;
  publicMessage: string;
  localEndpoint: string;
  publicEndpoint: string;
}

export function actionsConfig(profile: WorkspaceProfile): ActionsConfig {
  return {
    public_url: "",
    tunnel_type: "frp",
    frp_server: "",
    frp_subdomain: "",
    cloudflare_mode: "quick",
    local_port: DEFAULT_ACTIONS_PORT,
    bind_address: "127.0.0.1",
    permission_mode: "trusted",
    auth_type: "api_key",
    allowed_commands:
      "pytest,python,python3,npm,npx,node,pnpm,yarn,make,mvn,mvnw,gradle,gradlew,cargo,go,ruff,mypy,eslint,tsc",
    max_patch_bytes: 200_000,
    ...profile.actions,
  };
}

function connectHostForBindAddress(bindAddress = "127.0.0.1"): string {
  const address = bindAddress.trim() || "127.0.0.1";
  if (address === "0.0.0.0") return "127.0.0.1";
  if (address === "::") return "[::1]";
  return address.includes(":") ? `[${address}]` : address;
}

export function mcpLocalEndpoint(port: number, bindAddress = "127.0.0.1"): string {
  return `http://${connectHostForBindAddress(bindAddress)}:${port}/mcp`;
}

export function actionsLocalEndpoint(port: number, bindAddress = "127.0.0.1"): string {
  return `http://${connectHostForBindAddress(bindAddress)}:${port}`;
}

export interface ActionsAuthDraft {
  authType: string;
  oauthClientId: string;
  oauthScopes: string;
  useSharedSecrets?: boolean;
}

export interface FrpProfileSummary {
  id: string;
  name: string;
  server: string;
  serverPort: number;
}

export function frpPublicUrl(
  tunnelType: string,
  frpSubdomain: string,
  frpServer: string,
  frpProfileId: string | undefined,
  profiles: FrpProfileSummary[],
  publicUrl = "",
): string {
  if (tunnelType !== "frp" || !frpSubdomain) {
    return publicUrl.replace(/\/$/, "");
  }
  const server =
    profiles.find((profile) => profile.id === frpProfileId)?.server ?? frpServer;
  if (!server) return publicUrl.replace(/\/$/, "");
  return `https://${frpSubdomain}.${server}`;
}

export function actionsPublicBaseUrl(
  profile: WorkspaceProfile,
  frpProfiles: FrpProfileSummary[] = [],
): string {
  const actions = actionsConfig(profile);
  const publicUrl = frpPublicUrl(
    actions.tunnel_type,
    actions.frp_subdomain,
    actions.frp_server,
    actions.frp_profile_id,
    frpProfiles,
    actions.public_url,
  );
  if (publicUrl) return publicUrl;
  return actionsLocalEndpoint(actions.local_port, actions.bind_address);
}

export function actionsOpenApiUrl(
  profile: WorkspaceProfile,
  frpProfiles: FrpProfileSummary[] = [],
): string {
  const base = actionsPublicBaseUrl(profile, frpProfiles);
  return base ? `${base.replace(/\/$/, "")}/openapi.json` : "";
}

export function actionsPrivacyUrl(
  profile: WorkspaceProfile,
  frpProfiles: FrpProfileSummary[] = [],
): string {
  const base = actionsPublicBaseUrl(profile, frpProfiles);
  return base ? `${base.replace(/\/$/, "")}/privacy` : "";
}

export function actionsOAuthAuthorizeUrl(
  profile: WorkspaceProfile,
  frpProfiles: FrpProfileSummary[] = [],
): string {
  const base = actionsPublicBaseUrl(profile, frpProfiles);
  return base ? `${base.replace(/\/$/, "")}/oauth/authorize` : "";
}

export function actionsOAuthTokenUrl(
  profile: WorkspaceProfile,
  frpProfiles: FrpProfileSummary[] = [],
): string {
  const base = actionsPublicBaseUrl(profile, frpProfiles);
  return base ? `${base.replace(/\/$/, "")}/oauth/token` : "";
}
