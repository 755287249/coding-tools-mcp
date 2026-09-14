import test from 'node:test';
import assert from 'node:assert/strict';
import { access, appendFile, mkdtemp, readFile, rm } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import path from 'node:path';
import { createToolContext } from '../dist/server.js';
import { callTool } from '../dist/tools.js';
import {
  DIAGNOSTICS_LOG_FILE,
  DIAGNOSTICS_SCHEMA_VERSION,
  TOOL_USAGE_LOG_FILE,
  TOOL_USAGE_QUEUE_CAPACITY,
  TOOL_USAGE_SCHEMA_VERSION,
  ToolUsageStore
} from '../dist/toolUsage.js';

const nodeProgram = path.basename(process.execPath);

test('tool usage context depends on a pure contract instead of the telemetry implementation', async () => {
  const [typesSource, usageSource, contractSource, logStoreSource] = await Promise.all([
    readFile(new URL('../src/types.ts', import.meta.url), 'utf8'),
    readFile(new URL('../src/toolUsage.ts', import.meta.url), 'utf8'),
    readFile(new URL('../src/toolUsage/contract.ts', import.meta.url), 'utf8'),
    readFile(new URL('../src/toolUsage/logStore.ts', import.meta.url), 'utf8')
  ]);
  assert.match(typesSource, /from ['"]\.\/toolUsage\/contract\.js['"]/);
  assert.doesNotMatch(typesSource, /from ['"]\.\/toolUsage\.js['"]/);
  assert.match(usageSource, /implements ToolUsageStoreContract/);
  assert.doesNotMatch(contractSource, /from\s+['"]/);
  assert.match(logStoreSource, /createReadStream/);
  assert.match(logStoreSource, /visitCompleteRecords/);
  assert.doesNotMatch(logStoreSource, /readFile\(/);
});

function config(root, dataDir) {
  return {
    host: '127.0.0.1', port: 0, dataDir, permissionMode: 'trusted',
    management: { enabled: false },
    oauth: { clientId: 'chatgpt', password: 'usage-test-password', tokenSecret: 'usage-test-token-secret' },
    folders: [{ id: 'repo', name: 'Repo', path: root }],
    limits: { blockingConcurrency: 4, processConcurrency: 4, activeSessionLimit: 32, maxOutputBytes: 1024 * 1024 }
  };
}

async function fixture(t) {
  const root = await mkdtemp(path.join(tmpdir(), 'ctmcp-usage-root-'));
  const dataDir = await mkdtemp(path.join(tmpdir(), 'ctmcp-usage-data-'));
  t.after(async () => {
    await rm(root, { recursive: true, force: true, maxRetries: 5, retryDelay: 50 });
    await rm(dataDir, { recursive: true, force: true, maxRetries: 5, retryDelay: 50 });
  });
  return { root, dataDir };
}

function requestTiming(overrides = {}) {
  return {
    previousResponseCompletedTsMs: null,
    orchestrationGapMs: null,
    activityBurstId: 1,
    activityBurstSequence: 1,
    concurrentRequest: false,
    ...overrides
  };
}

function toolRecord(overrides = {}) {
  return {
    schema_version: TOOL_USAGE_SCHEMA_VERSION,
    event: 'tool_call',
    workspace_id: 'profile-a',
    runtime_boot_id: 'runtime-a',
    server_version: '0.19.0',
    started_ts_ms: 1_000,
    completed_ts_ms: 1_010,
    duration_ms: 10,
    tool: 'exec_command',
    tool_family: 'process',
    outcome: 'success',
    outcome_class: 'success',
    is_error: false,
    request_json_bytes: 10,
    response_json_bytes: 100,
    arguments_sha256: 'a'.repeat(64),
    activity_burst_id: 1,
    activity_burst_sequence: 1,
    concurrent_request: false,
    orchestration_gap_ms: null,
    command_kind: 'cargo_check',
    ...overrides
  };
}

test('persistent usage records survive restart and obey runtime/version/all scopes', async t => {
  const { root, dataDir } = await fixture(t);
  const first = await createToolContext(config(root, dataDir));
  const result = await callTool(first, 'server_info', {}, { 'openai/session': 'usage-persistence' });
  assert.equal(result.ok, true);
  await first.usageStore.flush();

  const second = await createToolContext(config(root, dataDir));
  const all = await second.usageStore.query({ scope: 'all', include_records: true, exclude_tools: [] });
  assert.equal(all.matched_lines, 1);
  assert.equal(all.records[0].tool, 'server_info');
  assert.equal(all.records[0].schema_version, TOOL_USAGE_SCHEMA_VERSION);

  const currentVersion = await second.usageStore.query({ scope: 'current_version', exclude_tools: [] });
  assert.equal(currentVersion.matched_lines, 1);
  const currentRuntime = await second.usageStore.query({ scope: 'current_runtime', exclude_tools: [] });
  assert.equal(currentRuntime.matched_lines, 0);

  const viaTool = await callTool(second, 'query_tool_usage', { scope: 'all', include_records: true }, { 'openai/session': 'usage-persistence' });
  assert.equal(viaTool.ok, true);
  assert.equal(viaTool.matched_lines, 1);
  assert.equal(viaTool.records[0].tool, 'server_info');
});

test('scope all separates current-version telemetry from historical versions', async t => {
  const { dataDir } = await fixture(t);
  const store = new ToolUsageStore(dataDir, {
    profileId: 'version-scope-profile', runtimeBootId: 'runtime-current', serverVersion: '0.19.0'
  });
  store.enqueue(toolRecord({ server_version: '0.18.0', runtime_boot_id: 'runtime-old', duration_ms: 40 }));
  store.enqueue(toolRecord({ server_version: '0.19.0', runtime_boot_id: 'runtime-current', duration_ms: 10 }));

  const report = await store.query({ scope: 'all', exclude_tools: [] });
  assert.equal(report.scope_breakdown.current_version.version, '0.19.0');
  assert.equal(report.scope_breakdown.current_version.stats.calls, 1);
  assert.equal(report.scope_breakdown.current_version.stats.duration_ms, 10);
  assert.deepEqual(report.scope_breakdown.previous_versions.versions, ['0.18.0']);
  assert.equal(report.scope_breakdown.previous_versions.stats.calls, 1);
  assert.equal(report.scope_breakdown.previous_versions.stats.duration_ms, 40);
  assert.match(report.scope_breakdown.analysis_hint, /Prioritize current_version/);
});

test('request timing distinguishes concurrency from the next orchestration gap', async t => {
  const { dataDir } = await fixture(t);
  const store = new ToolUsageStore(dataDir, {
    profileId: 'timing-profile', runtimeBootId: 'timing-runtime', serverVersion: '0.19.0'
  });
  const firstTiming = store.beginRequest(1_000);
  const secondTiming = store.beginRequest(1_001);
  assert.equal(firstTiming.concurrentRequest, false);
  assert.equal(secondTiming.concurrentRequest, true);
  assert.equal(secondTiming.orchestrationGapMs, null);
  store.recordToolCall({
    tool: 'server_info', arguments: {}, result: {
      ok: true,
      phase_durations_ms: {
        preflight_ms: 4, plan_ms: 2, commit_ms: 3, total_ms: 9,
        baseline_capture_ms: 5, error_enrichment_ms: 1,
        harness_begin_ms: 6, dispatch_ms: 2, harness_finish_ms: 7, serialization_ms: 1
      }
    },
    startedTsMs: 1_000, durationMs: 10, requestTiming: firstTiming
  });
  store.recordToolCall({
    tool: 'read_file', arguments: { path: 'a.txt' }, result: { ok: true },
    startedTsMs: 1_001, durationMs: 20, requestTiming: secondTiming
  });
  const thirdTiming = store.beginRequest(1_100);
  assert.equal(thirdTiming.concurrentRequest, false);
  assert.equal(thirdTiming.previousResponseCompletedTsMs, 1_021);
  assert.equal(thirdTiming.orchestrationGapMs, 79);
  store.recordToolCall({
    tool: 'project_map', arguments: {}, result: { ok: true },
    startedTsMs: 1_100, durationMs: 5, requestTiming: thirdTiming
  });
  await store.flush();

  const result = await store.query({ scope: 'all', exclude_tools: [], include_records: true });
  assert.equal(result.records.length, 3);
  assert.equal(result.records[0].phase_preflight_ms, 4);
  assert.equal(result.records[0].phase_plan_ms, 2);
  assert.equal(result.records[0].phase_commit_ms, 3);
  assert.equal(result.records[0].phase_total_ms, 9);
  assert.equal(result.records[0].phase_baseline_capture_ms, 5);
  assert.equal(result.records[0].phase_error_enrichment_ms, 1);
  assert.equal(result.records[0].phase_harness_begin_ms, 6);
  assert.equal(result.records[0].phase_dispatch_ms, 2);
  assert.equal(result.records[0].phase_harness_finish_ms, 7);
  assert.equal(result.records[0].phase_serialization_ms, 1);
  assert.equal(result.aggregate.phase_latency.preflight.samples, 1);
  assert.equal(result.aggregate.phase_latency.preflight.total_ms, 4);
  assert.equal(result.aggregate.phase_latency.preflight.avg_ms, 4);
  assert.equal(result.aggregate.phase_latency.preflight.p50_ms, 4);
  assert.equal(result.aggregate.phase_latency.preflight.p95_ms, 4);
  assert.equal(result.aggregate.phase_latency.total.total_ms, 9);
  assert.equal(result.aggregate.phase_latency.baseline_capture.total_ms, 5);
  assert.equal(result.aggregate.phase_latency.error_enrichment.total_ms, 1);
  assert.equal(result.aggregate.phase_latency.harness_begin.total_ms, 6);
  assert.equal(result.aggregate.phase_latency.dispatch.total_ms, 2);
  assert.equal(result.aggregate.phase_latency.harness_finish.total_ms, 7);
  assert.equal(result.aggregate.phase_latency.serialization.total_ms, 1);
  const serverInfoStats = result.aggregate.tools.find(tool => tool.tool === 'server_info');
  assert.equal(serverInfoStats.phase_latency.commit.samples, 1);
  assert.equal(serverInfoStats.phase_latency.commit.max_ms, 3);
  const legacyStats = result.aggregate.tools.find(tool => tool.tool === 'read_file');
  assert.equal(legacyStats.phase_latency.preflight.samples, 0);
  assert.equal(result.records[1].concurrent_request, true);
  assert.equal(result.records[2].orchestration_gap_ms, 79);
  assert.equal(result.performance.client_orchestration_gap_ms, 79);
});

test('outcome taxonomy separates recoverable tool failures from internal errors', async t => {
  const { dataDir } = await fixture(t);
  const store = new ToolUsageStore(dataDir, {
    profileId: 'taxonomy-profile', runtimeBootId: 'taxonomy-runtime', serverVersion: '0.19.0'
  });
  const cases = [
    ['EDIT_MATCH_COUNT_MISMATCH', 'validation', 'target_resolution_error'],
    ['PATCH_CONTEXT_AMBIGUOUS', 'validation', 'target_resolution_error'],
    ['FILE_VERSION_MISMATCH', 'conflict', 'state_conflict'],
    ['GIT_REPO_TARGET_MISMATCH', 'conflict', 'routing_error'],
    ['EDIT_CONTRACT_INVALID', 'validation', 'caller_argument_error'],
    ['PROTECTED_PATH', 'security', 'policy_rejection'],
    ['E_FAIL', 'runtime', 'internal_error']
  ];
  for (let index = 0; index < cases.length; index += 1) {
    const [code, category] = cases[index];
    store.recordToolCall({
      tool: 'edit_file',
      arguments: { path: `file-${index}.txt` },
      result: { ok: false, error: { code, category, retryable: false, details: {} } },
      startedTsMs: 1_000 + index,
      durationMs: 1,
      requestTiming: requestTiming({ activityBurstSequence: index + 1 })
    });
  }
  await store.flush();
  const queried = await store.query({ scope: 'all', exclude_tools: [], include_records: true });
  const byCode = new Map(queried.records.map(record => [record.error_code, record.outcome_class]));
  for (const [code, , expected] of cases) assert.equal(byCode.get(code), expected, code);
});

test('canonical diagnostics envelope separates transport failures from tool failures', async t => {
  const contract = JSON.parse(await readFile(new URL('../../../docs/specs/diagnostics/contract-v1.json', import.meta.url), 'utf8'));
  assert.equal(contract.schema_version, DIAGNOSTICS_SCHEMA_VERSION);
  assert.equal(contract.structured_log.file, DIAGNOSTICS_LOG_FILE);
  assert.equal(contract.compatibility.legacy_file, TOOL_USAGE_LOG_FILE);
  assert.equal(contract.compatibility.dual_write, true);
  assert.equal(contract.compatibility.dual_write_default, true);
  assert.equal(contract.compatibility.legacy_write_env, 'CODING_TOOLS_DIAGNOSTICS_LEGACY_WRITE');
  assert.equal(contract.compatibility.legacy_read_preserved, true);
  assert.equal(contract.compatibility.retirement_requires_compatibility_window, true);
  assert.equal(contract.compatibility.query_deduplicates_by, 'diagnostic_event_id');
  assert.equal(contract.records_pagination.cursor_origin, 'latest');
  assert.equal(contract.records_pagination.page_order, 'oldest_to_newest');
  assert.equal(contract.records_pagination.max_cursor, 10_000);
  assert.equal(contract.host_kinds.node, 'node_agent');
  assert.equal(contract.diagnostics_summary.sanitized, true);
  assert.ok(contract.diagnostics_summary.log_health_fields.includes('write_failures'));
  assert.ok(contract.diagnostics_summary.log_health_fields.includes('legacy_compat_write_failures'));
  const { dataDir } = await fixture(t);
  const store = new ToolUsageStore(dataDir, {
    profileId: 'diagnostics-profile', runtimeBootId: 'diagnostics-runtime', serverVersion: '0.19.0'
  });

  const toolFailure = store.recordToolCall({
    tool: 'edit_file',
    arguments: { path: 'main.txt' },
    result: { ok: false, harness_operation_id: 'harness-op-1', error: { code: 'E_FAIL', category: 'runtime', retryable: false, details: {} } },
    startedTsMs: 1_000,
    durationMs: 5,
    requestTiming: requestTiming()
  });
  assert.equal(toolFailure.diagnostic_schema_version, DIAGNOSTICS_SCHEMA_VERSION);
  assert.equal(toolFailure.event_type, 'tool_call');
  assert.equal(toolFailure.host_kind, 'node_agent');
  assert.equal(toolFailure.host_version, '0.19.0');
  assert.equal(toolFailure.severity, 'error');
  assert.equal(toolFailure.failure_domain, 'tool');
  assert.equal(toolFailure.timestamp_ms, 1_000);
  assert.equal(toolFailure.conversation_operation_id, 'harness-op-1');

  const processSession = store.recordAsyncSession({
    sessionId: 'session-1',
    commandKind: 'node_test',
    startedTsMs: 1_100,
    completedTsMs: 1_120,
    childProcessTotalMs: 20,
    exitCode: 0,
    terminationReason: 'exited',
    stdoutBytes: 12,
    stderrBytes: 0
  });
  assert.equal(processSession.diagnostic_schema_version, DIAGNOSTICS_SCHEMA_VERSION);
  assert.equal(processSession.event_type, 'process_session');
  assert.equal(processSession.host_kind, 'node_agent');
  assert.equal(processSession.severity, 'info');
  assert.equal(processSession.failure_domain, 'none');
  assert.equal(processSession.timestamp_ms, 1_100);
  assert.equal(processSession.conversation_operation_id, null);

  const recoveryEvent = store.recordDiagnosticEvent({
    eventType: 'recovery_event',
    event: 'recovery_attempt',
    severity: 'info',
    failureDomain: 'none',
    fields: { tool: 'edit_file', recovery_succeeded: true }
  });
  assert.equal(recoveryEvent.event_type, 'recovery_event');
  assert.equal(recoveryEvent.event, 'recovery_attempt');
  assert.equal(recoveryEvent.recovery_succeeded, true);
  assert.ok(Number(recoveryEvent.timestamp_ms) > 0);
  assert.equal(recoveryEvent.conversation_operation_id, null);
  assert.match(recoveryEvent.diagnostic_event_id, /^[0-9a-f-]{36}$/i);
  for (const field of contract.common_fields) {
    assert.equal(Object.hasOwn(toolFailure, field), true, `tool_call missing ${field}`);
    assert.equal(Object.hasOwn(processSession, field), true, `process_session missing ${field}`);
    assert.equal(Object.hasOwn(recoveryEvent, field), true, `recovery_event missing ${field}`);
  }

  store.recordDiagnosticEvent({
    eventType: 'transport_event',
    event: 'mcp_transport_error',
    severity: 'error',
    failureDomain: 'transport',
    fields: { rpc_error_code: '-32022', http_status: 400 }
  });

  store.enqueue(toolRecord({
    started_ts_ms: 1_200,
    completed_ts_ms: 1_201,
    event_type: 'transport_event',
    severity: 'error',
    failure_domain: 'transport',
    outcome: 'rpc_error',
    outcome_class: 'transport_error',
    is_error: false,
    rpc_error_code: '-32603',
    telemetry_dropped_before: 3,
    arguments_sha256: 'd'.repeat(64)
  }));
  await store.flush();

  const queried = await store.query({ scope: 'all', exclude_tools: [] });
  assert.equal(Number.isSafeInteger(queried.log_bytes_read), true);
  assert.deepEqual(
    Object.keys(queried).sort(),
    [...contract.query_response.fields].sort(),
    'Node diagnostics query keys must match the shared contract exactly'
  );
  assert.equal(queried.aggregate.errors, 2, 'legacy combined error count remains compatible');
  assert.equal(queried.aggregate.tool_errors, 1);
  assert.equal(queried.aggregate.transport_errors, 1);
  assert.equal(queried.aggregate.tool_errors_by_code.E_FAIL, 1);
  assert.equal(queried.aggregate.transport_errors_by_code['-32603'], 1);
  assert.equal(queried.diagnostics_contract.schema_version, DIAGNOSTICS_SCHEMA_VERSION);
  assert.equal(queried.diagnostics_contract.host_kind, 'node_agent');
  assert.equal(queried.diagnostics_contract.structured_log_file, DIAGNOSTICS_LOG_FILE);
  assert.equal(queried.diagnostics_contract.compatibility.legacy_file, TOOL_USAGE_LOG_FILE);
  assert.equal(queried.diagnostics_contract.compatibility.dual_write, true);
  assert.equal(queried.diagnostics_contract.legacy_errors_include_transport, true);
  assert.equal(queried.diagnostics_summary.schema_version, DIAGNOSTICS_SCHEMA_VERSION);
  assert.equal(queried.diagnostics_summary.host_kind, 'node_agent');
  assert.equal(queried.diagnostics_summary.calls, 2);
  assert.equal(queried.diagnostics_summary.tool_errors, 1);
  assert.equal(queried.diagnostics_summary.transport_errors, 2);
  assert.deepEqual(queried.diagnostics_summary.events_by_type, {
    process_session: 1,
    recovery_event: 1,
    tool_call: 1,
    transport_event: 2
  });
  assert.equal(queried.diagnostics_summary.persisted_dropped_records, 3);
  assert.deepEqual(queried.diagnostics_summary.top_tool_errors, [{ code: 'E_FAIL', count: 1 }]);
  assert.deepEqual(queried.diagnostics_summary.top_transport_errors, [
    { code: '-32022', count: 1 },
    { code: '-32603', count: 1 }
  ]);
  assert.equal(queried.diagnostics_summary.log_health.queue_capacity, TOOL_USAGE_QUEUE_CAPACITY);
  assert.equal(queried.diagnostics_summary.log_health.pending_records, 0);
  assert.equal(queried.diagnostics_summary.log_health.pending_dropped_records, 0);
  assert.equal(queried.diagnostics_summary.log_health.write_failures, 0);
  assert.equal(queried.diagnostics_summary.log_health.legacy_compat_write_failures, 0);
  assert.equal(queried.diagnostics_summary.log_health.write_error_present, false);
  assert.ok(Number(queried.diagnostics_summary.log_health.last_successful_write_ts_ms) > 0);
  assert.equal(queried.diagnostics_summary.sanitized, true);
  assert.deepEqual(
    Object.keys(queried.diagnostics_summary).sort(),
    [...contract.diagnostics_summary.fields].sort(),
    'Node diagnostics summary keys must match the shared contract exactly'
  );
  assert.deepEqual(
    Object.keys(queried.diagnostics_summary.log_health).sort(),
    [...contract.diagnostics_summary.log_health_fields].sort(),
    'Node log health keys must match the shared contract exactly'
  );
  assert.doesNotMatch(JSON.stringify(queried.diagnostics_summary), /main\.txt|arguments|stdout|stderr/i);
});

test('recovery attempts emit linked canonical recovery events without duplicating tool payloads', async t => {
  const { dataDir } = await fixture(t);
  const store = new ToolUsageStore(dataDir, {
    profileId: 'recovery-events-profile', runtimeBootId: 'recovery-events-runtime', serverVersion: '0.19.0'
  });
  const toolCall = store.recordToolCall({
    tool: 'edit_file',
    arguments: { retry_of_call_sequence: 7, path: 'do-not-copy.txt' },
    result: { ok: true, stdout: 'do-not-copy-output' },
    startedTsMs: 3_000,
    durationMs: 4,
    requestTiming: requestTiming()
  });
  await store.flush();

  const canonical = (await readFile(path.join(dataDir, 'logs', DIAGNOSTICS_LOG_FILE), 'utf8')).trim().split('\n').map(line => JSON.parse(line));
  const recovery = canonical.find(record => record.event_type === 'recovery_event');
  assert.ok(recovery);
  assert.equal(recovery.linked_diagnostic_event_id, toolCall.diagnostic_event_id);
  assert.equal(recovery.retry_of_call_sequence, 7);
  assert.equal(recovery.recovery_succeeded, true);
  assert.equal(Object.hasOwn(recovery, 'arguments'), false);
  assert.doesNotMatch(JSON.stringify(recovery), /do-not-copy/);

  const queried = await store.query({ scope: 'all', exclude_tools: [] });
  assert.equal(queried.diagnostics_summary.events_by_type.tool_call, 1);
  assert.equal(queried.diagnostics_summary.events_by_type.recovery_event, 1);
});

test('canonical diagnostics log dual-writes with stable ids and query deduplicates compatibility copies', async t => {
  const { dataDir } = await fixture(t);
  const store = new ToolUsageStore(dataDir, {
    profileId: 'migration-profile', runtimeBootId: 'migration-runtime', serverVersion: '0.19.0'
  });
  const record = store.recordToolCall({
    tool: 'server_info', arguments: {}, result: { ok: true },
    startedTsMs: 4_000, durationMs: 3, requestTiming: requestTiming()
  });
  assert.match(record.diagnostic_event_id, /^[0-9a-f-]{36}$/i);
  await store.flush();

  const canonical = (await readFile(path.join(dataDir, 'logs', DIAGNOSTICS_LOG_FILE), 'utf8')).trim().split('\n').map(line => JSON.parse(line));
  const legacy = (await readFile(path.join(dataDir, 'logs', TOOL_USAGE_LOG_FILE), 'utf8')).trim().split('\n').map(line => JSON.parse(line));
  assert.equal(canonical.length, 1);
  assert.equal(legacy.length, 1);
  assert.equal(canonical[0].diagnostic_event_id, record.diagnostic_event_id);
  assert.equal(legacy[0].diagnostic_event_id, record.diagnostic_event_id);

  const queried = await store.query({ scope: 'all', exclude_tools: [], include_records: true });
  assert.equal(queried.aggregate.calls, 1);
  assert.equal(queried.matched_lines, 1);
  assert.equal(queried.diagnostics_contract.compatibility.query_deduplicates_by, 'diagnostic_event_id');
  assert.equal(queried.diagnostics_contract.migration.duplicate_records_ignored, 1);
});

test('legacy diagnostics writer can retire while query keeps reading historical legacy records', async t => {
  const { dataDir } = await fixture(t);
  const store = new ToolUsageStore(dataDir, {
    profileId: 'retirement-profile',
    runtimeBootId: 'retirement-runtime',
    serverVersion: '0.19.0',
    legacyWriteEnabled: false
  });
  store.recordToolCall({
    tool: 'server_info', arguments: {}, result: { ok: true },
    startedTsMs: 5_000, durationMs: 2, requestTiming: requestTiming()
  });
  await store.flush();

  const logDir = path.join(dataDir, 'logs');
  await access(path.join(logDir, DIAGNOSTICS_LOG_FILE));
  await assert.rejects(access(path.join(logDir, TOOL_USAGE_LOG_FILE)));

  const historical = toolRecord({
    diagnostic_event_id: 'legacy-history-1',
    workspace_id: 'retirement-profile',
    runtime_boot_id: 'legacy-runtime',
    started_ts_ms: 4_000,
    completed_ts_ms: 4_001,
    tool: 'read_file',
    tool_family: 'read'
  });
  await appendFile(path.join(logDir, TOOL_USAGE_LOG_FILE), `${JSON.stringify(historical)}\n`, 'utf8');

  const queried = await store.query({ scope: 'all', exclude_tools: [], include_records: true });
  assert.equal(queried.diagnostics_contract.compatibility.dual_write, false);
  assert.equal(queried.diagnostics_contract.compatibility.legacy_read_preserved, true);
  assert.equal(queried.diagnostics_contract.compatibility.legacy_write_env, 'CODING_TOOLS_DIAGNOSTICS_LEGACY_WRITE');
  assert.equal(queried.aggregate.calls, 2);
  assert.equal(queried.diagnostics_contract.migration.legacy_scanned_lines, 1);
  assert.equal(queried.diagnostics_contract.migration.canonical_scanned_lines, 1);
});

test('query records paginate backward from the latest window while preserving page order', async t => {
  const { dataDir } = await fixture(t);
  const store = new ToolUsageStore(dataDir, {
    profileId: 'pagination-profile',
    runtimeBootId: 'pagination-runtime',
    serverVersion: '0.19.0',
    legacyWriteEnabled: false
  });
  for (let index = 1; index <= 5; index += 1) {
    store.recordToolCall({
      tool: 'server_info', arguments: {}, result: { ok: true },
      startedTsMs: index * 1_000, durationMs: 1, requestTiming: requestTiming()
    });
  }
  await store.flush();

  const summaryOnly = await store.query({ scope: 'all', exclude_tools: [], include_records: false, limit: 2, cursor: 2 });
  assert.equal(summaryOnly.records_pagination, null);

  const first = await store.query({ scope: 'all', exclude_tools: [], include_records: true, limit: 2, cursor: 0 });
  assert.deepEqual(first.records.map(record => record.started_ts_ms), [4_000, 5_000]);
  assert.deepEqual(first.records_pagination, {
    cursor: 0,
    next_cursor: 2,
    total: 5,
    limit: 2,
    cursor_origin: 'latest',
    page_order: 'oldest_to_newest',
    cursor_limit_reached: false
  });

  const second = await store.query({ scope: 'all', exclude_tools: [], include_records: true, limit: 2, cursor: 2 });
  assert.deepEqual(second.records.map(record => record.started_ts_ms), [2_000, 3_000]);
  assert.equal(second.records_pagination.next_cursor, 4);

  const third = await store.query({ scope: 'all', exclude_tools: [], include_records: true, limit: 2, cursor: 4 });
  assert.deepEqual(third.records.map(record => record.started_ts_ms), [1_000]);
  assert.equal(third.records_pagination.next_cursor, null);
  assert.equal(third.records_pagination.total, 5);
});

test('wait request timeout is classified separately from process timeout', async t => {
  const { dataDir } = await fixture(t);
  const store = new ToolUsageStore(dataDir, {
    profileId: 'wait-taxonomy-profile', runtimeBootId: 'wait-taxonomy-runtime', serverVersion: '0.19.0'
  });
  const waitTimeout = store.recordToolCall({
    tool: 'wait_command', arguments: { session_id: 'session-a', timeout_ms: 20 },
    result: { ok: true, request_timed_out: true, process_timed_out: false, process_still_running: true },
    startedTsMs: 1_000, durationMs: 20, requestTiming: requestTiming()
  });
  const processTimeout = store.recordToolCall({
    tool: 'wait_command', arguments: { session_id: 'session-b', timeout_ms: 20 },
    result: { ok: false, request_timed_out: false, process_timed_out: true, process_still_running: false },
    startedTsMs: 1_100, durationMs: 1, requestTiming: requestTiming({ activityBurstSequence: 2 })
  });
  assert.equal(waitTimeout.outcome_class, 'wait_timeout');
  assert.equal(processTimeout.outcome_class, 'timeout');
});

test('repeated failure detection resets on success and burst while counting concurrent duplicates', async t => {
  const { dataDir } = await fixture(t);
  const store = new ToolUsageStore(dataDir, {
    profileId: 'repeat-profile', runtimeBootId: 'repeat-runtime', serverVersion: '0.19.0'
  });
  const failure = {
    ok: false,
    error: {
      code: 'EDIT_MATCH_COUNT_MISMATCH',
      category: 'validation',
      retryable: false,
      details: { path: 'main.txt', edit_index: 0, actual_occurrences: 0, expected_occurrences: 1 }
    }
  };
  const call = (argumentsValue, timing, result = failure) => store.recordToolCall({
    tool: 'edit_file', arguments: argumentsValue, result,
    startedTsMs: 2_000 + timing.activityBurstSequence,
    durationMs: 1,
    requestTiming: timing
  });
  const first = call({ path: 'main.txt', edits: [{ type: 'replace', old_text: 'a', new_text: 'b' }] }, requestTiming({ activityBurstId: 7, activityBurstSequence: 1 }));
  const second = call({ path: 'main.txt', edits: [{ type: 'replace', old_text: 'a', new_text: 'b' }] }, requestTiming({ activityBurstId: 7, activityBurstSequence: 2 }));
  assert.match(first.failure_signature, /^[0-9a-f]{64}$/);
  assert.equal(first.repeat_failure_count, 1);
  assert.equal(first.repeated_failure, false);
  assert.equal(second.failure_signature, first.failure_signature);
  assert.equal(second.repeat_failure_count, 2);
  assert.equal(second.repeated_failure, true);
  assert.equal(second.retry_without_change, true);

  const changed = call({ path: 'main.txt', edits: [{ type: 'replace', old_text: 'different', new_text: 'b' }] }, requestTiming({ activityBurstId: 7, activityBurstSequence: 3 }));
  assert.equal(changed.repeat_failure_count, 1);
  const newBurst = call({ path: 'main.txt', edits: [{ type: 'replace', old_text: 'different', new_text: 'b' }] }, requestTiming({ activityBurstId: 8, activityBurstSequence: 1 }));
  assert.equal(newBurst.repeat_failure_count, 1);
  const concurrent = call({ path: 'main.txt', edits: [{ type: 'replace', old_text: 'different', new_text: 'b' }] }, requestTiming({ activityBurstId: 8, activityBurstSequence: 2, concurrentRequest: true }));
  assert.equal(concurrent.repeat_failure_count, 2);
  assert.equal(concurrent.repeated_failure, true);
  assert.equal(concurrent.concurrent_duplicate_failure, true);
  const afterConcurrent = call({ path: 'main.txt', edits: [{ type: 'replace', old_text: 'different', new_text: 'b' }] }, requestTiming({ activityBurstId: 8, activityBurstSequence: 3 }));
  assert.equal(afterConcurrent.repeat_failure_count, 3);
  assert.equal(afterConcurrent.repeated_failure, true);

  const success = call(
    { path: 'main.txt', edits: [{ type: 'replace', old_text: 'different', new_text: 'b' }] },
    requestTiming({ activityBurstId: 8, activityBurstSequence: 4 }),
    { ok: true }
  );
  assert.equal(success.failure_signature, undefined);
  const afterSuccess = call({ path: 'main.txt', edits: [{ type: 'replace', old_text: 'different', new_text: 'b' }] }, requestTiming({ activityBurstId: 8, activityBurstSequence: 5 }));
  assert.equal(afterSuccess.repeat_failure_count, 1);
  await store.flush();
  const queried = await store.query({ scope: 'all', exclude_tools: [] });
  const repeated = queried.optimization.repeated_failures;
  assert.equal(repeated.retry_count, 3);
  assert.equal(repeated.chain_count, 2);
  assert.equal(repeated.wasted_duration_ms, 3);
  assert.equal(repeated.max_attempt_count, 3);
  assert.ok(repeated.friction_score > 0);
  assert.equal(repeated.top[0].signature, concurrent.failure_signature);
  assert.equal(repeated.top[0].retry_count, 2);
  assert.equal(repeated.top[0].max_attempt_count, 3);
  assert.equal(repeated.top[0].error_code, 'EDIT_MATCH_COUNT_MISMATCH');
  assert.equal(repeated.top[0].deterministic_error_weight, 2);
  assert.ok(repeated.top[0].friction_score > 0);
  assert.match(repeated.recovery_hint, /Stop retrying unchanged arguments/);
});

test('search telemetry aggregates scan cost and usefulness signals', async t => {
  const { dataDir } = await fixture(t);
  const store = new ToolUsageStore(dataDir, {
    profileId: 'search-profile', runtimeBootId: 'search-runtime', serverVersion: '0.19.0'
  });
  store.recordToolCall({
    tool: 'search_text',
    arguments: { query: 'needle', max_results: 1 },
    result: {
      ok: true,
      returned_count: 1,
      total_matches: 2,
      total_matches_exact: false,
      files_considered: 2,
      scanned_files: 2,
      matched_files: 2,
      scan_completed: false,
      early_stop_reason: 'result_limit'
    },
    startedTsMs: 2_000,
    durationMs: 5,
    requestTiming: requestTiming({ activityBurstId: 13, activityBurstSequence: 1 })
  });
  store.recordToolCall({
    tool: 'search_text',
    arguments: { query: 'missing' },
    result: {
      ok: true,
      returned_count: 0,
      total_matches: 0,
      total_matches_exact: true,
      files_considered: 3,
      scanned_files: 3,
      matched_files: 0,
      scan_completed: true,
      early_stop_reason: null
    },
    startedTsMs: 2_010,
    durationMs: 5,
    requestTiming: requestTiming({ activityBurstId: 13, activityBurstSequence: 2 })
  });
  await store.flush();
  const queried = await store.query({ scope: 'all', exclude_tools: [] });
  assert.equal(queried.search.files_considered, 5);
  assert.equal(queried.search.files_scanned, 5);
  assert.equal(queried.search.returned_results, 1);
  assert.equal(queried.search.matched_files, 2);
  assert.equal(queried.search.zero_result_calls, 1);
  assert.equal(queried.search.early_stop_calls, 1);
  assert.equal(queried.search.exact_total_calls, 1);
});

test('recovery correlation preserves semantic fingerprints and reports successful chains', async t => {
  const { dataDir } = await fixture(t);
  const store = new ToolUsageStore(dataDir, {
    profileId: 'recovery-profile', runtimeBootId: 'recovery-runtime', serverVersion: '0.19.0'
  });
  const semanticArguments = { path: 'main.txt', start_line: 1 };
  const first = store.recordToolCall({
    tool: 'read_file',
    arguments: semanticArguments,
    result: { ok: false, error: { code: 'NOT_FOUND', category: 'not_found', retryable: false, details: {} } },
    startedTsMs: 1_000,
    durationMs: 10,
    requestTiming: requestTiming({ activityBurstId: 12, activityBurstSequence: 1 })
  });
  const recovered = store.recordToolCall({
    tool: 'read_file',
    arguments: {
      ...semanticArguments,
      retry_of_call_sequence: first.call_sequence,
      recovery_of_operation_id: 'operation-42',
      recovery_action_id: 'read_current_file'
    },
    result: { ok: true, recovery_attempt: true, recovery_succeeded: true },
    startedTsMs: 1_050,
    durationMs: 5,
    requestTiming: requestTiming({ activityBurstId: 12, activityBurstSequence: 2 })
  });
  assert.equal(recovered.arguments_sha256, first.arguments_sha256);
  assert.equal(recovered.retry_of_call_sequence, first.call_sequence);
  assert.match(recovered.recovery_of_operation_id_hash, /^[0-9a-f]{64}$/);
  assert.equal(recovered.recovery_action_id, 'read_current_file');
  assert.equal(recovered.recovery_attempt, true);

  await store.flush();
  const queried = await store.query({ scope: 'all', exclude_tools: [] });
  const chains = queried.optimization.recovery_chains;
  assert.equal(chains.chain_count, 1);
  assert.equal(chains.attempts, 1);
  assert.equal(chains.successful_chains, 1);
  assert.equal(chains.failed_chains, 0);
  assert.equal(chains.top[0].attempts, 1);
  assert.equal(chains.top[0].succeeded, true);
  assert.equal(chains.top[0].elapsed_ms, 45);
  assert.equal(chains.top[0].actions.read_current_file, 1);
});

test('bounded writer queue drops overload and annotates the next accepted record', async t => {
  const { dataDir } = await fixture(t);
  const store = new ToolUsageStore(dataDir, {
    profileId: 'queue-profile', runtimeBootId: 'queue-runtime',
    serverVersion: '0.19.0', queueCapacity: 1
  });
  assert.equal(TOOL_USAGE_QUEUE_CAPACITY, 1_024);
  for (let index = 0; index < 4; index += 1) {
    store.recordToolCall({
      tool: 'server_info', arguments: { index }, result: { ok: true },
      startedTsMs: 1_000 + index, durationMs: 1,
      requestTiming: requestTiming({ activityBurstSequence: index + 1 })
    });
  }
  await store.flush();
  store.recordToolCall({
    tool: 'project_map', arguments: {}, result: { ok: true },
    startedTsMs: 2_000, durationMs: 1,
    requestTiming: requestTiming({ activityBurstSequence: 5 })
  });
  await store.flush();

  const result = await store.query({ scope: 'all', exclude_tools: [], include_records: true });
  const annotated = result.records.find(record => record.tool === 'project_map');
  assert.ok(annotated);
  assert.ok(annotated.telemetry_dropped_before >= 1);
  assert.ok(result.matched_lines < 5);
});

test('rotation keeps bounded history and complete-line reader ignores an active partial tail', async t => {
  const { dataDir } = await fixture(t);
  const store = new ToolUsageStore(dataDir, {
    profileId: 'rotation-profile', runtimeBootId: 'rotation-runtime',
    serverVersion: '0.19.0', maxBytes: 700, retainedFiles: 2
  });
  for (let index = 0; index < 12; index += 1) {
    const startedTsMs = 1_000 + index * 10;
    store.recordToolCall({
      tool: 'server_info', arguments: { index, reason: 'x'.repeat(80) },
      result: { ok: true, returned_count: index }, startedTsMs, durationMs: index + 1,
      requestTiming: requestTiming({ activityBurstSequence: index + 1 })
    });
  }
  await store.flush();
  await access(`${store.logFile}.1`);
  await access(`${store.logFile}.2`);

  await appendFile(store.logFile, 'not-json\n{"partial":', 'utf8');
  const queried = await store.query({ scope: 'all', exclude_tools: [], include_records: true, limit: 1000 });
  assert.ok(queried.scanned_lines > 0);
  assert.equal(queried.invalid_complete_lines, 1);
  assert.ok(queried.log_bytes_read > 0);
  assert.ok(queried.matched_lines > 0);
  assert.ok(queried.matched_lines < 12, 'oldest records should be evicted by bounded rotation');
  assert.ok(queried.records.every(record => record.event === 'tool_call'));

  const currentLog = await readFile(path.join(dataDir, 'logs', TOOL_USAGE_LOG_FILE), 'utf8');
  assert.match(currentLog, /\{"partial":$/);
});

test('query aggregates percentiles, errors, bursts, parallel observations and async lifetimes', async t => {
  const { dataDir } = await fixture(t);
  const store = new ToolUsageStore(dataDir, {
    profileId: 'profile-a', runtimeBootId: 'runtime-a', serverVersion: '0.19.0'
  });
  store.enqueue(toolRecord({
    started_ts_ms: 1_000, completed_ts_ms: 1_010, duration_ms: 10,
    parallelism_observations: [{ pair: 'cargo:test@a|node:test@b', outcome: 'success', overlap_ms: 500, lock_wait_ms: 0 }]
  }));
  store.enqueue(toolRecord({
    started_ts_ms: 1_100, completed_ts_ms: 1_120, duration_ms: 20,
    outcome: 'tool_error', outcome_class: 'internal_error', is_error: true,
    error_code: 'E_FAIL', response_json_bytes: 300, arguments_sha256: 'b'.repeat(64),
    orchestration_gap_ms: 90, activity_burst_sequence: 2,
    parallelism_observations: [{ pair: 'cargo:test@a|node:test@b', outcome: 'conflict', overlap_ms: 25, lock_wait_ms: 10 }]
  }));
  store.enqueue(toolRecord({
    started_ts_ms: 1_200, completed_ts_ms: 1_300, duration_ms: 100,
    outcome: 'tool_error', outcome_class: 'internal_error', is_error: true,
    error_code: 'E_FAIL', response_json_bytes: 200, arguments_sha256: 'b'.repeat(64),
    orchestration_gap_ms: 80, activity_burst_sequence: 3
  }));
  store.enqueue(toolRecord({
    started_ts_ms: 5_000, completed_ts_ms: 5_005, duration_ms: 5,
    tool: 'read_file', tool_family: 'filesystem', command_kind: undefined,
    orchestration_gap_ms: 3_700, activity_burst_id: 2, activity_burst_sequence: 1,
    request_json_bytes: 5, response_json_bytes: 50, arguments_sha256: 'c'.repeat(64)
  }));
  store.enqueue({
    schema_version: TOOL_USAGE_SCHEMA_VERSION,
    event: 'async_session_finalized', workspace_id: 'profile-a',
    runtime_boot_id: 'runtime-a', server_version: '0.19.0',
    session_id: 'async-1', command_kind: 'cargo_check',
    started_ts_ms: 1_000, completed_ts_ms: 2_000,
    child_process_total_ms: 1_000, first_output_ms: 100,
    exit_code: 1, termination_reason: 'process_timeout',
    stdout_bytes: 10, stderr_bytes: 20
  });
  await store.flush();

  const summary = await store.query({
    scope: 'current_runtime', exclude_tools: [],
    burst_idle_ms: 1_000, sort_by: 'p95_ms', top: 10
  });
  assert.equal(summary.response_profile, 'summary');
  assert.deepEqual(summary.detail_sections, {
    records: false,
    slowest: false,
    largest: false,
    activity_bursts: false
  });
  assert.deepEqual(summary.records, []);
  assert.equal(summary.slowest, null);
  assert.equal(summary.largest, null);
  assert.equal(summary.performance.activity_bursts, null);

  const result = await store.query({
    scope: 'current_runtime', exclude_tools: [], include_records: true,
    include_slowest: true, include_largest: true, include_bursts: true,
    include_payloads: false, burst_idle_ms: 1_000, sort_by: 'p95_ms', top: 10
  });
  assert.equal(result.response_profile, 'detailed');
  assert.deepEqual(result.detail_sections, {
    records: true,
    slowest: true,
    largest: true,
    activity_bursts: true
  });
  assert.ok(Buffer.byteLength(JSON.stringify(summary)) < Buffer.byteLength(JSON.stringify(result)));
  assert.equal(result.matched_lines, 4);
  assert.equal(result.matched_async_session_events, 1);
  assert.equal(result.aggregate.calls, 4);
  assert.equal(result.aggregate.errors, 2);
  assert.equal(result.aggregate.p50_ms, 20);
  assert.equal(result.aggregate.p95_ms, 100);
  assert.equal(result.slowest[0].duration_ms, 100);
  assert.equal(result.largest[0].response_json_bytes, 300);
  assert.equal(result.optimization.repeated_identical_error_count, 1);
  assert.equal(result.performance.async_sessions_finalized, 1);
  assert.equal(result.performance.child_process_lifetime_ms, 1_000);
  assert.equal(result.performance.child_process_failures, 1);
  assert.equal(result.performance.child_process_terminations.process_timeout, 1);
  assert.equal(result.performance.child_process_p95_ms, 1_000);
  assert.equal(result.performance.first_output_p95_ms, 100);
  assert.equal(result.performance.parallelization_opportunity_bursts, 1);
  assert.equal(result.performance.parallelizable_exec_command_candidates, 3);
  assert.equal(result.performance.estimated_tool_call_reduction, 2);
  assert.equal(result.parallelism.total_observations, 2);
  assert.equal(result.parallelism.pairs[0].pair, 'cargo:test@a|node:test@b');
  assert.equal(result.parallelism.pairs[0].conflicts, 1);

  const filtered = await store.query({ scope: 'all', tools: ['read_file'], exclude_tools: [], min_duration_ms: 5 });
  assert.equal(filtered.matched_lines, 1);
  assert.equal(filtered.aggregate.tools[0].tool, 'read_file');
  const errors = await store.query({ scope: 'all', errors_only: true, exclude_tools: [] });
  assert.equal(errors.matched_lines, 2);
});

test('payloads are opt-in and persisted arguments remain centrally redacted', async t => {
  const { dataDir } = await fixture(t);
  const store = new ToolUsageStore(dataDir, {
    profileId: 'payload-profile', runtimeBootId: 'payload-runtime', serverVersion: '0.19.0'
  });
  const startedTsMs = 1_000;
  store.recordToolCall({
    tool: 'exec_command',
    arguments: {
      program: nodeProgram,
      args: ['-e', 'process.stdout.write("ok")'],
      password: 'super-secret-password',
      token: 'secret-token-value',
      env: { API_KEY: 'secret-api-key' }
    },
    result: { ok: true, stdout: 'ok', command_ok: true, process_exit_code: 0 },
    startedTsMs, durationMs: 10, requestTiming: requestTiming()
  });
  await store.flush();

  const compact = await store.query({ scope: 'all', exclude_tools: [], include_records: true });
  assert.equal(compact.records.length, 1);
  assert.equal(compact.records[0].arguments, undefined);
  assert.equal(compact.records[0].argument_field_bytes, undefined);

  const full = await store.query({ scope: 'all', exclude_tools: [], include_records: true, include_payloads: true });
  assert.equal(full.records[0].arguments.password, '[REDACTED]');
  assert.equal(full.records[0].arguments.token, '[REDACTED]');
  assert.equal(full.records[0].arguments.env.API_KEY, '[REDACTED]');
  const serialized = JSON.stringify(full.records[0]);
  assert.doesNotMatch(serialized, /super-secret-password|secret-token-value|secret-api-key/);
});

test('real retained process finalization emits an async lifetime event', async t => {
  const { root, dataDir } = await fixture(t);
  const ctx = await createToolContext(config(root, dataDir));
  const meta = { 'openai/session': 'usage-process' };
  const selected = await callTool(ctx, 'switch_workspace_folder', { folder_id: 'repo' }, meta);
  assert.equal(selected.ok, true);
  const executed = await callTool(ctx, 'exec_command', {
    program: nodeProgram,
    args: ['-e', 'setTimeout(() => process.stdout.write("done"), 20)'],
    timeout_ms: 10_000,
    yield_time_ms: 10_000
  }, meta);
  const finalized = executed.command_ok === null
    ? await callTool(ctx, 'wait_command', {
      session_id: executed.session_id,
      cursor: executed.latest_cursor,
      timeout_ms: 10_000,
      until: 'finalized',
      output_mode: 'delta'
    }, meta)
    : executed;
  assert.equal(finalized.command_ok, true);
  await ctx.usageStore.flush();

  const queried = await ctx.usageStore.query({
    scope: 'current_runtime', tools: ['exec_command'], exclude_tools: [], include_records: true
  });
  assert.equal(queried.matched_lines, 1);
  assert.equal(queried.matched_async_session_events, 1);
  assert.equal(queried.performance.async_sessions_finalized, 1);
  assert.equal(queried.performance.child_process_terminations.exited, 1);
  assert.equal(queried.performance.command_kinds[0].child_sessions, 1);
  assert.ok(queried.performance.child_process_lifetime_ms >= 0);
});
