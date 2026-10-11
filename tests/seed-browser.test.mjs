import test from 'node:test';
import assert from 'node:assert/strict';
import {readFileSync} from 'node:fs';
import {randomUUID} from 'node:crypto';
import vm from 'node:vm';
const source=readFileSync(new URL('../static/plugins/coderabbit-seeds.user.js',import.meta.url),'utf8');
const token='c'.repeat(64),ticket='a'.repeat(64),connection={address:'https://mcp.example/mcp/seed-library/folder',token,count:3};
async function fixture(options={}){
 const elements=new Map(),saved=new Map(),requests=[],bridges=[],timers=[],sessions=new Map(),local=new Map(),batches=new Map();let counter=0;
 sessions.set('user',JSON.stringify({state:{user:{id:'user-a',providerLogin:'alice',providerUserId:'123'},currentOrganization:{id:'org-a',organization_name:'team',provider_organization_id:'777',workspace_id:'ws-a',type:'github'}}}));sessions.set('accessToken','synthetic-login-token');local.set('clerkGitProvider','github');local.set('coding-agent-last-selected-repository','team/repo');
 const el=name=>{if(!elements.has(name))elements.set(name,{value:name==='#count'?'3':'',disabled:false,textContent:'',classList:{toggle(){},add(){}}});return elements.get(name);};
 const page={sessionStorage:{getItem:k=>sessions.get(k)??null},localStorage:{getItem:k=>local.get(k)??null},location:{pathname:'/code/new'},document:{querySelector:()=>({getAttribute:()=>`Repository: ${options.repoLabel??'repo'}`}),querySelectorAll:()=>[{getAttribute:()=>`Branch: ${options.branch??'main'}`} ]},fetch:async(url,init)=>{requests.push({url,init});if(url.includes('getAllRepos'))return {ok:true,json:async()=>[{result:{data:{repos:[{id:'1412803600',name:'team/repo'}]}}}]};if(options.create)await options.create(requests);if(options.unknown)throw Error('network');return {ok:true,json:async()=>[{result:{data:{taskId:randomUUID()}}}]};}};
 const panel={style:{},attachShadow:()=>({set innerHTML(v){},querySelector:el}),isConnected:false};
 if(options.restore)saved.set('ctmcp-seed-connection-v2',connection);
 const sandbox={unsafeWindow:page,window:{confirm:()=>true},document:{createElement:()=>panel,readyState:'complete',body:{append(){panel.isConnected=true;}}},URL,Headers,Request,AbortSignal,console,crypto:{randomUUID},setInterval:fn=>timers.push(fn),setTimeout:fn=>{options.delay?.({sessions,local});fn();},GM_getValue:(k,f)=>saved.get(k)??f,GM_setValue:(k,v)=>saved.set(k,v),GM_registerMenuCommand(){},GM_xmlhttpRequest:args=>{
   const body=JSON.parse(args.data);bridges.push({args,body});
   if(body.method==='seed/created'){if(options.registerFailure){options.registerFailure=false;args.ontimeout();return;}args.onload({status:200,responseText:JSON.stringify({result:{ok:true}})});return;}
   let result={workspace_folder_id:'folder',ok:true,total:0,capacity:10000};
   if(body.action==='batch'){
    if(!batches.has(body.request_id))batches.set(body.request_id,Array.from({length:body.count},()=>({seed_id:'seed-'+(++counter),ticket,prefix:body.prefix??'',source:'coderabbit',repo_id:body.repo_id,branch:body.branch,account:body.account,expires_at:Date.now()+3600000})));
    result.batch=batches.get(body.request_id);
    if(options.batchFailure){options.batchFailure=false;args.ontimeout();return;}
   }
   options.bridge?.({body,result,sessions,local});args.onload({status:200,responseText:JSON.stringify(result)});
 }};
 vm.runInNewContext(source,sandbox);await new Promise(setImmediate);
 el('#address').value=connection.address;el('#token').value=token;
 if(!options.restore)await el('#bind').onclick();
 return {el,page,saved,requests,bridges,timers,sessions,local,sandbox,options};
}
const creations=f=>f.requests.filter(r=>r.url.includes('enqueueCodingTask'));
test('paired browser identifies account, organization, workspace, repository ID and branch; persists only pairing and receipts',async()=>{
 const f=await fixture();f.el('#count').value='12';await f.el('#start').onclick();assert.equal(creations(f).length,12);
 assert.deepEqual(f.bridges.filter(b=>b.body.action==='batch').map(b=>b.body.count),[10,2]);
 const first=creations(f)[0],body=JSON.parse(first.init.body)['0'];assert.equal(body.repoId,'1412803600');assert.equal(body.branch_name,'main');assert.equal(first.init.headers.authorization,'Bearer synthetic-login-token');assert.equal(first.init.headers['x-coderabbitai-organization'],'org-a');assert.equal(first.init.headers['x-coderabbitai-workspace'],'ws-a');
 const stored=JSON.stringify([...f.saved.values()]);assert.ok(stored.includes(token));assert.ok(!stored.includes(ticket));assert.ok(!stored.includes('synthetic-login-token'));assert.equal(f.bridges.filter(b=>b.body.method==='seed/created').length,12);assert.match(f.el('.log').textContent,/12 颗/);
});
test('restores pairing and retries uncertain provisioning with same request ID, never duplicating submitted tasks',async()=>{
 const f=await fixture({restore:true,batchFailure:true});await f.el('#start').onclick();assert.equal(creations(f).length,0);await f.el('#start').onclick();assert.equal(creations(f).length,3);const calls=f.bridges.filter(b=>b.body.action==='batch');assert.equal(calls[0].body.request_id,calls[1].body.request_id);
});
test('stops on changed account, provider, organization, repository, branch, or logout and reads rotating token per request',async()=>{
 for(const change of ['account','provider','organization','repository','branch','logout','rotate']){
  const f=await fixture();f.sandbox.setTimeout=fn=>{if(change==='account'||change==='organization'){const state=JSON.parse(f.sessions.get('user'));if(change==='account')state.state.user.id='user-b';else state.state.currentOrganization.id='org-b';f.sessions.set('user',JSON.stringify(state));}if(change==='provider')f.local.set('clerkGitProvider','gitlab');if(change==='repository')f.local.set('coding-agent-last-selected-repository','different');if(change==='branch')f.options.branch='other';if(change==='logout')f.sessions.delete('accessToken');if(change==='rotate')f.sessions.set('accessToken','rotated-token');fn();};
  await f.el('#start').onclick();assert.equal(creations(f).length,change==='rotate'?3:1,change);if(change==='rotate')assert.equal(creations(f)[1].init.headers.authorization,'Bearer rotated-token');
 }
});
test('context changed during confirmation or provisioning cannot create in stale context',async()=>{
 for(const stage of ['confirm','batch']){const f=await fixture();const change=()=>f.local.set('coding-agent-last-selected-repository','other');if(stage==='confirm')f.sandbox.window.confirm=()=>{change();return true;};else f.options.bridge=({body})=>{if(body.action==='batch')change();};await f.el('#start').onclick();assert.equal(creations(f).length,0);}
});
test('unknown create outcome blocks resubmission and records no secrets',async()=>{
 const f=await fixture({unknown:true});await f.el('#start').onclick();await f.el('#start').onclick();assert.equal(creations(f).length,1);assert.match(f.el('.log').textContent,/结果不明/);assert.ok(JSON.stringify([...f.saved.values()]).includes('unknown'));
});
test('missing session, stale repository label, wrong route and wrong client folder block creation',async()=>{
 for(const change of ['session','label','route','folder']){const f=await fixture();if(change==='session')f.sessions.delete('user');if(change==='label')f.options.repoLabel='wrong';if(change==='route')f.page.location.pathname='/code';if(change==='folder')f.options.bridge=({body,result})=>{if(body.action==='batch')result.workspace_folder_id='other';};await f.el('#start').onclick();assert.equal(creations(f).length,0,change);}
});

test('failed host registration resumes registration without creating a second task; pending rounds cannot change pairing',async()=>{
 const f=await fixture({registerFailure:true});f.el('#count').value='1';await f.el('#start').onclick();assert.equal(creations(f).length,1);
 await f.el('#bind').onclick();assert.match(f.el('.log').textContent,/结束本轮/);
 // Existing connection remains valid when changing it was rejected.
 await f.el('#start').onclick();assert.equal(creations(f).length,1);assert.equal(f.bridges.filter(b=>b.body.method==='seed/created').length,2);assert.match(f.el('.log').textContent,/已创建并登记/);
});

test('browser snapshots a model prefix across batches, persists it, and includes it in host task titles',async()=>{
 const f=await fixture();f.el('#prefix').value='GPT-6-astra';f.el('#count').value='11';await f.el('#start').onclick();
 assert.equal(creations(f).length,11);assert.ok(f.bridges.filter(b=>b.body.action==='batch').every(b=>b.body.prefix==='GPT-6-astra'));
 assert.ok(creations(f).every(r=>JSON.parse(r.init.body)['0'].title.startsWith('GPT-6-astra MCP Seed ')));
 assert.equal(f.saved.get('ctmcp-seed-connection-v2').prefix,'GPT-6-astra');
});
