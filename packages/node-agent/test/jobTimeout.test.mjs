import test from 'node:test';
import assert from 'node:assert/strict';
import * as policy from '../dist/processes/timeoutPolicy.js';
import { operationResultSummary } from '../dist/operationSummary.js';

const commandMax = 1_800_000;
const jobMax = 21_600_000;
const resolve = (args, display = 'node benchmark.mjs', c = commandMax, j = jobMax) =>
  policy.resolveProcessTimeout(args, display, c, j);

test('ordinary timeout contract is explicit and keeps automatic defaults', () => {
  assert.deepEqual(resolve({}), { executionMode: 'command', requestedTimeoutMs: null, effectiveTimeoutMs: 30_000, limitMs: commandMax });
  assert.equal(resolve({}, 'cargo build').effectiveTimeoutMs, commandMax);
  assert.equal(resolve({ timeout_ms: 45_000 }).effectiveTimeoutMs, 45_000);
});

test('job budget is separate from the normal command cap', () => {
  assert.deepEqual(resolve({ job_timeout_ms: 7_200_000, operation_id: 'benchmark-20-steps' }), {
    executionMode: 'job', requestedTimeoutMs: 7_200_000, effectiveTimeoutMs: 7_200_000, limitMs: jobMax
  });
});

test('oversized budgets are rejected, never silently shortened', () => {
  for (const args of [{ timeout_ms: commandMax + 1 }, { job_timeout_ms: jobMax + 1, operation_id: 'job' }]) {
    assert.throws(() => resolve(args), error => error.code === 'COMMAND_TIMEOUT_EXCEEDS_LIMIT' && error.details.process_started === false);
  }
});

test('job mode requires an explicit bounded budget and stable id', () => {
  for (const args of [
    { job_timeout_ms: 1000 }, { job_timeout_ms: 1000, operation_id: '   ' },
    { job_timeout_ms: 1000, operation_id: 123 }, { job_timeout_ms: 1000, operation_id: 'x'.repeat(129) },
    { job_timeout_ms: 1000, operation_id: 'x', timeout_ms: 1000 },
    ...[0, -1, 0.5, null, '1000', Infinity, NaN].map(job_timeout_ms => ({ job_timeout_ms, operation_id: 'x' })),
    ...[0, -1, 0.5, null, '1000', Infinity, NaN].map(timeout_ms => ({ timeout_ms }))
  ]) assert.throws(() => resolve(args), { code: 'INVALID_ARGUMENT' });
});

test('host job cap can disable jobs but does not disable ordinary commands', () => {
  assert.throws(() => resolve({ job_timeout_ms: 1, operation_id: 'job' }, '', commandMax, 0), { code: 'LONG_RUNNING_JOBS_DISABLED' });
  assert.equal(resolve({ timeout_ms: 10 }, '', commandMax, 0).effectiveTimeoutMs, 10);
  assert.throws(() => resolve({ job_timeout_ms: 86_400_001, operation_id: 'job' }, '', commandMax, Number.MAX_SAFE_INTEGER), { code: 'COMMAND_TIMEOUT_EXCEEDS_LIMIT' });
});

test('host environment job cap is explicit, bounded, and fails closed', () => {
  assert.equal(policy.configuredJobTimeoutMaxMs({}), jobMax);
  assert.equal(policy.configuredJobTimeoutMaxMs({ CTMCP_JOB_TIMEOUT_MAX_MS: '0' }), 0);
  assert.equal(policy.configuredJobTimeoutMaxMs({ CTMCP_JOB_TIMEOUT_MAX_MS: '7200000' }), 7_200_000);
  assert.equal(policy.configuredJobTimeoutMaxMs({ CTMCP_JOB_TIMEOUT_MAX_MS: '99999999999' }), 86_400_000);
  for (const value of ['bad', '-1', '1.5', '']) assert.equal(policy.configuredJobTimeoutMaxMs({ CTMCP_JOB_TIMEOUT_MAX_MS: value }), 0);
});

test('operation summaries retain fixed timeout contract metadata without raw command data', () => {
  const summary = operationResultSummary('exec_command', {
    ok: true, command_ok: true, execution_mode: 'job', timeout_scope: 'process',
    timeout_clamped: false, polling_extends_process_deadline: false,
    requested_process_timeout_ms: 7_200_000, effective_process_timeout_ms: 7_200_000,
    process_timeout_limit_ms: 21_600_000, process_deadline_ts_ms: 123_456_789,
    process_timeout_remaining_ms: 7_199_000, command: 'secret command', stdout: 'secret output'
  });
  assert.equal(summary.execution_mode, 'job');
  assert.equal(summary.timeout_scope, 'process');
  assert.equal(summary.polling_extends_process_deadline, false);
  assert.equal(summary.effective_process_timeout_ms, 7_200_000);
  assert.equal(summary.process_timeout_limit_ms, 21_600_000);
  assert.equal(Object.hasOwn(summary, 'command'), false);
  assert.equal(Object.hasOwn(summary, 'stdout'), false);
});
