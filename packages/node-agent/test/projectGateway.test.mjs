import {createHash} from 'node:crypto';
import test from 'node:test';
import assert from 'node:assert/strict';
import {mkdtemp,mkdir,writeFile,rm} from 'node:fs/promises';
import path from 'node:path';
import os from 'node:os';
import {normalizeConfig} from '../dist/config.js';
import {createAgentRuntime} from '../dist/server.js';
import {ProjectGateway,projectCatalog} from '../dist/server/projectGateway.js';
import {callTool} from '../dist/tools.js';
import {chatUi} from '../dist/chat/store.js';
import {ApplicationConfigStore,loadApplication} from '../dist/application.js';
import {GLOBAL_MCP_ID} from '../dist/projectIsolation.js';
async function fixture(t) {
 const base=await mkdtemp(path.join(os.tmpdir(),'global-projects-')),runtimes=[];
 t.after(async()=>{for(const r of runtimes)await r.close();await rm(base,{recursive:true,force:true})});
 return async(id,global=false,options={})=>{
  const root=path.join(base,id);await mkdir(root,{recursive:true});
  const config=normalizeConfig({schema_version:1,globalConnection:global,dataDir:path.join(base,'state',id),folders:[{id,name:id,path:root}],toolProfile:'advanced',permissionMode:'trusted',tunnel:{enabled:false},oauth:{clientId:'test-client'}},{oauthPassword:'test-password',oauthTokenSecret:'test-token-secret'},{});
  config.workspaceId=global?GLOBAL_MCP_ID:id;
  const runtime=await createAgentRuntime(config,{projectIsolation:!global,...options});runtimes.push(runtime);return runtime;
 };
}
test('capability scopes tools/resources, rejects foreign roots and survives terminal wait',async t=>{
 const create=await fixture(t),control=await create('control',true),a=await create('a'),b=await create('b');
 const gateway=new ProjectGateway(()=>[a.context,b.context]);
 const chat=chatUi(a.context.config.folders[0].path,{action:'create'}).session.id;
 const request=(name,args)=>({jsonrpc:'2.0',id:1,method:'tools/call',params:{name,arguments:args,_meta:{'openai/session':'forged'}}});
 await assert.rejects(gateway.select(control.context,request('read_file',{path:'a.txt',workspace_folder_id:'a'})),/Open a chat/);
 const opening=await gateway.select(control.context,request('chat_open',{chat_id:chat,workspace_folder_id:'a'}));
 const opened=await callTool(opening.context,'chat_open',opening.request.params.arguments,opening.request.params._meta);
 assert.equal(opened.ok,true);const args={chat_id:chat,workspace_folder_id:'a',attachment_id:opened.attachment_id};
 const selected=await gateway.select(control.context,request('read_file',{...args,path:'a.txt'}));
 assert.equal(selected.context,a.context);assert.equal(selected.request.params._meta['openai/session'],`project-chat:${args.attachment_id}`);
 assert.equal(selected.request.params.arguments.attachment_id,undefined);
 await assert.rejects(gateway.select(control.context,request('read_file',{...args,workspace_folder_id:'b',path:'a.txt'})));
 await assert.rejects(gateway.select(control.context,request('git_status',args)),/sandbox/);
 await assert.rejects(gateway.select(control.context,{method:'resources/list',params:{_meta:{...args,attachment_id:'forged'}}}));
 assert.ok(!projectCatalog(a.context).names.includes('git_status'));
 const result=await callTool(a.context,'read_file',{path:path.join(b.context.config.folders[0].path,'private'),workspace_folder_id:'a'},selected.request.params._meta);assert.equal(result.ok,false);
 await callTool(a.context,'chat_close',args,selected.request.params._meta);
 assert.equal((await gateway.select(control.context,request('chat_wait',{...args,timeout_ms:1}))).context,a.context);
 await assert.rejects(gateway.select(control.context,request('read_file',{...args,path:'a.txt'})),/closed/);
 gateway.stop();await assert.rejects(gateway.select(control.context,{method:'tools/list'}),/stopped/);
});
test('empty global connection exposes no control directory',async t=>{
 const create=await fixture(t),control=await create('control',true),gateway=new ProjectGateway(()=>[]);
 const list=await gateway.select(control.context,{method:'tools/call',params:{name:'list_workspace_folders',arguments:{}}});assert.deepEqual(list.result.structuredContent.folders,[]);
 await assert.rejects(gateway.select(control.context,{method:'tools/call',params:{name:'read_file',arguments:{path:'secret'}}}));
});
test('migration preserves identities/secrets, reload and last-project removal; global save accepts zero folders',async t=>{
 const base=await mkdtemp(path.join(os.tmpdir(),'global-migration-'));t.after(()=>rm(base,{recursive:true,force:true}));
 const rootA=path.join(base,'a'),rootB=path.join(base,'b');await mkdir(rootA);await mkdir(rootB);
 const file=path.join(base,'agent.json');await writeFile(file,JSON.stringify({schema_version:1,dataDir:path.join(base,'state'),folders:[{id:'a',path:rootA},{id:'b',path:rootB}],oauth:{clientId:'stable-client',password:'synthetic-password',tokenSecret:'synthetic-token'},tunnel:{enabled:false}}));
 const app=await loadApplication(file);assert.equal(app.connection.id,GLOBAL_MCP_ID);assert.equal(app.connection.loaded.config.folders.length,0);assert.equal(app.connection.loaded.config.oauth.password,'synthetic-password');assert.equal(app.workspaces.length,2);assert.deepEqual(app.workspaces.map(w=>w.loaded.config.folders[0].id),['a','b']);
 const restored=await loadApplication(file);assert.deepEqual(restored.workspaces.map(w=>w.id),app.workspaces.map(w=>w.id));
 const store=new ApplicationConfigStore(restored);await assert.rejects(store.saveWorkspace(app.workspaces[0].id,{folders:[{id:'a',path:rootB}]}),/fixed directory/);
 for(const workspace of app.workspaces)await store.deleteWorkspace(workspace.id);
 const empty=await loadApplication(file);assert.equal(empty.workspaces.length,0);assert.equal(empty.connection.id,GLOBAL_MCP_ID);
 assert.equal((await new ApplicationConfigStore(empty).saveWorkspace(GLOBAL_MCP_ID,{name:'MCP',folders:[]})).ok,true);
});
test('authenticated HTTP gateway routes project tools and revokes removed project records',async t=>{
 const create=await fixture(t),registry=new Map();const a=await create('a',false,{runtimeRegistry:registry}),control=await create('control',true,{runtimeRegistry:registry});
 await writeFile(path.join(a.context.config.folders[0].path,'hello.txt'),'project A');
 await new Promise(resolve=>control.server.listen(0,'127.0.0.1',resolve));const base=`http://127.0.0.1:${control.server.address().port}`;
 const verifier='abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789-._~',redirect='http://127.0.0.1:8765/callback';
 const auth=await fetch(`${base}/oauth/authorize`,{method:'POST',redirect:'manual',body:new URLSearchParams({client_id:'test-client',redirect_uri:redirect,code_challenge:createHash('sha256').update(verifier).digest('base64url'),code_challenge_method:'S256',state:'test-state',password:'test-password'})});
 assert.equal(auth.status,303);const callback=new URL(auth.headers.get('location'));assert.equal(callback.searchParams.get('state'),'test-state');
 const token=await fetch(`${base}/oauth/token`,{method:'POST',body:new URLSearchParams({grant_type:'authorization_code',code:callback.searchParams.get('code'),redirect_uri:redirect,code_verifier:verifier,client_id:'test-client'})}).then(r=>r.json());assert.ok(token.access_token);
 const rpc=async(method,params={})=>{const r=await fetch(`${base}/mcp`,{method:'POST',headers:{authorization:`Bearer ${token.access_token}`,'content-type':'application/json'},body:JSON.stringify({jsonrpc:'2.0',id:1,method,params})});assert.equal(r.status,200);return r.json()};
 assert.equal((await fetch(`${base}/mcp`,{method:'POST',body:'{}'})).status,401);
 assert.match((await rpc('initialize',{protocolVersion:'2025-03-26'})).result.instructions,/global MCP/);
 assert.equal((await rpc('tools/call',{name:'list_workspace_folders',arguments:{}})).result.structuredContent.folders.length,1);
 const chat=chatUi(a.context.config.folders[0].path,{action:'create'}).session.id;
 const opened=(await rpc('tools/call',{name:'chat_open',arguments:{chat_id:chat,workspace_folder_id:'a'}})).result.structuredContent;
 const scope={chat_id:chat,workspace_folder_id:'a',attachment_id:opened.attachment_id};
 assert.equal((await rpc('tools/call',{name:'read_file',arguments:{...scope,path:'hello.txt'},_meta:{'openai/session':'spoof'}})).result.structuredContent.ok,true);
 assert.ok((await rpc('resources/list',{_meta:scope})).result);
 registry.delete('a');assert.equal((await rpc('tools/call',{name:'read_file',arguments:{...scope,path:'hello.txt'}})).error.code,-32003);
});

test('first launch without projects remains empty across restart',async t=>{
 const base=await mkdtemp(path.join(os.tmpdir(),'global-empty-'));t.after(()=>rm(base,{recursive:true,force:true}));
 const file=path.join(base,'agent.json');
 const first=await loadApplication(file);assert.equal(first.workspaces.length,0);assert.equal(first.connection.loaded.config.folders.length,0);
 const second=await loadApplication(file);assert.equal(second.workspaces.length,0);assert.equal(second.connection.id,GLOBAL_MCP_ID);
});
test('stopped isolated contexts reject retained tool calls',async t=>{
 const create=await fixture(t),project=await create('project');await project.close();
 const result=await callTool(project.context,'list_files',{},{});assert.equal(result.error.code,'PROJECT_STOPPED');
});

test('global bootstrap includes chat_open with a read-only donor',async t=>{
 const create=await fixture(t),control=await create('control',true);
 control.context.config.toolProfile='read-only';
 assert.ok(projectCatalog(control.context).names.includes('chat_open'));
});
test('stop while directory resolution is pending denies admission',async t=>{
 const create=await fixture(t),a=await create('a');const gateway=new ProjectGateway(()=>[a.context]);
 const pending=gateway.contextFor('a');gateway.stop();await assert.rejects(pending,/stopped/);
});

test('failed virtual runtime startup rolls back project registration',async t=>{
 const base=await mkdtemp(path.join(os.tmpdir(),'global-add-failure-'));t.after(()=>rm(base,{recursive:true,force:true}));
 const file=path.join(base,'agent.json'),app=await loadApplication(file),stopped=[];
 const store=new ApplicationConfigStore(app,async id=>{stopped.push(id)},async()=>{throw new Error('synthetic startup failure')});
 await assert.rejects(store.addWorkspace(base,'Synthetic'),/synthetic startup failure/);
 assert.equal(stopped.length,1);assert.equal(store.snapshot().workspaces.length,0);
 assert.equal((await loadApplication(file)).workspaces.length,0);
});

test('global seed endpoint retains its project capability and isolated catalog',async t=>{
 const create=await fixture(t),registry=new Map(),a=await create('a',false,{runtimeRegistry:registry});await create('b',false,{runtimeRegistry:registry});const control=await create('control',true,{runtimeRegistry:registry});
 const root=a.context.config.folders[0].path;
 chatUi(root,{action:'seed_settings',enabled:true});
 const seed=chatUi(root,{action:'seed_manual',account:'Synthetic account'}).batch[0];
 await new Promise(resolve=>control.server.listen(0,'127.0.0.1',resolve));
 const endpoint=`http://127.0.0.1:${control.server.address().port}/mcp/seeds/a/${seed.url_key}`;
 const rpc=async(method,params={},url=endpoint)=>fetch(url,{method:'POST',headers:{'content-type':'application/json','MCP-Protocol-Version':'2025-03-26'},body:JSON.stringify({jsonrpc:'2.0',id:1,method,params})}).then(r=>r.json());
 assert.ok((await rpc('initialize')).result);
 assert.ok((await rpc('initialize',{},endpoint.replace('/seeds/a/','/seeds/b/'))).error);
 const catalog=(await rpc('tools/list')).result.tools;
 assert.ok(catalog.some(t=>t.name==='seed_connect'));assert.ok(!catalog.some(t=>['git_status','format_files','switch_workspace_folder'].includes(t.name)));
 const ready=(await rpc('tools/call',{name:'seed_connect',arguments:{}})).result.structuredContent;
 assert.equal(ready.workspace_folder_id,'a');
 registry.delete('a');assert.ok((await rpc('tools/list')).error);
});

test('new projects receive distinct directory identities; ambiguous legacy identities are denied',async t=>{
 const base=await mkdtemp(path.join(os.tmpdir(),'global-unique-'));t.after(()=>rm(base,{recursive:true,force:true}));
 const app=await loadApplication(path.join(base,'agent.json')),store=new ApplicationConfigStore(app);
 const a=path.join(base,'a'),b=path.join(base,'b');await mkdir(a);await mkdir(b);
 await store.addWorkspace(a);await store.addWorkspace(b);
 const ids=store.snapshot().workspaces.map(w=>w.effective.folders[0].id);assert.equal(new Set(ids).size,2);
 await assert.rejects(store.importWorkspacePack(store.exportWorkspacePack(store.snapshot().workspaces[0].id)),/directory ID already exists/);
 const create=await fixture(t),one=await create('one'),two=await create('two');two.context.config.folders[0].id='one';
 await assert.rejects(new ProjectGateway(()=>[one.context,two.context]).contextFor('one'),/ambiguous/);
});
