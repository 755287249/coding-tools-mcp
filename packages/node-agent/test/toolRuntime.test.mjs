import test from 'node:test';
import assert from 'node:assert/strict';
import { toolNames } from '../dist/catalog.js';
import {
  canCoalesceToolCall,
  canonicalToolCall,
  registeredToolRuntimeNames,
  requestMutates,
  toolRuntimeFor,
  toolUsageFamily
} from '../dist/toolRuntime.js';

test('runtime registry covers every advanced catalog tool exactly once', () => {
  assert.deepEqual(
    [...registeredToolRuntimeNames()].sort(),
    [...toolNames].sort()
  );
  assert.equal(new Set(registeredToolRuntimeNames()).size, toolNames.length);
});

test('runtime registry preserves execution lanes, locks, harness, and permissions', () => {
  assert.equal(toolRuntimeFor('exec_command').lane, 'process');
  assert.equal(toolRuntimeFor('exec_command').admission, 'request');
  assert.equal(toolRuntimeFor('exec_many').admission, 'children');
  assert.equal(toolRuntimeFor('wait_command').lane, 'control');
  assert.equal(toolRuntimeFor('wait_command').admission, 'none');
  assert.equal(toolRuntimeFor('read_file').lane, 'blocking');
  assert.deepEqual(toolRuntimeFor('apply_patch').lockGroups, ['workspace_content']);
  assert.deepEqual(toolRuntimeFor('git_restore').lockGroups, ['git', 'workspace_content']);
  assert.deepEqual(toolRuntimeFor('start_task').lockGroups, ['task']);
  assert.equal(toolRuntimeFor('project_state').harnessTool, true);
  assert.equal(toolRuntimeFor('read_file').harnessTool, false);
  assert.equal(toolRuntimeFor('apply_patch').guardedPermission, 'workspace_mutation');
  assert.equal(toolRuntimeFor('format_files').guardedPermission, 'privileged_operation');
  assert.equal(toolRuntimeFor('exec_many').guardedPermission, 'process_execution');
  assert.equal(toolRuntimeFor('git_push').guardedPermission, 'network');
  assert.equal(toolRuntimeFor('read_file').workspaceSelector, true);
  assert.equal(toolRuntimeFor('git_commit').workspaceSelector, true);
  assert.equal(toolRuntimeFor('exec_command').workspaceSelector, true);
  assert.equal(toolRuntimeFor('history_session_checkpoint').workspaceSelector, false);
});

test('runtime registry preserves inflight coalescing policy', () => {
  assert.equal(canCoalesceToolCall('read_file', { path: 'README.md' }), true);
  assert.equal(canCoalesceToolCall('conversation_bootstrap', {}), false);
  assert.equal(canCoalesceToolCall('switch_workspace_folder', { folder_id: 'repo' }), false);
  assert.equal(canCoalesceToolCall('request_permissions', { resume_id: 'resume' }), false);
  assert.equal(canCoalesceToolCall('exec_command', { operation_id: 'stable' }), true);
  assert.equal(canCoalesceToolCall('exec_command', {}), false);
  assert.equal(canCoalesceToolCall('exec_many', { operation_id: 'stable' }), false);
});

test('runtime registry preserves aliases and telemetry classification', () => {
  assert.deepEqual(canonicalToolCall('edit_many', { files: [] }), {
    name: 'edit',
    args: { files: [] }
  });
  assert.deepEqual(canonicalToolCall('edit_file', {
    path: 'README.md',
    expected_sha256: 'abc',
    edits: [],
    dry_run: true,
    reason: 'test',
    ignored: true
  }), {
    name: 'edit',
    args: {
      files: [{ path: 'README.md', expected_sha256: 'abc', edits: [] }],
      dry_run: true,
      reason: 'test'
    }
  });

  assert.equal(toolUsageFamily('edit_file'), 'filesystem');
  assert.equal(toolUsageFamily('exec_health_check'), 'other');
  assert.equal(toolUsageFamily('patch_check'), 'other');
  assert.equal(toolUsageFamily('request_permissions'), 'runtime');
  assert.equal(requestMutates('edit_file', {}), true);
  assert.equal(requestMutates('format_files', { mode: 'check' }), false);
  assert.equal(requestMutates('format_files', { mode: 'apply' }), true);
  assert.equal(requestMutates('git_push', {}), false);
});

test('unknown requests retain neutral runtime defaults for catalog errors', () => {
  assert.deepEqual(toolRuntimeFor('missing_tool'), {
    name: 'missing_tool',
    canonicalName: 'missing_tool',
    domain: 'runtime',
    usageFamily: 'other',
    lane: 'blocking',
    admission: 'request',
    lockGroups: [],
    harnessTool: false,
    coalescing: 'never',
    mutation: 'never',
    workspaceSelector: false
  });
});
