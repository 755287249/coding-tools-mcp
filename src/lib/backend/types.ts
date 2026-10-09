import type { RuntimeStatus, SandboxBackendDescriptor, WorkspaceProfile } from "../types";
import type { HealthItem } from "../api/health";
import type {
  HistorySessionDetail,
  HistorySessionsResult,
} from "../api/history";
import type { LogChunk, LogService } from "../api/logs";
import type { SharedSecretKey, WorkspaceSecretKey } from "../api/secrets";
import type { FrpProfileDto, FrpProfileInput, ProxyConfigDto } from "../api/settings";
import type { DownloadConfig, SoftwareStatus } from "../api/software";
import type { TelemetryResult } from "../api/telemetry";
import type { ActivityEvent, ActivitySnapshot } from "../api/activity";
import type { TunnelService, TunnelStatus, TunnelTestResult } from "../api/tunnel";
import type { FrontendCapabilities } from "./capabilities";

export type {
  BooleanCapability,
  FrontendCapabilities,
  FrontendHost,
} from "./capabilities";
export type { HealthItem } from "../api/health";
export type {
  HistoryRecord,
  HistorySessionDetail,
  HistorySessionSummary,
  HistorySessionsResult,
} from "../api/history";
export type { LogChunk, LogService } from "../api/logs";
export type { SharedSecretKey, WorkspaceSecretKey } from "../api/secrets";
export type { FrpProfileDto, FrpProfileInput, ProxyConfigDto } from "../api/settings";
export type { DownloadConfig, SoftwareStatus } from "../api/software";
export type {
  TelemetryAggregate,
  TelemetryRecord,
  TelemetryResult,
  TelemetryToolStat,
} from "../api/telemetry";
export type { TunnelService, TunnelStatus, TunnelTestResult } from "../api/tunnel";

export type DialogKind = "info" | "warning" | "error";

export interface PickDirectoryOptions {
  defaultPath?: string;
  title?: string;
  multiple?: boolean;
}

export interface ConfirmOptions {
  title?: string;
  kind?: DialogKind;
  okLabel?: string;
  cancelLabel?: string;
}

export interface AlertOptions {
  title?: string;
  kind?: DialogKind;
}

export interface NativeUi {
  pickDirectory(options?: PickDirectoryOptions): Promise<string | string[] | null>;
  confirm(message: string, options?: ConfirmOptions): Promise<boolean>;
  alert(message: string, options?: AlertOptions): Promise<void>;
}

export interface TelemetryQueryOptions {
  limit?: number;
  errorsOnly?: boolean;
  minDurationMs?: number;
  sinceTsMs?: number;
  scope?: "current_runtime" | "current_version" | "all";
  sortBy?: string;
}

export interface WorkspaceBackend {
  list(): Promise<WorkspaceProfile[]>;
  create(path: string, name?: string): Promise<WorkspaceProfile>;
  listWslDistributions(): Promise<string[]>;
  listSandboxBackends(): Promise<SandboxBackendDescriptor[]>;
  update(profile: WorkspaceProfile): Promise<void>;
  addFolder(id: string, path: string, name?: string): Promise<WorkspaceProfile>;
  addWslFolder(
    id: string,
    distro: string,
    linuxPath: string,
    name?: string,
  ): Promise<WorkspaceProfile>;
  removeFolder(id: string, folderId: string): Promise<WorkspaceProfile>;
  openDirectory(path: string): Promise<void>;
  delete(id: string): Promise<void>;
  startRuntime(id: string): Promise<RuntimeStatus>;
  stopRuntime(id: string): Promise<RuntimeStatus>;
  getRuntimeStatus(id: string): Promise<RuntimeStatus>;
  startActionsRuntime(id: string): Promise<RuntimeStatus>;
  stopActionsRuntime(id: string): Promise<RuntimeStatus>;
  getActionsRuntimeStatus(id: string): Promise<RuntimeStatus>;
  restartRuntime(id: string): Promise<RuntimeStatus>;
  restartActionsRuntime(id: string): Promise<RuntimeStatus>;
}

export interface SettingsBackend {
  listFrpProfiles(): Promise<FrpProfileDto[]>;
  saveFrpProfile(profile: FrpProfileInput, token?: string): Promise<FrpProfileDto>;
  deleteFrpProfile(id: string): Promise<void>;
  getLastWorkspaceId(): Promise<string>;
  setLastWorkspace(id: string): Promise<void>;
  getProxy(): Promise<ProxyConfigDto>;
  setProxy(proxy: ProxyConfigDto): Promise<void>;
}

export interface TelemetryBackend {
  query(
    workspaceId: string,
    options?: TelemetryQueryOptions,
    signal?: AbortSignal,
  ): Promise<TelemetryResult>;
}

export interface ActivityBackend {
  read(workspaceId: string, sinceRev?: number): Promise<ActivitySnapshot>;
  detail(workspaceId: string, seq: number): Promise<ActivityEvent | null>;
  clear(workspaceId: string): Promise<void>;
}

export interface HistoryBackend {
  list(
    workspaceId: string,
    folderId?: string,
    signal?: AbortSignal,
  ): Promise<HistorySessionsResult>;
  read(
    workspaceId: string,
    number: number,
    folderId?: string,
    signal?: AbortSignal,
  ): Promise<HistorySessionDetail>;
}

export interface HealthBackend {
  run(workspaceId: string, signal?: AbortSignal): Promise<HealthItem[]>;
}

export interface LogsBackend {
  readRaw(workspaceId: string, service: LogService): Promise<LogChunk[]>;
}

export interface SecretsBackend {
  getWorkspaceSecret(id: string, key: WorkspaceSecretKey): Promise<string | null>;
  setWorkspaceSecret(id: string, key: WorkspaceSecretKey, value: string): Promise<void>;
  regenerateWorkspaceSecret(id: string, key: WorkspaceSecretKey): Promise<string>;
  getSharedSecret(key: SharedSecretKey): Promise<string | null>;
  setSharedSecret(key: SharedSecretKey, value: string): Promise<void>;
  regenerateSharedSecret(key: SharedSecretKey): Promise<string>;
}

export interface SoftwareBackend {
  list(): Promise<SoftwareStatus[]>;
  install(kind: string): Promise<SoftwareStatus>;
  uninstall(kind: string): Promise<SoftwareStatus>;
  getDownloadConfig(): Promise<DownloadConfig>;
  setDownloadConfig(config: DownloadConfig): Promise<void>;
}

export interface TunnelBackend {
  getFrpSnippet(id: string, service: TunnelService): Promise<string>;
  start(id: string, service: TunnelService): Promise<TunnelStatus>;
  stop(id: string, service: TunnelService): Promise<TunnelStatus>;
  test(id: string, service: TunnelService): Promise<TunnelTestResult>;
  restart(id: string, service: TunnelService): Promise<TunnelStatus>;
}

export interface AgentRestartResult {
  ok: true;
  restarting: true;
}

export interface DirectoryBrowseResult {
  ok: true;
  path: string;
  parent: string | null;
  roots: string[];
  directories: Array<{ name: string; path: string }>;
  totalDirectories: number;
  truncated: boolean;
}

export interface OperationLogQuery {
  folderId: string;
  status: "all" | "completed" | "failed" | "incomplete";
  tool: string;
  errorsOnly: boolean;
  limit: number;
}

export type SkillSource = "project" | "agents" | "claude" | "codex-user" | "claude-user";
export type SkillScope = "workspace" | "user";

export interface SkillInventoryItem {
  key: string;
  name: string;
  description: string;
  source: SkillSource;
  scope: SkillScope;
  relativePath: string;
  rootRelativePath: string;
  version: string | null;
  selected: boolean;
  enabled: boolean;
  folderId: string | null;
  folderName: string | null;
}

export interface SkillInventoryPayload {
  ok: true;
  workspaceId: string;
  active: boolean;
  skills: SkillInventoryItem[];
  diagnostics: Array<{
    code: string;
    message: string;
    path?: string;
    name?: string;
    source?: SkillSource;
    scope?: SkillScope;
  }>;
}

export interface SkillToggleResult {
  ok: true;
  workspaceId: string;
  skillKey: string;
  enabled: boolean;
  restartRequired: boolean;
  appliedImmediately?: string[];
}

export interface SkillMasterToggleResult {
  ok: true;
  workspaceId: string;
  active: boolean;
  restartRequired: boolean;
  appliedImmediately?: string[];
}

export type ExtensionProvider = "codex" | "claude";
export type ExtensionScope = "workspace" | "local" | "user";
export type ExtensionKind = "hook" | "mcp";

export interface HookInventoryItem {
  key: string;
  provider: ExtensionProvider;
  scope: ExtensionScope;
  folderId: string | null;
  event: string;
  matcher: string | null;
  handlerType: string;
  sourcePath: string;
  sourceEnabled: boolean;
  supported: boolean;
  selected: boolean;
  enabled: boolean;
  command: string | null;
  endpoint: string | null;
}

export interface McpServerInventoryItem {
  key: string;
  provider: ExtensionProvider;
  scope: ExtensionScope;
  folderId: string | null;
  name: string;
  transport: string;
  sourcePath: string;
  sourceEnabled: boolean;
  supported: boolean;
  selected: boolean;
  enabled: boolean;
  connected: boolean;
  toolCount: number;
  command: string | null;
  endpoint: string | null;
  error: string | null;
}

export interface ExtensionInventoryPayload {
  ok: true;
  workspaceId: string;
  hooksActive: boolean;
  mcpActive: boolean;
  hooks: HookInventoryItem[];
  mcpServers: McpServerInventoryItem[];
  diagnostics: Array<{
    code: string;
    message: string;
    provider?: ExtensionProvider;
    scope?: ExtensionScope;
    path?: string;
    key?: string;
  }>;
}

export interface ExtensionToggleResult {
  ok: true;
  workspaceId: string;
  extensionKind: ExtensionKind;
  extensionKey: string;
  enabled: boolean;
  restartRequired: boolean;
  appliedImmediately?: string[];
}

export interface ExtensionMasterToggleResult {
  ok: true;
  workspaceId: string;
  extensionKind: ExtensionKind;
  active: boolean;
  restartRequired: boolean;
  appliedImmediately?: string[];
}

export interface WorkspaceFeatureBackend {
  skills(workspaceId: string, signal?: AbortSignal): Promise<SkillInventoryPayload>;
  setSkillsActive(workspaceId: string, active: boolean): Promise<SkillMasterToggleResult>;
  setSkillEnabled(workspaceId: string, key: string, enabled: boolean): Promise<SkillToggleResult>;
  extensions(workspaceId: string, signal?: AbortSignal): Promise<ExtensionInventoryPayload>;
  setExtensionActive(
    workspaceId: string,
    kind: ExtensionKind,
    active: boolean,
  ): Promise<ExtensionMasterToggleResult>;
  setExtensionEnabled(
    workspaceId: string,
    kind: ExtensionKind,
    key: string,
    enabled: boolean,
  ): Promise<ExtensionToggleResult>;
}

export interface AgentBackend {
  restart(): Promise<AgentRestartResult>;
  status(signal?: AbortSignal): Promise<unknown>;
  loadConfig(signal?: AbortSignal): Promise<unknown>;
  saveConfig(workspaceId: string, payload: unknown): Promise<unknown>;
}

export interface DirectoryBackend {
  browse(
    path?: string,
    workspaceId?: string,
    signal?: AbortSignal,
  ): Promise<DirectoryBrowseResult>;
}

export interface OperationLogPayload {
  ok: true;
  generatedAt: number;
  folder: { id: string; name: string };
  source: "operation_log";
  cursor: number;
  limit: number;
  nextCursor: number | null;
  matched: number;
  summary: {
    total: number;
    completed: number;
    failed: number;
    incomplete: number;
  };
  operations: Array<{
    id: string;
    tool: string;
    status: Exclude<OperationLogQuery["status"], "all">;
    startedAt: number | null;
    finishedAt: number | null;
    durationMs: number | null;
    taskTracked: boolean;
    affectedFileCount: number;
    reason?: string;
    diagnostics: Record<string, unknown>;
    events: Array<{ kind: string; createdAt: number; ok: boolean }>;
  }>;
}

export interface OperationsBackend {
  query(
    workspaceId: string,
    filters: OperationLogQuery,
    cursor?: number,
    signal?: AbortSignal,
  ): Promise<OperationLogPayload>;
}

export interface ChatBackend {
  request(workspaceId: string, folderId: string, args: import("../api/chat").ChatAction): Promise<import("../api/chat").ChatResult>;
}

export interface FrontendBackend {
  readonly chat: ChatBackend;
  readonly capabilities: FrontendCapabilities;
  readonly native: NativeUi;
  readonly workspaces: WorkspaceBackend;
  readonly settings: SettingsBackend;
  readonly telemetry: TelemetryBackend;
  readonly activity: ActivityBackend;
  readonly history: HistoryBackend;
  readonly health: HealthBackend;
  readonly logs: LogsBackend;
  readonly secrets: SecretsBackend;
  readonly software: SoftwareBackend;
  readonly tunnel: TunnelBackend;
  readonly agent: AgentBackend;
  readonly directories: DirectoryBackend;
  readonly operations: OperationsBackend;
  readonly workspaceFeatures: WorkspaceFeatureBackend;
}

export type InvokeFn = <T>(cmd: string, args?: Record<string, unknown>) => Promise<T>;
