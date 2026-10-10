import test from 'node:test';
import assert from 'node:assert/strict';
import { mkdtempSync, rmSync, readFileSync, mkdirSync, symlinkSync, writeFileSync, existsSync } from 'node:fs';
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
for(const grouped of [false,true])for(const mode of ['merge','split'])test(`cancel one queued message safely (${grouped?'group':'work'}, ${mode})`,t=>{
  const {root,args}=fixture(t);
  if(grouped)chatUi(root,{action:'set_mode',chat_id:args.chat_id,mode:'group'});
  chatUi(root,{action:'set_queue_mode',chat_id:args.chat_id,mode});
  const read=()=>chatUi(root,{action:'read',chat_id:args.chat_id}).session;
  const send=(id,text)=>chatUi(root,{action:'send',chat_id:args.chat_id,message_id:id,text});
  const cancel=id=>chatUi(root,{action:'cancel_queued',chat_id:args.chat_id,message_id:id});
  send('active','Current work');
  for(const id of ['q1','q2','q3'])send(id,`Queued ${id}`);
  const before=read();
  cancel('q2');cancel('q2');send('q2','Queued q2');
  assert.deepEqual(read().queued_messages,before.queued_messages.filter(message=>message.id!=='q2'));
  assert.deepEqual(read().messages,before.messages);
  assert.throws(()=>send('q2','different'),/conflict/i);
  cancel('active');cancel('missing');assert.deepEqual(read().messages,before.messages);
  assert.ok(!readFileSync(path.join(root,read().archive_path),'utf8').includes('Queued q2'));
  chatTool(root,'chat_reply',{...args,message_id:'done',reply_to:'active',text:'Done',final:true});
  const delivered=chatTool(root,'chat_wait',args).message;
  assert.ok(!delivered.text.includes('Queued q2'));
  assert.ok(delivered.text.includes('Queued q1'));
  const published=read().messages;cancel('q1');assert.deepEqual(read().messages,published,'already published work is never removed');
  assert.equal(read().queued_messages.length,mode==='split'?1:0);
});
test('pickup receipts persist once for immediate and long-poll delivery', async t => {
  const {root,args}=fixture(t);
  const read=()=>chatUi(root,{action:'read',chat_id:args.chat_id}).session;
  chatUi(root,{action:'send',chat_id:args.chat_id,message_id:'u1',text:'immediate'});
  assert.equal(read().messages[0].received_at,undefined);
  const first=await chatWait(root,{...args,timeout_ms:0});
  assert.ok(first.message.received_at > 0);
  assert.equal(read().messages[0].received_at,first.message.received_at);
  assert.equal((await chatWait(root,{...args,timeout_ms:0})).message.received_at,first.message.received_at);
  chatTool(root,'chat_reply',{...args,message_id:'a1',reply_to:'u1',text:'done',final:true});
  const waiting=chatWait(root,{...args,timeout_ms:2000});
  chatUi(root,{action:'send',chat_id:args.chat_id,message_id:'u2',text:'delayed'});
  const delayed=await waiting;
  assert.equal(delayed.message.id,'u2');
  assert.ok(delayed.message.received_at > 0);
  assert.equal(read().messages.at(-1).received_at,delayed.message.received_at);
});
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
test('keepalive: the saved attachment survives lease expiry until another AI takes over', t => {
  const { root, args } = fixture(t);
  const file = path.join(root, 'docs/chat-sessions', args.chat_id + '.json');
  const expire = () => { const s = JSON.parse(readFileSync(file, 'utf8')); s.lease_until = 0; writeFileSync(file, JSON.stringify(s)); };
  expire();
  assert.doesNotThrow(() => chatTool(root, 'chat_wait', args));
  const publicId=chatUi(root,{action:'read',chat_id:args.chat_id}).session.connection_id;
  assert.match(publicId,/^[a-f0-9]{12}$/);assert.notEqual(publicId,args.attachment_id);
  chatUi(root,{action:'send',chat_id:args.chat_id,message_id:'long-task',text:'Work'});
  chatTool(root,'chat_wait',args);expire();
  assert.equal(chatTool(root,'chat_reply',{...args,reply_to:'long-task',message_id:'late-result',text:'Done',final:true}).persisted,true);
  assert.equal(chatUi(root,{action:'read',chat_id:args.chat_id}).session.connection_id,publicId);
  expire();
  assert.equal(chatTool(root, 'chat_open', args).attachment_id, args.attachment_id);
  expire();
  const other = chatTool(root, 'chat_open', { chat_id: args.chat_id });
  assert.notEqual(other.attachment_id, args.attachment_id);
  assert.throws(() => chatTool(root, 'chat_wait', args), /expired/);
});
test('chat protects folder boundaries, rejects wrong attachments and concurrent writers',async t=>{
  const {root,args}=fixture(t);
  assert.throws(()=>chatTool(root,'chat_open',{chat_id:args.chat_id}),/already attached/);
  assert.throws(()=>chatTool(root,'chat_reply',{...args,attachment_id:'wrong'}),/expired/);
  assert.throws(()=>chatUi(root,{action:'read',chat_id:'../../outside'}),/Invalid/);
  const other=mkdtempSync(path.join(tmpdir(),'chat-other-'));t.after(()=>rmSync(other,{recursive:true,force:true}));
  assert.throws(()=>chatUi(other,{action:'read',chat_id:args.chat_id}));
  mkdirSync(path.join(root,'docs/chat-sessions/.lock'));
  assert.throws(()=>chatUi(root,{action:'create'}),/Chat storage is busy/);
  assert.ok(existsSync(path.join(root,'docs/chat-sessions/.lock')));
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
  assert.match(opened.instruction,/skill.text/);
  const skillText = readFileSync(new URL('../../../skills/local-chat/SKILL.md', import.meta.url), 'utf8').replaceAll('\r\n', '\n');
  assert.equal(opened.skill.text, skillText);
  const resumed = await call('chat_open', {chat_id:session.id, attachment_id:opened.attachment_id});
  assert.equal(resumed.attachment_id, opened.attachment_id);
  assert.equal(resumed.skill.text, skillText);
  const groupId = chatUi(state.root, {action:'create', title:'Skill distribution', mode:'group'}).session.id;
  for (const agent_name of ['Coordinator', 'Member']) {
    const member = await call('chat_open', {chat_id:groupId, agent_name});
    assert.equal(member.skill.text, skillText);
    const restored = await call('chat_open', {chat_id:groupId, attachment_id:member.attachment_id});
    assert.equal(restored.attachment_id, member.attachment_id);
    assert.equal(restored.skill.text, skillText);
  }
  const resources = await responseJson(await mcpRequest(state, {jsonrpc:'2.0',id:++seq,method:'resources/list',params:{}}));
  const listed = resources.result.resources.find(resource => resource.uri === opened.skill.uri);
  assert.ok(listed);
  assert.equal(listed.text, undefined, 'discovery returns metadata, not the full skill');
  const resource = await responseJson(await mcpRequest(state, {jsonrpc:'2.0',id:++seq,method:'resources/read',params:{uri:opened.skill.uri}}));
  assert.equal(resource.result.contents[0].text, opened.skill.text);
  const denied = await mcpRequest(state, {jsonrpc:'2.0',id:++seq,method:'resources/read',params:{uri:opened.skill.uri}}, {auth:false});
  assert.equal(denied.status, 401);
  const args={chat_id:session.id,attachment_id:opened.attachment_id};
  const pairing = chatUi(state.root,{action:'request_connection',chat_id:session.id,message_id:'pair-http'});
  assert.equal(pairing.session.messages.filter(m=>m.role==='assistant').length,0);
  assert.equal((await call('chat_wait',{...args,timeout_ms:0})).message.kind,'connection_request');
  await call('chat_reply',{...args,reply_to:pairing.connection_request_id,message_id:'hello-http',text:'你好，有什么能帮到你？',final:true});
  assert.equal(chatUi(state.root,{action:'read',chat_id:session.id}).session.messages.at(-1).text,'你好，有什么能帮到你？');
  const waiting=call('chat_wait',{...args,timeout_ms:2000});
  chatUi(state.root,{action:'send',chat_id:session.id,message_id:'u1',text:'hello from local UI'});
  const incoming=await waiting;assert.equal(incoming.message.id,'u1');
  await call('chat_reply',{...args,reply_to:'u1',message_id:'p1',text:'tool result',final:false,tool_event:{name:'bash',status:'completed',input:'echo hello',output:'hello',output_truncated:false}});
  const imageBytes = 'iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAQAAAC1HAwCAAAAC0lEQVR42mP8/x8AAwMCAO+aXioAAAAASUVORK5CYII=';
  const uploaded = await call('chat_upload', { ...args, upload_id: 'http-image', name: 'test.png', data_base64: imageBytes });
  assert.equal(uploaded.attachment.mime, 'image/png');
  const boundary = await call('chat_upload', { ...args, upload_id: 'http-boundary', name: 'boundary.bin', data_base64: Buffer.alloc(512 * 1024, 7).toString('base64') });
  assert.equal(boundary.attachment.size, 512 * 1024);
  await call('chat_reply',{...args,reply_to:'u1',message_id:'a1',text:'hello from MCP',final:true,awaiting_user:true,attachment_ids:[uploaded.attachment.id]});
  const read = chatUi(state.root, { action: 'read_attachment', chat_id: session.id, upload_id: uploaded.attachment.id });
  assert.equal(read.data_base64, imageBytes);
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

test('chat waits for a short-lived external lock and preserves a stale lock', async t => {
  const { root } = fixture(t);
  const lockPath = path.join(root, 'docs/chat-sessions/.lock');
  mkdirSync(lockPath);
  const { Worker } = await import('node:worker_threads');
  const worker = new Worker(`const {workerData}=require('node:worker_threads'); setTimeout(()=>require('node:fs').rmdirSync(workerData),100)`, {eval:true,workerData:lockPath});
  const exited = new Promise((resolve,reject)=>{worker.on('exit',resolve);worker.on('error',reject)});
  assert.ok(chatUi(root,{action:'create'}).session.id);
  await exited;
});

test('attachments persist, dedupe and remain scoped to a conversation with integrity checks', async t => {
  const {root,args}=fixture(t);
  const data = Buffer.from('hello attachment');
  const upload={action:'upload',chat_id:args.chat_id,upload_id:'file-1',name:'notes.txt',data_base64:data.toString('base64')};
  const file=chatUi(root,upload).attachment;
  assert.equal(file.mime,'application/octet-stream');
  assert.deepEqual(chatUi(root,upload).attachment,file);
  assert.throws(()=>chatUi(root,{...upload,data_base64:Buffer.from('changed').toString('base64')}),/conflict/);
  assert.throws(()=>chatUi(root,{...upload,upload_id:'file-bad',name:'../bad'}),/name/);
  assert.throws(()=>chatUi(root,{...upload,upload_id:'oversize',data_base64:Buffer.alloc(2097153).toString('base64')}),/size|bytes/);
  const other=chatUi(root,{action:'create'}).session;
  assert.throws(()=>chatUi(root,{action:'send',chat_id:other.id,message_id:'cross',text:'x',attachment_ids:[file.id]}),/belong/);
  const send={action:'send',chat_id:args.chat_id,message_id:'attached',text:'',attachment_ids:[file.id]};
  chatUi(root,send);chatUi(root,send);
  assert.throws(()=>chatUi(root,{...send,text:'changed'}),/conflict/);
  const msg=(await chatWait(root,{...args,timeout_ms:0})).message;
  assert.equal(msg.attachments[0].sha256,file.sha256);
  assert.equal(msg.text,'📎');
  const download=chatUi(root,{action:'read_attachment',chat_id:args.chat_id,upload_id:file.id});
  assert.equal(download.data_base64,data.toString('base64'));
  assert.match(readFileSync(path.join(root,`docs/chat-sessions/${args.chat_id}.md`),'utf8'),/notes.txt/);
  const {writeFileSync}=await import('node:fs');writeFileSync(path.join(root,file.path),'changed');
  assert.throws(()=>chatUi(root,{action:'read_attachment',chat_id:args.chat_id,upload_id:file.id}),/changed/);
});

test('tool cards are progress events, are deduplicated and do not acknowledge a request', async t => {
  const {root,args}=fixture(t);
  chatUi(root,{action:'send',chat_id:args.chat_id,message_id:'u-tool',text:'inspect'});
  const event={...args,message_id:'tool-1',reply_to:'u-tool',text:'Read project root',final:false,tool_event:{name:'list_files',status:'completed'}};
  assert.equal(chatTool(root,'chat_reply',event).persisted,true);
  chatTool(root,'chat_reply',event);
  assert.equal((await chatWait(root,{...args,timeout_ms:0})).message.id,'u-tool');
  assert.throws(()=>chatTool(root,'chat_reply',{...event,final:true}),/final=false/);
  assert.throws(()=>chatTool(root,'chat_reply',{...event,tool_event:{name:'list_files',status:'failed'}}),/conflict/);
  const session=chatUi(root,{action:'read',chat_id:args.chat_id}).session;
  assert.equal(session.messages.filter(m=>m.tool_event).length,1);
});

test('confirmation replies acknowledge the message while preserving explicit waiting-for-user state', async t => {
  const {root,args}=fixture(t);
  chatUi(root,{action:'send',chat_id:args.chat_id,message_id:'u-question',text:'help me decide'});
  const question={...args,message_id:'q1',reply_to:'u-question',text:'Which option?',awaiting_user:true,final:true};
  assert.throws(()=>chatTool(root,'chat_reply',{...question,final:false}),/requires final=true/);
  assert.throws(()=>chatTool(root,'chat_reply',{...question,awaiting_user:'yes'}),/boolean/);
  assert.equal(chatTool(root,'chat_reply',question).persisted,true);
  chatTool(root,'chat_reply',question);
  assert.throws(()=>chatTool(root,'chat_reply',{...question,awaiting_user:false}),/conflict/);
  assert.equal((await chatWait(root,{...args,timeout_ms:0})).status,'idle');
  assert.equal(chatUi(root,{action:'read',chat_id:args.chat_id}).session.messages.at(-1).awaiting_user,true);
  assert.match(readFileSync(path.join(root,`docs/chat-sessions/${args.chat_id}.md`),'utf8'),/Reply state: awaiting_user/);
  chatUi(root,{action:'send',chat_id:args.chat_id,message_id:'u-answer',text:'Option A'});
  assert.equal((await chatWait(root,{...args,timeout_ms:0})).message.id,'u-answer');
});

test('tool input and output persist with redaction, bounds, truncation and idempotency', t => {
  const {root,args}=fixture(t);
  chatUi(root,{action:'send',chat_id:args.chat_id,message_id:'u-detail',text:'run check'});
  const reply={...args,message_id:'tool-detail',reply_to:'u-detail',text:'Command finished',final:false,tool_event:{name:'bash',status:'completed',input:'echo password=secret123',output:'token=secret456\n<script>untrusted text</script>',output_truncated:true}};
  chatTool(root,'chat_reply',reply);chatTool(root,'chat_reply',reply);
  const session=chatUi(root,{action:'read',chat_id:args.chat_id}).session;
  assert.equal(session.messages.length,2);
  const event=session.messages[1].tool_event;
  assert.doesNotMatch(event.input,/secret123/);assert.doesNotMatch(event.output,/secret456/);
  assert.equal(event.output_truncated,true);
  const markdown=readFileSync(path.join(root,session.archive_path),'utf8');
  assert.match(markdown,/Input:/);assert.match(markdown,/Output:/);assert.match(markdown,/Output truncated/);
  assert.doesNotMatch(markdown,/secret123|secret456/);
  assert.throws(()=>chatTool(root,'chat_reply',{...reply,tool_event:{...reply.tool_event,output:'different'}}),/conflict/);
  assert.throws(()=>chatTool(root,'chat_reply',{...reply,message_id:'large',tool_event:{...reply.tool_event,output:'x'.repeat(16001)}}),/16000/);
  assert.throws(()=>chatTool(root,'chat_reply',{...reply,message_id:'bad-bool',tool_event:{...reply.tool_event,output_truncated:'yes'}}),/boolean/);
});

test('Markdown tracks unread queue, actual pickup, confirmation and closure per user message', async t => {
  const {root,args}=fixture(t);
  const send=id=>chatUi(root,{action:'send',chat_id:args.chat_id,message_id:id,text:id});
  const states=()=>readFileSync(path.join(root,`docs/chat-sessions/${args.chat_id}.md`),'utf8').split('\n').filter(line=>line.startsWith('消息状态：'));
  send('u1');send('u2');
  assert.deepEqual(states(),['消息状态：未读 · 排队中']);
  await chatWait(root,{...args,timeout_ms:0});
  assert.deepEqual(states(),['消息状态：已读 · 正在处理']);
  chatTool(root,'chat_reply',{...args,message_id:'q1',reply_to:'u1',text:'Which option?',final:true,awaiting_user:true});
  assert.deepEqual(states(),['消息状态：已读 · 待确认']);
  send('u3');
  assert.equal(states()[0],'消息状态：已读 · 已回复');
  chatUi(root,{action:'close',chat_id:args.chat_id});
  assert.equal(states()[1],'消息状态：未读 · 会话已结束');
});

test('renaming persists title without changing messages, attachment ownership or archive identity', async t => {
  const { root, args } = fixture(t);
  const read = () => chatUi(root, { action: 'read', chat_id: args.chat_id }).session;
  const archive = read().archive_path;
  chatUi(root, { action: 'rename', chat_id: args.chat_id, title: '  新名称\n project  ' });
  chatUi(root, { action: 'send', chat_id: args.chat_id, message_id: 'rename-u1', text: 'Keep this message' });
  assert.equal(read().title, '新名称 project');
  assert.equal((await chatWait(root, { ...args, timeout_ms: 0 })).message.id, 'rename-u1');
  const before = JSON.parse(readFileSync(path.join(root, 'docs/chat-sessions', args.chat_id + '.json')));
  chatUi(root, { action: 'rename', chat_id: args.chat_id, title: '<b>Renamed again</b>' });
  const after = JSON.parse(readFileSync(path.join(root, 'docs/chat-sessions', args.chat_id + '.json')));
  assert.deepEqual(after.messages, before.messages);
  assert.equal(after.attachment_id, before.attachment_id);
  assert.equal(after.lease_until, before.lease_until);
  assert.equal(read().archive_path, archive);
  assert.ok(readFileSync(path.join(root, archive), 'utf8').startsWith('# <b>Renamed again</b>\n'));
  assert.equal(chatUi(root, { action: 'list' }).sessions[0].title, '<b>Renamed again</b>');
  for (const title of ['', ' \n ', '界'.repeat(81)]) assert.throws(() => chatUi(root, { action: 'rename', chat_id: args.chat_id, title }));
  assert.equal(read().title, '<b>Renamed again</b>');
  chatUi(root, { action: 'close', chat_id: args.chat_id });
  chatUi(root, { action: 'rename', chat_id: args.chat_id, title: 'Archived name' });
  assert.equal(read().closed, true);
  assert.equal(read().title, 'Archived name');
});


test('local detach releases a lost attachment, interrupts its wait and preserves the conversation', async t => {
  const { root, args } = fixture(t);
  chatUi(root, { action: 'send', chat_id: args.chat_id, message_id: 'u1', text: 'keep me' });
  chatTool(root, 'chat_reply', { ...args, message_id: 'a1', reply_to: 'u1', text: 'done', final: true });
  const before = chatUi(root, { action: 'read', chat_id: args.chat_id }).session;
  const waiting = chatWait(root, { ...args, timeout_ms: 2000 });
  const rejection = assert.rejects(waiting, /expired/);
  const detached = chatUi(root, { action: 'detach', chat_id: args.chat_id }).session;
  await rejection;
  assert.equal(detached.closed, false);
  assert.equal(detached.title, before.title);
  assert.deepEqual(detached.messages, before.messages);
  assert.equal(detached.archive_path, before.archive_path);
  assert.equal(chatUi(root, { action: 'read', chat_id: args.chat_id }).session.status, 'offline');
  assert.throws(() => chatTool(root, 'chat_reply', { ...args, message_id: 'late', reply_to: 'u1', text: 'late' }), /expired/);
  const next = chatTool(root, 'chat_open', { chat_id: args.chat_id });
  assert.notEqual(next.attachment_id, args.attachment_id);
  assert.equal((await chatWait(root, { chat_id: args.chat_id, attachment_id: next.attachment_id, timeout_ms: 0 })).status, 'idle');
  chatUi(root, { action: 'close', chat_id: args.chat_id });
  assert.equal(chatUi(root, { action: 'detach', chat_id: args.chat_id }).session.closed, true);
});


test('session summaries count only persisted assistant replies without retry inflation', t => {
  const { root, args } = fixture(t);
  const summary = () => chatUi(root, { action: 'list' }).sessions[0];
  assert.equal(summary().assistant_message_count, 0);
  chatUi(root, { action: 'send', chat_id: args.chat_id, message_id: 'u1', text: 'hello' });
  assert.equal(summary().assistant_message_count, 0);
  const reply = { ...args, message_id: 'a1', reply_to: 'u1', text: 'working', final: false };
  chatTool(root, 'chat_reply', reply); chatTool(root, 'chat_reply', reply);
  assert.equal(summary().assistant_message_count, 1);
  chatTool(root, 'chat_reply', { ...reply, message_id: 'a2', text: 'done', final: true });
  assert.equal(summary().assistant_message_count, 2);
  assert.equal(summary().messages, undefined);
});


test('AI upload requires ownership and reply references are immutable and conversation-local', t => {
  const { root, args } = fixture(t);
  const bytes = Buffer.from('AI attachment test');
  const upload = { ...args, upload_id: 'ai-file', name: 'result.txt', data_base64: bytes.toString('base64') };
  assert.throws(() => chatTool(root, 'chat_upload', { ...upload, attachment_id: 'wrong' }), /expired/);
  const first = chatTool(root, 'chat_upload', upload).attachment;
  assert.deepEqual(chatTool(root, 'chat_upload', upload).attachment, first);
  assert.deepEqual(readFileSync(path.join(root, first.path)), bytes);
  assert.throws(() => chatTool(root, 'chat_upload', { ...upload, data_base64: Buffer.from('changed').toString('base64') }), /conflict/);
  assert.throws(() => chatTool(root, 'chat_upload', { ...upload, upload_id: 'bad', name: '../bad' }), /name/);
  assert.throws(() => chatTool(root, 'chat_upload', { ...upload, upload_id: 'bad', data_base64: '%%%=' }), /encoding/);
  assert.throws(() => chatTool(root, 'chat_upload', { ...upload, upload_id: 'large', data_base64: Buffer.alloc(524289).toString('base64') }), /512 KiB/);
  const other = chatUi(root, { action: 'create' }).session;
  chatUi(root, { action: 'upload', chat_id: other.id, upload_id: 'foreign', name: 'other.txt', data_base64: 'eA==' });
  chatUi(root, { action: 'send', chat_id: args.chat_id, message_id: 'u1', text: 'send file' });
  const reply = { ...args, message_id: 'a1', reply_to: 'u1', text: 'Saved file', final: true, attachment_ids: [first.id] };
  assert.throws(() => chatTool(root, 'chat_reply', { ...reply, attachment_ids: ['foreign'] }), /does not belong/);
  assert.throws(() => chatTool(root, 'chat_reply', { ...reply, attachment_ids: [first.id, first.id] }), /unique/);
  assert.equal(chatTool(root, 'chat_reply', reply).persisted, true);
  assert.equal(chatTool(root, 'chat_reply', reply).persisted, true);
  assert.throws(() => chatTool(root, 'chat_reply', { ...reply, attachment_ids: [] }), /conflict/);
  const session = chatUi(root, { action: 'read', chat_id: args.chat_id }).session;
  assert.equal(session.messages.length, 2);
  assert.deepEqual(session.messages[1].attachments, [first]);
  assert.match(readFileSync(path.join(root, session.archive_path), 'utf8'), /Attachment: result.txt/);
  chatUi(root, { action: 'detach', chat_id: args.chat_id });
  assert.throws(() => chatTool(root, 'chat_upload', { ...upload, upload_id: 'after-detach' }), /expired/);
});


test('summary work state tracks pickup, progress, final and close without message bodies', async t => {
  const { root, args } = fixture(t);
  const state = () => chatUi(root, { action: 'list' }).sessions[0].work_state;
  assert.equal(state(), null);
  chatUi(root, { action: 'send', chat_id: args.chat_id, message_id: 'u1', text: 'hello' });
  assert.equal(state(), 'queued');
  await chatWait(root, { ...args, timeout_ms: 0 });
  assert.equal(state(), 'processing');
  chatTool(root, 'chat_reply', { ...args, message_id: 'a1', reply_to: 'u1', text: 'done', final: true });
  assert.equal(state(), null);
  chatUi(root, { action: 'send', chat_id: args.chat_id, message_id: 'u2', text: 'next' });
  chatTool(root, 'chat_reply', { ...args, message_id: 'a2', reply_to: 'u2', text: 'working', final: false });
  assert.equal(state(), 'processing');
  chatUi(root, { action: 'close', chat_id: args.chat_id });
  assert.equal(state(), null);
});

test('disk asset pool passes former count/byte quotas and still reads legacy archives', async t => {
  const {root,args}=fixture(t);
  const fs=await import('node:fs');
  const encoded=Buffer.alloc(1024*1024,7).toString('base64');
  let first;
  for(let i=0;i<33;i++) { const file=chatUi(root,{action:'upload',chat_id:args.chat_id,upload_id:`pool-${i}`,name:'asset.bin',data_base64:encoded}).attachment; first ??=file; assert.ok(file.path.startsWith(`mcp-assistant/chat-assets/${args.chat_id}/`)); }
  assert.equal(chatUi(root,{action:'read_attachment',chat_id:args.chat_id,upload_id:first.id}).data_base64,encoded);
  const archive=path.join(root,`docs/chat-sessions/${args.chat_id}.json`), state=JSON.parse(fs.readFileSync(archive,'utf8'));
  const legacy=`docs/chat-sessions/${args.chat_id}-${first.id}.bin`;
  fs.renameSync(path.join(root,first.path),path.join(root,legacy));state.files[0].path=legacy;fs.writeFileSync(archive,JSON.stringify(state));
  assert.equal(chatUi(root,{action:'read_attachment',chat_id:args.chat_id,upload_id:first.id}).data_base64,encoded);
  state.files[0].path='../outside.bin';fs.writeFileSync(archive,JSON.stringify(state));
  assert.throws(()=>chatUi(root,{action:'read_attachment',chat_id:args.chat_id,upload_id:first.id}),/path/);
});

test('local artifact references avoid byte transfer and preserve scope, hash and retry identity', async t => {
  const {root,args}=fixture(t);const fs=await import('node:fs');
  fs.mkdirSync(path.join(root,'mcp-assistant/artifacts'),{recursive:true});
  const relative='mcp-assistant/artifacts/large.bin',target=path.join(root,relative);
  fs.writeFileSync(target,Buffer.alloc(3*1024*1024,9));
  const request={...args,upload_id:'reference',name:'large.bin',source_path:relative};
  const first=chatTool(root,'chat_upload',request).attachment;
  assert.equal(first.local_reference,true);assert.equal(first.path,relative);assert.equal(first.size,3*1024*1024);
  assert.deepEqual(chatTool(root,'chat_upload',request).attachment,first);
  assert.equal(chatUi(root,{action:'read_attachment',chat_id:args.chat_id,upload_id:first.id}).local_only,true);
  assert.throws(()=>chatTool(root,'chat_upload',{...request,data_base64:'YQ=='}),/not both/);
  for(const source_path of ['../secret','/etc/passwd','mcp-assistant/artifacts/../secret','mcp-assistant/artifacts/x:stream','mcp-assistant/artifacts/dir\\secret','mcp-assistant/artifacts//secret']) assert.throws(()=>chatTool(root,'chat_upload',{...request,upload_id:'bad',source_path}),/inside/);
  fs.writeFileSync(target,'changed');
  assert.throws(()=>chatTool(root,'chat_upload',request),/conflict/);
  assert.throws(()=>chatUi(root,{action:'read_attachment',chat_id:args.chat_id,upload_id:first.id}),/changed/);
  if(process.platform!=='win32') {
    fs.symlinkSync(target,path.join(root,'mcp-assistant/artifacts/link'));
    assert.throws(()=>chatTool(root,'chat_upload',{...request,upload_id:'link',source_path:'mcp-assistant/artifacts/link'}),/symlink/);
  }
});

test('artifact preview is read-only for closed chats and rejects escapes, symlinks, nonimages and oversize files', async t=>{
  const {writeFileSync}=await import('node:fs');const {root,args}=fixture(t);
  const dir=path.join(root,'mcp-assistant/artifacts');mkdirSync(dir,{recursive:true});
  const bytes=Buffer.from('iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAQAAAC1HAwCAAAAC0lEQVR42mP8/x8AAusB9Wl2nXsAAAAASUVORK5CYII=','base64');
  writeFileSync(path.join(dir,'preview.png'),bytes);chatUi(root,{action:'close',chat_id:args.chat_id});
  const sessionPath=path.join(root,'docs/chat-sessions',args.chat_id+'.json'),before=readFileSync(sessionPath,'utf8');
  const read=source_path=>chatUi(root,{action:'read_artifact',chat_id:args.chat_id,source_path});
  const result=read('mcp-assistant/artifacts/preview.png');assert.equal(result.mime,'image/png');assert.equal(result.data_base64,bytes.toString('base64'));assert.equal(readFileSync(sessionPath,'utf8'),before);
  for(const source of ['../preview.png','mcp-assistant/artifacts/../preview.png','mcp-assistant/artifacts/x:stream','C:/preview.png'])assert.throws(()=>read(source));
  writeFileSync(path.join(dir,'fake.png'),'not an image');assert.throws(()=>read('mcp-assistant/artifacts/fake.png'),/Only PNG/);
  writeFileSync(path.join(dir,'huge.png'),Buffer.alloc(2*1024*1024+1));assert.throws(()=>read('mcp-assistant/artifacts/huge.png'),/2 MiB/);
  if(process.platform!=='win32'){symlinkSync(path.join(dir,'preview.png'),path.join(dir,'link.png'));assert.throws(()=>read('mcp-assistant/artifacts/link.png'));}
});

test('connection request is idempotent, survives reload and requires a real AI reply', async t => {
  const {root,args} = fixture(t);
  const request = {action:'request_connection', chat_id:args.chat_id, message_id:'connect-1'};
  const first = chatUi(root,request);
  assert.equal(first.connection_request_id,'connect-1');
  assert.equal(first.session.messages.length,1);
  assert.equal(first.session.messages[0].kind,'connection_request');
  assert.equal(chatUi(root,request).session.messages.length,1);
  assert.equal(chatUi(root,{...request,message_id:'connect-2'}).connection_request_id,'connect-1');
  assert.equal((await chatWait(root,{...args,timeout_ms:0})).message.id,'connect-1');
  assert.equal(chatUi(root,{action:'read',chat_id:args.chat_id}).session.messages.filter(m=>m.role==='assistant').length,0);
  assert.throws(()=>chatUi(root,{action:'send',chat_id:args.chat_id,message_id:'connect-1',text:first.session.messages[0].text}),/conflict/);
  chatTool(root,'chat_reply',{...args,message_id:'greeting',reply_to:'connect-1',text:'你好，有什么能帮到你？',final:true});
  assert.equal(chatUi(root,request).connection_request_id,'connect-1');
  assert.equal((await chatWait(root,{...args,timeout_ms:0})).status,'idle');
  const next = chatUi(root,{action:'send',chat_id:args.chat_id,message_id:'work',text:'Actual work title'}).session;
  assert.equal(next.title,'Actual work title');
  assert.match(readFileSync(path.join(root,next.archive_path),'utf8'),/接入请求/);
  assert.throws(()=>chatUi(root,{...request,message_id:'work'}),/conflict/);
  assert.equal(chatUi(root,{...request,message_id:'reconnect'}).connection_request_id,'reconnect');
  // Existing work stays first: opening the dialog must not invalidate an in-flight reply.
  assert.equal((await chatWait(root,{...args,timeout_ms:0})).message.id,'work');
  chatUi(root,{action:'close',chat_id:args.chat_id});
  assert.throws(()=>chatUi(root,{...request,message_id:'closed'}),/closed/);
});

test('chunk uploads persist large local files, stable references and safe retries',async t=>{
 const {root,args}=fixture(t);const bytes=Buffer.alloc(3*1024*1024+17,7);Buffer.from([137,80,78,71,13,10,26,10]).copy(bytes);
 const chunk=(offset,data=bytes.subarray(offset,offset+512*1024))=>({action:'upload_chunk',chat_id:args.chat_id,upload_id:'large',name:'image.png',offset,total_size:bytes.length,data_base64:data.toString('base64')});
 const first=chatUi(root,chunk(0));assert.equal(first.attachment,undefined);assert.equal(chatUi(root,chunk(0)).next_offset,512*1024);
 assert.throws(()=>chatUi(root,chunk(0,Buffer.from('wrong'))),/conflict/);
 assert.throws(()=>chatUi(root,chunk(2*512*1024)),/offset/);
 assert.throws(()=>chatUi(root,{...chunk(0),upload_id:'../escape'}),/Invalid/);
 let result;for(let offset=512*1024;offset<bytes.length;offset+=512*1024)result=chatUi(root,chunk(offset));
 const f=result.attachment;assert.equal(f.size,bytes.length);assert.equal(f.label,'图片1');assert.deepEqual(readFileSync(path.join(root,f.path)),bytes);
 assert.equal(chatUi(root,chunk(bytes.length-17)).attachment.id,'large');
 const parts=[];for(let offset=0;offset<f.size;){const part=chatUi(root,{action:'read_attachment_chunk',chat_id:args.chat_id,upload_id:f.id,offset});parts.push(Buffer.from(part.data_base64,'base64'));offset=part.next_offset;}
 assert.deepEqual(Buffer.concat(parts),bytes);
 const ids=[f.id];for(let i=0;i<6;i++){const extra=chatUi(root,{action:'upload_chunk',chat_id:args.chat_id,upload_id:'extra'+i,name:'report.txt',offset:0,total_size:1,data_base64:'YQ=='}).attachment;assert.equal(extra.label,`文件${i+1}`);ids.push(extra.id);}
 const sent=chatUi(root,{action:'send',chat_id:args.chat_id,message_id:'many',text:'参考@图片1和@文件6',attachment_ids:ids}).session;
 assert.equal(sent.messages[0].attachments.length,7);assert.equal(sent.messages[0].attachments[6].label,'文件6');
 assert.match(readFileSync(path.join(root,sent.archive_path),'utf8'),/Reference: @图片1/);
 assert.throws(()=>chatUi(root,{action:'read_attachment_chunk',chat_id:args.chat_id,upload_id:'large',offset:-1}),/range/);
 assert.equal(chatUi(root,{action:'read',chat_id:args.chat_id}).session.messages[0].attachments[0].label,'图片1');
});

test('outbox releases only at next wait, merges attachments and survives retries and process reload', async t => {
  const {root,args}=fixture(t);
  const read=()=>chatUi(root,{action:'read',chat_id:args.chat_id}).session;
  const upload=chatUi(root,{action:'upload',chat_id:args.chat_id,upload_id:'queue-file',name:'file.txt',data_base64:Buffer.from('queued file').toString('base64')}).attachment;
  const send=(id,text=id)=>chatUi(root,{action:'send',chat_id:args.chat_id,message_id:id,text,attachment_ids:[upload.id]}).session;
  send('u1');send('__proto__');send('constructor');send('constructor');
  assert.equal(read().messages.length,1);
  assert.equal(read().queued_messages.length,2);
  assert.match(readFileSync(path.join(root,read().archive_path),'utf8'),/## 待发送队列/);
  const opened=chatTool(root,'chat_open',args).session;
  for(const key of ['queue','queued_messages','queue_receipts'])assert.equal(opened[key],undefined);
  assert.equal(opened.messages.length,1);
  chatTool(root,'chat_reply',{...args,message_id:'progress',reply_to:'u1',text:'working',final:false});
  assert.equal((await chatWait(root,{...args,timeout_ms:0})).message.id,'u1');
  assert.equal(read().queued_messages.length,2);
  chatTool(root,'chat_reply',{...args,message_id:'done',reply_to:'u1',text:'done',final:true});
  assert.equal(read().queued_messages.length,2);
  assert.equal(chatTool(root,'chat_open',args).session.messages.length,3);
  const delivered=(await chatWait(root,{...args,timeout_ms:0})).message;
  assert.equal(delivered.id,'__proto__');
  assert.equal(delivered.text,'队列1：\n\n__proto__\n\n队列2：\n\nconstructor');
  assert.deepEqual(delivered.attachments.map(f=>f.id),[upload.id]);
  assert.ok(delivered.received_at>=delivered.created_at);
  assert.equal(read().queued_messages.length,0);
  send('__proto__');send('constructor');
  assert.equal(read().messages.length,4);
  assert.throws(()=>send('constructor','different'),/conflict/);
  assert.throws(()=>chatUi(root,{action:'request_connection',chat_id:args.chat_id,message_id:'constructor'}),/conflict/);
  assert.throws(()=>chatTool(root,'chat_reply',{...args,message_id:'constructor',reply_to:'__proto__',text:'collision',final:true}),/conflict/);
  const {execFileSync}=await import('node:child_process');
  const script=`import {chatUi} from ${JSON.stringify(new URL('../dist/chat/store.js',import.meta.url).href)}; const s=chatUi(process.argv[1],{action:'read',chat_id:process.argv[2]}).session; process.stdout.write(JSON.stringify(s));`;
  const restarted=JSON.parse(execFileSync(process.execPath,['--input-type=module','-e',script,root,args.chat_id],{encoding:'utf8'}));
  assert.deepEqual(restarted.messages,read().messages);
  assert.deepEqual(restarted.queued_messages,[]);
});

test('split outbox pauses for confirmation, answers directly and resumes one per completion', async t => {
  const {root,args}=fixture(t);
  const read=()=>chatUi(root,{action:'read',chat_id:args.chat_id}).session;
  const send=id=>chatUi(root,{action:'send',chat_id:args.chat_id,message_id:id,text:id});
  const reply=(id,to,awaiting_user=false)=>chatTool(root,'chat_reply',{...args,message_id:id,reply_to:to,text:id,final:true,awaiting_user});
  const wait=()=>chatWait(root,{...args,timeout_ms:0});
  send('u1');send('u2');send('u3');
  chatUi(root,{action:'set_queue_mode',chat_id:args.chat_id,mode:'split'});
  assert.equal(read().queue_mode,'split');
  assert.throws(()=>chatUi(root,{action:'set_queue_mode',chat_id:args.chat_id,mode:'bad'}),/Invalid/);
  reply('question','u1',true);
  assert.equal((await wait()).status,'idle');
  assert.equal(read().queued_messages.length,2);
  const waiting=chatWait(root,{...args,timeout_ms:2000});
  send('confirmation');
  assert.equal((await waiting).message.id,'confirmation');
  assert.deepEqual(read().queued_messages.map(m=>m.id),['u2','u3']);
  reply('confirmed','confirmation');
  assert.equal((await wait()).message.id,'u2');
  assert.equal((await wait()).message.id,'u2');
  assert.deepEqual(read().queued_messages.map(m=>m.id),['u3']);
  reply('a2','u2');
  assert.equal((await wait()).message.id,'u3');
  reply('a3','u3');
  assert.equal((await wait()).status,'idle');
});

test('pin is persistent, scoped and sorts before recently updated sessions', t => {
  const {root,args}=fixture(t);
  const newer=chatUi(root,{action:'create',title:'newer'}).session;
  chatUi(root,{action:'send',chat_id:newer.id,message_id:'new',text:'recent'});
  chatUi(root,{action:'pin',chat_id:args.chat_id,pinned:true});
  assert.equal(chatUi(root,{action:'list'}).sessions[0].id,args.chat_id);
  assert.equal(chatUi(root,{action:'read',chat_id:args.chat_id}).session.pinned,true);
  assert.throws(()=>chatUi(root,{action:'pin',chat_id:args.chat_id,pinned:'yes'}),/boolean/);
  chatUi(root,{action:'pin',chat_id:args.chat_id,pinned:false});
  assert.equal(chatUi(root,{action:'read',chat_id:args.chat_id}).session.pinned,false);
});

test('conversations can be archived and deleted with their records and assets', t => {
  let now = Date.now();
  t.mock.method(Date, 'now', () => now);
  const root = mkdtempSync(path.join(tmpdir(), 'chat-test-'));
  t.after(() => rmSync(root, { recursive: true, force: true }));
  const older = chatUi(root, { action: 'create', title: 'older' }).session;
  now += 1;
  const newer = chatUi(root, { action: 'create', title: 'newer' }).session;
  assert.throws(() => chatUi(root, { action: 'archive', chat_id: newer.id, archived: 'yes' }), /boolean/);
  assert.equal(chatUi(root, { action: 'archive', chat_id: newer.id, archived: true }).session.archived, true);
  let sessions = chatUi(root, { action: 'list' }).sessions;
  assert.equal(sessions[0].id, older.id, 'archived conversations sort last');
  assert.equal(sessions[1].archived, true);
  chatUi(root, { action: 'archive', chat_id: newer.id, archived: false });
  assert.equal(chatUi(root, { action: 'list' }).sessions[0].id, newer.id);

  const dir = path.join(root, 'docs/chat-sessions');
  writeFileSync(path.join(dir, `${newer.id}.activity.json`), '{}');
  writeFileSync(path.join(dir, `${newer.id}-0f1e2d3c-0000-4000-8000-000000000000.png`), 'png');
  const assets = path.join(root, 'mcp-assistant/chat-assets', newer.id);
  mkdirSync(assets, { recursive: true }); writeFileSync(path.join(assets, 'a.txt'), 'a');
  const otherAssets = path.join(root, 'mcp-assistant/chat-assets', older.id); mkdirSync(otherAssets, { recursive: true });
  assert.deepEqual(chatUi(root, { action: 'delete', chat_id: newer.id }), { deleted: true, chat_id: newer.id });
  for (const name of [`${newer.id}.json`, `${newer.id}.md`, `${newer.id}.activity.json`, `${newer.id}-0f1e2d3c-0000-4000-8000-000000000000.png`]) {
    assert.equal(existsSync(path.join(dir, name)), false, name);
  }
  assert.equal(existsSync(assets), false);
  assert.equal(existsSync(path.join(dir, `${older.id}.json`)), true);
  assert.equal(existsSync(otherAssets), true);
  assert.equal(chatUi(root, { action: 'list' }).sessions.length, 1);
  assert.throws(() => chatUi(root, { action: 'delete', chat_id: newer.id }));

  chatTool(root, 'chat_open', { chat_id: older.id, agent_name: 'Tester' });
  assert.throws(() => chatUi(root, { action: 'delete', chat_id: older.id }), /Disconnect the AI/);
  chatUi(root, { action: 'detach', chat_id: older.id });
  chatUi(root, { action: 'delete', chat_id: older.id });
  assert.equal(existsSync(path.join(dir, `${older.id}.json`)), false);
});

test('MCP plans persist per chat and request, reject foreign ownership and retain legacy plan calls',async t=>{
 const fixture=await createMcpFixture(t);let seq=0;
 const call=async(name,args)=>{
  const response=await mcpRequest(fixture,{jsonrpc:'2.0',id:++seq,method:'tools/call',params:{name,arguments:{workspace_folder_id:'repo',...args}}});
  assert.equal(response.status,200);const body=await responseJson(response);return body.result?.structuredContent??body;
 };
 const a=chatUi(fixture.root,{action:'create',title:'A'}).session.id,b=chatUi(fixture.root,{action:'create',title:'B'}).session.id;
 for(const chat_id of [a,b])chatUi(fixture.root,{action:'send',chat_id,message_id:'same-user-id',text:'task'});
 const ownA=(await call('chat_open',{chat_id:a})).attachment_id,ownB=(await call('chat_open',{chat_id:b})).attachment_id;
 const scope={chat_id:a,attachment_id:ownA,reply_to:'same-user-id'};
 const read=id=>chatUi(fixture.root,{action:'read',chat_id:id}).session;
 const todos=[{id:'read',title:'Read code',status:'completed'},{id:'change',title:'Implement',status:'in_progress'}];
 assert.equal((await call('set_todos',{...scope,goal:'Feature A',todos})).persisted,true);
 assert.equal(read(b).messages[0].task_plan,undefined);
 assert.equal((await call('set_todos',{...scope,attachment_id:ownB,todos})).ok,false);
 assert.equal((await call('set_todos',{attachment_id:ownA,todos})).ok,false);
 assert.equal((await call('report_progress',{...scope,message:'password=synthetic-value working',percent:40,todo_id:'change'})).persisted,true);
 assert.doesNotMatch(JSON.stringify(read(a)),/synthetic-value/);
 assert.equal(read(a).messages[0].task_plan.progress.percent,40);
 for(const invalid of [{percent:101},{todo_id:'missing'},{percent:2.5}])assert.equal((await call('report_progress',{...scope,message:'bad',...invalid})).ok,false);
 assert.equal((await call('update_plan',{...scope,explanation:'Next step',plan:[{step:'Read code',status:'completed'},{step:'Implement',status:'completed'},{step:'Verify',status:'in_progress'}]})).persisted,true);
 assert.deepEqual(read(a).messages[0].task_plan.todos.map(t=>t.id),['read','change','todo-1']);
 assert.match(readFileSync(path.join(fixture.root,read(a).archive_path),'utf8'),/任务计划[\s\S]*Feature A[\s\S]*Verify/);
 assert.equal((await call('chat_open',{chat_id:a,attachment_id:ownA})).session.messages[0].task_plan.goal,'Feature A');
 for(const bad of [[...todos,{id:'read',title:'dup',status:'pending'}],[{id:'a',title:'A',status:'in_progress'},{id:'b',title:'B',status:'in_progress'}]])assert.equal((await call('set_todos',{...scope,todos:bad})).ok,false);
 assert.equal((await call('set_todos',{goal:'Legacy',todos})).ok,true);
 assert.equal(read(a).messages[0].task_plan.goal,'Feature A');
 await call('chat_reply',{...scope,message_id:'done',text:'done',final:true});
 chatUi(fixture.root,{action:'send',chat_id:a,message_id:'new-user',text:'new request'});
 assert.equal(read(a).messages.at(-1).task_plan,undefined);
 assert.equal((await call('set_todos',{...scope,todos})).ok,false);
 assert.equal((await call('set_todos',{...scope,reply_to:'new-user',todos})).persisted,true);
 assert.equal((await call('set_todos',{...scope,reply_to:'new-user',todos:[]})).plan.cleared,true);
 assert.equal(read(a).messages.at(-1).task_plan,undefined);
 chatUi(fixture.root,{action:'detach',chat_id:a});
 assert.equal((await call('set_todos',{...scope,reply_to:'new-user',todos})).ok,false);
});


test('invalid deletion assets preserve the authoritative chat and operation records', t => {
  const {root,args}=fixture(t);
  chatUi(root,{action:'detach',chat_id:args.chat_id});
  const dir=path.join(root,'docs/chat-sessions');
  const record=path.join(dir,`${args.chat_id}.json`);
  const original=readFileSync(record,'utf8');
  writeFileSync(path.join(dir,`${args.chat_id}.operations.json`),'[]');
  const assets=path.join(root,'mcp-assistant/chat-assets');mkdirSync(assets,{recursive:true});
  writeFileSync(path.join(assets,args.chat_id),'invalid directory fixture');
  assert.throws(()=>chatUi(root,{action:'delete',chat_id:args.chat_id}),/not a directory/);
  assert.equal(readFileSync(record,'utf8'),original);
  assert.equal(readFileSync(path.join(dir,`${args.chat_id}.operations.json`),'utf8'),'[]');
  assert.ok(existsSync(path.join(dir,`${args.chat_id}.md`)));
});

test('UI snapshots and attachment reads remain available while a writer owns the folder lock', t => {
  const {root,args}=fixture(t);
  chatUi(root,{action:'upload',chat_id:args.chat_id,upload_id:'snapshot-file',name:'snapshot.txt',data_base64:Buffer.from('snapshot').toString('base64')});
  const record=path.join(root,'docs/chat-sessions',args.chat_id+'.json');
  const before=readFileSync(record,'utf8');
  const lockPath=path.join(root,'docs/chat-sessions/.lock');
  mkdirSync(lockPath);
  assert.equal(chatUi(root,{action:'list'}).sessions.length,1);
  assert.equal(chatUi(root,{action:'read',chat_id:args.chat_id}).session.id,args.chat_id);
  assert.equal(Buffer.from(chatUi(root,{action:'read_attachment',chat_id:args.chat_id,upload_id:'snapshot-file'}).data_base64,'base64').toString(),'snapshot');
  assert.ok(existsSync(lockPath));
  assert.equal(readFileSync(record,'utf8'),before);
  writeFileSync(record,'broken');
  assert.throws(()=>chatUi(root,{action:'list'}),SyntaxError);
  rmSync(record);
  assert.deepEqual(chatUi(root,{action:'list'}).sessions,[]);
  assert.throws(()=>chatUi(root,{action:'read',chat_id:args.chat_id}),{code:'ENOENT'});
});

test('listing an unused folder is read-only and does not create storage',t=>{
  const root=mkdtempSync(path.join(tmpdir(),'chat-empty-'));
  t.after(()=>rmSync(root,{recursive:true,force:true}));
  assert.deepEqual(chatUi(root,{action:'list'}),{sessions:[]});
  assert.equal(existsSync(path.join(root,'docs')),false);
});

test('independent writers preserve every queued message while snapshots are polled', async t=>{
  const {root,args}=fixture(t);
  const {Worker}=await import('node:worker_threads');
  const moduleUrl=new URL('../dist/chat/store.js',import.meta.url).href;
  const workers=Array.from({length:3},(_,index)=>new Worker(`
    const {workerData}=require('node:worker_threads');
    import(workerData.moduleUrl).then(({chatUi})=>{
      for(let n=0;n<12;n++)chatUi(workerData.root,{action:'send',chat_id:workerData.chat_id,message_id:'writer-'+workerData.index+'-'+n,text:'message '+n});
    }).catch(e=>{throw e});`,{eval:true,workerData:{root,chat_id:args.chat_id,moduleUrl,index}}));
  let polls=0;
  const timer=setInterval(()=>{assert.equal(chatUi(root,{action:'list'}).sessions.length,1);assert.equal(chatUi(root,{action:'read',chat_id:args.chat_id}).session.id,args.chat_id);polls++;},5);
  try { await Promise.all(workers.map(worker=>new Promise((resolve,reject)=>{worker.on('error',reject);worker.on('exit',code=>code===0?resolve():reject(new Error('worker exit '+code)));}))); }
  finally {clearInterval(timer);await Promise.all(workers.map(worker=>worker.terminate()));}
  const session=chatUi(root,{action:'read',chat_id:args.chat_id}).session;
  const messages=[...session.messages,...session.queued_messages];
  assert.equal(messages.length,36);
  assert.equal(new Set(messages.map(m=>m.id)).size,36);
  assert.ok(polls>0);
});

test('message whitespace survives send, retry, merged queues, replies and tool details', t=>{
 const {root,args}=fixture(t),text='    indented code\r\n\r\n正文 **bold**  \n\t尾行\n\n';
 const send={action:'send',chat_id:args.chat_id,message_id:'format-a',text};chatUi(root,send);chatUi(root,send);
 assert.equal(chatTool(root,'chat_wait',args).message.text,text);
 assert.throws(()=>chatUi(root,{...send,text:text.trim()}),/conflict/);
 for(const id of ['format-b','format-c'])chatUi(root,{...send,message_id:id});
 const reply={...args,message_id:'format-reply',reply_to:'format-a',text,final:false,tool_event:{name:'format-check',status:'completed',input:text,output:text}};
 chatTool(root,'chat_reply',reply);chatTool(root,'chat_reply',reply);
 const detail=chatUi(root,{action:'read',chat_id:args.chat_id}).session;
 assert.equal(detail.messages[1].text,text);assert.equal(detail.messages[1].tool_event.input,text);assert.equal(detail.messages[1].tool_event.output,text);
 assert.ok(readFileSync(path.join(root,detail.archive_path),'utf8').includes(text));
 chatTool(root,'chat_reply',{...args,message_id:'format-done',reply_to:'format-a',text:'done',final:true});
 assert.equal(chatTool(root,'chat_wait',args).message.text,`队列1：\n\n${text}\n\n队列2：\n\n${text}`);
 assert.throws(()=>chatUi(root,{...send,message_id:'too-big',text:' '.repeat(32000)+'x'}),/bytes/);
});
