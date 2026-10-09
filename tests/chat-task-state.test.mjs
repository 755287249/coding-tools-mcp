import test from 'node:test';
import assert from 'node:assert/strict';
import {readFileSync} from 'node:fs';
import {stripTypeScriptTypes} from 'node:module';
const source=readFileSync(new URL('../src/lib/chat/task-state.ts',import.meta.url),'utf8').replace("'./status'",JSON.stringify(new URL('../src/lib/chat/status.ts',import.meta.url).href));
const {currentChatTask,taskPlanMarkdown}=await import('data:text/javascript;base64,'+Buffer.from(stripTypeScriptTypes(source)).toString('base64'));
const old={id:'old',role:'user',text:'old task',created_at:1,received_at:2,task_plan:{goal:'Old goal',todos:[{id:'step',title:'done',status:'completed'}],updated_ms:2}};
const end={id:'done',role:'assistant',text:'complete',reply_to:'old',final:true,created_at:3};
test('new request replaces completed task immediately; pickup, interruption and confirmation use message evidence',()=>{
 const session={status:'connected',closed:false,messages:[old,end]};
 assert.equal(currentChatTask(session).status,'completed');
 session.messages.push({id:'new',role:'user',text:'new request',created_at:4});
 assert.equal(currentChatTask(session).status,'queued');assert.equal(currentChatTask(session).plan,undefined);
 session.messages.at(-1).received_at=5;assert.equal(currentChatTask(session).status,'processing');
 session.status='offline';assert.equal(currentChatTask(session).status,'interrupted');
 session.messages.push({id:'ask',role:'assistant',reply_to:'new',text:'Confirm?',final:true,awaiting_user:true,created_at:6});
 assert.equal(currentChatTask(session).status,'awaiting_user');
 session.messages.push({id:'answer',role:'user',text:'yes',created_at:7});
 assert.equal(currentChatTask(session).message.id,'answer');assert.equal(currentChatTask(session).status,'queued');
});
test('plans, work logs and progress never leak from another request or chat',()=>{
 const session={status:'connected',messages:[old,end,{id:'new',role:'user',text:'task',created_at:4,received_at:5,task_plan:{goal:'New',todos:[],updated_ms:5,progress:{message:'new report',updated_ms:10}}},{id:'old-tool',role:'assistant',reply_to:'old',text:'old output',tool_event:{name:'exec',status:'completed'},created_at:9}]};
 assert.equal(currentChatTask(session).progress,'new report');assert.deepEqual(currentChatTask(session).tools,[]);
 assert.equal(currentChatTask({status:'connected',messages:[]}),null);assert.equal(currentChatTask(null),null);
 session.messages.push({id:'new-tool',role:'assistant',reply_to:'new',text:'output',tool_event:{name:'exec',status:'running'},created_at:11});
 assert.equal(currentChatTask(session).tools.length,1);
 assert.match(taskPlanMarkdown(old.task_plan),/\[x\] done/);
});

test('group task panel keeps participant plans separate, including delegated work',()=>{
 const p={goal:'Build',todos:[],updated_ms:1};
 const session={status:'connected',members:[{id:'a',name:'Chief'},{id:'b',name:'Builder'}],messages:[{id:'u',role:'user',text:'Task',recipient_ids:['a'],received_at:1,agent_plans:[{agent_id:'a',plan:{...p,goal:'Coordinate'}}]},{id:'assign',role:'assistant',kind:'assignment',reply_to:'u',recipient_ids:['b'],text:'Build component',agent_plans:[{agent_id:'b',plan:p}]},{id:'bdone',role:'assistant',agent_id:'b',reply_to:'assign',final:true}]};
 const task=currentChatTask(session);assert.equal(task.status,'processing');assert.deepEqual(task.agentTasks.map(x=>[x.name,x.plan.goal,x.complete]),[['Chief','Coordinate',false],['Builder','Build',true]]);
});
