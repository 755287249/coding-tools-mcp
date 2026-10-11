import { spawn } from 'node:child_process';

const DEFAULT_ERROR_TAIL_BYTES = 64 * 1024;

function appendTail(current, chunk, maxBytes) {
  const combined = current + chunk;
  const bytes = Buffer.from(combined);
  if (bytes.length <= maxBytes) return combined;
  return bytes.subarray(bytes.length - maxBytes).toString();
}

export function runCapturedStdoutWithProgress(program, args, {
  cwd,
  label = program,
  quietHeartbeatMs = 15_000,
  stderr = process.stderr,
  streamStderr = true,
  errorTailBytes = DEFAULT_ERROR_TAIL_BYTES,
  windowsHide = true
} = {}) {
  return new Promise((resolve, reject) => {
    const child = spawn(program, args, {
      cwd,
      windowsHide,
      stdio: ['ignore', 'pipe', 'pipe']
    });
    // A pipe chunk can end inside a multibyte character. Decode per stream.
    child.stdout.setEncoding('utf8');
    child.stderr.setEncoding('utf8');
    let stdout = '';
    let stderrTail = '';
    let settled = false;
    let lastVisibleAt = Date.now();
    const startedAt = lastVisibleAt;
    const heartbeatIntervalMs = Number.isFinite(quietHeartbeatMs) && quietHeartbeatMs > 0
      ? Math.max(10, Math.floor(quietHeartbeatMs))
      : 0;

    const heartbeat = heartbeatIntervalMs > 0
      ? setInterval(() => {
          const now = Date.now();
          if (now - lastVisibleAt < heartbeatIntervalMs) return;
          const elapsedSeconds = Math.max(1, Math.round((now - startedAt) / 1000));
          stderr.write(`[${label}] still running (${elapsedSeconds}s elapsed)\n`);
          lastVisibleAt = now;
        }, Math.min(heartbeatIntervalMs, 1_000))
      : undefined;
    heartbeat?.unref?.();

    const cleanup = () => {
      if (heartbeat) clearInterval(heartbeat);
    };
    const rejectOnce = error => {
      if (settled) return;
      settled = true;
      cleanup();
      reject(error);
    };

    child.stdout.on('data', chunk => {
      stdout += chunk.toString();
    });
    child.stderr.on('data', chunk => {
      const text = chunk.toString();
      stderrTail = appendTail(stderrTail, text, errorTailBytes);
      if (streamStderr) {
        lastVisibleAt = Date.now();
        stderr.write(text);
      }
    });
    child.once('error', error => {
      const detail = stderrTail.trim();
      rejectOnce(new Error(`${label} failed to start: ${error.message}${detail ? `\n${detail}` : ''}`));
    });
    child.once('close', code => {
      if (settled) return;
      settled = true;
      cleanup();
      if (code === 0) {
        resolve(stdout);
        return;
      }
      const detail = stderrTail.trim() || stdout.trim();
      reject(new Error(`${label} failed (${code ?? 'unknown'})${detail ? `\n${detail}` : ''}`));
    });
  });
}