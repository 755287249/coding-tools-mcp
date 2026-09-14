import test from 'node:test';
import assert from 'node:assert/strict';
import path from 'node:path';
import {
  LATEST_MCP_PROTOCOL_VERSION,
  SUPPORTED_MCP_PROTOCOL_VERSIONS,
  mcpOriginAllowed,
  validateModernMcpToolHeaders
} from '../dist/mcpTransport.js';
import { forwardedTunnelRequestHeaders } from '../dist/tunnel.js';
import { createMcpFixture, mcpRequest, responseJson } from './mcpTestHelpers.mjs';

const ping = { jsonrpc: '2.0', id: 1, method: 'ping', params: {} };
const nodeProgram = path.basename(process.execPath);
const tasksExtension = 'io.modelcontextprotocol/tasks';

async function assertTransportError(response, status, code, message, expectedId = null, expectedData = undefined) {
  assert.equal(response.status, status);
  assert.equal(response.headers.get('cache-control'), 'no-store');
  const body = await responseJson(response);
  assert.equal(body.jsonrpc, '2.0');
  assert.equal(body.id, expectedId);
  assert.equal(body.error.code, code);
  assert.equal(body.error.message, message);
  if (expectedData !== undefined) assert.deepEqual(body.error.data, expectedData);
}

function modernRequest(method, id = 1, params = {}) {
  return {
    jsonrpc: '2.0',
    id,
    method,
    params: {
      ...params,
      _meta: {
        'io.modelcontextprotocol/protocolVersion': '2026-07-28',
        'io.modelcontextprotocol/clientCapabilities': {},
        'io.modelcontextprotocol/clientInfo': { name: 'node-agent-test', version: '1.0.0' }
      }
    }
  };
}

function modernHeaders(method, name) {
  return {
    'mcp-protocol-version': '2026-07-28',
    'mcp-method': method,
    ...(name === undefined ? {} : { 'mcp-name': name })
  };
}

function modernTaskRequest(method, id, params, session) {
  const request = modernRequest(method, id, params);
  request.params._meta['io.modelcontextprotocol/clientCapabilities'] = {
    extensions: { [tasksExtension]: {} }
  };
  request.params._meta['openai/session'] = session;
  return request;
}

test('MCP Origin validation matches listener, public and ChatGPT allowlists', async t => {
  assert.equal(mcpOriginAllowed('http://192.0.2.10:4321', { host: '0.0.0.0', publicBaseUrl: '' }, 4321), true);
  assert.equal(mcpOriginAllowed('http://localhost:4321', { host: '0.0.0.0', publicBaseUrl: '' }, 4321), true);
  assert.equal(mcpOriginAllowed('http://192.0.2.10:4321', { host: '192.0.2.10', publicBaseUrl: '' }, 4321), true);
  assert.equal(mcpOriginAllowed('http://192.0.2.11:4321', { host: '192.0.2.10', publicBaseUrl: '' }, 4321), false);
  assert.equal(mcpOriginAllowed('http://localhost:4321', { host: '192.0.2.10', publicBaseUrl: '' }, 4321), false);
  assert.equal(mcpOriginAllowed('http://127.0.0.2:4321', { host: '127.0.0.1', publicBaseUrl: '' }, 4321), true);
  assert.equal(mcpOriginAllowed('http://[2001:db8::1]:4321', { host: '::', publicBaseUrl: '' }, 4321), true);
  assert.equal(mcpOriginAllowed('http://localhost:4321', { host: '::1', publicBaseUrl: '' }, 4321), true);
  const state = await createMcpFixture(t);
  const acceptedOrigins = [
    undefined,
    `${state.localBase}/ignored/path`,
    `http://localhost:${state.port}`,
    `${new URL(state.config.publicBaseUrl).origin}/another/path`,
    'https://chatgpt.com',
    'https://chat.openai.com'
  ];
  for (const origin of acceptedOrigins) {
    const response = await mcpRequest(state, ping, {
      headers: origin === undefined ? {} : { origin }
    });
    assert.equal(response.status, 200, origin ?? 'missing Origin');
    assert.deepEqual((await responseJson(response)).result, {});
  }

  const rejectedOrigins = [
    'https://attacker.example',
    `http://127.0.0.1:${state.port + 1}`,
    `https://127.0.0.1:${state.port}`,
    `http://example.test:${state.port}`,
    'null'
  ];
  for (const origin of rejectedOrigins) {
    await assertTransportError(
      await mcpRequest(state, ping, { headers: { origin } }),
      403,
      -32000,
      'Invalid Origin header'
    );
  }

  assert.equal(mcpOriginAllowed(undefined, state.config, state.port), true);
  assert.equal(mcpOriginAllowed('https://attacker.example', state.config, state.port), false);
  assert.equal(mcpOriginAllowed(' https://chatgpt.com ', state.config, state.port), true);
});

test('MCP protocol header accepts modern and legacy revisions and rejects invalid values before dispatch', async t => {
  const state = await createMcpFixture(t);
  assert.equal(LATEST_MCP_PROTOCOL_VERSION, '2026-07-28');
  assert.deepEqual([...SUPPORTED_MCP_PROTOCOL_VERSIONS], ['2026-07-28', '2025-11-25', '2025-06-18', '2025-03-26']);
  for (const version of SUPPORTED_MCP_PROTOCOL_VERSIONS.filter(value => value !== '2026-07-28')) {
    const response = await mcpRequest(state, ping, {
      headers: { 'mcp-protocol-version': version }
    });
    assert.equal(response.status, 200, version);
  }
  const modern = await mcpRequest(state, modernRequest('ping'), { headers: modernHeaders('ping') });
  assert.equal(modern.status, 404);
  assert.equal((await responseJson(modern)).error.code, -32601);

  await assertTransportError(
    await mcpRequest(state, ping, { headers: { 'mcp-protocol-version': '2024-01-01' } }),
    400,
    -32022,
    'Unsupported MCP protocol version: 2024-01-01',
    null,
    {
      supported: ['2026-07-28', '2025-11-25', '2025-06-18', '2025-03-26'],
      requested: '2024-01-01'
    }
  );
  await assertTransportError(
    await mcpRequest(state, ping, { headers: { 'mcp-protocol-version': '' } }),
    400,
    -32022,
    'Unsupported MCP protocol version: ',
    null,
    {
      supported: ['2026-07-28', '2025-11-25', '2025-06-18', '2025-03-26'],
      requested: ''
    }
  );
  await assertTransportError(
    await mcpRequest(state, ping, { headers: { 'mcp-protocol-version': '2025-11-25, 2025-06-18' } }),
    400,
    -32022,
    'Unsupported MCP protocol version: 2025-11-25, 2025-06-18',
    null,
    {
      supported: ['2026-07-28', '2025-11-25', '2025-06-18', '2025-03-26'],
      requested: '2025-11-25, 2025-06-18'
    }
  );
});

test('MCP runtime and HTTP boundary emit canonical lifecycle service and transport diagnostics', async t => {
  const state = await createMcpFixture(t);
  await assertTransportError(
    await mcpRequest(state, ping, { headers: { 'mcp-protocol-version': '2024-01-01' } }),
    400,
    -32022,
    'Unsupported MCP protocol version: 2024-01-01',
    null,
    {
      supported: ['2026-07-28', '2025-11-25', '2025-06-18', '2025-03-26'],
      requested: '2024-01-01'
    }
  );
  await state.runtime.context.usageStore.flush();
  let queried = await state.runtime.context.usageStore.query({ scope: 'all', exclude_tools: [] });
  assert.equal(queried.diagnostics_summary.events_by_type.lifecycle_event, 1);
  assert.equal(queried.diagnostics_summary.events_by_type.service_event, 1);
  assert.equal(queried.diagnostics_summary.events_by_type.transport_event, 1);
  assert.equal(queried.diagnostics_summary.transport_errors, 1);
  assert.deepEqual(queried.diagnostics_summary.top_transport_errors, [{ code: '-32022', count: 1 }]);

  state.runtime.server.closeAllConnections?.();
  await state.runtime.close();
  queried = await state.runtime.context.usageStore.query({ scope: 'all', exclude_tools: [] });
  assert.equal(queried.diagnostics_summary.events_by_type.lifecycle_event, 2);
  assert.equal(queried.diagnostics_summary.events_by_type.service_event, 2);
  assert.equal(queried.diagnostics_summary.events_by_type.transport_event, 1);
});

test('MCP 2026-07-28 rejects client JSON-RPC responses and unknown request methods at the HTTP boundary', async t => {
  const state = await createMcpFixture(t);

  await assertTransportError(
    await mcpRequest(state, { jsonrpc: '2.0', id: 18, result: {} }, {
      headers: { 'mcp-protocol-version': '2026-07-28' }
    }),
    400,
    -32600,
    'Streamable HTTP accepts only JSON-RPC requests or notifications from clients',
    18
  );

  const unknown = await mcpRequest(
    state,
    modernRequest('unknown/method', 19),
    { headers: modernHeaders('unknown/method') }
  );
  assert.equal(unknown.status, 404);
  const body = await responseJson(unknown);
  assert.equal(body.id, 19);
  assert.equal(body.error.code, -32601);
  assert.equal(body.error.message, 'Method not found: unknown/method');

  const legacyResponse = await mcpRequest(
    state,
    { jsonrpc: '2.0', id: 20, result: {} },
    { headers: { 'mcp-protocol-version': '2025-11-25' } }
  );
  assert.equal(legacyResponse.status, 202);
});

test('MCP 2026-07-28 validates per-request metadata and standard routing headers', async t => {
  const state = await createMcpFixture(t);

  await assertTransportError(
    await mcpRequest(state, modernRequest('ping', 21), {
      headers: { 'mcp-protocol-version': '2026-07-28' }
    }),
    400,
    -32020,
    'Header mismatch: Mcp-Method header is required',
    21
  );

  await assertTransportError(
    await mcpRequest(state, modernRequest('ping', 22), {
      headers: modernHeaders('tools/list')
    }),
    400,
    -32020,
    "Header mismatch: Mcp-Method header value 'tools/list' does not match body value 'ping'",
    22
  );

  await assertTransportError(
    await mcpRequest(state, modernRequest('tools/call', 23, { name: 'server_info', arguments: {} }), {
      headers: modernHeaders('tools/call')
    }),
    400,
    -32020,
    'Header mismatch: Mcp-Name header is required',
    23
  );

  const missingCapabilities = modernRequest('ping', 24);
  delete missingCapabilities.params._meta['io.modelcontextprotocol/clientCapabilities'];
  await assertTransportError(
    await mcpRequest(state, missingCapabilities, { headers: modernHeaders('ping') }),
    400,
    -32602,
    'Invalid request metadata: io.modelcontextprotocol/clientCapabilities is required',
    24
  );

  await assertTransportError(
    await mcpRequest(state, modernRequest('ping', 25), { headers: { 'mcp-method': 'ping' } }),
    400,
    -32020,
    'Header mismatch: MCP-Protocol-Version header is required for 2026-07-28 requests',
    25
  );
});

test('MCP 2026-07-28 validates mirrored Mcp-Param tool headers including Base64 sentinel values', () => {
  const tool = {
    inputSchema: {
      type: 'object',
      properties: {
        workspace_folder_id: { type: 'string', 'x-mcp-header': 'Workspace' }
      }
    }
  };
  const request = modernRequest('tools/call', 31, {
    name: 'read_file',
    arguments: { workspace_folder_id: 'folder-a' }
  });
  const baseHeaders = modernHeaders('tools/call', 'read_file');
  assert.equal(
    validateModernMcpToolHeaders(baseHeaders, request, tool)?.message,
    'Header mismatch: Mcp-Param-Workspace header is required'
  );
  assert.equal(
    validateModernMcpToolHeaders({ ...baseHeaders, 'mcp-param-workspace': 'folder-b' }, request, tool)?.message,
    "Header mismatch: Mcp-Param-Workspace header value 'folder-b' does not match body value 'folder-a'"
  );
  assert.equal(
    validateModernMcpToolHeaders({ ...baseHeaders, 'mcp-param-workspace': 'folder-a' }, request, tool),
    undefined
  );
  const spaced = modernRequest('tools/call', 32, {
    name: 'read_file',
    arguments: { workspace_folder_id: ' folder-a' }
  });
  assert.equal(
    validateModernMcpToolHeaders(
      { ...baseHeaders, 'mcp-param-workspace': '=?BASE64?IGZvbGRlci1h?=' },
      spaced,
      tool
    ),
    undefined
  );
});

test('MCP 2026-07-28 uses MRTR to resume guarded permission approvals', async t => {
  const state = await createMcpFixture(t, { permissionMode: 'guarded' });
  const argumentsValue = {
    workspace_folder_id: 'repo',
    program: process.execPath,
    args: ['-e', "process.stdout.write('mrtr-ok')"]
  };
  const headers = {
    ...modernHeaders('tools/call', 'exec_command'),
    'mcp-param-workspace': 'repo'
  };

  await assertTransportError(
    await mcpRequest(state, modernRequest('tools/call', 41, { name: 'exec_command', arguments: argumentsValue }), { headers }),
    400,
    -32021,
    'Client capability elicitation is required to approve exec_command',
    41,
    { requiredCapabilities: { elicitation: {} } }
  );

  const initial = modernRequest('tools/call', 42, { name: 'exec_command', arguments: argumentsValue });
  initial.params._meta['io.modelcontextprotocol/clientCapabilities'] = { elicitation: {} };
  const initialResponse = await mcpRequest(state, initial, { headers });
  assert.equal(initialResponse.status, 200);
  const initialBody = await responseJson(initialResponse);
  assert.equal(initialBody.result.resultType, 'input_required');
  assert.equal(initialBody.result.inputRequests.permission_approval.method, 'elicitation/create');
  assert.equal(initialBody.result.inputRequests.permission_approval.params.mode, 'form');
  assert.match(initialBody.result.requestState, /^permission:/);

  const retry = modernRequest('tools/call', 43, {
    name: 'exec_command',
    arguments: argumentsValue,
    requestState: initialBody.result.requestState,
    inputResponses: {
      permission_approval: {
        action: 'accept',
        content: { approve: true }
      }
    }
  });
  const retryResponse = await mcpRequest(state, retry, { headers });
  assert.equal(retryResponse.status, 200);
  const retryBody = await responseJson(retryResponse);
  assert.equal(retryBody.result.resultType, 'complete');
  assert.equal(retryBody.result.structuredContent.resumed, true);
  assert.equal(retryBody.result.structuredContent.permission_grant.status, 'granted_and_resumed');
  assert.equal(retryBody.result.structuredContent.permission_grant.permission, 'process_execution');
});
test('MCP 2026-07-28 exposes Tasks and projects retained exec sessions through get, update, and cancel', async t => {
  const state = await createMcpFixture(t);
  const session = `tasks-${Date.now()}`;

  const discoveredResponse = await mcpRequest(
    state,
    modernRequest('server/discover', 51),
    { headers: modernHeaders('server/discover') }
  );
  assert.equal(discoveredResponse.status, 200);
  const discovered = await responseJson(discoveredResponse);
  assert.deepEqual(discovered.result.capabilities.extensions[tasksExtension], {});

  const completeArguments = {
    workspace_folder_id: 'repo',
    program: nodeProgram,
    args: ['-e', 'setTimeout(() => process.stdout.write("task-complete"), 200)'],
    yield_time_ms: 0,
    timeout_ms: 5_000,
    output_mode: 'all'
  };
  const toolHeaders = {
    ...modernHeaders('tools/call', 'exec_command'),
    'mcp-param-workspace': 'repo'
  };
  const createdResponse = await mcpRequest(
    state,
    modernTaskRequest('tools/call', 52, { name: 'exec_command', arguments: completeArguments }, session),
    { headers: toolHeaders }
  );
  assert.equal(createdResponse.status, 200);
  const created = await responseJson(createdResponse);
  assert.equal(created.result.resultType, 'task');
  assert.equal(created.result.status, 'working');
  assert.match(created.result.taskId, /^exec:/);
  assert.equal(created.result.pollIntervalMs, 1_000);
  assert.equal(created.result.ttlMs, 60 * 60_000);
  assert.match(created.result.createdAt, /^\d{4}-\d{2}-\d{2}T/);

  const unadvertisedGet = modernRequest('tasks/get', 53, { taskId: created.result.taskId });
  unadvertisedGet.params._meta['openai/session'] = session;
  const unadvertisedResponse = await mcpRequest(state, unadvertisedGet, { headers: modernHeaders('tasks/get') });
  assert.equal(unadvertisedResponse.status, 200);
  assert.equal((await responseJson(unadvertisedResponse)).error.code, -32601);

  let completed;
  for (let attempt = 0; attempt < 30; attempt += 1) {
    const response = await mcpRequest(
      state,
      modernTaskRequest('tasks/get', 54 + attempt, { taskId: created.result.taskId }, session),
      { headers: modernHeaders('tasks/get') }
    );
    assert.equal(response.status, 200);
    const body = await responseJson(response);
    assert.equal(body.result.resultType, 'complete');
    if (body.result.status === 'completed') {
      completed = body.result;
      break;
    }
    assert.equal(body.result.status, 'working');
    await new Promise(resolve => setTimeout(resolve, 25));
  }
  assert.ok(completed, 'retained process task did not complete');
  assert.equal(completed.result.resultType, 'complete');
  assert.equal(completed.result.isError, false);
  assert.match(completed.result.structuredContent.stdout, /task-complete/);

  const cancelArguments = {
    workspace_folder_id: 'repo',
    program: nodeProgram,
    args: ['-e', 'setTimeout(() => process.stdout.write("too-late"), 5000)'],
    yield_time_ms: 0,
    timeout_ms: 10_000,
    output_mode: 'none'
  };
  const cancellableResponse = await mcpRequest(
    state,
    modernTaskRequest('tools/call', 90, { name: 'exec_command', arguments: cancelArguments }, session),
    { headers: toolHeaders }
  );
  assert.equal(cancellableResponse.status, 200);
  const cancellable = await responseJson(cancellableResponse);
  assert.equal(cancellable.result.resultType, 'task');
  assert.equal(cancellable.result.status, 'working');

  const updateResponse = await mcpRequest(
    state,
    modernTaskRequest('tasks/update', 91, {
      taskId: cancellable.result.taskId,
      inputResponses: { unused: { action: 'accept' } }
    }, session),
    { headers: modernHeaders('tasks/update') }
  );
  assert.equal(updateResponse.status, 200);
  const update = await responseJson(updateResponse);
  assert.equal(update.error.code, -32602);
  assert.equal(update.error.data.error_code, 'TASK_NOT_INPUT_REQUIRED');

  const cancelResponse = await mcpRequest(
    state,
    modernTaskRequest('tasks/cancel', 92, { taskId: cancellable.result.taskId }, session),
    { headers: modernHeaders('tasks/cancel') }
  );
  assert.equal(cancelResponse.status, 200);
  const cancelledAck = await responseJson(cancelResponse);
  assert.equal(cancelledAck.result.resultType, 'complete');

  let cancelled;
  for (let attempt = 0; attempt < 30; attempt += 1) {
    const response = await mcpRequest(
      state,
      modernTaskRequest('tasks/get', 93 + attempt, { taskId: cancellable.result.taskId }, session),
      { headers: modernHeaders('tasks/get') }
    );
    assert.equal(response.status, 200);
    const body = await responseJson(response);
    if (body.result?.status === 'cancelled') {
      cancelled = body.result;
      break;
    }
    await new Promise(resolve => setTimeout(resolve, 25));
  }
  assert.ok(cancelled, 'cancelled task did not reach a terminal tombstone');
  assert.equal(cancelled.resultType, 'complete');
  assert.equal(cancelled.result, undefined);
});

test('MCP accepts exactly one valid JSON-RPC request, notification or client response', async t => {
  const state = await createMcpFixture(t);
  const invalid = [
    { value: [], message: 'The request body must be one JSON-RPC message' },
    { value: null, message: 'The request body must be one JSON-RPC message' },
    { value: 1, message: 'The request body must be one JSON-RPC message' },
    { value: { id: 1, method: 'ping' }, message: "jsonrpc must be '2.0'" },
    { value: { jsonrpc: '1.0', id: 1, method: 'ping' }, message: "jsonrpc must be '2.0'" },
    { value: { jsonrpc: '2.0', id: 1 }, message: 'Invalid JSON-RPC request, notification, or response' }
  ];
  for (const fixture of invalid) {
    await assertTransportError(
      await mcpRequest(state, fixture.value),
      400,
      -32600,
      fixture.message
    );
  }

  await assertTransportError(
    await mcpRequest(state, undefined, { rawBody: '{' }),
    400,
    -32700,
    'Parse error'
  );

  const notification = await mcpRequest(state, {
    jsonrpc: '2.0', method: 'notifications/initialized', params: {}
  });
  assert.equal(notification.status, 202);
  assert.equal(await notification.text(), '');

  const clientResponse = await mcpRequest(state, {
    jsonrpc: '2.0', id: 9, result: { accepted: true }
  });
  assert.equal(clientResponse.status, 202);
  assert.equal(await clientResponse.text(), '');
});

test('built-in tunnel forwards Origin and MCP protocol headers into local validation', () => {
  const headers = forwardedTunnelRequestHeaders([
    { name: 'Origin', value: 'https://attacker.example' },
    { name: 'MCP-Protocol-Version', value: '2024-01-01' },
    { name: 'Connection', value: 'keep-alive' },
    { name: 'X-Test', value: 'ok' }
  ]);
  assert.equal(headers.origin, 'https://attacker.example');
  assert.equal(headers['mcp-protocol-version'], '2024-01-01');
  assert.equal(headers.connection, undefined);
  assert.equal(headers['x-test'], 'ok');
});
