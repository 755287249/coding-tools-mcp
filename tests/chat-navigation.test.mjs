import assert from 'node:assert/strict';
import test from 'node:test';
import { readFileSync } from 'node:fs';
import ts from 'typescript';
const compile = source => ts.transpileModule(source, {compilerOptions:{module:ts.ModuleKind.ESNext,target:ts.ScriptTarget.ES2022}}).outputText;
const moduleUrl = source => 'data:text/javascript;base64,' + Buffer.from(compile(source)).toString('base64');
const statusUrl = moduleUrl(readFileSync(new URL('../src/lib/chat/status.ts', import.meta.url), 'utf8'));
const source = readFileSync(new URL('../src/lib/chat/navigation.ts', import.meta.url), 'utf8').replace("'./status'", JSON.stringify(statusUrl));
const { sessionPresence, messageOutline, isConnectedConversation } = await import(moduleUrl(source));

test('polling transport transitions keep online presence stable; work and interruption are distinct', () => {
  const session = {closed:false, status:'waiting', work_state:null};
  assert.equal(sessionPresence(session), 'online');
  session.status = 'connected';
  assert.equal(sessionPresence(session), 'online');
  session.work_state = 'processing';
  assert.equal(sessionPresence(session), 'working');
  session.status = 'offline';
  assert.equal(sessionPresence(session), 'error');
  session.work_state = 'queued';
  assert.equal(sessionPresence(session), 'offline');
  session.closed = true;
  assert.equal(sessionPresence(session), 'closed');
});

test('selected details agree with summary work state and final acknowledgement settles presence', () => {
  const session = {closed:false, status:'connected', messages:[{id:'u1',role:'user',text:'task',received_at:1}]};
  assert.equal(sessionPresence(session), 'working');
  session.messages.push({id:'a1',role:'assistant',reply_to:'u1',final:true,text:'done'});
  assert.equal(sessionPresence(session), 'online');
});

test('outline preserves request order, attachment-only messages, bounded latest prose, and excludes tool noise', () => {
  const messages = [
    {id:'u1',role:'user',text:'First\n task'},
    {id:'a1',role:'assistant',reply_to:'u1',text:'progress'},
    {id:'a2',role:'assistant',reply_to:'u1',text:'tool output',tool_event:{name:'bash',status:'completed'}},
    {id:'u2',role:'user',text:'',attachments:[{name:'photo.png'}]},
    {id:'a3',role:'assistant',reply_to:'u1',text:'final '.repeat(100)},
  ];
  const outline = messageOutline(messages);
  assert.deepEqual(outline.map(({id,title}) => ({id,title})), [{id:'u1',title:'First task'},{id:'u2',title:'photo.png'}]);
  assert.equal(outline[0].preview.length, 180);
  assert.ok(outline[0].preview.startsWith('final'));
  assert.equal(outline[1].preview, '');
});

test('conversation URLs preserve explicit workspace, folder and chat IDs and keep new drafts separate', async () => {
  const { chatLocation } = await import('../src/lib/chat/location.ts');
  const url = new URL(chatLocation('work space/1', 'folder&two', 'chat?three'), 'http://local');
  assert.equal(decodeURIComponent(url.pathname.slice('/workspace/'.length)), 'work space/1');
  assert.equal(url.searchParams.get('folder'), 'folder&two');
  assert.equal(url.searchParams.get('chat'), 'chat?three');
  assert.equal(url.searchParams.has('new'), false);
  const fresh = new URL(chatLocation('w2', 'f2'), 'http://local');
  assert.equal(fresh.searchParams.get('new'), '1');
  assert.equal(fresh.searchParams.has('chat'), false);
});

test('navigation preference tolerates invalid storage and bounds resizing', async () => {
  const { navigationWidth } = await import('../src/lib/chat/location.ts');
  assert.equal(navigationWidth(null), 280);
  assert.equal(navigationWidth('bad'), 280);
  assert.equal(navigationWidth(1000), 420);
  assert.equal(navigationWidth(20), 220);
  assert.equal(navigationWidth('340'), 340);
});

 test('compact conversation switcher includes live busy and idle AI, excludes empty, detached and archived chats',()=>{
  for(const status of ['connected','waiting'])for(const work_state of [null,'processing','queued'])assert.equal(isConnectedConversation({status,work_state,closed:false}),true);
  for(const session of [{status:'offline'},{status:'offline',work_state:'processing'},{status:'closed'},{status:'connected',closed:true},{status:'waiting',archived:true}])assert.equal(isConnectedConversation(session),false);
});

test('live conversations first; recent uses only actual messages and ignores pins/metadata',async()=>{
 const {conversationOrder,recentConversationOrder}=await import(moduleUrl(source));
 const old={id:'a',created_at:1,updated_at:9999,last_message_at:20,pinned:true,status:'offline'};
 const live={id:'b',created_at:2,updated_at:2,last_message_at:10,status:'waiting'};
 const fresh={id:'c',created_at:3,updated_at:3,last_message_at:30,status:'offline'};
 assert.deepEqual([old,fresh,live].sort(conversationOrder).map(s=>s.id),['b','a','c']);
 assert.deepEqual([old,live,fresh].sort(recentConversationOrder).map(s=>s.id),['c','a','b']);
 assert.equal(sessionPresence({...live,work_state:'queued'}),'queued');
 assert.equal(sessionPresence({...live,awaiting_user:true}),'awaiting_user');
});
test('mobile swipe direction requires distance, horizontal intent and a short gesture',async()=>{
 const {swipeDirection}=await import('../src/lib/chat/conversation-swipe.ts');
 assert.equal(swipeDirection(-100,15,300),1);assert.equal(swipeDirection(100,-15,300),-1);
 for(const args of [[40,0,300],[100,80,300],[100,0,900]])assert.equal(swipeDirection(...args),null);
});
