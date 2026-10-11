import test from 'node:test';
import assert from 'node:assert/strict';
import {mkdtempSync,rmSync,readFileSync,writeFileSync,symlinkSync} from 'node:fs';
import {tmpdir} from 'node:os';
import path from 'node:path';
import {chatUi,chatTool,seedStore} from '../dist/chat/store.js';
import {seedInitialize,seedCreated,seedAuthenticate,seedPoll,seedBegin,seedFinish,SEED_REPLACE_MS} from '../dist/chat/seeds.js';
import {createMcpFixture} from './mcpTestHelpers.mjs';
import {tunnelPathAllowed} from '../dist/tunnel.js';
function fixture(t,count=3){const root=mkdtempSync(path.join(tmpdir(),'seeds-'));t.after(()=>rmSync(root,{recursive:true,force:true}));chatUi(root,{action:'seed_settings',enabled:true});const batch=chatUi(root,{action:'seed_batch',count,account:'test',repo_id:'repo-id',branch:'main'}).batch;const store=seedStore(root),tokens=batch.map(s=>seedInitialize(store,s.seed_id,s.ticket));return {root,batch,store,tokens};}
test('credentials are exchanged, bounded and never exposed in inventory or archives',t=>{
 const {root,batch,store,tokens}=fixture(t);const s=batch[0];assert.equal(seedInitialize(store,s.seed_id,s.ticket),tokens[0]);
 assert.throws(()=>seedAuthenticate(store,s.seed_id,s.ticket),/invalid/);assert.doesNotThrow(()=>seedAuthenticate(store,s.seed_id,tokens[0]));
 assert.throws(()=>seedInitialize(store,s.seed_id,s.ticket,Date.now()+61000),/expired/);
 assert.throws(()=>chatUi(root,{action:'seed_batch',count:51,account:'a',repo_id:'r',branch:'m'}),/1–50/);
 for(const output of [JSON.stringify(chatUi(root,{action:'seed_list'})),readFileSync(path.join(root,'docs/chat-sessions/seed-library.seeds.json'),'utf8')]){assert.ok(!output.includes(s.ticket));assert.ok(!output.includes(tokens[0]));}
 assert.equal(chatUi(root,{action:'list'}).sessions.length,0);
});
test('assignment is exclusive, retains history and fences retired owners',t=>{
 const {root,batch,store,tokens}=fixture(t);const chat=chatUi(root,{action:'create'}).session;
 chatUi(root,{action:'send',chat_id:chat.id,message_id:'pending',text:'Continue my project'});
 const first=seedPoll(store,batch[0].seed_id,tokens[0]);assert.equal(first.chat_id,chat.id);
 assert.equal(seedPoll(store,batch[1].seed_id,tokens[1]).status,'idle');
 assert.throws(()=>chatTool(root,'chat_open',{chat_id:chat.id}),/reserved/);
 const second=seedPoll(store,batch[1].seed_id,tokens[1],Date.now()+SEED_REPLACE_MS+1);assert.equal(second.chat_id,chat.id);assert.notEqual(second.attachment_id,first.attachment_id);assert.equal(second.generation,2);
 assert.throws(()=>seedBegin(store,batch[0].seed_id,tokens[0],'apply_patch',{},'late'),/retired/);
 assert.throws(()=>chatTool(root,'chat_reply',{chat_id:chat.id,attachment_id:first.attachment_id}),/expired/);
 const opened=chatTool(root,'chat_open',second);assert.equal(opened.session.messages[0].id,'pending');
 assert.equal(chatUi(root,{action:'seed_list'}).retired_count,1);chatUi(root,{action:'seed_cleanup'});assert.equal(chatUi(root,{action:'seed_list'}).retired_count,1);
});
test('in-flight and retained commands prevent replacement; explicit detach is respected',t=>{
 const {root,batch,store,tokens}=fixture(t);const chat=chatUi(root,{action:'create'}).session;chatTool(root,'chat_open',seedPoll(store,batch[0].seed_id,tokens[0]));
 const run=seedBegin(store,batch[0].seed_id,tokens[0],'exec_command',{},'exec1');const future=Date.now()+SEED_REPLACE_MS+10;
 assert.equal(seedPoll(store,batch[1].seed_id,tokens[1],future).status,'idle');assert.throws(()=>chatUi(root,{action:'seed_retire',seed_id:batch[0].seed_id}),/unresolved/);
 seedFinish(store,batch[0].seed_id,run.call_id,'exec_command',{}, {process_still_running:true,session_id:'process-1'});
 assert.equal(seedPoll(store,batch[1].seed_id,tokens[1],future).status,'idle');assert.throws(()=>seedBegin(store,batch[0].seed_id,tokens[0],'kill_session',{session_id:'other'},'bad'),/belong/);
 const wait=seedBegin(store,batch[0].seed_id,tokens[0],'wait_command',{session_id:'process-1'},'wait1');seedFinish(store,batch[0].seed_id,wait.call_id,'wait_command',{session_id:'process-1'},{process_still_running:false});
 chatUi(root,{action:'detach',chat_id:chat.id});assert.equal(seedPoll(store,batch[1].seed_id,tokens[1],future).status,'idle');assert.throws(()=>seedBegin(store,batch[0].seed_id,tokens[0],'apply_patch',{},'late'),/paused/);
});
test('manual chats, group chats and user confirmation are not automatically replaced',t=>{
 const {root,batch,store,tokens}=fixture(t);chatUi(root,{action:'seed_settings',enabled:false});const manual=chatUi(root,{action:'create'}).session;chatTool(root,'chat_open',{chat_id:manual.id});chatUi(root,{action:'seed_settings',enabled:true});
 chatUi(root,{action:'create',mode:'group'});assert.equal(seedPoll(store,batch[0].seed_id,tokens[0]).status,'idle');
 const chat=chatUi(root,{action:'create'}).session;chatUi(root,{action:'send',chat_id:chat.id,message_id:'u',text:'Proceed?'});const assigned=seedPoll(store,batch[0].seed_id,tokens[0]);chatTool(root,'chat_wait',assigned);chatTool(root,'chat_reply',{...assigned,message_id:'a',reply_to:'u',text:'Choose',final:true,awaiting_user:true});
 assert.equal(seedPoll(store,batch[1].seed_id,tokens[1],Date.now()+SEED_REPLACE_MS+1).status,'idle');
});
test('registry corruption and symlinks fail closed',t=>{
 const {root}=fixture(t);const file=path.join(root,'docs/chat-sessions/seed-library.seeds.json');writeFileSync(file,'{');assert.throws(()=>chatUi(root,{action:'seed_list'}));rmSync(file);const target=path.join(root,'outside.json');writeFileSync(target,'{}');symlinkSync(target,file);assert.throws(()=>chatUi(root,{action:'seed_list'}),/symlink/);
});
test('real HTTP seed transport initializes, scopes tools, reads project and rejects wrong identity',async t=>{
 const f=await createMcpFixture(t);chatUi(f.root,{action:'seed_settings',enabled:true});const seed=chatUi(f.root,{action:'seed_batch',count:1,account:'fixture',repo_id:'repo',branch:'main'}).batch[0];const endpoint=`${f.endpoint}/seeds/repo/${seed.seed_id}`;let seq=0;
 const rpc=async(method,params,token=seed.ticket,notification=false)=>{const r=await fetch(endpoint,{method:'POST',headers:{'content-type':'application/json','MCP-Protocol-Version':'2025-03-26',authorization:`Bearer ${token}`},body:JSON.stringify({jsonrpc:'2.0',...(!notification?{id:++seq}:{}),method,params})});return r.status===202?{accepted:true}:r.json();};
 const initialized=await rpc('initialize',{protocolVersion:'2025-03-26'});const token=initialized.result?._meta.seed_access_token;assert.ok(token,JSON.stringify(initialized));assert.deepEqual(await rpc('notifications/initialized',{},token,true),{accepted:true});
 const tools=(await rpc('tools/list',{},token)).result.tools;assert.ok(tools.some(t=>t.name==='seed_wait'));assert.ok(!tools.some(t=>t.name==='switch_workspace_folder'));
 const call=(name,args={},key=token)=>rpc('tools/call',{name,arguments:args},key);assert.ok((await call('read_file',{path:'sample.txt'})).error);assert.ok((await call('seed_wait',{timeout_ms:0},seed.ticket)).error);assert.equal((await call('list_workspace_folders')).result.structuredContent.folders.length,1);
 const chat=chatUi(f.root,{action:'create'}).session;const assignment=(await call('seed_wait',{timeout_ms:0})).result.structuredContent;assert.equal(assignment.chat_id,chat.id);writeFileSync(path.join(f.root,'sample.txt'),'seed project');assert.ok((await call('read_file',{path:'sample.txt',workspace_folder_id:'other'})).error);
 const opened=await call('chat_open',{chat_id:chat.id,attachment_id:assignment.attachment_id});assert.equal(opened.result?.structuredContent?.ok,true,JSON.stringify(opened));const read=await call('read_file',{path:'sample.txt'});assert.equal(read.result?.structuredContent?.content,'seed project',JSON.stringify(read));
 chatUi(f.root,{action:'seed_retire',seed_id:seed.seed_id});assert.ok((await call('read_file',{path:'sample.txt'})).error);assert.equal(tunnelPathAllowed(f.config,new URL(endpoint).pathname),true);assert.equal(tunnelPathAllowed(f.config,new URL(endpoint).pathname+'/extra'),false);
});
test('assignment waits for acknowledgement and recovers an interrupted registry save',t=>{
 const {root,batch,store,tokens}=fixture(t);const chat=chatUi(root,{action:'create'}).session;
 const registry=path.join(root,'docs/chat-sessions/seed-library.seeds.json'),before=readFileSync(registry);
 const assignment=seedPoll(store,batch[0].seed_id,tokens[0]);
 assert.equal(chatUi(root,{action:'list'}).sessions[0].status,'offline');
 assert.throws(()=>seedBegin(store,batch[0].seed_id,tokens[0],'read_file',{},'early'),/chat_open/);
 // The chat owner was saved but the registry write was lost. A fresh store must reconcile it.
 writeFileSync(registry,before);const recovered=seedStore(root);
 assert.equal(seedPoll(recovered,batch[1].seed_id,tokens[1]).status,'idle');
 assert.equal(seedPoll(recovered,batch[0].seed_id,tokens[0]).attachment_id,assignment.attachment_id);
 assert.equal(chatTool(root,'chat_open',assignment).session.status,'connected');
 assert.doesNotThrow(()=>seedBegin(recovered,batch[0].seed_id,tokens[0],'read_file',{},'ready'));
 assert.equal(JSON.parse(readFileSync(registry)).seeds[0].chat_id,chat.id);
 assert.throws(()=>seedPoll(recovered,batch[0].seed_id,tokens[0],Date.now()+8*86400000),/expired/);
});
test('concurrent HTTP seed waits allocate one owner and reject a duplicate waiter',async t=>{
 const f=await createMcpFixture(t);const {batch,store,tokens}=(()=>{chatUi(f.root,{action:'seed_settings',enabled:true});const batch=chatUi(f.root,{action:'seed_batch',count:2,account:'test',repo_id:'repo',branch:'main'}).batch;const store=seedStore(f.root);return {batch,store,tokens:batch.map(s=>seedInitialize(store,s.seed_id,s.ticket))};})();
 let seq=0;const wait=async(i,timeout=0)=>{const response=await fetch(`${f.endpoint}/seeds/repo/${batch[i].seed_id}`,{method:'POST',headers:{'content-type':'application/json','MCP-Protocol-Version':'2025-03-26',authorization:`Bearer ${tokens[i]}`},body:JSON.stringify({jsonrpc:'2.0',id:++seq,method:'tools/call',params:{name:'seed_wait',arguments:{timeout_ms:timeout}}})});return response.json();};
 chatUi(f.root,{action:'create'});
 const results=await Promise.all([wait(0),wait(1)]);
 assert.equal(results.filter(r=>r.result?.structuredContent?.status==='assigned').length,1);
 const idle=results.findIndex(r=>r.result?.structuredContent?.status==='idle');
 const waiting=wait(idle,1000);await new Promise(resolve=>setTimeout(resolve,100));
 assert.match((await wait(idle)).error.message,/already active/);
 assert.equal((await waiting).result.structuredContent.status,'idle');
});

// Configuration corrections only revoke unused enrollment tickets, never a live project binding.
test('correcting unconnected batches requires confirmation, retires old tickets and retains history',t=>{
 const root=mkdtempSync(path.join(tmpdir(),'seed-correction-'));t.after(()=>rmSync(root,{recursive:true,force:true}));
 const args={action:'seed_batch',count:1,account:'a',repo_id:'1232',branch:'main'};
 const first=chatUi(root,args).batch[0],store=seedStore(root);
 assert.throws(()=>chatUi(root,{...args,repo_id:'1412803600'}),/Confirm/);
 const next=chatUi(root,{...args,repo_id:'1412803600',replace_unconnected:true}).batch[0];
 assert.throws(()=>seedInitialize(store,first.seed_id,first.ticket),/invalid/);
 assert.equal(chatUi(root,{action:'seed_list'}).retired_count,1);
 assert.doesNotThrow(()=>chatUi(root,{...args,repo_id:'1412803600'}));
 seedInitialize(store,next.seed_id,next.ticket);
 chatUi(root,{action:'seed_retire',seed_id:next.seed_id});
 assert.throws(()=>chatUi(root,{...args,repo_id:'other',replace_unconnected:true}),/Connected/);
});
test('a recorded host task prevents configuration correction even before initialization',t=>{
 const root=mkdtempSync(path.join(tmpdir(),'seed-submitted-'));t.after(()=>rmSync(root,{recursive:true,force:true}));
 const args={action:'seed_batch',count:1,account:'a',repo_id:'repo',branch:'main'};
 const first=chatUi(root,args).batch[0];seedCreated(seedStore(root),first.seed_id,first.ticket,'task-id');
 assert.throws(()=>chatUi(root,{...args,branch:'different',replace_unconnected:true}),/Connected/);
 assert.equal(chatUi(root,{action:'seed_list'}).retired_count,0);
});
