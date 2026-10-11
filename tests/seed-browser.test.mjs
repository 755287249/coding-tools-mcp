import test from 'node:test';
import assert from 'node:assert/strict';
import {readFileSync} from 'node:fs';
import vm from 'node:vm';
const source=readFileSync(new URL('../static/plugins/coderabbit-seeds.user.js',import.meta.url),'utf8');
const seed={seed_id:'00000000-0000-4000-a000-000000000001',ticket:'a'.repeat(64),repo_id:'1412803600',branch:'main',account:'test',expires_at:Date.now()+3600000};
function fixture(){
 const elements=new Map(),saved=new Map(),requests=[],timers=[];
 const el=name=>{if(!elements.has(name))elements.set(name,{value:'',checked:false,disabled:false,textContent:'',classList:{toggle(){},add(){}}});return elements.get(name);};
 const page={Clerk:{user:{id:'user-a'},session:{getToken:async()=> 'synthetic-login-token'}},fetch:async(url,init)=>{requests.push({url,init});return {ok:true,json:async()=>[{result:{data:{taskId:seed.seed_id}}}]};},location:{reload(){}}};
 const panel={style:{},attachShadow:()=>({set innerHTML(v){},querySelector:el}),isConnected:false};
 const sandbox={unsafeWindow:page,window:{confirm:()=>true},document:{createElement:()=>panel,readyState:'complete',body:{append(){panel.isConnected=true;}}},URL,Headers,Request,AbortSignal,console,
   setInterval:fn=>timers.push(fn),setTimeout:fn=>{fn();},GM_getValue:(k,f)=>saved.get(k)??f,GM_setValue:(k,v)=>saved.set(k,v),GM_registerMenuCommand(){},GM_xmlhttpRequest:args=>args.onload({status:200,responseText:'{"result":{"ok":true}}'})};
 vm.runInNewContext(source,sandbox);
 el('#batch').value=JSON.stringify({version:1,endpoint:'https://mcp.example/mcp',workspace_folder_id:'folder',seeds:[seed]});
 return {el,page,saved,requests,timers,observe:(org,workspace)=>page.fetch('/trpc/codingAgent.listTasks',{headers:{'x-coderabbitai-organization':org,...(workspace?{'x-coderabbitai-workspace':workspace}:{})}})};
}
test('userscript creates with automatically observed organization/workspace, never persisting credentials',async()=>{
 const f=fixture();await f.observe('organization-a','workspace-a');
 assert.equal(f.el('#org').value,'organization-a');assert.equal(f.el('#workspace').value,'workspace-a');assert.equal(f.el('#org').readOnly,true);
 await f.el('#start').onclick();
 const calls=f.requests.filter(r=>r.url.includes('enqueueCodingTask'));assert.equal(calls.length,1);
 assert.equal(calls[0].init.headers['x-coderabbitai-organization'],'organization-a');assert.equal(calls[0].init.headers['x-coderabbitai-workspace'],'workspace-a');
 assert.equal(JSON.parse(calls[0].init.body)['0'].repoId,'1412803600');
 const stored=JSON.stringify([...f.saved.values()]);assert.ok(!stored.includes(seed.ticket));assert.ok(!stored.includes('synthetic-login-token'));
});
test('switching account clears identifiers; missing organization blocks submission',async()=>{
 const f=fixture();await f.observe('organization-a','workspace-a');f.page.Clerk.user={id:'user-b'};f.timers[0]();
 assert.equal(f.el('#org').value,'');assert.equal(f.el('#workspace').value,'');await f.el('#start').onclick();
 assert.equal(f.requests.filter(r=>r.url.includes('enqueueCodingTask')).length,0);assert.match(f.el('.log').textContent,/尚未识别/);
 await f.observe('organization-b');assert.equal(f.el('#workspace').value,'');
});
test('a routing change during token acquisition prevents submitting to stale workspace even with manual override',async()=>{
 for(const manual of [false,true]){
  const f=fixture();await f.observe('organization-a','workspace-a');
  if(manual){f.el('#manual').checked=true;f.el('#manual').onchange();}
  let resolveToken;f.page.Clerk.session.getToken=()=>new Promise(resolve=>{resolveToken=resolve;});
  const running=f.el('#start').onclick();await f.observe('organization-b','workspace-b');resolveToken('synthetic-login-token');await running;
  assert.equal(f.requests.filter(r=>r.url.includes('enqueueCodingTask')).length,0);assert.match(f.el('.log').textContent,/已切换/);
 }
});
