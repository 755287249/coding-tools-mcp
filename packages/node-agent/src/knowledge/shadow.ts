import { createHash } from 'node:crypto';
import { KnowledgeImpactStore, type KnowledgeImpactRecord } from './impact.js';
import { KnowledgeStore } from './store.js';
import type { ExperienceObservation, KnowledgeRecord } from './types.js';

function toolFor(record: KnowledgeRecord): string {
  return typeof record.trigger.tool === 'string' ? record.trigger.tool : '';
}

function evolvedSkillTriggerMatches(record: KnowledgeRecord, observation: ExperienceObservation): boolean {
  if (toolFor(record) !== observation.tool || !observation.skill) return false;
  if (record.trigger.pattern !== 'stale_guarded_edit_recovery') return false;
  if (!['FILE_VERSION_MISMATCH', 'EDIT_MATCH_COUNT_MISMATCH'].includes(observation.errorCode ?? '')) return false;
  if (record.trigger.skill_source !== observation.skill.source) return false;
  if (record.trigger.base_content_sha256 !== observation.skill.contentSha256) return false;
  const nameHash = createHash('sha256').update(observation.skill.name).digest('hex');
  return record.trigger.skill_name_sha256 === nameHash;
}

function triggerMatches(record: KnowledgeRecord, observation: ExperienceObservation): boolean {
  if (record.target === 'evolved_skill') return evolvedSkillTriggerMatches(record, observation);
  if (toolFor(record) !== observation.tool) return false;
  if (typeof record.trigger.errorCode === 'string' && record.trigger.errorCode !== observation.errorCode) return false;
  for (const [key, expected] of Object.entries(record.trigger)) {
    if (key === 'tool' || key === 'errorCode') continue;
    if (observation.features?.[key] !== expected) return false;
  }
  if (record.recommendedAction.recommendation === 'prefer_bounded_search') {
    return Number(observation.features?.files_considered ?? 0) >= 500 || observation.durationMs >= 1_000;
  }
  if (record.recommendedAction.proposal === 'incremental_or_cached_baseline') {
    return Number(observation.features?.phase_baseline_capture_ms ?? 0) >= 1_000;
  }
  return true;
}

function delta(record: KnowledgeRecord, observations: readonly ExperienceObservation[]): KnowledgeImpactRecord | undefined {
  const relevant = observations.filter(observation => observation.tool === toolFor(record));
  if (!relevant.length) return undefined;
  const matched = relevant.filter(observation => triggerMatches(record, observation));
  const firstSeenAtMs = Math.min(...relevant.map(observation => observation.timestampMs));
  const lastSeenAtMs = Math.max(...relevant.map(observation => observation.timestampMs));
  return {
    schemaVersion: 1,
    knowledgeId: record.id,
    target: record.target,
    tool: toolFor(record),
    sourceEventIds: relevant.map(observation => observation.eventId).sort(),
    matchedTaskIds: [...new Set(matched.map(observation => observation.taskId).filter((value): value is string => Boolean(value)))].sort(),
    creditedTaskIds: [],
    considered: relevant.length,
    matched: matched.length,
    actualSuccesses: matched.filter(item => item.outcome === 'success').length,
    actualFailures: matched.filter(item => item.outcome === 'failure').length,
    verifiedSuccesses: matched.filter(item => item.verificationOutcome === 'passed').length,
    verificationFailures: matched.filter(item => item.verificationOutcome === 'failed').length,
    recoverySuccesses: matched.filter(item => item.recoveryOutcome === 'passed').length,
    recoveryFailures: matched.filter(item => item.recoveryOutcome === 'failed').length,
    taskVerifiedSuccesses: 0,
    taskUnverifiedSuccesses: 0,
    taskFailures: 0,
    taskRollbacks: 0,
    totalObservedDurationMs: matched.reduce((total, item) => total + item.durationMs, 0),
    totalObservedRequestBytes: matched.reduce((total, item) => total + (item.requestBytes ?? 0), 0),
    totalObservedResponseBytes: matched.reduce((total, item) => total + (item.responseBytes ?? 0), 0),
    firstSeenAtMs,
    lastSeenAtMs
  };
}

export class ShadowEvaluator {
  constructor(
    private readonly knowledgeStore: KnowledgeStore,
    private readonly impactStore: KnowledgeImpactStore
  ) {}

  async evaluate(
    observations: readonly ExperienceObservation[],
    recordsBeforeBatch: readonly KnowledgeRecord[]
  ): Promise<{ considered: number; matched: number }> {
    let considered = 0;
    let matched = 0;
    for (const record of recordsBeforeBatch) {
      if (!['candidate', 'shadow'].includes(record.status)) continue;
      if (!['tool_strategy', 'tool_evolution', 'evolved_skill'].includes(record.target)) continue;
      const impact = delta(record, observations);
      if (!impact) continue;
      considered += impact.considered;
      matched += impact.matched;
      await this.impactStore.record(impact);
    }
    return { considered, matched };
  }

  async activateCandidates(updatedAtMs: number): Promise<number> {
    let activated = 0;
    for (const record of await this.knowledgeStore.list()) {
      if (record.status !== 'candidate') continue;
      if (!['tool_strategy', 'tool_evolution', 'evolved_skill'].includes(record.target)) continue;
      if (await this.knowledgeStore.transitionStatus(record.id, ['candidate'], 'shadow', updatedAtMs)) activated += 1;
    }
    return activated;
  }
}
