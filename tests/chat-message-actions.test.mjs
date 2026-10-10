import test from 'node:test';
import assert from 'node:assert/strict';
import {createDraftHistory} from '../src/lib/chat/draft-history.ts';
import {messagePreview,messageClipboardText,createMessageTransfer,forwardMessage} from '../src/lib/chat/message-actions.ts';
import {mkdtempSync,rmSync} from 'node:fs';
import {tmpdir} from 'node:os';
import path from 'node:path';
import {chatUi} from '../packages/node-agent/dist/chat/store.js';
const file={id:'a',name:'image.png',path:'assets/a.png',label:'图片1',mime:'image/png',size:20,sha256:'hash'};
const snapshot=(text,attachments=[],start=text.length)=>({text,attachments,start,end:start});
test('draft undo groups typing but keeps attachment changes, selections and redo branches distinct',()=>{
 const h=createDraftHistory();h.reset(snapshot('a'));
 h.record(snapshot('ab'),true,1000);h.record(snapshot('abc'),true,1100);
 h.record(snapshot('abc',[file]),false,1200);h.record(snapshot('abc @图片1',[file]),true,1300);
 assert.deepEqual(h.undo(),snapshot('abc',[file]));assert.deepEqual(h.undo(),snapshot('abc'));assert.deepEqual(h.undo(),snapshot('a'));
 assert.deepEqual(h.redo(),snapshot('abc'));h.record(snapshot('new'));assert.equal(h.redo(),null);
 h.reset(snapshot('selected',[file],2));h.record(snapshot('s',[file]));assert.equal(h.undo().start,2);
 h.reset(snapshot(''));assert.equal(h.undo(),null,'successful send/scope reset must discard prior undo history');
});
test('removing attachments is reversible and snapshots do not alias mutable input arrays',()=>{
 const h=createDraftHistory(2),files=[{...file}];h.reset(snapshot('@图片1',files));files[0].name='mutated';
 h.record(snapshot(''));assert.equal(h.undo().attachments[0].name,'image.png');h.redo();h.record(snapshot('one'));h.record(snapshot('two'));h.record(snapshot('three'));
 assert.equal(h.undo().text,'two');assert.equal(h.undo().text,'one');assert.equal(h.undo(),null);
});
test('long message previews retain complete copy text and attachment descriptions',()=>{
 const text='中文😀'.repeat(300);const preview=messagePreview(text);assert.ok(preview.length<text.length);assert.ok(preview.endsWith('…'));assert.ok(preview.isWellFormed());
 const lines=Array.from({length:20},(_,i)=>'line '+i).join('\n');assert.ok(messagePreview(lines).split('\n').length<=12);
 assert.equal(messagePreview('short'),'short');const copied=messageClipboardText({text,attachments:[file]});assert.ok(copied.startsWith(text));assert.ok(copied.includes(file.path));assert.ok(copied.includes(file.name));
});
test('cross-folder forwarding preserves chunked bytes, remaps references and deduplicates lost upload/send responses',async t=>{
 const a=mkdtempSync(path.join(tmpdir(),'forward-source-')),b=mkdtempSync(path.join(tmpdir(),'forward-target-'));t.after(()=>{rmSync(a,{recursive:true,force:true});rmSync(b,{recursive:true,force:true});});
 const sourceChat=chatUi(a,{action:'create',title:'Source'}).session.id,targetChat=chatUi(b,{action:'create',title:'Target'}).session.id;
 const bytes=Buffer.alloc(512*1024+37,97);const sourceFile=chatUi(a,{action:'upload',chat_id:sourceChat,upload_id:'source-file',name:'notes.txt',data_base64:bytes.toString('base64')}).attachment;
 chatUi(b,{action:'upload',chat_id:targetChat,upload_id:'existing',name:'existing.txt',data_base64:Buffer.from('existing').toString('base64')});
 const message={text:`@${sourceFile.label} ${sourceFile.path} @文件10`,attachments:[sourceFile]};let ids=0,loseUpload=true,loseSend=true;
 const state=createMessageTransfer(()=>`transfer-${++ids}`);const source=async args=>chatUi(a,args);
 const target=async args=>{const result=chatUi(b,args);if(args.action==='upload_chunk'&&result.attachment&&loseUpload){loseUpload=false;throw Error('lost upload response')}if(args.action==='send'&&loseSend){loseSend=false;throw Error('lost send response')}return result;};
 const run=()=>forwardMessage(message,sourceChat,targetChat,source,target,state,()=>`transfer-${++ids}`);
 await assert.rejects(run,/lost upload/);await assert.rejects(run,/lost send/);await run();
 const session=chatUi(b,{action:'read',chat_id:targetChat}).session;assert.equal(session.messages.length,1);const sent=session.messages[0];assert.equal(sent.id,state.messageId);assert.equal(sent.attachments.length,1);const copied=sent.attachments[0];
 assert.equal(copied.sha256,sourceFile.sha256);assert.equal(copied.size,bytes.length);assert.notEqual(copied.path,sourceFile.path);assert.notEqual(copied.label,sourceFile.label);
 assert.equal(sent.text,`@${copied.label} ${copied.path} @文件10`);assert.deepEqual(Buffer.from(chatUi(b,{action:'read_attachment',chat_id:targetChat,upload_id:copied.id}).data_base64,'base64'),bytes);
 assert.equal(ids,2,'retries must reuse message and upload identities');
});
test('forwarding refuses changed content, invalid ranges and oversize messages before sending',async()=>{
 const message={text:'body',attachments:[{...file,size:1}]};
 for(const read of [{attachment:{...file,size:1},data_base64:'YQ==',next_offset:2},{attachment:{...file,size:1,sha256:'changed'},data_base64:'YQ==',next_offset:1}]){
  let writes=0;await assert.rejects(()=>forwardMessage(message,'a','b',async()=>read,async()=>{writes++;return {};},createMessageTransfer(()=> 'id'),()=> 'file'),/content changed/);assert.equal(writes,0);
 }
 let sent=false;await assert.rejects(()=>forwardMessage(message,'a','b',async()=>({attachment:{...file,size:1},data_base64:'YQ==',next_offset:1}),async args=>{if(args.action==='send')sent=true;return {next_offset:1,attachment:{...file,size:1,sha256:'wrong'}};},createMessageTransfer(()=> 'id'),()=> 'file'),/integrity/);assert.equal(sent,false);
 await assert.rejects(()=>forwardMessage({text:'中'.repeat(11000)},'a','b',async()=>assert.fail(),async()=>assert.fail(),createMessageTransfer(()=> 'id'),()=> 'file'),/32000/);
});
