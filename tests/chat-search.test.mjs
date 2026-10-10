import test from 'node:test';
import assert from 'node:assert/strict';
import { searchConversations } from '../src/lib/chat/search.ts';
const item=(id,mode,updated_at,archived=false,workspaceId='a')=>({workspaceId,workspaceName:'Alpha project',folderId:'folder/'+workspaceId,folderName:'Design docs',chat:{id,title:`Plan ${id}`,mode,updated_at,archived}});
test('search covers both modes and legacy work, matches title/project/folder case-insensitively',()=>{
 const rows=[item('old','work',1),item('group','group',3),item('legacy',undefined,2)];
 assert.deepEqual(searchConversations(rows,'ALPHA docs').map(i=>i.chat.id),['group','legacy','old']);
 assert.deepEqual(searchConversations(rows,'','work').map(i=>i.chat.id),['legacy','old']);
 assert.deepEqual(searchConversations(rows,'plan','group').map(i=>i.chat.id),['group']);
 assert.deepEqual(searchConversations(rows,'unmatched'),[]);
});
test('empty search hides archives, typed search finds them and preserves scoped identities',()=>{
 const rows=[item('same','work',1,false,'a'),item('same','group',2,false,'b'),item('archived','group',4,true)];
 assert.deepEqual(searchConversations(rows,'').map(i=>i.workspaceId),['b','a']);
 assert.deepEqual(searchConversations(rows,'plan').map(i=>i.chat.id),['same','same','archived']);
 assert.equal(rows[0].workspaceId,'a');
});
test('recent and query results are bounded independently after sorting',()=>{
 const rows=Array.from({length:50},(_,i)=>item(String(i),'work',i));
 assert.equal(searchConversations(rows,'').length,9);
 assert.equal(searchConversations(rows,'plan').length,40);
 assert.equal(searchConversations(rows,'plan')[0].chat.id,'49');
});
