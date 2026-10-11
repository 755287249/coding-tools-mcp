import test from 'node:test';
import assert from 'node:assert/strict';
import { mkdir, mkdtemp, readFile, realpath, rm, symlink, writeFile } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import path from 'node:path';
import { createToolContext } from '../dist/server.js';
import { callTool } from '../dist/tools.js';
import { defaultPolicy } from '../dist/policy.js';
import { disposeProcessSessions } from '../dist/processes.js';
import { dashboardPayload } from '../dist/dashboard.js';

const nodeProgram = path.basename(process.execPath);

test('ToolContext exposes folder-scoped runtime state without single-folder aliases', async () => {
  const [typesSource, serverSource, folderRuntimeSource] = await Promise.all([
    readFile(new URL('../src/types.ts', import.meta.url), 'utf8'),
    readFile(new URL('../src/server.ts', import.meta.url), 'utf8'),
    readFile(new URL('../src/folderRuntime.ts', import.meta.url), 'utf8')
  ]);
  const contextSource = typesSource.match(/export interface ToolContext \{([\s\S]*?)\n\}/)?.[1] ?? '';
  assert.ok(contextSource);
  for (const alias of ['sessions', 'operationsByFingerprint', 'pendingOperations', 'editProposals', 'admission']) {
    assert.doesNotMatch(contextSource, new RegExp(`\\b${alias}\\s*:`));
  }
  assert.doesNotMatch(serverSource, /firstRuntime\.(?:sessions|operationsByFingerprint|pendingOperations|editProposals|admission)/);
  assert.doesNotMatch(folderRuntimeSource, /ctx\.(?:sessions|operationsByFingerprint|pendingOperations|editProposals|admission)\s*=/);
});

function config(base, permissionMode = 'trusted') {
  return {
    host: '127.0.0.1',
    port: 0,
    dataDir: path.join(base, 'data'),
    permissionMode,
    toolProfile: 'advanced',
    activeToolProfile: 'advanced',
    policy: defaultPolicy(),
    management: { enabled: false },
    oauth: {
      clientId: 'chatgpt',
      password: 'folder-isolation-password',
      tokenSecret: 'folder-isolation-token-secret'
    },
    folders: [
      { id: 'a', name: 'Workspace A', path: path.join(base, 'a') },
      { id: 'b', name: 'Workspace B', path: path.join(base, 'b') }
    ],
    limits: {
      blockingConcurrency: 4,
      processConcurrency: 4,
      activeSessionLimit: 16,
      maxOutputBytes: 1024 * 1024
    }
  };
}

async function fixture(t, permissionMode = 'trusted') {
  const base = await mkdtemp(path.join(tmpdir(), 'ctmcp-folder-isolation-'));
  await mkdir(path.join(base, 'a'));
  await mkdir(path.join(base, 'b'));
  const ctx = await createToolContext(config(base, permissionMode));
  const meta = { 'openai/session': `folder-isolation-${Date.now()}-${Math.random()}` };
  t.after(async () => {
    await disposeProcessSessions(ctx);
    await rm(base, { recursive: true, force: true, maxRetries: 5, retryDelay: 50 });
  });
  return { base, a: path.join(base, 'a'), b: path.join(base, 'b'), ctx, meta };
}

async function select(ctx, meta, folderId) {
  const result = await callTool(ctx, 'switch_workspace_folder', { folder_id: folderId }, meta);
  assert.equal(result.ok, true, JSON.stringify(result));
  assert.equal(result.selected_folder_id, folderId);
  assert.equal(result.selected_folder.id, folderId);
  return result;
}

async function missing(file) {
  try {
    await readFile(file);
    return false;
  } catch (error) {
    if (error?.code === 'ENOENT') return true;
    throw error;
  }
}

async function createDirectorySymlinkOrSkip(t, target, link) {
  try {
    await symlink(target, link, process.platform === 'win32' ? 'junction' : 'dir');
    return true;
  } catch (error) {
    if (error?.code === 'EPERM' || error?.code === 'EACCES') {
      t.skip(`directory links are unavailable: ${error.code}`);
      return false;
    }
    throw error;
  }
}

function expectError(result, code) {
  assert.equal(result.ok, false, JSON.stringify(result));
  assert.equal(result.error?.code, code, JSON.stringify(result));
}

async function stop(ctx, meta, folderId, sessionId) {
  await select(ctx, meta, folderId);
  const result = await callTool(ctx, 'kill_session', { session_id: sessionId, wait_ms: 5_000 }, meta);
  assert.equal(result.ok, true, JSON.stringify(result));
}

test('process sessions, operations, fingerprints and locks are isolated by workspace folder', async t => {
  const { ctx, meta } = await fixture(t);
  const runtimeA = ctx.folderRuntimes.get('a');
  const runtimeB = ctx.folderRuntimes.get('b');
  assert.notEqual(runtimeA.sessions, runtimeB.sessions);
  assert.notEqual(runtimeA.operationsByFingerprint, runtimeB.operationsByFingerprint);
  assert.notEqual(runtimeA.admission.locks, runtimeB.admission.locks);

  const command = {
    operation_id: 'shared-operation',
    lock_group: 'shared-lock',
    program: nodeProgram,
    args: ['-e', 'setTimeout(() => {}, 30000)'],
    timeout_ms: 30_000,
    yield_time_ms: 0
  };

  await select(ctx, meta, 'a');
  const startedA = await callTool(ctx, 'exec_command', command, meta);
  assert.equal(startedA.ok, true, JSON.stringify(startedA));
  assert.equal(startedA.workspace_folder_id, 'a');
  assert.equal(runtimeA.sessions.has(startedA.session_id), true);
  assert.equal(runtimeB.sessions.has(startedA.session_id), false);

  await select(ctx, meta, 'b');
  const listedB = await callTool(ctx, 'list_sessions', { include_finalized: true }, meta);
  assert.equal(listedB.sessions.some(session => session.session_id === startedA.session_id), false);
  const waitedAFromB = await callTool(ctx, 'wait_command', {
    session_id: startedA.session_id, timeout_ms: 0
  }, meta);
  assert.equal(waitedAFromB.ok, true, JSON.stringify(waitedAFromB));
  assert.equal(waitedAFromB.resolved_workspace_id, 'a');
  assert.equal(waitedAFromB.workspace_route_source, 'session_id');
  assert.equal(waitedAFromB.workspace_route_changed, true);
  assert.equal(waitedAFromB.conversation_selection_changed, false);

  const outputAFromB = await callTool(ctx, 'read_output', {
    output_ref: startedA.output_refs.stdout
  }, meta);
  assert.equal(outputAFromB.ok, true, JSON.stringify(outputAFromB));
  assert.equal(outputAFromB.resolved_workspace_id, 'a');
  assert.equal(outputAFromB.workspace_route_source, 'session_id');
  assert.equal(outputAFromB.workspace_route_changed, true);
  assert.equal(outputAFromB.conversation_selection_changed, false);

  expectError(await callTool(ctx, 'resolve_operation', {
    operation_id: 'shared-operation'
  }, meta), 'OPERATION_NOT_FOUND');
  const selectedAfterRouting = await callTool(ctx, 'list_workspace_folders', {}, meta);
  assert.equal(selectedAfterRouting.selected_folder_id, 'b');

  const startedB = await callTool(ctx, 'exec_command', command, meta);
  assert.equal(startedB.ok, true, JSON.stringify(startedB));
  assert.equal(startedB.workspace_folder_id, 'b');
  assert.notEqual(startedB.session_id, startedA.session_id);
  assert.equal(runtimeB.sessions.has(startedB.session_id), true);
  const listedBAfter = await callTool(ctx, 'list_sessions', { include_finalized: true }, meta);
  assert.deepEqual(listedBAfter.sessions.map(session => session.workspace_folder_id), ['b']);

  const dashboard = await dashboardPayload(ctx, Date.now() - 1_000);
  assert.equal(dashboard.sessions.total, 2);
  assert.deepEqual(
    dashboard.sessions.items.map(session => session.workspaceId).sort(),
    ['a', 'b']
  );
  assert.deepEqual(dashboard.admission.blocking, { limit: 8, active: 0, queued: 0 });
  assert.deepEqual(dashboard.admission.process, { limit: 8, active: 0, queued: 0 });

  await select(ctx, meta, 'a');
  const listedA = await callTool(ctx, 'list_sessions', { include_finalized: true }, meta);
  assert.deepEqual(listedA.sessions.map(session => session.workspace_folder_id), ['a']);

  await stop(ctx, meta, 'a', startedA.session_id);
  await stop(ctx, meta, 'b', startedB.session_id);
});

test('permission resume IDs stay bound to their original workspace and use Rust-compatible lifecycle', async t => {
  const { a, b, ctx, meta } = await fixture(t, 'guarded');
  await select(ctx, meta, 'a');
  const blocked = await callTool(ctx, 'file_ops', {
    operations: [{ type: 'create', path: 'marker.txt', content: 'from-a' }]
  }, meta);
  expectError(blocked, 'PERMISSION_REQUIRED');
  const resumeId = blocked.error.details.permission_request.resume_id;
  assert.equal(blocked.error.details.permission_request.workspace_folder_id, 'a');

  await select(ctx, meta, 'b');
  const declined = await callTool(ctx, 'request_permissions', { resume_id: resumeId }, meta);
  expectError(declined, 'PERMISSION_NOT_APPROVED');
  assert.equal(declined.error.details.workspace_folder_id, 'a');

  const resumed = await callTool(ctx, 'request_permissions', {
    resume_id: resumeId, approve: true, confirm: true
  }, meta);
  assert.equal(resumed.ok, true, JSON.stringify(resumed));
  assert.equal(resumed.resumed, true);
  assert.equal(resumed.resumed_workspace_folder_id, 'a');
  assert.equal(resumed.resumed_execution_lane, 'blocking_worker');
  assert.equal(await readFile(path.join(a, 'marker.txt'), 'utf8'), 'from-a');
  assert.equal(await missing(path.join(b, 'marker.txt')), true);

  expectError(await callTool(ctx, 'request_permissions', {
    resume_id: resumeId, approve: true, confirm: true
  }, meta), 'RESUME_OPERATION_NOT_FOUND');
  await select(ctx, meta, 'a');
  const expired = await callTool(ctx, 'file_ops', {
    operations: [{ type: 'create', path: 'expired.txt', content: 'expired' }]
  }, meta);
  const expiredId = expired.error.details.permission_request.resume_id;
  ctx.folderRuntimes.get('a').pendingOperations.get(expiredId).expiresAt = Date.now() - 1;
  await select(ctx, meta, 'b');
  expectError(await callTool(ctx, 'request_permissions', {
    resume_id: expiredId, approve: true, confirm: true
  }, meta), 'RESUME_OPERATION_NOT_FOUND');

  await select(ctx, meta, 'a');
  const stale = await callTool(ctx, 'file_ops', {
    operations: [{ type: 'create', path: 'stale.txt', content: 'stale' }]
  }, meta);
  const staleId = stale.error.details.permission_request.resume_id;
  ctx.folderRuntimes.get('a').pendingOperations.get(staleId).workspacePath = b;
  await select(ctx, meta, 'b');
  expectError(await callTool(ctx, 'request_permissions', {
    resume_id: staleId, approve: true, confirm: true
  }, meta), 'RESUME_OPERATION_STALE');
  assert.equal(await missing(path.join(a, 'stale.txt')), true);
  assert.equal(await missing(path.join(b, 'stale.txt')), true);

  expectError(await callTool(ctx, 'request_permissions', {
    resume_id: 'missing-resume-id', approve: true, confirm: true
  }, meta), 'RESUME_OPERATION_NOT_FOUND');
});

test('configured root symlink replacement cannot redirect a retained operation', async t => {
  const base = await mkdtemp(path.join(tmpdir(), 'ctmcp-folder-root-binding-'));
  const original = path.join(base, 'original');
  const replacement = path.join(base, 'replacement');
  const linkedRoot = path.join(base, 'workspace-link');
  await mkdir(original);
  await mkdir(replacement);
  if (!await createDirectorySymlinkOrSkip(t, original, linkedRoot)) {
    await rm(base, { recursive: true, force: true });
    return;
  }
  const runtimeConfig = config(base, 'guarded');
  runtimeConfig.folders = [{ id: 'linked', name: 'Linked', path: linkedRoot }];
  const ctx = await createToolContext(runtimeConfig);
  const meta = { 'openai/session': `root-binding-${Date.now()}-${Math.random()}` };
  t.after(async () => {
    await disposeProcessSessions(ctx);
    await rm(base, { recursive: true, force: true, maxRetries: 5, retryDelay: 50 });
  });

  // Compare canonical paths while retaining the original short-path fixture input.
  assert.equal(ctx.config.folders[0].path, await realpath(original));
  await select(ctx, meta, 'linked');
  const blocked = await callTool(ctx, 'file_ops', {
    operations: [{ type: 'create', path: 'bound.txt', content: 'original' }]
  }, meta);
  expectError(blocked, 'PERMISSION_REQUIRED');
  const resumeId = blocked.error.details.permission_request.resume_id;

  await rm(linkedRoot, { force: true });
  assert.equal(await createDirectorySymlinkOrSkip(t, replacement, linkedRoot), true);
  const resumed = await callTool(ctx, 'request_permissions', {
    resume_id: resumeId, approve: true, confirm: true
  }, meta);
  assert.equal(resumed.ok, true, JSON.stringify(resumed));
  assert.equal(await readFile(path.join(original, 'bound.txt'), 'utf8'), 'original');
  assert.equal(await missing(path.join(replacement, 'bound.txt')), true);
});

test('pending permission stores are independently capped at 256 and evict the oldest request', async t => {
  const { ctx, meta } = await fixture(t, 'guarded');
  await select(ctx, meta, 'a');
  const ids = [];
  for (let index = 0; index < 257; index += 1) {
    const blocked = await callTool(ctx, 'file_ops', {
      operations: [{ type: 'create', path: `pending-${index}.txt`, content: String(index) }]
    }, meta);
    expectError(blocked, 'PERMISSION_REQUIRED');
    ids.push(blocked.error.details.permission_request.resume_id);
  }
  assert.equal(ctx.folderRuntimes.get('a').pendingOperations.size, 256);
  assert.equal(ctx.folderRuntimes.get('b').pendingOperations.size, 0);
  expectError(await callTool(ctx, 'request_permissions', {
    resume_id: ids[0], approve: true, confirm: true
  }, meta), 'RESUME_OPERATION_NOT_FOUND');
  expectError(await callTool(ctx, 'request_permissions', {
    resume_id: ids.at(-1)
  }, meta), 'PERMISSION_NOT_APPROVED');
});

test('edit proposals cannot be applied from another workspace folder', async t => {
  const { a, b, ctx, meta } = await fixture(t);
  await writeFile(path.join(a, 'main.txt'), 'let  value = 1;\n');
  await writeFile(path.join(b, 'main.txt'), 'let  value = 1;\n');

  await select(ctx, meta, 'a');
  const proposal = await callTool(ctx, 'edit_file', {
    path: 'main.txt',
    edits: [{ type: 'replace', old_text: 'let value = 1;', new_text: 'let value = 2;' }]
  }, meta);
  assert.equal(proposal.ok, true, JSON.stringify(proposal));
  assert.equal(proposal.status, 'proposal_required');
  assert.equal(ctx.folderRuntimes.get('a').editProposals.has(proposal.proposal_id), true);

  await select(ctx, meta, 'b');
  const foreign = await callTool(ctx, 'edit_file', {
    path: 'main.txt', apply_proposal: { proposal_id: proposal.proposal_id }
  }, meta);
  expectError(foreign, 'EDIT_PROPOSAL_NOT_FOUND');
  assert.equal(await readFile(path.join(b, 'main.txt'), 'utf8'), 'let  value = 1;\n');

  await select(ctx, meta, 'a');
  const applied = await callTool(ctx, 'edit_file', {
    path: 'main.txt', apply_proposal: { proposal_id: proposal.proposal_id }
  }, meta);
  assert.equal(applied.ok, true, JSON.stringify(applied));
  assert.equal(applied.status, 'proposal_applied');
  assert.equal(await readFile(path.join(a, 'main.txt'), 'utf8'), 'let value = 2;\n');
});
