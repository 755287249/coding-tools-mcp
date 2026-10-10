import test from 'node:test';
import assert from 'node:assert/strict';
import { createHash } from 'node:crypto';
import { access, mkdtemp, rm } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { request as httpRequest } from 'node:http';
import path from 'node:path';
import {
  configuredToolProfile, normalizeToolProfile, resolveToolProfile,
  toolNamesForProfile, toolsForProfile, toolsetRevisionForProfile
} from '../dist/catalog.js';
import { normalizeConfig } from '../dist/config.js';
import { createAgentRuntime, createToolContext } from '../dist/server.js';
import { callTool } from '../dist/tools.js';

const verifier = 'abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789-._~';
const redirectUri = 'https://chatgpt.com/connector_platform_oauth_redirect';

function agentConfig(root, dataDir, toolProfile, permissionMode = 'trusted') {
  return {
    host: '127.0.0.1',
    port: 0,
    dataDir,
    permissionMode,
    toolProfile,
    activeToolProfile: resolveToolProfile(toolProfile, permissionMode),
    management: { enabled: false },
    oauth: {
      clientId: 'chatgpt',
      password: 'profile-test-passphrase',
      tokenSecret: 'profile-test-token-secret-that-is-long-enough'
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

async function temporaryWorkspace(t, prefix) {
  const root = await mkdtemp(path.join(tmpdir(), `${prefix}-root-`));
  const dataDir = await mkdtemp(path.join(tmpdir(), `${prefix}-data-`));
  const cleanup = [];
  t.after(async () => {
    for (const action of cleanup) await action();
    await rm(root, { recursive: true, force: true, maxRetries: 5, retryDelay: 50 });
    await rm(dataDir, { recursive: true, force: true, maxRetries: 5, retryDelay: 50 });
  });
  return { root, dataDir, cleanup: action => cleanup.push(action) };
}

async function requestLocal(url, options = {}) {
  return new Promise((resolve, reject) => {
    const request = httpRequest(url, {
      method: options.method ?? 'GET',
      headers: options.headers ?? {}
    }, response => {
      const chunks = [];
      response.on('data', chunk => chunks.push(Buffer.from(chunk)));
      response.once('error', reject);
      response.once('end', () => {
        const text = Buffer.concat(chunks).toString('utf8');
        resolve({
          status: response.statusCode ?? 0,
          headers: response.headers,
          text,
          json: () => JSON.parse(text)
        });
      });
    });
    request.once('error', reject);
    if (options.body !== undefined) {
      request.write(options.body instanceof URLSearchParams ? options.body.toString() : String(options.body));
    }
    request.end();
  });
}

async function rpc(endpoint, token, request) {
  const response = await requestLocal(endpoint, {
    method: 'POST',
    headers: { authorization: `Bearer ${token}`, 'content-type': 'application/json' },
    body: JSON.stringify(request)
  });
  assert.equal(response.status, 200);
  return response.json();
}

async function authorize(localBase) {
  const challenge = createHash('sha256').update(verifier).digest('base64url');
  const form = new URLSearchParams({
    client_id: 'chatgpt',
    redirect_uri: redirectUri,
    code_challenge: challenge,
    code_challenge_method: 'S256',
    state: 'profile-state',
    password: 'profile-test-passphrase'
  });
  const authorized = await requestLocal(`${localBase}/oauth/authorize`, {
    method: 'POST',
    headers: { 'content-type': 'application/x-www-form-urlencoded' },
    body: form,
    redirect: 'manual'
  });
  assert.equal(authorized.status, 303);
  const code = new URL(String(authorized.headers.location), localBase).searchParams.get('code');
  assert.ok(code);
  const tokenResponse = await requestLocal(`${localBase}/oauth/token`, {
    method: 'POST',
    headers: { 'content-type': 'application/x-www-form-urlencoded' },
    body: new URLSearchParams({
      grant_type: 'authorization_code',
      code,
      redirect_uri: redirectUri,
      code_verifier: verifier,
      client_id: 'chatgpt'
    })
  });
  assert.equal(tokenResponse.status, 200);
  return (await tokenResponse.json()).access_token;
}

test('generated profile catalogs match Rust counts, membership and revisions', () => {
  const expectedCounts = {
    advanced: 71,
    'read-only': 18,
    'compat-readonly-all': 71,
    'guarded-core': 47,
    'trusted-core': 46
  };
  for (const [profile, count] of Object.entries(expectedCounts)) {
    const tools = toolsForProfile(profile);
    const names = tools.map(tool => tool.name);
    assert.equal(tools.length, count, profile);
    assert.equal(new Set(names).size, count, profile);
    assert.match(toolsetRevisionForProfile(profile), /^[0-9a-f]{16}$/);
  }

  const advanced = new Set(toolNamesForProfile('advanced'));
  const trusted = new Set(toolNamesForProfile('trusted-core'));
  const guarded = new Set(toolNamesForProfile('guarded-core'));
  const readOnly = new Set(toolNamesForProfile('read-only'));
  assert.equal(trusted.has('chat_discuss'), true);
  assert.equal(guarded.has('chat_discuss'), true);
  assert.equal(readOnly.has('chat_discuss'), false);
  assert.equal(trusted.has('chat_upload'), true);
  assert.equal(guarded.has('chat_upload'), true);
  assert.equal(readOnly.has('chat_upload'), false);
  assert.equal(trusted.has('conversation_bootstrap'), true);
  assert.equal(guarded.has('conversation_bootstrap'), true);
  assert.equal(readOnly.has('conversation_bootstrap'), false);
  assert.ok([...trusted].every(name => advanced.has(name)));
  assert.ok([...readOnly].every(name => advanced.has(name)));
  assert.deepEqual([...guarded].filter(name => !trusted.has(name)), ['request_permissions']);
  assert.equal(trusted.has('start_task'), false);
  assert.equal(advanced.has('fail_task'), true);
  assert.equal(advanced.has('close_failed_task'), true);
  assert.equal(advanced.has('rollback_task'), true);
  assert.equal(trusted.has('fail_task'), false);
  assert.equal(readOnly.has('switch_workspace_folder'), false);
  assert.equal(readOnly.has('read_file'), true);
});

test('compat-readonly-all keeps all tools while rewriting annotations', () => {
  const advanced = new Map(toolsForProfile('advanced').map(tool => [tool.name, tool]));
  const compatibility = toolsForProfile('compat-readonly-all');
  assert.equal(compatibility.length, advanced.size);
  for (const tool of compatibility) {
    assert.equal(tool.annotations.readOnlyHint, true, tool.name);
    assert.equal(tool.annotations.destructiveHint, false, tool.name);
    assert.equal(tool.annotations.idempotentHint, true, tool.name);
    assert.equal(tool.annotations.openWorldHint, false, tool.name);
    assert.deepEqual(tool.inputSchema, advanced.get(tool.name).inputSchema, tool.name);
  }
  assert.notEqual(toolsetRevisionForProfile('compat-readonly-all'), toolsetRevisionForProfile('advanced'));
});

test('core and unknown settings resolve like Rust permission-aware profiles', () => {
  assert.equal(configuredToolProfile('core'), 'core');
  assert.equal(configuredToolProfile('unknown-profile'), 'core');
  assert.equal(normalizeToolProfile('core'), 'trusted-core');
  assert.equal(normalizeToolProfile('unknown-profile'), 'trusted-core');
  assert.equal(resolveToolProfile('core', 'trusted'), 'trusted-core');
  assert.equal(resolveToolProfile('trusted-core', 'dangerous'), 'trusted-core');
  assert.equal(resolveToolProfile('core', 'guarded'), 'guarded-core');
  assert.equal(resolveToolProfile('trusted-core', 'read-only'), 'guarded-core');
  assert.equal(resolveToolProfile('advanced', 'read-only'), 'advanced');
  assert.equal(resolveToolProfile('read-only', 'dangerous'), 'read-only');
});

test('configuration defaults to core and environment overrides the saved profile', () => {
  const document = {
    schema_version: 1,
    dataDir: path.join(tmpdir(), 'ctmcp-profile-config'),
    permissionMode: 'trusted',
    folders: [{ id: 'repo', name: 'Repo', path: tmpdir() }]
  };
  const secrets = { oauthPassword: 'password', oauthTokenSecret: 'token' };
  const base = normalizeConfig(document, secrets, {});
  assert.equal(base.toolProfile, 'core');
  assert.equal(base.activeToolProfile, 'trusted-core');

  const guarded = normalizeConfig({ ...document, permissionMode: 'guarded', toolProfile: 'core' }, secrets, {});
  assert.equal(guarded.toolProfile, 'core');
  assert.equal(guarded.activeToolProfile, 'guarded-core');

  const overridden = normalizeConfig({ ...document, toolProfile: 'read-only' }, secrets, {
    CTMCP_TOOL_PROFILE: 'advanced',
    CTMCP_PERMISSION_MODE: 'read-only'
  });
  assert.equal(overridden.toolProfile, 'advanced');
  assert.equal(overridden.activeToolProfile, 'advanced');
});

test('direct callTool rejects hidden tools before workspace side effects', async t => {
  const { root, dataDir, cleanup } = await temporaryWorkspace(t, 'ctmcp-profile-direct');
  const ctx = await createToolContext(agentConfig(root, dataDir, 'read-only'));
  cleanup(() => ctx.usageStore.flush());
  const meta = { 'openai/session': 'profile-direct' };
  const target = path.join(root, 'blocked.txt');
  const hidden = await callTool(ctx, 'edit_file', {
    path: 'blocked.txt',
    edits: [{ type: 'replace', old_text: '', new_text: 'must-not-exist' }]
  }, meta);
  assert.equal(hidden.ok, false);
  assert.equal(hidden.error.code, 'UNKNOWN_TOOL');
  assert.equal(hidden.error.category, 'catalog');
  assert.equal(hidden.error.details.tool_profile, 'read-only');
  assert.equal(hidden.error.details.available_tools.length, 18);
  await assert.rejects(access(target));

  const info = await callTool(ctx, 'server_info', {}, meta);
  assert.equal(info.ok, true);
  assert.equal(info.tool_profile, 'read-only');
  assert.equal(info.configured_tool_profile, 'read-only');
  assert.equal(info.tool_count, 18);
  assert.deepEqual(info.tools, toolNamesForProfile('read-only'));
  assert.equal(info.toolset_revision, toolsetRevisionForProfile('read-only'));
});

test('HTTP tools/list, server_info and mcp/info expose one profile contract', async t => {
  const { root, dataDir, cleanup } = await temporaryWorkspace(t, 'ctmcp-profile-http');
  const runtime = await createAgentRuntime(agentConfig(root, dataDir, 'guarded-core', 'guarded'));
  const { server } = runtime;
  await new Promise(resolve => server.listen(0, '127.0.0.1', resolve));
  cleanup(async () => {
    await new Promise(resolve => server.close(resolve));
    await runtime.context.usageStore.flush();
  });
  const address = server.address();
  assert.ok(address && typeof address === 'object');
  const localBase = `http://127.0.0.1:${address.port}`;
  const expectedNames = toolNamesForProfile('guarded-core');
  const expectedRevision = toolsetRevisionForProfile('guarded-core');

  const health = (await requestLocal(`${localBase}/health`)).json();
  assert.equal(health.toolProfile, 'guarded-core');
  assert.equal(health.tools, expectedNames.length);
  assert.equal(health.toolsetRevision, expectedRevision);

  const mcpInfo = (await requestLocal(`${localBase}/mcp/info`)).json();
  assert.equal(mcpInfo.toolProfile, 'guarded-core');
  assert.deepEqual(mcpInfo.tools, expectedNames);
  assert.equal(mcpInfo.toolsetRevision, expectedRevision);

  const token = await authorize(localBase);
  const endpoint = `${localBase}/mcp`;
  const listed = await rpc(endpoint, token, {
    jsonrpc: '2.0', id: 1, method: 'tools/list', params: {}
  });
  assert.deepEqual(listed.result.tools.map(tool => tool.name), expectedNames);
  assert.equal(listed.result.toolsetRevision, expectedRevision);

  const serverInfo = await rpc(endpoint, token, {
    jsonrpc: '2.0', id: 2, method: 'tools/call',
    params: { name: 'server_info', arguments: {}, _meta: { 'openai/session': 'profile-http' } }
  });
  assert.equal(serverInfo.result.structuredContent.tool_profile, 'guarded-core');
  assert.deepEqual(serverInfo.result.structuredContent.tools, expectedNames);
  assert.equal(serverInfo.result.structuredContent.toolset_revision, expectedRevision);

  const hidden = await rpc(endpoint, token, {
    jsonrpc: '2.0', id: 3, method: 'tools/call',
    params: { name: 'start_task', arguments: {}, _meta: { 'openai/session': 'profile-http' } }
  });
  assert.equal(hidden.error.code, -32602);
  assert.equal(hidden.error.data.error_code, 'UNKNOWN_TOOL');
  assert.equal(hidden.error.data.toolset_revision, expectedRevision);
  assert.deepEqual(hidden.error.data.available_tools, expectedNames);

  runtime.context.config.toolProfile = 'advanced';
  runtime.context.config.activeToolProfile = 'advanced';
  const advancedNames = toolNamesForProfile('advanced');
  const advancedRevision = toolsetRevisionForProfile('advanced');

  const updatedHealth = (await requestLocal(`${localBase}/health`)).json();
  assert.equal(updatedHealth.toolProfile, 'advanced');
  assert.equal(updatedHealth.tools, advancedNames.length);
  assert.equal(updatedHealth.toolsetRevision, advancedRevision);

  const updatedInfo = (await requestLocal(`${localBase}/mcp/info`)).json();
  assert.equal(updatedInfo.toolProfile, 'advanced');
  assert.deepEqual(updatedInfo.tools, advancedNames);
  assert.equal(updatedInfo.toolsetRevision, advancedRevision);

  const updatedList = await rpc(endpoint, token, {
    jsonrpc: '2.0', id: 4, method: 'tools/list', params: {}
  });
  assert.deepEqual(updatedList.result.tools.map(tool => tool.name), advancedNames);
  assert.equal(updatedList.result.toolsetRevision, advancedRevision);

  const newlyExposed = await rpc(endpoint, token, {
    jsonrpc: '2.0', id: 5, method: 'tools/call',
    params: { name: 'start_task', arguments: {}, _meta: { 'openai/session': 'profile-http' } }
  });
  assert.equal(newlyExposed.error, undefined);
  assert.ok(newlyExposed.result);

  runtime.context.config.toolProfile = 'read-only';
  runtime.context.config.activeToolProfile = 'read-only';
  const hiddenAgain = await rpc(endpoint, token, {
    jsonrpc: '2.0', id: 6, method: 'tools/call',
    params: { name: 'start_task', arguments: {}, _meta: { 'openai/session': 'profile-http' } }
  });
  assert.equal(hiddenAgain.error.code, -32602);
  assert.equal(hiddenAgain.error.data.toolset_revision, toolsetRevisionForProfile('read-only'));
  assert.deepEqual(hiddenAgain.error.data.available_tools, toolNamesForProfile('read-only'));
});
