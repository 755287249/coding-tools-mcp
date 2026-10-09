import test from 'node:test';
import assert from 'node:assert/strict';
import { readFile, readdir } from 'node:fs/promises';
import path from 'node:path';
import { fileURLToPath } from 'node:url';

const root = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '..');

async function read(relative) {
  return readFile(path.join(root, relative), 'utf8');
}

function escapePattern(value) {
  return value.replace(/[.*+?^${}()|[\]\\]/g, '\\$&');
}

test('every Rust catalog tool has an explicit Node regression reference', async () => {
  const catalog = await read('packages/node-agent/src/rustCatalog.generated.ts');
  const toolNames = [...catalog.matchAll(/\n\s+"name": "([^"]+)"/g)].map(match => match[1]);
  const directory = path.join(root, 'packages', 'node-agent', 'test');
  const testFiles = (await readdir(directory)).filter(name => name.endsWith('.test.mjs'));
  const testSource = (await Promise.all(testFiles.map(name => readFile(path.join(directory, name), 'utf8')))).join('\n');
  const missing = toolNames.filter(name => !new RegExp(`\\b${escapePattern(name)}\\b`).test(testSource));

  assert.equal(toolNames.length, 71);
  assert.deepEqual(missing, []);
});

test('bounded harness, persisted change selection, and exec-health contracts stay synchronized across Rust and Node', async () => {
  const [nodeProcessDispatcher, nodeHarness, rustHarness, rustState, nodeHealthTests, nodeHarnessTests, rustHarnessTests] = await Promise.all([
    read('packages/node-agent/src/toolDispatchers/process.ts'),
    read('packages/node-agent/src/taskTools.ts'),
    read('src-tauri/src/harness/tools.rs'),
    read('src-tauri/src/harness/state.rs'),
    read('packages/node-agent/test/processLifecycle.test.mjs'),
    read('packages/node-agent/test/harnessBaseline.test.mjs'),
    read('src-tauri/tests/harness_tool_contract.rs')
  ]);

  for (const marker of ['session_create', 'command_run', 'stdout_capture', 'stderr_capture', 'exec-health-stderr']) {
    assert.match(nodeProcessDispatcher, new RegExp(marker));
  }
  assert.match(nodeHealthTests, /exec_health_check matches the Rust worker, session, and stream-capture contract/);

  assert.match(nodeHarness, /args\.max_bytes/);
  assert.match(nodeHarness, /Buffer\.byteLength\(JSON\.stringify\(result\)\)/);
  assert.match(rustHarness, /args\s*\.get\("max_bytes"\)/);
  assert.match(rustHarness, /serde_json::to_vec\(&tool_ok\(value\.clone\(\)\)\)/);
  assert.match(nodeHarnessTests, /task_context enforces the public max_bytes budget/);
  assert.match(rustHarnessTests, /task_context遵守max_bytes预算/);

  assert.match(nodeHarness, /args\.summary/);
  assert.match(nodeHarness, /args\.change_id/);
  assert.match(nodeHarness, /latest_change_id/);
  assert.match(nodeHarness, /setTaskAndChange/);
  assert.match(rustHarness, /args\.get\("summary"\)/);
  assert.match(rustHarness, /optional_change_id\(args\)/);
  assert.match(rustHarness, /latest_change_id/);
  assert.match(rustState, /save_change/);
  assert.match(nodeHarnessTests, /finish_task summary persists an immutable change selected by change_id/);
  assert.match(rustHarnessTests, /finish_task摘要持久化且change_id选择不可变快照/);

  const nodeClean = nodeHarness.indexOf("const clean = allFiles.every(file => file.status === 'unchanged')");
  const nodeSlice = nodeHarness.indexOf('const files = allFiles.slice(0, maxFiles)');
  assert.ok(nodeClean >= 0 && nodeClean < nodeSlice);
  const rustClean = rustState.indexOf('let clean = files.iter().all(|file| file.status == "unchanged")');
  const rustTruncate = rustState.indexOf('let files = files.into_iter().take(max_files.max(1))');
  assert.ok(rustClean >= 0 && rustClean < rustTruncate);
  assert.match(nodeHarnessTests, /project_state computes clean from the complete file set before max_files truncation/);
});

test('Rust and Node telemetry share the recoverable outcome taxonomy', async () => {
  const [rustTelemetry, nodeTelemetry] = await Promise.all([
    read('src-tauri/src/mcp/telemetry.rs'),
    read('packages/node-agent/src/toolUsage.ts')
  ]);
  for (const outcome of [
    'target_resolution_error',
    'state_conflict',
    'routing_error',
    'caller_argument_error',
    'policy_rejection',
    'internal_error'
  ]) {
    assert.match(rustTelemetry, new RegExp(outcome));
    assert.match(nodeTelemetry, new RegExp(outcome));
  }
  assert.doesNotMatch(rustTelemetry, /"tool_internal_error"/);
  assert.doesNotMatch(nodeTelemetry, /return 'tool_internal_error'/);
});


test('Rust and Node edits share replay-plan and phase-latency contracts', async () => {
  const [rustPatchSupport, rustPatchEdit, rustUsage, nodeFiles, rustTelemetry, nodeTelemetry, nodeObservability] = await Promise.all([
    read('src-tauri/src/tools/patch/support.rs'),
    read('src-tauri/src/tools/patch/edit_ops.rs'),
    read('src-tauri/src/tools/tool_usage.rs'),
    read('packages/node-agent/src/fileTools.ts'),
    read('src-tauri/src/mcp/telemetry.rs'),
    read('packages/node-agent/src/toolUsage.ts'),
    read('packages/node-agent/src/managementObservability.ts')
  ]);
  const rustPatch = `${rustPatchSupport}\n${rustPatchEdit}`;
  for (const marker of [
    'schema_version',
    'plan_sha256',
    'stateful_dependencies',
    'expected_result',
    'phase_durations_ms',
    'preflight_ms',
    'plan_ms',
    'commit_ms',
    'total_ms'
  ]) {
    assert.match(rustPatch, new RegExp(marker));
    assert.match(nodeFiles, new RegExp(marker));
  }
  assert.match(nodeTelemetry, /record\[`phase_\$\{source\}`\]/);
  for (const field of ['phase_preflight_ms', 'phase_plan_ms', 'phase_commit_ms', 'phase_total_ms']) {
    assert.match(rustTelemetry, new RegExp(field));
    assert.match(nodeObservability, new RegExp(field));
  }
  for (const marker of ['phase_latency', 'samples', 'total_ms', 'avg_ms', 'p50_ms', 'p95_ms', 'max_ms']) {
    assert.match(rustUsage, new RegExp(marker));
    assert.match(nodeTelemetry, new RegExp(marker));
  }
  for (const marker of ['repeated_failures', 'wasted_duration_ms', 'max_attempt_count', 'friction_score', 'deterministic_error_weight', 'legacy_adjacent_retry_count', 'Stop retrying unchanged arguments']) {
    assert.match(rustUsage, new RegExp(marker));
    assert.match(nodeTelemetry, new RegExp(marker));
  }
  for (const marker of [
    'tool-failure-v1',
    'failure_signature',
    'repeat_failure_count',
    'repeated_failure',
    'retry_without_change'
  ]) {
    assert.match(rustTelemetry, new RegExp(marker));
    assert.match(nodeTelemetry, new RegExp(marker));
  }
  for (const field of ['failure_signature', 'repeat_failure_count', 'repeated_failure', 'retry_without_change']) {
    assert.match(nodeObservability, new RegExp(field));
  }
});
