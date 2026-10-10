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
