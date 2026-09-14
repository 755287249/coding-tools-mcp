import { closeSync, openSync, readFileSync, renameSync, writeFileSync } from 'node:fs';
import { spawn, spawnSync } from 'node:child_process';
import path from 'node:path';
import { fileURLToPath } from 'node:url';

interface DurableWorkerManifest {
  version: 1;
  program: string;
  args: string[];
  cwd: string;
  shell: boolean;
  windowsVerbatimArguments?: boolean;
  deadlineMs: number;
  stdoutPath: string;
  stderrPath: string;
  runtimePath: string;
  resultPath: string;
}

function atomicWriteJson(target: string, value: unknown): void {
  const temp = `${target}.${process.pid}.tmp`;
  writeFileSync(temp, `${JSON.stringify(value)}\n`, { encoding: 'utf8', mode: 0o600 });
  renameSync(temp, target);
}

function terminateTarget(pid: number): void {
  if (process.platform === 'win32') {
    spawnSync('taskkill.exe', ['/pid', String(pid), '/t', '/f'], { windowsHide: true, stdio: 'ignore' });
    return;
  }
  try { process.kill(-pid, 'SIGKILL'); }
  catch { try { process.kill(pid, 'SIGKILL'); } catch { /* already exited */ } }
}

async function run(manifestPath: string): Promise<void> {
  const manifest = JSON.parse(readFileSync(manifestPath, 'utf8')) as DurableWorkerManifest;
  if (manifest.version !== 1) throw new Error(`Unsupported durable worker manifest version: ${manifest.version}`);
  const stdoutFd = openSync(manifest.stdoutPath, 'a', 0o600);
  const stderrFd = openSync(manifest.stderrPath, 'a', 0o600);
  let timeout: NodeJS.Timeout | undefined;
  let terminationReason = 'exited';
  try {
    const child = spawn(manifest.program, manifest.args, {
      cwd: manifest.cwd,
      env: process.env,
      windowsHide: true,
      windowsVerbatimArguments: manifest.windowsVerbatimArguments,
      detached: process.platform !== 'win32',
      shell: manifest.shell,
      stdio: ['ignore', stdoutFd, stderrFd]
    });
    atomicWriteJson(manifest.runtimePath, {
      version: 1,
      workerPid: process.pid,
      processPid: child.pid ?? null,
      startedAt: Date.now()
    });
    timeout = setTimeout(() => {
      if (!child.pid) return;
      terminationReason = 'process_timeout';
      terminateTarget(child.pid);
    }, Math.max(1, manifest.deadlineMs - Date.now()));
    timeout.unref();
    const result = await new Promise<{ exitCode: number | null; signal: string | null; error?: string }>(resolve => {
      let settled = false;
      const finish = (value: { exitCode: number | null; signal: string | null; error?: string }) => {
        if (settled) return;
        settled = true;
        resolve(value);
      };
      child.once('error', error => {
        terminationReason = 'crashed';
        finish({ exitCode: null, signal: null, error: error.message });
      });
      child.once('close', (code, signal) => finish({ exitCode: code, signal }));
    });
    if (timeout) clearTimeout(timeout);
    atomicWriteJson(manifest.resultPath, {
      version: 1,
      exitCode: result.exitCode,
      signal: result.signal,
      terminationReason,
      error: result.error ?? null,
      endedAt: Date.now()
    });
  } finally {
    if (timeout) clearTimeout(timeout);
    closeSync(stdoutFd);
    closeSync(stderrFd);
  }
}

const invokedPath = process.argv[1] ? path.resolve(process.argv[1]) : '';
if (invokedPath === path.resolve(fileURLToPath(import.meta.url))) {
  const manifestPath = process.argv[2];
  if (!manifestPath) {
    process.exitCode = 2;
  } else {
    void run(manifestPath).catch(error => {
      try {
        const manifest = JSON.parse(readFileSync(manifestPath, 'utf8')) as DurableWorkerManifest;
        atomicWriteJson(manifest.resultPath, {
          version: 1,
          exitCode: null,
          signal: null,
          terminationReason: 'crashed',
          error: error instanceof Error ? error.message : String(error),
          endedAt: Date.now()
        });
      } catch { /* no usable manifest */ }
      process.exitCode = 1;
    });
  }
}
