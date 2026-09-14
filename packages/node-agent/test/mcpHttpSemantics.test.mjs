import test from 'node:test';
import assert from 'node:assert/strict';
import { readFile } from 'node:fs/promises';
import { fileURLToPath } from 'node:url';
import { createMcpFixture, mcpRequest, responseJson } from './mcpTestHelpers.mjs';

const ping = { jsonrpc: '2.0', id: 1, method: 'ping', params: {} };

test('MCP HTTP methods validate connection and auth before returning Allow: POST', async t => {
  const state = await createMcpFixture(t);
  const unauthorized = await mcpRequest(state, undefined, {
    auth: false,
    method: 'GET',
    body: false
  });
  assert.equal(unauthorized.status, 401);
  assert.match(unauthorized.headers.get('www-authenticate') ?? '', /^Bearer /);
  assert.equal(unauthorized.headers.get('cache-control'), 'no-store');

  for (const method of ['GET', 'DELETE', 'PATCH']) {
    const response = await mcpRequest(state, undefined, { method, body: false });
    assert.equal(response.status, 405, method);
    assert.equal(response.headers.get('allow'), 'POST');
    assert.equal(response.headers.get('cache-control'), 'no-store');
    assert.equal(await response.text(), '');
  }

  const badOrigin = await mcpRequest(state, undefined, {
    method: 'GET',
    body: false,
    headers: { origin: 'https://attacker.example' }
  });
  assert.equal(badOrigin.status, 403);
  assert.equal((await responseJson(badOrigin)).error.code, -32000);
});

test('MCP notifications and client responses return 202 without an RPC body', async t => {
  const state = await createMcpFixture(t);
  const fixtures = [
    { jsonrpc: '2.0', method: 'notifications/initialized', params: {} },
    { jsonrpc: '2.0', method: 'unknown/notification', params: {} },
    { jsonrpc: '2.0', id: 11, result: { ok: true } },
    { jsonrpc: '2.0', id: 12, error: { code: -1, message: 'client-side' } }
  ];
  for (const payload of fixtures) {
    const response = await mcpRequest(state, payload);
    assert.equal(response.status, 202, JSON.stringify(payload));
    assert.equal(response.headers.get('cache-control'), 'no-store');
    assert.equal(await response.text(), '');
  }
});

test('transport errors use HTTP 400 or 403 while JSON-RPC method errors remain HTTP 200', async t => {
  const state = await createMcpFixture(t);
  const invalid = await mcpRequest(state, { jsonrpc: '2.0', id: 1 });
  assert.equal(invalid.status, 400);
  assert.equal((await responseJson(invalid)).error.code, -32600);

  const forbidden = await mcpRequest(state, ping, {
    headers: { origin: 'https://attacker.example' }
  });
  assert.equal(forbidden.status, 403);
  assert.equal((await responseJson(forbidden)).error.code, -32000);

  const unknown = await mcpRequest(state, {
    jsonrpc: '2.0', id: 13, method: 'unknown/method', params: {}
  });
  assert.equal(unknown.status, 200);
  assert.equal(unknown.headers.get('x-coding-tools-streaming'), '1');
  assert.equal(unknown.headers.get('x-accel-buffering'), 'no');
  const body = await responseJson(unknown);
  assert.equal(body.id, 13);
  assert.equal(body.error.code, -32601);
  assert.equal(body.error.message, 'Method not found: unknown/method');
});

test('MCP 2026-07-28 supports stateless discovery and cacheable tool listing', async t => {
  const state = await createMcpFixture(t);
  const packagePath = fileURLToPath(new URL('../package.json', import.meta.url));
  const packageMetadata = JSON.parse(await readFile(packagePath, 'utf8'));
  const meta = {
    'io.modelcontextprotocol/protocolVersion': '2026-07-28',
    'io.modelcontextprotocol/clientCapabilities': {},
    'io.modelcontextprotocol/clientInfo': { name: 'node-agent-test', version: '1.0.0' }
  };
  const headers = method => ({
    'mcp-protocol-version': '2026-07-28',
    'mcp-method': method
  });

  const discoverResponse = await mcpRequest(state, {
    jsonrpc: '2.0', id: 'discover', method: 'server/discover', params: { _meta: meta }
  }, { headers: headers('server/discover') });
  assert.equal(discoverResponse.status, 200);
  const discover = (await responseJson(discoverResponse)).result;
  assert.equal(discover.resultType, 'complete');
  assert.deepEqual(discover.supportedVersions, ['2026-07-28']);
  assert.deepEqual(discover.capabilities, {
    tools: { listChanged: false },
    prompts: { listChanged: false },
    resources: { subscribe: false, listChanged: false },
    extensions: { 'io.modelcontextprotocol/tasks': {} }
  });
  assert.equal(discover.ttlMs, 0);
  assert.equal(discover.cacheScope, 'private');
  assert.equal(discover._meta['io.modelcontextprotocol/serverInfo'].name, 'coding-tools-mcp-node');
  assert.equal(discover._meta['io.modelcontextprotocol/serverInfo'].version, packageMetadata.version);

  const promptsResponse = await mcpRequest(state, {
    jsonrpc: '2.0', id: 'prompts', method: 'prompts/list', params: { _meta: meta }
  }, { headers: headers('prompts/list') });
  assert.equal(promptsResponse.status, 200);
  const prompts = (await responseJson(promptsResponse)).result;
  assert.ok(Array.isArray(prompts.prompts));
  assert.equal(prompts.resultType, 'complete');
  assert.equal(prompts.ttlMs, 0);
  assert.equal(prompts.cacheScope, 'private');

  const resourcesResponse = await mcpRequest(state, {
    jsonrpc: '2.0', id: 'resources', method: 'resources/list', params: { _meta: meta }
  }, { headers: headers('resources/list') });
  assert.equal(resourcesResponse.status, 200);
  const resources = (await responseJson(resourcesResponse)).result;
  assert.ok(Array.isArray(resources.resources));
  assert.equal(resources.resultType, 'complete');
  assert.equal(resources.ttlMs, 0);
  assert.equal(resources.cacheScope, 'private');

  const toolsResponse = await mcpRequest(state, {
    jsonrpc: '2.0', id: 'tools', method: 'tools/list', params: { _meta: meta }
  }, { headers: headers('tools/list') });
  assert.equal(toolsResponse.status, 200);
  const tools = (await responseJson(toolsResponse)).result;
  assert.equal(tools.resultType, 'complete');
  assert.ok(Array.isArray(tools.tools));
  assert.equal(tools.ttlMs, 0);
  assert.equal(tools.cacheScope, 'private');
  assert.equal(tools._meta['io.modelcontextprotocol/serverInfo'].version, packageMetadata.version);

  const legacyInitialize = await mcpRequest(state, {
    jsonrpc: '2.0', id: 'legacy', method: 'initialize', params: { protocolVersion: '2026-07-28' }
  });
  const legacyInitializeResult = (await responseJson(legacyInitialize)).result;
  assert.equal(legacyInitializeResult.protocolVersion, '2025-11-25');
  assert.equal(legacyInitializeResult.serverInfo.version, packageMetadata.version);

  const modernInitialize = await mcpRequest(state, {
    jsonrpc: '2.0', id: 'modern-init', method: 'initialize', params: { protocolVersion: '2026-07-28', _meta: meta }
  }, { headers: headers('initialize') });
  const modernInitializeBody = await responseJson(modernInitialize);
  assert.equal(modernInitialize.status, 404);
  assert.equal(modernInitializeBody.error.code, -32601);

  const modernPing = await mcpRequest(state, {
    jsonrpc: '2.0', id: 'modern-ping', method: 'ping', params: { _meta: meta }
  }, { headers: headers('ping') });
  const modernPingBody = await responseJson(modernPing);
  assert.equal(modernPing.status, 404);
  assert.equal(modernPingBody.error.code, -32601);

  const legacyDiscover = await mcpRequest(state, {
    jsonrpc: '2.0', id: 'legacy-discover', method: 'server/discover', params: {}
  }, { headers: { 'mcp-protocol-version': '2025-11-25' } });
  const legacyDiscoverBody = await responseJson(legacyDiscover);
  assert.equal(legacyDiscover.status, 200);
  assert.equal(legacyDiscoverBody.error.code, -32601);
});

test('MCP discovery reports streamable HTTP and the complete supported protocol set', async t => {
  const state = await createMcpFixture(t);
  const response = await fetch(state.infoEndpoint);
  assert.equal(response.status, 200);
  assert.equal(response.headers.get('cache-control'), 'no-store');
  const info = await responseJson(response);
  assert.equal(info.transport, 'streamable-http');
  assert.equal(info.protocolVersion, '2026-07-28');
  assert.deepEqual(info.supportedProtocolVersions, ['2026-07-28', '2025-11-25', '2025-06-18', '2025-03-26']);
  assert.equal(info.name, 'coding-tools-mcp-node');
});
