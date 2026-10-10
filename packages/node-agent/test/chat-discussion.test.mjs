import test from 'node:test';
import assert from 'node:assert/strict';
import {mkdtempSync,rmSync,readFileSync,writeFileSync} from 'node:fs';
import {tmpdir} from 'node:os';
import path from 'node:path';
import {chatUi,chatTool} from '../dist/chat/store.js';
import {discussionAction} from '../dist/chat/discussion.js';
function fixture(t){const root=mkdtempSync(path.join(tmpdir(),'discussion-'));t.after(()=>rmSync(root,{recursive:true,force:true}));const create=()=>chatUi(root,{action:'create'}).session.id;const a=create(),b=create(),c=create();const aa=chatTool(root,'chat_open',{chat_id:a,agent_name:'A'}),bb=chatTool(root,'chat_open',{chat_id:b,agent_name:'B'});const ui=args=>chatUi(root,args),tool=(id,attachment,name,args={})=>chatTool(root,name,{chat_id:id,attachment_id:attachment,...args});const wa=()=>tool(a,aa.attachment_id,'chat_wait'),wb=()=>tool(b,bb.attachment_id,'chat_wait');const group=ui({action:'discussion_create',discussion_id:'group1',title:'Research',goal:'Solve the task',member_chat_ids:[a,b]}).discussion;return {root,a,b,c,aa,bb,ui,tool,wa,wb,group};}
test('independent sessions receive targeted tasks and group collects linked results; retries and inbox are durable',t=>{
 const {a,b,aa,bb,ui,tool,wa,wb}=fixture(t);
 const args={action:'post',discussion_id:'group1',message_id:'task-1',text:'Investigate',purpose:'task',recipient_chat_ids:[b]};
 const post=tool(a,aa.attachment_id,'chat_discuss',args).discussion.posts[0];
 assert.equal(tool(a,aa.attachment_id,'chat_discuss',args).discussion.posts.length,1);
 assert.throws(()=>tool(a,aa.attachment_id,'chat_discuss',{...args,text:'Changed'}),/conflicts/);
 const message=wb().message;assert.equal(message.id,post.deliveries[0].message_id);assert.match(message.text,/Solve the task/);
 tool(b,bb.attachment_id,'chat_reply',{message_id:'result',reply_to:message.id,text:'Found it',final:true});
 const read=ui({action:'discussion_read',discussion_id:'group1'}).discussion;
 assert.equal(read.posts[0].deliveries[0].status,'completed');assert.equal(read.posts[0].deliveries[0].replies[0].text,'Found it');
 const incoming=wa().message;assert.match(incoming.text,/Found it/);assert.equal(incoming.discussion.purpose,'result');
 assert.equal(wa().message.id,incoming.id);
 tool(a,aa.attachment_id,'chat_reply',{message_id:'summary',reply_to:incoming.id,text:'Thank you',final:true});
 assert.equal(wa().status,'idle');assert.equal(wa().status,'idle');
});
test('membership and attachment are enforced; archive retains history; removing a member revokes access',t=>{
 const {root,a,b,c,aa,ui,tool}=fixture(t);const cc=chatTool(root,'chat_open',{chat_id:c,agent_name:'C'});
 assert.deepEqual(tool(c,cc.attachment_id,'chat_discuss',{action:'list'}).discussions,[]);
 assert.throws(()=>tool(c,cc.attachment_id,'chat_discuss',{action:'read',discussion_id:'group1'}),/not a discussion member/);
 assert.throws(()=>tool(a,'bad','chat_discuss',{action:'list'}),/attachment/);
 const args={action:'post',discussion_id:'group1',message_id:'m1',text:'Hello',recipient_chat_ids:[b]};
 assert.throws(()=>tool(a,aa.attachment_id,'chat_discuss',{...args,recipient_chat_ids:[c]}),/members/);
 assert.throws(()=>tool(a,aa.attachment_id,'chat_discuss',{...args,recipient_chat_ids:[a]}),/yourself/);
 tool(a,aa.attachment_id,'chat_discuss',args);ui({action:'discussion_update',discussion_id:'group1',archived:true});
 assert.equal(ui({action:'discussion_read',discussion_id:'group1'}).discussion.posts.length,1);
 assert.throws(()=>tool(a,aa.attachment_id,'chat_discuss',{...args,message_id:'new'}),/archived/);
 ui({action:'discussion_update',discussion_id:'group1',member_chat_ids:[b]});
 assert.throws(()=>tool(a,aa.attachment_id,'chat_discuss',{action:'read',discussion_id:'group1'}),/not a discussion member/);
});
test('merge mode preserves discussion identities between normal user queue entries',t=>{
 const {b,bb,ui,tool,wb}=fixture(t);
 ui({action:'send',chat_id:b,message_id:'first',text:'First'});wb();
 ui({action:'send',chat_id:b,message_id:'before',text:'Before'});
 const posted=ui({action:'discussion_post',discussion_id:'group1',message_id:'post',text:'Separate task',recipient_chat_ids:[b]}).discussion.posts[0];
 ui({action:'send',chat_id:b,message_id:'after',text:'After'});
 const done=(id,to)=>tool(b,bb.attachment_id,'chat_reply',{message_id:id,reply_to:to,text:'Done',final:true});
 done('d1','first');assert.equal(wb().message.id,'before');done('d2','before');
 assert.equal(wb().message.id,posted.deliveries[0].message_id);done('d3',posted.deliveries[0].message_id);assert.equal(wb().message.id,'after');
});
test('partial delivery resumes from durable intent without duplicating earlier recipients',t=>{
 const {root,a,b,ui}=fixture(t);let fail=true;const sessions=new Map([a,b].map(id=>[id,JSON.parse(readFileSync(path.join(root,'docs/chat-sessions',id+'.json'),'utf8'))]));
 const io={safe:p=>path.join(root,p),load:id=>structuredClone(sessions.get(id)),save:s=>{if(s.id===b&&fail)throw Error('Injected disk failure');sessions.set(s.id,structuredClone(s));},validateId:v=>String(v),text:v=>String(v)};
 const args={action:'post',discussion_id:'group1',message_id:'partial',text:'One delivery each'};
 assert.throws(()=>discussionAction(io,args),/Injected disk failure/);assert.equal(sessions.get(a).queue.length,1);fail=false;
 discussionAction(io,args);discussionAction(io,args);assert.equal(sessions.get(a).queue.length,1);assert.equal(sessions.get(b).queue.length,1);
});
test('pagination retains all posts and rejects invalid offsets',t=>{
 const {root,a,b,ui}=fixture(t);const file=path.join(root,'docs/chat-sessions/discussions/group1.json');const d=JSON.parse(readFileSync(file));
 d.posts=Array.from({length:53},(_,i)=>({id:'p'+i,from:'user',text:String(i),name:'User',goal:'',purpose:'notice',created_at:i,targets:[],deliveries:[]}));writeFileSync(file,JSON.stringify(d));
 const first=ui({action:'discussion_read',discussion_id:'group1'}).discussion;assert.equal(first.posts.length,50);assert.equal(first.posts[0].id,'p3');assert.equal(first.next_offset,50);
 const earlier=ui({action:'discussion_read',discussion_id:'group1',offset:50}).discussion;assert.equal(earlier.posts.length,3);assert.equal(earlier.next_offset,null);
 assert.throws(()=>ui({action:'discussion_read',discussion_id:'group1',offset:-1}),/offset/);
});

test('collaboration reuses attachments, routes #aliases, hides controls and exports shared replies',t=>{
 const {root,a,b,c,aa,bb,ui,tool,wa,wb}=fixture(t);
 const create={action:'discussion_create',discussion_id:'team',title:'Team',collaboration:true,member_chat_ids:[a,b],coordinator_chat_id:a};
 const group=ui(create).discussion;assert.equal(group.aliases[a],'A');assert.equal(group.coordinator_chat_id,a);
 assert.throws(()=>ui({...create,discussion_id:'bad',member_chat_ids:[c]}),/connected robots/);
 assert.throws(()=>ui({...create,coordinator_chat_id:b}),/conflicts/);
 ui({action:'send',chat_id:a,message_id:'ordinary',text:'Private task'});
 const post={action:'discussion_post',discussion_id:'team',message_id:'group-default',text:'Plan this'};
 ui(post);ui(post);assert.equal(wa().message.id,'ordinary');
 tool(a,aa.attachment_id,'chat_reply',{message_id:'ordinary-reply',reply_to:'ordinary',text:'Private result',final:true});
 const work=wa().message;assert.equal(work.discussion.hidden,true);assert.match(work.text,/不要重新 chat_open/);assert.match(work.text,/Team/);
 assert.equal(wb().status,'idle');
 assert.deepEqual(ui({action:'read',chat_id:a}).session.messages.map(m=>m.id),['ordinary','ordinary-reply']);
 tool(a,aa.attachment_id,'chat_reply',{message_id:'group-question',reply_to:work.id,text:'Which target?',final:true,awaiting_user:true});
 ui({action:'discussion_post',discussion_id:'team',message_id:'answer',text:'Use default'});
 const answer=wa().message;assert.equal(answer.discussion.post_id,'answer');
 tool(a,aa.attachment_id,'chat_reply',{message_id:'answer-done',reply_to:answer.id,text:'Group plan',final:true});
 ui({action:'discussion_post',discussion_id:'team',message_id:'mention',text:'#b investigate'});
 assert.equal(wa().status,'idle');const assigned=wb().message;assert.equal(assigned.discussion.post_id,'mention');
 tool(b,bb.attachment_id,'chat_reply',{message_id:'b-result',reply_to:assigned.id,text:'Shared finding',final:true});
 const shared=ui({action:'discussion_read',discussion_id:'team'}).discussion;
 assert.equal(shared.posts.length,3);assert.equal(shared.posts[2].deliveries[0].chat_id,b);
 const md=readFileSync(path.join(root,shared.archive_path),'utf8');assert.match(md,/Shared finding/);assert.doesNotMatch(md,/协作控制信息|Private task/);
 const privateMd=readFileSync(path.join(root,`docs/chat-sessions/${a}.md`),'utf8');assert.match(privateMd,/Private result/);assert.doesNotMatch(privateMd,/Group plan|Which target|协作控制信息/);
 assert.equal(ui({action:'list'}).sessions.find(s=>s.id===a).assistant_message_count,1);
 assert.equal(wa().status,'idle');assert.equal(wb().status,'idle');
});

test('coordinator delegation returns hidden results and summary to shared timeline without reconnecting',t=>{
 const {root,a,b,aa,bb,ui,tool,wa,wb}=fixture(t);
 ui({action:'discussion_create',discussion_id:'team',title:'Team',collaboration:true,member_chat_ids:[a,b]});
 const args={action:'post',discussion_id:'team',message_id:'delegate',text:'Inspect',purpose:'task',recipient_chat_ids:[b]};
 tool(a,aa.attachment_id,'chat_discuss',args);const m=wb().message;
 tool(b,bb.attachment_id,'chat_reply',{message_id:'found',reply_to:m.id,text:'Found',final:true});
 const result=wa().message;assert.equal(result.discussion.hidden,true);
 assert.deepEqual(ui({action:'read',chat_id:a}).session.messages,[]);
 tool(a,aa.attachment_id,'chat_reply',{message_id:'summary',reply_to:result.id,text:'Final summary',final:true});
 const d=ui({action:'discussion_read',discussion_id:'team'}).discussion;
 assert.equal(d.posts[0].summaries[0].text,'Final summary');assert.match(readFileSync(path.join(root,d.archive_path),'utf8'),/Final summary/);
 assert.equal(wa().status,'idle');assert.equal(wb().status,'idle');
 ui({action:'discussion_update',discussion_id:'team',member_chat_ids:[a]});
 assert.throws(()=>tool(b,bb.attachment_id,'chat_discuss',{action:'read',discussion_id:'team'}),/not a discussion member/);
});

test('disconnect blocks new group tasks, keeps source attachment and in-flight results, and pin survives reads',t=>{
 const {a,b,aa,bb,ui,tool,wb}=fixture(t);
 const post={action:'discussion_post',discussion_id:'group1',message_id:'before',text:'Finish this',recipient_chat_ids:[b]};
 ui(post);const message=wb().message;const connection=ui({action:'read',chat_id:b}).session.connection_id;
 const paused=ui({action:'discussion_update',discussion_id:'group1',paused:true,pinned:true}).discussion;
 assert.equal(paused.paused,true);assert.equal(paused.pinned,true);
 assert.throws(()=>ui({...post,message_id:'blocked'}),/disconnected/);
 assert.throws(()=>tool(a,aa.attachment_id,'chat_discuss',{...post,action:'post',message_id:'blocked-agent'}),/disconnected/);
 assert.equal(ui({action:'read',chat_id:b}).session.connection_id,connection);
 tool(b,bb.attachment_id,'chat_reply',{message_id:'reply-after-pause',reply_to:message.id,text:'Still finished',final:true});
 assert.equal(ui({action:'discussion_read',discussion_id:'group1'}).discussion.posts[0].deliveries[0].status,'completed');
 ui(post);assert.equal(ui({action:'discussion_read',discussion_id:'group1'}).discussion.posts.length,1);
 ui({action:'discussion_update',discussion_id:'group1',paused:false});ui({...post,message_id:'resumed'});
 const restored=ui({action:'discussion_list'}).discussions[0];assert.equal(restored.paused,false);assert.equal(restored.pinned,true);
 assert.throws(()=>ui({action:'discussion_update',discussion_id:'group1',paused:'true'}),/boolean/);
 assert.throws(()=>tool(a,aa.attachment_id,'chat_discuss',{action:'update',discussion_id:'group1',paused:true}),/local interface/);
});

test('robot cards separate queued work, picked-up tasks, reported failures and finished tasks',t=>{
 const {b,bb,ui,tool,wb}=fixture(t);const state=()=>ui({action:'discussion_read',discussion_id:'group1'}).discussion.member_details.find(m=>m.id===b);
 ui({action:'send',chat_id:b,message_id:'presence-task',text:'Check this'});assert.equal(state().busy,false);assert.equal(state().error,false);assert.equal(state().status,'connected');
 wb();assert.equal(state().busy,true);
 tool(b,bb.attachment_id,'chat_reply',{reply_to:'presence-task',message_id:'presence-fail',text:'Failed',final:false,tool_event:{name:'test',status:'failed'}});assert.equal(state().error,true);
 tool(b,bb.attachment_id,'chat_reply',{reply_to:'presence-task',message_id:'presence-fixed',text:'Recovered',final:false,tool_event:{name:'test',status:'completed'}});assert.equal(state().error,false);assert.equal(state().busy,true);
 tool(b,bb.attachment_id,'chat_reply',{reply_to:'presence-task',message_id:'presence-done',text:'Done',final:true});assert.equal(state().busy,false);assert.equal(state().error,false);
 ui({action:'detach',chat_id:b});assert.equal(state().status,'offline');
});

test('existing collaboration members may go offline without blocking group management',t=>{
 const {a,b,c,ui}=fixture(t);
 ui({action:'discussion_create',discussion_id:'offline-members',title:'Team',collaboration:true,member_chat_ids:[a,b]});ui({action:'detach',chat_id:b});
 const updated=ui({action:'discussion_update',discussion_id:'offline-members',paused:true,pinned:true}).discussion;assert.equal(updated.paused,true);assert.ok(updated.aliases[b]);
 assert.equal(ui({action:'discussion_update',discussion_id:'offline-members',paused:false,member_chat_ids:[a,b]}).discussion.paused,false);
 assert.throws(()=>ui({action:'discussion_update',discussion_id:'offline-members',member_chat_ids:[a,b,c]}),/previously connected/);
});
test('group-owned attachments survive chunk retry, stay isolated, and reach recipients with original labels',t=>{
 const {root,a,b,c,ui,wb}=fixture(t);const cid='discussion:owned';
 ui({action:'discussion_create',discussion_id:'owned',title:'Owned files',member_chat_ids:[a,b],collaboration:true,coordinator_chat_id:a});
 ui({action:'upload',chat_id:b,upload_id:'before',name:'before.txt',data_base64:'Yg=='});
 ui({action:'upload',chat_id:b,upload_id:'shared',name:'different.txt',data_base64:'Yg=='});
 const bytes=Buffer.alloc(512*1024+17,65),chunk=(offset)=>({action:'upload_chunk',chat_id:cid,upload_id:'shared',name:'large.txt',offset,total_size:bytes.length,data_base64:bytes.subarray(offset,offset+512*1024).toString('base64')});
 assert.equal(ui(chunk(0)).next_offset,512*1024);assert.equal(ui(chunk(0)).next_offset,512*1024);
 const file=ui(chunk(512*1024)).attachment;assert.equal(file.label,'文件1');assert.equal(ui(chunk(512*1024)).attachment.id,file.id);
 assert.deepEqual(ui({action:'list'}).sessions.map(s=>s.id).sort(),[a,b,c].sort());
 const args={action:'discussion_post',discussion_id:'owned',message_id:'attached',text:'',attachment_ids:[file.id],recipient_chat_ids:[b]};
 const first=ui(args).discussion.posts[0];assert.equal(first.text,'📎');assert.deepEqual(ui(args).discussion.posts[0].attachments,[file]);
 assert.throws(()=>ui({...args,text:'📎',attachment_ids:[]}),/conflicts/);
 const received=wb().message;assert.deepEqual(received.attachments,[file]);assert.deepEqual(wb().message.attachments,[file]);assert.equal(readFileSync(path.join(root,file.path)).length,bytes.length);
 assert.equal(Buffer.from(ui({action:'read_attachment_chunk',chat_id:cid,upload_id:file.id,offset:512*1024}).data_base64,'base64').length,17);
 assert.equal(ui({action:'read_attachment',chat_id:cid,upload_id:file.id}).attachment.sha256,file.sha256);
 assert.equal(ui({action:'discussion_read',discussion_id:'owned'}).discussion.files,undefined);
 assert.match(readFileSync(path.join(root,'docs/chat-sessions/discussions/owned.md'),'utf8'),/large.txt/);
 ui({action:'discussion_create',discussion_id:'other',title:'Other',member_chat_ids:[a,b]});
 assert.throws(()=>ui({action:'read_attachment_chunk',chat_id:'discussion:other',upload_id:file.id,offset:0}),/not found/);
 assert.throws(()=>ui({...args,discussion_id:'other'}),/does not belong/);
 assert.throws(()=>ui({action:'send',chat_id:cid,message_id:'bad',text:'x'}),/Unsupported/);
 ui({action:'discussion_update',discussion_id:'owned',paused:true});
 assert.throws(()=>ui({...chunk(0),upload_id:'blocked'}),/closed/);assert.equal(ui({action:'read_attachment_chunk',chat_id:cid,upload_id:file.id,offset:0}).attachment.id,file.id);
});
test('robot group posts and task result inboxes preserve their own uploaded attachments',t=>{
 const {a,b,aa,bb,ui,tool,wa,wb}=fixture(t);
 const upload=(chat,id)=>ui({action:'upload',chat_id:chat,upload_id:id,name:id+'.txt',data_base64:'YQ=='}).attachment;
 const source=upload(a,'task-file'),result=upload(b,'result-file');
 const args={action:'post',discussion_id:'group1',message_id:'file-task',text:'Check',purpose:'task',recipient_chat_ids:[b],attachment_ids:[source.id]};
 const p=tool(a,aa.attachment_id,'chat_discuss',args).discussion.posts[0];assert.equal(p.attachment_chat_id,a);
 assert.throws(()=>tool(a,aa.attachment_id,'chat_discuss',{...args,message_id:'bad',attachment_ids:[result.id]}),/does not belong/);
 const received=wb().message;assert.deepEqual(received.attachments,[source]);
 tool(b,bb.attachment_id,'chat_reply',{message_id:'result',reply_to:received.id,text:'Done',final:true,attachment_ids:[result.id]});
 assert.deepEqual(wa().message.attachments,[result]);
});

test('group messages preserve code indentation, blank lines and exact retry identity',t=>{
 const {ui,wb}=fixture(t),text='    code\n\n**正文**  \n\tend\n\n';
 const send={action:'discussion_post',discussion_id:'group1',message_id:'format-group',text,recipient_chat_ids:['unused']};delete send.recipient_chat_ids;
 const post=ui(send).discussion.posts[0];assert.equal(post.text,text);assert.equal(ui(send).discussion.posts.length,1);
 assert.throws(()=>ui({...send,text:text.trim()}),/conflict/);assert.ok(wb().message.text.includes(text));
});

test('discussion recent timestamp includes member replies but excludes metadata updates',t=>{
 const {root,ui,tool,b,bb,wb}=fixture(t);let clock=Date.now()+1000;t.mock.method(Date,'now',()=>clock);
 ui({action:'discussion_post',discussion_id:'group1',message_id:'activity-post',text:'Task',recipient_chat_ids:[b]});
 const summary=()=>ui({action:'discussion_list'}).discussions.find(d=>d.id==='group1');
 assert.equal(summary().last_message_at,clock);clock+=1000;
 ui({action:'discussion_update',discussion_id:'group1',title:'Renamed',pinned:true});assert.equal(summary().last_message_at,clock-1000);
 const message=wb().message;tool(b,bb.attachment_id,'chat_reply',{message_id:'activity-reply',reply_to:message.id,text:'Result',final:true});
 const archive=path.join(root,'docs/chat-sessions/discussions/group1.json'),before=readFileSync(archive,'utf8');
 assert.equal(summary().last_message_at,clock);assert.equal(summary().posts,undefined);assert.equal(readFileSync(archive,'utf8'),before,'listing must not rewrite the archive');
});
