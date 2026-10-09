import test from 'node:test';
import assert from 'node:assert/strict';
import { mkdtempSync, rmSync, readFileSync, mkdirSync, symlinkSync } from 'node:fs';
import { tmpdir } from 'node:os';
import path from 'node:path';
import { chatUi, chatTool, chatWait } from '../dist/chat/store.js';
import { createMcpFixture, mcpRequest, responseJson } from './mcpTestHelpers.mjs';
function fixture(t) {
  const root = mkdtempSync(path.join(tmpdir(), 'chat-test-'));
  t.after(() => rmSync(root, { recursive: true, force: true }));
  const session = chatUi(root, { action: 'create', title: '测试会话' }).session;
  const { attachment_id } = chatTool(root, 'chat_open', { chat_id: session.id });
  return { root, args: { chat_id: session.id, attachment_id } };
}
test('chat persists replies, redacts secrets, deduplicates retries and redelivers unacknowledged messages', async t => {
  const {root,args}=fixture(t);
  const send={action:'send',chat_id:args.chat_id,message_id:'user-1',text:'请检查项目'};
  chatUi(root,send);chatUi(root,send);
  assert.equal((await chatWait(root,{...args,timeout_ms:0})).message.id,'user-1');
  assert.equal((await chatWait(root,{...args,timeout_ms:0})).message.id,'user-1');
  const reply={...args,message_id:'reply-1',reply_to:'user-1',text:'password=secret123 已检查',final:true};
  assert.equal(chatTool(root,'chat_reply',reply).persisted,true);
  chatTool(root,'chat_reply',reply);
  assert.throws(()=>chatTool(root,'chat_reply',{...reply,text:'different'}),/conflict/);
  assert.equal((await chatWait(root,{...args,timeout_ms:0})).status,'idle');
  const detail=chatUi(root,{action:'read',chat_id:args.chat_id}).session;
  assert.equal(detail.messages.length,2);assert.equal(detail.attachment_id,undefined);
  const markdown=readFileSync(path.join(root,detail.archive_path),'utf8');
  assert.match(markdown,/请检查项目/);assert.doesNotMatch(markdown,/secret123/);
  assert.equal(JSON.parse(readFileSync(path.join(root,'docs/chat-sessions',args.chat_id+'.json'))).messages.length,2);
});
test('wait is live only during the request, supports cancellation and close',async t=>{
  const {root,args}=fixture(t);const controller=new AbortController();
  const wait=chatWait(root,{...args,timeout_ms:1000},controller.signal);
  assert.equal(chatUi(root,{action:'read',chat_id:args.chat_id}).session.status,'waiting');
  await assert.rejects(chatWait(root,{...args,timeout_ms:1}),/already active/);
  controller.abort();await assert.rejects(wait,/abort/i);
  assert.notEqual(chatUi(root,{action:'read',chat_id:args.chat_id}).session.status,'waiting');
  const again=chatWait(root,{...args,timeout_ms:1000});
  chatUi(root,{action:'close',chat_id:args.chat_id});
  assert.equal((await again).status,'closed');
});
test('chat protects folder boundaries, rejects wrong attachments and concurrent writers',async t=>{
  const {root,args}=fixture(t);
  assert.throws(()=>chatTool(root,'chat_open',{chat_id:args.chat_id}),/already attached/);
  assert.throws(()=>chatTool(root,'chat_reply',{...args,attachment_id:'wrong'}),/expired/);
  assert.throws(()=>chatUi(root,{action:'read',chat_id:'../../outside'}),/Invalid/);
  const other=mkdtempSync(path.join(tmpdir(),'chat-other-'));t.after(()=>rmSync(other,{recursive:true,force:true}));
  assert.throws(()=>chatUi(other,{action:'read',chat_id:args.chat_id}));
  mkdirSync(path.join(root,'docs/chat-sessions/.lock'));
  assert.throws(()=>chatUi(root,{action:'list'}),/EEXIST/);
  rmSync(path.join(root,'docs/chat-sessions/.lock'),{recursive:true});
  mkdirSync(path.join(other,'linked'));symlinkSync(path.join(root,'docs'),path.join(other,'linked/docs'),'dir');
  assert.throws(()=>chatUi(path.join(other,'linked'),{action:'list'}),/symlink/);
  await assert.rejects(chatWait(root,{...args,timeout_ms:180001}),/timeout_ms/);
});
test('real MCP catalog and explicit workspace routing complete the chat loop without platform session metadata',async t=>{
  const state=await createMcpFixture(t);
  const session=chatUi(state.root,{action:'create',title:'MCP loop'}).session;
  let seq=0;
  async function call(name,args){
    const response=await mcpRequest(state,{jsonrpc:'2.0',id:++seq,method:'tools/call',params:{name,arguments:{...args,workspace_folder_id:'repo'}}});
    assert.equal(response.status,200);const body=await responseJson(response);
    assert.equal(body.result?.structuredContent?.ok,true,JSON.stringify(body));return body.result.structuredContent;
  }
  const opened=await call('chat_open',{chat_id:session.id});
  const args={chat_id:session.id,attachment_id:opened.attachment_id};
  const waiting=call('chat_wait',{...args,timeout_ms:2000});
  chatUi(state.root,{action:'send',chat_id:session.id,message_id:'u1',text:'hello from local UI'});
  const incoming=await waiting;assert.equal(incoming.message.id,'u1');
  await call('chat_reply',{...args,reply_to:'u1',message_id:'a1',text:'hello from MCP',final:true});
  assert.equal(chatUi(state.root,{action:'read',chat_id:session.id}).session.messages.at(-1).text,'hello from MCP');
  assert.equal((await call('chat_wait',{...args,timeout_ms:0})).status,'idle');
  await call('chat_close',args);
});

test('aborting the HTTP wait clears live waiting state before the tool timeout', async t => {
  const state = await createMcpFixture(t);
  const session = chatUi(state.root, { action: 'create' }).session;
  const { attachment_id } = chatTool(state.root, 'chat_open', { chat_id: session.id });
  const controller = new AbortController();
  const request = fetch(state.endpoint, {
    method: 'POST', signal: controller.signal,
    headers: { authorization: state.authorization, 'content-type': 'application/json' },
    body: JSON.stringify({ jsonrpc: '2.0', id: 1, method: 'tools/call', params: { name: 'chat_wait', arguments: { chat_id: session.id, attachment_id, workspace_folder_id: 'repo', timeout_ms: 10000 } } })
  }).then(response => response.text()).catch(error => error);
  const status = () => chatUi(state.root, { action: 'read', chat_id: session.id }).session.status;
  const deadline = Date.now() + 2000;
  while (status() !== 'waiting' && Date.now() < deadline) await new Promise(resolve => setTimeout(resolve, 20));
  assert.equal(status(), 'waiting');
  controller.abort(); await request;
  const end = Date.now() + 1500;
  while (status() === 'waiting' && Date.now() < end) await new Promise(resolve => setTimeout(resolve, 20));
  assert.notEqual(status(), 'waiting');
});
