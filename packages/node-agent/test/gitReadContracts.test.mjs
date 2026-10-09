import test from 'node:test';
import assert from 'node:assert/strict';
import { execFile as execFileCallback } from 'node:child_process';
import { mkdir, mkdtemp, rm, writeFile } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import path from 'node:path';
import { promisify } from 'node:util';
import { createToolContext } from '../dist/server.js';
import { callTool } from '../dist/tools.js';

const execFile = promisify(execFileCallback);

function config(root, dataDir) {
  return {
    host: '127.0.0.1', port: 0, dataDir, permissionMode: 'trusted',
    management: { enabled: false },
    oauth: { clientId: 'chatgpt', password: 'git-read-test-password', tokenSecret: 'git-read-test-token-secret' },
    folders: [{ id: 'repo', name: 'Repo', path: root }],
    limits: { blockingConcurrency: 4, processConcurrency: 4, activeSessionLimit: 16, maxOutputBytes: 4 * 1024 * 1024 }
  };
}

async function git(cwd, ...args) {
  const environment = { ...process.env, GIT_TERMINAL_PROMPT: '0' };
  // Fixture commits must use each temporary repository's configured identity.
  for (const key of ['GIT_AUTHOR_NAME', 'GIT_AUTHOR_EMAIL', 'GIT_COMMITTER_NAME', 'GIT_COMMITTER_EMAIL']) delete environment[key];
  const result = await execFile('git', args, {
    cwd,
    encoding: 'utf8',
    env: environment
  });
  return result.stdout.trim();
}

async function selectedContext(t) {
  const root = await mkdtemp(path.join(tmpdir(), 'ctmcp-git-read-root-'));
  const dataDir = await mkdtemp(path.join(tmpdir(), 'ctmcp-git-read-data-'));
  t.after(async () => {
    await rm(root, { recursive: true, force: true, maxRetries: 5, retryDelay: 50 });
    await rm(dataDir, { recursive: true, force: true, maxRetries: 5, retryDelay: 50 });
  });
  const ctx = await createToolContext(config(root, dataDir));
  const meta = { 'openai/session': `git-read-${Date.now()}-${Math.random()}` };
  const selected = await callTool(ctx, 'switch_workspace_folder', { folder_id: 'repo' }, meta);
  assert.equal(selected.ok, true);
  return { root, dataDir, ctx, meta };
}

async function repositoryFixture(t) {
  const state = await selectedContext(t);
  const remote = await mkdtemp(path.join(tmpdir(), 'ctmcp-git-read-remote-'));
  const peer = await mkdtemp(path.join(tmpdir(), 'ctmcp-git-read-peer-'));
  t.after(async () => {
    await rm(remote, { recursive: true, force: true, maxRetries: 5, retryDelay: 50 });
    await rm(peer, { recursive: true, force: true, maxRetries: 5, retryDelay: 50 });
  });

  await git(remote, 'init', '--bare');
  await git(remote, 'symbolic-ref', 'HEAD', 'refs/heads/main');
  await git(state.root, 'init');
  await git(state.root, 'config', 'user.name', 'Node Agent Test');
  await git(state.root, 'config', 'user.email', 'node-agent@example.invalid');
  await writeFile(path.join(state.root, 'tracked.txt'), 'alpha\nbeta\ngamma\n');
  await writeFile(path.join(state.root, 'blame.txt'), 'one\ntwo\nthree\n');
  await writeFile(path.join(state.root, 'rename-source.txt'), 'rename me\n');
  await writeFile(path.join(state.root, 'staged-only.txt'), 'stage base\n');
  await writeFile(path.join(state.root, 'both.txt'), 'both base\n');
  await writeFile(path.join(state.root, 'large.txt'), 'large base\n');
  await git(state.root, 'add', '-A');
  await git(state.root, 'commit', '-m', 'initial');
  await git(state.root, 'branch', '-M', 'main');
  await git(state.root, 'remote', 'add', 'origin', remote);
  await git(state.root, 'push', '-u', 'origin', 'main');

  await git(peer, 'clone', remote, '.');
  await git(peer, 'config', 'user.name', 'Remote Test');
  await git(peer, 'config', 'user.email', 'remote@example.invalid');
  await writeFile(path.join(peer, 'remote-only.txt'), 'remote\n');
  await git(peer, 'add', '--', 'remote-only.txt');
  await git(peer, 'commit', '-m', 'remote ahead');
  await git(peer, 'push', 'origin', 'main');

  await writeFile(path.join(state.root, 'blame.txt'), 'one\ntwo changed\nthree\n');
  await writeFile(path.join(state.root, 'local-only.txt'), 'local\n');
  await git(state.root, 'add', '--', 'blame.txt', 'local-only.txt');
  await git(state.root, 'commit', '-m', 'local ahead');
  await git(state.root, 'fetch', 'origin');
  const head = await git(state.root, 'rev-parse', 'HEAD');

  await git(state.root, 'mv', 'rename-source.txt', 'renamed.txt');
  await writeFile(path.join(state.root, 'tracked.txt'), 'alpha\nbeta worktree\ngamma\n');
  await writeFile(path.join(state.root, 'staged-only.txt'), 'stage changed\n');
  await git(state.root, 'add', '--', 'staged-only.txt');
  await writeFile(path.join(state.root, 'both.txt'), 'both staged\n');
  await git(state.root, 'add', '--', 'both.txt');
  await writeFile(path.join(state.root, 'both.txt'), 'both worktree\n');
  await writeFile(path.join(state.root, 'large.txt'), `${'0123456789abcdef'.repeat(800)}\n`);
  await writeFile(path.join(state.root, 'untracked.txt'), 'untracked\n');
  await mkdir(path.join(state.root, 'folder'));

  return { ...state, head };
}

async function tool(state, name, args = {}) {
  return callTool(state.ctx, name, args, state.meta);
}

test('git_status reports Rust branch tracking, clean state and rename metadata', async t => {
  const state = await repositoryFixture(t);
  const status = await tool(state, 'git_status');
  assert.equal(status.ok, true, JSON.stringify(status));
  assert.equal(status.is_repo, true);
  assert.equal(status.branch, 'main');
  assert.equal(status.upstream, 'origin/main');
  assert.equal(status.ahead, 1);
  assert.equal(status.behind, 1);
  assert.equal(status.clean, false);
  assert.equal(status.head, state.head);
  assert.deepEqual(status.warnings, []);
  const renamed = status.entries.find(entry => entry.path === 'renamed.txt');
  assert.ok(renamed, JSON.stringify(status.entries));
  assert.equal(renamed.original_path, 'rename-source.txt');
  assert.equal(renamed.index_status, 'R');
  assert.ok(status.entries.some(entry => entry.path === 'untracked.txt'));

  const trackedOnly = await tool(state, 'git_status', { include_untracked: false });
  assert.equal(trackedOnly.entries.some(entry => entry.path === 'untracked.txt'), false);

  const limited = await tool(state, 'git_status', { max_entries: 1 });
  assert.equal(limited.entries.length, 1);
  assert.equal(limited.truncated, true);
});

test('git_diff combines unstaged and staged output with normalized metadata and files', async t => {
  const state = await repositoryFixture(t);
  const combined = await tool(state, 'git_diff', {
    staged: true,
    unstaged: true,
    paths: ['tracked.txt', 'staged-only.txt', 'both.txt'],
    context_lines: 100,
    max_bytes: 1024 * 1024
  });
  assert.equal(combined.ok, true, JSON.stringify(combined));
  assert.match(combined.diff, /beta worktree/);
  assert.match(combined.diff, /stage changed/);
  assert.match(combined.diff, /both worktree/);
  assert.match(combined.diff, /both staged/);
  assert.equal(combined.arguments_normalized, true);
  assert.deepEqual(combined.normalized_arguments, { context_lines: 20 });
  assert.equal(combined.truncated, false);
  assert.deepEqual(combined.warnings, []);
  assert.ok(combined.files.length >= 3);
  assert.ok(combined.files.every(file => file.status === 'modified' && file.binary === false));
  assert.ok(combined.files.some(file => file.path === 'tracked.txt'));
  assert.ok(combined.files.some(file => file.path === 'staged-only.txt'));

  const truncated = await tool(state, 'git_diff', {
    paths: ['large.txt'],
    max_bytes: 1024
  });
  assert.equal(truncated.truncated, true);
  assert.deepEqual(truncated.warnings, ['diff truncated']);
  assert.ok(Buffer.byteLength(truncated.diff) <= 1026);
  assert.ok(truncated.bytes > 1024);

  const disabled = await tool(state, 'git_diff', { staged: false, unstaged: false });
  assert.equal(disabled.diff, '');
  assert.deepEqual(disabled.files, []);

  const escaped = await tool(state, 'git_diff', { paths: ['../outside.txt'] });
  assert.equal(escaped.ok, false);
  assert.equal(escaped.error.code, 'PATH_OUTSIDE_WORKSPACE');
});

test('git_log and git_show expose bounded Rust-compatible revision metadata', async t => {
  const state = await repositoryFixture(t);
  const log = await tool(state, 'git_log', { ref: 'HEAD', path: '.', max_count: 1 });
  assert.equal(log.ok, true, JSON.stringify(log));
  assert.equal(log.is_repo, true);
  assert.equal(log.ref, 'HEAD');
  assert.equal(log.path, '.');
  assert.equal(log.commits.length, 1);
  assert.equal(log.commits[0].hash, state.head);
  assert.match(log.commits[0].short_hash, /^[0-9a-f]+$/);
  assert.equal(log.commits[0].author_name, 'Node Agent Test');
  assert.equal(log.commits[0].author_email, 'node-agent@example.invalid');
  assert.match(log.commits[0].author_date, /^\d{4}-\d{2}-\d{2}T/);
  assert.equal(log.commits[0].authored_at, log.commits[0].author_date);
  assert.equal(log.truncated, true);
  assert.deepEqual(log.warnings, ['commit limit reached']);

  const pathLog = await tool(state, 'git_log', { path: 'blame.txt', max_count: 10 });
  assert.equal(pathLog.path, 'blame.txt');
  assert.ok(pathLog.commits.length >= 2);
  assert.equal(pathLog.truncated, false);

  const invalidLog = await tool(state, 'git_log', { ref: '--all' });
  assert.equal(invalidLog.ok, false);
  assert.equal(invalidLog.error.code, 'INVALID_ARGUMENT');

  const shown = await tool(state, 'git_show', {
    rev: 'HEAD',
    context_lines: 100,
    max_bytes: 1024 * 1024
  });
  assert.equal(shown.ok, true, JSON.stringify(shown));
  assert.equal(shown.is_repo, true);
  assert.equal(shown.rev, 'HEAD');
  assert.match(shown.content, /^commit /);
  assert.equal(shown.output, shown.content);
  assert.equal(shown.arguments_normalized, true);
  assert.deepEqual(shown.normalized_arguments, { context_lines: 20 });
  assert.ok(shown.files.some(file => file.path === 'blame.txt'));
  assert.equal(shown.output_bytes, Buffer.byteLength(shown.content));
  assert.ok(shown.bytes >= shown.output_bytes);

  const metadataOnly = await tool(state, 'git_show', { include_diff: false });
  assert.match(metadataOnly.content, /^commit /);
  assert.deepEqual(metadataOnly.files, []);

  const truncatedShow = await tool(state, 'git_show', { max_bytes: 40 });
  assert.equal(truncatedShow.truncated, true);
  assert.deepEqual(truncatedShow.warnings, ['output truncated']);
  assert.equal(truncatedShow.output_bytes, Buffer.byteLength(truncatedShow.content));

  const invalidShow = await tool(state, 'git_show', { rev: '-bad' });
  assert.equal(invalidShow.ok, false);
  assert.equal(invalidShow.error.code, 'INVALID_ARGUMENT');
});

test('git_blame returns bounded structured line records and stable validation', async t => {
  const state = await repositoryFixture(t);
  const blame = await tool(state, 'git_blame', {
    path: 'blame.txt',
    rev: 'HEAD',
    start_line: 1,
    end_line: 3,
    max_lines: 2
  });
  assert.equal(blame.ok, true, JSON.stringify(blame));
  assert.equal(blame.is_repo, true);
  assert.equal(blame.path, 'blame.txt');
  assert.equal(blame.rev, 'HEAD');
  assert.equal(blame.start_line, 1);
  assert.equal(blame.end_line, 2);
  assert.equal(blame.lines.length, 2);
  assert.equal(blame.truncated, true);
  assert.deepEqual(blame.warnings, ['line limit reached']);
  assert.deepEqual(blame.lines.map(line => line.content), ['one', 'two changed']);
  for (const line of blame.lines) {
    assert.match(line.commit, /^[0-9a-f]{40}$/);
    assert.equal(typeof line.original_line, 'number');
    assert.equal(typeof line.line, 'number');
    assert.equal(typeof line.author, 'string');
    assert.equal(typeof line.author_mail, 'string');
    assert.equal(typeof line.author_time, 'number');
    assert.equal(typeof line.summary, 'string');
  }

  const invalidRange = await tool(state, 'git_blame', { path: 'blame.txt', start_line: 3, end_line: 2 });
  assert.equal(invalidRange.ok, false);
  assert.equal(invalidRange.error.code, 'INVALID_ARGUMENT');

  const directory = await tool(state, 'git_blame', { path: 'folder' });
  assert.equal(directory.ok, false);
  assert.equal(directory.error.code, 'IS_DIRECTORY');
});

test('Git read tools return stable non-repository contracts', async t => {
  const state = await selectedContext(t);
  await writeFile(path.join(state.root, 'plain.txt'), 'plain\n');

  const status = await tool(state, 'git_status');
  assert.equal(status.is_repo, false);
  assert.equal(status.clean, true);
  assert.deepEqual(status.entries, []);
  assert.equal(status.warnings.length, 1);

  const diff = await tool(state, 'git_diff');
  assert.equal(diff.diff, '');
  assert.deepEqual(diff.files, []);
  assert.equal(diff.truncated, false);
  assert.deepEqual(diff.warnings, ['not a git repository']);

  const log = await tool(state, 'git_log');
  assert.equal(log.is_repo, false);
  assert.deepEqual(log.commits, []);
  assert.equal(log.truncated, false);

  const show = await tool(state, 'git_show');
  assert.equal(show.is_repo, false);
  assert.equal(show.content, '');
  assert.deepEqual(show.files, []);

  const blame = await tool(state, 'git_blame', { path: 'plain.txt' });
  assert.equal(blame.is_repo, false);
  assert.equal(blame.path, 'plain.txt');
  assert.deepEqual(blame.lines, []);
  assert.equal(blame.truncated, false);
});
