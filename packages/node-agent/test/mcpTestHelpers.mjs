import { createHmac } from 'node:crypto';
import { mkdtemp, rm } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import path from 'node:path';
import { createAgentRuntime } from '../dist/server.js';

function base64url(value) {
  return Buffer.from(value).toString('base64url');
}

export function bearerToken(baseUrl, secret) {
  const issuer = baseUrl.replace(/\/$/, '');
  const now = Math.floor(Date.now() / 1000);
  const header = base64url(JSON.stringify({ alg: 'HS256', typ: 'JWT' }));
  const body = base64url(JSON.stringify({
    iss: issuer,
    aud: `${issuer}/mcp`,
    iat: now,
    exp: now + 3600,
    scope: 'mcp'
  }));
  const signature = createHmac('sha256', secret).update(`${header}.${body}`).digest('base64url');
  return `${header}.${body}.${signature}`;
}

let mcpPortSequence = 0;

async function listenOnFetchSafePort(server, host) {
  let lastError;
  for (let attempt = 0; attempt < 32; attempt += 1) {
    const port = 48_000 + ((process.pid + mcpPortSequence++ * 997) % 8_000);
    try {
      await new Promise((resolve, reject) => {
        const onError = error => {
          server.off('listening', onListening);
          reject(error);
        };
        const onListening = () => {
          server.off('error', onError);
          resolve();
        };
        server.once('error', onError);
        server.once('listening', onListening);
        server.listen(port, host);
      });
      return;
    } catch (error) {
      lastError = error;
      // Windows reserves port ranges; retry a different range without changing host policy.
      if (error?.code !== 'EADDRINUSE' && !(process.platform === 'win32' && error?.code === 'EACCES')) throw error;
    }
  }
  throw lastError ?? new Error('unable to allocate a fetch-safe MCP test port');
}

export async function createMcpFixture(t, options = {}) {
  const root = await mkdtemp(path.join(tmpdir(), 'ctmcp-http-root-'));
  const dataDir = await mkdtemp(path.join(tmpdir(), 'ctmcp-http-data-'));
  const publicBaseUrl = options.publicBaseUrl ?? 'https://public.example/builtin/clients/test';
  const tokenSecret = 'mcp-http-parity-token-secret';
  const config = {
    host: options.host ?? '127.0.0.1',
    port: 0,
    publicBaseUrl,
    dataDir,
    permissionMode: options.permissionMode ?? 'trusted',
    management: { enabled: false },
    oauth: { clientId: 'chatgpt', password: 'test-password', tokenSecret },
    folders: [{ id: 'repo', name: 'Repo', path: root }],
    limits: {
      blockingConcurrency: 4,
      processConcurrency: 4,
      activeSessionLimit: 32,
      maxOutputBytes: 1024 * 1024
    }
  };
  const runtime = await createAgentRuntime(config, {
    mcpHeartbeatIntervalMs: options.mcpHeartbeatIntervalMs
  });
  await listenOnFetchSafePort(runtime.server, config.host);
  const address = runtime.server.address();
  if (!address || typeof address === 'string') throw new Error('Node Agent test listener has no TCP address');
  const routePrefix = new URL(publicBaseUrl).pathname.replace(/\/$/, '');
  const localBase = `http://127.0.0.1:${address.port}`;
  const endpoint = `${localBase}${routePrefix}/mcp`;
  const token = bearerToken(publicBaseUrl, tokenSecret);
  const state = {
    runtime,
    config,
    root,
    dataDir,
    localBase,
    endpoint,
    infoEndpoint: `${localBase}${routePrefix}/mcp/info`,
    port: address.port,
    token,
    authorization: `Bearer ${token}`
  };
  t.after(async () => {
    runtime.server.closeAllConnections?.();
    await runtime.close();
    await rm(root, { recursive: true, force: true, maxRetries: 5, retryDelay: 50 });
    await rm(dataDir, { recursive: true, force: true, maxRetries: 5, retryDelay: 50 });
  });
  return state;
}

export async function mcpRequest(state, payload, options = {}) {
  const headers = {
    ...(options.auth === false ? {} : { authorization: state.authorization }),
    ...(options.body === false ? {} : { 'content-type': 'application/json' }),
    ...(options.headers ?? {})
  };
  return fetch(options.url ?? state.endpoint, {
    method: options.method ?? 'POST',
    headers,
    body: options.body === false
      ? undefined
      : options.rawBody ?? JSON.stringify(payload)
  });
}

export async function responseJson(response) {
  const text = await response.text();
  return text ? JSON.parse(text) : undefined;
}
