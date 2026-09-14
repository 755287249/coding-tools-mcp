import test from 'node:test';
import assert from 'node:assert/strict';
import { createHash } from 'node:crypto';
import { syncToolEvolutionRuntimes } from '../dist/folderRuntime.js';
import { mkdir, mkdtemp, readFile, rm } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import path from 'node:path';
import {
  CanaryImpactStore,
  CanaryStrategyEngine,
  ExperienceCompiler,
  EvolvedSkillCanaryEngine,
  EvolvedSkillRegistry,
  KnowledgeIngestor,
  KnowledgeImpactStore,
  KnowledgeStore,
  ToolEvolutionExperimentStore,
  ToolEvolutionPlanner,
  ToolEvolutionProposalStore,
  ToolEvolutionBenchmarkCollector,
  ToolEvolutionCandidateClaimer,
  ToolEvolutionRegistry,
  ToolStrategyRegistry,
  ValidationGate,
  createToolEvolutionExperiment,
  materializeToolEvolutionProposal,
  evaluateCanaryPromotions,
  evaluateCanaryRollbacks,
  evaluateEvolvedSkillPromotions,
  evaluateEvolvedSkillRollbacks,
  EVOLVED_SKILL_CANARY_IMPLEMENTATION,
  knowledgeRecordId,
  listToolEvolutionResources,
  readToolEvolutionResource,
  promoteToolEvolutionExperiment,
  resolveToolStrategyImplementation,
  toolEvolutionExperimentId,
  toolEvolutionProposalId,
  toolEvolutionResourceUri,
  toolStrategyImplementationSnapshot,
  toolUsageRecordToObservation
} from '../dist/knowledge/index.js';

function observation(overrides = {}) {
  return {
    eventId: 'event-1',
    tool: 'search_text',
    outcome: 'success',
    durationMs: 100,
    requestBytes: 40,
    responseBytes: 80,
    timestampMs: 1_000,
    features: { calculate_total: true, files_considered: 100 },
    ...overrides
  };
}

function contextId(kind, value) {
  return createHash('sha256').update(`${kind}\0${value}`).digest('hex');
}

function record(overrides = {}) {
  const base = {
    id: '',
    schemaVersion: 1,
    scope: 'runtime',
    target: 'tool_strategy',
    status: 'candidate',
    trigger: { tool: 'search_text', calculate_total: true },
    hypothesis: 'Exact totals are expensive.',
    recommendedAction: { calculate_total: false },
    evidence: {
      observations: 1,
      successes: 1,
      failures: 0,
      totalDurationMs: 100,
      totalRequestBytes: 10,
      totalResponseBytes: 20,
      firstSeenAtMs: 1000,
      lastSeenAtMs: 1000
    },
    confidence: 1,
    sourceEventIds: ['event-1'],
    counterexampleEventIds: [],
    createdAtMs: 1000,
    updatedAtMs: 1000,
    ...overrides
  };
  base.id = knowledgeRecordId(base);
  return base;
}

function evolutionRecord(overrides = {}) {
  const evidence = overrides.evidence ?? {};
  const sourceEventIds = overrides.sourceEventIds ?? Array.from({ length: 5 }, (_, index) => `evolution-event-${index + 1}`);
  return record({
    ...overrides,
    target: 'tool_evolution',
    evidence: {
      observations: 5,
      successes: 5,
      failures: 0,
      totalDurationMs: 500,
      totalRequestBytes: 50,
      totalResponseBytes: 100,
      firstSeenAtMs: 1000,
      lastSeenAtMs: 1004,
      conversationContextIds: [contextId('conversation', 'fixture-a'), contextId('conversation', 'fixture-b')],
      ...evidence
    },
    sourceEventIds
  });
}

test('knowledgeRecordId is deterministic across object key order', () => {
  const first = record({ trigger: { tool: 'search_text', calculate_total: true } });
  const second = record({ trigger: { calculate_total: true, tool: 'search_text' } });
  assert.equal(first.id, second.id);
});

test('ExperienceCompiler emits bounded tool strategy knowledge and does not retain unknown features', () => {
  const compiler = new ExperienceCompiler();
  const observations = Array.from({ length: 5 }, (_, index) => observation({
    eventId: `event-${index + 1}`,
    timestampMs: 1000 + index,
    features: {
      calculate_total: true,
      files_considered: 1000 + index,
      secret_token: 'must-not-survive',
      path: 'C:/private/workspace'
    }
  }));
  const records = compiler.compile(observations);
  assert.equal(records.length, 1, 'metric variation must not fragment one semantic tool-use pattern');
  const [item] = records;
  assert.equal(item.target, 'tool_strategy');
  assert.equal(item.status, 'candidate');
  assert.equal(item.evidence.observations, 5);
  assert.equal(item.trigger.calculate_total, true);
  assert.equal(item.recommendedAction.calculate_total, undefined, 'learned knowledge must not silently rewrite explicit exact-count intent');
  assert.equal(item.recommendedAction.caller_intent_required, 'bounded_results_only');
  assert.equal(item.recommendedAction.semantic_preserving_implementation, 'search_exact_total_fast_tail');
  assert.equal('files_considered' in item.trigger, false);
  assert.equal('secret_token' in item.trigger, false);
  assert.equal('path' in item.trigger, false);
  assert.match(item.id, /^[a-f0-9]{64}$/);
});

test('ExperienceCompiler requires independent context evidence before promoting MCP implementation cost', () => {
  const compiler = new ExperienceCompiler();
  const repeatedSameContext = Array.from({ length: 5 }, (_, index) => observation({
    eventId: `project-${index + 1}`,
    tool: 'project_state',
    conversationContextId: 'conversation-a',
    timestampMs: 2000 + index,
    durationMs: 10_000,
    features: { phase_baseline_capture_ms: 9_000 }
  }));
  const [observed] = compiler.compile(repeatedSameContext);
  assert.equal(observed.target, 'tool_evolution');
  assert.equal(observed.status, 'observed');
  assert.equal(observed.evidence.observations, 5);
  assert.equal(observed.evidence.conversationContextIds.length, 1);

  const diversified = repeatedSameContext.map((item, index) => ({
    ...item,
    conversationContextId: index < 3 ? 'conversation-a' : 'conversation-b'
  }));
  const [candidate] = compiler.compile(diversified);
  assert.equal(candidate.status, 'candidate');
  assert.equal(candidate.evidence.conversationContextIds.length, 2);
  assert.equal(candidate.evidence.conversationContextIds.includes('conversation-a'), false, 'persisted knowledge must hash raw context identifiers');
  assert.equal(candidate.metadata.proposalType, 'performance');
  assert.equal(candidate.recommendedAction.proposal, 'incremental_or_cached_baseline');
});

test('ExperienceCompiler proposes bounded evolved Skill guidance only after independent attributed contexts', async () => {
  const compiler = new ExperienceCompiler();
  const skill = { source: 'project', name: 'release-helper', contentSha256: 'a'.repeat(64), generation: 0 };
  const sameContext = Array.from({ length: 5 }, (_, index) => observation({
    eventId: `skill-edit-${index + 1}`,
    tool: 'edit',
    outcome: 'failure',
    errorCode: 'FILE_VERSION_MISMATCH',
    conversationContextId: 'skill-conversation-a',
    timestampMs: 2500 + index,
    skill
  }));
  const observed = compiler.compile(sameContext).find(item => item.target === 'evolved_skill');
  assert.ok(observed);
  assert.equal(observed.status, 'observed');
  assert.equal(observed.scope, 'workspace');
  assert.equal(observed.metadata.name, 'release-helper');
  assert.deepEqual(observed.metadata.base, { source: skill.source, name: skill.name, contentSha256: skill.contentSha256 });
  assert.equal(observed.metadata.generation, 1);
  assert.deepEqual(Object.keys(observed.recommendedAction), ['append_guidance']);
  assert.match(observed.recommendedAction.append_guidance[0], /re-read the current file/i);
  assert.equal(JSON.stringify(observed).includes('Skill:'), false);

  const diversified = sameContext.map((item, index) => ({
    ...item,
    conversationContextId: index < 3 ? 'skill-conversation-a' : 'skill-conversation-b'
  }));
  const candidate = compiler.compile(diversified).find(item => item.target === 'evolved_skill');
  assert.ok(candidate);
  assert.equal(candidate.status, 'candidate');
  assert.equal(candidate.evidence.conversationContextIds.length, 2);

  const withoutAttribution = compiler.compile(diversified.map(({ skill: _skill, ...item }) => item));
  assert.equal(withoutAttribution.some(item => item.target === 'evolved_skill'), false);

  const root = await mkdtemp(path.join(tmpdir(), 'ctmcp-evolved-compiler-'));
  const store = new KnowledgeStore(root);
  await store.upsert(candidate);
  const registry = await new EvolvedSkillRegistry(store).snapshot({ includeNonPromoted: true });
  assert.equal(registry.skills.length, 1);
  assert.equal(registry.skills[0].status, 'candidate');
  assert.deepEqual(registry.skills[0].overlay, candidate.recommendedAction);
});
test('KnowledgeIngestor moves evolved Skill candidates into shadow without exposing them as effective promoted skills', async () => {
  const root = await mkdtemp(path.join(tmpdir(), 'ctmcp-evolved-ingestor-'));
  const store = new KnowledgeStore(root);
  const impact = new KnowledgeImpactStore(root);
  const ingestor = new KnowledgeIngestor(store, impact, 'repo');
  const skill = { source: 'project', name: 'release-helper', contentSha256: 'b'.repeat(64), generation: 0 };
  for (let index = 0; index < 5; index += 1) {
    ingestor.enqueueToolUsage({
      event_type: 'tool_call',
      tool: 'edit',
      diagnostic_event_id: `evolved-ingest-${index}`,
      selected_workspace_id: 'repo',
      outcome: 'tool_error',
      error_code: 'EDIT_MATCH_COUNT_MISMATCH',
      duration_ms: 10,
      completed_ts_ms: 3000 + index
    }, {
      conversationContextId: index < 3 ? 'evolved-conversation-a' : 'evolved-conversation-b',
      skill
    });
  }
  await ingestor.flush();
  const evolved = (await store.list()).filter(item => item.target === 'evolved_skill');
  assert.equal(evolved.length, 1);
  assert.equal(evolved[0].status, 'shadow');
  assert.equal((await new EvolvedSkillRegistry(store).snapshot()).skills.length, 0);
  const all = await new EvolvedSkillRegistry(store).snapshot({ includeNonPromoted: true });
  assert.equal(all.skills.length, 1);
  assert.equal(all.skills[0].status, 'shadow');
});
test('ExperienceCompiler classifies broad non-exact search scans as tool evolution rather than caller misuse', () => {
  const compiler = new ExperienceCompiler();
  const observations = Array.from({ length: 5 }, (_, index) => observation({
    eventId: `broad-search-${index + 1}`,
    conversationContextId: `broad-conversation-${index % 2}`,
    timestampMs: 3000 + index,
    durationMs: 25_000,
    features: {
      calculate_total: false,
      files_considered: 8_000 + index,
      scanned_files: 7_900 + index,
      returned_count: 4,
      scan_excluded_worktree_container_count: 0
    }
  }));
  const [candidate] = compiler.compile(observations);
  assert.equal(candidate.target, 'tool_evolution');
  assert.equal(candidate.status, 'candidate');
  assert.equal(candidate.metadata.proposalType, 'performance');
  assert.equal(candidate.recommendedAction.proposal, 'exclude_untracked_worktree_management_containers');
  assert.equal('files_considered' in candidate.trigger, false);
  assert.equal('scan_excluded_worktree_container_count' in candidate.trigger, false);
});

test('ExperienceCompiler classifies high successful edit Harness overhead as tool evolution', () => {
  const compiler = new ExperienceCompiler();
  const observations = Array.from({ length: 5 }, (_, index) => observation({
    eventId: `edit-harness-${index + 1}`,
    tool: 'edit',
    runtimeBootId: `runtime-boot-${index % 2}`,
    timestampMs: 4000 + index,
    durationMs: 2_500,
    features: {
      phase_harness_begin_ms: 1_100,
      phase_harness_finish_ms: 1_200,
      baseline_refresh_mode: 'full',
      baseline_refresh_file_count: 0
    }
  }));
  const [candidate] = compiler.compile(observations);
  assert.equal(candidate.target, 'tool_evolution');
  assert.equal(candidate.status, 'candidate');
  assert.equal(candidate.metadata.proposalType, 'performance');
  assert.equal(candidate.recommendedAction.proposal, 'incremental_post_edit_baseline_refresh');
  assert.equal('phase_harness_begin_ms' in candidate.trigger, false);
  assert.equal('baseline_refresh_mode' in candidate.trigger, false);
});

test('ExperienceCompiler classifies repeated retained exec_many polling as tool evolution without penalizing the initial detach', () => {
  const compiler = new ExperienceCompiler();
  const repeatedReattachments = Array.from({ length: 5 }, (_, index) => observation({
    eventId: `exec-many-reattach-${index + 1}`,
    tool: 'exec_many',
    taskId: `exec-many-task-${index % 3}`,
    timestampMs: 5000 + index,
    durationMs: 30_100,
    features: {
      graph_action: 'run',
      detached: true,
      reattached: true,
      graph_yield_ms: 30_000,
      graph_wait_ms: 29_950 + index
    }
  }));
  const [candidate] = compiler.compile(repeatedReattachments);
  assert.equal(candidate.target, 'tool_evolution');
  assert.equal(candidate.status, 'candidate');
  assert.equal(candidate.metadata.proposalType, 'performance');
  assert.equal(candidate.recommendedAction.proposal, 'adaptive_retained_graph_reattach_wait');
  assert.equal(candidate.recommendedAction.preserve_initial_responsiveness, true);
  assert.equal(candidate.trigger.graph_action, 'run');
  assert.equal(candidate.trigger.detached, true);
  assert.equal(candidate.trigger.reattached, true);
  assert.equal('graph_wait_ms' in candidate.trigger, false);
  assert.equal('graph_yield_ms' in candidate.trigger, false);

  const initialDetaches = repeatedReattachments.map((item, index) => ({
    ...item,
    eventId: `exec-many-initial-${index + 1}`,
    features: { ...item.features, reattached: false }
  }));
  assert.deepEqual(compiler.compile(initialDetaches), []);
});

test('KnowledgeStore merges evidence without duplicating source event ids', async () => {
  const root = await mkdtemp(path.join(tmpdir(), 'ctmcp-knowledge-'));
  const store = new KnowledgeStore(root);
  const first = record();
  await store.upsert(first);
  const replayed = await store.upsert(first);
  assert.equal(replayed.evidence.observations, 1, 'replaying the same event batch must be idempotent');

  const second = record({
    evidence: { ...first.evidence, observations: 1, successes: 0, failures: 1, firstSeenAtMs: 1100, lastSeenAtMs: 1100 },
    sourceEventIds: ['event-2'],
    updatedAtMs: 1100
  });
  const merged = await store.upsert(second);
  assert.equal(merged.evidence.observations, 2);
  assert.equal(merged.evidence.successes, 1);
  assert.equal(merged.evidence.failures, 1);
  assert.deepEqual(merged.sourceEventIds, ['event-1', 'event-2']);

  await assert.rejects(
    store.upsert(record({
      evidence: { ...first.evidence, observations: 2 },
      sourceEventIds: ['event-2', 'event-3'],
      updatedAtMs: 1200
    })),
    /must be disjoint or exact replays/
  );
  assert.equal((await store.list()).length, 1);
  assert.match(await store.revision(), /^[a-f0-9]{64}$/);
});


test('KnowledgeStore requires independent Tool Evolution context across evidence batches and bounds identifiers', async () => {
  const root = await mkdtemp(path.join(tmpdir(), 'ctmcp-knowledge-diversity-'));
  const store = new KnowledgeStore(root);
  const semantic = {
    target: 'tool_evolution',
    trigger: { tool: 'project_state', variant: 'context-diversity' },
    hypothesis: 'Independent contexts are required before changing MCP implementation.',
    recommendedAction: { proposal: 'context-diversity-check' },
    metadata: { proposalType: 'performance' }
  };
  const first = record({
    ...semantic,
    status: 'candidate',
    evidence: {
      observations: 3, successes: 3, failures: 0,
      totalDurationMs: 300, totalRequestBytes: 30, totalResponseBytes: 60,
      firstSeenAtMs: 1, lastSeenAtMs: 3,
      conversationContextIds: [contextId('conversation', 'first')]
    },
    sourceEventIds: ['diverse-1', 'diverse-2', 'diverse-3']
  });
  const observed = await store.upsert(first);
  assert.equal(observed.status, 'observed');
  assert.equal(observed.evidence.conversationContextIds.length, 1);

  const second = record({
    ...semantic,
    status: 'candidate',
    evidence: {
      observations: 2, successes: 2, failures: 0,
      totalDurationMs: 200, totalRequestBytes: 20, totalResponseBytes: 40,
      firstSeenAtMs: 4, lastSeenAtMs: 5,
      conversationContextIds: [contextId('conversation', 'second')]
    },
    sourceEventIds: ['diverse-4', 'diverse-5'],
    updatedAtMs: 5
  });
  const candidate = await store.upsert(second);
  assert.equal(candidate.status, 'candidate');
  assert.equal(candidate.evidence.observations, 5);
  assert.equal(candidate.evidence.conversationContextIds.length, 2);

  const bounded = await store.upsert(evolutionRecord({
    trigger: { tool: 'project_state', variant: 'bounded-contexts' },
    hypothesis: 'Context evidence storage is bounded.',
    recommendedAction: { proposal: 'bounded-contexts' },
    evidence: {
      taskContextIds: Array.from({ length: 300 }, (_, index) => contextId('task', `task-${index}`))
    }
  }));
  assert.equal(bounded.evidence.taskContextIds.length, 256);
  await assert.rejects(
    store.upsert(evolutionRecord({
      trigger: { tool: 'project_state', variant: 'raw-context-rejected' },
      hypothesis: 'Raw context identifiers must not persist.',
      recommendedAction: { proposal: 'raw-context-rejected' },
      evidence: { conversationContextIds: ['raw-conversation-id'] }
    })),
    /SHA-256 context identifiers/
  );
});

test('telemetry adapter carries only bounded Skill provenance from explicit attribution', () => {
  const record = {
    event_type: 'tool_call', tool: 'read_file', diagnostic_event_id: 'skill-event', selected_workspace_id: 'repo',
    outcome: 'success', duration_ms: 5, completed_ts_ms: 10, runtime_boot_id: 'boot'
  };
  const skill = { source: 'project', name: 'demo', contentSha256: 'a'.repeat(64), generation: 2 };
  const observed = toolUsageRecordToObservation(record, 'repo', { skill });
  assert.deepEqual(observed.skill, skill);
  assert.equal(JSON.stringify(observed).includes('prompt'), false);
});
test('telemetry adapter retains only bounded scalar learning signals', () => {
  const observation = toolUsageRecordToObservation({
    event_type: 'tool_call',
    diagnostic_event_id: 'diag-1',
    selected_workspace_id: 'repo',
    tool: 'edit',
    outcome: 'tool_error',
    duration_ms: 42,
    request_json_bytes: 300,
    response_json_bytes: 500,
    completed_ts_ms: 5000,
    error_code: 'FILE_VERSION_MISMATCH',
    recovery_attempt: true,
    recovery_succeeded: false,
    error_direct_recovery_action_count: 2,
    verification_ok: false,
    arguments: { path: 'private/file.txt', token: 'secret' },
    command_preview: 'must not survive'
  }, 'repo', { operationId: 'op-1', taskId: 'task-1', conversationContextId: 'conversation-private' });
  assert.equal(observation.eventId, 'diag-1');
  assert.equal(observation.errorCode, 'FILE_VERSION_MISMATCH');
  assert.equal(observation.outcome, 'failure');
  assert.equal(observation.verificationOutcome, 'failed');
  assert.equal(observation.recoveryOutcome, 'failed');
  assert.equal(observation.features.retry_attempt, true);
  assert.equal(observation.features.error_direct_recovery_action_count, 2);
  assert.equal(observation.operationId, 'op-1');
  assert.equal(observation.taskId, 'task-1');
  assert.equal(observation.conversationContextId, 'conversation-private');
  assert.equal('arguments' in observation, false);
  assert.equal('command_preview' in observation, false);
});

test('telemetry adapter retains only bounded exec_many graph polling signals', () => {
  const observation = toolUsageRecordToObservation({
    event_type: 'tool_call',
    diagnostic_event_id: 'diag-exec-many',
    selected_workspace_id: 'repo',
    tool: 'exec_many',
    outcome: 'success',
    duration_ms: 30_100,
    completed_ts_ms: 6000,
    graph_action: 'run',
    detached: true,
    reattached: true,
    graph_yield_ms: 30_000,
    graph_wait_ms: 30_001,
    commands: [{ program: 'private-command', args: ['secret'] }],
    results: [{ stdout: 'must not survive' }]
  }, 'repo');
  assert.deepEqual(observation.features, {
    graph_action: 'run',
    detached: true,
    reattached: true,
    graph_yield_ms: 30_000,
    graph_wait_ms: 30_001
  });
  assert.equal('commands' in observation, false);
  assert.equal('results' in observation, false);
});

test('KnowledgeIngestor activates candidates for held-out shadow measurement without applying them', async () => {
  const root = await mkdtemp(path.join(tmpdir(), 'ctmcp-knowledge-ingest-'));
  const store = new KnowledgeStore(root);
  const impactStore = new KnowledgeImpactStore(root);
  const ingestor = new KnowledgeIngestor(store, impactStore, 'repo');
  for (let index = 0; index < 5; index += 1) {
    ingestor.enqueueToolUsage({
      event_type: 'tool_call',
      diagnostic_event_id: `diag-${index}`,
      selected_workspace_id: 'repo',
      tool: 'search_text',
      outcome: 'success',
      duration_ms: 1000 + index,
      request_json_bytes: 100,
      response_json_bytes: 200,
      completed_ts_ms: 10_000 + index,
      calculate_total: true,
      files_considered: 1000 + index,
      arguments: { query: 'private query text' }
    });
  }
  ingestor.enqueueToolUsage({
    event_type: 'tool_call', diagnostic_event_id: 'foreign', selected_workspace_id: 'other',
    tool: 'search_text', outcome: 'success', duration_ms: 1, completed_ts_ms: 20_000, calculate_total: true
  });
  await ingestor.flush();
  let [learned] = await store.list();
  assert.equal(learned.target, 'tool_strategy');
  assert.equal(learned.status, 'shadow');
  assert.equal(learned.evidence.observations, 5);
  assert.equal(learned.confidence, 0.5);
  assert.deepEqual(learned.sourceEventIds, ['diag-0', 'diag-1', 'diag-2', 'diag-3', 'diag-4']);
  assert.deepEqual(await impactStore.list(), [], 'training evidence must not be reused as shadow evidence');

  ingestor.enqueueToolUsage({
    event_type: 'tool_call', diagnostic_event_id: 'heldout-expensive', selected_workspace_id: 'repo',
    tool: 'search_text', outcome: 'success', duration_ms: 2500, completed_ts_ms: 30_000,
    request_json_bytes: 110, response_json_bytes: 210, calculate_total: true, files_considered: 900
  });
  await ingestor.flush();
  [learned] = await store.list();
  const [impact] = await impactStore.list();
  assert.equal(learned.status, 'shadow', 'new automatic evidence must not downgrade shadow state');
  assert.equal(learned.evidence.observations, 6);
  assert.equal(learned.confidence, 0.5455);
  assert.equal(impact.considered, 1);
  assert.equal(impact.matched, 1);
  assert.equal(impact.actualSuccesses, 1);
  assert.equal(impact.totalObservedDurationMs, 2500);
  assert.deepEqual(impact.sourceEventIds, ['heldout-expensive']);

  ingestor.enqueueToolUsage({
    event_type: 'tool_call', diagnostic_event_id: 'heldout-cheap', selected_workspace_id: 'repo',
    tool: 'search_text', outcome: 'success', duration_ms: 10, completed_ts_ms: 31_000,
    calculate_total: true, files_considered: 10
  });
  await ingestor.flush();
  const [impactAfterCheap] = await impactStore.list();
  assert.equal(impactAfterCheap.considered, 2);
  assert.equal(impactAfterCheap.matched, 1, 'cheap exact-count calls are observed but not treated as policy matches');

  const snapshot = ingestor.snapshot();
  assert.equal(snapshot.processed, 7);
  assert.equal(snapshot.matched, 6);
  assert.equal(snapshot.ignored, 2);
  assert.equal(snapshot.shadowConsidered, 2);
  assert.equal(snapshot.shadowMatched, 1);
  assert.equal(snapshot.shadowActivated, 1);
  assert.equal(snapshot.failed, 0);
  assert.equal(snapshot.queued, 0);
});

test('task-level held-out credit validates a strategy only after enough verified task outcomes', async () => {
  const root = await mkdtemp(path.join(tmpdir(), 'ctmcp-knowledge-validation-'));
  const store = new KnowledgeStore(root);
  const impactStore = new KnowledgeImpactStore(root);
  const ingestor = new KnowledgeIngestor(store, impactStore, 'repo');
  for (let index = 0; index < 5; index += 1) {
    ingestor.enqueueToolUsage({
      event_type: 'tool_call', diagnostic_event_id: `train-${index}`, selected_workspace_id: 'repo',
      tool: 'search_text', outcome: 'success', duration_ms: 1500, completed_ts_ms: 1000 + index,
      calculate_total: true, files_considered: 800
    });
  }
  await ingestor.flush();
  assert.equal((await store.list())[0].status, 'shadow');

  let sequence = 0;
  const taskSizes = [4, 3, 3];
  for (let taskIndex = 0; taskIndex < taskSizes.length; taskIndex += 1) {
    const taskId = `task-${taskIndex + 1}`;
    for (let item = 0; item < taskSizes[taskIndex]; item += 1) {
      sequence += 1;
      const terminal = item === taskSizes[taskIndex] - 1;
      ingestor.enqueueToolUsage({
        event_type: 'tool_call', diagnostic_event_id: `heldout-${sequence}`, selected_workspace_id: 'repo',
        tool: 'search_text', outcome: 'success', duration_ms: 1800, completed_ts_ms: 10_000 + sequence,
        calculate_total: true, files_considered: 900
      }, {
        taskId,
        ...(terminal ? { taskOutcome: 'verified_success' } : {})
      });
    }
    await ingestor.flush();
  }

  const [validated] = await store.list();
  const [impact] = await impactStore.list();
  assert.equal(validated.status, 'validated');
  assert.equal(impact.matched, 10);
  assert.deepEqual(impact.matchedTaskIds, ['task-1', 'task-2', 'task-3']);
  assert.deepEqual(impact.creditedTaskIds, ['task-1', 'task-2', 'task-3']);
  assert.equal(impact.taskVerifiedSuccesses, 3);
  assert.equal(impact.taskFailures, 0);
  assert.equal(impact.taskRollbacks, 0);
  const snapshot = ingestor.snapshot();
  assert.equal(snapshot.taskCredits, 3);
  assert.equal(snapshot.validationValidated, 1);
});

test('task credit is idempotent and rollback blocks validation', async () => {
  const root = await mkdtemp(path.join(tmpdir(), 'ctmcp-knowledge-rollback-'));
  const store = new KnowledgeStore(root);
  const impactStore = new KnowledgeImpactStore(root);
  const shadow = await store.upsert(record({
    status: 'shadow',
    evidence: {
      observations: 15, successes: 15, failures: 0,
      totalDurationMs: 15_000, totalRequestBytes: 150, totalResponseBytes: 300,
      firstSeenAtMs: 1, lastSeenAtMs: 15
    },
    sourceEventIds: Array.from({ length: 15 }, (_, index) => `seed-${index}`),
    updatedAtMs: 15
  }));
  await impactStore.record({
    schemaVersion: 1,
    knowledgeId: shadow.id,
    target: 'tool_strategy',
    tool: 'search_text',
    sourceEventIds: Array.from({ length: 10 }, (_, index) => `heldout-${index}`),
    matchedTaskIds: ['task-a', 'task-b', 'task-c'],
    creditedTaskIds: [],
    considered: 10,
    matched: 10,
    actualSuccesses: 10,
    actualFailures: 0,
    verifiedSuccesses: 0,
    verificationFailures: 0,
    recoverySuccesses: 0,
    recoveryFailures: 0,
    taskVerifiedSuccesses: 0,
    taskUnverifiedSuccesses: 0,
    taskFailures: 0,
    taskRollbacks: 0,
    totalObservedDurationMs: 15_000,
    totalObservedRequestBytes: 100,
    totalObservedResponseBytes: 200,
    firstSeenAtMs: 100,
    lastSeenAtMs: 109
  });
  assert.equal(await impactStore.creditTaskOutcome('task-a', 'verified_success', 110), 1);
  assert.equal(await impactStore.creditTaskOutcome('task-a', 'verified_success', 111), 0);
  assert.equal(await impactStore.creditTaskOutcome('task-b', 'verified_success', 112), 1);
  assert.equal(await impactStore.creditTaskOutcome('task-c', 'rolled_back', 113), 1);
  const [impact] = await impactStore.list();
  assert.equal(impact.taskVerifiedSuccesses, 2);
  assert.equal(impact.taskRollbacks, 1);
  assert.deepEqual(impact.creditedTaskIds, ['task-a', 'task-b', 'task-c']);

  const gate = await new ValidationGate(store, impactStore).evaluate(114);
  assert.equal(gate.evaluated, 1);
  assert.equal(gate.validated, 0);
  assert.equal(gate.blocked, 1);
  assert.equal((await store.get(shadow.id)).status, 'shadow');
});

test('tool strategy implementation registry exposes only semantics-preserving runtime implementations', () => {
  const snapshot = toolStrategyImplementationSnapshot();
  assert.equal(snapshot.count, 1);
  assert.match(snapshot.revision, /^[a-f0-9]{64}$/);
  assert.deepEqual(snapshot.implementations, [{
    id: 'search_exact_total_fast_tail',
    tool: 'search_text',
    safety: 'semantics_preserving'
  }]);
  const safe = record({
    status: 'validated',
    recommendedAction: {
      recommendation: 'prefer_bounded_search',
      semantic_preserving_implementation: 'search_exact_total_fast_tail'
    }
  });
  const resolved = resolveToolStrategyImplementation(safe, 'search_text', {
    query: 'needle',
    calculate_total: true
  });
  assert.equal(resolved?.id, 'search_exact_total_fast_tail');
  assert.equal(resolved?.safety, 'semantics_preserving');
  assert.equal(resolveToolStrategyImplementation(safe, 'search_text', {
    query: 'needle',
    calculate_total: false
  }), undefined);
  assert.equal(resolveToolStrategyImplementation(safe, 'edit', {
    path: 'file.txt'
  }), undefined);
});

test('validated search canaries are deterministic and never rewrite caller exact-count intent', async () => {
  const root = await mkdtemp(path.join(tmpdir(), 'ctmcp-knowledge-canary-decision-'));
  const store = new KnowledgeStore(root);
  const impactStore = new CanaryImpactStore(root);
  const validated = await store.upsert(record({
    status: 'validated',
    recommendedAction: { recommendation: 'prefer_bounded_search', calculate_total: false }
  }));
  const engine = new CanaryStrategyEngine(store, impactStore);
  const args = { query: 'needle', calculate_total: true, max_results: 1 };
  const first = await engine.decide('search_text', args, 'conversation-a');
  const replay = await engine.decide('search_text', args, 'conversation-a');
  assert.ok(first);
  assert.deepEqual(replay, first);
  assert.equal(first.knowledgeId, validated.id);
  assert.equal(first.implementation, 'search_exact_total_fast_tail');
  assert.equal(first.stage, 'canary');
  assert.equal(args.calculate_total, true, 'canary selection must not mutate explicit caller intent');
  assert.equal(await engine.decide('search_text', { query: 'needle', calculate_total: false }, 'conversation-a'), undefined);
  assert.equal(await engine.decide('search_text', { query: 'needle', calculate_total: true, count_only: true }, 'conversation-a'), undefined);

  const cohorts = [];
  for (let index = 0; index < 100; index += 1) {
    cohorts.push(await engine.decide('search_text', args, `conversation-${index}`));
  }
  assert.ok(cohorts.some(item => item?.applied === true));
  assert.ok(cohorts.some(item => item?.applied === false));
});

test('evolved Skill lifecycle requires held-out task evidence, uses session canary, and rolls back regressions', async () => {
  const root = await mkdtemp(path.join(tmpdir(), 'ctmcp-evolved-skill-canary-'));
  const store = new KnowledgeStore(root);
  const shadowImpactStore = new KnowledgeImpactStore(root);
  const skill = { source: 'project', name: 'release-helper', contentSha256: 'a'.repeat(64), generation: 0 };
  const ingestor = new KnowledgeIngestor(store, shadowImpactStore, 'repo');
  for (let index = 0; index < 5; index += 1) {
    ingestor.enqueueToolUsage({
      event_type: 'tool_call', diagnostic_event_id: `evolved-train-${index}`, selected_workspace_id: 'repo',
      tool: 'edit', outcome: 'tool_error', error_code: 'FILE_VERSION_MISMATCH', duration_ms: 100, completed_ts_ms: 100 + index
    }, { conversationContextId: index < 3 ? 'train-a' : 'train-b', skill });
  }
  await ingestor.flush();
  let [learned] = (await store.list()).filter(item => item.target === 'evolved_skill');
  assert.ok(learned);
  assert.equal(learned.status, 'shadow');

  for (let index = 0; index < 10; index += 1) {
    ingestor.enqueueToolUsage({
      event_type: 'tool_call', diagnostic_event_id: `evolved-shadow-${index}`, selected_workspace_id: 'repo',
      tool: 'edit', outcome: 'tool_error', error_code: 'EDIT_MATCH_COUNT_MISMATCH', duration_ms: 90, completed_ts_ms: 200 + index
    }, {
      conversationContextId: `held-out-${index % 3}`, skill,
      ...(index < 3 ? { taskId: `evolved-shadow-task-${index}`, taskOutcome: 'verified_success' } : {})
    });
  }
  await ingestor.flush();
  learned = await store.get(learned.id);
  assert.equal(learned.status, 'validated');
  const shadowImpact = (await shadowImpactStore.list()).find(item => item.knowledgeId === learned.id);
  assert.ok(shadowImpact);
  assert.equal(shadowImpact.matched, 10);
  assert.equal(shadowImpact.actualFailures, 10, 'trigger failures are applicability evidence, not a validation failure-rate gate');
  assert.equal(shadowImpact.taskVerifiedSuccesses, 3);

  const canaryImpactStore = new CanaryImpactStore(root);
  const engine = new EvolvedSkillCanaryEngine(store, canaryImpactStore);
  const baseSkill = { source: skill.source, name: skill.name, contentSha256: skill.contentSha256 };
  const decisions = [];
  for (let index = 0; index < 100; index += 1) decisions.push(await engine.decide(baseSkill, `evolved-session-${index}`));
  assert.ok(decisions.every(item => item?.implementation === EVOLVED_SKILL_CANARY_IMPLEMENTATION));
  assert.ok(decisions.some(item => item?.applied === true));
  assert.ok(decisions.some(item => item?.applied === false));

  const canaryObservations = [];
  for (let index = 0; index < 10; index += 1) {
    canaryObservations.push(observation({
      eventId: `evolved-control-${index}`, tool: 'edit', outcome: index < 4 ? 'failure' : 'success', durationMs: 1_000,
      timestampMs: 400 + index, canary: { knowledgeId: learned.id, implementation: EVOLVED_SKILL_CANARY_IMPLEMENTATION, eligible: true, applied: false, bucket: 50 + index }
    }));
  }
  for (let index = 0; index < 20; index += 1) {
    canaryObservations.push(observation({
      eventId: `evolved-intervention-${index}`, tool: 'edit', outcome: index < 2 ? 'failure' : 'success', durationMs: 900,
      timestampMs: 500 + index, ...(index < 3 ? { taskId: `evolved-promote-task-${index}` } : {}),
      canary: { knowledgeId: learned.id, implementation: EVOLVED_SKILL_CANARY_IMPLEMENTATION, eligible: true, applied: true, bucket: index }
    }));
  }
  assert.equal(await canaryImpactStore.recordObservations(canaryObservations), 30);
  for (let index = 0; index < 3; index += 1) {
    assert.equal(await canaryImpactStore.creditTaskOutcome(`evolved-promote-task-${index}`, 'verified_success', 600 + index), 1);
  }
  assert.deepEqual(await evaluateEvolvedSkillPromotions(store, canaryImpactStore, 700), { evaluated: 1, promoted: 1 });
  learned = await store.get(learned.id);
  assert.equal(learned.status, 'promoted');
  const promotedDecision = await engine.decide({ ...baseSkill, evolution: { knowledgeId: learned.id, generation: 1, baseContentSha256: skill.contentSha256 } }, 'after-promotion');
  assert.equal(promotedDecision.stage, 'promoted');
  assert.equal(promotedDecision.applied, true);
  assert.equal(promotedDecision.alreadyApplied, true);

  assert.equal(await canaryImpactStore.recordObservations([observation({
    eventId: 'evolved-post-promotion', tool: 'edit', outcome: 'failure', durationMs: 900, taskId: 'evolved-post-task', timestampMs: 800,
    canary: { knowledgeId: learned.id, implementation: EVOLVED_SKILL_CANARY_IMPLEMENTATION, eligible: true, applied: true, bucket: 3 }
  })]), 1);
  assert.equal(await canaryImpactStore.creditTaskOutcome('evolved-post-task', 'rolled_back', 801), 1);
  assert.deepEqual(await evaluateEvolvedSkillRollbacks(store, canaryImpactStore, 802), { evaluated: 1, rolledBack: 1 });
  assert.equal((await store.get(learned.id)).status, 'rejected');
  assert.equal(await engine.decide(baseSkill, 'after-rollback'), undefined);
});
test('successful canary promotes to 100 percent rollout and post-promotion rollback disables it', async () => {
  const root = await mkdtemp(path.join(tmpdir(), 'ctmcp-knowledge-canary-promotion-'));
  const store = new KnowledgeStore(root);
  const shadowImpactStore = new KnowledgeImpactStore(root);
  const canaryImpactStore = new CanaryImpactStore(root);
  const validated = await store.upsert(record({
    status: 'validated',
    recommendedAction: { recommendation: 'prefer_bounded_search', semantic_preserving_implementation: 'search_exact_total_fast_tail' }
  }));
  const observations = [];
  for (let index = 0; index < 10; index += 1) {
    observations.push(observation({
      eventId: `control-${index}`,
      durationMs: 1_000,
      timestampMs: 100 + index,
      ...(index === 0 ? { taskId: 'cross-promotion-task' } : {}),
      canary: {
        knowledgeId: validated.id,
        implementation: 'search_exact_total_fast_tail',
        eligible: true,
        applied: false,
        bucket: 50 + index
      }
    }));
  }
  for (let index = 0; index < 20; index += 1) {
    observations.push(observation({
      eventId: `intervention-${index}`,
      durationMs: 600,
      timestampMs: 200 + index,
      ...(index < 3 ? { taskId: `promote-task-${index}` } : {}),
      canary: {
        knowledgeId: validated.id,
        implementation: 'search_exact_total_fast_tail',
        eligible: true,
        applied: true,
        bucket: index
      }
    }));
  }
  assert.equal(await canaryImpactStore.recordObservations(observations), 30);
  for (let index = 0; index < 3; index += 1) {
    assert.equal(await canaryImpactStore.creditTaskOutcome(`promote-task-${index}`, 'verified_success', 300 + index), 1);
  }

  const promotion = await evaluateCanaryPromotions(store, canaryImpactStore, 400);
  assert.deepEqual(promotion, { evaluated: 1, promoted: 1 });
  assert.equal((await store.get(validated.id)).status, 'promoted');
  let [impact] = await canaryImpactStore.list();
  assert.equal(impact.promotedAtMs, 400);
  assert.equal(impact.promotionInterventionCalls, 20);
  assert.equal(impact.promotionInterventionTaskVerifiedSuccesses, 3);

  const promotedEngine = new CanaryStrategyEngine(store, canaryImpactStore);
  for (let index = 0; index < 100; index += 1) {
    const decision = await promotedEngine.decide(
      'search_text',
      { query: 'needle', calculate_total: true },
      `post-promotion-conversation-${index}`
    );
    assert.ok(decision);
    assert.equal(decision.stage, 'promoted');
    assert.equal(decision.applied, true);
  }

  assert.equal(await canaryImpactStore.recordObservations([observation({
    eventId: 'cross-promotion-intervention',
    durationMs: 620,
    taskId: 'cross-promotion-task',
    timestampMs: 450,
    canary: {
      knowledgeId: validated.id,
      implementation: 'search_exact_total_fast_tail',
      eligible: true,
      applied: true,
      bucket: 88
    }
  })]), 1);
  assert.equal(await canaryImpactStore.creditTaskOutcome('cross-promotion-task', 'verified_success', 451), 1);
  [impact] = await canaryImpactStore.list();
  assert.deepEqual(impact.controlTaskIds, ['cross-promotion-task']);
  assert.equal(impact.interventionTaskIds.includes('cross-promotion-task'), false);
  assert.equal(impact.controlTaskVerifiedSuccesses, 1);

  assert.equal(await canaryImpactStore.recordObservations([observation({
    eventId: 'post-promotion-1',
    durationMs: 650,
    taskId: 'post-promotion-task',
    timestampMs: 500,
    canary: {
      knowledgeId: validated.id,
      implementation: 'search_exact_total_fast_tail',
      eligible: true,
      applied: true,
      bucket: 99
    }
  })]), 1);
  assert.equal(await canaryImpactStore.creditTaskOutcome('post-promotion-task', 'rolled_back', 501), 1);
  const rollback = await evaluateCanaryRollbacks(store, shadowImpactStore, canaryImpactStore, 502);
  assert.deepEqual(rollback, { evaluated: 1, rolledBack: 1 });
  assert.equal((await store.get(validated.id)).status, 'rejected');
  [impact] = await canaryImpactStore.list();
  assert.equal(impact.rollbackReason, 'promoted_task_rollback');
  assert.equal(impact.interventionTaskRollbacks, 1);

  await promotedEngine.refresh();
  assert.equal(await promotedEngine.decide(
    'search_text',
    { query: 'needle', calculate_total: true },
    'after-rollback'
  ), undefined);
});

test('canary rollback marker write failure restores the original knowledge status', async () => {
  const root = await mkdtemp(path.join(tmpdir(), 'ctmcp-knowledge-canary-rollback-write-failure-'));
  const store = new KnowledgeStore(root);
  const shadowImpactStore = new KnowledgeImpactStore(root);
  const canaryImpactStore = new CanaryImpactStore(root);
  const validated = await store.upsert(record({
    status: 'validated',
    recommendedAction: { recommendation: 'prefer_bounded_search', semantic_preserving_implementation: 'search_exact_total_fast_tail' }
  }));
  assert.equal(await canaryImpactStore.recordObservations([observation({
    eventId: 'rollback-write-failure-event',
    taskId: 'rollback-write-failure-task',
    timestampMs: 100,
    canary: {
      knowledgeId: validated.id,
      implementation: 'search_exact_total_fast_tail',
      eligible: true,
      applied: true,
      bucket: 1
    }
  })]), 1);
  assert.equal(await canaryImpactStore.creditTaskOutcome('rollback-write-failure-task', 'rolled_back', 101), 1);

  await mkdir(path.join(root, 'canary-impact.json.tmp'));
  await assert.rejects(evaluateCanaryRollbacks(store, shadowImpactStore, canaryImpactStore, 102));
  assert.equal((await store.get(validated.id)).status, 'validated');
});

test('canary impact is idempotent and an intervention task rollback disables the strategy', async () => {
  const root = await mkdtemp(path.join(tmpdir(), 'ctmcp-knowledge-canary-rollback-'));
  const store = new KnowledgeStore(root);
  const shadowImpactStore = new KnowledgeImpactStore(root);
  const canaryImpactStore = new CanaryImpactStore(root);
  const validated = await store.upsert(record({
    status: 'validated',
    recommendedAction: { recommendation: 'prefer_bounded_search', calculate_total: false }
  }));
  await shadowImpactStore.record({
    schemaVersion: 1,
    knowledgeId: validated.id,
    target: 'tool_strategy',
    tool: 'search_text',
    sourceEventIds: ['baseline-1'],
    matchedTaskIds: [],
    creditedTaskIds: [],
    considered: 1,
    matched: 1,
    actualSuccesses: 1,
    actualFailures: 0,
    verifiedSuccesses: 0,
    verificationFailures: 0,
    recoverySuccesses: 0,
    recoveryFailures: 0,
    taskVerifiedSuccesses: 0,
    taskUnverifiedSuccesses: 0,
    taskFailures: 0,
    taskRollbacks: 0,
    totalObservedDurationMs: 1_000,
    totalObservedRequestBytes: 10,
    totalObservedResponseBytes: 20,
    firstSeenAtMs: 1,
    lastSeenAtMs: 1
  });
  const canaryObservation = observation({
    eventId: 'canary-1',
    durationMs: 900,
    taskId: 'task-canary',
    timestampMs: 100,
    canary: {
      knowledgeId: validated.id,
      implementation: 'search_exact_total_fast_tail',
      eligible: true,
      applied: true,
      bucket: 4
    }
  });
  assert.equal(await canaryImpactStore.recordObservations([canaryObservation]), 1);
  assert.equal(await canaryImpactStore.recordObservations([canaryObservation]), 0);
  assert.equal(await canaryImpactStore.creditTaskOutcome('task-canary', 'rolled_back', 101), 1);
  assert.equal(await canaryImpactStore.creditTaskOutcome('task-canary', 'rolled_back', 102), 0);

  const rolledBack = await evaluateCanaryRollbacks(store, shadowImpactStore, canaryImpactStore, 103);
  assert.deepEqual(rolledBack, { evaluated: 1, rolledBack: 1 });
  assert.equal((await store.get(validated.id)).status, 'rejected');
  const [impact] = await canaryImpactStore.list();
  assert.equal(impact.interventionCalls, 1);
  assert.equal(impact.interventionTaskRollbacks, 1);
  assert.equal(impact.rollbackReason, 'intervention_task_rollback');

  const engine = new CanaryStrategyEngine(store, canaryImpactStore);
  assert.equal(await engine.decide('search_text', { query: 'needle', calculate_total: true }, 'conversation-a'), undefined);
});

test('separate registries expose tool strategies, tool evolution candidates, and only promoted evolved skills by default', async () => {
  const root = await mkdtemp(path.join(tmpdir(), 'ctmcp-knowledge-registry-'));
  const store = new KnowledgeStore(root);
  await store.upsert(record());
  await store.upsert(evolutionRecord({
    trigger: { tool: 'project_state' },
    hypothesis: 'Baseline capture is expensive.',
    recommendedAction: { proposal: 'incremental_baseline' },
    metadata: { proposalType: 'performance' }
  }));
  await store.upsert(record({
    target: 'evolved_skill',
    status: 'candidate',
    trigger: { skill: 'context-search' },
    hypothesis: 'Overlay can reduce broad searches.',
    recommendedAction: { add: ['prefer semantic narrowing'] },
    metadata: {
      name: 'context-search',
      generation: 1,
      base: { source: 'claude-user', name: 'context-search', contentSha256: 'abc' }
    }
  }));
  await store.upsert(record({
    target: 'evolved_skill',
    status: 'promoted',
    trigger: { skill: 'other-skill' },
    hypothesis: 'Validated overlay.',
    recommendedAction: { add: ['validated rule'] },
    metadata: {
      name: 'other-skill',
      generation: 2,
      base: { source: 'project', name: 'other-skill', contentSha256: 'def' }
    }
  }));

  const strategies = await new ToolStrategyRegistry(store).snapshot();
  const evolution = await new ToolEvolutionRegistry(store).snapshot();
  const evolved = await new EvolvedSkillRegistry(store).snapshot();
  const allEvolved = await new EvolvedSkillRegistry(store).snapshot({ includeNonPromoted: true });

  assert.equal(strategies.strategies.length, 1);
  assert.equal(evolution.candidates.length, 1);
  assert.deepEqual(evolved.skills.map(item => item.name), ['other-skill']);
  assert.deepEqual(allEvolved.skills.map(item => item.name), ['context-search', 'other-skill']);
});

test('Tool Evolution lifecycle claims candidates before planning and isolates per-workspace failures', async () => {
  const calls = [];
  const diagnostics = [];
  const runtime = {
    folderId: 'repo',
    toolEvolutionCandidateClaimer: {
      async sync() {
        calls.push('repo:claim');
        return { enabled: true, considered: 1, materialized: 1, replayed: 0, ignored: 0, failed: 0 };
      }
    },
    toolEvolutionPlanner: {
      async sync() {
        calls.push('repo:plan');
        return { enabled: true, considered: 1, planned: 1, superseded: 0 };
      }
    }
  };
  const failingRuntime = {
    folderId: 'broken',
    toolEvolutionCandidateClaimer: {
      async sync() {
        calls.push('broken:claim');
        throw new Error('fixture claim failure');
      }
    },
    toolEvolutionPlanner: {
      async sync() {
        calls.push('broken:plan');
        return { enabled: true, considered: 0, planned: 0, superseded: 0 };
      }
    }
  };
  await syncToolEvolutionRuntimes(
    [runtime, failingRuntime],
    { recordDiagnosticEvent: event => diagnostics.push(event) },
    'tool_evolution_workspace_hot_apply'
  );
  assert.ok(calls.indexOf('repo:claim') < calls.indexOf('repo:plan'));
  assert.equal(calls.includes('broken:plan'), false, 'a failed claim must not continue to planning for that workspace');
  const applied = diagnostics.find(event => event.event === 'tool_evolution_workspace_hot_apply');
  assert.equal(applied.fields.workspace_folder_id, 'repo');
  assert.equal(applied.fields.claims_materialized, 1);
  assert.equal(applied.fields.proposals_planned, 1);
  const failed = diagnostics.find(event => event.event === 'tool_evolution_workspace_hot_apply_failed');
  assert.equal(failed.fields.workspace_folder_id, 'broken');
});

test('ToolEvolutionPlanner plans trusted eligible knowledge and supersedes proposals when the base changes', async () => {
  const root = await mkdtemp(path.join(tmpdir(), 'ctmcp-tool-evolution-proposals-'));
  const knowledgeStore = new KnowledgeStore(root);
  const proposalStore = new ToolEvolutionProposalStore(root);
  const knowledge = await knowledgeStore.upsert(evolutionRecord({
    trigger: { tool: 'project_state', variant: 'proposal-planner' },
    hypothesis: 'A verified implementation opportunity should become an auditable change proposal.',
    recommendedAction: { proposal: 'incremental_baseline', preserve_semantics: true },
    metadata: { proposalType: 'performance' }
  }));

  const disabled = await new ToolEvolutionPlanner(knowledgeStore, proposalStore, {
    revision: 'base-a', trusted: false
  }).sync(90);
  assert.deepEqual(disabled, { enabled: false, considered: 0, planned: 0, superseded: 0 });
  assert.deepEqual(await proposalStore.list(), []);

  const planner = new ToolEvolutionPlanner(knowledgeStore, proposalStore, { revision: 'base-a', trusted: true });
  assert.deepEqual(await planner.sync(100), { enabled: true, considered: 1, planned: 1, superseded: 0 });
  assert.deepEqual(await planner.sync(101), { enabled: true, considered: 1, planned: 0, superseded: 0 });
  let proposals = await proposalStore.list();
  assert.equal(proposals.length, 1);
  assert.equal(proposals[0].status, 'proposed');
  assert.equal(proposals[0].knowledgeId, knowledge.id);
  assert.equal(proposals[0].baseRevision, 'base-a');
  assert.deepEqual(proposals[0].action, { proposal: 'incremental_baseline', preserve_semantics: true });
  assert.equal(proposals[0].proposalId, toolEvolutionProposalId({
    knowledgeId: knowledge.id,
    tool: 'project_state',
    proposalType: 'performance',
    baseRevision: 'base-a'
  }));

  const nextBase = new ToolEvolutionPlanner(knowledgeStore, proposalStore, { revision: 'base-b', trusted: true });
  assert.deepEqual(await nextBase.sync(200), { enabled: true, considered: 1, planned: 1, superseded: 1 });
  proposals = await proposalStore.list();
  assert.equal(proposals.length, 2);
  assert.deepEqual(proposals.map(item => [item.baseRevision, item.status]).sort(), [
    ['base-a', 'superseded'],
    ['base-b', 'proposed']
  ]);
  const registry = await new ToolEvolutionRegistry(knowledgeStore, undefined, proposalStore).snapshot();
  assert.match(registry.proposalRevision, /^[a-f0-9]{64}$/);
  assert.equal(registry.candidates[0].latestProposal.baseRevision, 'base-b');
  assert.equal(registry.candidates[0].latestProposal.status, 'proposed');
  assert.match(await proposalStore.revision(), /^[a-f0-9]{64}$/);
});

test('KnowledgeIngestor automatically plans Tool Evolution proposals only after diversity gating succeeds', async () => {
  const root = await mkdtemp(path.join(tmpdir(), 'ctmcp-tool-evolution-auto-plan-'));
  const knowledgeStore = new KnowledgeStore(root);
  const impactStore = new KnowledgeImpactStore(root);
  const proposalStore = new ToolEvolutionProposalStore(root);
  const planner = new ToolEvolutionPlanner(knowledgeStore, proposalStore, { revision: 'base-auto', trusted: true });
  const ingestor = new KnowledgeIngestor(knowledgeStore, impactStore, 'repo', undefined, undefined, planner);
  for (let index = 0; index < 5; index += 1) {
    ingestor.enqueueToolUsage({
      event_type: 'tool_call',
      diagnostic_event_id: `proposal-auto-${index}`,
      selected_workspace_id: 'repo',
      tool: 'project_state',
      outcome: 'success',
      verification_ok: true,
      duration_ms: 10_000,
      completed_ts_ms: 1_000 + index,
      phase_baseline_capture_ms: 9_000
    }, { conversationContextId: `conversation-${index % 2}` });
  }
  await ingestor.flush();
  const [learned] = await knowledgeStore.list();
  const [proposal] = await proposalStore.list();
  assert.equal(learned.target, 'tool_evolution');
  assert.equal(learned.status, 'shadow');
  assert.ok(proposal);
  assert.equal(proposal.knowledgeId, learned.id);
  assert.equal(proposal.status, 'proposed');
  assert.equal(proposal.baseRevision, 'base-auto');
  assert.equal(ingestor.snapshot().toolEvolutionProposalsPlanned, 1);
});

test('candidate commit claims materialize the prior-base proposal before the new planner can supersede it', async () => {
  const root = await mkdtemp(path.join(tmpdir(), 'ctmcp-tool-evolution-claim-'));
  const knowledgeStore = new KnowledgeStore(root);
  const proposalStore = new ToolEvolutionProposalStore(root);
  const experimentStore = new ToolEvolutionExperimentStore(root);
  const baseRevision = 'a'.repeat(40);
  const candidateRevision = 'b'.repeat(40);
  const knowledge = await knowledgeStore.upsert(evolutionRecord({
    trigger: { tool: 'search_text', variant: 'candidate-claim' },
    hypothesis: 'A commit trailer should bind the exact candidate build to the proposal it implements.',
    recommendedAction: { proposal: 'candidate-claim-fast-path' },
    metadata: { proposalType: 'performance' }
  }));
  const proposal = await proposalStore.plan(knowledge, baseRevision, 100);

  const untrusted = new ToolEvolutionCandidateClaimer(
    knowledgeStore,
    proposalStore,
    experimentStore,
    { revision: candidateRevision, trusted: false, proposalIds: [proposal.proposalId] }
  );
  assert.deepEqual(await untrusted.sync(150), {
    enabled: false, considered: 0, materialized: 0, replayed: 0, ignored: 0, failed: 0
  });
  assert.equal((await proposalStore.get(proposal.proposalId)).status, 'proposed');

  const claimer = new ToolEvolutionCandidateClaimer(
    knowledgeStore,
    proposalStore,
    experimentStore,
    {
      revision: candidateRevision,
      trusted: true,
      proposalIds: [proposal.proposalId, 'f'.repeat(64), proposal.proposalId]
    }
  );
  assert.deepEqual(await claimer.sync(200), {
    enabled: true, considered: 2, materialized: 1, replayed: 0, ignored: 1, failed: 0
  });
  const materialized = await proposalStore.get(proposal.proposalId);
  assert.equal(materialized.status, 'materialized');
  assert.equal(materialized.candidateRevision, candidateRevision);
  const [experiment] = await experimentStore.list();
  assert.equal(experiment.baseRevision, baseRevision);
  assert.equal(experiment.candidateRevision, candidateRevision);
  assert.equal(experiment.status, 'planned');

  const candidatePlanner = new ToolEvolutionPlanner(
    knowledgeStore,
    proposalStore,
    { revision: candidateRevision, trusted: true },
    experimentStore
  );
  assert.deepEqual(await candidatePlanner.sync(201), {
    enabled: true, considered: 1, planned: 0, superseded: 0
  });
  assert.equal((await proposalStore.list()).length, 1, 'an active materialized experiment must block a duplicate candidate-base proposal');
  assert.deepEqual(await claimer.sync(202), {
    enabled: true, considered: 2, materialized: 0, replayed: 1, ignored: 1, failed: 0
  });
});

test('Tool Evolution proposals materialize only after a real candidate revision exists', async () => {
  const root = await mkdtemp(path.join(tmpdir(), 'ctmcp-tool-evolution-materialize-'));
  const knowledgeStore = new KnowledgeStore(root);
  const proposalStore = new ToolEvolutionProposalStore(root);
  const experimentStore = new ToolEvolutionExperimentStore(root);
  const knowledge = await knowledgeStore.upsert(evolutionRecord({
    trigger: { tool: 'search_text', variant: 'materialize-proposal' },
    hypothesis: 'A proposed search implementation needs an exact candidate revision before benchmarking.',
    recommendedAction: { proposal: 'search-fast-path' },
    metadata: { proposalType: 'performance' }
  }));
  const proposal = await proposalStore.plan(knowledge, 'base-a', 100);
  await assert.rejects(
    materializeToolEvolutionProposal(
      knowledgeStore, proposalStore, experimentStore, proposal.proposalId, 'base-a', 'base-a', 150
    ),
    /must differ from baseRevision/
  );

  const materialized = await materializeToolEvolutionProposal(
    knowledgeStore, proposalStore, experimentStore, proposal.proposalId, 'base-a', 'candidate-b', 200
  );
  assert.equal(materialized.materialized, true);
  assert.equal(materialized.proposal.status, 'materialized');
  assert.equal(materialized.proposal.candidateRevision, 'candidate-b');
  assert.equal(materialized.experiment.status, 'planned');
  assert.equal(materialized.experiment.baseRevision, 'base-a');
  assert.equal(materialized.experiment.candidateRevision, 'candidate-b');
  const replay = await materializeToolEvolutionProposal(
    knowledgeStore, proposalStore, experimentStore, proposal.proposalId, 'base-a', 'candidate-b', 201
  );
  assert.equal(replay.experiment.experimentId, materialized.experiment.experimentId);

  const staleKnowledge = await knowledgeStore.upsert(evolutionRecord({
    trigger: { tool: 'project_state', variant: 'stale-proposal' },
    hypothesis: 'A proposal tied to an old base must not start a benchmark.',
    recommendedAction: { proposal: 'stale-proposal' },
    metadata: { proposalType: 'performance' }
  }));
  const staleProposal = await proposalStore.plan(staleKnowledge, 'base-old', 300);
  const stale = await materializeToolEvolutionProposal(
    knowledgeStore, proposalStore, experimentStore, staleProposal.proposalId, 'base-new', 'candidate-new', 301
  );
  assert.equal(stale.materialized, false);
  assert.equal(stale.reason, 'base_revision_changed');
  assert.equal(stale.proposal.status, 'superseded');
  assert.equal((await experimentStore.list()).length, 1);
});

test('Tool Evolution materialization transaction rolls back a failed proposal write without an orphan experiment', async () => {
  const root = await mkdtemp(path.join(tmpdir(), 'ctmcp-tool-evolution-atomic-materialize-'));
  const knowledgeStore = new KnowledgeStore(root);
  const proposalStore = new ToolEvolutionProposalStore(root);
  const experimentStore = new ToolEvolutionExperimentStore(root);
  const knowledge = await knowledgeStore.upsert(evolutionRecord({
    trigger: { tool: 'search_text', variant: 'atomic-materialize' },
    hypothesis: 'Proposal and experiment persistence must commit or roll back as one lifecycle operation.',
    recommendedAction: { proposal: 'atomic-search-fast-path' },
    metadata: { proposalType: 'performance' }
  }));
  const proposal = await proposalStore.plan(knowledge, 'base-atomic', 100);
  const blockingTempPath = path.join(root, 'tool-evolution-proposals.json.tmp');
  await mkdir(blockingTempPath);

  await assert.rejects(materializeToolEvolutionProposal(
    knowledgeStore,
    proposalStore,
    experimentStore,
    proposal.proposalId,
    'base-atomic',
    'candidate-atomic',
    200
  ));

  assert.equal((await proposalStore.get(proposal.proposalId)).status, 'proposed');
  assert.equal((await experimentStore.list()).length, 0);
  await assert.rejects(
    readFile(path.join(root, 'tool-evolution-lifecycle-transaction.json'), 'utf8'),
    error => error?.code === 'ENOENT'
  );

  await rm(blockingTempPath, { recursive: true, force: true });
  const retry = await materializeToolEvolutionProposal(
    knowledgeStore,
    proposalStore,
    experimentStore,
    proposal.proposalId,
    'base-atomic',
    'candidate-atomic',
    201
  );
  assert.equal(retry.materialized, true);
  assert.equal(retry.proposal.status, 'materialized');
  assert.equal((await experimentStore.list()).length, 1);
});

test('pending Tool Evolution MCP resources expose implementation contracts without raw evidence identifiers', async () => {
  const root = await mkdtemp(path.join(tmpdir(), 'ctmcp-tool-evolution-resource-'));
  const knowledgeStore = new KnowledgeStore(root);
  const proposalStore = new ToolEvolutionProposalStore(root);
  const conversationA = contextId('conversation', 'resource-a');
  const conversationB = contextId('conversation', 'resource-b');
  const knowledge = await knowledgeStore.upsert(evolutionRecord({
    trigger: { tool: 'search_text', variant: 'resource-contract' },
    hypothesis: 'Pending evolution work should be discoverable without becoming runtime prompt context.',
    recommendedAction: { proposal: 'resource-fast-path', preserve_semantics: true },
    metadata: { proposalType: 'performance' },
    evidence: { conversationContextIds: [conversationA, conversationB] },
    sourceEventIds: ['resource-secret-event-1', 'resource-secret-event-2', 'resource-secret-event-3', 'resource-secret-event-4', 'resource-secret-event-5']
  }));
  const proposal = await proposalStore.plan(knowledge, 'a'.repeat(40), 100);
  const ctx = {
    config: { folders: [{ id: 'repo', name: 'Repo' }] },
    folderRuntimes: new Map([['repo', { knowledgeStore, toolEvolutionProposalStore: proposalStore }]])
  };
  const uri = toolEvolutionResourceUri('repo', proposal.proposalId);
  const listed = await listToolEvolutionResources(ctx);
  assert.equal(listed.resources.length, 1);
  assert.equal(listed.resources[0].uri, uri);
  assert.equal(listed.resources[0].mimeType, 'application/json');
  assert.equal(listed.resources[0]._meta['coding-tools/resource-kind'], 'tool-evolution-proposal');

  const loaded = await readToolEvolutionResource(ctx, uri);
  const document = JSON.parse(loaded.contents[0].text);
  assert.equal(document.kind, 'tool_evolution_proposal');
  assert.equal(document.proposal.base_revision, 'a'.repeat(40));
  assert.deepEqual(document.recommended_action, { proposal: 'resource-fast-path', preserve_semantics: true });
  assert.equal(document.evidence.observations, 5);
  assert.equal(document.evidence.conversation_context_count, 2);
  assert.equal(document.implementation_contract.isolated_worktree_required, true);
  assert.equal(document.implementation_contract.no_automatic_promotion, true);
  assert.equal(
    document.implementation_contract.required_commit_trailer,
    `Tool-Evolution-Proposal: ${proposal.proposalId}`
  );
  const serialized = JSON.stringify(document);
  assert.doesNotMatch(serialized, /resource-secret-event/);
  assert.equal(serialized.includes(conversationA), false);
  assert.equal(serialized.includes(conversationB), false);

  await proposalStore.supersede(proposal.proposalId, 200);
  assert.deepEqual((await listToolEvolutionResources(ctx)).resources, []);
  await assert.rejects(readToolEvolutionResource(ctx, uri), /Resource not found/);
});

test('tool evolution experiments reject observed knowledge without independent context evidence', async () => {
  const root = await mkdtemp(path.join(tmpdir(), 'ctmcp-tool-evolution-evidence-gate-'));
  const knowledgeStore = new KnowledgeStore(root);
  const experimentStore = new ToolEvolutionExperimentStore(root);
  const knowledge = await knowledgeStore.upsert(record({
    target: 'tool_evolution',
    status: 'candidate',
    trigger: { tool: 'project_state', variant: 'single-context' },
    hypothesis: 'Repeated evidence from one context must not authorize MCP evolution.',
    recommendedAction: { proposal: 'single-context-blocked' },
    metadata: { proposalType: 'performance' },
    evidence: {
      observations: 5, successes: 5, failures: 0,
      totalDurationMs: 500, totalRequestBytes: 50, totalResponseBytes: 100,
      firstSeenAtMs: 1, lastSeenAtMs: 5,
      conversationContextIds: [contextId('conversation', 'only-one')]
    },
    sourceEventIds: Array.from({ length: 5 }, (_, index) => `single-context-${index}`)
  }));
  assert.equal(knowledge.status, 'observed');
  await assert.rejects(
    createToolEvolutionExperiment(knowledgeStore, experimentStore, knowledge.id, 'base-a', 'candidate-b', 100),
    /not eligible for a new experiment in status observed/
  );
});

test('tool evolution experiment plans are deterministic and benchmark storage is bounded and idempotent', async () => {
  const root = await mkdtemp(path.join(tmpdir(), 'ctmcp-tool-evolution-plan-'));
  const knowledgeStore = new KnowledgeStore(root);
  const experimentStore = new ToolEvolutionExperimentStore(root);
  const knowledge = await knowledgeStore.upsert(evolutionRecord({
    trigger: { tool: 'project_state' },
    hypothesis: 'Baseline capture should become incremental.',
    recommendedAction: { proposal: 'incremental_baseline' },
    metadata: { proposalType: 'performance' }
  }));
  const first = await createToolEvolutionExperiment(knowledgeStore, experimentStore, knowledge.id, 'base-a', 'candidate-b', 100);
  const replay = await createToolEvolutionExperiment(knowledgeStore, experimentStore, knowledge.id, 'base-a', 'candidate-b', 200);
  assert.equal(first.experimentId, replay.experimentId);
  assert.equal(first.experimentId, toolEvolutionExperimentId({
    knowledgeId: knowledge.id,
    tool: 'project_state',
    proposalType: 'performance',
    baseRevision: 'base-a',
    candidateRevision: 'candidate-b'
  }));
  assert.equal(replay.createdAtMs, 100);

  const sample = {
    sampleId: 'baseline-1',
    sourceRevision: 'base-a',
    taskOutcome: 'verified_success',
    toolCalls: 4,
    toolFailures: 0,
    durationMs: 1_000,
    responseBytes: 2_000,
    command: 'must not survive',
    patch: 'secret patch must not survive'
  };
  let updated = await experimentStore.recordBenchmark(first.experimentId, 'baseline', sample, 300);
  assert.equal(updated.baseline.samples, 1);
  updated = await experimentStore.recordBenchmark(first.experimentId, 'baseline', sample, 301);
  assert.equal(updated.baseline.samples, 1);
  await assert.rejects(
    experimentStore.recordBenchmark(first.experimentId, 'baseline', { ...sample, durationMs: 2_000 }, 302),
    /replayed with different metrics/
  );
  assert.equal(JSON.stringify(await experimentStore.list()).includes('must not survive'), false);
  await assert.rejects(
    experimentStore.recordBenchmark(first.experimentId, 'candidate', {
      sampleId: 'wrong-source',
      sourceRevision: 'some-other-revision',
      taskOutcome: 'verified_success',
      toolCalls: 1,
      toolFailures: 0,
      durationMs: 1,
      responseBytes: 1
    }, 303),
    /sourceRevision must match/
  );
});

test('ToolEvolutionBenchmarkCollector records task-wide bounded metrics only for trusted matching experiment revisions', async () => {
  const root = await mkdtemp(path.join(tmpdir(), 'ctmcp-tool-evolution-collector-'));
  const knowledgeStore = new KnowledgeStore(root);
  const experimentStore = new ToolEvolutionExperimentStore(root);
  const knowledge = await knowledgeStore.upsert(evolutionRecord({
    trigger: { tool: 'search_text' },
    hypothesis: 'Search implementation benchmark.',
    recommendedAction: { proposal: 'search-fast-path' },
    metadata: { proposalType: 'performance' }
  }));
  const experiment = await createToolEvolutionExperiment(
    knowledgeStore,
    experimentStore,
    knowledge.id,
    'base-auto',
    'candidate-auto',
    100
  );
  const collector = new ToolEvolutionBenchmarkCollector(experimentStore, { revision: 'base-auto', trusted: true });
  const enqueue = (eventId, tool, timestampMs, extras = {}, attribution = { taskId: 'task-a' }) => collector.enqueueToolUsage({
    event_type: 'tool_call',
    diagnostic_event_id: eventId,
    tool,
    outcome: 'success',
    verification_ok: true,
    duration_ms: 10,
    response_json_bytes: 100,
    completed_ts_ms: timestampMs,
    arguments: { secret: 'must not survive' },
    result: { stdout: 'must not survive' },
    ...extras
  }, attribution);

  enqueue('event-start', 'start_task', 110);
  enqueue('event-search', 'search_text', 120);
  enqueue('event-search', 'search_text', 120);
  enqueue('event-read', 'read_file', 130, { outcome: 'tool_error', verification_ok: false });
  enqueue('event-finish', 'finish_task', 140, {}, { taskId: 'task-a', taskOutcome: 'verified_success' });
  await collector.flush();

  let stored = await experimentStore.get(experiment.experimentId);
  assert.equal(stored.baseline.samples, 1);
  assert.equal(stored.baseline.taskOutcomes, 1);
  assert.equal(stored.baseline.verifiedTaskSuccesses, 1);
  assert.equal(stored.baseline.toolCalls, 4);
  assert.equal(stored.baseline.toolFailures, 1);
  assert.equal(stored.baseline.durationMs, 40);
  assert.equal(stored.baseline.responseBytes, 400);
  assert.equal(JSON.stringify(stored).includes('must not survive'), false);
  assert.equal(collector.snapshot().duplicateEvents, 1);
  assert.equal(collector.snapshot().recordedSamples, 1);

  enqueue('event-b-start', 'start_task', 150, {}, { taskId: 'task-without-target' });
  enqueue('event-b-read', 'read_file', 160, {}, { taskId: 'task-without-target' });
  enqueue('event-b-finish', 'finish_task', 170, {}, { taskId: 'task-without-target', taskOutcome: 'verified_success' });
  enqueue('event-old-start', 'start_task', 90, {}, { taskId: 'task-started-before-experiment' });
  enqueue('event-old-search', 'search_text', 180, {}, { taskId: 'task-started-before-experiment' });
  enqueue('event-old-finish', 'finish_task', 190, {}, { taskId: 'task-started-before-experiment', taskOutcome: 'verified_success' });
  await collector.flush();
  stored = await experimentStore.get(experiment.experimentId);
  assert.equal(stored.baseline.samples, 1);

  const candidateCollector = new ToolEvolutionBenchmarkCollector(experimentStore, { revision: 'candidate-auto', trusted: true });
  candidateCollector.enqueueToolUsage({
    event_type: 'tool_call', diagnostic_event_id: 'candidate-search', tool: 'search_text', outcome: 'success',
    verification_ok: true, duration_ms: 4, response_json_bytes: 50, completed_ts_ms: 200
  }, { taskId: 'candidate-task' });
  candidateCollector.enqueueToolUsage({
    event_type: 'tool_call', diagnostic_event_id: 'candidate-finish', tool: 'finish_task', outcome: 'success',
    verification_ok: true, duration_ms: 1, response_json_bytes: 20, completed_ts_ms: 210
  }, { taskId: 'candidate-task', taskOutcome: 'verified_success' });
  await candidateCollector.flush();
  stored = await experimentStore.get(experiment.experimentId);
  assert.equal(stored.candidate.samples, 1);
  assert.equal(stored.candidate.toolCalls, 2);

  const disabled = new ToolEvolutionBenchmarkCollector(experimentStore, { revision: 'base-auto', trusted: false });
  disabled.enqueueToolUsage({
    event_type: 'tool_call', diagnostic_event_id: 'disabled-search', tool: 'search_text', outcome: 'success', completed_ts_ms: 220
  }, { taskId: 'disabled-task', taskOutcome: 'verified_success' });
  await disabled.flush();
  assert.equal(disabled.snapshot().enabled, false);
  assert.equal((await experimentStore.get(experiment.experimentId)).baseline.samples, 1);
});

test('ToolEvolutionBenchmarkCollector rejects tasks that began before an experiment was planned', async () => {
  const root = await mkdtemp(path.join(tmpdir(), 'ctmcp-tool-evolution-mid-task-'));
  const knowledgeStore = new KnowledgeStore(root);
  const experimentStore = new ToolEvolutionExperimentStore(root);
  const collector = new ToolEvolutionBenchmarkCollector(experimentStore, { revision: 'base-mid-task', trusted: true });
  collector.enqueueToolUsage({
    event_type: 'tool_call', diagnostic_event_id: 'before-plan', tool: 'start_task', outcome: 'success',
    verification_ok: true, duration_ms: 1, response_json_bytes: 1, completed_ts_ms: 90
  }, { taskId: 'task-before-plan' });
  await collector.flush();
  assert.equal(collector.snapshot().pendingTasks, 1);

  const knowledge = await knowledgeStore.upsert(evolutionRecord({
    trigger: { tool: 'search_text', variant: 'mid-task-plan' },
    hypothesis: 'Tasks already in flight must not contaminate a new experiment.',
    recommendedAction: { proposal: 'mid-task-plan-guard' },
    metadata: { proposalType: 'performance' }
  }));
  const experiment = await createToolEvolutionExperiment(
    knowledgeStore,
    experimentStore,
    knowledge.id,
    'base-mid-task',
    'candidate-mid-task',
    100
  );
  collector.enqueueToolUsage({
    event_type: 'tool_call', diagnostic_event_id: 'after-plan-target', tool: 'search_text', outcome: 'success',
    verification_ok: true, duration_ms: 10, response_json_bytes: 20, completed_ts_ms: 110
  }, { taskId: 'task-before-plan' });
  collector.enqueueToolUsage({
    event_type: 'tool_call', diagnostic_event_id: 'after-plan-finish', tool: 'finish_task', outcome: 'success',
    verification_ok: true, duration_ms: 1, response_json_bytes: 5, completed_ts_ms: 120
  }, { taskId: 'task-before-plan', taskOutcome: 'verified_success' });
  await collector.flush();
  assert.equal((await experimentStore.get(experiment.experimentId)).baseline.samples, 0);
  assert.equal(collector.snapshot().pendingTasks, 0);
});

test('tool evolution experiments require quality preservation plus material efficiency improvement before promotion', async () => {
  const root = await mkdtemp(path.join(tmpdir(), 'ctmcp-tool-evolution-validate-'));
  const knowledgeStore = new KnowledgeStore(root);
  const experimentStore = new ToolEvolutionExperimentStore(root);
  const knowledge = await knowledgeStore.upsert(evolutionRecord({
    trigger: { tool: 'search_text' },
    hypothesis: 'Broad scans should avoid managed scratch trees.',
    recommendedAction: { proposal: 'exclude_untracked_worktree_management_containers' },
    metadata: { proposalType: 'performance' }
  }));
  const experiment = await createToolEvolutionExperiment(knowledgeStore, experimentStore, knowledge.id, 'base-search', 'candidate-search', 100);
  for (let index = 0; index < 5; index += 1) {
    await experimentStore.recordBenchmark(experiment.experimentId, 'baseline', {
      sampleId: `baseline-${index}`,
      sourceRevision: 'base-search',
      taskOutcome: 'verified_success',
      toolCalls: 10,
      toolFailures: 0,
      durationMs: 1_000,
      responseBytes: 10_000
    }, 200 + index);
    await experimentStore.recordBenchmark(experiment.experimentId, 'candidate', {
      sampleId: `candidate-${index}`,
      sourceRevision: 'candidate-search',
      taskOutcome: 'verified_success',
      toolCalls: 8,
      toolFailures: 0,
      durationMs: 850,
      responseBytes: 7_200
    }, 300 + index);
  }
  const validated = await experimentStore.evaluate(experiment.experimentId, 'base-search', 400);
  assert.equal(validated.status, 'validated');
  assert.equal(validated.decisionReason, 'quality_preserved_with_material_efficiency_improvement');

  const promotion = await promoteToolEvolutionExperiment(
    knowledgeStore,
    experimentStore,
    experiment.experimentId,
    'base-search',
    'candidate-search',
    500
  );
  assert.equal(promotion.promoted, true);
  assert.equal(promotion.experiment.status, 'promoted');
  assert.equal((await knowledgeStore.get(knowledge.id)).status, 'promoted');

  const registry = await new ToolEvolutionRegistry(knowledgeStore, experimentStore).snapshot();
  assert.match(registry.experimentRevision, /^[a-f0-9]{64}$/);
  assert.equal(registry.candidates[0].latestExperiment.experimentId, experiment.experimentId);
  assert.equal(registry.candidates[0].latestExperiment.status, 'promoted');
  assert.equal(registry.candidates[0].latestExperiment.candidateRevision, 'candidate-search');
});

test('tool evolution experiments reject stale bases and task-quality regressions', async () => {
  const root = await mkdtemp(path.join(tmpdir(), 'ctmcp-tool-evolution-reject-'));
  const knowledgeStore = new KnowledgeStore(root);
  const experimentStore = new ToolEvolutionExperimentStore(root);
  const staleKnowledge = await knowledgeStore.upsert(evolutionRecord({
    trigger: { tool: 'project_state', variant: 'stale' },
    hypothesis: 'Stale experiment candidate.',
    recommendedAction: { proposal: 'incremental_baseline' },
    metadata: { proposalType: 'performance' }
  }));
  const staleExperiment = await createToolEvolutionExperiment(knowledgeStore, experimentStore, staleKnowledge.id, 'base-old', 'candidate-old', 100);
  const stale = await experimentStore.evaluate(staleExperiment.experimentId, 'base-new', 200);
  assert.equal(stale.status, 'stale');
  assert.equal(stale.decisionReason, 'base_revision_changed');

  const driftKnowledge = await knowledgeStore.upsert(evolutionRecord({
    trigger: { tool: 'search_text', variant: 'candidate-drift' },
    hypothesis: 'Promotion must use the exact benchmarked candidate revision.',
    recommendedAction: { proposal: 'candidate-drift-guard' },
    metadata: { proposalType: 'performance' }
  }));
  const driftExperiment = await createToolEvolutionExperiment(
    knowledgeStore,
    experimentStore,
    driftKnowledge.id,
    'base-drift',
    'candidate-drift-v1',
    210
  );
  for (let index = 0; index < 5; index += 1) {
    await experimentStore.recordBenchmark(driftExperiment.experimentId, 'baseline', {
      sampleId: `drift-baseline-${index}`,
      sourceRevision: 'base-drift',
      taskOutcome: 'verified_success',
      toolCalls: 10,
      toolFailures: 0,
      durationMs: 1_000,
      responseBytes: 10_000
    }, 220 + index);
    await experimentStore.recordBenchmark(driftExperiment.experimentId, 'candidate', {
      sampleId: `drift-candidate-${index}`,
      sourceRevision: 'candidate-drift-v1',
      taskOutcome: 'verified_success',
      toolCalls: 8,
      toolFailures: 0,
      durationMs: 800,
      responseBytes: 7_000
    }, 230 + index);
  }
  assert.equal((await experimentStore.evaluate(driftExperiment.experimentId, 'base-drift', 240)).status, 'validated');
  const driftPromotion = await promoteToolEvolutionExperiment(
    knowledgeStore,
    experimentStore,
    driftExperiment.experimentId,
    'base-drift',
    'candidate-drift-v2',
    250
  );
  assert.equal(driftPromotion.promoted, false);
  assert.equal(driftPromotion.experiment.status, 'stale');
  assert.equal(driftPromotion.experiment.decisionReason, 'candidate_revision_changed');
  assert.equal((await knowledgeStore.get(driftKnowledge.id)).status, 'candidate');

  const regressionKnowledge = await knowledgeStore.upsert(evolutionRecord({
    trigger: { tool: 'edit', variant: 'quality-regression' },
    hypothesis: 'Candidate must not trade correctness for speed.',
    recommendedAction: { proposal: 'candidate-edit-primitive' },
    metadata: { proposalType: 'primitive' }
  }));
  const regression = await createToolEvolutionExperiment(knowledgeStore, experimentStore, regressionKnowledge.id, 'base-edit', 'candidate-edit', 300);
  for (let index = 0; index < 5; index += 1) {
    await experimentStore.recordBenchmark(regression.experimentId, 'baseline', {
      sampleId: `baseline-${index}`,
      sourceRevision: 'base-edit',
      taskOutcome: 'verified_success',
      toolCalls: 6,
      toolFailures: 0,
      durationMs: 1_000,
      responseBytes: 6_000
    }, 400 + index);
    await experimentStore.recordBenchmark(regression.experimentId, 'candidate', {
      sampleId: `candidate-${index}`,
      sourceRevision: 'candidate-edit',
      taskOutcome: index === 0 ? 'rolled_back' : 'verified_success',
      toolCalls: 3,
      toolFailures: 0,
      durationMs: 400,
      responseBytes: 2_000
    }, 500 + index);
  }
  const rejected = await experimentStore.evaluate(regression.experimentId, 'base-edit', 600);
  assert.equal(rejected.status, 'rejected');
  assert.equal(rejected.decisionReason, 'candidate_task_rollback');
  const promotion = await promoteToolEvolutionExperiment(
    knowledgeStore,
    experimentStore,
    regression.experimentId,
    'base-edit',
    'candidate-edit',
    700
  );
  assert.equal(promotion.promoted, false);
  assert.equal((await knowledgeStore.get(regressionKnowledge.id)).status, 'candidate');
});
