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
 assert.throws(()=>chatUi(root,{action:'set_mode',chat_id:a.chat_id,mode:'work'}),/Disconnect/);
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
 assert.equal(chatUi(root,{action:'set_mode',chat_id:a.chat_id,mode:'work'}).session.mode,'work');
 assert.equal(chatTool(root,'chat_open',a).attachment_id,a.attachment_id);
 const legacy=chatUi(root,{action:'create'}).session.id;const old=chatTool(root,'chat_open',{chat_id:legacy,agent_name:'Existing'});
 chatUi(root,{action:'send',chat_id:legacy,message_id:'old-u',text:'Existing task'});
 chatTool(root,'chat_reply',{chat_id:legacy,attachment_id:old.attachment_id,message_id:'progress',reply_to:'old-u',text:'working',final:false});
 chatUi(root,{action:'set_mode',chat_id:legacy,mode:'group'});const resumed=chatTool(root,'chat_open',{chat_id:legacy,attachment_id:old.attachment_id});
 assert.equal(resumed.role,'coordinator');assert.equal(resumed.session.members[0].name,'Existing');
 assert.equal(chatTool(root,'chat_reply',{chat_id:legacy,attachment_id:old.attachment_id,message_id:'progress',reply_to:'old-u',text:'working',final:false}).persisted,true);
});
test('group membership identity survives lease expiry; false credentials, duplicate names and malformed assignments fail',t=>{
 const {root,a,b,join,send,reply}=fixture(t);const archive=path.join(root,'docs/chat-sessions',a.chat_id+'.json');const s=JSON.parse(readFileSync(archive));s.members[1].lease_until=0;writeFileSync(archive,JSON.stringify(s));
 assert.throws(()=>chatTool(root,'chat_wait',b),/expired/);assert.equal(chatTool(root,'chat_open',b).agent_id,b.agent_id);
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
