import test from 'node:test';
import assert from 'node:assert/strict';
import { mkdir, mkdtemp, readFile, realpath, stat, writeFile } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import path from 'node:path';
import { ApplicationConfigStore, loadApplication } from '../dist/application.js';
import { loadConfigBundle } from '../dist/config.js';
import { createAgentRuntime } from '../dist/server.js';

// Workspace fixtures must never inherit and mutate a developer's live Agent data directory.
delete process.env.CTMCP_DATA_DIR;

function configDocument(root, dataDir, port, overrides = {}) {
  return {
    schema_version: 1,
    host: '127.0.0.1',
    port,
    dataDir,
    permissionMode: 'trusted',
    toolProfile: 'core',
    management: { enabled: true },
    oauth: {},
    folders: [{ id: 'repo', name: 'Repo', path: root }],
    limits: {
      blockingConcurrency: 2,
      processConcurrency: 2,
      activeSessionLimit: 8,
      maxOutputBytes: 1024 * 1024
    },
    ...overrides
  };
}

async function fixture(prefix) {
  const base = await mkdtemp(path.join(tmpdir(), `${prefix}-`));
  const root = path.join(base, 'root');
  const dataDir = path.join(base, 'data');
  const configPath = path.join(dataDir, 'agent.json');
  await import('node:fs/promises').then(({ mkdir }) => Promise.all([
    mkdir(root, { recursive: true }),
    mkdir(dataDir, { recursive: true })
  ]));
  return { base, root, dataDir, configPath };
}

let managementPortSequence = 0;

async function listen(server) {
  let lastError;
  for (let attempt = 0; attempt < 32; attempt += 1) {
    const port = 44_000 + ((process.pid + managementPortSequence++) % 4_000);
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
  throw lastError ?? new Error('unable to allocate management test port');
}

function managementToken(html) {
  return html.match(/name="ctmcp-admin-token" content="([A-Za-z0-9_-]+)"/)?.[1];
}

function updatePayload(workspace, overrides = {}) {
  const saved = workspace.saved;
  return {
    name: workspace.name,
    host: saved.host,
    port: saved.port,
    publicBaseUrl: saved.publicBaseUrl,
    dataDir: saved.dataDir,
    permissionMode: saved.permissionMode,
    toolProfile: saved.toolProfile,
    management: saved.management,
    oauth: {
      clientId: saved.oauth.clientId,
      password: '',
      clientSecret: '',
      clearClientSecret: false
    },
    folders: saved.folders,
    limits: saved.limits,
    tunnel: {
      enabled: saved.tunnel.enabled,
      publicUrl: saved.tunnel.publicUrl,
      enrollmentUrl: '',
      clearEnrollmentUrl: false
    },
    ...overrides
  };
}

test('application initialization persists a Rust-style random OAuth client ID and authorization password', async () => {
  const { root, dataDir, configPath } = await fixture('ctmcp-application-init');
  await writeFile(configPath, `${JSON.stringify(configDocument(root, dataDir, 43121), null, 2)}\n`);

  const first = await loadApplication(configPath);
  assert.equal(first.registryCreated, true);
  assert.equal(first.workspaces.length, 1);
  assert.match(first.workspaces[0].id, /^[0-9a-f]{32}$/);
  assert.equal(first.workspaces[0].name, 'Repo');
  assert.match(first.workspaces[0].loaded.config.oauth.clientId, /^chatgpt-client-[0-9a-f-]{12}$/);
  assert.match(first.workspaces[0].loaded.config.oauth.password, /^[A-Za-z0-9_-]{32}$/);
  assert.notEqual(first.workspaces[0].loaded.config.oauth.password, 'change-me');

  const saved = JSON.parse(await readFile(configPath, 'utf8'));
  assert.equal(saved.schemaVersion, 2);
  assert.equal(saved.auth.oauthClientId, first.workspaces[0].loaded.config.oauth.clientId);
  assert.equal(saved.auth.password, undefined);
  assert.equal(saved.oauth, undefined);
  const registry = JSON.parse(await readFile(first.registryPath, 'utf8'));
  assert.equal(registry.workspaces[0].id, first.workspaces[0].id);

  const second = await loadApplication(configPath);
  assert.equal(second.registryCreated, false);
  assert.equal(second.workspaces[0].id, first.workspaces[0].id);
  assert.equal(second.workspaces[0].loaded.config.oauth.clientId, first.workspaces[0].loaded.config.oauth.clientId);
  assert.equal(second.workspaces[0].loaded.config.oauth.password, first.workspaces[0].loaded.config.oauth.password);
});

test('application startup removes missing workspace folders and persists the repaired config', async () => {
  const { base, root, dataDir, configPath } = await fixture('ctmcp-application-stale-folder');
  const validRoot = path.join(base, 'valid-root');
  const missingRoot = path.join(base, 'missing-root');
  await mkdir(validRoot, { recursive: true });
  await writeFile(configPath, `${JSON.stringify(configDocument(root, dataDir, 43125, {
    folders: [
      { id: 'valid', name: 'Valid', path: validRoot },
      { id: 'missing', name: 'Missing', path: missingRoot }
    ]
  }), null, 2)}\n`);

  const application = await loadApplication(configPath);
  assert.deepEqual(application.workspaces[0].loaded.config.folders.map(folder => folder.id), ['valid']);
  assert.equal(application.workspaces[0].loaded.config.folders[0].path, await realpath(validRoot));

  const saved = JSON.parse(await readFile(configPath, 'utf8'));
  assert.deepEqual(saved.folders.map(folder => folder.id), ['valid']);
  assert.equal(saved.folders[0].path, await realpath(validRoot));
  assert.equal(saved.folders.some(folder => folder.path === missingRoot), false);
});

test('application startup creates and persists a safe fallback when every workspace folder is missing', async () => {
  const { base, dataDir, configPath } = await fixture('ctmcp-application-all-stale-folders');
  const missingRoot = path.join(base, 'missing-root');
  await writeFile(configPath, `${JSON.stringify(configDocument(missingRoot, dataDir, 43126), null, 2)}\n`);

  const application = await loadApplication(configPath);
  const fallbackRoot = path.join(dataDir, 'workspace');
  const fallbackCanonical = await realpath(fallbackRoot);
  const metadata = await stat(fallbackRoot);
  assert.equal(metadata.isDirectory(), true);
  assert.equal(application.workspaces[0].loaded.config.folders.length, 1);
  assert.equal(application.workspaces[0].loaded.config.folders[0].id, 'default');
  assert.equal(application.workspaces[0].loaded.config.folders[0].path, fallbackCanonical);

  const saved = JSON.parse(await readFile(configPath, 'utf8'));
  assert.equal(saved.folders.length, 1);
  assert.equal(saved.folders[0].id, 'default');
  assert.equal(saved.folders[0].path, fallbackCanonical);
  assert.equal(saved.folders.some(folder => folder.path === missingRoot), false);
});

test('workspace registry keeps settings, secrets, ports and multiple folders independent', async () => {
  const primary = await fixture('ctmcp-application-primary');
  const secondary = await fixture('ctmcp-application-secondary');
  const secondFolder = path.join(secondary.base, 'docs');
  await import('node:fs/promises').then(({ mkdir }) => mkdir(secondFolder, { recursive: true }));

  await writeFile(primary.configPath, `${JSON.stringify(configDocument(primary.root, primary.dataDir, 43131), null, 2)}\n`);
  await writeFile(secondary.configPath, `${JSON.stringify(configDocument(secondary.root, secondary.dataDir, 43132, {
    permissionMode: 'guarded',
    folders: [
      { id: 'source', name: 'Source', path: secondary.root },
      { id: 'docs', name: 'Docs', path: secondFolder }
    ]
  }), null, 2)}\n`);

  const registryPath = path.join(primary.dataDir, 'workspace-profiles.json');
  await writeFile(registryPath, `${JSON.stringify({
    schema_version: 1,
    workspaces: [
      { id: 'primary', name: 'Primary Workspace', configPath: primary.configPath },
      { id: 'secondary', name: 'Secondary Workspace', configPath: secondary.configPath }
    ]
  }, null, 2)}\n`);

  const application = await loadApplication(primary.configPath);
  assert.deepEqual(application.workspaces.map(workspace => workspace.id), ['primary', 'secondary']);
  assert.deepEqual(application.workspaces.map(workspace => workspace.loaded.config.port), [43131, 43132]);
  assert.equal(application.workspaces[0].loaded.config.folders.length, 1);
  assert.equal(application.workspaces[1].loaded.config.folders.length, 2);
  assert.equal(application.workspaces[1].loaded.config.permissionMode, 'guarded');

  const store = new ApplicationConfigStore(application);
  const before = store.snapshot();
  const primaryPassword = store.secret('primary', 'oauthPassword');
  const secondaryPassword = store.secret('secondary', 'oauthPassword');
  assert.notEqual(primaryPassword, secondaryPassword);

  const rotated = await store.regenerateSecret('secondary', 'oauthPassword');
  assert.notEqual(rotated.value, secondaryPassword);
  assert.equal(store.secret('primary', 'oauthPassword'), primaryPassword);
  assert.equal(store.secret('secondary', 'oauthPassword'), rotated.value);

  const secondarySnapshot = before.workspaces.find(workspace => workspace.id === 'secondary');
  const renamedFolders = [
    ...secondarySnapshot.saved.folders,
    { id: 'assets', name: 'Assets', path: primary.root }
  ];
  await store.saveWorkspace('secondary', updatePayload(secondarySnapshot, {
    name: 'Secondary Renamed',
    folders: renamedFolders
  }));

  const after = store.snapshot();
  const primaryAfter = after.workspaces.find(workspace => workspace.id === 'primary');
  const secondaryAfter = after.workspaces.find(workspace => workspace.id === 'secondary');
  assert.equal(primaryAfter.name, 'Primary Workspace');
  assert.equal(primaryAfter.saved.folders.length, 1);
  assert.equal(secondaryAfter.name, 'Secondary Renamed');
  assert.equal(secondaryAfter.saved.folders.length, 3);

  const savedRegistry = JSON.parse(await readFile(registryPath, 'utf8'));
  assert.equal(savedRegistry.workspaces[1].name, 'Secondary Renamed');
  const reloadedSecondary = await loadConfigBundle(secondary.configPath);
  assert.equal(reloadedSecondary.config.folders.length, 3);
  assert.equal(reloadedSecondary.config.oauth.password, rotated.value);
});

test('workspace management API scopes settings, dashboards and authorization passwords by profile', async t => {
  const primary = await fixture('ctmcp-application-api-primary');
  const secondary = await fixture('ctmcp-application-api-secondary');
  await writeFile(primary.configPath, `${JSON.stringify(configDocument(primary.root, primary.dataDir, 43151), null, 2)}\n`);
  await writeFile(secondary.configPath, `${JSON.stringify(configDocument(secondary.root, secondary.dataDir, 43152, {
    permissionMode: 'guarded',
    oauth: { clientId: 'secondary-client' },
    folders: [{ id: 'secondary-root', name: 'Secondary Root', path: secondary.root }],
    tunnel: { enabled: true, publicUrl: 'https://secondary.example.test/builtin/clients/secondary-client/mcp' }
  }), null, 2)}\n`);
  await writeFile(path.join(primary.dataDir, 'workspace-profiles.json'), `${JSON.stringify({
    schema_version: 1,
    workspaces: [
      { id: 'primary', name: 'Primary Workspace', configPath: primary.configPath },
      { id: 'secondary', name: 'Secondary Workspace', configPath: secondary.configPath }
    ]
  }, null, 2)}\n`);

  const application = await loadApplication(primary.configPath);
  const store = new ApplicationConfigStore(application);
  const runtimes = new Map();
  const primaryProfile = application.workspaces[0];
  const secondaryProfile = application.workspaces[1];
  primaryProfile.loaded.config.port = 0;
  secondaryProfile.loaded.config.port = 0;
  secondaryProfile.loaded.config.management.enabled = false;

  const primaryRuntime = await createAgentRuntime(primaryProfile.loaded.config, {
    configStore: store.workspace('primary').store,
    workspaceStore: store,
    runtimeRegistry: runtimes
  });
  const secondaryConfigStore = store.workspace('secondary').store;
  const secondaryRuntime = await createAgentRuntime(secondaryProfile.loaded.config, {
    runtimeRegistry: runtimes,
    secretResolver: reference => secondaryConfigStore.secretValue(reference)
  });
  const secondaryTunnelReconfigurations = [];
  const secondaryRuntimeRecord = runtimes.get('secondary');
  assert.ok(secondaryRuntimeRecord);
  secondaryRuntimeRecord.tunnel = {
    async reconfigure(tunnel, publicBaseUrl) {
      secondaryTunnelReconfigurations.push({
        tunnel: tunnel ? structuredClone(tunnel) : undefined,
        publicBaseUrl
      });
    },
    async enforceSecurity() {}
  };
  const port = await listen(primaryRuntime.server);
  t.after(() => new Promise(resolve => primaryRuntime.server.close(resolve)));
  const base = `http://127.0.0.1:${port}`;
  const token = managementToken(await fetch(`${base}/ui`).then(response => response.text()));
  assert.ok(token);
  const headers = { 'x-ctmcp-admin-token': token };

  const snapshot = await fetch(`${base}/admin/api/config`, { headers }).then(response => response.json());
  assert.equal(snapshot.primaryWorkspaceId, 'primary');
  assert.deepEqual(snapshot.workspaces.map(workspace => workspace.id), ['primary', 'secondary']);

  const status = await fetch(`${base}/admin/api/status`, { headers }).then(response => response.json());
  assert.deepEqual(status.workspaces.map(workspace => workspace.id), ['primary', 'secondary']);

  const secondaryDashboard = await fetch(`${base}/admin/api/dashboard?workspaceId=secondary`, { headers });
  assert.equal(secondaryDashboard.status, 200);
  const secondaryDashboardPayload = await secondaryDashboard.json();
  assert.equal(secondaryDashboardPayload.permissions.byWorkspace[0].workspaceFolderId, 'secondary-root');

  const passwordResponse = await fetch(`${base}/admin/api/workspaces/secondary/secrets/oauth-password`, { headers });
  assert.equal(passwordResponse.status, 200);
  const originalPassword = (await passwordResponse.json()).value;
  assert.equal(originalPassword, store.secret('secondary', 'oauthPassword'));

  const regenerated = await fetch(`${base}/admin/api/workspaces/secondary/secrets/oauth-password/regenerate`, {
    method: 'POST',
    headers: { ...headers, origin: base }
  }).then(response => response.json());
  assert.notEqual(regenerated.value, originalPassword);
  assert.equal(regenerated.restartRequired, false);
  assert.deepEqual(regenerated.appliedImmediately, ['oauth']);
  assert.equal(store.secret('secondary', 'oauthPassword'), regenerated.value);
  assert.equal(secondaryRuntime.oauth.password, regenerated.value);
  assert.equal(secondaryRuntime.context.config.oauth.password, regenerated.value);

  const enrollmentUrl = 'https://secondary.example.test/enroll/once';
  const enrollmentResponse = await fetch(`${base}/admin/api/workspaces/secondary/secrets/builtin-tunnel-enrollment-url`, {
    method: 'PUT',
    headers: { ...headers, 'content-type': 'application/json', origin: base },
    body: JSON.stringify({ value: enrollmentUrl })
  });
  assert.equal(enrollmentResponse.status, 200);
  const enrollmentResult = await enrollmentResponse.json();
  assert.equal(enrollmentResult.restartRequired, false);
  assert.deepEqual(enrollmentResult.appliedImmediately, ['tunnel']);
  assert.equal(secondaryTunnelReconfigurations.length, 1);
  assert.equal(secondaryTunnelReconfigurations[0].tunnel.enrollmentUrl, enrollmentUrl);
  assert.equal(secondaryRuntime.context.config.tunnel.enrollmentUrl, enrollmentUrl);
  const persistedSecondary = await loadConfigBundle(secondary.configPath);
  assert.equal(persistedSecondary.config.tunnel.enrollmentUrl, enrollmentUrl);
  assert.doesNotMatch(await readFile(secondary.configPath, 'utf8'), /enroll\/once/);

  const namedSecretValue = 'named-secret-test-value-12345';
  const namedSecretUrl = `${base}/admin/api/workspaces/secondary/secrets/named/admin_api_key`;
  const namedSecretResponse = await fetch(namedSecretUrl, {
    method: 'PUT',
    headers: { ...headers, 'content-type': 'application/json', origin: base },
    body: JSON.stringify({ value: namedSecretValue })
  });
  assert.equal(namedSecretResponse.status, 200);
  const namedSecretResult = await namedSecretResponse.json();
  assert.equal(namedSecretResult.restartRequired, false);
  assert.deepEqual(namedSecretResult.appliedImmediately, ['process-secrets']);
  assert.doesNotMatch(JSON.stringify(namedSecretResult), new RegExp(namedSecretValue));
  assert.equal(secondaryConfigStore.secretValue('admin_api_key'), namedSecretValue);
  assert.equal(secondaryRuntime.context.resolveSecret?.('admin_api_key'), namedSecretValue);
  const namedSecretStorePath = secondaryConfigStore.snapshot().secretStorePath;
  assert.doesNotMatch(await readFile(namedSecretStorePath, 'utf8'), new RegExp(namedSecretValue));
  const reloadedNamedSecretApplication = await loadApplication(primary.configPath);
  const reloadedNamedSecretStore = new ApplicationConfigStore(reloadedNamedSecretApplication);
  assert.equal(reloadedNamedSecretStore.workspace('secondary').store.secretValue('admin_api_key'), namedSecretValue);
  const namedSecretReadResponse = await fetch(namedSecretUrl, { headers });
  assert.equal(namedSecretReadResponse.status, 405);

  const secondarySnapshot = snapshot.workspaces.find(workspace => workspace.id === 'secondary');
  const saveResponse = await fetch(`${base}/admin/api/workspaces/secondary/config`, {
    method: 'PUT',
    headers: { ...headers, 'content-type': 'application/json', origin: base },
    body: JSON.stringify(updatePayload(secondarySnapshot, { name: 'Secondary API Renamed' }))
  });
  const saveResult = await saveResponse.json();
  assert.equal(saveResponse.status, 200, JSON.stringify(saveResult));
  assert.equal(saveResult.name, 'Secondary API Renamed');

  const refreshed = await fetch(`${base}/admin/api/config`, { headers }).then(response => response.json());
  assert.equal(refreshed.workspaces.find(workspace => workspace.id === 'secondary').name, 'Secondary API Renamed');
  assert.equal(refreshed.workspaces.find(workspace => workspace.id === 'primary').name, 'Primary Workspace');

  const extraFolder = path.join(secondary.base, 'linux-extra');
  await import('node:fs/promises').then(({ mkdir }) => mkdir(extraFolder, { recursive: true }));
  const refreshedSecondary = refreshed.workspaces.find(workspace => workspace.id === 'secondary');
  const folderResponse = await fetch(`${base}/admin/api/workspaces/secondary/config`, {
    method: 'PUT',
    headers: { ...headers, 'content-type': 'application/json', origin: base },
    body: JSON.stringify(updatePayload(refreshedSecondary, {
      folders: [
        ...refreshedSecondary.saved.folders,
        { name: 'Linux Extra', path: extraFolder }
      ]
    }))
  });
  const folderResult = await folderResponse.json();
  assert.equal(folderResponse.status, 200, JSON.stringify(folderResult));
  assert.ok(folderResult.appliedImmediately.includes('folders'));
  assert.equal(folderResult.hotApplyDeferredReason, null);
  assert.equal(secondaryRuntime.context.config.folders.length, 2);
  const canonicalExtraFolder = await import('node:fs/promises').then(({ realpath }) => realpath(extraFolder));
  assert.equal(secondaryRuntime.context.config.folders[1].path, canonicalExtraFolder);

  const foldersSnapshot = await fetch(`${base}/admin/api/config`, { headers }).then(response => response.json());
  const foldersWorkspace = foldersSnapshot.workspaces.find(workspace => workspace.id === 'secondary');
  assert.equal(foldersWorkspace.saved.folders.length, 2);
  assert.equal(foldersWorkspace.effective.folders.length, 2);
});

test('workspace registry rejects duplicate runtime ports before servers start', async () => {
  const primary = await fixture('ctmcp-application-conflict-primary');
  const secondary = await fixture('ctmcp-application-conflict-secondary');
  await writeFile(primary.configPath, `${JSON.stringify(configDocument(primary.root, primary.dataDir, 43141), null, 2)}\n`);
  await writeFile(secondary.configPath, `${JSON.stringify(configDocument(secondary.root, secondary.dataDir, 43141), null, 2)}\n`);
  await writeFile(path.join(primary.dataDir, 'workspace-profiles.json'), `${JSON.stringify({
    schema_version: 1,
    workspaces: [
      { id: 'one', name: 'One', configPath: primary.configPath },
      { id: 'two', name: 'Two', configPath: secondary.configPath }
    ]
  }, null, 2)}\n`);

  await assert.rejects(loadApplication(primary.configPath), /Workspace ports conflict/);
});

test('workspace pack export strips secrets and import allocates a local port and dataDir', async () => {
  const source = await fixture('ctmcp-pack-source');
  const dest = await fixture('ctmcp-pack-dest');
  await writeFile(source.configPath, `${JSON.stringify(configDocument(source.root, source.dataDir, 43201), null, 2)}\n`);
  await writeFile(dest.configPath, `${JSON.stringify(configDocument(dest.root, dest.dataDir, 43202), null, 2)}\n`);
  const sourceApp = await loadApplication(source.configPath);
  const sourceStore = new ApplicationConfigStore(sourceApp);
  const sourceId = sourceApp.workspaces[0].id;
  const pack = sourceStore.exportWorkspacePack(sourceId);
  const packText = JSON.stringify(pack);
  assert.equal(pack.schemaVersion, 2);
  assert.equal(pack.secretPresence.oauthPassword, true);
  assert.equal(pack.host?.node?.dataDir, undefined);
  assert.doesNotMatch(packText, /"password"|"tokenSecret"|"clientSecret"|"enrollmentUrl"/);
  assert.doesNotMatch(packText, new RegExp(source.dataDir.replace(/[\\/]/g, '[\\\\/]')));

  const destApp = await loadApplication(dest.configPath);
  const destStore = new ApplicationConfigStore(destApp);
  const imported = await destStore.importWorkspacePack(pack);
  assert.equal(imported.ok, true);
  assert.notEqual(imported.saved.port, 43202);
  assert.match(imported.saved.dataDir, /workspaces/);
  assert.notEqual(imported.saved.dataDir, source.dataDir);
  const destFile = JSON.parse(await readFile(destStore.workspace(imported.id).store.configPath, 'utf8'));
  assert.equal(destFile.schemaVersion, 2);
  assert.doesNotMatch(JSON.stringify(destFile), /oauthPassword|password/);
  if (process.env.CTMCP_PHASE5_DUMP) {
    const dump = process.env.CTMCP_PHASE5_DUMP;
    await writeFile(`${dump}/export.ctmcp-workspace.json`, `${JSON.stringify(pack, null, 2)}\n`);
    await writeFile(`${dump}/import-node.json`, `${JSON.stringify(destFile, null, 2)}\n`);
  }
});

test('shared drop workspace.json opens without replacing the Node data dir', async () => {
  const { mkdir } = await import('node:fs/promises');
  const { sharedWorkspaceFile, sharedWorkspacesRoot } = await import('../dist/application.js');
  const dest = await fixture('ctmcp-drop-dest');
  await writeFile(dest.configPath, `${JSON.stringify(configDocument(dest.root, dest.dataDir, 43301), null, 2)}\n`);
  const destApp = await loadApplication(dest.configPath);
  const destStore = new ApplicationConfigStore(destApp);

  const dropRoot = path.join(dest.base, 'CodingToolsMCP', 'workspaces');
  process.env.CTMCP_SHARED_WORKSPACES_ROOT = dropRoot;
  const source = await fixture('ctmcp-drop-source');
  await writeFile(source.configPath, `${JSON.stringify(configDocument(source.root, source.dataDir, 43302, {
    securityPolicy: { restrictToolCatalog: true }
  }), null, 2)}\n`);
  const sourceApp = await loadApplication(source.configPath);
  const sourceStore = new ApplicationConfigStore(sourceApp);
  const pack = sourceStore.exportWorkspacePack(sourceApp.workspaces[0].id);
  pack.host = { ...pack.host, desktop: { authType: 'bearer', actions: { localPort: 9 } }, node: { dataDir: 'C:/should-not-import' } };
  const dropFile = sharedWorkspaceFile('ws-drop');
  await mkdir(path.dirname(dropFile), { recursive: true });
  await writeFile(dropFile, `${JSON.stringify(pack, null, 2)}\n`);
  assert.match(dropFile.replaceAll('\\', '/'), /CodingToolsMCP\/workspaces\/ws-drop\/workspace.json/);

  const imported = await destStore.importSharedWorkspace('ws-drop');
  assert.equal(imported.ok, true);
  assert.notEqual(imported.saved.dataDir, dest.dataDir);
  assert.notEqual(imported.saved.dataDir, 'C:/should-not-import');
  assert.match(imported.saved.dataDir, /workspaces/);
  const saved = JSON.parse(await readFile(destStore.workspace(imported.id).store.configPath, 'utf8'));
  assert.equal(saved.auth?.type ?? 'oauth', 'oauth');
  assert.equal(saved.host?.desktop?.authType, 'bearer');
  if (process.env.CTMCP_PHASE6_DUMP) {
    await writeFile(`${process.env.CTMCP_PHASE6_DUMP}/phase6-open.txt`, [
      `shared_root=${sharedWorkspacesRoot()}`,
      `drop_file=${dropFile}`,
      `imported_id=${imported.id}`,
      `imported_dataDir=${imported.saved.dataDir}`,
      `primary_dataDir=${dest.dataDir}`,
      `desktop_authType=${saved.host?.desktop?.authType}`,
      `runtime_auth=${saved.auth?.type}`
    ].join('\n'));
  }
  delete process.env.CTMCP_SHARED_WORKSPACES_ROOT;
});
