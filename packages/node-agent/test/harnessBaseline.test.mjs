import test from 'node:test';
import assert from 'node:assert/strict';
import { execFile as execFileCallback } from 'node:child_process';
import { createHash } from 'node:crypto';
import { mkdir, mkdtemp, readFile, readdir, writeFile } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import path from 'node:path';
import { promisify } from 'node:util';
import { createToolContext } from '../dist/server.js';
import { callTool } from '../dist/tools.js';
import { captureBaseline, harnessWorkspaceId } from '../dist/taskTools.js';

const execFile = promisify(execFileCallback);
const nodeProgram = path.basename(process.execPath);

test('runtime context depends on a pure state contract instead of the store implementation', async () => {
  const [typesSource, stateSource, contractSource] = await Promise.all([
    readFile(new URL('../src/types.ts', import.meta.url), 'utf8'),
    readFile(new URL('../src/state.ts', import.meta.url), 'utf8'),
    readFile(new URL('../src/state/contract.ts', import.meta.url), 'utf8')
  ]);
  assert.match(typesSource, /from ['"]\.\/state\/contract\.js['"]/);
  assert.doesNotMatch(typesSource, /from ['"]\.\/state\.js['"]/);
  assert.match(typesSource, /export type \{[^}]*OperationRecord[^}]*\} from ['"]\.\/state\/contract\.js['"]/s);
  assert.match(typesSource, /export type \{[^}]*TaskRecord[^}]*\} from ['"]\.\/state\/contract\.js['"]/s);
  assert.match(stateSource, /implements StateStoreContract/);
  assert.doesNotMatch(stateSource, /from ['"]\.\/types\.js['"]/);
  assert.doesNotMatch(contractSource, /from\s+['"]/);
});

function config(folders, dataDir) {
  return {
    host: '127.0.0.1',
    port: 0,
    dataDir,
    permissionMode: 'trusted',
    oauth: { clientId: 'chatgpt', password: 'test-password', tokenSecret: 'a sufficiently long test token secret' },
    folders,
    limits: { blockingConcurrency: 4, processConcurrency: 4, activeSessionLimit: 16, maxOutputBytes: 1024 * 1024 }
  };
}

function meta() { return { 'openai/session': `harness-${Math.random().toString(36).slice(2)}` }; }

async function git(root, ...args) {
  return execFile('git', args, { cwd: root, encoding: 'utf8' });
}

async function initRepo(root, files = { 'tracked.txt': 'initial\n' }) {
  await mkdir(root, { recursive: true });
  await git(root, 'init');
  await git(root, 'config', 'user.name', 'Harness Test');
  await git(root, 'config', 'user.email', 'harness@example.invalid');
  for (const [relative, content] of Object.entries(files)) {
    const file = path.join(root, relative);
    await mkdir(path.dirname(file), { recursive: true });
    await writeFile(file, content);
  }
  await git(root, 'add', '--all');
  await git(root, 'commit', '-m', 'initial');
}

async function fixture(files) {
  const root = await mkdtemp(path.join(tmpdir(), 'ctmcp-harness-workspace-'));
  const dataDir = await mkdtemp(path.join(tmpdir(), 'ctmcp-harness-state-'));
  await initRepo(root, files);
  const session = meta();
  const ctx = await createToolContext(config([{ id: 'repo', name: 'Repo', path: root }], dataDir));
  const selected = await callTool(ctx, 'switch_workspace_folder', { folder_id: 'repo' }, session);
  assert.equal(selected.ok, true);
  return { root, dataDir, ctx, meta: session };
}

function sha256(value) { return createHash('sha256').update(value).digest('hex'); }

function expectedFingerprint(entries) {
  const hash = createHash('sha256');
  for (const entry of [...entries].sort((left, right) => Buffer.compare(Buffer.from(left.path), Buffer.from(right.path)))) {
    hash.update(Buffer.from(entry.path));
    hash.update(Buffer.from(entry.sha256));
    const bytes = Buffer.alloc(8);
    bytes.writeBigUInt64LE(BigInt(entry.bytes));
    hash.update(bytes);
  }
  return hash.digest('hex');
}

test('start_task captures Git-visible file entries and reports a matching baseline', async () => {
  const state = await fixture({
    '.gitignore': ['node_modules/', 'target/', 'dist/', 'build/', '.svelte-kit/', '.mcp-probe-kit/'].join('\n') + '\n',
    'tracked.txt': 'initial\n',
    'nested/code.ts': 'export const value = 1;\n'
  });
  await writeFile(path.join(state.root, 'binary.bin'), Buffer.from([1, 0, 2]));
  for (const directory of ['node_modules', 'target', 'dist', 'build', '.svelte-kit', '.mcp-probe-kit']) {
    await mkdir(path.join(state.root, directory), { recursive: true });
    await writeFile(path.join(state.root, directory, 'ignored.txt'), 'ignored\n');
  }

  const started = await callTool(state.ctx, 'start_task', { objective: 'Capture baseline' }, state.meta);
  assert.equal(started.ok, true);
  const entries = started.task.baseline.entries;
  assert.equal(started.task.workspace_id, await harnessWorkspaceId(state.root));
  assert.deepEqual(entries.map(entry => entry.path), ['.gitignore', 'binary.bin', 'nested/code.ts', 'tracked.txt']);
  assert.equal(entries.find(entry => entry.path === 'binary.bin').is_binary, true);
  assert.equal(entries.find(entry => entry.path === 'tracked.txt').sha256, sha256('initial\n'));
  assert.equal(started.task.baseline.worktree_fingerprint, expectedFingerprint(entries));

  const status = await callTool(state.ctx, 'harness_status', {}, state.meta);
  assert.equal(status.baseline_matches, true);
  assert.equal(status.writable, true);
  assert.equal(status.capabilities.write.status, 'available');
  assert.equal(status.branch, started.task.baseline.branch);
  assert.equal(status.head, started.task.baseline.head);
});

test('baseline follows Git ignore rules and excludes nested worktree management containers from untracked state', async () => {
  const state = await fixture({
    '.gitignore': 'runtime-data/\ncache/\n',
    'tracked.txt': 'initial\n',
    'managed-worktrees/keep.txt': 'tracked sibling\n'
  });
  await mkdir(path.join(state.root, 'runtime-data'), { recursive: true });
  await writeFile(path.join(state.root, 'runtime-data', 'state.json'), '{"version":1}\n');
  const container = path.join(state.root, 'managed-worktrees');
  const linked = path.join(container, 'linked-worktree');
  const linkedBranch = `linked-${Math.random().toString(36).slice(2)}`;
  await git(state.root, 'worktree', 'add', '-b', linkedBranch, linked);
  await mkdir(path.join(container, 'cargo-target', 'release'), { recursive: true });
  await writeFile(path.join(container, 'cargo-target', 'release', 'artifact.bin'), 'generated\n');

  const started = await callTool(state.ctx, 'start_task', { objective: 'Ignore runtime-owned files' }, state.meta);
  assert.equal(started.ok, true);
  assert.equal(started.task.baseline.entries.some(entry => entry.path.startsWith('runtime-data/')), false);
  assert.equal(started.task.baseline.entries.some(entry => entry.path.startsWith('managed-worktrees/linked-worktree/')), false);
  assert.equal(started.task.baseline.entries.some(entry => entry.path === 'managed-worktrees/cargo-target/release/artifact.bin'), false);
  assert.equal(started.task.baseline.entries.some(entry => entry.path === 'managed-worktrees/keep.txt'), true);

  await writeFile(path.join(state.root, 'runtime-data', 'state.json'), '{"version":2}\n');
  await writeFile(path.join(linked, 'tracked.txt'), 'linked change\n');
  await writeFile(path.join(container, 'cargo-target', 'release', 'artifact.bin'), 'generated again\n');
  const status = await callTool(state.ctx, 'harness_status', {}, state.meta);
  assert.equal(status.baseline_matches, true);
  assert.equal(status.writable, true);
});

test('broad workspace scans exclude wholly-untracked worktree containers but explicit paths remain accessible', async () => {
  const state = await fixture({ 'tracked.txt': 'visible marker\n' });
  const container = path.join(state.root, 'scan-worktrees');
  const linked = path.join(container, 'linked-worktree');
  const linkedBranch = `scan-linked-${Math.random().toString(36).slice(2)}`;
  await git(state.root, 'worktree', 'add', '-b', linkedBranch, linked);
  const scratch = path.join(container, 'cargo-target', 'release');
  await mkdir(scratch, { recursive: true });
  await writeFile(path.join(scratch, 'artifact.txt'), 'scratch-only-marker\n');

  const broadSearch = await callTool(state.ctx, 'search_text', { query: 'scratch-only-marker' }, state.meta);
  assert.equal(broadSearch.ok, true);
  assert.equal(broadSearch.returned_count, 0);
  assert.equal(broadSearch.scan_excluded_worktree_container_count, 1);

  const broadList = await callTool(state.ctx, 'list_files', { recursive: true, max_results: 100 }, state.meta);
  assert.equal(broadList.ok, true);
  assert.equal(broadList.entries.some(entry => entry.path.startsWith('scan-worktrees/')), false);
  assert.equal(broadList.scan_excluded_worktree_container_count, 1);

  const project = await callTool(state.ctx, 'project_map', { max_depth: 4 }, state.meta);
  assert.equal(project.ok, true);
  assert.equal(project.tree.some(entry => entry.path.startsWith('scan-worktrees/')), false);
  assert.equal(project.scan_excluded_worktree_container_count, 1);

  const explicitSearch = await callTool(state.ctx, 'search_text', {
    path: 'scan-worktrees/cargo-target',
    query: 'scratch-only-marker'
  }, state.meta);
  assert.equal(explicitSearch.ok, true);
  assert.equal(explicitSearch.returned_count, 1);
  assert.equal(explicitSearch.scan_excluded_worktree_container_count, 0);

  const explicitList = await callTool(state.ctx, 'list_files', {
    path: 'scan-worktrees/cargo-target',
    recursive: true,
    max_results: 100
  }, state.meta);
  assert.equal(explicitList.ok, true);
  assert.equal(explicitList.entries.some(entry => entry.path.endsWith('artifact.txt')), true);
  assert.equal(explicitList.scan_excluded_worktree_container_count, 0);
});

test('non-ignored untracked file drift remains visible without blocking writes', async () => {
  const state = await fixture();
  await writeFile(path.join(state.root, 'notes.txt'), 'before\n');
  const started = await callTool(state.ctx, 'start_task', { objective: 'Track untracked files' }, state.meta);
  assert.equal(started.task.baseline.entries.some(entry => entry.path === 'notes.txt'), true);

  await writeFile(path.join(state.root, 'notes.txt'), 'after\n');
  const status = await callTool(state.ctx, 'harness_status', {}, state.meta);
  assert.equal(status.baseline_matches, false);
  assert.equal(status.writable, true);
  assert.equal(status.capabilities.write.status, 'available');
});

test('project_state computes clean from the complete file set before max_files truncation', async () => {
  const state = await fixture({ 'a-clean.txt': 'clean\n', 'z-changed.txt': 'before\n' });
  await callTool(state.ctx, 'start_task', { objective: 'Bound project state' }, state.meta);
  await writeFile(path.join(state.root, 'z-changed.txt'), 'after\n');

  const project = await callTool(state.ctx, 'project_state', { max_files: 1 }, state.meta);
  assert.equal(project.truncated, true);
  assert.equal(project.total_files, 2);
  assert.equal(project.files.length, 1);
  assert.equal(project.files[0].path, 'a-clean.txt');
  assert.equal(project.files[0].status, 'unchanged');
  assert.equal(project.clean, false);
});

test('task_context enforces the public max_bytes budget', async () => {
  const state = await fixture();
  const objective = 'bounded-context-'.repeat(2_000);
  const started = await callTool(state.ctx, 'start_task', { objective }, state.meta);
  assert.equal(started.ok, true);

  const context = await callTool(state.ctx, 'task_context', { max_bytes: 8_192 }, state.meta);
  assert.equal(context.ok, true);
  assert.equal(context.max_bytes, 8_192);
  assert.equal(context.truncated, true);
  assert.ok(Buffer.byteLength(JSON.stringify(context)) <= 8_192);
  assert.ok(context.task.objective.length < objective.length);
});

test('task_context trims a production-sized baseline without quadratic work', { timeout: 30_000 }, async () => {
  const state = await fixture();
  const started = await callTool(state.ctx, 'start_task', { objective: 'Bound a large baseline' }, state.meta);
  assert.equal(started.ok, true);
  const entryCount = 50_000;
  started.task.baseline.entries = Array.from({ length: entryCount }, (_, index) => ({
    path: `generated/${String(index).padStart(5, '0')}.txt`,
    exists: true,
    is_binary: false,
    sha256: '0'.repeat(64),
    bytes: 1
  }));
  await state.ctx.state.setTask(started.task.workspace_id, started.task);

  const beforeCpu = process.cpuUsage();
  const context = await callTool(state.ctx, 'task_context', { max_bytes: 8_192 }, state.meta);
  const cpu = process.cpuUsage(beforeCpu);
  const elapsedCpuMs = (cpu.user + cpu.system) / 1_000;
  assert.equal(context.ok, true);
  assert.equal(context.truncated, true);
  assert.ok(Buffer.byteLength(JSON.stringify(context)) <= 8_192);
  assert.ok(context.task.baseline.entries.length < entryCount);
  assert.ok(elapsedCpuMs < 5_000, `large task_context used ${elapsedCpuMs.toFixed(0)}ms CPU`);
});

test('external changes remain writable and are adopted before real writes', async () => {
  const state = await fixture();
  const started = await callTool(state.ctx, 'start_task', { objective: 'Track active task' }, state.meta);
  assert.equal(started.ok, true);
  await writeFile(path.join(state.root, 'tracked.txt'), 'external\n');
  const externalHash = sha256('external\n');

  const status = await callTool(state.ctx, 'harness_status', {}, state.meta);
  assert.equal(status.baseline_matches, false);
  assert.equal(status.writable, true);
  assert.equal(status.capabilities.write.status, 'available');
  assert.deepEqual(status.next_actions.slice(0, 2), ['project_state', 'git_diff']);

  const dryRun = await callTool(state.ctx, 'edit_file', {
    path: 'tracked.txt',
    expected_sha256: externalHash,
    dry_run: true,
    edits: [{ type: 'replace', old_text: 'external\n', new_text: 'planned\n' }]
  }, state.meta);
  assert.equal(dryRun.ok, true);
  assert.equal(dryRun.applied, false);

  const applied = await callTool(state.ctx, 'edit_file', {
    path: 'tracked.txt',
    expected_sha256: externalHash,
    edits: [{ type: 'replace', old_text: 'external\n', new_text: 'applied\n' }]
  }, state.meta);
  assert.equal(applied.ok, true);
  assert.equal(await readFile(path.join(state.root, 'tracked.txt'), 'utf8'), 'applied\n');
  const adoptedStatus = await callTool(state.ctx, 'harness_status', {}, state.meta);
  assert.equal(adoptedStatus.baseline_matches, true);
  assert.equal(adoptedStatus.writable, true);
});

test('branch or HEAD changes are adopted before starting a process', async () => {
  const state = await fixture();
  await callTool(state.ctx, 'start_task', { objective: 'Track Git baseline' }, state.meta);
  await git(state.root, 'checkout', '-b', 'external-branch');

  const result = await callTool(state.ctx, 'exec_command', {
    program: 'git',
    args: ['status'],
    yield_time_ms: 30_000
  }, state.meta);
  assert.equal(result.ok, true);
  const status = await callTool(state.ctx, 'harness_status', {}, state.meta);
  assert.equal(status.baseline_matches, true);
  assert.equal(status.writable, true);
});

test('successful tracked mutations refresh expected state and persist change evidence', async () => {
  const state = await fixture({ 'tracked.txt': 'initial\n', 'delete.txt': 'remove\n' });
  const started = await callTool(state.ctx, 'start_task', { objective: 'Record file evidence' }, state.meta);
  const taskId = started.task.id;

  const edited = await callTool(state.ctx, 'edit_file', {
    path: 'tracked.txt',
    expected_sha256: sha256('initial\n'),
    edits: [{ type: 'replace', old_text: 'initial\n', new_text: 'changed\n' }]
  }, state.meta);
  assert.equal(edited.ok, true);
  assert.match(edited.operation_id, /^[0-9a-f]{32}$/);
  assert.equal(edited.baseline_refresh_mode, 'incremental_verified_edit');
  assert.equal(edited.baseline_refresh_file_count, 1);
  assert.ok(Number(edited.phase_durations_ms?.baseline_refresh_ms ?? -1) >= 0);

  const files = await callTool(state.ctx, 'file_ops', {
    operations: [
      { type: 'create', path: 'added.txt', content: 'added\n' },
      { type: 'delete', path: 'delete.txt' }
    ]
  }, state.meta);
  assert.equal(files.ok, true);

  const status = await callTool(state.ctx, 'harness_status', {}, state.meta);
  assert.equal(status.baseline_matches, true);
  const project = await callTool(state.ctx, 'project_state', { max_files: 20 }, state.meta);
  assert.equal(project.clean, false);
  assert.deepEqual(
    Object.fromEntries(project.files.filter(file => file.status !== 'unchanged').map(file => [file.path, file.status])),
    { 'added.txt': 'added', 'delete.txt': 'deleted', 'tracked.txt': 'modified' }
  );
  assert.equal(project.files.find(file => file.path === 'tracked.txt').sha256, sha256('changed\n'));
  assert.equal(project.files.find(file => file.path === 'delete.txt').sha256, '');

  const summary = await callTool(state.ctx, 'change_summary', { task_id: taskId }, state.meta);
  assert.deepEqual(summary.files.map(file => file.path), ['added.txt', 'delete.txt', 'tracked.txt']);
  assert.deepEqual(summary.verification, []);
  assert.deepEqual(summary.risks, []);
  assert.equal(summary.rollback_capability, 'not_available_in_foundation');
  assert.ok(summary.evidence.some(item => item.kind === 'operation_started' && item.tool_name === 'edit'));
  assert.ok(summary.evidence.some(item => item.kind === 'operation_finished' && item.tool_name === 'file_ops'));

  const log = await callTool(state.ctx, 'operation_log', { cursor: 0, limit: 100 }, state.meta);
  const editRows = log.operations.filter(row => row.tool === 'edit');
  assert.deepEqual(editRows.map(row => row.kind), ['completed', 'started']);
  assert.equal(editRows[0].id, editRows[1].id);
  assert.equal(editRows[0].task_id, taskId);
  assert.deepEqual(editRows[0].affected_files, []);
  assert.deepEqual(editRows[0].result_summary.affected_files, edited.affected_files);
});

test('finish_task summary persists an immutable change selected by change_id', async () => {
  const state = await fixture({ 'tracked.txt': 'initial\n' });
  const started = await callTool(state.ctx, 'start_task', { objective: 'Persist completion evidence' }, state.meta);
  const taskId = started.task.id;

  const blank = await callTool(state.ctx, 'finish_task', {
    task_id: taskId,
    summary: '   ',
    allow_unverified: true
  }, state.meta);
  assert.equal(blank.ok, false);
  assert.equal(blank.error.code, 'INVALID_ARGUMENT');
  const stillActive = await callTool(state.ctx, 'task_context', {}, state.meta);
  assert.equal(stillActive.task.status, 'active');

  const edited = await callTool(state.ctx, 'edit_file', {
    path: 'tracked.txt',
    expected_sha256: sha256('initial\n'),
    edits: [{ type: 'replace', old_text: 'initial\n', new_text: 'released\n' }]
  }, state.meta);
  assert.equal(edited.ok, true);

  const finished = await callTool(state.ctx, 'finish_task', {
    task_id: taskId,
    summary: '  Release evidence captured  ',
    allow_unverified: true
  }, state.meta);
  assert.equal(finished.ok, true);
  assert.equal(finished.summary, 'Release evidence captured');
  assert.match(finished.change_id, /^[0-9a-f]{32}$/);
  assert.equal(finished.task.latest_change_id, finished.change_id);
  assert.equal(finished.task.status, 'completed_unverified');
  assert.equal(finished.change_summary.change_id, finished.change_id);
  assert.deepEqual(finished.change_summary.why, {
    text: 'Release evidence captured',
    source: 'finish_task_summary'
  });
  const captured = finished.change_summary.files.find(file => file.path === 'tracked.txt');
  assert.equal(captured.status, 'modified');
  assert.equal(captured.before_sha256, sha256('initial\n'));
  assert.equal(captured.after_sha256, sha256('released\n'));
  assert.ok(finished.change_summary.evidence.some(item =>
    item.kind === 'task_finished'
      && item.reason?.text === 'Release evidence captured'
      && item.result_summary?.change_id === finished.change_id));

  await writeFile(path.join(state.root, 'tracked.txt'), 'later external change\n');
  const selected = await callTool(state.ctx, 'change_summary', { change_id: finished.change_id }, state.meta);
  assert.equal(selected.change_id, finished.change_id);
  assert.equal(selected.files.find(file => file.path === 'tracked.txt').after_sha256, sha256('released\n'));
  const latest = await callTool(state.ctx, 'change_summary', { task_id: taskId }, state.meta);
  assert.deepEqual(latest.files, selected.files);

  const malformed = await callTool(state.ctx, 'change_summary', { change_id: '../not-a-change' }, state.meta);
  assert.equal(malformed.ok, false);
  assert.equal(malformed.error.code, 'INVALID_ARGUMENT');

  const restarted = await createToolContext(config([{ id: 'repo', name: 'Repo', path: state.root }], state.dataDir));
  await callTool(restarted, 'switch_workspace_folder', { folder_id: 'repo' }, state.meta);
  const restored = await callTool(restarted, 'change_summary', { change_id: finished.change_id }, state.meta);
  assert.deepEqual(restored.files, selected.files);
  assert.deepEqual(restored.why, selected.why);

  const persisted = JSON.parse(await readFile(path.join(state.dataDir, 'state.json'), 'utf8'));
  assert.equal(persisted.tasks[taskId].latest_change_id, finished.change_id);
  assert.equal(persisted.changeSets[finished.change_id].reason.text, 'Release evidence captured');

  const next = await callTool(restarted, 'start_task', { objective: 'Next task' }, state.meta);
  const mismatch = await callTool(restarted, 'change_summary', {
    task_id: next.task.id,
    change_id: finished.change_id
  }, state.meta);
  assert.equal(mismatch.ok, false);
  assert.equal(mismatch.error.code, 'CHANGE_TASK_MISMATCH');

  const fallback = await callTool(restarted, 'finish_task', {
    task_id: next.task.id,
    allow_unverified: true
  }, state.meta);
  assert.equal(fallback.ok, true);
  assert.equal(fallback.summary, 'Next task');
  assert.deepEqual(fallback.change_summary.why, {
    text: 'Next task',
    source: 'task_objective'
  });
});

test('finish_task finalizes a verifying task without recapturing its immutable change', async () => {
  const state = await fixture({ 'tracked.txt': 'initial\n' });
  const started = await callTool(state.ctx, 'start_task', { objective: 'Verified completion' }, state.meta);
  await writeFile(path.join(state.root, 'tracked.txt'), 'verified\n');

  const verifying = await callTool(state.ctx, 'finish_task', {
    task_id: started.task.id,
    summary: 'Captured before verification'
  }, state.meta);
  assert.equal(verifying.ok, true);
  assert.equal(verifying.task.status, 'verifying');
  const changeId = verifying.change_id;

  const completed = await callTool(state.ctx, 'finish_task', { task_id: started.task.id }, state.meta);
  assert.equal(completed.ok, true);
  assert.equal(completed.task.status, 'completed');
  assert.equal(completed.change_id, changeId);
  assert.equal(completed.finalized_existing_change, true);
  assert.equal(completed.phase_durations_ms.baseline_capture_ms, 0);
  assert.equal(completed.change_summary.why.text, 'Captured before verification');
});

test('active task operations automatically feed task credit into shadow knowledge', async () => {
  const state = await fixture({ 'tracked.txt': 'initial\n' });
  const runtime = state.ctx.folderRuntimes.get('repo');
  assert.ok(runtime);
  const learned = await runtime.knowledgeStore.upsert({
    id: '', schemaVersion: 1, scope: 'runtime', target: 'tool_strategy', status: 'shadow',
    trigger: { tool: 'read_file' },
    hypothesis: 'Read operations inside tasks provide a task-attribution fixture.',
    recommendedAction: { recommendation: 'observe_only' },
    evidence: {
      observations: 5, successes: 5, failures: 0,
      totalDurationMs: 5, totalRequestBytes: 5, totalResponseBytes: 5,
      firstSeenAtMs: 1, lastSeenAtMs: 5
    },
    confidence: 0.5,
    sourceEventIds: ['seed-1', 'seed-2', 'seed-3', 'seed-4', 'seed-5'],
    counterexampleEventIds: [], createdAtMs: 1, updatedAtMs: 5
  });

  const started = await callTool(state.ctx, 'start_task', { objective: 'Credit shadow task' }, state.meta);
  const read = await callTool(state.ctx, 'read_file', { path: 'tracked.txt' }, state.meta);
  assert.equal(read.ok, true);
  await runtime.knowledgeIngestor.flush();
  let impact = (await runtime.knowledgeImpactStore.list()).find(item => item.knowledgeId === learned.id);
  assert.ok(impact);
  assert.deepEqual(impact.matchedTaskIds, [started.task.id]);
  assert.deepEqual(impact.creditedTaskIds, []);

  const verifying = await callTool(state.ctx, 'finish_task', { task_id: started.task.id }, state.meta);
  assert.equal(verifying.task.status, 'verifying');
  const completed = await callTool(state.ctx, 'finish_task', { task_id: started.task.id }, state.meta);
  assert.equal(completed.task.status, 'completed');
  await runtime.knowledgeIngestor.flush();

  impact = (await runtime.knowledgeImpactStore.list()).find(item => item.knowledgeId === learned.id);
  assert.ok(impact);
  assert.deepEqual(impact.creditedTaskIds, [started.task.id]);
  assert.equal(impact.taskVerifiedSuccesses, 1);
  assert.equal(impact.taskFailures, 0);
});

test('recoverable task failure is credited only after close_failed_task terminalizes it', async () => {
  const state = await fixture({ 'tracked.txt': 'initial\n' });
  const runtime = state.ctx.folderRuntimes.get('repo');
  assert.ok(runtime);
  const learned = await runtime.knowledgeStore.upsert({
    id: '', schemaVersion: 1, scope: 'runtime', target: 'tool_strategy', status: 'shadow',
    trigger: { tool: 'read_file' },
    hypothesis: 'Recoverable task failures must not be treated as terminal learning outcomes.',
    recommendedAction: { recommendation: 'observe_only' },
    evidence: {
      observations: 5, successes: 5, failures: 0,
      totalDurationMs: 5, totalRequestBytes: 5, totalResponseBytes: 5,
      firstSeenAtMs: 1, lastSeenAtMs: 5
    },
    confidence: 0.5,
    sourceEventIds: ['fail-seed-1', 'fail-seed-2', 'fail-seed-3', 'fail-seed-4', 'fail-seed-5'],
    counterexampleEventIds: [], createdAtMs: 1, updatedAtMs: 5
  });

  const started = await callTool(state.ctx, 'start_task', { objective: 'Recoverable failure credit' }, state.meta);
  assert.equal((await callTool(state.ctx, 'read_file', { path: 'tracked.txt' }, state.meta)).ok, true);
  const failed = await callTool(state.ctx, 'fail_task', { task_id: started.task.id }, state.meta);
  assert.equal(failed.task.status, 'failed');
  await runtime.knowledgeIngestor.flush();
  let impact = (await runtime.knowledgeImpactStore.list()).find(item => item.knowledgeId === learned.id);
  assert.ok(impact);
  assert.deepEqual(impact.creditedTaskIds, []);
  assert.equal(impact.taskFailures, 0);

  const closed = await callTool(state.ctx, 'close_failed_task', { task_id: started.task.id }, state.meta);
  assert.equal(closed.task.status, 'failed_final');
  await runtime.knowledgeIngestor.flush();
  impact = (await runtime.knowledgeImpactStore.list()).find(item => item.knowledgeId === learned.id);
  assert.ok(impact);
  assert.deepEqual(impact.creditedTaskIds, [started.task.id]);
  assert.equal(impact.taskFailures, 1);
});

test('rollback_task requires the original task baseline before terminalizing rollback', async () => {
  const state = await fixture({ 'tracked.txt': 'initial\n' });
  const started = await callTool(state.ctx, 'start_task', { objective: 'Verified rollback' }, state.meta);
  await writeFile(path.join(state.root, 'tracked.txt'), 'changed\n');
  const failed = await callTool(state.ctx, 'fail_task', { task_id: started.task.id }, state.meta);
  assert.equal(failed.task.status, 'failed');

  const rejected = await callTool(state.ctx, 'rollback_task', { task_id: started.task.id }, state.meta);
  assert.equal(rejected.ok, false);
  assert.equal(rejected.error.code, 'ROLLBACK_NOT_VERIFIED');

  await writeFile(path.join(state.root, 'tracked.txt'), 'initial\n');
  const rolledBack = await callTool(state.ctx, 'rollback_task', { task_id: started.task.id }, state.meta);
  assert.equal(rolledBack.ok, true);
  assert.equal(rolledBack.task.status, 'rolled_back');
  assert.equal(rolledBack.baseline_verified, true);
});

test('start_task can safely finalize a completed verifying task before starting the next task', async () => {
  const state = await fixture();
  const first = await callTool(state.ctx, 'start_task', { objective: 'First task' }, state.meta);
  const verifying = await callTool(state.ctx, 'finish_task', { task_id: first.task.id }, state.meta);
  assert.equal(verifying.task.status, 'verifying');

  const next = await callTool(state.ctx, 'start_task', {
    objective: 'Second task',
    existing_task: 'finish_if_complete'
  }, state.meta);
  assert.equal(next.ok, true);
  assert.equal(next.recovered_task_id, first.task.id);
  assert.equal(next.task.status, 'active');
  assert.notEqual(next.task.id, first.task.id);
});

test('operation logs persist bounded execution diagnostics without raw process payloads', async () => {
  const state = await fixture();
  const outputMarker = 'OPERATION_OUTPUT_MUST_NOT_PERSIST';
  const executed = await callTool(state.ctx, 'exec_command', {
    program: nodeProgram,
    args: ['-e', `setTimeout(() => { process.stderr.write('${outputMarker}'); process.exit(7); }, 5_000)`],
    deduplicate: true,
    yield_time_ms: 0,
    timeout_ms: 10_000
  }, state.meta);
  assert.ok(executed.command_ok === null || executed.command_ok === false, JSON.stringify(executed));
  const reattached = await callTool(state.ctx, 'exec_command', {
    program: nodeProgram,
    args: ['-e', `setTimeout(() => { process.stderr.write('${outputMarker}'); process.exit(7); }, 5_000)`],
    deduplicate: true,
    yield_time_ms: 0,
    timeout_ms: 10_000
  }, state.meta);
  assert.ok(reattached.command_ok === null || reattached.command_ok === false, JSON.stringify(reattached));
  assert.equal(reattached.session_id, executed.session_id);
  assert.equal(reattached.deduplicated, true);
  assert.notEqual(reattached.harness_operation_id, executed.harness_operation_id);
  const finalized = reattached.command_ok === false
    ? reattached
    : executed.command_ok === false
      ? executed
      : await callTool(state.ctx, 'wait_command', {
        session_id: executed.session_id,
        cursor: executed.latest_cursor,
        timeout_ms: 30_000,
        until: 'finalized',
        output_mode: 'delta'
      }, state.meta);
  assert.equal(finalized.ok, true, JSON.stringify(finalized));
  assert.equal(finalized.transport_ok, true, JSON.stringify(finalized));
  assert.equal(finalized.command_ok, false, JSON.stringify(finalized));
  assert.equal(finalized.failure_origin, 'child_process', JSON.stringify(finalized));
  assert.equal(finalized.policy_blocked, false, JSON.stringify(finalized));
  assert.equal(finalized.process_exit_code, 7, JSON.stringify(finalized));

  const log = await callTool(state.ctx, 'operation_log', { cursor: 0, limit: 100 }, state.meta);
  const operationIds = [executed.harness_operation_id, reattached.harness_operation_id];
  for (const operationId of operationIds) {
    const correlated = log.operations.filter(row => row.id === operationId);
    assert.deepEqual(correlated.map(row => row.kind), ['failed', 'started']);
  }
  const terminal = log.operations.find(row => row.id === executed.harness_operation_id && row.kind === 'failed');
  assert.ok(terminal, JSON.stringify(log));
  assert.equal(terminal.result_summary.command_ok, false);
  assert.equal(terminal.result_summary.process_exit_code, 7);
  assert.equal(terminal.result_summary.termination_reason, 'exited');
  assert.equal(terminal.result_summary.stderr_bytes, Buffer.byteLength(outputMarker));
  for (const forbidden of ['command', 'args', 'arguments', 'program', 'stdout', 'stderr', 'environment', 'env']) {
    assert.equal(Object.hasOwn(terminal.result_summary, forbidden), false, `persisted raw ${forbidden}`);
  }
  assert.doesNotMatch(JSON.stringify(terminal.result_summary), new RegExp(outputMarker));
});

test('legacy folder-id tasks migrate to the Rust workspace identity without losing active state', async () => {
  const root = await mkdtemp(path.join(tmpdir(), 'ctmcp-harness-workspace-'));
  const dataDir = await mkdtemp(path.join(tmpdir(), 'ctmcp-harness-state-'));
  await initRepo(root);
  const baseline = await captureBaseline(root);
  const task = {
    id: 'legacy-task',
    workspace_id: 'repo',
    objective: 'Resume legacy state',
    status: 'active',
    baseline,
    expected_fingerprint: baseline.worktree_fingerprint,
    completed_steps: [],
    pending_steps: ['continue'],
    created_at: '1',
    updated_at: '2'
  };
  await writeFile(path.join(dataDir, 'state.json'), `${JSON.stringify({
    tasks: { [task.id]: task },
    currentTasks: { repo: task.id },
    taskEvents: { [task.id]: [] },
    operations: []
  }, null, 2)}\n`);

  const session = meta();
  const ctx = await createToolContext(config([{ id: 'repo', name: 'Repo', path: root }], dataDir));
  await callTool(ctx, 'switch_workspace_folder', { folder_id: 'repo' }, session);
  const status = await callTool(ctx, 'harness_status', {}, session);
  const workspaceId = await harnessWorkspaceId(root);
  assert.equal(status.task_id, task.id);
  assert.equal(status.workspace_id, workspaceId);
  assert.equal(status.baseline_matches, true);
  const context = await callTool(ctx, 'task_context', {}, session);
  assert.equal(context.task.workspace_id, workspaceId);
  const persisted = JSON.parse(await readFile(path.join(dataDir, 'state.json'), 'utf8'));
  assert.equal(persisted.tasks[task.id].workspace_id, workspaceId);
  assert.equal(persisted.currentTasks[workspaceId], task.id);
  assert.equal(persisted.currentTasks.repo, undefined);
});

test('operation logs are isolated per workspace and survive Agent restart', async () => {
  const base = await mkdtemp(path.join(tmpdir(), 'ctmcp-harness-multi-'));
  const dataDir = await mkdtemp(path.join(tmpdir(), 'ctmcp-harness-state-'));
  const rootA = path.join(base, 'a');
  const rootB = path.join(base, 'b');
  await initRepo(rootA, { 'a.txt': 'a\n' });
  await initRepo(rootB, { 'b.txt': 'b\n' });
  const folders = [
    { id: 'alpha', name: 'Alpha', path: rootA },
    { id: 'beta', name: 'Beta', path: rootB }
  ];
  const session = meta();
  const ctx = await createToolContext(config(folders, dataDir));
  const alphaWorkspaceId = await harnessWorkspaceId(rootA);
  const betaWorkspaceId = await harnessWorkspaceId(rootB);
  await callTool(ctx, 'switch_workspace_folder', { folder_id: 'alpha' }, session);
  await callTool(ctx, 'git_status', {}, session);
  const alpha = await callTool(ctx, 'operation_log', { cursor: 0, limit: 20 }, session);
  assert.ok(alpha.operations.length >= 2);
  assert.ok(alpha.operations.every(row => row.workspace_id === alphaWorkspaceId));

  await callTool(ctx, 'switch_workspace_folder', { folder_id: 'beta' }, session);
  const betaEmpty = await callTool(ctx, 'operation_log', { cursor: 0, limit: 20 }, session);
  assert.deepEqual(betaEmpty.operations, []);
  await callTool(ctx, 'git_status', {}, session);
  const beta = await callTool(ctx, 'operation_log', { cursor: 0, limit: 20 }, session);
  assert.ok(beta.operations.every(row => row.workspace_id === betaWorkspaceId));

  const restarted = await createToolContext(config(folders, dataDir));
  await callTool(restarted, 'switch_workspace_folder', { folder_id: 'alpha' }, session);
  const restored = await callTool(restarted, 'operation_log', { cursor: 0, limit: 20 }, session);
  assert.deepEqual(restored.operations, alpha.operations);
  const workspaceDirs = await readdir(path.join(dataDir, 'harness', 'workspaces'), { withFileTypes: true });
  assert.deepEqual(
    workspaceDirs.filter(entry => entry.isDirectory()).map(entry => entry.name).sort(),
    [alphaWorkspaceId, betaWorkspaceId].sort()
  );
});

test('active task conflicts are state-aware and use lightweight error enrichment', async () => {
  const state = await fixture();
  const started = await callTool(state.ctx, 'start_task', { objective: 'Keep active task' }, state.meta);
  assert.ok(Number(started.phase_durations_ms.baseline_capture_ms) >= 0);
  assert.ok(Number(started.phase_durations_ms.dispatch_ms) >= 0);
  assert.ok(Number(started.phase_durations_ms.serialization_ms) >= 0);
  await writeFile(path.join(state.root, 'tracked.txt'), 'external\n');

  const duplicate = await callTool(state.ctx, 'start_task', { objective: 'Duplicate task' }, state.meta);
  assert.equal(duplicate.ok, false);
  assert.equal(duplicate.error.code, 'TASK_ALREADY_ACTIVE');
  assert.equal(duplicate.error.retryable, false);
  assert.equal(duplicate.error.details.retry_mode, 'after_state_change');
  assert.equal(duplicate.error.details.active_task_id, started.task.id);
  assert.equal(duplicate.error.details.recovery_actions[0].tool, 'task_context');
  assert.equal(duplicate.harness.baseline_check_performed, false);
  assert.equal(duplicate.harness.baseline_matches, null);
  assert.equal(duplicate.phase_durations_ms.harness_begin_ms, undefined);
  assert.ok(Number(duplicate.phase_durations_ms.dispatch_ms) >= 0);
  assert.ok(Number(duplicate.phase_durations_ms.error_enrichment_ms) >= 0);
  assert.equal(duplicate.phase_durations_ms.baseline_capture_ms, undefined);
  assert.ok(Number(duplicate.phase_durations_ms.serialization_ms) >= 0);
});

test('active tasks are scoped to the current linked worktree while operation history stays workspace-wide', async () => {
  const state = await fixture();
  const rootTask = await callTool(state.ctx, 'start_task', { objective: 'Root task' }, state.meta);
  assert.equal(rootTask.ok, true);

  const linked = path.join(state.root, '.worktrees', 'scoped-task');
  await mkdir(path.dirname(linked), { recursive: true });
  const branch = `task-scope-${Math.random().toString(36).slice(2)}`;
  await git(state.root, 'worktree', 'add', '-b', branch, linked);
  const selected = await callTool(state.ctx, 'set_default_cwd', { path: '.worktrees/scoped-task' }, state.meta);
  assert.equal(selected.ok, true);

  const linkedStatusBeforeTask = await callTool(state.ctx, 'git_status', {}, state.meta);
  assert.equal(linkedStatusBeforeTask.ok, true, JSON.stringify(linkedStatusBeforeTask));
  assert.equal(linkedStatusBeforeTask.is_repo, true);
  assert.equal(linkedStatusBeforeTask.branch, branch);
  assert.equal(linkedStatusBeforeTask.repo.repo_root.replaceAll('\\', '/'), linked.replaceAll('\\', '/'));
  const linkedHead = (await git(linked, 'rev-parse', 'HEAD')).stdout.trim();
  const linkedCommitDryRun = await callTool(state.ctx, 'git_commit', {
    message: 'linked worktree dry run',
    paths: ['tracked.txt'],
    expected_head: linkedHead,
    dry_run: true
  }, state.meta);
  assert.equal(linkedCommitDryRun.ok, true, JSON.stringify(linkedCommitDryRun));
  assert.equal(linkedCommitDryRun.repo.repo_root.replaceAll('\\', '/'), linked.replaceAll('\\', '/'));

  const linkedTask = await callTool(state.ctx, 'start_task', { objective: 'Linked worktree task' }, state.meta);
  assert.equal(linkedTask.ok, true, JSON.stringify(linkedTask));
  assert.notEqual(linkedTask.task.id, rootTask.task.id);
  assert.equal(linkedTask.task.workspace_id, rootTask.task.workspace_id);
  assert.notEqual(linkedTask.task.scope_id, rootTask.task.scope_id);
  assert.equal(linkedTask.task.scope_root.replaceAll('\\', '/'), linked.replaceAll('\\', '/'));

  const duplicate = await callTool(state.ctx, 'start_task', { objective: 'Duplicate linked task' }, state.meta);
  assert.equal(duplicate.ok, false);
  assert.equal(duplicate.error.code, 'TASK_ALREADY_ACTIVE');
  assert.equal(duplicate.error.details.active_task_id, linkedTask.task.id);

  const edited = await callTool(state.ctx, 'edit_file', {
    path: 'tracked.txt',
    edits: [{ type: 'replace', old_text: 'initial\n', new_text: 'linked\n' }]
  }, state.meta);
  assert.equal(edited.ok, true, JSON.stringify(edited));
  const linkedDiff = await callTool(state.ctx, 'git_diff', {}, state.meta);
  assert.equal(linkedDiff.ok, true, JSON.stringify(linkedDiff));
  assert.match(linkedDiff.diff, /linked/);
  assert.ok(linkedDiff.files.some(file => file.path === 'tracked.txt'));
  const operations = await callTool(state.ctx, 'operation_log', { cursor: 0, limit: 100 }, state.meta);
  assert.ok(operations.operations.some(row => row.tool === 'edit' && row.task_id === linkedTask.task.id));
  assert.equal(operations.operations.some(row => row.tool === 'edit' && row.task_id === rootTask.task.id), false);

  await callTool(state.ctx, 'set_default_cwd', { path: '.' }, state.meta);
  const rootStatus = await callTool(state.ctx, 'harness_status', {}, state.meta);
  assert.equal(rootStatus.task_id, rootTask.task.id);

  await callTool(state.ctx, 'set_default_cwd', { path: '.worktrees/scoped-task' }, state.meta);
  const linkedStatus = await callTool(state.ctx, 'harness_status', {}, state.meta);
  assert.equal(linkedStatus.task_id, linkedTask.task.id);

  const restarted = await createToolContext(config([{ id: 'repo', name: 'Repo', path: state.root }], state.dataDir));
  await callTool(restarted, 'switch_workspace_folder', { folder_id: 'repo' }, state.meta);
  await callTool(restarted, 'set_default_cwd', { path: '.worktrees/scoped-task' }, state.meta);
  const restoredLinked = await callTool(restarted, 'harness_status', {}, state.meta);
  assert.equal(restoredLinked.task_id, linkedTask.task.id);
  await callTool(restarted, 'set_default_cwd', { path: '.' }, state.meta);
  const restoredRoot = await callTool(restarted, 'harness_status', {}, state.meta);
  assert.equal(restoredRoot.task_id, rootTask.task.id);
});

test('disabling automatic baseline checks keeps task evidence without adopting external drift', async t => {
  const state = await fixture();
  const started = await callTool(state.ctx, 'start_task', { objective: 'Track without automatic baseline scans' }, state.meta);
  const originalFingerprint = started.task.expected_fingerprint;
  state.ctx.config.securityPolicy.enforceHarnessBaseline = false;
  await writeFile(path.join(state.root, 'tracked.txt'), 'external\n');

  const status = await callTool(state.ctx, 'harness_status', {}, state.meta);
  assert.equal(status.baseline_check_enabled, false);
  assert.equal(status.baseline_check_performed, false);
  assert.equal(status.baseline_matches, null);

  const clock = t.mock.method(performance, 'now', () => 100);
  const edited = await callTool(state.ctx, 'edit_file', {
    path: 'tracked.txt',
    expected_sha256: sha256('external\n'),
    edits: [{ type: 'replace', old_text: 'external\n', new_text: 'changed\n' }]
  }, state.meta);
  clock.mock.restore();
  assert.equal(edited.ok, true);
  assert.equal(edited.phase_durations_ms.baseline_capture_ms, undefined);
  assert.ok(Number(edited.phase_durations_ms.harness_begin_ms) >= 0);
  assert.ok(Number(edited.phase_durations_ms.dispatch_ms) >= 0);
  assert.equal(edited.phase_durations_ms.harness_finish_ms, 0, 'executed phase is retained even below timer resolution');
  assert.ok(Number(edited.phase_durations_ms.serialization_ms) >= 0);
  const context = await callTool(state.ctx, 'task_context', { task_id: started.task.id }, state.meta);
  assert.equal(context.task.expected_fingerprint, originalFingerprint);
  assert.ok(context.events.some(item => item.kind === 'operation_started' && item.tool_name === 'edit'));
  const startedEvent = context.events.find(item => item.kind === 'operation_started' && item.tool_name === 'edit');
  assert.equal(startedEvent.result_summary.baseline_check_performed, false);
});

test('exec_many remains outside the Rust write-baseline gate', async () => {
  const state = await fixture();
  await callTool(state.ctx, 'start_task', { objective: 'Match Rust classifier' }, state.meta);
  await writeFile(path.join(state.root, 'tracked.txt'), 'external\n');

  const result = await callTool(state.ctx, 'exec_many', {
    mode: 'sequential',
    commands: [{ id: 'probe', program: nodeProgram, args: ['-e', 'process.stdout.write("ok")'] }]
  }, state.meta);
  assert.equal(result.ok, true);
  assert.equal(result.results[0].stdout, 'ok');
  const status = await callTool(state.ctx, 'harness_status', {}, state.meta);
  assert.equal(status.baseline_matches, false);
});
