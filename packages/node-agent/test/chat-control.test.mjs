import test from 'node:test';
import assert from 'node:assert/strict';
import {mkdtempSync,rmSync,readFileSync} from 'node:fs';
import {tmpdir} from 'node:os';
import path from 'node:path';
import {chatUi,chatTool,chatStopNotice} from '../dist/chat/store.js';

function setup(t){
  const root=mkdtempSync(path.join(tmpdir(),'chat-control-'));t.after(()=>rmSync(root,{recursive:true,force:true}));
  const chat=chatUi(root,{action:'create',seed_manual:true}).session.id;
  const opened=chatTool(root,'chat_open',{chat_id:chat});
  return {root,chat,args:{chat_id:chat,attachment_id:opened.attachment_id}};
}
const send=(root,chat,id,text)=>chatUi(root,{action:'send',chat_id:chat,message_id:id,text});
const reply=(root,args,id,to,text)=>chatTool(root,'chat_reply',{...args,message_id:id,reply_to:to,text,final:true});

test('recall before pickup hides the message from the AI and the archive',t=>{
  const {root,chat,args}=setup(t);
  send(root,chat,'m1','secret draft');
  chatUi(root,{action:'recall',chat_id:chat,message_id:'m1'});
  assert.equal(chatTool(root,'chat_wait',args).status,'idle');
  send(root,chat,'m1','secret draft');
  assert.equal(chatTool(root,'chat_wait',args).status,'idle');
  assert.ok(!readFileSync(path.join(root,'docs/chat-sessions',`${chat}.md`),'utf8').includes('secret draft'));
});

test('recall during work stops tools until the AI replies or waits',t=>{
  const {root,chat,args}=setup(t);
  send(root,chat,'m1','do it');
  assert.equal(chatTool(root,'chat_wait',args).message.id,'m1');
  chatUi(root,{action:'recall',chat_id:chat,message_id:'m1'});
  assert.equal(chatStopNotice(root,chat).error.code,'USER_RECALLED');
  const late=reply(root,args,'r1','m1','done');
  assert.equal(late.persisted,false);
  assert.equal(chatStopNotice(root,chat),undefined);
  assert.equal(chatTool(root,'chat_wait',args).status,'idle');
});

test('pause keeps the connection and delivers the next message',t=>{
  const {root,chat,args}=setup(t);
  send(root,chat,'m1','long task');chatTool(root,'chat_wait',args);
  chatUi(root,{action:'interrupt',chat_id:chat});
  assert.equal(chatStopNotice(root,chat).error.code,'USER_PAUSED');
  assert.equal(reply(root,args,'r1','m1','paused at step 2').persisted,true);
  send(root,chat,'m2','new instruction');
  assert.equal(chatTool(root,'chat_wait',args).message.id,'m2');
});

test('jump queue interrupts and delivers the correction first',t=>{
  const {root,chat,args}=setup(t);
  chatUi(root,{action:'set_queue_mode',chat_id:chat,mode:'split'});
  send(root,chat,'m1','task');chatTool(root,'chat_wait',args);
  send(root,chat,'q1','later');send(root,chat,'q2','correction');
  chatUi(root,{action:'jump_queue',chat_id:chat,message_id:'q2'});
  assert.equal(chatStopNotice(root,chat).error.code,'USER_CORRECTION');
  assert.equal(chatTool(root,'chat_wait',args).message.id,'q2');
  reply(root,args,'r2','q2','ok');
  assert.equal(chatTool(root,'chat_wait',args).message.id,'q1');
});

test('answered messages cannot be recalled or paused',t=>{
  const {root,chat,args}=setup(t);
  send(root,chat,'m1','hi');chatTool(root,'chat_wait',args);reply(root,args,'r1','m1','hello');
  assert.throws(()=>chatUi(root,{action:'recall',chat_id:chat,message_id:'m1'}));
  assert.throws(()=>chatUi(root,{action:'interrupt',chat_id:chat}));
});

test('pause holds the queue until the user sends again',t=>{
  const {root,chat,args}=setup(t);
  send(root,chat,'m1','task');chatTool(root,'chat_wait',args);
  send(root,chat,'q1','queued');
  assert.throws(()=>chatUi(root,{action:'interrupt',chat_id:chat,message_id:'other'}));
  chatUi(root,{action:'interrupt',chat_id:chat,message_id:'m1'});
  assert.equal(chatTool(root,'chat_wait',args).status,'idle');
  send(root,chat,'m2','go on');
  assert.equal(chatTool(root,'chat_wait',args).message.id,'q1');
});

test('interrupt needs a picked-up task and recall returns merged text',t=>{
  const {root,chat,args}=setup(t);
  send(root,chat,'m1','first');
  assert.throws(()=>chatUi(root,{action:'interrupt',chat_id:chat}));
  chatTool(root,'chat_wait',args);
  send(root,chat,'q1','a');send(root,chat,'q2','b');
  reply(root,args,'r1','m1','ok');chatTool(root,'chat_wait',args);
  const out=chatUi(root,{action:'recall',chat_id:chat,message_id:'q1'});
  assert.ok(out.recalled.text.includes('a')&&out.recalled.text.includes('b'),out.recalled.text);
});

test('a recalled answer keeps the question open',t=>{
  const {root,chat,args}=setup(t);
  send(root,chat,'m1','task');chatTool(root,'chat_wait',args);
  chatTool(root,'chat_reply',{...args,message_id:'r1',reply_to:'m1',text:'which?',final:true,awaiting_user:true});
  send(root,chat,'m2','answer');send(root,chat,'q1','unrelated');
  chatUi(root,{action:'recall',chat_id:chat,message_id:'m2'});
  assert.equal(chatTool(root,'chat_wait',args).status,'idle');
});
