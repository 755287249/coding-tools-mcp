import test from 'node:test';
import assert from 'node:assert/strict';
import { mkdir, mkdtemp, readFile, realpath, rm, symlink, writeFile } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import path from 'node:path';
import {
  ConversationStore, markMcpConversationMetadata, MAX_CONVERSATION_CONTEXTS
} from '../dist/conversation.js';
import { createToolContext } from '../dist/server.js';
import { callTool } from '../dist/tools.js';
import { sameWorkspacePath, workspacePathIdentity } from '../dist/wsl.js';

test('conversation context depends on a pure contract instead of the store implementation', async () => {
  const [typesSource, conversationSource, contractSource] = await Promise.all([
    readFile(new URL('../src/types.ts', import.meta.url), 'utf8'),
    readFile(new URL('../src/conversation.ts', import.meta.url), 'utf8'),
    readFile(new URL('../src/conversation/contract.ts', import.meta.url), 'utf8')
  ]);
  assert.match(typesSource, /from ['"]\.\/conversation\/contract\.js['"]/);
  assert.doesNotMatch(typesSource, /from ['"]\.\/conversation\.js['"]/);
  assert.match(conversationSource, /implements ConversationStoreContract/);
  assert.match(conversationSource, /export type \{[^}]*ConversationIdentity[^}]*MutableStringMap[^}]*\} from ['"]\.\/conversation\/contract\.js['"]/s);
  assert.doesNotMatch(contractSource, /from\s+['"]/);
});

function config(folders, dataDir) {
  return {
    host: '127.0.0.1',
    port: 0,
    dataDir,
    permissionMode: 'trusted',
    oauth: {
      clientId: 'chatgpt',
      password: 'workspace-conversation-test',
      tokenSecret: 'workspace-conversation-test-token'
    },
    folders,
    limits: {
      blockingConcurrency: 4,
      processConcurrency: 4,
      globalBlockingConcurrency: 8,
      globalProcessConcurrency: 8,
      activeSessionLimit: 16,
      maxOutputBytes: 1024 * 1024
    }
  };
}

async function fixture(t) {
  const base = await mkdtemp(path.join(tmpdir(), 'ctmcp-conversation-'));
  const first = path.join(base, 'first');
  const second = path.join(base, 'second');
  const dataDir = path.join(base, 'data');
  await Promise.all([
    mkdir(path.join(first, 'src'), { recursive: true }),
    mkdir(path.join(second, 'packages'), { recursive: true }),
    mkdir(dataDir, { recursive: true })
  ]);
  await Promise.all([
    writeFile(path.join(first, 'marker.txt'), 'first workspace', 'utf8'),
    writeFile(path.join(second, 'marker.txt'), 'second workspace', 'utf8'),
    writeFile(path.join(second, 'alternate-only.txt'), 'alternate workspace only', 'utf8')
  ]);
  const folders = [
    { id: 'first', name: 'First', path: first },
    { id: 'second', name: 'Second', path: second }
  ];
  const ctx = await createToolContext(config(folders, dataDir));
  t.after(async () => {
    await ctx.conversations.flush();
    await ctx.usageStore.flush();
    await rm(base, { recursive: true, force: true, maxRetries: 5, retryDelay: 50 });
  });
  return { base, first: await realpath(first), second: await realpath(second), dataDir, folders, ctx };
}

test('workspace listing and switching expose conversation-isolated Rust metadata', async t => {
  const state = await fixture(t);
  const unscoped = await callTool(state.ctx, 'list_workspace_folders', {}, undefined);
  assert.equal(unscoped.ok, true);
  assert.equal(unscoped.multi_folder, true);
  assert.equal(unscoped.selected_folder_id, null);
  assert.equal(unscoped.selection_scope, 'unselected');
  assert.equal(unscoped.conversation_isolated, false);
  assert.equal(unscoped.default_cwd, null);
  assert.match(unscoped.profile_id, /^node-[0-9a-f]{16}$/);
  assert.deepEqual(unscoped.folders.map(folder => folder.selected), [false, false]);
  assert.ok(unscoped.folders.every(folder => folder.history_dir.endsWith(path.join('docs', 'history-session'))));

  const legacy = await callTool(state.ctx, 'switch_workspace_folder', { folder_id: 'first' }, undefined);
  assert.equal(legacy.ok, true);
  assert.equal(legacy.selection_scope, 'runtime');
  assert.equal(legacy.conversation_isolated, false);
  assert.equal(legacy.conversation_source, 'stable_fallback');

  const missingMcpMeta = markMcpConversationMetadata(undefined);
  const missingListing = await callTool(state.ctx, 'list_workspace_folders', {}, missingMcpMeta);
  assert.equal(missingListing.selected_folder_id, null);
  assert.equal(missingListing.selection_scope, 'unselected');
  assert.equal(missingListing.conversation_source, 'missing_mcp_conversation');
  const denied = await callTool(state.ctx, 'switch_workspace_folder', { folder_id: 'first' }, missingMcpMeta);
  assert.equal(denied.ok, false);
  assert.equal(denied.error.code, 'WORKSPACE_FOLDER_NOT_SELECTED');
  assert.equal(denied.error.category, 'workspace_routing');
  assert.equal(state.ctx.selections.size, 1);

  const isolatedRuntime = await createToolContext(config(state.folders, path.join(state.base, 'isolated-runtime')));
  assert.notEqual(isolatedRuntime.conversations.fallbackKey, state.ctx.conversations.fallbackKey);
  const isolatedListing = await callTool(isolatedRuntime, 'list_workspace_folders', {}, undefined);
  assert.equal(isolatedListing.selected_folder_id, null);
  await isolatedRuntime.conversations.flush();
  await isolatedRuntime.usageStore.flush();

  const metaA = { 'openai/session': 'conversation-a' };
  const metaB = { 'openai/session': 'conversation-b' };
  const selectedA = await callTool(state.ctx, 'switch_workspace_folder', { folder_id: 'first' }, metaA);
  const selectedB = await callTool(state.ctx, 'switch_workspace_folder', { folder_id: 'second' }, metaB);
  assert.equal(selectedA.selection_scope, 'conversation');
  assert.equal(selectedA.conversation_isolated, true);
  assert.equal(selectedA.default_cwd, '.');
  assert.equal(selectedA.profile_id, unscoped.profile_id);
  assert.equal(selectedA.history_dir, path.join(state.first, 'docs', 'history-session'));
  assert.equal(selectedB.selected_folder_id, 'second');

  const listedA = await callTool(state.ctx, 'list_workspace_folders', {}, metaA);
  const listedB = await callTool(state.ctx, 'list_workspace_folders', {}, metaB);
  assert.equal(listedA.selected_folder_id, 'first');
  assert.equal(listedB.selected_folder_id, 'second');
  assert.deepEqual(listedA.folders.map(folder => folder.selected), [true, false]);
  assert.deepEqual(listedB.folders.map(folder => folder.selected), [false, true]);
});

test('workspace_folder_id routes one call without changing conversation selection', async t => {
  const state = await fixture(t);
  const meta = { 'openai/session': 'one-call-routing' };

  const unselectedRead = await callTool(state.ctx, 'read_file', {
    path: 'marker.txt',
    workspace_folder_id: 'second'
  }, meta);
  assert.equal(unselectedRead.ok, true, JSON.stringify(unselectedRead));
  assert.equal(unselectedRead.content, 'second workspace');
  assert.equal(unselectedRead.requested_workspace_id, 'second');
  assert.equal(unselectedRead.resolved_workspace_id, 'second');
  assert.equal(unselectedRead.workspace_route_source, 'explicit');
  assert.equal(unselectedRead.workspace_route_changed, true);
  assert.equal(unselectedRead.conversation_selection_changed, false);

  const stillUnselected = await callTool(state.ctx, 'list_workspace_folders', {}, meta);
  assert.equal(stillUnselected.selected_folder_id, null);
  assert.deepEqual(stillUnselected.folders.map(folder => folder.selected), [false, false]);

  await callTool(state.ctx, 'switch_workspace_folder', { folder_id: 'first' }, meta);
  const crossRead = await callTool(state.ctx, 'read_file', {
    path: 'marker.txt',
    workspace_folder_id: 'second'
  }, meta);
  assert.equal(crossRead.content, 'second workspace');
  assert.equal(crossRead.workspace_route_changed, true);

  const selected = await callTool(state.ctx, 'list_workspace_folders', {}, meta);
  assert.equal(selected.selected_folder_id, 'first');
  assert.deepEqual(selected.folders.map(folder => folder.selected), [true, false]);
});

test('NOT_FOUND returns a complete one-call retry for a unique alternate workspace match', async t => {
  const state = await fixture(t);
  const meta = { 'openai/session': 'alternate-workspace-recovery' };
  await callTool(state.ctx, 'switch_workspace_folder', { folder_id: 'first' }, meta);

  const missing = await callTool(state.ctx, 'read_file', { path: 'alternate-only.txt' }, meta);
  assert.equal(missing.ok, false, JSON.stringify(missing));
  assert.equal(missing.error.code, 'NOT_FOUND');
  assert.equal(missing.error.retryable, true);
  assert.equal(missing.error.details.alternate_workspace_match_count, 1);
  assert.deepEqual(missing.error.details.alternate_workspace_matches, [
    { folder_id: 'second', folder_name: 'Second' }
  ]);
  assert.deepEqual(missing.error.details.recovery_actions, [{
    action: 'retry_in_workspace',
    action_id: 'workspace-retry-second',
    tool: 'read_file',
    required_arguments: [],
    arguments: { path: 'alternate-only.txt', workspace_folder_id: 'second' },
    reason: 'alternate_workspace_match'
  }]);

  const recovered = await callTool(
    state.ctx,
    missing.error.details.recovery_actions[0].tool,
    missing.error.details.recovery_actions[0].arguments,
    meta
  );
  assert.equal(recovered.ok, true, JSON.stringify(recovered));
  assert.equal(recovered.content, 'alternate workspace only');
  assert.equal(recovered.workspace_route_source, 'explicit');
  const selectionAfterRecovery = await callTool(state.ctx, 'list_workspace_folders', {}, meta);
  assert.equal(selectionAfterRecovery.selected_folder_id, 'first');
});

test('recovery metadata is stripped from execution and yields stable failure ids', async t => {
  const state = await fixture(t);
  const meta = { 'openai/session': 'recovery-routing' };
  await callTool(state.ctx, 'switch_workspace_folder', { folder_id: 'first' }, meta);

  const first = await callTool(state.ctx, 'read_file', {
    path: 'missing.txt',
    retry_of_call_sequence: 41,
    recovery_of_operation_id: 'operation-41',
    recovery_action_id: 'read_current_file'
  }, meta);
  assert.equal(first.ok, false, JSON.stringify(first));
  assert.match(first.failure_id, /^[0-9a-f]{64}$/);
  assert.equal(first.retry_of_call_sequence, 41);
  assert.match(first.recovery_of_operation_id_hash, /^[0-9a-f]{64}$/);
  assert.equal(first.recovery_action_id, 'read_current_file');
  assert.equal(first.recovery_attempt, true);
  assert.equal(first.recovery_succeeded, false);

  const second = await callTool(state.ctx, 'read_file', {
    path: 'missing.txt',
    retry_of_call_sequence: 42,
    recovery_action_id: 'refresh_target'
  }, meta);
  assert.equal(second.failure_id, first.failure_id);

  const recovered = await callTool(state.ctx, 'read_file', {
    path: 'marker.txt',
    retry_of_call_sequence: 41,
    recovery_of_operation_id: 'operation-41',
    recovery_action_id: 'read_current_file'
  }, meta);
  assert.equal(recovered.ok, true, JSON.stringify(recovered));
  assert.equal(recovered.recovery_attempt, true);
  assert.equal(recovered.recovery_succeeded, true);
  assert.equal(recovered.failure_id, undefined);
});

test('process control recovers the workspace of an explicitly routed session', async t => {
  const state = await fixture(t);
  const meta = { 'openai/session': 'one-call-process-routing' };
  await callTool(state.ctx, 'switch_workspace_folder', { folder_id: 'first' }, meta);

  const started = await callTool(state.ctx, 'exec_command', {
    program: path.basename(process.execPath),
    args: ['-e', 'setTimeout(() => process.stdout.write("done"), 100)'],
    yield_time_ms: 0,
    timeout_ms: 10_000,
    workspace_folder_id: 'second'
  }, meta);
  assert.equal(started.ok, true, JSON.stringify(started));
  assert.equal(started.resolved_workspace_id, 'second');
  assert.equal(started.workspace_route_source, 'explicit');
  assert.equal(started.process_still_running, true);

  const waited = await callTool(state.ctx, 'wait_command', {
    session_id: started.session_id,
    cursor: started.next_cursor,
    timeout_ms: 10_000,
    until: 'finalized',
    output_mode: 'tail'
  }, meta);
  assert.equal(waited.ok, true, JSON.stringify(waited));
  assert.equal(waited.resolved_workspace_id, 'second');
  assert.equal(waited.workspace_route_source, 'session_id');
  assert.equal(waited.workspace_route_changed, true);
  assert.equal(waited.conversation_selection_changed, false);

  const selected = await callTool(state.ctx, 'list_workspace_folders', {}, meta);
  assert.equal(selected.selected_folder_id, 'first');
});

test('conversation_bootstrap returns choices when ambiguous then binds and bootstraps in one call', async t => {
  const state = await fixture(t);
  const meta = { 'openai/session': 'bootstrap-conversation' };

  const ambiguous = await callTool(state.ctx, 'conversation_bootstrap', {}, meta);
  assert.equal(ambiguous.ok, true);
  assert.equal(ambiguous.needs_folder_selection, true);
  assert.equal(ambiguous.selected_folder_id, null);
  assert.equal(ambiguous.next_action.tool, 'conversation_bootstrap');
  assert.deepEqual(ambiguous.folders.map(folder => folder.selected), [false, false]);

  const bootstrapped = await callTool(state.ctx, 'conversation_bootstrap', { folder_id: 'first' }, meta);
  assert.equal(bootstrapped.ok, true);
  assert.equal(bootstrapped.selected_folder_id, 'first');
  assert.equal(bootstrapped.needs_folder_selection, false);
  assert.equal(bootstrapped.response_mode, 'compact');
  assert.equal(bootstrapped.startup_flow, 'workspace_and_history_bootstrapped');
  assert.equal(bootstrapped.learning.runtime_policy, 'adaptive_rollout_phase_2e');
  assert.equal(bootstrapped.learning.ingestion.failed, 0);
  assert.equal(bootstrapped.learning.ingestion.dropped, 0);
  assert.equal(bootstrapped.learning.shadow_impact_count, 0);
  assert.equal(bootstrapped.learning.canary_impact_count, 0);
  assert.equal(bootstrapped.learning.canary_promoted_count, 0);
  assert.equal(bootstrapped.learning.canary_rolled_back_count, 0);
  assert.equal(bootstrapped.learning.validated_tool_strategy_count, 0);
  assert.equal(bootstrapped.learning.promoted_tool_strategy_count, 0);
  assert.equal(bootstrapped.learning.validated_tool_evolution_count, 0);
  assert.match(bootstrapped.learning.shadow_impact_revision, /^[a-f0-9]{64}$/);
  assert.match(bootstrapped.learning.canary_impact_revision, /^[a-f0-9]{64}$/);
  assert.match(bootstrapped.learning.knowledge_revision, /^[a-f0-9]{64}$/);
  assert.match(bootstrapped.learning.tool_strategy_implementation_revision, /^[a-f0-9]{64}$/);
  assert.equal(bootstrapped.learning.tool_strategy_implementation_count, 1);
  assert.equal(bootstrapped.learning.tool_strategy_count, 0);
  assert.equal(bootstrapped.learning.tool_evolution_candidate_count, 0);
  assert.equal(bootstrapped.learning.promoted_evolved_skill_count, 0);
  assert.equal('records' in bootstrapped.learning, false);
  assert.equal('candidates' in bootstrapped.learning, false);
  assert.equal(bootstrapped.current_path.replaceAll('\\', '/'), 'docs/history-session/1.md');

  const resumed = await callTool(state.ctx, 'conversation_bootstrap', {}, meta);
  assert.equal(resumed.selected_folder_id, 'first');
  assert.equal(resumed.current_path, bootstrapped.current_path);
  assert.equal(resumed.resumed, true);
});

test('conversation_bootstrap keeps workspace usable when history initialization fails', async t => {
  const state = await fixture(t);
  const meta = { 'openai/session': 'bootstrap-history-degraded' };

  const degraded = await callTool(state.ctx, 'conversation_bootstrap', {
    folder_id: 'first',
    history_dir: 'marker.txt'
  }, meta);
  assert.equal(degraded.ok, true, JSON.stringify(degraded));
  assert.equal(degraded.selected_folder_id, 'first');
  assert.equal(degraded.needs_folder_selection, false);
  assert.equal(degraded.startup_flow, 'workspace_bootstrapped_history_degraded');
  assert.equal(degraded.history_bootstrap_ok, false);
  assert.equal(degraded.history_bootstrap_error.code, 'NOT_A_DIRECTORY');
  assert.equal(degraded.recovery_actions[0].tool, 'history_session_bootstrap');

  const readable = await callTool(state.ctx, 'read_file', { path: 'marker.txt' }, meta);
  assert.equal(readable.ok, true);
  assert.equal(readable.content, 'first workspace');

  const directHistory = await callTool(state.ctx, 'history_session_bootstrap', { history_dir: 'marker.txt' }, meta);
  assert.equal(directHistory.ok, false);
  assert.equal(directHistory.error.code, 'NOT_A_DIRECTORY');
});

test('switching back to a folder restores that conversation folder cwd', async t => {
  const state = await fixture(t);
  const meta = { 'openai/session': 'cwd-conversation' };

  await callTool(state.ctx, 'switch_workspace_folder', { folder_id: 'first' }, meta);
  const firstCwd = await callTool(state.ctx, 'set_default_cwd', { path: 'src' }, meta);
  assert.equal(firstCwd.default_cwd, 'src');

  await callTool(state.ctx, 'switch_workspace_folder', { folder_id: 'second' }, meta);
  const secondCwd = await callTool(state.ctx, 'set_default_cwd', { path: 'packages' }, meta);
  assert.equal(secondCwd.default_cwd, 'packages');

  const restoredFirst = await callTool(state.ctx, 'switch_workspace_folder', { folder_id: 'first' }, meta);
  assert.equal(restoredFirst.default_cwd, 'src');
  assert.equal(restoredFirst.resolved_cwd, path.join(state.first, 'src'));

  const listing = await callTool(state.ctx, 'list_workspace_folders', {}, meta);
  assert.equal(listing.default_cwd, 'src');
  assert.equal(listing.folders.find(folder => folder.id === 'first').default_cwd, 'src');
  assert.equal(listing.folders.find(folder => folder.id === 'second').default_cwd, 'packages');
});

test('relative workspace tools honor default cwd and stale cwd falls back to workspace root', async t => {
  const state = await fixture(t);
  const meta = { 'openai/session': 'cwd-relative-tools' };
  await writeFile(path.join(state.first, 'src', 'marker.txt'), 'nested marker', 'utf8');

  await callTool(state.ctx, 'switch_workspace_folder', { folder_id: 'first' }, meta);
  const selected = await callTool(state.ctx, 'set_default_cwd', { path: 'src' }, meta);
  assert.equal(selected.ok, true);

  const read = await callTool(state.ctx, 'read_file', { path: 'marker.txt' }, meta);
  assert.equal(read.ok, true, JSON.stringify(read));
  assert.equal(read.content, 'nested marker');
  assert.equal(read.path.replaceAll('\\', '/'), 'src/marker.txt');

  const search = await callTool(state.ctx, 'search_text', { path: '.', query: 'nested marker' }, meta);
  assert.equal(search.ok, true);
  assert.equal(search.returned_count, 1);
  assert.equal(search.matches[0].path.replaceAll('\\', '/'), 'src/marker.txt');

  await rm(path.join(state.first, 'src'), { recursive: true, force: true });
  const listing = await callTool(state.ctx, 'list_workspace_folders', {}, meta);
  assert.equal(listing.default_cwd, '.');

  const rootRead = await callTool(state.ctx, 'read_file', { path: 'marker.txt' }, meta);
  assert.equal(rootRead.ok, true, JSON.stringify(rootRead));
  assert.equal(rootRead.content, 'first workspace');
});

test('workspace selection, per-folder cwd, and fallback identity survive Agent restart', async t => {
  const state = await fixture(t);
  const meta = { 'openai/session': 'restart-conversation' };

  await callTool(state.ctx, 'switch_workspace_folder', { folder_id: 'first' }, meta);
  await callTool(state.ctx, 'set_default_cwd', { path: 'src' }, meta);
  await callTool(state.ctx, 'switch_workspace_folder', { folder_id: 'second' }, meta);
  await callTool(state.ctx, 'set_default_cwd', { path: 'packages' }, meta);
  await callTool(state.ctx, 'switch_workspace_folder', { folder_id: 'first' }, meta);
  await callTool(state.ctx, 'switch_workspace_folder', { folder_id: 'second' }, undefined);
  await state.ctx.conversations.flush();

  const restarted = await createToolContext(config(state.folders, state.dataDir));
  t.after(async () => {
    await restarted.conversations.flush();
    await restarted.usageStore.flush();
  });
  assert.equal(restarted.conversations.fallbackKey, state.ctx.conversations.fallbackKey);

  const restored = await callTool(restarted, 'list_workspace_folders', {}, meta);
  assert.equal(restored.selected_folder_id, 'first');
  assert.equal(restored.default_cwd, 'src');
  assert.equal(restored.folders.find(folder => folder.id === 'second').default_cwd, 'packages');
  const restoredSecond = await callTool(restarted, 'switch_workspace_folder', { folder_id: 'second' }, meta);
  assert.equal(restoredSecond.default_cwd, 'packages');

  const fallback = await callTool(restarted, 'list_workspace_folders', {}, undefined);
  assert.equal(fallback.selected_folder_id, 'second');
});

test('conversation contexts use deterministic 128-entry LRU while preserving bindings and cwd', () => {
  const store = new ConversationStore(MAX_CONVERSATION_CONTEXTS);
  store.selectFolder('conversation-000', 'repo');
  store.setCurrentCwd('conversation-000', 'remembered');
  for (let index = 1; index <= MAX_CONVERSATION_CONTEXTS + 160; index += 1) {
    store.selectFolder(`conversation-${String(index).padStart(3, '0')}`, 'repo');
  }

  assert.equal(store.activeContextCount, MAX_CONVERSATION_CONTEXTS);
  assert.equal(store.selectionCount, MAX_CONVERSATION_CONTEXTS + 161);
  assert.equal(store.savedCwdCount, 161);
  assert.equal(store.isContextActive('conversation-000', 'repo'), false);
  assert.equal(store.isContextActive('conversation-288', 'repo'), true);
  assert.equal(store.selectedFolder('conversation-000'), 'repo');

  assert.equal(store.currentCwd('conversation-000'), 'remembered');
  assert.equal(store.activeContextCount, MAX_CONVERSATION_CONTEXTS);
  assert.equal(store.isContextActive('conversation-000', 'repo'), true);
  assert.equal(store.isContextActive('conversation-161', 'repo'), false);
  assert.equal(store.savedCwdCount, 161);
});

test('equivalent Windows, UNC, and WSL roots share a stable identity', () => {
  assert.equal(
    sameWorkspacePath(String.raw`C:\Work\Repo`, String.raw`\\?\C:\work\repo`),
    true
  );
  assert.equal(
    sameWorkspacePath(String.raw`\\SERVER\Share\Repo`, String.raw`\\?\UNC\server\share\repo`),
    true
  );
  assert.equal(
    sameWorkspacePath(
      String.raw`\\wsl$\Ubuntu-24.04\opt\src\Project`,
      String.raw`\\wsl.localhost\ubuntu-24.04\opt\src\Project`
    ),
    true
  );
  assert.equal(
    sameWorkspacePath(
      String.raw`\\wsl.localhost\Ubuntu-24.04\opt\src\Project`,
      String.raw`\\wsl.localhost\Ubuntu-24.04\opt\src\project`
    ),
    false
  );
  assert.equal(
    workspacePathIdentity(String.raw`\\wsl$\Ubuntu-24.04\opt\src\Project`),
    workspacePathIdentity(String.raw`\\wsl.localhost\ubuntu-24.04\opt\src\Project`)
  );
});

test('canonical duplicate roots are rejected and profile identity survives restart', async t => {
  const state = await fixture(t);
  const duplicateConfig = config([
    { id: 'primary', name: 'Primary', path: state.first },
    { id: 'alias', name: 'Alias', path: path.join(state.first, '.') }
  ], path.join(state.base, 'duplicate-data'));
  await assert.rejects(
    createToolContext(duplicateConfig),
    error => error?.code === 'WORKSPACE_FOLDER_DUPLICATE_ROOT'
  );

  const physicalAlias = path.join(state.base, 'physical-alias');
  await symlink(state.first, physicalAlias, process.platform === 'win32' ? 'junction' : 'dir');
  const physicalDuplicateConfig = config([
    { id: 'physical', name: 'Physical', path: state.first },
    { id: 'symlink', name: 'Symlink', path: physicalAlias }
  ], path.join(state.base, 'physical-duplicate-data'));
  await assert.rejects(
    createToolContext(physicalDuplicateConfig),
    error => error?.code === 'WORKSPACE_FOLDER_DUPLICATE_ROOT'
  );

  const firstContext = await createToolContext(config(state.folders, path.join(state.base, 'restart-a')));
  const secondContext = await createToolContext(config(state.folders, path.join(state.base, 'restart-b')));
  assert.equal(firstContext.workspaceProfileId, secondContext.workspaceProfileId);
  await firstContext.conversations.flush();
  await secondContext.conversations.flush();
  await firstContext.usageStore.flush();
  await secondContext.usageStore.flush();
  assert.match(firstContext.workspaceProfileId, /^node-[0-9a-f]{16}$/);
});
