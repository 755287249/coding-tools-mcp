import assert from 'node:assert/strict';
import test from 'node:test';
import { pendingChatState } from '../src/lib/chat/status.ts';

test('pickup, progress and final replies drive the pending indicator', () => {
  const user = {id:'u1', role:'user', text:'hello', created_at:1};
  const session = {closed:false, status:'connected', messages:[user]};
  assert.equal(pendingChatState(session), 'queued', 'attachment alone does not mean pickup');
  user.received_at = 2;
  assert.equal(pendingChatState(session), 'processing');
  session.messages.push({id:'p1', role:'assistant', reply_to:'u1', text:'working', final:false});
  assert.equal(pendingChatState(session), 'processing', 'progress must not hide the indicator');
  session.status = 'offline';
  assert.equal(pendingChatState(session), 'interrupted');
  session.status = 'connected';
  session.messages.push({id:'a1', role:'assistant', reply_to:'u1', text:'done', final:true});
  assert.equal(pendingChatState(session), null);
  session.messages.push({id:'u2', role:'user', text:'next', created_at:3});
  assert.equal(pendingChatState(session), 'queued', 'old replies do not acknowledge the next message');
  session.closed = true;
  assert.equal(pendingChatState(session), null);
});

test('older servers can establish pickup via progress; empty sessions stay quiet', () => {
  assert.equal(pendingChatState(null), null);
  assert.equal(pendingChatState({closed:false, messages:[]}), null);
  assert.equal(pendingChatState({closed:false, status:'connected', messages:[
    {id:'u1',role:'user'}, {id:'p1',role:'assistant',reply_to:'u1',final:false}
  ]}), 'processing');
});

test('supplementary and confirmation labels track replies without blocking the next request', async () => {
  const {chatReplyState}=await import('../src/lib/chat/status.ts');
  const session={closed:false,status:'connected',messages:[{id:'u1',role:'user'}, {id:'p1',role:'assistant',reply_to:'u1',final:false}]};
  assert.equal(chatReplyState(session,'p1'),'supplementing');
  session.messages.push({id:'q1',role:'assistant',reply_to:'u1',final:true,awaiting_user:true});
  assert.equal(chatReplyState(session,'p1'),null);
  assert.equal(chatReplyState(session,'q1'),'awaiting_user');
  assert.equal(pendingChatState(session),'awaiting_user');
  session.messages.push({id:'u2',role:'user'});
  assert.equal(chatReplyState(session,'q1'),'answered');
  assert.equal(pendingChatState(session),'queued');
  session.messages.push({id:'a2',role:'assistant',reply_to:'u2',final:true});
  assert.equal(chatReplyState(session,'a2'),'complete');
  assert.equal(pendingChatState(session),null);
  assert.equal(chatReplyState(session,'u1'),null);
  assert.equal(chatReplyState(session,'missing'),null);
  session.messages.push({id:'u3',role:'user'}, {id:'q3',role:'assistant',reply_to:'u3',final:true,awaiting_user:true});
  session.closed=true;
  assert.equal(chatReplyState(session,'q3'),null);
  assert.equal(pendingChatState(session),null);
});

test('per-message receipts distinguish the active message from unread queued messages', async () => {
  const {chatUserState}=await import('../src/lib/chat/status.ts');
  const session={closed:false,status:'connected',messages:[{id:'u1',role:'user'},{id:'u2',role:'user'}]};
  assert.deepEqual(chatUserState(session,'u1'),{read:false,status:'queued'});
  session.messages[0].received_at=10;
  assert.deepEqual(chatUserState(session,'u1'),{read:true,status:'processing'});
  assert.deepEqual(chatUserState(session,'u2'),{read:false,status:'queued'});
  session.messages.push({id:'p1',role:'assistant',reply_to:'u1',final:false});
  delete session.messages[0].received_at;
  assert.deepEqual(chatUserState(session,'u1'),{read:true,status:'processing'},'legacy reply proves receipt');
  session.messages.push({id:'q1',role:'assistant',reply_to:'u1',final:true,awaiting_user:true});
  assert.deepEqual(chatUserState(session,'u1'),{read:true,status:'awaiting_user'},'earlier queued user is not an answer to a later question');
  session.messages.push({id:'u3',role:'user'});
  assert.deepEqual(chatUserState(session,'u1'),{read:true,status:'replied'});
  session.closed=true;
  assert.deepEqual(chatUserState(session,'u2'),{read:false,status:'closed'});
  assert.deepEqual(chatUserState(session,'u1'),{read:true,status:'replied'});
  assert.equal(chatUserState(session,'p1'),null);
});
