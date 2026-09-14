import test from 'node:test';
import assert from 'node:assert/strict';
import { mkdir, mkdtemp, rm, symlink, writeFile } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import path from 'node:path';
import { createToolContext } from '../dist/server.js';
import { callTool } from '../dist/tools.js';
import { defaultPolicy } from '../dist/policy.js';
import { redactToolOutput } from '../dist/redaction.js';
import {
  MAX_MCP_SUMMARY_BYTES,
  mcpResultSummary,
  normalizeToolResult,
  toolErrorResult,
  wrapMcpToolResult
} from '../dist/toolContract.js';

function config(root, dataDir) {
  return {
    host: '127.0.0.1',
    port: 0,
    dataDir,
    permissionMode: 'trusted',
    toolProfile: 'advanced',
    activeToolProfile: 'advanced',
    policy: defaultPolicy(),
    management: { enabled: false },
    oauth: {
      clientId: 'chatgpt',
      password: 'mcp-response-password',
      tokenSecret: 'mcp-response-contract-token-secret'
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

async function fixture(t) {
  const base = await mkdtemp(path.join(tmpdir(), 'ctmcp-response-contract-'));
  const root = path.join(base, 'root');
  const outside = path.join(base, 'outside');
  const dataDir = path.join(base, 'data');
  await mkdir(path.join(root, 'directory'), { recursive: true });
  await mkdir(outside, { recursive: true });
  await mkdir(dataDir, { recursive: true });
  await writeFile(path.join(root, 'file.txt'), 'visible\n');
  await writeFile(path.join(outside, 'secret.txt'), 'outside\n');
  await symlink(outside, path.join(root, 'outside-link'), process.platform === 'win32' ? 'junction' : 'dir');
  const ctx = await createToolContext(config(root, dataDir));
  const meta = { 'openai/session': `mcp-response-${Date.now()}-${Math.random()}` };
  t.after(async () => {
    await ctx.usageStore.flush();
    await rm(base, { recursive: true, force: true, maxRetries: 5, retryDelay: 50 });
  });
  return { base, root, outside, ctx, meta };
}

function assertErrorEnvelope(result, code, category, expectedStatus = 'error') {
  assert.equal(result.ok, false, JSON.stringify(result));
  assert.equal(result.status, expectedStatus, JSON.stringify(result));
  assert.equal(result.error?.code, code, JSON.stringify(result));
  assert.equal(result.error?.category, category, JSON.stringify(result));
  assert.equal(typeof result.error?.retryable, 'boolean', JSON.stringify(result));
  assert.ok(result.error?.details && typeof result.error.details === 'object');
  assert.equal(result.summary, result.error.message);
}

function legacyMcpResult(structured) {
  return {
    content: [{ type: 'text', text: JSON.stringify(structured) }],
    structuredContent: structured,
    isError: structured.ok === false
  };
}

test('large MCP success payloads keep one structured copy and a bounded concise summary', () => {
  const marker = 'PAYLOAD_MARKER_'.repeat(4096);
  const cases = [
    ['read_file', { ok: true, content: marker, total_bytes: marker.length }],
    ['wait_command', { ok: true, status: 'exited', stdout: marker, stderr: '', exit_code: 0 }],
    ['git_diff', { ok: true, diff: marker, files: [{ path: 'large.txt', additions: 1 }] }],
    ['project_map', { ok: true, content: marker, returned_count: 250 }],
    ['query_tool_usage', { ok: true, records: [{ tool: 'read_file', payload: marker }] }]
  ];

  for (const [toolName, structured] of cases) {
    const wrapped = wrapMcpToolResult(toolName, {}, structured);
    assert.equal(wrapped.isError, false);
    assert.deepEqual(wrapped.structuredContent, structured);
    assert.equal(wrapped.content.length, 1);
    assert.equal(wrapped.content[0].type, 'text');
    assert.ok(Buffer.byteLength(wrapped.content[0].text) <= MAX_MCP_SUMMARY_BYTES);
    assert.doesNotMatch(wrapped.content[0].text, /PAYLOAD_MARKER/);
    const actualBytes = Buffer.byteLength(JSON.stringify(wrapped));
    const legacyBytes = Buffer.byteLength(JSON.stringify(legacyMcpResult(structured)));
    assert.ok(actualBytes < legacyBytes * 0.7, `${toolName}: ${actualBytes} must be < 70% of ${legacyBytes}`);
  }
});

test('summary precedence and UTF-8 truncation match the Rust contract', () => {
  assert.equal(mcpResultSummary('project_map', { ok: true, returned_count: 7 }), 'project_map completed with 7 returned items.');
  assert.equal(mcpResultSummary('search_text', { ok: true, total_matches: 9 }), 'search_text completed with 9 matches.');
  assert.equal(mcpResultSummary('exec_many', { ok: true, commands_executed: 3 }), 'exec_many completed after executing 3 commands.');
  assert.equal(mcpResultSummary('exec_command', { ok: true, status: 'exited' }), 'exec_command status: exited.');
  const bounded = mcpResultSummary('read_file', { ok: true, summary: '測'.repeat(1000) });
  assert.ok(Buffer.byteLength(bounded) <= MAX_MCP_SUMMARY_BYTES);
  assert.match(bounded, /\.\.\.$/);
});

test('error normalizer preserves typed codes and maps filesystem errno without path leakage', () => {
  const policy = Object.assign(new Error('confirmation required'), {
    name: 'PolicyError',
    code: 'DANGEROUS_OPERATION_REQUIRES_CONFIRMATION'
  });
  assertErrorEnvelope(toolErrorResult(policy), 'DANGEROUS_OPERATION_REQUIRES_CONFIRMATION', 'policy');

  const filesystemCases = [
    ['ENOENT', 'NOT_FOUND', 'not_found'],
    ['ENOTDIR', 'NOT_A_DIRECTORY', 'validation'],
    ['EISDIR', 'IS_DIRECTORY', 'validation'],
    ['EACCES', 'PERMISSION_DENIED', 'security'],
    ['EPERM', 'PERMISSION_DENIED', 'security']
  ];
  for (const [fsCode, code, category] of filesystemCases) {
    const nativeError = Object.assign(new Error(`${fsCode}: secret path`), {
      code: fsCode,
      syscall: 'open',
      path: 'C:\\private\\secret.txt'
    });
    const mapped = toolErrorResult(nativeError);
    assertErrorEnvelope(mapped, code, category);
    assert.equal(mapped.error.details.fs_code, fsCode);
    assert.equal(mapped.error.details.syscall, 'open');
    assert.equal(mapped.error.details.path, undefined);
    assert.doesNotMatch(JSON.stringify(mapped), /private|secret\.txt/);
  }

  const normalized = normalizeToolResult({
    ok: false,
    status: 'unsupported',
    error: { code: 'EXAMPLE', message: 'example failure', category: 'validation', retryable: true, details: { field: 'x' } }
  });
  assertErrorEnvelope(normalized, 'EXAMPLE', 'validation', 'unsupported');
  assert.equal(normalized.error.retryable, true);
  assert.deepEqual(normalized.error.details, { field: 'x' });
});

test('direct tool calls expose stable workspace and filesystem error envelopes', async t => {
  const { root, ctx, meta } = await fixture(t);
  assertErrorEnvelope(await callTool(ctx, 'read_file', { path: 'file.txt' }, meta), 'WORKSPACE_FOLDER_NOT_SELECTED', 'workspace_routing');
  assertErrorEnvelope(await callTool(ctx, 'list_sessions', {}, meta), 'WORKSPACE_FOLDER_NOT_SELECTED', 'workspace_routing');
  assertErrorEnvelope(await callTool(ctx, 'switch_workspace_folder', { folder_id: 'missing' }, meta), 'WORKSPACE_FOLDER_NOT_FOUND', 'workspace_routing');

  const selected = await callTool(ctx, 'switch_workspace_folder', { folder_id: 'repo' }, meta);
  assert.equal(selected.ok, true, JSON.stringify(selected));

  assertErrorEnvelope(await callTool(ctx, 'read_file', { path: 'missing.txt' }, meta), 'NOT_FOUND', 'not_found');
  assertErrorEnvelope(await callTool(ctx, 'read_file', { path: 'directory' }, meta), 'IS_DIRECTORY', 'validation');
  assertErrorEnvelope(await callTool(ctx, 'set_default_cwd', { path: 'file.txt' }, meta), 'NOT_A_DIRECTORY', 'validation');
  assertErrorEnvelope(await callTool(ctx, 'read_file', { path: root }, meta), 'ABSOLUTE_PATH_DENIED', 'security');
  assertErrorEnvelope(await callTool(ctx, 'read_file', { path: '../outside/secret.txt' }, meta), 'PATH_OUTSIDE_WORKSPACE', 'security');
  assertErrorEnvelope(await callTool(ctx, 'read_file', { path: 'outside-link/secret.txt' }, meta), 'SYMLINK_ESCAPE', 'security');
  assertErrorEnvelope(await callTool(ctx, 'definitely_unknown_tool', {}, meta), 'UNKNOWN_TOOL', 'catalog');
});

test('redaction occurs before MCP summary and structured serialization', () => {
  const secret = 'mcp-contract-secret-value';
  const redacted = redactToolOutput('server_info', {}, {
    ok: false,
    error: {
      code: 'EXAMPLE_SECRET',
      message: `api_key=${secret}`,
      category: 'tool',
      retryable: false,
      details: { bearer_token: secret }
    }
  });
  const wrapped = wrapMcpToolResult('server_info', {}, redacted);
  const serialized = JSON.stringify(wrapped);
  assert.equal(wrapped.isError, true);
  assert.doesNotMatch(serialized, new RegExp(secret));
  assert.ok(wrapped.structuredContent.sensitive_data_redacted);
  assert.match(wrapped.content[0].text, /\[REDACTED\]/);
});

test('image results retain one encoded MCP payload and metadata-only structured content', () => {
  const data = Buffer.from('image-bytes').toString('base64');
  const structured = {
    ok: true,
    mime_type: 'image/png',
    width: 1,
    height: 1,
    base64: data,
    data_url: `data:image/png;base64,${data}`,
    content: [{ type: 'image', data, mimeType: 'image/png' }]
  };
  const wrapped = wrapMcpToolResult('view_image', {}, structured);
  assert.equal(wrapped.content.length, 1);
  assert.equal(wrapped.content[0].data, data);
  assert.equal(wrapped.structuredContent.base64, undefined);
  assert.equal(wrapped.structuredContent.data_url, undefined);
  assert.equal(wrapped.structuredContent.content, undefined);
  assert.equal(JSON.stringify(wrapped).split(data).length - 1, 1);
});

test('desktop screenshots promote top-level encoded payloads into MCP image content', () => {
  const data = Buffer.from('desktop-image-bytes').toString('base64');
  const wrapped = wrapMcpToolResult('desktop_screenshot', {}, {
    ok: true,
    mime_type: 'image/jpeg',
    width: 1920,
    height: 1080,
    bytes: Buffer.from(data, 'base64').byteLength,
    base64: data,
    data_url: `data:image/jpeg;base64,${data}`
  });

  assert.deepEqual(wrapped.content, [{ type: 'image', data, mimeType: 'image/jpeg' }]);
  assert.equal(wrapped.structuredContent.base64, undefined);
  assert.equal(wrapped.structuredContent.data_url, undefined);
  assert.equal(wrapped.structuredContent.mime_type, 'image/jpeg');
  assert.equal(JSON.stringify(wrapped).split(data).length - 1, 1);
});

test('multiple image blocks fall back to bounded text instead of duplicating image payloads', () => {
  const image = { type: 'image', data: 'aGVsbG8=', mimeType: 'image/png' };
  const wrapped = wrapMcpToolResult('view_image', {}, {
    ok: true,
    content: [image, image],
    mime_type: 'image/png'
  });
  assert.equal(wrapped.content.length, 1);
  assert.equal(wrapped.content[0].type, 'text');
  assert.ok(Buffer.byteLength(wrapped.content[0].text) <= MAX_MCP_SUMMARY_BYTES);
  assert.equal(wrapped.structuredContent.content.length, 2);
});
