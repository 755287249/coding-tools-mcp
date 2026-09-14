import test from 'node:test';
import assert from 'node:assert/strict';
import { readFile } from 'node:fs/promises';
import path from 'node:path';
import { fileURLToPath } from 'node:url';
import { validateUiParity } from '../scripts/check-node-agent-ui-parity.mjs';

const root = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '..');

async function read(relative) {
  return readFile(path.join(root, relative), 'utf8');
}

test('Node Agent UI parity checklist is complete and executable', async () => {
  const result = await validateUiParity();
  assert.deepEqual(result.errors, []);
  const packageMetadata = JSON.parse(await read('packages/node-agent/package.json'));
  assert.equal(result.version, packageMetadata.version);
  assert.deepEqual(result.items, ['UI-001', 'UI-002', 'UI-003', 'UI-004', 'UI-005', 'UI-006', 'UI-007']);
  assert.equal(result.checks, 7);
});

test('telemetry and diagnostics keep sensitive runtime fields outside the response contract', async () => {
  const [observability, managementObservabilityRoute] = await Promise.all([
    read('packages/node-agent/src/managementObservability.ts'),
    read('packages/node-agent/src/management/routes/observability.ts')
  ]);
  const allowlist = observability.match(/const TELEMETRY_RECORD_FIELDS = \[([\s\S]*?)\] as const;/)?.[1];
  assert.ok(allowlist);
  for (const forbidden of ['command_preview', 'argument_record', 'resolved_cwd', 'session_id', 'runtime_boot_id', 'arguments_sha256']) {
    assert.doesNotMatch(allowlist, new RegExp(`['"]${forbidden}['"]`));
  }
  assert.match(observability, /content\.replace\(\/\^\\\*\\\*Session key:/);
  assert.doesNotMatch(observability, /fixedProbe\([^\n]*(?:publicBaseUrl|tunnel\.publicUrl)/);
  assert.match(managementObservabilityRoute, /localListenerBaseUrl\(req\)/);
  assert.doesNotMatch(managementObservabilityRoute, /managementHealthPayload\([^\n]*req\.headers\.host/);
  assert.match(observability, /canonicalPath\(folder\.path\)/);
  assert.match(observability, /HISTORY_PATH_OUTSIDE_WORKSPACE/);
  assert.match(observability, /validateManagementHealthPayload/);
  assert.match(observability, /mcpAuthenticationProbe/);
  assert.match(observability, /resource_metadata=/);
  assert.match(managementObservabilityRoute, /action === 'logs'/);
  assert.match(observability, /managementOperationLogPayload/);
  assert.match(observability, /harnessWorkspaceId\(folder\.path\)/);
  assert.match(observability, /redactSensitiveText/);
  assert.match(observability, /'\[WORKSPACE\]'/);
  const operationResponse = observability.match(/function safeOperationGroup[\s\S]*?return \{([\s\S]*?)\r?\n  \};\r?\n\}/)?.[1];
  assert.ok(operationResponse);
  for (const forbidden of ['workspace_id', 'task_id', 'input_summary', 'result_summary', 'affected_files']) {
    assert.doesNotMatch(operationResponse, new RegExp(`\\b${forbidden}\\s*:`));
  }
});

test('workspace UI retains Rust-aligned shared observability surfaces without desktop-only services', async () => {
  const [workspacePage, checklist, capabilities] = await Promise.all([
    read('src/routes/workspace/[id]/+page.svelte'),
    read('docs/todo/node-agent-ui-parity/CHECKLIST.md'),
    read('src/lib/backend/capabilities.ts')
  ]);
  assert.match(workspacePage, /type WorkspaceTab = "overview" \| "history" \| "telemetry" \| "logs" \| "health" \| "features" \| "mcp" \| "actions" \| "settings"/);
  for (const marker of ['HistoryViewer', 'TelemetryViewer', 'OperationLogViewer', 'HealthPanel']) {
    assert.match(workspacePage, new RegExp(marker));
  }
  assert.match(workspacePage, /role="tabpanel"/);
  assert.match(workspacePage, /capabilities\.operationLogs/);
  assert.match(workspacePage, /capabilities\.workspaceFeatureControls/);
  assert.match(workspacePage, /capabilities\.actions/);
  assert.match(capabilities, /agentRestart: true/);
  assert.match(capabilities, /openNativePath: true/);
  assert.match(capabilities, /workspaceLifecycle: true/);
  assert.match(capabilities, /workspaceFeatureControls: true/);
  assert.match(capabilities, /rawRuntimeLogs: false/);
  assert.match(checklist, /Actions\/OpenAPI/);
  assert.match(checklist, /Live history-session running\/active\/inactive badges/);
  assert.match(checklist, /Raw Desktop per-service stdout\/stderr/);
  assert.match(checklist, /## UI-007 — Structured operation log browser/);
  assert.doesNotMatch(checklist, /- \[ \]/);
});

test('observability views retain complete filters, pagination, and host-scoped API access', async () => {
  const [telemetry, operationLog, history, nodeBackend] = await Promise.all([
    read('src/lib/components/TelemetryViewer.svelte'),
    read('src/lib/components/OperationLogViewer.svelte'),
    read('src/lib/components/HistoryViewer.svelte'),
    read('src/lib/backend/node.ts')
  ]);
  assert.match(telemetry, /readWorkspaceTelemetry/);
  assert.match(operationLog, /operations\.query/);
  assert.match(operationLog, /nextCursor/);
  assert.match(operationLog, /errorsOnly/);
  assert.match(operationLog, /Load older/);
  assert.match(history, /listHistorySessions/);
  assert.match(history, /readHistorySession/);
  assert.match(nodeBackend, /x-ctmcp-admin-token/);
  assert.match(nodeBackend, /credentials: "same-origin"/);
});

test('operation-log integration omits raw payloads and keeps Rust and Node debugging summaries synchronized', async () => {
  const [managementTest, harnessTest, taskTools, operationSummary, processes, processHarnessTracking, rustDispatch, rustDispatchTracking, rustSession, rustSessionAttachment, rustSessionLifecycle] = await Promise.all([
    read('packages/node-agent/test/management.test.mjs'),
    read('packages/node-agent/test/harnessBaseline.test.mjs'),
    read('packages/node-agent/src/taskTools.ts'),
    read('packages/node-agent/src/operationSummary.ts'),
    read('packages/node-agent/src/processes.ts'),
    read('packages/node-agent/src/processes/harnessTracking.ts'),
    read('src-tauri/src/tools/dispatch.rs'),
    read('src-tauri/src/tools/dispatch/tracking.rs'),
    read('src-tauri/src/tools/session.rs'),
    read('src-tauri/src/tools/session/attachment.rs'),
    read('src-tauri/src/tools/session/lifecycle.rs')
  ]);
  for (const marker of [
    'completed-operation-id', 'failed-operation-id', 'incomplete-operation-id',
    'OP_LOG_REASON_MARKER', 'OP_LOG_COMMAND_MARKER', 'OP_LOG_MULTILINE_TAIL',
    'OPERATION_COMMAND_SECRET', 'COMMAND_FAILED', 'operationWorkspaceId',
    'durationMs', 'affectedFileCount', 'diagnostics', 'process_exit_code', 'warning_count', 'nextCursor'
  ]) assert.match(managementTest, new RegExp(marker));
  assert.match(managementTest, /status=failed/);
  assert.match(managementTest, /status=unknown/);
  assert.match(harnessTest, /operation logs persist bounded execution diagnostics without raw process payloads/);
  assert.match(harnessTest, /OPERATION_OUTPUT_MUST_NOT_PERSIST/);
  for (const marker of ['command_ok', 'verification_ok', 'process_exit_code', 'warning_count']) {
    assert.match(operationSummary, new RegExp(marker));
    assert.match(rustDispatchTracking, new RegExp(marker));
  }
  assert.match(operationSummary, /function operationResultSummary/);
  assert.match(rustDispatchTracking, /fn operation_result_summary/);
  assert.match(rustSessionAttachment, /attach_harness_operation/);
  assert.match(rustSessionLifecycle, /record_harness_operation_finalization/);
  assert.match(rustSession, /harness_operation_recorded/);
  assert.match(rustDispatchTracking, /deferred_process_operation/);
  assert.match(rustDispatchTracking, /session\.attach_harness_operation/);
  assert.match(rustDispatch, /vec!\["started", "failed"\]/);
  assert.doesNotMatch(operationSummary.match(/function operationResultSummary[\s\S]*?\n\}/)?.[0] ?? '', /\.\.\.result/);
  assert.doesNotMatch(rustDispatchTracking.match(/fn operation_result_summary[\s\S]*?\n\}/)?.[0] ?? '', /\.extend\(/);
  for (const marker of ['attachHarnessOperation', 'deferredProcessOperation', 'result.command_ok === null']) {
    assert.match(taskTools, new RegExp(marker.replace(/[.*+?^${}()|[\]\\]/g, '\\$&')));
  }
  for (const marker of ['recordHarnessOperationFinalization', 'harnessOperations', 'harnessOperationRecordedIds']) {
    assert.match(processHarnessTracking, new RegExp(marker));
  }
  assert.match(processes, /await recordHarnessOperationFinalization\(ctx, session\)/);
  assert.match(harnessTest, /yield_time_ms: 0/);
  assert.match(harnessTest, /\['failed', 'started'\]/);
  assert.match(managementTest, /status: 'running'/);
});

test('all settings entry points preserve fine-grained policy and global limits', async () => {
  const [form, nodeMap, testSource] = await Promise.all([
    read('src/lib/components/RuntimePolicyForm.svelte'),
    read('src/lib/backend/node-map.ts'),
    read('packages/node-agent/test/management.test.mjs')
  ]);
  for (const marker of ['allowedCommands', 'workspaceLocalEntries', 'workspaceScriptExtensions']) {
    assert.match(form, new RegExp(marker));
  }
  for (const marker of ['globalBlockingConcurrency', 'globalProcessConcurrency', 'maxPatchBytes']) {
    assert.match(testSource, new RegExp(marker));
  }
  assert.match(nodeMap, /allowedCommands: commands/);
  assert.match(nodeMap, /limits:/);
});

test('Node settings UI exposes sandbox configuration and preserves it in guided setup', async () => {
  const [types, form, nodeMap, configStore] = await Promise.all([
    read('src/lib/types.ts'),
    read('src/lib/components/workspace/SandboxSettings.svelte'),
    read('src/lib/backend/node-map.ts'),
    read('packages/node-agent/src/management/configStore.ts')
  ]);
  for (const marker of ['export interface SandboxConfig', 'export interface SandboxBackendDescriptor', 'sandbox\\?: SandboxConfig']) {
    assert.match(types, new RegExp(marker));
  }
  for (const marker of ['Enable command sandbox', 'Sandbox backend', 'Read-only external paths', 'Writable external paths']) {
    assert.match(form, new RegExp(marker));
  }
  assert.match(nodeMap, /toNodeSandbox/);
  assert.match(configStore, /sandboxBackends: sandboxBackends\(\)/);
});
