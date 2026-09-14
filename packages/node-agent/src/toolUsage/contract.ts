export type ToolUsageJsonObject = Record<string, unknown>;

export interface ToolRequestTiming {
  previousResponseCompletedTsMs: number | null;
  orchestrationGapMs: number | null;
  activityBurstId: number;
  activityBurstSequence: number;
  concurrentRequest: boolean;
}

export interface ToolUsageInput {
  tool: string;
  arguments: ToolUsageJsonObject;
  result: ToolUsageJsonObject;
  startedTsMs: number;
  durationMs: number;
  requestTiming: ToolRequestTiming;
  requestJsonBytes?: number;
  requestId?: unknown;
  workspaceId?: string;
  transportMode?: string;
  protocolVersion?: string;
  method?: string;
  rpcFastPath?: boolean;
}

export type DiagnosticEventType = 'tool_call' | 'process_session' | 'service_event' | 'transport_event' | 'lifecycle_event' | 'recovery_event';

export interface DiagnosticEventInput {
  eventType: DiagnosticEventType;
  event: string;
  severity?: 'info' | 'warning' | 'error';
  failureDomain?: 'none' | 'tool' | 'transport' | 'process';
  timestampMs?: number;
  fields?: ToolUsageJsonObject;
}

export interface AsyncSessionUsageInput {
  sessionId: string;
  commandKind: string;
  startedTsMs: number;
  completedTsMs?: number;
  childProcessTotalMs: number;
  firstOutputMs?: number | null;
  exitCode?: number | null;
  terminationReason: string;
  stdoutBytes: number;
  stderrBytes: number;
}

export interface ToolUsageStoreContract {
  readonly runtimeBootId: string;
  readonly serverVersion: string;
  redactTelemetry: boolean;

  setRedactTelemetry(value: boolean): void;
  beginRequest(startedTsMs?: number): ToolRequestTiming;
  recordToolCall(input: ToolUsageInput): ToolUsageJsonObject;
  recordAsyncSession(input: AsyncSessionUsageInput): ToolUsageJsonObject;
  recordDiagnosticEvent(input: DiagnosticEventInput): ToolUsageJsonObject;
  enqueue(record: ToolUsageJsonObject): void;
  flush(): Promise<void>;
  dashboardSummary(): Promise<ToolUsageJsonObject>;
  query(args: ToolUsageJsonObject): Promise<ToolUsageJsonObject>;
}
