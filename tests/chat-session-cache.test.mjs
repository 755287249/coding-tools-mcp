import assert from 'node:assert/strict';
import test from 'node:test';
import { createSessionCache, sessionCacheKey } from '../src/lib/chat/session-cache.ts';
const session = (id, text = id) => ({ id, title:id, messages:[{id:'message',role:'user',text,created_at:1}],created_at:1,updated_at:1,closed:false,status:'offline',archive_path:`${id}.md` });

test('cached conversation history is isolated by workspace, folder and chat', () => {
  const cache = createSessionCache();
  const key = sessionCacheKey('workspace', 'folder', 'a');
  cache.put(key, session('a', 'history A'));
  assert.equal(cache.get(key).messages[0].text, 'history A');
  assert.equal(cache.get(sessionCacheKey('other', 'folder', 'a')), null);
  assert.equal(cache.get(sessionCacheKey('workspace', 'other', 'a')), null);
  assert.equal(cache.get(sessionCacheKey('workspace', 'folder', 'b')), null);
  assert.notEqual(sessionCacheKey('a:b', 'c', 'd'), sessionCacheKey('a', 'b:c', 'd'));
});

test('cache keeps recently visited conversations and replaces snapshots after new messages', () => {
  const cache = createSessionCache(2);
  cache.put('a', session('a')); cache.put('b', session('b'));
  cache.get('a'); cache.put('c', session('c'));
  assert.equal(cache.get('b'), null);
  assert.ok(cache.get('a'));
  cache.put('a', session('a', 'new reply'));
  assert.equal(cache.get('a').messages[0].text, 'new reply');
  cache.remove('a'); assert.equal(cache.get('a'), null);
});

test('cache bounds retained history size and drops a replaced oversized snapshot', () => {
  const a = session('a'), b = session('b');
  const cache = createSessionCache(10, JSON.stringify(a).length + 10);
  cache.put('a', a); cache.put('b', b);
  assert.equal(cache.get('a'), null);
  assert.ok(cache.get('b'));
  cache.put('b', session('b', 'x'.repeat(1000)));
  assert.equal(cache.get('b'), null);
  cache.put('a', a); assert.ok(cache.get('a'));
});
