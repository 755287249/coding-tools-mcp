import { spawn } from 'node:child_process';
import { createHash, randomUUID } from 'node:crypto';
import { mkdir, open, readFile, rename, stat, unlink, utimes } from 'node:fs/promises';
import path from 'node:path';
import { fileURLToPath } from 'node:url';

const scriptPath = fileURLToPath(import.meta.url);
const packageRoot = path.resolve(path.dirname(scriptPath), '..');
const normalizedPackageRoot = process.platform === 'win32' ? packageRoot.toLowerCase() : packageRoot;
const lockKey = createHash('sha256').update(normalizedPackageRoot).digest('hex').slice(0, 24);
const lockRoot = path.resolve(process.env.CTMCP_DIST_LOCK_ROOT ?? path.join(packageRoot, '.ctmcp-dist-locks'));
const lockPath = path.join(lockRoot, `${lockKey}.lock`);
const inheritedLock = process.env.CTMCP_DIST_LOCK_KEY === lockKey
  && typeof process.env.CTMCP_DIST_LOCK_TOKEN === 'string'
  && process.env.CTMCP_DIST_LOCK_TOKEN.length > 0;

function duration(value, fallback, minimum) {
  const parsed = Number(value);
  return Number.isFinite(parsed) ? Math.max(minimum, parsed) : fallback;
}

const lockTimeoutMs = duration(process.env.CTMCP_DIST_LOCK_TIMEOUT_MS, 30 * 60_000, 1_000);
const staleLockMs = duration(process.env.CTMCP_DIST_LOCK_STALE_MS, 10 * 60_000, 250);
const heartbeatIntervalMs = Math.max(100, Math.min(5_000, Math.floor(staleLockMs / 4)));
const command = process.argv.slice(2);
if (command[0] === '--') command.shift();
if (command.length === 0) throw new Error('with-dist-lock.mjs requires a command to run');

const sleep = ms => new Promise(resolve => setTimeout(resolve, ms));

async function heartbeatAgeMs(file) {
  try {
    const info = await stat(file);
    return Math.max(0, Date.now() - info.mtimeMs);
  } catch (error) {
    if (error?.code === 'ENOENT') return Number.POSITIVE_INFINITY;
    throw error;
  }
}

async function recoverStaleLock() {
  if (await heartbeatAgeMs(lockPath) < staleLockMs) return false;
  const quarantined = `${lockPath}.stale-${randomUUID()}`;
  try {
    await rename(lockPath, quarantined);
  } catch (error) {
    if (['ENOENT', 'EACCES', 'EPERM'].includes(error?.code)) return false;
    throw error;
  }
  if (await heartbeatAgeMs(quarantined) < staleLockMs) {
    try { await rename(quarantined, lockPath); } catch { /* another contender won the race */ }
    return false;
  }
  await unlink(quarantined).catch(error => {
    if (error?.code !== 'ENOENT') throw error;
  });
  return true;
}

async function acquireLock() {
  await mkdir(lockRoot, { recursive: true });
  const started = Date.now();
  let announcedWait = false;
  for (;;) {
    const token = randomUUID();
    try {
      const handle = await open(lockPath, 'wx');
      const now = new Date();
      try {
        await handle.writeFile(`${JSON.stringify({ pid: process.pid, token, startedAt: now.toISOString() })}\n`, 'utf8');
      } finally {
        await handle.close();
      }
      const heartbeat = setInterval(() => {
        const next = new Date();
        void utimes(lockPath, next, next).catch(() => undefined);
      }, heartbeatIntervalMs);
      heartbeat.unref();
      return { token, heartbeat };
    } catch (error) {
      if (error?.code !== 'EEXIST') throw error;
      if (await recoverStaleLock()) continue;
      if (!announcedWait && Date.now() - started >= 500) {
        announcedWait = true;
        console.error('Waiting for another Node Agent build/test to release dist...');
      }
      if (Date.now() - started >= lockTimeoutMs) {
        throw new Error(`Timed out waiting for Node Agent dist lock after ${lockTimeoutMs}ms`);
      }
      await sleep(100);
    }
  }
}

async function closeLock(lock) {
  if (!lock) return;
  clearInterval(lock.heartbeat);
  try {
    const owner = JSON.parse(await readFile(lockPath, 'utf8'));
    if (owner?.token !== lock.token) return;
  } catch (error) {
    if (error?.code === 'ENOENT') return;
    throw error;
  }
  await unlink(lockPath).catch(error => {
    if (error?.code !== 'ENOENT') throw error;
  });
}

function runCommand(env) {
  return new Promise((resolve, reject) => {
    const packageManagerExecPath = String(env.npm_execpath ?? '').trim();
    const packageManagerCommand = ['npm', 'pnpm'].includes(command[0].toLowerCase())
      && packageManagerExecPath.length > 0;
    const packageManagerExt = path.extname(packageManagerExecPath).toLowerCase();
    const packageManagerIsNodeScript = ['.js', '.cjs', '.mjs'].includes(packageManagerExt);
    const executable = packageManagerCommand
      ? (packageManagerIsNodeScript ? process.execPath : packageManagerExecPath)
      : command[0];
    const args = packageManagerCommand && packageManagerIsNodeScript
      ? [packageManagerExecPath, ...command.slice(1)]
      : command.slice(1);
    const child = spawn(executable, args, {
      cwd: process.cwd(),
      env,
      shell: false,
      stdio: 'inherit',
      windowsHide: false
    });
    child.once('error', reject);
    child.once('exit', (code, signal) => resolve({ code, signal }));
  });
}

let lock;
try {
  const env = { ...process.env };
  if (!inheritedLock) {
    lock = await acquireLock();
    env.CTMCP_DIST_LOCK_KEY = lockKey;
    env.CTMCP_DIST_LOCK_TOKEN = lock.token;
  }
  const { code, signal } = await runCommand(env);
  if (signal) {
    console.error(`Command terminated by ${signal}`);
    process.exitCode = 1;
  } else {
    process.exitCode = code ?? 1;
  }
} finally {
  await closeLock(lock);
}
