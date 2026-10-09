import assert from 'node:assert/strict';
import test from 'node:test';
import {readFileSync} from 'node:fs';
import ts from 'typescript';
const source=readFileSync(new URL('../src/lib/chat/schedules.ts',import.meta.url),'utf8');
const compile=s=>ts.transpileModule(s,{compilerOptions:{module:ts.ModuleKind.ESNext,target:ts.ScriptTarget.ES2022}}).outputText;
const url=s=>'data:text/javascript;base64,'+Buffer.from(s).toString('base64');
const storeUrl=url('export function writable(value){return {set(next){value=next}, get(){return value}}}');
const chatUrl=url('export const localChat=(...args)=>globalThis.scheduleSend(...args)');
const compiled=compile(source.replace("'svelte/store'",JSON.stringify(storeUrl)).replace("'$lib/api/chat'",JSON.stringify(chatUrl)));
const scheduler=await import(url(compiled));
const memory=new Map();
globalThis.localStorage={getItem:k=>memory.get(k)??null,setItem:(k,v)=>memory.set(k,v)};
let queue=Promise.resolve();
Object.defineProperty(globalThis,'navigator',{configurable:true,value:{locks:{request:(_key,options,fn)=>{const callback=typeof options==='function'?options:fn;const promise=queue.then(()=>callback({}));queue=promise.catch(()=>{});return promise;}}}});
globalThis.window={addEventListener(){},removeEventListener(){}};
const settle=async()=>{await new Promise(resolve=>setTimeout(resolve,20));await queue};
const task=(id='job-1')=>({id,workspaceId:'workspace-a',folderId:'folder-a',chatId:'chat-a',title:'Test',text:'Run the requested check',due:Date.now()-100,status:'pending'});

test('two runners deliver a due task once to its exact project and conversation',async()=>{
  memory.clear();const calls=[];globalThis.scheduleSend=async(...args)=>{calls.push(args);return {session:{id:'chat-a'}}};
  await scheduler.changeSchedule(()=>[task()]);const stops=[scheduler.startScheduleRunner(),scheduler.startScheduleRunner()];
  await settle();stops.forEach(stop=>stop());assert.equal(calls.length,1);assert.deepEqual(calls[0],['workspace-a','folder-a',{action:'send',chat_id:'chat-a',message_id:'job-1',text:'Run the requested check'}]);assert.equal(scheduler.scheduledChats.get()[0].status,'sent');
});
test('failed delivery only retries explicitly with the identical ID and text',async()=>{
  memory.clear();const calls=[];globalThis.scheduleSend=async(...args)=>{calls.push(args);if(calls.length===1)throw new Error('response lost');return {}};
  await scheduler.changeSchedule(()=>[task()]);let stop=scheduler.startScheduleRunner();await settle();stop();assert.equal(scheduler.scheduledChats.get()[0].status,'error');
  stop=scheduler.startScheduleRunner();await settle();stop();assert.equal(calls.length,1);
  await scheduler.changeSchedule(tasks=>tasks.map(t=>({...t,status:'pending'})));stop=scheduler.startScheduleRunner();await settle();stop();assert.equal(calls.length,2);assert.deepEqual(calls[0],calls[1]);
});
test('restart resumes interrupted send; future and paused tasks stay untouched',async()=>{
  memory.clear();const calls=[];globalThis.scheduleSend=async(...args)=>{calls.push(args);return {}};
  await scheduler.changeSchedule(()=>[{...task('interrupted'),status:'sending'},{...task('future'),due:Date.now()+60000},{...task('paused'),status:'paused'}]);const stop=scheduler.startScheduleRunner();await settle();stop();assert.equal(calls.length,1);assert.equal(calls[0][2].message_id,'interrupted');assert.deepEqual(scheduler.scheduledChats.get().map(t=>t.status),['sent','pending','paused']);
});
test('unreadable storage is reported without sending or overwriting it',async()=>{
  const before='{broken';memory.set('ctmcp-chat-schedules-v1',before);globalThis.scheduleSend=async()=>assert.fail('must not send');const stop=scheduler.startScheduleRunner();await settle();stop();assert.ok(scheduler.scheduleError.get());assert.equal(memory.get('ctmcp-chat-schedules-v1'),before);
});

test('wallpaper effects use Mica only on supported builds, otherwise solid',async()=>{
  const glass=readFileSync(new URL('../src/lib/stores/glass.ts',import.meta.url),'utf8');
  const fn=glass.match(/export function effectForBuild[\s\S]*?\n}/)?.[0];assert.ok(fn);
  const {effectForBuild}=await import(url(compile(fn)));
  for(const value of [null,undefined,NaN,Infinity,-1,0,19045,21999])assert.equal(effectForBuild(value),'solid');
  for(const value of [22000,22631,26100])assert.equal(effectForBuild(value),'mica');
});
