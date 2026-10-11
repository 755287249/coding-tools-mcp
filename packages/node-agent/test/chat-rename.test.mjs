import test from 'node:test';
import assert from 'node:assert/strict';
import fs from 'node:fs';
import os from 'node:os';
import path from 'node:path';
import { renameChatFileSync } from '../dist/chat/rename.js';

function fixture(t, platform = 'win32') {
  const directory = fs.mkdtempSync(path.join(os.tmpdir(), 'ctmcp-chat-rename-'));
  const source = path.join(directory, 'pending.tmp');
  const target = path.join(directory, 'chat.json');
  fs.writeFileSync(source, 'new archive');
  fs.writeFileSync(target, 'old archive');
  const originalPlatform = Object.getOwnPropertyDescriptor(process, 'platform');
  Object.defineProperty(process, 'platform', { ...originalPlatform, value: platform });
  t.after(() => {
    Object.defineProperty(process, 'platform', originalPlatform);
    fs.rmSync(directory, { recursive: true, force: true });
  });
  const pauses = [];
  t.mock.method(Atomics, 'wait', (_buffer, _index, _expected, delay) => {
    pauses.push(delay);
    return 'timed-out';
  });
  return { source, target, pauses };
}

test('Windows replacement retries sharing errors without deleting the old archive', t => {
  const { source, target, pauses } = fixture(t);
  const rename = fs.renameSync;
  const errors = ['EPERM', 'EACCES', 'EBUSY'];
  let attempts = 0;
  t.mock.method(fs, 'renameSync', (from, to) => {
    assert.equal(fs.readFileSync(target, 'utf8'), 'old archive');
    const code = errors[attempts++];
    if (code) throw Object.assign(new Error(code), { code });
    return rename(from, to);
  });
  renameChatFileSync(source, target);
  assert.equal(attempts, 4);
  assert.deepEqual(pauses, [10, 20, 40]);
  assert.equal(fs.readFileSync(target, 'utf8'), 'new archive');
  assert.equal(fs.existsSync(source), false);
});

test('persistent sharing errors have a bounded retry budget and preserve both files', t => {
  const { source, target, pauses } = fixture(t);
  const failure = Object.assign(new Error('locked'), { code: 'EPERM' });
  let attempts = 0;
  t.mock.method(fs, 'renameSync', () => { attempts++; throw failure; });
  assert.throws(() => renameChatFileSync(source, target), error => error === failure);
  assert.equal(attempts, 8);
  assert.deepEqual(pauses, [10, 20, 40, 80, 160, 160, 160]);
  assert.equal(fs.readFileSync(target, 'utf8'), 'old archive');
  assert.equal(fs.readFileSync(source, 'utf8'), 'new archive');
});

for (const [platform, code] of [['linux', 'EPERM'], ['win32', 'ENOENT']]) {
  test(platform + ' ' + code + ' propagates immediately without retry', t => {
    const { source, target, pauses } = fixture(t, platform);
    const failure = Object.assign(new Error(code), { code });
    let attempts = 0;
    t.mock.method(fs, 'renameSync', () => { attempts++; throw failure; });
    assert.throws(() => renameChatFileSync(source, target), error => error === failure);
    assert.equal(attempts, 1);
    assert.deepEqual(pauses, []);
    assert.equal(fs.readFileSync(target, 'utf8'), 'old archive');
  });
}
