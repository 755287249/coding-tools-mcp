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
test('chat protects folder boundaries, rejects wrong attachments and concurrent writers',async t=>{
  const {root,args}=fixture(t);
  assert.throws(()=>chatTool(root,'chat_open',{chat_id:args.chat_id}),/already attached/);
  assert.throws(()=>chatTool(root,'chat_reply',{...args,attachment_id:'wrong'}),/expired/);
  assert.throws(()=>chatUi(root,{action:'read',chat_id:'../../outside'}),/Invalid/);
  const other=mkdtempSync(path.join(tmpdir(),'chat-other-'));t.after(()=>rmSync(other,{recursive:true,force:true}));
  assert.throws(()=>chatUi(other,{action:'read',chat_id:args.chat_id}));
  mkdirSync(path.join(root,'docs/chat-sessions/.lock'));
  assert.throws(()=>chatUi(root,{action:'list'}),/Chat storage is busy/);
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
  assert.match(opened.instruction,/awaiting_user=true/);
  assert.match(opened.instruction,/never stop voluntarily/);
  const args={chat_id:session.id,attachment_id:opened.attachment_id};
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
  assert.ok(Array.isArray(chatUi(root,{action:'list'}).sessions));
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
  assert.deepEqual(states(),['消息状态：未读 · 排队中','消息状态：未读 · 排队中']);
  await chatWait(root,{...args,timeout_ms:0});
  assert.deepEqual(states(),['消息状态：已读 · 正在处理','消息状态：未读 · 排队中']);
  chatTool(root,'chat_reply',{...args,message_id:'q1',reply_to:'u1',text:'Which option?',final:true,awaiting_user:true});
  assert.deepEqual(states(),['消息状态：已读 · 待确认','消息状态：未读 · 排队中']);
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
