import test from 'node:test';
import assert from 'node:assert/strict';
import { access, mkdir, mkdtemp, readFile, realpath, rm, symlink, writeFile } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import path from 'node:path';
import { request as httpRequest } from 'node:http';
import { ApplicationConfigStore, loadApplication } from '../dist/application.js';
import { loadConfigBundle } from '../dist/config.js';
import { runtimeForFolderId } from '../dist/folderRuntime.js';
import { ConfigStore } from '../dist/management.js';
import { validateManagementHealthPayload } from '../dist/managementObservability.js';
import { renderDocument } from '../dist/historyMarkdown.js';
import { harnessWorkspaceId } from '../dist/taskTools.js';
import { readAgentSecrets } from '../dist/secrets.js';
import { createAgentRuntime, createAgentServer } from '../dist/server.js';
import { startAndYield, waitForSession } from '../dist/processes.js';
import { AGENT_VERSION } from '../dist/version.js';

// Test config documents own their temporary dataDir/port. Do not inherit developer-machine overrides.
delete process.env.CTMCP_DATA_DIR;
delete process.env.CTMCP_PORT;

function document(root, dataDir, overrides = {}) {
  return {
    host: '127.0.0.1',
    port: 3789,
    dataDir,
    permissionMode: 'trusted',
    toolProfile: 'core',
    management: { enabled: true },
    oauth: {
      clientId: 'chatgpt',
      password: 'management-test-password',
      clientSecret: 'management-test-client-secret',
      tokenSecret: 'management-test-token-secret-that-is-long-enough'
    },
    folders: [{ id: 'repo', name: 'Repo', path: root }],
    limits: {
      blockingConcurrency: 4,
      processConcurrency: 4,
      activeSessionLimit: 16,
      maxOutputBytes: 1024 * 1024
    },
    ...overrides
  };
}
function editable(config, overrides = {}) {
  return {
    host: config.host,
    port: config.port,
    publicBaseUrl: config.publicBaseUrl ?? '',
    dataDir: config.dataDir,
    permissionMode: config.permissionMode,
    toolProfile: config.toolProfile,
    management: { enabled: true },
    oauth: { clientId: config.oauth.clientId, password: '', clientSecret: '', clearClientSecret: false },
    policy: config.policy,
    folders: config.folders,
    limits: config.limits,
    tunnel: { enabled: false, publicUrl: '', enrollmentUrl: '', clearEnrollmentUrl: false },
    ...overrides
  };
}

function managementToken(html) {
  return html.match(/name="ctmcp-admin-token" content="([A-Za-z0-9_-]+)"/)?.[1];
}

async function withoutEnvironmentOverrides(keys, callback) {
  const previous = new Map(keys.map(key => [key, process.env[key]]));
  for (const key of keys) delete process.env[key];
  try {
    return await callback();
  } finally {
    for (const [key, value] of previous) {
      if (value === undefined) delete process.env[key];
      else process.env[key] = value;
    }
  }
}

let managementPortSequence = 0;

async function listenOnFetchSafePort(server) {
  let lastError;
  for (let attempt = 0; attempt < 32; attempt += 1) {
    const port = 40_000 + ((process.pid + managementPortSequence++) % 8_000);
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
        server.listen(port, '127.0.0.1');
      });
      return port;
    } catch (error) {
      lastError = error;
      if (error?.code !== 'EADDRINUSE') throw error;
    }
  }
  throw lastError ?? new Error('unable to allocate a fetch-safe management test port');
}

async function rawJsonRequest(url, options = {}) {
  return new Promise((resolve, reject) => {
    const request = httpRequest(url, options, response => {
      const chunks = [];
      response.on('data', chunk => chunks.push(chunk));
      response.on('end', () => {
        const text = Buffer.concat(chunks).toString('utf8');
        resolve({
          status: response.statusCode ?? 0,
          headers: response.headers,
          json: text ? JSON.parse(text) : null
        });
      });
    });
    request.on('error', reject);
    request.end();
  });
}

async function startManagementServer(t, documentOverrides = {}) {
  const root = await mkdtemp(path.join(tmpdir(), 'ctmcp-management-root-'));
  const dataDir = await mkdtemp(path.join(tmpdir(), 'ctmcp-management-data-'));
  const configPath = path.join(dataDir, 'agent.json');
  await writeFile(configPath, `${JSON.stringify(document(root, dataDir, documentOverrides), null, 2)}\n`);
  const loaded = await loadConfigBundle(configPath);
  loaded.config.port = 0;
  const store = new ConfigStore(loaded);
  const runtimeRegistry = new Map();
  const runtime = await createAgentRuntime(loaded.config, { configStore: store, runtimeRegistry });
  const tunnelReconfigurations = [];
  const workspaceId = loaded.config.workspaceId ?? runtime.context.workspaceProfileId;
  const runtimeRecord = runtimeRegistry.get(workspaceId);
  if (runtimeRecord) {
    runtimeRecord.tunnel = {
      async reconfigure(tunnel, publicBaseUrl) {
        tunnelReconfigurations.push({
          tunnel: tunnel ? structuredClone(tunnel) : undefined,
          publicBaseUrl
        });
        if (tunnel) runtime.context.config.tunnel = structuredClone(tunnel);
        else delete runtime.context.config.tunnel;
        if (publicBaseUrl) runtime.context.config.publicBaseUrl = publicBaseUrl;
        else delete runtime.context.config.publicBaseUrl;
      },
      async enforceSecurity() {},
      async start() {
        runtime.context.tunnelStatus = {
          enabled: true,
          state: 'running',
          publicUrl: runtime.context.config.tunnel?.publicUrl || 'https://example.test/mcp',
          workers: 1,
          connectedWorkers: 1,
          completedRequests: 0
        };
      },
      async stop() {
        if (runtime.context.tunnelStatus) {
          runtime.context.tunnelStatus.state = 'stopped';
          runtime.context.tunnelStatus.connectedWorkers = 0;
          runtime.context.tunnelStatus.workers = 0;
        }
      }
    };
  }
  const port = await listenOnFetchSafePort(runtime.server);
  t.after(() => new Promise(resolve => runtime.server.close(resolve)));
  return {
    root, dataDir, configPath, loaded, store, context: runtime.context,
    oauth: runtime.oauth, runtimeRegistry, tunnelReconfigurations,
    base: `http://127.0.0.1:${port}`
  };
}

test('management UI is loopback-only, token protected and never returns configured secrets', async t => {
  const runtime = await startManagementServer(t);
  const page = await fetch(`${runtime.base}/ui`);
  assert.equal(page.status, 200);
  const csp = page.headers.get('content-security-policy');
  assert.match(csp, /default-src 'none'/);
  assert.match(csp, /script-src 'self'/);
  assert.match(csp, /style-src 'self'/);
  assert.doesNotMatch(csp, /unsafe-inline/);
  assert.equal(page.headers.get('x-frame-options'), 'DENY');
  const html = await page.text();
  assert.match(html, /Headless Agent/);
  assert.match(html, /data-ui-framework="svelte"/);
  assert.doesNotMatch(html, /<style\b/);
  assert.doesNotMatch(html, /<script(?![^>]*\bsrc=)/);
  const token = managementToken(html);
  assert.ok(token);

  const scriptSrc = html.match(/src="(\/ui\/[^"]+\.js)"/)?.[1];
  if (scriptSrc) {
    const scriptResponse = await fetch(`${runtime.base}${scriptSrc}`);
    if (scriptResponse.status === 200) {
      assert.match(scriptResponse.headers.get('content-type'), /javascript/);
      assert.equal(scriptResponse.headers.get('cache-control'), 'no-store');
      const script = await scriptResponse.text();
      assert.doesNotMatch(script, new RegExp(token));
    }
    const rejectedAssetMethod = await fetch(`${runtime.base}${scriptSrc}`, { method: 'POST' });
    assert.equal(rejectedAssetMethod.status, 405);
    assert.equal(rejectedAssetMethod.headers.get('allow'), 'GET');
  }

  const manifestResponse = await fetch(`${runtime.base}/ui/manifest.webmanifest`);
  assert.equal(manifestResponse.status, 200);
  const manifest = await manifestResponse.json();
  assert.equal(manifest.display, 'standalone');
  assert.equal(manifest.start_url, '/ui/');
  const worker = await fetch(`${runtime.base}/ui/sw.js`);
  assert.equal(worker.status, 200);
  assert.equal(worker.headers.get('cache-control'), 'no-store');
  const workerSource = await worker.text();
  assert.doesNotMatch(workerSource, /caches?\.(?:open|match)|fetch\s*\(/);
  const denied = await fetch(`${runtime.base}/admin/api/config`);
  assert.equal(denied.status, 403);

  const rejectedOrigin = await fetch(`${runtime.base}/admin/api/config`, {
    headers: { 'x-ctmcp-admin-token': token, origin: 'https://attacker.example' }
  });
  assert.equal(rejectedOrigin.status, 403);

  const response = await fetch(`${runtime.base}/admin/api/config`, {
    headers: { 'x-ctmcp-admin-token': token }
  });
  assert.equal(response.status, 200);
  const snapshot = await response.json();
  const serialized = JSON.stringify(snapshot);
  assert.doesNotMatch(serialized, /management-test-password/);
  assert.doesNotMatch(serialized, /management-test-client-secret/);
  assert.doesNotMatch(serialized, /management-test-token-secret/);
  assert.equal(snapshot.effective.folders[0].id, 'repo');
  assert.equal(snapshot.effective.oauth.passwordConfigured, true);
  assert.equal(snapshot.effective.oauth.clientSecretConfigured, true);
  assert.equal(snapshot.saved.toolProfile, 'core');
  assert.equal(snapshot.saved.activeToolProfile, 'trusted-core');
  assert.equal(snapshot.effective.toolProfile, 'core');
  assert.equal(snapshot.effective.activeToolProfile, 'trusted-core');
  assert.equal(snapshot.schemaVersion, 1);
  assert.equal(snapshot.secretStorePath, runtime.loaded.secretStorePath);
  assert.equal(snapshot.migrationApplied, true);
  assert.equal(snapshot.migratedFromSchema, 0);

  const statusResponse = await fetch(`${runtime.base}/admin/api/status`, {
    headers: { 'x-ctmcp-admin-token': token }
  });
  assert.equal(statusResponse.status, 200);
  const status = await statusResponse.json();
  assert.equal(status.configuredToolProfile, 'core');
  assert.equal(status.toolProfile, 'trusted-core');
  assert.equal(status.tools, 40);
  assert.match(status.toolsetRevision, /^[0-9a-f]{16}$/);
});

test('management folder picker lists directories without exposing files', async t => {
  const runtime = await startManagementServer(t);
  const childA = path.join(runtime.root, 'child-a');
  const childB = path.join(runtime.root, 'child-b');
  await Promise.all([
    mkdir(childA),
    mkdir(childB),
    writeFile(path.join(runtime.root, 'not-a-directory.txt'), 'hidden from picker\n')
  ]);

  const pageHtml = await fetch(`${runtime.base}/ui`).then(response => response.text());
  const token = managementToken(pageHtml);
  assert.ok(token);
  const headers = { 'x-ctmcp-admin-token': token, origin: runtime.base };
  const workspaceId = runtime.context.config.workspaceId ?? runtime.context.workspaceProfileId;
  const response = await fetch(`${runtime.base}/admin/api/directories?workspaceId=${encodeURIComponent(workspaceId)}&path=${encodeURIComponent(runtime.root)}`, { headers });
  assert.equal(response.status, 200);
  const payload = await response.json();
  assert.equal(payload.ok, true);
  assert.equal(payload.path, path.normalize(runtime.root));
  assert.equal(payload.parent, path.dirname(path.normalize(runtime.root)));
  assert.deepEqual(payload.directories.map(entry => entry.name), ['child-a', 'child-b']);
  assert.deepEqual(payload.directories.map(entry => entry.path), [childA, childB]);
  assert.equal(payload.totalDirectories, 2);
  assert.equal(payload.truncated, false);
  assert.ok(payload.roots.includes(path.parse(runtime.root).root));

  const unknownWorkspaceResponse = await fetch(`${runtime.base}/admin/api/directories?workspaceId=missing-workspace`, { headers });
  assert.equal(unknownWorkspaceResponse.status, 404);
  assert.equal((await unknownWorkspaceResponse.json()).error.code, 'WORKSPACE_NOT_FOUND');

  const fileResponse = await fetch(`${runtime.base}/admin/api/directories?path=${encodeURIComponent(path.join(runtime.root, 'not-a-directory.txt'))}`, { headers });
  assert.equal(fileResponse.status, 400);
  assert.equal((await fileResponse.json()).error.code, 'DIRECTORY_BROWSE_FAILED');

  const missingResponse = await fetch(`${runtime.base}/admin/api/directories?path=${encodeURIComponent(path.join(runtime.root, 'missing'))}`, { headers });
  assert.equal(missingResponse.status, 404);
  assert.equal((await missingResponse.json()).error.code, 'DIRECTORY_BROWSE_FAILED');

  const opened = await fetch(`${runtime.base}/admin/api/directories/open`, {
    method: 'POST',
    headers: { ...headers, 'content-type': 'application/json' },
    body: JSON.stringify({ path: runtime.root })
  });
  assert.equal(opened.status, 200);
  assert.equal((await opened.json()).path, path.normalize(runtime.root));
});

test('management can start and stop built-in WSS for a workspace', async t => {
  const runtime = await startManagementServer(t, {
    tunnel: { enabled: true, publicUrl: 'https://example.test/builtin/clients/test/mcp' }
  });
  const pageHtml = await fetch(`${runtime.base}/ui`).then(response => response.text());
  const token = managementToken(pageHtml);
  const headers = {
    'x-ctmcp-admin-token': token,
    origin: runtime.base,
    'content-type': 'application/json'
  };
  const workspaceId = runtime.context.config.workspaceId ?? runtime.context.workspaceProfileId;
  const started = await fetch(`${runtime.base}/admin/api/workspaces/${workspaceId}/tunnel/start`, {
    method: 'POST',
    headers,
    body: JSON.stringify({ service: 'mcp' })
  });
  assert.equal(started.status, 200);
  const startedPayload = await started.json();
  assert.equal(startedPayload.state, 'running');
  assert.match(startedPayload.publicUrl, /^https:\/\//);

  const stopped = await fetch(`${runtime.base}/admin/api/workspaces/${workspaceId}/tunnel/stop`, {
    method: 'POST',
    headers,
    body: JSON.stringify({ service: 'mcp' })
  });
  assert.equal(stopped.status, 200);
  assert.equal((await stopped.json()).state, 'stopped');
});

test('workspace registry can add and remove a secondary workspace', async t => {
  const root = await mkdtemp(path.join(tmpdir(), 'ctmcp-registry-root-'));
  const extra = await mkdtemp(path.join(tmpdir(), 'ctmcp-registry-extra-'));
  const dataDir = await mkdtemp(path.join(tmpdir(), 'ctmcp-registry-data-'));
  t.after(() => Promise.all([root, extra, dataDir].map(dir => rm(dir, { recursive: true, force: true }))));
  const configPath = path.join(dataDir, 'agent.json');
  await writeFile(configPath, `${JSON.stringify(document(root, dataDir), null, 2)}\n`);
  const application = await loadApplication(configPath);
  const store = new ApplicationConfigStore(application);
  const created = await store.addWorkspace(extra, 'Extra');
  assert.equal(created.ok, true);
  assert.equal(created.restartRequired, true);
  assert.equal(store.entries().length, 2);
  assert.equal(store.workspace(created.id).name, 'Extra');
  const removed = await store.deleteWorkspace(created.id);
  assert.equal(removed.ok, true);
  assert.equal(store.entries().length, 1);
  await assert.rejects(() => store.deleteWorkspace(store.primaryWorkspaceId), /primary workspace cannot be deleted/);
});


test('management hot-applies idle workspace folder changes without restart', async t => {
  const runtime = await startManagementServer(t);
  const nextRoot = await mkdtemp(path.join(tmpdir(), 'ctmcp-management-hot-root-'));
  t.after(() => rm(nextRoot, { recursive: true, force: true }));
  const pageHtml = await fetch(`${runtime.base}/ui`).then(response => response.text());
  const token = managementToken(pageHtml);
  assert.ok(token);

  runtime.loaded.config.port = runtime.loaded.document.port;
  runtime.context.config.port = runtime.loaded.document.port;
  const previousWorkspaceOverride = process.env.CTMCP_WORKSPACES;
  delete process.env.CTMCP_WORKSPACES;
  try {
    const response = await fetch(`${runtime.base}/admin/api/config`, {
      method: 'PUT',
      headers: {
        'x-ctmcp-admin-token': token,
        'content-type': 'application/json',
        origin: runtime.base
      },
      body: JSON.stringify(editable(runtime.loaded.config, {
        folders: [{ id: 'repo', name: 'Hot root', path: nextRoot }]
      }))
    });
    assert.equal(response.status, 200);
    const result = await response.json();
    const canonicalRoot = await realpath(nextRoot);
    assert.equal(result.restartRequired, false);
    assert.deepEqual(result.appliedImmediately, ['folders']);
    assert.equal(result.hotApplyDeferredReason, null);
    assert.equal(runtime.loaded.config.folders[0].path, canonicalRoot);
    assert.equal(runtime.context.config.folders[0].path, canonicalRoot);
    assert.equal(runtime.context.folderRuntimes.get('repo')?.workspacePath, canonicalRoot);

    const snapshot = await fetch(`${runtime.base}/admin/api/config`, {
      headers: { 'x-ctmcp-admin-token': token }
    }).then(item => item.json());
    assert.equal(snapshot.restartRequired, false);
    assert.equal(snapshot.effective.folders[0].path, canonicalRoot);
  } finally {
    if (previousWorkspaceOverride === undefined) delete process.env.CTMCP_WORKSPACES;
    else process.env.CTMCP_WORKSPACES = previousWorkspaceOverride;
  }
});

test('management hot-applies sandbox generations while existing process work keeps its original boundary', async t => {
  const runtime = await startManagementServer(t, {
    sandbox: { enabled: false, backend: 'appcontainer', externalPaths: [], options: {} }
  });
  const pageHtml = await fetch(`${runtime.base}/ui`).then(response => response.text());
  const token = pageHtml.match(/<meta name="ctmcp-admin-token" content="([^"]+)"/)?.[1];
  assert.ok(token);
  runtime.loaded.config.port = runtime.loaded.document.port;
  runtime.context.config.port = runtime.loaded.document.port;
  const workspaceId = runtime.loaded.config.workspaceId ?? runtime.context.workspaceProfileId;
  const runtimeRecord = runtime.runtimeRegistry.get(workspaceId);
  assert.ok(runtimeRecord);
  const preflighted = [];
  runtimeRecord.preflightSandbox = async sandbox => preflighted.push(structuredClone(sandbox));

  await withoutEnvironmentOverrides([
    'CTMCP_WORKSPACES', 'CTMCP_SANDBOX_ENABLED', 'CTMCP_SANDBOX_BACKEND',
    'CTMCP_WSLC_IMAGE', 'CTMCP_WSLC_NETWORK'
  ], async () => {
    const sandbox = {
      enabled: true,
      backend: 'wslc',
      externalPaths: [],
      options: { 'wslc.image': 'alpine:3.20', 'wslc.network': 'none' }
    };
    const response = await fetch(`${runtime.base}/admin/api/config`, {
      method: 'PUT',
      headers: {
        'x-ctmcp-admin-token': token,
        'content-type': 'application/json',
        origin: runtime.base
      },
      body: JSON.stringify(editable(runtime.loaded.config, { sandbox }))
    });
    assert.equal(response.status, 200);
    const result = await response.json();
    assert.equal(result.restartRequired, false);
    assert.ok(result.appliedImmediately.includes('sandbox'));
    assert.equal(result.hotApplyDeferredReason, null);
    assert.deepEqual(preflighted, [sandbox]);
    assert.deepEqual(runtime.loaded.config.sandbox, sandbox);
    assert.deepEqual(runtime.context.config.sandbox, sandbox);

    const folderRuntime = runtime.context.folderRuntimes.get('repo');
    assert.ok(folderRuntime);
    folderRuntime.sessions.set('sandbox-busy-fixture', {
      finalizedAt: undefined,
      sandboxEnforced: true,
      sandboxBackend: 'wslc',
      executionBoundary: 'wslc'
    });
    try {
      const nextSandbox = { ...sandbox, options: { ...sandbox.options, 'wslc.network': 'bridge' } };
      const switchedResponse = await fetch(`${runtime.base}/admin/api/config`, {
        method: 'PUT',
        headers: {
          'x-ctmcp-admin-token': token,
          'content-type': 'application/json',
          origin: runtime.base
        },
        body: JSON.stringify(editable(runtime.loaded.config, { sandbox: nextSandbox }))
      });
      assert.equal(switchedResponse.status, 200);
      const switched = await switchedResponse.json();
      assert.equal(switched.restartRequired, false);
      assert.equal(switched.hotApplyDeferredReason, null);
      assert.ok(switched.appliedImmediately.includes('sandbox'));
      assert.equal(runtime.context.config.sandbox.options['wslc.network'], 'bridge');
      const oldSession = folderRuntime.sessions.get('sandbox-busy-fixture');
      assert.equal(oldSession.sandboxEnforced, true);
      assert.equal(oldSession.sandboxBackend, 'wslc');
      assert.equal(oldSession.executionBoundary, 'wslc');
    } finally {
      folderRuntime.sessions.delete('sandbox-busy-fixture');
    }

    const disabledSandbox = { ...sandbox, enabled: false };
    const disableResponse = await fetch(`${runtime.base}/admin/api/config`, {
      method: 'PUT',
      headers: {
        'x-ctmcp-admin-token': token,
        'content-type': 'application/json',
        origin: runtime.base
      },
      body: JSON.stringify(editable(runtime.loaded.config, { sandbox: disabledSandbox }))
    });
    assert.equal(disableResponse.status, 200);
    const disabled = await disableResponse.json();
    assert.equal(disabled.restartRequired, false);
    assert.equal(disabled.hotApplyDeferredReason, null);
    assert.ok(disabled.appliedImmediately.includes('sandbox'));
    assert.equal(disabled.saved.sandbox.enabled, false);
    assert.equal(runtime.context.config.sandbox.enabled, false);
  });
});

test('management leaves persisted and live sandbox generations unchanged when preflight fails', async t => {
  const runtime = await startManagementServer(t, {
    sandbox: { enabled: false, backend: 'appcontainer', externalPaths: [], options: {} }
  });
  const pageHtml = await fetch(`${runtime.base}/ui`).then(response => response.text());
  const token = pageHtml.match(/<meta name="ctmcp-admin-token" content="([^"]+)"/)?.[1];
  assert.ok(token);
  runtime.loaded.config.port = runtime.loaded.document.port;
  runtime.context.config.port = runtime.loaded.document.port;
  const workspaceId = runtime.loaded.config.workspaceId ?? runtime.context.workspaceProfileId;
  const runtimeRecord = runtime.runtimeRegistry.get(workspaceId);
  assert.ok(runtimeRecord);
  runtimeRecord.preflightSandbox = async () => { throw new Error('sandbox-preflight-fixture'); };
  const beforeFile = await readFile(runtime.configPath, 'utf8');
  const beforeLive = structuredClone(runtime.context.config.sandbox);
  const target = { enabled: true, backend: 'appcontainer', externalPaths: [], options: {} };

  const response = await fetch(`${runtime.base}/admin/api/config`, {
    method: 'PUT',
    headers: {
      'x-ctmcp-admin-token': token,
      'content-type': 'application/json',
      origin: runtime.base
    },
    body: JSON.stringify(editable(runtime.loaded.config, { sandbox: target }))
  });
  assert.equal(response.status, 400);
  assert.match(JSON.stringify(await response.json()), /sandbox-preflight-fixture/);
  assert.equal(await readFile(runtime.configPath, 'utf8'), beforeFile);
  assert.deepEqual(runtime.context.config.sandbox, beforeLive);
  assert.deepEqual(runtime.loaded.config.sandbox, beforeLive);
});
test('management hot-applies policy and live limits without restart', async t => {
  const runtime = await startManagementServer(t);
  const pageHtml = await fetch(`${runtime.base}/ui`).then(response => response.text());
  const token = managementToken(pageHtml);
  assert.ok(token);
  runtime.loaded.config.port = runtime.loaded.document.port;
  runtime.context.config.port = runtime.loaded.document.port;

  await withoutEnvironmentOverrides([
    'CTMCP_WORKSPACES', 'CTMCP_ALLOWED_COMMANDS', 'CTMCP_WORKSPACE_LOCAL_ENTRIES',
    'CTMCP_WORKSPACE_SCRIPT_EXTENSIONS', 'CTMCP_MAX_PATCH_BYTES',
    'CTMCP_ACTIVE_SESSION_LIMIT', 'CTMCP_MAX_OUTPUT_BYTES', 'CTMCP_COMMAND_TIMEOUT_MAX_MS',
    'CTMCP_BLOCKING_CONCURRENCY', 'CTMCP_PROCESS_CONCURRENCY',
    'CTMCP_GLOBAL_BLOCKING_CONCURRENCY', 'CTMCP_GLOBAL_PROCESS_CONCURRENCY'
  ], async () => {
    const policy = {
      ...runtime.loaded.config.policy,
      allowedCommands: [...runtime.loaded.config.policy.allowedCommands, 'hot-apply-test-command'],
      workspaceLocalEntries: !runtime.loaded.config.policy.workspaceLocalEntries,
      maxPatchBytes: runtime.loaded.config.policy.maxPatchBytes + 1
    };
    const limits = {
      ...runtime.loaded.config.limits,
      blockingConcurrency: runtime.loaded.config.limits.blockingConcurrency + 1,
      processConcurrency: runtime.loaded.config.limits.processConcurrency + 1,
      globalBlockingConcurrency: runtime.loaded.config.limits.globalBlockingConcurrency + 1,
      globalProcessConcurrency: runtime.loaded.config.limits.globalProcessConcurrency + 1,
      activeSessionLimit: runtime.loaded.config.limits.activeSessionLimit + 1,
      maxOutputBytes: runtime.loaded.config.limits.maxOutputBytes + 1_024,
      commandTimeoutMaxMs: Math.min(runtime.loaded.config.limits.commandTimeoutMaxMs + 60_000, 3_600_000)
    };
    const response = await fetch(`${runtime.base}/admin/api/config`, {
      method: 'PUT',
      headers: {
        'x-ctmcp-admin-token': token,
        'content-type': 'application/json',
        origin: runtime.base
      },
      body: JSON.stringify(editable(runtime.loaded.config, { policy, limits }))
    });
    assert.equal(response.status, 200);
    const result = await response.json();
    assert.equal(result.restartRequired, false);
    assert.deepEqual([...result.appliedImmediately].sort(), [
      'limits.activeSessionLimit', 'limits.blockingConcurrency',
      'limits.commandTimeoutMaxMs', 'limits.globalBlockingConcurrency', 'limits.globalProcessConcurrency',
      'limits.maxOutputBytes', 'limits.processConcurrency', 'policy'
    ]);
    assert.deepEqual(runtime.loaded.config.policy, policy);
    assert.deepEqual(runtime.context.config.policy, policy);
    assert.equal(runtime.loaded.config.limits.activeSessionLimit, limits.activeSessionLimit);
    assert.equal(runtime.context.config.limits.activeSessionLimit, limits.activeSessionLimit);
    assert.equal(runtime.loaded.config.limits.maxOutputBytes, limits.maxOutputBytes);
    assert.equal(runtime.context.config.limits.maxOutputBytes, limits.maxOutputBytes);
    assert.equal(runtime.loaded.config.limits.commandTimeoutMaxMs, limits.commandTimeoutMaxMs);
    assert.equal(runtime.context.config.limits.commandTimeoutMaxMs, limits.commandTimeoutMaxMs);
    assert.equal(runtime.context.folderRuntimes.get('repo')?.admission.blocking.limit, limits.blockingConcurrency);
    assert.equal(runtime.context.folderRuntimes.get('repo')?.admission.process.limit, limits.processConcurrency);
    assert.equal(runtime.context.hubAdmission.blocking.limit, limits.globalBlockingConcurrency);
    assert.equal(runtime.context.hubAdmission.process.limit, limits.globalProcessConcurrency);
  });
});

test('management hot-applies OAuth credentials without restart', async t => {
  const runtime = await startManagementServer(t);
  const pageHtml = await fetch(`${runtime.base}/ui`).then(response => response.text());
  const token = managementToken(pageHtml);
  assert.ok(token);
  runtime.loaded.config.port = runtime.loaded.document.port;
  runtime.context.config.port = runtime.loaded.document.port;

  await withoutEnvironmentOverrides([
    'CTMCP_WORKSPACES', 'CTMCP_OAUTH_CLIENT_ID', 'CTMCP_OAUTH_CLIENT_SECRET',
    'CTMCP_OAUTH_PASSWORD', 'CTMCP_OAUTH_TOKEN_SECRET'
  ], async () => {
    const oauth = {
      clientId: 'chatgpt-rotated',
      password: 'rotated-management-password',
      clientSecret: 'rotated-management-client-secret',
      clearClientSecret: false
    };
    const response = await fetch(`${runtime.base}/admin/api/config`, {
      method: 'PUT',
      headers: {
        'x-ctmcp-admin-token': token,
        'content-type': 'application/json',
        origin: runtime.base
      },
      body: JSON.stringify(editable(runtime.loaded.config, { oauth }))
    });
    assert.equal(response.status, 200);
    const result = await response.json();
    assert.equal(result.restartRequired, false);
    assert.deepEqual(result.appliedImmediately, ['oauth']);
    assert.equal(runtime.oauth.clientId, oauth.clientId);
    assert.equal(runtime.oauth.password, oauth.password);
    assert.equal(runtime.oauth.clientSecret, oauth.clientSecret);
    assert.equal(runtime.context.config.oauth.clientId, oauth.clientId);
    assert.equal(runtime.loaded.config.oauth.clientId, oauth.clientId);
  });
});

test('management hot-applies tunnel configuration through the runtime controller', async t => {
  const runtime = await startManagementServer(t);
  const pageHtml = await fetch(`${runtime.base}/ui`).then(response => response.text());
  const token = managementToken(pageHtml);
  assert.ok(token);
  runtime.loaded.config.port = runtime.loaded.document.port;
  runtime.context.config.port = runtime.loaded.document.port;

  await withoutEnvironmentOverrides([
    'CTMCP_WORKSPACES', 'CTMCP_PUBLIC_BASE_URL', 'CTMCP_BUILTIN_ENABLED',
    'CTMCP_BUILTIN_PUBLIC_URL', 'CTMCP_BUILTIN_ENROLLMENT_URL'
  ], async () => {
    const publicBaseUrl = 'https://tunnel.example/builtin/clients/device_1';
    const tunnel = {
      enabled: true,
      publicUrl: `${publicBaseUrl}/mcp`,
      enrollmentUrl: '',
      clearEnrollmentUrl: false
    };
    const response = await fetch(`${runtime.base}/admin/api/config`, {
      method: 'PUT',
      headers: {
        'x-ctmcp-admin-token': token,
        'content-type': 'application/json',
        origin: runtime.base
      },
      body: JSON.stringify(editable(runtime.loaded.config, { publicBaseUrl, tunnel }))
    });
    assert.equal(response.status, 200);
    const result = await response.json();
    assert.equal(result.restartRequired, false);
    assert.deepEqual(result.appliedImmediately, ['tunnel']);
    assert.equal(runtime.tunnelReconfigurations.length, 1);
    assert.equal(runtime.tunnelReconfigurations[0].publicBaseUrl, publicBaseUrl);
    assert.equal(runtime.tunnelReconfigurations[0].tunnel.enabled, true);
    assert.equal(runtime.tunnelReconfigurations[0].tunnel.publicUrl, `${publicBaseUrl}/mcp`);
    assert.equal(runtime.tunnelReconfigurations[0].tunnel.enrollmentUrl, undefined);
    assert.match(runtime.tunnelReconfigurations[0].tunnel.stateFile, /builtin-tunnel-identity\.enc\.json$/);
    assert.equal(runtime.context.config.tunnel.publicUrl, `${publicBaseUrl}/mcp`);
    assert.equal(runtime.loaded.config.tunnel.publicUrl, `${publicBaseUrl}/mcp`);
    assert.equal(runtime.loaded.config.publicBaseUrl, publicBaseUrl);
  });
});

test('management defers a busy concurrency lane without replacing its semaphore', async t => {
  const runtime = await startManagementServer(t);
  const pageHtml = await fetch(`${runtime.base}/ui`).then(response => response.text());
  const token = managementToken(pageHtml);
  assert.ok(token);
  runtime.loaded.config.port = runtime.loaded.document.port;
  runtime.context.config.port = runtime.loaded.document.port;

  await withoutEnvironmentOverrides([
    'CTMCP_WORKSPACES', 'CTMCP_GLOBAL_PROCESS_CONCURRENCY'
  ], async () => {
    const previousLimit = runtime.context.hubAdmission.process.limit;
    const release = await runtime.context.hubAdmission.process.acquire();
    try {
      const limits = {
        ...runtime.loaded.config.limits,
        globalProcessConcurrency: previousLimit + 1
      };
      const response = await fetch(`${runtime.base}/admin/api/config`, {
        method: 'PUT',
        headers: {
          'x-ctmcp-admin-token': token,
          'content-type': 'application/json',
          origin: runtime.base
        },
        body: JSON.stringify(editable(runtime.loaded.config, { limits }))
      });
      assert.equal(response.status, 200);
      const result = await response.json();
      assert.equal(result.restartRequired, true);
      assert.deepEqual(result.appliedImmediately, []);
      assert.match(result.hotApplyDeferredReason, /globalProcessConcurrency/);
      assert.equal(runtime.context.hubAdmission.process.limit, previousLimit);
      assert.equal(runtime.context.config.limits.globalProcessConcurrency, previousLimit);
    } finally {
      release();
    }
  });
});

test('management hot-applies security policy telemetry and tool catalog changes', async t => {
  const runtime = await startManagementServer(t);
  const pageHtml = await fetch(`${runtime.base}/ui`).then(response => response.text());
  const token = managementToken(pageHtml);
  assert.ok(token);
  runtime.loaded.config.port = runtime.loaded.document.port;
  runtime.context.config.port = runtime.loaded.document.port;

  await withoutEnvironmentOverrides([
    'CTMCP_WORKSPACES', 'CTMCP_PERMISSION_MODE', 'CTMCP_TOOL_PROFILE'
  ], async () => {
    const securityPolicy = {
      ...runtime.loaded.config.securityPolicy,
      requireShellConfirmation: !runtime.loaded.config.securityPolicy.requireShellConfirmation,
      redactTelemetry: !runtime.loaded.config.securityPolicy.redactTelemetry,
      enforceHarnessBaseline: false
    };
    const safeResponse = await fetch(`${runtime.base}/admin/api/config`, {
      method: 'PUT',
      headers: {
        'x-ctmcp-admin-token': token,
        'content-type': 'application/json',
        origin: runtime.base
      },
      body: JSON.stringify(editable(runtime.loaded.config, { securityPolicy }))
    });
    assert.equal(safeResponse.status, 200);
    const safeResult = await safeResponse.json();
    assert.equal(safeResult.restartRequired, false);
    assert.deepEqual(safeResult.appliedImmediately, ['securityPolicy']);
    assert.deepEqual(runtime.context.config.securityPolicy, securityPolicy);
    assert.equal(runtime.context.config.securityPolicyCustomized, true);
    assert.equal(runtime.context.usageStore.redactTelemetry, securityPolicy.redactTelemetry);
    const reloaded = await loadConfigBundle(runtime.configPath);
    assert.equal(reloaded.config.securityPolicy.enforceHarnessBaseline, false);

    const catalogChangingPolicy = { ...securityPolicy, restrictToolCatalog: false };
    const deferredResponse = await fetch(`${runtime.base}/admin/api/config`, {
      method: 'PUT',
      headers: {
        'x-ctmcp-admin-token': token,
        'content-type': 'application/json',
        origin: runtime.base
      },
      body: JSON.stringify(editable(runtime.loaded.config, { securityPolicy: catalogChangingPolicy }))
    });
    assert.equal(deferredResponse.status, 200);
    const deferredResult = await deferredResponse.json();
    assert.equal(deferredResult.restartRequired, false);
    assert.deepEqual(deferredResult.appliedImmediately, ['securityPolicy', 'toolCatalog']);
    assert.equal(runtime.context.config.securityPolicy.restrictToolCatalog, false);
    assert.equal(runtime.context.config.activeToolProfile, 'advanced');
  });
});

test('management core is separated from the React UI implementation', async () => {
  const [managementSource, routerSource, configSource, observabilitySource, uiAdapterSource, layoutSource, nodeBackend] = await Promise.all([
    readFile(new URL('../src/management.ts', import.meta.url), 'utf8'),
    readFile(new URL('../src/management/router.ts', import.meta.url), 'utf8'),
    readFile(new URL('../src/management/configStore.ts', import.meta.url), 'utf8'),
    readFile(new URL('../src/management/routes/observability.ts', import.meta.url), 'utf8'),
    readFile(new URL('../src/managementUi.ts', import.meta.url), 'utf8'),
    readFile(new URL('../../../src/routes/+layout.svelte', import.meta.url), 'utf8'),
    readFile(new URL('../../../src/lib/backend/node.ts', import.meta.url), 'utf8')
  ]);
  const backendSource = `${managementSource}\n${routerSource}\n${configSource}\n${observabilitySource}`;
  assert.doesNotMatch(backendSource, /<!doctype html|<style\b|document\.(?:getElementById|querySelector|createElement)|from ['"]react|from ['"]react-bootstrap|bootstrap\/dist/i);
  assert.match(managementSource, /management\/configStore/);
  assert.match(managementSource, /management\/router/);
  assert.match(routerSource, /handleManagementUiRequest/);
  assert.doesNotMatch(routerSource, /writeConfigDocument|writeAgentSecrets/);
  assert.match(configSource, /class ConfigStore/);
  assert.doesNotMatch(configSource, /\/admin\/api\//);
  assert.match(observabilitySource, /managementTelemetryPayload/);
  assert.match(uiAdapterSource, /data-ui-framework="svelte"/);
  assert.match(uiAdapterSource, /script-src 'self'/);
  assert.doesNotMatch(uiAdapterSource, /unsafe-inline/);
  assert.match(layoutSource, /installHostBackend\(\)/);
  assert.match(nodeBackend, /x-ctmcp-admin-token/);
  assert.doesNotMatch(layoutSource, /dangerouslySetInnerHTML|innerHTML/);
});

test('management runtime contracts remain independent from the config store', async () => {
  const [configSource, typesSource, runtimeContractSource] = await Promise.all([
    readFile(new URL('../src/management/configStore.ts', import.meta.url), 'utf8'),
    readFile(new URL('../src/management/types.ts', import.meta.url), 'utf8'),
    readFile(new URL('../src/management/runtimeContract.ts', import.meta.url), 'utf8')
  ]);
  assert.match(configSource, /from ['"]\.\/runtimeContract\.js['"]/);
  assert.doesNotMatch(configSource, /from ['"]\.\/types\.js['"]/);
  assert.match(typesSource, /from ['"]\.\/runtimeContract\.js['"]/);
  assert.match(typesSource, /from ['"]\.\/configStore\.js['"]/);
  assert.doesNotMatch(runtimeContractSource, /configStore|from ['"]\.\/types\.js['"]/);
  assert.match(runtimeContractSource, /interface RuntimeHotApplyTarget/);
  assert.match(runtimeContractSource, /interface TunnelRuntimeController/);
});

test('shared Svelte observability surfaces stay on the frontend backend contract', async () => {
  const [historySource, telemetrySource, logsSource, healthSource, nodeBackend] = await Promise.all([
    readFile(new URL('../../../src/lib/components/HistoryViewer.svelte', import.meta.url), 'utf8'),
    readFile(new URL('../../../src/lib/components/TelemetryViewer.svelte', import.meta.url), 'utf8'),
    readFile(new URL('../../../src/lib/components/OperationLogViewer.svelte', import.meta.url), 'utf8'),
    readFile(new URL('../../../src/lib/components/HealthPanel.svelte', import.meta.url), 'utf8'),
    readFile(new URL('../../../src/lib/backend/node.ts', import.meta.url), 'utf8')
  ]);
  assert.match(historySource, /listHistorySessions/);
  assert.match(telemetrySource, /readWorkspaceTelemetry/);
  assert.match(logsSource, /operations\.query/);
  assert.match(healthSource, /runHealthChecks/);
  assert.match(nodeBackend, /workspaceRoute\(workspaceId, "telemetry"\)/);
  assert.match(nodeBackend, /workspaceRoute\(workspaceId, "history"\)/);
  assert.match(nodeBackend, /workspaceRoute\(workspaceId, "health"\)/);
  assert.match(nodeBackend, /workspaceRoute\(workspaceId, "logs"\)/);
});

test('management dashboard returns bounded telemetry without command, environment or output secrets', async t => {
  const runtime = await startManagementServer(t);
  const key = 'dashboard-test';
  runtime.context.selections.set(key, 'repo');
  runtime.context.defaultCwds.set(key, '.');
  const started = await startAndYield(runtime.context, key, {
    program: path.basename(process.execPath),
    args: ['-e', 'process.stdout.write("DASHBOARD_OUTPUT_SECRET")', 'DASHBOARD_ARG_SECRET'],
    env: { DASHBOARD_SECRET: 'DASHBOARD_ENV_SECRET' },
    timeout_ms: 10_000,
    yield_time_ms: 10_000,
    post_checks: [{
      name: 'DASHBOARD_POST_CHECK_NAME_SECRET',
      program: path.basename(process.execPath),
      args: ['-e', 'process.stdout.write("DASHBOARD_POST_CHECK_OUTPUT_SECRET")']
    }]
  });
  const retained = runtimeForFolderId(runtime.context, 'repo').sessions.get(started.session_id);
  assert.ok(retained);
  await waitForSession(retained, retained.sequence, 10_000, 'finalized');

  const now = Date.now();
  for (let index = 0; index < 120; index += 1) {
    runtime.context.usage.push({
      tool: index % 2 ? 'read_file' : 'search_text',
      startedAt: now - index,
      durationMs: index + 1,
      ok: index % 4 !== 0,
      queueWaitMs: index % 3,
      lockWaitMs: index % 2,
      responseBytes: 100 + index
    });
  }
  await runtime.context.state.addOperation('repo', {
    id: 'DASHBOARD_OPERATION_ID_SECRET',
    workspace_id: 'repo',
    tool: 'exec_command',
    kind: 'failed',
    input_summary: {},
    result_summary: { ok: false, duration_ms: 12 },
    reason: 'DASHBOARD_OPERATION_SUMMARY_SECRET',
    affected_files: [],
    created_at: String(now)
  });
  runtimeForFolderId(runtime.context, 'repo').pendingOperations.set('DASHBOARD_PENDING_ID_SECRET', {
    resumeId: 'DASHBOARD_PENDING_ID_SECRET',
    name: 'exec_command',
    args: { secret: 'DASHBOARD_PENDING_ARG_SECRET' },
    meta: null,
    reason: 'DASHBOARD_PENDING_REASON_SECRET',
    createdAt: now,
    expiresAt: now + 60_000
  });

  const html = await fetch(`${runtime.base}/ui`).then(response => response.text());
  const token = managementToken(html);
  assert.ok(token);
  const scriptSrc = html.match(/src="(\/ui\/[^"]+\.js)"/)?.[1];
  if (scriptSrc) {
    const scriptResponse = await fetch(`${runtime.base}${scriptSrc}`);
    if (scriptResponse.status === 200) {
      const script = await scriptResponse.text();
      assert.doesNotMatch(script, new RegExp(token));
    }
  }
  assert.equal((await fetch(`${runtime.base}/admin/api/dashboard`)).status, 403);
  const response = await fetch(`${runtime.base}/admin/api/dashboard`, {
    headers: { 'x-ctmcp-admin-token': token }
  });
  assert.equal(response.status, 200);
  const dashboard = await response.json();
  const serialized = JSON.stringify(dashboard);
  for (const marker of [
    'DASHBOARD_OUTPUT_SECRET', 'DASHBOARD_ARG_SECRET', 'DASHBOARD_ENV_SECRET',
    'DASHBOARD_POST_CHECK_NAME_SECRET', 'DASHBOARD_POST_CHECK_OUTPUT_SECRET',
    'DASHBOARD_OPERATION_ID_SECRET', 'DASHBOARD_OPERATION_SUMMARY_SECRET',
    'DASHBOARD_PENDING_ID_SECRET', 'DASHBOARD_PENDING_ARG_SECRET', 'DASHBOARD_PENDING_REASON_SECRET'
  ]) assert.doesNotMatch(serialized, new RegExp(marker));
  assert.doesNotMatch(serialized, new RegExp(runtime.root.replace(/[.*+?^${}()|[\]\\]/g, '\\$&')));

  assert.equal(dashboard.runtime.version, AGENT_VERSION);
  assert.equal(dashboard.health.state, 'degraded');
  assert.equal(dashboard.permissions.pending, 1);
  assert.equal(dashboard.usage.recent.length, 100);
  assert.equal(dashboard.usage.windowSize, 120);
  assert.equal(dashboard.usage.persistent.enabled, true);
  assert.ok(dashboard.usage.persistent.matchedAsyncSessionEvents >= 1);
  assert.equal(typeof dashboard.usage.persistent.runtimeBootId, 'string');
  assert.equal(dashboard.activity.length, 1);
  assert.equal(dashboard.sessions.items.length, 1);
  const session = dashboard.sessions.items[0];
  assert.equal(session.workspaceId, 'repo');
  assert.equal(session.cwd, '.');
  assert.ok(session.stdoutBytes + session.stderrBytes > 0);
  for (const forbidden of ['command', 'operationId', 'fingerprint', 'stdout', 'stderr', 'postChecks']) {
    assert.equal(Object.hasOwn(session, forbidden), false, `session summary leaked ${forbidden}`);
  }
  assert.deepEqual(dashboard.admission.blocking, { limit: 4, active: 0, queued: 0 });
  assert.deepEqual(dashboard.admission.process, { limit: 4, active: 0, queued: 0 });
});

test('management health validators reject incomplete local metadata contracts', () => {
  assert.equal(validateManagementHealthPayload('/health', {
    ok: true,
    server: 'coding-tools-mcp-node',
    version: AGENT_VERSION,
    toolProfile: 'core'
  }).ok, true);
  assert.equal(validateManagementHealthPayload('/health', { ok: true }).ok, false);

  assert.equal(validateManagementHealthPayload('/mcp/info', {
    name: 'coding-tools-mcp-node',
    version: AGENT_VERSION,
    transport: 'streamable-http',
    supportedProtocolVersions: ['2025-11-25'],
    tools: ['server_info']
  }).ok, true);
  assert.equal(validateManagementHealthPayload('/mcp/info', {
    name: 'coding-tools-mcp-node',
    version: AGENT_VERSION,
    transport: 'streamable-http',
    supportedProtocolVersions: [],
    tools: []
  }).ok, false);

  assert.equal(validateManagementHealthPayload('/.well-known/oauth-authorization-server', {
    issuer: 'http://127.0.0.1:3789',
    authorization_endpoint: 'http://127.0.0.1:3789/oauth/authorize',
    token_endpoint: 'http://127.0.0.1:3789/oauth/token',
    response_types_supported: ['code'],
    grant_types_supported: ['authorization_code'],
    code_challenge_methods_supported: ['S256'],
    token_endpoint_auth_methods_supported: ['none']
  }).ok, true);
  assert.equal(validateManagementHealthPayload('/.well-known/oauth-authorization-server', {
    issuer: 'http://127.0.0.1:3789'
  }).ok, false);

  assert.equal(validateManagementHealthPayload('/.well-known/oauth-protected-resource/mcp', {
    resource: 'http://127.0.0.1:3789/mcp',
    authorization_servers: ['http://127.0.0.1:3789'],
    bearer_methods_supported: ['header'],
    scopes_supported: ['mcp']
  }).ok, true);
  assert.equal(validateManagementHealthPayload('/.well-known/oauth-protected-resource/mcp', {
    resource: 'http://127.0.0.1:3789/mcp'
  }).ok, false);
});

test('management observability routes expose sanitized telemetry, operation logs, history, health and diagnostics', async t => {
  const runtime = await startManagementServer(t);
  const pageHtml = await fetch(`${runtime.base}/ui`).then(response => response.text());
  const token = managementToken(pageHtml);
  assert.ok(token);
  const workspaceId = runtime.context.config.workspaceId ?? runtime.context.workspaceProfileId;
  const headers = { 'x-ctmcp-admin-token': token, origin: runtime.base };

  runtime.context.usageStore.enqueue({
    schema_version: 7,
    event: 'tool_call',
    workspace_id: workspaceId,
    runtime_boot_id: runtime.context.usageStore.runtimeBootId,
    server_version: runtime.context.usageStore.serverVersion,
    tool: 'exec_command',
    started_ts_ms: Date.now(),
    duration_ms: 42,
    outcome: 'success',
    outcome_class: 'success',
    is_error: false,
    response_json_bytes: 123,
    command_preview: 'TELEMETRY_COMMAND_SECRET',
    resolved_cwd: 'TELEMETRY_PATH_SECRET',
    session_id: 'TELEMETRY_SESSION_SECRET',
    argument_record: { password: 'TELEMETRY_ARGUMENT_SECRET' }
  });

  const telemetryResponse = await fetch(`${runtime.base}/admin/api/workspaces/${workspaceId}/telemetry?scope=current_runtime&limit=50`, { headers });
  assert.equal(telemetryResponse.status, 200);
  const telemetry = await telemetryResponse.json();
  assert.equal(telemetry.records.length, 1);
  assert.equal(telemetry.records[0].tool, 'exec_command');
  const telemetryText = JSON.stringify(telemetry);
  for (const marker of ['TELEMETRY_COMMAND_SECRET', 'TELEMETRY_PATH_SECRET', 'TELEMETRY_SESSION_SECRET', 'TELEMETRY_ARGUMENT_SECRET']) {
    assert.doesNotMatch(telemetryText, new RegExp(marker));
  }
  const operationWorkspaceId = await harnessWorkspaceId(runtime.root);
  const operationNow = Date.now();
  const operationReasonMarker = 'OP_LOG_REASON_MARKER';
  const operationPasswordMarker = 'OP_LOG_PASSWORD_MARKER';
  const operationCommandMarker = 'OP_LOG_COMMAND_MARKER';
  const operationTailMarker = 'OP_LOG_MULTILINE_TAIL';
  const operationOutsidePath = path.join(path.dirname(runtime.root), 'outside debug', 'secret.txt');
  const operationRecords = [
    {
      id: 'completed-operation-id', workspace_id: operationWorkspaceId, task_id: 'OPERATION_TASK_SECRET',
      tool: 'edit_file', kind: 'started', input_summary: { arguments_present: true }, result_summary: { ok: true },
      affected_files: [], created_at: String(operationNow - 4_000)
    },
    {
      id: 'completed-operation-id', workspace_id: operationWorkspaceId, task_id: 'OPERATION_TASK_SECRET',
      tool: 'edit_file', kind: 'completed', input_summary: { arguments_present: true },
      result_summary: { ok: true, affected_files: [{ path: path.join(runtime.root, 'private.txt') }] },
      reason: `${['to', 'ken'].join('')}=${operationReasonMarker} workspace=${runtime.root}`,
      affected_files: [], created_at: String(operationNow - 3_500)
    },
    {
      id: 'failed-operation-id', workspace_id: operationWorkspaceId,
      tool: 'exec_command', kind: 'started', input_summary: { arguments_present: true }, result_summary: { ok: true },
      affected_files: [], created_at: String(operationNow - 3_000)
    },
    {
      id: 'failed-operation-id', workspace_id: operationWorkspaceId,
      tool: 'exec_command', kind: 'completed', input_summary: { arguments_present: true },
      result_summary: {
        ok: true,
        transport_ok: true,
        execution_ok: true,
        command_ok: false,
        verification_ok: false,
        termination_reason: 'exited',
        process_exit_code: 7,
        process_timed_out: false,
        request_timed_out: false,
        retryable: true,
        error_code: 'COMMAND_FAILED',
        error_category: 'process',
        elapsed_ms: 1_000,
        first_output_ms: 50,
        stdout_bytes: 10,
        stderr_bytes: 20,
        blocking_queue_wait_ms: 5,
        workspace_admission_wait_ms: 6,
        warning_count: 2,
        command: 'OPERATION_COMMAND_SECRET',
        stdout: 'OPERATION_OUTPUT_SECRET'
      },
      reason: `${['pass', 'word'].join('')}=${operationPasswordMarker}; ${['com', 'mand'].join('')}=${operationCommandMarker}; path="${operationOutsidePath}"\n${operationTailMarker}`,
      affected_files: [], created_at: String(operationNow - 2_000)
    },
    {
      id: 'incomplete-operation-id', workspace_id: operationWorkspaceId,
      tool: 'git_status', kind: 'started', input_summary: { arguments_present: true }, result_summary: { ok: true },
      affected_files: [], created_at: String(operationNow - 1_000)
    },
    {
      id: 'incomplete-operation-id', workspace_id: operationWorkspaceId,
      tool: 'git_status', kind: 'completed', input_summary: { arguments_present: true },
      result_summary: { ok: true, status: 'running', command_ok: null },
      affected_files: [], created_at: String(operationNow - 900)
    }
  ];
  for (const record of operationRecords) await runtime.context.state.addOperation(operationWorkspaceId, record);

  const failedLogsResponse = await fetch(`${runtime.base}/admin/api/workspaces/${workspaceId}/logs?folderId=repo&status=failed&limit=1`, { headers });
  assert.equal(failedLogsResponse.status, 200);
  const failedLogs = await failedLogsResponse.json();
  assert.equal(failedLogs.source, 'operation_log');
  assert.deepEqual(failedLogs.summary, { total: 3, completed: 1, failed: 1, incomplete: 1 });
  assert.equal(failedLogs.matched, 1);
  assert.equal(failedLogs.operations[0].id, 'failed-operation-id');
  assert.equal(failedLogs.operations[0].status, 'failed');
  assert.equal(failedLogs.operations[0].durationMs, 1_000);
  assert.equal(failedLogs.operations[0].events.at(-1).kind, 'completed');
  assert.deepEqual(failedLogs.operations[0].diagnostics, {
    commandOk: false,
    transportOk: true,
    executionOk: true,
    verificationOk: false,
    errorCode: 'COMMAND_FAILED',
    errorCategory: 'process',
    retryable: true,
    runtimeStatus: null,
    terminationReason: 'exited',
    executionLane: null,
    outcomeClass: null,
    exitCode: 7,
    processTimedOut: false,
    requestTimedOut: false,
    recoverable: null,
    truncated: null,
    stdoutTruncated: null,
    stderrTruncated: null,
    cursorExpired: null,
    postChecksPending: null,
    detached: null,
    deduplicated: null,
    elapsedMs: 1_000,
    actualWaitMs: null,
    firstOutputMs: 50,
    stdoutBytes: 10,
    stderrBytes: 20,
    warningCount: 2,
    waitMs: {
      blocking: 5,
      workspaceAdmission: 6,
      globalAdmission: null,
      admissionQueue: null,
      workspaceLock: null,
      operationLock: null,
      resourceLock: null,
      historyLock: null,
      sessionRegistry: null
    }
  });
  assert.match(failedLogs.operations[0].reason, /\[REDACTED\]/);
  assert.match(failedLogs.operations[0].reason, /\[PATH\]/);

  const logsResponse = await fetch(`${runtime.base}/admin/api/workspaces/${workspaceId}/logs?folderId=repo&limit=2`, { headers });
  assert.equal(logsResponse.status, 200);
  const logs = await logsResponse.json();
  assert.equal(logs.operations.length, 2);
  assert.equal(logs.operations[0].status, 'incomplete');
  assert.equal(logs.operations[0].events.at(-1).kind, 'completed');
  assert.equal(logs.operations[0].diagnostics.runtimeStatus, 'running');
  assert.equal(logs.nextCursor, 2);
  const olderLogsResponse = await fetch(`${runtime.base}/admin/api/workspaces/${workspaceId}/logs?folderId=repo&cursor=2&limit=2`, { headers });
  assert.equal(olderLogsResponse.status, 200);
  const olderLogs = await olderLogsResponse.json();
  assert.equal(olderLogs.operations[0].id, 'completed-operation-id');
  assert.equal(olderLogs.operations[0].taskTracked, true);
  assert.equal(olderLogs.operations[0].affectedFileCount, 1);
  assert.equal(olderLogs.nextCursor, null);
  const operationLogText = JSON.stringify({ failedLogs, logs, olderLogs });
  for (const marker of [
    operationReasonMarker, operationPasswordMarker, operationCommandMarker, operationTailMarker,
    operationOutsidePath, 'OPERATION_COMMAND_SECRET', 'OPERATION_OUTPUT_SECRET', 'OPERATION_TASK_SECRET', operationWorkspaceId
  ]) assert.doesNotMatch(operationLogText, new RegExp(marker));
  assert.doesNotMatch(operationLogText, new RegExp(runtime.root.replace(/[.*+?^${}()|[\]\\]/g, '\\$&')));

  const invalidLogFilter = await fetch(`${runtime.base}/admin/api/workspaces/${workspaceId}/logs?folderId=repo&status=unknown`, { headers });
  assert.equal(invalidLogFilter.status, 400);
  const historyDir = path.join(runtime.root, 'docs', 'history-session');
  await mkdir(historyDir, { recursive: true });
  const historyContent = renderDocument(1, 'Management UI archive', 'HISTORY_SESSION_KEY_SECRET', 'unix:1', 'unix:2', 'completed', [{
    turn_id: 'ui-observability',
    timestamp: 'unix:2',
    user_intent: 'Expose archive records',
    findings: ['History is read-only'],
    decisions: ['Do not add live activity badges'],
    files_changed: ['docs/example.md'],
    tests: ['management route test'],
    runtime_state: ['healthy'],
    remaining_issues: [],
    next_actions: ['ship'],
    notes: 'archive note'
  }]);
  await writeFile(path.join(historyDir, '1.md'), historyContent);

  const historyResponse = await fetch(`${runtime.base}/admin/api/workspaces/${workspaceId}/history?folderId=repo`, { headers });
  assert.equal(historyResponse.status, 200);
  const history = await historyResponse.json();
  assert.equal(history.sessions.length, 1);
  assert.equal(history.sessions[0].checkpointCount, 1);
  assert.doesNotMatch(JSON.stringify(history), /HISTORY_SESSION_KEY_SECRET/);

  const detailResponse = await fetch(`${runtime.base}/admin/api/workspaces/${workspaceId}/history/1?folderId=repo`, { headers });
  assert.equal(detailResponse.status, 200);
  const detail = await detailResponse.json();
  assert.equal(detail.records[0].turnId, 'ui-observability');
  assert.equal(detail.records[0].decisions[0], 'Do not add live activity badges');
  assert.doesNotMatch(detail.content, /HISTORY_SESSION_KEY_SECRET/);
  assert.match(detail.content, /\*\*Session key:\*\* \[REDACTED\]/);

  const externalHistory = await mkdtemp(path.join(tmpdir(), 'ctmcp-management-history-outside-'));
  t.after(() => rm(externalHistory, { recursive: true, force: true }));
  await writeFile(path.join(externalHistory, '1.md'), historyContent.replace('archive note', 'OUTSIDE_HISTORY_SECRET'));
  await rm(historyDir, { recursive: true, force: true });
  await symlink(externalHistory, historyDir, process.platform === 'win32' ? 'junction' : 'dir');
  const escapedHistoryResponse = await fetch(`${runtime.base}/admin/api/workspaces/${workspaceId}/history?folderId=repo`, { headers });
  assert.equal(escapedHistoryResponse.status, 403);
  assert.equal((await escapedHistoryResponse.json()).error.code, 'HISTORY_PATH_OUTSIDE_WORKSPACE');

  const healthResponse = await rawJsonRequest(`${runtime.base}/admin/api/workspaces/${workspaceId}/health`, {
    method: 'POST',
    headers: {
      ...headers,
      host: 'localhost:9',
      origin: 'http://localhost:9'
    }
  });
  assert.equal(healthResponse.status, 200);
  const health = healthResponse.json;
  assert.equal(health.ok, true);
  assert.equal(health.items.some(item => item.id === '/health' && item.ok), true);
  assert.equal(health.items.some(item => item.id === '/mcp/info' && item.ok), true);
  assert.equal(health.items.some(item => item.id === '/.well-known/oauth-authorization-server' && item.ok), true);
  assert.equal(health.items.some(item => item.id === '/.well-known/oauth-protected-resource/mcp' && item.ok), true);
  const mcpChallenge = health.items.find(item => item.id === '/mcp');
  assert.equal(mcpChallenge?.ok, true);
  assert.equal(mcpChallenge?.status, 401);
  assert.match(String(mcpChallenge?.detail), /OAuth protected-resource challenge/);
  assert.doesNotMatch(JSON.stringify(health), /localhost:9/);

  const diagnosticsResponse = await fetch(`${runtime.base}/admin/api/workspaces/${workspaceId}/diagnostics`, { headers });
  assert.equal(diagnosticsResponse.status, 200);
  const diagnosticsText = JSON.stringify(await diagnosticsResponse.json());
  assert.doesNotMatch(diagnosticsText, new RegExp(runtime.root.replace(/[.*+?^${}()|[\]\\]/g, '\\$&')));
  for (const marker of [
    'management-test-password', 'management-test-client-secret', 'management-test-token-secret',
    'TELEMETRY_COMMAND_SECRET', 'TELEMETRY_PATH_SECRET', 'TELEMETRY_SESSION_SECRET', 'TELEMETRY_ARGUMENT_SECRET',
    'HISTORY_SESSION_KEY_SECRET'
  ]) assert.doesNotMatch(diagnosticsText, new RegExp(marker));
});

test('management API atomically saves restart configuration while preserving omitted secrets', async t => {
  const runtime = await startManagementServer(t);
  const pageHtml = await fetch(`${runtime.base}/ui`).then(response => response.text());
  const token = managementToken(pageHtml);
  assert.ok(token);

  const payload = editable(runtime.loaded.config, {
    port: 4791,
    securityPolicy: { ...runtime.loaded.config.securityPolicy, blockNetworkCommands: true },
    folders: [
      { id: 'repo', name: 'Primary', path: path.join(runtime.root, 'nested', '..') },
      { id: 'second', name: 'Second', path: runtime.dataDir }
    ],
    policy: {
      allowedCommands: ['git', 'node', 'npm'],
      workspaceLocalEntries: false,
      workspaceScriptExtensions: ['.cmd', '.ps1', '.sh'],
      maxPatchBytes: 2 * 1024 * 1024
    },
    limits: {
      ...runtime.loaded.config.limits,
      processConcurrency: 7,
      globalBlockingConcurrency: 33,
      globalProcessConcurrency: 17
    }
  });
  const response = await fetch(`${runtime.base}/admin/api/config`, {
    method: 'PUT',
    headers: {
      'x-ctmcp-admin-token': token,
      'content-type': 'application/json',
      origin: runtime.base
    },
    body: JSON.stringify(payload)
  });
  assert.equal(response.status, 200);
  const result = await response.json();
  assert.equal(result.restartRequired, true);
  assert.equal(result.saved.port, 4791);
  assert.equal(result.saved.permissionMode, 'guarded');
  assert.equal(result.saved.folders.length, 2);

  const saved = JSON.parse(await readFile(runtime.configPath, 'utf8'));
  assert.equal(saved.schemaVersion, 2);
  assert.equal(saved.bind.port, 4791);
  assert.equal(saved.auth.oauthClientId, 'chatgpt');
  assert.equal(saved.oauth, undefined);
  assert.equal(saved.limits.processConcurrency, 7);
  assert.equal(saved.limits.globalBlockingConcurrency, 33);
  assert.equal(saved.limits.globalProcessConcurrency, 17);
  assert.deepEqual(saved.policy.allowedCommands, ['git', 'node', 'npm']);
  assert.equal(saved.policy.workspaceLocalEntries, false);
  assert.deepEqual(saved.policy.workspaceScriptExtensions, ['.cmd', '.ps1', '.sh']);
  assert.equal(saved.policy.maxPatchBytes, 2 * 1024 * 1024);
  assert.equal(saved.folders[0].path, await realpath(runtime.root));
  const publicText = JSON.stringify(saved);
  assert.doesNotMatch(publicText, /management-test-password|management-test-client-secret|management-test-token-secret/);
  const encrypted = await readFile(runtime.loaded.secretStorePath, 'utf8');
  assert.doesNotMatch(encrypted, /management-test-password|management-test-client-secret|management-test-token-secret/);
  const restarted = await loadConfigBundle(runtime.configPath);
  assert.equal(restarted.config.oauth.password, 'management-test-password');
  assert.equal(restarted.config.oauth.clientSecret, 'management-test-client-secret');
  assert.equal(restarted.config.oauth.tokenSecret, 'management-test-token-secret-that-is-long-enough');
  assert.equal(restarted.config.port, 4791);
  assert.equal(restarted.config.folders[0].path, await realpath(runtime.root));
  assert.equal(runtime.loaded.config.port, 0, 'running configuration must not be hot-mutated');
});

test('management rejects duplicate canonical workspace roots before restart', async t => {
  const runtime = await startManagementServer(t);
  const pageHtml = await fetch(`${runtime.base}/ui`).then(response => response.text());
  const token = managementToken(pageHtml);
  assert.ok(token);
  const payload = editable(runtime.loaded.config, {
    folders: [
      { id: 'primary', name: 'Primary', path: runtime.root },
      { id: 'alias', name: 'Alias', path: path.join(runtime.root, '.') }
    ]
  });
  const response = await fetch(`${runtime.base}/admin/api/config`, {
    method: 'PUT',
    headers: {
      'x-ctmcp-admin-token': token,
      'content-type': 'application/json',
      origin: runtime.base
    },
    body: JSON.stringify(payload)
  });
  assert.equal(response.status, 400);
  const result = await response.json();
  assert.equal(result.error.code, 'CONFIG_INVALID');
  assert.match(result.error.message, /same physical workspace root/i);
});

test('saving the effective configuration unchanged does not require a restart', async () => {
  const root = await mkdtemp(path.join(tmpdir(), 'ctmcp-management-same-root-'));
  const dataDir = await mkdtemp(path.join(tmpdir(), 'ctmcp-management-same-data-'));
  const configPath = path.join(dataDir, 'agent.json');
  await writeFile(configPath, `${JSON.stringify(document(root, dataDir), null, 2)}\n`);
  const loaded = await loadConfigBundle(configPath);
  const store = new ConfigStore(loaded);
  assert.equal(store.snapshot().restartRequired, false);
  const result = await store.save(editable(loaded.config));
  assert.equal(result.restartRequired, false);
  assert.equal(result.warning, null);
});

test('management keeps saved values separate while writing secrets to the effective environment data directory', async () => {
  const root = await mkdtemp(path.join(tmpdir(), 'ctmcp-management-env-root-'));
  const savedDataDir = await mkdtemp(path.join(tmpdir(), 'ctmcp-management-env-saved-'));
  const effectiveDataDir = await mkdtemp(path.join(tmpdir(), 'ctmcp-management-env-effective-'));
  const configPath = path.join(savedDataDir, 'agent.json');
  await writeFile(configPath, `${JSON.stringify(document(root, savedDataDir), null, 2)}\n`);
  const previous = process.env.CTMCP_DATA_DIR;
  process.env.CTMCP_DATA_DIR = effectiveDataDir;
  try {
    const loaded = await loadConfigBundle(configPath);
    const store = new ConfigStore(loaded);
    const snapshot = store.snapshot();
    assert.equal(snapshot.effective.dataDir, effectiveDataDir);
    assert.equal(snapshot.saved.dataDir, savedDataDir);
    const result = await store.save(editable(loaded.config, {
      dataDir: savedDataDir,
      oauth: {
        clientId: loaded.config.oauth.clientId,
        password: 'environment-data-password',
        clientSecret: '',
        clearClientSecret: false
      }
    }));
    assert.equal(result.secretStorePath, path.join(effectiveDataDir, 'agent-secrets.enc.json'));
    const effectiveSecrets = await readAgentSecrets(effectiveDataDir);
    assert.equal(effectiveSecrets.secrets.oauthPassword, 'environment-data-password');
    assert.equal(JSON.parse(await readFile(configPath, 'utf8')).host.node.dataDir, savedDataDir);
  } finally {
    if (previous === undefined) delete process.env.CTMCP_DATA_DIR;
    else process.env.CTMCP_DATA_DIR = previous;
  }
});

test('management save rolls back encrypted secrets when the public config write fails', async () => {
  const root = await mkdtemp(path.join(tmpdir(), 'ctmcp-management-rollback-root-'));
  const dataDir = await mkdtemp(path.join(tmpdir(), 'ctmcp-management-rollback-data-'));
  const configPath = path.join(dataDir, 'agent.json');
  await writeFile(configPath, `${JSON.stringify(document(root, dataDir), null, 2)}\n`);
  const loaded = await loadConfigBundle(configPath);
  const store = new ConfigStore(loaded);
  const beforeStore = await readFile(loaded.secretStorePath, 'utf8');
  const beforeKey = await readFile(loaded.secretKeyPath, 'utf8');

  await rm(configPath);
  await mkdir(configPath);
  await assert.rejects(store.save(editable(loaded.config, {
    oauth: {
      clientId: loaded.config.oauth.clientId,
      password: 'replacement-password',
      clientSecret: '',
      clearClientSecret: false
    }
  })));

  assert.equal(await readFile(loaded.secretStorePath, 'utf8'), beforeStore);
  assert.equal(await readFile(loaded.secretKeyPath, 'utf8'), beforeKey);
  const restored = await readAgentSecrets(dataDir);
  assert.equal(restored.secrets.oauthPassword, 'management-test-password');
  assert.equal(restored.secrets.oauthClientSecret, 'management-test-client-secret');
  assert.equal(restored.secrets.oauthTokenSecret, 'management-test-token-secret-that-is-long-enough');
});

test('management save copies encrypted secrets when the data directory changes', async () => {
  const root = await mkdtemp(path.join(tmpdir(), 'ctmcp-management-move-root-'));
  const dataDir = await mkdtemp(path.join(tmpdir(), 'ctmcp-management-move-data-'));
  const nextDataDir = await mkdtemp(path.join(tmpdir(), 'ctmcp-management-move-next-'));
  const configPath = path.join(dataDir, 'agent.json');
  await writeFile(configPath, `${JSON.stringify(document(root, dataDir), null, 2)}\n`);
  const loaded = await loadConfigBundle(configPath);
  const store = new ConfigStore(loaded);
  const result = await store.save(editable(loaded.config, { dataDir: nextDataDir }));
  assert.equal(result.restartRequired, true);
  assert.equal(result.secretStorePath, path.join(nextDataDir, 'agent-secrets.enc.json'));
  await access(result.secretStorePath);
  await access(path.join(nextDataDir, 'agent-secrets.key'));
  const saved = JSON.parse(await readFile(configPath, 'utf8'));
  assert.equal(saved.host.node.dataDir, nextDataDir);
  const restarted = await loadConfigBundle(configPath);
  assert.equal(restarted.config.oauth.password, 'management-test-password');
  assert.equal(restarted.config.oauth.clientSecret, 'management-test-client-secret');
  assert.equal(restarted.config.oauth.tokenSecret, 'management-test-token-secret-that-is-long-enough');
});

test('management derives compatibility permission and tool profiles from security policy', async () => {
  const root = await mkdtemp(path.join(tmpdir(), 'ctmcp-management-profile-root-'));
  const dataDir = await mkdtemp(path.join(tmpdir(), 'ctmcp-management-profile-data-'));
  const configPath = path.join(dataDir, 'agent.json');
  await writeFile(configPath, `${JSON.stringify(document(root, dataDir), null, 2)}\n`);
  const loaded = await loadConfigBundle(configPath);
  const store = new ConfigStore(loaded);

  const guardedPolicy = { ...loaded.config.securityPolicy, blockNetworkCommands: true };
  const result = await store.save(editable(loaded.config, {
    permissionMode: 'dangerous',
    toolProfile: 'not-a-profile',
    securityPolicy: guardedPolicy
  }));
  assert.equal(result.restartRequired, true);
  assert.equal(result.saved.permissionMode, 'guarded');
  assert.equal(result.saved.toolProfile, 'trusted-core');
  assert.equal(result.saved.activeToolProfile, 'trusted-core');
  assert.deepEqual(result.saved.securityPolicy, guardedPolicy);
  const persisted = JSON.parse(await readFile(configPath, 'utf8'));
  assert.equal(persisted.permissionMode, 'guarded');
  assert.equal(persisted.toolProfile, 'trusted-core');
  assert.deepEqual(persisted.securityPolicy, guardedPolicy);
});

test('management Skill inventory hot-applies individual and master Skill toggles', async t => {
  const runtime = await startManagementServer(t);
  const skillRoot = path.join(runtime.root, 'skills', 'ui-toggle');
  await mkdir(skillRoot, { recursive: true });
  await writeFile(path.join(skillRoot, 'SKILL.md'), [
    '---',
    'name: ui-toggle',
    'description: Toggle this Skill from the management UI.',
    '---',
    '',
    '# UI toggle'
  ].join('\n'));

  const page = await fetch(`${runtime.base}/ui`);
  const token = managementToken(await page.text());
  assert.ok(token);
  const workspaceId = runtime.context.config.workspaceId ?? runtime.context.workspaceProfileId;
  const endpoint = `${runtime.base}/admin/api/workspaces/${encodeURIComponent(workspaceId)}/skills`;
  const listed = await fetch(endpoint, { headers: { 'x-ctmcp-admin-token': token } });
  assert.equal(listed.status, 200);
  const inventory = await listed.json();
  assert.equal(inventory.active, true);
  const skill = inventory.skills.find(item => item.name === 'ui-toggle');
  assert.ok(skill);
  assert.equal(skill.selected, true);
  assert.equal(skill.enabled, true);
  assert.equal(skill.scope, 'workspace');

  const disabledResponse = await fetch(endpoint, {
    method: 'PUT',
    headers: {
      'x-ctmcp-admin-token': token,
      'content-type': 'application/json',
      origin: runtime.base
    },
    body: JSON.stringify({ key: skill.key, enabled: false })
  });
  assert.equal(disabledResponse.status, 200);
  const disabled = await disabledResponse.json();
  assert.equal(disabled.enabled, false);
  assert.ok(disabled.appliedImmediately.includes('skills'));

  const folderRuntime = runtime.context.folderRuntimes.get('repo');
  assert.ok(folderRuntime);
  const disabledSnapshot = await folderRuntime.skillRegistry.snapshot();
  assert.equal(disabledSnapshot.skills.some(item => item.name === 'ui-toggle'), false);
  const persisted = JSON.parse(await readFile(runtime.configPath, 'utf8'));
  assert.ok(persisted.skills.disabled.includes(skill.key));

  const listedDisabled = await fetch(endpoint, { headers: { 'x-ctmcp-admin-token': token } }).then(response => response.json());
  const disabledSkill = listedDisabled.skills.find(item => item.key === skill.key);
  assert.equal(disabledSkill.selected, false);
  assert.equal(disabledSkill.enabled, false);

  const enabledResponse = await fetch(endpoint, {
    method: 'PUT',
    headers: {
      'x-ctmcp-admin-token': token,
      'content-type': 'application/json',
      origin: runtime.base
    },
    body: JSON.stringify({ key: skill.key, enabled: true })
  });
  assert.equal(enabledResponse.status, 200);
  const enabled = await enabledResponse.json();
  assert.equal(enabled.enabled, true);
  assert.ok(enabled.appliedImmediately.includes('skills'));
  assert.equal((await folderRuntime.skillRegistry.snapshot()).skills.some(item => item.name === 'ui-toggle'), true);

  const masterOffResponse = await fetch(endpoint, {
    method: 'PUT',
    headers: {
      'x-ctmcp-admin-token': token,
      'content-type': 'application/json',
      origin: runtime.base
    },
    body: JSON.stringify({ active: false })
  });
  assert.equal(masterOffResponse.status, 200);
  assert.equal((await masterOffResponse.json()).active, false);
  const masterOffInventory = await fetch(endpoint, { headers: { 'x-ctmcp-admin-token': token } }).then(response => response.json());
  assert.equal(masterOffInventory.active, false);
  const masterOffSkill = masterOffInventory.skills.find(item => item.key === skill.key);
  assert.equal(masterOffSkill.selected, true);
  assert.equal(masterOffSkill.enabled, false);
  assert.equal((await folderRuntime.skillRegistry.snapshot()).skills.length, 0);
  const persistedMasterOff = JSON.parse(await readFile(runtime.configPath, 'utf8'));
  assert.equal(persistedMasterOff.skills.active, false);
  assert.equal(persistedMasterOff.skills.disabled.includes(skill.key), false);

  const masterOnResponse = await fetch(endpoint, {
    method: 'PUT',
    headers: {
      'x-ctmcp-admin-token': token,
      'content-type': 'application/json',
      origin: runtime.base
    },
    body: JSON.stringify({ active: true })
  });
  assert.equal(masterOnResponse.status, 200);
  assert.equal((await masterOnResponse.json()).active, true);
  assert.equal((await folderRuntime.skillRegistry.snapshot()).skills.some(item => item.name === 'ui-toggle'), true);
});
test('management extension inventory hot-applies individual and master Hook/MCP toggles safely', async t => {
  const runtime = await startManagementServer(t);
  await mkdir(path.join(runtime.root, '.claude'), { recursive: true });
  await writeFile(path.join(runtime.root, '.claude', 'settings.json'), JSON.stringify({
    hooks: {
      PreToolUse: [{
        matcher: 'read_file',
        hooks: [{ type: 'command', command: process.execPath, args: ['-e', 'process.exit(0)'] }]
      }]
    }
  }));
  await writeFile(path.join(runtime.root, '.mcp.json'), JSON.stringify({
    mcpServers: {
      inventoryFixture: { type: 'stdio', command: process.execPath, args: ['-e', 'process.exit(0)'] }
    }
  }));

  const page = await fetch(`${runtime.base}/ui`);
  const token = managementToken(await page.text());
  assert.ok(token);
  const workspaceId = runtime.context.config.workspaceId ?? runtime.context.workspaceProfileId;
  const endpoint = `${runtime.base}/admin/api/workspaces/${encodeURIComponent(workspaceId)}/extensions`;
  const listed = await fetch(endpoint, { headers: { 'x-ctmcp-admin-token': token } });
  assert.equal(listed.status, 200);
  const inventory = await listed.json();
  assert.equal(inventory.hooksActive, true);
  assert.equal(inventory.mcpActive, true);
  const hook = inventory.hooks.find(item => item.sourcePath === '.claude/settings.json' && item.event === 'PreToolUse');
  assert.ok(hook);
  assert.equal(hook.selected, false);
  assert.equal(hook.enabled, false);
  assert.equal(hook.supported, true);
  const mcp = inventory.mcpServers.find(item => item.name === 'inventoryFixture');
  assert.ok(mcp);
  assert.equal(mcp.selected, false);
  assert.equal(mcp.enabled, false);
  assert.equal(mcp.transport, 'stdio');
  assert.equal(Object.hasOwn(mcp, 'env'), false);
  assert.equal(Object.hasOwn(mcp, 'headers'), false);

  const enabledResponse = await fetch(endpoint, {
    method: 'PUT',
    headers: {
      'x-ctmcp-admin-token': token,
      'content-type': 'application/json',
      origin: runtime.base
    },
    body: JSON.stringify({ kind: 'hook', key: hook.key, enabled: true })
  });
  assert.equal(enabledResponse.status, 200);
  const enabled = await enabledResponse.json();
  assert.equal(enabled.enabled, true);
  assert.ok(enabled.appliedImmediately.includes('extensions'));
  const persistedEnabled = JSON.parse(await readFile(runtime.configPath, 'utf8'));
  assert.ok(persistedEnabled.extensions.hooks.enabled.includes(hook.key));
  assert.ok(runtime.context.config.extensions.hooks.enabled.includes(hook.key));

  const listedEnabled = await fetch(endpoint, { headers: { 'x-ctmcp-admin-token': token } }).then(response => response.json());
  const selectedEnabledHook = listedEnabled.hooks.find(item => item.key === hook.key);
  assert.equal(selectedEnabledHook.selected, true);
  assert.equal(selectedEnabledHook.enabled, true);

  const hookMasterOffResponse = await fetch(endpoint, {
    method: 'PUT',
    headers: {
      'x-ctmcp-admin-token': token,
      'content-type': 'application/json',
      origin: runtime.base
    },
    body: JSON.stringify({ kind: 'hook', active: false })
  });
  assert.equal(hookMasterOffResponse.status, 200);
  assert.equal((await hookMasterOffResponse.json()).active, false);
  const hookMasterOffInventory = await fetch(endpoint, { headers: { 'x-ctmcp-admin-token': token } }).then(response => response.json());
  assert.equal(hookMasterOffInventory.hooksActive, false);
  const selectedHook = hookMasterOffInventory.hooks.find(item => item.key === hook.key);
  assert.equal(selectedHook.selected, true);
  assert.equal(selectedHook.enabled, false);
  const persistedHookMasterOff = JSON.parse(await readFile(runtime.configPath, 'utf8'));
  assert.equal(persistedHookMasterOff.extensions.hooks.active, false);
  assert.ok(persistedHookMasterOff.extensions.hooks.enabled.includes(hook.key));

  const hookMasterOnResponse = await fetch(endpoint, {
    method: 'PUT',
    headers: {
      'x-ctmcp-admin-token': token,
      'content-type': 'application/json',
      origin: runtime.base
    },
    body: JSON.stringify({ kind: 'hook', active: true })
  });
  assert.equal(hookMasterOnResponse.status, 200);
  assert.equal((await hookMasterOnResponse.json()).active, true);
  const restoredHookInventory = await fetch(endpoint, { headers: { 'x-ctmcp-admin-token': token } }).then(response => response.json());
  assert.equal(restoredHookInventory.hooks.find(item => item.key === hook.key).enabled, true);

  const mcpMasterOffResponse = await fetch(endpoint, {
    method: 'PUT',
    headers: {
      'x-ctmcp-admin-token': token,
      'content-type': 'application/json',
      origin: runtime.base
    },
    body: JSON.stringify({ kind: 'mcp', active: false })
  });
  assert.equal(mcpMasterOffResponse.status, 200);
  assert.equal((await mcpMasterOffResponse.json()).active, false);
  const mcpMasterOffInventory = await fetch(endpoint, { headers: { 'x-ctmcp-admin-token': token } }).then(response => response.json());
  assert.equal(mcpMasterOffInventory.mcpActive, false);
  assert.equal(JSON.parse(await readFile(runtime.configPath, 'utf8')).extensions.mcp.active, false);

  const mcpMasterOnResponse = await fetch(endpoint, {
    method: 'PUT',
    headers: {
      'x-ctmcp-admin-token': token,
      'content-type': 'application/json',
      origin: runtime.base
    },
    body: JSON.stringify({ kind: 'mcp', active: true })
  });
  assert.equal(mcpMasterOnResponse.status, 200);
  assert.equal((await mcpMasterOnResponse.json()).active, true);

  const disabledResponse = await fetch(endpoint, {
    method: 'PUT',
    headers: {
      'x-ctmcp-admin-token': token,
      'content-type': 'application/json',
      origin: runtime.base
    },
    body: JSON.stringify({ kind: 'hook', key: hook.key, enabled: false })
  });
  assert.equal(disabledResponse.status, 200);
  assert.equal((await disabledResponse.json()).enabled, false);
  const persistedDisabled = JSON.parse(await readFile(runtime.configPath, 'utf8'));
  assert.equal(persistedDisabled.extensions.hooks.enabled.includes(hook.key), false);
});

test('management UI can be disabled without disabling the headless server', async t => {
  const root = await mkdtemp(path.join(tmpdir(), 'ctmcp-no-ui-root-'));
  const dataDir = await mkdtemp(path.join(tmpdir(), 'ctmcp-no-ui-data-'));
  const configPath = path.join(dataDir, 'agent.json');
  const input = document(root, dataDir);
  input.management.enabled = false;
  await writeFile(configPath, JSON.stringify(input));
  const loaded = await loadConfigBundle(configPath);
  loaded.config.port = 0;
  const server = await createAgentServer(loaded.config, { configStore: new ConfigStore(loaded) });
  await new Promise(resolve => server.listen(0, '127.0.0.1', resolve));
  t.after(() => new Promise(resolve => server.close(resolve)));
  const address = server.address();
  assert.ok(address && typeof address === 'object');
  const base = `http://127.0.0.1:${address.port}`;
  assert.equal((await fetch(`${base}/ui`)).status, 404);
  const health = await fetch(`${base}/health`).then(response => response.json());
  assert.equal(health.ok, true);
  assert.equal(health.headless, true);
  assert.equal(health.management.enabled, false);
});
