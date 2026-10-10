import test from 'node:test';
import assert from 'node:assert/strict';
import {mkdtempSync,rmSync,readFileSync,writeFileSync,readdirSync} from 'node:fs';
import {tmpdir} from 'node:os';
import path from 'node:path';
import {chatUi,chatTool} from '../dist/chat/store.js';
import {packHistory,unpackHistory,agentContext} from '../dist/chat/history.js';

function fixture(t) {
 const root=mkdtempSync(path.join(tmpdir(),'chat-history-'));
 t.after(()=>rmSync(root,{recursive:true,force:true}));
 const session=chatUi(root,{action:'create',title:'Long history'}).session;
 const args={chat_id:session.id,...chatTool(root,'chat_open',{chat_id:session.id})};
 const file=path.join(root,`docs/chat-sessions/${session.id}.json`);
 const read=()=>chatUi(root,{action:'read',chat_id:session.id}).session;
 return {root,args,file,read};
}
function completed(n,text='history') {
 return Array.from({length:n},(_,i)=>[
  {id:`u${i}`,role:'user',text,created_at:i*2},
  {id:`r${i}`,role:'assistant',text:'done',reply_to:`u${i}`,final:true,created_at:i*2+1}
 ]).flat();
}
test('legacy 500-message session can finish, advance queued work and retry archived replies',t=>{
 const {root,args,file,read}=fixture(t),s=JSON.parse(readFileSync(file,'utf8'));
 const plan={goal:'Keep my task',todos:[],updated_ms:1};
 s.messages=[...completed(249),{id:'active',role:'user',text:'Continue',created_at:499,task_plan:plan},
  {id:'progress',role:'assistant',text:'working',reply_to:'active',final:false,created_at:500}];
 s.queue=[{id:'queued',role:'user',text:'Next queued request',created_at:501}];
 writeFileSync(file,JSON.stringify(s));
 assert.equal(chatTool(root,'chat_reply',{...args,reply_to:'active',message_id:'finish',text:'Finished'}).persisted,true);
 assert.equal(read().messages.length,501);
 assert.equal(JSON.parse(readFileSync(file,'utf8')).version,3);
 assert.equal(chatTool(root,'chat_wait',args).message.id,'queued');
 assert.deepEqual(read().messages.find(m=>m.id==='active').task_plan,plan);
 assert.equal(chatTool(root,'chat_reply',{...args,reply_to:'u0',message_id:'r0',text:'done'}).persisted,true);
 assert.equal(read().messages.length,502);
 const open=chatTool(root,'chat_open',args);
 assert.equal(open.attachment_id,args.attachment_id);
 assert.equal(open.session.context.total_messages,502);
 assert.ok(open.session.messages.length<=13);
 assert.equal(open.session.context.pending_message_id,'queued');
 assert.ok(readFileSync(path.join(root,read().archive_path),'utf8').includes('Next queued request'));
 const pages=readdirSync(path.dirname(file)).filter(n=>n.includes('.history.'));
 chatTool(root,'chat_wait',args);
 assert.deepEqual(readdirSync(path.dirname(file)).filter(n=>n.includes('.history.')),pages,'heartbeat reuses history pages');
});
test('transcripts beyond 2 MiB retain exact messages and immutable pages detect corruption',t=>{
 const {root,args,file,read}=fixture(t),s=JSON.parse(readFileSync(file,'utf8'));
 s.messages=completed(180,'历史😀'.repeat(600));
 assert.ok(Buffer.byteLength(JSON.stringify(s))<2*1024*1024);
 writeFileSync(file,JSON.stringify(s));
 chatUi(root,{action:'send',chat_id:args.chat_id,message_id:'active',text:'continue'});
 const text='扩展😀'.repeat(3000);
 for(let i=0;i<45;i++) chatTool(root,'chat_reply',{...args,reply_to:'active',message_id:`step${i}`,text,final:false});
 assert.ok(Buffer.byteLength(JSON.stringify(read()))>2*1024*1024);
 assert.equal(read().messages.at(-1).text,text);
 assert.ok(readFileSync(file).length<2*1024*1024);
 const opened=chatTool(root,'chat_open',args);
 assert.ok(Buffer.byteLength(JSON.stringify(opened.session))<64*1024);
 assert.equal(opened.session.context.pending_message_id,'active');
 assert.equal(chatTool(root,'chat_wait',args).message.text,'continue');
 const manifest=readFileSync(file),state=JSON.parse(manifest),page=state.message_pages[0];
 writeFileSync(path.join(path.dirname(file),`${args.chat_id}.history.${page.sha256}.json`),'[]');
 assert.throws(()=>chatTool(root,'chat_wait',args),/integrity/);
 assert.deepEqual(readFileSync(file),manifest,'damaged history never replaces the manifest');
});
test('v3 page descriptors reject path escapes and incomplete page writes leave input untouched',()=>{
 const s={id:'example',version:2,messages:completed(100)};
 const before=JSON.stringify(s);
 assert.throws(()=>packHistory(s,()=>{throw Error('disk full')}),/disk full/);
 assert.equal(JSON.stringify(s),before);
 assert.throws(()=>unpackHistory({version:3,session_version:1,message_count:1,messages:[],message_pages:[{sha256:'../escape',count:1,bytes:2}]},()=>{throw Error('must not read')}),/descriptor/);
});
test('shared wire fixture is readable and context excerpts do not change stored Unicode or plans',()=>{
 const wire=JSON.parse(readFileSync(new URL('../../../tests/fixtures/chat-history-v3.json',import.meta.url),'utf8'));
 assert.deepEqual(unpackHistory(wire.manifest,()=>Buffer.from(wire.page_body)).messages,wire.messages);
 const all=completed(100,'原记录😀'.repeat(900));
 const active={id:'active',role:'user',text:'current',task_plan:{goal:'preserve'},created_at:10};
 all.splice(2,0,active);
 const view={messages:all,archive_path:'docs/chat-sessions/example.md'};
 const window=agentContext(view,'active');
 assert.ok(Buffer.byteLength(JSON.stringify(window))<64*1024);
 assert.equal(window.messages.find(m=>m.id==='active').task_plan.goal,'preserve');
 assert.ok(window.messages.every(m=>!m.text.includes('\ufffd')));
 assert.equal(all[0].text,'原记录😀'.repeat(900));
 const pages=new Map(),packed=packHistory({id:'sample',version:1,messages:all},(digest,bytes)=>pages.set(digest,bytes));
 assert.deepEqual(unpackHistory(packed,digest=>pages.get(digest)).messages,all);
});
