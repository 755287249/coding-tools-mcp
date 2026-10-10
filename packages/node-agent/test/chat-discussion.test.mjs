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
