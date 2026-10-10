import test from 'node:test';
import assert from 'node:assert/strict';
import {robotQuery,memberTint,discussionTimeline} from '../src/lib/chat/collaboration.ts';
test('robot completion keeps # separate from attachment references and middle-of-text insertion',()=>{
 assert.deepEqual(robotQuery('check #codex1 next',13),{start:6,end:13,query:'codex1'});
 assert.equal(robotQuery('@图片1',4),null);assert.equal(robotQuery('url#anchor',10),null);
 assert.deepEqual(robotQuery('#',1),{start:0,end:1,query:''});
 assert.equal(memberTint('stable-id'),memberTint('stable-id'));assert.notEqual(memberTint('a'),memberTint('b'));
});
test('shared timeline includes delegation summaries, stable source identity and chronological replies',()=>{
 const d={aliases:{a:'Renamed',b:'B'},posts:[{id:'p',from:'a',name:'A',text:'Task',created_at:1,deliveries:[{chat_id:'b',title:'B',replies:[{id:'r',text:'Result',created_at:3}]}],summaries:[{id:'s',text:'Summary',created_at:4}]}]};
 const rows=discussionTimeline(d);assert.deepEqual(rows.map(r=>r.text),['Task','Result','Summary']);assert.deepEqual(rows.map(r=>r.memberId),['a','b','a']);assert.equal(rows[2].memberName,'Renamed');
});
