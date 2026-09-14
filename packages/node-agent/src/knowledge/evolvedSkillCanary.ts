import type { SkillDescriptor } from '../skills/types.js';
import { CanaryImpactStore, canaryCohortBucket, type CanaryImpactRecord } from './canary.js';
import { EvolvedSkillRegistry } from './registries.js';
import { KnowledgeStore } from './store.js';
import type { EvolvedSkillRecord } from './types.js';

export const EVOLVED_SKILL_CANARY_IMPLEMENTATION = 'evolved_skill_append_guidance_v1';
const CANARY_PERCENT = 25;
const MIN_INTERVENTION_CALLS = 20;
const MIN_CONTROL_CALLS = 10;
const MIN_TASK_OUTCOMES = 3;
const MIN_VERIFIED_TASK_RATE = 0.8;
const MAX_TASK_FAILURE_RATE = 0.2;
const MAX_INTERVENTION_FAILURE_RATE = 0.25;
const MIN_RELATIVE_FAILURE_IMPROVEMENT = 0.2;
const MIN_ABSOLUTE_FAILURE_IMPROVEMENT = 0.1;
const MAX_DURATION_REGRESSION_RATIO = 1.25;
const POST_PROMOTION_MIN_CALLS = 10;
const MAX_ROLLBACK_FAILURE_RATE_DELTA = 0.15;

export interface EvolvedSkillCanaryDecision {
  knowledgeId: string;
  implementation: typeof EVOLVED_SKILL_CANARY_IMPLEMENTATION;
  eligible: true;
  stage: 'canary' | 'promoted';
  applied: boolean;
  bucket: number;
  alreadyApplied: boolean;
  record: EvolvedSkillRecord;
}

function ratio(numerator: number, denominator: number): number {
  return denominator > 0 ? numerator / denominator : 0;
}

function taskOutcomeCount(record: CanaryImpactRecord, side: 'control' | 'intervention'): number {
  if (side === 'control') {
    return record.controlTaskVerifiedSuccesses
      + record.controlTaskUnverifiedSuccesses
      + record.controlTaskFailures
      + record.controlTaskRollbacks;
  }
  return record.interventionTaskVerifiedSuccesses
    + record.interventionTaskUnverifiedSuccesses
    + record.interventionTaskFailures
    + record.interventionTaskRollbacks;
}

function baseContentSha256(skill: SkillDescriptor): string {
  return skill.evolution?.baseContentSha256 ?? skill.contentSha256;
}

function matchesSkill(skill: SkillDescriptor, record: EvolvedSkillRecord): boolean {
  return record.base.source === skill.source
    && record.base.name.toLocaleLowerCase('en-US') === skill.name.toLocaleLowerCase('en-US')
    && record.base.contentSha256 === baseContentSha256(skill);
}

export class EvolvedSkillCanaryEngine {
  constructor(
    private readonly knowledgeStore: KnowledgeStore,
    private readonly impactStore: CanaryImpactStore
  ) {}

  async decide(skill: SkillDescriptor, sampleKey: string): Promise<EvolvedSkillCanaryDecision | undefined> {
    const [snapshot, impacts] = await Promise.all([
      new EvolvedSkillRegistry(this.knowledgeStore).snapshot({ includeNonPromoted: true }),
      this.impactStore.list()
    ]);
    const rolledBack = new Set(impacts.filter(item => Boolean(item.rolledBackAtMs)).map(item => item.knowledgeId));
    const eligible = snapshot.skills
      .filter(record => ['validated', 'promoted'].includes(record.status))
      .filter(record => !rolledBack.has(record.knowledgeId))
      .filter(record => matchesSkill(skill, record))
      .sort((left, right) => right.generation - left.generation || left.knowledgeId.localeCompare(right.knowledgeId));
    if (!eligible.length) return undefined;
    const generation = eligible[0]!.generation;
    const latest = eligible.filter(record => record.generation === generation);
    if (latest.length !== 1) return undefined;
    const record = latest[0]!;
    const bucket = canaryCohortBucket(record.knowledgeId, sampleKey);
    const promoted = record.status === 'promoted';
    return {
      knowledgeId: record.knowledgeId,
      implementation: EVOLVED_SKILL_CANARY_IMPLEMENTATION,
      eligible: true,
      stage: promoted ? 'promoted' : 'canary',
      applied: promoted || bucket < CANARY_PERCENT,
      bucket,
      alreadyApplied: skill.evolution?.knowledgeId === record.knowledgeId,
      record
    };
  }
}

function failureImproved(controlRate: number, interventionRate: number): boolean {
  if (controlRate <= 0) return false;
  const absoluteImprovement = controlRate - interventionRate;
  const relativeImprovement = absoluteImprovement / controlRate;
  return absoluteImprovement >= MIN_ABSOLUTE_FAILURE_IMPROVEMENT
    || relativeImprovement >= MIN_RELATIVE_FAILURE_IMPROVEMENT;
}

export async function evaluateEvolvedSkillPromotions(
  knowledgeStore: KnowledgeStore,
  canaryImpactStore: CanaryImpactStore,
  timestampMs: number
): Promise<{ evaluated: number; promoted: number }> {
  const [records, impacts] = await Promise.all([knowledgeStore.list(), canaryImpactStore.list()]);
  const knowledge = new Map(records.map(record => [record.id, record]));
  let evaluated = 0;
  let promoted = 0;
  for (const impact of impacts) {
    if (impact.implementation !== EVOLVED_SKILL_CANARY_IMPLEMENTATION || impact.rolledBackAtMs || impact.promotedAtMs) continue;
    const record = knowledge.get(impact.knowledgeId);
    if (!record || record.target !== 'evolved_skill' || record.status !== 'validated') continue;
    if (impact.interventionCalls < MIN_INTERVENTION_CALLS || impact.controlCalls < MIN_CONTROL_CALLS) continue;
    evaluated += 1;
    if (impact.interventionTaskRollbacks > 0) continue;
    const taskOutcomes = taskOutcomeCount(impact, 'intervention');
    if (taskOutcomes < MIN_TASK_OUTCOMES) continue;
    if (ratio(impact.interventionTaskVerifiedSuccesses, taskOutcomes) < MIN_VERIFIED_TASK_RATE) continue;
    if (ratio(impact.interventionTaskFailures, taskOutcomes) > MAX_TASK_FAILURE_RATE) continue;
    const interventionFailureRate = ratio(impact.interventionFailures, impact.interventionCalls);
    const controlFailureRate = ratio(impact.controlFailures, impact.controlCalls);
    if (interventionFailureRate > MAX_INTERVENTION_FAILURE_RATE) continue;
    if (!failureImproved(controlFailureRate, interventionFailureRate)) continue;
    const interventionAverageMs = ratio(impact.interventionDurationMs, impact.interventionCalls);
    const controlAverageMs = ratio(impact.controlDurationMs, impact.controlCalls);
    if (controlAverageMs > 0 && interventionAverageMs > controlAverageMs * MAX_DURATION_REGRESSION_RATIO) continue;
    const transitioned = await knowledgeStore.transitionStatus(record.id, ['validated'], 'promoted', timestampMs);
    if (!transitioned) continue;
    try {
      if (!await canaryImpactStore.markPromoted(record.id, timestampMs)) {
        await knowledgeStore.transitionStatus(record.id, ['promoted'], 'validated', timestampMs);
        continue;
      }
    } catch (error) {
      await knowledgeStore.transitionStatus(record.id, ['promoted'], 'validated', timestampMs).catch(() => false);
      throw error;
    }
    promoted += 1;
  }
  return { evaluated, promoted };
}

function promotedDelta(impact: CanaryImpactRecord) {
  return {
    calls: Math.max(0, impact.interventionCalls - (impact.promotionInterventionCalls ?? impact.interventionCalls)),
    failures: Math.max(0, impact.interventionFailures - (impact.promotionInterventionFailures ?? impact.interventionFailures)),
    durationMs: Math.max(0, impact.interventionDurationMs - (impact.promotionInterventionDurationMs ?? impact.interventionDurationMs)),
    taskVerifiedSuccesses: Math.max(0, impact.interventionTaskVerifiedSuccesses - (impact.promotionInterventionTaskVerifiedSuccesses ?? impact.interventionTaskVerifiedSuccesses)),
    taskUnverifiedSuccesses: Math.max(0, impact.interventionTaskUnverifiedSuccesses - (impact.promotionInterventionTaskUnverifiedSuccesses ?? impact.interventionTaskUnverifiedSuccesses)),
    taskFailures: Math.max(0, impact.interventionTaskFailures - (impact.promotionInterventionTaskFailures ?? impact.interventionTaskFailures)),
    taskRollbacks: Math.max(0, impact.interventionTaskRollbacks - (impact.promotionInterventionTaskRollbacks ?? impact.interventionTaskRollbacks))
  };
}

export async function evaluateEvolvedSkillRollbacks(
  knowledgeStore: KnowledgeStore,
  canaryImpactStore: CanaryImpactStore,
  timestampMs: number
): Promise<{ evaluated: number; rolledBack: number }> {
  const [records, impacts] = await Promise.all([knowledgeStore.list(), canaryImpactStore.list()]);
  const knowledge = new Map(records.map(record => [record.id, record]));
  let evaluated = 0;
  let rolledBack = 0;
  for (const impact of impacts) {
    if (impact.implementation !== EVOLVED_SKILL_CANARY_IMPLEMENTATION || impact.rolledBackAtMs) continue;
    const record = knowledge.get(impact.knowledgeId);
    if (!record || record.target !== 'evolved_skill' || !['validated', 'promoted'].includes(record.status)) continue;
    if (impact.interventionCalls === 0) continue;
    evaluated += 1;
    const controlFailureRate = ratio(impact.controlFailures, impact.controlCalls);
    const controlAverageMs = ratio(impact.controlDurationMs, impact.controlCalls);
    let reason: string | undefined;
    if (record.status === 'promoted') {
      const delta = promotedDelta(impact);
      if (delta.taskRollbacks > 0) reason = 'promoted_task_rollback';
      else if (delta.calls >= POST_PROMOTION_MIN_CALLS) {
        const failureRate = ratio(delta.failures, delta.calls);
        if (failureRate > Math.max(MAX_INTERVENTION_FAILURE_RATE, controlFailureRate + MAX_ROLLBACK_FAILURE_RATE_DELTA)) {
          reason = 'promoted_edit_failure_rate_regression';
        }
        const averageMs = ratio(delta.durationMs, delta.calls);
        if (!reason && controlAverageMs > 0 && averageMs > controlAverageMs * MAX_DURATION_REGRESSION_RATIO) {
          reason = 'promoted_edit_duration_regression';
        }
        const taskOutcomes = delta.taskVerifiedSuccesses + delta.taskUnverifiedSuccesses + delta.taskFailures + delta.taskRollbacks;
        if (!reason && taskOutcomes >= MIN_TASK_OUTCOMES && ratio(delta.taskFailures, taskOutcomes) > 0.3) {
          reason = 'promoted_task_failure_regression';
        }
      }
    } else if (impact.interventionTaskRollbacks > 0) reason = 'intervention_task_rollback';
    else if (impact.interventionCalls >= 10) {
      const failureRate = ratio(impact.interventionFailures, impact.interventionCalls);
      if (failureRate > Math.max(MAX_INTERVENTION_FAILURE_RATE, controlFailureRate + MAX_ROLLBACK_FAILURE_RATE_DELTA)) {
        reason = 'intervention_edit_failure_rate_regression';
      }
      const averageMs = ratio(impact.interventionDurationMs, impact.interventionCalls);
      if (!reason && controlAverageMs > 0 && averageMs > controlAverageMs * MAX_DURATION_REGRESSION_RATIO) {
        reason = 'intervention_edit_duration_regression';
      }
      const taskOutcomes = taskOutcomeCount(impact, 'intervention');
      if (!reason && taskOutcomes >= MIN_TASK_OUTCOMES && ratio(impact.interventionTaskFailures, taskOutcomes) > 0.3) {
        reason = 'intervention_task_failure_regression';
      }
    }
    if (!reason) continue;
    const transitioned = await knowledgeStore.transitionStatus(record.id, [record.status], 'rejected', timestampMs);
    if (!transitioned) continue;
    try {
      if (!await canaryImpactStore.markRolledBack(record.id, reason, timestampMs)) {
        await knowledgeStore.transitionStatus(record.id, ['rejected'], record.status, timestampMs);
        continue;
      }
    } catch (error) {
      await knowledgeStore.transitionStatus(record.id, ['rejected'], record.status, timestampMs).catch(() => false);
      throw error;
    }
    rolledBack += 1;
  }
  return { evaluated, rolledBack };
}
