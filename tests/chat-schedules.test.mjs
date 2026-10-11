import assert from 'node:assert/strict';
import test from 'node:test';
import {readFileSync} from 'node:fs';
import {get,writable} from 'svelte/store';
import ts from 'typescript';
const source=readFileSync(new URL('../src/lib/chat/schedules.ts',import.meta.url),'utf8');
const compile=s=>ts.transpileModule(s,{compilerOptions:{module:ts.ModuleKind.ESNext,target:ts.ScriptTarget.ES2022}}).outputText;
const url=s=>'data:text/javascript;base64,'+Buffer.from(s).toString('base64');
const task=(id='job-1')=>({id,workspaceId:'old-project',folderId:'folder-a',chatId:'chat-a',title:'Test',text:'Run once',due:1,status:'pending'});
let sequence=0;
async function fixture(){
 const memory=new Map(),calls=[],polls=[];
 globalThis.localStorage={getItem:k=>memory.get(k)??null,setItem:(k,v)=>memory.set(k,v)};
 const f={workspaces:writable([{id:'project-a',folders:[{id:'folder-a'}]}]),polls,calls,reset(){},request:async(w,d,args)=>{calls.push([w,d,args]);return {tasks:[]}}};globalThis.scheduleFixture=f;
 const stubs={
  '$lib/backend':'export const getBackend=()=>({chat:{request:(...args)=>globalThis.scheduleFixture.request(...args)}})',
  '$lib/stores/app':'export const workspaces=globalThis.scheduleFixture.workspaces',
  '$lib/types':'export const workspaceFolders=ws=>ws.folders',
  './polling':'export const startVisiblePolling=fn=>{globalThis.scheduleFixture.polls.push(fn);return ()=>{}}',
  './cache-lifecycle.js':'export const onChatCacheReset=fn=>{globalThis.scheduleFixture.reset=fn}',
  '$lib/stores/workspace-lifecycle':'export const workspaceBlocked=()=>false;export const workspaceEpoch=()=>0'
 };
 let code=source.replace("'svelte/store'",JSON.stringify(import.meta.resolve('svelte/store')));
 for(const [name,stub] of Object.entries(stubs))code=code.replace(`'${name}'`,JSON.stringify(url(stub+`\n//${sequence}`)));
 const scheduler=await import(url(compile(code)+`\n//${sequence++}`));
 return {...f,memory,scheduler,setRequest(fn){f.request=fn},reset:()=>f.reset()};
}
test('browser migrates stable IDs to the matching fixed project and never delivers messages itself',async()=>{
 const f=await fixture();f.memory.set('ctmcp-chat-schedules-v1',JSON.stringify([task()]));
 const stop=f.scheduler.startScheduleRunner();await f.polls[0]();
 assert.deepEqual(f.calls.map(c=>c[2].action),['schedule_import','schedule_list']);assert.equal(f.calls[0][0],'project-a');assert.equal(f.calls[0][1],'folder-a');assert.equal(f.calls[0][2].id,'job-1');assert.equal(f.memory.get('ctmcp-chat-schedules-v1'),'[]');stop();
});
test('failed migration preserves local tasks for an identical retry',async()=>{
 const f=await fixture();f.memory.set('ctmcp-chat-schedules-v1',JSON.stringify([task()]));const imports=[];
 f.setRequest(async(w,d,args)=>{if(args.action==='schedule_import'){imports.push(args);throw Error('temporary failure')}return {tasks:[]}});
 const stop=f.scheduler.startScheduleRunner();await f.polls[0]();await f.polls[0]();assert.equal(JSON.parse(f.memory.get('ctmcp-chat-schedules-v1')).length,1);assert.deepEqual(imports[0],imports[1]);assert.match(get(f.scheduler.scheduleError),/temporary/);stop();
});
test('identity reset rejects an in-flight response without clearing its legacy source',async()=>{
 const f=await fixture();f.memory.set('ctmcp-chat-schedules-v1',JSON.stringify([task()]));let release;
 f.setRequest(()=>new Promise(resolve=>release=resolve));const stop=f.scheduler.startScheduleRunner();const poll=f.polls[0]();
 f.reset();release({tasks:[task()]});await poll;assert.deepEqual(get(f.scheduler.scheduledChats),[]);assert.equal(JSON.parse(f.memory.get('ctmcp-chat-schedules-v1')).length,1);stop();
});
test('unreadable storage preserves bytes while server sync remains available',async()=>{
 const f=await fixture();f.memory.set('ctmcp-chat-schedules-v1','{broken');const stop=f.scheduler.startScheduleRunner();await f.polls[0]();assert.ok(get(f.scheduler.scheduleError));assert.equal(f.memory.get('ctmcp-chat-schedules-v1'),'{broken');assert.equal(f.calls[0][2].action,'schedule_list');stop();
});
test('schedule updates use compare-and-swap status and the exact project',async()=>{
 const f=await fixture();await f.scheduler.updateSchedule({...task(),workspaceId:'project-a'},'paused');assert.deepEqual(f.calls[0],['project-a','folder-a',{action:'schedule_update',id:'job-1',status:'paused',expected_status:'pending'}]);
});
test('wallpaper effects use Mica only on supported builds, otherwise solid',async()=>{
 const glass=readFileSync(new URL('../src/lib/stores/glass.ts',import.meta.url),'utf8');const fn=glass.match(/export function effectForBuild[\s\S]*?\n}/)?.[0];assert.ok(fn);const {effectForBuild}=await import(url(compile(fn)));
 for(const value of [null,undefined,NaN,Infinity,-1,0,19045,21999])assert.equal(effectForBuild(value),'solid');for(const value of [22000,22631,26100])assert.equal(effectForBuild(value),'mica');
});
