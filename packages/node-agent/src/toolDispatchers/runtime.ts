import { stat } from 'node:fs/promises';
import { toolNamesForProfile, toolsetRevisionForProfile } from '../catalog.js';
import type { ConversationIdentity } from '../conversation/contract.js';
import { ConversationRoutingError } from '../conversation.js';
import { ABSOLUTE_COMMAND_TIMEOUT_MAX_MS } from '../executionLimits.js';
import { ABSOLUTE_JOB_TIMEOUT_MAX_MS, configuredJobTimeoutMaxMs } from '../processes/timeoutPolicy.js';
import { bootstrapHistory } from '../history.js';
import type { ToolDispatchRequest, ToolHandlerMap } from '../toolDispatch/contract.js';
import type { JsonObject, ToolContext } from '../types.js';
import { runtimeRevisionForWorkspace } from '../runtimeRevision.js';
import { skillSummary } from '../skills/types.js';
import { toolStrategyImplementationSnapshot } from '../knowledge/strategyImplementations.js';
import { AGENT_VERSION, CLIENT_COMPAT_VERSION } from '../version.js';
import { resolveExistingDirectory, resolveInside, rootAndCwd, selectedFolderSafe, validatedFolderCwd } from '../workspace.js';
import { normalizedSandboxConfig, sandboxAvailable, sandboxBackend, sandboxBackends } from '../sandbox.js';
import { LATEST_MCP_PROTOCOL_VERSION, SUPPORTED_MCP_PROTOCOL_VERSIONS } from '../mcpTransport.js';
import { validateWslWorkspacePath } from '../wsl.js';

function ok(value: JsonObject = {}): JsonObject {
  return { ok: true, ...value };
}

function workspaceHistoryDir(folder: { path: string }): string {
  return resolveInside(folder.path, 'docs/history-session');
}

function historyBootstrapError(error: unknown): JsonObject {
  const record = error && typeof error === 'object' ? error as Record<string, unknown> : {};
  const code = typeof record.code === 'string' && record.code ? record.code : 'HISTORY_BOOTSTRAP_FAILED';
  const message = error instanceof Error && error.message ? error.message : 'Session history bootstrap failed.';
  const category = typeof record.category === 'string' && record.category
    ? record.category
    : ['EACCES', 'EPERM'].includes(code) ? 'permission' : 'history';
  const retryable = typeof record.retryable === 'boolean'
    ? record.retryable
    : ['EACCES', 'EPERM', 'EBUSY'].includes(code);
  const details = record.details && typeof record.details === 'object' && !Array.isArray(record.details)
    ? record.details as JsonObject
    : {};
  return { code, message, category, retryable, details };
}

function workspaceFolderListing(ctx: ToolContext, key: string, identity: ConversationIdentity): JsonObject {
  const routable = identity.source !== 'missing_mcp_conversation';
  const selectedFolderId = routable ? ctx.selections.get(key) : undefined;
  const defaultCwd = selectedFolderId ? validatedFolderCwd(ctx, key, selectedFolderId) : null;
  const selectionScope = selectedFolderId
    ? identity.isolated ? 'conversation' : 'runtime'
    : 'unselected';
  return {
    multi_folder: ctx.config.folders.length > 1,
    profile_id: ctx.workspaceProfileId,
    selected_folder_id: selectedFolderId ?? null,
    selection_scope: selectionScope,
    conversation_isolated: identity.isolated,
    conversation_source: identity.source,
    default_cwd: defaultCwd,
    folders: ctx.config.folders.map(folder => ({
      ...folder,
      selected: folder.id === selectedFolderId,
      history_dir: workspaceHistoryDir(folder),
      default_cwd: routable ? validatedFolderCwd(ctx, key, folder.id) : '.'
    }))
  };
}

async function bindWorkspaceFolder(
  ctx: ToolContext,
  key: string,
  identity: ConversationIdentity,
  id: string
): Promise<JsonObject> {
  if (identity.requiresConversation && !identity.isolated) {
    throw new ConversationRoutingError(
      'WORKSPACE_FOLDER_NOT_SELECTED',
      'MCP conversation/session identity is missing; a workspace folder cannot be bound without isolated conversation metadata.',
      false,
      { selection_scope: 'unselected', conversation_isolated: false }
    );
  }
  const folder = ctx.config.folders.find(item => item.id === id);
  if (!folder) {
    throw new ConversationRoutingError(
      'WORKSPACE_FOLDER_NOT_FOUND',
      `Workspace folder is not allowed: ${id}`,
      false,
      { folder_id: id, available_folder_ids: ctx.config.folders.map(item => item.id) }
    );
  }
  await validateWslWorkspacePath(folder.path);
  const info = await stat(folder.path);
  if (!info.isDirectory()) throw new ConversationRoutingError('WORKSPACE_FOLDER_NOT_DIRECTORY', `Workspace root must be a directory: ${id}`);
  const defaultCwd = ctx.conversations.selectFolder(key, id);
  return {
    selected_folder_id: id,
    selected_folder: folder,
    profile_id: ctx.workspaceProfileId,
    selection_scope: identity.isolated ? 'conversation' : 'runtime',
    conversation_isolated: identity.isolated,
    conversation_source: identity.source,
    history_dir: workspaceHistoryDir(folder),
    default_cwd: defaultCwd,
    resolved_cwd: resolveInside(folder.path, defaultCwd)
  };
}

async function serverInfo({ ctx, key, identity }: ToolDispatchRequest): Promise<JsonObject> {
  const folder = identity.source === 'missing_mcp_conversation' ? undefined : selectedFolderSafe(ctx, key);
  const profileTools = toolNamesForProfile(ctx.config.activeToolProfile);
  const profileRevision = toolsetRevisionForProfile(ctx.config.activeToolProfile);
  const processStartedAtMs = Math.round(Date.now() - process.uptime() * 1000);
  const revision = await runtimeRevisionForWorkspace(folder?.path);
  const sandboxConfig = normalizedSandboxConfig(ctx.config.sandbox);
  const sandboxDescriptor = sandboxBackend(sandboxConfig.backend);
  const sandboxReady = sandboxAvailable(sandboxConfig);
  const sandboxEnforced = sandboxConfig.enabled && sandboxReady;
  const sandboxBoundary = sandboxEnforced
    ? sandboxConfig.backend
    : sandboxConfig.enabled
      ? 'sandbox_unavailable'
      : 'policy_only';
  return ok({
    server: 'coding-tools-mcp-node', title: 'Coding Tools MCP Node Agent', version: AGENT_VERSION,
    client_compat_version: CLIENT_COMPAT_VERSION,
    protocol_version: LATEST_MCP_PROTOCOL_VERSION, supported_protocol_versions: [...SUPPORTED_MCP_PROTOCOL_VERSIONS],
    endpoint_path: '/mcp', auth_enabled: true, auth_type: 'oauth', tool_count: profileTools.length,
    tools: profileTools, toolset_revision: profileRevision, workspace: folder?.path ?? null,
    configured_tool_profile: ctx.config.toolProfile, tool_profile: ctx.config.activeToolProfile,
    runtime_revision: {
      process_started_at_ms: processStartedAtMs,
      ...revision,
      // Kept for older clients. Exact build-SHA matching above is the authoritative trust signal.
      workspace_head_committed_at_ms: null,
      runtime_predates_workspace_head: null
    },
    profile_id: ctx.workspaceProfileId,
    selected_folder_id: folder?.id ?? null,
    selection_scope: folder ? identity.isolated ? 'conversation' : 'runtime' : 'unselected',
    conversation_isolated: identity.isolated,
    conversation_source: identity.source,
    default_cwd: folder ? ctx.conversations.peekCwdFor(key, folder.id) : null,
    permission_mode: ctx.config.permissionMode,
    policy: ctx.config.policy,
    node_version: process.version, platform: process.platform, arch: process.arch,
    environment: {
      filesystem_sandbox: {
        enabled: sandboxConfig.enabled,
        available: sandboxReady,
        enforced: sandboxEnforced,
        backend: sandboxConfig.backend,
        host_supported: sandboxDescriptor?.hostSupported ?? false,
        enforcement_ready: sandboxDescriptor?.enforcementReady ?? false,
        verification_tool: 'exec_health_check',
        live_verification_required: sandboxConfig.enabled,
        backends: sandboxBackends()
      },
      workspace_exec: {
        available: !sandboxConfig.enabled || sandboxReady,
        sandbox_enforced: sandboxEnforced,
        sandbox_backend: sandboxConfig.backend,
        boundary: sandboxBoundary
      }
    },
    limits: {
      ...ctx.config.limits,
      commandTimeoutAbsoluteMaxMs: ABSOLUTE_COMMAND_TIMEOUT_MAX_MS,
      jobTimeoutMaxMs: configuredJobTimeoutMaxMs(),
      jobTimeoutAbsoluteMaxMs: ABSOLUTE_JOB_TIMEOUT_MAX_MS
    },
    tunnel: ctx.tunnelStatus ?? { enabled: false, state: 'disabled', workers: 0, connectedWorkers: 0, completedRequests: 0 },
    native_binary_free: true, unsupported_tunnels: ['frp', 'cloudflare']
  });
}

async function switchWorkspaceFolder({ ctx, key, args, identity }: ToolDispatchRequest): Promise<JsonObject> {
  const id = String(args.folder_id ?? '').trim();
  return ok({
    ...await bindWorkspaceFolder(ctx, key, identity, id),
    next_action: 'Call history_session_bootstrap after selecting a folder for a new conversation.'
  });
}

async function conversationBootstrap({ ctx, key, args, historyArgs, identity }: ToolDispatchRequest): Promise<JsonObject> {
  const listing = workspaceFolderListing(ctx, key, identity);
  const requestedId = String(args.folder_id ?? '').trim();
  const selectedId = typeof listing.selected_folder_id === 'string' ? listing.selected_folder_id : '';
  const id = requestedId || selectedId || (ctx.config.folders.length === 1 ? ctx.config.folders[0]!.id : '');
  if (!id) {
    return ok({
      ...listing,
      needs_folder_selection: true,
      next_action: {
        tool: 'conversation_bootstrap',
        required_arguments: ['folder_id'],
        suggestion: 'Choose one folders entry and retry conversation_bootstrap with its id.'
      }
    });
  }
  const routing = await bindWorkspaceFolder(ctx, key, identity, id);
  const bootstrapArgs = { ...historyArgs };
  delete bootstrapArgs.folder_id;
  const skillRuntime = ctx.folderRuntimes.get(id);
  const skillSnapshot = skillRuntime ? await skillRuntime.skillRegistry.snapshot() : undefined;
  const strategyImplementationSnapshot = toolStrategyImplementationSnapshot();
  const learningSnapshot = skillRuntime ? await Promise.all([
    skillRuntime.knowledgeStore.revision(),
    skillRuntime.toolStrategyRegistry.snapshot(),
    skillRuntime.toolEvolutionRegistry.snapshot(),
    skillRuntime.evolvedSkillRegistry.snapshot(),
    skillRuntime.knowledgeImpactStore.revision(),
    skillRuntime.knowledgeImpactStore.list(),
    skillRuntime.canaryImpactStore.revision(),
    skillRuntime.canaryImpactStore.list()
  ]) : undefined;
  const projectSkills = skillSnapshot ? {
    count: skillSnapshot.skills.length,
    skillset_revision: skillSnapshot.revision,
    skills: skillSnapshot.skills.map(skillSummary),
    diagnostics: skillSnapshot.diagnostics,
    mcp_surfaces: ['prompts/list', 'prompts/get', 'resources/list', 'resources/read'],
    loading_policy: 'Load only clearly relevant workspace or user-level skills; skill guidance never changes runtime permissions.'
  } : { count: 0, skills: [] };
  const learning = learningSnapshot ? {
    knowledge_revision: learningSnapshot[0],
    tool_strategy_revision: learningSnapshot[1].revision,
    tool_strategy_count: learningSnapshot[1].strategies.length,
    tool_strategy_implementation_revision: strategyImplementationSnapshot.revision,
    tool_strategy_implementation_count: strategyImplementationSnapshot.count,
    tool_evolution_revision: learningSnapshot[2].revision,
    tool_evolution_candidate_count: learningSnapshot[2].candidates.length,
    evolved_skill_revision: learningSnapshot[3].revision,
    promoted_evolved_skill_count: learningSnapshot[3].skills.length,
    shadow_impact_revision: learningSnapshot[4],
    shadow_impact_count: learningSnapshot[5].length,
    canary_impact_revision: learningSnapshot[6],
    canary_impact_count: learningSnapshot[7].length,
    canary_promoted_count: learningSnapshot[7].filter(item => Boolean(item.promotedAtMs) && !item.rolledBackAtMs).length,
    canary_rolled_back_count: learningSnapshot[7].filter(item => Boolean(item.rolledBackAtMs)).length,
    validated_tool_strategy_count: learningSnapshot[1].strategies.filter(item => item.status === 'validated').length,
    promoted_tool_strategy_count: learningSnapshot[1].strategies.filter(item => item.status === 'promoted').length,
    validated_tool_evolution_count: learningSnapshot[2].candidates.filter(item => item.status === 'validated').length,
    ingestion: skillRuntime?.knowledgeIngestor.snapshot(),
    runtime_policy: 'adaptive_rollout_phase_2e',
    canary_policy: 'Validated strategies use a deterministic 25% semantics-preserving canary; evidence-positive strategies promote to 100% rollout, while post-promotion regressions automatically reject and disable them. Caller arguments and static security policy remain authoritative.',
    knowledge_loading_policy: 'Persistent knowledge and rejected/candidate experiments are not injected into inference context; rollout lifecycle and impact remain out of band.'
  } : undefined;
  const legacyStartupFallback = ['list_workspace_folders', 'switch_workspace_folder', 'history_session_bootstrap'];
  let history: JsonObject;
  try {
    history = await bootstrapHistory(ctx, key, bootstrapArgs);
  } catch (error) {
    return ok({
      ...routing,
      needs_folder_selection: false,
      startup_flow: 'workspace_bootstrapped_history_degraded',
      history_bootstrap_ok: false,
      history_bootstrap_error: historyBootstrapError(error),
      warnings: ['Session history could not be initialized; workspace project tools remain available.'],
      recovery_actions: [{
        action: 'retry_history_bootstrap',
        tool: 'history_session_bootstrap',
        suggestion: 'Retry history_session_bootstrap, optionally with a writable history_dir, when durable handoff/checkpointing is needed.'
      }],
      project_skills: projectSkills,
      ...(learning ? { learning } : {}),
      legacy_startup_fallback: legacyStartupFallback
    });
  }
  return {
    ...history,
    ...routing,
    needs_folder_selection: false,
    startup_flow: 'workspace_and_history_bootstrapped',
    history_bootstrap_ok: true,
    project_skills: projectSkills,
    ...(learning ? { learning } : {}),
    legacy_startup_fallback: legacyStartupFallback
  };
}

async function setDefaultCwd({ ctx, key, args }: ToolDispatchRequest): Promise<JsonObject> {
  const resolved = await resolveExistingDirectory(
    rootAndCwd(ctx, key).root,
    String(args.path ?? '.'),
    'Default cwd must be a directory'
  );
  ctx.conversations.setFolderCwd(key, rootAndCwd(ctx, key).folder.id, resolved.display);
  return ok({ default_cwd: resolved.display, resolved_cwd: resolved.full });
}

export const runtimeToolHandlers = {
  server_info: serverInfo,
  list_workspace_folders: ({ ctx, key, identity }) => ok(workspaceFolderListing(ctx, key, identity)),
  switch_workspace_folder: switchWorkspaceFolder,
  conversation_bootstrap: conversationBootstrap,
  query_tool_usage: ({ ctx, args }) => ctx.usageStore.query(args),
  set_default_cwd: setDefaultCwd
} satisfies ToolHandlerMap;
