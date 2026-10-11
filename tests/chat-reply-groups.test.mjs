import test from 'node:test';
import assert from 'node:assert/strict';
import {groupReplyTurns,combinedReply} from '../src/lib/chat/reply-groups.ts';
const reply=(id,text,extra={})=>({id,role:'assistant',reply_to:'user',agent_name:'Builder',text,created_at:1000,final:false,...extra});
test('a full reply preserves progress, tools, final text and unique attachments with one stable identity',()=>{
 const file={id:'file',path:'a.txt',name:'a.txt'};
 const messages=[{id:'user',role:'user',text:'request',received_at:500},reply('first','first',{attachments:[file]}),reply('tool','report',{agent_name:undefined,tool_event:{name:'check',status:'failed',input:'in',output:'out',output_truncated:true}}),reply('final','last',{final:true,attachments:[file],questions:[{prompt:'Question?',options:[{label:'A',description:'option'}]}]})];
 const groups=groupReplyTurns(messages);assert.equal(groups.length,2);assert.equal(groups[1].id,'reply:first');assert.equal(groups[1].request.received_at,500);assert.equal(groups[1].messages.length,3);
 const result=combinedReply(groups[1]);assert.equal(result.attachments.length,1);assert.equal(result.final,true);
 for(const text of ['first','report','check (failed)','in','out','Output truncated','last','Question?','option'])assert.ok(result.text.includes(text));
 assert.equal(messages[1].text,'first');
});
test('different requests and agent identities never share a reply or completion state',()=>{
 const groups=groupReplyTurns([reply('a','one',{agent_id:'A'}),reply('b','two',{agent_id:'B',final:true}),reply('c','three',{agent_id:'A'}),reply('d','four',{reply_to:'other'}),reply('orphan','five',{reply_to:undefined}),reply('orphan2','six',{reply_to:undefined})]);
 assert.equal(groups.length,5);assert.deepEqual(groups[0].messages.map(m=>m.id),['a','c']);assert.equal(combinedReply(groups[0]).final,false);assert.equal(combinedReply(groups[1]).final,true);
});
test('ambiguous unnamed reports are not attributed to either named agent',()=>{
 const groups=groupReplyTurns([reply('a','one'),reply('b','two',{agent_name:'Other'}),reply('c','report',{agent_name:undefined,tool_event:{name:'tool',status:'completed'}})]);
 assert.equal(groups.length,3);assert.equal(groups[2].name,'AI');
});
