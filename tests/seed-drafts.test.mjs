import test from 'node:test';
import assert from 'node:assert/strict';
import {createSeedDraftStore,seedDraftKey} from '../src/lib/chat/seed-drafts.ts';
const draft={account:'account 1',repo:'1412803600',branch:'main',count:3,endpoint:'https://example.com/mcp'};
const storage=()=>{const map=new Map();return {getItem:k=>map.get(k)??null,setItem:(k,v)=>map.set(k,v)};};
test('seed configuration survives reload, isolates projects and never persists batch tickets',()=>{
 const disk=storage(),store=createSeedDraftStore(()=>disk),key=seedDraftKey('w','a'),other=seedDraftKey('w','b');
 store.save(key,{...draft,ticket:'secret',bundle:'secret'});store.save(other,{...draft,repo:'other'});
 assert.deepEqual(createSeedDraftStore(()=>disk).load(key),draft);assert.equal(store.load(other).repo,'other');
 const text=JSON.stringify({seeds:[{ticket:'secret',expires_at:Date.now()+10000}]});store.setBatch(key,text);
 assert.equal(store.batch(key),text);assert.equal(store.batch(other),'');assert.ok(!disk.getItem(key).includes('secret'));
 assert.equal(createSeedDraftStore(()=>disk).batch(key),'');assert.equal(store.batch(key,Date.now()+11000),'');
 store.setBatch(key,text);store.setBatch(key,'');assert.equal(store.batch(key),'');
});
test('blocked storage retains route drafts and reports failed persistence; invalid saved state is ignored',()=>{
 const store=createSeedDraftStore(()=>{throw Error('disabled')});assert.equal(store.save('a',draft),false);assert.deepEqual(store.load('a'),draft);
 const disk=storage();disk.setItem('bad','{');disk.setItem('null','null');
 assert.equal(createSeedDraftStore(()=>disk).load('bad').branch,'main');assert.equal(createSeedDraftStore(()=>disk).load('null').count,3);
});
