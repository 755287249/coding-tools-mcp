import test from 'node:test';
import assert from 'node:assert/strict';
import { randomId } from '../src/lib/browser-tools.ts';
import { messageBytes } from '../src/lib/chat/autogrow.ts';

test('browser IDs do not depend on secure-context randomUUID and byte limits account for Unicode', () => {
  const a=randomId(),b=randomId();assert.match(a,/^[\da-f]{8}-[\da-f]{4}-4[\da-f]{3}-[89ab][\da-f]{3}-[\da-f]{12}$/);assert.notEqual(a,b);
  assert.equal(messageBytes(' 中文😀 '),12);
  assert.ok(messageBytes('中文😀'.repeat(3201))>32000);
});

test('browser login sends no URL credential, expires on 401 and clears local state after failed logout',async()=>{
  const oldFetch=globalThis.fetch,oldStorage=globalThis.sessionStorage;
  const storage=new Map();globalThis.sessionStorage={getItem:key=>storage.get(key)??null,setItem:(key,value)=>storage.set(key,value),removeItem:key=>storage.delete(key)};
  const {browserLogin,browserInvoke,browserLogout}=await import('../src/lib/backend/browser-session.ts');
  try {
    await assert.rejects(browserInvoke('list_workspaces'),/login required/);
    globalThis.fetch=async(url,init)=>{assert.equal(url,'/browser/login');assert.equal(init.method,'POST');return Response.json({token:'synthetic-browser-session'})};
    await browserLogin('synthetic-login');
    globalThis.fetch=async(url,init)=>{assert.equal(url,'/browser/invoke');assert.equal(init.headers.authorization,'Bearer synthetic-browser-session');assert.deepEqual(JSON.parse(init.body),{command:'local_chat',args:{id:'workspace'}});return Response.json({value:{ok:true}})};
    assert.deepEqual(await browserInvoke('local_chat',{id:'workspace'}),{ok:true});
    globalThis.fetch=async()=>Response.json({error:'Expired'}, {status:401});
    await assert.rejects(browserInvoke('list_workspaces'),/Expired/);assert.equal(storage.size,0);
    await assert.rejects(browserInvoke('list_workspaces'),/login required/);
    globalThis.fetch=async()=>Response.json({token:'synthetic-browser-session'});await browserLogin('synthetic-login');
    globalThis.fetch=async()=>{throw Error('offline')};await assert.rejects(browserLogout(),/offline/);assert.equal(storage.size,0);
    await assert.rejects(browserInvoke('list_workspaces'),/login required/);
  } finally {globalThis.fetch=oldFetch;globalThis.sessionStorage=oldStorage;}
});

test('proxy HTML401 clears expired login and invalid successful login cannot authenticate',async()=>{
 const oldFetch=globalThis.fetch,oldStorage=globalThis.sessionStorage;const memory=new Map();
 globalThis.sessionStorage={getItem:k=>memory.get(k)??null,setItem:(k,v)=>memory.set(k,v),removeItem:k=>memory.delete(k)};
 const s=await import('../src/lib/backend/browser-session.ts');s.forgetBrowserSession();
 try{
  globalThis.fetch=async()=>Response.json({token:'fixture-session'});await s.browserLogin('fixture');
  globalThis.fetch=async()=>new Response('<html>proxy denied</html>',{status:401});
  await assert.rejects(s.browserInvoke('list_workspaces'));assert.equal(memory.size,0);
  globalThis.fetch=async()=>Response.json({});await assert.rejects(s.browserLogin('fixture'),/Invalid browser response/);assert.equal(memory.size,0);
 }finally{s.forgetBrowserSession();globalThis.fetch=oldFetch;globalThis.sessionStorage=oldStorage;}
});

test('late requests cannot erase a newer login or restore a session after logout',async()=>{
 const oldFetch=globalThis.fetch,oldStorage=globalThis.sessionStorage;const memory=new Map();
 globalThis.sessionStorage={getItem:k=>memory.get(k)??null,setItem:(k,v)=>memory.set(k,v),removeItem:k=>memory.delete(k)};
 const s=await import('../src/lib/backend/browser-session.ts');s.forgetBrowserSession();
 const login=async name=>{globalThis.fetch=async()=>Response.json({token:name});await s.browserLogin('fixture')};
 try{
  await login('fixture-old');let resolve;
  globalThis.fetch=()=>new Promise(r=>resolve=r);const pending=s.browserInvoke('list_workspaces');
  await login('fixture-new');resolve(Response.json({error:'Expired'},{status:401}));await assert.rejects(pending);assert.equal(memory.get('ctmcp-browser-session'),'fixture-new');
  globalThis.fetch=()=>new Promise(r=>resolve=r);const logout=s.browserLogout();assert.equal(memory.size,0);
  await login('fixture-newer');resolve(Response.json({ok:true}));await logout;assert.equal(memory.get('ctmcp-browser-session'),'fixture-newer');
  globalThis.fetch=()=>new Promise(r=>resolve=r);const stale=s.browserInvoke('list_workspaces');
  s.forgetBrowserSession();resolve(Response.json({value:['old-workspace']}));await assert.rejects(stale,/session changed/);
  globalThis.fetch=()=>new Promise(r=>resolve=r);const pendingLogin=s.browserLogin('fixture');
  s.forgetBrowserSession();resolve(Response.json({token:'fixture-too-late'}));await assert.rejects(pendingLogin,/session changed/);assert.equal(memory.size,0);
 }finally{s.forgetBrowserSession();globalThis.fetch=oldFetch;globalThis.sessionStorage=oldStorage;}
});

test('transient proxy failures retry reads once, preserve login, and never replay writes',async()=>{
 const oldFetch=globalThis.fetch;const s=await import('../src/lib/backend/browser-session.ts');s.forgetBrowserSession();
 try{
  globalThis.fetch=async()=>Response.json({token:'fixture-session'});await s.browserLogin('fixture');
  let calls=0;
  globalThis.fetch=async()=>++calls===1?new Response('<html>temporary proxy page</html>',{headers:{'content-type':'text/html'}}):Response.json({value:['recovered']});
  assert.deepEqual(await s.browserInvoke('list_workspaces'),['recovered']);assert.equal(calls,2);
  calls=0;globalThis.fetch=async()=>{calls++;return new Response('proxy',{status:502,headers:{'content-type':'text/html'}})};
  await assert.rejects(s.browserInvoke('local_chat',{args:{action:'send'}}),/HTTP 502, text\/html/);assert.equal(calls,1);
  calls=0;await assert.rejects(s.browserInvoke('local_chat',{args:{action:'read'}}),/HTTP 502/);assert.equal(calls,2);
  calls=0;globalThis.fetch=async()=>{calls++;return Response.json({error:'invalid'}, {status:400})};
  await assert.rejects(s.browserInvoke('list_workspaces'),/invalid/);assert.equal(calls,1);
  calls=0;globalThis.fetch=async()=>{calls++;return Response.json({other:'missing value'})};
  await assert.rejects(s.browserInvoke('list_workspaces'),/missing result/);assert.equal(calls,1);
 }finally{s.forgetBrowserSession();globalThis.fetch=oldFetch}
});

test('logout during retry delay cancels the retry rather than sending with another session',async()=>{
 const oldFetch=globalThis.fetch;const s=await import('../src/lib/backend/browser-session.ts');s.forgetBrowserSession();
 try{
  globalThis.fetch=async()=>Response.json({token:'fixture-session'});await s.browserLogin('fixture');let calls=0;
  globalThis.fetch=async()=>{calls++;return new Response('proxy',{status:503})};
  const pending=s.browserInvoke('list_workspaces');await new Promise(r=>setTimeout(r,40));s.forgetBrowserSession();
  await assert.rejects(pending,/session changed/);assert.equal(calls,1);
 }finally{s.forgetBrowserSession();globalThis.fetch=oldFetch}
});
