import test from 'node:test';
import assert from 'node:assert/strict';
import { spawn, spawnSync } from 'node:child_process';
import { mkdir, mkdtemp, readFile, readdir, rm, writeFile } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import path from 'node:path';
import {
  buildDockerSbxMounts,
  dockerSbxRuntimePath,
  prepareDockerSbxLaunch
} from '../dist/sandboxDockerSbx.js';
import {
  APPCONTAINER_HELPER_SOURCE,
  APPCONTAINER_HELPER_SOURCE_HASH,
  prepareAppContainer,
  prepareAppContainerLaunch,
  normalizeAppContainerSpec,
  resolveAppContainerPathProgram
} from '../dist/sandboxAppContainer.js';
import { sandboxBoundary } from '../dist/sandbox.js';
import {
  sandboxBackends,
  sandboxUsesPortableCommand
} from '../dist/sandbox.js';

async function directories(t) {
  const base = await mkdtemp(path.join(tmpdir(), 'ctmcp-node-sandbox-backends-'));
  const workspace = path.join(base, 'workspace');
  const data = path.join(base, 'data');
  const writable = path.join(base, 'writable');
  const readonly = path.join(base, 'readonly');
  const nestedReadonly = path.join(writable, 'nested-readonly');
  await Promise.all([workspace, data, writable, readonly, nestedReadonly].map(value => mkdir(value, { recursive: true })));
  t.after(() => rm(base, { recursive: true, force: true, maxRetries: 5, retryDelay: 50 }));
  return { workspace, data, writable, readonly, nestedReadonly };
}

function windowsDaclSddl(target) {
  const literal = target.replaceAll("'", "''");
  const result = spawnSync('powershell.exe', [
    '-NoLogo', '-NoProfile', '-NonInteractive', '-Command',
    `(Get-Acl -LiteralPath '${literal}').Sddl`
  ], { windowsHide: true, encoding: 'utf8', timeout: 10_000 });
  assert.equal(result.status, 0, `Get-Acl failed for ${target}: ${result.stderr}`);
  return result.stdout.trim();
}

async function waitForFile(target, timeoutMs = 5_000) {
  const deadline = Date.now() + timeoutMs;
  for (;;) {
    try {
      await readFile(target);
      return;
    } catch {
      if (Date.now() >= deadline) throw new Error(`Timed out waiting for ${target}`);
      await new Promise(resolve => setTimeout(resolve, 25));
    }
  }
}

async function singleLeaseDirectory(leaseRoot) {
  const entries = await readdir(leaseRoot, { withFileTypes: true });
  const directories = entries.filter(entry => entry.isDirectory()).map(entry => path.join(leaseRoot, entry.name));
  assert.equal(directories.length, 1, `expected one lease directory, got ${JSON.stringify(directories)}`);
  return directories[0];
}

test('Node backend descriptors expose the Rust sandbox set and readiness contract', () => {
  assert.deepEqual(sandboxBackends().map(backend => backend.id), ['appcontainer', 'docker', 'podman', 'docker_sbx', 'wslc']);
  assert.ok(sandboxBackends().every(backend => backend.enforcementReady));
  const appcontainer = sandboxBackends().find(backend => backend.id === 'appcontainer');
  assert.equal(appcontainer?.supportsWsl, false);
  assert.equal(appcontainer?.options[0]?.id, 'appcontainer.network');
  assert.equal(appcontainer?.options[0]?.defaultValue, 'none');
  const docker = sandboxBackends().find(backend => backend.id === 'docker');
  assert.equal(docker?.supportsWsl, true);
  assert.equal(docker?.options[0]?.id, 'docker.image');
  assert.equal(docker?.options[1]?.id, 'docker.network');
  const podman = sandboxBackends().find(backend => backend.id === 'podman');
  assert.equal(podman?.supportsWsl, true);
  assert.equal(podman?.options[0]?.id, 'podman.image');
  const dockerSbx = sandboxBackends().find(backend => backend.id === 'docker_sbx');
  assert.equal(dockerSbx?.supportsWsl, true);
  const wslc = sandboxBackends().find(backend => backend.id === 'wslc');
  assert.equal(wslc?.supportsWsl, true);
  assert.equal(wslc?.options[0]?.id, 'wslc.image');
  assert.equal(wslc?.options[1]?.id, 'wslc.network');
  assert.equal(sandboxUsesPortableCommand('docker'), true);
  assert.equal(sandboxUsesPortableCommand('podman'), true);
  assert.equal(sandboxUsesPortableCommand('docker_sbx'), true);
  assert.equal(sandboxUsesPortableCommand('wslc'), true);
  assert.equal(sandboxUsesPortableCommand('appcontainer'), false);
});

test('AppContainer is rejected for WSL folders while container backends advertise WSL support', () => {
  const folder = { id: 'wsl', name: 'wsl', path: '\\\\wsl.localhost\\Ubuntu\\home\\dev' };
  assert.throws(
    () => sandboxBoundary({ enabled: true, backend: 'appcontainer', externalPaths: [], options: {} }, folder),
    error => error?.code === 'SANDBOX_BACKEND_UNSUPPORTED'
  );
  assert.equal(sandboxBackends().find(backend => backend.id === 'docker')?.supportsWsl, true);
  assert.equal(sandboxBackends().find(backend => backend.id === 'podman')?.supportsWsl, true);
  assert.equal(sandboxBackends().find(backend => backend.id === 'docker_sbx')?.supportsWsl, true);
  assert.equal(sandboxBackends().find(backend => backend.id === 'wslc')?.supportsWsl, true);
});

test('AppContainer PATH runtime resolution follows the current PATH without stale lookup state', async t => {
  const base = await mkdtemp(path.join(tmpdir(), 'ctmcp-node-appcontainer-path-'));
  const first = path.join(base, 'first');
  const second = path.join(base, 'second');
  await Promise.all([first, second].map(value => mkdir(value, { recursive: true })));
  await Promise.all([
    writeFile(path.join(first, 'pwsh.exe'), 'first'),
    writeFile(path.join(second, 'pwsh.exe'), 'second')
  ]);
  t.after(() => rm(base, { recursive: true, force: true }));

  assert.equal(
    resolveAppContainerPathProgram('pwsh.exe', [first, second].join(path.delimiter), '.EXE', base),
    path.join(first, 'pwsh.exe')
  );
  assert.equal(
    resolveAppContainerPathProgram('pwsh.exe', second, '.EXE', base),
    path.join(second, 'pwsh.exe')
  );
  assert.equal(resolveAppContainerPathProgram('missing.exe', first, '.EXE', base), undefined);
});

test('AppContainer wraps cmd and PowerShell scripts the same way as the Rust provider', () => {
  const cmd = normalizeAppContainerSpec({
    program: 'C:\\workspace\\run.cmd',
    argv: ['a&b'],
    display: 'run.cmd',
    shell: false
  });
  assert.match(cmd.program.toLowerCase(), /cmd\.exe$/);
  assert.deepEqual(cmd.argv, ['/d', '/s', '/c']);
  assert.equal(cmd.rawArg, 'call "C:\\workspace\\run.cmd" "a&b"');

  const ps1 = normalizeAppContainerSpec({
    program: 'C:\\workspace\\run.ps1',
    argv: ["it's"],
    display: 'run.ps1',
    shell: false
  });
  assert.match(ps1.program.toLowerCase(), /(?:pwsh|powershell)\.exe$/);
  assert.ok(ps1.argv.includes('-Command'));
  assert.ok(ps1.argv.at(-1)?.includes("& 'C:\\workspace\\run.ps1' 'it''s'"));
});

test('AppContainer helper grants minimal non-inheriting traversal on ACL target ancestors', () => {
  const traversal = APPCONTAINER_HELPER_SOURCE.indexOf('private static void GrantAncestorTraversal');
  const applyGrant = APPCONTAINER_HELPER_SOURCE.indexOf('GrantAncestorTraversal(options, path, sid, granted);');
  const recordGrant = APPCONTAINER_HELPER_SOURCE.indexOf('RecordGrant(options, ancestor);', traversal);
  const rights = APPCONTAINER_HELPER_SOURCE.indexOf('FileSystemRights.Traverse | FileSystemRights.ReadAttributes', traversal);
  const noInheritance = APPCONTAINER_HELPER_SOURCE.indexOf('InheritanceFlags.None', rights);
  assert.ok(traversal >= 0, 'embedded helper must define ancestor traversal grants');
  assert.ok(applyGrant >= 0, 'every explicit ACL target must prepare ancestor traversal first');
  assert.ok(recordGrant > traversal, 'ancestor ACEs must be journaled for cleanup');
  assert.ok(rights > traversal, 'ancestor ACEs must use minimal traverse/read-attributes rights');
  assert.ok(noInheritance > rights, 'ancestor ACEs must not inherit into sibling directory trees');
  assert.match(APPCONTAINER_HELPER_SOURCE, /while \(current != null\)/, 'volume root traversal must also be granted');
});

test('Docker sbx mount model preserves read-only grants without broadening the workspace', async t => {
  const { workspace, writable, readonly } = await directories(t);
  const mounts = await buildDockerSbxMounts(workspace, [
    { path: readonly, access: 'read_only' },
    { path: writable, access: 'modify' }
  ]);
  assert.equal(mounts[0].host, workspace);
  assert.equal(mounts[0].access, 'modify');
  assert.ok(mounts.some(mount => mount.host === readonly && mount.access === 'read_only'));
  assert.ok(mounts.some(mount => mount.host === writable && mount.access === 'modify'));
});

test('Docker sbx rejects writable-parent/read-only-child overlaps', async t => {
  const { workspace, writable, nestedReadonly } = await directories(t);
  await assert.rejects(
    buildDockerSbxMounts(workspace, [
      { path: writable, access: 'modify' },
      { path: nestedReadonly, access: 'read_only' }
    ]),
    error => error?.code === 'SANDBOX_SBX_MOUNT_OVERLAP'
  );
});

test('Docker sbx launch uses the remote supervisor and forwards environment explicitly', async t => {
  const { workspace, readonly } = await directories(t);
  const mounts = await buildDockerSbxMounts(workspace, [{ path: readonly, access: 'read_only' }]);
  const launch = prepareDockerSbxLaunch(
    { cli: 'sbx', sandboxName: 'ctmcp-test', mounts },
    workspace,
    { program: 'sh', argv: ['-c', 'printf ok'], display: 'sh -c printf ok', shell: false },
    [['A', 'B'], ['DROP', 'value']],
    ['DROP']
  );
  assert.equal(launch.program, 'sbx');
  assert.equal(launch.environmentMode, 'forwarded');
  assert.equal(launch.processTreeContained, false);
  assert.equal(launch.processTreeControl, 'sbx_supervised_process_group');
  assert.deepEqual(launch.args.slice(0, 5), ['exec', '-i', '-w', dockerSbxRuntimePath(workspace), 'ctmcp-test']);
  assert.ok(launch.args.includes('A=B'));
  assert.ok(!launch.args.includes('DROP=value'));
  assert.ok(launch.args.includes('env'));
  assert.ok(launch.args.some(value => value.includes('setsid -w')));
});

test('AppContainer completion marker skips redundant cleanup helper launch', async t => {
  const { workspace, data } = await directories(t);
  const leaseRoot = path.join(data, 'sandbox', 'appcontainer', 'leases');
  await mkdir(leaseRoot, { recursive: true });
  const prepared = {
    helper: path.join(data, 'missing-helper.exe'),
    workspaceRoot: workspace,
    stateRoot: path.join(data, 'state'),
    externalPaths: [],
    environment: [],
    network: 'none',
    leaseRoot
  };
  const launch = prepareAppContainerLaunch(
    prepared,
    workspace,
    { program: 'tool.exe', argv: [], display: 'tool.exe', shell: false },
    [],
    []
  );
  const leaseDirectory = await singleLeaseDirectory(leaseRoot);
  assert.equal(await readFile(path.join(leaseDirectory, 'lease.json'), 'utf8').then(Boolean), true);
  await Promise.all([
    writeFile(path.join(leaseDirectory, 'complete'), 'clean'),
    writeFile(path.join(leaseDirectory, 'cleanup.state'), 'D\t.git\tdummy')
  ]);
  await launch.cleanup();
  assert.deepEqual(await readdir(leaseRoot), []);

  const failSafeLaunch = prepareAppContainerLaunch(
    prepared,
    workspace,
    { program: 'tool.exe', argv: [], display: 'tool.exe', shell: false },
    [],
    []
  );
  failSafeLaunch.onSpawn?.(12345);
  await assert.rejects(
    failSafeLaunch.cleanup(),
    error => error?.code === 'SANDBOX_APPCONTAINER_HELPER_FAILED'
  );
});

test('AppContainer live launch verifies its boundary and enforces read-only external grants', { skip: process.platform !== 'win32' || process.env.CTMCP_TEST_APPCONTAINER !== '1' }, async t => {
  const { workspace, data, readonly } = await directories(t);
  const readonlyInput = path.join(readonly, 'input.txt');
  const readonlyBlocked = path.join(readonly, 'blocked.txt');
  const workspaceMarker = path.join(workspace, 'workspace-marker.txt');
  const gitDir = path.join(workspace, '.git');
  await mkdir(gitDir, { recursive: true });
  const gitDaclBefore = windowsDaclSddl(gitDir);
  await writeFile(readonlyInput, 'readonly-ok');
  const prepared = await prepareAppContainer({
    enabled: true,
    backend: 'appcontainer',
    externalPaths: [{ path: readonly, access: 'read_only' }],
    options: {}
  }, workspace, data);
  assert.equal(path.basename(prepared.helper), `appcontainer-helper-${APPCONTAINER_HELPER_SOURCE_HASH}.exe`);
  const script = [
    "const fs = require('node:fs');",
    'const [marker, input, blocked, leaseAttack] = process.argv.slice(1);',
    "fs.writeFileSync(marker, 'workspace-ok');",
    "const content = fs.readFileSync(input, 'utf8').trim();",
    'let denied = false;',
    "try { fs.writeFileSync(blocked, 'unexpected'); } catch { denied = true; }",
    "if (!denied) { process.stderr.write('readonly-write-unexpected'); process.exit(42); }",
    'let leaseDenied = false;',
    "try { fs.writeFileSync(leaseAttack, 'forged'); } catch { leaseDenied = true; }",
    "if (!leaseDenied) { process.stderr.write('lease-write-unexpected'); process.exit(43); }",
    "process.stdout.write(content + '|write-denied|lease-denied');"
  ].join('');
  const launch = prepareAppContainerLaunch(
    prepared,
    workspace,
    { program: process.execPath, argv: ['-e', script, workspaceMarker, readonlyInput, readonlyBlocked], display: 'node -e appcontainer-security-probe', shell: false },
    [],
    []
  );
  const protectedStateIndex = launch.args.indexOf('--protected-state');
  assert.ok(protectedStateIndex >= 0);
  const leaseAttack = path.join(path.dirname(launch.args[protectedStateIndex + 1]), 'forged-by-child');
  launch.args.push(leaseAttack);
  t.after(() => launch.cleanup().catch(() => undefined));
  const result = spawnSync(launch.program, launch.args, { env: process.env, windowsHide: true, timeout: 30_000, encoding: 'utf8' });
  assert.equal(result.status, 0, `stderr: ${result.stderr}`);
  assert.equal(result.stdout, 'readonly-ok|write-denied|lease-denied');
  assert.equal(await readFile(workspaceMarker, 'utf8'), 'workspace-ok');
  assert.equal(windowsDaclSddl(gitDir), gitDaclBefore, 'natural helper exit must restore the exact .git DACL');
  await assert.rejects(readFile(readonlyBlocked, 'utf8'));
  const leaseRoot = path.join(data, 'sandbox', 'appcontainer', 'leases');
  const leaseDirectory = await singleLeaseDirectory(leaseRoot);
  assert.deepEqual((await readdir(leaseDirectory)).sort(), ['cleanup.state', 'complete', 'lease.json']);
  await launch.cleanup();
  assert.deepEqual(await readdir(leaseRoot), []);
});

test('AppContainer rejects external grants that target protected repository metadata', { skip: process.platform !== 'win32' || process.env.CTMCP_TEST_APPCONTAINER !== '1' }, async t => {
  const { workspace, data } = await directories(t);
  const gitDir = path.join(workspace, '.git');
  const workflowDir = path.join(workspace, '.github', 'workflows');
  await Promise.all([gitDir, workflowDir].map(value => mkdir(value, { recursive: true })));

  for (const target of [gitDir, workflowDir]) {
    await assert.rejects(
      prepareAppContainer({
        enabled: true,
        backend: 'appcontainer',
        externalPaths: [{ path: target, access: 'modify' }],
        options: {}
      }, workspace, data),
      error => error?.code === 'SANDBOX_EXTERNAL_PATH_PROTECTED'
    );
  }
});

test('AppContainer keeps its lease attestation isolated when dataDir lives inside the workspace', { skip: process.platform !== 'win32' || process.env.CTMCP_TEST_APPCONTAINER !== '1' }, async t => {
  const base = await mkdtemp(path.join(tmpdir(), 'ctmcp-node-appcontainer-overlap-'));
  const workspace = path.join(base, 'workspace');
  const data = path.join(workspace, '.ctmcp-data');
  const targetMarker = path.join(workspace, 'target-ran.txt');
  await Promise.all([workspace, data].map(value => mkdir(value, { recursive: true })));
  t.after(() => rm(base, { recursive: true, force: true, maxRetries: 5, retryDelay: 50 }));
  const prepared = await prepareAppContainer({
    enabled: true,
    backend: 'appcontainer',
    externalPaths: [],
    options: {}
  }, workspace, data);
  const script = [
    "const fs = require('node:fs');",
    "fs.writeFileSync(process.argv[1], 'ran');",
    'let denied = false;',
    "try { fs.writeFileSync(process.argv[2], 'forged'); } catch { denied = true; }",
    "if (!denied) process.exit(44);",
    "process.stdout.write('lease-denied');"
  ].join('');
  const launch = prepareAppContainerLaunch(
    prepared,
    workspace,
    {
      program: process.execPath,
      argv: ['-e', script, targetMarker],
      display: 'node -e lease-overlap-probe',
      shell: false
    },
    [],
    []
  );
  const protectedStateIndex = launch.args.indexOf('--protected-state');
  assert.ok(protectedStateIndex >= 0);
  const leaseAttack = path.join(path.dirname(launch.args[protectedStateIndex + 1]), 'forged-by-child');
  launch.args.push(leaseAttack);
  t.after(() => launch.cleanup().catch(() => undefined));
  const result = spawnSync(launch.program, launch.args, { env: process.env, windowsHide: true, timeout: 30_000, encoding: 'utf8' });
  assert.equal(result.status, 0, `workspace-contained dataDir launch failed: ${result.stderr}`);
  assert.equal(result.stdout, 'lease-denied');
  assert.equal(await readFile(targetMarker, 'utf8'), 'ran');
  await assert.rejects(readFile(leaseAttack, 'utf8'));
  await launch.cleanup();
  assert.deepEqual(await readdir(path.join(data, 'sandbox', 'appcontainer', 'leases')), []);
});

test('AppContainer fallback cleanup restores the exact protected DACL after helper termination', { skip: process.platform !== 'win32' || process.env.CTMCP_TEST_APPCONTAINER !== '1' }, async t => {
  const { workspace, data } = await directories(t);
  const gitDir = path.join(workspace, '.git');
  const runningMarker = path.join(workspace, 'running.txt');
  const gitPointer = 'gitdir: ../repo/.git/worktrees/linked\r\n';
  await writeFile(gitDir, gitPointer);
  const gitDaclBefore = windowsDaclSddl(gitDir);
  const prepared = await prepareAppContainer({
    enabled: true,
    backend: 'appcontainer',
    externalPaths: [],
    options: {}
  }, workspace, data);
  const script = [
    "const fs = require('node:fs');",
    "fs.writeFileSync(process.argv[1], 'running');",
    'setInterval(() => {}, 1000);'
  ].join('');
  const launch = prepareAppContainerLaunch(
    prepared,
    workspace,
    { program: process.execPath, argv: ['-e', script, runningMarker], display: 'node -e appcontainer-running-probe', shell: false },
    [],
    []
  );
  t.after(() => launch.cleanup().catch(() => undefined));
  const helper = spawn(launch.program, launch.args, { env: process.env, windowsHide: true });
  launch.onSpawn?.(helper.pid);
  const helperClosed = new Promise((resolve, reject) => {
    helper.once('error', reject);
    helper.once('close', (code, signal) => resolve({ code, signal }));
  });
  await waitForFile(runningMarker);
  assert.notEqual(windowsDaclSddl(gitDir), gitDaclBefore, 'running sandbox should hold the protected .git DACL');
  const leaseRoot = path.join(data, 'sandbox', 'appcontainer', 'leases');
  const leaseDirectory = await singleLeaseDirectory(leaseRoot);
  const leaseFiles = await readdir(leaseDirectory);
  assert.ok(leaseFiles.includes('cleanup.state'));
  assert.ok(!leaseFiles.includes('complete'));
  const cleanupStateLines = (await readFile(path.join(leaseDirectory, 'cleanup.state'), 'utf8')).split(/\r?\n/).filter(Boolean);
  const recordedGrants = cleanupStateLines
    .filter(line => line.startsWith('G\t'))
    .map(line => Buffer.from(line.slice(2), 'base64').toString('utf8'));
  assert.ok(
    recordedGrants.some(value => value.toLowerCase() === path.dirname(process.execPath).toLowerCase()),
    `runtime grant missing from cleanup journal: ${JSON.stringify(recordedGrants)}`
  );
  assert.ok(cleanupStateLines.some(line => line.startsWith('D\t.git\t')), 'original .git DACL must be journaled before protection');
  assert.equal(helper.kill(), true);
  await helperClosed;
  assert.ok(!(await readdir(leaseDirectory)).includes('complete'));
  await launch.cleanup();
  assert.equal(windowsDaclSddl(gitDir), gitDaclBefore, 'fallback cleanup must restore the exact .git DACL');
  assert.equal(await readFile(gitDir, 'utf8'), gitPointer, 'fallback cleanup must preserve the .git pointer file content');
  assert.deepEqual(await readdir(leaseRoot), []);
});

test('AppContainer Git drive aliases are exclusive, scoped and released', { skip: process.platform !== 'win32' }, async t => {
  const { workspace, data } = await directories(t);
  const nested = path.join(workspace, 'nested');
  await mkdir(nested);
  const prepared = await prepareAppContainer({ enabled: true, backend: 'appcontainer', externalPaths: [], options: {} }, workspace, data);
  const script = path.join(data, 'alias-test.ps1');
  await writeFile(script, String.raw`
param($Helper, $Workspace, $Nested, $Outside)
$ErrorActionPreference = 'Stop'
$assembly = [Reflection.Assembly]::LoadFile($Helper)
$type = $assembly.GetType('Program').GetNestedType('GitDriveAlias', [Reflection.BindingFlags]::NonPublic)
$create = $type.GetMethod('Create')
$first = $null; $second = $null
try {
  $first = $create.Invoke($null, @($Workspace))
  $second = $create.Invoke($null, @($Workspace))
  $one = $first.Cwd($Workspace); $two = $second.Cwd($Workspace)
  if ($one -eq $two) { throw 'Aliases collided' }
  if (!(Test-Path -LiteralPath $first.Cwd($Nested))) { throw 'Nested cwd missing' }
  $denied = $false
  try { $null = $first.Cwd($Outside) } catch { $denied = $true }
  if (!$denied) { throw 'Outside cwd was accepted' }
  $first.Dispose(); $first = $null
  if (Test-Path -LiteralPath $one) { throw 'First mapping survived dispose' }
  if (!(Test-Path -LiteralPath $two)) { throw 'Second mapping was removed by first owner' }
  $second.Dispose(); $second = $null
  if (Test-Path -LiteralPath $two) { throw 'Second mapping survived dispose' }
  Write-Output 'git-alias-lifecycle-ok'
} finally {
  if ($first) { $first.Dispose() }
  if ($second) { $second.Dispose() }
}
`);
  const result = spawnSync('powershell.exe', ['-NoLogo', '-NoProfile', '-NonInteractive', '-File', script, prepared.helper, workspace, nested, data], { windowsHide: true, timeout: 30_000, encoding: 'utf8' });
  assert.equal(result.status, 0, `stderr: ${result.stderr}`);
  assert.match(result.stdout, /git-alias-lifecycle-ok/);
});
