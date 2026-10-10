import test from 'node:test';
import assert from 'node:assert/strict';
import {discussionDrafts} from '../src/lib/chat/discussion-drafts.ts';
test('group drafts retain files and exact retry IDs per scope, including storage failure',()=>{
 const values=new Map();globalThis.localStorage={getItem:k=>values.get(k)??null,setItem:(k,v)=>values.set(k,v),removeItem:k=>values.delete(k)};
 const file={id:'f',name:'f.txt',path:'mcp-assistant/chat-assets/d/f.bin',mime:'text/plain',size:1,sha256:'a'.repeat(64)};
 const draft={text:'@文件1',attachments:[file],pending:{action:'discussion_post',discussion_id:'g',message_id:'stable-id',text:'@文件1',attachment_ids:['f']}};
 assert.equal(discussionDrafts.save('group-a',draft),true);draft.attachments.length=0;
 assert.equal(discussionDrafts.load('group-a').attachments.length,1);assert.equal(discussionDrafts.load('group-a').pending.message_id,'stable-id');assert.equal(discussionDrafts.load('group-b').pending,null);
 globalThis.localStorage.setItem=()=>{throw Error('Storage full')};assert.equal(discussionDrafts.save('group-a',discussionDrafts.load('group-a')),false);assert.equal(discussionDrafts.load('group-a').attachments.length,1);
 values.set('invalid','{');assert.deepEqual(discussionDrafts.load('invalid'),{text:'',attachments:[],pending:null});
 delete globalThis.localStorage;
});
