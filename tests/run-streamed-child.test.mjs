import test from 'node:test';
import assert from 'node:assert/strict';
import { runCapturedStdoutWithProgress } from '../scripts/run-streamed-child.mjs';

const splitUnicode = String.raw`
const bytes = Buffer.from('对话🙂資料');
for (const part of [bytes.subarray(0, 1), bytes.subarray(1, 5), bytes.subarray(5)]) {
  process.stdout.write(part);
  process.stderr.write(part);
  await new Promise(resolve => setTimeout(resolve, 50));
}
`;

test('captured stdout and streamed stderr preserve UTF-8 across child output chunks', async () => {
  let stderr = '';
  const stdout = await runCapturedStdoutWithProgress(process.execPath, ['--input-type=module', '-e', splitUnicode], {
    quietHeartbeatMs: 0,
    stderr: { write: chunk => { stderr += chunk; } }
  });
  assert.equal(stdout, '对话🙂資料');
  assert.equal(stderr, '对话🙂資料');
});

test('failed child diagnostics preserve split UTF-8 with stderr streaming disabled', async () => {
  await assert.rejects(
    runCapturedStdoutWithProgress(process.execPath, ['--input-type=module', '-e', splitUnicode + '\nprocess.exitCode = 7;'], {
      label: 'catalog fixture', quietHeartbeatMs: 0, streamStderr: false
    }),
    { message: 'catalog fixture failed (7)\n对话🙂資料' }
  );
});
