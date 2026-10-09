import assert from 'node:assert/strict';
import test from 'node:test';
import { groupChatMessages, summarizeToolActivity } from '../src/lib/chat/tool-activity.ts';
const event = (id, status, name = 'bash', reply_to = 'u1') => ({id, role:'assistant', reply_to, text:id, created_at:1, final:false, tool_event:{name,status}});

test('interleaved progress keeps one activity group per user request without dropping prose or attachments', () => {
  const a = event('start', 'running'); a.attachments=[{id:'file1'}];
  const progress={id:'progress',role:'assistant',reply_to:'u1',text:'still working'};
  const result=groupChatMessages([{id:'u1',role:'user',text:'do work'},a,progress,event('end','completed'),event('other','running','read_file','u2')]);
  assert.deepEqual(result.map(item=>item.kind),['message','tools','message','tools']);
  assert.deepEqual(result[1].messages.map(m=>m.id),['start','end']);
  assert.equal(result[1].messages[0].attachments[0].id,'file1');
  assert.equal(result[2].message,progress);
});

test('parallel same-name work stays running until every start has a terminal report', () => {
  const messages=[event('a','running'),event('b','running'),event('c','completed')];
  assert.deepEqual(summarizeToolActivity(messages),{calls:2,completed:1,failed:0,running:1,unresolved:0});
  messages.push(event('d','failed'));
  assert.deepEqual(summarizeToolActivity(messages),{calls:2,completed:1,failed:1,running:0,unresolved:0});
  messages.push(event('e','running','read_file'));
  assert.equal(summarizeToolActivity(messages).running,1);
});

test('result-only older reports count once and cannot finish another tool name', () => {
  assert.deepEqual(summarizeToolActivity([event('a','completed')]),{calls:1,completed:1,failed:0,running:0,unresolved:0});
  assert.deepEqual(summarizeToolActivity([event('a','running'),event('b','completed','read_file')]),{calls:2,completed:1,failed:0,running:1,unresolved:0});
  const orphan=event('a','completed');delete orphan.reply_to;
  const orphan2=event('b','completed');delete orphan2.reply_to;
  assert.equal(groupChatMessages([orphan,orphan2]).length,2);
});

test('final reply stops stale activity without turning an unmatched start into success', () => {
  const reports=[event('a','running'),event('b','running'),event('c','completed')];
  const messages=[...reports,{id:'answer',role:'assistant',reply_to:'u1',final:true,text:'Done'}];
  const group=groupChatMessages(messages).find(item=>item.kind==='tools');
  assert.equal(group.settled,true);
  assert.deepEqual(summarizeToolActivity(group.messages,group.settled),{
    calls:2,completed:1,failed:0,running:0,unresolved:1,
  });
  assert.deepEqual(group.messages,reports);
  assert.equal(reports[0].tool_event.status,'running'); // Original evidence is preserved.
});

test('a completed or awaiting-user request cannot settle another active request', () => {
  const messages=[event('a','running'),
    {id:'question',role:'assistant',reply_to:'u1',final:true,awaiting_user:true,text:'Which option?'},
    event('b','running','bash','u2'),
    {id:'progress',role:'assistant',reply_to:'u2',final:false,text:'Working'}];
  const groups=groupChatMessages(messages).filter(item=>item.kind==='tools');
  assert.deepEqual(groups.map(group=>group.settled),[true,false]);
  assert.equal(summarizeToolActivity(groups[0].messages,groups[0].settled).unresolved,1);
  assert.equal(summarizeToolActivity(groups[1].messages,groups[1].settled).running,1);
});

test('closing a conversation ends incomplete reports and retains known failures', () => {
  const messages=[event('a','running'),event('b','running'),event('c','failed')];
  const group=groupChatMessages(messages,true)[0];
  assert.equal(group.settled,true);
  assert.deepEqual(summarizeToolActivity(group.messages,group.settled),{
    calls:2,completed:0,failed:1,running:0,unresolved:1,
  });
});

test('members with same request and tool name keep separate activity groups',()=>{
 const a={...event('a','running'),agent_id:'a'},b={...event('b','running'),agent_id:'b'};
 const groups=groupChatMessages([a,b,{id:'done',role:'assistant',reply_to:'u1',agent_id:'b',final:true}]).filter(x=>x.kind==='tools');
 assert.equal(groups.length,2);assert.equal(groups[0].settled,false);assert.equal(groups[1].settled,true);
});
