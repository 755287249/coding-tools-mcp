import fs from 'node:fs';

const RETRY_DELAYS_MS = [10, 20, 40, 80, 160, 160, 160];
const retryWait = new Int32Array(new SharedArrayBuffer(4));

// Windows readers can briefly deny replacement. Keep the old destination
// intact until rename succeeds, and surface persistent or unrelated errors.
export function renameChatFileSync(source: string, target: string): void {
  for (let attempt = 0; ; attempt++) {
    try {
      fs.renameSync(source, target);
      return;
    } catch (error) {
      const code = (error as NodeJS.ErrnoException).code;
      if (process.platform !== 'win32' ||
          !['EPERM', 'EACCES', 'EBUSY'].includes(code ?? '') ||
          attempt >= RETRY_DELAYS_MS.length) {
        throw error;
      }
      Atomics.wait(retryWait, 0, 0, RETRY_DELAYS_MS[attempt]);
    }
  }
}
