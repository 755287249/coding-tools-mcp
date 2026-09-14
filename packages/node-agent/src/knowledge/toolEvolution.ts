import { createHash } from 'node:crypto';
import { mkdir, readFile, rename, rm, writeFile } from 'node:fs/promises';
import path from 'node:path';
import type {
  KnowledgeRecord,
  ToolEvolutionChangeProposalRecord,
  ToolEvolutionBenchmarkAggregate,
  ToolEvolutionBenchmarkSample,
  ToolEvolutionExperimentPlan,
  ToolEvolutionExperimentRecord,
  ToolEvolutionExperimentStatus,
  ToolEvolutionProposalType
} from './types.js';
import { KnowledgeStore } from './store.js';

const MAX_SAMPLE_DIGESTS_PER_ARM = 2_048;
const MIN_BENCHMARK_SAMPLES = 5;
const MIN_TASK_OUTCOMES = 3;
const REQUIRED_IMPROVEMENT_RATIO = 0.9;
const MAX_EFFICIENCY_REGRESSION_RATIO = 1.05;
const MAX_PROPOSAL_ACTION_BYTES = 8_192;

interface ToolEvolutionProposalDocument {
  schemaVersion: 1;
  records: ToolEvolutionChangeProposalRecord[];
}

interface ToolEvolutionExperimentDocument {
  schemaVersion: 1;
  records: ToolEvolutionExperimentRecord[];
}

interface ToolEvolutionLifecycleTransactionDocument {
  schemaVersion: 1;
  state: 'prepared' | 'committed';
  proposalBeforeExists: boolean;
  experimentBeforeExists: boolean;
  proposalBefore: ToolEvolutionProposalDocument;
  proposalAfter: ToolEvolutionProposalDocument;
  experimentBefore: ToolEvolutionExperimentDocument;
  experimentAfter: ToolEvolutionExperimentDocument;
}

const lifecycleRecoveryChecked = new Set<string>();
const lifecycleMutationTails = new Map<string, Promise<void>>();

function stableValue(value: unknown): unknown {
  if (Array.isArray(value)) return value.map(stableValue);
  if (!value || typeof value !== 'object') return value;
  return Object.fromEntries(Object.entries(value as Record<string, unknown>)
    .sort(([left], [right]) => left.localeCompare(right))
    .map(([key, child]) => [key, stableValue(child)]));
}

function stableJson(value: unknown): string {
  return JSON.stringify(stableValue(value));
}

function proposalFilePath(rootDir: string): string {
  return path.join(rootDir, 'tool-evolution-proposals.json');
}

function experimentFilePath(rootDir: string): string {
  return path.join(rootDir, 'tool-evolution-experiments.json');
}

function lifecycleTransactionFilePath(rootDir: string): string {
  return path.join(rootDir, 'tool-evolution-lifecycle-transaction.json');
}

async function writePrivateJsonAtomic(rootDir: string, filePath: string, document: unknown): Promise<void> {
  await mkdir(rootDir, { recursive: true });
  const tempPath = `${filePath}.tmp`;
  await writeFile(tempPath, `${JSON.stringify(document, null, 2)}\n`, { encoding: 'utf8', mode: 0o600 });
  await rename(tempPath, filePath);
}

async function readProposalDocumentRaw(rootDir: string): Promise<{ exists: boolean; document: ToolEvolutionProposalDocument }> {
  const filePath = proposalFilePath(rootDir);
  try {
    const parsed = JSON.parse(await readFile(filePath, 'utf8')) as ToolEvolutionProposalDocument;
    if (parsed?.schemaVersion !== 1 || !Array.isArray(parsed.records)) throw new Error('Unsupported tool evolution proposal schema.');
    return { exists: true, document: parsed };
  } catch (error) {
    if ((error as NodeJS.ErrnoException).code === 'ENOENT') return { exists: false, document: { schemaVersion: 1, records: [] } };
    throw error;
  }
}

async function readExperimentDocumentRaw(rootDir: string): Promise<{ exists: boolean; document: ToolEvolutionExperimentDocument }> {
  const filePath = experimentFilePath(rootDir);
  try {
    const parsed = JSON.parse(await readFile(filePath, 'utf8')) as ToolEvolutionExperimentDocument;
    if (parsed?.schemaVersion !== 1 || !Array.isArray(parsed.records)) throw new Error('Unsupported tool evolution experiment schema.');
    return { exists: true, document: parsed };
  } catch (error) {
    if ((error as NodeJS.ErrnoException).code === 'ENOENT') return { exists: false, document: { schemaVersion: 1, records: [] } };
    throw error;
  }
}

async function readLifecycleTransactionRaw(rootDir: string): Promise<ToolEvolutionLifecycleTransactionDocument | undefined> {
  try {
    const parsed = JSON.parse(await readFile(lifecycleTransactionFilePath(rootDir), 'utf8')) as ToolEvolutionLifecycleTransactionDocument;
    if (parsed?.schemaVersion !== 1 || !['prepared', 'committed'].includes(parsed.state)
      || !parsed.proposalBefore || !parsed.proposalAfter || !parsed.experimentBefore || !parsed.experimentAfter) {
      throw new Error('Unsupported tool evolution lifecycle transaction schema.');
    }
    return parsed;
  } catch (error) {
    if ((error as NodeJS.ErrnoException).code === 'ENOENT') return undefined;
    throw error;
  }
}

async function restoreProposalDocument(rootDir: string, exists: boolean, document: ToolEvolutionProposalDocument): Promise<void> {
  const filePath = proposalFilePath(rootDir);
  if (!exists) {
    await rm(filePath, { force: true });
    return;
  }
  const current = await readProposalDocumentRaw(rootDir);
  if (current.exists && stableJson(current.document) === stableJson(document)) return;
  await writePrivateJsonAtomic(rootDir, filePath, document);
}

async function restoreExperimentDocument(rootDir: string, exists: boolean, document: ToolEvolutionExperimentDocument): Promise<void> {
  const filePath = experimentFilePath(rootDir);
  if (!exists) {
    await rm(filePath, { force: true });
    return;
  }
  const current = await readExperimentDocumentRaw(rootDir);
  if (current.exists && stableJson(current.document) === stableJson(document)) return;
  await writePrivateJsonAtomic(rootDir, filePath, document);
}

async function recoverToolEvolutionLifecycleTransactionRaw(rootDir: string): Promise<void> {
  const transaction = await readLifecycleTransactionRaw(rootDir);
  if (!transaction) return;
  if (transaction.state === 'committed') {
    await restoreExperimentDocument(rootDir, true, transaction.experimentAfter);
    await restoreProposalDocument(rootDir, true, transaction.proposalAfter);
  } else {
    await restoreExperimentDocument(rootDir, transaction.experimentBeforeExists, transaction.experimentBefore);
    await restoreProposalDocument(rootDir, transaction.proposalBeforeExists, transaction.proposalBefore);
  }
  await rm(lifecycleTransactionFilePath(rootDir), { force: true });
}

async function ensureToolEvolutionLifecycleRecovered(rootDir: string): Promise<void> {
  const key = path.resolve(rootDir);
  const active = lifecycleMutationTails.get(key);
  if (active) await active;
  if (lifecycleRecoveryChecked.has(key)) return;
  await recoverToolEvolutionLifecycleTransactionRaw(rootDir);
  lifecycleRecoveryChecked.add(key);
}

async function withToolEvolutionLifecycleMutation<T>(rootDir: string, operation: () => Promise<T>): Promise<T> {
  const key = path.resolve(rootDir);
  const previous = lifecycleMutationTails.get(key) ?? Promise.resolve();
  let release!: () => void;
  const gate = new Promise<void>(resolve => { release = resolve; });
  const tail = previous.then(() => gate);
  lifecycleMutationTails.set(key, tail);
  await previous;
  try {
    if (!lifecycleRecoveryChecked.has(key)) {
      await recoverToolEvolutionLifecycleTransactionRaw(rootDir);
      lifecycleRecoveryChecked.add(key);
    }
    return await operation();
  } finally {
    release();
    if (lifecycleMutationTails.get(key) === tail) {
      void tail.finally(() => lifecycleMutationTails.delete(key));
    }
  }
}

function assertBoundedText(value: string, label: string, maximum = 128): string {
  const normalized = String(value ?? '').trim();
  if (!normalized || normalized.length > maximum || /[\r\n\0]/.test(normalized)) {
    throw new Error(`${label} must be non-empty bounded single-line text.`);
  }
  return normalized;
}

function boundedInteger(value: number, label: string, maximum: number): number {
  if (!Number.isSafeInteger(value) || value < 0 || value > maximum) {
    throw new Error(`${label} must be a non-negative bounded integer.`);
  }
  return value;
}

function boundedNumber(value: number, label: string, maximum: number): number {
  if (!Number.isFinite(value) || value < 0 || value > maximum) {
    throw new Error(`${label} must be a non-negative bounded number.`);
  }
  return value;
}

function emptyAggregate(): ToolEvolutionBenchmarkAggregate {
  return {
    samples: 0,
    taskOutcomes: 0,
    verifiedTaskSuccesses: 0,
    unverifiedTaskSuccesses: 0,
    taskFailures: 0,
    taskRollbacks: 0,
    toolCalls: 0,
    toolFailures: 0,
    durationMs: 0,
    responseBytes: 0,
    sampleDigests: []
  };
}

function normalizeSample(sample: ToolEvolutionBenchmarkSample): ToolEvolutionBenchmarkSample {
  const sampleId = assertBoundedText(sample.sampleId, 'sampleId');
  const sourceRevision = assertBoundedText(sample.sourceRevision, 'sourceRevision', 128);
  const toolCalls = boundedInteger(sample.toolCalls, 'toolCalls', 1_000_000);
  const toolFailures = boundedInteger(sample.toolFailures, 'toolFailures', 1_000_000);
  if (toolFailures > toolCalls) throw new Error('toolFailures cannot exceed toolCalls.');
  const taskOutcome = sample.taskOutcome;
  if (taskOutcome && !['verified_success', 'unverified_success', 'failure', 'rolled_back'].includes(taskOutcome)) {
    throw new Error('Unsupported taskOutcome.');
  }
  return {
    sampleId,
    sourceRevision,
    ...(taskOutcome ? { taskOutcome } : {}),
    toolCalls,
    toolFailures,
    durationMs: boundedNumber(sample.durationMs, 'durationMs', 86_400_000),
    responseBytes: boundedInteger(sample.responseBytes, 'responseBytes', 1_000_000_000_000)
  };
}

function sampleDigest(sample: ToolEvolutionBenchmarkSample): string {
  return createHash('sha256').update(stableJson(sample)).digest('hex');
}

function addSample(
  aggregate: ToolEvolutionBenchmarkAggregate,
  sample: ToolEvolutionBenchmarkSample
): { aggregate: ToolEvolutionBenchmarkAggregate; added: boolean } {
  const normalized = normalizeSample(sample);
  const digest = sampleDigest(normalized);
  const existing = aggregate.sampleDigests.find(item => item.sampleId === normalized.sampleId);
  if (existing) {
    if (existing.digest !== digest) throw new Error(`Benchmark sample ${normalized.sampleId} was replayed with different metrics.`);
    return { aggregate, added: false };
  }
  if (aggregate.sampleDigests.length >= MAX_SAMPLE_DIGESTS_PER_ARM) {
    throw new Error(`Tool evolution benchmark arm is capped at ${MAX_SAMPLE_DIGESTS_PER_ARM} samples.`);
  }
  const next = structuredClone(aggregate);
  next.samples += 1;
  next.toolCalls += normalized.toolCalls;
  next.toolFailures += normalized.toolFailures;
  next.durationMs += normalized.durationMs;
  next.responseBytes += normalized.responseBytes;
  if (normalized.taskOutcome) {
    next.taskOutcomes += 1;
    if (normalized.taskOutcome === 'verified_success') next.verifiedTaskSuccesses += 1;
    else if (normalized.taskOutcome === 'unverified_success') next.unverifiedTaskSuccesses += 1;
    else if (normalized.taskOutcome === 'failure') next.taskFailures += 1;
    else next.taskRollbacks += 1;
  }
  next.sampleDigests.push({ sampleId: normalized.sampleId, digest });
  next.sampleDigests.sort((left, right) => left.sampleId.localeCompare(right.sampleId));
  return { aggregate: next, added: true };
}

function ratio(numerator: number, denominator: number): number {
  return denominator > 0 ? numerator / denominator : 0;
}

function callsPerVerifiedSuccess(metrics: ToolEvolutionBenchmarkAggregate): number {
  return metrics.verifiedTaskSuccesses > 0 ? metrics.toolCalls / metrics.verifiedTaskSuccesses : Number.POSITIVE_INFINITY;
}

function averageDuration(metrics: ToolEvolutionBenchmarkAggregate): number {
  return metrics.samples > 0 ? metrics.durationMs / metrics.samples : Number.POSITIVE_INFINITY;
}

function responseBytesPerCall(metrics: ToolEvolutionBenchmarkAggregate): number {
  return metrics.toolCalls > 0 ? metrics.responseBytes / metrics.toolCalls : 0;
}

function noRegression(candidate: number, baseline: number): boolean {
  if (!Number.isFinite(baseline)) return Number.isFinite(candidate) || candidate === baseline;
  if (baseline === 0) return candidate === 0;
  return candidate <= baseline * MAX_EFFICIENCY_REGRESSION_RATIO;
}

function materialImprovement(candidate: number, baseline: number): boolean {
  return Number.isFinite(candidate) && Number.isFinite(baseline) && baseline > 0 && candidate <= baseline * REQUIRED_IMPROVEMENT_RATIO;
}

function proposalType(record: KnowledgeRecord): ToolEvolutionProposalType {
  const candidate = String(record.metadata?.proposalType ?? 'performance');
  return ['schema', 'primitive', 'recovery', 'performance', 'result'].includes(candidate)
    ? candidate as ToolEvolutionProposalType
    : 'performance';
}

function toolName(record: KnowledgeRecord): string {
  return typeof record.trigger.tool === 'string' ? record.trigger.tool : '';
}

function proposalAction(record: KnowledgeRecord): Record<string, unknown> {
  const action = structuredClone(record.recommendedAction);
  if (Buffer.byteLength(stableJson(action)) > MAX_PROPOSAL_ACTION_BYTES) {
    throw new Error(`Tool evolution proposal action exceeds ${MAX_PROPOSAL_ACTION_BYTES} bytes.`);
  }
  return action;
}

export function toolEvolutionProposalId(
  plan: Pick<ToolEvolutionChangeProposalRecord, 'knowledgeId' | 'tool' | 'proposalType' | 'baseRevision'>
): string {
  return createHash('sha256').update(stableJson({
    knowledgeId: plan.knowledgeId,
    tool: plan.tool,
    proposalType: plan.proposalType,
    baseRevision: plan.baseRevision
  })).digest('hex');
}

export class ToolEvolutionProposalStore {
  private readonly filePath: string;

  constructor(readonly rootDir: string) {
    this.filePath = proposalFilePath(rootDir);
  }

  async list(): Promise<ToolEvolutionChangeProposalRecord[]> {
    const document = await this.readDocument();
    return document.records.map(record => structuredClone(record));
  }

  async get(proposalId: string): Promise<ToolEvolutionChangeProposalRecord | undefined> {
    return (await this.list()).find(record => record.proposalId === proposalId);
  }

  async revision(): Promise<string> {
    return createHash('sha256').update(stableJson(await this.list())).digest('hex');
  }

  async plan(knowledge: KnowledgeRecord, baseRevision: string, timestampMs: number): Promise<ToolEvolutionChangeProposalRecord> {
    if (knowledge.target !== 'tool_evolution') throw new Error(`Tool evolution knowledge required: ${knowledge.id}`);
    if (!['candidate', 'shadow', 'validated'].includes(knowledge.status)) {
      throw new Error(`Tool evolution knowledge ${knowledge.id} is not eligible for a proposal in status ${knowledge.status}.`);
    }
    const tool = assertBoundedText(toolName(knowledge), 'tool');
    const normalizedBaseRevision = assertBoundedText(baseRevision, 'baseRevision', 128);
    const normalizedProposalType = proposalType(knowledge);
    const action = proposalAction(knowledge);
    const proposalId = toolEvolutionProposalId({
      knowledgeId: knowledge.id,
      tool,
      proposalType: normalizedProposalType,
      baseRevision: normalizedBaseRevision
    });
    const document = await this.readDocument();
    const existing = document.records.find(record => record.proposalId === proposalId);
    if (existing) return structuredClone(existing);
    const now = boundedInteger(timestampMs, 'timestampMs', Number.MAX_SAFE_INTEGER);
    const record: ToolEvolutionChangeProposalRecord = {
      schemaVersion: 1,
      proposalId,
      knowledgeId: knowledge.id,
      tool,
      proposalType: normalizedProposalType,
      baseRevision: normalizedBaseRevision,
      action,
      status: 'proposed',
      createdAtMs: now,
      updatedAtMs: now
    };
    document.records.push(record);
    document.records.sort((left, right) => left.proposalId.localeCompare(right.proposalId));
    await this.writeDocument(document);
    return structuredClone(record);
  }

  async supersede(proposalId: string, timestampMs: number): Promise<ToolEvolutionChangeProposalRecord | undefined> {
    return this.transition(proposalId, ['proposed'], 'superseded', timestampMs);
  }

  async materialize(
    proposalId: string,
    candidateRevision: string,
    experimentId: string,
    timestampMs: number
  ): Promise<ToolEvolutionChangeProposalRecord> {
    const document = await this.readDocument();
    const index = document.records.findIndex(record => record.proposalId === proposalId);
    if (index < 0) throw new Error(`Tool evolution proposal not found: ${proposalId}`);
    const existing = document.records[index]!;
    const candidate = assertBoundedText(candidateRevision, 'candidateRevision', 128);
    const experiment = assertBoundedText(experimentId, 'experimentId', 128);
    if (existing.status === 'materialized') {
      if (existing.candidateRevision !== candidate || existing.experimentId !== experiment) {
        throw new Error(`Tool evolution proposal ${proposalId} was already materialized with a different candidate.`);
      }
      return structuredClone(existing);
    }
    if (existing.status !== 'proposed') {
      throw new Error(`Tool evolution proposal ${proposalId} cannot materialize from status ${existing.status}.`);
    }
    document.records[index] = {
      ...existing,
      status: 'materialized',
      candidateRevision: candidate,
      experimentId: experiment,
      updatedAtMs: Math.max(existing.updatedAtMs, boundedInteger(timestampMs, 'timestampMs', Number.MAX_SAFE_INTEGER))
    };
    await this.writeDocument(document);
    return structuredClone(document.records[index]!);
  }

  private async transition(
    proposalId: string,
    allowedFrom: readonly ToolEvolutionChangeProposalRecord['status'][],
    status: ToolEvolutionChangeProposalRecord['status'],
    timestampMs: number
  ): Promise<ToolEvolutionChangeProposalRecord | undefined> {
    const document = await this.readDocument();
    const index = document.records.findIndex(record => record.proposalId === proposalId);
    if (index < 0) return undefined;
    const existing = document.records[index]!;
    if (!allowedFrom.includes(existing.status)) return structuredClone(existing);
    document.records[index] = {
      ...existing,
      status,
      updatedAtMs: Math.max(existing.updatedAtMs, boundedInteger(timestampMs, 'timestampMs', Number.MAX_SAFE_INTEGER))
    };
    await this.writeDocument(document);
    return structuredClone(document.records[index]!);
  }

  private async readDocument(): Promise<ToolEvolutionProposalDocument> {
    await ensureToolEvolutionLifecycleRecovered(this.rootDir);
    return (await readProposalDocumentRaw(this.rootDir)).document;
  }

  private async writeDocument(document: ToolEvolutionProposalDocument): Promise<void> {
    await writePrivateJsonAtomic(this.rootDir, this.filePath, document);
  }
}

export interface ToolEvolutionRuntimeSource {
  revision: string;
  trusted: boolean;
  proposalIds?: readonly string[];
}

export interface ToolEvolutionCandidateClaimResult {
  enabled: boolean;
  considered: number;
  materialized: number;
  replayed: number;
  ignored: number;
  failed: number;
}

function claimedProposalIds(source: ToolEvolutionRuntimeSource): string[] {
  return [...new Set((source.proposalIds ?? [])
    .map(value => String(value ?? '').trim().toLowerCase())
    .filter(value => /^[0-9a-f]{64}$/.test(value)))]
    .sort()
    .slice(0, 16);
}

export class ToolEvolutionCandidateClaimer {
  constructor(
    private readonly knowledgeStore: KnowledgeStore,
    private readonly proposalStore: ToolEvolutionProposalStore,
    private readonly experimentStore: ToolEvolutionExperimentStore,
    private readonly source: ToolEvolutionRuntimeSource
  ) {}

  async sync(timestampMs: number): Promise<ToolEvolutionCandidateClaimResult> {
    const revision = String(this.source.revision ?? '').trim().toLowerCase();
    const proposalIds = claimedProposalIds(this.source);
    if (!this.source.trusted || !/^[0-9a-f]{40}$/.test(revision)) {
      return { enabled: false, considered: 0, materialized: 0, replayed: 0, ignored: 0, failed: 0 };
    }
    let materialized = 0;
    let replayed = 0;
    let ignored = 0;
    let failed = 0;
    for (const proposalId of proposalIds) {
      try {
        const proposal = await this.proposalStore.get(proposalId);
        if (!proposal) {
          ignored += 1;
          continue;
        }
        if (proposal.status === 'materialized') {
          if (proposal.candidateRevision === revision && proposal.experimentId) replayed += 1;
          else ignored += 1;
          continue;
        }
        if (proposal.status !== 'proposed' || proposal.baseRevision === revision) {
          ignored += 1;
          continue;
        }
        const result = await materializeToolEvolutionProposal(
          this.knowledgeStore,
          this.proposalStore,
          this.experimentStore,
          proposalId,
          proposal.baseRevision,
          revision,
          timestampMs
        );
        if (result.materialized) materialized += 1;
        else ignored += 1;
      } catch {
        failed += 1;
      }
    }
    return {
      enabled: true,
      considered: proposalIds.length,
      materialized,
      replayed,
      ignored,
      failed
    };
  }
}

export interface ToolEvolutionProposalSyncResult {
  enabled: boolean;
  considered: number;
  planned: number;
  superseded: number;
}

export class ToolEvolutionPlanner {
  constructor(
    private readonly knowledgeStore: KnowledgeStore,
    private readonly proposalStore: ToolEvolutionProposalStore,
    private readonly source: ToolEvolutionRuntimeSource,
    private readonly experimentStore?: ToolEvolutionExperimentStore
  ) {}

  async sync(timestampMs: number): Promise<ToolEvolutionProposalSyncResult> {
    const revision = String(this.source.revision ?? '').trim();
    if (!this.source.trusted || !revision) return { enabled: false, considered: 0, planned: 0, superseded: 0 };
    assertBoundedText(revision, 'baseRevision', 128);
    const knowledge = (await this.knowledgeStore.list()).filter(record =>
      record.target === 'tool_evolution' && ['candidate', 'shadow', 'validated'].includes(record.status)
    );
    const eligibleIds = new Set(knowledge.map(record => record.id));
    const existing = await this.proposalStore.list();
    const experiments = this.experimentStore ? await this.experimentStore.list() : [];
    const activeExperimentIds = new Set(experiments
      .filter(experiment => ['planned', 'running', 'validated', 'promoted'].includes(experiment.status))
      .map(experiment => experiment.experimentId));
    const blockedKnowledgeIds = new Set(existing
      .filter(proposal => proposal.status === 'materialized'
        && (!this.experimentStore || Boolean(proposal.experimentId && activeExperimentIds.has(proposal.experimentId))))
      .map(proposal => proposal.knowledgeId));
    let superseded = 0;
    for (const proposal of existing) {
      if (proposal.status !== 'proposed') continue;
      if (proposal.baseRevision === revision && eligibleIds.has(proposal.knowledgeId)) continue;
      const updated = await this.proposalStore.supersede(proposal.proposalId, timestampMs);
      if (updated?.status === 'superseded') superseded += 1;
    }
    const knownIds = new Set(existing.map(record => record.proposalId));
    let planned = 0;
    for (const record of knowledge) {
      if (blockedKnowledgeIds.has(record.id)) continue;
      const proposalId = toolEvolutionProposalId({
        knowledgeId: record.id,
        tool: toolName(record),
        proposalType: proposalType(record),
        baseRevision: revision
      });
      if (knownIds.has(proposalId)) continue;
      await this.proposalStore.plan(record, revision, timestampMs);
      knownIds.add(proposalId);
      planned += 1;
    }
    return { enabled: true, considered: knowledge.length, planned, superseded };
  }
}

export function toolEvolutionExperimentId(
  plan: Pick<ToolEvolutionExperimentPlan, 'knowledgeId' | 'tool' | 'proposalType' | 'baseRevision' | 'candidateRevision'>
): string {
  return createHash('sha256').update(stableJson({
    knowledgeId: plan.knowledgeId,
    tool: plan.tool,
    proposalType: plan.proposalType,
    baseRevision: plan.baseRevision,
    candidateRevision: plan.candidateRevision
  })).digest('hex');
}

function plannedToolEvolutionExperiment(plan: ToolEvolutionExperimentPlan): ToolEvolutionExperimentRecord {
  const normalizedPlan = {
    knowledgeId: assertBoundedText(plan.knowledgeId, 'knowledgeId', 128),
    tool: assertBoundedText(plan.tool, 'tool', 128),
    proposalType: ['schema', 'primitive', 'recovery', 'performance', 'result'].includes(String(plan.proposalType))
      ? plan.proposalType
      : (() => { throw new Error('Unsupported tool evolution proposalType.'); })(),
    baseRevision: assertBoundedText(plan.baseRevision, 'baseRevision', 128),
    candidateRevision: assertBoundedText(plan.candidateRevision, 'candidateRevision', 128),
    createdAtMs: boundedInteger(plan.createdAtMs, 'createdAtMs', Number.MAX_SAFE_INTEGER)
  } satisfies ToolEvolutionExperimentPlan;
  if (normalizedPlan.baseRevision === normalizedPlan.candidateRevision) {
    throw new Error('Tool evolution candidateRevision must differ from baseRevision.');
  }
  return {
    schemaVersion: 1,
    experimentId: toolEvolutionExperimentId(normalizedPlan),
    ...normalizedPlan,
    status: 'planned',
    baseline: emptyAggregate(),
    candidate: emptyAggregate(),
    createdAtMs: normalizedPlan.createdAtMs,
    updatedAtMs: normalizedPlan.createdAtMs
  };
}

export class ToolEvolutionExperimentStore {
  private readonly filePath: string;

  constructor(readonly rootDir: string) {
    this.filePath = experimentFilePath(rootDir);
  }

  async list(): Promise<ToolEvolutionExperimentRecord[]> {
    const document = await this.readDocument();
    return document.records.map(record => structuredClone(record));
  }

  async get(experimentId: string): Promise<ToolEvolutionExperimentRecord | undefined> {
    return (await this.list()).find(record => record.experimentId === experimentId);
  }

  async revision(): Promise<string> {
    return createHash('sha256').update(stableJson(await this.list())).digest('hex');
  }

  async plan(plan: ToolEvolutionExperimentPlan): Promise<ToolEvolutionExperimentRecord> {
    const record = plannedToolEvolutionExperiment(plan);
    const document = await this.readDocument();
    const existing = document.records.find(item => item.experimentId === record.experimentId);
    if (existing) return structuredClone(existing);
    document.records.push(record);
    document.records.sort((left, right) => left.experimentId.localeCompare(right.experimentId));
    await this.writeDocument(document);
    return structuredClone(record);
  }

  async recordBenchmark(
    experimentId: string,
    arm: 'baseline' | 'candidate',
    sample: ToolEvolutionBenchmarkSample,
    timestampMs: number
  ): Promise<ToolEvolutionExperimentRecord> {
    const document = await this.readDocument();
    const index = document.records.findIndex(record => record.experimentId === experimentId);
    if (index < 0) throw new Error(`Tool evolution experiment not found: ${experimentId}`);
    const record = document.records[index]!;
    if (!['planned', 'running'].includes(record.status)) {
      throw new Error(`Tool evolution experiment ${experimentId} is not accepting benchmark samples in status ${record.status}.`);
    }
    const expectedSourceRevision = arm === 'baseline' ? record.baseRevision : record.candidateRevision;
    if (String(sample.sourceRevision ?? '').trim() !== expectedSourceRevision) {
      throw new Error(`Benchmark ${arm} sourceRevision must match the experiment ${arm === 'baseline' ? 'baseRevision' : 'candidateRevision'}.`);
    }
    const updated = addSample(record[arm], sample);
    if (!updated.added) return structuredClone(record);
    document.records[index] = {
      ...record,
      status: 'running',
      [arm]: updated.aggregate,
      updatedAtMs: Math.max(record.updatedAtMs, boundedInteger(timestampMs, 'timestampMs', Number.MAX_SAFE_INTEGER)),
      decisionReason: undefined
    };
    await this.writeDocument(document);
    return structuredClone(document.records[index]!);
  }

  async evaluate(
    experimentId: string,
    currentBaseRevision: string,
    timestampMs: number
  ): Promise<ToolEvolutionExperimentRecord> {
    const document = await this.readDocument();
    const index = document.records.findIndex(record => record.experimentId === experimentId);
    if (index < 0) throw new Error(`Tool evolution experiment not found: ${experimentId}`);
    const record = document.records[index]!;
    if (record.status === 'promoted' || record.status === 'rejected' || record.status === 'stale') return structuredClone(record);
    const now = boundedInteger(timestampMs, 'timestampMs', Number.MAX_SAFE_INTEGER);
    const currentBase = assertBoundedText(currentBaseRevision, 'currentBaseRevision', 128);
    if (currentBase !== record.baseRevision) {
      document.records[index] = { ...record, status: 'stale', decisionReason: 'base_revision_changed', updatedAtMs: Math.max(record.updatedAtMs, now) };
      await this.writeDocument(document);
      return structuredClone(document.records[index]!);
    }
    const baseline = record.baseline;
    const candidate = record.candidate;
    if (baseline.samples < MIN_BENCHMARK_SAMPLES || candidate.samples < MIN_BENCHMARK_SAMPLES
      || baseline.taskOutcomes < MIN_TASK_OUTCOMES || candidate.taskOutcomes < MIN_TASK_OUTCOMES) {
      document.records[index] = { ...record, status: 'running', decisionReason: 'insufficient_benchmark_evidence', updatedAtMs: Math.max(record.updatedAtMs, now) };
      await this.writeDocument(document);
      return structuredClone(document.records[index]!);
    }

    let reason: string | undefined;
    const baselineVerifiedRate = ratio(baseline.verifiedTaskSuccesses, baseline.taskOutcomes);
    const candidateVerifiedRate = ratio(candidate.verifiedTaskSuccesses, candidate.taskOutcomes);
    const baselineTaskFailureRate = ratio(baseline.taskFailures, baseline.taskOutcomes);
    const candidateTaskFailureRate = ratio(candidate.taskFailures, candidate.taskOutcomes);
    const baselineToolFailureRate = ratio(baseline.toolFailures, baseline.toolCalls);
    const candidateToolFailureRate = ratio(candidate.toolFailures, candidate.toolCalls);
    if (candidate.taskRollbacks > 0) reason = 'candidate_task_rollback';
    else if (candidateVerifiedRate < baselineVerifiedRate) reason = 'verified_task_success_regression';
    else if (candidateTaskFailureRate > baselineTaskFailureRate) reason = 'task_failure_rate_regression';
    else if (candidateToolFailureRate > baselineToolFailureRate) reason = 'tool_failure_rate_regression';

    const baselineEfficiency = {
      calls: callsPerVerifiedSuccess(baseline),
      duration: averageDuration(baseline),
      responseBytes: responseBytesPerCall(baseline)
    };
    const candidateEfficiency = {
      calls: callsPerVerifiedSuccess(candidate),
      duration: averageDuration(candidate),
      responseBytes: responseBytesPerCall(candidate)
    };
    if (!reason && (
      !noRegression(candidateEfficiency.calls, baselineEfficiency.calls)
      || !noRegression(candidateEfficiency.duration, baselineEfficiency.duration)
      || !noRegression(candidateEfficiency.responseBytes, baselineEfficiency.responseBytes)
    )) reason = 'efficiency_regression';
    if (!reason && !(
      materialImprovement(candidateEfficiency.calls, baselineEfficiency.calls)
      || materialImprovement(candidateEfficiency.duration, baselineEfficiency.duration)
      || materialImprovement(candidateEfficiency.responseBytes, baselineEfficiency.responseBytes)
    )) reason = 'no_material_efficiency_improvement';

    const status: ToolEvolutionExperimentStatus = reason ? 'rejected' : 'validated';
    document.records[index] = {
      ...record,
      status,
      decisionReason: reason ?? 'quality_preserved_with_material_efficiency_improvement',
      updatedAtMs: Math.max(record.updatedAtMs, now)
    };
    await this.writeDocument(document);
    return structuredClone(document.records[index]!);
  }

  async promote(
    experimentId: string,
    currentBaseRevision: string,
    currentCandidateRevision: string,
    timestampMs: number
  ): Promise<ToolEvolutionExperimentRecord> {
    const current = await this.evaluate(experimentId, currentBaseRevision, timestampMs);
    if (current.status === 'promoted') return current;
    if (current.status !== 'validated') return current;
    if (assertBoundedText(currentCandidateRevision, 'currentCandidateRevision', 128) !== current.candidateRevision) {
      const document = await this.readDocument();
      const index = document.records.findIndex(record => record.experimentId === experimentId);
      if (index < 0) throw new Error(`Tool evolution experiment not found: ${experimentId}`);
      document.records[index] = {
        ...document.records[index]!,
        status: 'stale',
        decisionReason: 'candidate_revision_changed',
        updatedAtMs: Math.max(document.records[index]!.updatedAtMs, boundedInteger(timestampMs, 'timestampMs', Number.MAX_SAFE_INTEGER))
      };
      await this.writeDocument(document);
      return structuredClone(document.records[index]!);
    }
    const document = await this.readDocument();
    const index = document.records.findIndex(record => record.experimentId === experimentId);
    if (index < 0) throw new Error(`Tool evolution experiment not found: ${experimentId}`);
    const record = document.records[index]!;
    document.records[index] = {
      ...record,
      status: 'promoted',
      decisionReason: 'validated_candidate_promoted',
      updatedAtMs: Math.max(record.updatedAtMs, boundedInteger(timestampMs, 'timestampMs', Number.MAX_SAFE_INTEGER))
    };
    await this.writeDocument(document);
    return structuredClone(document.records[index]!);
  }

  private async readDocument(): Promise<ToolEvolutionExperimentDocument> {
    await ensureToolEvolutionLifecycleRecovered(this.rootDir);
    return (await readExperimentDocumentRaw(this.rootDir)).document;
  }

  private async writeDocument(document: ToolEvolutionExperimentDocument): Promise<void> {
    await writePrivateJsonAtomic(this.rootDir, this.filePath, document);
  }
}

export async function createToolEvolutionExperiment(
  knowledgeStore: KnowledgeStore,
  experimentStore: ToolEvolutionExperimentStore,
  knowledgeId: string,
  baseRevision: string,
  candidateRevision: string,
  timestampMs: number
): Promise<ToolEvolutionExperimentRecord> {
  const knowledge = await knowledgeStore.get(knowledgeId);
  if (!knowledge || knowledge.target !== 'tool_evolution') throw new Error(`Tool evolution knowledge not found: ${knowledgeId}`);
  if (!['candidate', 'shadow', 'validated'].includes(knowledge.status)) {
    throw new Error(`Tool evolution knowledge ${knowledgeId} is not eligible for a new experiment in status ${knowledge.status}.`);
  }
  const tool = toolName(knowledge);
  if (!tool) throw new Error(`Tool evolution knowledge ${knowledgeId} does not identify a tool.`);
  return experimentStore.plan({
    knowledgeId,
    tool,
    proposalType: proposalType(knowledge),
    baseRevision,
    candidateRevision,
    createdAtMs: timestampMs
  });
}

async function materializeToolEvolutionLifecycleAtomic(
  rootDir: string,
  proposalId: string,
  plan: ToolEvolutionExperimentPlan,
  timestampMs: number
): Promise<{ proposal: ToolEvolutionChangeProposalRecord; experiment: ToolEvolutionExperimentRecord }> {
  return withToolEvolutionLifecycleMutation(rootDir, async () => {
    const proposalSnapshot = await readProposalDocumentRaw(rootDir);
    const experimentSnapshot = await readExperimentDocumentRaw(rootDir);
    const proposalBefore = structuredClone(proposalSnapshot.document);
    const experimentBefore = structuredClone(experimentSnapshot.document);
    const proposalAfter = structuredClone(proposalBefore);
    const experimentAfter = structuredClone(experimentBefore);
    const proposalIndex = proposalAfter.records.findIndex(record => record.proposalId === proposalId);
    if (proposalIndex < 0) throw new Error(`Tool evolution proposal not found: ${proposalId}`);
    const existingProposal = proposalAfter.records[proposalIndex]!;
    const plannedExperiment = plannedToolEvolutionExperiment(plan);
    if (existingProposal.status === 'materialized') {
      if (existingProposal.candidateRevision !== plannedExperiment.candidateRevision
        || existingProposal.experimentId !== plannedExperiment.experimentId) {
        throw new Error(`Tool evolution proposal ${proposalId} was already materialized with a different candidate.`);
      }
      const existingExperiment = experimentAfter.records.find(record => record.experimentId === plannedExperiment.experimentId);
      if (!existingExperiment) throw new Error(`Materialized tool evolution experiment not found: ${plannedExperiment.experimentId}`);
      return { proposal: structuredClone(existingProposal), experiment: structuredClone(existingExperiment) };
    }
    if (existingProposal.status !== 'proposed') {
      throw new Error(`Tool evolution proposal ${proposalId} cannot materialize from status ${existingProposal.status}.`);
    }
    if (existingProposal.knowledgeId !== plannedExperiment.knowledgeId
      || existingProposal.tool !== plannedExperiment.tool
      || existingProposal.proposalType !== plannedExperiment.proposalType
      || existingProposal.baseRevision !== plannedExperiment.baseRevision) {
      throw new Error(`Tool evolution proposal ${proposalId} does not match the experiment plan.`);
    }
    let experiment = experimentAfter.records.find(record => record.experimentId === plannedExperiment.experimentId);
    if (experiment) {
      if (experiment.knowledgeId !== plannedExperiment.knowledgeId
        || experiment.tool !== plannedExperiment.tool
        || experiment.proposalType !== plannedExperiment.proposalType
        || experiment.baseRevision !== plannedExperiment.baseRevision
        || experiment.candidateRevision !== plannedExperiment.candidateRevision) {
        throw new Error(`Tool evolution experiment ${plannedExperiment.experimentId} conflicts with the deterministic experiment plan.`);
      }
    } else {
      experimentAfter.records.push(plannedExperiment);
      experimentAfter.records.sort((left, right) => left.experimentId.localeCompare(right.experimentId));
      experiment = plannedExperiment;
    }
    const materialized: ToolEvolutionChangeProposalRecord = {
      ...existingProposal,
      status: 'materialized',
      candidateRevision: plannedExperiment.candidateRevision,
      experimentId: plannedExperiment.experimentId,
      updatedAtMs: Math.max(existingProposal.updatedAtMs, boundedInteger(timestampMs, 'timestampMs', Number.MAX_SAFE_INTEGER))
    };
    proposalAfter.records[proposalIndex] = materialized;
    const transaction: ToolEvolutionLifecycleTransactionDocument = {
      schemaVersion: 1,
      state: 'prepared',
      proposalBeforeExists: proposalSnapshot.exists,
      experimentBeforeExists: experimentSnapshot.exists,
      proposalBefore,
      proposalAfter,
      experimentBefore,
      experimentAfter
    };
    const journalPath = lifecycleTransactionFilePath(rootDir);
    await writePrivateJsonAtomic(rootDir, journalPath, transaction);
    try {
      await writePrivateJsonAtomic(rootDir, experimentFilePath(rootDir), experimentAfter);
      await writePrivateJsonAtomic(rootDir, proposalFilePath(rootDir), proposalAfter);
      await writePrivateJsonAtomic(rootDir, journalPath, { ...transaction, state: 'committed' });
    } catch (error) {
      try {
        await recoverToolEvolutionLifecycleTransactionRaw(rootDir);
      } catch (recoveryError) {
        lifecycleRecoveryChecked.delete(path.resolve(rootDir));
        throw new AggregateError(
          [error, recoveryError],
          'Tool evolution lifecycle transaction failed and rollback could not complete.'
        );
      }
      throw error;
    }
    try {
      await rm(journalPath, { force: true });
    } catch {
      lifecycleRecoveryChecked.delete(path.resolve(rootDir));
    }
    return { proposal: structuredClone(materialized), experiment: structuredClone(experiment) };
  });
}

export async function materializeToolEvolutionProposal(
  knowledgeStore: KnowledgeStore,
  proposalStore: ToolEvolutionProposalStore,
  experimentStore: ToolEvolutionExperimentStore,
  proposalId: string,
  currentBaseRevision: string,
  candidateRevision: string,
  timestampMs: number
): Promise<{
  materialized: boolean;
  proposal: ToolEvolutionChangeProposalRecord;
  experiment?: ToolEvolutionExperimentRecord;
  reason?: 'base_revision_changed';
}> {
  const proposal = await proposalStore.get(proposalId);
  if (!proposal) throw new Error(`Tool evolution proposal not found: ${proposalId}`);
  const candidate = assertBoundedText(candidateRevision, 'candidateRevision', 128);
  if (proposal.status === 'materialized') {
    if (proposal.candidateRevision !== candidate || !proposal.experimentId) {
      throw new Error(`Tool evolution proposal ${proposalId} was already materialized with a different candidate.`);
    }
    const experiment = await experimentStore.get(proposal.experimentId);
    if (!experiment) throw new Error(`Materialized tool evolution experiment not found: ${proposal.experimentId}`);
    return { materialized: true, proposal, experiment };
  }
  const currentBase = assertBoundedText(currentBaseRevision, 'currentBaseRevision', 128);
  if (proposal.baseRevision !== currentBase) {
    const stale = await proposalStore.supersede(proposalId, timestampMs) ?? proposal;
    return { materialized: false, proposal: stale, reason: 'base_revision_changed' };
  }
  if (candidate === currentBase) throw new Error('Tool evolution candidateRevision must differ from baseRevision.');
  if (proposal.status !== 'proposed') {
    throw new Error(`Tool evolution proposal ${proposalId} cannot materialize from status ${proposal.status}.`);
  }
  if (path.resolve(proposalStore.rootDir) !== path.resolve(experimentStore.rootDir)) {
    throw new Error('Tool evolution proposal and experiment stores must share the same rootDir for atomic materialization.');
  }
  const knowledge = await knowledgeStore.get(proposal.knowledgeId);
  if (!knowledge || knowledge.target !== 'tool_evolution') {
    throw new Error(`Tool evolution knowledge not found: ${proposal.knowledgeId}`);
  }
  if (!['candidate', 'shadow', 'validated'].includes(knowledge.status)) {
    throw new Error(`Tool evolution knowledge ${proposal.knowledgeId} is not eligible for a new experiment in status ${knowledge.status}.`);
  }
  const tool = toolName(knowledge);
  if (!tool) throw new Error(`Tool evolution knowledge ${proposal.knowledgeId} does not identify a tool.`);
  const lifecycle = await materializeToolEvolutionLifecycleAtomic(
    proposalStore.rootDir,
    proposalId,
    {
      knowledgeId: proposal.knowledgeId,
      tool,
      proposalType: proposalType(knowledge),
      baseRevision: proposal.baseRevision,
      candidateRevision: candidate,
      createdAtMs: timestampMs
    },
    timestampMs
  );
  return { materialized: true, proposal: lifecycle.proposal, experiment: lifecycle.experiment };
}

export async function promoteToolEvolutionExperiment(
  knowledgeStore: KnowledgeStore,
  experimentStore: ToolEvolutionExperimentStore,
  experimentId: string,
  currentBaseRevision: string,
  currentCandidateRevision: string,
  timestampMs: number
): Promise<{ promoted: boolean; experiment: ToolEvolutionExperimentRecord; knowledge?: KnowledgeRecord }> {
  const experiment = await experimentStore.get(experimentId);
  if (!experiment) throw new Error(`Tool evolution experiment not found: ${experimentId}`);
  const knowledge = await knowledgeStore.get(experiment.knowledgeId);
  if (!knowledge || knowledge.target !== 'tool_evolution') {
    throw new Error(`Tool evolution knowledge not found: ${experiment.knowledgeId}`);
  }
  if (experiment.status === 'promoted' && knowledge.status === 'promoted') {
    return { promoted: true, experiment, knowledge };
  }
  if (!['candidate', 'shadow', 'validated'].includes(knowledge.status)) {
    return { promoted: false, experiment, knowledge };
  }
  const originalStatus = knowledge.status;
  const transitioned = await knowledgeStore.transitionStatus(knowledge.id, [originalStatus], 'promoted', timestampMs);
  if (!transitioned) return { promoted: false, experiment, knowledge };
  try {
    const promotedExperiment = await experimentStore.promote(
      experimentId,
      currentBaseRevision,
      currentCandidateRevision,
      timestampMs
    );
    if (promotedExperiment.status !== 'promoted') {
      const restored = await knowledgeStore.transitionStatus(knowledge.id, ['promoted'], originalStatus, timestampMs);
      return { promoted: false, experiment: promotedExperiment, knowledge: restored ?? knowledge };
    }
    return { promoted: true, experiment: promotedExperiment, knowledge: await knowledgeStore.get(knowledge.id) };
  } catch (error) {
    await knowledgeStore.transitionStatus(knowledge.id, ['promoted'], originalStatus, timestampMs).catch(() => undefined);
    throw error;
  }
}
