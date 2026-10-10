import test from 'node:test';
import assert from 'node:assert/strict';
import {mkdtempSync,rmSync,readFileSync,writeFileSync} from 'node:fs';
import {tmpdir} from 'node:os';
import path from 'node:path';
import {chatUi,chatTool,chatWait,readChatOperations,writeChatOperation} from '../dist/chat/store.js';
import {bindChatOperations,beginChatOperation} from '../dist/chat/operations.js';
import {createMcpFixture,mcpRequest,responseJson} from './mcpTestHelpers.mjs';
function fixture(t){const root=mkdtempSync(path.join(tmpdir(),'chat-ops-'));t.after(()=>rmSync(root,{recursive:true,force:true}));return root;}
async function attach(root,key,mode='work',name='Coder'){
 const id=chatUi(root,{action:'create',mode}).session.id;
 const args={chat_id:id,agent_name:name};const result=chatTool(root,'chat_open',args);args.attachment_id=result.attachment_id;bindChatOperations(root,key,'chat_open',args,result);
 chatUi(root,{action:'send',chat_id:id,message_id:'u',text:'inspect'});
 bindChatOperations(root,key,'chat_wait',args,await chatWait(root,{...args,timeout_ms:0}));return args;
}
test('actual MCP requests capture file operations, survive reread, and omit read contents / credentials',async t=>{
 const state=await createMcpFixture(t);let seq=0;
 const cid=chatUi(state.root,{action:'create'}).session.id;
 const call=async(name,args)=>{const r=await responseJson(await mcpRequest(state,{jsonrpc:'2.0',id:++seq,method:'tools/call',params:{name,arguments:{...args,workspace_folder_id:'repo'}}}));return r.result.structuredContent;};
 const opened=await call('chat_open',{chat_id:cid,agent_name:'Tester'});assert.equal(opened.ok,true);
 const args={chat_id:cid,attachment_id:opened.attachment_id};
 chatUi(state.root,{action:'send',chat_id:cid,message_id:'u',text:'read file'});await call('chat_wait',{...args,timeout_ms:0});
 writeFileSync(path.join(state.root,'sample.txt'),'private-file-body\n');
 assert.equal((await call('read_file',{path:'sample.txt'})).ok,true);
 await call('read_file',{path:'missing.txt'});
 const edit=await call('file_ops',{operations:[{type:'create',path:'created.txt',content:'written\n'}]});assert.equal(edit.ok,true,JSON.stringify(edit));assert.equal(readFileSync(path.join(state.root,'created.txt'),'utf8'),'written\n');
 const events=chatUi(state.root,{action:'read',chat_id:cid}).session.operations;
 assert.equal(events.length,3);assert.equal(events[2].kind,'edit');assert.equal(events[2].status,'completed');assert.ok(events[2].paths.includes('created.txt'));assert.ok(events[2].diff?.includes('+written'));assert.equal(events[0].tool,'read_file');assert.equal(events[0].reply_to,'u');assert.equal(events[0].agent_name,'Tester');assert.deepEqual(events[0].paths,['sample.txt']);assert.equal(events[0].status,'completed');assert.equal(events[1].status,'failed');
 const disk=readFileSync(path.join(state.root,`docs/chat-sessions/${cid}.operations.json`),'utf8');assert.doesNotMatch(disk,/private-file-body/);assert.ok(!disk.includes(args.attachment_id));
 assert.match(readFileSync(path.join(state.root,`docs/chat-sessions/${cid}.operations.md`),'utf8'),/Tester · read_file/);
 await call('chat_reply',{...args,reply_to:'u',message_id:'done',text:'done',final:true});await call('read_file',{path:'sample.txt'});
 assert.equal(readChatOperations(state.root,cid).operations.length,3,'no attribution after final');
});
test('ambiguous clients, wrong folders and expired attachments are never assigned to another chat',async t=>{
 const root=fixture(t);const a=await attach(root,'one');
 assert.equal(beginChatOperation(root,'other','read_file',{path:'a'}),undefined);
 assert.equal(beginChatOperation(fixture(t),'one','read_file',{path:'a'}),undefined);
 const b=await attach(root,'one');assert.equal(beginChatOperation(root,'one','read_file',{path:'a'}),undefined);
 chatUi(root,{action:'close',chat_id:b.chat_id});
 const op=beginChatOperation(root,'one','read_file',{path:'a'});assert.ok(op);op.finish({ok:true});
 chatUi(root,{action:'detach',chat_id:a.chat_id});assert.equal(beginChatOperation(root,'one','read_file',{path:'a'}),undefined);
 assert.equal(readChatOperations(root,b.chat_id).operations.length,0);
});
test('parallel records retain actor/request, failures and dry-run diffs redact secrets',async t=>{
 const root=fixture(t);const a=await attach(root,'group','group','Builder');
 const first=beginChatOperation(root,'group','file_ops',{operations:[{path:'one.txt',content:'do-not-log-body'}],dry_run:true});
 const second=beginChatOperation(root,'group','exec_command',{command:'echo password=synthetic-secret'});
 second.finish({ok:true,exit_code:2,stdout:'token=synthetic-token'});
 first.finish({ok:true,diff:'--- a/one.txt\n+++ b/one.txt\n+password=hidden-value\n',affected_files:[{path:'one.txt',operation:'write'}]});
 const events=readChatOperations(root,a.chat_id).operations;assert.equal(events.length,2);assert.equal(events[0].agent_name,'Builder');assert.ok(events[0].agent_id);assert.equal(events[0].dry_run,true);assert.equal(events[1].status,'failed');assert.ok(!JSON.stringify(events).match(/synthetic-secret|synthetic-token|hidden-value|do-not-log-body/));
 const interrupted=beginChatOperation(root,'group','exec_command',{});interrupted.finish({},true);assert.equal(readChatOperations(root,a.chat_id).operations.at(-1).status,'interrupted');
});
test('bounded sidecar does not consume chat messages and preserves unsupported/corrupt archives',async t=>{
 const root=fixture(t);const a=await attach(root,'bounded');const op=beginChatOperation(root,'bounded','read_file',{path:'a'});op.finish({ok:true});const seed=readChatOperations(root,a.chat_id).operations[0];
 for(let i=0;i<250;i++)writeChatOperation(root,a.chat_id,{...seed,id:String(i),output:'x'.repeat(4000)});
 const events=readChatOperations(root,a.chat_id).operations;assert.ok(events.length<=240);assert.ok(Buffer.byteLength(JSON.stringify(events))<=512000);assert.equal(events.at(-1).id,'249');assert.equal(chatUi(root,{action:'read',chat_id:a.chat_id}).session.messages.length,1);
 const target=path.join(root,`docs/chat-sessions/${a.chat_id}.operations.json`);writeFileSync(target,'broken');assert.ok(readChatOperations(root,a.chat_id).operations_error);assert.throws(()=>writeChatOperation(root,a.chat_id,seed));assert.equal(readFileSync(target,'utf8'),'broken');
});


test('operation storage depends on a pure operation contract without a reverse runtime import', () => {
 const source = file => readFileSync(new URL('../src/chat/' + file, import.meta.url), 'utf8');
 assert.match(source('store.ts'), /from ['"]\.\/operation-contract\.js['"]/);
 assert.doesNotMatch(source('store.ts'), /from ['"]\.\/operations\.js['"]/);
 assert.doesNotMatch(source('operation-contract.ts'), /\bimport\b/);
});
