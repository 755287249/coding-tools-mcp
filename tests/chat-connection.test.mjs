import test from 'node:test';
import assert from 'node:assert/strict';
import { connectionResult, visibleChatMessages, isPristineConversation, canChooseConversationMode } from '../src/lib/chat/connection.ts';
const request = {id:'request',role:'user',kind:'connection_request',text:'connect',created_at:1};
const greeting = {id:'reply',role:'assistant',reply_to:'request',text:'你好，有什么能帮到你？',final:true,created_at:2};
test('pairing requires the exact persisted final reply to the current request',()=>{
  for(const status of ['offline','connected','waiting']) assert.equal(connectionResult({status,messages:[request]},'request'),'waiting');
  assert.equal(connectionResult({messages:[request,{...greeting,final:false}]},'request'),'waiting');
  assert.equal(connectionResult({messages:[request,{...greeting,reply_to:'old'}]},'request'),'waiting');
  assert.equal(connectionResult({messages:[greeting]},'request'),'waiting');
  assert.equal(connectionResult({messages:[request,greeting]},'request'),'success');
  assert.equal(connectionResult({messages:[request,{...greeting,text:'你好，有什么能帮到你? 已接入。'}]},'request'),'success');
  assert.equal(connectionResult({messages:[request,{...greeting,text:'unrelated'}]},'request'),'failed');
  assert.equal(connectionResult({messages:[request,{...greeting,awaiting_user:true}]},'request'),'failed');
});
test('connection control is hidden without hiding genuine messages or the greeting',()=>{
  assert.deepEqual(visibleChatMessages({messages:[request,greeting,{id:'u',role:'user',text:'work'}]}).map(m=>m.id),['reply','u']);
  assert.deepEqual(visibleChatMessages(null),[]);
});

test('mode selection is prominent only before conversation activity, including hidden connection controls',()=>{
 const empty={status:'offline',messages:[]};
 assert.equal(isPristineConversation(empty),true);
 for(const s of [null,{...empty,closed:true},{...empty,status:'connected'},{...empty,status:'waiting'},{...empty,messages:[request]},{...empty,messages:[greeting]},{...empty,members:[{id:'ai',status:'offline'}]},{...empty,queued_messages:[request]},{...empty,connection_id:'public-id'},{...empty,agent_name:'AI'}])assert.equal(isPristineConversation(s),false);
});

test('mode selection stays available through pairing but locks after sent or queued user content',()=>{
 assert.equal(canChooseConversationMode(null),false);
 for(const status of ['offline','connected','waiting'])assert.equal(canChooseConversationMode({status,messages:[request,greeting],members:[{id:'ai'}]}),true);
 for(const session of [{closed:true},{messages:[{role:'user',text:'task'}]},{queued_messages:[{role:'user',text:'task'}]},{messages:[{id:'u',role:'user',text:'task'},{role:'assistant',reply_to:'u',final:true}]}])assert.equal(canChooseConversationMode(session),false);
});
