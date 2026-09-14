import { spawn, spawnSync } from 'node:child_process';
import { existsSync, openSync, watch } from 'node:fs';
import { mkdir, readFile, writeFile } from 'node:fs/promises';
import { homedir } from 'node:os';
import path from 'node:path';
import { fileURLToPath } from 'node:url';

const scriptPath = fileURLToPath(import.meta.url);
const packageRoot = path.resolve(path.dirname(scriptPath), '..');
const repoRoot = path.resolve(packageRoot, '..', '..');
const argv = process.argv.slice(2);

function option(name) {
  const index = argv.indexOf(name);
  return index >= 0 ? argv[index + 1] : undefined;
}

function integerOption(name, fallback, minimum = 1) {
  const raw = option(name);
  if (raw === undefined) return fallback;
  const value = Number(raw);
  if (!Number.isInteger(value) || value < minimum) throw new Error(`${name} must be an integer >= ${minimum}`);
  return value;
}

const defaultDataDir = process.platform === 'win32'
  ? path.join(process.env.LOCALAPPDATA ?? homedir(), 'CodingToolsMCPNode')
  : path.join(homedir(), '.coding-tools-mcp-node');
const dataDir = path.resolve(process.env.CTMCP_DATA_DIR ?? defaultDataDir);
const configPath = path.resolve(option('--config') ?? process.env.CTMCP_CONFIG_FILE ?? path.join(dataDir, 'agent.json'));
const statusPath = path.resolve(option('--status') ?? path.join(dataDir, 'dev-server-status.json'));
const agentLogPath = path.resolve(option('--agent-log') ?? path.join(dataDir, 'dev-server-agent.log'));
const debounceMs = integerOption('--debounce-ms', 350, 50);
const healthTimeoutMs = integerOption('--health-timeout-ms', 30_000, 1_000);
const once = argv.includes('--once');
const buildOnly = argv.includes('--build-only');

let managedChild;
let replacing = false;
let shuttingDown = false;
let cycleRunning = false;
let rerunRequested = false;
let debounceTimer;

async function writeStatus(state, details = {}) {
  await mkdir(path.dirname(statusPath), { recursive: true });
  await writeFile(statusPath, `${JSON.stringify({
    state,
    supervisorPid: process.pid,
    agentPid: managedChild?.pid ?? null,
    configPath,
    packageRoot,
    updatedAt: new Date().toISOString(),
    ...details
  }, null, 2)}\n`);
}

function run(command, args, cwd = packageRoot) {
  return new Promise((resolve, reject) => {
    const executable = process.platform === 'win32' && command === 'pnpm' ? 'cmd.exe' : command;
    const commandArgs = process.platform === 'win32' && command === 'pnpm'
      ? ['/d', '/s', '/c', `pnpm ${args.join(' ')}`]
      : args;
    const child = spawn(executable, commandArgs, {
      cwd,
      env: process.env,
      shell: false,
      stdio: 'inherit',
      windowsHide: true
    });
    child.once('error', reject);
    child.once('exit', (code, signal) => resolve({ code: code ?? 1, signal }));
  });
}

async function buildServer() {
  console.log(`[dev-server] building Node Agent server (${new Date().toLocaleTimeString()})`);
  const result = await run('pnpm', ['run', 'build:server']);
  if (result.code !== 0) {
    console.error(`[dev-server] build failed with exit code ${result.code}; keeping the current Agent running.`);
    await writeStatus('build-failed', { exitCode: result.code });
    return false;
  }
  return true;
}

async function readJson(file) {
  return JSON.parse(await readFile(file, 'utf8'));
}

function probeHost(host) {
  if (!host || host === '0.0.0.0') return '127.0.0.1';
  if (host === '::') return '::1';
  return host;
}

function healthUrl(endpoint) {
  const host = endpoint.host.includes(':') ? `[${endpoint.host}]` : endpoint.host;
  return `http://${host}:${endpoint.port}/health`;
}

async function loadEndpoints() {
  const registryPath = path.join(path.dirname(configPath), 'workspace-profiles.json');
  let configFiles = [configPath];
  if (existsSync(registryPath)) {
    const registry = await readJson(registryPath);
    if (Array.isArray(registry.workspaces) && registry.workspaces.length) {
      configFiles = registry.workspaces.map(entry => path.resolve(path.dirname(registryPath), String(entry.configPath)));
    }
  }
  const endpoints = [];
  const seen = new Set();
  for (const file of configFiles) {
    const document = await readJson(file);
    const host = probeHost(String(process.env.CTMCP_HOST ?? document.host ?? '127.0.0.1'));
    const port = Number(process.env.CTMCP_PORT ?? document.port ?? 3789);
    if (!Number.isInteger(port) || port < 1 || port > 65_535) throw new Error(`Invalid Node Agent port in ${file}`);
    const key = `${host}:${port}`;
    if (!seen.has(key)) {
      seen.add(key);
      endpoints.push({ host, port, configPath: file });
    }
  }
  return endpoints;
}

async function healthy(endpoint) {
  try {
    const response = await fetch(healthUrl(endpoint), { signal: AbortSignal.timeout(1_500) });
    if (!response.ok) return false;
    const body = await response.json();
    return body?.ok === true && body?.server === 'coding-tools-mcp-node';
  } catch {
    return false;
  }
}

async function waitHealthy(endpoints) {
  const deadline = Date.now() + healthTimeoutMs;
  const pending = new Map(endpoints.map(endpoint => [`${endpoint.host}:${endpoint.port}`, endpoint]));
  while (pending.size && Date.now() < deadline) {
    for (const [key, endpoint] of pending) {
      if (await healthy(endpoint)) pending.delete(key);
    }
    if (pending.size) await new Promise(resolve => setTimeout(resolve, 250));
  }
  if (pending.size) {
    throw new Error(`Node Agent health timeout: ${[...pending.values()].map(healthUrl).join(', ')}`);
  }
}

function listenerPids(ports) {
  if (process.platform !== 'win32' || ports.length === 0) return [];
  const portList = ports.join(',');
  const command = `$ports=@(${portList}); @(Get-NetTCPConnection -State Listen -ErrorAction SilentlyContinue | Where-Object { $ports -contains $_.LocalPort } | Select-Object -ExpandProperty OwningProcess -Unique) | ConvertTo-Json -Compress`;
  const result = spawnSync('powershell.exe', ['-NoProfile', '-Command', command], { encoding: 'utf8', windowsHide: true });
  if (result.status !== 0 || !result.stdout.trim()) return [];
  const parsed = JSON.parse(result.stdout.trim());
  return (Array.isArray(parsed) ? parsed : [parsed]).map(Number).filter(pid => Number.isInteger(pid) && pid > 0 && pid !== process.pid);
}

function killTree(pid) {
  if (!pid) return;
  if (process.platform === 'win32') {
    spawnSync('taskkill.exe', ['/PID', String(pid), '/T', '/F'], { stdio: 'ignore', windowsHide: true });
    return;
  }
  try { process.kill(pid, 'SIGTERM'); } catch { /* already exited */ }
}

async function stopCurrentAgent(endpoints) {
  replacing = true;
  if (managedChild?.pid) {
    killTree(managedChild.pid);
    managedChild = undefined;
  }
  for (const pid of listenerPids(endpoints.map(endpoint => endpoint.port))) killTree(pid);
  await new Promise(resolve => setTimeout(resolve, 300));
}

async function startAgent() {
  await mkdir(path.dirname(agentLogPath), { recursive: true });
  const logFd = openSync(agentLogPath, 'a');
  managedChild = spawn(process.execPath, [path.join(packageRoot, 'dist', 'cli.js'), '--config', configPath, '--restart-supervised'], {
    cwd: repoRoot,
    env: { ...process.env, CTMCP_RESTART_SUPERVISOR: 'active-v1' },
    stdio: ['ignore', logFd, logFd],
    windowsHide: true
  });
  const child = managedChild;
  child.once('exit', code => {
    if (managedChild === child) managedChild = undefined;
    if (shuttingDown || replacing) return;
    if (code === 75) {
      console.log('[dev-server] management UI requested restart; restarting the current build.');
      void startAgent().catch(error => console.error(`[dev-server] restart failed: ${error.message}`));
      return;
    }
    console.error(`[dev-server] Agent exited with code ${code ?? 'unknown'}; waiting for the next source change.`);
    void writeStatus('agent-exited', { exitCode: code ?? null }).catch(() => undefined);
  });
  replacing = false;
  return child.pid;
}

async function performCycle(reason) {
  await writeStatus('building', { reason });
  if (!(await buildServer())) return false;
  if (buildOnly) {
    console.log('[dev-server] build-only cycle complete.');
    await writeStatus('built', { reason });
    return true;
  }

  const endpoints = await loadEndpoints();
  console.log(`[dev-server] build succeeded; restarting ${endpoints.length} workspace endpoint(s).`);
  await writeStatus('restarting', { reason, endpoints });
  await stopCurrentAgent(endpoints);
  const agentPid = await startAgent();
  try {
    await waitHealthy(endpoints);
    console.log(`[dev-server] Agent healthy (pid=${agentPid}).`);
    await writeStatus('healthy', { reason, endpoints, agentPid });
    return true;
  } catch (error) {
    console.error(`[dev-server] ${error.message}`);
    await writeStatus('health-failed', { reason, endpoints, error: error.message, agentPid });
    return false;
  }
}

async function runCycle(reason) {
  if (cycleRunning) {
    rerunRequested = true;
    return true;
  }
  cycleRunning = true;
  let success = true;
  try {
    let nextReason = reason;
    do {
      rerunRequested = false;
      success = (await performCycle(nextReason)) && success;
      nextReason = 'changes queued during build';
    } while (rerunRequested);
  } catch (error) {
    success = false;
    console.error(`[dev-server] cycle failed: ${error.stack ?? error.message}`);
    await writeStatus('cycle-failed', { reason, error: error.message }).catch(() => undefined);
  } finally {
    cycleRunning = false;
  }
  return success;
}

function schedule(reason) {
  clearTimeout(debounceTimer);
  debounceTimer = setTimeout(() => void runCycle(reason), debounceMs);
}

function startWatchers() {
  const watchers = [];
  const add = (target, recursive, filter = () => true) => {
    if (!existsSync(target)) return;
    watchers.push(watch(target, { recursive }, (_event, filename) => {
      const name = filename ? String(filename).replaceAll('\\', '/') : '';
      if (filter(name)) schedule(`${path.relative(packageRoot, target)}:${name || 'change'}`);
    }));
  };
  add(path.join(packageRoot, 'src'), true);
  add(path.join(packageRoot, 'sandbox'), true);
  const buildScripts = new Set([
    'build-appcontainer-helper.mjs', 'clean-server-dist.mjs', 'copy-sandbox-assets.mjs', 'write-build-info.mjs'
  ]);
  add(path.join(packageRoot, 'scripts'), false, name => buildScripts.has(name));
  const packageFiles = new Set(['package.json', 'tsconfig.json']);
  add(packageRoot, false, name => packageFiles.has(name));
  const workspaceFiles = new Set(['pnpm-lock.yaml', 'pnpm-workspace.yaml']);
  add(repoRoot, false, name => workspaceFiles.has(name));
  return watchers;
}

async function shutdown() {
  if (shuttingDown) return;
  shuttingDown = true;
  clearTimeout(debounceTimer);
  if (managedChild?.pid) killTree(managedChild.pid);
  await writeStatus('stopped').catch(() => undefined);
}

process.on('SIGINT', () => void shutdown().finally(() => process.exit(0)));
process.on('SIGTERM', () => void shutdown().finally(() => process.exit(0)));

const initialOk = await runCycle('initial');
if (once) {
  if (!buildOnly && managedChild) managedChild.unref();
  process.exitCode = initialOk ? 0 : 1;
} else {
  const watchers = startWatchers();
  console.log(`[dev-server] watching ${watchers.length} source roots; debounce=${debounceMs}ms`);
  await writeStatus('watching', { watchers: watchers.length });
}
