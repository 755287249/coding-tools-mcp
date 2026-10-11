import test from 'node:test';
import assert from 'node:assert/strict';
import { mkdtemp, readFile, rm, writeFile } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import path from 'node:path';
import { fileURLToPath } from 'node:url';
import { validateParity } from '../scripts/check-node-agent-parity.mjs';

const root = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '..');
const roadmapDirectory = path.join(root, 'docs', 'todo', 'node-agent-parity');
const manifestPath = path.join(roadmapDirectory, 'manifest.json');
const registryPath = path.join(roadmapDirectory, 'assertions.json');

async function readLiveManifest() {
  return JSON.parse(await readFile(manifestPath, 'utf8'));
}

function runtimeBaseline(manifest) {
  return {
    repositoryHead: manifest.baseline.repository_head,
    desktopClientVersion: manifest.baseline.desktop_client_version,
    nodeAgentVersion: manifest.baseline.node_agent_version,
    clientCompatibilityVersion: manifest.baseline.client_compatibility_version
  };
}

async function validateMutation(t, mutate, options = {}) {
  const directory = await mkdtemp(path.join(tmpdir(), 'ctmcp-parity-validator-'));
  t.after(() => rm(directory, { recursive: true, force: true }));
  const manifest = await readLiveManifest();
  mutate(manifest);
  if (options.registryMutate) {
    const registry = JSON.parse(await readFile(registryPath, 'utf8'));
    options.registryMutate(registry);
    const temporaryRegistry = path.join(directory, 'assertions.json');
    await writeFile(temporaryRegistry, `${JSON.stringify(registry, null, 2)}\n`, 'utf8');
    manifest.assertion_registry = temporaryRegistry;
  }
  const file = path.join(directory, 'manifest.json');
  await writeFile(file, `${JSON.stringify(manifest, null, 2)}\n`, 'utf8');
  const { registryMutate: _registryMutate, ...validationOptions } = options;
  return validateParity(root, file, {
    roadmapDirectory,
    runtimeBaseline: runtimeBaseline(manifest),
    isAncestor: true,
    ...validationOptions
  });
}

async function executableResults(overrides = {}) {
  const registry = JSON.parse(await readFile(registryPath, 'utf8'));
  return registry.assertions
    .filter(assertion => assertion.mode === 'node_test')
    .map(assertion => ({
      id: assertion.id,
      item_ids: assertion.item_ids,
      status: overrides[assertion.id]?.status ?? 'passed',
      message: overrides[assertion.id]?.message ?? 'fixture passed'
    }));
}

test('Node Agent Rust parity roadmap is structurally valid and dependency ordered', async () => {
  const result = await validateParity();
  assert.deepEqual(result.errors, []);
  assert.equal(result.items.length, 29);
  assert.equal(result.assertions.length, 29);
  assert.deepEqual(result.ready.map(item => item.id), []);
  assert.deepEqual(result.pending.map(item => item.id), []);
  assert.deepEqual(result.counts, { todo: 0, in_progress: 0, blocked: 0, done: 28, excluded: 1 });
  assert.equal(result.items.find(item => item.id === 'NP-016')?.status, 'excluded');
  assert.equal(result.baselineStatus.ancestor, true);
  assert.equal(result.manifest.intentional_divergences.length, 10);
  assert.deepEqual(result.assertionResults.filter(value => value.status === 'planned'), []);
  for (const id of ['PA-025-MCP-HTTP-VALIDATION', 'PA-026-MCP-STREAMING', 'PA-027-MCP-HTTP-SEMANTICS']) {
    const assertion = result.assertions.find(value => value.id === id);
    assert.equal(assertion?.mode, 'node_test');
    assert.equal(assertion?.required, true);
  }
});

test('stale baseline versions and non-ancestor anchors fail precisely', async t => {
  const staleVersion = await validateMutation(t, manifest => {
    manifest.baseline.node_agent_version = '0.0.0';
  }, {
    runtimeBaseline: {
      repositoryHead: '673be6430f274f954b4b51bc8b5b2034239f743a',
      desktopClientVersion: '0.1.37',
      nodeAgentVersion: '0.29.6',
      clientCompatibilityVersion: '0.1.37'
    }
  });
  assert.ok(staleVersion.errors.some(error => error.includes('baseline node_agent_version 0.0.0 does not match 0.29.6')));

  const staleHead = await validateMutation(t, manifest => {
    manifest.baseline.repository_head = '1111111111111111111111111111111111111111';
  }, { isAncestor: false });
  assert.ok(staleHead.errors.some(error => error.includes('is not an ancestor of HEAD')));
});

test('non-product TODOs require bidirectional behavioral assertion IDs', async t => {
  const result = await validateMutation(t, manifest => {
    delete manifest.items.find(item => item.id === 'NP-017').assertion_ids;
  });
  assert.ok(result.errors.includes('NP-017: non-product item must declare assertion_ids'));
  assert.ok(result.errors.includes('NP-017/PA-017-PATH-CONTAINMENT: item does not link back to assertion'));
});

test('failed and skipped differential fixtures report exact TODO and assertion IDs', async t => {
  const failed = await validateMutation(t, () => {}, {
    runAssertions: true,
    assertionResults: await executableResults({
      'PA-017-PATH-CONTAINMENT': { status: 'failed', message: 'escape fixture failed' }
    })
  });
  assert.ok(failed.errors.includes('NP-017/PA-017-PATH-CONTAINMENT: escape fixture failed'));

  const skipped = await validateMutation(t, () => {}, {
    runAssertions: true,
    assertionResults: await executableResults({
      'PA-023-TUNNEL-TIMING': { status: 'skipped', message: 'timeout fixture unavailable' }
    })
  });
  assert.ok(skipped.errors.includes('NP-023/PA-023-TUNNEL-TIMING: timeout fixture unavailable'));
});

test('completed roadmap items cannot depend on reverted work', async t => {
  const result = await validateMutation(t, manifest => {
    manifest.items.find(item => item.id === 'NP-023').status = 'todo';
    manifest.items.find(item => item.id === 'NP-024').status = 'done';
  });
  assert.ok(result.errors.includes('NP-024: completed item depends on incomplete NP-023'));
});

test('intentional divergences require explicit known shared assertions', async t => {
  const missing = await validateMutation(t, manifest => {
    delete manifest.intentional_divergences[0].shared_assertion_ids;
  });
  assert.ok(missing.errors.includes('ND-001: shared_assertion_ids must be explicit'));

  const unknown = await validateMutation(t, manifest => {
    manifest.intentional_divergences[0].shared_assertion_ids = ['PA-UNKNOWN'];
  });
  assert.ok(unknown.errors.includes('ND-001/PA-UNKNOWN: unknown shared assertion'));
});

test('planned assertions cannot claim completion and required modes are enforced', async t => {
  const completed = await validateMutation(t, () => {}, {
    registryMutate(registry) {
      const assertion = registry.assertions.find(value => value.id === 'PA-025-MCP-HTTP-VALIDATION');
      assertion.mode = 'planned';
      assertion.required = false;
    }
  });
  assert.ok(completed.errors.includes('NP-025/PA-025-MCP-HTTP-VALIDATION: completed item cannot use a planned assertion'));
  assert.ok(completed.errors.includes('NP-025/PA-025-MCP-HTTP-VALIDATION: planned assertion owner must remain incomplete'));

  const invalidPlanned = await validateMutation(t, () => {}, {
    registryMutate(registry) {
      const assertion = registry.assertions.find(value => value.id === 'PA-025-MCP-HTTP-VALIDATION');
      assertion.mode = 'planned';
      assertion.required = true;
    }
  });
  assert.ok(invalidPlanned.errors.includes('PA-025-MCP-HTTP-VALIDATION: planned assertions must set required to false'));

  const invalidExecutable = await validateMutation(t, () => {}, {
    registryMutate(registry) {
      registry.assertions.find(value => value.id === 'PA-025-MCP-HTTP-VALIDATION').required = false;
    }
  });
  assert.ok(invalidExecutable.errors.includes('PA-025-MCP-HTTP-VALIDATION: executable and evidence assertions must set required to true'));
});
