import { createHash } from 'node:crypto';
import { mkdir, readFile, rename, writeFile } from 'node:fs/promises';
import path from 'node:path';
import type { JsonObject } from '../types.js';
import { KnowledgeImpactStore } from './impact.js';
import { KnowledgeStore } from './store.js';
import { resolveToolStrategyImplementation, type ToolStrategyImplementationId } from './strategyImplementations.js';
import type { ExperienceObservation, KnowledgeRecord, TaskOutcome } from './types.js';

const CANARY_PERCENT = 25;
const MIN_INTERVENTION_CALLS = 10;
const MIN_CONTROL_CALLS = 5;
const MAX_DURATION_REGRESSION_RATIO = 1.25;
const MAX_FAILURE_RATE = 0.25;
const MAX_FAILURE_RATE_DELTA = 0.15;
const MIN_PROMOTION_INTERVENTION_CALLS = 20;
const MIN_PROMOTION_CONTROL_CALLS = 10;
const MIN_PROMOTION_TASK_OUTCOMES = 3;
const MIN_PROMOTION_VERIFIED_TASK_RATE = 0.8;
const MAX_PROMOTION_FAILURE_RATE = 0.1;
const MAX_PROMOTION_FAILURE_RATE_DELTA = 0.05;
const MAX_PROMOTION_DURATION_RATIO = 0.9;
const POST_PROMOTION_MIN_CALLS = 10;

export interface KnowledgeCanaryDecision {
  knowledgeId: string;
  implementation: ToolStrategyImplementationId;
  eligible: true;
  stage: 'canary' | 'promoted';
  applied: boolean;
  bucket: number;
}

export interface CanaryImpactRecord {
  schemaVersion: 1;
  knowledgeId: string;
  implementation: string;
  controlEventIds: string[];
  interventionEventIds: string[];
  controlTaskIds: string[];
  interventionTaskIds: string[];
  creditedControlTaskIds: string[];
  creditedInterventionTaskIds: string[];
  controlCalls: number;
  interventionCalls: number;
  controlSuccesses: number;
  controlFailures: number;
  interventionSuccesses: number;
  interventionFailures: number;
  controlDurationMs: number;
  interventionDurationMs: number;
  controlTaskVerifiedSuccesses: number;
  controlTaskUnverifiedSuccesses: number;
  controlTaskFailures: number;
  controlTaskRollbacks: number;
  interventionTaskVerifiedSuccesses: number;
  interventionTaskUnverifiedSuccesses: number;
  interventionTaskFailures: number;
  interventionTaskRollbacks: number;
  firstSeenAtMs: number;
  lastSeenAtMs: number;
  promotedAtMs?: number;
  promotionInterventionCalls?: number;
  promotionInterventionFailures?: number;
  promotionInterventionDurationMs?: number;
  promotionInterventionTaskVerifiedSuccesses?: number;
  promotionInterventionTaskUnverifiedSuccesses?: number;
  promotionInterventionTaskFailures?: number;
  promotionInterventionTaskRollbacks?: number;
  rolledBackAtMs?: number;
  rollbackReason?: string;
}

interface CanaryImpactDocument {
  schemaVersion: 1;
  records: CanaryImpactRecord[];
}

function unique(values: readonly string[]): string[] {
  return [...new Set(values)].sort();
}

function stableValue(value: unknown): unknown {
  if (Array.isArray(value)) return value.map(stableValue);
  if (!value || typeof value !== 'object') return value;
  return Object.fromEntries(Object.entries(value as Record<string, unknown>)
    .sort(([left], [right]) => left.localeCompare(right))
    .map(([key, child]) => [key, stableValue(child)]));
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

function addTaskOutcome(record: CanaryImpactRecord, side: 'control' | 'intervention', outcome: TaskOutcome): void {
  if (side === 'control') {
    if (outcome === 'verified_success') record.controlTaskVerifiedSuccesses += 1;
    else if (outcome === 'unverified_success') record.controlTaskUnverifiedSuccesses += 1;
    else if (outcome === 'failure') record.controlTaskFailures += 1;
    else record.controlTaskRollbacks += 1;
    return;
  }
  if (outcome === 'verified_success') record.interventionTaskVerifiedSuccesses += 1;
  else if (outcome === 'unverified_success') record.interventionTaskUnverifiedSuccesses += 1;
  else if (outcome === 'failure') record.interventionTaskFailures += 1;
  else record.interventionTaskRollbacks += 1;
}

function emptyRecord(observation: ExperienceObservation): CanaryImpactRecord {
  const canary = observation.canary!;
  return {
    schemaVersion: 1,
    knowledgeId: canary.knowledgeId,
    implementation: canary.implementation,
    controlEventIds: [],
    interventionEventIds: [],
    controlTaskIds: [],
    interventionTaskIds: [],
    creditedControlTaskIds: [],
    creditedInterventionTaskIds: [],
    controlCalls: 0,
    interventionCalls: 0,
    controlSuccesses: 0,
    controlFailures: 0,
    interventionSuccesses: 0,
    interventionFailures: 0,
    controlDurationMs: 0,
    interventionDurationMs: 0,
    controlTaskVerifiedSuccesses: 0,
    controlTaskUnverifiedSuccesses: 0,
    controlTaskFailures: 0,
    controlTaskRollbacks: 0,
    interventionTaskVerifiedSuccesses: 0,
    interventionTaskUnverifiedSuccesses: 0,
    interventionTaskFailures: 0,
    interventionTaskRollbacks: 0,
    firstSeenAtMs: observation.timestampMs,
    lastSeenAtMs: observation.timestampMs
  };
}

export function canaryCohortBucket(knowledgeId: string, sampleKey: string): number {
  const digest = createHash('sha256').update(`${knowledgeId}\u0000${sampleKey}`).digest();
  return digest.readUInt32BE(0) % 100;
}

export class CanaryImpactStore {
  private readonly filePath: string;

  constructor(readonly rootDir: string) {
    this.filePath = path.join(rootDir, 'canary-impact.json');
  }

  async list(): Promise<CanaryImpactRecord[]> {
    const document = await this.readDocument();
    return document.records.map(record => structuredClone(record));
  }

  async recordObservations(observations: readonly ExperienceObservation[]): Promise<number> {
    const relevant = observations.filter(observation => observation.canary?.eligible === true);
    if (!relevant.length) return 0;
    const document = await this.readDocument();
    let recorded = 0;
    for (const observation of relevant) {
      const canary = observation.canary!;
      let record = document.records.find(item => item.knowledgeId === canary.knowledgeId);
      if (!record) {
        record = emptyRecord(observation);
        document.records.push(record);
      }
      const side = canary.applied ? 'intervention' : 'control';
      const eventIds = side === 'intervention' ? record.interventionEventIds : record.controlEventIds;
      if (eventIds.includes(observation.eventId)) continue;
      if (side === 'intervention') {
        record.interventionEventIds = unique([...record.interventionEventIds, observation.eventId]);
        record.interventionCalls += 1;
        if (observation.outcome === 'success') record.interventionSuccesses += 1;
        else record.interventionFailures += 1;
        record.interventionDurationMs += Math.max(0, observation.durationMs);
        if (observation.taskId
          && !record.controlTaskIds.includes(observation.taskId)
          && !record.interventionTaskIds.includes(observation.taskId)) {
          record.interventionTaskIds = unique([...record.interventionTaskIds, observation.taskId]);
        }
      } else {
        record.controlEventIds = unique([...record.controlEventIds, observation.eventId]);
        record.controlCalls += 1;
        if (observation.outcome === 'success') record.controlSuccesses += 1;
        else record.controlFailures += 1;
        record.controlDurationMs += Math.max(0, observation.durationMs);
        if (observation.taskId
          && !record.controlTaskIds.includes(observation.taskId)
          && !record.interventionTaskIds.includes(observation.taskId)) {
          record.controlTaskIds = unique([...record.controlTaskIds, observation.taskId]);
        }
      }
      record.firstSeenAtMs = Math.min(record.firstSeenAtMs, observation.timestampMs);
      record.lastSeenAtMs = Math.max(record.lastSeenAtMs, observation.timestampMs);
      recorded += 1;
    }
    if (recorded > 0) {
      document.records.sort((left, right) => left.knowledgeId.localeCompare(right.knowledgeId));
      await this.writeDocument(document);
    }
    return recorded;
  }

  async creditTaskOutcome(taskId: string, outcome: TaskOutcome, timestampMs: number): Promise<number> {
    const document = await this.readDocument();
    let credited = 0;
    for (const record of document.records) {
      for (const side of ['control', 'intervention'] as const) {
        const taskIds = side === 'control' ? record.controlTaskIds : record.interventionTaskIds;
        const creditedTaskIds = side === 'control' ? record.creditedControlTaskIds : record.creditedInterventionTaskIds;
        if (!taskIds.includes(taskId) || creditedTaskIds.includes(taskId)) continue;
        if (side === 'control') record.creditedControlTaskIds = unique([...record.creditedControlTaskIds, taskId]);
        else record.creditedInterventionTaskIds = unique([...record.creditedInterventionTaskIds, taskId]);
        addTaskOutcome(record, side, outcome);
        record.lastSeenAtMs = Math.max(record.lastSeenAtMs, timestampMs);
        credited += 1;
      }
    }
    if (credited > 0) await this.writeDocument(document);
    return credited;
  }

  async markPromoted(knowledgeId: string, timestampMs: number): Promise<boolean> {
    const document = await this.readDocument();
    const record = document.records.find(item => item.knowledgeId === knowledgeId);
    if (!record || record.promotedAtMs || record.rolledBackAtMs) return false;
    record.promotedAtMs = timestampMs;
    record.promotionInterventionCalls = record.interventionCalls;
    record.promotionInterventionFailures = record.interventionFailures;
    record.promotionInterventionDurationMs = record.interventionDurationMs;
    record.promotionInterventionTaskVerifiedSuccesses = record.interventionTaskVerifiedSuccesses;
    record.promotionInterventionTaskUnverifiedSuccesses = record.interventionTaskUnverifiedSuccesses;
    record.promotionInterventionTaskFailures = record.interventionTaskFailures;
    record.promotionInterventionTaskRollbacks = record.interventionTaskRollbacks;
    record.lastSeenAtMs = Math.max(record.lastSeenAtMs, timestampMs);
    await this.writeDocument(document);
    return true;
  }

  async markRolledBack(knowledgeId: string, reason: string, timestampMs: number): Promise<boolean> {
    const document = await this.readDocument();
    const record = document.records.find(item => item.knowledgeId === knowledgeId);
    if (!record || record.rolledBackAtMs) return false;
    record.rolledBackAtMs = timestampMs;
    record.rollbackReason = reason;
    record.lastSeenAtMs = Math.max(record.lastSeenAtMs, timestampMs);
    await this.writeDocument(document);
    return true;
  }

  async revision(): Promise<string> {
    return createHash('sha256').update(JSON.stringify(stableValue(await this.list()))).digest('hex');
  }

  private async readDocument(): Promise<CanaryImpactDocument> {
    try {
      const parsed = JSON.parse(await readFile(this.filePath, 'utf8')) as CanaryImpactDocument;
      if (parsed?.schemaVersion !== 1 || !Array.isArray(parsed.records)) throw new Error('Unsupported canary impact schema.');
      return parsed;
    } catch (error) {
      if ((error as NodeJS.ErrnoException).code === 'ENOENT') return { schemaVersion: 1, records: [] };
      throw error;
    }
  }

  private async writeDocument(document: CanaryImpactDocument): Promise<void> {
    await mkdir(this.rootDir, { recursive: true });
    const tempPath = `${this.filePath}.tmp`;
    await writeFile(tempPath, `${JSON.stringify(document, null, 2)}\n`, { encoding: 'utf8', mode: 0o600 });
    await rename(tempPath, this.filePath);
  }
}

export class CanaryStrategyEngine {
  private loaded = false;
  private strategies: KnowledgeRecord[] = [];
  private rolledBack = new Set<string>();

  constructor(
    private readonly knowledgeStore: KnowledgeStore,
    private readonly impactStore: CanaryImpactStore
  ) {}

  async refresh(): Promise<void> {
    const [records, impacts] = await Promise.all([this.knowledgeStore.list(), this.impactStore.list()]);
    this.strategies = records.filter(record => record.target === 'tool_strategy'
      && (record.status === 'validated' || record.status === 'promoted'));
    this.rolledBack = new Set(impacts.filter(record => Boolean(record.rolledBackAtMs)).map(record => record.knowledgeId));
    this.loaded = true;
  }

  async decide(tool: string, args: JsonObject, sampleKey: string): Promise<KnowledgeCanaryDecision | undefined> {
    if (!this.loaded) await this.refresh();
    for (const record of this.strategies) {
      if (this.rolledBack.has(record.id)) continue;
      const implementation = resolveToolStrategyImplementation(record, tool, args);
      if (!implementation) continue;
      const bucket = canaryCohortBucket(record.id, sampleKey);
      return {
        knowledgeId: record.id,
        implementation: implementation.id,
        eligible: true,
        stage: record.status === 'promoted' ? 'promoted' : 'canary',
        applied: record.status === 'promoted' || bucket < CANARY_PERCENT,
        bucket
      };
    }
    return undefined;
  }
}

export async function evaluateCanaryPromotions(
  knowledgeStore: KnowledgeStore,
  canaryImpactStore: CanaryImpactStore,
  timestampMs: number
): Promise<{ evaluated: number; promoted: number }> {
  const [records, impacts] = await Promise.all([
    knowledgeStore.list(),
    canaryImpactStore.list()
  ]);
  const knowledge = new Map(records.map(record => [record.id, record]));
  let evaluated = 0;
  let promoted = 0;
  for (const impact of impacts) {
    if (impact.rolledBackAtMs || impact.promotedAtMs) continue;
    const record = knowledge.get(impact.knowledgeId);
    if (!record || record.target !== 'tool_strategy' || record.status !== 'validated') continue;
    if (impact.interventionCalls < MIN_PROMOTION_INTERVENTION_CALLS
      || impact.controlCalls < MIN_PROMOTION_CONTROL_CALLS) continue;
    evaluated += 1;
    if (impact.interventionTaskRollbacks > 0) continue;
    const taskOutcomes = taskOutcomeCount(impact, 'intervention');
    if (taskOutcomes < MIN_PROMOTION_TASK_OUTCOMES) continue;
    if (ratio(impact.interventionTaskVerifiedSuccesses, taskOutcomes) < MIN_PROMOTION_VERIFIED_TASK_RATE) continue;
    const interventionFailureRate = ratio(impact.interventionFailures, impact.interventionCalls);
    const controlFailureRate = ratio(impact.controlFailures, impact.controlCalls);
    if (interventionFailureRate > MAX_PROMOTION_FAILURE_RATE) continue;
    if (interventionFailureRate > controlFailureRate + MAX_PROMOTION_FAILURE_RATE_DELTA) continue;
    const interventionAverageMs = ratio(impact.interventionDurationMs, impact.interventionCalls);
    const controlAverageMs = ratio(impact.controlDurationMs, impact.controlCalls);
    if (controlAverageMs <= 0 || interventionAverageMs > controlAverageMs * MAX_PROMOTION_DURATION_RATIO) continue;
    const transitioned = await knowledgeStore.transitionStatus(impact.knowledgeId, ['validated'], 'promoted', timestampMs);
    if (!transitioned) continue;
    try {
      if (!await canaryImpactStore.markPromoted(impact.knowledgeId, timestampMs)) {
        await knowledgeStore.transitionStatus(impact.knowledgeId, ['promoted'], 'validated', timestampMs);
        continue;
      }
    } catch (error) {
      await knowledgeStore.transitionStatus(impact.knowledgeId, ['promoted'], 'validated', timestampMs).catch(() => false);
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

export async function evaluateCanaryRollbacks(
  knowledgeStore: KnowledgeStore,
  shadowImpactStore: KnowledgeImpactStore,
  canaryImpactStore: CanaryImpactStore,
  timestampMs: number
): Promise<{ evaluated: number; rolledBack: number }> {
  const [records, shadowImpacts, canaryImpacts] = await Promise.all([
    knowledgeStore.list(),
    shadowImpactStore.list(),
    canaryImpactStore.list()
  ]);
  const knowledge = new Map(records.map(record => [record.id, record]));
  const baseline = new Map(shadowImpacts.map(record => [record.knowledgeId, record]));
  let evaluated = 0;
  let rolledBack = 0;
  for (const impact of canaryImpacts) {
    if (impact.rolledBackAtMs) continue;
    const record = knowledge.get(impact.knowledgeId);
    if (!record || record.target !== 'tool_strategy'
      || (record.status !== 'validated' && record.status !== 'promoted')) continue;
    if (impact.interventionCalls === 0) continue;
    evaluated += 1;
    let reason: string | undefined;
    const shadow = baseline.get(impact.knowledgeId);
    const baselineAverageMs = impact.controlCalls >= MIN_CONTROL_CALLS
      ? ratio(impact.controlDurationMs, impact.controlCalls)
      : shadow && shadow.matched > 0 ? ratio(shadow.totalObservedDurationMs, shadow.matched) : 0;
    const controlFailureRate = ratio(impact.controlFailures, impact.controlCalls);
    if (record.status === 'promoted') {
      const delta = promotedDelta(impact);
      if (delta.taskRollbacks > 0) {
        reason = 'promoted_task_rollback';
      } else if (delta.calls >= POST_PROMOTION_MIN_CALLS) {
        const postFailureRate = ratio(delta.failures, delta.calls);
        if (postFailureRate > Math.max(MAX_FAILURE_RATE, controlFailureRate + MAX_FAILURE_RATE_DELTA)) {
          reason = 'promoted_failure_rate_regression';
        }
        const postAverageMs = ratio(delta.durationMs, delta.calls);
        if (!reason && baselineAverageMs > 0 && postAverageMs > baselineAverageMs * MAX_DURATION_REGRESSION_RATIO) {
          reason = 'promoted_duration_regression';
        }
        const postTaskOutcomes = delta.taskVerifiedSuccesses
          + delta.taskUnverifiedSuccesses
          + delta.taskFailures
          + delta.taskRollbacks;
        if (!reason && postTaskOutcomes >= 3 && ratio(delta.taskFailures, postTaskOutcomes) > 0.3) {
          reason = 'promoted_task_failure_regression';
        }
      }
    } else if (impact.interventionTaskRollbacks > 0) {
      reason = 'intervention_task_rollback';
    } else if (impact.interventionCalls >= MIN_INTERVENTION_CALLS) {
      const interventionFailureRate = ratio(impact.interventionFailures, impact.interventionCalls);
      if (interventionFailureRate > Math.max(MAX_FAILURE_RATE, controlFailureRate + MAX_FAILURE_RATE_DELTA)) {
        reason = 'intervention_failure_rate_regression';
      }
      const interventionAverageMs = ratio(impact.interventionDurationMs, impact.interventionCalls);
      if (!reason && baselineAverageMs > 0 && interventionAverageMs > baselineAverageMs * MAX_DURATION_REGRESSION_RATIO) {
        reason = 'intervention_duration_regression';
      }
      const interventionTaskOutcomes = taskOutcomeCount(impact, 'intervention');
      if (!reason && interventionTaskOutcomes >= 3
        && ratio(impact.interventionTaskFailures, interventionTaskOutcomes) > 0.3) {
        reason = 'intervention_task_failure_regression';
      }
    }
    if (!reason) continue;
    const transitioned = await knowledgeStore.transitionStatus(impact.knowledgeId, [record.status], 'rejected', timestampMs);
    if (!transitioned) continue;
    try {
      if (!await canaryImpactStore.markRolledBack(impact.knowledgeId, reason, timestampMs)) {
        await knowledgeStore.transitionStatus(impact.knowledgeId, ['rejected'], record.status, timestampMs);
        continue;
      }
    } catch (error) {
      await knowledgeStore.transitionStatus(impact.knowledgeId, ['rejected'], record.status, timestampMs).catch(() => false);
      throw error;
    }
    rolledBack += 1;
  }
  return { evaluated, rolledBack };
}
