import test from 'node:test';
import assert from 'node:assert/strict';
import {mkdtempSync,rmSync,readFileSync,writeFileSync,symlinkSync} from 'node:fs';
import {tmpdir} from 'node:os';
import path from 'node:path';
import {chatUi,chatTool,seedStore} from '../dist/chat/store.js';
import {seedProvision,seedInitialize,seedCreated,seedAuthenticate,seedPoll,seedBegin,seedFinish,SEED_REPLACE_MS} from '../dist/chat/seeds.js';
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

test('mixed host repositories preserve pending, connected and retired seeds without changing folder scope',t=>{
 const root=mkdtempSync(path.join(tmpdir(),'seed-mixed-'));t.after(()=>rmSync(root,{recursive:true,force:true}));
 const args={action:'seed_batch',count:1,account:'a',repo_id:'repo-a',branch:'main'};
 const first=chatUi(root,args).batch[0],store=seedStore(root);
 const registry=()=>JSON.parse(readFileSync(path.join(root,'docs/chat-sessions/seed-library.seeds.json')));
 const before=registry().seeds[0];
 const other={...args,account:'b',repo_id:'repo-b',branch:'develop',replace_unconnected:true};
 const next=chatUi(root,other).batch[0];assert.deepEqual(registry().seeds[0],before);
 const access=seedInitialize(store,first.seed_id,first.ticket);seedInitialize(store,next.seed_id,next.ticket);
 assert.doesNotThrow(()=>chatUi(root,other));assert.doesNotThrow(()=>seedAuthenticate(store,first.seed_id,access));
 const separate=fixture(t,1);assert.throws(()=>seedAuthenticate(separate.store,first.seed_id,access));
 chatUi(root,{action:'seed_retire',seed_id:first.seed_id});assert.doesNotThrow(()=>chatUi(root,other));
 const list=chatUi(root,{action:'seed_list'});assert.equal(list.retired_count,1);assert.equal(list.total,4);
 assert.equal(next.repo_id,'repo-b');assert.equal(next.branch,'develop');
});
test('a recorded host task survives another repository batch',t=>{
 const root=mkdtempSync(path.join(tmpdir(),'seed-submitted-'));t.after(()=>rmSync(root,{recursive:true,force:true}));
 const args={action:'seed_batch',count:1,account:'a',repo_id:'repo',branch:'main'};
 const first=chatUi(root,args).batch[0];seedCreated(seedStore(root),first.seed_id,first.ticket,'task-id');
 const registry=()=>JSON.parse(readFileSync(path.join(root,'docs/chat-sessions/seed-library.seeds.json')));
 const before=registry().seeds[0];
 assert.doesNotThrow(()=>chatUi(root,{...args,repo_id:'other-repo',branch:'different'}));
 assert.deepEqual(registry().seeds[0],before);assert.equal(chatUi(root,{action:'seed_list'}).retired_count,0);
});

test('pairing capability rotates, isolates folders, retries exactly and never persists raw secrets',t=>{
 const {root,store}=fixture(t,1),token='b'.repeat(64),next='c'.repeat(64);
 chatUi(root,{action:'seed_pairing',token});
 const args={action:'batch',request_id:'attempt-1',count:2,account:'test',repo_id:'repo-id',branch:'main',metadata:{repo_name:'owner/repo',organization_id:'org'}};
 const first=seedProvision(store,token,args);assert.deepEqual(seedProvision(store,token,args),first);
 assert.throws(()=>seedProvision(store,token,{...args,count:3}),/changed/);assert.throws(()=>seedProvision(store,token,{...args,repo_id:'different'}),/changed/);assert.throws(()=>seedProvision(store,token,{...args,branch:'different'}),/changed/);assert.throws(()=>seedProvision(store,token,{action:'read_file'}),/only/);
 assert.throws(()=>seedProvision(store,next,{action:'status'}),/invalid/);
 const other=fixture(t,1);assert.throws(()=>seedProvision(other.store,token,{action:'status'}),/invalid/);
 for(const body of [readFileSync(path.join(root,'docs/chat-sessions/seed-library.seeds.json'),'utf8'),JSON.stringify(chatUi(root,{action:'seed_list'}))]){assert.ok(!body.includes(token));for(const s of first.batch)assert.ok(!body.includes(s.ticket));}
 chatUi(root,{action:'seed_pairing',token:next});assert.throws(()=>seedProvision(store,token,{action:'status'}),/revoked/);assert.equal(seedProvision(store,next,{action:'status'}).ok,true);
 chatUi(root,{action:'seed_pairing',enabled:false});assert.throws(()=>seedProvision(store,next,{action:'status'}),/revoked/);
});
test('600 records paginate, filter and summarize; 500 warm standby polls share one scan per account and chat creation invalidates it',t=>{
 const {root,store,tokens}=fixture(t,1),file=path.join(root,'docs/chat-sessions/seed-library.seeds.json');
 const data=JSON.parse(readFileSync(file)),template=data.seeds[0],now=Date.now();data.seeds=Array.from({length:600},(_,i)=>({...template,id:'load-'+i,account:i%2?'odd':'even',ready:true,metadata:{repo_name:'owner/repo'},last_seen:now}));writeFileSync(file,JSON.stringify(data));
 let scans=0;const counted={...store,ids:()=>{scans++;return store.ids();}};
 // Each synthetic seed shares the fixture access hash solely for this load test.
 const start=performance.now();for(let i=0;i<500;i++)assert.equal(seedPoll(counted,'load-'+i,tokens[0],now).status,'idle');
 assert.equal(scans,2);t.diagnostic(`500 warm polls / 600 records: ${(performance.now()-start).toFixed(0)}ms, ${scans} full chat scan`);
 const page=chatUi(root,{action:'seed_list',limit:50,offset:50,account:'odd',search:'OWNER/REPO',status:'ready'});assert.equal(page.total,600);assert.equal(page.filtered_total,300);assert.equal(page.seeds.length,50);assert.equal(page.seeds[0].id,'load-101');assert.equal(page.counts.ready,600);assert.equal(page.capacity,10000);
 assert.equal(chatUi(root,{action:'seed_list',limit:1000}).seeds.length,200);
 const chat=chatUi(root,{action:'create'}).session;assert.equal(seedPoll(counted,'load-0',tokens[0],now).chat_id,chat.id);assert.equal(scans,3);
});
test('paired HTTP endpoint is exact, folder scoped, no-store and cannot authenticate ordinary MCP',async t=>{
 const f=await createMcpFixture(t),token='d'.repeat(64),endpoint=f.endpoint+'/seed-library/repo';chatUi(f.root,{action:'seed_pairing',token});
 const post=async(url,args,secret=token)=>fetch(url,{method:'POST',headers:{'content-type':'application/json',authorization:'Bearer '+secret},body:JSON.stringify(args)});
 const state=await post(endpoint,{action:'status'});assert.equal(state.status,200);assert.equal(state.headers.get('cache-control'),'no-store');assert.equal((await state.json()).workspace_folder_id,'repo');
 assert.equal((await post(endpoint,{action:'status'},'e'.repeat(64))).status,400);assert.equal((await post(endpoint,{action:'read_file'})).status,400);
 assert.equal((await post(f.endpoint+'/seed-library/other',{action:'status'})).status,404);
 assert.equal((await post(f.endpoint,{jsonrpc:'2.0',id:1,method:'tools/list'})).status,401);
 assert.equal(tunnelPathAllowed(f.config,new URL(endpoint).pathname),true);assert.equal(tunnelPathAllowed(f.config,new URL(endpoint).pathname+'/extra'),false);
 assert.equal((await fetch(endpoint)).status,405);
});

test('source and prefix route initial and replacement owners; another warm pool cannot suppress matching scans',t=>{
 const {root,store,batch,tokens}=fixture(t,1);
 for(let i=1;i<3;i++){const seed=chatUi(root,{action:'seed_batch',count:1,account:'host-'+i,repo_id:'repo-'+i,branch:'branch-'+i}).batch[0];batch.push(seed);tokens.push(seedInitialize(store,seed.seed_id,seed.ticket));}
 for(let i=0;i<3;i++)chatUi(root,{action:'seed_label',seed_id:batch[i].seed_id,source:'coderabbit',prefix:i===1?'GPT-6-sol':'GPT-6-astra'});
 for(let i=0;i<3;i++)assert.equal(seedPoll(store,batch[i].seed_id,tokens[i]).status,'idle');
 const chat=chatUi(root,{action:'create',seed_filter:{source:'coderabbit',keyword:'ASTRA'}}).session;
 assert.deepEqual(chat.seed_filter,{source:'coderabbit',keyword:'ASTRA'});
 assert.equal(seedPoll(store,batch[1].seed_id,tokens[1]).status,'idle');
 const first=seedPoll(store,batch[0].seed_id,tokens[0]);assert.equal(first.chat_id,chat.id);
 const future=Date.now()+SEED_REPLACE_MS+5;
 assert.equal(seedPoll(store,batch[1].seed_id,tokens[1],future).status,'idle');
 const second=seedPoll(store,batch[2].seed_id,tokens[2],future);assert.equal(second.chat_id,chat.id);assert.equal(second.generation,2);
 assert.deepEqual(chatUi(root,{action:'read',chat_id:chat.id}).session.seed_filter,chat.seed_filter);
 assert.throws(()=>seedBegin(store,batch[0].seed_id,tokens[0],'read_file',{},'old'),/retired/);
 const unavailable=chatUi(root,{action:'create',seed_filter:{source:'arena',keyword:'grok'}}).session;
 assert.equal(seedPoll(store,batch[1].seed_id,tokens[1]).status,'idle');assert.equal(chatUi(root,{action:'read',chat_id:unavailable.id}).session.seed_owner,undefined);
 assert.equal(chatUi(root,{action:'create',seed_manual:true}).session.seed_auto,undefined);
});
test('manual enrollment has no host repo requirement and classifies independently alongside mixed browser host repositories',t=>{
 const {root,store}=fixture(t,1);
 const manual=chatUi(root,{action:'seed_manual',source:'Arena',prefix:'Opus',account:'personal'}).batch[0];assert.equal(manual.repo_id,'');assert.equal(manual.branch,'');assert.equal(manual.source,'arena');
 const access=seedInitialize(store,manual.seed_id,manual.ticket);
 const chat=chatUi(root,{action:'create',seed_filter:{source:'arena',keyword:'opus'}}).session;
 assert.equal(seedPoll(store,manual.seed_id,access).chat_id,chat.id);
 assert.doesNotThrow(()=>chatUi(root,{action:'seed_batch',count:1,account:'test',repo_id:'repo-id',branch:'main'}));
 assert.doesNotThrow(()=>chatUi(root,{action:'seed_batch',count:1,account:'test',repo_id:'other',branch:'main',replace_unconnected:true}));
 const list=chatUi(root,{action:'seed_list',source:'arena',keyword:'OPUS'});assert.equal(list.seeds.length,1);assert.equal(list.groups.find(g=>g.source==='arena').prefix,'Opus');
 assert.ok(!JSON.stringify(list).includes(manual.ticket));
});
test('paired prefix is stable across retries and existing seeds can be classified',t=>{
 const {root,store,batch}=fixture(t,1),token='e'.repeat(64);chatUi(root,{action:'seed_pairing',token});
 const args={action:'batch',request_id:'classified',count:1,account:'test',repo_id:'repo-id',branch:'main',prefix:'GPT-6-astra'};
 const first=seedProvision(store,token,args);assert.deepEqual(seedProvision(store,token,args),first);assert.equal(first.batch[0].prefix,'GPT-6-astra');assert.equal(first.batch[0].source,'coderabbit');
 assert.throws(()=>seedProvision(store,token,{...args,prefix:'grok'}),/changed/);
 chatUi(root,{action:'seed_label',seed_id:batch[0].seed_id,source:'coderabbit',prefix:'astra-old'});assert.equal(chatUi(root,{action:'seed_list',keyword:'astra'}).seeds.length,2);
});

test('account cards summarize every filtered seed before detail pagination',t=>{
 const {root}=fixture(t,1),file=path.join(root,'docs/chat-sessions/seed-library.seeds.json');
 const data=JSON.parse(readFileSync(file)),template=data.seeds[0],now=Date.now();
 data.seeds=Array.from({length:60},(_,i)=>({...template,id:'summary-'+i,account:'account-a',prefix:'Astra',ready:true,last_seen:now,
   ...(i%6===1?{chat_id:'assigned-chat'}:{}),...(i%6===2?{inflight:['operation']}:{}),
   ...(i%6===3?{redeemed_at:0}:{}),...(i%6===4?{last_seen:now-700000}:{}),
   ...(i%6===5?{retired_at:now}:{}),archived:i===59}));
 data.seeds.push({...template,id:'summary-b',account:'__proto__',source:'custom',prefix:'Sol',ready:true,last_seen:now});
 writeFileSync(file,JSON.stringify(data));
 const list=args=>chatUi(root,{action:'seed_list',include_archived:false,...args});
 const page=list({limit:1,offset:40}),a=page.account_summaries.find(s=>s.account==='account-a');
 assert.equal(page.seeds.length,1);assert.equal(page.filtered_total,60);
 assert.deepEqual(a,{account:'account-a',total:59,valid:30,counts:{ready:10,assigned:10,working:10,preparing:10,offline:10,retired:9}});
 assert.deepEqual(page.account_summaries,list({limit:1,offset:0}).account_summaries);
 assert.equal(page.ready_groups.reduce((n,g)=>n+g.ready,0),11);assert.equal(page.ready_groups.find(g=>g.account==='account-a').ready,10);
 assert.equal(page.account_summaries.find(s=>s.account==='__proto__').valid,1);
 assert.equal(list({include_archived:true}).account_summaries.find(s=>s.account==='account-a').total,60);
 for(const filter of [{account:'account-a'},{source:'coderabbit',keyword:'ASTRA'},{search:'ACCOUNT-A'}]){
  assert.deepEqual(list(filter).account_summaries,[a]);
 }
 assert.deepEqual(list({account:'account-a',status:'offline'}).account_summaries,[{account:'account-a',total:10,valid:0,counts:{offline:10}}]);
 assert.deepEqual(list({keyword:'no-match'}).account_summaries,[]);
});

test('exact prefix and account selection fence initial and replacement owners and isolate scan caches',t=>{
 const {root,store,batch,tokens}=fixture(t,6),file=path.join(root,'docs/chat-sessions/seed-library.seeds.json');
 const data=JSON.parse(readFileSync(file));data.seeds.forEach((s,i)=>{s.account=i===0?'a':'b';s.prefix=['Astra','Astra','Astra','Astra-plus','astra',''][i];});writeFileSync(file,JSON.stringify(data));
 const now=Date.now();for(let i=0;i<6;i++)assert.equal(seedPoll(store,batch[i].seed_id,tokens[i],now).status,'idle');
 const ready=chatUi(root,{action:'seed_list',limit:1}).ready_groups;
 assert.equal(ready.reduce((n,g)=>n+g.ready,0),6);assert.equal(ready.find(g=>g.account==='b'&&g.prefix==='Astra').ready,2);
 const filter={source:'coderabbit',keyword:'',prefix:'Astra',account:'b'};
 const chat=chatUi(root,{action:'create',seed_filter:filter}).session;assert.deepEqual(chat.seed_filter,filter);
 for(const i of [0,3,4,5])assert.equal(seedPoll(store,batch[i].seed_id,tokens[i],now).status,'idle');
 const assigned=seedPoll(store,batch[1].seed_id,tokens[1],now);assert.equal(assigned.chat_id,chat.id);
 const after=chatUi(root,{action:'seed_list',limit:1}).ready_groups;assert.equal(after.find(g=>g.account==='b'&&g.prefix==='Astra').ready,1);
 const future=now+SEED_REPLACE_MS+1;for(const i of [0,3,4])assert.equal(seedPoll(store,batch[i].seed_id,tokens[i],future).status,'idle');
 const replacement=seedPoll(store,batch[2].seed_id,tokens[2],future);assert.equal(replacement.chat_id,chat.id);assert.equal(replacement.generation,2);
 assert.deepEqual(chatUi(root,{action:'read',chat_id:chat.id}).session.seed_filter,filter);
 const unlabeled=chatUi(root,{action:'create',seed_filter:{source:'',keyword:'',prefix:''}}).session;
 assert.equal(seedPoll(store,batch[4].seed_id,tokens[4],future).status,'idle');assert.equal(seedPoll(store,batch[5].seed_id,tokens[5],future).chat_id,unlabeled.id);
 for(const field of ['prefix','account'])assert.throws(()=>chatUi(root,{action:'create',seed_filter:{[field]:{bad:true}}}),/Invalid seed/);
});

test('URL-only HTTP onboarding discovers rules, registers, waits, opens and replies without identity parameters',async t=>{
 const f=await createMcpFixture(t);chatUi(f.root,{action:'seed_settings',enabled:true});
 const seed=chatUi(f.root,{action:'seed_manual',account:'url-fixture'}).batch[0],endpoint=`${f.endpoint}/seeds/repo/${seed.url_key}`;let seq=0;
 const rpc=async(method,params={},target=endpoint,notification=false)=>{const r=await fetch(target,{method:'POST',headers:{'content-type':'application/json','MCP-Protocol-Version':'2025-03-26'},body:JSON.stringify({jsonrpc:'2.0',...(!notification?{id:++seq}:{}),method,params})});return r.status===202?{accepted:true}:r.json();};
 const call=(name,args={})=>rpc('tools/call',{name,arguments:args});
 const intro=await fetch(endpoint);assert.equal(intro.status,200);assert.equal(intro.headers.get('cache-control'),'no-store');assert.match((await intro.json()).instructions,/seed_connect/);
 assert.equal(chatUi(f.root,{action:'seed_list'}).seeds[0].status,'preparing');
 assert.ok((await call('seed_connect')).error,'GET did not redeem');
 assert.ok((await rpc('initialize',{},`${f.endpoint}/seeds/repo/${seed.seed_id}`)).error,'public ID is not a capability');
 assert.ok((await rpc('initialize',{},`${f.endpoint}/seeds/other/${seed.url_key}`)).error);
 const init=await rpc('initialize');assert.match(init.result.instructions,/chat_wait/);assert.equal(init.result._meta,undefined);
 assert.deepEqual(await rpc('notifications/initialized',{},endpoint,true),{accepted:true});
 const catalog=(await rpc('tools/list')).result.tools;assert.ok(catalog.some(t=>t.name==='seed_connect'));
 for(const name of ['chat_wait','chat_reply','chat_open'])assert.ok(!catalog.find(t=>t.name===name).inputSchema.required?.includes('chat_id'));
 assert.equal((await rpc('resources/read',{uri:'coding-tools://skills/seed-connect'})).result.contents[0].text,init.result.instructions);
 const ready=(await call('seed_connect')).result.structuredContent;assert.equal(ready.registered,true);assert.equal(ready.seed_id,seed.seed_id);assert.equal(ready.workspace_folder_id,'repo');
 assert.equal(chatUi(f.root,{action:'seed_list'}).seeds[0].status,'ready');
 assert.equal((await call('chat_wait',{timeout_ms:0})).result.structuredContent.status,'idle');
 assert.ok((await call('read_file',{path:'sample.txt'})).error);
 const waiting=call('chat_wait',{timeout_ms:1000});await new Promise(r=>setTimeout(r,80));
 assert.match((await call('chat_wait',{timeout_ms:0})).error.message,/already active/);
 const chat=chatUi(f.root,{action:'create'}).session;chatUi(f.root,{action:'send',chat_id:chat.id,message_id:'url-message',text:'Do the work'});
 const opened=(await waiting).result.structuredContent;assert.equal(opened.status,'assigned');assert.equal(opened.chat_id,chat.id);assert.ok(opened.skill.text);assert.ok(opened.session);
 assert.ok((await call('chat_wait',{timeout_ms:0,chat_id:'other'})).error);
 const message=(await call('chat_wait',{timeout_ms:0})).result.structuredContent;assert.equal(message.status,'message');assert.equal(message.message.id,'url-message');
 assert.equal((await call('chat_reply',{message_id:'url-reply',reply_to:'url-message',text:'Done',final:true})).result.structuredContent.persisted,true);
 assert.equal((await call('chat_wait',{timeout_ms:0})).result.structuredContent.status,'idle');
 assert.ok((await rpc('initialize')).result,'reinitialize needs no exchange receipt');
 const again=(await call('seed_connect')).result.structuredContent;assert.equal(again.attachment_id,opened.attachment_id);
 for(const body of [readFileSync(path.join(f.root,'docs/chat-sessions/seed-library.seeds.json'),'utf8'),JSON.stringify(chatUi(f.root,{action:'seed_list'})),JSON.stringify(init)])assert.ok(!body.includes(seed.url_key));
 assert.equal((await call('chat_close')).result.structuredContent.ok,true);
 const retired=(await call('chat_wait',{timeout_ms:0})).result.structuredContent;
 assert.equal(retired.status,'retired');assert.match(retired.instruction,/Stop waiting/);
 chatUi(f.root,{action:'seed_retire',seed_id:seed.seed_id});assert.ok((await rpc('initialize')).error);assert.equal((await fetch(endpoint)).status,404);
});

test('URL capability survives a new store, keeps fixed expiry, and isolates folders',async t=>{
 const {seedUrl}=await import('../dist/chat/seeds.js');
 const root=mkdtempSync(path.join(tmpdir(),'seed-url-'));t.after(()=>rmSync(root,{recursive:true,force:true}));
 const seed=chatUi(root,{action:'seed_manual',account:'url'}).batch[0],store=seedStore(root),now=Date.now();
 assert.throws(()=>seedUrl(store,seed.url_key,true,now+3600001),/expired/);
 assert.equal(seedUrl(store,seed.url_key,true,now),seed.seed_id);
 const first=JSON.parse(readFileSync(path.join(root,'docs/chat-sessions/seed-library.seeds.json'))).seeds[0];
 assert.equal(seedUrl(seedStore(root),seed.url_key,true,now+3600001),seed.seed_id);
 assert.equal(JSON.parse(readFileSync(path.join(root,'docs/chat-sessions/seed-library.seeds.json'))).seeds[0].access_until,first.access_until);
 assert.throws(()=>seedUrl(store,seed.url_key,true,now+8*86400000),/expired/);
 const other=fixture(t,1);assert.throws(()=>seedUrl(other.store,seed.url_key,true),/invalid/);
});
