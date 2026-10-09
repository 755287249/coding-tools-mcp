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
  assert.deepEqual(summarizeToolActivity(messages),{calls:2,completed:1,failed:0,running:1});
  messages.push(event('d','failed'));
  assert.deepEqual(summarizeToolActivity(messages),{calls:2,completed:1,failed:1,running:0});
  messages.push(event('e','running','read_file'));
  assert.equal(summarizeToolActivity(messages).running,1);
});

test('result-only older reports count once and cannot finish another tool name', () => {
  assert.deepEqual(summarizeToolActivity([event('a','completed')]),{calls:1,completed:1,failed:0,running:0});
  assert.deepEqual(summarizeToolActivity([event('a','running'),event('b','completed','read_file')]),{calls:2,completed:1,failed:0,running:1});
  const orphan=event('a','completed');delete orphan.reply_to;
  const orphan2=event('b','completed');delete orphan2.reply_to;
  assert.equal(groupChatMessages([orphan,orphan2]).length,2);
});
