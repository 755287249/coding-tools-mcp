import test from 'node:test';
import assert from 'node:assert/strict';
import { mkdtemp } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import path from 'node:path';

import { tools } from '../dist/catalog.js';
import { createToolContext } from '../dist/server.js';
import { callTool } from '../dist/tools.js';

const nodeProgram = path.basename(process.execPath);

function config(root, dataDir) {
  return {
    host: '127.0.0.1',
    port: 0,
    dataDir,
    permissionMode: 'trusted',
    oauth: {
      clientId: 'chatgpt',
      password: 'test-password',
      tokenSecret: 'a sufficiently long test token secret'
    },
    folders: [{ id: 'repo', name: 'Repo', path: root }],
    limits: {
      blockingConcurrency: 4,
      processConcurrency: 4,
      activeSessionLimit: 16,
      maxOutputBytes: 1024 * 1024
    }
  };
}

async function context() {
  const root = await mkdtemp(path.join(tmpdir(), 'ctmcp-secret-ref-'));
  const dataDir = await mkdtemp(path.join(tmpdir(), 'ctmcp-secret-ref-state-'));
  const ctx = await createToolContext(config(root, dataDir));
  const meta = { 'openai/session': `secret-ref-${Math.random().toString(36).slice(2)}` };
  const selected = await callTool(ctx, 'switch_workspace_folder', { folder_id: 'repo' }, meta);
  assert.equal(selected.ok, true);
  return { ctx, meta };
}

test('command schemas expose local secret references', () => {
  const exec = tools.find(tool => tool.name === 'exec_command');
  const execMany = tools.find(tool => tool.name === 'exec_many');
  assert.ok(exec);
  assert.ok(execMany);
  assert.equal(exec.inputSchema.properties.secret_env.type, 'object');
  assert.equal(exec.inputSchema.properties.stdin_secret.type, 'string');
  assert.equal(execMany.inputSchema.properties.commands.items.properties.secret_env.type, 'object');
  assert.equal(execMany.inputSchema.properties.commands.items.properties.stdin_secret.type, 'string');
});

test('exec_command injects local secret references without placing plaintext in tool arguments', async () => {
  const { ctx, meta } = await context();
  const secretValue = 'LOCAL_EXEC_SECRET_VALUE_12345';
  ctx.resolveSecret = name => name === 'admin_api_key' ? secretValue : undefined;

  const result = await callTool(ctx, 'exec_command', {
    program: nodeProgram,
    args: [
      '-e',
      'let input=\"\"; process.stdin.on(\"data\", c => input += c); process.stdin.on(\"end\", () => process.exit(process.env.ADMIN_API_KEY === input ? 0 : 7));'
    ],
    secret_env: { ADMIN_API_KEY: 'admin_api_key' },
    stdin_secret: 'admin_api_key',
    yield_time_ms: 30000,
    output_mode: 'all'
  }, meta);

  assert.equal(result.command_ok, true);
  assert.equal(result.exit_code, 0);
  assert.equal(result.stdout, '[REDACTED]');
  assert.equal(result.stderr, '[REDACTED]');
  assert.equal(result.sensitive_data_redacted, true);
  assert.doesNotMatch(JSON.stringify(result), new RegExp(secretValue));

  await ctx.usageStore.flush();
  const usage = await ctx.usageStore.query({
    scope: 'current_runtime',
    tools: ['exec_command'],
    exclude_tools: [],
    include_records: true,
    include_payloads: true
  });
  const serialized = JSON.stringify(usage.records.at(-1));
  assert.doesNotMatch(serialized, new RegExp(secretValue));
});

test('exec_command rejects missing or ambiguous local secret references', async () => {
  const { ctx, meta } = await context();
  ctx.resolveSecret = () => undefined;

  const missing = await callTool(ctx, 'exec_command', {
    program: nodeProgram,
    args: ['-e', 'process.exit(0)'],
    secret_env: { ADMIN_API_KEY: 'missing_secret' }
  }, meta);
  assert.equal(missing.ok, false);
  assert.equal(missing.error.code, 'SECRET_NOT_FOUND');

  ctx.resolveSecret = () => 'local-value';
  const ambiguousStdin = await callTool(ctx, 'exec_command', {
    program: nodeProgram,
    args: ['-e', 'process.exit(0)'],
    stdin: 'literal',
    stdin_secret: 'admin_api_key'
  }, meta);
  assert.equal(ambiguousStdin.ok, false);
  assert.equal(ambiguousStdin.error.code, 'INVALID_ARGUMENT');

  const ambiguousEnv = await callTool(ctx, 'exec_command', {
    program: nodeProgram,
    args: ['-e', 'process.exit(0)'],
    env: { ADMIN_API_KEY: 'literal' },
    secret_env: { ADMIN_API_KEY: 'admin_api_key' }
  }, meta);
  assert.equal(ambiguousEnv.ok, false);
  assert.equal(ambiguousEnv.error.code, 'INVALID_ARGUMENT');
});
