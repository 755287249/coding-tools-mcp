import test from 'node:test';
import assert from 'node:assert/strict';
import {startVisiblePolling,reconcileSnapshot} from '../src/lib/chat/polling.ts';

test('unchanged transcripts keep their references while heartbeat and edited messages update',()=>{
 const previous={status:'waiting',messages:[{id:'1',text:'  formatted\n text  '},{id:'2',text:'old'}]};
 assert.equal(reconcileSnapshot(previous,structuredClone(previous)),previous);
 const heartbeat=reconcileSnapshot(previous,{...structuredClone(previous),status:'connected'});
 assert.notEqual(heartbeat,previous);assert.equal(heartbeat.messages,previous.messages);
 const edit=reconcileSnapshot(previous,{...structuredClone(previous),messages:[previous.messages[0],{id:'2',text:'new'}]});
 assert.equal(edit.messages[0],previous.messages[0]);assert.equal(edit.messages[1].text,'new');assert.equal(previous.messages[1].text,'old');
 assert.deepEqual(reconcileSnapshot(previous,{messages:[]}),{messages:[]});
 const special=JSON.parse('{"__proto__":{"polluted":true}}');assert.deepEqual(reconcileSnapshot({},special),special);assert.equal({}.polluted,undefined);
});

test('polling pauses hidden tabs, resumes immediately, stays serial and stops on unmount',async t=>{
 t.mock.timers.enable({apis:['setTimeout']});
 const previous=globalThis.document;const doc=new EventTarget();doc.hidden=false;globalThis.document=doc;
 let calls=0,finish;const flush=async()=>{await Promise.resolve();await Promise.resolve()};
 const stop=startVisiblePolling(async()=>{calls++;await new Promise(r=>finish=r)},()=>1500);
 try {
  assert.equal(calls,1);t.mock.timers.tick(6000);assert.equal(calls,1);
  doc.hidden=true;doc.dispatchEvent(new Event('visibilitychange'));doc.hidden=false;doc.dispatchEvent(new Event('visibilitychange'));assert.equal(calls,1);
  finish();await flush();t.mock.timers.tick(1500);assert.equal(calls,2);
  doc.hidden=true;doc.dispatchEvent(new Event('visibilitychange'));finish();await flush();t.mock.timers.tick(60000);assert.equal(calls,2);
  doc.hidden=false;doc.dispatchEvent(new Event('visibilitychange'));assert.equal(calls,3);
  finish();await flush();doc.hidden=true;doc.dispatchEvent(new Event('visibilitychange'));t.mock.timers.tick(60000);assert.equal(calls,3);
  doc.hidden=false;doc.dispatchEvent(new Event('visibilitychange'));assert.equal(calls,4);
  stop();finish();await flush();t.mock.timers.tick(60000);doc.dispatchEvent(new Event('visibilitychange'));assert.equal(calls,4);
 } finally {stop();globalThis.document=previous}
});
