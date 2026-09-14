import { createHash } from 'node:crypto';
import type { ExperienceObservation, KnowledgeArtifactTarget, KnowledgeEvidence, KnowledgeRecord } from './types.js';
import { knowledgeEvidenceCandidateReady, knowledgeRecordId } from './store.js';

const SAFE_FEATURES = new Set([
  'calculate_total',
  'files_considered',
  'scanned_files',
  'scan_excluded_worktree_container_count',
  'returned_count',
  'phase_baseline_capture_ms',
  'phase_baseline_refresh_ms',
  'phase_harness_begin_ms',
  'phase_harness_finish_ms',
  'baseline_refresh_mode',
  'baseline_refresh_file_count',
  'recovery_succeeded',
  'error_direct_recovery_action_count',
  'retry_attempt',
  'graph_action',
  'detached',
  'reattached',
  'graph_yield_ms',
  'graph_wait_ms',
  'parallel_conflict',
  'parallel_serialized'
]);

const IDENTITY_FEATURES = new Set([
  'calculate_total',
  'graph_action',
  'detached',
  'reattached',
  'parallel_conflict',
  'parallel_serialized'
]);

const EVOLVED_SKILL_SOURCES = new Set(['project', 'agents', 'claude', 'codex-user', 'claude-user']);
const SHA256_PATTERN = /^[0-9a-f]{64}$/;
const STALE_EDIT_GUIDANCE = 'When a guarded edit reports stale state, re-read the current file and rebuild the guarded edit before retrying.';

function safeFeatures(observation: ExperienceObservation): Record<string, string | number | boolean | null> {
  return Object.fromEntries(
    Object.entries(observation.features ?? {})
      .filter(([key, value]) => SAFE_FEATURES.has(key) && (['string', 'number', 'boolean'].includes(typeof value) || value === null))
      .sort(([left], [right]) => left.localeCompare(right))
  );
}

function identityFeatures(observation: ExperienceObservation): Record<string, string | number | boolean | null> {
  return Object.fromEntries(
    Object.entries(safeFeatures(observation)).filter(([key]) => IDENTITY_FEATURES.has(key))
  );
}

function evidenceFor(observations: readonly ExperienceObservation[]): KnowledgeEvidence {
  const successes = observations.filter(item => item.outcome === 'success').length;
  const taskContextIds = contextIds(observations, 'task', observation => observation.taskId);
  const conversationContextIds = contextIds(observations, 'conversation', observation => observation.conversationContextId);
  const runtimeBootContextIds = contextIds(observations, 'runtime-boot', observation => observation.runtimeBootId);
  return {
    observations: observations.length,
    successes,
    failures: observations.length - successes,
    verifiedSuccesses: observations.filter(item => item.verificationOutcome === 'passed').length,
    verificationFailures: observations.filter(item => item.verificationOutcome === 'failed').length,
    recoverySuccesses: observations.filter(item => item.recoveryOutcome === 'passed').length,
    recoveryFailures: observations.filter(item => item.recoveryOutcome === 'failed').length,
    totalDurationMs: observations.reduce((total, item) => total + Math.max(0, item.durationMs), 0),
    totalRequestBytes: observations.reduce((total, item) => total + Math.max(0, item.requestBytes ?? 0), 0),
    totalResponseBytes: observations.reduce((total, item) => total + Math.max(0, item.responseBytes ?? 0), 0),
    firstSeenAtMs: observations[0]!.timestampMs,
    lastSeenAtMs: observations[observations.length - 1]!.timestampMs,
    ...(taskContextIds.length ? { taskContextIds } : {}),
    ...(conversationContextIds.length ? { conversationContextIds } : {}),
    ...(runtimeBootContextIds.length ? { runtimeBootContextIds } : {})
  };
}

function confidence(observations: number): number {
  if (observations <= 0) return 0;
  return Math.round((observations / (observations + 5)) * 10_000) / 10_000;
}

function contextDigest(kind: string, value: string | undefined): string | undefined {
  const normalized = value?.trim();
  if (!normalized) return undefined;
  return createHash('sha256').update(`${kind}\0${normalized}`).digest('hex');
}

function contextIds(
  observations: readonly ExperienceObservation[],
  kind: string,
  value: (observation: ExperienceObservation) => string | undefined
): string[] {
  return [...new Set(observations
    .map(observation => contextDigest(kind, value(observation)))
    .filter((item): item is string => Boolean(item)))]
    .sort()
    .slice(0, 256);
}

function groupedExpensiveSearch(
  observations: readonly ExperienceObservation[],
  features: readonly Record<string, string | number | boolean | null>[]
): boolean {
  return features.some((feature, index) => feature.calculate_total === true
    && (Number(feature.files_considered ?? 0) >= 500 || observations[index]!.durationMs >= 1_000));
}

function groupedBroadSlowSearch(
  observations: readonly ExperienceObservation[],
  features: readonly Record<string, string | number | boolean | null>[]
): boolean {
  return features.some((feature, index) => feature.calculate_total !== true
    && Number(feature.files_considered ?? 0) >= 5_000
    && observations[index]!.durationMs >= 5_000);
}

function groupedExpensiveEditHarness(
  observations: readonly ExperienceObservation[],
  features: readonly Record<string, string | number | boolean | null>[]
): boolean {
  return features.some((feature, index) => observations[index]!.outcome === 'success'
    && Number(feature.phase_harness_begin_ms ?? 0) + Number(feature.phase_harness_finish_ms ?? 0) >= 1_000);
}

function groupedRetainedGraphPolling(
  observations: readonly ExperienceObservation[],
  features: readonly Record<string, string | number | boolean | null>[]
): boolean {
  return features.some((feature, index) => {
    const yieldMs = Number(feature.graph_yield_ms ?? 0);
    const waitMs = Number(feature.graph_wait_ms ?? 0);
    return observations[index]!.outcome === 'success'
      && feature.graph_action === 'run'
      && feature.detached === true
      && feature.reattached === true
      && yieldMs >= 25_000
      && waitMs >= Math.max(25_000, yieldMs * 0.9);
  });
}

function classify(observations: readonly ExperienceObservation[]): {
  target: KnowledgeArtifactTarget;
  hypothesis: string;
  action: Record<string, unknown>;
  metadata?: Record<string, unknown>;
} {
  const first = observations[0]!;
  const errorCodes = new Set(observations.map(item => item.errorCode).filter(Boolean));
  const features = observations.map(safeFeatures);

  if (first.tool === 'search_text' && groupedExpensiveSearch(observations, features)) {
    return {
      target: 'tool_strategy',
      hypothesis: 'Exact total counting can dominate search cost when callers only need bounded matches.',
      action: {
        recommendation: 'prefer_bounded_search',
        caller_intent_required: 'bounded_results_only',
        semantic_preserving_implementation: 'search_exact_total_fast_tail'
      }
    };
  }
  if (first.tool === 'search_text' && groupedBroadSlowSearch(observations, features)) {
    return {
      target: 'tool_evolution',
      hypothesis: 'Broad workspace discovery can spend most of its time scanning files outside the active worktree when managed worktree containers are mixed into the workspace root.',
      action: { proposal: 'exclude_untracked_worktree_management_containers' },
      metadata: { proposalType: 'performance' }
    };
  }
  if (first.tool === 'project_state' && features.some(feature => Number(feature.phase_baseline_capture_ms ?? 0) >= 1_000)) {
    return {
      target: 'tool_evolution',
      hypothesis: 'Repeated full baseline capture is a structural tool cost and should be reduced inside the MCP implementation.',
      action: { proposal: 'incremental_or_cached_baseline' },
      metadata: { proposalType: 'performance' }
    };
  }
  if (first.tool === 'edit' && errorCodes.size === 0 && groupedExpensiveEditHarness(observations, features)) {
    return {
      target: 'tool_evolution',
      hypothesis: 'Successful guarded edits can spend more time refreshing whole-workspace Harness state than applying the verified file mutation.',
      action: { proposal: 'incremental_post_edit_baseline_refresh' },
      metadata: { proposalType: 'performance' }
    };
  }
  if (first.tool === 'exec_many' && groupedRetainedGraphPolling(observations, features)) {
    return {
      target: 'tool_evolution',
      hypothesis: 'Retained exec_many reattachments that repeatedly exhaust the graph wait window force avoidable MCP and model round-trips while child work is still healthy.',
      action: {
        proposal: 'adaptive_retained_graph_reattach_wait',
        preserve_initial_responsiveness: true
      },
      metadata: { proposalType: 'performance' }
    };
  }
  if (first.tool === 'edit' && (errorCodes.has('FILE_VERSION_MISMATCH') || errorCodes.has('EDIT_MATCH_COUNT_MISMATCH'))) {
    return {
      target: 'tool_strategy',
      hypothesis: 'Stale edit state should be refreshed before retrying the same mutation.',
      action: { recovery: ['use_direct_recovery_action_if_available', 'read_file', 'retry_edit'] }
    };
  }
  return {
    target: 'ignore',
    hypothesis: 'No promotable tool-learning pattern matched this observation group.',
    action: {}
  };
}

function compileEvolvedSkillRecords(observations: readonly ExperienceObservation[]): KnowledgeRecord[] {
  const groups = new Map<string, ExperienceObservation[]>();
  for (const observation of observations) {
    const skill = observation.skill;
    if (!skill || observation.tool !== 'edit'
      || !['FILE_VERSION_MISMATCH', 'EDIT_MATCH_COUNT_MISMATCH'].includes(observation.errorCode ?? '')
      || !EVOLVED_SKILL_SOURCES.has(skill.source)
      || !skill.name.trim() || skill.name.length > 128
      || !SHA256_PATTERN.test(skill.contentSha256)
      || !Number.isInteger(skill.generation) || skill.generation < 0 || skill.generation > 10_000) continue;
    const groupKey = JSON.stringify({
      source: skill.source,
      name: skill.name,
      contentSha256: skill.contentSha256,
      generation: skill.generation,
      pattern: 'stale_guarded_edit_recovery'
    });
    const group = groups.get(groupKey) ?? [];
    group.push(observation);
    groups.set(groupKey, group);
  }
  const records: KnowledgeRecord[] = [];
  for (const grouped of groups.values()) {
    grouped.sort((left, right) => left.timestampMs - right.timestampMs || left.eventId.localeCompare(right.eventId));
    const skill = grouped[0]!.skill!;
    const evidence = evidenceFor(grouped);
    const record: KnowledgeRecord = {
      id: '',
      schemaVersion: 1,
      scope: 'workspace',
      target: 'evolved_skill',
      status: knowledgeEvidenceCandidateReady('evolved_skill', evidence) ? 'candidate' : 'observed',
      trigger: {
        tool: 'edit',
        pattern: 'stale_guarded_edit_recovery',
        skill_source: skill.source,
        skill_name_sha256: createHash('sha256').update(skill.name).digest('hex'),
        base_content_sha256: skill.contentSha256
      },
      hypothesis: 'A loaded Skill can incorporate deterministic stale-edit recovery guidance after the same friction recurs across independent contexts.',
      recommendedAction: { append_guidance: [STALE_EDIT_GUIDANCE] },
      evidence,
      confidence: confidence(grouped.length),
      sourceEventIds: grouped.map(item => item.eventId).sort(),
      counterexampleEventIds: [],
      createdAtMs: grouped[0]!.timestampMs,
      updatedAtMs: grouped[grouped.length - 1]!.timestampMs,
      metadata: {
        name: skill.name,
        base: { source: skill.source, name: skill.name, contentSha256: skill.contentSha256 },
        generation: skill.generation + 1,
        pattern: 'stale_guarded_edit_recovery'
      }
    };
    record.id = knowledgeRecordId(record);
    records.push(record);
  }
  return records;
}

export class ExperienceCompiler {
  compile(observations: readonly ExperienceObservation[]): KnowledgeRecord[] {
    const groups = new Map<string, ExperienceObservation[]>();
    for (const observation of observations) {
      const groupKey = JSON.stringify({
        tool: observation.tool,
        errorCode: observation.errorCode ?? null,
        features: identityFeatures(observation)
      });
      const group = groups.get(groupKey) ?? [];
      group.push(observation);
      groups.set(groupKey, group);
    }

    const records: KnowledgeRecord[] = [];
    for (const grouped of groups.values()) {
      grouped.sort((left, right) => left.timestampMs - right.timestampMs || left.eventId.localeCompare(right.eventId));
      const classification = classify(grouped);
      if (classification.target === 'ignore') continue;
      const evidence = evidenceFor(grouped);
      const trigger: Record<string, unknown> = {
        tool: grouped[0]!.tool,
        ...(grouped[0]!.errorCode ? { errorCode: grouped[0]!.errorCode } : {}),
        ...identityFeatures(grouped[0]!)
      };
      const now = grouped[grouped.length - 1]!.timestampMs;
      const record: KnowledgeRecord = {
        id: '',
        schemaVersion: 1,
        scope: 'runtime',
        target: classification.target,
        status: knowledgeEvidenceCandidateReady(classification.target, evidence) ? 'candidate' : 'observed',
        trigger,
        hypothesis: classification.hypothesis,
        recommendedAction: classification.action,
        evidence,
        confidence: confidence(grouped.length),
        sourceEventIds: grouped.map(item => item.eventId).sort(),
        counterexampleEventIds: [],
        createdAtMs: grouped[0]!.timestampMs,
        updatedAtMs: now,
        metadata: classification.metadata
      };
      record.id = knowledgeRecordId(record);
      records.push(record);
    }
    records.push(...compileEvolvedSkillRecords(observations));
    return records.sort((left, right) => left.id.localeCompare(right.id));
  }
}
