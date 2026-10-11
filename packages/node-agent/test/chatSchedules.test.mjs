import test from 'node:test';
import assert from 'node:assert/strict';
import {mkdtempSync,rmSync,readFileSync,writeFileSync} from 'node:fs';
import path from 'node:path';
import os from 'node:os';
import {chatUi,runSchedules} from '../dist/chat/store.js';
test('durable schedules survive restart, dedupe import/recovery and retain deletion tombstones',()=>{
 const root=mkdtempSync(path.join(os.tmpdir(),'schedules-'));
 try {
  const chat=(chatUi(root,{action:'create'}).session).id;
  const req={action:'schedule_create',id:'task-one',chatId:chat,title:'task',text:'Run once',due:1};
  chatUi(root,req);runSchedules(root);runSchedules(root);
  const read=()=>chatUi(root,{action:'read',chat_id:chat}).session;
  assert.equal(read().messages.length,1);
  chatUi(root,{...req,action:'schedule_import',status:'pending'});runSchedules(root);assert.equal(read().messages.length,1);
  assert.throws(()=>chatUi(root,{action:'schedule_update',id:req.id,expected_status:'pending',status:'paused'}),/changed/);
  chatUi(root,{action:'schedule_update',id:req.id,expected_status:'sent',status:'deleted'});
  chatUi(root,{...req,action:'schedule_import',status:'pending'});assert.deepEqual(chatUi(root,{action:'schedule_list'}).tasks,[]);
  chatUi(root,{...req,id:'task-two'});
  chatUi(root,{action:'schedule_update',id:'task-two',expected_status:'pending',status:'paused'});runSchedules(root);assert.equal(read().messages.length,1);
  chatUi(root,{action:'schedule_update',id:'task-two',expected_status:'paused',status:'pending'});
  chatUi(root,{action:'send',chat_id:chat,message_id:'task-two',text:req.text});
  const file=path.join(root,'docs/chat-sessions/.schedules.json'),store=JSON.parse(readFileSync(file,'utf8'));store.tasks[1].status='sending';writeFileSync(file,JSON.stringify(store));
  runSchedules(root);assert.equal(read().messages.length+(read().queued_messages?.length??0),2);
 }finally{rmSync(root,{recursive:true,force:true})}
});
test('schedule folder scope, immutable payload and closed chat errors',()=>{
 const root=mkdtempSync(path.join(os.tmpdir(),'schedules-')),other=mkdtempSync(path.join(os.tmpdir(),'schedules-'));
 try {
  const chat=(chatUi(root,{action:'create'}).session).id;
  const req={action:'schedule_create',id:'task-one',chatId:chat,title:'task',text:'Run once',due:1};
  assert.throws(()=>chatUi(other,req));chatUi(root,req);assert.throws(()=>chatUi(root,{...req,text:'changed'}),/conflict/);
  chatUi(root,{action:'close',chat_id:chat});runSchedules(root);
  assert.equal((chatUi(root,{action:'schedule_list'}).tasks)[0].status,'error');
 }finally{rmSync(root,{recursive:true,force:true});rmSync(other,{recursive:true,force:true})}
});
test('historical sent tasks migrate after chat removal without sending again',()=>{
 const root=mkdtempSync(path.join(os.tmpdir(),'schedules-history-'));
 try {
  const req={action:'schedule_import',id:'sent-task',chatId:'removed-chat',title:'History',text:'Already sent',due:1,status:'sent'};
  chatUi(root,req);runSchedules(root);assert.equal(chatUi(root,{action:'schedule_list'}).tasks[0].status,'sent');
 }finally{rmSync(root,{recursive:true,force:true})}
});
test('independent workers deduplicate the same persistent due task',async()=>{
 const {spawn}=await import('node:child_process');
 const root=mkdtempSync(path.join(os.tmpdir(),'schedules-workers-'));
 try {
  const chat=chatUi(root,{action:'create'}).session.id;
  chatUi(root,{action:'schedule_create',id:'shared-job',chatId:chat,title:'Task',text:'Deliver exactly once',due:1});
  const store=new URL('../dist/chat/store.js',import.meta.url).href;
  const run=()=>new Promise((resolve,reject)=>{const child=spawn(process.execPath,['--input-type=module','-e',`import {runSchedules} from ${JSON.stringify(store)};runSchedules(process.argv[1]);`,root],{stdio:'pipe'});let error='';child.stderr.on('data',d=>error+=d);child.on('error',reject);child.on('exit',code=>code===0?resolve():reject(Error(error)))});
  await Promise.all([run(),run(),run()]);
  const session=chatUi(root,{action:'read',chat_id:chat}).session;assert.equal(session.messages.length+(session.queued_messages?.length??0),1);
 }finally{rmSync(root,{recursive:true,force:true})}
});
test('runtime delivers while every browser page is closed',async()=>{
 const {normalizeConfig}=await import('../dist/config.js');const {createAgentRuntime}=await import('../dist/server.js');
 const root=mkdtempSync(path.join(os.tmpdir(),'schedules-runtime-'));let runtime;
 try {
  const chat=chatUi(root,{action:'create'}).session.id;
  chatUi(root,{action:'schedule_create',id:'background-job',chatId:chat,title:'Task',text:'Deliver in background',due:1});
  const config=normalizeConfig({schema_version:1,dataDir:path.join(root,'.runtime'),folders:[{id:'folder',path:root}],tunnel:{enabled:false}},{oauthPassword:'synthetic-password',oauthTokenSecret:'synthetic-token'},{});
  runtime=await createAgentRuntime(config);
  const deadline=Date.now()+4000;while(Date.now()<deadline&&chatUi(root,{action:'schedule_list'}).tasks[0].status!=='sent')await new Promise(r=>setTimeout(r,50));
  assert.equal(chatUi(root,{action:'schedule_list'}).tasks[0].status,'sent');assert.equal(chatUi(root,{action:'read',chat_id:chat}).session.messages.length,1);
 }finally{await runtime?.close();rmSync(root,{recursive:true,force:true})}
});
