import test from 'node:test';
import assert from 'node:assert/strict';
import {
  createHash, createPublicKey, generateKeyPairSync, sign as signSignature,
  verify as verifySignature
} from 'node:crypto';
import { createServer } from 'node:http';
import { mkdtemp, readFile } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import path from 'node:path';
import { WebSocketServer, WebSocket } from 'ws';
import { createToolContext } from '../dist/server.js';
import {
  authSigningPayload, builtinEndpointForClient, BuiltinTunnelManager, parseBuiltinPublicUrl,
  publicTunnelSecurityError, serverAckSigningPayload, serverChallengeSigningPayload,
  tunnelPathAllowed, BUILTIN_TUNNEL_DEMAND_TTL_MS, BUILTIN_TUNNEL_LOCAL_CONNECT_TIMEOUT_MS,
  BUILTIN_TUNNEL_LOCAL_REQUEST_TIMEOUT_MS, TUNNEL_PROTOCOL_VERSION, TUNNEL_SUBPROTOCOL
} from '../dist/tunnel.js';
import { loadOrEnroll } from '../dist/tunnel/identity.js';
import {
  configuredBurstWarmFloor, configuredMaxConnecting, jitteredLimit,
  normalizeWorkerPolicy, poolAdjustment, workerShouldRecycle
} from '../dist/tunnelPolicy.js';

const defaultPolicy = {
  start_workers: 4,
  min_idle_workers: 2,
  max_idle_workers: 4,
  max_workers: 16,
  max_requests_per_worker: 500,
  max_lifetime_seconds: 3600,
  scale_down_delay_seconds: 60,
  recycle_jitter_percent: 10,
  max_pending_requests: 32,
  worker_acquire_timeout_ms: 10000,
  max_connecting_workers: 0,
  connecting_capacity_grace_ms: 1000,
  scale_down_step: 4,
  burst_warm_workers: 0,
  burst_warm_seconds: 120,
  revision: 1
};

const testServerKeyPair = generateKeyPairSync('ed25519');
const testServerPublicKeyRaw = Buffer.from(
  testServerKeyPair.publicKey.export({ format: 'der', type: 'spki' })
).subarray(-32).toString('base64url');
const testServerId = createHash('sha256')
  .update(Buffer.from(testServerPublicKeyRaw, 'base64url'))
  .digest('base64url');

function enrollmentResponse(deviceId, clientId) {
  return {
    device_id: deviceId,
    client_id: clientId,
    server_id: testServerId,
    server_public_key: testServerPublicKeyRaw
  };
}

function sendSignedServerChallenge(socket, request, nonce) {
  const expiresAtUnixMs = Date.now() + 30_000;
  const deviceId = String(request.headers['x-coding-tools-device-id'] ?? '');
  const clientId = String(request.headers['x-coding-tools-client-id'] ?? '');
  const workerId = String(request.headers['x-coding-tools-worker-id'] ?? '');
  assert.ok(deviceId);
  assert.ok(clientId);
  assert.ok(workerId);
  const serverSignature = signSignature(
    null,
    serverChallengeSigningPayload(nonce, expiresAtUnixMs, testServerId, deviceId, clientId, workerId),
    testServerKeyPair.privateKey
  ).toString('base64url');
  socket.send(JSON.stringify({
    kind: 'challenge',
    nonce,
    expires_at_unix_ms: expiresAtUnixMs,
    server_id: testServerId,
    server_signature: serverSignature
  }));
}

function sendSignedServerAck(socket, nonce, authenticate, workerPolicy) {
  const serverSignature = signSignature(
    null,
    serverAckSigningPayload(
      nonce,
      testServerId,
      authenticate.device_id,
      authenticate.hello.client_id,
      authenticate.hello.worker_id,
      workerPolicy
    ),
    testServerKeyPair.privateKey
  ).toString('base64url');
  socket.send(JSON.stringify({
    kind: 'hello_ack',
    protocol_version: TUNNEL_PROTOCOL_VERSION,
    worker_policy: workerPolicy,
    server_id: testServerId,
    server_signature: serverSignature
  }));
}

async function waitFor(predicate, message, timeoutMs = 5000) {
  const started = Date.now();
  while (Date.now() - started < timeoutMs) {
    const value = predicate();
    if (value) return value;
    await new Promise(resolve => setTimeout(resolve, 20));
  }
  throw new Error(message);
}

async function withTimeout(promise, timeoutMs, message) {
  let timer;
  try {
    return await Promise.race([
      promise,
      new Promise((_, reject) => {
        timer = setTimeout(() => reject(new Error(message())), timeoutMs);
        timer.unref();
      })
    ]);
  } finally {
    if (timer) clearTimeout(timer);
  }
}

async function closeServer(server) {
  await new Promise(resolve => server.close(() => resolve()));
}

test('built-in tunnel URL derives the v4 websocket endpoint', () => {
  const endpoint = parseBuiltinPublicUrl('https://tunnel.example/builtin/clients/device_1/mcp');
  assert.equal(endpoint.clientId, 'device_1');
  assert.equal(endpoint.baseUrl, 'https://tunnel.example/builtin/clients/device_1');
  assert.equal(endpoint.websocketUrl, 'wss://tunnel.example/_tunnel/v1');
  assert.equal(TUNNEL_PROTOCOL_VERSION, 4);
  assert.equal(TUNNEL_SUBPROTOCOL, 'coding-tools-tunnel-v4');
  assert.equal(BUILTIN_TUNNEL_DEMAND_TTL_MS, 3_000);
  assert.equal(BUILTIN_TUNNEL_LOCAL_CONNECT_TIMEOUT_MS, 10_000);
  assert.equal(BUILTIN_TUNNEL_LOCAL_REQUEST_TIMEOUT_MS, 5 * 60_000);
});

test('built-in tunnel rebuilds the public endpoint from the authoritative client ID', () => {
  const endpoint = builtinEndpointForClient(
    'https://tunnel.example/builtin/clients/provisional/mcp',
    'server_assigned_1'
  );
  assert.equal(endpoint.publicUrl, 'https://tunnel.example/builtin/clients/server_assigned_1/mcp');
  assert.equal(endpoint.baseUrl, 'https://tunnel.example/builtin/clients/server_assigned_1');
  assert.equal(endpoint.clientId, 'server_assigned_1');
});

test('authentication signing payload matches the Rust field order and names', () => {
  const payload = authSigningPayload('nonce', 'device', 'client', 'worker');
  assert.equal(payload.toString(), '{"protocol_version":4,"nonce":"nonce","device_id":"device","client_id":"client","service":"mcp","worker_id":"worker"}');
});

test('built-in tunnel rejects non-HTTPS and non-client MCP paths', () => {
  assert.throws(() => parseBuiltinPublicUrl('http://tunnel.example/builtin/clients/device/mcp'), /HTTPS/);
  assert.throws(() => parseBuiltinPublicUrl('https://tunnel.example/other/path'), /builtin\/clients/);
});

test('public tunnel security is independent of permission mode and sandbox', () => {
  const config = {
    tunnel: { enabled: true, publicUrl: 'https://tunnel.example/builtin/clients/device/mcp' },
    sandbox: { enabled: false },
    permissionMode: 'guarded'
  };
  assert.equal(publicTunnelSecurityError(config), undefined);

  delete config.sandbox;
  config.permissionMode = 'trusted';
  assert.equal(publicTunnelSecurityError(config), undefined);

  config.sandbox = { enabled: true };
  config.permissionMode = 'dangerous';
  assert.equal(publicTunnelSecurityError(config), undefined);
});

test('invalid built-in tunnel configuration records a recoverable error status', async () => {
  const dataDir = await mkdtemp(path.join(tmpdir(), 'ctmcp-tunnel-invalid-'));
  const config = {
    host: '127.0.0.1',
    port: 3789,
    dataDir,
    permissionMode: 'guarded',
    sandbox: { enabled: true, backend: 'appcontainer', externalPaths: [], options: {} },
    oauth: { clientId: 'chatgpt', password: 'test-password', tokenSecret: 'a sufficiently long test token secret' },
    folders: [{ id: 'repo', name: 'Repo', path: dataDir }],
    limits: { blockingConcurrency: 4, processConcurrency: 4, activeSessionLimit: 16, maxOutputBytes: 1024 * 1024 },
    tunnel: {
      enabled: true,
      publicUrl: 'https://tunnel.example/mcp',
      stateFile: path.join(dataDir, 'identity.enc.json')
    }
  };
  const context = await createToolContext(config);
  const manager = new BuiltinTunnelManager(config, context);

  await assert.rejects(manager.start(), /builtin\/clients/);
  assert.equal(context.tunnelStatus.state, 'error');
  assert.equal(context.tunnelStatus.workers, 0);
  assert.equal(context.tunnelStatus.publicUrl, 'https://tunnel.example/mcp');
  assert.match(context.tunnelStatus.lastError, /builtin\/clients/);
});

test('built-in tunnel reconfigure can disable the runtime without restarting the agent', async () => {
  const dataDir = await mkdtemp(path.join(tmpdir(), 'ctmcp-tunnel-reconfigure-'));
  const config = {
    host: '127.0.0.1',
    port: 3789,
    dataDir,
    permissionMode: 'guarded',
    sandbox: { enabled: true, backend: 'appcontainer', externalPaths: [], options: {} },
    oauth: { clientId: 'chatgpt', password: 'test-password', tokenSecret: 'a sufficiently long test token secret' },
    folders: [{ id: 'repo', name: 'Repo', path: dataDir }],
    limits: { blockingConcurrency: 4, processConcurrency: 4, activeSessionLimit: 16, maxOutputBytes: 1024 * 1024 },
    tunnel: {
      enabled: false,
      publicUrl: 'https://tunnel.example/builtin/clients/device_1/mcp'
    }
  };
  const context = await createToolContext(config);
  const manager = new BuiltinTunnelManager(config, context);

  await manager.reconfigure(undefined, undefined);
  assert.equal(config.tunnel, undefined);
  assert.equal(context.config.tunnel, undefined);
  assert.equal(config.publicBaseUrl, undefined);
  assert.equal(context.config.publicBaseUrl, undefined);
  assert.deepEqual(context.tunnelStatus, {
    enabled: false,
    state: 'disabled',
    workers: 0,
    connectedWorkers: 0,
    completedRequests: 0
  });
});

test('WorkerPolicy validation and pool planning match Rust behavior', () => {
  const policy = normalizeWorkerPolicy(defaultPolicy);
  assert.equal(configuredMaxConnecting(policy), 4);
  assert.equal(configuredBurstWarmFloor(policy), 8);
  assert.deepEqual(poolAdjustment(
    policy,
    { total: 1, connecting: 1, idle: 0, busy: 0 },
    1, 4, 0, false, 4
  ), { spawn: 3, retire: 0 });
  assert.deepEqual(poolAdjustment(
    policy,
    { total: 4, connecting: 0, idle: 1, busy: 3 },
    0, 4, 16, false, 8
  ), { spawn: 4, retire: 0 });
  assert.deepEqual(poolAdjustment(
    policy,
    { total: 16, connecting: 0, idle: 16, busy: 0 },
    0, 4, 0, true, 8
  ), { spawn: 0, retire: 4 });
  assert.throws(() => normalizeWorkerPolicy({ ...defaultPolicy, min_idle_workers: 5 }), /worker counts/);
});

test('worker recycle limits use bounded deterministic jitter', () => {
  const policy = normalizeWorkerPolicy(defaultPolicy);
  const first = jitteredLimit(500, 7, 10);
  const second = jitteredLimit(500, 8, 10);
  assert.ok(first >= 450 && first <= 550);
  assert.ok(second >= 450 && second <= 550);
  assert.notEqual(first, second);
  assert.equal(workerShouldRecycle(policy, 7, first - 1, 10_000), false);
  assert.equal(workerShouldRecycle(policy, 7, first, 10_000), true);
});

test('built-in tunnel exposes only scoped MCP and OAuth routes', () => {
  const config = {
    publicBaseUrl: 'https://tunnel.example/builtin/clients/device_1',
    tunnel: { publicUrl: 'https://tunnel.example/builtin/clients/device_1/mcp' }
  };
  assert.equal(tunnelPathAllowed(config, '/builtin/clients/device_1/mcp'), true);
  assert.equal(tunnelPathAllowed(config, '/builtin/clients/device_1/mcp/pairing'), true);
  assert.equal(tunnelPathAllowed(config, '/builtin/clients/other/mcp/pairing'), false);
  assert.equal(tunnelPathAllowed(config, '/mcp/pairing'), false);
  assert.equal(tunnelPathAllowed(config, '/builtin/clients/device_1/oauth/authorize'), true);
  assert.equal(tunnelPathAllowed(config, '/builtin/clients/device_1/oauth/register'), true);
  assert.equal(tunnelPathAllowed(config, '/builtin/clients/other/oauth/register'), false);
  assert.equal(tunnelPathAllowed(config, '/.well-known/oauth-protected-resource/builtin/clients/device_1/mcp'), true);
  assert.equal(tunnelPathAllowed(config, '/ui'), false);
  assert.equal(tunnelPathAllowed(config, '/admin/api/config'), false);
  assert.equal(tunnelPathAllowed(config, '/builtin/clients/device_1/../ui'), false);
});

test('built-in tunnel reuses an enrolled identity when a stale enrollment URL remains after restart', async () => {
  const dataDir = await mkdtemp(path.join(tmpdir(), 'ctmcp-tunnel-restart-'));
  const stateFile = path.join(dataDir, 'identity.enc.json');
  const tokenSecret = 'a sufficiently long restart test token secret';
  const initialEndpoint = parseBuiltinPublicUrl('https://tunnel.example/builtin/clients/provisional_1/mcp');
  const enrollmentUrl = 'https://tunnel.example/_tunnel/enroll/TESTCODE';
  const config = {
    oauth: { tokenSecret },
    tunnel: {
      enabled: true,
      publicUrl: initialEndpoint.publicUrl,
      enrollmentUrl,
      stateFile
    }
  };
  let enrollmentCalls = 0;
  const enrolled = await loadOrEnroll(config, initialEndpoint, async (_url, init) => {
    enrollmentCalls += 1;
    const body = JSON.parse(String(init?.body ?? '{}'));
    return new Response(JSON.stringify(enrollmentResponse(body.device_id, 'server_assigned_1')), {
      status: 200,
      headers: { 'content-type': 'application/json' }
    });
  });

  assert.equal(enrolled.enrollmentCompleted, true);
  assert.equal(enrolled.identity.enrolled, true);
  assert.equal(enrolled.identity.clientId, 'server_assigned_1');
  assert.equal(enrollmentCalls, 1);

  const restartedEndpoint = parseBuiltinPublicUrl('https://tunnel.example/builtin/clients/server_assigned_1/mcp');
  const restarted = await loadOrEnroll({
    ...config,
    tunnel: {
      ...config.tunnel,
      publicUrl: restartedEndpoint.publicUrl,
      enrollmentUrl
    }
  }, restartedEndpoint, async () => {
    enrollmentCalls += 1;
    throw new Error('an enrolled identity must not re-enroll during restart');
  });

  assert.equal(restarted.enrollmentCompleted, false);
  assert.equal(enrollmentCalls, 1);
  assert.equal(restarted.identity.enrolled, true);
  assert.equal(restarted.identity.clientId, enrolled.identity.clientId);
  assert.equal(restarted.identity.deviceId, enrolled.identity.deviceId);
  assert.equal(restarted.identity.privateKeyDer, enrolled.identity.privateKeyDer);
  assert.equal(restarted.identity.publicKeyRaw, enrolled.identity.publicKeyRaw);
});

test('dynamic built-in tunnel performs enrollment, auth, forwarding, scale-up and scale-down', async t => {
  const dataDir = await mkdtemp(path.join(tmpdir(), 'ctmcp-tunnel-'));
  const stateFile = path.join(dataDir, 'identity.enc.json');
  let fakeNow = 1;
  let enrolledPublicKey = '';
  const errors = [];
  const readySockets = new Set();
  const allSockets = new Set();
  const response = { status: 0, headers: [], chunks: [] };
  let resolveResponse;
  let rejectResponse;
  const responseDone = new Promise((resolve, reject) => { resolveResponse = resolve; rejectResponse = reject; });

  const server = createServer(async (request, result) => {
    if (!request.url?.startsWith('/builtin/clients/server_assigned_1/mcp')) {
      result.writeHead(404).end('not found');
      return;
    }
    const chunks = [];
    for await (const chunk of request) chunks.push(Buffer.from(chunk));
    result.writeHead(201, { 'content-type': 'application/json', 'x-node-agent-test': 'ok', 'x-coding-tools-streaming': '1' });
    result.write('\n');
    await new Promise(resolve => setTimeout(resolve, 20));
    result.end(JSON.stringify({ echo: Buffer.concat(chunks).toString('utf8') }));
  });
  await new Promise(resolve => server.listen(0, '127.0.0.1', resolve));
  t.after(() => closeServer(server));
  const address = server.address();
  assert.ok(address && typeof address === 'object');

  const wss = new WebSocketServer({
    server,
    path: '/_tunnel/v1',
    handleProtocols(protocols) { return protocols.has(TUNNEL_SUBPROTOCOL) ? TUNNEL_SUBPROTOCOL : false; }
  });
  t.after(() => wss.close());

  const growPolicy = {
    ...defaultPolicy,
    start_workers: 2,
    min_idle_workers: 1,
    max_idle_workers: 2,
    max_workers: 4,
    max_requests_per_worker: 0,
    max_lifetime_seconds: 0,
    scale_down_delay_seconds: 1,
    burst_warm_seconds: 0,
    revision: 1
  };
  const shrinkPolicy = {
    ...growPolicy,
    start_workers: 1,
    min_idle_workers: 1,
    max_idle_workers: 1,
    max_workers: 1,
    revision: 2
  };

  wss.on('connection', (socket, request) => {
    allSockets.add(socket);
    const nonce = `nonce-${allSockets.size}`;
    assert.equal(request.headers['x-coding-tools-client-id'], 'server_assigned_1');
    assert.equal(request.headers['x-coding-tools-service'], 'mcp');
    sendSignedServerChallenge(socket, request, nonce);
    socket.on('close', () => { readySockets.delete(socket); allSockets.delete(socket); });
    socket.on('message', (data, isBinary) => {
      try {
        if (isBinary) {
          response.chunks.push(Buffer.from(data));
          return;
        }
        const control = JSON.parse(data.toString());
        if (control.kind === 'authenticate') {
          const raw = Buffer.from(enrolledPublicKey, 'base64url');
          const spki = Buffer.concat([Buffer.from('302a300506032b6570032100', 'hex'), raw]);
          const publicKey = createPublicKey({ key: spki, format: 'der', type: 'spki' });
          const valid = verifySignature(
            null,
            authSigningPayload(nonce, control.device_id, control.hello.client_id, control.hello.worker_id),
            publicKey,
            Buffer.from(control.signature, 'base64url')
          );
          assert.equal(valid, true);
          sendSignedServerAck(socket, nonce, control, growPolicy);
        } else if (control.kind === 'ready') {
          readySockets.add(socket);
        } else if (control.kind === 'response_head') {
          response.status = control.status;
          response.headers = control.headers;
        } else if (control.kind === 'response_end') {
          resolveResponse(response);
        } else if (control.kind === 'error') {
          rejectResponse(new Error(control.message));
        }
      } catch (error) {
        errors.push(error);
        rejectResponse(error);
      }
    });
  });

  const config = {
    host: '127.0.0.1',
    port: address.port,
    publicBaseUrl: 'https://tunnel.example/builtin/clients/provisional_1',
    dataDir,
    permissionMode: 'guarded',
    sandbox: { enabled: true, backend: 'appcontainer', externalPaths: [], options: {} },
    oauth: { clientId: 'chatgpt', password: 'password', tokenSecret: 'a sufficiently long test token secret' },
    folders: [{ id: 'repo', name: 'Repo', path: dataDir }],
    limits: { blockingConcurrency: 4, processConcurrency: 4, activeSessionLimit: 16, maxOutputBytes: 1024 * 1024 },
    tunnel: {
      enabled: true,
      publicUrl: 'https://tunnel.example/builtin/clients/provisional_1/mcp',
      enrollmentUrl: 'https://tunnel.example/_tunnel/enroll/TESTCODE',
      workers: 2,
      stateFile
    }
  };
  const context = await createToolContext(config);
  let resolvedEndpoint;
  const manager = new BuiltinTunnelManager(config, context, {
    websocketUrlOverride: `ws://127.0.0.1:${address.port}/_tunnel/v1`,
    reconcileIntervalMs: 20,
    demandNow: () => fakeNow,
    enrollmentFetch: async (_url, init) => {
      const body = JSON.parse(String(init?.body ?? '{}'));
      enrolledPublicKey = body.public_key;
      assert.equal(body.client_id, 'provisional_1');
      return new Response(JSON.stringify(enrollmentResponse(body.device_id, 'server_assigned_1')), {
        status: 200,
        headers: { 'content-type': 'application/json' }
      });
    },
    onEndpointResolved: endpoint => { resolvedEndpoint = endpoint; }
  });
  t.after(() => manager.stop());
  await manager.start();

  assert.deepEqual(resolvedEndpoint, {
    publicUrl: 'https://tunnel.example/builtin/clients/server_assigned_1/mcp',
    publicBaseUrl: 'https://tunnel.example/builtin/clients/server_assigned_1',
    enrollmentCompleted: true
  });
  assert.equal(config.tunnel.publicUrl, resolvedEndpoint.publicUrl);
  assert.equal(config.publicBaseUrl, resolvedEndpoint.publicBaseUrl);
  assert.equal(config.tunnel.enrollmentUrl, undefined);
  assert.equal(context.tunnelStatus.publicUrl, resolvedEndpoint.publicUrl);

  await waitFor(
    () => readySockets.size === 2,
    `expected two ready workers; ready=${readySockets.size}; sockets=${allSockets.size}; status=${JSON.stringify(context.tunnelStatus)}; errors=${errors.map(String).join('; ')}`
  );
  assert.equal(context.tunnelStatus.policyRevision, 1);
  assert.equal(context.tunnelStatus.connectedWorkers, 2);
  assert.equal(context.tunnelStatus.idleWorkers, 2);
  assert.equal(context.tunnelStatus.workers, 4);

  const worker = [...readySockets][0];
  readySockets.delete(worker);
  worker.send(JSON.stringify({
    kind: 'request_head',
    request_id: 'request-1',
    method: 'POST',
    path_and_query: '/builtin/clients/server_assigned_1/mcp?source=tunnel',
    headers: [{ name: 'content-type', value: 'text/plain' }],
    demand: { queued_requests: 1, oldest_queue_wait_ms: 25, desired_workers: 4 }
  }));
  worker.send(Buffer.from('payload'));
  worker.send(JSON.stringify({ kind: 'request_end', request_id: 'request-1' }));

  const forwarded = await withTimeout(
    responseDone,
    5000,
    () => `tunnel response timeout; errors=${errors.map(String).join('; ')}`
  );
  assert.equal(forwarded.status, 201);
  assert.ok(forwarded.chunks.length >= 2, `expected heartbeat and final response chunks, received ${forwarded.chunks.length}`);
  assert.equal(Buffer.concat(forwarded.chunks).toString('utf8'), '\n{"echo":"payload"}');
  assert.ok(forwarded.headers.some(header => header.name === 'x-node-agent-test' && header.value === 'ok'));
  assert.ok(forwarded.headers.some(header => header.name === 'x-coding-tools-streaming' && header.value === '1'));
  await waitFor(() => context.tunnelStatus.completedRequests === 1, 'completed request metric was not updated');
  await waitFor(
    () => context.tunnelStatus.connectedWorkers === 4 && readySockets.size === 4,
    `demand hint did not scale the pool to four workers: ${JSON.stringify(context.tunnelStatus)}; errors=${errors.map(String).join('; ')}`
  );

  fakeNow = 1 + BUILTIN_TUNNEL_DEMAND_TTL_MS;
  await new Promise(resolve => setTimeout(resolve, 80));
  assert.equal(context.tunnelStatus.connectedWorkers, 4, 'demand hint expired at the inclusive Rust boundary');
  fakeNow += 1;
  await waitFor(
    () => context.tunnelStatus.connectedWorkers === 2,
    `pool did not return to its configured idle floor after demand TTL: ${JSON.stringify(context.tunnelStatus)}`
  );

  for (const socket of allSockets) {
    if (socket.readyState === WebSocket.OPEN) socket.send(JSON.stringify({ kind: 'policy_update', worker_policy: shrinkPolicy }));
  }
  await waitFor(
    () => context.tunnelStatus.policyRevision === 2 && context.tunnelStatus.connectedWorkers === 1,
    `pool did not shrink: ${JSON.stringify(context.tunnelStatus)}`
  );
  assert.equal(context.tunnelStatus.workers, 1);
  assert.equal(context.tunnelStatus.idleWorkers, 1);
  assert.equal(errors.length, 0);

  const encrypted = await readFile(stateFile, 'utf8');
  assert.doesNotMatch(encrypted, /privateKeyDer|publicKeyRaw|deviceId/);
  await manager.stop();
});

test('built-in tunnel bounds local connect and overall request phases without leaking request data', async t => {
  const dataDir = await mkdtemp(path.join(tmpdir(), 'ctmcp-tunnel-timeout-'));
  const stateFile = path.join(dataDir, 'identity.enc.json');
  const pendingResponses = new Set();
  let delayedHeadersStarted = false;
  let delayedHeadersClosed = false;

  const localServer = createServer((request, result) => {
    const requestUrl = new URL(request.url ?? '/', 'http://127.0.0.1');
    const phase = requestUrl.searchParams.get('phase') ?? 'normal';
    pendingResponses.add(result);
    result.on('close', () => {
      pendingResponses.delete(result);
      if (phase === 'headers') delayedHeadersClosed = true;
    });
    if (phase === 'headers') {
      delayedHeadersStarted = true;
      return;
    }
    result.writeHead(200, { 'content-type': 'text/plain' });
    result.end(`ok:${phase}`);
  });
  await new Promise(resolve => localServer.listen(0, '127.0.0.1', resolve));
  const localAddress = localServer.address();
  assert.ok(localAddress && typeof localAddress === 'object');
  const localPort = localAddress.port;
  await closeServer(localServer);

  const tunnelServer = createServer((_request, result) => result.writeHead(404).end('not found'));
  await new Promise(resolve => tunnelServer.listen(0, '127.0.0.1', resolve));
  const tunnelAddress = tunnelServer.address();
  assert.ok(tunnelAddress && typeof tunnelAddress === 'object');
  const wss = new WebSocketServer({
    server: tunnelServer,
    path: '/_tunnel/v1',
    handleProtocols(protocols) { return protocols.has(TUNNEL_SUBPROTOCOL) ? TUNNEL_SUBPROTOCOL : false; }
  });

  const policy = {
    ...defaultPolicy,
    start_workers: 1,
    min_idle_workers: 1,
    max_idle_workers: 1,
    max_workers: 1,
    max_requests_per_worker: 0,
    max_lifetime_seconds: 0,
    scale_down_delay_seconds: 0,
    burst_warm_seconds: 0,
    revision: 1
  };
  const responses = new Map();
  const pongPayloads = [];
  let worker;
  let readyCount = 0;
  let activeRequestId;
  let lookupMode = 'normal';
  let hangingLookupCalls = 0;

  const responseFor = requestId => {
    let response = responses.get(requestId);
    if (!response) {
      response = { heads: [], chunks: [], ended: false, errors: [] };
      responses.set(requestId, response);
    }
    return response;
  };
  const sendRequest = (requestId, phase) => {
    assert.ok(worker && worker.readyState === WebSocket.OPEN);
    activeRequestId = requestId;
    responseFor(requestId);
    worker.send(JSON.stringify({
      kind: 'request_head',
      request_id: requestId,
      method: 'GET',
      path_and_query: `/builtin/clients/device_1/mcp?phase=${phase}`,
      headers: []
    }));
    worker.send(JSON.stringify({ kind: 'request_end', request_id: requestId }));
  };

  wss.on('connection', (socket, request) => {
    worker = socket;
    sendSignedServerChallenge(socket, request, 'timeout-test');
    socket.on('pong', data => pongPayloads.push(Buffer.from(data).toString('utf8')));
    socket.on('message', (data, isBinary) => {
      if (isBinary) {
        if (activeRequestId) responseFor(activeRequestId).chunks.push(Buffer.from(data));
        return;
      }
      const control = JSON.parse(data.toString());
      if (control.kind === 'authenticate') {
        sendSignedServerAck(socket, 'timeout-test', control, policy);
      } else if (control.kind === 'ready') {
        readyCount += 1;
        activeRequestId = undefined;
      } else if (control.kind === 'response_head') {
        activeRequestId = control.request_id;
        responseFor(control.request_id).heads.push(control);
      } else if (control.kind === 'response_end') {
        responseFor(control.request_id).ended = true;
      } else if (control.kind === 'error') {
        responseFor(control.request_id ?? activeRequestId ?? 'unknown').errors.push(control.message);
      }
    });
  });

  const config = {
    host: '127.0.0.1',
    port: localPort,
    publicBaseUrl: 'https://tunnel.example/builtin/clients/device_1',
    dataDir,
    permissionMode: 'guarded',
    sandbox: { enabled: true, backend: 'appcontainer', externalPaths: [], options: {} },
    oauth: { clientId: 'chatgpt', password: 'test-password', tokenSecret: 'a sufficiently long test token secret' },
    folders: [{ id: 'repo', name: 'Repo', path: dataDir }],
    limits: { blockingConcurrency: 4, processConcurrency: 4, activeSessionLimit: 16, maxOutputBytes: 1024 * 1024 },
    tunnel: {
      enabled: true,
      publicUrl: 'https://tunnel.example/builtin/clients/device_1/mcp',
      enrollmentUrl: 'https://tunnel.example/_tunnel/enroll/TIMEOUTTEST',
      workers: 1,
      stateFile
    }
  };
  const context = await createToolContext(config);
  const manager = new BuiltinTunnelManager(config, context, {
    websocketUrlOverride: `ws://127.0.0.1:${tunnelAddress.port}/_tunnel/v1`,
    localOriginOverride: `http://loopback.test:${localPort}`,
    localConnectTimeoutMs: 100,
    localRequestTimeoutMs: 250,
    reconcileIntervalMs: 20,
    localLookup: (_hostname, options, callback) => {
      if (lookupMode === 'hang') {
        hangingLookupCalls += 1;
        return;
      }
      if (options?.all) callback(null, [{ address: '127.0.0.1', family: 4 }]);
      else callback(null, '127.0.0.1', 4);
    },
    enrollmentFetch: async (_url, init) => {
      const body = JSON.parse(String(init?.body ?? '{}'));
      return new Response(JSON.stringify(enrollmentResponse(body.device_id, 'device_1')), {
        status: 200,
        headers: { 'content-type': 'application/json' }
      });
    }
  });
  t.after(async () => {
    await manager.stop();
    for (const result of pendingResponses) result.destroy();
    await new Promise(resolve => wss.close(() => resolve()));
    tunnelServer.closeAllConnections?.();
    await closeServer(tunnelServer);
    if (localServer.listening) {
      localServer.closeAllConnections?.();
      await closeServer(localServer);
    }
  });
  await manager.start();
  await waitFor(() => readyCount === 1, 'timeout test worker did not become ready');

  sendRequest('refused-request', 'refused');
  await waitFor(() => responseFor('refused-request').errors.length === 1, 'refused local connection did not fail');
  await waitFor(() => readyCount === 2, 'worker was not reusable after refused connection');
  assert.match(responseFor('refused-request').errors[0], /ECONNREFUSED|connect/i);
  assert.equal(context.tunnelStatus.lastRequestTimeout, undefined);

  await new Promise(resolve => localServer.listen(localPort, '127.0.0.1', resolve));

  lookupMode = 'hang';
  sendRequest('cancel-during-connect', 'cancel-connect');
  await waitFor(() => hangingLookupCalls === 1, 'connect-cancel fixture did not enter DNS/connect phase');
  worker.send(JSON.stringify({ kind: 'cancel', request_id: 'cancel-during-connect' }));
  await waitFor(() => readyCount === 3, 'worker was not reusable after connect cancellation');
  assert.deepEqual(responseFor('cancel-during-connect').errors, []);
  assert.equal(context.tunnelStatus.lastRequestTimeout, undefined);

  sendRequest('connect-timeout-request', 'connect-timeout');
  await waitFor(
    () => responseFor('connect-timeout-request').errors.includes('local tunnel connection timed out'),
    'hanging local connection did not hit the connect timeout'
  );
  await waitFor(() => readyCount === 4, 'worker was not reusable after connect timeout');
  assert.equal(context.tunnelStatus.lastRequestTimeout, 'connect');
  assert.equal(typeof context.tunnelStatus.lastRequestTimeoutAt, 'number');

  lookupMode = 'normal';
  sendRequest('overall-timeout-request', 'headers');
  await waitFor(() => delayedHeadersStarted, 'overall-timeout fixture did not reach the local server');
  worker.ping(Buffer.from('timeout-heartbeat'));
  await waitFor(() => pongPayloads.includes('timeout-heartbeat'), 'heartbeat stalled during local response wait');
  await waitFor(
    () => responseFor('overall-timeout-request').errors.includes('local tunnel request timed out'),
    'delayed local response did not hit the overall timeout'
  );
  await waitFor(() => delayedHeadersClosed, 'overall timeout did not abort the delayed local response');
  await waitFor(() => readyCount === 5, 'worker was not reusable after overall timeout');
  assert.equal(context.tunnelStatus.lastRequestTimeout, 'overall');
  assert.equal(typeof context.tunnelStatus.lastRequestTimeoutAt, 'number');
  const boundedStatus = JSON.stringify(context.tunnelStatus);
  assert.doesNotMatch(boundedStatus, /connect-timeout-request|overall-timeout-request|phase=headers/);

  sendRequest('reuse-after-timeouts', 'normal');
  await waitFor(() => responseFor('reuse-after-timeouts').ended, 'worker could not complete a request after timeouts');
  await waitFor(() => readyCount === 6, 'worker did not return to ready after timeout reuse');
  assert.equal(Buffer.concat(responseFor('reuse-after-timeouts').chunks).toString('utf8'), 'ok:normal');
  assert.equal(context.tunnelStatus.completedRequests, 2);
  assert.equal(context.tunnelStatus.connectedWorkers, 1);
  assert.equal(context.tunnelStatus.idleWorkers, 1);
});

test('built-in tunnel cancels delayed local responses and keeps the worker reusable', async t => {
  const dataDir = await mkdtemp(path.join(tmpdir(), 'ctmcp-tunnel-cancel-'));
  const stateFile = path.join(dataDir, 'identity.enc.json');
  const pendingResponses = new Set();
  const localState = {
    headersStarted: false,
    headersClosed: false,
    streamStarted: false,
    streamClosed: false
  };

  const server = createServer((request, result) => {
    const requestUrl = new URL(request.url ?? '/', 'http://127.0.0.1');
    const phase = requestUrl.searchParams.get('phase') ?? 'normal';
    pendingResponses.add(result);
    result.on('close', () => {
      pendingResponses.delete(result);
      if (phase === 'headers') localState.headersClosed = true;
      if (phase === 'stream') localState.streamClosed = true;
    });
    if (phase === 'headers') {
      localState.headersStarted = true;
      return;
    }
    if (phase === 'stream') {
      result.writeHead(200, { 'content-type': 'text/plain', 'x-cancel-phase': 'stream' });
      result.write('first-chunk');
      localState.streamStarted = true;
      return;
    }
    result.writeHead(200, { 'content-type': 'text/plain' });
    result.end(`ok:${phase}`);
  });
  await new Promise(resolve => server.listen(0, '127.0.0.1', resolve));
  const address = server.address();
  assert.ok(address && typeof address === 'object');

  const wss = new WebSocketServer({
    server,
    path: '/_tunnel/v1',
    handleProtocols(protocols) { return protocols.has(TUNNEL_SUBPROTOCOL) ? TUNNEL_SUBPROTOCOL : false; }
  });
  const policy = {
    ...defaultPolicy,
    start_workers: 1,
    min_idle_workers: 1,
    max_idle_workers: 1,
    max_workers: 1,
    max_requests_per_worker: 0,
    max_lifetime_seconds: 0,
    scale_down_delay_seconds: 0,
    burst_warm_seconds: 0,
    revision: 1
  };
  const responses = new Map();
  const pongPayloads = [];
  let worker;
  let readyCount = 0;
  let activeRequestId;

  const responseFor = requestId => {
    let response = responses.get(requestId);
    if (!response) {
      response = { heads: [], chunks: [], ended: false, errors: [] };
      responses.set(requestId, response);
    }
    return response;
  };
  const sendRequest = (requestId, phase) => {
    assert.ok(worker && worker.readyState === WebSocket.OPEN);
    activeRequestId = requestId;
    responseFor(requestId);
    worker.send(JSON.stringify({
      kind: 'request_head',
      request_id: requestId,
      method: 'GET',
      path_and_query: `/builtin/clients/device_1/mcp?phase=${phase}`,
      headers: []
    }));
    worker.send(JSON.stringify({ kind: 'request_end', request_id: requestId }));
  };

  wss.on('connection', (socket, request) => {
    worker = socket;
    sendSignedServerChallenge(socket, request, 'cancel-test');
    socket.on('pong', data => pongPayloads.push(Buffer.from(data).toString('utf8')));
    socket.on('message', (data, isBinary) => {
      if (isBinary) {
        if (activeRequestId) responseFor(activeRequestId).chunks.push(Buffer.from(data));
        return;
      }
      const control = JSON.parse(data.toString());
      if (control.kind === 'authenticate') {
        sendSignedServerAck(socket, 'cancel-test', control, policy);
      } else if (control.kind === 'ready') {
        readyCount += 1;
        activeRequestId = undefined;
      } else if (control.kind === 'response_head') {
        activeRequestId = control.request_id;
        responseFor(control.request_id).heads.push(control);
      } else if (control.kind === 'response_end') {
        responseFor(control.request_id).ended = true;
      } else if (control.kind === 'error') {
        responseFor(control.request_id ?? activeRequestId ?? 'unknown').errors.push(control.message);
      }
    });
  });

  const config = {
    host: '127.0.0.1',
    port: address.port,
    publicBaseUrl: 'https://tunnel.example/builtin/clients/device_1',
    dataDir,
    permissionMode: 'guarded',
    sandbox: { enabled: true, backend: 'appcontainer', externalPaths: [], options: {} },
    oauth: { clientId: 'chatgpt', password: 'test password', tokenSecret: 'a sufficiently long test token secret' },
    folders: [{ id: 'repo', name: 'Repo', path: dataDir }],
    limits: { blockingConcurrency: 4, processConcurrency: 4, activeSessionLimit: 16, maxOutputBytes: 1024 * 1024 },
    tunnel: {
      enabled: true,
      publicUrl: 'https://tunnel.example/builtin/clients/device_1/mcp',
      enrollmentUrl: 'https://tunnel.example/_tunnel/enroll/CANCELTEST',
      workers: 1,
      stateFile
    }
  };
  const context = await createToolContext(config);
  const manager = new BuiltinTunnelManager(config, context, {
    websocketUrlOverride: `ws://127.0.0.1:${address.port}/_tunnel/v1`,
    reconcileIntervalMs: 20,
    enrollmentFetch: async (_url, init) => {
      const body = JSON.parse(String(init?.body ?? '{}'));
      return new Response(JSON.stringify(enrollmentResponse(body.device_id, 'device_1')), {
        status: 200,
        headers: { 'content-type': 'application/json' }
      });
    }
  });
  t.after(async () => {
    await manager.stop();
    for (const result of pendingResponses) result.destroy();
    await new Promise(resolve => wss.close(() => resolve()));
    server.closeAllConnections?.();
    await closeServer(server);
  });
  await manager.start();
  await waitFor(() => readyCount === 1, 'cancellation test worker did not become ready');

  sendRequest('cancel-before-headers', 'headers');
  await waitFor(() => localState.headersStarted, 'local delayed-header request did not start');
  worker.ping(Buffer.from('headers'));
  await waitFor(() => pongPayloads.includes('headers'), 'worker did not answer heartbeat ping while awaiting headers');
  worker.send(JSON.stringify({ kind: 'cancel', request_id: 'cancel-before-headers' }));
  await waitFor(() => localState.headersClosed, 'cancel did not abort the local fetch before headers');
  await waitFor(() => readyCount === 2, 'worker did not return to ready after header cancellation');
  const headerCancel = responseFor('cancel-before-headers');
  assert.equal(headerCancel.heads.length, 0);
  assert.equal(headerCancel.ended, false);
  assert.deepEqual(headerCancel.errors, []);

  sendRequest('reuse-after-headers', 'reuse-headers');
  await waitFor(() => responseFor('reuse-after-headers').ended, 'worker could not complete a request after header cancellation');
  await waitFor(() => readyCount === 3, 'worker did not return to ready after header-cancel reuse');
  assert.equal(Buffer.concat(responseFor('reuse-after-headers').chunks).toString('utf8'), 'ok:reuse-headers');

  sendRequest('cancel-during-stream', 'stream');
  await waitFor(() => localState.streamStarted, 'local streaming response did not start');
  await waitFor(() => responseFor('cancel-during-stream').heads.length === 1, 'streaming response head was not forwarded');
  await waitFor(
    () => Buffer.concat(responseFor('cancel-during-stream').chunks).toString('utf8') === 'first-chunk',
    'first streaming response chunk was not forwarded'
  );
  worker.ping(Buffer.from('stream'));
  await waitFor(() => pongPayloads.includes('stream'), 'worker did not answer heartbeat ping during response streaming');
  worker.send(JSON.stringify({ kind: 'cancel', request_id: 'cancel-during-stream' }));
  await waitFor(() => localState.streamClosed, 'cancel did not close the local streaming response');
  await waitFor(() => readyCount === 4, 'worker did not return to ready after stream cancellation');
  const streamCancel = responseFor('cancel-during-stream');
  assert.equal(streamCancel.ended, false);
  assert.deepEqual(streamCancel.errors, []);

  sendRequest('reuse-after-stream', 'reuse-stream');
  await waitFor(() => responseFor('reuse-after-stream').ended, 'worker could not complete a request after stream cancellation');
  await waitFor(() => readyCount === 5, 'worker did not return to ready after stream-cancel reuse');
  assert.equal(Buffer.concat(responseFor('reuse-after-stream').chunks).toString('utf8'), 'ok:reuse-stream');
  assert.equal(context.tunnelStatus.completedRequests, 4);
  assert.equal(context.tunnelStatus.connectedWorkers, 1);
  assert.equal(context.tunnelStatus.idleWorkers, 1);
  assert.equal(context.tunnelStatus.busyWorkers, 0);
});
