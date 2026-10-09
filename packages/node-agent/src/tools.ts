import { createHash } from 'node:crypto';
import type { JsonObject, ToolContext } from './types.js';
import { toolNamesForProfile, toolsetRevisionForProfile } from './catalog.js';
import { editTool } from './fileTools.js';
import {
  type ProcessRequestLifecycle
} from './processes.js';
import {
  attachHarnessStatus, beginHarnessTracking, finishHarnessTracking,
  HarnessError, type HarnessTracking
} from './taskTools.js';
import { resolveExistingPath, selectedFolderSafe, validatedFolderCwd } from './workspace.js';
import { OutputRedactionContext } from './redaction.js';
import { validateToolPolicy } from './policy.js';
import { currentExecutionBinding, runWithExecutionBinding } from './executionScope.js';
import { currentFolderRuntime, runtimeForFolderId } from './folderRuntime.js';
import { normalizeToolResult, toolErrorResult, toolFail } from './toolContract.js';
import { canCoalesceToolCall, canonicalToolCall, toolRuntimeFor } from './toolRuntime.js';
import { dispatchDomainTool } from './toolDispatch.js';
import { pendingPermissionBinding, permissionDecision } from './permissionTools.js';
import { ConversationRoutingError } from './conversation.js';
import type { KnowledgeCanaryDecision } from './knowledge/canary.js';
import { selectedSkillAttribution } from './knowledge/skillAttribution.js';

const inflightToolCalls = new WeakMap<ToolContext, Map<string, Promise<JsonObject>>>();
const conversationSessionRoutes = new WeakMap<object, Map<string, Map<string, string>>>();
const conversationLearningTasks = new WeakMap<object, Map<string, string>>();
const ALTERNATE_WORKSPACE_PATH_RECOVERY_TOOLS = new Set([
  'read_file', 'view_image', 'list_files', 'search_text', 'project_map', 'git_status'
]);

function learningTaskKey(key: string, folderId: string | undefined): string {
  return `${key}\u0000${folderId ?? ''}`;
}

function learningTaskMap(ctx: ToolContext): Map<string, string> {
  const owner = ctx.conversations as object;
  let tasks = conversationLearningTasks.get(owner);
  if (!tasks) {
    tasks = new Map();
    conversationLearningTasks.set(owner, tasks);
  }
  return tasks;
}

function resultTask(result: JsonObject): { id: string; status?: string } | undefined {
  if (!result.task || typeof result.task !== 'object' || Array.isArray(result.task)) return undefined;
  const task = result.task as JsonObject;
  if (typeof task.id !== 'string' || !task.id) return undefined;
  return { id: task.id, ...(typeof task.status === 'string' ? { status: task.status } : {}) };
}

function terminalTaskOutcome(status: string | undefined): 'verified_success' | 'unverified_success' | 'failure' | 'rolled_back' | undefined {
  if (status === 'completed') return 'verified_success';
  if (status === 'completed_unverified') return 'unverified_success';
  if (status === 'failed_final') return 'failure';
  if (status === 'rolled_back') return 'rolled_back';
  return undefined;
}

function taskLearningAttribution(
  ctx: ToolContext,
  key: string,
  folderId: string | undefined,
  input: Parameters<ToolContext['usageStore']['recordToolCall']>[0],
  record: JsonObject
): { operationId?: string; taskId?: string; conversationContextId: string; taskOutcome?: 'verified_success' | 'unverified_success' | 'failure' | 'rolled_back' } {
  const routeKey = learningTaskKey(key, folderId);
  const conversationContextId = createHash('sha256').update(`conversation\0${key}`).digest('hex');
  const tasks = learningTaskMap(ctx);
  const returnedTask = resultTask(input.result);
  const explicitTaskId = typeof input.arguments.task_id === 'string' && input.arguments.task_id.trim()
    ? input.arguments.task_id.trim()
    : returnedTask?.id;
  const candidateOperationIds = [
    record.conversation_operation_id,
    input.result.harness_operation_id,
    input.result.operation_id
  ].filter((value): value is string => typeof value === 'string' && value.length > 0);
  let matchedOperation: ReturnType<ToolContext['state']['operations']>[number] | undefined;
  if (candidateOperationIds.length > 0) {
    const operations = ctx.state.operations(undefined, 64);
    for (let index = operations.length - 1; index >= 0; index -= 1) {
      const operation = operations[index]!;
      if (candidateOperationIds.includes(operation.id)) {
        matchedOperation = operation;
        break;
      }
    }
  }
  const operationId = matchedOperation?.id;
  const taskId = explicitTaskId ?? matchedOperation?.task_id ?? tasks.get(routeKey);
  const taskOutcome = terminalTaskOutcome(returnedTask?.status);
  if (returnedTask) {
    if (taskOutcome) tasks.delete(routeKey);
    else tasks.set(routeKey, returnedTask.id);
  }
  if (!taskId) return { ...(operationId ? { operationId } : {}), conversationContextId };
  return {
    ...(operationId ? { operationId } : {}),
    taskId,
    conversationContextId,
    ...(taskOutcome ? { taskOutcome } : {})
  };
}

function recordToolUsageAndLearn(
  ctx: ToolContext,
  key: string,
  folderId: string | undefined,
  input: Parameters<ToolContext['usageStore']['recordToolCall']>[0]
): void {
  const record = ctx.usageStore.recordToolCall(input);
  if (folderId) {
    const runtime = ctx.folderRuntimes.get(folderId);
    if (runtime) {
      const taskAttribution = taskLearningAttribution(ctx, key, folderId, input, record);
      const selection = selectedSkillAttribution(ctx.conversations as object, key, folderId);
      const skill = selection ? {
        source: selection.source,
        name: selection.name,
        contentSha256: selection.contentSha256,
        generation: selection.generation
      } : undefined;
      const attribution = {
        ...taskAttribution,
        ...(skill ? { skill } : {}),
        ...(selection?.canary ? { canary: { ...selection.canary } } : {})
      };
      runtime.knowledgeIngestor.enqueueToolUsage(record, attribution);
      runtime.toolEvolutionBenchmarkCollector.enqueueToolUsage(record, attribution);
    }
  }
}

function conversationSessionRouteMap(ctx: ToolContext, key: string): Map<string, string> {
  const routeOwner = ctx.conversations as object;
  let byConversation = conversationSessionRoutes.get(routeOwner);
  if (!byConversation) {
    byConversation = new Map();
    conversationSessionRoutes.set(routeOwner, byConversation);
  }
  let routes = byConversation.get(key);
  if (!routes) {
    if (byConversation.size >= 128) {
      const oldest = byConversation.keys().next().value;
      if (oldest !== undefined) byConversation.delete(oldest);
    }
    routes = new Map();
    byConversation.set(key, routes);
  }
  return routes;
}

function controlSessionId(name: string, args: JsonObject): string | undefined {
  if (['wait_command', 'send_input', 'kill_session'].includes(name)) {
    const value = String(args.session_id ?? '').trim();
    return value || undefined;
  }
  if (name === 'read_output') {
    const outputRef = String(args.output_ref ?? '').trim();
    if (!outputRef.startsWith('output://')) return undefined;
    return outputRef.slice('output://'.length).split('/', 1)[0] || undefined;
  }
  return undefined;
}

function recordConversationSessionRoutes(
  ctx: ToolContext,
  key: string,
  folderId: string | undefined,
  result: JsonObject
): void {
  if (!folderId) return;
  const routes = conversationSessionRouteMap(ctx, key);
  const sessionIds = new Set<string>();
  const collect = (value: unknown): void => {
    if (!value || typeof value !== 'object' || Array.isArray(value)) return;
    const object = value as JsonObject;
    const sessionId = typeof object.session_id === 'string' ? object.session_id.trim() : '';
    if (sessionId) sessionIds.add(sessionId);
    if (object.output_refs && typeof object.output_refs === 'object' && !Array.isArray(object.output_refs)) {
      for (const outputRef of Object.values(object.output_refs as JsonObject)) {
        if (typeof outputRef !== 'string' || !outputRef.startsWith('output://')) continue;
        const parsed = outputRef.slice('output://'.length).split('/', 1)[0];
        if (parsed) sessionIds.add(parsed);
      }
    }
    if (Array.isArray(object.results)) {
      for (const item of object.results) {
        if (item && typeof item === 'object' && !Array.isArray(item)) {
          collect((item as JsonObject).result ?? item);
        }
      }
    }
  };
  collect(result);
  for (const sessionId of sessionIds) {
    if (routes.size >= 256 && !routes.has(sessionId)) {
      const oldest = routes.keys().next().value;
      if (oldest !== undefined) routes.delete(oldest);
    }
    routes.set(sessionId, folderId);
  }
}

function stableRequestValue(value: unknown): unknown {
  if (Array.isArray(value)) return value.map(stableRequestValue);
  if (value && typeof value === 'object') {
    return Object.fromEntries(Object.entries(value as Record<string, unknown>)
      .sort(([left], [right]) => left.localeCompare(right))
      .map(([field, nested]) => [field, stableRequestValue(nested)]));
  }
  return value;
}

function inflightCallKey(
  conversationKey: string,
  name: string,
  args: JsonObject,
  binding: ReturnType<typeof executionBindingFor>
): string {
  const material = stableRequestValue({
    conversation_key: conversationKey,
    tool: name,
    arguments: args,
    folder_id: binding.folderId ?? null,
    default_cwd: binding.defaultCwd ?? null,
    requested_workspace_id: binding.requestedWorkspaceId ?? null,
    route_source: binding.routeSource ?? null
  });
  return createHash('sha256').update(JSON.stringify(material)).digest('hex');
}

function contextInflightCalls(ctx: ToolContext): Map<string, Promise<JsonObject>> {
  let calls = inflightToolCalls.get(ctx);
  if (!calls) {
    calls = new Map();
    inflightToolCalls.set(ctx, calls);
  }
  return calls;
}

function elapsedPhaseMs(startedAt: number): number {
  return Math.max(0, Math.round(performance.now() - startedAt));
}

function addPhaseDuration(result: JsonObject, phase: string, durationMs: number): void {
  const phases = result.phase_durations_ms && typeof result.phase_durations_ms === 'object' && !Array.isArray(result.phase_durations_ms)
    ? { ...result.phase_durations_ms as JsonObject }
    : {};
  const previous = Number(phases[phase] ?? 0);
  phases[phase] = Math.max(0, Math.round(Number.isFinite(previous) ? previous : 0)) + Math.max(0, Math.round(durationMs));
  result.phase_durations_ms = phases;
}

const fail = toolFail;

function selectedFolderId(ctx: ToolContext, key: string): string | undefined {
  return selectedFolderSafe(ctx, key)?.id;
}

async function dispatch(
  ctx: ToolContext,
  name: string,
  args: JsonObject,
  meta: unknown,
  processLifecycle?: ProcessRequestLifecycle,
  knowledgeCanary?: KnowledgeCanaryDecision
): Promise<JsonObject> {
  const identity = ctx.conversations.identity(meta);
  const key = identity.key;
  const historyArgs = identity.isolated
    ? { ...args, _host_session_key: key }
    : identity.source === 'stable_fallback'
      ? { ...args, _fallback_session_key: key }
      : args;
  const domainResult = dispatchDomainTool(name, {
    ctx,
    key,
    identity,
    args,
    historyArgs,
    processLifecycle,
    knowledgeCanary,
    resumeTool: request => callTool(
      ctx,
      request.name,
      request.args,
      request.meta,
      true,
      processLifecycle,
      { folderId: request.folderId, defaultCwd: request.defaultCwd }
    )
  });
  if (domainResult) return domainResult;

  return fail('UNKNOWN_TOOL', `Unknown tool: ${name}`, 'catalog', true, {
    available_tools: toolNamesForProfile(ctx.config.activeToolProfile),
    toolset_revision: toolsetRevisionForProfile(ctx.config.activeToolProfile)
  });
}

interface ToolBindingOverride {
  folderId: string;
  defaultCwd?: string;
}

function executionBindingFor(
  ctx: ToolContext,
  key: string,
  name: string,
  args: JsonObject,
  override?: ToolBindingOverride,
  requestedWorkspaceId?: string
) {
  const selectedWorkspaceId = ctx.selections.get(key);
  if (override) {
    const runtime = runtimeForFolderId(ctx, override.folderId);
    return {
      ctx, key, folderId: override.folderId,
      defaultCwd: validatedFolderCwd(ctx, key, override.folderId, override.defaultCwd ?? '.'), runtime,
      selectedWorkspaceId, routeSource: 'permission_resume' as const
    };
  }
  if (name === 'list_workspace_folders' || name === 'switch_workspace_folder' || name === 'conversation_bootstrap') return { ctx, key };
  if (requestedWorkspaceId) {
    const runtime = ctx.folderRuntimes.get(requestedWorkspaceId);
    if (!runtime) {
      throw new ConversationRoutingError(
        'WORKSPACE_FOLDER_NOT_FOUND',
        `Workspace folder is not configured: ${requestedWorkspaceId}`,
        false,
        { folder_id: requestedWorkspaceId }
      );
    }
    return {
      ctx, key, folderId: requestedWorkspaceId,
      defaultCwd: validatedFolderCwd(ctx, key, requestedWorkspaceId), runtime,
      requestedWorkspaceId, selectedWorkspaceId, routeSource: 'explicit' as const
    };
  }
  if (name === 'request_permissions') {
    const binding = pendingPermissionBinding(ctx, args);
    if (binding) return { ctx, key, ...binding, selectedWorkspaceId, routeSource: 'resume_id' as const };
  }
  const sessionId = controlSessionId(name, args);
  if (sessionId) {
    const folderId = conversationSessionRouteMap(ctx, key).get(sessionId);
    if (folderId) {
      return {
        ctx, key, folderId, defaultCwd: validatedFolderCwd(ctx, key, folderId),
        runtime: runtimeForFolderId(ctx, folderId), selectedWorkspaceId, routeSource: 'session_id' as const
      };
    }
  }
  if (!selectedWorkspaceId) return { ctx, key };
  return {
    ctx,
    key,
    folderId: selectedWorkspaceId,
    defaultCwd: validatedFolderCwd(ctx, key, selectedWorkspaceId),
    runtime: runtimeForFolderId(ctx, selectedWorkspaceId),
    selectedWorkspaceId,
    routeSource: 'conversation' as const
  };
}

async function enrichAlternateWorkspaceRecovery(
  ctx: ToolContext,
  name: string,
  args: JsonObject,
  result: JsonObject,
  binding: { folderId?: string; routeSource?: string } | undefined
): Promise<JsonObject> {
  if (result.ok !== false || !binding || binding.routeSource !== 'conversation' || !binding.folderId) return result;
  if (!ALTERNATE_WORKSPACE_PATH_RECOVERY_TOOLS.has(name)) return result;
  const error = result.error && typeof result.error === 'object' && !Array.isArray(result.error)
    ? result.error as JsonObject
    : undefined;
  if (!error || error.code !== 'NOT_FOUND') return result;
  const rawPath = typeof args.path === 'string' ? args.path.trim() : '';
  if (!rawPath || rawPath === '.') return result;

  const matches: Array<{ folder_id: string; folder_name: string }> = [];
  for (const folder of ctx.config.folders) {
    if (folder.id === binding.folderId) continue;
    try {
      await resolveExistingPath(folder.path, rawPath);
      matches.push({ folder_id: folder.id, folder_name: folder.name });
    } catch {
      // Preserve the original failure if this path does not safely resolve in the alternate folder.
    }
  }
  if (!matches.length) return result;

  const recoveryActions = matches.slice(0, 8).map(folder => ({
    action: 'retry_in_workspace',
    action_id: `workspace-retry-${folder.folder_id}`,
    tool: name,
    required_arguments: [],
    arguments: { ...args, workspace_folder_id: folder.folder_id },
    reason: 'alternate_workspace_match'
  }));
  const details = error.details && typeof error.details === 'object' && !Array.isArray(error.details)
    ? { ...error.details as JsonObject }
    : {};
  return {
    ...result,
    error: {
      ...error,
      retryable: true,
      details: {
        ...details,
        alternate_workspace_match_count: matches.length,
        alternate_workspace_matches: matches,
        recovery_actions: recoveryActions,
        suggestion: matches.length === 1
          ? 'Retry this call with the provided workspace_folder_id; the conversation selection will not change.'
          : 'Choose one matching workspace recovery action; the conversation selection will not change.'
      }
    }
  };
}

interface RecoveryContext {
  retryOfCallSequence?: number;
  recoveryOfOperationId?: string;
  recoveryActionId?: string;
}

function takeRecoveryContext(args: JsonObject): RecoveryContext | JsonObject {
  const recovery: RecoveryContext = {};
  const retry = args.retry_of_call_sequence;
  delete args.retry_of_call_sequence;
  if (retry !== undefined) {
    if (!Number.isSafeInteger(retry) || Number(retry) <= 0) {
      return fail('INVALID_ARGUMENT', 'retry_of_call_sequence must be a positive integer', 'validation', false);
    }
    recovery.retryOfCallSequence = Number(retry);
  }
  const operationId = args.recovery_of_operation_id;
  delete args.recovery_of_operation_id;
  if (operationId !== undefined) {
    if (typeof operationId !== 'string' || !operationId.trim() || operationId.trim().length > 128) {
      return fail('INVALID_ARGUMENT', 'recovery_of_operation_id must contain 1-128 characters', 'validation', false);
    }
    recovery.recoveryOfOperationId = operationId.trim();
  }
  const actionId = args.recovery_action_id;
  delete args.recovery_action_id;
  if (actionId !== undefined) {
    const token = typeof actionId === 'string' ? actionId.trim() : '';
    if (!token || token.length > 128 || !/^[A-Za-z0-9._:-]+$/.test(token)) {
      return fail('INVALID_ARGUMENT', 'recovery_action_id must be a stable ASCII token', 'validation', false);
    }
    recovery.recoveryActionId = token;
  }
  return recovery;
}

function recoveryRequested(recovery: RecoveryContext): boolean {
  return recovery.retryOfCallSequence !== undefined
    || recovery.recoveryOfOperationId !== undefined
    || recovery.recoveryActionId !== undefined;
}

function failureId(name: string, semanticArgs: JsonObject, result: JsonObject, folderId?: string): string {
  const error = result.error && typeof result.error === 'object' && !Array.isArray(result.error)
    ? result.error as JsonObject
    : {};
  const details = error.details && typeof error.details === 'object' && !Array.isArray(error.details)
    ? error.details as JsonObject
    : {};
  const argumentsSha = createHash('sha256').update(JSON.stringify(semanticArgs)).digest('hex');
  const identity = {
    version: 'tool-failure-v2',
    tool: name,
    arguments_sha256: argumentsSha,
    resolved_workspace_id: folderId ?? null,
    error_code: error.code ?? result.error_code ?? null,
    error_category: error.category ?? result.error_category ?? null,
    stage: details.stage ?? null,
    reason: details.reason ?? null,
    path: details.path ?? null,
    file_index: details.file_index ?? null,
    edit_index: details.edit_index ?? null,
    expected_sha256: details.expected_sha256 ?? null,
    actual_sha256: details.actual_sha256 ?? null
  };
  return createHash('sha256').update(JSON.stringify(identity)).digest('hex');
}

function attachRecoveryMetadata(
  result: JsonObject,
  name: string,
  semanticArgs: JsonObject,
  recovery: RecoveryContext,
  folderId?: string
): JsonObject {
  if (recovery.retryOfCallSequence !== undefined) result.retry_of_call_sequence = recovery.retryOfCallSequence;
  if (recovery.recoveryOfOperationId !== undefined) {
    result.recovery_of_operation_id_hash = createHash('sha256')
      .update(recovery.recoveryOfOperationId)
      .digest('hex');
  }
  if (recovery.recoveryActionId !== undefined) result.recovery_action_id = recovery.recoveryActionId;
  if (recoveryRequested(recovery)) {
    result.recovery_attempt = true;
    result.recovery_succeeded = result.ok === true;
  }
  if (result.ok === false) result.failure_id = failureId(name, semanticArgs, result, folderId);
  return result;
}

function recoveryTelemetryArguments(args: JsonObject, recovery: RecoveryContext): JsonObject {
  return {
    ...args,
    ...(recovery.retryOfCallSequence !== undefined ? { retry_of_call_sequence: recovery.retryOfCallSequence } : {}),
    ...(recovery.recoveryOfOperationId !== undefined ? { recovery_of_operation_id: recovery.recoveryOfOperationId } : {}),
    ...(recovery.recoveryActionId !== undefined ? { recovery_action_id: recovery.recoveryActionId } : {})
  };
}

export async function callTool(
  ctx: ToolContext,
  name: string,
  args: JsonObject,
  meta: unknown,
  skipPermission = false,
  processLifecycle?: ProcessRequestLifecycle,
  bindingOverride?: ToolBindingOverride
): Promise<JsonObject> {
  const key = ctx.conversations.identity(meta).key;
  const cleanArgs = { ...args };
  const parsedRecovery = takeRecoveryContext(cleanArgs);
  if ('ok' in parsedRecovery) return normalizeToolResult(parsedRecovery);
  const recovery = parsedRecovery;
  const rawWorkspaceId = cleanArgs.workspace_folder_id;
  delete cleanArgs.workspace_folder_id;
  const canonical = canonicalToolCall(name, cleanArgs);
  const telemetryArgs = recoveryTelemetryArguments(canonical.args, recovery);
  let requestedWorkspaceId: string | undefined;
  if (rawWorkspaceId !== undefined) {
    if (!toolRuntimeFor(canonical.name).workspaceSelector) {
      return normalizeToolResult(fail(
        'INVALID_ARGUMENT',
        `workspace_folder_id is not supported by ${canonical.name}`,
        'validation',
        false
      ));
    }
    if (typeof rawWorkspaceId !== 'string' || !rawWorkspaceId.trim()) {
      return normalizeToolResult(fail(
        'INVALID_ARGUMENT',
        'workspace_folder_id must be a non-empty string',
        'validation',
        false
      ));
    }
    requestedWorkspaceId = rawWorkspaceId.trim();
  }
  let binding: ReturnType<typeof executionBindingFor>;
  try {
    binding = executionBindingFor(
      ctx,
      key,
      canonical.name,
      canonical.args,
      bindingOverride,
      requestedWorkspaceId
    );
  } catch (error) {
    return normalizeToolResult(toolErrorResult(error));
  }
  if (skipPermission || !canCoalesceToolCall(canonical.name, canonical.args)) {
    return runWithExecutionBinding(
      binding,
      () => callToolInScope(
        ctx,
        canonical.name,
        canonical.args,
        meta,
        skipPermission,
        processLifecycle,
        telemetryArgs,
        recovery
      )
    );
  }

  const callKey = inflightCallKey(key, canonical.name, canonical.args, binding);
  const inflight = contextInflightCalls(ctx);
  const existing = inflight.get(callKey);
  if (existing) {
    const startedAt = Date.now();
    const requestTiming = ctx.usageStore.beginRequest(startedAt);
    const shared = structuredClone(await existing);
    delete shared.phase_durations_ms;
    const durationMs = Date.now() - startedAt;
    Object.assign(shared, {
      coalesced_inflight: true,
      coalesced_wait_ms: durationMs,
      duration_ms: durationMs,
      admission_queue_wait_ms: 0,
      global_admission_wait_ms: 0,
      workspace_admission_wait_ms: 0,
      workspace_lock_wait_ms: 0
    });
    attachRecoveryMetadata(shared, canonical.name, canonical.args, recovery, binding.folderId);
    addPhaseDuration(shared, 'serialization_ms', 0);
    const serializationStartedAt = performance.now();
    const serializedShared = JSON.stringify(shared);
    const serializationMs = elapsedPhaseMs(serializationStartedAt);
    const sharedPhases = shared.phase_durations_ms as JsonObject;
    sharedPhases.serialization_ms = serializationMs;
    const responseBytes = Buffer.byteLength(serializedShared);
    const folderId = binding.folderId ?? selectedFolderId(ctx, key);
    recordToolUsageAndLearn(ctx, key, folderId, {
      tool: canonical.name,
      arguments: telemetryArgs,
      result: shared,
      startedTsMs: startedAt,
      durationMs,
      requestTiming,
      requestJsonBytes: Buffer.byteLength(JSON.stringify(telemetryArgs)),
      workspaceId: folderId
    });
    ctx.usage.push({ tool: canonical.name, startedAt, durationMs, ok: shared.ok === true, queueWaitMs: 0, lockWaitMs: 0, responseBytes });
    if (ctx.usage.length > 5_000) ctx.usage.splice(0, 1_000);
    return shared;
  }

  const promise = runWithExecutionBinding(
    binding,
    () => callToolInScope(
      ctx,
      canonical.name,
      canonical.args,
      meta,
      skipPermission,
      processLifecycle,
      telemetryArgs,
      recovery
    )
  );
  inflight.set(callKey, promise);
  try {
    return await promise;
  } finally {
    if (inflight.get(callKey) === promise) inflight.delete(callKey);
    if (inflight.size === 0) inflightToolCalls.delete(ctx);
  }
}

async function callToolInScope(
  ctx: ToolContext,
  name: string,
  args: JsonObject,
  meta: unknown,
  skipPermission = false,
  processLifecycle?: ProcessRequestLifecycle,
  telemetryArgs: JsonObject = args,
  recovery: RecoveryContext = {}
): Promise<JsonObject> {
  ctx = { ...ctx, config: structuredClone(ctx.config) };
  const redaction = new OutputRedactionContext(name, args, ctx.config.securityPolicy);
  const startedAt = Date.now();
  const requestTiming = skipPermission ? undefined : ctx.usageStore.beginRequest(startedAt);
  const key = ctx.conversations.identity(meta).key;
  const binding = currentExecutionBinding(ctx, key);
  const workspaceAdmission = binding?.runtime?.admission;
  const lockAdmission = workspaceAdmission ?? ctx.hubAdmission;
  const runtimePolicy = toolRuntimeFor(name);
  const requestAdmission = runtimePolicy.admission === 'request';
  const globalLane = !requestAdmission ? undefined : runtimePolicy.lane === 'process' ? ctx.hubAdmission.process : ctx.hubAdmission.blocking;
  const workspaceLane = !requestAdmission ? undefined : runtimePolicy.lane === 'process' ? workspaceAdmission?.process : workspaceAdmission?.blocking;
  let releaseGlobalLane: (() => void) | undefined;
  let releaseWorkspaceLane: (() => void) | undefined;
  let releaseLocks: (() => void) | undefined;
  let tracking: HarnessTracking | undefined;
  let trackingFinished = false;
  let queueWaitMs = 0;
  let globalAdmissionWaitMs = 0;
  let workspaceAdmissionWaitMs = 0;
  let lockWaitMs = 0;
  let preflightMs = 0;
  let harnessBeginMs = 0;
  let dispatchMs = 0;
  let harnessFinishMs = 0;
  let harnessFinishObserved = false;
  let preflightObserved = false;
  let harnessBeginObserved = false;
  let dispatchObserved = false;
  let result: JsonObject;
  const availableTools = toolNamesForProfile(ctx.config.activeToolProfile);
  const exposedTools = new Set(availableTools);
  const profileRevision = toolsetRevisionForProfile(ctx.config.activeToolProfile);
  const hookCwd = binding?.runtime?.workspacePath ?? process.cwd();
  const hookSessionId = key;
  let hookContext: string[] = [];
  let hookPreBlocked = false;
  let hookBlockResult: JsonObject | undefined;
  let knowledgeCanary: KnowledgeCanaryDecision | undefined;
  if (availableTools.includes(name)) {
    const pre = await ctx.extensions.preToolUse(name, args, hookCwd, hookSessionId, binding?.folderId);
    if (pre.blocked) {
      hookPreBlocked = true;
      hookBlockResult = fail('HOOK_BLOCKED', pre.blocked.message, 'policy', false, { hook_key: pre.blocked.hookKey });
    } else {
      args = pre.input;
      hookContext = pre.context;
    }
  }

  const mapError = (error: unknown): JsonObject => toolErrorResult(error);

  if (!availableTools.includes(name)) {
    result = fail('UNKNOWN_TOOL', `Unknown tool: ${name}`, 'catalog', true, {
      reason: 'unknown_tool',
      suggestion: 'Refresh tools/list and retry with the current tool catalog.',
      tool_profile: ctx.config.activeToolProfile,
      toolset_revision: profileRevision,
      available_tools: availableTools
    });
  } else if (hookBlockResult) {
    result = hookBlockResult;
  } else {
    try {
      if (name === 'search_text' && binding?.runtime) {
        try {
          knowledgeCanary = await binding.runtime.canaryStrategyEngine.decide(name, args, key);
        } catch {
          knowledgeCanary = undefined;
        }
      }
      await validateToolPolicy(ctx, key, name, args);
      const denied = skipPermission ? undefined : permissionDecision(ctx, key, name, args, meta);
      if (denied) {
        result = denied;
      } else {
        if (globalLane) {
          const globalStarted = Date.now();
          releaseGlobalLane = await globalLane.acquire(30_000, processLifecycle?.signal);
          globalAdmissionWaitMs = Date.now() - globalStarted;
        }
        if (workspaceLane) {
          const workspaceStarted = Date.now();
          releaseWorkspaceLane = await workspaceLane.acquire(30_000, processLifecycle?.signal);
          workspaceAdmissionWaitMs = Date.now() - workspaceStarted;
        }
        queueWaitMs = globalAdmissionWaitMs + workspaceAdmissionWaitMs;
        const groups = runtimePolicy.lockGroups;
        if (groups.length) {
          const lockStarted = Date.now();
          releaseLocks = await lockAdmission.locks.acquire([...groups]);
          lockWaitMs = Date.now() - lockStarted;
        }
        let preflightResult: JsonObject | undefined;
        if (name === 'edit') {
          const preflightStartedAt = performance.now();
          try {
            const checked = await editTool(ctx, key, { ...args, dry_run: true });
            if (args.dry_run === true || checked.ok !== true || checked.status === 'proposal_required') preflightResult = checked;
          } catch (error) {
            preflightResult = mapError(error);
          } finally {
            preflightMs += elapsedPhaseMs(preflightStartedAt);
            preflightObserved = true;
          }
        }
        if (preflightResult) {
          result = preflightResult;
        } else {
          if (!runtimePolicy.harnessTool) {
            const harnessBeginStartedAt = performance.now();
            try {
              tracking = await beginHarnessTracking(ctx, key, name, args);
            } finally {
              harnessBeginMs += elapsedPhaseMs(harnessBeginStartedAt);
              harnessBeginObserved = true;
            }
          }
          const dispatchStartedAt = performance.now();
          try {
            result = await dispatch(ctx, name, args, meta, processLifecycle, knowledgeCanary);
          } catch (error) {
            result = mapError(error);
          } finally {
            dispatchMs += elapsedPhaseMs(dispatchStartedAt);
            dispatchObserved = true;
          }
          if (tracking) {
            const harnessFinishStartedAt = performance.now();
            try {
              result = await finishHarnessTracking(ctx, key, name, args, tracking, result, exposedTools);
              trackingFinished = true;
            } finally {
              harnessFinishObserved = true;
              harnessFinishMs += elapsedPhaseMs(harnessFinishStartedAt);
            }
          } else if (runtimePolicy.harnessTool && result.ok === false) {
            result = await attachHarnessStatus(ctx, key, result, false, exposedTools);
          }
        }
      }
    } catch (error) {
      result = mapError(error);
      if (tracking && !trackingFinished) {
        const harnessFinishStartedAt = performance.now();
        try {
          result = await finishHarnessTracking(ctx, key, name, args, tracking, result, exposedTools).catch(() => result);
        } finally {
          harnessFinishObserved = true;
          harnessFinishMs += elapsedPhaseMs(harnessFinishStartedAt);
        }
      } else if (error instanceof HarnessError) {
        result = await attachHarnessStatus(ctx, key, result, false, exposedTools);
      }
    } finally {
      releaseLocks?.();
      releaseWorkspaceLane?.();
      releaseGlobalLane?.();
    }
  }

  result = await enrichAlternateWorkspaceRecovery(ctx, name, args, result, binding);

  if (availableTools.includes(name) && !hookPreBlocked) {
    try {
      const post = await ctx.extensions.postToolUse(
        result.ok === false ? 'PostToolUseFailure' : 'PostToolUse',
        name,
        args,
        result,
        hookCwd,
        hookSessionId,
        binding?.folderId
      );
      if (hookContext.length) result.hook_context = hookContext;
      if (post.feedback.length) result.hook_feedback = post.feedback;
    } catch (error) {
      result.hook_feedback = [`Hook execution failed: ${error instanceof Error ? error.message : String(error)}`];
    }
  }

  const knowledgeCanaryTelemetry = knowledgeCanary ? {
    knowledge_canary_id: knowledgeCanary.knowledgeId,
    knowledge_canary_implementation: knowledgeCanary.implementation,
    knowledge_canary_eligible: true,
    knowledge_canary_stage: knowledgeCanary.stage,
    knowledge_canary_selected: knowledgeCanary.applied,
    knowledge_canary_applied: knowledgeCanary.applied === true && result.exact_total_fast_tail === true,
    knowledge_canary_bucket: knowledgeCanary.bucket
  } : undefined;

  const durationMs = Date.now() - startedAt;
  Object.assign(result, {
    execution_lane: runtimePolicy.lane,
    admission_lane: runtimePolicy.lane,
    admission_mode: runtimePolicy.admission,
    admission_scope: workspaceLane ? 'global_and_workspace' : globalLane ? 'global' : 'none',
    admission_queue_wait_ms: queueWaitMs,
    global_admission_wait_ms: globalAdmissionWaitMs,
    workspace_admission_wait_ms: workspaceAdmissionWaitMs,
    workspace_lock_wait_ms: lockWaitMs,
    duration_ms: durationMs,
    ...(binding?.folderId ? {
      requested_workspace_id: binding.requestedWorkspaceId ?? null,
      resolved_workspace_id: binding.folderId,
      workspace_route_source: binding.routeSource ?? 'conversation',
      workspace_route_changed: (binding.routeSource ?? 'conversation') !== 'conversation'
        && binding.selectedWorkspaceId !== binding.folderId,
      conversation_selection_changed: false
    } : {})
  });
  attachRecoveryMetadata(result, name, args, recovery, binding?.folderId);
  if (preflightObserved) addPhaseDuration(result, 'preflight_ms', preflightMs);
  if (harnessBeginObserved) addPhaseDuration(result, 'harness_begin_ms', harnessBeginMs);
  if (dispatchObserved) addPhaseDuration(result, 'dispatch_ms', dispatchMs);
  if (harnessFinishObserved) addPhaseDuration(result, 'harness_finish_ms', harnessFinishMs);
  result = normalizeToolResult(result);
  result = redaction.redact(result);
  recordConversationSessionRoutes(ctx, key, binding?.folderId, result);
  addPhaseDuration(result, 'serialization_ms', 0);
  const serializationStartedAt = performance.now();
  const serializedResult = JSON.stringify(result);
  const serializationMs = elapsedPhaseMs(serializationStartedAt);
  const resultPhases = result.phase_durations_ms as JsonObject;
  resultPhases.serialization_ms = serializationMs;
  const responseBytes = Buffer.byteLength(serializedResult);
  const folderId = binding?.folderId ?? selectedFolderId(ctx, key);
  if (requestTiming) {
    if (knowledgeCanaryTelemetry) Object.assign(result, knowledgeCanaryTelemetry);
    try {
      recordToolUsageAndLearn(ctx, key, folderId, {
        tool: name,
        arguments: telemetryArgs,
        result,
        startedTsMs: startedAt,
        durationMs,
        requestTiming,
        requestJsonBytes: Buffer.byteLength(JSON.stringify(telemetryArgs)),
        workspaceId: folderId
      });
    } finally {
      if (knowledgeCanaryTelemetry) {
        for (const field of Object.keys(knowledgeCanaryTelemetry)) delete result[field];
      }
    }
  }
  ctx.usage.push({ tool: name, startedAt, durationMs, ok: result.ok === true, queueWaitMs, lockWaitMs, responseBytes });
  if (ctx.usage.length > 5_000) ctx.usage.splice(0, 1_000);
  return result;
}
