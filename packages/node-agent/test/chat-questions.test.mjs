import test from 'node:test';
import assert from 'node:assert/strict';
import {mkdtempSync,rmSync,readFileSync,writeFileSync} from 'node:fs';
import {tmpdir} from 'node:os';
import path from 'node:path';
import {chatUi,chatTool} from '../dist/chat/store.js';
import {parseQuestions} from '../dist/chat/questions.js';
const questions=[{id:'scope',prompt:'范围？',options:[{id:'custom',label:'当前页',description:'只改当前页'},{id:'all',label:'全部'}]},{id:'note',prompt:'补充说明？',options:[]}];
const answers=[{question_id:'scope',option_id:'custom'},{question_id:'note',custom_text:'仅修改配色 @Other'}];
function fixture(t,grouped=false){
 const root=mkdtempSync(path.join(tmpdir(),'chat-questions-'));t.after(()=>rmSync(root,{recursive:true,force:true}));
 const {session}=chatUi(root,{action:'create'}),args={chat_id:session.id,...chatTool(root,'chat_open',{chat_id:session.id,agent_name:'Chief'})};
 if(grouped)chatUi(root,{action:'set_mode',chat_id:session.id,mode:'group'});
 const ui=(action,extra={})=>chatUi(root,{chat_id:session.id,action,...extra});
 const reply=(extra={})=>chatTool(root,'chat_reply',{chat_id:session.id,attachment_id:args.attachment_id,reply_to:'request',message_id:'card',text:'请选择 **范围**',questions,final:true,awaiting_user:true,...extra});
 const answer=(extra={})=>ui('answer_question',{question_message_id:'card',message_id:'answer',answers,...extra});
 ui('send',{message_id:'request',text:'请调整'});
 return {root,args,ui,reply,answer};
}
for(const grouped of [false,true])test(`question reply/answer persistence and queue isolation (${grouped})`,t=>{
 const {root,args,ui,reply,answer}=fixture(t,grouped);
 if(grouped)chatTool(root,'chat_open',{chat_id:args.chat_id,agent_name:'Other'});
 ui('send',{message_id:'queued',text:'稍后处理'});reply();reply();
 assert.equal(chatTool(root,'chat_wait',args).status,'idle');
 assert.equal(ui('read').session.messages.at(-1).questions_active,true);
 assert.match(readFileSync(path.join(root,ui('read').session.archive_path),'utf8'),/当前页/);
 answer();answer();answer({message_id:'retry-after-reload',answers:[...answers].reverse()});
 const saved=ui('read').session,card=saved.messages.find(m=>m.id==='card');
 assert.equal(saved.messages.filter(m=>m.question_response).length,1);
 assert.equal(card.questions_active,false);assert.equal(card.question_answer.message_id,'answer');
 const delivered=chatTool(root,'chat_wait',args).message;
 assert.deepEqual(delivered.question_response,{message_id:'card',answers});
 assert.equal(delivered.id,'answer');assert.equal(saved.queued_messages.length,1);
 if(grouped)assert.deepEqual(delivered.recipient_ids,[card.agent_id]);
 assert.throws(()=>answer({answers:[{question_id:'scope',option_id:'all'},answers[1]]}),/already answered/);
 chatTool(root,'chat_reply',{chat_id:args.chat_id,attachment_id:args.attachment_id,reply_to:'answer',message_id:'done',text:'收到',final:true});
 assert.equal(chatTool(root,'chat_wait',args).message.id,'queued');
});
test('question schema rejects ambiguity, excessive content and incompatible reply flags',t=>{
 const {reply}=fixture(t);
 for(const value of [null,[],Array(4).fill(questions[0]),[questions[0],questions[0]],[{...questions[0],options:[questions[0].options[0],questions[0].options[0]]}],[{...questions[0],extra:true}],[{...questions[0],prompt:'界'.repeat(334)}],[{...questions[0],options:Array(7).fill(questions[0].options[0])}]])assert.throws(()=>parseQuestions(value));
 assert.throws(()=>reply({final:false}),/final/);assert.throws(()=>reply({awaiting_user:false}),/awaiting_user/);
 reply();assert.throws(()=>reply({questions:[{...questions[0],prompt:'changed'}]}),/conflict/);
});
test('answers reject missing/unknown/duplicate/ambiguous values without mutation',t=>{
 const {ui,reply,answer}=fixture(t);reply();
 for(const value of [[],[answers[0],answers[0]],[{question_id:'missing',option_id:'all'},answers[1]],[{...answers[0],option_id:'missing'},answers[1]],[{...answers[0],custom_text:'also'},answers[1]],[answers[0],{question_id:'note',custom_text:' '}],[answers[0],{question_id:'note',custom_text:'界'.repeat(1334)}]])assert.throws(()=>answer({answers:value}));
 assert.equal(ui('read').session.messages.length,2);
 assert.throws(()=>answer({message_id:'request'}),/conflict/);answer();
});
for(const action of ['send','close'])test(`question becomes stale after ${action}`,t=>{
 const {ui,reply,answer}=fixture(t);reply();ui(action,{message_id:'new',text:'换个方向'});
 assert.equal(ui('read').session.messages.find(m=>m.id==='card').questions_active,false);
 assert.throws(()=>answer(),/no longer active/);
});
test('group members cannot bypass coordinator with questions',t=>{
 const {root,args,reply}=fixture(t,true),member=chatTool(root,'chat_open',{chat_id:args.chat_id,agent_name:'Other'});
 assert.throws(()=>reply({attachment_id:member.attachment_id}),/coordinator/);
});
test('MCP clients discover questions and receive structured answers through chat_wait',async t=>{
 const {createMcpFixture,mcpRequest,responseJson}=await import('./mcpTestHelpers.mjs');
 const state=await createMcpFixture(t),session=chatUi(state.root,{action:'create'}).session;let seq=0;
 const rpc=async(method,params)=>responseJson(await mcpRequest(state,{jsonrpc:'2.0',id:++seq,method,params}));
 const call=async(name,args)=>{const r=await rpc('tools/call',{name,arguments:{...args,workspace_folder_id:'repo'}});assert.equal(r.result?.structuredContent?.ok,true,JSON.stringify(r));return r.result.structuredContent;};
 const catalog=await rpc('tools/list',{}),schema=catalog.result.tools.find(t=>t.name==='chat_reply').inputSchema.properties.questions;
 assert.equal(schema.maxItems,3);assert.equal(schema.items.properties.options.maxItems,6);
 const opened=await call('chat_open',{chat_id:session.id});assert.match(opened.skill.text,/question_response/);
 const args={chat_id:session.id,attachment_id:opened.attachment_id};
 chatUi(state.root,{action:'send',chat_id:session.id,message_id:'request',text:'Choose'});
 await call('chat_reply',{...args,reply_to:'request',message_id:'card',text:'请选择 **范围**',final:true,awaiting_user:true,questions});
 chatUi(state.root,{action:'answer_question',chat_id:session.id,question_message_id:'card',message_id:'answer',answers});
 const received=await call('chat_wait',{...args,timeout_ms:0});assert.deepEqual(received.message.question_response,{message_id:'card',answers});
});

test('retries survive Rust object-key ordering in the shared archive',t=>{
 const {root,args,ui,reply,answer}=fixture(t);reply();
 const file=path.join(root,`docs/chat-sessions/${args.chat_id}.json`);
 const sort=value=>Array.isArray(value)?value.map(sort):value&&typeof value==='object'?Object.fromEntries(Object.keys(value).sort().map(k=>[k,sort(value[k])])):value;
 const reorder=()=>writeFileSync(file,JSON.stringify(sort(JSON.parse(readFileSync(file,'utf8')))));
 reorder();reply();answer();reorder();answer({message_id:'after-switch'});
 assert.equal(ui('read').session.messages.length,3);
});
