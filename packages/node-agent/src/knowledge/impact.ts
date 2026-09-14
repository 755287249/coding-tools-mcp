import { createHash } from 'node:crypto';
import { mkdir, readFile, rename, writeFile } from 'node:fs/promises';
import path from 'node:path';
import type { KnowledgeArtifactTarget, TaskOutcome } from './types.js';

export interface KnowledgeImpactRecord {
  schemaVersion: 1;
  knowledgeId: string;
  target: KnowledgeArtifactTarget;
  tool: string;
  sourceEventIds: string[];
  matchedTaskIds: string[];
  creditedTaskIds: string[];
  considered: number;
  matched: number;
  actualSuccesses: number;
  actualFailures: number;
  verifiedSuccesses: number;
  verificationFailures: number;
  recoverySuccesses: number;
  recoveryFailures: number;
  taskVerifiedSuccesses: number;
  taskUnverifiedSuccesses: number;
  taskFailures: number;
  taskRollbacks: number;
  totalObservedDurationMs: number;
  totalObservedRequestBytes: number;
  totalObservedResponseBytes: number;
  firstSeenAtMs: number;
  lastSeenAtMs: number;
}

interface KnowledgeImpactDocument {
  schemaVersion: 1;
  records: KnowledgeImpactRecord[];
}

function stableValue(value: unknown): unknown {
  if (Array.isArray(value)) return value.map(stableValue);
  if (!value || typeof value !== 'object') return value;
  return Object.fromEntries(Object.entries(value as Record<string, unknown>)
    .sort(([left], [right]) => left.localeCompare(right))
    .map(([key, child]) => [key, stableValue(child)]));
}

function unique(values: readonly string[]): string[] {
  return [...new Set(values)].sort();
}

function normalize(record: KnowledgeImpactRecord): KnowledgeImpactRecord {
  return {
    ...record,
    sourceEventIds: unique(record.sourceEventIds ?? []),
    matchedTaskIds: unique(record.matchedTaskIds ?? []),
    creditedTaskIds: unique(record.creditedTaskIds ?? []),
    taskVerifiedSuccesses: Number(record.taskVerifiedSuccesses ?? 0),
    taskUnverifiedSuccesses: Number(record.taskUnverifiedSuccesses ?? 0),
    taskFailures: Number(record.taskFailures ?? 0),
    taskRollbacks: Number(record.taskRollbacks ?? 0)
  };
}

function merge(left: KnowledgeImpactRecord, right: KnowledgeImpactRecord): KnowledgeImpactRecord {
  return {
    ...left,
    ...right,
    sourceEventIds: unique([...left.sourceEventIds, ...right.sourceEventIds]),
    matchedTaskIds: unique([...left.matchedTaskIds, ...right.matchedTaskIds]),
    creditedTaskIds: unique([...left.creditedTaskIds, ...right.creditedTaskIds]),
    considered: left.considered + right.considered,
    matched: left.matched + right.matched,
    actualSuccesses: left.actualSuccesses + right.actualSuccesses,
    actualFailures: left.actualFailures + right.actualFailures,
    verifiedSuccesses: left.verifiedSuccesses + right.verifiedSuccesses,
    verificationFailures: left.verificationFailures + right.verificationFailures,
    recoverySuccesses: left.recoverySuccesses + right.recoverySuccesses,
    recoveryFailures: left.recoveryFailures + right.recoveryFailures,
    taskVerifiedSuccesses: left.taskVerifiedSuccesses + right.taskVerifiedSuccesses,
    taskUnverifiedSuccesses: left.taskUnverifiedSuccesses + right.taskUnverifiedSuccesses,
    taskFailures: left.taskFailures + right.taskFailures,
    taskRollbacks: left.taskRollbacks + right.taskRollbacks,
    totalObservedDurationMs: left.totalObservedDurationMs + right.totalObservedDurationMs,
    totalObservedRequestBytes: left.totalObservedRequestBytes + right.totalObservedRequestBytes,
    totalObservedResponseBytes: left.totalObservedResponseBytes + right.totalObservedResponseBytes,
    firstSeenAtMs: Math.min(left.firstSeenAtMs, right.firstSeenAtMs),
    lastSeenAtMs: Math.max(left.lastSeenAtMs, right.lastSeenAtMs)
  };
}

export class KnowledgeImpactStore {
  private readonly filePath: string;

  constructor(readonly rootDir: string) {
    this.filePath = path.join(rootDir, 'impact.json');
  }

  async list(): Promise<KnowledgeImpactRecord[]> {
    const document = await this.readDocument();
    return document.records.map(record => structuredClone(normalize(record)));
  }

  async record(delta: KnowledgeImpactRecord): Promise<KnowledgeImpactRecord> {
    const document = await this.readDocument();
    const index = document.records.findIndex(record => record.knowledgeId === delta.knowledgeId);
    const normalizedDelta = normalize(delta);
    if (index < 0) document.records.push(structuredClone(normalizedDelta));
    else {
      const existing = normalize(document.records[index]!);
      const existingEvents = new Set(existing.sourceEventIds);
      const incomingEvents = new Set(normalizedDelta.sourceEventIds);
      const overlapping = [...incomingEvents].filter(eventId => existingEvents.has(eventId));
      const novel = [...incomingEvents].filter(eventId => !existingEvents.has(eventId));
      if (incomingEvents.size > 0 && novel.length === 0) return structuredClone(existing);
      if (overlapping.length > 0) {
        throw new Error('Shadow impact batches must be disjoint or exact replays; partial overlap would double-count impact.');
      }
      document.records[index] = merge(existing, normalizedDelta);
    }
    document.records.sort((left, right) => left.knowledgeId.localeCompare(right.knowledgeId));
    await this.writeDocument(document);
    return structuredClone(document.records.find(record => record.knowledgeId === delta.knowledgeId)!);
  }

  async creditTaskOutcome(taskId: string, outcome: TaskOutcome, timestampMs: number): Promise<number> {
    const document = await this.readDocument();
    let credited = 0;
    for (let index = 0; index < document.records.length; index += 1) {
      const record = normalize(document.records[index]!);
      if (!record.matchedTaskIds.includes(taskId) || record.creditedTaskIds.includes(taskId)) continue;
      record.creditedTaskIds = unique([...record.creditedTaskIds, taskId]);
      if (outcome === 'verified_success') record.taskVerifiedSuccesses += 1;
      else if (outcome === 'unverified_success') record.taskUnverifiedSuccesses += 1;
      else if (outcome === 'failure') record.taskFailures += 1;
      else record.taskRollbacks += 1;
      record.lastSeenAtMs = Math.max(record.lastSeenAtMs, timestampMs);
      document.records[index] = record;
      credited += 1;
    }
    if (credited > 0) await this.writeDocument(document);
    return credited;
  }

  async revision(): Promise<string> {
    return createHash('sha256').update(JSON.stringify(stableValue(await this.list()))).digest('hex');
  }

  private async readDocument(): Promise<KnowledgeImpactDocument> {
    try {
      const parsed = JSON.parse(await readFile(this.filePath, 'utf8')) as KnowledgeImpactDocument;
      if (parsed?.schemaVersion !== 1 || !Array.isArray(parsed.records)) throw new Error('Unsupported knowledge impact schema.');
      return parsed;
    } catch (error) {
      if ((error as NodeJS.ErrnoException).code === 'ENOENT') return { schemaVersion: 1, records: [] };
      throw error;
    }
  }

  private async writeDocument(document: KnowledgeImpactDocument): Promise<void> {
    await mkdir(this.rootDir, { recursive: true });
    const tempPath = `${this.filePath}.tmp`;
    await writeFile(tempPath, `${JSON.stringify(document, null, 2)}\n`, { encoding: 'utf8', mode: 0o600 });
    await rename(tempPath, this.filePath);
  }
}
