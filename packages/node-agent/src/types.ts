import type { ChildProcessWithoutNullStreams } from 'node:child_process';
import type { EventEmitter } from 'node:events';
import type { KeyedMutex, Semaphore } from './runtime.js';
import type { SkillRegistry } from './skills/registry.js';
import type { EvolvedSkillRegistry, ToolEvolutionRegistry, ToolStrategyRegistry } from './knowledge/registries.js';
import type { KnowledgeIngestor } from './knowledge/ingestor.js';
import type { KnowledgeImpactStore } from './knowledge/impact.js';
import type { KnowledgeStore } from './knowledge/store.js';
import type { CanaryImpactStore, CanaryStrategyEngine } from './knowledge/canary.js';
import type { EvolvedSkillCanaryEngine } from './knowledge/evolvedSkillCanary.js';
import type {
  ToolEvolutionCandidateClaimer,
  ToolEvolutionExperimentStore,
  ToolEvolutionPlanner,
  ToolEvolutionProposalStore
} from './knowledge/toolEvolution.js';
import type { ToolEvolutionBenchmarkCollector } from './knowledge/toolEvolutionCollector.js';
import type { ExtensionRegistry } from './extensions/registry.js';
import type { OperationRecord, StateStoreContract } from './state/contract.js';
import type { ToolUsageStoreContract } from './toolUsage/contract.js';
import type { WorkerPolicy } from './tunnelPolicy.js';
import type { StartupDiagnostics } from './processStartup.js';
import type { ProcessTimeoutContract } from './processes/timeoutPolicy.js';
import type { ConversationStoreContract, MutableStringMap } from './conversation/contract.js';

export type {
  ChangeSet,
  OperationRecord,
  PersistentState,
  ProjectBaseline,
  TaskEvent,
  TaskRecord,
  TaskStatus
} from './state/contract.js';

export type JsonObject = Record<string, unknown>;
export type PermissionMode = 'read-only' | 'guarded' | 'trusted' | 'dangerous';
export type ToolProfile = 'advanced' | 'read-only' | 'compat-readonly-all' | 'guarded-core' | 'trusted-core';
export type ToolProfileSetting = ToolProfile | 'core';

export interface SecurityPolicy {
  restrictToolCatalog: boolean;
  enforceCommandAllowlist: boolean;
  requireDangerousConfirmation: boolean;
  requireShellConfirmation: boolean;
  blockNetworkCommands: boolean;
  enforceWorkspaceBoundary: boolean;
  protectRepositoryMetadata: boolean;
  blockSymlinkEscape: boolean;
  protectEnvironmentVariables: boolean;
  enforceHarnessBaseline: boolean;
  requireWriteConfirmation: boolean;
  verifyWriteConflicts: boolean;
  enforceResourceLimits: boolean;
  redactSensitiveOutput: boolean;
  withholdSensitiveSourceOutput: boolean;
  redactTelemetry: boolean;
  redactHistory: boolean;
}

export interface WorkspaceFolder {
  id: string;
  name: string;
  path: string;
}

export interface WorkspaceFolderDocument {
  id?: string;
  name?: string;
  path: string;
}

export type SandboxPathAccess = 'read_only' | 'modify';

export interface SandboxPathGrant {
  path: string;
  access: SandboxPathAccess;
}

export interface SandboxConfig {
  enabled: boolean;
  backend: string;
  externalPaths: SandboxPathGrant[];
  options: Record<string, string>;
}

export interface EditProposalRecord {
  path: string;
  fileSha256: string;
  start: number;
  end: number;
  actualText: string;
  replacement: string;
  createdAt: number;
}

export interface AgentConfig {
  workspaceId?: string;
  workspaceName?: string;
  host: string;
  port: number;
  publicBaseUrl?: string;
  dataDir: string;
  permissionMode: PermissionMode;
  toolProfile: ToolProfileSetting;
  activeToolProfile: ToolProfile;
  securityPolicy: SecurityPolicy;
  securityPolicyCustomized: boolean;
  policy: {
    allowedCommands: string[];
    workspaceLocalEntries: boolean;
    workspaceScriptExtensions: string[];
    maxPatchBytes: number;
  };
  management: {
    enabled: boolean;
  };
  skills: {
    active: boolean;
    disabled: string[];
  };
  extensions: {
    hooks: { active: boolean; enabled: string[] };
    mcp: { active: boolean; enabled: string[] };
  };
  sandbox: SandboxConfig;
  oauth: {
    clientId: string;
    clientSecret?: string;
    password: string;
    tokenSecret: string;
    tokenTtlSeconds?: number;
  };
  folders: WorkspaceFolder[];
  limits: {
    blockingConcurrency: number;
    processConcurrency: number;
    globalBlockingConcurrency: number;
    globalProcessConcurrency: number;
    activeSessionLimit: number;
    maxOutputBytes: number;
    commandTimeoutMaxMs: number;
  };
  tunnel?: {
    enabled: boolean;
    publicUrl: string;
    enrollmentUrl?: string;
    stateFile: string;
  };
}

export interface AgentConfigDocument {
  schema_version: 1;
  host?: string;
  port?: number;
  publicBaseUrl?: string;
  dataDir?: string;
  permissionMode?: PermissionMode;
  toolProfile?: ToolProfileSetting | string;
  securityPolicy?: Partial<SecurityPolicy>;
  policy?: {
    allowedCommands?: string[] | string;
    workspaceLocalEntries?: boolean;
    workspaceScriptExtensions?: string[] | string;
    maxPatchBytes?: number;
  };
  management?: Partial<AgentConfig['management']>;
  skills?: {
    active?: boolean;
    disabled?: string[];
  };
  extensions?: {
    hooks?: { active?: boolean; enabled?: string[] };
    mcp?: { active?: boolean; enabled?: string[] };
  };
  sandbox?: {
    enabled?: boolean;
    backend?: string;
    externalPaths?: SandboxPathGrant[];
    options?: Record<string, string>;
  };
  oauth?: {
    clientId?: string;
    tokenTtlSeconds?: number;
  };
  folders?: WorkspaceFolderDocument[];
  limits?: Partial<AgentConfig['limits']>;
  tunnel?: {
    enabled?: boolean;
    publicUrl?: string;
    stateFile?: string;
  };
}

export interface WorkspaceRegistryEntry {
  id: string;
  name: string;
  configPath: string;
}

export interface WorkspaceRegistryDocument {
  schema_version: 1;
  workspaces: WorkspaceRegistryEntry[];
}

export interface AgentSecrets {
  oauthPassword?: string;
  oauthClientSecret?: string;
  oauthTokenSecret?: string;
  tunnelEnrollmentUrl?: string;
  named?: Record<string, string>;
}

export interface ToolDefinition {
  name: string;
  title: string;
  description: string;
  inputSchema: JsonObject;
  annotations: {
    title: string;
    readOnlyHint: boolean;
    destructiveHint: boolean;
    idempotentHint: boolean;
    openWorldHint: boolean;
  };
}

export interface PendingOperation {
  resumeId: string;
  name: string;
  args: JsonObject;
  meta: unknown;
  permission: string;
  reason: string;
  folderId: string;
  workspacePath: string;
  defaultCwd: string;
  createdAt: number;
  expiresAt: number;
}

export interface FolderRuntime {
  folderId: string;
  workspacePath: string;
  sessions: Map<string, ProcessSession>;
  operationsByFingerprint: Map<string, string>;
  pendingOperations: Map<string, PendingOperation>;
  editProposals: Map<string, EditProposalRecord>;
  skillRegistry: SkillRegistry;
  knowledgeStore: KnowledgeStore;
  knowledgeImpactStore: KnowledgeImpactStore;
  knowledgeIngestor: KnowledgeIngestor;
  canaryImpactStore: CanaryImpactStore;
  canaryStrategyEngine: CanaryStrategyEngine;
  evolvedSkillCanaryEngine: EvolvedSkillCanaryEngine;
  toolEvolutionExperimentStore: ToolEvolutionExperimentStore;
  toolEvolutionProposalStore: ToolEvolutionProposalStore;
  toolEvolutionCandidateClaimer: ToolEvolutionCandidateClaimer;
  toolEvolutionPlanner: ToolEvolutionPlanner;
  toolEvolutionBenchmarkCollector: ToolEvolutionBenchmarkCollector;
  toolStrategyRegistry: ToolStrategyRegistry;
  toolEvolutionRegistry: ToolEvolutionRegistry;
  evolvedSkillRegistry: EvolvedSkillRegistry;
  admission: { blocking: Semaphore; process: Semaphore; locks: KeyedMutex };
}

export interface ToolContext {
  config: AgentConfig;
  conversations: ConversationStoreContract;
  workspaceProfileId: string;
  selections: MutableStringMap;
  defaultCwds: MutableStringMap;
  folderRuntimes: Map<string, FolderRuntime>;
  extensions: ExtensionRegistry;
  hubAdmission: { blocking: Semaphore; process: Semaphore; locks: KeyedMutex };
  usage: UsageRecord[];
  usageStore: ToolUsageStoreContract;
  state: StateStoreContract;
  resolveSecret?: (name: string) => string | undefined;
  tunnelStatus?: TunnelStatus;
}

export interface ProcessOutputEvent {
  sequence: number;
  stream: 'stdout' | 'stderr';
  offset: number;
  data: string;
}

export interface ProcessSession {
  id: string;
  folderId: string;
  workspacePath: string;
  operationId?: string;
  fingerprint: string;
  command: string;
  program: string;
  argv: string[];
  shell: boolean;
  cwd: string;
  startupDiagnostics: StartupDiagnostics;
  startedAt: number;
  timeoutContract?: ProcessTimeoutContract;
  processDeadlineMs?: number;
  firstOutputAt?: number;
  endedAt?: number;
  finalizedAt?: number;
  exitCode?: number | null;
  signal?: string | null;
  stdout: string;
  stderr: string;
  stdoutBytes: number;
  stderrBytes: number;
  stdoutStart: number;
  stderrStart: number;
  sequence: number;
  outputEvents: ProcessOutputEvent[];
  outputEventBytes: number;
  child?: ChildProcessWithoutNullStreams;
  processId?: number;
  durableDirectory?: string;
  durableWorkerPid?: number;
  durableMonitor?: NodeJS.Timeout;
  interactive: boolean;
  stdinOpen: boolean;
  timedOut: boolean;
  killed: boolean;
  sandboxEnforced?: boolean;
  sandboxBackend?: string;
  executionBoundary?: string;
  sandboxPrepareMs?: number;
  sandboxStartupMs?: number;
  sandboxCleanupMs?: number;
  processTreeContained?: boolean;
  processTreeControl?: string;
  backendKill?: () => Promise<void>;
  terminationReason?: string;
  telemetryCommandKind: string;
  testRunnerCapability?: JsonObject;
  testWorkflow?: JsonObject;
  telemetryRecorded?: boolean;
  harnessOperations?: Map<string, OperationRecord>;
  harnessOperationRecordedIds?: Set<string>;
  sensitiveOutput: boolean;
  postChecks: JsonObject[];
  postChecksPending: boolean;
  verificationOk?: boolean;
  resourceLockGroup?: string;
  resourceLockTarget?: string;
  operationLockWaitMs: number;
  resourceLockWaitMs: number;
  lockRelease?: () => void;
  timeoutTimer?: NodeJS.Timeout;
  attachmentGeneration: number;
  detachedGeneration: number;
  detachedTimer?: NodeJS.Timeout;
  events: EventEmitter;
}

export interface UsageRecord {
  tool: string;
  startedAt: number;
  durationMs: number;
  ok: boolean;
  queueWaitMs: number;
  lockWaitMs: number;
  responseBytes: number;
}

export interface TunnelStatus {
  enabled: boolean;
  state: 'disabled' | 'starting' | 'running' | 'reconnecting' | 'stopped' | 'error';
  publicUrl?: string;
  workers: number;
  connectedWorkers: number;
  connectingWorkers?: number;
  idleWorkers?: number;
  busyWorkers?: number;
  recycledWorkers?: number;
  completedRequests: number;
  policyRevision?: number;
  workerPolicy?: WorkerPolicy;
  lastError?: string;
  lastRequestTimeout?: 'connect' | 'overall';
  lastRequestTimeoutAt?: number;
  startedAt?: number;
  updateId?: string;
  updateTargetVersion?: string;
  updateState?: 'idle' | 'downloading' | 'verified' | 'scheduled' | 'failed';
  updateError?: string;
}
