import assert from 'node:assert/strict';
import test from 'node:test';
import { createDraftStore, chatDraftKey, conversationAccent } from '../src/lib/chat/drafts.ts';
const file={id:'f1',name:'test.png',path:'docs/test.png',mime:'image/png',sha256:'test',size:20};
const draft={text:' draft A ',attachments:[file],retry:{id:'send-1',text:'draft A',attachmentKey:'f1'}};
function storage() { const map=new Map();return {getItem:k=>map.get(k)??null,setItem:(k,v)=>map.set(k,v),removeItem:k=>map.delete(k)}; }

test('drafts retain text, uploaded files and retry identity across routes and reloads with isolated scope',()=>{
  const disk=storage(), first=createDraftStore(()=>disk), a=chatDraftKey('ws','folder','a'),b=chatDraftKey('ws','folder','b');
  first.save(a,draft); first.save(b,{text:'B',attachments:[],retry:null});
  assert.deepEqual(first.load(a).draft,draft);
  assert.equal(first.load(b).draft.text,'B');
  const reload=createDraftStore(()=>disk);
  assert.deepEqual(reload.load(a).draft,draft);
  assert.equal(reload.load(chatDraftKey('other','folder','a')).draft.text,'');
  assert.notEqual(chatDraftKey('a:b','c','d'),chatDraftKey('a','b:c','d'));
});

test('successful send clears only its matching snapshot, leaving a newer edit or another chat untouched',()=>{
  const disk=storage(),store=createDraftStore(()=>disk);
  store.save('a',draft);store.save('b',{...draft,text:'other'});
  assert.equal(store.clearSent('a',{...draft.retry,id:'wrong'}).cleared,false);
  store.save('a',{...draft,text:'edited while response pending'});
  assert.equal(store.clearSent('a',draft.retry).cleared,false);
  store.save('a',draft);
  assert.equal(store.clearSent('a',draft.retry).cleared,true);
  assert.equal(createDraftStore(()=>disk).load('a').draft.text,'');
  assert.equal(store.load('b').draft.text,'other');
});

test('unavailable storage retains memory drafts without throwing and reports the persistence limit',()=>{
  const store=createDraftStore(()=>{throw Error('storage disabled')});
  assert.equal(store.save('a',draft),false);
  assert.deepEqual(store.load('a'),{draft,persisted:false});
  assert.equal(store.clearSent('a',draft.retry).cleared,true);
  assert.equal(store.load('a').draft.text,'');
});

test('corrupt storage is bounded and conversation accents are stable',()=>{
  const disk=storage();disk.setItem('bad','{broken');
  assert.equal(createDraftStore(()=>disk).load('bad').draft.text,'');
  disk.setItem('bad',JSON.stringify({text:'x'.repeat(32001),attachments:[]}));
  assert.equal(createDraftStore(()=>disk).load('bad').draft.text,'');
  assert.equal(conversationAccent('a'),conversationAccent('a'));
  assert.notEqual(conversationAccent('a'),conversationAccent('b'));
  assert.match(conversationAccent('a'),/^hsl\(\d+ 34% 55%\)$/);
});
