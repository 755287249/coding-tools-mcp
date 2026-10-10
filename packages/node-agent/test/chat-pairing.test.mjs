import test from 'node:test';
import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import path from 'node:path';
import { spawnSync } from 'node:child_process';
import { PairingRegistry } from '../dist/chat/pairing.js';
import { chatUi, chatTool } from '../dist/chat/store.js';
import { createMcpFixture } from './mcpTestHelpers.mjs';

test('pairing tickets expire, isolate folders, replace old attempts and never renew on probes', () => {
  let now=100;const r=new PairingRegistry(()=>now);
  const a=r.issue('/root','chat','attempt');
  assert.deepEqual(r.issue('/root','chat','attempt'),a);
  assert.equal(r.mark(a.ticket,['/other']),false);
  assert.equal(r.mark(a.ticket,['/root']),true);
  now++;assert.equal(r.mark(a.ticket,['/root']),true);
  assert.equal(r.status('/root','chat').started_at,100);
  assert.equal(r.status('/other','chat'),null);
  assert.equal(r.status('/root','chat').ticket,undefined);
  now=a.expires_at;assert.equal(r.mark(a.ticket,['/root']),false);
  assert.equal(r.status('/root','chat'),null);
  const b=r.issue('/root','chat','next');r.issue('/root','chat','replacement');
  assert.equal(r.mark(b.ticket,['/root']),false);
  for(let i=0;i<300;i++)r.issue('/root','chat'+i,'new');
  assert.equal(r.status('/root','chat0'),null);
  assert.ok(r.status('/root','chat299'));
});

test('early HTTP pairing supports prefixed routes without granting MCP or writing chat history',async t=>{
  const f=await createMcpFixture(t);
  const chat=chatUi(f.root,{action:'create'}).session.id;
  const archive=path.join(f.root,'docs/chat-sessions',chat+'.json');
  const original=readFileSync(archive,'utf8');
  const prepared=chatUi(f.root,{action:'prepare_pairing',chat_id:chat,message_id:'pairing-http'}).pairing;
  const headers={'User-Agent':'PairingTest','X-Chat-Pairing':prepared.ticket};
  const url=f.endpoint+'/pairing';
  assert.equal((await fetch(url)).status,405);
  assert.equal((await fetch(url,{method:'POST'})).status,404);
  const response=await fetch(url,{method:'POST',headers});
  assert.equal(response.status,202);assert.equal(response.headers.get('cache-control'),'no-store');
  assert.deepEqual(await response.json(),{status:'preparing',authenticated:false});
  const session=chatUi(f.root,{action:'read',chat_id:chat}).session;
  assert.ok(session.pairing.started_at);assert.equal(session.status,'offline');assert.equal(session.connection_id,undefined);
  assert.equal(readFileSync(archive,'utf8'),original);
  assert.ok(!JSON.stringify(session).includes(prepared.ticket));
  const forbidden=await fetch(f.endpoint,{method:'POST',headers:{...headers,'Content-Type':'application/json'},body:JSON.stringify({jsonrpc:'2.0',id:1,method:'initialize',params:{protocolVersion:'2025-11-25',capabilities:{},clientInfo:{name:'pairing',version:'1'}}})});
  assert.equal(forbidden.status,401);
  const other=await createMcpFixture(t);assert.equal((await fetch(other.endpoint+'/pairing',{method:'POST',headers})).status,404);
  chatUi(f.root,{action:'close',chat_id:chat});assert.throws(()=>chatUi(f.root,{action:'prepare_pairing',chat_id:chat,message_id:'closed'}),/closed/);
});

test('group pairing greets the new member only and preserves resume and other members work',async t=>{
  const f=await createMcpFixture(t);
  const chat_id=chatUi(f.root,{action:'create',mode:'group'}).session.id;
  const chief=chatTool(f.root,'chat_open',{chat_id,agent_name:'Chief'});
  chatUi(f.root,{action:'prepare_pairing',chat_id,message_id:'new-member'});
  const member=chatTool(f.root,'chat_open',{chat_id,agent_name:'Member'});
  assert.equal(chatTool(f.root,'chat_wait',{chat_id,attachment_id:chief.attachment_id}).status,'idle');
  const args={chat_id,attachment_id:member.attachment_id};
  assert.equal(chatTool(f.root,'chat_wait',args).message.id,'new-member');
  chatTool(f.root,'chat_reply',{...args,message_id:'hello',reply_to:'new-member',text:'你好，有什么能帮到你？',final:true});
  chatTool(f.root,'chat_open',args);
  const s=chatUi(f.root,{action:'read',chat_id}).session;
  assert.equal(s.messages.filter(m=>m.kind==='connection_request').length,1);
  assert.equal(s.messages.at(-1).agent_id,member.agent_id);
  assert.equal(chatTool(f.root,'chat_wait',args).status,'idle');
});


test('group greeting survives loss of the in-memory early status ticket',async t=>{
 const f=await createMcpFixture(t);const chat_id=chatUi(f.root,{action:'create',mode:'group'}).session.id;
 const prepared=chatUi(f.root,{action:'prepare_pairing',chat_id,message_id:'after-restart'}).pairing;
 const archive=readFileSync(path.join(f.root,'docs/chat-sessions',chat_id+'.json'),'utf8');
 assert.ok(!archive.includes(prepared.ticket));
 const module=new URL('../dist/chat/store.js',import.meta.url).href;
 const script=`import{chatTool}from ${JSON.stringify(module)};const [root,chat_id]=process.argv.slice(1);const opened=chatTool(root,'chat_open',{chat_id,agent_name:'Restarted'});const r=chatTool(root,'chat_wait',{chat_id,attachment_id:opened.attachment_id});if(r.message?.id!=='after-restart')throw Error('Missing greeting after restart');console.log('Restart greeting delivered');`;
 const child=spawnSync(process.execPath,['--input-type=module','-e',script,f.root,chat_id],{encoding:'utf8'});
 assert.equal(child.status,0,child.stderr);assert.match(child.stdout,/Restart greeting delivered/);
});
