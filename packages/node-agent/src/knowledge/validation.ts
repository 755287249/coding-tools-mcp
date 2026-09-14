import { KnowledgeImpactStore, type KnowledgeImpactRecord } from './impact.js';
import { KnowledgeStore } from './store.js';
import type { KnowledgeRecord } from './types.js';

const MIN_SHADOW_MATCHES = 10;
const MIN_TASK_OUTCOMES = 3;
const MIN_VERIFIED_TASK_RATE = 0.8;
const MAX_TASK_FAILURE_RATE = 0.2;
const MAX_TOOL_FAILURE_RATE = 0.25;
const TOOL_EVOLUTION_FALLBACK_MATCHES = 20;

export interface ValidationGateResult {
  evaluated: number;
  validated: number;
  blocked: number;
}

function ratio(numerator: number, denominator: number): number {
  return denominator > 0 ? numerator / denominator : 0;
}

function taskOutcomeCount(impact: KnowledgeImpactRecord): number {
  return impact.taskVerifiedSuccesses
    + impact.taskUnverifiedSuccesses
    + impact.taskFailures
    + impact.taskRollbacks;
}

function toolStrategyReady(impact: KnowledgeImpactRecord): boolean {
  const taskOutcomes = taskOutcomeCount(impact);
  if (impact.matched < MIN_SHADOW_MATCHES || taskOutcomes < MIN_TASK_OUTCOMES) return false;
  if (impact.taskRollbacks > 0) return false;
  if (ratio(impact.taskVerifiedSuccesses, taskOutcomes) < MIN_VERIFIED_TASK_RATE) return false;
  if (ratio(impact.taskFailures, taskOutcomes) > MAX_TASK_FAILURE_RATE) return false;
  if (ratio(impact.actualFailures, impact.matched) > MAX_TOOL_FAILURE_RATE) return false;
  return true;
}

function toolEvolutionReady(impact: KnowledgeImpactRecord): boolean {
  if (impact.matched < MIN_SHADOW_MATCHES) return false;
  const averageDurationMs = ratio(impact.totalObservedDurationMs, impact.matched);
  if (averageDurationMs < 1_000) return false;
  const taskOutcomes = taskOutcomeCount(impact);
  if (taskOutcomes < MIN_TASK_OUTCOMES) return impact.matched >= TOOL_EVOLUTION_FALLBACK_MATCHES;
  if (impact.taskRollbacks > 0) return false;
  return ratio(impact.taskFailures, taskOutcomes) <= MAX_TASK_FAILURE_RATE;
}

function evolvedSkillReady(impact: KnowledgeImpactRecord): boolean {
  const taskOutcomes = taskOutcomeCount(impact);
  if (impact.matched < MIN_SHADOW_MATCHES || taskOutcomes < MIN_TASK_OUTCOMES) return false;
  if (impact.taskRollbacks > 0) return false;
  if (ratio(impact.taskVerifiedSuccesses, taskOutcomes) < MIN_VERIFIED_TASK_RATE) return false;
  return ratio(impact.taskFailures, taskOutcomes) <= MAX_TASK_FAILURE_RATE;
}

function validationReady(record: KnowledgeRecord, impact: KnowledgeImpactRecord): boolean {
  if (record.target === 'tool_strategy') return toolStrategyReady(impact);
  if (record.target === 'tool_evolution') return toolEvolutionReady(impact);
  if (record.target === 'evolved_skill') return evolvedSkillReady(impact);
  return false;
}

export class ValidationGate {
  constructor(
    private readonly knowledgeStore: KnowledgeStore,
    private readonly impactStore: KnowledgeImpactStore
  ) {}

  async evaluate(updatedAtMs: number): Promise<ValidationGateResult> {
    const impacts = new Map((await this.impactStore.list()).map(impact => [impact.knowledgeId, impact]));
    let evaluated = 0;
    let validated = 0;
    let blocked = 0;
    for (const record of await this.knowledgeStore.list()) {
      if (record.status !== 'shadow') continue;
      if (!['tool_strategy', 'tool_evolution', 'evolved_skill'].includes(record.target)) continue;
      const impact = impacts.get(record.id);
      if (!impact) continue;
      evaluated += 1;
      if (!validationReady(record, impact)) {
        blocked += 1;
        continue;
      }
      if (await this.knowledgeStore.transitionStatus(record.id, ['shadow'], 'validated', updatedAtMs)) {
        validated += 1;
      }
    }
    return { evaluated, validated, blocked };
  }
}
