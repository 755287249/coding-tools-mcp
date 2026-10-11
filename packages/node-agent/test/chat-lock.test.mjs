import test from 'node:test';
import assert from 'node:assert/strict';
import fs from 'node:fs';
import { syncBuiltinESMExports } from 'node:module';
import os from 'node:os';
import path from 'node:path';
import { chatUi } from '../dist/chat/store.js';

function fixture(t, platform = 'win32') {
  const root = fs.mkdtempSync(path.join(os.tmpdir(), 'chat-lock-retry-'));
  const session = chatUi(root, { action: 'create', title: 'before retry' }).session;
  const record = path.join(root, 'docs/chat-sessions', session.id + '.json');
  const lock = path.join(root, 'docs/chat-sessions/.lock');
  const original = fs.readFileSync(record, 'utf8');
  const descriptor = Object.getOwnPropertyDescriptor(process, 'platform');
  Object.defineProperty(process, 'platform', { ...descriptor, value: platform });
  let elapsed = 0;
  const started = Date.now();
  t.mock.method(Date, 'now', () => started + elapsed);
  t.mock.method(Atomics, 'wait', (_buffer, _index, _expected, delay) => {
    elapsed += delay;
    return 'timed-out';
  });
  t.after(() => {
    t.mock.restoreAll();
    syncBuiltinESMExports();
    Object.defineProperty(process, 'platform', descriptor);
    fs.rmSync(root, { recursive: true, force: true });
  });
  const mkdir = fs.mkdirSync;
  function intercept(createLock) {
    t.mock.method(fs, 'mkdirSync', (target, options) => target === lock ? createLock(() => mkdir(target, options)) : mkdir(target, options));
    syncBuiltinESMExports();
  }
  return {
    root, record, lock, original, intercept,
    elapsed: () => elapsed,
    write: () => chatUi(root, { action: 'rename', chat_id: session.id, title: 'after retry' })
  };
}

test('Windows lock recreation retries deletion-pending errors before writing', t => {
  const f = fixture(t);
  const failures = ['EPERM', 'EACCES'];
  f.intercept(create => {
    assert.equal(fs.readFileSync(f.record, 'utf8'), f.original);
    const code = failures.shift();
    if (code) throw Object.assign(new Error('directory is delete-pending'), { code });
    return create();
  });
  f.write();
  assert.equal(JSON.parse(fs.readFileSync(f.record, 'utf8')).title, 'after retry');
  assert.ok(f.elapsed() > 0);
  assert.equal(fs.existsSync(f.lock), false);
});

test('persistent Windows lock permission errors remain bounded and preserve the archive', t => {
  const f = fixture(t);
  const failure = Object.assign(new Error('permission denied'), { code: 'EPERM' });
  f.intercept(() => { throw failure; });
  assert.throws(f.write, error => error === failure);
  assert.ok(f.elapsed() >= 2000 && f.elapsed() <= 2020);
  assert.equal(fs.readFileSync(f.record, 'utf8'), f.original);
  assert.equal(fs.existsSync(f.lock), false);
});

test('waiting for a lock never removes another writer\'s lock', t => {
  const f = fixture(t);
  fs.mkdirSync(f.lock);
  assert.throws(f.write, /Chat storage is busy/);
  assert.ok(f.elapsed() >= 2000 && f.elapsed() <= 2020);
  assert.equal(fs.existsSync(f.lock), true);
  assert.equal(fs.readFileSync(f.record, 'utf8'), f.original);
});

for (const [platform, code] of [['linux', 'EPERM'], ['win32', 'ENOENT']]) {
  test(`${platform} ${code} fails immediately without mutating chat storage`, t => {
    const f = fixture(t, platform);
    const failure = Object.assign(new Error(code), { code });
    f.intercept(() => { throw failure; });
    assert.throws(f.write, error => error === failure);
    assert.equal(f.elapsed(), 0);
    assert.equal(fs.readFileSync(f.record, 'utf8'), f.original);
    assert.equal(fs.existsSync(f.lock), false);
  });
}
