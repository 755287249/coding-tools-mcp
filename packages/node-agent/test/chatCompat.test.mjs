import test from 'node:test';
import assert from 'node:assert/strict';
import {randomUUID} from 'node:crypto';
import {readFileSync,writeFileSync,unlinkSync} from 'node:fs';
import path from 'node:path';
import {setTimeout as sleep} from 'node:timers/promises';
import {createMcpFixture} from './mcpTestHelpers.mjs';
import {chatUi,chatTool,chatCompatManagement} from '../dist/chat/store.js';
import {CompatGrantRegistry,compatGrants} from '../dist/chat/compat-grants.js';
import {compatSkill} from '../dist/chat/compat-skill.generated.js';
async function fixture(t){
 const f=await createMcpFixture(t),chat=chatUi(f.root,{action:'create',title:'GET trial'}).session.id,profile=f.runtime.context.workspaceProfileId;
 const issue=(message_id='attempt')=>chatCompatManagement(f.root,profile,'repo',{action:'prepare_compat',chat_id:chat,message_id}).compat;
 const grant=issue(),url=(op,extra={})=>`${f.endpoint}/chat-compat?${new URLSearchParams({key:grant.key,nonce:randomUUID(),op,...extra})}`;
 const request=async(op,extra={},init)=>{const r=await fetch(url(op,extra),init),raw=await r.text();assert.ok(!raw.includes(grant.key));return {status:r.status,body:raw?JSON.parse(raw):null,headers:r.headers,raw};};
 return {...f,chat,profile,grant,issue,url,request};
}
test('GET download/read scope, open, messages, idempotent replies, continued waiting',async t=>{
 const f=await fixture(t),info=await f.request('info');assert.equal(info.body.workspace_folder.path,f.root);assert.equal(info.body.chat_id,f.chat);assert.equal(info.body.workspace_folder.id,'repo');
 assert.match(info.headers.get('cache-control'),/no-store/);assert.equal(info.headers.get('referrer-policy'),'no-referrer');
 const open=await f.request('open');assert.equal(open.body.status,'connected');assert.equal(open.body.instruction_lines.join('\n'),compatSkill);
 const file=path.join(f.root,'download.json');writeFileSync(file,open.raw);assert.equal(JSON.parse(readFileSync(file,'utf8')).protocol,'chat-get-v1');unlinkSync(file);
 assert.equal((await f.request('open')).body.status,'connected');
 chatUi(f.root,{action:'request_connection',chat_id:f.chat,message_id:'greeting'});
 const received=(await f.request('wait',{timeout_ms:'0'})).body;assert.equal(received.status,'message');assert.equal(received.message.kind,'connection_request');
 const data=JSON.stringify({message_id:'answer',reply_to:received.message.id,text:'你好，有什么能帮到你？',final:true});
 for(let i=0;i<2;i++)assert.equal((await f.request('reply',{data})).body.persisted,true);
 assert.equal((await f.request('reply',{data:JSON.stringify({...JSON.parse(data),text:'changed'})})).status,400);
 assert.equal((await f.request('wait',{timeout_ms:'300'})).body.status,'idle');
 const text='中文🙂\n'.repeat(1500);chatUi(f.root,{action:'send',chat_id:f.chat,message_id:'long',text});
 const m=(await f.request('wait',{timeout_ms:'0'})).body.message;assert.equal(m.text_lines.join(''),text);assert.ok(m.text_lines.every(s=>Array.from(s).length<=120));
 assert.ok(!readFileSync(path.join(f.root,'docs/chat-sessions',f.chat+'.json'),'utf8').includes(f.grant.key));
});
test('HEAD, unsupported tools, query/scope injection, wrong keys cannot attach',async t=>{
 const f=await fixture(t);assert.equal((await f.request('open',{}, {method:'HEAD'})).status,405);
 for(const [op,extra] of [['exec_command',{}],['open',{workspace_folder_id:'other'}],['open',{chat_id:'other'}],['open',{data:'{}'}],['open',{key:'0'.repeat(64)}],['open',{nonce:''}]])assert.equal((await f.request(op,extra)).status,400);
 assert.equal((await fetch(f.url('open')+'&op=reply')).status,400);assert.equal((await f.request('wait',{timeout_ms:'0'})).status,400);
 assert.equal(chatUi(f.root,{action:'read',chat_id:f.chat}).session.status,'offline');
 await f.request('open');for(const timeout_ms of ['10001','-1','x'])assert.equal((await f.request('wait',{timeout_ms})).status,400);
 for(const extra of [{tool:'exec_command'},{chat_id:'other'},{text:'x'.repeat(2001)},{text:f.grant.key}])assert.equal((await f.request('reply',{data:JSON.stringify({message_id:'a',reply_to:'b',text:'hello',final:true,...extra})})).status,400);
});
test('registry enforces profile, expiry and revocation',()=>{
 let now=100;const r=new CompatGrantRegistry(()=>now),g=r.issue('p','f','r','c','a');assert.equal(r.find('p','f','c','a'),g);assert.throws(()=>r.get(g.key,'other'),/authorization/);
 now+=30*60_000;assert.throws(()=>r.get(g.key,'p'),/expired/);const h=r.issue('p','f','r','c','b');r.revoke('p','f','c');assert.throws(()=>r.get(h.key,'p'),/revoked/);
});
test('stable issuance, concurrent waits, revoke in flight and stale grant after detach',async t=>{
 const f=await fixture(t);assert.deepEqual(f.issue(),f.grant);await f.request('open');
 const waiting=f.request('wait',{timeout_ms:'2000'});await sleep(60);assert.equal((await f.request('wait',{timeout_ms:'0'})).status,409);
 chatCompatManagement(f.root,f.profile,'repo',{action:'revoke_compat',chat_id:f.chat});assert.equal((await waiting).status,400);assert.equal((await f.request('open')).status,400);assert.equal(chatUi(f.root,{action:'read',chat_id:f.chat}).session.status,'offline');
 chatUi(f.root,{action:'detach',chat_id:f.chat});const fresh=f.issue('again');chatUi(f.root,{action:'detach',chat_id:f.chat});
 assert.equal((await fetch(f.url('open',{key:fresh.key}))).status,400);
});
test('existing native AI, group, closed session and policy changes remain isolated',async t=>{
 const f=await fixture(t);chatTool(f.root,'chat_open',{chat_id:f.chat});assert.equal((await f.request('open')).status,400);
 chatUi(f.root,{action:'detach',chat_id:f.chat});chatUi(f.root,{action:'set_mode',chat_id:f.chat,mode:'group'});assert.throws(()=>f.issue('group'),/work conversation/);
 chatUi(f.root,{action:'set_mode',chat_id:f.chat,mode:'work'});const fresh=f.issue('policy'),get=()=>f.request('info',{key:fresh.key});
 const c=f.runtime.context.config;c.securityPolicyCustomized=true;assert.equal((await get()).status,200);c.securityPolicyCustomized=false;c.extensions.hooks.enabled=['test'];assert.equal((await get()).status,400);
 c.extensions.hooks.enabled=[];c.activeToolProfile='read-only';assert.equal((await get()).status,400);
 chatUi(f.root,{action:'close',chat_id:f.chat});assert.throws(()=>f.issue('closed'),/work conversation/);
});
test('configured prefix and root alias; runtime close revokes',async t=>{
 const f=await fixture(t),u=new URL(f.url('info'));u.pathname='/mcp/chat-compat';assert.equal((await fetch(u)).status,200);
 await f.runtime.close();assert.throws(()=>compatGrants.get(f.grant.key,f.profile),/authorization/);
});
