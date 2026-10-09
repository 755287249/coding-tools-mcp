import test from 'node:test';
import assert from 'node:assert/strict';
import { createHash } from 'node:crypto';
import { EventEmitter } from 'node:events';
import { request as httpRequest } from 'node:http';
import { mkdtemp, readFile, rm, stat, writeFile } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import path from 'node:path';
import {
  attachHarnessOperation,
  cargoTargetLock,
  nodeGeneratedLock,
  DETACHED_SESSION_GRACE_MS,
  disposeProcessSessions,
  FINALIZED_SESSION_RETENTION_MS,
  MAX_RETAINED_FINALIZED_SESSIONS,
  ProcessRequestLifecycle,
  pruneProcessSessions,
  resolvedCommandTimeoutMs,
  startAndYield,
  waitForSession,
  killProcessTree
} from '../dist/processes.js';
import { runtimeForFolderId } from '../dist/folderRuntime.js';
import { createAgentRuntime, createToolContext } from '../dist/server.js';
import { callTool } from '../dist/tools.js';
import {
  managedWslcSessionStorage,
  wslcHostAvailable
} from '../dist/sandboxWslcProvisioner.js';

test('process execution identity is owned by a dedicated module behind the processes facade', async () => {
  const identity = await import('../dist/processes/identity.js');
  assert.equal(identity.cargoTargetLock, cargoTargetLock);
  assert.equal(identity.nodeGeneratedLock, nodeGeneratedLock);
  assert.equal(typeof identity.commandFingerprint, 'function');
  assert.equal(typeof identity.safeAutomaticDedup, 'function');
});

test('process timeout policy is owned by a dedicated module behind the processes facade', async () => {
  const timeoutPolicy = await import('../dist/processes/timeoutPolicy.js');
  assert.equal(timeoutPolicy.resolvedCommandTimeoutMs, resolvedCommandTimeoutMs);
  assert.equal(typeof timeoutPolicy.boundedInteger, 'function');
  assert.equal(typeof timeoutPolicy.commandTimeoutMaxMs, 'function');
});
test('process environment policy is owned by a dedicated module', async () => {
  const environment = await import('../dist/processes/environment.js');
  assert.equal(typeof environment.commandEnvironment, 'function');
  assert.equal(typeof environment.explicitEnvironment, 'function');
  assert.equal(typeof environment.removedEnvironment, 'function');
});
test('retained process session registry is owned by a dedicated module behind the processes facade', async () => {
  const registry = await import('../dist/processes/sessionRegistry.js');
  assert.equal(registry.pruneProcessSessions, pruneProcessSessions);
  assert.equal(registry.MAX_RETAINED_FINALIZED_SESSIONS, MAX_RETAINED_FINALIZED_SESSIONS);
  assert.equal(registry.FINALIZED_SESSION_RETENTION_MS, FINALIZED_SESSION_RETENTION_MS);
  assert.equal(typeof registry.requireProcessSession, 'function');
  assert.equal(typeof registry.findProcessOperation, 'function');
});
test('process harness operation tracking is owned by a dedicated module behind the processes facade', async () => {
  const tracking = await import('../dist/processes/harnessTracking.js');
  assert.equal(tracking.attachHarnessOperation, attachHarnessOperation);
  assert.equal(typeof tracking.recordHarnessOperationFinalization, 'function');
});
test('process output module owns retained recovery action projection', async () => {
  const output = await import('../dist/processes/output.js');
  assert.equal(typeof output.retainedNextActions, 'function');
});
test('process post-check orchestration is owned by a dedicated module', async () => {
  const postChecks = await import('../dist/processes/postChecks.js');
  assert.equal(typeof postChecks.runProcessPostChecks, 'function');
});

const nodeProgram = path.basename(process.execPath);

function repoRuntime(ctx) {
  return runtimeForFolderId(ctx, 'repo');
}

test('known build and packaging commands inherit the configured long timeout when omitted', () => {
  const max = 30 * 60_000;
  assert.equal(resolvedCommandTimeoutMs({}, 'cmd.exe /d /s /c npm run desktop:portable', max), max);
  assert.equal(resolvedCommandTimeoutMs({}, 'npm --prefix packages/node-agent run build:server', max), max);
  assert.equal(resolvedCommandTimeoutMs({}, 'npm run node-agent:parity:check', max), max);
  assert.equal(resolvedCommandTimeoutMs({}, 'npm --prefix packages/node-agent run sync:rust-contract', max), max);
  assert.equal(resolvedCommandTimeoutMs({}, 'pnpm desktop:portable', max), max);
  assert.equal(resolvedCommandTimeoutMs({}, 'pnpm node-agent:build', max), max);
  assert.equal(resolvedCommandTimeoutMs({}, 'cargo test --manifest-path src-tauri/Cargo.toml', max), max);
  assert.equal(resolvedCommandTimeoutMs({}, 'cargo clippy --manifest-path src-tauri/Cargo.toml', max), max);
  assert.equal(resolvedCommandTimeoutMs({}, 'node scripts/check.mjs', max), 30_000);
  assert.equal(resolvedCommandTimeoutMs({ timeout_ms: 600_000 }, 'npm run desktop:portable', max), 600_000);
});

test('Cargo resource locks follow resolved target directories across worktrees', () => {
  const cargo = { program: 'cargo', argv: ['check'], display: 'cargo check', shell: false };
  const main = path.resolve('C:/repo/main');
  const linked = path.resolve('C:/repo/main/.worktrees/feature');
  const mainLock = cargoTargetLock(main, cargo, {});
  const linkedLock = cargoTargetLock(linked, cargo, {});
  assert.equal(mainLock?.target, path.resolve(main, 'target'));
  assert.equal(linkedLock?.target, path.resolve(linked, 'target'));
  assert.notEqual(mainLock?.group, linkedLock?.group);

  const sharedTarget = path.resolve('C:/cargo/shared-target');
  const sharedMain = cargoTargetLock(main, cargo, { env: { CARGO_TARGET_DIR: sharedTarget } });
  const sharedLinked = cargoTargetLock(linked, cargo, { env: { cargo_target_dir: sharedTarget } });
  assert.equal(sharedMain?.target, sharedTarget);
  assert.equal(sharedLinked?.target, sharedTarget);
  assert.equal(sharedMain?.group, sharedLinked?.group);

  const manifest = cargoTargetLock(main, {
    program: 'cargo',
    argv: ['check', '--manifest-path', 'src-tauri/Cargo.toml'],
    display: 'cargo check --manifest-path src-tauri/Cargo.toml',
    shell: false
  }, {});
  assert.equal(manifest?.target, path.resolve(main, 'src-tauri', 'target'));
  assert.equal(cargoTargetLock(main, { program: nodeProgram, argv: [], display: nodeProgram, shell: false }, {}), undefined);
});

test('Node Agent build and tests infer the same generated-output lock', () => {
  const root = path.resolve('C:/repo/main');
  const build = nodeGeneratedLock(root, {
    program: 'pnpm',
    argv: ['--filter', '@coding-tools/node-agent', 'run', 'build:server'],
    display: 'pnpm --filter @coding-tools/node-agent run build:server',
    shell: false
  });
  const tests = nodeGeneratedLock(root, {
    program: nodeProgram,
    argv: ['--import', './packages/node-agent/test/setup.mjs', '--test', 'packages/node-agent/test/workspaceConversation.test.mjs'],
    display: `${nodeProgram} --import ./packages/node-agent/test/setup.mjs --test packages/node-agent/test/workspaceConversation.test.mjs`,
    shell: false
  });
  assert.equal(build?.target, path.resolve(root, 'packages/node-agent/dist'));
  assert.equal(tests?.target, build?.target);
  assert.equal(tests?.group, build?.group);

  const unrelated = nodeGeneratedLock(root, {
    program: nodeProgram,
    argv: ['scripts/check.mjs'],
    display: `${nodeProgram} scripts/check.mjs`,
    shell: false
  });
  assert.equal(unrelated, undefined);
});

function config(root, dataDir, maxOutputBytes = 1024 * 1024) {
  return {
    host: '127.0.0.1', port: 0, dataDir, permissionMode: 'trusted',
    management: { enabled: false },
    oauth: { clientId: 'chatgpt', password: 'process-test-password', tokenSecret: 'process-test-token-secret' },
    folders: [{ id: 'repo', name: 'Repo', path: root }],
    limits: { blockingConcurrency: 4, processConcurrency: 4, activeSessionLimit: 32, maxOutputBytes }
  };
}

async function fixture(t, maxOutputBytes) {
  const root = await mkdtemp(path.join(tmpdir(), 'ctmcp-process-root-'));
  const dataDir = await mkdtemp(path.join(tmpdir(), 'ctmcp-process-data-'));
  const ctx = await createToolContext(config(root, dataDir, maxOutputBytes));
  const meta = { 'openai/session': `process-${Date.now()}-${Math.random()}` };
  const selected = await callTool(ctx, 'switch_workspace_folder', { folder_id: 'repo' }, meta);
  assert.equal(selected.ok, true);
  t.after(async () => {
    await disposeProcessSessions(ctx);
    await rm(root, { recursive: true, force: true, maxRetries: 5, retryDelay: 50 });
    await rm(dataDir, { recursive: true, force: true, maxRetries: 5, retryDelay: 50 });
  });
  return { root, dataDir, ctx, meta };
}

async function waitFinal(state, sessionId, cursor = 0, outputMode = 'tail') {
  let nextCursor = cursor;
  let result;
  for (let attempt = 0; attempt < 6; attempt += 1) {
    result = await callTool(state.ctx, 'wait_command', {
      session_id: sessionId,
      cursor: nextCursor,
      timeout_ms: 10_000,
      until: 'finalized',
      output_mode: outputMode,
      max_output_bytes: 1024 * 1024
    }, state.meta);
    if (result.process_completed) return result;
    nextCursor = Number(result.latest_cursor ?? nextCursor);
  }
  return result;
}

async function waitForValue(read, timeoutMs = 5_000) {
  const started = Date.now();
  while (Date.now() - started < timeoutMs) {
    const value = read();
    if (value) return value;
    await new Promise(resolve => setTimeout(resolve, 10));
  }
  throw new Error('Timed out waiting for process lifecycle state');
}

const interactiveEcho = `
process.stdin.setEncoding('utf8');
process.stdin.on('data', chunk => process.stdout.write(chunk));
process.stdin.on('end', () => process.exit(0));
setInterval(() => {}, 1000);
`;

test('managed job survives Agent restart and reattaches by session and operation id', async t => {
  const root = await mkdtemp(path.join(tmpdir(), 'ctmcp-durable-root-'));
  const dataDir = await mkdtemp(path.join(tmpdir(), 'ctmcp-durable-data-'));
  const operationId = `durable-restart-${Date.now()}-${Math.random()}`;
  const script = `console.log('before-restart'); setTimeout(() => { console.log('after-restart'); }, 1500);`;
  const command = {
    program: nodeProgram,
    args: ['-e', script],
    operation_id: operationId,
    job_timeout_ms: 15_000,
    lock_group: 'durable-restart-lock',
    yield_time_ms: 100
  };
  const meta1 = { 'openai/session': `durable-a-${Date.now()}-${Math.random()}` };
  const ctx1 = await createToolContext(config(root, dataDir));
  let cleanupCtx = ctx1;
  let sessionId;
  t.after(async () => {
    if (sessionId) {
      try {
        const status = await callTool(cleanupCtx, 'wait_command', {
          session_id: sessionId,
          timeout_ms: 0,
          until: 'exit',
          output_mode: 'none'
        }, { 'openai/session': `durable-cleanup-${Date.now()}` });
        if (status.process_still_running) {
          await callTool(cleanupCtx, 'kill_session', { session_id: sessionId, signal: 'KILL' }, { 'openai/session': `durable-kill-${Date.now()}` });
        }
      } catch { /* best effort */ }
    }
    await disposeProcessSessions(cleanupCtx);
    await rm(root, { recursive: true, force: true, maxRetries: 5, retryDelay: 50 });
    await rm(dataDir, { recursive: true, force: true, maxRetries: 5, retryDelay: 50 });
  });

  assert.equal((await callTool(ctx1, 'switch_workspace_folder', { folder_id: 'repo' }, meta1)).ok, true);
  const started = await callTool(ctx1, 'exec_command', command, meta1);
  sessionId = started.session_id;
  assert.equal(started.execution_mode, 'job');
  assert.equal(started.restart_recoverable, true);
  assert.equal(started.process_still_running, true);
  assert.ok(sessionId);

  await disposeProcessSessions(ctx1);

  const ctx2 = await createToolContext(config(root, dataDir));
  cleanupCtx = ctx2;
  const meta2 = { 'openai/session': `durable-b-${Date.now()}-${Math.random()}` };
  assert.equal((await callTool(ctx2, 'switch_workspace_folder', { folder_id: 'repo' }, meta2)).ok, true);

  const bySession = await callTool(ctx2, 'wait_command', {
    session_id: sessionId,
    timeout_ms: 0,
    until: 'exit',
    output_mode: 'tail'
  }, meta2);
  assert.equal(bySession.session_id, sessionId);
  assert.equal(bySession.restart_recoverable, true);
  assert.notEqual(bySession.termination_reason, 'server_restart');

  const byOperation = await callTool(ctx2, 'exec_command', command, meta2);
  assert.equal(byOperation.session_id, sessionId);
  assert.equal(byOperation.operation_id, operationId);
  assert.equal(byOperation.deduplicated, true);

  const contenderPromise = callTool(ctx2, 'exec_command', {
    program: nodeProgram,
    args: ['-e', `console.log('contender');`],
    lock_group: 'durable-restart-lock',
    timeout_ms: 5_000,
    yield_time_ms: 0
  }, meta2);
  const final = await waitFinal({ ctx: ctx2, meta: meta2 }, sessionId, 0, 'tail');
  const contender = await contenderPromise;
  const contenderFinal = contender.process_still_running
    ? await waitFinal({ ctx: ctx2, meta: meta2 }, contender.session_id, 0, 'tail')
    : contender;
  assert.equal(contenderFinal.process_exit_code, 0);
  assert.ok(contenderFinal.resource_lock_wait_ms >= 500, JSON.stringify(contenderFinal));
  assert.equal(final.process_completed, true);
  assert.equal(final.process_exit_code, 0);
  assert.equal(final.termination_reason, 'exited');
  assert.match(final.stdout, /before-restart/);
  assert.match(final.stdout, /after-restart/);
  assert.equal(final.restart_recoverable, true);
});

test('durable managed job keeps its fixed timeout across Agent restart', async t => {
  const root = await mkdtemp(path.join(tmpdir(), 'ctmcp-durable-timeout-root-'));
  const dataDir = await mkdtemp(path.join(tmpdir(), 'ctmcp-durable-timeout-data-'));
  const ctx1 = await createToolContext(config(root, dataDir));
  const meta1 = { 'openai/session': `durable-timeout-a-${Date.now()}` };
  let ctx2 = ctx1;
  let sessionId;
  t.after(async () => {
    if (sessionId) {
      try {
        const status = await callTool(ctx2, 'wait_command', { session_id: sessionId, timeout_ms: 0, output_mode: 'none' }, { 'openai/session': `durable-timeout-clean-${Date.now()}` });
        if (status.process_still_running) await callTool(ctx2, 'kill_session', { session_id: sessionId, signal: 'KILL' }, { 'openai/session': `durable-timeout-kill-${Date.now()}` });
      } catch { /* best effort */ }
    }
    await disposeProcessSessions(ctx2);
    await rm(root, { recursive: true, force: true, maxRetries: 5, retryDelay: 50 });
    await rm(dataDir, { recursive: true, force: true, maxRetries: 5, retryDelay: 50 });
  });
  assert.equal((await callTool(ctx1, 'switch_workspace_folder', { folder_id: 'repo' }, meta1)).ok, true);
  const started = await callTool(ctx1, 'exec_command', {
    program: nodeProgram,
    args: ['-e', `console.log('timeout-start'); setTimeout(() => console.log('too-late'), 5000);`],
    operation_id: `durable-timeout-${Date.now()}`,
    job_timeout_ms: 800,
    yield_time_ms: 50
  }, meta1);
  sessionId = started.session_id;
  assert.equal(started.restart_recoverable, true);
  await disposeProcessSessions(ctx1);

  ctx2 = await createToolContext(config(root, dataDir));
  const meta2 = { 'openai/session': `durable-timeout-b-${Date.now()}` };
  assert.equal((await callTool(ctx2, 'switch_workspace_folder', { folder_id: 'repo' }, meta2)).ok, true);
  const final = await waitFinal({ ctx: ctx2, meta: meta2 }, sessionId, 0, 'tail');
  assert.equal(final.process_completed, true);
  assert.equal(final.process_timed_out, true);
  assert.equal(final.termination_reason, 'process_timeout');
  assert.equal(final.restart_recoverable, true);
  assert.match(final.stdout, /timeout-start/);
  assert.doesNotMatch(final.stdout, /too-late/);
});

test('kill_session terminates a durable managed job after Agent restart', async t => {
  const root = await mkdtemp(path.join(tmpdir(), 'ctmcp-durable-kill-root-'));
  const dataDir = await mkdtemp(path.join(tmpdir(), 'ctmcp-durable-kill-data-'));
  const ctx1 = await createToolContext(config(root, dataDir));
  const meta1 = { 'openai/session': `durable-kill-a-${Date.now()}` };
  let ctx2 = ctx1;
  let sessionId;
  t.after(async () => {
    if (sessionId) {
      try {
        const status = await callTool(ctx2, 'wait_command', { session_id: sessionId, timeout_ms: 0, output_mode: 'none' }, { 'openai/session': `durable-kill-clean-${Date.now()}` });
        if (status.process_still_running) await callTool(ctx2, 'kill_session', { session_id: sessionId, signal: 'KILL' }, { 'openai/session': `durable-kill-force-${Date.now()}` });
      } catch { /* best effort */ }
    }
    await disposeProcessSessions(ctx2);
    await rm(root, { recursive: true, force: true, maxRetries: 5, retryDelay: 50 });
    await rm(dataDir, { recursive: true, force: true, maxRetries: 5, retryDelay: 50 });
  });
  assert.equal((await callTool(ctx1, 'switch_workspace_folder', { folder_id: 'repo' }, meta1)).ok, true);
  const started = await callTool(ctx1, 'exec_command', {
    program: nodeProgram,
    args: ['-e', `console.log('kill-start'); setTimeout(() => console.log('too-late'), 60000);`],
    operation_id: `durable-kill-${Date.now()}`,
    job_timeout_ms: 60_000,
    yield_time_ms: 50
  }, meta1);
  sessionId = started.session_id;
  await disposeProcessSessions(ctx1);

  ctx2 = await createToolContext(config(root, dataDir));
  const meta2 = { 'openai/session': `durable-kill-b-${Date.now()}` };
  assert.equal((await callTool(ctx2, 'switch_workspace_folder', { folder_id: 'repo' }, meta2)).ok, true);
  const killed = await callTool(ctx2, 'kill_session', { session_id: sessionId, signal: 'KILL', wait_ms: 15_000 }, meta2);
  assert.equal(killed.ok, true);
  assert.equal(killed.termination_reason, 'killed');
  assert.equal(killed.killed, true);
  assert.equal(killed.restart_recoverable, true);
  assert.equal(killed.evicted, true);
  assert.match(killed.stdout, /kill-start/);
  assert.doesNotMatch(killed.stdout, /too-late/);
});

test('exec_many server restart preserves an already-started durable child session', async t => {
  const root = await mkdtemp(path.join(tmpdir(), 'ctmcp-durable-graph-root-'));
  const dataDir = await mkdtemp(path.join(tmpdir(), 'ctmcp-durable-graph-data-'));
  const ctx1 = await createToolContext(config(root, dataDir));
  const meta1 = { 'openai/session': `durable-graph-a-${Date.now()}` };
  const childOperationId = `durable-graph-child-${Date.now()}-${Math.random()}`;
  const graphOperationId = `durable-graph-${Date.now()}-${Math.random()}`;
  let ctx2 = ctx1;
  let sessionId;
  t.after(async () => {
    if (sessionId) {
      try {
        const status = await callTool(ctx2, 'wait_command', { session_id: sessionId, timeout_ms: 0, output_mode: 'none' }, { 'openai/session': `durable-graph-clean-${Date.now()}` });
        if (status.process_still_running) await callTool(ctx2, 'kill_session', { session_id: sessionId, signal: 'KILL' }, { 'openai/session': `durable-graph-kill-${Date.now()}` });
      } catch { /* best effort */ }
    }
    await disposeProcessSessions(ctx2);
    await rm(root, { recursive: true, force: true, maxRetries: 5, retryDelay: 50 });
    await rm(dataDir, { recursive: true, force: true, maxRetries: 5, retryDelay: 50 });
  });

  assert.equal((await callTool(ctx1, 'switch_workspace_folder', { folder_id: 'repo' }, meta1)).ok, true);
  let graph = await callTool(ctx1, 'exec_many', {
    operation_id: graphOperationId,
    mode: 'parallel',
    max_parallel: 1,
    yield_time_ms: 100,
    result_mode: 'full',
    commands: [{
      id: 'durable-child',
      program: nodeProgram,
      args: ['-e', `console.log('graph-before-restart'); setTimeout(() => console.log('graph-after-restart'), 1600);`],
      operation_id: childOperationId,
      job_timeout_ms: 15_000,
      yield_time_ms: 0
    }]
  }, meta1);
  let child = graph.results.find(result => result.id === 'durable-child');
  for (let attempt = 0; !child?.session_id && attempt < 20; attempt += 1) {
    await new Promise(resolve => setTimeout(resolve, 50));
    graph = await callTool(ctx1, 'exec_many', {
      operation_id: graphOperationId,
      action: 'status',
      result_mode: 'full'
    }, meta1);
    child = graph.results.find(result => result.id === 'durable-child');
  }
  sessionId = child?.session_id;
  assert.ok(sessionId, JSON.stringify(graph));
  assert.equal(child.restart_recoverable, true);
  assert.equal(child.process_still_running, true);

  await disposeProcessSessions(ctx1);

  ctx2 = await createToolContext(config(root, dataDir));
  const meta2 = { 'openai/session': `durable-graph-b-${Date.now()}` };
  assert.equal((await callTool(ctx2, 'switch_workspace_folder', { folder_id: 'repo' }, meta2)).ok, true);
  const restored = await callTool(ctx2, 'wait_command', {
    session_id: sessionId,
    timeout_ms: 0,
    until: 'exit',
    output_mode: 'tail'
  }, meta2);
  assert.equal(restored.restart_recoverable, true);
  assert.notEqual(restored.termination_reason, 'graph_cancelled');
  assert.notEqual(restored.termination_reason, 'server_restart');

  const byOperation = await callTool(ctx2, 'exec_command', {
    program: nodeProgram,
    args: ['-e', `console.log('graph-before-restart'); setTimeout(() => console.log('graph-after-restart'), 1600);`],
    operation_id: childOperationId,
    job_timeout_ms: 15_000,
    yield_time_ms: 0
  }, meta2);
  assert.equal(byOperation.session_id, sessionId);
  assert.equal(byOperation.deduplicated, true);

  let final;
  const finalDeadline = Date.now() + 30_000;
  while (Date.now() < finalDeadline) {
    final = await callTool(ctx2, 'wait_command', {
      session_id: sessionId,
      cursor: Number(final?.latest_cursor ?? 0),
      timeout_ms: 2_000,
      until: 'finalized',
      output_mode: 'tail',
      max_output_bytes: 1024 * 1024
    }, meta2);
    if (final.process_completed) break;
    await new Promise(resolve => setTimeout(resolve, 100));
  }
  assert.equal(final?.process_completed, true, JSON.stringify(final));
  assert.equal(final.process_exit_code, 0);
  assert.equal(final.termination_reason, 'exited');
  assert.match(final.stdout, /graph-before-restart/);
  assert.match(final.stdout, /graph-after-restart/);
});

test('exec_many explicit cancel still terminates an owned durable child', async t => {
  const root = await mkdtemp(path.join(tmpdir(), 'ctmcp-durable-graph-cancel-root-'));
  const dataDir = await mkdtemp(path.join(tmpdir(), 'ctmcp-durable-graph-cancel-data-'));
  const ctx = await createToolContext(config(root, dataDir));
  const meta = { 'openai/session': `durable-graph-cancel-${Date.now()}` };
  const graphOperationId = `durable-graph-cancel-${Date.now()}-${Math.random()}`;
  let sessionId;
  t.after(async () => {
    if (sessionId) {
      try {
        const status = await callTool(ctx, 'wait_command', {
          session_id: sessionId,
          timeout_ms: 0,
          until: 'exit',
          output_mode: 'none'
        }, { 'openai/session': `durable-graph-cancel-clean-${Date.now()}` });
        if (status.process_still_running) {
          await callTool(ctx, 'kill_session', {
            session_id: sessionId,
            signal: 'KILL',
            wait_ms: 15_000
          }, { 'openai/session': `durable-graph-cancel-force-${Date.now()}` });
        }
      } catch { /* best effort */ }
    }
    await disposeProcessSessions(ctx);
    await rm(root, { recursive: true, force: true, maxRetries: 5, retryDelay: 50 });
    await rm(dataDir, { recursive: true, force: true, maxRetries: 5, retryDelay: 50 });
  });

  assert.equal((await callTool(ctx, 'switch_workspace_folder', { folder_id: 'repo' }, meta)).ok, true);
  let graph = await callTool(ctx, 'exec_many', {
    operation_id: graphOperationId,
    mode: 'parallel',
    max_parallel: 1,
    yield_time_ms: 100,
    result_mode: 'full',
    commands: [{
      id: 'durable-child',
      program: nodeProgram,
      args: ['-e', `console.log('cancel-start'); setTimeout(() => console.log('too-late'), 60000);`],
      operation_id: `durable-graph-cancel-child-${Date.now()}-${Math.random()}`,
      job_timeout_ms: 60_000,
      yield_time_ms: 0
    }]
  }, meta);
  let child = graph.results.find(result => result.id === 'durable-child');
  for (let attempt = 0; !child?.session_id && attempt < 20; attempt += 1) {
    await new Promise(resolve => setTimeout(resolve, 50));
    graph = await callTool(ctx, 'exec_many', {
      operation_id: graphOperationId,
      action: 'status',
      result_mode: 'full'
    }, meta);
    child = graph.results.find(result => result.id === 'durable-child');
  }
  sessionId = child?.session_id;
  assert.ok(sessionId, JSON.stringify(graph));
  assert.equal(child.restart_recoverable, true);
  assert.equal(child.process_still_running, true);

  // A session ID confirms ownership; wait for child output before testing cancellation.
  let ready;
  const readyDeadline = Date.now() + 10_000;
  do {
    ready = await callTool(ctx, 'wait_command', {
      session_id: sessionId,
      timeout_ms: 100,
      until: 'exit',
      output_mode: 'tail'
    }, meta);
  } while (!ready.stdout?.includes('cancel-start') && ready.process_still_running && Date.now() < readyDeadline);
  assert.match(ready.stdout ?? '', /cancel-start/, JSON.stringify(ready));
  assert.equal(ready.process_still_running, true);

  const cancelled = await callTool(ctx, 'exec_many', {
    operation_id: graphOperationId,
    action: 'cancel',
    reason: 'test explicit cancel',
    yield_time_ms: 15_000,
    result_mode: 'full'
  }, meta);
  assert.equal(cancelled.cancel_accepted, true, JSON.stringify(cancelled));
  assert.ok(cancelled.cancelled_session_count >= 1, JSON.stringify(cancelled));

  const final = await waitFinal({ ctx, meta }, sessionId, 0, 'tail');
  assert.equal(final.process_completed, true);
  assert.equal(final.termination_reason, 'graph_cancelled');
  assert.match(final.stdout, /cancel-start/);
  assert.doesNotMatch(final.stdout, /too-late/);
});

test('restart-persistent managed jobs reject interactive and protected credential sources before spawn', async t => {
  const state = await fixture(t);
  for (const command of [
    { program: nodeProgram, args: ['-e', 'setInterval(() => {}, 1000)'], operation_id: 'durable-tty', job_timeout_ms: 5_000, tty: true },
    { program: nodeProgram, args: ['-e', 'console.log(1)'], operation_id: 'durable-secret', job_timeout_ms: 5_000, secret_env: { API_TOKEN: 'fixture-secret' } },
    { program: nodeProgram, args: ['-e', 'console.log(1)'], operation_id: 'durable-env', job_timeout_ms: 5_000, env: { API_TOKEN: 'plain-but-sensitive' } },
    { program: nodeProgram, args: ['-e', 'console.log(1)', '--token', 'plain-command-secret'], operation_id: 'durable-argv', job_timeout_ms: 5_000 }
  ]) {
    const result = await callTool(state.ctx, 'exec_command', command, state.meta);
    assert.equal(result.ok, false);
    assert.equal(result.error.code, 'DURABLE_JOB_UNSUPPORTED');
    assert.equal(result.error.details.process_started, false);
  }
  const runtime = runtimeForFolderId(state.ctx, 'repo');
  assert.equal([...runtime.sessions.values()].filter(session => session.timeoutContract?.executionMode === 'job').length, 0);
});

test('Docker Linux sandbox rejects a Windows host executable before any host process session is created', {
  skip: process.platform !== 'win32'
}, async t => {
  const state = await fixture(t);
  state.ctx.config.sandbox = {
    enabled: true,
    backend: 'docker_sbx',
    externalPaths: [],
    options: {}
  };
  const result = await callTool(state.ctx, 'exec_command', {
    program: 'node.exe',
    args: ['-e', 'process.stdout.write("must-not-run")'],
    timeout_ms: 5_000
  }, state.meta);
  assert.equal(result.ok, false, JSON.stringify(result));
  assert.equal(result.error.code, 'SANDBOX_COMMAND_UNSUPPORTED');
  assert.equal(result.error.category, 'policy');
  assert.equal(result.error.retryable, false);
  assert.equal(repoRuntime(state.ctx).sessions.size, 0);
});

test('Node WSLC reports host availability without an explicit storage option', async t => {
  const state = await fixture(t);
  state.ctx.config.sandbox = {
    enabled: true,
    backend: 'wslc',
    externalPaths: [],
    options: { 'wslc.image': 'alpine:3.20', 'wslc.network': 'none' }
  };
  const info = await callTool(state.ctx, 'server_info', {}, state.meta);
  const available = wslcHostAvailable();
  assert.equal(info.environment.filesystem_sandbox.available, available);
  assert.equal(info.environment.filesystem_sandbox.enforced, available);
  assert.equal(info.environment.filesystem_sandbox.enforcement_ready, true);
  assert.equal(info.environment.filesystem_sandbox.verification_tool, 'exec_health_check');
  assert.equal(info.environment.filesystem_sandbox.live_verification_required, true);
  assert.equal(info.environment.workspace_exec.available, available);
  assert.equal(info.environment.workspace_exec.sandbox_enforced, available);
  assert.equal(info.environment.workspace_exec.boundary, available ? 'wslc' : 'sandbox_unavailable');
  assert.equal(repoRuntime(state.ctx).sessions.size, 0);
});

test('server_info reports backend readiness separately from live sandbox availability', async t => {
  const state = await fixture(t);
  state.ctx.config.sandbox = {
    enabled: true,
    backend: 'docker_sbx',
    externalPaths: [],
    options: {}
  };
  const result = await callTool(state.ctx, 'server_info', {}, state.meta);
  assert.equal(result.ok, true, JSON.stringify(result));
  assert.equal(result.environment.filesystem_sandbox.enabled, true);
  const backend = result.environment.filesystem_sandbox.backends.find(value => value.id === 'docker_sbx');
  assert.equal(result.environment.filesystem_sandbox.available, backend?.hostSupported ?? false);
  assert.equal(result.environment.filesystem_sandbox.enforced, result.environment.filesystem_sandbox.available);
  assert.equal(result.environment.filesystem_sandbox.backend, 'docker_sbx');
  assert.equal(result.environment.filesystem_sandbox.enforcement_ready, true);
  assert.equal(result.environment.filesystem_sandbox.verification_tool, 'exec_health_check');
  assert.equal(result.environment.filesystem_sandbox.live_verification_required, true);
  assert.equal(result.environment.workspace_exec.available, result.environment.filesystem_sandbox.available);
  assert.equal(result.environment.workspace_exec.sandbox_enforced, result.environment.filesystem_sandbox.available);
  assert.equal(result.environment.workspace_exec.boundary, result.environment.filesystem_sandbox.available ? 'docker_sbx' : 'sandbox_unavailable');
  assert.ok(result.environment.filesystem_sandbox.backends.some(backend => backend.id === 'wslc'));
  assert.ok(result.environment.filesystem_sandbox.backends.some(backend => backend.id === 'docker'));
  assert.ok(result.environment.filesystem_sandbox.backends.some(backend => backend.id === 'podman'));
});

test('live WSLC sandbox enforces Node Agent process, post-check, health and cancellation boundaries', {
  skip: process.platform !== 'win32'
    || process.env.CTMCP_TEST_WSLC !== '1'
}, async t => {
  const state = await fixture(t);
  const readonly = await mkdtemp(path.join(tmpdir(), 'ctmcp-node-wslc-ro-'));
  const writable = await mkdtemp(path.join(tmpdir(), 'ctmcp-node-wslc-rw-'));
  const hidden = await mkdtemp(path.join(tmpdir(), 'ctmcp-node-wslc-hidden-'));
  t.after(async () => {
    await rm(readonly, { recursive: true, force: true, maxRetries: 5, retryDelay: 50 });
    await rm(writable, { recursive: true, force: true, maxRetries: 5, retryDelay: 50 });
    await rm(hidden, { recursive: true, force: true, maxRetries: 5, retryDelay: 50 });
  });
  await writeFile(path.join(readonly, 'readonly-marker.txt'), 'readonly');
  await writeFile(path.join(writable, 'writable-marker.txt'), 'writable');
  await writeFile(path.join(hidden, 'hidden-marker.txt'), 'hidden');

  state.ctx.config.sandbox = {
    enabled: true,
    backend: 'wslc',
    externalPaths: [
      { path: readonly, access: 'read_only' },
      { path: writable, access: 'modify' }
    ],
    options: {
      'wslc.image': process.env.CTMCP_TEST_WSLC_IMAGE ?? 'alpine:3.20',
      'wslc.network': 'none'
    }
  };

  const started = await callTool(state.ctx, 'exec_command', {
    program: 'sh',
    args: ['-c', [
      'set -e',
      'printf "%s|" "$CTMCP_NODE_WSLC"',
      'cat',
      'printf workspace-write > live.txt',
      'ro=$(find /ctmcp/grants -name readonly-marker.txt -print -quit)',
      'rw=$(find /ctmcp/grants -name writable-marker.txt -print -quit)',
      'test -n "$ro"',
      'test -n "$rw"',
      'if printf blocked > "$(dirname "$ro")/blocked.txt"; then exit 41; fi',
      'printf allowed > "$(dirname "$rw")/allowed.txt"',
      'test ! -e /ctmcp/grants/2'
    ].join('; ')],
    env: { CTMCP_NODE_WSLC: 'node-wslc' },
    stdin: 'stdin-ok',
    timeout_ms: 30_000,
    yield_time_ms: 30_000,
    output_mode: 'all',
    post_checks: [{
      name: 'inside-alpine',
      program: 'sh',
      args: ['-c', 'test -f /etc/alpine-release'],
      timeout_ms: 10_000
    }]
  }, state.meta);
  let completed = started;
  if (started.session_id && !started.process_completed) completed = await waitFinal(state, started.session_id, 0, 'all');
  assert.equal(completed.ok, true, JSON.stringify(completed));
  assert.equal(completed.command_ok, true, JSON.stringify(completed));
  assert.equal(completed.sandbox_enforced, true);
  assert.equal(completed.sandbox_backend, 'wslc');
  assert.equal(completed.execution_boundary, 'wslc');
  assert.ok(Number.isFinite(completed.sandbox_phase_durations_ms?.prepare_ms), JSON.stringify(completed.sandbox_phase_durations_ms));
  assert.ok(Number.isFinite(completed.sandbox_phase_durations_ms?.startup_ms), JSON.stringify(completed.sandbox_phase_durations_ms));
  assert.ok(Number.isFinite(completed.sandbox_phase_durations_ms?.cleanup_ms), JSON.stringify(completed.sandbox_phase_durations_ms));
  assert.equal(completed.process_tree_contained, true);
  assert.equal(completed.process_tree_control, 'wslc_container');
  assert.match(completed.stdout, /node-wslc\|stdin-ok/);
  assert.equal(completed.post_checks?.[0]?.ok, true, JSON.stringify(completed.post_checks));
  const managedStorage = await managedWslcSessionStorage(state.dataDir, state.root);
  assert.ok((await stat(path.join(managedStorage, 'storage.vhdx'))).size > 0);
  assert.equal(await readFile(path.join(state.root, 'live.txt'), 'utf8'), 'workspace-write');
  assert.equal(await readFile(path.join(writable, 'allowed.txt'), 'utf8'), 'allowed');
  await assert.rejects(readFile(path.join(readonly, 'blocked.txt'), 'utf8'));
  assert.equal(await readFile(path.join(hidden, 'hidden-marker.txt'), 'utf8'), 'hidden');

  const health = await callTool(state.ctx, 'exec_health_check', {}, state.meta);
  assert.equal(health.status, 'success', JSON.stringify(health));
  assert.equal(health.probe.sandbox_enforced, true);
  assert.equal(health.probe.sandbox_backend, 'wslc');
  assert.equal(health.probe.execution_boundary, 'wslc');
  assert.ok(Number.isFinite(health.probe.sandbox_phase_durations_ms?.prepare_ms), JSON.stringify(health.probe));
  assert.ok(Number.isFinite(health.probe.sandbox_phase_durations_ms?.startup_ms), JSON.stringify(health.probe));
  assert.ok(Number.isFinite(health.probe.sandbox_phase_durations_ms?.cleanup_ms), JSON.stringify(health.probe));
  assert.deepEqual(health.sandbox_verification, {
    required: true,
    verified: true,
    backend: 'wslc',
    execution_boundary: 'wslc'
  });

  const info = await callTool(state.ctx, 'server_info', {}, state.meta);
  assert.equal(info.environment.filesystem_sandbox.available, true);
  assert.equal(info.environment.filesystem_sandbox.enforced, true);
  assert.equal(info.environment.workspace_exec.boundary, 'wslc');

  const parallelStartedAt = Date.now();
  const [parallelA, parallelB] = await Promise.all([
    callTool(state.ctx, 'exec_command', {
      program: 'sh',
      args: ['-c', 'sleep 2; printf parallel-a'],
      timeout_ms: 30_000,
      yield_time_ms: 0,
      deduplicate: false
    }, state.meta),
    callTool(state.ctx, 'exec_command', {
      program: 'sh',
      args: ['-c', 'sleep 2; printf parallel-b'],
      timeout_ms: 30_000,
      yield_time_ms: 0,
      deduplicate: false
    }, state.meta)
  ]);
  assert.equal(parallelA.ok, true, JSON.stringify(parallelA));
  assert.equal(parallelB.ok, true, JSON.stringify(parallelB));
  assert.ok(Date.now() - parallelStartedAt >= 1_500, 'second command should wait for the first storage lease');
  const [parallelAFinal, parallelBFinal] = await Promise.all([
    waitFinal(state, parallelA.session_id, 0, 'all'),
    waitFinal(state, parallelB.session_id, 0, 'all')
  ]);
  assert.equal(parallelAFinal.command_ok, true, JSON.stringify(parallelAFinal));
  assert.equal(parallelBFinal.command_ok, true, JSON.stringify(parallelBFinal));
  assert.equal(parallelAFinal.sandbox_enforced, true);
  assert.equal(parallelBFinal.sandbox_enforced, true);
  assert.match(parallelAFinal.stdout, /parallel-a/);
  assert.match(parallelBFinal.stdout, /parallel-b/);

  const queueHolder = await startAndYield(state.ctx, state.meta['openai/session'], {
    program: 'sh',
    args: ['-c', 'sleep 3; printf queue-holder'],
    timeout_ms: 30_000,
    yield_time_ms: 0,
    deduplicate: false
  });
  const queuedLifecycle = new ProcessRequestLifecycle(state.ctx);
  const queued = startAndYield(state.ctx, state.meta['openai/session'], {
    program: 'sh',
    args: ['-c', 'printf leaked > queued-cancel-leak.txt'],
    timeout_ms: 30_000,
    yield_time_ms: 0,
    deduplicate: false
  }, queuedLifecycle);
  await new Promise(resolve => setTimeout(resolve, 200));
  queuedLifecycle.abort();
  await assert.rejects(queued, error => error?.code === 'SANDBOX_WSLC_QUEUE_CANCELLED');
  await waitFinal(state, queueHolder.session_id, 0, 'all');
  await new Promise(resolve => setTimeout(resolve, 250));
  await assert.rejects(readFile(path.join(state.root, 'queued-cancel-leak.txt'), 'utf8'));

  const longRunning = await callTool(state.ctx, 'exec_command', {
    program: 'sh',
    args: ['-c', 'sleep 5; printf leaked > cancel-leak.txt'],
    timeout_ms: 30_000,
    yield_time_ms: 0
  }, state.meta);
  assert.equal(longRunning.process_still_running, true, JSON.stringify(longRunning));
  const killed = await callTool(state.ctx, 'kill_session', {
    session_id: longRunning.session_id,
    signal: 'KILL',
    wait_ms: 5_000
  }, state.meta);
  assert.equal(killed.ok, true, JSON.stringify(killed));
  await new Promise(resolve => setTimeout(resolve, 6_000));
  await assert.rejects(readFile(path.join(state.root, 'cancel-leak.txt'), 'utf8'));
});

test('interactive sessions expose stdin state, byte counts, timing and closed-input errors', async t => {
  const state = await fixture(t);
  const started = await callTool(state.ctx, 'exec_command', {
    program: nodeProgram,
    args: ['-e', interactiveEcho],
    tty: true,
    yield_time_ms: 0,
    timeout_ms: 10_000,
    operation_id: 'interactive-stdin'
  }, state.meta);
  assert.equal(started.ok, true, JSON.stringify(started));
  assert.equal(started.startup.attempts, 1);
  assert.equal(started.startup.retry_count, 0);
  assert.ok(started.startup.gate_wait_ms >= 0);
  assert.deepEqual(started.startup.retry_delays_ms, []);
  assert.equal(started.startup.startup_slots, process.platform === 'win32' ? 4 : 0);
  assert.equal(started.startup.start_interval_ms, process.platform === 'win32' ? 75 : 0);
  assert.equal(started.startup.error_dialog_suppressed, false);
  assert.equal(started.interactive, true);
  assert.equal(started.stdin_open, true);
  assert.equal(started.status, 'running');
  assert.equal(started.termination_reason, 'running');
  assert.equal(started.recoverable, false);
  assert.equal(started.first_output_ms, null);
  assert.ok(Array.isArray(started.next_actions));

  const firstInput = await callTool(state.ctx, 'send_input', {
    session_id: started.session_id,
    chars: 'hé'
  }, state.meta);
  assert.equal(firstInput.bytes_written, Buffer.byteLength('hé'));
  assert.equal(firstInput.stdin_closed, false);
  assert.equal(firstInput.stdin_open, true);

  const closed = await callTool(state.ctx, 'send_input', {
    session_id: started.session_id,
    chars: 'llo\n',
    close_stdin: true
  }, state.meta);
  assert.equal(closed.bytes_written, 4);
  assert.equal(closed.stdin_closed, true);
  assert.equal(closed.stdin_open, false);

  const finalized = await waitFinal(state, started.session_id);
  assert.equal(finalized.command_ok, true, JSON.stringify(finalized));
  assert.equal(finalized.termination_reason, 'exited');
  assert.equal(finalized.interactive, true);
  assert.equal(finalized.stdin_open, false);
  assert.match(finalized.stdout, /héllo/);
  assert.equal(typeof finalized.first_output_ms, 'number');
  assert.ok(finalized.elapsed_ms >= finalized.first_output_ms);

  const rejected = await callTool(state.ctx, 'send_input', {
    session_id: started.session_id,
    chars: 'late'
  }, state.meta);
  assert.equal(rejected.ok, false);
  assert.equal(rejected.error.code, 'SESSION_CLOSED');
  assert.equal(rejected.error.retryable, false);
});

test('wait_command caps a long requested wait to the transport-safe chunk', async t => {
  const state = await fixture(t);
  const started = await callTool(state.ctx, 'exec_command', {
    program: nodeProgram,
    args: ['-e', 'setTimeout(() => process.exit(0), 200)'],
    yield_time_ms: 0,
    timeout_ms: 10_000,
    output_mode: 'none'
  }, state.meta);
  assert.equal(started.next_actions[0].arguments.timeout_ms, 20_000);

  const waitedAt = Date.now();
  const waited = await callTool(state.ctx, 'wait_command', {
    session_id: started.session_id,
    cursor: started.latest_cursor,
    timeout_ms: 60 * 60_000,
    until: 'exit',
    output_mode: 'none'
  }, state.meta);
  assert.ok(Date.now() - waitedAt < 5_000, JSON.stringify(waited));
  assert.equal(waited.wait_timeout_ms, 60 * 60_000);
  assert.equal(waited.effective_wait_ms, 20_000);
  assert.equal(waited.process_still_running, false);
});
test('wait_command ignores the legacy heartbeat interval and uses the bounded server wait window', async t => {
  const state = await fixture(t);
  const started = await callTool(state.ctx, 'exec_command', {
    program: nodeProgram,
    args: ['-e', 'setInterval(() => {}, 1000)'],
    yield_time_ms: 0,
    timeout_ms: 10_000
  }, state.meta);
  assert.equal(started.operation_id, null);
  assert.equal(started.next_actions[0].arguments.timeout_ms, 20_000);
  assert.equal(started.next_actions[0].arguments.until, 'output_or_exit');

  const waited = await callTool(state.ctx, 'wait_command', {
    session_id: started.session_id,
    cursor: started.latest_cursor,
    timeout_ms: 600,
    heartbeat_ms: 50,
    until: 'finalized',
    output_mode: 'delta'
  }, state.meta);
  assert.equal(waited.heartbeat, false, JSON.stringify(waited));
  assert.equal(waited.request_timed_out, true);
  assert.equal(waited.effective_wait_ms, 600);
  assert.ok(waited.actual_wait_ms >= 500, JSON.stringify(waited));
  assert.equal(waited.process_still_running, true);
  assert.ok(Array.isArray(waited.next_actions));
  assert.equal(waited.next_actions[0].arguments.timeout_ms, 600);
  assert.equal(Object.hasOwn(waited.next_actions[0].arguments, 'heartbeat_ms'), false);
  assert.equal(waited.wait_until, 'finalized');
  assert.equal(typeof waited.session_registry_wait_ms, 'number');
  assert.equal(typeof waited.snapshot_ms, 'number');

  const mistaken = await callTool(state.ctx, 'wait_command', {
    session_id: started.output_refs.stdout,
    timeout_ms: 0
  }, state.meta);
  assert.equal(mistaken.ok, false);
  assert.equal(mistaken.error.code, 'OUTPUT_REF_USED_AS_SESSION_ID');
  assert.equal(mistaken.error.retryable, true);
  assert.equal(mistaken.error.details.corrected_session_id, started.session_id);

  const killed = await callTool(state.ctx, 'kill_session', {
    session_id: started.session_id,
    signal: 'KILL',
    wait_ms: 5_000
  }, state.meta);
  assert.equal(killed.evicted, true, JSON.stringify(killed));
});

test('kill_session retains an exited session until post-check finalization completes', async t => {
  const state = await fixture(t);
  const started = await callTool(state.ctx, 'exec_command', {
    program: nodeProgram,
    args: ['-e', 'setTimeout(() => process.exit(0), 50)'],
    tty: true,
    yield_time_ms: 0,
    timeout_ms: 5_000,
    post_checks: [{
      name: 'slow-verify',
      program: nodeProgram,
      args: ['-e', 'setTimeout(() => process.exit(0), 750)'],
      timeout_ms: 2_000
    }]
  }, state.meta);
  const session = repoRuntime(state.ctx).sessions.get(started.session_id);
  assert.ok(session);
  await waitForSession(session, session.sequence, 5_000, 'exit');
  assert.ok(session.endedAt);
  assert.equal(session.finalizedAt, undefined);

  const verifying = await callTool(state.ctx, 'kill_session', {
    session_id: started.session_id,
    wait_ms: 0
  }, state.meta);
  assert.equal(verifying.killed, false, JSON.stringify(verifying));
  assert.equal(verifying.status, 'verifying', JSON.stringify(verifying));
  assert.equal(verifying.evicted, false, JSON.stringify(verifying));
  assert.equal(repoRuntime(state.ctx).sessions.has(started.session_id), true);

  await waitForSession(session, session.sequence, 5_000, 'finalized');
  assert.ok(session.finalizedAt);
  const evicted = await callTool(state.ctx, 'kill_session', {
    session_id: started.session_id,
    wait_ms: 0
  }, state.meta);
  assert.equal(evicted.evicted, true, JSON.stringify(evicted));
  assert.equal(repoRuntime(state.ctx).sessions.has(started.session_id), false);
});

test('Windows tree termination falls back to direct SIGKILL when taskkill fails', { skip: process.platform !== 'win32' }, async () => {
  let fallbackSignal;
  const child = {
    pid: 2_147_483_647,
    exitCode: null,
    signalCode: null,
    kill(signal) {
      fallbackSignal = signal;
      return true;
    }
  };
  const session = {
    endedAt: undefined,
    child,
    terminationReason: undefined,
    killed: false,
    events: new EventEmitter()
  };

  await killProcessTree(session, 'KILL', 'killed');
  assert.equal(fallbackSignal, 'SIGKILL');
});

test('Windows TERM kill_session forcefully terminates the managed tree like Rust', { skip: process.platform !== 'win32' }, async t => {
  const state = await fixture(t);
  const started = await callTool(state.ctx, 'exec_command', {
    program: nodeProgram,
    args: ['-e', 'require("node:http").createServer((_req, res) => res.end("ok")).listen(0); setInterval(() => {}, 1000)'],
    yield_time_ms: 0,
    timeout_ms: 30_000
  }, state.meta);
  assert.equal(started.process_still_running, true, JSON.stringify(started));

  const killed = await callTool(state.ctx, 'kill_session', {
    session_id: started.session_id,
    signal: 'TERM',
    wait_ms: 5_000
  }, state.meta);
  assert.equal(killed.ok, true, JSON.stringify(killed));
  assert.equal(killed.status, 'killed', JSON.stringify(killed));
  assert.equal(killed.killed, true, JSON.stringify(killed));
  assert.equal(killed.process_still_running, false, JSON.stringify(killed));
  assert.equal(killed.evicted, true, JSON.stringify(killed));
});

test('operation reattachment is stable while automatic dedupe only coalesces active sessions', async t => {
  const state = await fixture(t);
  const explicitArgs = {
    program: nodeProgram,
    args: ['-e', 'process.stdout.write("explicit")'],
    operation_id: 'explicit-operation',
    lock_group: 'process-fixture',
    timeout_ms: 10_000,
    yield_time_ms: 10_000
  };
  const first = await callTool(state.ctx, 'exec_command', explicitArgs, state.meta);
  assert.equal(first.operation_id, 'explicit-operation');
  assert.equal(first.resource_lock_group, 'process-fixture');
  assert.equal(first.resource_lock_target, state.root);
  assert.equal(typeof first.operation_lock_wait_ms, 'number');
  assert.equal(typeof first.resource_lock_wait_ms, 'number');
  const second = await callTool(state.ctx, 'exec_command', explicitArgs, state.meta);
  assert.equal(second.session_id, first.session_id);
  assert.equal(second.deduplicated, true);
  assert.equal(second.attached_to_session_id, first.session_id);
  assert.equal(typeof second.operation_lock_wait_ms, 'number');

  const resolved = await callTool(state.ctx, 'resolve_operation', {
    operation_id: 'explicit-operation',
    output_mode: 'tail'
  }, state.meta);
  assert.equal(resolved.resolved_by, 'operation_id');
  assert.equal(resolved.deduplicated, true);
  assert.equal(resolved.attached_to_session_id, first.session_id);

  const conflict = await callTool(state.ctx, 'exec_command', {
    ...explicitArgs,
    timeout_ms: 20_000
  }, state.meta);
  assert.equal(conflict.ok, false);
  assert.equal(conflict.error.code, 'OPERATION_ID_CONFLICT');
  assert.equal(conflict.error.category, 'conflict');
  assert.equal(conflict.error.details.existing_session_id, first.session_id);

  const missing = await callTool(state.ctx, 'resolve_operation', {
    operation_id: 'missing-operation'
  }, state.meta);
  assert.equal(missing.ok, false);
  assert.equal(missing.error.code, 'OPERATION_NOT_FOUND');
  assert.equal(missing.error.retryable, false);
  assert.equal(missing.error.details.retention_seconds, FINALIZED_SESSION_RETENTION_MS / 1000);

  const automaticArgs = {
    program: nodeProgram,
    args: ['-e', 'process.stdout.write("automatic")'],
    deduplicate: true,
    timeout_ms: 10_000,
    yield_time_ms: 10_000
  };
  const automaticStarted = await callTool(state.ctx, 'exec_command', automaticArgs, state.meta);
  assert.match(automaticStarted.operation_id, /^auto:[0-9a-f]{32}$/);
  const automaticFirst = automaticStarted.process_still_running
    ? await waitFinal(state, automaticStarted.session_id, automaticStarted.latest_cursor, 'tail')
    : automaticStarted;
  assert.equal(automaticFirst.process_still_running, false, JSON.stringify(automaticFirst));
  assert.ok(Number(automaticFirst.finalized_ts_ms) > 0, JSON.stringify(automaticFirst));
  const automaticSecond = await callTool(state.ctx, 'exec_command', automaticArgs, state.meta);
  assert.notEqual(automaticSecond.session_id, automaticFirst.session_id);
  assert.equal(automaticSecond.deduplicated, false);

  const activeArgs = {
    program: nodeProgram,
    args: ['-e', 'setTimeout(() => process.exit(0), 30_000)'],
    deduplicate: true,
    timeout_ms: 60_000,
    yield_time_ms: 0,
    output_mode: 'none'
  };
  const activeFirst = await callTool(state.ctx, 'exec_command', activeArgs, state.meta);
  assert.equal(activeFirst.process_still_running, true, JSON.stringify(activeFirst));
  const activeSecond = await callTool(state.ctx, 'exec_command', activeArgs, state.meta);
  assert.equal(activeSecond.session_id, activeFirst.session_id);
  assert.equal(activeSecond.deduplicated, true);
  assert.equal(activeSecond.attached_to_session_id, activeFirst.session_id);
  const activeCleanup = await callTool(state.ctx, 'kill_session', {
    session_id: activeFirst.session_id,
    signal: 'KILL',
    wait_ms: 5_000
  }, state.meta);
  assert.equal(activeCleanup.ok, true, JSON.stringify(activeCleanup));
  assert.equal(activeCleanup.killed, true, JSON.stringify(activeCleanup));
  assert.equal(activeCleanup.process_still_running, false, JSON.stringify(activeCleanup));
});

test('Windows startup admission retains sessions even when it consumes the command timeout', {
  skip: process.platform !== 'win32'
}, async t => {
  const state = await fixture(t);
  const started = await Promise.all(Array.from({ length: 8 }, (_, index) => callTool(state.ctx, 'exec_command', {
    program: nodeProgram,
    args: ['-e', 'setTimeout(() => {}, 10000)'],
    tty: true,
    yield_time_ms: 0,
    timeout_ms: 100,
    operation_id: `startup-timeout-${index}`
  }, state.meta)));

  for (const result of started) {
    assert.equal(typeof result.session_id, 'string', JSON.stringify(result));
  }
  const finalized = await Promise.all(started.map(result => waitFinal(state, result.session_id, 0, 'none')));
  for (const result of finalized) {
    assert.equal(result.command_ok, false, JSON.stringify(result));
    assert.equal(result.termination_reason, 'process_timeout');
    assert.equal(result.status, 'timed_out');
    assert.equal(result.process_timed_out, true);
  }
});

test('timeout and detached grace expose Rust recovery contracts and reattachment cancels cleanup', async t => {
  const state = await fixture(t);
  const timed = await callTool(state.ctx, 'exec_command', {
    program: nodeProgram,
    args: ['-e', 'setTimeout(() => {}, 10000)'],
    tty: true,
    yield_time_ms: 0,
    timeout_ms: 100,
    operation_id: 'timeout-operation'
  }, state.meta);
  const timedFinal = await waitFinal(state, timed.session_id);
  assert.equal(timedFinal.command_ok, false);
  assert.equal(timedFinal.termination_reason, 'process_timeout');
  assert.equal(timedFinal.status, 'timed_out');
  assert.equal(timedFinal.process_timed_out, true);
  assert.equal(timedFinal.recoverable, true);
  assert.match(timedFinal.suggestion, /timeout_ms/);

  const lifecycle = new ProcessRequestLifecycle(state.ctx, 80);
  const detachedStart = await startAndYield(state.ctx, state.meta['openai/session'], {
    program: nodeProgram,
    args: ['-e', 'setTimeout(() => {}, 600000)'],
    tty: true,
    yield_time_ms: 0,
    timeout_ms: 600_000,
    operation_id: 'detached-operation'
  }, lifecycle);
  lifecycle.abort();
  const detachedSession = repoRuntime(state.ctx).sessions.get(detachedStart.session_id);
  assert.ok(detachedSession);
  const detachedDeadline = Date.now() + 180_000;
  while (!detachedSession.finalizedAt && Date.now() < detachedDeadline) {
    await waitForSession(detachedSession, detachedSession.sequence, 20_000, 'finalized');
  }
  assert.ok(detachedSession.finalizedAt, 'detached timeout must finalize through the normal lifecycle');
  const detachedFinal = await callTool(state.ctx, 'resolve_operation', {
    operation_id: 'detached-operation',
    output_mode: 'none'
  }, state.meta);
  assert.equal(detachedFinal.termination_reason, 'detached_timeout');
  assert.equal(detachedFinal.recoverable, true);
  assert.equal(detachedFinal.detached, false, 'resolve_operation reattaches and clears detached state');

  const cancelLifecycle = new ProcessRequestLifecycle(state.ctx, 250);
  const cancelStart = await startAndYield(state.ctx, state.meta['openai/session'], {
    program: nodeProgram,
    args: ['-e', 'setTimeout(() => {}, 120000)'],
    tty: true,
    yield_time_ms: 0,
    timeout_ms: 120_000,
    operation_id: 'cancel-detached-operation'
  }, cancelLifecycle);
  cancelLifecycle.abort();
  await new Promise(resolve => setTimeout(resolve, 40));
  const reattached = await callTool(state.ctx, 'resolve_operation', {
    operation_id: 'cancel-detached-operation',
    output_mode: 'none'
  }, state.meta);
  assert.equal(reattached.detached, false);
  await new Promise(resolve => setTimeout(resolve, 300));
  const stillRunning = repoRuntime(state.ctx).sessions.get(cancelStart.session_id);
  assert.ok(stillRunning && !stillRunning.endedAt);

  const killed = await callTool(state.ctx, 'kill_session', {
    session_id: cancelStart.session_id,
    signal: 'KILL',
    wait_ms: 5_000
  }, state.meta);
  assert.equal(killed.killed, true);
  assert.equal(killed.evicted, true);
  assert.equal(killed.status, 'killed');
  assert.equal(repoRuntime(state.ctx).sessions.has(cancelStart.session_id), false);
});

test('retained output pagination reports expiry, UTF-8 alignment and delta continuation', async t => {
  const clippedState = await fixture(t, 8);
  const clipped = await callTool(clippedState.ctx, 'exec_command', {
    program: nodeProgram,
    args: ['-e', 'process.stdout.write("A😀BCDEF")'],
    timeout_ms: 10_000,
    yield_time_ms: 10_000
  }, clippedState.meta);
  const expired = await callTool(clippedState.ctx, 'read_output', {
    output_ref: clipped.output_refs.stdout,
    offset: 0,
    limit: 3
  }, clippedState.meta);
  assert.equal(expired.cursor_expired, true);
  assert.ok(expired.retained_start_offset > 0);
  assert.equal(expired.content, 'BCD');
  assert.equal(expired.next_offset, expired.offset + 3);
  assert.equal(expired.truncated, true);
  assert.match(expired.warnings[0], /expired/);
  assert.equal(expired.total_stream_bytes, Buffer.byteLength('A😀BCDEF'));

  const state = await fixture(t);
  const alignedSource = await callTool(state.ctx, 'exec_command', {
    program: nodeProgram,
    args: ['-e', 'process.stdout.write("A😀B")'],
    timeout_ms: 10_000,
    yield_time_ms: 10_000
  }, state.meta);
  const aligned = await callTool(state.ctx, 'read_output', {
    output_ref: alignedSource.output_refs.stdout,
    offset: 2,
    limit: 8
  }, state.meta);
  assert.equal(aligned.offset, 5);
  assert.equal(aligned.content, 'B');
  assert.equal(aligned.next_offset, null);
  assert.match(aligned.warnings[0], /aligned/);

  const streamed = await callTool(state.ctx, 'exec_command', {
    program: nodeProgram,
    args: ['-e', `
      process.stdout.write('A'.repeat(700));
      process.stdin.once('data', () => {
        process.stdout.write('B'.repeat(700));
        process.exit(0);
      });
      setInterval(() => {}, 1000);
    `],
    tty: true,
    yield_time_ms: 0,
    timeout_ms: 10_000
  }, state.meta);
  const firstOutput = await callTool(state.ctx, 'wait_command', {
    session_id: streamed.session_id,
    cursor: 0,
    timeout_ms: 10_000,
    until: 'output_or_exit',
    output_mode: 'delta',
    max_output_bytes: 1024
  }, state.meta);
  assert.equal(firstOutput.event_detail, 'compact');
  assert.equal(firstOutput.events.length, 1);
  assert.equal(firstOutput.events[0].data, undefined);
  assert.equal(Buffer.byteLength(firstOutput.stdout), 700);
  await callTool(state.ctx, 'send_input', {
    session_id: streamed.session_id,
    chars: 'continue',
    close_stdin: true
  }, state.meta);

  const firstPage = await callTool(state.ctx, 'wait_command', {
    session_id: streamed.session_id,
    cursor: 0,
    timeout_ms: 10_000,
    until: 'finalized',
    output_mode: 'delta',
    event_detail: 'full',
    max_output_bytes: 1024
  }, state.meta);
  assert.equal(firstPage.event_detail, 'full');
  assert.equal(firstPage.command_ok, true);
  assert.equal(firstPage.has_more_output, true);
  assert.ok(firstPage.next_cursor < firstPage.latest_cursor);
  assert.equal(firstPage.events.length, 1);
  assert.equal(Buffer.byteLength(firstPage.events[0].data), 700);

  const secondPage = await callTool(state.ctx, 'wait_command', {
    session_id: streamed.session_id,
    cursor: firstPage.next_cursor,
    timeout_ms: 0,
    until: 'finalized',
    output_mode: 'delta',
    event_detail: 'full',
    max_output_bytes: 1024
  }, state.meta);
  assert.equal(secondPage.event_detail, 'full');
  assert.equal(secondPage.events.length, 1);
  assert.equal(Buffer.byteLength(secondPage.events[0].data), 700);
  assert.equal(secondPage.has_more_output, false);
  assert.equal(secondPage.next_cursor, secondPage.latest_cursor);
});

test('closing the Agent finalizes running sessions with server_restart recovery metadata', async t => {
  const root = await mkdtemp(path.join(tmpdir(), 'ctmcp-process-server-root-'));
  const dataDir = await mkdtemp(path.join(tmpdir(), 'ctmcp-process-server-data-'));
  const runtime = await createAgentRuntime(config(root, dataDir));
  const meta = { 'openai/session': 'process-server-close' };
  const selected = await callTool(runtime.context, 'switch_workspace_folder', { folder_id: 'repo' }, meta);
  assert.equal(selected.ok, true);
  await new Promise((resolve, reject) => {
    runtime.server.once('error', reject);
    runtime.server.listen(0, '127.0.0.1', resolve);
  });
  t.after(async () => {
    await runtime.close();
    await rm(root, { recursive: true, force: true, maxRetries: 5, retryDelay: 50 });
    await rm(dataDir, { recursive: true, force: true, maxRetries: 5, retryDelay: 50 });
  });

  const started = await callTool(runtime.context, 'exec_command', {
    program: nodeProgram,
    args: ['-e', 'setTimeout(() => {}, 60000)'],
    tty: true,
    yield_time_ms: 0,
    timeout_ms: 60_000,
    operation_id: 'server-close-operation'
  }, meta);
  const session = repoRuntime(runtime.context).sessions.get(started.session_id);
  assert.ok(session && !session.endedAt);

  await runtime.close();
  assert.ok(session.endedAt);
  assert.ok(session.finalizedAt, 'server close must wait for lifecycle finalization');
  const resolved = await callTool(runtime.context, 'resolve_operation', {
    operation_id: 'server-close-operation',
    output_mode: 'none'
  }, meta);
  assert.equal(resolved.termination_reason, 'server_restart');
  assert.equal(resolved.recoverable, true);
  assert.match(resolved.suggestion, /wait_command|重新执行|恢复/);
  assert.equal(resolved.process_still_running, false);
});

test('aborting a real MCP request starts detached grace and resolve_operation reattaches it', async t => {
  const root = await mkdtemp(path.join(tmpdir(), 'ctmcp-process-abort-root-'));
  const dataDir = await mkdtemp(path.join(tmpdir(), 'ctmcp-process-abort-data-'));
  const runtime = await createAgentRuntime(config(root, dataDir));
  const meta = { 'openai/session': 'process-http-abort' };
  const selected = await callTool(runtime.context, 'switch_workspace_folder', { folder_id: 'repo' }, meta);
  assert.equal(selected.ok, true);
  await new Promise((resolve, reject) => {
    runtime.server.once('error', reject);
    runtime.server.listen(0, '127.0.0.1', resolve);
  });
  t.after(async () => {
    if (runtime.server.listening) await new Promise(resolve => runtime.server.close(() => resolve()));
    await disposeProcessSessions(runtime.context);
    await rm(root, { recursive: true, force: true, maxRetries: 5, retryDelay: 50 });
    await rm(dataDir, { recursive: true, force: true, maxRetries: 5, retryDelay: 50 });
  });

  const address = runtime.server.address();
  assert.ok(address && typeof address === 'object');
  const base = `http://127.0.0.1:${address.port}`;
  const verifier = 'abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789-._~';
  const challenge = createHash('sha256').update(verifier).digest('base64url');
  const redirectUri = 'https://chatgpt.com/connector_platform_oauth_redirect';
  const authorized = runtime.oauth.authorizeSubmit(new URLSearchParams({
    client_id: 'chatgpt',
    redirect_uri: redirectUri,
    code_challenge: challenge,
    code_challenge_method: 'S256',
    state: 'abort-state',
    password: 'process-test-password'
  }), base);
  const code = new URL(authorized.location).searchParams.get('code');
  assert.ok(code);
  const tokenResponse = runtime.oauth.exchangeToken(new URLSearchParams({
    grant_type: 'authorization_code',
    code,
    redirect_uri: redirectUri,
    code_verifier: verifier,
    client_id: 'chatgpt'
  }), {}, base);
  const token = tokenResponse.body.access_token;
  assert.equal(typeof token, 'string');

  const payload = JSON.stringify({
    jsonrpc: '2.0',
    id: 1,
    method: 'tools/call',
    params: {
      name: 'exec_command',
      arguments: {
        program: nodeProgram,
        args: ['-e', 'setTimeout(() => {}, 10000)'],
        yield_time_ms: 30_000,
        timeout_ms: 10_000,
        operation_id: 'http-abort-operation'
      },
      _meta: meta
    }
  });
  let clientRequest;
  let clientResponse;
  const requestFinished = new Promise(resolve => {
    clientRequest = httpRequest(`${base}/mcp`, {
      method: 'POST',
      headers: {
        authorization: `Bearer ${token}`,
        'content-type': 'application/json',
        'content-length': Buffer.byteLength(payload)
      }
    }, response => {
      clientResponse = response;
      response.resume();
      response.once('end', resolve);
      response.once('error', resolve);
      response.once('close', resolve);
    });
    clientRequest.once('error', resolve);
    clientRequest.end(payload);
  });

  const session = await waitForValue(() => [...repoRuntime(runtime.context).sessions.values()]
    .find(candidate => candidate.operationId === 'http-abort-operation'));
  assert.ok(clientResponse, 'streaming response headers should arrive before process completion');
  clientResponse.destroy();
  clientRequest.destroy();
  await requestFinished;
  await waitForValue(() => session.detachedGeneration !== 0 && session);
  assert.notEqual(session.detachedGeneration, 0);

  const reattached = await callTool(runtime.context, 'resolve_operation', {
    operation_id: 'http-abort-operation',
    output_mode: 'none'
  }, meta);
  assert.equal(reattached.detached, false);
  assert.equal(reattached.attached_to_session_id, session.id);

  const killed = await callTool(runtime.context, 'kill_session', {
    session_id: session.id,
    signal: 'KILL',
    wait_ms: 5_000
  }, meta);
  assert.equal(killed.evicted, true);
});

test('finalized session retention expires old records and caps retained summaries', async t => {
  const state = await fixture(t);
  const baseResult = await callTool(state.ctx, 'exec_command', {
    program: nodeProgram,
    args: ['-e', 'process.exit(0)'],
    timeout_ms: 10_000,
    yield_time_ms: 10_000,
    operation_id: 'retention-base'
  }, state.meta);
  const base = repoRuntime(state.ctx).sessions.get(baseResult.session_id);
  assert.ok(base?.finalizedAt);

  const expired = {
    ...base,
    id: 'expired-session',
    operationId: 'expired-operation',
    fingerprint: 'expired-fingerprint',
    finalizedAt: Date.now() - FINALIZED_SESSION_RETENTION_MS,
    outputEvents: [],
    outputEventBytes: 0,
    postChecks: [],
    events: new EventEmitter(),
    timeoutTimer: undefined,
    detachedTimer: undefined,
    lockRelease: undefined
  };
  repoRuntime(state.ctx).sessions.set(expired.id, expired);
  repoRuntime(state.ctx).operationsByFingerprint.set(expired.fingerprint, expired.id);
  pruneProcessSessions(state.ctx);
  assert.equal(repoRuntime(state.ctx).sessions.has(expired.id), false);
  assert.equal(repoRuntime(state.ctx).operationsByFingerprint.has(expired.fingerprint), false);

  for (let index = 0; index < MAX_RETAINED_FINALIZED_SESSIONS + 5; index += 1) {
    const session = {
      ...base,
      id: `finalized-${index}`,
      operationId: `operation-${index}`,
      fingerprint: `fingerprint-${index}`,
      finalizedAt: Date.now() - (MAX_RETAINED_FINALIZED_SESSIONS + 5 - index),
      outputEvents: [],
      outputEventBytes: 0,
      postChecks: [],
      events: new EventEmitter(),
      timeoutTimer: undefined,
      detachedTimer: undefined,
      lockRelease: undefined
    };
    repoRuntime(state.ctx).sessions.set(session.id, session);
    repoRuntime(state.ctx).operationsByFingerprint.set(session.fingerprint, session.id);
  }
  pruneProcessSessions(state.ctx);
  const finalizedCount = [...repoRuntime(state.ctx).sessions.values()]
    .filter(session => session.finalizedAt).length;
  assert.equal(finalizedCount, MAX_RETAINED_FINALIZED_SESSIONS);

  const listed = await callTool(state.ctx, 'list_sessions', { include_finalized: true, limit: 1000 }, state.meta);
  assert.equal(listed.retention_seconds, FINALIZED_SESSION_RETENTION_MS / 1000);
  assert.equal(listed.include_finalized, true);
  assert.equal(listed.finalized_count, MAX_RETAINED_FINALIZED_SESSIONS);
});

test('exec_health_check matches the Rust worker, session, and stream-capture contract', async t => {
  const state = await fixture(t);
  const result = await callTool(state.ctx, 'exec_health_check', {}, state.meta);
  assert.equal(result.ok, true, JSON.stringify(result));
  assert.deepEqual(result.worker, { alive: true });
  assert.equal(result.session_create, true, JSON.stringify(result));
  assert.equal(result.command_run, true, JSON.stringify(result));
  assert.equal(result.stdout_capture, true, JSON.stringify(result));
  assert.equal(result.stderr_capture, true, JSON.stringify(result));
  assert.deepEqual(result.sandbox_verification, {
    required: false,
    verified: null,
    backend: null,
    execution_boundary: 'policy_only'
  });
  assert.equal(result.status, 'success');
  assert.deepEqual(result.next_actions, []);
  assert.ok(result.duration_ms >= 0);
  assert.equal(typeof result.probe.session_id, 'string');
  assert.equal(result.probe.status, 'exited');
  assert.equal(result.probe.process_still_running, false);
  assert.deepEqual(result.probe.sandbox_phase_durations_ms, {
    prepare_ms: null,
    startup_ms: null,
    cleanup_ms: null
  });
  assert.equal(Number(result.probe.process_exit_code ?? result.probe.exit_code), 0);
  assert.match(result.probe.stdout, /exec-health/);
  assert.match(result.probe.stderr, /exec-health-stderr/);
});

assert.equal(DETACHED_SESSION_GRACE_MS, 90_000);
