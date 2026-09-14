import type { JsonObject, ToolContext } from '../types.js';
import { DEFAULT_COMMAND_TIMEOUT_MAX_MS } from '../executionLimits.js';
import { classifyCommandKind } from '../toolUsage.js';
import { ProcessToolError } from './errors.js';

export const DEFAULT_JOB_TIMEOUT_MAX_MS = 6 * 60 * 60_000;
export const ABSOLUTE_JOB_TIMEOUT_MAX_MS = 24 * 60 * 60_000;

export interface ProcessTimeoutContract {
  executionMode: 'command' | 'job';
  requestedTimeoutMs: number | null;
  effectiveTimeoutMs: number;
  limitMs: number;
}

/** Host configuration, never child-process env. Invalid explicit values disable jobs. */
export function configuredJobTimeoutMaxMs(environment: NodeJS.ProcessEnv = process.env): number {
  const raw = environment.CTMCP_JOB_TIMEOUT_MAX_MS;
  if (raw === undefined) return DEFAULT_JOB_TIMEOUT_MAX_MS;
  if (!/^[0-9]+$/.test(raw)) return 0;
  const value = Number(raw);
  return Number.isSafeInteger(value) ? Math.min(value, ABSOLUTE_JOB_TIMEOUT_MAX_MS) : 0;
}

/** Resolve a fixed process budget independently of response wait windows. */
export function resolveProcessTimeout(
  args: JsonObject,
  commandDisplay: string,
  commandMaxMs = DEFAULT_COMMAND_TIMEOUT_MAX_MS,
  jobMaxMs = configuredJobTimeoutMaxMs()
): ProcessTimeoutContract {
  const job = Object.hasOwn(args, 'job_timeout_ms');
  const ordinary = Object.hasOwn(args, 'timeout_ms');
  const invalid = (message: string): never => {
    throw new ProcessToolError('INVALID_ARGUMENT', message, 'validation', false, { process_started: false, timeout_scope: 'process' });
  };
  if (job && ordinary) invalid('job_timeout_ms and timeout_ms are mutually exclusive; wait_command.timeout_ms only controls polling.');
  if (job && (typeof args.operation_id !== 'string' || !args.operation_id.trim() || args.operation_id.length > 128)) {
    invalid('job_timeout_ms requires a stable nonempty operation_id of at most 128 characters.');
  }
  const field = job ? 'job_timeout_ms' : 'timeout_ms';
  const value = args[field];
  if ((job || ordinary) && (typeof value !== 'number' || !Number.isSafeInteger(value) || value < 1)) {
    invalid(`${field} must be a positive safe integer in milliseconds.`);
  }
  const limitMs = job
    ? Math.max(0, Math.min(Number.isFinite(jobMaxMs) ? Math.trunc(jobMaxMs) : 0, ABSOLUTE_JOB_TIMEOUT_MAX_MS))
    : Math.max(1, Math.trunc(commandMaxMs));
  if (job && limitMs === 0) {
    throw new ProcessToolError('LONG_RUNNING_JOBS_DISABLED', 'Long-running jobs are disabled by the host CTMCP_JOB_TIMEOUT_MAX_MS setting.', 'policy', false, { process_started: false, timeout_scope: 'process' });
  }
  const requestedTimeoutMs = job || ordinary ? value as number : null;
  if (requestedTimeoutMs !== null && requestedTimeoutMs > limitMs) {
    throw new ProcessToolError('COMMAND_TIMEOUT_EXCEEDS_LIMIT', `${field} exceeds configured limit (${limitMs} ms); no process was started.`, 'policy', false, {
      process_started: false, timeout_scope: 'process', execution_mode: job ? 'job' : 'command',
      requested_process_timeout_ms: requestedTimeoutMs, process_timeout_limit_ms: limitMs,
      polling_extends_process_deadline: false,
      suggestion: job ? 'Choose a budget within the host job limit or ask the host administrator to change it.' : 'For an authorized long job, use job_timeout_ms with a stable operation_id instead of timeout_ms. Polling cannot extend a process deadline.'
    });
  }
  return {
    executionMode: job ? 'job' : 'command', requestedTimeoutMs,
    effectiveTimeoutMs: requestedTimeoutMs ?? resolvedCommandTimeoutMs(args, commandDisplay, limitMs), limitMs
  };
}

const DEFAULT_PROCESS_TIMEOUT_MS = 30_000;

export function boundedInteger(value: unknown, fallback: number, minimum: number, maximum: number): number {
  const parsed = Number(value);
  return Number.isFinite(parsed) ? Math.max(minimum, Math.min(maximum, Math.trunc(parsed))) : fallback;
}

export function commandTimeoutMaxMs(ctx: ToolContext): number {
  return ctx.config.limits.commandTimeoutMaxMs ?? DEFAULT_COMMAND_TIMEOUT_MAX_MS;
}

function packageScriptLooksLongRunning(commandDisplay: string): boolean {
  const value = commandDisplay.toLowerCase();
  const longScript = /(?:build|portable|package|release|verify|test|check|sync|parity)/;
  const packageManager = /(?:^|[\\/\s])(?:npm(?:\.cmd)?|pnpm(?:\.cmd)?|yarn(?:\.cmd)?|bun(?:\.exe)?)(?:\s|$)/.test(value);
  if (!packageManager) return false;
  if (value.includes(' run ')) return value.split(/\s+/).some(token => longScript.test(token));
  return /(?:^|[\\/\s])(?:pnpm(?:\.cmd)?|yarn(?:\.cmd)?|bun(?:\.exe)?)\s+([^\s]+)/.test(value)
    && longScript.test(value.match(/(?:^|[\\/\s])(?:pnpm(?:\.cmd)?|yarn(?:\.cmd)?|bun(?:\.exe)?)\s+([^\s]+)/)?.[1] ?? '');
}

export function resolvedCommandTimeoutMs(
  args: JsonObject,
  commandDisplay: string,
  timeoutMaxMs = DEFAULT_COMMAND_TIMEOUT_MAX_MS
): number {
  const maximum = Math.max(1, timeoutMaxMs);
  if (args.timeout_ms !== undefined) {
    return boundedInteger(args.timeout_ms, DEFAULT_PROCESS_TIMEOUT_MS, 1, maximum);
  }
  const commandKind = classifyCommandKind(args);
  const lowered = commandDisplay.toLowerCase();
  const longRunning = ['cargo_test', 'cargo_check', 'build'].includes(commandKind)
    || /(?:^|\s)cargo(?:\.exe)?\s+(?:build|check|test|clippy)(?:\s|$)/.test(lowered)
    || /(?:^|\s)tauri(?:\.exe)?\s+build(?:\s|$)/.test(lowered)
    || packageScriptLooksLongRunning(commandDisplay);
  return longRunning ? maximum : Math.min(DEFAULT_PROCESS_TIMEOUT_MS, maximum);
}
