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

const {createSessionReader,chatSessionReader}=await import('../src/lib/chat/session-cache.ts');
const {clearChatCaches}=await import('../src/lib/chat/cache-lifecycle.js');
const deferred=()=>{let resolve;const promise=new Promise(r=>resolve=r);return {promise,resolve}};

test('concurrent panel/list reads coalesce while reopened panels retain snapshots',async()=>{
 const reader=createSessionReader(),d=deferred();let calls=0;
 const load=()=>{calls++;return d.promise};
 const a=reader.request('w','f',{action:'read',chat_id:'a'},load);
 const b=reader.request('w','f',{action:'read',chat_id:'a'},load);
 await Promise.resolve();assert.equal(calls,1);d.resolve({session:session('a')});await Promise.all([a,b]);
 assert.equal(reader.snapshots.get(sessionCacheKey('w','f','a')).id,'a');
 let lists=0;const list=async()=>{lists++;return {sessions:[session('a')]}};
 await reader.request('w','f',{action:'list'},list);await reader.request('w','f',{action:'list'},list);assert.equal(lists,1);
 await reader.request('w','other',{action:'list'},list);assert.equal(lists,2);
});

test('mutation fences stale reads, refreshes snapshot and invalidates list without replaying writes',async()=>{
 const reader=createSessionReader(),d=deferred();let writes=0,lists=0;
 const reading=reader.request('w','f',{action:'read',chat_id:'a'},()=>d.promise);
 const rejected=assert.rejects(reading,/changed/);
 const list=()=>reader.request('w','f',{action:'list'},async()=>{lists++;return {sessions:[]}});
 await list();
 await reader.request('w','f',{action:'send',chat_id:'a'},async()=>{writes++;return {session:session('a','new reply')}});
 d.resolve({session:session('a','old')});await rejected;
 assert.equal(reader.snapshots.get(sessionCacheKey('w','f','a')).messages[0].text,'new reply');
 await list();assert.equal(lists,2);assert.equal(writes,1);
 await reader.request('w','f',{action:'delete',chat_id:'a'},async()=>({deleted:true}));assert.equal(reader.snapshots.get(sessionCacheKey('w','f','a')),null);
});

test('failed reads retry and late read/write completions cannot repopulate a logged-out cache',async()=>{
 const reader=createSessionReader();
 await assert.rejects(reader.request('w','f',{action:'read',chat_id:'a'},async()=>{throw Error('network')}));
 await reader.request('w','f',{action:'read',chat_id:'a'},async()=>({session:session('a')}));
 for(const action of ['read','send']){
  const d=deferred(),pending=reader.request('w','f',{action,chat_id:'a'},()=>d.promise),rejected=assert.rejects(pending,/changed/);
  await Promise.resolve();reader.clear();d.resolve({session:session('a')});await rejected;
  assert.equal(reader.snapshots.get(sessionCacheKey('w','f','a')),null);
 }
 chatSessionReader.snapshots.put('test',session('a'));clearChatCaches();assert.equal(chatSessionReader.snapshots.get('test'),null);
});
