import { createHash } from 'node:crypto';
import { mkdir, readFile, rename, writeFile } from 'node:fs/promises';
import path from 'node:path';
import type { KnowledgeArtifactTarget, KnowledgeEvidence, KnowledgeRecord } from './types.js';

interface KnowledgeDocument {
  schemaVersion: 1;
  records: KnowledgeRecord[];
}

const MAX_EVIDENCE_CONTEXT_IDS = 256;
const CONTEXT_ID_PATTERN = /^[0-9a-f]{64}$/;

function stableValue(value: unknown): unknown {
  if (Array.isArray(value)) return value.map(stableValue);
  if (!value || typeof value !== 'object') return value;
  return Object.fromEntries(
    Object.entries(value as Record<string, unknown>)
      .sort(([left], [right]) => left.localeCompare(right))
      .map(([key, child]) => [key, stableValue(child)])
  );
}

function stableJson(value: unknown): string {
  return JSON.stringify(stableValue(value));
}

function unique(values: readonly string[]): string[] {
  return [...new Set(values)].sort();
}

function contextIds(values: readonly string[] | undefined, label: string): string[] {
  if (!values?.length) return [];
  if (values.some(value => !CONTEXT_ID_PATTERN.test(value))) {
    throw new Error(`${label} must contain only SHA-256 context identifiers.`);
  }
  return unique(values).slice(0, MAX_EVIDENCE_CONTEXT_IDS);
}

function normalizedEvidence(evidence: KnowledgeEvidence): KnowledgeEvidence {
  const taskContextIds = contextIds(evidence.taskContextIds, 'taskContextIds');
  const conversationContextIds = contextIds(evidence.conversationContextIds, 'conversationContextIds');
  const runtimeBootContextIds = contextIds(evidence.runtimeBootContextIds, 'runtimeBootContextIds');
  return {
    ...structuredClone(evidence),
    ...(taskContextIds.length ? { taskContextIds } : { taskContextIds: undefined }),
    ...(conversationContextIds.length ? { conversationContextIds } : { conversationContextIds: undefined }),
    ...(runtimeBootContextIds.length ? { runtimeBootContextIds } : { runtimeBootContextIds: undefined })
  };
}

export function knowledgeEvidenceCandidateReady(
  target: KnowledgeArtifactTarget,
  evidence: KnowledgeEvidence
): boolean {
  if (evidence.observations < 5) return false;
  if (target !== 'tool_evolution' && target !== 'evolved_skill') return true;
  return (evidence.taskContextIds?.length ?? 0) >= 3
    || (evidence.conversationContextIds?.length ?? 0) >= 2
    || (evidence.runtimeBootContextIds?.length ?? 0) >= 2;
}

function mergeEvidence(left: KnowledgeEvidence, right: KnowledgeEvidence): KnowledgeEvidence {
  const normalizedLeft = normalizedEvidence(left);
  const normalizedRight = normalizedEvidence(right);
  const taskContextIds = contextIds([...(normalizedLeft.taskContextIds ?? []), ...(normalizedRight.taskContextIds ?? [])], 'taskContextIds');
  const conversationContextIds = contextIds([...(normalizedLeft.conversationContextIds ?? []), ...(normalizedRight.conversationContextIds ?? [])], 'conversationContextIds');
  const runtimeBootContextIds = contextIds([...(normalizedLeft.runtimeBootContextIds ?? []), ...(normalizedRight.runtimeBootContextIds ?? [])], 'runtimeBootContextIds');
  return {
    observations: left.observations + right.observations,
    successes: left.successes + right.successes,
    failures: left.failures + right.failures,
    verifiedSuccesses: (left.verifiedSuccesses ?? 0) + (right.verifiedSuccesses ?? 0),
    verificationFailures: (left.verificationFailures ?? 0) + (right.verificationFailures ?? 0),
    recoverySuccesses: (left.recoverySuccesses ?? 0) + (right.recoverySuccesses ?? 0),
    recoveryFailures: (left.recoveryFailures ?? 0) + (right.recoveryFailures ?? 0),
    totalDurationMs: left.totalDurationMs + right.totalDurationMs,
    totalRequestBytes: left.totalRequestBytes + right.totalRequestBytes,
    totalResponseBytes: left.totalResponseBytes + right.totalResponseBytes,
    ...(taskContextIds.length ? { taskContextIds } : {}),
    ...(conversationContextIds.length ? { conversationContextIds } : {}),
    ...(runtimeBootContextIds.length ? { runtimeBootContextIds } : {}),
    firstSeenAtMs: Math.min(left.firstSeenAtMs, right.firstSeenAtMs),
    lastSeenAtMs: Math.max(left.lastSeenAtMs, right.lastSeenAtMs)
  };
}

function automaticStatus(status: KnowledgeRecord['status']): boolean {
  return status === 'observed' || status === 'candidate';
}

function evidenceTrackingStatus(status: KnowledgeRecord['status']): boolean {
  return automaticStatus(status) || status === 'shadow';
}

function supportConfidence(observations: number): number {
  if (observations <= 0) return 0;
  return Math.round((observations / (observations + 5)) * 10_000) / 10_000;
}

export function knowledgeRecordId(record: Pick<KnowledgeRecord, 'scope' | 'target' | 'trigger' | 'hypothesis' | 'recommendedAction'>): string {
  return createHash('sha256').update(stableJson({
    scope: record.scope,
    target: record.target,
    trigger: record.trigger,
    hypothesis: record.hypothesis,
    recommendedAction: record.recommendedAction
  })).digest('hex');
}

export class KnowledgeStore {
  private readonly filePath: string;

  constructor(readonly rootDir: string) {
    this.filePath = path.join(rootDir, 'knowledge.json');
  }

  async list(): Promise<KnowledgeRecord[]> {
    const document = await this.readDocument();
    return document.records.map(record => structuredClone(record));
  }

  async get(id: string): Promise<KnowledgeRecord | undefined> {
    const records = await this.list();
    return records.find(record => record.id === id);
  }

  async upsert(record: KnowledgeRecord): Promise<KnowledgeRecord> {
    const document = await this.readDocument();
    const canonicalId = knowledgeRecordId(record);
    if (record.id && record.id !== canonicalId) throw new Error(`Knowledge record id mismatch: expected ${canonicalId}`);
    const normalized = { ...structuredClone(record), id: canonicalId, evidence: normalizedEvidence(record.evidence) };
    const index = document.records.findIndex(existing => existing.id === canonicalId);
    if (index < 0) {
      document.records.push({
        ...normalized,
        status: normalized.target === 'tool_evolution' && automaticStatus(normalized.status)
          ? knowledgeEvidenceCandidateReady(normalized.target, normalized.evidence) ? 'candidate' : 'observed'
          : normalized.status
      });
    } else {
      const existing = document.records[index]!;
      const existingEvents = new Set(existing.sourceEventIds);
      const incomingEvents = new Set(normalized.sourceEventIds);
      const overlappingEvents = [...incomingEvents].filter(eventId => existingEvents.has(eventId));
      const newEvents = [...incomingEvents].filter(eventId => !existingEvents.has(eventId));
      const incomingIsReplay = incomingEvents.size > 0 && newEvents.length === 0;
      if (overlappingEvents.length > 0 && newEvents.length > 0) {
        throw new Error('Knowledge evidence batches must be disjoint or exact replays; partial overlap would double-count evidence.');
      }
      const evidence = incomingIsReplay ? normalizedEvidence(existing.evidence) : mergeEvidence(existing.evidence, normalized.evidence);
      const incomingAutomatic = automaticStatus(normalized.status);
      const existingAutomatic = automaticStatus(existing.status);
      const status = !incomingAutomatic
        ? normalized.status
        : existing.status === 'shadow'
          ? 'shadow'
          : !existingAutomatic
            ? existing.status
            : knowledgeEvidenceCandidateReady(normalized.target, evidence) ? 'candidate' : 'observed';
      const confidence = incomingAutomatic && evidenceTrackingStatus(existing.status)
        ? supportConfidence(evidence.observations)
        : !incomingAutomatic ? normalized.confidence : existing.confidence;
      document.records[index] = {
        ...existing,
        ...normalized,
        status,
        confidence,
        createdAtMs: Math.min(existing.createdAtMs, normalized.createdAtMs),
        updatedAtMs: Math.max(existing.updatedAtMs, normalized.updatedAtMs),
        evidence,
        sourceEventIds: unique([...existing.sourceEventIds, ...normalized.sourceEventIds]),
        counterexampleEventIds: unique([...existing.counterexampleEventIds, ...normalized.counterexampleEventIds]),
        supersedes: unique([...(existing.supersedes ?? []), ...(normalized.supersedes ?? [])])
      };
    }
    document.records.sort((left, right) => left.id.localeCompare(right.id));
    await this.writeDocument(document);
    return structuredClone(document.records.find(item => item.id === canonicalId)!);
  }

  async transitionStatus(
    id: string,
    allowedFrom: readonly KnowledgeRecord['status'][],
    next: KnowledgeRecord['status'],
    updatedAtMs: number
  ): Promise<KnowledgeRecord | undefined> {
    const document = await this.readDocument();
    const index = document.records.findIndex(record => record.id === id);
    if (index < 0) return undefined;
    const existing = document.records[index]!;
    if (!allowedFrom.includes(existing.status)) return undefined;
    document.records[index] = {
      ...existing,
      status: next,
      updatedAtMs: Math.max(existing.updatedAtMs, updatedAtMs)
    };
    await this.writeDocument(document);
    return structuredClone(document.records[index]!);
  }

  async revision(): Promise<string> {
    const records = await this.list();
    return createHash('sha256').update(stableJson(records)).digest('hex');
  }

  private async readDocument(): Promise<KnowledgeDocument> {
    try {
      const parsed = JSON.parse(await readFile(this.filePath, 'utf8')) as KnowledgeDocument;
      if (parsed?.schemaVersion !== 1 || !Array.isArray(parsed.records)) throw new Error('Unsupported knowledge store schema.');
      return parsed;
    } catch (error) {
      if ((error as NodeJS.ErrnoException).code === 'ENOENT') return { schemaVersion: 1, records: [] };
      throw error;
    }
  }

  private async writeDocument(document: KnowledgeDocument): Promise<void> {
    await mkdir(this.rootDir, { recursive: true });
    const tempPath = `${this.filePath}.tmp`;
    await writeFile(tempPath, `${JSON.stringify(document, null, 2)}\n`, { encoding: 'utf8', mode: 0o600 });
    await rename(tempPath, this.filePath);
  }
}
