import test from 'node:test';
import assert from 'node:assert/strict';
import {readFileSync,writeFileSync,renameSync,existsSync,mkdtempSync,rmSync} from 'node:fs';
import {tmpdir} from 'node:os';
import path from 'node:path';
import {chatUi} from '../dist/chat/store.js';
import {shareToolAllowed} from '../dist/server/routes/share.js';
import {tunnelPathAllowed} from '../dist/tunnel.js';
import {createMcpFixture} from './mcpTestHelpers.mjs';

test('share link URL connects without headers, binds the project and enforces read-only',async t=>{
  const f=await createMcpFixture(t);
  writeFileSync(path.join(f.root,'hello.txt'),'shared hello');
  assert.equal(chatUi(f.root,{action:'share_status'}).enabled,false);
  const created=chatUi(f.root,{action:'share_create',access:'readonly'});
  assert.match(created.url_key,/^[a-f0-9]{64}$/);
  assert.ok(!readFileSync(path.join(f.root,'docs/chat-sessions/project.share.json'),'utf8').includes(created.url_key),'only the hash is stored');
  assert.deepEqual(chatUi(f.root,{action:'list'}).sessions,[],'the share file is not a chat');
  const url=`${f.endpoint}/share/repo/${created.url_key}`;let seq=0;
  const rpc=async(method,params={},target=url,notification=false)=>{const r=await fetch(target,{method:'POST',headers:{'content-type':'application/json'},body:JSON.stringify({jsonrpc:'2.0',...(notification?{}:{id:++seq}),method,params})});return r.status===202?{accepted:true}:r.json();};
  const init=await rpc('initialize',{protocolVersion:'2025-03-26',capabilities:{},clientInfo:{name:'t',version:'1'}});
  assert.match(init.result.instructions,/Read-only/);
  assert.deepEqual(await rpc('notifications/initialized',{},url,true),{accepted:true});
  const names=(await rpc('tools/list')).result.tools.map(tool=>tool.name);
  assert.ok(names.includes('read_file'));
  for(const hidden of ['apply_patch','exec_command','switch_workspace_folder','chat_wait'])assert.ok(!names.includes(hidden),hidden);
  const schema=(await rpc('tools/list')).result.tools.find(tool=>tool.name==='read_file').inputSchema;
  assert.ok(!(schema.required??[]).includes('workspace_folder_id'));
  const read=await rpc('tools/call',{name:'read_file',arguments:{path:'hello.txt'}});
  assert.match(JSON.stringify(read.result),/shared hello/);
  assert.match((await rpc('tools/call',{name:'apply_patch',arguments:{patch:'x'}})).error.message,/not available/);
  assert.ok((await rpc('tools/list',{},`${f.endpoint}/share/repo/${'0'.repeat(64)}`)).error);
  assert.ok((await rpc('tools/list',{},`${f.endpoint}/share/other/${created.url_key}`)).error);
  const full=chatUi(f.root,{action:'share_create',access:'full'});
  assert.ok((await rpc('tools/list')).error,'regenerating revokes the old URL');
  const fullNames=(await rpc('tools/list',{},`${f.endpoint}/share/repo/${full.url_key}`)).result.tools.map(tool=>tool.name);
  assert.ok(fullNames.includes('apply_patch'));
  chatUi(f.root,{action:'share_revoke'});
  assert.ok((await rpc('tools/list',{},`${f.endpoint}/share/repo/${full.url_key}`)).error);
  assert.equal((await fetch(`${f.endpoint}/share/repo/${full.url_key}`)).status,404);
});

test('share tool policy matches the Rust gateway',()=>{
  assert.ok(shareToolAllowed('read_file','readonly'));
  assert.ok(!shareToolAllowed('apply_patch','readonly'));
  assert.ok(shareToolAllowed('apply_patch','full'));
  for(const name of ['switch_workspace_folder','chat_open','request_permissions','conversation_bootstrap'])assert.ok(!shareToolAllowed(name,'full'),name);
});

test('tunnel forwards share URLs only with a full key',()=>{
  const config={publicBaseUrl:'https://x.example.test'};
  assert.equal(tunnelPathAllowed(config,`/mcp/share/repo/${'a'.repeat(64)}`),true);
  assert.equal(tunnelPathAllowed(config,'/mcp/share/repo/short'),false);
  assert.equal(tunnelPathAllowed(config,`/mcp/share/repo/${'a'.repeat(64)}/extra`),false);
});

test('v0.1.158 share-link.json no longer breaks the chat list and is migrated',t=>{
  const root=mkdtempSync(path.join(tmpdir(),'share-legacy-'));t.after(()=>rmSync(root,{recursive:true,force:true}));
  const chat=chatUi(root,{action:'create'}).session;
  chatUi(root,{action:'share_create',access:'full'});
  const dir=path.join(root,'docs/chat-sessions');
  renameSync(path.join(dir,'project.share.json'),path.join(dir,'share-link.json'));
  assert.deepEqual(chatUi(root,{action:'list'}).sessions.map(s=>s.id),[chat.id]);
  assert.ok(!existsSync(path.join(dir,'share-link.json')));
  assert.equal(chatUi(root,{action:'share_status'}).access,'full');
});

test('list reports connecting until the greeting arrives',t=>{
  const root=mkdtempSync(path.join(tmpdir(),'connecting-'));t.after(()=>rmSync(root,{recursive:true,force:true}));
  const chat=chatUi(root,{action:'create',seed_manual:true}).session;
  assert.equal(chatUi(root,{action:'list'}).sessions[0].connecting,false);
  chatUi(root,{action:'request_connection',chat_id:chat.id,message_id:'pair-1'});
  assert.equal(chatUi(root,{action:'list'}).sessions[0].connecting,true);
});
