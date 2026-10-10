import assert from 'node:assert/strict';
import test from 'node:test';
import { createDraftStore, chatDraftKey, conversationAccent } from '../src/lib/chat/drafts.ts';
const file={id:'f1',name:'test.png',path:'docs/test.png',mime:'image/png',sha256:'test',size:20};
const draft={text:' draft A ',attachments:[file],retry:{id:'send-1',text:' draft A ',attachmentKey:'f1'}};
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

test('corrupt storage is ignored, long drafts are preserved and conversation accents are stable',()=>{
  const disk=storage();disk.setItem('bad','{broken');
  assert.equal(createDraftStore(()=>disk).load('bad').draft.text,'');
  const long='中文😀'.repeat(9000);
  disk.setItem('long',JSON.stringify({text:long,attachments:[],retry:null}));
  assert.equal(createDraftStore(()=>disk).load('long').draft.text,long);
  assert.equal(conversationAccent('a'),conversationAccent('a'));
  assert.notEqual(conversationAccent('a'),conversationAccent('b'));
  assert.match(conversationAccent('a'),/^hsl\(\d+ 34% 55%\)$/);
});

test('drafts retain more than five path-only attachment references',()=>{
 const values=new Map();const store=createDraftStore(()=>({getItem:k=>values.get(k)??null,setItem:(k,v)=>values.set(k,v),removeItem:k=>values.delete(k)}));
 const attachments=Array.from({length:12},(_,i)=>({id:String(i),label:`图片${i+1}`,name:'image.png',path:`assets/${i}.png`,mime:'image/png',size:4*1024*1024,sha256:'hash'}));
 store.save('many',{text:'@图片12',attachments,retry:null});
 assert.equal(store.load('many').draft.attachments.length,12);
 const restored=createDraftStore(()=>({getItem:k=>values.get(k)??null,setItem(){},removeItem(){}}));
 assert.equal(restored.load('many').draft.attachments[11].label,'图片12');
 assert.equal(values.get('many').includes('data_base64'),false);
});


test('successful sends clear the exact original whitespace, including image mention suffixes',()=>{
  const disk=storage(), store=createDraftStore(()=>disk);
  for(const text of ['@图片1 ', '  图片说明\n@图片1 \n', '普通文字 ', '\n代码\n', '']){
    const sent={id:'send-whitespace',text,attachmentKey:'f1'};
    store.save('a',{text,attachments:[file],retry:sent});
    assert.equal(store.clearSent('a',sent).cleared,true,JSON.stringify(text));
    assert.deepEqual(createDraftStore(()=>disk).load('a').draft,{text:'',attachments:[],retry:null});
  }
});

test('an in-flight send cannot erase whitespace edits, added files, or a newer retry',()=>{
  const store=createDraftStore(()=>storage()),sent={id:'original',text:'@图片1 ',attachmentKey:'f1'};
  const original={text:sent.text,attachments:[file],retry:sent};
  for(const newer of [
    {...original,text:'@图片1'},
    {...original,text:'@图片1  '},
    {...original,text:'@图片1 \n'},
    {...original,attachments:[file,{...file,id:'f2'}]},
    {...original,retry:{...sent,id:'new-retry'}}
  ]){
    store.save('a',newer);
    assert.equal(store.clearSent('a',sent).cleared,false);
    assert.deepEqual(store.load('a').draft,newer);
  }
});
