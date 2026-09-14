import { createHash } from 'node:crypto';
import type { JsonObject } from '../types.js';
import type { TaskOutcome, ToolEvolutionExperimentRecord } from './types.js';
import { ToolEvolutionExperimentStore } from './toolEvolution.js';

const DEFAULT_QUEUE_CAPACITY = 1_024;
const DEFAULT_PENDING_TASK_CAPACITY = 256;
const MAX_EVENT_IDS_PER_TASK = 4_096;
const PENDING_TASK_TTL_MS = 6 * 60 * 60_000;
const MAX_DRAIN_BATCH = 128;

export interface ToolEvolutionBenchmarkSource {
  revision?: string;
  trusted: boolean;
}

export interface ToolEvolutionBenchmarkAttribution {
  taskId?: string;
  taskOutcome?: TaskOutcome;
}

export interface ToolEvolutionBenchmarkCollectorSnapshot {
  enabled: boolean;
  sourceRevision?: string;
  queued: number;
  pendingTasks: number;
  processed: number;
  ignored: number;
  duplicateEvents: number;
  dropped: number;
  finalizedTasks: number;
  recordedSamples: number;
  failedSamples: number;
  draining: boolean;
  lastError?: string;
}

interface BenchmarkEvent {
  eventId: string;
  taskId: string;
  tool: string;
  failed: boolean;
  durationMs: number;
  responseBytes: number;
  timestampMs: number;
  taskOutcome?: TaskOutcome;
}

interface PendingTaskAggregate {
  taskId: string;
  firstSeenAtMs: number;
  lastSeenAtMs: number;
  eventIds: Set<string>;
  toolsSeen: Set<string>;
  toolCalls: number;
  toolFailures: number;
  durationMs: number;
  responseBytes: number;
}

function boundedNumber(value: unknown, maximum: number): number {
  const parsed = Number(value);
  if (!Number.isFinite(parsed) || parsed < 0) return 0;
  return Math.min(parsed, maximum);
}

function normalizedToken(value: unknown, maximum = 128): string | undefined {
  if (typeof value !== 'string') return undefined;
  const normalized = value.trim();
  if (!normalized || normalized.length > maximum || /[\r\n\0]/.test(normalized)) return undefined;
  return normalized;
}

function benchmarkEvent(
  record: JsonObject,
  attribution: ToolEvolutionBenchmarkAttribution
): BenchmarkEvent | undefined {
  const eventId = normalizedToken(record.diagnostic_event_id, 128);
  const taskId = normalizedToken(attribution.taskId, 128);
  const tool = normalizedToken(record.tool, 128);
  if (!eventId || !taskId || !tool) return undefined;
  return {
    eventId,
    taskId,
    tool,
    failed: record.outcome !== 'success' || record.verification_ok === false,
    durationMs: boundedNumber(record.duration_ms, 86_400_000),
    responseBytes: boundedNumber(record.response_json_bytes, 1_000_000_000_000),
    timestampMs: boundedNumber(record.completed_ts_ms ?? record.timestamp_ms ?? Date.now(), Number.MAX_SAFE_INTEGER),
    ...(attribution.taskOutcome ? { taskOutcome: attribution.taskOutcome } : {})
  };
}

function activeExperimentsForSource(
  records: readonly ToolEvolutionExperimentRecord[],
  sourceRevision: string
): Array<{ experiment: ToolEvolutionExperimentRecord; arm: 'baseline' | 'candidate' }> {
  const active: Array<{ experiment: ToolEvolutionExperimentRecord; arm: 'baseline' | 'candidate' }> = [];
  for (const experiment of records) {
    if (!['planned', 'running'].includes(experiment.status)) continue;
    if (experiment.baseRevision === sourceRevision) active.push({ experiment, arm: 'baseline' });
    else if (experiment.candidateRevision === sourceRevision) active.push({ experiment, arm: 'candidate' });
  }
  return active;
}

function taskSampleId(experimentId: string, sourceRevision: string, taskId: string): string {
  const digest = createHash('sha256').update(`${experimentId}\0${sourceRevision}\0${taskId}`).digest('hex');
  return `task-${digest}`;
}

export class ToolEvolutionBenchmarkCollector {
  private readonly sourceRevision: string | undefined;
  private readonly queue: BenchmarkEvent[] = [];
  private readonly pending = new Map<string, PendingTaskAggregate>();
  private drainPromise: Promise<void> | undefined;
  private processed = 0;
  private ignored = 0;
  private duplicateEvents = 0;
  private dropped = 0;
  private finalizedTasks = 0;
  private recordedSamples = 0;
  private failedSamples = 0;
  private lastError: string | undefined;

  constructor(
    private readonly experimentStore: ToolEvolutionExperimentStore,
    source: ToolEvolutionBenchmarkSource,
    private readonly queueCapacity = DEFAULT_QUEUE_CAPACITY,
    private readonly pendingTaskCapacity = DEFAULT_PENDING_TASK_CAPACITY
  ) {
    this.sourceRevision = source.trusted ? normalizedToken(source.revision, 128) : undefined;
  }

  enqueueToolUsage(record: JsonObject, attribution: ToolEvolutionBenchmarkAttribution = {}): void {
    if (!this.sourceRevision) {
      this.ignored += 1;
      return;
    }
    const event = benchmarkEvent(record, attribution);
    if (!event) {
      this.ignored += 1;
      return;
    }
    if (this.queue.length >= this.queueCapacity) {
      this.dropped += 1;
      return;
    }
    this.queue.push(event);
    this.startDrain();
  }

  snapshot(): ToolEvolutionBenchmarkCollectorSnapshot {
    return {
      enabled: Boolean(this.sourceRevision),
      ...(this.sourceRevision ? { sourceRevision: this.sourceRevision } : {}),
      queued: this.queue.length,
      pendingTasks: this.pending.size,
      processed: this.processed,
      ignored: this.ignored,
      duplicateEvents: this.duplicateEvents,
      dropped: this.dropped,
      finalizedTasks: this.finalizedTasks,
      recordedSamples: this.recordedSamples,
      failedSamples: this.failedSamples,
      draining: Boolean(this.drainPromise),
      ...(this.lastError ? { lastError: this.lastError } : {})
    };
  }

  async flush(): Promise<void> {
    while (this.drainPromise || this.queue.length) {
      this.startDrain();
      if (this.drainPromise) await this.drainPromise;
    }
  }

  private startDrain(): void {
    if (this.drainPromise || !this.queue.length) return;
    this.drainPromise = this.drain().finally(() => {
      this.drainPromise = undefined;
      if (this.queue.length) this.startDrain();
    });
  }

  private prunePending(now: number): void {
    for (const [taskId, task] of this.pending) {
      if (now - task.lastSeenAtMs > PENDING_TASK_TTL_MS) {
        this.pending.delete(taskId);
        this.dropped += 1;
      }
    }
    while (this.pending.size > this.pendingTaskCapacity) {
      const oldest = [...this.pending.values()]
        .sort((left, right) => left.lastSeenAtMs - right.lastSeenAtMs || left.taskId.localeCompare(right.taskId))[0];
      if (!oldest) break;
      this.pending.delete(oldest.taskId);
      this.dropped += 1;
    }
  }

  private async finalizeTask(
    task: PendingTaskAggregate,
    outcome: TaskOutcome,
    active: readonly { experiment: ToolEvolutionExperimentRecord; arm: 'baseline' | 'candidate' }[]
  ): Promise<void> {
    if (!this.sourceRevision) return;
    this.finalizedTasks += 1;
    for (const { experiment, arm } of active) {
      if (task.firstSeenAtMs < experiment.createdAtMs || !task.toolsSeen.has(experiment.tool)) continue;
      try {
        await this.experimentStore.recordBenchmark(experiment.experimentId, arm, {
          sampleId: taskSampleId(experiment.experimentId, this.sourceRevision, task.taskId),
          sourceRevision: this.sourceRevision,
          taskOutcome: outcome,
          toolCalls: task.toolCalls,
          toolFailures: task.toolFailures,
          durationMs: task.durationMs,
          responseBytes: task.responseBytes
        }, task.lastSeenAtMs);
        this.recordedSamples += 1;
      } catch (error) {
        this.failedSamples += 1;
        this.lastError = error instanceof Error ? error.message : String(error);
      }
    }
  }

  private async drain(): Promise<void> {
    while (this.queue.length) {
      const batch = this.queue.splice(0, MAX_DRAIN_BATCH);
      this.processed += batch.length;
      this.lastError = undefined;
      try {
        const active = activeExperimentsForSource(await this.experimentStore.list(), this.sourceRevision!);
        for (const event of batch) {
          let task = this.pending.get(event.taskId);
          if (!task) {
            task = {
              taskId: event.taskId,
              firstSeenAtMs: event.timestampMs,
              lastSeenAtMs: event.timestampMs,
              eventIds: new Set(),
              toolsSeen: new Set(),
              toolCalls: 0,
              toolFailures: 0,
              durationMs: 0,
              responseBytes: 0
            };
            this.pending.set(event.taskId, task);
          }
          if (task.eventIds.has(event.eventId)) {
            this.duplicateEvents += 1;
            continue;
          }
          if (task.eventIds.size >= MAX_EVENT_IDS_PER_TASK) {
            this.dropped += 1;
            continue;
          }
          task.eventIds.add(event.eventId);
          task.toolsSeen.add(event.tool);
          task.firstSeenAtMs = Math.min(task.firstSeenAtMs, event.timestampMs);
          task.lastSeenAtMs = Math.max(task.lastSeenAtMs, event.timestampMs);
          task.toolCalls += 1;
          if (event.failed) task.toolFailures += 1;
          task.durationMs += event.durationMs;
          task.responseBytes += event.responseBytes;
          if (event.taskOutcome) {
            await this.finalizeTask(task, event.taskOutcome, active);
            this.pending.delete(event.taskId);
          }
          this.prunePending(event.timestampMs);
        }
      } catch (error) {
        this.lastError = error instanceof Error ? error.message : String(error);
      }
    }
  }
}
