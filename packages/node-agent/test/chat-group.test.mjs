import test from 'node:test';
import assert from 'node:assert/strict';
import {mkdtempSync,rmSync,readFileSync,writeFileSync} from 'node:fs';
import {tmpdir} from 'node:os';
import path from 'node:path';
import {chatUi,chatTool,chatWait,chatPlan} from '../dist/chat/store.js';
import {createMcpFixture,mcpRequest,responseJson} from './mcpTestHelpers.mjs';
function fixture(t){const root=mkdtempSync(path.join(tmpdir(),'group-chat-'));t.after(()=>rmSync(root,{recursive:true,force:true}));const chat_id=chatUi(root,{action:'create',mode:'group'}).session.id;const join=name=>{const r=chatTool(root,'chat_open',{chat_id,agent_name:name});return {chat_id,attachment_id:r.attachment_id,agent_id:r.agent_id};};const a=join('总管'),b=join('前端');const send=(id,text)=>chatUi(root,{action:'send',chat_id,message_id:id,text});const reply=(who,id,to,more={})=>chatTool(root,'chat_reply',{...who,message_id:id,reply_to:to,text:id,final:true,...more});return {root,chat_id,a,b,join,send,reply,read:()=>chatUi(root,{action:'read',chat_id}).session};}
test('independent group waits, addressed delivery, per-member acknowledgement and safe shared archive',async t=>{
 const f=fixture(t),{root,a,b,send,reply,read}=f;
 const wa=chatWait(root,{...a,timeout_ms:2000}),wb=chatWait(root,{...b,timeout_ms:2000});
 assert.deepEqual(read().members.map(m=>m.status),['waiting','waiting']);
 await assert.rejects(chatWait(root,{...a,timeout_ms:0}),/already active/);
 send('u','@前端 请实现');assert.equal((await wa).message.id,'u');assert.equal((await wb).message.id,'u');assert.deepEqual(new Set(read().messages[0].received_by),new Set([a.agent_id,b.agent_id]));
 assert.throws(()=>reply(a,'early','u'),/assigned members/);
 chatPlan(root,'set_todos',{...a,reply_to:'u',goal:'Coordinate',todos:[{id:'s',title:'Review',status:'pending'}]});
 chatPlan(root,'set_todos',{...b,reply_to:'u',goal:'Implement',todos:[{id:'s',title:'Build',status:'in_progress'}]});
 assert.deepEqual(read().messages[0].agent_plans.map(p=>p.plan.goal),['Coordinate','Implement']);
 reply(b,'b-done','u');assert.equal(chatTool(root,'chat_wait',b).status,'idle');assert.equal(read().work_state,'processing');
 assert.throws(()=>reply(a,'b-done','u'),/another agent/);
 reply(a,'a-done','u');reply(a,'a-done','u');assert.equal(read().work_state,null);
 const md=readFileSync(path.join(root,read().archive_path),'utf8');assert.match(md,/Agent: 前端/);assert.match(md,/Coordinate/);assert.match(md,/Implement/);
 for(const aid of [a.attachment_id,b.attachment_id]){assert.ok(!md.includes(aid));assert.ok(!JSON.stringify(read()).includes(aid));}
 assert.equal(JSON.parse(readFileSync(path.join(root,'docs/chat-sessions',a.chat_id+'.json'))).version,2);
 assert.throws(()=>chatUi(root,{action:'set_mode',chat_id:a.chat_id,mode:'work'}),/fixed after/);
 chatUi(root,{action:'detach_member',chat_id:a.chat_id,member_id:b.agent_id});assert.throws(()=>chatTool(root,'chat_open',b),/paused/);
 chatUi(root,{action:'resume_member',chat_id:a.chat_id,member_id:b.agent_id});assert.equal(chatTool(root,'chat_open',b).agent_id,b.agent_id);
});
test('coordinator assigns tasks, waits for helpers, rejects cross-agent writes and resumes without duplicate work',async t=>{
 const {root,a,b,send,reply,read}=fixture(t);send('u','Implement');
 assert.equal(chatTool(root,'chat_wait',b).status,'idle');
 assert.throws(()=>reply(b,'steal','u'),/oldest/);
 assert.throws(()=>reply(b,'delegate','u',{final:false,recipient_ids:[a.agent_id]}),/coordinator/);
 reply(a,'assignment','u',{final:false,recipient_ids:[b.agent_id]});reply(a,'assignment','u',{final:false,recipient_ids:[b.agent_id]});
 assert.equal(read().messages.filter(m=>m.kind==='assignment').length,1);
 assert.equal(chatTool(root,'chat_wait',a).status,'idle');
 const wait=chatWait(root,{...a,timeout_ms:2000});
 const delivery=chatTool(root,'chat_wait',b);assert.equal(delivery.message.id,'assignment');
 assert.equal(chatTool(root,'chat_open',b).agent_id,b.agent_id);
 assert.throws(()=>reply(a,'final','u'),/assigned members/);
 assert.throws(()=>reply(b,'question','assignment',{awaiting_user:true}),/coordinator/);
 assert.throws(()=>chatTool(root,'chat_close',b),/coordinator/);
 chatPlan(root,'set_todos',{...b,reply_to:'assignment',todos:[{id:'s',title:'Implement',status:'completed'}]});
 reply(b,'helper-final','assignment');assert.equal((await wait).message.id,'u');reply(a,'all-done','u');
 assert.equal(read().work_state,null);
});
test('legacy attachment becomes coordinator, group queue preserves recipients and switches safely',t=>{
 const {root,a,b,send,reply,read}=fixture(t);send('one','One');send('two','@前端 Two');send('three','Three');
 assert.equal(read().queue_mode,'split');reply(a,'one-done','one');
 assert.equal(chatTool(root,'chat_wait',a).message.id,'two');assert.equal(read().queued_messages.length,1);
 chatUi(root,{action:'rename_member',chat_id:a.chat_id,member_id:b.agent_id,name:'新版前端'});
 assert.equal(chatTool(root,'chat_wait',b).message.id,'two');reply(b,'two-b','two');reply(a,'two-a','two');
 assert.equal(chatTool(root,'chat_wait',a).message.id,'three');assert.equal(chatTool(root,'chat_wait',b).status,'idle');reply(a,'three-done','three');
 chatUi(root,{action:'detach_member',chat_id:a.chat_id,member_id:b.agent_id});
 assert.throws(()=>chatUi(root,{action:'set_mode',chat_id:a.chat_id,mode:'work'}),/fixed after/);
 assert.equal(chatTool(root,'chat_open',a).attachment_id,a.attachment_id);
 const legacy=chatUi(root,{action:'create'}).session.id;const old=chatTool(root,'chat_open',{chat_id:legacy,agent_name:'Existing'});
 chatUi(root,{action:'set_mode',chat_id:legacy,mode:'group'});
 chatUi(root,{action:'send',chat_id:legacy,message_id:'old-u',text:'Existing task'});
 chatTool(root,'chat_reply',{chat_id:legacy,attachment_id:old.attachment_id,message_id:'progress',reply_to:'old-u',text:'working',final:false});
 assert.throws(()=>chatUi(root,{action:'set_mode',chat_id:legacy,mode:'work'}),/fixed after/);const resumed=chatTool(root,'chat_open',{chat_id:legacy,attachment_id:old.attachment_id});
 assert.equal(resumed.role,'coordinator');assert.equal(resumed.session.members[0].name,'Existing');
 assert.equal(chatTool(root,'chat_reply',{chat_id:legacy,attachment_id:old.attachment_id,message_id:'progress',reply_to:'old-u',text:'working',final:false}).persisted,true);
});
test('group membership identity survives lease expiry; false credentials, duplicate names and malformed assignments fail',t=>{
 const {root,a,b,join,send,reply}=fixture(t);const archive=path.join(root,'docs/chat-sessions',a.chat_id+'.json');const s=JSON.parse(readFileSync(archive));s.members[1].lease_until=0;writeFileSync(archive,JSON.stringify(s));
 assert.doesNotThrow(()=>chatTool(root,'chat_wait',b),'keepalive: own attachment survives lease expiry');assert.equal(chatTool(root,'chat_open',b).agent_id,b.agent_id);
 assert.throws(()=>join('前端'),/already exists/);assert.throws(()=>join('bad name'),/name/);assert.throws(()=>chatTool(root,'chat_open',{...a,attachment_id:'foreign'}),/expired/);
 send('u','Task');for(const ids of [[],[a.agent_id],[b.agent_id,b.agent_id],['foreign'],['__proto__']])assert.throws(()=>reply(a,'assign','u',{recipient_ids:ids,final:false}));
 assert.throws(()=>chatPlan(root,'set_todos',{...b,reply_to:'u',todos:[]}),/current unanswered/);
});
test('HTTP MCP exposes group schema and keeps two participant attachments independent',async t=>{
 const f=await createMcpFixture(t);const chat_id=chatUi(f.root,{action:'create',mode:'group'}).session.id;
 const call=async(name,args)=>{const res=await responseJson(await mcpRequest(f,{jsonrpc:'2.0',id:1,method:'tools/call',params:{name,arguments:{workspace_folder_id:'repo',chat_id,...args}}}));assert.equal(res.result.isError,false,JSON.stringify(res));return res.result.structuredContent;};
 const a=await call('chat_open',{agent_name:'Coordinator'}),b=await call('chat_open',{agent_name:'Helper'});
 assert.notEqual(a.attachment_id,b.attachment_id);assert.equal(a.role,'coordinator');assert.equal(b.role,'member');
 chatUi(f.root,{action:'send',chat_id,message_id:'u',text:'@Helper work'});
 for(const x of [a,b])assert.equal((await call('chat_wait',{attachment_id:x.attachment_id,timeout_ms:0})).message.id,'u');
});

test('switching a live work wait to group keeps one wait per attachment',async t=>{
 const {root}=fixture(t);const chat_id=chatUi(root,{action:'create'}).session.id;
 const args={chat_id,attachment_id:chatTool(root,'chat_open',{chat_id}).attachment_id,timeout_ms:2000};
 const wait=chatWait(root,args);chatUi(root,{action:'set_mode',chat_id,mode:'group'});
 assert.equal(chatUi(root,{action:'read',chat_id}).session.members[0].status,'waiting');
 await assert.rejects(chatWait(root,{...args,timeout_ms:0}),/already active/);
 chatUi(root,{action:'send',chat_id,message_id:'new',text:'Work after switching'});assert.equal((await wait).message.id,'new');
});

test('group Markdown follows the coordinator confirmation after a helper final', t => {
 const {root,a,b,send,reply,read}=fixture(t);
 send('u','@前端 Work');reply(b,'helper-done','u');reply(a,'question','u',{awaiting_user:true});
 let md=readFileSync(path.join(root,read().archive_path),'utf8');
 assert.match(md,/消息状态：已读 · 待确认/);
 send('answer','Approved');
 md=readFileSync(path.join(root,read().archive_path),'utf8');
 assert.match(md,/消息状态：已读 · 已回复/);
});

test('disconnect all pauses members and switching mode cannot revive a paused identity',t=>{
 const {root,a,b}=fixture(t);
 chatUi(root,{action:'detach',chat_id:a.chat_id});
 for(const args of [a,b]){
  assert.throws(()=>chatTool(root,'chat_wait',args),/paused/);
  assert.throws(()=>chatTool(root,'chat_open',args),/paused/);
 }
 chatUi(root,{action:'set_mode',chat_id:a.chat_id,mode:'work'});
 assert.throws(()=>chatTool(root,'chat_open',a),/expired/);
 const fresh=chatTool(root,'chat_open',{chat_id:a.chat_id});assert.notEqual(fresh.attachment_id,a.attachment_id);
});

test('greeting permits roundtrips but sent and queued work lock mode while retaining the same AI',t=>{
 const root=mkdtempSync(path.join(tmpdir(),'chat-roundtrip-'));t.after(()=>rmSync(root,{recursive:true,force:true}));
 const chat_id=chatUi(root,{action:'create'}).session.id;
 const a={chat_id,attachment_id:chatTool(root,'chat_open',{chat_id,agent_name:'Roundtrip'}).attachment_id};
 const mode=value=>chatUi(root,{action:'set_mode',chat_id,mode:value}).session;
 const reply=(id,to)=>chatTool(root,'chat_reply',{...a,message_id:id,reply_to:to,text:'done',final:true});
 chatUi(root,{action:'request_connection',chat_id,message_id:'greeting'});
 const chief=mode('group').members[0].id;
 for(let i=0;i<2;i++){mode('work');assert.equal(chatTool(root,'chat_wait',a).message.id,'greeting');assert.equal(mode('group').members[0].id,chief);}
 mode('work');reply('hello','greeting');mode('group');assert.equal(chatTool(root,'chat_wait',a).status,'idle');
 chatUi(root,{action:'send',chat_id,message_id:'one',text:'one'});
 chatTool(root,'chat_reply',{...a,message_id:'progress',reply_to:'one',text:'working',final:false});
 chatUi(root,{action:'send',chat_id,message_id:'two',text:'two'});
 assert.throws(()=>mode('work'),/fixed after/);mode('group');
 assert.equal(chatTool(root,'chat_reply',{...a,message_id:'progress',reply_to:'one',text:'working',final:false}).persisted,true);
 assert.equal(chatTool(root,'chat_wait',a).message.id,'one');reply('one-done','one');
 assert.equal(chatTool(root,'chat_wait',a).message.id,'two');reply('two-done','two');
 assert.equal(chatTool(root,'chat_wait',a).status,'idle');
 const before=mode('group').messages;assert.throws(()=>mode('work'),/fixed after/);assert.deepEqual(mode('group').messages,before);
});
test('mode rollback cannot discard paused collaborators unfinished assignments or queued mentions',t=>{
 const {root,chat_id,a,b,send,reply}=fixture(t);
 send('one','one');reply(a,'assignment','one',{final:false,recipient_ids:[b.agent_id]});
 chatUi(root,{action:'detach_member',chat_id,member_id:b.agent_id});
 assert.throws(()=>chatUi(root,{action:'set_mode',chat_id,mode:'work'}),/fixed after/);
 chatUi(root,{action:'resume_member',chat_id,member_id:b.agent_id});reply(b,'assigned-done','assignment');
 send('two','@前端 later');chatUi(root,{action:'detach_member',chat_id,member_id:b.agent_id});
 assert.throws(()=>chatUi(root,{action:'set_mode',chat_id,mode:'work'}),/fixed after/);
});

test('group outbox merge/split is reversible and merged delivery preserves saved targets and receipts',t=>{
 const {root,chat_id,a,b,send,reply,read}=fixture(t);
 const file=chatUi(root,{action:'upload',chat_id,upload_id:'queue-attachment',name:'note.txt',data_base64:Buffer.from('attachment').toString('base64')}).attachment;
 send('active','Active');
 const enqueue=(id,text)=>chatUi(root,{action:'send',chat_id,message_id:id,text,attachment_ids:[file.id]});
 enqueue('q1','First');enqueue('q2','Second');enqueue('q3','@前端 Third 😀');
 const before=read().queued_messages;
 for(const mode of ['merge','split','merge','split']){
  const result=chatUi(root,{action:'set_queue_mode',chat_id,mode}).session;
  assert.equal(result.queue_mode,mode);assert.deepEqual(result.queued_messages,before);
  assert.equal(chatTool(root,'chat_wait',a).message.id,'active');
 }
 reply(a,'active-done','active');
 assert.equal(chatTool(root,'chat_wait',a).message.id,'q1');
 assert.deepEqual(read().queued_messages.map(m=>m.id),['q2','q3']);
 chatUi(root,{action:'set_queue_mode',chat_id,mode:'merge'});
 chatUi(root,{action:'rename_member',chat_id,member_id:b.agent_id,name:'Renamed'});
 reply(a,'q1-done','q1');
 assert.equal(read().queued_messages.length,2,'final reply does not publish');
 const merged=chatTool(root,'chat_wait',a).message;
 assert.equal(merged.text,'队列1：\n\nSecond\n\n队列2：\n\n@前端 Third 😀');
 assert.deepEqual(new Set(merged.recipient_ids),new Set([a.agent_id,b.agent_id]));
 assert.deepEqual(merged.attachments.map(f=>f.id),[file.id]);
 assert.equal(chatTool(root,'chat_wait',b).message.id,'q2');
 assert.throws(()=>reply(a,'too-early','q2'),/assigned members/);
 reply(b,'b-merged','q2');reply(a,'a-merged','q2');
 enqueue('q2','Second');enqueue('q3','@前端 Third 😀');
 assert.deepEqual(read().queued_messages,[]);assert.equal(chatTool(root,'chat_wait',a).status,'idle');
 assert.throws(()=>enqueue('q3','Changed'),/conflicts/);
 for(const mode of ['split','merge'])assert.equal(chatUi(root,{action:'set_queue_mode',chat_id,mode}).session.queue_mode,mode);
});

test('first queued user message locks mode in both directions without changing persisted state',t=>{
 const root=mkdtempSync(path.join(tmpdir(),'chat-mode-lock-'));t.after(()=>rmSync(root,{recursive:true,force:true}));
 for(const mode of ['work','group']){
  const chat_id=chatUi(root,{action:'create',mode}).session.id;
  chatUi(root,{action:'request_connection',chat_id,message_id:'greeting'});
  chatUi(root,{action:'send',chat_id,message_id:'first-user',text:'Real user task'});
  const file=path.join(root,'docs/chat-sessions',chat_id+'.json');const before=readFileSync(file,'utf8');
  assert.throws(()=>chatUi(root,{action:'set_mode',chat_id,mode:mode==='work'?'group':'work'}),/fixed after/);
  assert.equal(readFileSync(file,'utf8'),before);
 }
});
