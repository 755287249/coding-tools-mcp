import test from 'node:test';
import assert from 'node:assert/strict';
import {ctmcpUpdates} from '../workers/ctmcp-updates.mjs';
const digest='a'.repeat(64),asset={id:42,name:'ctmcp-9.8.7-win64.exe',size:4,digest:'sha256:'+digest};
const release={tag_name:'client-v9.8.7',body:'## Release\n\nComplete notes',assets:[asset]};
const env={CTMCP_REPO:'owner/coding-tools',REPO:'owner/brana'};
const req=path=>new Request('https://updates.example'+path);
test('same domain routes keep legacy application completely separate',async()=>{
 let calls=0;for(const path of ['/latest','/ticket','/beta/latest','/admin','/ctmcpOther/latest'])assert.equal(await ctmcpUpdates(req(path),env,()=>{calls++;throw Error()}),null);
 assert.equal(calls,0);assert.deepEqual(env,{CTMCP_REPO:'owner/coding-tools',REPO:'owner/brana'});
});
test('CTMCP metadata and pinned download use only the configured CTMCP repository',async()=>{
 const seen=[];const fetcher=async(url,init)=>{seen.push(url);assert.ok(init.headers['user-agent']);return url.endsWith('/latest')?Response.json(release):new Response('MZok',{headers:{'content-length':'4'}})};
 const r=await ctmcpUpdates(req('/ctmcp/latest'),env,fetcher),data=await r.json();assert.equal(r.status,200);assert.equal(data.appId,'coding-tools-mcp');assert.equal(data.files.portable.sha256,digest);assert.equal(data.notes,release.body);
 assert.equal((await ctmcpUpdates(req('/ctmcp/download?version=9.8.7&sha256='+digest),env,fetcher)).status,200);
 assert.ok(seen.every(url=>url.startsWith('https://api.github.com/repos/owner/coding-tools/')));
 assert.equal((await ctmcpUpdates(req('/ctmcp/download?version=9.8.6&sha256='+digest),env,fetcher)).status,409);
 assert.equal((await ctmcpUpdates(req('/ctmcp/download?version=9.8.7&sha256='+'b'.repeat(64)),env,fetcher)).status,409);
});
test('wrong products, missing hashes, oversized binaries and provider failures fail closed',async()=>{
 for(const changed of [{name:'BranaAi-Setup-9.8.7.exe'},{digest:null},{size:201*1024*1024},{size:0}])assert.equal((await ctmcpUpdates(req('/ctmcp/latest'),env,async()=>Response.json({...release,assets:[{...asset,...changed}]}))).status,502);
 assert.equal((await ctmcpUpdates(req('/ctmcp/latest'),{},()=>assert.fail())).status,503);
 assert.equal((await ctmcpUpdates(req('/ctmcp/latest'),env,async()=>new Response('',{status:404}))).status,404);
 assert.equal((await ctmcpUpdates(req('/ctmcp/latest'),env,async()=>{throw Error('private upstream failure')})).status,502);
 assert.equal((await ctmcpUpdates(new Request(req('/ctmcp/latest'),{method:'POST'}),env,()=>assert.fail())).status,405);
});

test('explicit beta channel supports existing prerelease workflow without crossing product assets',async()=>{
 const seen=[];const request=async url=>{seen.push(url);return Response.json([{...release,tag_name:'v10.0.0',prerelease:true,assets:[{...asset,name:'BranaAi-Setup-10.0.0.exe'}]},{...release,tag_name:'v99.0.0',draft:true},{...release,prerelease:true}])};
 const response=await ctmcpUpdates(req('/ctmcp/latest'),{...env,CTMCP_CHANNEL:'beta'},request);assert.equal(response.status,200);assert.equal((await response.json()).version,'9.8.7');assert.ok(seen[0].endsWith('/releases?per_page=100'));
 assert.equal((await ctmcpUpdates(req('/ctmcp/latest'),env,async()=>Response.json({...release,prerelease:true}))).status,502);
});
