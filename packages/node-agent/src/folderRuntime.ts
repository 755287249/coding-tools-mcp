import { createHash } from 'node:crypto';
import path from 'node:path';
import { deriveWorkspaceProfileId } from './conversation.js';
import { currentExecutionBinding } from './executionScope.js';
import { KeyedMutex, Semaphore } from './runtime.js';
import { EvolvedSkillRegistry, ToolEvolutionRegistry, ToolStrategyRegistry } from './knowledge/registries.js';
import { CanaryImpactStore, CanaryStrategyEngine } from './knowledge/canary.js';
import { EvolvedSkillCanaryEngine } from './knowledge/evolvedSkillCanary.js';
import { KnowledgeIngestor } from './knowledge/ingestor.js';
import { KnowledgeImpactStore } from './knowledge/impact.js';
import { KnowledgeStore } from './knowledge/store.js';
import {
  ToolEvolutionCandidateClaimer,
  ToolEvolutionExperimentStore,
  ToolEvolutionPlanner,
  ToolEvolutionProposalStore,
  type ToolEvolutionRuntimeSource
} from './knowledge/toolEvolution.js';
import { ToolEvolutionBenchmarkCollector } from './knowledge/toolEvolutionCollector.js';
import { SkillRegistry } from './skills/registry.js';
import type { AgentConfig, FolderRuntime, PendingOperation, ToolContext, WorkspaceFolder } from './types.js';
import { BUILD_GIT_SHA, BUILD_SOURCE_CLEAN, BUILD_TOOL_EVOLUTION_PROPOSAL_IDS } from './version.js';

function knowledgeRuntime(config: AgentConfig, folder: WorkspaceFolder) {
  const workspaceKey = createHash('sha256').update(folder.path).digest('hex').slice(0, 24);
  const store = new KnowledgeStore(path.join(config.dataDir, 'knowledge', workspaceKey));
  const impactStore = new KnowledgeImpactStore(store.rootDir);
  const canaryImpactStore = new CanaryImpactStore(store.rootDir);
  const canaryStrategyEngine = new CanaryStrategyEngine(store, canaryImpactStore);
  const evolvedSkillCanaryEngine = new EvolvedSkillCanaryEngine(store, canaryImpactStore);
  const toolEvolutionExperimentStore = new ToolEvolutionExperimentStore(store.rootDir);
  const toolEvolutionProposalStore = new ToolEvolutionProposalStore(store.rootDir);
  const toolEvolutionSource: ToolEvolutionRuntimeSource = {
    revision: BUILD_GIT_SHA,
    trusted: BUILD_SOURCE_CLEAN === true && /^[0-9a-f]{40}$/.test(BUILD_GIT_SHA),
    proposalIds: BUILD_TOOL_EVOLUTION_PROPOSAL_IDS
  };
  const toolEvolutionCandidateClaimer = new ToolEvolutionCandidateClaimer(
    store,
    toolEvolutionProposalStore,
    toolEvolutionExperimentStore,
    toolEvolutionSource
  );
  const toolEvolutionPlanner = new ToolEvolutionPlanner(
    store,
    toolEvolutionProposalStore,
    toolEvolutionSource,
    toolEvolutionExperimentStore
  );
  const toolEvolutionBenchmarkCollector = new ToolEvolutionBenchmarkCollector(toolEvolutionExperimentStore, toolEvolutionSource);
  return {
    knowledgeStore: store,
    knowledgeImpactStore: impactStore,
    canaryImpactStore,
    canaryStrategyEngine,
    evolvedSkillCanaryEngine,
    toolEvolutionExperimentStore,
    toolEvolutionProposalStore,
    toolEvolutionCandidateClaimer,
    toolEvolutionPlanner,
    toolEvolutionBenchmarkCollector,
    knowledgeIngestor: new KnowledgeIngestor(store, impactStore, folder.id, undefined, {
      impactStore: canaryImpactStore,
      strategyEngine: canaryStrategyEngine
    }, toolEvolutionPlanner),
    toolStrategyRegistry: new ToolStrategyRegistry(store),
    toolEvolutionRegistry: new ToolEvolutionRegistry(store, toolEvolutionExperimentStore, toolEvolutionProposalStore),
    evolvedSkillRegistry: new EvolvedSkillRegistry(store)
  };
}

function skillRegistry(config: AgentConfig, folder: WorkspaceFolder, evolvedSkillRegistry: EvolvedSkillRegistry): SkillRegistry {
  return new SkillRegistry(folder.path, {
    workspaceKey: folder.id,
    active: config.skills?.active ?? true,
    disabledSkillKeys: config.skills?.disabled ?? [],
    evolvedSkillProvider: async () => (await evolvedSkillRegistry.snapshot()).skills
  });
}

export function createFolderRuntime(config: AgentConfig, folder: WorkspaceFolder): FolderRuntime {
  const knowledge = knowledgeRuntime(config, folder);
  return {
    folderId: folder.id,
    workspacePath: folder.path,
    sessions: new Map(),
    operationsByFingerprint: new Map(),
    pendingOperations: new Map(),
    editProposals: new Map(),
    skillRegistry: skillRegistry(config, folder, knowledge.evolvedSkillRegistry),
    ...knowledge,
    admission: {
      blocking: new Semaphore(config.limits.blockingConcurrency),
      process: new Semaphore(config.limits.processConcurrency),
      locks: new KeyedMutex()
    }
  };
}

export type ToolEvolutionLifecycleEvent = 'tool_evolution_startup' | 'tool_evolution_workspace_hot_apply';

export async function syncToolEvolutionRuntimes(
  runtimes: readonly FolderRuntime[],
  usageStore: ToolContext['usageStore'],
  event: ToolEvolutionLifecycleEvent
): Promise<void> {
  await Promise.all(runtimes.map(async runtime => {
    try {
      const timestampMs = Date.now();
      const claims = await runtime.toolEvolutionCandidateClaimer.sync(timestampMs);
      const proposals = await runtime.toolEvolutionPlanner.sync(timestampMs);
      if (claims.considered || claims.failed || proposals.planned || proposals.superseded) {
        usageStore.recordDiagnosticEvent({
          eventType: 'lifecycle_event',
          event,
          fields: {
            workspace_folder_id: runtime.folderId,
            claims_considered: claims.considered,
            claims_materialized: claims.materialized,
            claims_replayed: claims.replayed,
            claims_ignored: claims.ignored,
            claims_failed: claims.failed,
            proposals_planned: proposals.planned,
            proposals_superseded: proposals.superseded
          }
        });
      }
    } catch {
      usageStore.recordDiagnosticEvent({
        eventType: 'lifecycle_event',
        event: `${event}_failed`,
        fields: { workspace_folder_id: runtime.folderId, failure: true }
      });
    }
  }));
}

export interface WorkspaceFolderHotApplyResult {
  changed: boolean;
  applied: boolean;
  deferredReason?: string;
}

function folderRuntimeBusy(runtime: FolderRuntime): boolean {
  return [...runtime.sessions.values()].some(session => !session.finalizedAt)
    || runtime.pendingOperations.size > 0
    || runtime.editProposals.size > 0;
}

function sameFolderConfiguration(left: readonly WorkspaceFolder[], right: readonly WorkspaceFolder[]): boolean {
  return left.length === right.length && left.every((folder, index) => {
    const candidate = right[index];
    return candidate?.id === folder.id && candidate.name === folder.name && candidate.path === folder.path;
  });
}

export async function applyWorkspaceFolderConfiguration(
  ctx: ToolContext,
  folders: readonly WorkspaceFolder[]
): Promise<WorkspaceFolderHotApplyResult> {
  if (sameFolderConfiguration(ctx.config.folders, folders)) return { changed: false, applied: false };

  const desiredById = new Map(folders.map(folder => [folder.id, folder]));
  for (const [folderId, runtime] of ctx.folderRuntimes) {
    const desired = desiredById.get(folderId);
    if ((!desired || desired.path !== runtime.workspacePath) && folderRuntimeBusy(runtime)) {
      return {
        changed: true,
        applied: false,
        deferredReason: `Workspace folder ${folderId} is still in use by an active session or pending operation.`
      };
    }
  }

  const removedFolder = [...ctx.folderRuntimes.keys()].some(folderId => !desiredById.has(folderId));
  const pathChanged = folders.some(folder => {
    const runtime = ctx.folderRuntimes.get(folder.id);
    return Boolean(runtime && runtime.workspacePath !== folder.path);
  });
  const runtimesToInitialize: FolderRuntime[] = [];
  const nextRuntimes = new Map<string, FolderRuntime>();
  for (const folder of folders) {
    const existing = ctx.folderRuntimes.get(folder.id);
    const runtime = existing ?? createFolderRuntime(ctx.config, folder);
    if (!existing) runtimesToInitialize.push(runtime);
    if (runtime.workspacePath !== folder.path) {
      runtime.workspacePath = folder.path;
      const knowledge = knowledgeRuntime(ctx.config, folder);
      Object.assign(runtime, knowledge);
      runtime.skillRegistry = skillRegistry(ctx.config, folder, knowledge.evolvedSkillRegistry);
      runtimesToInitialize.push(runtime);
      runtime.operationsByFingerprint.clear();
    }
    nextRuntimes.set(folder.id, runtime);
  }

  ctx.folderRuntimes.clear();
  for (const [folderId, runtime] of nextRuntimes) ctx.folderRuntimes.set(folderId, runtime);
  ctx.config.folders = folders.map(folder => ({ ...folder }));
  ctx.extensions.setFolders(ctx.config.folders);
  if (!ctx.config.workspaceId) ctx.workspaceProfileId = deriveWorkspaceProfileId(ctx.config.folders);
  if (!ctx.folderRuntimes.size) throw new Error('at least one workspace folder is required');

  if (runtimesToInitialize.length) {
    await syncToolEvolutionRuntimes(
      [...new Set(runtimesToInitialize)],
      ctx.usageStore,
      'tool_evolution_workspace_hot_apply'
    );
  }

  if (removedFolder) ctx.selections.clear();
  else if (pathChanged) ctx.defaultCwds.clear();
  return { changed: true, applied: true };
}

export function runtimeForFolderId(ctx: ToolContext, folderId: string): FolderRuntime {
  const runtime = ctx.folderRuntimes.get(folderId);
  if (!runtime) throw new Error('WORKSPACE_FOLDER_NOT_FOUND');
  return runtime;
}

export function currentFolderRuntime(ctx: ToolContext, key: string): FolderRuntime {
  const binding = currentExecutionBinding(ctx, key);
  if (binding?.runtime) return binding.runtime;
  const folderId = binding?.folderId ?? ctx.selections.get(key);
  if (!folderId) throw new Error('WORKSPACE_FOLDER_NOT_SELECTED');
  return runtimeForFolderId(ctx, folderId);
}

export function allFolderRuntimes(ctx: ToolContext): FolderRuntime[] {
  return [...ctx.folderRuntimes.values()];
}

export function findPendingOperation(
  ctx: ToolContext,
  resumeId: string
): { runtime: FolderRuntime; operation: PendingOperation } | undefined {
  for (const runtime of ctx.folderRuntimes.values()) {
    const operation = runtime.pendingOperations.get(resumeId);
    if (operation) return { runtime, operation };
  }
  return undefined;
}
