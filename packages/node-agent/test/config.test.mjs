import test from 'node:test';
import assert from 'node:assert/strict';
import { access, mkdtemp, readFile, realpath, rm, writeFile } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import path from 'node:path';
import { CANONICAL_SCHEMA_VERSION, CURRENT_CONFIG_SCHEMA_VERSION, loadConfig, loadConfigBundle } from '../dist/config.js';

// Never let destructive missing-key/tamper fixtures target a developer's live Agent data directory.
delete process.env.CTMCP_DATA_DIR;

function legacyDocument(root, dataDir, overrides = {}) {
  return {
    host: '127.0.0.1',
    port: 3789,
    dataDir,
    permissionMode: 'trusted',
    oauth: {
      clientId: 'chatgpt',
      password: 'legacy-password',
      clientSecret: 'legacy-client-secret',
      tokenSecret: 'legacy-token-secret-that-is-long-enough'
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

async function fixture(prefix = 'ctmcp-config') {
  const root = await mkdtemp(path.join(tmpdir(), `${prefix}-root-`));
  const dataDir = await mkdtemp(path.join(tmpdir(), `${prefix}-data-`));
  const configFile = path.join(dataDir, 'agent.json');
  return { root, dataDir, configFile };
}

test('command timeout ceiling defaults to 30 minutes and supports a bounded environment override', async () => {
  const { root, dataDir, configFile } = await fixture('ctmcp-command-timeout');
  await writeFile(configFile, JSON.stringify(legacyDocument(root, dataDir)));
  const previous = process.env.CTMCP_COMMAND_TIMEOUT_MAX_MS;
  delete process.env.CTMCP_COMMAND_TIMEOUT_MAX_MS;
  try {
    const defaults = await loadConfig(configFile);
    assert.equal(defaults.limits.commandTimeoutMaxMs, 1_800_000);
    process.env.CTMCP_COMMAND_TIMEOUT_MAX_MS = '3600000';
    const overridden = await loadConfig(configFile);
    assert.equal(overridden.limits.commandTimeoutMaxMs, 3_600_000);
    process.env.CTMCP_COMMAND_TIMEOUT_MAX_MS = '3600001';
    const clamped = await loadConfig(configFile);
    assert.equal(clamped.limits.commandTimeoutMaxMs, 3_600_000);
  } finally {
    if (previous === undefined) delete process.env.CTMCP_COMMAND_TIMEOUT_MAX_MS;
    else process.env.CTMCP_COMMAND_TIMEOUT_MAX_MS = previous;
  }
});

test('legacy configs default sandboxing off while retaining AppContainer as the selected backend', async () => {
  const { root, dataDir, configFile } = await fixture('ctmcp-sandbox-default');
  await writeFile(configFile, JSON.stringify(legacyDocument(root, dataDir)));
  const loaded = await loadConfig(configFile);
  assert.deepEqual(loaded.sandbox, {
    enabled: false,
    backend: 'appcontainer',
    externalPaths: [],
    options: {}
  });
});

test('sandbox environment overrides remain backend-neutral and bounded', async () => {
  const { root, dataDir, configFile } = await fixture('ctmcp-sandbox-env');
  await writeFile(configFile, JSON.stringify(legacyDocument(root, dataDir, {
    sandbox: {
      enabled: true,
      backend: 'appcontainer',
      externalPaths: [{ path: root, access: 'read_only' }],
      options: { 'wslc.image': 'ubuntu:24.04' }
    }
  })));
  const previous = {
    enabled: process.env.CTMCP_SANDBOX_ENABLED,
    backend: process.env.CTMCP_SANDBOX_BACKEND,
    image: process.env.CTMCP_WSLC_IMAGE,
    network: process.env.CTMCP_WSLC_NETWORK,
    sessionStorage: process.env.CTMCP_WSLC_SESSION_STORAGE
  };
  process.env.CTMCP_SANDBOX_ENABLED = '0';
  process.env.CTMCP_SANDBOX_BACKEND = 'wslc';
  process.env.CTMCP_WSLC_IMAGE = 'alpine:3.20';
  process.env.CTMCP_WSLC_NETWORK = 'none';
  process.env.CTMCP_WSLC_SESSION_STORAGE = path.join(dataDir, 'wslc-session');
  try {
    const loaded = await loadConfig(configFile);
    assert.equal(loaded.sandbox.enabled, false);
    assert.equal(loaded.sandbox.backend, 'wslc');
    assert.deepEqual(loaded.sandbox.externalPaths, [{ path: root, access: 'read_only' }]);
    assert.equal(loaded.sandbox.options['wslc.image'], 'alpine:3.20');
    assert.equal(loaded.sandbox.options['wslc.network'], 'none');
    assert.equal(loaded.sandbox.options['wslc.session_storage'], path.join(dataDir, 'wslc-session'));
  } finally {
    for (const [key, value] of Object.entries({
      CTMCP_SANDBOX_ENABLED: previous.enabled,
      CTMCP_SANDBOX_BACKEND: previous.backend,
      CTMCP_WSLC_IMAGE: previous.image,
      CTMCP_WSLC_NETWORK: previous.network,
      CTMCP_WSLC_SESSION_STORAGE: previous.sessionStorage
    })) {
      if (value === undefined) delete process.env[key];
      else process.env[key] = value;
    }
  }
});

test('loadConfig canonicalizes folder identities and rejects duplicate physical roots', async () => {
  const canonicalFixture = await fixture('ctmcp-canonical-folders');
  const alias = path.join(canonicalFixture.root, 'nested', '..');
  await writeFile(
    canonicalFixture.configFile,
    `${JSON.stringify(legacyDocument(alias, canonicalFixture.dataDir), null, 2)}\n`
  );
  const loaded = await loadConfigBundle(canonicalFixture.configFile);
  assert.equal(loaded.config.folders[0].path, await realpath(canonicalFixture.root));

  const duplicateFixture = await fixture('ctmcp-duplicate-folders');
  const duplicate = legacyDocument(duplicateFixture.root, duplicateFixture.dataDir, {
    folders: [
      { id: 'primary', name: 'Primary', path: duplicateFixture.root },
      { id: 'alias', name: 'Alias', path: path.join(duplicateFixture.root, '.') }
    ]
  });
  await writeFile(duplicateFixture.configFile, `${JSON.stringify(duplicate, null, 2)}\n`);
  await assert.rejects(
    loadConfigBundle(duplicateFixture.configFile),
    error => error?.code === 'WORKSPACE_FOLDER_DUPLICATE_ROOT'
  );
});

test('loadConfig derives and persists folder id and name when only path is configured', async () => {
  const { root, dataDir, configFile } = await fixture('ctmcp-derived-folder');
  const input = legacyDocument(root, dataDir, {
    schema_version: CURRENT_CONFIG_SCHEMA_VERSION,
    folders: [{ path: root }]
  });
  await writeFile(configFile, `${JSON.stringify(input, null, 2)}\n`);

  const first = await loadConfigBundle(configFile);
  assert.match(first.config.folders[0].id, /^[0-9a-f]{32}$/);
  assert.equal(first.config.folders[0].name, path.basename(root));

  const persisted = JSON.parse(await readFile(configFile, 'utf8'));
  assert.equal(persisted.folders[0].id, first.config.folders[0].id);
  assert.equal(persisted.folders[0].name, path.basename(root));

  const restarted = await loadConfigBundle(configFile);
  assert.equal(restarted.config.folders[0].id, first.config.folders[0].id);
  assert.equal(restarted.migrationApplied, false);
});

test('legacy plaintext configuration migrates to schema v1 and encrypted secret storage', async () => {
  const { root, dataDir, configFile } = await fixture('ctmcp-migration');
  const input = legacyDocument(root, dataDir, {
    tunnel: {
      enabled: true,
      publicUrl: 'https://tunnel.example/builtin/clients/device_1/mcp',
      enrollmentUrl: 'https://tunnel.example/_tunnel/enroll/LEGACYCODE'
    }
  });
  await writeFile(configFile, `${JSON.stringify(input, null, 2)}\n`);

  const loaded = await loadConfigBundle(configFile);
  assert.equal(loaded.migrationApplied, true);
  assert.equal(loaded.migratedFromSchema, 0);
  assert.equal(loaded.config.oauth.password, 'legacy-password');
  assert.equal(loaded.config.oauth.clientSecret, 'legacy-client-secret');
  assert.equal(loaded.config.oauth.tokenSecret, 'legacy-token-secret-that-is-long-enough');
  assert.equal(loaded.config.tunnel.enrollmentUrl, 'https://tunnel.example/_tunnel/enroll/LEGACYCODE');

  const migrated = JSON.parse(await readFile(configFile, 'utf8'));
  assert.equal(migrated.schemaVersion, CANONICAL_SCHEMA_VERSION);
  assert.equal(migrated.schema_version, undefined);
  assert.equal(migrated.auth.oauthClientId, 'chatgpt');
  assert.equal(migrated.oauth, undefined);
  assert.equal(migrated.tunnel.builtin.publicUrl, 'https://tunnel.example/builtin/clients/device_1/mcp');
  assert.equal(migrated.tunnel.enrollmentUrl, undefined);
  assert.equal(migrated.tunnel.builtin.enrollmentUrl, undefined);
  const publicText = JSON.stringify(migrated);
  assert.doesNotMatch(publicText, /legacy-password|legacy-client-secret|legacy-token-secret|LEGACYCODE/);
  await access(`${configFile}.bak`);

  const encrypted = await readFile(loaded.secretStorePath, 'utf8');
  assert.match(encrypted, /"algorithm": "aes-256-gcm"/);
  assert.doesNotMatch(encrypted, /legacy-password|legacy-client-secret|legacy-token-secret|LEGACYCODE/);
  assert.ok((await readFile(loaded.secretKeyPath, 'utf8')).trim().length >= 43);

  const restarted = await loadConfigBundle(configFile);
  assert.equal(restarted.migrationApplied, false);
  assert.equal(restarted.config.oauth.password, 'legacy-password');
  assert.equal(restarted.config.oauth.clientSecret, 'legacy-client-secret');
  assert.equal(restarted.config.oauth.tokenSecret, 'legacy-token-secret-that-is-long-enough');
  assert.equal(restarted.config.tunnel.enrollmentUrl, 'https://tunnel.example/_tunnel/enroll/LEGACYCODE');
});

test('loadConfig persists an automatically generated OAuth token secret in the encrypted store', async () => {
  const { root, dataDir, configFile } = await fixture('ctmcp-generated-token');
  const document = legacyDocument(root, dataDir);
  delete document.oauth.tokenSecret;
  await writeFile(configFile, JSON.stringify(document));
  const previous = process.env.CTMCP_OAUTH_TOKEN_SECRET;
  delete process.env.CTMCP_OAUTH_TOKEN_SECRET;
  try {
    const first = await loadConfigBundle(configFile);
    const second = await loadConfig(configFile);
    assert.equal(first.config.oauth.tokenSecret, second.oauth.tokenSecret);
    assert.match(first.config.oauth.tokenSecret, /^[0-9a-f]{64}$/);
    assert.doesNotMatch(await readFile(first.secretStorePath, 'utf8'), new RegExp(first.config.oauth.tokenSecret));
    await assert.rejects(access(path.join(dataDir, 'oauth-token-secret')));
  } finally {
    if (previous === undefined) delete process.env.CTMCP_OAUTH_TOKEN_SECRET;
    else process.env.CTMCP_OAUTH_TOKEN_SECRET = previous;
  }
});

test('legacy plaintext OAuth token file is migrated and removed', async () => {
  const { root, dataDir, configFile } = await fixture('ctmcp-token-file');
  const document = legacyDocument(root, dataDir);
  delete document.oauth.tokenSecret;
  document.schema_version = CURRENT_CONFIG_SCHEMA_VERSION;
  delete document.oauth.password;
  delete document.oauth.clientSecret;
  await writeFile(configFile, JSON.stringify(document));
  const legacyToken = 'legacy-file-token-secret-that-is-long-enough';
  const legacyFile = path.join(dataDir, 'oauth-token-secret');
  await writeFile(legacyFile, `${legacyToken}\n`);

  const loaded = await loadConfigBundle(configFile);
  assert.equal(loaded.config.oauth.tokenSecret, legacyToken);
  await assert.rejects(access(legacyFile));
  assert.doesNotMatch(await readFile(loaded.secretStorePath, 'utf8'), new RegExp(legacyToken));
});

test('environment secrets override encrypted values without replacing the stored fallback', async () => {
  const { root, dataDir, configFile } = await fixture('ctmcp-env-secret');
  await writeFile(configFile, JSON.stringify(legacyDocument(root, dataDir)));
  await loadConfigBundle(configFile);
  const previous = process.env.CTMCP_OAUTH_TOKEN_SECRET;
  process.env.CTMCP_OAUTH_TOKEN_SECRET = 'environment-token-secret-that-is-long-enough';
  try {
    const overridden = await loadConfigBundle(configFile);
    assert.equal(overridden.config.oauth.tokenSecret, 'environment-token-secret-that-is-long-enough');
  } finally {
    if (previous === undefined) delete process.env.CTMCP_OAUTH_TOKEN_SECRET;
    else process.env.CTMCP_OAUTH_TOKEN_SECRET = previous;
  }
  const fallback = await loadConfigBundle(configFile);
  assert.equal(fallback.config.oauth.tokenSecret, 'legacy-token-secret-that-is-long-enough');
});

test('loadConfig accepts a non-empty short legacy OAuth token secret like Rust', async () => {
  const { root, dataDir, configFile } = await fixture('ctmcp-short-token');
  const document = legacyDocument(root, dataDir);
  delete document.oauth.tokenSecret;
  await writeFile(configFile, JSON.stringify(document));
  const legacyFile = path.join(dataDir, 'oauth-token-secret');
  await writeFile(legacyFile, 'short\n');
  const previous = process.env.CTMCP_OAUTH_TOKEN_SECRET;
  delete process.env.CTMCP_OAUTH_TOKEN_SECRET;
  try {
    const loaded = await loadConfig(configFile);
    assert.equal(loaded.oauth.tokenSecret, 'short');
    await assert.rejects(access(legacyFile));
  } finally {
    if (previous === undefined) delete process.env.CTMCP_OAUTH_TOKEN_SECRET;
    else process.env.CTMCP_OAUTH_TOKEN_SECRET = previous;
  }
});

test('loadConfig rejects a blank legacy OAuth token secret', async () => {
  const { root, dataDir, configFile } = await fixture('ctmcp-blank-token');
  const document = legacyDocument(root, dataDir);
  delete document.oauth.tokenSecret;
  await writeFile(configFile, JSON.stringify(document));
  await writeFile(path.join(dataDir, 'oauth-token-secret'), '   \n');
  const previous = process.env.CTMCP_OAUTH_TOKEN_SECRET;
  delete process.env.CTMCP_OAUTH_TOKEN_SECRET;
  try {
    await assert.rejects(loadConfig(configFile), /Persisted OAuth token secret is invalid/);
  } finally {
    if (previous === undefined) delete process.env.CTMCP_OAUTH_TOKEN_SECRET;
    else process.env.CTMCP_OAUTH_TOKEN_SECRET = previous;
  }
});

test('loadConfig normalizes a string schema version to the numeric current version', async () => {
  const { root, dataDir, configFile } = await fixture('ctmcp-string-schema');
  const document = legacyDocument(root, dataDir);
  document.schema_version = String(CURRENT_CONFIG_SCHEMA_VERSION);
  await writeFile(configFile, JSON.stringify(document));
  const loaded = await loadConfigBundle(configFile);
  assert.equal(loaded.migrationApplied, true);
  assert.equal(JSON.parse(await readFile(configFile, 'utf8')).schemaVersion, CANONICAL_SCHEMA_VERSION);
});

test('loadConfig rejects non-numeric schema version values', async () => {
  const { root, dataDir, configFile } = await fixture('ctmcp-invalid-schema-type');
  await writeFile(configFile, JSON.stringify({
    ...legacyDocument(root, dataDir),
    schema_version: false
  }));
  await assert.rejects(loadConfig(configFile), /schema_version must be a non-negative integer/);
});

test('loadConfig rejects a future config schema version', async () => {
  const { root, dataDir, configFile } = await fixture('ctmcp-future-schema');
  await writeFile(configFile, JSON.stringify({
    ...legacyDocument(root, dataDir),
    schema_version: CURRENT_CONFIG_SCHEMA_VERSION + 1
  }));
  await assert.rejects(loadConfig(configFile), /Unsupported config schema_version/);
});

test('loadConfig restores a missing primary secret key from its local backup', async () => {
  const { root, dataDir, configFile } = await fixture('ctmcp-missing-key');
  await writeFile(configFile, JSON.stringify(legacyDocument(root, dataDir)));
  const loaded = await loadConfigBundle(configFile);
  const backupPath = `${loaded.secretKeyPath}.backup`;
  const backup = await readFile(backupPath, 'utf8');
  await rm(loaded.secretKeyPath);
  const restored = await loadConfigBundle(configFile);
  assert.equal(restored.config.oauth.password, 'legacy-password');
  assert.equal(await readFile(restored.secretKeyPath, 'utf8'), backup);
});

test('loadConfig rejects an encrypted secret store when both master key copies are missing', async () => {
  const { root, dataDir, configFile } = await fixture('ctmcp-missing-both-keys');
  await writeFile(configFile, JSON.stringify(legacyDocument(root, dataDir)));
  const loaded = await loadConfigBundle(configFile);
  await rm(loaded.secretKeyPath);
  await rm(`${loaded.secretKeyPath}.backup`);
  await assert.rejects(loadConfig(configFile), /exists without its key/);
});

test('loadConfig rejects a tampered encrypted secret store', async () => {
  const { root, dataDir, configFile } = await fixture('ctmcp-tampered-store');
  await writeFile(configFile, JSON.stringify(legacyDocument(root, dataDir)));
  const loaded = await loadConfigBundle(configFile);
  const envelope = JSON.parse(await readFile(loaded.secretStorePath, 'utf8'));
  envelope.tag = envelope.tag.replace(/^./, envelope.tag.startsWith('A') ? 'B' : 'A');
  await writeFile(loaded.secretStorePath, JSON.stringify(envelope));
  await assert.rejects(loadConfig(configFile), /Unable to decrypt agent secret store/);
});

test('loadConfig reads canonical v2, keeps host.desktop extras, and leaves secrets out of JSON', async () => {
  const { root, dataDir, configFile } = await fixture('ctmcp-canonical-v2');
  await writeFile(configFile, JSON.stringify({
    schemaVersion: CANONICAL_SCHEMA_VERSION,
    id: 'ws-v2',
    name: 'Repo',
    folders: [{ id: 'repo', name: 'Repo', path: root }],
    bind: { host: '127.0.0.1', port: 3789 },
    auth: { type: 'oauth', oauthClientId: 'chatgpt' },
    host: {
      node: { dataDir, management: { enabled: true } },
      desktop: { authType: 'bearer', actions: { localPort: 9 } }
    }
  }));
  const loaded = await loadConfigBundle(configFile);
  assert.equal(loaded.config.port, 3789);
  assert.equal(loaded.config.host, '127.0.0.1');
  assert.equal(loaded.canonical.id, 'ws-v2');
  assert.equal(loaded.canonical.host.desktop.authType, 'bearer');
  const onDisk = JSON.parse(await readFile(configFile, 'utf8'));
  assert.equal(onDisk.schemaVersion, CANONICAL_SCHEMA_VERSION);
  assert.equal(onDisk.host.desktop.authType, 'bearer');
  assert.equal(onDisk.host.desktop.actions.localPort, 9);
  assert.equal(onDisk.host.node.dataDir, dataDir);
  const publicText = JSON.stringify(onDisk);
  assert.doesNotMatch(publicText, /password|clientSecret|tokenSecret|enrollmentUrl/);
  const restarted = await loadConfigBundle(configFile);
  assert.equal(restarted.migrationApplied, false);
  assert.equal(restarted.canonical.host.desktop.actions.localPort, 9);
});

test('loadConfig rejects a future canonical schemaVersion', async () => {
  const { root, dataDir, configFile } = await fixture('ctmcp-future-canonical');
  await writeFile(configFile, JSON.stringify({
    schemaVersion: CANONICAL_SCHEMA_VERSION + 1,
    id: 'ws-future',
    name: 'Repo',
    folders: [{ id: 'repo', name: 'Repo', path: root }],
    bind: { host: '127.0.0.1', port: 3789 },
    auth: { type: 'oauth', oauthClientId: 'chatgpt' },
    host: { node: { dataDir } }
  }));
  await assert.rejects(loadConfig(configFile), /Unsupported workspace schemaVersion/);
});

test('loadConfigBundle seeds a missing config with the environment workspace before canonical migration', async () => {
  const { root, dataDir, configFile } = await fixture('ctmcp-fresh-config');
  const previousWorkspaces = process.env.CTMCP_WORKSPACES;
  const previousDataDir = process.env.CTMCP_DATA_DIR;
  process.env.CTMCP_WORKSPACES = root;
  process.env.CTMCP_DATA_DIR = dataDir;
  try {
    const loaded = await loadConfigBundle(configFile);
    assert.equal(loaded.config.folders.length, 1);
    assert.equal(await realpath(loaded.config.folders[0].path), await realpath(root));
    assert.equal(await realpath(loaded.canonical.folders[0].path), await realpath(root));
    const onDisk = JSON.parse(await readFile(configFile, 'utf8'));
    assert.equal(onDisk.schemaVersion, CANONICAL_SCHEMA_VERSION);
    assert.equal(onDisk.folders.length, 1);
    assert.equal(await realpath(onDisk.folders[0].path), await realpath(root));
  } finally {
    if (previousWorkspaces === undefined) delete process.env.CTMCP_WORKSPACES;
    else process.env.CTMCP_WORKSPACES = previousWorkspaces;
    if (previousDataDir === undefined) delete process.env.CTMCP_DATA_DIR;
    else process.env.CTMCP_DATA_DIR = previousDataDir;
  }
});

test('loadConfig falls back to one workspace folder when CTMCP_WORKSPACES contains only separators', async () => {
  const { root, dataDir, configFile } = await fixture('ctmcp-empty-workspaces-env');
  await writeFile(configFile, `${JSON.stringify(legacyDocument(root, dataDir), null, 2)}\n`);
  const previous = process.env.CTMCP_WORKSPACES;
  process.env.CTMCP_WORKSPACES = path.delimiter;
  try {
    const loaded = await loadConfigBundle(configFile);
    assert.equal(loaded.config.folders.length, 1);
    assert.ok(loaded.config.folders[0].path.length > 0);
  } finally {
    if (previous === undefined) delete process.env.CTMCP_WORKSPACES;
    else process.env.CTMCP_WORKSPACES = previous;
  }
});

test('loadConfig repairs and persists canonical workspaces with no folders', async () => {
  const { dataDir, configFile } = await fixture('ctmcp-empty-canonical-folders');
  const canonical = {
    schemaVersion: CANONICAL_SCHEMA_VERSION,
    id: 'empty-canonical',
    name: 'Empty Canonical',
    folders: [],
    activeFolderId: '',
    bind: { host: '127.0.0.1', port: 43119 },
    auth: { type: 'oauth', oauthClientId: 'chatgpt' },
    host: { desktop: {}, node: { dataDir } }
  };
  await writeFile(configFile, `${JSON.stringify(canonical, null, 2)}\n`);

  const loaded = await loadConfigBundle(configFile);
  assert.equal(loaded.config.folders.length, 1);
  assert.equal(loaded.migrationApplied, true);

  const persisted = JSON.parse(await readFile(configFile, 'utf8'));
  assert.equal(persisted.folders.length, 1);
  assert.equal(persisted.activeFolderId, persisted.folders[0].id);
});
