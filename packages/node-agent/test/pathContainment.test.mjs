import test from 'node:test';
import assert from 'node:assert/strict';
import {
  lstat, mkdir, mkdtemp, readFile, realpath, rm, symlink, writeFile
} from 'node:fs/promises';
import { tmpdir } from 'node:os';
import path from 'node:path';
import { createToolContext } from '../dist/server.js';
import { callTool } from '../dist/tools.js';

const GIF_1X1 = Buffer.from('R0lGODlhAQABAIAAAAAAAP///ywAAAAAAQABAAACAUwAOw==', 'base64');
const nodeProgram = path.basename(process.execPath);

function config(root, dataDir) {
  return {
    host: '127.0.0.1', port: 0, dataDir, permissionMode: 'trusted',
    management: { enabled: false },
    oauth: { clientId: 'chatgpt', password: 'path-test-password', tokenSecret: 'path-test-token-secret' },
    folders: [{ id: 'repo', name: 'Repo', path: root }],
    limits: { blockingConcurrency: 4, processConcurrency: 4, activeSessionLimit: 16, maxOutputBytes: 1024 * 1024 }
  };
}

async function fixture(t) {
  const base = await mkdtemp(path.join(tmpdir(), 'ctmcp-path-containment-'));
  const root = path.join(base, 'root');
  const outside = path.join(base, 'outside');
  const dataDir = path.join(base, 'data');
  await mkdir(root);
  await mkdir(outside);
  t.after(() => rm(base, { recursive: true, force: true, maxRetries: 5, retryDelay: 50 }));
  const ctx = await createToolContext(config(root, dataDir));
  const meta = { 'openai/session': `path-${Date.now()}-${Math.random()}` };
  const selected = await callTool(ctx, 'switch_workspace_folder', { folder_id: 'repo' }, meta);
  assert.equal(selected.ok, true, JSON.stringify(selected));
  return { root, outside, ctx, meta };
}

async function createSymlinkOrSkip(t, target, link, type) {
  try {
    await symlink(target, link, type);
    return true;
  } catch (error) {
    if (error?.code === 'EPERM' || error?.code === 'EACCES') {
      t.skip(`symbolic links are unavailable: ${error.code}`);
      return false;
    }
    throw error;
  }
}

function expectPathError(result, code) {
  assert.equal(result.ok, false, JSON.stringify(result));
  assert.equal(result.error?.code, code, JSON.stringify(result));
  assert.equal(result.error?.category, 'security', JSON.stringify(result));
  assert.equal(result.error?.retryable, false, JSON.stringify(result));
}

test('absolute and parent paths are rejected before filesystem access', async t => {
  const { root, ctx, meta } = await fixture(t);
  await writeFile(path.join(root, 'inside.txt'), 'inside\n');
  const requests = [
    [path.join(root, 'inside.txt'), 'ABSOLUTE_PATH_DENIED'],
    ['/tmp/inside.txt', 'ABSOLUTE_PATH_DENIED'],
    ['C:relative.txt', 'ABSOLUTE_PATH_DENIED'],
    [String.raw`\\server\share\inside.txt`, 'ABSOLUTE_PATH_DENIED'],
    [String.raw`\\?\C:\inside.txt`, 'ABSOLUTE_PATH_DENIED'],
    ['../outside.txt', 'PATH_OUTSIDE_WORKSPACE'],
    ['nested/../inside.txt', 'PATH_OUTSIDE_WORKSPACE']
  ];
  const absoluteDiff = await callTool(ctx, 'git_diff', { paths: [path.join(root, 'inside.txt')] }, meta);
  expectPathError(absoluteDiff, 'ABSOLUTE_PATH_DENIED');
  const protectedStage = await callTool(ctx, 'git_stage', {
    paths: ['.github/workflows/blocked.yml'], dry_run: true
  }, meta);
  expectPathError(protectedStage, 'PROTECTED_PATH');

  for (const [pathValue, code] of requests) {
    const result = await callTool(ctx, 'read_file', { path: pathValue }, meta);
    expectPathError(result, code);
  }
});

test('direct external symlinks cannot be read, edited, batch-read, or viewed as images', async t => {
  const { root, outside, ctx, meta } = await fixture(t);
  const outsideText = path.join(outside, 'outside.txt');
  const outsideImage = path.join(outside, 'outside.gif');
  await writeFile(outsideText, 'outside-before\n');
  await writeFile(outsideImage, GIF_1X1);
  if (!await createSymlinkOrSkip(t, outsideText, path.join(root, 'text-link.txt'), 'file')) return;
  if (!await createSymlinkOrSkip(t, outsideImage, path.join(root, 'image-link.gif'), 'file')) return;

  expectPathError(await callTool(ctx, 'read_file', { path: 'text-link.txt' }, meta), 'SYMLINK_ESCAPE');
  expectPathError(await callTool(ctx, 'git_blame', { path: 'text-link.txt' }, meta), 'SYMLINK_ESCAPE');

  const batch = await callTool(ctx, 'read_many', { items: [{ path: 'text-link.txt' }] }, meta);
  assert.equal(batch.ok, true, JSON.stringify(batch));
  assert.equal(batch.failed_count, 1);
  assert.equal(batch.results[0].error.code, 'SYMLINK_ESCAPE');
  assert.equal(batch.results[0].error.category, 'security');

  const edit = await callTool(ctx, 'edit_file', {
    path: 'text-link.txt', edits: [{ type: 'replace', old_text: 'outside-before', new_text: 'outside-after' }]
  }, meta);
  expectPathError(edit, 'SYMLINK_ESCAPE');
  assert.equal(await readFile(outsideText, 'utf8'), 'outside-before\n');

  const image = await callTool(ctx, 'view_image', { path: 'image-link.gif', output: 'data_url' }, meta);
  expectPathError(image, 'SYMLINK_ESCAPE');
  assert.deepEqual(await readFile(outsideImage), GIF_1X1);
});

test('ancestor symlinks cannot redirect reads, edits, scans, or create operations', async t => {
  const { root, outside, ctx, meta } = await fixture(t);
  await writeFile(path.join(outside, 'nested.txt'), 'outside-before\n');
  if (!await createSymlinkOrSkip(t, outside, path.join(root, 'outside-dir'), 'dir')) return;

  for (const [tool, args] of [
    ['read_file', { path: 'outside-dir/nested.txt' }],
    ['project_map', { path: 'outside-dir' }],
    ['list_files', { path: 'outside-dir' }],
    ['search_text', { path: 'outside-dir', query: 'outside' }],
    ['git_status', { path: 'outside-dir' }]
  ]) expectPathError(await callTool(ctx, tool, args, meta), 'SYMLINK_ESCAPE');

  expectPathError(await callTool(ctx, 'set_default_cwd', { path: 'outside-dir' }, meta), 'SYMLINK_ESCAPE');
  const escapedCommand = await callTool(ctx, 'exec_command', {
    program: nodeProgram,
    args: ['-e', "require('node:fs').writeFileSync('escaped-command.txt','escaped')"],
    workdir: 'outside-dir',
    yield_time_ms: 10_000
  }, meta);
  expectPathError(escapedCommand, 'SYMLINK_ESCAPE');
  await assert.rejects(readFile(path.join(outside, 'escaped-command.txt')), { code: 'ENOENT' });

  const edit = await callTool(ctx, 'edit_file', {
    path: 'outside-dir/nested.txt', edits: [{ type: 'replace', old_text: 'outside-before', new_text: 'outside-after' }]
  }, meta);
  expectPathError(edit, 'SYMLINK_ESCAPE');
  assert.equal(await readFile(path.join(outside, 'nested.txt'), 'utf8'), 'outside-before\n');

  const escapedCreate = await callTool(ctx, 'file_ops', {
    confirm: true,
    operations: [{ type: 'create', path: 'outside-dir/new.txt', content: 'escaped' }]
  }, meta);
  expectPathError(escapedCreate, 'SYMLINK_ESCAPE');
  await assert.rejects(readFile(path.join(outside, 'new.txt')), { code: 'ENOENT' });

  const safeCreate = await callTool(ctx, 'file_ops', {
    confirm: true,
    operations: [{ type: 'create', path: 'safe/deep/new.txt', content: 'safe' }]
  }, meta);
  assert.equal(safeCreate.ok, true, JSON.stringify(safeCreate));
  assert.equal(await readFile(path.join(root, 'safe', 'deep', 'new.txt'), 'utf8'), 'safe');
});

test('direct in-workspace symlinks are readable and formattable but text mutations remain blocked', async t => {
  const { root, ctx, meta } = await fixture(t);
  const target = path.join(root, 'target.txt');
  const link = path.join(root, 'inside-link.txt');
  const jsonTarget = path.join(root, 'target.json');
  const jsonLink = path.join(root, 'inside-link.json');
  await writeFile(target, 'before\n');
  await writeFile(jsonTarget, '{"value":1}\n');
  if (!await createSymlinkOrSkip(t, target, link, 'file')) return;
  if (!await createSymlinkOrSkip(t, jsonTarget, jsonLink, 'file')) return;

  const read = await callTool(ctx, 'read_file', { path: 'inside-link.txt' }, meta);
  assert.equal(read.ok, true, JSON.stringify(read));
  assert.equal(read.path, 'target.txt');
  assert.equal(read.content, 'before\n');

  expectPathError(await callTool(ctx, 'edit_file', {
    path: 'inside-link.txt', edits: [{ type: 'replace', old_text: 'before', new_text: 'after' }]
  }, meta), 'SYMLINK_ESCAPE');
  expectPathError(await callTool(ctx, 'edit_many', {
    files: [{ path: 'inside-link.txt', edits: [{ type: 'replace', old_text: 'before', new_text: 'after' }] }]
  }, meta), 'SYMLINK_ESCAPE');
  expectPathError(await callTool(ctx, 'file_ops', {
    confirm: true, operations: [{ type: 'delete', path: 'inside-link.txt' }]
  }, meta), 'SYMLINK_ESCAPE');
  assert.equal(await readFile(target, 'utf8'), 'before\n');
  assert.equal((await lstat(link)).isSymbolicLink(), true);

  const formatted = await callTool(ctx, 'format_files', {
    mode: 'apply', paths: ['inside-link.json'], formatter: 'builtin-json'
  }, meta);
  assert.equal(formatted.ok, true, JSON.stringify(formatted));
  assert.deepEqual(formatted.files_changed, ['target.json']);
  assert.equal(await readFile(jsonTarget, 'utf8'), '{\n  "value": 1\n}\n');
  assert.equal((await lstat(jsonLink)).isSymbolicLink(), true);
});

test('safe in-workspace ancestor symlinks resolve to canonical targets for reads and mutations', async t => {
  const { root, ctx, meta } = await fixture(t);
  const realDirectory = path.join(root, 'real-dir');
  const aliasDirectory = path.join(root, 'alias-dir');
  await mkdir(realDirectory);
  // Windows temp paths may use 8.3 aliases; runtime results are canonical.
  const canonicalRealDirectory = await realpath(realDirectory);
  await writeFile(path.join(realDirectory, 'value.txt'), 'before\n');
  await writeFile(path.join(realDirectory, 'config.json'), '{"enabled":true}\n');
  if (!await createSymlinkOrSkip(t, realDirectory, aliasDirectory, 'dir')) return;

  const selectedCwd = await callTool(ctx, 'set_default_cwd', { path: 'alias-dir' }, meta);
  assert.equal(selectedCwd.ok, true, JSON.stringify(selectedCwd));
  assert.equal(selectedCwd.default_cwd, 'real-dir');
  assert.equal(selectedCwd.resolved_cwd, canonicalRealDirectory);
  let command = await callTool(ctx, 'exec_command', {
    program: nodeProgram,
    args: ['-e', 'process.stdout.write(process.cwd())'],
    yield_time_ms: 10_000
  }, meta);
  if (command.process_still_running) {
    command = await callTool(ctx, 'wait_command', {
      session_id: command.session_id,
      cursor: command.next_cursor,
      timeout_ms: 10_000,
      until: 'finalized',
      output_mode: 'all'
    }, meta);
  }
  assert.equal(command.ok, true, JSON.stringify(command));
  assert.equal(command.command_ok, true, JSON.stringify(command));
  assert.equal(command.cwd, canonicalRealDirectory);
  assert.equal(command.stdout, canonicalRealDirectory);

  const read = await callTool(ctx, 'read_file', { path: 'alias-dir/value.txt' }, meta);
  assert.equal(read.ok, true, JSON.stringify(read));
  assert.equal(read.path, 'real-dir/value.txt');

  const edit = await callTool(ctx, 'edit_file', {
    path: 'alias-dir/value.txt', edits: [{ type: 'replace', old_text: 'before', new_text: 'after' }]
  }, meta);
  assert.equal(edit.ok, true, JSON.stringify(edit));
  assert.equal(edit.path, 'real-dir/value.txt');
  assert.equal(await readFile(path.join(realDirectory, 'value.txt'), 'utf8'), 'after\n');

  const created = await callTool(ctx, 'file_ops', {
    confirm: true, operations: [{ type: 'create', path: 'alias-dir/new.txt', content: 'created' }]
  }, meta);
  assert.equal(created.ok, true, JSON.stringify(created));
  assert.equal(await readFile(path.join(realDirectory, 'new.txt'), 'utf8'), 'created');

  const formatted = await callTool(ctx, 'format_files', {
    mode: 'apply', paths: ['alias-dir/config.json'], formatter: 'builtin-json'
  }, meta);
  assert.equal(formatted.ok, true, JSON.stringify(formatted));
  assert.deepEqual(formatted.files_changed, ['real-dir/config.json']);
  assert.equal(await readFile(path.join(realDirectory, 'config.json'), 'utf8'), '{\n  "enabled": true\n}\n');
  assert.equal((await lstat(aliasDirectory)).isSymbolicLink(), true);
});

test('patch tools reject absolute, parent, direct-symlink, and ancestor-symlink targets', async t => {
  const { root, outside, ctx, meta } = await fixture(t);
  const outsideText = path.join(outside, 'outside.txt');
  await writeFile(outsideText, 'outside-before\n');
  if (!await createSymlinkOrSkip(t, outsideText, path.join(root, 'patch-link.txt'), 'file')) return;
  if (!await createSymlinkOrSkip(t, outside, path.join(root, 'outside-dir'), 'dir')) return;

  const directPatch = [
    '--- a/patch-link.txt', '+++ b/patch-link.txt', '@@ -1 +1 @@',
    '-outside-before', '+outside-after', ''
  ].join('\n');
  expectPathError(await callTool(ctx, 'patch_check', { patch: directPatch }, meta), 'SYMLINK_ESCAPE');
  expectPathError(await callTool(ctx, 'apply_patch', { patch: directPatch }, meta), 'SYMLINK_ESCAPE');

  const ancestorPatch = [
    '--- /dev/null', '+++ b/outside-dir/new-patch.txt', '@@ -0,0 +1 @@', '+escaped', ''
  ].join('\n');
  expectPathError(await callTool(ctx, 'patch_check', { patch: ancestorPatch }, meta), 'SYMLINK_ESCAPE');

  const parentPatch = [
    '--- a/../outside/outside.txt', '+++ b/../outside/outside.txt', '@@ -1 +1 @@',
    '-outside-before', '+outside-after', ''
  ].join('\n');
  expectPathError(await callTool(ctx, 'patch_check', { patch: parentPatch }, meta), 'PATH_OUTSIDE_WORKSPACE');

  const absolutePatch = [
    '--- C:/outside.txt', '+++ C:/outside.txt', '@@ -1 +1 @@', '-before', '+after', ''
  ].join('\n');
  expectPathError(await callTool(ctx, 'patch_check', { patch: absolutePatch }, meta), 'ABSOLUTE_PATH_DENIED');

  assert.equal(await readFile(outsideText, 'utf8'), 'outside-before\n');
  await assert.rejects(readFile(path.join(outside, 'new-patch.txt')), { code: 'ENOENT' });
});
