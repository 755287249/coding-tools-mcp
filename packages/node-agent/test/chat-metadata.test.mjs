import test from 'node:test';
import assert from 'node:assert/strict';
import {mkdtempSync,rmSync,readFileSync} from 'node:fs';
import {tmpdir} from 'node:os';
import path from 'node:path';
import {chatUi,chatTool} from '../dist/chat/store.js';
function fixture(t){const root=mkdtempSync(path.join(tmpdir(),'chat-metadata-'));t.after(()=>rmSync(root,{recursive:true,force:true}));return {root,create:()=>chatUi(root,{action:'create'}).session.id,read:id=>chatUi(root,{action:'read',chat_id:id}).session,open:(id,name='CodeRabbit',extra={})=>chatTool(root,'chat_open',{chat_id:id,agent_name:name,...extra})};}
test('agent titles allocate smallest unused name, remain stable on resume, and reuse deleted slots',t=>{
 const {root,create,read,open}=fixture(t),a=create(),b=create(),c=create();
 const aa=open(a),bb=open(b);open(c);
 assert.deepEqual([read(a).title,read(b).title,read(c).title],['CodeRabbit','CodeRabbit2','CodeRabbit3']);
 chatUi(root,{action:'send',chat_id:b,message_id:'user',text:'Do not replace the AI name'});assert.equal(read(b).title,'CodeRabbit2');
 chatUi(root,{action:'detach',chat_id:b});chatUi(root,{action:'delete',chat_id:b});
 const d=create();assert.equal(open(d).session.title,'CodeRabbit2');assert.equal(read(c).title,'CodeRabbit3');
 assert.equal(open(a,'CodeRabbit',{attachment_id:aa.attachment_id}).session.title,'CodeRabbit');
 assert.throws(()=>open(a,'OtherAI'),/already attached/);assert.equal(read(a).title,'CodeRabbit');assert.equal(read(a).agent_name,'CodeRabbit');
 assert.throws(()=>open(a,'OtherAI',{attachment_id:bb.attachment_id}),/expired or replaced/);
 chatUi(root,{action:'rename',chat_id:c,title:'My chosen name'});chatUi(root,{action:'detach',chat_id:c});open(c,'DifferentAI');assert.equal(read(c).title,'My chosen name');
});
test('archived and custom names remain reserved; group members do not replace coordinator title',t=>{
 const {root,create,read,open}=fixture(t),reserved=create();chatUi(root,{action:'rename',chat_id:reserved,title:'CodeRabbit'});chatUi(root,{action:'archive',chat_id:reserved,archived:true});
 const group=chatUi(root,{action:'create',mode:'group'}).session.id,chief=open(group);assert.equal(chief.session.title,'CodeRabbit2');open(group,'Helper');assert.equal(read(group).title,'CodeRabbit2');assert.equal(open(group,'IgnoredOnResume',{attachment_id:chief.attachment_id}).session.title,'CodeRabbit2');
 const other=mkdtempSync(path.join(tmpdir(),'chat-metadata-other-'));t.after(()=>rmSync(other,{recursive:true,force:true}));const id=chatUi(other,{action:'create'}).session.id;assert.equal(chatTool(other,'chat_open',{chat_id:id,agent_name:'CodeRabbit'}).session.title,'CodeRabbit');
});
test('notes persist independently in list, JSON and Markdown and support clearing',t=>{
 const {root,create,read,open}=fixture(t),id=create();open(id);chatUi(root,{action:'send',chat_id:id,message_id:'u',text:'Original text'});const before=read(id);
 const updated=chatUi(root,{action:'set_note',chat_id:id,note:'  构建\n 服务  '}).session;assert.equal(updated.note,'构建 服务');assert.equal(updated.title,before.title);assert.deepEqual(updated.messages,before.messages);
 assert.equal(chatUi(root,{action:'list'}).sessions[0].note,'构建 服务');assert.equal(JSON.parse(readFileSync(path.join(root,'docs/chat-sessions',id+'.json'))).note,'构建 服务');assert.match(readFileSync(path.join(root,'docs/chat-sessions',id+'.md'),'utf8'),/备注：构建 服务/);
 for(const note of [null,5,'中'.repeat(334)])assert.throws(()=>chatUi(root,{action:'set_note',chat_id:id,note}),/1000 bytes/);
 assert.equal(read(id).note,'构建 服务');chatUi(root,{action:'rename',chat_id:id,title:'Renamed'});assert.equal(read(id).note,'构建 服务');chatUi(root,{action:'set_note',chat_id:id,note:''});assert.equal(read(id).note,'');assert.equal(read(id).title,'Renamed');
});
