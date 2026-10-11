import test from 'node:test';
import assert from 'node:assert/strict';
import {readFileSync} from 'node:fs';
import vm from 'node:vm';
const source=readFileSync(new URL('../static/plugins/coderabbit-seeds.user.js',import.meta.url),'utf8');
const seed={seed_id:'00000000-0000-4000-a000-000000000001',ticket:'a'.repeat(64),repo_id:'1412803600',branch:'main',account:'test',expires_at:Date.now()+3600000};
function fixture(){
 const elements=new Map(),saved=new Map(),requests=[],timers=[],sessionStorage=new Map(),localStorage=new Map();
 sessionStorage.set('user',JSON.stringify({state:{user:{id:'user-a'}},version:1}));sessionStorage.set('accessToken','synthetic-login-token');localStorage.set('clerkGitProvider','github');
 const el=name=>{if(!elements.has(name))elements.set(name,{value:'',checked:false,disabled:false,textContent:'',classList:{toggle(){},add(){}}});return elements.get(name);};
 const page={sessionStorage:{getItem:k=>sessionStorage.get(k)??null},localStorage:{getItem:k=>localStorage.get(k)??null},fetch:async(url,init)=>{requests.push({url,init});return {ok:true,json:async()=>[{result:{data:{taskId:seed.seed_id}}}]};},location:{reload(){}}};
 const panel={style:{},attachShadow:()=>({set innerHTML(v){},querySelector:el}),isConnected:false};
 const sandbox={unsafeWindow:page,window:{confirm:()=>true},document:{createElement:()=>panel,readyState:'complete',body:{append(){panel.isConnected=true;}}},URL,Headers,Request,AbortSignal,console,
   setInterval:fn=>timers.push(fn),setTimeout:fn=>{fn();},GM_getValue:(k,f)=>saved.get(k)??f,GM_setValue:(k,v)=>saved.set(k,v),GM_registerMenuCommand(){},GM_xmlhttpRequest:args=>args.onload({status:200,responseText:'{"result":{"ok":true}}'})};
 vm.runInNewContext(source,sandbox);
 el('#batch').value=JSON.stringify({version:1,endpoint:'https://mcp.example/mcp',workspace_folder_id:'folder',seeds:[seed]});
 return {el,page,saved,requests,timers,sessionStorage,localStorage,sandbox,observe:(org,workspace)=>page.fetch('/trpc/codingAgent.listTasks',{headers:{'x-coderabbitai-organization':org,...(workspace?{'x-coderabbitai-workspace':workspace}:{})}})};
}
test('userscript creates with automatically observed organization/workspace, never persisting credentials',async()=>{
 const f=fixture();await f.observe('organization-a','workspace-a');
 assert.equal(f.page.Clerk,undefined);
 assert.equal(f.el('#org').value,'organization-a');assert.equal(f.el('#workspace').value,'workspace-a');assert.equal(f.el('#org').readOnly,true);
 await f.el('#start').onclick();
 const calls=f.requests.filter(r=>r.url.includes('enqueueCodingTask'));assert.equal(calls.length,1);
 assert.equal(calls[0].init.headers['x-coderabbitai-organization'],'organization-a');assert.equal(calls[0].init.headers['x-coderabbitai-workspace'],'workspace-a');
 assert.equal(calls[0].init.headers.authorization,'Bearer synthetic-login-token');assert.equal(calls[0].init.headers['x-clerk-git-provider'],'github');
 assert.equal(JSON.parse(calls[0].init.body)['0'].repoId,'1412803600');
 const stored=JSON.stringify([...f.saved.values()]);assert.ok(!stored.includes(seed.ticket));assert.ok(!stored.includes('synthetic-login-token'));
});
test('switching account clears identifiers; missing organization blocks submission',async()=>{
 const f=fixture();await f.observe('organization-a','workspace-a');f.sessionStorage.set('user',JSON.stringify({state:{user:{id:'user-b'}}}));f.timers[0]();
 assert.equal(f.el('#org').value,'');assert.equal(f.el('#workspace').value,'');await f.el('#start').onclick();
 assert.equal(f.requests.filter(r=>r.url.includes('enqueueCodingTask')).length,0);assert.match(f.el('.log').textContent,/尚未识别/);
 await f.observe('organization-b');assert.equal(f.el('#workspace').value,'');
});
test('a routing change during confirmation prevents submitting to stale workspace even with manual override',async()=>{
 for(const manual of [false,true]){
  const f=fixture();await f.observe('organization-a','workspace-a');
  if(manual){f.el('#manual').checked=true;f.el('#manual').onchange();}
  f.sandbox.window.confirm=()=>{f.observe('organization-b','workspace-b');return true;};
  await f.el('#start').onclick();
  assert.equal(f.requests.filter(r=>r.url.includes('enqueueCodingTask')).length,0);assert.match(f.el('.log').textContent,/已切换/);
 }
});
test('missing or inaccessible app session blocks creation even when a Clerk SDK exists',async()=>{
 for(const invalid of ['missing-token','missing-user','malformed-user','blocked-storage']){
  const f=fixture();await f.observe('organization-a');
  f.page.Clerk={user:{id:'clerk-user'},session:{getToken(){throw Error('must not request Clerk token');}}};
  if(invalid==='missing-token')f.sessionStorage.delete('accessToken');
  if(invalid==='missing-user')f.sessionStorage.delete('user');
  if(invalid==='malformed-user')f.sessionStorage.set('user','{');
  if(invalid==='blocked-storage')f.page.sessionStorage.getItem=()=>{throw Error('denied');};
  await f.el('#start').onclick();
  assert.equal(f.requests.filter(r=>r.url.includes('enqueueCodingTask')).length,0);
  assert.match(f.el('.log').textContent,/登录会话尚未就绪/);
  assert.equal(f.saved.size,0);
 }
});
test('each submission uses the latest app token and stops on logout, account or provider changes',async()=>{
 for(const change of ['rotate','logout','account','provider']){
  const f=fixture();await f.observe('organization-a');
  const batch=JSON.parse(f.el('#batch').value);batch.seeds.push({...seed,seed_id:'00000000-0000-4000-a000-000000000002'});f.el('#batch').value=JSON.stringify(batch);
  f.sandbox.setTimeout=fn=>{
   if(change==='rotate')f.sessionStorage.set('accessToken','rotated-token');
   if(change==='logout')f.sessionStorage.delete('accessToken');
   if(change==='account')f.sessionStorage.set('user',JSON.stringify({state:{user:{id:'user-b'}}}));
   if(change==='provider')f.localStorage.set('clerkGitProvider','gitlab');
   fn();
  };
  await f.el('#start').onclick();
  const calls=f.requests.filter(r=>r.url.includes('enqueueCodingTask'));
  assert.equal(calls.length,change==='rotate'?2:1);
  if(change==='rotate')assert.equal(calls[1].init.headers.authorization,'Bearer rotated-token');
  else assert.match(f.el('.log').textContent,/已退出或账号已切换/);
  const persisted=JSON.stringify([...f.saved.values()]);assert.ok(!persisted.includes('token'));assert.ok(!persisted.includes(seed.ticket));
 }
});
