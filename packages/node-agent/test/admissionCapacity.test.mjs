import test from 'node:test';
import assert from 'node:assert/strict';
import { mkdtemp, rm, writeFile } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import path from 'node:path';
import { createToolContext } from '../dist/server.js';
import { disposeProcessSessions, ProcessRequestLifecycle } from '../dist/processes.js';
import { callTool } from '../dist/tools.js';

function config(folders, dataDir, processConcurrency = 1) {
  return {
    host: '127.0.0.1', port: 0, dataDir, permissionMode: 'trusted',
    management: { enabled: false },
    oauth: { clientId: 'chatgpt', password: 'admission-test-password', tokenSecret: 'admission-test-token-secret' },
    folders,
    limits: {
      blockingConcurrency: 1,
      processConcurrency,
      globalBlockingConcurrency: 1,
      globalProcessConcurrency: processConcurrency,
      activeSessionLimit: 512,
      maxOutputBytes: 1024 * 1024
    }
  };
}

async function waitFor(read, timeoutMs = 2_000) {
  const started = Date.now();
  while (Date.now() - started < timeoutMs) {
    const value = read();
    if (value) return value;
    await new Promise(resolve => setTimeout(resolve, 5));
  }
  throw new Error('timed out waiting for admission state');
}

async function fixture(t, processConcurrency = 1) {
  const rootA = await mkdtemp(path.join(tmpdir(), 'ctmcp-admission-a-'));
  const rootB = await mkdtemp(path.join(tmpdir(), 'ctmcp-admission-b-'));
  const dataDir = await mkdtemp(path.join(tmpdir(), 'ctmcp-admission-data-'));
  await writeFile(path.join(rootA, 'value.txt'), 'a');
  await writeFile(path.join(rootB, 'value.txt'), 'b');
  const ctx = await createToolContext(config([
    { id: 'a', name: 'A', path: rootA },
    { id: 'b', name: 'B', path: rootB }
  ], dataDir, processConcurrency));
  t.after(async () => {
    await disposeProcessSessions(ctx);
    await Promise.allSettled([ctx.conversations.flush(), ctx.usageStore.flush()]);
    const remove = (target) => rm(target, { recursive: true, force: true, maxRetries: 5, retryDelay: 50 });
    await remove(rootA);
    await remove(rootB);
    await remove(dataDir);
  });
  return { ctx };
}

test('global admission is acquired before workspace capacity', async t => {
  const { ctx } = await fixture(t);
  const metaA = { 'openai/session': 'admission-a' };
  const metaB = { 'openai/session': 'admission-b' };
  await callTool(ctx, 'switch_workspace_folder', { folder_id: 'a' }, metaA);
  await callTool(ctx, 'switch_workspace_folder', { folder_id: 'b' }, metaB);

  const releaseGlobal = await ctx.hubAdmission.blocking.acquire();
  const requestA = callTool(ctx, 'read_file', { path: 'value.txt' }, metaA);
  const requestB = callTool(ctx, 'read_file', { path: 'value.txt' }, metaB);
  await waitFor(() => ctx.hubAdmission.blocking.queued === 2);

  assert.equal(ctx.folderRuntimes.get('a').admission.blocking.active, 0);
  assert.equal(ctx.folderRuntimes.get('b').admission.blocking.active, 0);
  assert.equal(ctx.folderRuntimes.get('a').admission.blocking.queued, 0);
  assert.equal(ctx.folderRuntimes.get('b').admission.blocking.queued, 0);

  releaseGlobal();
  const [resultA, resultB] = await Promise.all([requestA, requestB]);
  for (const result of [resultA, resultB]) {
    assert.equal(result.ok, true, JSON.stringify(result));
    assert.equal(result.admission_scope, 'global_and_workspace');
    assert.equal(result.admission_lane, 'blocking');
    assert.equal(typeof result.global_admission_wait_ms, 'number');
    assert.equal(typeof result.workspace_admission_wait_ms, 'number');
    assert.equal(result.admission_queue_wait_ms, result.global_admission_wait_ms + result.workspace_admission_wait_ms);
  }
  assert.equal(ctx.hubAdmission.blocking.active, 0);
  assert.equal(ctx.folderRuntimes.get('a').admission.blocking.active, 0);
  assert.equal(ctx.folderRuntimes.get('b').admission.blocking.active, 0);
});

test('cancelling an admission wait removes the waiter without releasing another permit', async t => {
  const { ctx } = await fixture(t);
  const meta = { 'openai/session': 'admission-cancel' };
  await callTool(ctx, 'switch_workspace_folder', { folder_id: 'a' }, meta);

  const releaseGlobal = await ctx.hubAdmission.blocking.acquire();
  const lifecycle = new ProcessRequestLifecycle(ctx);
  const pending = callTool(ctx, 'read_file', { path: 'value.txt' }, meta, false, lifecycle);
  await waitFor(() => ctx.hubAdmission.blocking.queued === 1);
  lifecycle.abort();
  const result = await pending;

  assert.equal(result.ok, false);
  assert.equal(ctx.hubAdmission.blocking.queued, 0);
  assert.equal(ctx.hubAdmission.blocking.active, 1);
  assert.equal(ctx.folderRuntimes.get('a').admission.blocking.active, 0);
  releaseGlobal();
  releaseGlobal();
  assert.equal(ctx.hubAdmission.blocking.active, 0);
});

test('exec_many orchestrator does not hold process admission and each child is admitted independently', async t => {
  const { ctx } = await fixture(t);
  const meta = { 'openai/session': 'exec-many-child-admission' };
  await callTool(ctx, 'switch_workspace_folder', { folder_id: 'a' }, meta);
  const runtime = ctx.folderRuntimes.get('a');
  const graph = await callTool(ctx, 'exec_many', {
    operation_id: 'exec-many-child-admission',
    mode: 'parallel',
    max_parallel: 2,
    yield_time_ms: 0,
    commands: [
      { id: 'one', program: 'node', args: ['-e', 'setTimeout(() => process.stdout.write("one"), 180)'], timeout_ms: 5_000 },
      { id: 'two', program: 'node', args: ['-e', 'setTimeout(() => process.stdout.write("two"), 180)'], timeout_ms: 5_000 }
    ]
  }, meta);

  assert.equal(graph.ok, true, JSON.stringify(graph));
  assert.equal(graph.graph_completed, false);
  assert.equal(graph.admission_mode, 'children');
  assert.equal(graph.admission_scope, 'none');
  await waitFor(() =>
    ctx.hubAdmission.process.active === 1
    && runtime.admission.process.active === 1
    && [...runtime.sessions.values()].filter(session => !session.finalizedAt).length === 1
  );
  assert.equal([...runtime.sessions.values()].filter(session => !session.finalizedAt).length, 1);

  const finalized = await callTool(ctx, 'exec_many', {
    operation_id: 'exec-many-child-admission',
    yield_time_ms: 2_000,
    result_mode: 'full'
  }, meta);
  assert.equal(finalized.graph_completed, true, JSON.stringify(finalized));
  assert.equal(finalized.graph_execution_ok, true, JSON.stringify(finalized));
  assert.equal(finalized.results.length, 2);
  assert.ok(finalized.results.every(result => result.admission_mode === 'child'));
  assert.ok(finalized.results.every(result => result.admission_scope === 'global_and_workspace'));
  assert.equal(ctx.hubAdmission.process.active, 0);
  assert.equal(runtime.admission.process.active, 0);
});

test('exec_many serializes io_heavy children independently of ordinary process concurrency', async t => {
  const { ctx } = await fixture(t, 2);
  const meta = { 'openai/session': 'exec-many-io-heavy' };
  await callTool(ctx, 'switch_workspace_folder', { folder_id: 'a' }, meta);
  const startedAt = Date.now();
  const graph = await callTool(ctx, 'exec_many', {
    operation_id: 'exec-many-io-heavy',
    mode: 'parallel',
    max_parallel: 2,
    yield_time_ms: 2_000,
    result_mode: 'full',
    commands: [
      { id: 'one', resource_class: 'io_heavy', program: 'node', args: ['-e', 'setTimeout(() => process.stdout.write("one"), 220)'], timeout_ms: 5_000 },
      { id: 'two', resource_class: 'io_heavy', program: 'node', args: ['-e', 'setTimeout(() => process.stdout.write("two"), 220)'], timeout_ms: 5_000 }
    ]
  }, meta);

  assert.equal(graph.graph_completed, true, JSON.stringify(graph));
  assert.equal(graph.graph_execution_ok, true, JSON.stringify(graph));
  assert.ok(Date.now() - startedAt >= 400, JSON.stringify(graph));
  assert.ok(graph.results.every(result => result.resource_class === 'io_heavy'));
  assert.ok(graph.results.every(result => result.io_heavy_admission_limit === 1));
  assert.ok(graph.results.some(result => result.io_heavy_admission_wait_ms >= 150), JSON.stringify(graph.results));
});
